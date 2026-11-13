use std::collections::BTreeSet;
use std::io::Read;
use std::path::{Path, PathBuf};

use super::zip::ZipFile;

pub const MAX_WALK_DEPTH: usize = 6;
pub const MAX_WALK_FILES: usize = 4096;

pub struct Staging(PathBuf);

impl Staging {
    pub fn new(root: &Path) -> Result<Self, String> {
        let base = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
        for n in 0..32 {
            let path = root.join(format!(".install-{}-{base}-{n}", std::process::id()));
            match std::fs::create_dir(&path) {
                Ok(()) => return Ok(Staging(path)),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(e) => return Err(format!("{}: {e}", path.display())),
            }
        }
        Err(format!("{}: no staging folder could be made", root.display()))
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Staging {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

pub fn is_content_package(magic: &[u8]) -> bool {
    matches!(magic.get(..4), Some(b"LIVE" | b"CON " | b"PIRS"))
}

pub fn candidates(
    source: &Path,
    staging: &Path,
    max_bytes: u64,
    skipped: &mut Vec<(String, String)>,
) -> Result<Vec<(String, PathBuf)>, String> {
    let meta = std::fs::symlink_metadata(source).map_err(|e| format!("{}: {e}", source.display()))?;
    let mut out = Vec::new();
    if meta.is_dir() {
        let mut files = Vec::new();
        walk(source, 0, &mut files)?;
        for path in files {
            let name = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
            out.push((name, path));
        }
    } else if meta.is_file() {
        let mut magic = [0u8; 4];
        let read =
            std::fs::File::open(source).map_err(|e| format!("{}: {e}", source.display()))?.read_exact(&mut magic);
        if read.is_ok() && is_content_package(&magic) {
            let name = source.file_name().unwrap_or_default().to_string_lossy().into_owned();
            out.push((name, source.to_path_buf()));
        } else {
            let mut zip = ZipFile::open(source)?;
            let entries: Vec<(usize, String, u64)> = zip
                .entries()
                .iter()
                .enumerate()
                .filter(|(_, e)| !e.folder)
                .map(|(i, e)| (i, e.name.clone(), e.size))
                .collect();
            for (i, name, size) in entries {
                let base = name.rsplit('/').next().unwrap_or(&name).to_string();
                if size > max_bytes {
                    skipped.push((name, format!("{size} bytes (a package is at most {max_bytes})")));
                    continue;
                }
                let staged = staging.join("in").join(format!("{i:04}"));
                if let Some(parent) = staged.parent() {
                    ssx_files::create_dir_all(parent).map_err(|e| e.to_string())?;
                }
                let mut file = std::fs::File::create(&staged).map_err(|e| format!("{}: {e}", staged.display()))?;
                zip.extract(i, &mut file)?;
                drop(file);
                out.push((base, staged));
            }
        }
    } else {
        return Err(format!("{}: not a file or folder", source.display()));
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    let mut seen = BTreeSet::new();
    out.retain(|(name, _)| {
        if seen.insert(name.to_ascii_lowercase()) {
            true
        } else {
            skipped.push((name.clone(), "another file of this name is in the source".into()));
            false
        }
    });
    Ok(out)
}

fn walk(root: &Path, depth: usize, out: &mut Vec<PathBuf>) -> Result<(), String> {
    if depth > MAX_WALK_DEPTH {
        return Ok(());
    }
    let read = std::fs::read_dir(root).map_err(|e| format!("{}: {e}", root.display()))?;
    for entry in read.flatten() {
        let path = entry.path();
        let kind = entry.file_type().map_err(|e| format!("{}: {e}", path.display()))?;
        if kind.is_dir() {
            walk(&path, depth + 1, out)?;
        } else if kind.is_file() {
            if out.len() >= MAX_WALK_FILES {
                return Err(format!("{}: more than {MAX_WALK_FILES} files", root.display()));
            }
            out.push(path);
        }
    }
    Ok(())
}

pub enum Candidate {
    Package(Vec<u8>),
    Skip(String),
}

pub fn read_package(path: &Path, max_bytes: u64) -> Result<Candidate, String> {
    let mut file = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let size = file.metadata().map_err(|e| format!("{}: {e}", path.display()))?.len();
    if size > max_bytes {
        return Ok(Candidate::Skip(format!("{size} bytes (a package is at most {max_bytes})")));
    }
    let mut magic = [0u8; 4];
    if file.read_exact(&mut magic).is_err() || !is_content_package(&magic) {
        return Ok(Candidate::Skip("not an Xbox 360 content package".into()));
    }
    let capacity = usize::try_from(size).map_err(|_| format!("{}: {size} bytes", path.display()))?;
    let mut bytes = Vec::with_capacity(capacity);
    bytes.extend_from_slice(&magic);
    (&mut file).take(max_bytes + 1 - 4).read_to_end(&mut bytes).map_err(|e| format!("{}: {e}", path.display()))?;
    if bytes.len() as u64 != size {
        return Err(format!("{}: changed while it was read ({size} bytes, then {})", path.display(), bytes.len()));
    }
    Ok(Candidate::Package(bytes))
}

pub fn read_file_at_most(path: &Path, cap: u64) -> Result<Vec<u8>, String> {
    let meta = std::fs::symlink_metadata(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if !meta.is_file() || meta.len() > cap {
        return Err(format!("{}: not a file of at most {cap} bytes", path.display()));
    }
    let file = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut bytes = Vec::with_capacity(usize::try_from(meta.len()).unwrap_or(0));
    file.take(cap.saturating_add(1)).read_to_end(&mut bytes).map_err(|e| format!("{}: {e}", path.display()))?;
    if bytes.len() as u64 > cap {
        return Err(format!("{}: more than {cap} bytes", path.display()));
    }
    Ok(bytes)
}

pub fn installed_folder(root: &Path, folder: &str) -> Option<String> {
    let read = std::fs::read_dir(root).ok()?;
    read.flatten().find_map(|e| {
        let name = e.file_name().to_string_lossy().into_owned();
        (e.path().is_dir() && name.eq_ignore_ascii_case(folder)).then_some(name)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_candidate_is_read_only_when_its_size_and_magic_say_it_is_a_package() {
        let dir = std::env::temp_dir().join(format!("ssx-intake-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let write = |name: &str, bytes: &[u8]| {
            let path = dir.join(name);
            std::fs::write(&path, bytes).unwrap();
            path
        };
        let package = write("pkg", b"LIVE and the rest");
        match read_package(&package, 64).unwrap() {
            Candidate::Package(bytes) => assert_eq!(bytes, b"LIVE and the rest"),
            Candidate::Skip(why) => panic!("{why}"),
        }
        assert!(matches!(read_package(&package, 8).unwrap(), Candidate::Skip(why) if why.contains("at most 8")));
        assert!(matches!(read_package(&write("txt", b"hello world"), 64).unwrap(), Candidate::Skip(_)));
        assert!(matches!(read_package(&write("short", b"LI"), 64).unwrap(), Candidate::Skip(_)));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_staging_folder_is_removed_when_dropped() {
        let dir = std::env::temp_dir().join(format!("ssx-staging-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = {
            let staging = Staging::new(&dir).unwrap();
            std::fs::write(staging.path().join("x"), b"x").unwrap();
            staging.path().to_path_buf()
        };
        assert!(!path.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
