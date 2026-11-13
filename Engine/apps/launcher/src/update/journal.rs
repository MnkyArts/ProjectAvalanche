use std::fs::{File, OpenOptions, TryLockError};
use std::io::{ErrorKind, Read};
use std::path::{Path, PathBuf};
use std::time::Duration;

use sha2::{Digest, Sha256};
use ssx_files::{Identity, Options};

use super::install::{FONTS, allowed_files, folder_of, launcher_name, path_of};
use super::{is_staging_name, private_dir};

pub(crate) const BACKUP: &str = ".update-backup";
pub(crate) const LEGACY_BACKUP: &str = ".update-backup-";
const LOCK: &str = "lock";
const JOURNAL: &str = "journal";
const COMMITTED: &str = "committed";
const MAGIC: &str = "project-avalanche update journal 1";
const MAX_JOURNAL: u64 = 4096;
// PORT(choice): 20 tries 100 ms apart (2 s), the updater 0.2.0's own choice, for the exited launcher's image to be
// unmapped and a virus scanner to let go of a new file; tuning: time both on Windows with Defender on (the helper
// logging each try it needs); the original has no launcher.
const TRIES: u32 = 20;
const TRY_WAIT: Duration = Duration::from_millis(100);

type Sha = [u8; 32];

#[derive(Clone, Debug, PartialEq, Eq)]
struct Entry {
    name: String,
    old: Option<Sha>,
    new: Sha,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Journal {
    staged: String,
    made: Vec<String>,
    entries: Vec<Entry>,
}

impl Journal {
    fn text(&self) -> String {
        let mut text = format!("{MAGIC}\nstaged {}\n", self.staged);
        for folder in &self.made {
            text.push_str(&format!("folder {folder}\n"));
        }
        for e in &self.entries {
            let old = e.old.as_ref().map_or_else(|| "-".to_string(), hex);
            text.push_str(&format!("file {} {old} {}\n", hex(&e.new), e.name));
        }
        text
    }

    fn parse(text: &str) -> Result<Journal, String> {
        let body = text.strip_suffix('\n').ok_or("it does not end with a line break")?;
        let mut lines = body.split('\n');
        if lines.next() != Some(MAGIC) {
            return Err("its first line is not this launcher's".into());
        }
        let staged = lines.next().and_then(|l| l.strip_prefix("staged ")).ok_or("it names no staged folder")?;
        if !is_staging_name(staged) {
            return Err(format!("its staged folder {staged:?} is not one the launcher makes"));
        }
        let mut made: Vec<String> = Vec::new();
        let mut entries: Vec<Entry> = Vec::new();
        for line in lines {
            if let Some(folder) = line.strip_prefix("folder ") {
                if folder != FONTS || !made.is_empty() || !entries.is_empty() {
                    return Err(format!("it has a folder line the launcher does not write: {line:?}"));
                }
                made.push(folder.to_string());
                continue;
            }
            let mut parts = line.splitn(4, ' ');
            let (Some("file"), Some(new), Some(old), Some(name)) =
                (parts.next(), parts.next(), parts.next(), parts.next())
            else {
                return Err(format!("it has a line the launcher does not write: {line:?}"));
            };
            if !allowed_files().contains(&name) {
                return Err(format!("it names {name:?}, which is not a file the launcher may replace"));
            }
            if entries.iter().any(|e| e.name == name) {
                return Err(format!("it names {name} twice"));
            }
            let new = unhex(new).ok_or_else(|| format!("{name}'s new SHA-256 does not read"))?;
            let old = match old {
                "-" => None,
                old => Some(unhex(old).ok_or_else(|| format!("{name}'s old SHA-256 does not read"))?),
            };
            entries.push(Entry { name: name.to_string(), old, new });
        }
        if entries.last().map(|e| e.name.as_str()) != Some(launcher_name()) {
            return Err("it does not end with the launcher".into());
        }
        Ok(Journal { staged: staged.to_string(), made, entries })
    }
}

fn hex(sha: &Sha) -> String {
    sha.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex(text: &str) -> Option<Sha> {
    if text.len() != 64 || !text.bytes().all(|c| matches!(c, b'0'..=b'9' | b'a'..=b'f')) {
        return None;
    }
    let mut sha = [0u8; 32];
    for (i, b) in sha.iter_mut().enumerate() {
        *b = u8::from_str_radix(&text[2 * i..2 * i + 2], 16).ok()?;
    }
    Some(sha)
}

fn read_journal(dir: &Path) -> Result<Option<Journal>, String> {
    let path = dir.join(JOURNAL);
    let file = match ssx_files::open_plain(&path) {
        Ok(Some(f)) => f,
        Ok(None) => return Err(format!("{} is not a plain file", path.display())),
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.to_string()),
    };
    let mut bytes = Vec::new();
    file.take(MAX_JOURNAL + 1).read_to_end(&mut bytes).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    if bytes.len() as u64 > MAX_JOURNAL {
        return Err(format!("{} is longer than a journal", path.display()));
    }
    let text = String::from_utf8(bytes).map_err(|_| format!("{} is not text", path.display()))?;
    Journal::parse(&text).map(Some).map_err(|e| format!("{} is not a journal of this launcher: {e}", path.display()))
}

fn content(path: &Path) -> Result<Option<Sha>, String> {
    let mut file = match retried(|| ssx_files::open_plain(path)) {
        Ok(Some(f)) => f,
        Ok(None) => return Err(format!("{} is not a plain file", path.display())),
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.to_string()),
    };
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        match file.read(&mut buf) {
            Ok(0) => return Ok(Some(hasher.finalize().into())),
            Ok(n) => hasher.update(&buf[..n]),
            Err(e) if e.kind() == ErrorKind::Interrupted => {}
            Err(e) => return Err(format!("cannot read {}: {e}", path.display())),
        }
    }
}

