use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde_json::Value;
use ssx_formats::big::BigArchive;
use ssx_formats::xdvdfs::{Disc, Extent, Layout};
use ssx_formats::xex::Xex;

#[cfg(feature = "pipeline")]
pub use super::extract::run;

pub const ARCHIVES: [&str; 14] = [
    "data0",
    "bootcache",
    "memresident",
    "cache0",
    "cache1",
    "cache3",
    "cache4",
    "cache5",
    "cache6",
    "cache7",
    "cache8",
    "audiobestream",
    "feaudiocache",
    "fecache",
];

pub const SSX_PE_NAME: &str = "ssxvzf.exe";
pub const MARKER: &str = "setup.json";
pub const MARKER_FORMAT: u64 = 2;
pub const MOVIE_DIR: &str = "data/common/fmv";

pub fn big_root(out: &Path) -> PathBuf {
    out.join("big")
}

pub fn disc_root(out: &Path) -> PathBuf {
    out.join("disc")
}

pub fn app_dir() -> PathBuf {
    let env = |k: &str| std::env::var_os(k).filter(|v| !v.is_empty()).map(PathBuf::from);
    if let Some(home) = env("PROJECT_AVALANCHE_HOME") {
        return home;
    }
    if cfg!(windows) {
        let base = env("LOCALAPPDATA").or_else(|| env("USERPROFILE").map(|h| h.join("AppData").join("Local")));
        return base.unwrap_or_else(|| PathBuf::from(".")).join("Project Avalanche");
    }
    if cfg!(target_os = "macos") {
        let home = env("HOME").unwrap_or_else(|| PathBuf::from("."));
        return home.join("Library").join("Application Support").join("Project Avalanche");
    }
    let base = env("XDG_DATA_HOME").or_else(|| env("HOME").map(|h| h.join(".local").join("share")));
    base.unwrap_or_else(|| PathBuf::from(".")).join("project-avalanche")
}

pub fn default_data_dir() -> PathBuf {
    app_dir().join("GameData")
}

#[derive(Debug)]
pub struct Cancelled;

impl std::fmt::Display for Cancelled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("setup cancelled")
    }
}

impl std::error::Error for Cancelled {}

#[derive(Clone, Debug)]
pub struct DiscCheck {
    pub layout: Layout,
    pub image_size: u64,
    pub files: usize,
    pub pe_name: String,
    pub archives: Vec<(String, u64)>,
    pub movies: Vec<(String, u64)>,
    pub fingerprint: String,
    pub xex_header: Option<Vec<u8>>,
}

impl DiscCheck {
    pub fn archive_bytes(&self) -> u64 {
        self.archives.iter().map(|(_, n)| n).sum()
    }

    pub fn movie_bytes(&self) -> u64 {
        self.movies.iter().map(|(_, n)| n).sum()
    }
}

#[derive(Clone, Debug, Default)]
pub struct Progress {
    pub archive: String,
    pub archive_index: usize,
    pub archive_count: usize,
    pub files_done: usize,
    pub files_total: usize,
    pub bytes_done: u64,
    pub bytes_total: u64,
    pub failed: usize,
}

#[derive(Clone, Debug, Default)]
pub struct Summary {
    pub extracted: Vec<String>,
    pub already_done: Vec<String>,
    pub members_written: usize,
    pub members_kept: usize,
    pub members_replaced: usize,
    pub bytes_written: u64,
    pub failures: Vec<String>,
    pub movies_written: usize,
    pub movies_kept: usize,
    pub movies_replaced: usize,
    pub content_files: usize,
}

struct Fnv(u64);

impl Fnv {
    fn new() -> Self {
        Fnv(0xCBF2_9CE4_8422_2325)
    }

    fn update(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.0 = (self.0 ^ b as u64).wrapping_mul(0x0000_0100_0000_01B3);
        }
    }
}

pub(super) fn open_disc(iso: &Path) -> Result<Disc<File>> {
    let file = File::open(iso).with_context(|| format!("opening {}", iso.display()))?;
    Disc::open(file).with_context(|| format!("{} is not an Xbox / Xbox 360 disc image (XDVDFS)", iso.display()))
}

pub fn inspect(iso: &Path) -> Result<DiscCheck> {
    check_disc(&mut open_disc(iso)?)
}

