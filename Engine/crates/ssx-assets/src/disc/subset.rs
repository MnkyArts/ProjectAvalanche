use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use ssx_formats::chunk;
use ssx_formats::xdvdfs::Disc;

use super::setup::{
    ARCHIVES, Cancelled, DiscCheck, Progress, big_root, check_disc, open_disc, portable_name, read_index, safe_relative,
};

pub const MARKER: &str = "subset.json";
const FORMAT: u64 = 1;
// PORT(choice): 16 MiB, 2.5 times the largest server member of the one disc measured (`cache0` `anim_all.cba`, 6 484
// 944 bytes); tuning: the largest member of the other pressings of the disc; a bound of the port's server subset.
pub const MAX_MEMBER: u64 = 16 << 20;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Member {
    pub archive: String,
    pub path: String,
}

impl Member {
    pub fn data_path(&self) -> String {
        format!("big/{}/{}", self.archive, self.path)
    }

    pub fn from_data_path(rel: &str) -> Result<Self> {
        let Some((archive, path)) = rel.strip_prefix("big/").and_then(|r| r.split_once('/')) else {
            bail!("{rel:?} is not a member path (big/<archive>/<path>)");
        };
        Member::new(archive, path)
    }

    pub fn new(archive: &str, path: &str) -> Result<Self> {
        if !ARCHIVES.contains(&archive) {
            bail!("{archive:?} is not one of the disc's archives");
        }
        let portable = path.is_ascii() && path.split('/').all(portable_name);
        if safe_relative(path).is_none() || !portable || path.len() > 255 {
            bail!("{:?} is not a usable member path", path.chars().take(80).collect::<String>());
        }
        Ok(Member { archive: archive.to_string(), path: path.to_string() })
    }

    fn folded(&self) -> String {
        self.data_path().to_ascii_lowercase()
    }
}

#[derive(Clone, Debug, Default)]
pub struct Selection {
    members: BTreeMap<String, BTreeSet<String>>,
    folded: BTreeSet<String>,
}

impl Selection {
    pub fn insert(&mut self, m: Member) -> Result<()> {
        if self.contains(&m) {
            return Ok(());
        }
        if !self.folded.insert(m.folded()) {
            bail!("{} differs from another member only in case", m.data_path());
        }
        self.members.entry(m.archive).or_default().insert(m.path);
        Ok(())
    }