trait SystemError {
    fn system(&self) -> &std::io::Error;
}

impl SystemError for ssx_files::Error {
    fn system(&self) -> &std::io::Error {
        self.io()
    }
}

impl SystemError for std::io::Error {
    fn system(&self) -> &std::io::Error {
        self
    }
}

fn retried<T, E: SystemError>(mut step: impl FnMut() -> Result<T, E>) -> Result<T, E> {
    let mut tries = 1;
    loop {
        match step() {
            Err(e) if tries < TRIES && held_open(e.system()) => {
                tries += 1;
                std::thread::sleep(TRY_WAIT);
            }
            done => return done,
        }
    }
}

#[cfg(windows)]
fn held_open(e: &std::io::Error) -> bool {
    use windows_sys::Win32::Foundation::{ERROR_LOCK_VIOLATION, ERROR_SHARING_VIOLATION};
    e.kind() == ErrorKind::PermissionDenied
        || e.raw_os_error().is_some_and(|c| c == ERROR_SHARING_VIOLATION as i32 || c == ERROR_LOCK_VIOLATION as i32)
}

#[cfg(not(windows))]
fn held_open(_: &std::io::Error) -> bool {
    false
}

struct Lock {
    file: File,
    path: PathBuf,
}

impl Lock {
    fn create(dir: &Path) -> Result<Lock, String> {
        let path = dir.join(LOCK);
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|e| format!("cannot make {}: {e}", path.display()))?;
        match file.try_lock() {
            Ok(()) => Ok(Lock { file, path }),
            Err(TryLockError::WouldBlock) => Err(format!("{} is held by another process", path.display())),
            Err(TryLockError::Error(e)) => Err(format!("cannot lock {}: {e}", path.display())),
        }
    }

    fn take(dir: &Path) -> std::io::Result<Option<Lock>> {
        let path = dir.join(LOCK);
        let file = match ssx_files::open_plain(&path).map_err(ssx_files::Error::into_io) {
            Ok(Some(f)) => f,
            Ok(None) => return Err(std::io::Error::new(ErrorKind::InvalidData, "the lock is not a plain file")),
            Err(e) if removed_while_open(&e) => return Ok(None),
            Err(e) => return Err(e),
        };
        match file.try_lock() {
            Ok(()) => {}
            Err(TryLockError::WouldBlock) => return Ok(None),
            Err(TryLockError::Error(e)) => return Err(e),
        }
        match Identity::of(&path).map_err(ssx_files::Error::into_io) {
            Ok(now) if now == Some(Identity::of_file(&file)?) => Ok(Some(Lock { file, path })),
            Ok(_) => Ok(None),
            Err(e) if removed_while_open(&e) || e.kind() == ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
        }
    }

    fn release(self) -> Result<(), String> {
        let removed = std::fs::remove_file(&self.path);
        drop(self.file);
        removed.map_err(|e| format!("cannot remove {}: {e}", self.path.display()))
    }
}

