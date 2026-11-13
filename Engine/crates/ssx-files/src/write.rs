use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::error::Error;
use crate::folder::{Folder, PathFolder};
use crate::slot::{self, Claim, SLOTS};
use crate::{Options, Step, fault, sys};

// PORT(choice): 10 s, looked at every 10 ms (an update of `exports.tsv` takes milliseconds; a writer that holds it
// longer is stuck); tuning: an update timed on a slow network folder; the port's own file layer.
const UPDATE_WAIT: Duration = Duration::from_secs(10);
const UPDATE_POLL: Duration = Duration::from_millis(10);

fn narrowed(mode: u32, old: Option<u32>) -> u32 {
    old.map_or(mode, |o| mode & o)
}

fn split(path: &Path) -> Result<(PathFolder, &OsStr), Error> {
    PathFolder::split(path).map_err(|e| Error::new(path, e))
}

fn first_partial(folder: &dyn Folder, target: &OsStr) -> PathBuf {
    folder.shown(&slot::slot_name(target, 0))
}

fn claimed<F: Folder + Clone>(folder: &F, target: &OsStr, mode: u32) -> Result<Claim<F>, Error> {
    slot::claim(folder, target, mode, 0..SLOTS).map_err(|e| Error::new(first_partial(folder, target), e))?.ok_or_else(
        || {
            let why = format!("all {SLOTS} partial files of this name are held by other writers");
            Error::new(first_partial(folder, target), io::Error::new(io::ErrorKind::WouldBlock, why))
        },
    )
}

pub(crate) fn finish<F: Folder, E: From<Error>>(
    mut claim: Claim<F>,
    target: &OsStr,
    opts: &Options,
    fill: impl FnOnce(&mut File, &Path) -> Result<(), E>,
) -> Result<(), E> {
    let partial = claim.folder.shown(&claim.name);
    fault::check(Step::Write).map_err(|e| Error::new(&partial, e))?;
    fill(&mut claim.file, &partial)?;
    Ok(commit(claim, target, opts)?)
}

fn commit<F: Folder>(mut claim: Claim<F>, target: &OsStr, opts: &Options) -> Result<(), Error> {
    let partial = claim.folder.shown(&claim.name);
    fault::check(Step::Sync).and_then(|()| claim.file.sync_all()).map_err(|e| Error::new(&partial, e))?;
    fault::check(Step::Rename)
        .and_then(|()| claim.folder.rename_replace(&claim.name, target))
        .map_err(|e| Error::new(claim.folder.shown(target), e))?;
    claim.done();
    if opts.sync_folder {
        fault::check(Step::FolderSync)
            .and_then(|()| claim.folder.sync())
            .map_err(|e| Error::new(claim.folder.shown_folder(), e))?;
    }
    Ok(())
}

pub(crate) fn replace_at<F: Folder + Clone, E: From<Error>>(
    folder: &F,
    target: &OsStr,
    opts: &Options,
    fill: impl FnOnce(&mut File, &Path) -> Result<(), E>,
) -> Result<(), E> {
    let claim = claimed(folder, target, narrowed(opts.mode, folder.mode_of(target)))?;
    finish(claim, target, opts, fill)
}

pub(crate) fn write_bytes(bytes: &[u8]) -> impl FnOnce(&mut File, &Path) -> Result<(), Error> + '_ {
    move |f, partial| f.write_all(bytes).map_err(|e| Error::new(partial, e))
}

pub fn replace(path: &Path, bytes: &[u8], opts: &Options) -> Result<(), Error> {
    let (folder, name) = split(path)?;
    replace_at(&folder, name, opts, write_bytes(bytes))
}

pub fn replace_with<E: From<Error>>(
    path: &Path,
    opts: &Options,
    fill: impl FnOnce(&mut File) -> Result<(), E>,
) -> Result<(), E> {
    let (folder, name) = split(path)?;
    replace_at(&folder, name, opts, |f, _| fill(f))
}

pub struct Prepared {
    claim: Claim<PathFolder>,
    target: OsString,
    opts: Options,
}

impl Prepared {
    pub fn target(&self) -> PathBuf {
        self.claim.folder.shown(&self.target)
    }