    pub fn len(&self) -> usize {
        self.members.values().map(BTreeSet::len).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn contains(&self, m: &Member) -> bool {
        self.members.get(&m.archive).is_some_and(|s| s.contains(&m.path))
    }

    pub fn members(&self) -> Vec<Member> {
        ARCHIVES
            .iter()
            .filter_map(|a| self.members.get(*a).map(|set| (a, set)))
            .flat_map(|(a, set)| set.iter().map(move |p| Member { archive: a.to_string(), path: p.clone() }))
            .collect()
    }
}

#[derive(Clone, Debug)]
pub struct SubsetRun {
    pub disc: DiscCheck,
    pub members: usize,
    pub written: usize,
    pub kept: usize,
    pub bytes: u64,
}

pub fn read_marker(out: &Path) -> Option<Value> {
    let path = out.join(MARKER);
    let meta = fs::symlink_metadata(&path).ok()?;
    if !meta.is_file() || meta.len() > 64 * 1024 {
        return None;
    }
    let value: Value = serde_json::from_str(&fs::read_to_string(path).ok()?).ok()?;
    value.is_object().then_some(value)
}

pub fn is_complete(out: &Path) -> bool {
    read_marker(out).is_some_and(|m| m["complete"] == json!(true) && m["format"] == json!(FORMAT))
}

pub fn run(
    iso: &Path,
    out: &Path,
    selection: &Selection,
    label: Value,
    check: &mut dyn FnMut(&Member, &[u8]) -> Result<()>,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(&Progress),
) -> Result<SubsetRun> {
    if selection.is_empty() {
        bail!("nothing to take: the selection is empty");
    }
    let mut disc = open_disc(iso)?;
    let disc_check = check_disc(&mut disc)?;
    let dir = fs::symlink_metadata(out).with_context(|| format!("{}", out.display()))?;
    if !dir.is_dir() {
        bail!("{} is not a folder (a link or a file): refused", out.display());
    }
    let same_disc = |m: &Value| {
        m["format"] == json!(FORMAT)
            && m["fingerprint"] == json!(disc_check.fingerprint)
            && m["iso_size"] == json!(disc_check.image_size)
    };
    let resume = read_marker(out).is_some_and(|m| same_disc(&m));
    let mut marker = json!({
        "format": FORMAT,
        "iso": iso.display().to_string(),
        "iso_size": disc_check.image_size,
        "fingerprint": disc_check.fingerprint,
        "layout": format!("{:?}", disc_check.layout),
        "selection": label,
        "members": selection.len(),
        "complete": false,
    });
    write_synced(out, MARKER, serde_json::to_string_pretty(&marker)?.as_bytes())?;

    let members = selection.members();
    let mut p = Progress { archive_count: ARCHIVES.len(), files_total: members.len(), ..Default::default() };
    let mut result = SubsetRun { disc: disc_check, members: members.len(), written: 0, kept: 0, bytes: 0 };
    for (index, archive) in ARCHIVES.iter().enumerate() {
        let wanted: Vec<&Member> = members.iter().filter(|m| m.archive == *archive).collect();
        if wanted.is_empty() {
            continue;
        }
        let extent = disc.find(&format!("{archive}.big")).context("archive vanished")?.extent();
        let big = read_index(&mut disc, archive, extent)?;
        let by_path: BTreeMap<String, &ssx_formats::big::BigEntry> =
            big.entries.iter().map(|e| (e.path(), e)).collect();
        p.archive = archive.to_string();
        p.archive_index = index;
        progress(&p);
        for m in wanted {
            if cancel.load(Ordering::Relaxed) {
                return Err(Cancelled.into());
            }
            let Some(entry) = by_path.get(&m.path) else {
                bail!(
                    "{archive}.big on this disc has no member {}: not the disc the server's member list is of",
                    m.path
                );
            };
            if entry.offset + entry.size as u64 > extent.size as u64 {
                bail!("{archive}.big: {} ends past the archive (a damaged image?)", m.path);
            }
            let dest = member_dest(out, m)?;
            let kept = resume && keep_existing(out, m, check)?;
            let size = if kept {
                result.kept += 1;
                fs::symlink_metadata(&dest)?.len()
            } else {
                let data = read_member(&mut disc, extent, entry.offset, entry.size, m)?;
                check(m, &data).with_context(|| format!("{}: refused", m.data_path()))?;
                write_member(&dest, &data)?;
                result.written += 1;
                data.len() as u64
            };
            result.bytes += size;
            p.files_done += 1;
            p.bytes_done += size;
            progress(&p);
        }
    }
    marker["complete"] = json!(true);
    marker["bytes"] = json!(result.bytes);
    write_synced(out, MARKER, serde_json::to_string_pretty(&marker)?.as_bytes())?;
    Ok(result)
}

pub fn read(iso: &Path, members: &[Member]) -> Result<Vec<Vec<u8>>> {
    let mut disc = open_disc(iso)?;
    check_disc(&mut disc)?;
    let mut out = Vec::with_capacity(members.len());
    let mut indexes = BTreeMap::new();
    for m in members {
        if !indexes.contains_key(&m.archive) {
            let extent = disc.find(&format!("{}.big", m.archive)).context("archive vanished")?.extent();
            indexes.insert(m.archive.clone(), (extent, read_index(&mut disc, &m.archive, extent)?));
        }
        let (extent, big) = &indexes[&m.archive];
        let Some(entry) = big.entries.iter().find(|e| e.path() == m.path) else {
            bail!("{}.big on this disc has no member {}", m.archive, m.path);
        };
        if entry.offset + entry.size as u64 > extent.size as u64 {
            bail!("{}.big: {} ends past the archive (a damaged image?)", m.archive, m.path);
        }
        out.push(read_member(&mut disc, *extent, entry.offset, entry.size, m)?);
    }
    Ok(out)
}

fn read_member(
    disc: &mut Disc<File>,
    extent: ssx_formats::xdvdfs::Extent,
    offset: u64,
    size: u32,
    m: &Member,
) -> Result<Vec<u8>> {
    if u64::from(size) > MAX_MEMBER {
        bail!("{}: {size} bytes stored, more than a server member may have ({MAX_MEMBER})", m.data_path());
    }
    let mut r = disc.reader(extent)?;
    r.seek(SeekFrom::Start(offset))?;
    let mut stored = vec![0u8; size as usize];
    r.read_exact(&mut stored).with_context(|| format!("reading {} from the image", m.data_path()))?;
    if chunk::detect(&stored).is_none() {
        return Ok(stored);
    }
    let head = chunk::read_header(&stored).with_context(|| format!("{}: its container's header", m.data_path()))?;
    if u64::from(head.full_size) > MAX_MEMBER {
        bail!("{}: the container says {} bytes, more than a server member may have", m.data_path(), head.full_size);
    }
    chunk::decompress(&stored).with_context(|| format!("{}: does not decode (a damaged image?)", m.data_path()))
}

pub fn put(out: &Path, m: &Member, data: &[u8]) -> Result<()> {
    write_member(&member_dest(out, m)?, data)
}

pub fn existing(out: &Path, m: &Member) -> Result<Option<Vec<u8>>> {
    let dest = member_dest(out, m)?;
    let Ok(meta) = fs::symlink_metadata(&dest) else { return Ok(None) };
    if !meta.is_file() || meta.len() > MAX_MEMBER {
        return Ok(None);
    }
    let mut data = Vec::new();
    File::open(&dest)?.take(MAX_MEMBER).read_to_end(&mut data)?;
    Ok(Some(data))
}

fn member_dest(out: &Path, m: &Member) -> Result<PathBuf> {
    let rel = safe_relative(&m.path).with_context(|| format!("unusable member path {:?}", m.path))?;
    let mut dir = big_root(out);
    let mut parts: Vec<_> = Path::new(&m.archive).join(&rel).components().map(|c| c.as_os_str().to_owned()).collect();
    let name = parts.pop().context("member path has a name")?;
    real_folder(&dir)?;
    for part in parts {
        dir.push(part);
        real_folder(&dir)?;
    }
    let dest = dir.join(name);
    if let Ok(meta) = fs::symlink_metadata(&dest)
        && !meta.is_file()
    {
        bail!("{} is a link or a folder, not a file: refused (remove it)", dest.display());
    }
    Ok(dest)
}

fn real_folder(dir: &Path) -> Result<()> {
    match fs::symlink_metadata(dir) {
        Ok(meta) if meta.is_dir() => Ok(()),
        Ok(_) => bail!("{} is a link or a file, not a folder: refused", dir.display()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir(dir).with_context(|| format!("creating {}", dir.display()))?;
            sync_parent(dir)
        }
        Err(e) => Err(e).with_context(|| format!("{}", dir.display())),
    }
}

fn keep_existing(out: &Path, m: &Member, check: &mut dyn FnMut(&Member, &[u8]) -> Result<()>) -> Result<bool> {
    Ok(existing(out, m)?.is_some_and(|data| check(m, &data).is_ok()))
}

fn write_member(dest: &Path, data: &[u8]) -> Result<()> {
    ssx_files::replace(dest, data, &ssx_files::Options::new()).with_context(|| format!("writing {}", dest.display()))
}

pub fn write_synced(dir: &Path, name: &str, bytes: &[u8]) -> Result<()> {
    let dest = dir.join(name);
    if let Ok(meta) = fs::symlink_metadata(&dest)
        && !meta.is_file()
    {
        bail!("{} is a link or a folder, not a file: refused", dest.display());
    }
    write_member(&dest, bytes)
}

fn sync_parent(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        ssx_files::sync_folder(parent)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn member_paths_are_checked() {
        let m = Member::from_data_path("big/bootcache/data/xenon/attribdb/attribdb.vlt").expect("valid");
        assert_eq!((m.archive.as_str(), m.path.as_str()), ("bootcache", "data/xenon/attribdb/attribdb.vlt"));
        assert_eq!(m.data_path(), "big/bootcache/data/xenon/attribdb/attribdb.vlt");
        for bad in [
            "bootcache/data/x",
            "big/nocache/data/x",
            "big/bootcache/../x",
            "big/bootcache/data//x",
            "big/bootcache/c:x",
            "big/bootcache/a\\b",
            "big/bootcache",
            "big/bootcache/data/CON",
            "big/bootcache/data/nul.txt",
            "big/bootcache/Com1.bin",
            "big/bootcache/lpt9",
            "big/bootcache/conout$",
            "big/bootcache/data/x.",
            "big/bootcache/data/x ",
            "big/bootcache/data/caf\u{e9}.bin",
        ] {
            assert!(Member::from_data_path(bad).is_err(), "{bad:?}");
        }
        for good in ["big/bootcache/data/console.bin", "big/bootcache/com10", "big/bootcache/nul_x", "big/bootcache/.a"]
        {
            assert!(Member::from_data_path(good).is_ok(), "{good:?}");
        }
    }

    #[test]
    fn members_differing_only_in_case_are_refused() {
        let mut s = Selection::default();
        s.insert(Member::new("data0", "data/xenon/a.bin").unwrap()).expect("first");
        s.insert(Member::new("data0", "data/xenon/a.bin").unwrap()).expect("the same again");
        let e = s.insert(Member::new("data0", "Data/Xenon/A.bin").unwrap()).unwrap_err().to_string();
        assert!(e.contains("only in case"), "{e}");
        s.insert(Member::new("bootcache", "data/xenon/a.bin").unwrap()).expect("another archive");
        assert_eq!(s.len(), 2);
    }

    #[cfg(unix)]
    #[test]
    fn links_in_the_folder_are_refused() {
        let dir = std::env::temp_dir().join(format!("ssx-subset-links-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("elsewhere")).unwrap();
        let m = Member::new("bootcache", "data/xenon/a.bin").unwrap();
        fs::create_dir_all(dir.join("out")).unwrap();
        std::os::unix::fs::symlink(dir.join("elsewhere"), dir.join("out/big")).unwrap();
        let e = member_dest(&dir.join("out"), &m).unwrap_err().to_string();
        assert!(e.contains("not a folder: refused"), "{e}");
        fs::remove_file(dir.join("out/big")).unwrap();
        let dest = member_dest(&dir.join("out"), &m).expect("real folders");
        std::os::unix::fs::symlink(dir.join("elsewhere/target"), &dest).unwrap();
        assert!(member_dest(&dir.join("out"), &m).unwrap_err().to_string().contains("not a file"));
        fs::remove_file(&dest).unwrap();
        let mut partial = dest.as_os_str().to_owned();
        partial.push(".partial");
        std::os::unix::fs::symlink(dir.join("elsewhere/victim"), PathBuf::from(&partial)).unwrap();
        write_member(&dest, b"member").expect("a link at the partial name is passed over, never written through");
        assert!(!dir.join("elsewhere/victim").exists(), "nothing was written through the link");
        assert!(fs::symlink_metadata(&partial).unwrap().file_type().is_symlink(), "the link left as it was");
        assert_eq!(fs::read(&dest).unwrap(), b"member");
        fs::remove_dir_all(&dir).unwrap();
    }
}