#[cfg(windows)]
fn removed_while_open(e: &std::io::Error) -> bool {
    e.kind() == ErrorKind::PermissionDenied
}

#[cfg(not(windows))]
fn removed_while_open(_: &std::io::Error) -> bool {
    false
}

#[cfg(test)]
pub(crate) mod crash {
    use std::cell::Cell;

    thread_local! {
        static AT: Cell<Option<(usize, bool)>> = const { Cell::new(None) };
        static PASSED: Cell<usize> = const { Cell::new(0) };
    }

    pub fn crash_at(n: Option<usize>) {
        AT.set(n.map(|n| (n, true)));
        PASSED.set(0);
    }

    pub fn fail_at(n: usize) {
        AT.set(Some((n, false)));
        PASSED.set(0);
    }

    pub(super) fn now() -> bool {
        let n = PASSED.get();
        PASSED.set(n + 1);
        match AT.get() {
            Some((at, true)) => n >= at,
            Some((at, false)) => n == at,
            None => false,
        }
    }

    pub fn happened() -> bool {
        AT.get().is_some_and(|(at, crash)| crash && PASSED.get() > at)
    }

    pub fn reached() -> bool {
        AT.get().is_some_and(|(at, _)| PASSED.get() > at)
    }
}

fn point() -> Result<(), String> {
    #[cfg(test)]
    if crash::now() {
        return Err("stopped here by a test".into());
    }
    Ok(())
}

fn crashed() -> bool {
    #[cfg(test)]
    {
        crash::happened()
    }
    #[cfg(not(test))]
    {
        false
    }
}

fn also(first: String, cleanup: Result<(), String>) -> String {
    match cleanup {
        Ok(()) => first,
        Err(e) => format!("{first}; then {e}"),
    }
}

fn ordered(files: &[String]) -> Result<Vec<String>, String> {
    let mut names = Vec::new();
    for (i, f) in files.iter().enumerate() {
        if !allowed_files().contains(&f.as_str()) {
            return Err(format!("refusing to replace {f}"));
        }
        if files[..i].contains(f) {
            return Err(format!("the update names {f} twice"));
        }
        if f != launcher_name() {
            names.push(f.clone());
        }
    }
    if !files.iter().any(|f| f == launcher_name()) {
        return Err(format!("the update has no {}", launcher_name()));
    }
    names.push(launcher_name().to_string());
    Ok(names)
}