    pub fn commit(self) -> Result<(), Error> {
        commit(self.claim, &self.target, &self.opts)
    }
}

pub fn prepare_with<E: From<Error>>(
    path: &Path,
    opts: &Options,
    fill: impl FnOnce(&mut File) -> Result<(), E>,
) -> Result<Prepared, E> {
    let (folder, name) = split(path)?;
    let mut claim = claimed(&folder, name, narrowed(opts.mode, folder.mode_of(name)))?;
    let partial = claim.folder.shown(&claim.name);
    fault::check(Step::Write).map_err(|e| Error::new(&partial, e))?;
    fill(&mut claim.file)?;
    Ok(Prepared { claim, target: name.to_os_string(), opts: *opts })
}

pub fn create(path: &Path, bytes: &[u8], opts: &Options) -> Result<(), Error> {
    create_at(path, opts, write_bytes(bytes))
}

pub fn create_with<E: From<Error>>(
    path: &Path,
    opts: &Options,
    fill: impl FnOnce(&mut File) -> Result<(), E>,
) -> Result<(), E> {
    create_at(path, opts, |f, _| fill(f))
}

pub(crate) fn create_at<E: From<Error>>(
    path: &Path,
    opts: &Options,
    fill: impl FnOnce(&mut File, &Path) -> Result<(), E>,
) -> Result<(), E> {
    let (folder, name) = split(path)?;
    let mut claim = claimed(&folder, name, opts.mode)?;
    let partial = folder.shown(&claim.name);
    fault::check(Step::Write).map_err(|e| Error::new(&partial, e))?;
    fill(&mut claim.file, &partial)?;
    fault::check(Step::Sync).and_then(|()| claim.file.sync_all()).map_err(|e| Error::new(&partial, e))?;
    fault::check(Step::Rename).and_then(|()| sys::rename_noreplace(&partial, path)).map_err(|e| Error::new(path, e))?;
    claim.done();
    drop(claim);
    if opts.sync_folder {
        fault::check(Step::FolderSync)
            .and_then(|()| folder.sync())
            .map_err(|e| Error::new(folder.shown_folder(), e))?;
    }
    Ok(())
}

pub fn update<E: From<Error>>(
    path: &Path,
    opts: &Options,
    max_old: u64,
    change: impl FnOnce(Option<Vec<u8>>) -> Result<Vec<u8>, E>,
) -> Result<(), E> {
    let (folder, name) = split(path)?;
    let mode = narrowed(opts.mode, folder.mode_of(name));
    let deadline = Instant::now() + UPDATE_WAIT;
    let claim = loop {
        if let Some(c) =
            slot::claim(&folder, name, mode, 0..1).map_err(|e| Error::new(first_partial(&folder, name), e))?
        {
            break c;
        }
        if Instant::now() >= deadline {
            let why = format!("another writer has been updating this file for {} s", UPDATE_WAIT.as_secs());
            return Err(Error::new(first_partial(&folder, name), io::Error::new(io::ErrorKind::WouldBlock, why)).into());
        }
        std::thread::sleep(UPDATE_POLL);
    };
    let old = read_old(&folder, name, max_old)?;
    let new = change(old)?;
    finish(claim, name, opts, |f, partial| write_bytes(&new)(f, partial).map_err(E::from))
}

fn read_old(folder: &PathFolder, name: &OsStr, max: u64) -> Result<Option<Vec<u8>>, Error> {
    let shown = || folder.shown(name);
    let file = match folder.open_regular(name) {
        Ok(Some(f)) => f,
        Ok(None) => return Err(Error::new(shown(), io::Error::new(io::ErrorKind::InvalidData, "not a regular file"))),
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(Error::new(shown(), e)),
    };
    let mut bytes = Vec::new();
    file.take(max.saturating_add(1)).read_to_end(&mut bytes).map_err(|e| Error::new(shown(), e))?;
    if bytes.len() as u64 > max {
        let why = format!("larger than {max} bytes");
        return Err(Error::new(shown(), io::Error::new(io::ErrorKind::InvalidData, why)));
    }
    Ok(Some(bytes))
}
