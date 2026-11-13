use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock, RwLock};

pub const MAX_PATH_BYTES: usize = 512;
const MAX_ARCHIVES: usize = 256;
const MAX_FILES: usize = 8192;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TunerVault {
    pub vlt: Arc<[u8]>,
    pub bin: Arc<[u8]>,
    pub version: u32,
    pub source: String,
}

impl TunerVault {
    pub fn from_pack(pack: &ssx_formats::tuner::TunerPack, source: impl Into<String>) -> Self {
        TunerVault {
            vlt: Arc::from(pack.vlt.clone()),
            bin: Arc::from(pack.bin.clone()),
            version: pack.version,
            source: source.into(),
        }
    }
}

static TUNER: OnceLock<RwLock<Option<Arc<TunerVault>>>> = OnceLock::new();

pub fn set_tuner(vault: Option<TunerVault>) {
    let lock = TUNER.get_or_init(|| RwLock::new(None));
    *lock.write().unwrap_or_else(std::sync::PoisonError::into_inner) = vault.map(Arc::new);
    GENERATION.fetch_add(1, Ordering::AcqRel);
}

pub fn tuner() -> Option<Arc<TunerVault>> {
    TUNER.get().and_then(|l| l.read().unwrap_or_else(std::sync::PoisonError::into_inner).clone())
}

static OVERLAYS: OnceLock<RwLock<Vec<PathBuf>>> = OnceLock::new();
static TITLE_UPDATE: OnceLock<RwLock<Option<PathBuf>>> = OnceLock::new();
static GENERATION: AtomicU64 = AtomicU64::new(0);

pub fn set_overlays(roots: Vec<PathBuf>) {
    let lock = OVERLAYS.get_or_init(|| RwLock::new(Vec::new()));
    *lock.write().unwrap_or_else(std::sync::PoisonError::into_inner) = roots;
    GENERATION.fetch_add(1, Ordering::AcqRel);
}

pub fn overlays() -> Vec<PathBuf> {
    OVERLAYS.get().map_or_else(Vec::new, |l| l.read().unwrap_or_else(std::sync::PoisonError::into_inner).clone())
}

pub fn set_title_update(root: Option<PathBuf>) {
    let lock = TITLE_UPDATE.get_or_init(|| RwLock::new(None));
    *lock.write().unwrap_or_else(std::sync::PoisonError::into_inner) = root;
    GENERATION.fetch_add(1, Ordering::AcqRel);
}

pub fn title_update() -> Option<PathBuf> {
    TITLE_UPDATE.get().and_then(|l| l.read().unwrap_or_else(std::sync::PoisonError::into_inner).clone())
}

fn title_update_file(rel: &str) -> Option<PathBuf> {
    let root = title_update()?;
    overlay_file(&root, rel)
}

pub fn overlay_only(rel: &str) -> Option<PathBuf> {
    let rel = checked(rel)?;
    overlays().iter().find_map(|r| overlay_file(r, &rel)).or_else(|| title_update_file(&rel))
}

pub fn overlay_track_file(track: &str, name: &str) -> Option<PathBuf> {
    let track = checked(track)?;
    if track.contains('/') || name.contains('/') {
        return None;
    }
    overlay_only(&format!("data/xenon/gameexplorer/{track}/{name}"))
}

pub fn generation() -> u64 {
    GENERATION.load(Ordering::Acquire)
}

fn checked(rel: &str) -> Option<String> {
    if rel.len() > MAX_PATH_BYTES {
        return None;
    }
    let folded = rel.replace('\\', "/");
    let mut out = String::with_capacity(folded.len());
    for (i, seg) in folded.split('/').enumerate() {
        if seg.is_empty() && i == 0 {
            continue;
        }
        if seg.is_empty()
            || seg == "."
            || seg == ".."
            || seg.contains([':', '\0'])
            || seg.ends_with(['.', ' '])
            || ssx_formats::is_reserved_device_name(seg)
        {
            return None;
        }
        if !out.is_empty() {
            out.push('/');
        }
        out.push_str(seg);
    }
    (!out.is_empty()).then_some(out)
}

fn overlay_file(root: &Path, rel: &str) -> Option<PathBuf> {
    let member = root.join(rel);
    if member.is_file() {
        return Some(member);
    }
    let decoded = root.join("bootcache").join(rel);
    decoded.is_file().then_some(decoded)
}

fn archives(big_root: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = std::fs::read_dir(big_root)
        .ok()
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .take(MAX_ARCHIVES)
        .collect();
    out.sort();
    out.truncate(MAX_ARCHIVES);
    out
}

fn base_file(big_root: &Path, rel: &str) -> Option<PathBuf> {
    let direct = big_root.join(rel);
    if direct.is_file() {
        return Some(direct);
    }
    archives(big_root).into_iter().map(|a| a.join(rel)).find(|p| p.is_file())
}

