use std::collections::BTreeSet;
use std::ffi::{OsStr, OsString};
use std::fs::{File, TryLockError};
use std::io;
use std::ops::Range;
use std::sync::{Mutex, MutexGuard, PoisonError};

use crate::folder::Folder;
use crate::sys::{self, FileId};

// PORT(choice): 8 writers of one file at once (each program has one writer per file; two only when a program runs twice
// or two installs meet); tuning: a program that writes one file from more than 8 threads or processes at once; the
// port's own file layer.
pub const SLOTS: u32 = 8;
const SUFFIX: &str = ".partial";

pub fn slot_name(target: &OsStr, slot: u32) -> OsString {
    let mut name = target.to_os_string();
    name.push(SUFFIX);
    if slot > 0 {
        name.push(format!("-{slot}"));
    }
    name
}

pub fn partial_target(name: &str) -> Option<&str> {
    let (target, slot) = name.rsplit_once(SUFFIX)?;
    let ok = slot.is_empty()
        || slot
            .strip_prefix('-')
            .is_some_and(|n| n.len() == 1 && n.parse::<u32>().is_ok_and(|n| (1..SLOTS).contains(&n)));
    (ok && !target.is_empty()).then_some(target)
}

static HELD: Mutex<BTreeSet<FileId>> = Mutex::new(BTreeSet::new());

fn held() -> MutexGuard<'static, BTreeSet<FileId>> {
    HELD.lock().unwrap_or_else(PoisonError::into_inner)
}

pub struct Claim<F: Folder> {
    pub folder: F,
    pub name: OsString,
    pub file: File,
    id: FileId,
    keep: bool,
}

impl<F: Folder> Claim<F> {
    pub fn done(&mut self) {
        self.keep = true;
    }
}

impl<F: Folder> Drop for Claim<F> {
    fn drop(&mut self) {
        if !self.keep {
            let _ = self.folder.remove(&self.name);
        }
        let _ = sys::unlock_writers(&self.file);
        held().remove(&self.id);
    }
}

enum Slot<F: Folder> {
    Claimed(Claim<F>),
    Freed,
    Busy,
}

pub fn claim<F: Folder + Clone>(
    folder: &F,
    target: &OsStr,
    mode: u32,
    slots: Range<u32>,
) -> io::Result<Option<Claim<F>>> {
    for slot in slots {
        let name = slot_name(target, slot);
        for _ in 0..2 {
            match take(folder, &name, mode)? {
                Slot::Claimed(c) => return Ok(Some(c)),
                Slot::Freed => {}
                Slot::Busy => break,
            }
        }
    }
    Ok(None)
}

fn take<F: Folder + Clone>(folder: &F, name: &OsStr, mode: u32) -> io::Result<Slot<F>> {
    let mut ids = held();
    let file = match folder.create_new(name, mode) {
        Ok(f) => f,
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
            return Ok(if reclaim(folder, name, &ids) { Slot::Freed } else { Slot::Busy });
        }
        Err(e) if cfg!(windows) && e.kind() == io::ErrorKind::PermissionDenied => return Ok(Slot::Busy),
        Err(e) => return Err(e),
    };
    let id = match sys::id(&file) {
        Ok(id) => id,
        Err(e) => {
            let _ = folder.remove(name);
            return Err(e);
        }
    };
    ids.insert(id);
    drop(ids);
    let mut claim = Claim { folder: folder.clone(), name: name.to_os_string(), file, id, keep: false };
    match sys::try_lock_writers(&claim.file) {
        Ok(()) => {}
        Err(TryLockError::WouldBlock) => {
            claim.done();
            return Ok(Slot::Busy);
        }
        Err(TryLockError::Error(_)) => {}
    }
    if folder.id_of(name).ok().flatten() != Some(id) {
        claim.done();
        return Ok(Slot::Busy);
    }
    Ok(Slot::Claimed(claim))
}

fn reclaim<F: Folder>(folder: &F, name: &OsStr, ids: &BTreeSet<FileId>) -> bool {
    let file = match folder.open_regular(name) {
        Ok(Some(f)) => f,
        Ok(None) => return false,
        Err(e) => return e.kind() == io::ErrorKind::NotFound,
    };
    let Ok(id) = sys::id(&file) else { return false };
    if ids.contains(&id) || sys::try_lock_writers(&file).is_err() {
        return false;
    }
    let gone = folder.id_of(name).ok().flatten() == Some(id)
        && match folder.remove(name) {
            Ok(()) => true,
            Err(e) => e.kind() == io::ErrorKind::NotFound,
        };
    let _ = sys::unlock_writers(&file);
    gone
}
