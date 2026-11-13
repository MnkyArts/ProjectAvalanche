use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use crate::keys::{IDENTITY_FILE, NETCODE_FILE};

pub const OWN: [&str; 12] = [
    IDENTITY_FILE,
    NETCODE_FILE,
    crate::logging::LOG_FOLDER,
    "big",
    ssx_assets::subset::MARKER,
    super::index::INDEX_FILE,
    crate::operation::store::BANS_FILE,
    crate::operation::store::ACL_FILE,
    crate::resources::config::DEFAULT_FOLDER,
    crate::resources::STATE_FOLDER,
    crate::scripts::STORAGE_FOLDER,
    ssx_content::blocklist::CACHE_FOLDER,
];

#[derive(Clone, Copy, Debug)]
pub struct Configured<'a> {
    pub resources: &'a Path,
    pub rules: Option<&'a Path>,
    pub blocklist: Option<&'a Path>,
}

impl<'a> Configured<'a> {
    pub fn of(config: &'a crate::config::Config) -> Self {
        Configured {
            resources: &config.resources.folder,
            rules: Some(&config.data_folder),
            blocklist: config.discovery.blocklist.as_deref(),
        }
    }

    pub fn resources(resources: &'a Path) -> Self {
        Configured { resources, rules: None, blocklist: None }
    }

    fn names(&self, out: &Path) -> Vec<&'a std::ffi::OsStr> {
        [Some(self.resources), self.rules, self.blocklist]
            .into_iter()
            .flatten()
            .filter_map(|p| top_name(out, p))
            .collect()
    }
}

const SHOWN: usize = 8;
// PORT(choice): 20 000 entries looked at (the whole list makes 1 925 files in about 300 folders), enough unless the
// list grows (resources' files are elsewhere); the original has no dedicated server.
const MAX_ENTRIES: usize = 20_000;

pub fn shown(name: &std::ffi::OsStr) -> String {
    ssx_core::text::shown_max(&name.to_string_lossy(), 80).to_string()
}

fn top_name<'a>(out: &Path, configured: &'a Path) -> Option<&'a std::ffi::OsStr> {
    use std::path::Component;
    let below = if configured.is_absolute() { configured.strip_prefix(out).ok()? } else { configured };
    let mut parts = below.components();
    match (parts.next(), parts.next()) {
        (Some(Component::Normal(name)), None) => Some(name),
        _ => None,
    }
}

