mod install;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::RwLock;

use serde::{Deserialize, Serialize};
use ssx_content::path::check_package_path;
use ssx_formats::tu::{TitleUpdatePackage, TuIdentity, version_text};

pub use install::{InstallTuError, TuInstallReport, TuInstalled, install, install_files, installed_folders};

pub const TU_DIR: &str = "Title Updates";
pub const LIST_FILE: &str = "tu.toml";
pub const LIST_FORMAT: u32 = 1;
pub const MAX_LIST_BYTES: u64 = 64 << 10;
pub const MAX_UPDATES: usize = 16;
pub const MAX_PACKAGE_BYTES: u64 = 64 << 20;
pub const MAX_RETAIL_HEADER_BYTES: usize = 0x4000;

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EnabledUpdate {
    pub folder: String,
    #[serde(default)]
    pub name: String,
    pub enabled: bool,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ListFile {
    format: u32,
    #[serde(default)]
    updates: Vec<EnabledUpdate>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TuList {
    pub entries: Vec<EnabledUpdate>,
}

impl TuList {
    pub fn parse(bytes: &[u8], file: &str) -> Result<Self, String> {
        if bytes.len() as u64 > MAX_LIST_BYTES {
            return Err(format!("{file}: over {MAX_LIST_BYTES} bytes"));
        }
        let text = std::str::from_utf8(bytes).map_err(|e| format!("{file}: not UTF-8: {e}"))?;
        let parsed: ListFile = toml::from_str(text).map_err(|e| format!("{file}: {e}"))?;
        if parsed.format != LIST_FORMAT {
            let f = parsed.format;
            return Err(format!("{file}: format {f}; this build reads format {LIST_FORMAT}"));
        }
        if parsed.updates.len() > MAX_UPDATES {
            return Err(format!("{file}: {} entries (at most {MAX_UPDATES})", parsed.updates.len()));
        }
        let mut seen = BTreeSet::new();
        for e in &parsed.updates {
            if e.folder.contains('/') {
                return Err(format!("{file}: {:?} is not a folder name below {TU_DIR}/", e.folder));
            }
            check_package_path(&e.folder, false).map_err(|m| format!("{file}: {m}"))?;
            if !seen.insert(e.folder.to_ascii_lowercase()) {
                return Err(format!("{file}: {:?} is listed twice", e.folder));
            }
        }
        Ok(TuList { entries: parsed.updates })
    }

    pub fn to_toml(&self) -> String {
        toml::to_string(&ListFile { format: LIST_FORMAT, updates: self.entries.clone() }).unwrap_or_default()
    }

    pub fn write(&self, root: &Path) -> Result<(), String> {
        let text = self.to_toml();
        Self::parse(text.as_bytes(), LIST_FILE)?;
        ssx_files::create_dir_all(root).map_err(|e| e.to_string())?;
        ssx_files::replace(&root.join(LIST_FILE), text.as_bytes(), &ssx_files::Options::new())
            .map_err(|e| e.to_string())
    }

    pub fn read(root: &Path) -> Result<Option<Self>, String> {
        let path = root.join(LIST_FILE);
        let name = path.display().to_string();
        match std::fs::symlink_metadata(&path) {
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(format!("{name}: {e}")),
        }
        let bytes = crate::content::dlc::intake::read_file_at_most(&path, MAX_LIST_BYTES)?;
        Self::parse(&bytes, &name).map(Some)
    }
}

#[derive(Clone, Debug)]
pub struct TuEntry {
    pub folder: String,
    pub root: PathBuf,
    pub name: String,
    pub enabled: bool,
    pub target_version: u32,
    pub identity: Option<TuIdentity>,
    pub build_date: BuildDate,
    pub has_patch: bool,
    pub members: usize,
    pub bytes: u64,
    pub ready: bool,
}

impl TuEntry {
    pub fn version_text(&self) -> String {
        version_text(self.target_version)
    }

    pub fn display_name(&self) -> &str {
        if self.name.is_empty() { &self.folder } else { &self.name }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BuildDate {
    Known(String),
    NotInHeader,
    NoDiscHeader,
    NotDecoded(String),
}

impl BuildDate {
    pub fn identify(update: &TitleUpdatePackage<'_>, header: Option<&[u8]>) -> (Option<TuIdentity>, Self) {
        let Some(header) = header else { return (None, Self::NoDiscHeader) };
        match update.identify(Some(header)) {
            Ok(i) => {
                let date =
                    i.pe_timestamp.map_or(Self::NotInHeader, |t| Self::Known(ssx_formats::tu::timestamp_text(t)));
                (Some(i), date)
            }
            Err(e) => (None, Self::NotDecoded(ssx_core::text::message(&e.to_string()).to_string())),
        }
    }
}

impl std::fmt::Display for BuildDate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Known(date) => write!(f, "built {date}"),
            Self::NotInHeader => f.write_str("no build date in its header"),
            Self::NoDiscHeader => f.write_str("build date unknown (re-run the setup to show it)"),
            Self::NotDecoded(why) => write!(f, "build date unknown (the disc's header does not decode it: {why})"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TuSkip {
    pub folder: String,
    pub reason: String,
}

#[derive(Clone, Debug, Default)]
pub struct TuSet {
    pub root: PathBuf,
    pub entries: Vec<TuEntry>,
    pub skipped: Vec<TuSkip>,
    pub list_error: Option<String>,
    pub applied: Option<usize>,
}

impl TuSet {
    pub fn load(tu_root: &Path, retail_header: Option<&[u8]>) -> Self {
        let mut skipped = Vec::new();
        let (list, list_error) = match TuList::read(tu_root) {
            Ok(list) => (list, None),
            Err(e) => {
                skipped.push(TuSkip { folder: LIST_FILE.to_string(), reason: e.clone() });
                (None, Some(e))
            }
        };
        let mut entries = Vec::new();
        if list_error.is_none() {
            for e in list.unwrap_or_default().entries {
                match load_folder(&tu_root.join(&e.folder), &e.folder, &e.name, e.enabled, retail_header) {
                    Ok(entry) => entries.push(entry),
                    Err(reason) => skipped.push(TuSkip { folder: e.folder, reason }),
                }
            }
        }
        let mut set = TuSet { root: tu_root.to_path_buf(), entries, skipped, list_error, applied: None };
        set.choose();
        set.skipped.sort_by(|a, b| a.folder.cmp(&b.folder));
        set
    }

    fn choose(&mut self) {
        let best = self
            .entries
            .iter()
            .enumerate()
            .filter(|(_, e)| e.enabled && e.ready)
            .max_by_key(|(_, e)| e.target_version)
            .map(|(i, _)| i);
        self.applied = best;
        for (i, e) in self.entries.iter().enumerate() {
            if Some(i) == best {
                continue;
            }
            if !e.enabled {
                continue;
            }
            let reason = if !e.ready {
                "not ready (its extracted data is missing); reinstall it".to_string()
            } else {
                let chosen = &self.entries[best.expect("a ready entry exists")];
                format!(
                    "one title update applies: {} is used; {} is not (the updates are alternative rebuilds, not \
                     cumulative)",
                    chosen.version_text(),
                    e.version_text()
                )
            };
            self.skipped.push(TuSkip { folder: e.folder.clone(), reason });
        }
    }

    pub fn mounted(&self) -> Option<&TuEntry> {
        self.applied.and_then(|i| self.entries.get(i))
    }

    pub fn mounted_root(&self) -> Option<PathBuf> {
        self.mounted().map(|e| e.root.clone())
    }

    pub fn lines(&self) -> Vec<ssx_core::text::Text> {
        self.line_strings().into_iter().map(ssx_core::text::Text::from).collect()
    }

    pub(super) fn line_strings(&self) -> Vec<String> {
        let mut out = Vec::new();
        for e in &self.entries {
            let state = if e.enabled { "on" } else { "off" };
            let applied =
                if Some(e.folder.as_str()) == self.mounted().map(|m| m.folder.as_str()) { ", applied" } else { "" };
            out.push(format!(
                "title update: {} {} — XEX {}, {}, patch0.big {}{applied}, folder {}",
                e.display_name(),
                state,
                e.version_text(),
                e.build_date,
                if e.has_patch { format!("{} members", e.members) } else { "none".to_string() },
                e.folder
            ));
        }
        for s in &self.skipped {
            out.push(format!("title update: skipped {}: {}", s.folder, s.reason));
        }
        out
    }

    pub fn set_enabled(&self, folder: &str, on: bool) -> Result<(), String> {
        let mut list = TuList::read(&self.root)?.unwrap_or_default();
        match list.entries.iter_mut().find(|e| e.folder == folder) {
            Some(e) => e.enabled = on,
            None => return Err(format!("{folder} is not in {LIST_FILE}")),
        }
        list.write(&self.root)
    }

    pub fn forget(&self, folder: &str) -> Result<(), String> {
        let mut list = TuList::read(&self.root)?.unwrap_or_default();
        list.entries.retain(|e| e.folder != folder);
        list.write(&self.root)
    }
}

fn load_folder(
    root: &Path,
    folder: &str,
    name: &str,
    enabled: bool,
    retail_header: Option<&[u8]>,
) -> Result<TuEntry, String> {
    let package_path = root.join("package");
    let bytes = crate::content::dlc::intake::read_file_at_most(&package_path, MAX_PACKAGE_BYTES)?;
    let update = TitleUpdatePackage::parse(&bytes).map_err(|e| format!("{}: {e}", package_path.display()))?;
    let target_version = update.target_version().map_err(|e| format!("{}: {e}", package_path.display()))?;
    let (identity, build_date) = BuildDate::identify(&update, retail_header);
    let patch = update.patch().is_some();
    let data = root.join("data");
    let ready = !patch || data.is_dir();
    let (members, decoded) = if patch {
        let report = update.patch().and_then(|p| p.verify().ok()).unwrap_or_default();
        (report.members, report.bytes)
    } else {
        (0, 0)
    };
    Ok(TuEntry {
        folder: folder.to_string(),
        root: root.to_path_buf(),
        name: name.to_string(),
        enabled,
        target_version,
        identity,
        build_date,
        has_patch: patch,
        members,
        bytes: decoded,
        ready,
    })
}

pub fn root(dlc_root: &Path) -> PathBuf {
    dlc_root.join(TU_DIR)
}

pub fn retail_header(big_root: &Path) -> Option<Vec<u8>> {
    retail_header_file(big_root).or_else(|| {
        big_root.parent().and_then(crate::setup::read_marker).and_then(|m| crate::setup::marker_xex_header(&m))
    })
}

fn retail_header_file(big_root: &Path) -> Option<Vec<u8>> {
    let mut candidates = Vec::new();
    if let Some(parent) = big_root.parent() {
        candidates.push(parent.join("disc/default.xex"));
    }
    candidates.push(big_root.join("disc/default.xex"));
    for path in candidates {
        let Ok(meta) = std::fs::symlink_metadata(&path) else { continue };
        if !meta.is_file() {
            continue;
        }
        let Ok(mut file) = std::fs::File::open(&path) else { continue };
        let mut header = vec![0u8; MAX_RETAIL_HEADER_BYTES];
        use std::io::Read;
        if file.read_exact(&mut header).is_ok() {
            return Some(header);
        }
    }
    None
}

pub fn folders(tu_root: &Path) -> Vec<String> {
    let Ok(read) = std::fs::read_dir(tu_root) else { return Vec::new() };
    let mut out: Vec<String> =
        read.flatten().filter(|e| e.path().is_dir()).map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    out.sort();
    out
}

static ACTIVE: RwLock<Option<std::sync::Arc<TuSet>>> = RwLock::new(None);

pub fn set_active(set: std::sync::Arc<TuSet>) {
    *ACTIVE.write().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(set);
}

pub fn active() -> Option<std::sync::Arc<TuSet>> {
    ACTIVE.read().unwrap_or_else(std::sync::PoisonError::into_inner).clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(folder: &str, version: u32, enabled: bool, ready: bool) -> TuEntry {
        TuEntry {
            folder: folder.into(),
            root: PathBuf::from(folder),
            name: folder.into(),
            enabled,
            target_version: version,
            identity: None,
            build_date: BuildDate::NoDiscHeader,
            has_patch: version >> 8 >= 3,
            members: 0,
            bytes: 0,
            ready,
        }
    }

    #[test]
    fn one_title_update_applies_the_newest_and_the_other_says_why() {
        let mut set = TuSet {
            root: PathBuf::new(),
            entries: vec![entry("tu_one", 0x0102, true, true), entry("tu_three", 0x0302, true, true)],
            skipped: Vec::new(),
            list_error: None,
            applied: None,
        };
        set.choose();
        assert_eq!(set.mounted().map(|e| e.folder.as_str()), Some("tu_three"));
        assert_eq!(set.skipped.len(), 1);
        assert!(set.skipped[0].reason.contains("3.02 is used"), "{}", set.skipped[0].reason);
        set.entries[1].enabled = false;
        set.choose();
        assert_eq!(set.mounted().map(|e| e.folder.as_str()), Some("tu_one"));
        set.entries[1].enabled = true;
        set.entries[0].ready = false;
        set.choose();
        assert_eq!(set.mounted().map(|e| e.folder.as_str()), Some("tu_three"));
    }

    #[test]
    fn the_list_is_read_strictly() {
        let ok = b"format = 1\n[[updates]]\nfolder = \"tu_three\"\nname = \"tu00000002_00000000\"\nenabled = true\n";
        let list = TuList::parse(ok, "tu.toml").unwrap();
        assert_eq!(list.entries.len(), 1);
        assert_eq!(TuList::parse(list.to_toml().as_bytes(), "tu.toml").unwrap(), list);
        for bad in [
            b"format = 2\n" as &[u8],
            b"format = 1\n[[updates]]\nfolder = \"../x\"\nenabled = true\n",
            b"format = 1\n[[updates]]\nfolder = \"a\"\nenabled = true\n[[updates]]\nfolder = \"A\"\nenabled = true\n",
            b"format = 1\n[[updates]]\nfolder = \"a\"\nenabled = true\nunknown = 1\n",
        ] {
            assert!(TuList::parse(bad, "tu.toml").is_err(), "{}", String::from_utf8_lossy(bad));
        }
    }

    #[test]
    fn the_build_date_or_why_it_is_unknown() {
        assert_eq!(BuildDate::NoDiscHeader.to_string(), "build date unknown (re-run the setup to show it)");
        assert_eq!(BuildDate::Known(ssx_formats::tu::timestamp_text(1_341_792_000)).to_string(), "built 2012-07-09");
        assert_eq!(BuildDate::NotInHeader.to_string(), "no build date in its header");
        let damaged = BuildDate::NotDecoded("an LZX block runs past its data".into()).to_string();
        assert_eq!(
            damaged,
            "build date unknown (the disc's header does not decode it: an LZX block runs past its data)"
        );
    }

    #[test]
    fn the_retail_header_is_beside_the_big_root() {
        let dir = std::env::temp_dir().join(format!("ssx-tu-header-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("disc")).unwrap();
        std::fs::create_dir_all(dir.join("big/data0")).unwrap();
        std::fs::write(dir.join("disc/default.xex"), vec![0xAB; 0x5000]).unwrap();
        let header = retail_header(&dir.join("big")).unwrap();
        assert_eq!(header.len(), MAX_RETAIL_HEADER_BYTES);
        assert!(header.iter().all(|b| *b == 0xAB));
        std::fs::remove_file(dir.join("disc/default.xex")).unwrap();
        assert!(retail_header(&dir.join("big")).is_none());
        let copy = vec![0xCD; MAX_RETAIL_HEADER_BYTES];
        let marker = |v: &str| format!("{{\"format\": 2, \"{}\": \"{v}\"}}", crate::setup::MARKER_XEX_HEADER);
        std::fs::write(dir.join(crate::setup::MARKER), marker(&crate::setup::xex_header_hex(&copy))).unwrap();
        assert_eq!(retail_header(&dir.join("big")), Some(copy));
        std::fs::write(dir.join(crate::setup::MARKER), marker("cdcd")).unwrap();
        assert!(retail_header(&dir.join("big")).is_none(), "a short copy");
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(retail_header(&dir.join("big")).is_none());
    }
}