pub(crate) fn install(staged: &Path, install: &Path, files: &[String]) -> Result<Option<String>, String> {
    let names = ordered(files)?;
    let staged_name = staged
        .file_name()
        .and_then(|n| n.to_str())
        .filter(|n| is_staging_name(n) && staged.parent() == Some(install))
        .ok_or("the update folder is not one the launcher made beside it")?;
    let (dir, lock) = claim(install)?;
    let journal = (|| -> Result<Journal, String> {
        point()?;
        let mut made = Vec::new();
        for folder in folders(&names) {
            let at = install.join(folder);
            match std::fs::symlink_metadata(&at) {
                Ok(meta) if meta.is_dir() => {}
                Ok(_) => return Err(format!("{} is not a plain folder; the update leaves it as it is", at.display())),
                Err(e) if e.kind() == ErrorKind::NotFound => made.push(folder.to_string()),
                Err(e) => return Err(format!("cannot read {}: {e}", at.display())),
            }
        }
        let mut entries = Vec::new();
        for name in &names {
            let new = content(&path_of(staged, name))?.ok_or_else(|| format!("the update has no {name}"))?;
            entries.push(Entry { name: name.clone(), old: content(&path_of(install, name))?, new });
        }
        let journal = Journal { staged: staged_name.to_string(), made, entries };
        ssx_files::create(&dir.join(JOURNAL), journal.text().as_bytes(), &Options::owner_only())
            .map_err(|e| e.to_string())?;
        point()?;
        Ok(journal)
    })();
    let journal = match journal {
        Ok(journal) => journal,
        Err(e) if crashed() => return Err(e),
        Err(e) => return Err(also(e, finish(&dir, lock, &[]))),
    };
    let changed = (|| -> Result<(), String> {
        for folder in &journal.made {
            let at = install.join(folder);
            std::fs::create_dir(&at).map_err(|e| format!("cannot make {}: {e}", at.display()))?;
            ssx_files::sync_folder(install).map_err(|e| e.to_string())?;
            point()?;
        }
        for folder in folders(&names) {
            let at = dir.join(folder);
            std::fs::create_dir(&at).map_err(|e| format!("cannot make {}: {e}", at.display()))?;
            point()?;
        }
        for e in &journal.entries {
            let (new, now, kept) = (path_of(staged, &e.name), path_of(install, &e.name), path_of(&dir, &e.name));
            match e.old {
                Some(old) => {
                    retried(|| ssx_files::keep_copy(&now, &kept))
                        .map_err(|err| format!("cannot keep the old {}: {err}", e.name))?;
                    point()?;
                    if content(&kept)? != Some(old) {
                        return Err(format!("{} changed while the update was being installed", now.display()));
                    }
                    retried(|| ssx_files::rename_over(&new, &now))
                }
                None => retried(|| ssx_files::rename_new(&new, &now)),
            }
            .map_err(|err| format!("cannot put the new {} in place: {err}", e.name))?;
            point()?;
        }
        ssx_files::create(&dir.join(COMMITTED), b"", &Options::owner_only()).map_err(|e| e.to_string())?;
        point()
    })();
    match changed {
        Ok(()) => Ok(finish(&dir, lock, &journal.entries)
            .err()
            .map(|e| format!("the update is installed, but {e}; the next start of the launcher removes it"))),
        Err(e) if crashed() => Err(e),
        Err(e) if there(&dir.join(COMMITTED)) => {
            let note = format!("the update is installed, but {e} after its commit");
            Ok(Some(also(note, finish(&dir, lock, &journal.entries))))
        }
        Err(e) => match roll_back(install, &dir, &journal) {
            Ok(()) => Err(also(format!("{e}; the old files are back"), finish(&dir, lock, &journal.entries))),
            Err(r) => Err(format!(
                "{e}; putting the old files back stopped ({r}): the next start of the launcher finishes it from {}",
                dir.display()
            )),
        },
    }
}

fn folders(names: &[String]) -> Vec<&str> {
    let mut out: Vec<&str> = Vec::new();
    for folder in names.iter().filter_map(|n| folder_of(n)) {
        if !out.contains(&folder) {
            out.push(folder);
        }
    }
    out
}

fn claim(install: &Path) -> Result<(PathBuf, Lock), String> {
    let dir = install.join(BACKUP);
    match private_dir(&dir) {
        Ok(()) => {}
        Err(e) if e.kind() == ErrorKind::AlreadyExists => {
            return Err(format!(
                "an earlier update did not finish, or another one is being installed ({}); start the launcher again \
                 to finish it",
                dir.display()
            ));
        }
        Err(e) => return Err(format!("cannot make the backup folder {}: {e}", dir.display())),
    }
    let lock = (|| {
        ssx_files::sync_folder(install).map_err(|e| e.to_string())?;
        point()?;
        Lock::create(&dir)
    })();
    match lock {
        Ok(lock) => Ok((dir, lock)),
        Err(e) if crashed() => Err(e),
        Err(e) => Err(also(e, remove_empty(&dir))),
    }
}