pub fn prepare(out: &Path, wanted: &BTreeSet<String>, configured: &Configured<'_>) -> Result<(), String> {
    let configured = configured.names(out);
    match fs::symlink_metadata(out) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return fs::create_dir(out).map_err(|e| format!("{}: cannot be made: {e}", out.display()));
        }
        Err(e) => return Err(format!("{}: {e}", out.display())),
        Ok(m) if m.file_type().is_symlink() => {
            return Err(format!("{} is a link: refused (give the folder itself)", out.display()));
        }
        Ok(m) if !m.is_dir() => return Err(format!("{} is not a folder", out.display())),
        Ok(_) => {}
    }
    let (mut foreign, mut foreign_count) = (BTreeSet::new(), 0usize);
    for e in fs::read_dir(out).map_err(|e| format!("{}: {e}", out.display()))? {
        let e = e.map_err(|e| format!("{}: {e}", out.display()))?;
        let name = e.file_name();
        let own = configured.contains(&name.as_os_str())
            || name.to_str().is_some_and(|n| {
                OWN.contains(&n) || ssx_files::partial_target(n).is_some_and(|stem| OWN.contains(&stem))
            });
        if name == "setup.json" {
            return Err(format!(
                "{} holds the game's data (its setup.json): a server's data folder is a folder of its own",
                out.display()
            ));
        }
        if !own {
            foreign_count += 1;
            foreign.insert(shown(&name));
            if foreign.len() > SHOWN {
                foreign.pop_last();
            }
        }
    }
    if foreign_count > 0 {
        let more = foreign_count.saturating_sub(foreign.len());
        return Err(format!(
            "{} holds files that are not a server's ({}{}): choose an empty folder or the server's data folder",
            out.display(),
            foreign.into_iter().collect::<Vec<_>>().join(", "),
            if more > 0 { format!(" and {more} more") } else { String::new() }
        ));
    }
    let big = out.join("big");
    if let Ok(m) = fs::symlink_metadata(&big) {
        if !m.is_dir() {
            return Err(format!("{} is a link or a file, not a folder: refused", big.display()));
        }
        let mut extra = Vec::new();
        let mut seen = 0usize;
        let mut dirs = vec![big.clone()];
        while let Some(d) = dirs.pop() {
            for e in fs::read_dir(&d).map_err(|e| format!("{}: {e}", d.display()))? {
                let e = e.map_err(|e| format!("{}: {e}", d.display()))?;
                seen += 1;
                if seen > MAX_ENTRIES {
                    return Err(format!(
                        "{} holds more than {MAX_ENTRIES} entries: not a server's subset",
                        big.display()
                    ));
                }
                let t = e.file_type().map_err(|err| format!("{}: {err}", e.path().display()))?;
                let path = e.path();
                if t.is_symlink() {
                    return Err(format!("{} is a link: refused (remove it)", path.display()));
                }
                if t.is_dir() {
                    dirs.push(path);
                    continue;
                }
                let rel = path.strip_prefix(out).map_err(|e| e.to_string())?;
                let rel: Vec<String> = rel.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
                let rel = rel.join("/");
                let member = ssx_files::partial_target(&rel).unwrap_or(&rel);
                if !t.is_file() || !wanted.contains(member) {
                    extra.push(rel.chars().take(120).collect::<String>());
                }
            }
        }
        if !extra.is_empty() {
            let n = extra.len();
            extra.sort();
            extra.truncate(SHOWN);
            return Err(format!(
                "{} holds {n} file(s) this setup was not asked for ({}{}): add their class (--with …) or remove them",
                big.display(),
                extra.join(", "),
                if n > SHOWN { ", …" } else { "" }
            ));
        }
    }
    Ok(())
}

pub fn free_bytes(path: &Path) -> Result<u64, String> {
    free(path).map_err(|e| format!("the free space of {}: {e}", path.display()))
}

#[cfg(unix)]
fn free(path: &Path) -> std::io::Result<u64> {
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(path.as_os_str().as_bytes()).map_err(|_| std::io::ErrorKind::InvalidInput)?;
    let mut s: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(c.as_ptr(), &mut s) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(wide(s.f_bavail).saturating_mul(wide(s.f_frsize)))
}

#[cfg(unix)]
fn wide<T: TryInto<u64>>(v: T) -> u64 {
    v.try_into().unwrap_or(u64::MAX)
}