pub fn find(big_root: &Path, rel: &str) -> Option<PathBuf> {
    let rel = checked(rel)?;
    overlays()
        .iter()
        .find_map(|r| overlay_file(r, &rel))
        .or_else(|| title_update_file(&rel))
        .or_else(|| base_file(big_root, &rel))
}

pub fn find_all(big_root: &Path, rel: &str) -> Vec<PathBuf> {
    let Some(rel) = checked(rel) else { return Vec::new() };
    let mut out: Vec<PathBuf> = Vec::new();
    for p in overlays().iter().filter_map(|r| overlay_file(r, &rel)).chain(title_update_file(&rel)) {
        if !out.iter().any(|q| same_file(q, &p)) {
            out.push(p);
        }
    }
    for p in std::iter::once(big_root.join(&rel)).chain(archives(big_root).into_iter().map(|a| a.join(&rel))) {
        if p.is_file() && !out.iter().any(|q| same_file(q, &p)) {
            out.push(p);
        }
    }
    out
}

fn same_file(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

pub fn find_dir(big_root: &Path, rel: &str) -> Option<PathBuf> {
    let rel = checked(rel)?;
    for root in overlays() {
        let dir = root.join(&rel);
        if dir.is_dir() {
            return Some(dir);
        }
    }
    let direct = big_root.join(&rel);
    if direct.is_dir() {
        return Some(direct);
    }
    archives(big_root).into_iter().map(|a| a.join(&rel)).find(|p| p.is_dir())
}

pub fn track_dir(big_root: &Path, track: &str) -> Option<PathBuf> {
    let track = checked(track)?;
    if track.contains('/') {
        return None;
    }
    find_dir(big_root, &format!("data/xenon/gameexplorer/{track}"))
}

pub fn read(big_root: &Path, rel: &str) -> std::io::Result<Vec<u8>> {
    let path = find(big_root, rel)
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, format!("{rel} not found")))?;
    std::fs::read(path)
}

pub fn files_in(big_root: &Path, rel: &str, extension: &str) -> Vec<PathBuf> {
    let Some(rel) = checked(rel) else { return Vec::new() };
    let mut out: Vec<PathBuf> = Vec::new();
    let mut seen: std::collections::BTreeSet<PathBuf> = std::collections::BTreeSet::new();
    let mut dirs: Vec<PathBuf> = overlays()
        .iter()
        .flat_map(|r| [r.join(&rel), r.join("bootcache").join(&rel)])
        .chain(title_update().into_iter().flat_map(|r| [r.join(&rel), r.join("bootcache").join(&rel)]))
        .collect();
    dirs.push(big_root.join(&rel));
    dirs.extend(archives(big_root).into_iter().map(|a| a.join(&rel)));
    for dir in dirs {
        let Ok(read) = std::fs::read_dir(&dir) else { continue };
        let mut files: Vec<PathBuf> = read
            .flatten()
            .take(MAX_FILES)
            .map(|e| e.path())
            .filter(|p| p.is_file() && p.extension().is_some_and(|x| x.eq_ignore_ascii_case(extension)))
            .collect();
        files.sort();
        for p in files {
            if out.len() >= MAX_FILES {
                return out;
            }
            let key = std::fs::canonicalize(&p).unwrap_or_else(|_| p.clone());
            if seen.insert(key) {
                out.push(p);
            }
        }
    }
    out
}