pub(super) fn check_disc(disc: &mut Disc<File>) -> Result<DiscCheck> {
    let xex_extent = disc.find("default.xex").context("the image has no default.xex — not a game disc")?.extent();
    let xex = disc.read_extent(xex_extent)?;
    let pe_name =
        Xex::parse(&xex).context("default.xex is not a XEX2 executable")?.original_pe_name()?.unwrap_or_default();
    if !pe_name.eq_ignore_ascii_case(SSX_PE_NAME) {
        bail!("this is not SSX (2012): default.xex was built as {pe_name:?}, expected {SSX_PE_NAME:?}");
    }
    let mut archives = Vec::new();
    for name in ARCHIVES {
        let extent =
            disc.find(&format!("{name}.big")).with_context(|| format!("the image has no {name}.big"))?.extent();
        let mut magic = [0u8; 2];
        disc.reader(extent)?.read_exact(&mut magic).with_context(|| format!("reading {name}.big"))?;
        if &magic != b"EB" {
            bail!("{name}.big is not an EB archive");
        }
        archives.push((name.to_string(), extent.size as u64));
    }
    let movies = movie_files(disc).iter().map(|(p, e)| (p.clone(), e.size as u64)).collect::<Vec<_>>();
    if movies.is_empty() {
        bail!("the image has no movies in {MOVIE_DIR} (*.vp6)");
    }
    let mut h = Fnv::new();
    h.update(&disc.image_size.to_le_bytes());
    h.update(&disc.volume.root_sector.to_le_bytes());
    h.update(&disc.volume.root_size.to_le_bytes());
    h.update(&disc.volume.filetime.to_le_bytes());
    h.update(&disc.volume.unk_reserved);
    for n in &disc.nodes {
        h.update(n.path.as_bytes());
        h.update(&n.entry.extent.sector.to_le_bytes());
        h.update(&n.entry.extent.size.to_le_bytes());
        h.update(&[n.entry.attributes]);
    }
    h.update(&xex[..xex.len().min(0x10000)]);
    let xex_header = xex.get(..XEX_HEADER_BYTES).map(<[u8]>::to_vec);
    Ok(DiscCheck {
        layout: disc.layout,
        image_size: disc.image_size,
        files: disc.files().count(),
        pe_name,
        archives,
        movies,
        fingerprint: format!("{:016x}", h.0),
        xex_header,
    })
}

pub const XEX_HEADER_BYTES: usize = crate::content::title_update::MAX_RETAIL_HEADER_BYTES;
pub const MARKER_XEX_HEADER: &str = "xex_header";