#[cfg(windows)]
fn free(path: &Path) -> std::io::Result<u64> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
    let mut available = 0u64;
    if unsafe { GetDiskFreeSpaceExW(wide.as_ptr(), &mut available, std::ptr::null_mut(), std::ptr::null_mut()) } == 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(available)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("avalanche-guard-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn foreign_and_game_folders_are_refused() {
        let wanted: BTreeSet<String> = ["big/bootcache/a.bin".to_string()].into();
        let r = &Configured::resources(Path::new(crate::resources::config::DEFAULT_FOLDER));
        let d = folder("foreign");
        prepare(&d, &wanted, r).expect("a missing folder is made");
        fs::write(d.join(IDENTITY_FILE), "k").unwrap();
        fs::create_dir_all(d.join("big/bootcache")).unwrap();
        fs::write(d.join("big/bootcache/a.bin"), "m").unwrap();
        fs::write(d.join("big/bootcache/a.bin.partial"), "m").unwrap();
        fs::write(d.join("big/bootcache/a.bin.partial-3"), "m").unwrap();
        fs::write(d.join(format!("{IDENTITY_FILE}.partial-1")), "k").unwrap();
        fs::write(d.join("bans.toml"), "").unwrap();
        fs::write(d.join("acl.toml.partial-2"), "").unwrap();
        prepare(&d, &wanted, r).expect("the server's own files");
        fs::write(d.join("big/bootcache/b.bin"), "x").unwrap();
        assert!(prepare(&d, &wanted, r).unwrap_err().contains("not asked for (big/bootcache/b.bin)"));
        fs::remove_file(d.join("big/bootcache/b.bin")).unwrap();
        fs::write(d.join("notes.txt"), "x").unwrap();
        assert!(prepare(&d, &wanted, r).unwrap_err().contains("not a server's (notes.txt)"));
        for n in 0..20 {
            fs::write(d.join(format!("other-{n:02}")), "x").unwrap();
        }
        let e = prepare(&d, &wanted, r).unwrap_err();
        let first: Vec<String> =
            std::iter::once("notes.txt".to_string()).chain((0..SHOWN - 1).map(|n| format!("other-{n:02}"))).collect();
        assert!(e.contains(&format!("not a server's ({} and 13 more)", first.join(", "))), "{e}");
        for n in 0..20 {
            fs::remove_file(d.join(format!("other-{n:02}"))).unwrap();
        }
        fs::write(d.join("setup.json"), "{}").unwrap();
        assert!(prepare(&d, &wanted, r).unwrap_err().contains("the game's data"));
        fs::remove_dir_all(&d).unwrap();
        assert!(free_bytes(&std::env::temp_dir()).expect("statvfs") > 0);
    }

    #[test]
    fn the_resources_folders_are_the_servers() {
        let wanted: BTreeSet<String> = BTreeSet::new();
        let d = folder("resources");
        let at = |resources: &Path| prepare(&d, &wanted, &Configured::resources(resources));
        at(Path::new("resources")).expect("a missing folder is made");
        fs::create_dir_all(d.join("resources/slope/maps")).unwrap();
        fs::create_dir_all(d.join("resource-state/store")).unwrap();
        at(Path::new("resources")).expect("the default resources folder and the server's state");
        fs::create_dir_all(d.join("maps")).unwrap();
        assert!(at(Path::new("resources")).unwrap_err().contains("not a server's (maps)"));
        at(Path::new("maps")).expect("the configured folder");
        at(&d.join("maps")).expect("the configured folder, absolute");
        for elsewhere in [Path::new("maps/x"), Path::new("../maps"), Path::new("/srv/maps"), Path::new(".")] {
            assert!(at(elsewhere).unwrap_err().contains("not a server's (maps)"), "{elsewhere:?}");
        }
        fs::create_dir_all(d.join("rules")).unwrap();
        fs::write(d.join("my-blocklist.txt"), "").unwrap();
        let configured = |rules: &'static str, blocklist: &'static str| Configured {
            resources: Path::new("maps"),
            rules: Some(Path::new(rules)),
            blocklist: Some(Path::new(blocklist)),
        };
        prepare(&d, &wanted, &configured("rules", "my-blocklist.txt")).expect("the configured rules folder and list");
        let e = prepare(&d, &wanted, &configured("big", "lists/my-blocklist.txt")).unwrap_err();
        assert!(e.contains("not a server's (my-blocklist.txt, rules)"), "{e}");
        fs::remove_dir_all(&d).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn links_are_refused() {
        let wanted: BTreeSet<String> = BTreeSet::new();
        let r = &Configured::resources(Path::new(crate::resources::config::DEFAULT_FOLDER));
        let d = folder("links");
        fs::create_dir_all(d.join("real/big")).unwrap();
        std::os::unix::fs::symlink(d.join("real"), d.join("link")).unwrap();
        assert!(prepare(&d.join("link"), &wanted, r).unwrap_err().contains("is a link"));
        std::os::unix::fs::symlink("/etc", d.join("real/big/bootcache")).unwrap();
        assert!(prepare(&d.join("real"), &wanted, r).unwrap_err().contains("bootcache is a link"));
        fs::remove_dir_all(&d).unwrap();
    }
}