pub fn in_packages(path: &Path) -> bool {
    let path = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    overlays().iter().any(|root| {
        let root = std::fs::canonicalize(root).unwrap_or_else(|_| root.clone());
        path.starts_with(root)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ssx-data-roots-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn packages_override_the_disc_and_both_layouts_answer() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let dir = scratch("order");
        let (base, first, second) = (dir.join("base"), dir.join("a"), dir.join("b"));
        for d in [&base, &first, &second] {
            std::fs::create_dir_all(d).unwrap();
        }
        std::fs::create_dir_all(base.join("cache1/data/xenon")).unwrap();
        std::fs::write(base.join("cache1/data/xenon/x.txt"), b"disc").unwrap();
        std::fs::create_dir_all(first.join("data/xenon")).unwrap();
        std::fs::write(first.join("data/xenon/x.txt"), b"first").unwrap();
        std::fs::create_dir_all(second.join("bootcache/data/xenon")).unwrap();
        std::fs::write(second.join("bootcache/data/xenon/x.txt"), b"second").unwrap();
        set_overlays(vec![first.clone(), second.clone()]);
        assert_eq!(read(&base, "data/xenon/x.txt").unwrap(), b"first");
        assert_eq!(find_all(&base, "data/xenon/x.txt").len(), 3);
        assert_eq!(read(&base, "bootcache/data/xenon/x.txt").unwrap(), b"second");
        assert_eq!(read(&base, "cache1/data/xenon/x.txt").unwrap(), b"disc");
        set_overlays(Vec::new());
        assert_eq!(read(&base, "data/xenon/x.txt").unwrap(), b"disc");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn hostile_paths_never_leave_their_roots() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let dir = scratch("hostile");
        std::fs::write(dir.join("outside"), b"x").unwrap();
        set_overlays(Vec::new());
        for bad in ["../outside", "data/../../outside", "c:/x", "data/x\0y", "data/./x", "data/CON", "data/nul.txt"] {
            assert_eq!(find(&dir, bad), None, "{bad}");
        }
        assert_eq!(find(&dir, "data/x"), None);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_title_update_file_sits_below_the_packages_but_above_the_disc() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let dir = scratch("tu");
        let base = dir.join("base");
        let pkg = dir.join("pkg");
        let tu = dir.join("tu");
        for d in [&base, &pkg, &tu] {
            std::fs::create_dir_all(d).unwrap();
        }
        std::fs::create_dir_all(base.join("data0/data/xenon/loc")).unwrap();
        std::fs::write(base.join("data0/data/xenon/loc/eng_us.db"), b"disc").unwrap();
        std::fs::create_dir_all(base.join("data0/data/xenon/gameexplorer/alps0101")).unwrap();
        std::fs::create_dir_all(base.join("data0/data/xenon/gameexplorer/newzeal0901")).unwrap();
        std::fs::write(
            base.join("data0/data/xenon/gameexplorer/newzeal0901/newzeal0901_track_boundary.spdt"),
            b"disc boundary",
        )
        .unwrap();
        std::fs::create_dir_all(pkg.join("data/xenon/loc")).unwrap();
        std::fs::write(pkg.join("data/xenon/loc/eng_us.db"), b"dlc").unwrap();
        std::fs::create_dir_all(pkg.join("data/xenon/gameexplorer/japan1101")).unwrap();
        std::fs::create_dir_all(tu.join("data/xenon/loc")).unwrap();
        std::fs::write(tu.join("data/xenon/loc/eng_us.db"), b"tu").unwrap();
        std::fs::create_dir_all(tu.join("data/xenon/gameexplorer/newzeal0901")).unwrap();
        std::fs::write(tu.join("data/xenon/gameexplorer/newzeal0901/newzeal0901_track_boundary.spdt"), b"boundary")
            .unwrap();
        set_overlays(vec![pkg.clone()]);
        set_title_update(Some(tu.clone()));
        assert_eq!(read(&base, "data/xenon/loc/eng_us.db").unwrap(), b"dlc");
        assert_eq!(overlay_only("data/xenon/loc/eng_us.db").unwrap(), pkg.join("data/xenon/loc/eng_us.db"));
        let all = find_all(&base, "data/xenon/loc/eng_us.db");
        assert_eq!(all.len(), 3);
        assert_eq!(all[0], pkg.join("data/xenon/loc/eng_us.db"));
        assert_eq!(all[1], tu.join("data/xenon/loc/eng_us.db"));
        assert_eq!(all[2], base.join("data0/data/xenon/loc/eng_us.db"));
        assert_eq!(
            overlay_track_file("newzeal0901", "newzeal0901_track_boundary.spdt").unwrap(),
            tu.join("data/xenon/gameexplorer/newzeal0901/newzeal0901_track_boundary.spdt")
        );
        assert_eq!(
            track_dir(&base, "newzeal0901").map(|p| p.to_string_lossy().into_owned()),
            Some(base.join("data0/data/xenon/gameexplorer/newzeal0901").to_string_lossy().into_owned())
        );
        assert_eq!(track_dir(&base, "japan1101").unwrap(), pkg.join("data/xenon/gameexplorer/japan1101"));
        set_overlays(Vec::new());
        assert_eq!(read(&base, "data/xenon/loc/eng_us.db").unwrap(), b"tu");
        set_title_update(None);
        assert_eq!(read(&base, "data/xenon/loc/eng_us.db").unwrap(), b"disc");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_track_folder_is_found_on_the_package_or_the_disc() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let dir = scratch("track");
        let pkg = dir.join("pkg");
        std::fs::create_dir_all(pkg.join("data/xenon/gameexplorer/japan1101")).unwrap();
        std::fs::create_dir_all(dir.join("base/data0/data/xenon/gameexplorer/alps0101")).unwrap();
        set_overlays(vec![pkg.clone()]);
        assert_eq!(track_dir(&dir.join("base"), "japan1101").unwrap(), pkg.join("data/xenon/gameexplorer/japan1101"));
        assert_eq!(
            track_dir(&dir.join("base"), "alps0101").unwrap(),
            dir.join("base/data0/data/xenon/gameexplorer/alps0101")
        );
        assert_eq!(track_dir(&dir.join("base"), "../escape"), None);
        set_overlays(Vec::new());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