pub fn xex_header_hex(header: &[u8]) -> String {
    use std::fmt::Write;
    header.iter().fold(String::with_capacity(header.len() * 2), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

pub fn marker_xex_header(marker: &Value) -> Option<Vec<u8>> {
    let text = marker.get(MARKER_XEX_HEADER)?.as_str()?.as_bytes();
    if text.len() != XEX_HEADER_BYTES * 2 {
        return None;
    }
    let digit = |c: u8| match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        _ => None,
    };
    text.as_chunks::<2>().0.iter().map(|&[hi, lo]| Some(digit(hi)? << 4 | digit(lo)?)).collect()
}

pub(super) fn movie_files(disc: &Disc<File>) -> Vec<(String, Extent)> {
    let dir = format!("{MOVIE_DIR}/");
    disc.files()
        .filter(|n| {
            let p = n.path.to_ascii_lowercase();
            p.strip_prefix(&dir).is_some_and(|name| !name.contains('/') && name.ends_with(".vp6"))
        })
        .map(|n| (n.path.clone(), n.extent()))
        .collect()
}

const MAX_MARKER_BYTES: u64 = 64 * 1024;

pub fn read_marker(out: &Path) -> Option<Value> {
    marker(out).ok().flatten()
}

pub fn marker(out: &Path) -> Result<Option<Value>, String> {
    let path = out.join(MARKER);
    let shown = || path.display().to_string();
    let file = match ssx_files::open_plain(&path) {
        Ok(Some(file)) => file,
        Ok(None) => return Err(format!("{}: a link or a folder, not the setup's marker", shown())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.to_string()),
    };
    let mut text = String::new();
    file.take(MAX_MARKER_BYTES + 1).read_to_string(&mut text).map_err(|e| format!("{}: {e}", shown()))?;
    if text.len() as u64 > MAX_MARKER_BYTES {
        return Err(format!("{}: larger than {MAX_MARKER_BYTES} bytes", shown()));
    }
    match serde_json::from_str::<Value>(&text) {
        Ok(value) if value.is_object() => Ok(Some(value)),
        Ok(_) => Err(format!("{}: not a JSON object", shown())),
        Err(e) => Err(format!("{}: {e}", shown())),
    }
}

#[cfg(feature = "pipeline")]
pub(super) fn write_marker(out: &Path, marker: &Value) -> Result<()> {
    Ok(ssx_files::replace(
        &out.join(MARKER),
        serde_json::to_string_pretty(marker)?.as_bytes(),
        &ssx_files::Options::new(),
    )?)
}

pub(super) fn safe_relative(path: &str) -> Option<PathBuf> {
    let mut out = PathBuf::new();
    for part in path.split('/') {
        let bad = |c: char| c.is_control() || "\\:*?\"<>|".contains(c);
        if part.is_empty() || part == "." || part == ".." || part.chars().any(bad) || !portable_name(part) {
            return None;
        }
        out.push(part);
    }
    Some(out)
}

pub(super) fn portable_name(name: &str) -> bool {
    const DEVICES: [&str; 6] = ["con", "prn", "aux", "nul", "conin$", "conout$"];
    let stem = name.split('.').next().unwrap_or(name).trim_end_matches(' ').to_ascii_lowercase();
    let numbered = (stem.starts_with("com") || stem.starts_with("lpt"))
        && stem.len() == 4
        && stem.as_bytes().get(3).is_some_and(u8::is_ascii_digit);
    !(DEVICES.contains(&stem.as_str()) || numbered || name.ends_with('.') || name.ends_with(' '))
}

pub(super) fn read_index(disc: &mut Disc<File>, name: &str, extent: Extent) -> Result<BigArchive> {
    let mut r = disc.reader(extent)?;
    let mut head = [0u8; 0x30];
    r.read_exact(&mut head)?;
    let size = BigArchive::table_size(&head)?;
    if size as u64 > extent.size as u64 {
        bail!("{name}.big: tables ({size} bytes) are larger than the archive");
    }
    let mut table = vec![0u8; size];
    r.seek(SeekFrom::Start(0))?;
    r.read_exact(&mut table)?;
    BigArchive::parse(&table).with_context(|| format!("parsing {name}.big"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn member_paths_stay_inside_the_output() {
        assert_eq!(safe_relative("data/xenon/a.bin"), Some(Path::new("data").join("xenon").join("a.bin")));
        assert!(safe_relative("data/common/fe/views/liveeventresults_i68.dds.$$$").is_some(), "a name of the disc");
        for bad in ["../x", "a/../../x", "a//b", "/abs", "c:x", "a\\b", "a/./b", "nul\0", "x?", "a/con.txt", "a/b."] {
            assert_eq!(safe_relative(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn fingerprint_hash_is_fnv1a_64() {
        let mut h = Fnv::new();
        h.update(b"a");
        assert_eq!(h.0, 0xAF63_DC4C_8601_EC8C);
    }

    #[test]
    fn the_markers_xex_header_is_read_strictly() {
        let header: Vec<u8> = (0..XEX_HEADER_BYTES).map(|i| (i * 7 % 251) as u8).collect();
        let hex = xex_header_hex(&header);
        assert_eq!(hex.len(), XEX_HEADER_BYTES * 2);
        let with = |v: Value| serde_json::json!({ "format": 2, MARKER_XEX_HEADER: v });
        assert_eq!(marker_xex_header(&with(Value::from(hex.clone()))), Some(header));
        let bad = [
            Value::from(&hex[..hex.len() - 2]),
            Value::from(&hex[..hex.len() - 1]),
            Value::from(format!("{hex}00")),
            Value::from(format!("g{}", &hex[1..])),
            Value::from(hex.to_uppercase()),
            Value::from(format!("{} {}", &hex[..10], &hex[11..])),
            Value::from(1234),
            Value::Null,
        ];
        for v in bad {
            assert_eq!(marker_xex_header(&with(v.clone())), None, "{}", v.to_string().get(..40).unwrap_or(""));
        }
        assert_eq!(marker_xex_header(&serde_json::json!({ "format": 2 })), None, "a marker without it");
    }

    #[test]
    fn the_marker_is_read_strictly() {
        let dir = std::env::temp_dir().join(format!("ssx-setup-marker-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(marker(&dir), Ok(None));
        std::fs::write(dir.join(MARKER), "{\"format\": 2}").unwrap();
        assert_eq!(marker(&dir).unwrap().unwrap()["format"], 2);
        for bad in ["[2]", "{", "x"] {
            std::fs::write(dir.join(MARKER), bad).unwrap();
            assert!(marker(&dir).is_err(), "{bad:?}");
        }
        std::fs::write(dir.join(MARKER), vec![b' '; MAX_MARKER_BYTES as usize + 1]).unwrap();
        assert!(marker(&dir).unwrap_err().contains("larger than"));
        std::fs::remove_file(dir.join(MARKER)).unwrap();
        std::fs::create_dir(dir.join(MARKER)).unwrap();
        assert!(marker(&dir).is_err(), "a folder");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