fn remove_empty(dir: &Path) -> Result<(), String> {
    match std::fs::remove_dir(dir) {
        Ok(()) => {}
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(format!("cannot remove {}: {e}", dir.display())),
    }
    match dir.parent() {
        Some(parent) => ssx_files::sync_folder(parent).map_err(|e| e.to_string()),
        None => Ok(()),
    }
}

fn there(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok()
}

fn remove_if_there(path: &Path) -> Result<(), String> {
    match retried(|| std::fs::remove_file(path)) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("cannot remove {}: {e}", path.display())),
    }
}

fn finish(dir: &Path, lock: Lock, entries: &[Entry]) -> Result<(), String> {
    for e in entries {
        remove_if_there(&path_of(dir, &e.name))?;
        point()?;
    }
    let fonts = dir.join(FONTS);
    for folder in [dir, fonts.as_path()] {
        let listing = match std::fs::read_dir(folder) {
            Ok(listing) => listing,
            Err(e) if e.kind() == ErrorKind::NotFound && folder != dir => continue,
            Err(e) => return Err(format!("cannot list {}: {e}", folder.display())),
        };
        for entry in listing {
            let entry = entry.map_err(|e| format!("cannot list {}: {e}", folder.display()))?;
            if entry.file_name().to_str().is_some_and(|n| ssx_files::partial_target(n).is_some()) {
                remove_if_there(&entry.path())?;
            }
        }
    }
    match std::fs::remove_dir(&fonts) {
        Ok(()) => point()?,
        Err(e) if e.kind() == ErrorKind::NotFound => {}
        Err(e) => return Err(format!("cannot remove {}: {e}", fonts.display())),
    }
    remove_if_there(&dir.join(JOURNAL))?;
    point()?;
    remove_if_there(&dir.join(COMMITTED))?;
    lock.release()?;
    point()?;
    remove_empty(dir)
}

fn roll_back(install: &Path, dir: &Path, journal: &Journal) -> Result<(), String> {
    for e in journal.entries.iter().rev() {
        let (now, kept) = (path_of(install, &e.name), path_of(dir, &e.name));
        match (e.old, content(&now)?) {
            (Some(old), Some(held)) if held == old => remove_if_there(&kept)?,
            (Some(old), Some(held)) if held == e.new => put_back(&kept, &now, old, true)?,
            (Some(old), None) => put_back(&kept, &now, old, false)?,
            (None, Some(held)) if held == e.new => {
                remove_if_there(&now)?;
                ssx_files::sync_folder(now.parent().unwrap_or(install)).map_err(|err| err.to_string())?;
            }
            (None, None) => {}
            _ => {
                return Err(format!(
                    "{} is neither the version from before the update nor the update's; it and {} are left as they are",
                    now.display(),
                    dir.display()
                ));
            }
        }
        point()?;
    }
    for folder in journal.made.iter().rev() {
        let at = install.join(folder);
        match std::fs::remove_dir(&at) {
            Ok(()) => ssx_files::sync_folder(install).map_err(|e| e.to_string())?,
            Err(e) if matches!(e.kind(), ErrorKind::NotFound | ErrorKind::DirectoryNotEmpty) => {}
            Err(e) => return Err(format!("cannot remove {}: {e}", at.display())),
        }
        point()?;
    }
    Ok(())
}

