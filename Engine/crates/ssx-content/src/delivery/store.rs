use std::collections::BTreeSet;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::hash::ContentHash;
use crate::path::check_package_path;

pub const UNCONFIRMED: &str = "unconfirmed";
pub const MOUNTED: &str = "mounted";
// PORT(choice): 16 slots, a port cap (one per running game; test clients on one machine use 2); the original loads no
// packages.
pub const MOUNT_SLOTS: usize = 16;
const PARTIAL_TRIES: usize = 8;

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("{path}: {source}")]
    Io { path: PathBuf, source: io::Error },
    #[error("{0} does not have the content its name says (removed)")]
    Mismatch(ContentHash),
    #[error("{hash}: {size} bytes, the manifest says {expected}")]
    Size { hash: ContentHash, size: u64, expected: u64 },
    #[error("{0} is not in the store")]
    Missing(ContentHash),
    #[error("{0} is being fetched by another program")]
    Busy(ContentHash),
    #[error("{0}")]
    Refused(String),
}

fn io_at(path: &Path) -> impl FnOnce(io::Error) -> StoreError + '_ {
    move |source| StoreError::Io { path: path.to_path_buf(), source }
}

fn files_err(e: ssx_files::Error) -> StoreError {
    StoreError::Io { path: e.path().to_path_buf(), source: e.into_io() }
}

fn open_plain(path: &Path) -> Result<Option<File>, StoreError> {
    match ssx_files::open_plain(path) {
        Ok(f) => Ok(f),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(files_err(e)),
    }
}

fn copy_hashing(from: &mut impl Read, to: &mut impl Write, max: u64) -> io::Result<(u64, ContentHash)> {
    let mut h = blake3::Hasher::new();
    let mut buf = vec![0u8; 1 << 20];
    let mut size = 0u64;
    loop {
        let n = from.read(&mut buf)?;
        if n == 0 {
            return Ok((size, ContentHash(*h.finalize().as_bytes())));
        }
        size += n as u64;
        if size > max {
            return Err(io::Error::new(io::ErrorKind::InvalidData, format!("more than {max} bytes")));
        }
        h.update(&buf[..n]);
        to.write_all(&buf[..n])?;
    }
}

fn touch(path: &Path) {
    let mut o = std::fs::OpenOptions::new();
    o.write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        o.access_mode(0x0100).custom_flags(0x0020_0000);
    }
    if let Ok(f) = o.open(path)
        && f.metadata().is_ok_and(|m| m.is_file())
    {
        let _ = f.set_modified(SystemTime::now());
    }
}

pub struct Partial {
    pub hash: ContentHash,
    file: File,
    path: PathBuf,
}

impl Partial {
    pub fn len(&self) -> Result<u64, StoreError> {
        self.file.metadata().map(|m| m.len()).map_err(io_at(&self.path))
    }

    pub fn is_empty(&self) -> Result<bool, StoreError> {
        self.len().map(|n| n == 0)
    }

    pub fn append(&mut self, bytes: &[u8]) -> Result<(), StoreError> {
        self.file.seek(SeekFrom::End(0)).and_then(|_| self.file.write_all(bytes)).map_err(io_at(&self.path))
    }

    pub fn restart(&mut self) -> Result<(), StoreError> {
        self.file.set_len(0).map_err(io_at(&self.path))
    }

    pub fn discard(self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

pub struct FileStore {
    root: PathBuf,
}

impl FileStore {
    pub fn open(root: &Path) -> Result<FileStore, StoreError> {
        for dir in [root.join(UNCONFIRMED), root.join(MOUNTED)] {
            ssx_files::create_dir_all(&dir).map_err(files_err)?;
        }
        Ok(FileStore { root: root.to_path_buf() })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn path_of(&self, hash: &ContentHash) -> PathBuf {
        self.root.join(hash.to_string())
    }

    fn open_entry(&self, hash: &ContentHash, size: u64) -> Result<Option<File>, StoreError> {
        let path = self.path_of(hash);
        let Some(file) = open_plain(&path)? else { return Ok(None) };
        if file.metadata().map_err(io_at(&path))?.len() != size {
            return Ok(None);
        }
        Ok(Some(file))
    }

    pub fn has(&self, hash: &ContentHash, size: u64) -> bool {
        matches!(self.open_entry(hash, size), Ok(Some(_)))
    }

    pub fn read(&self, hash: &ContentHash, size: u64) -> Result<Vec<u8>, StoreError> {
        let mut file = self.open_entry(hash, size)?.ok_or(StoreError::Missing(*hash))?;
        let mut out = Vec::with_capacity(usize::try_from(size).unwrap_or(0).min(1 << 26));
        let (n, h) = copy_hashing(&mut file, &mut out, size).map_err(io_at(&self.path_of(hash)))?;
        if n != size || h != *hash {
            self.remove(hash);
            return Err(StoreError::Mismatch(*hash));
        }
        touch(&self.path_of(hash));
        Ok(out)
    }

    fn remove(&self, hash: &ContentHash) {
        let _ = std::fs::remove_file(self.path_of(hash));
    }

    pub fn forget(&self, hash: &ContentHash) -> bool {
        let there = std::fs::symlink_metadata(self.path_of(hash)).is_ok_and(|m| m.is_file());
        self.remove(hash);
        there
    }

    fn partial_path(&self, hash: &ContentHash) -> PathBuf {
        self.root.join(UNCONFIRMED).join(format!("{hash}.part"))
    }

    pub fn partial_len(&self, hash: &ContentHash) -> Option<u64> {
        std::fs::symlink_metadata(self.partial_path(hash)).ok().filter(|m| m.is_file()).map(|m| m.len())
    }

    pub fn partial(&self, hash: &ContentHash) -> Result<Partial, StoreError> {
        let path = self.partial_path(hash);
        for _ in 0..PARTIAL_TRIES {
            if std::fs::symlink_metadata(&path).is_ok_and(|m| !m.is_file()) {
                return Err(StoreError::Refused(format!("{}: not a plain file", path.display())));
            }
            let mut o = std::fs::OpenOptions::new();
            o.read(true).write(true).create(true).truncate(false);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                o.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
            }
            let file = o.open(&path).map_err(io_at(&path))?;
            match ssx_files::try_lock_writers(&file) {
                Ok(()) => {}
                Err(std::fs::TryLockError::WouldBlock) => return Err(StoreError::Busy(*hash)),
                Err(std::fs::TryLockError::Error(e)) => return Err(StoreError::Io { path, source: e }),
            }
            let held = ssx_files::Identity::of_file(&file).map_err(io_at(&path))?;
            if ssx_files::Identity::of(&path).map_err(files_err)? == Some(held) {
                return Ok(Partial { hash: *hash, file, path });
            }
        }
        Err(StoreError::Busy(*hash))
    }

    pub fn confirm(&self, p: Partial, size: u64) -> Result<(), StoreError> {
        self.settle(p, size, false)
    }

    fn settle(&self, mut p: Partial, size: u64, put: bool) -> Result<(), StoreError> {
        let checked = match p.len() {
            Ok(n) if n != size => Ok((n, p.hash)),
            Ok(_) => p.file.seek(SeekFrom::Start(0)).and_then(|_| copy_hashing(&mut p.file, &mut io::sink(), size)),
            Err(e) => return Err(e),
        };
        let result = match checked {
            Ok((n, _)) if n != size => Err(StoreError::Size { hash: p.hash, size: n, expected: size }),
            Ok((_, h)) if h != p.hash => Err(StoreError::Mismatch(p.hash)),
            Ok(_) if put => p.file.sync_all().map_err(io_at(&p.path)),
            Ok(_) => Ok(()),
            Err(e) => Err(StoreError::Io { path: p.path.clone(), source: e }),
        };
        if let Err(e) = result {
            if put {
                p.discard();
            } else {
                let _ = p.restart();
            }
            return Err(e);
        }
        if put {
            ssx_files::rename_over(&p.path, &self.path_of(&p.hash)).map_err(files_err)?;
            return ssx_files::sync_folder(&self.root).map_err(files_err);
        }
        std::fs::rename(&p.path, self.path_of(&p.hash)).map_err(io_at(&p.path))
    }

    pub fn put(&self, hash: &ContentHash, size: u64, from: &mut impl Read) -> Result<(), StoreError> {
        if self.has(hash, size) {
            return Ok(());
        }
        let mut p = self.partial(hash)?;
        p.restart()?;
        let copied = copy_hashing(from, &mut p.file, size).map_err(io_at(&p.path));
        if let Err(e) = copied {
            p.discard();
            return Err(e);
        }
        self.settle(p, size, true)
    }

    pub fn entries(&self) -> Result<Vec<(ContentHash, u64, SystemTime)>, StoreError> {
        let mut out = Vec::new();
        for e in std::fs::read_dir(&self.root).map_err(io_at(&self.root))? {
            let e = e.map_err(io_at(&self.root))?;
            let Some(hash) = e.file_name().to_str().and_then(|n| n.parse::<ContentHash>().ok()) else { continue };
            let Ok(meta) = std::fs::symlink_metadata(e.path()) else { continue };
            if meta.is_file() {
                out.push((hash, meta.len(), meta.modified().unwrap_or(SystemTime::UNIX_EPOCH)));
            }
        }
        Ok(out)
    }

    pub fn bytes(&self) -> Result<u64, StoreError> {
        Ok(self.entries()?.iter().map(|e| e.1).sum())
    }

    fn partials(&self) -> Result<Vec<(ContentHash, u64, SystemTime)>, StoreError> {
        let dir = self.root.join(UNCONFIRMED);
        let mut out = Vec::new();
        for e in std::fs::read_dir(&dir).map_err(io_at(&dir))? {
            let e = e.map_err(io_at(&dir))?;
            let name = e.file_name();
            let Some(hash) = name.to_str().and_then(|n| n.strip_suffix(".part")?.parse::<ContentHash>().ok()) else {
                continue;
            };
            let Ok(meta) = std::fs::symlink_metadata(e.path()) else { continue };
            if meta.is_file() {
                out.push((hash, meta.len(), meta.modified().unwrap_or(SystemTime::UNIX_EPOCH)));
            }
        }
        Ok(out)
    }

    pub fn trim(&self, quota: u64, keep: &BTreeSet<ContentHash>) -> Result<(usize, u64), StoreError> {
        let entries = self.entries()?.into_iter().map(|(h, n, t)| (t, false, h, n));
        let mut all: Vec<_> = entries.chain(self.partials()?.into_iter().map(|(h, n, t)| (t, true, h, n))).collect();
        let mut total: u64 = all.iter().map(|e| e.3).sum();
        all.sort();
        let (mut files, mut bytes) = (0, 0);
        for (_, partial, hash, size) in all {
            if total <= quota {
                break;
            }
            if keep.contains(&hash) {
                continue;
            }
            if partial {
                let Ok(p) = self.partial(&hash) else { continue };
                p.discard();
            } else {
                self.remove(&hash);
            }
            total -= size;
            files += 1;
            bytes += size;
        }
        Ok((files, bytes))
    }

    pub fn mount(&self) -> Result<Mount, StoreError> {
        let dir = self.root.join(MOUNTED);
        for n in 0..MOUNT_SLOTS {
            let lock_path = dir.join(format!("{n}.lock"));
            let mut o = std::fs::OpenOptions::new();
            o.read(true).write(true).create(true).truncate(false);
            let lock = o.open(&lock_path).map_err(io_at(&lock_path))?;
            if ssx_files::try_lock_writers(&lock).is_err() {
                continue;
            }
            let slot = Mount { dir: dir.join(n.to_string()), _lock: lock };
            slot.clear()?;
            return Ok(slot);
        }
        Err(StoreError::Refused(format!("all {MOUNT_SLOTS} mount slots of {} are in use", dir.display())))
    }
}

pub struct Mount {
    dir: PathBuf,
    _lock: File,
}

fn link_or_copy(from: &Path, entry: File, to: &Path, size: u64) -> io::Result<()> {
    if std::fs::hard_link(from, to).is_ok() {
        return Ok(());
    }
    let mut out = std::fs::OpenOptions::new().write(true).create_new(true).open(to)?;
    let copied = io::copy(&mut entry.take(size), &mut out)?;
    if copied != size {
        return Err(io::Error::new(io::ErrorKind::UnexpectedEof, format!("{copied} of {size} bytes")));
    }
    Ok(())
}

impl Mount {
    fn clear(&self) -> Result<(), StoreError> {
        match std::fs::remove_dir_all(&self.dir) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(StoreError::Io { path: self.dir.clone(), source: e }),
        }
    }

    pub fn lay_out(
        &self,
        store: &FileStore,
        name: &str,
        files: &[(&str, ContentHash, u64)],
    ) -> Result<PathBuf, StoreError> {
        if !crate::id::valid_namespace(name) {
            return Err(StoreError::Refused(format!("{name:?} is not a resource name")));
        }
        let dest = self.dir.join(name);
        if std::fs::symlink_metadata(&dest).is_ok() {
            std::fs::remove_dir_all(&dest).map_err(io_at(&dest))?;
        }
        for (rel, hash, size) in files {
            check_package_path(rel, false).map_err(StoreError::Refused)?;
            let to = rel.split('/').fold(dest.clone(), |p, seg| p.join(seg));
            if let Some(parent) = to.parent() {
                std::fs::create_dir_all(parent).map_err(io_at(parent))?;
            }
            let entry = store.open_entry(hash, *size)?.ok_or(StoreError::Missing(*hash))?;
            link_or_copy(&store.path_of(hash), entry, &to, *size).map_err(io_at(&to))?;
            let mut file = open_plain(&to)?.ok_or(StoreError::Missing(*hash))?;
            let (n, h) = copy_hashing(&mut file, &mut io::sink(), *size).map_err(io_at(&to))?;
            if n != *size || h != *hash {
                store.remove(hash);
                return Err(StoreError::Mismatch(*hash));
            }
            touch(&store.path_of(hash));
        }
        Ok(dest)
    }

    pub fn take_down(&self, name: &str) -> Result<(), StoreError> {
        if !crate::id::valid_namespace(name) {
            return Err(StoreError::Refused(format!("{name:?} is not a resource name")));
        }
        let dest = self.dir.join(name);
        match std::fs::remove_dir_all(&dest) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(StoreError::Io { path: dest, source: e }),
        }
    }
}

impl Drop for Mount {
    fn drop(&mut self) {
        let _ = self.clear();
    }
}