fn put_back(kept: &Path, now: &Path, old: Sha, over: bool) -> Result<(), String> {
    match content(kept)? {
        Some(held) if held == old => {}
        Some(_) => return Err(format!("the backup {} is not the old version; nothing is put back", kept.display())),
        None => return Err(format!("{} has no backup in {}", now.display(), kept.display())),
    }
    if over { retried(|| ssx_files::rename_over(kept, now)) } else { retried(|| ssx_files::rename_new(kept, now)) }
        .map_err(|e| format!("cannot put the old {} back: {e}", now.display()))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Recovered {
    Nothing,
    Busy,
    Cleared,
    Finished,
    RolledBack,
}

pub(crate) fn recover(install: &Path) -> Result<Recovered, String> {
    let dir = install.join(BACKUP);
    match std::fs::symlink_metadata(&dir) {
        Ok(meta) if meta.is_dir() => {}
        Ok(_) => return Err(format!("{} is not a folder; it is left as it is", dir.display())),
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(Recovered::Nothing),
        Err(e) => return Err(format!("cannot read {}: {e}", dir.display())),
    }
    let lock = match Lock::take(&dir) {
        Ok(Some(lock)) => lock,
        Ok(None) => return Ok(Recovered::Busy),
        Err(e) if e.kind() == ErrorKind::NotFound => {
            return remove_empty(&dir)
                .map(|()| Recovered::Cleared)
                .map_err(|e| format!("{e}: it holds files but no lock; it is left as it is"));
        }
        Err(e) => return Err(format!("cannot take the lock in {}: {e}", dir.display())),
    };
    let Some(journal) = read_journal(&dir)? else {
        for entry in std::fs::read_dir(&dir).map_err(|e| format!("cannot list {}: {e}", dir.display()))? {
            let name = entry.map_err(|e| format!("cannot list {}: {e}", dir.display()))?.file_name();
            let ours = name.to_str().is_some_and(|n| {
                matches!(n, LOCK | COMMITTED) || ssx_files::partial_target(n).is_some_and(|t| t == JOURNAL)
            });
            if !ours {
                return Err(format!("{} holds {name:?} but no journal; it is left as it is", dir.display()));
            }
        }
        finish(&dir, lock, &[])?;
        return Ok(Recovered::Cleared);
    };
    let outcome = if there(&dir.join(COMMITTED)) {
        Recovered::Finished
    } else if all_new(install, &journal)? {
        ssx_files::create(&dir.join(COMMITTED), b"", &Options::owner_only()).map_err(|e| e.to_string())?;
        point()?;
        Recovered::Finished
    } else {
        roll_back(install, &dir, &journal)?;
        Recovered::RolledBack
    };
    finish(&dir, lock, &journal.entries)?;
    Ok(outcome)
}

fn all_new(install: &Path, journal: &Journal) -> Result<bool, String> {
    for e in &journal.entries {
        if content(&path_of(install, &e.name))? != Some(e.new) {
            return Ok(false);
        }
    }
    Ok(true)
}

pub(crate) fn recover_legacy(install: &Path) -> Vec<String> {
    let mut notes = Vec::new();
    let Ok(entries) = std::fs::read_dir(install) else {
        return notes;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        let dir = entry.path();
        let folder = std::fs::symlink_metadata(&dir).is_ok_and(|m| m.is_dir());
        if !name.starts_with(LEGACY_BACKUP) || !folder {
            continue;
        }
        let files: Vec<_> = match std::fs::read_dir(&dir) {
            Ok(files) => files.flatten().map(|f| f.file_name()).collect(),
            Err(e) => {
                notes.push(format!("cannot list {}: {e}; it is left as it is", dir.display()));
                continue;
            }
        };
        for file in files {
            let Some(file) = file.to_str().filter(|f| allowed_files().contains(f)) else {
                notes.push(format!("{} holds {file:?}, which the updater did not put there; kept", dir.display()));
                continue;
            };
            let (kept, now) = (dir.join(file), install.join(file));
            if there(&now) {
                notes.push(format!("{} kept: the install holds a {file} too", kept.display()));
                continue;
            }
            match ssx_files::rename_new(&kept, &now) {
                Ok(()) => notes.push(format!("{file} put back from {} (an update 0.2.0 cut short)", dir.display())),
                Err(e) => notes.push(format!("cannot put {file} back from {}: {e}; kept", dir.display())),
            }
        }
        let holds = std::fs::read_dir(&dir).map(|mut left| left.next().is_some());
        if !matches!(holds, Ok(true))
            && let Err(e) = remove_empty(&dir)
        {
            notes.push(e);
        }
    }
    notes
}

#[cfg(test)]
mod tests;
