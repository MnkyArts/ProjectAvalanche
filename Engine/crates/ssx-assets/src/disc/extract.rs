use std::collections::BTreeSet;
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context, Result, bail};
use serde_json::{Map, Value, json};
use ssx_content::hash::{ContentHash, ContentIndex};
use ssx_formats::big::BigArchive;
use ssx_formats::chunk;
use ssx_formats::xdvdfs::{Disc, Extent};

use super::folder::{INDEX_FILE, check_files};
use super::rows::{self, Part};
use super::setup::{
    ARCHIVES, Cancelled, MARKER_FORMAT, MARKER_XEX_HEADER, Progress, Summary, big_root, check_disc, disc_root, marker,
    movie_files, open_disc, read_index, safe_relative, write_marker, xex_header_hex,
};

const COPY_CHUNK: usize = 1 << 20;

const MEMBER: ssx_files::Options = ssx_files::Options::new().folder_synced_by_caller();

// PORT(choice): 16 threads (the most measured, on one btrfs SSD); tuning: the setup timed with 8 / 16 / 32 on ext4,
// NTFS and a hard disk; the console reads its disc itself.
const COMMIT_THREADS: usize = 16;

pub fn run(iso: &Path, out: &Path, cancel: &AtomicBool, progress: &mut dyn FnMut(&Progress)) -> Result<Summary> {
    let mut disc = open_disc(iso)?;
    let check = check_disc(&mut disc)?;
    ssx_files::create_dir_all(out).with_context(|| format!("creating {}", out.display()))?;
    let same_disc = |m: &Value| {
        m["format"].as_u64().is_some_and(|f| (1..=MARKER_FORMAT).contains(&f))
            && m["fingerprint"] == json!(check.fingerprint)
            && m["iso_size"] == json!(check.image_size)
    };
    let previous = marker(out).ok().flatten().filter(same_disc);
    let complete_before =
        previous.as_ref().is_some_and(|m| m["format"] == json!(MARKER_FORMAT) && m["complete"] == json!(true));
    let earlier = if previous.is_some() { rows::earlier(out) } else { ContentIndex::default() };
    let damage = check_files(out, &earlier);
    let pinned = |entry: &Value, part: Part<'_>| {
        entry["rows"].as_str() == Some(&rows::digest(&earlier, part).to_string()) && !damage.below(&part.prefix())
    };

    let mut summary = Summary::default();
    let mut done = ContentIndex::default();
    let mut finished = Map::new();
    let mut todo = Vec::new();
    for name in ARCHIVES {
        let entry = previous.as_ref().map_or(&Value::Null, |m| &m["finished"][name]);
        let tree = fs::symlink_metadata(big_root(out).join(name)).is_ok_and(|m| m.is_dir());
        if entry.is_object() && tree && pinned(entry, Part::Archive(name)) {
            rows::take_part(&earlier, Part::Archive(name), &mut done);
            finished.insert(name.to_string(), entry.clone());
            summary.already_done.push(name.to_string());
            continue;
        }
        if complete_before && entry.is_object() {
            let mut entry = entry.clone();
            entry.as_object_mut().map(|o| o.remove("rows"));
            finished.insert(name.to_string(), entry);
        }
        let extent = disc.find(&format!("{name}.big")).context("archive vanished")?.extent();
        let big = read_index(&mut disc, name, extent)?;
        todo.push((name, extent, big));
    }
    let movies_entry = previous.as_ref().map_or(&Value::Null, |m| &m["movies"]);
    let movies_done = movies_entry.is_object() && pinned(movies_entry, Part::Movies);
    let mut marker = json!({
        "format": MARKER_FORMAT,
        "iso": iso.display().to_string(),
        "iso_size": check.image_size,
        "fingerprint": check.fingerprint,
        "layout": format!("{:?}", check.layout),
        "finished": finished,
        "complete": complete_before,
    });
    if let Some(header) = &check.xex_header {
        marker[MARKER_XEX_HEADER] = json!(xex_header_hex(header));
    }
    if movies_done {
        rows::take_part(&earlier, Part::Movies, &mut done);
        marker["movies"] = movies_entry.clone();
    } else if complete_before && movies_entry.is_object() {
        let mut entry = movies_entry.clone();
        entry.as_object_mut().map(|o| o.remove("rows"));
        marker["movies"] = entry;
    }
    rows::write(out, &done)?;
    write_marker(out, &marker)?;

    let movies = if movies_done { Vec::new() } else { movie_files(&disc) };
    let member_bytes: u64 = todo.iter().map(|(_, _, b)| b.entries.iter().map(|e| e.size as u64).sum::<u64>()).sum();
    let p = Progress {
        archive_count: todo.len() + usize::from(!movies_done),
        files_total: todo.iter().map(|(_, _, b)| b.entries.len()).sum::<usize>() + movies.len(),
        bytes_total: member_bytes + movies.iter().map(|(_, e)| e.size as u64).sum::<u64>(),
        ..Default::default()
    };
    let mut job = Job { disc: &mut disc, cancel, p, progress, summary: &mut summary, top: out.to_path_buf() };
    for (i, (name, extent, big)) in todo.iter().enumerate() {
        job.p.archive = name.to_string();
        job.p.archive_index = i;
        (job.progress)(&job.p);
        let (mut entry, part) = job.archive(name, *extent, big, &big_root(out).join(name))?;
        entry["rows"] = json!(rows::digest(&part, Part::Archive(name)).to_string());
        done.entries.extend(part.entries);
        rows::write(out, &done)?;
        marker["finished"][*name] = entry;
        write_marker(out, &marker)?;
        job.summary.extracted.push(name.to_string());
    }
    if !movies_done {
        job.p.archive = "movies".to_string();
        job.p.archive_index = todo.len();
        (job.progress)(&job.p);
        let (mut entry, part) = job.movies(&movies, &disc_root(out))?;
        entry["rows"] = json!(rows::digest(&part, Part::Movies).to_string());
        done.entries.extend(part.entries);
        rows::write(out, &done)?;
        marker["movies"] = entry;
        write_marker(out, &marker)?;
    }
    if cancel.load(Ordering::Relaxed) {
        return Err(Cancelled.into());
    }
    let text = done.to_tsv();
    let index = out.join(INDEX_FILE);
    ssx_files::replace(&index, text.as_bytes(), &ssx_files::Options::new())
        .with_context(|| format!("writing {}", index.display()))?;
    let bytes = done.entries.values().map(|(size, _)| size).sum::<u64>();
    summary.content_files = done.entries.len();
    marker["content_index"] =
        json!({ "files": done.entries.len(), "bytes": bytes, "blake3": ContentHash::of(text.as_bytes()).to_string() });
    marker["complete"] = json!(true);
    write_marker(out, &marker)?;
    rows::remove(out)?;
    Ok(summary)
}

struct Job<'a> {
    disc: &'a mut Disc<File>,
    cancel: &'a AtomicBool,
    p: Progress,
    progress: &'a mut dyn FnMut(&Progress),
    summary: &'a mut Summary,
    top: PathBuf,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Outcome {
    Written,
    Kept,
    Replaced,
}

struct Placed {
    outcome: Outcome,
    size: u64,
    hash: ContentHash,
}

#[derive(Default)]
struct Counts {
    written: usize,
    kept: usize,
    replaced: usize,
    bytes: u64,
}

impl Counts {
    fn add(&mut self, placed: &Placed) {
        match placed.outcome {
            Outcome::Written => self.written += 1,
            Outcome::Kept => self.kept += 1,
            Outcome::Replaced => self.replaced += 1,
        }
        if placed.outcome != Outcome::Kept {
            self.bytes += placed.size;
        }
    }
}

impl Job<'_> {
    fn check_cancel(&self) -> Result<()> {
        if self.cancel.load(Ordering::Relaxed) {
            return Err(anyhow::Error::new(Cancelled));
        }
        Ok(())
    }

    fn movies(&mut self, list: &[(String, Extent)], root: &Path) -> Result<(Value, ContentIndex)> {
        let mut list: Vec<_> = list.iter().collect();
        list.sort_by_key(|(_, e)| e.sector);
        let mut counts = Counts::default();
        let mut folders = BTreeSet::new();
        let mut checked = BTreeSet::new();
        let mut rows = ContentIndex::default();
        let mut bufs = Buffers::new();
        let top = self.top.clone();
        ssx_files::committing(COMMIT_THREADS, |commits| -> Result<()> {
            for (path, extent) in &list {
                self.check_cancel()?;
                let rel = safe_relative(path).with_context(|| format!("unusable movie path {path:?}"))?;
                let dest = root.join(rel);
                let dir = dest.parent().context("movie path has a parent")?;
                real_folders(&top, dir, &mut checked)?;
                let mut r = self.disc.reader(*extent)?;
                let mut job = Progressing { cancel: self.cancel, p: &mut self.p, progress: &mut *self.progress };
                let base = job.p.bytes_done;
                let placed = place_stream(&mut r, 0, extent.size as u64, &dest, &mut bufs, &mut job, commits)?;
                self.p.bytes_done = base;
                if placed.outcome != Outcome::Kept {
                    folders.insert(dir.to_path_buf());
                }
                counts.add(&placed);
                rows.entries.insert(format!("disc/{path}"), (placed.size, placed.hash));
                self.p.files_done += 1;
                self.p.bytes_done += extent.size as u64;
                (self.progress)(&self.p);
            }
            Ok(())
        })?;
        sync_folders(&self.top, &folders)?;
        self.summary.movies_written += counts.written;
        self.summary.movies_kept += counts.kept;
        self.summary.movies_replaced += counts.replaced;
        self.summary.bytes_written += counts.bytes;
        let Counts { written, kept, replaced, bytes } = counts;
        let entry =
            json!({ "files": list.len(), "written": written, "kept": kept, "replaced": replaced, "bytes": bytes });
        Ok((entry, rows))
    }

    fn archive(&mut self, name: &str, extent: Extent, big: &BigArchive, root: &Path) -> Result<(Value, ContentIndex)> {
        let mut entries: Vec<_> = big.entries.iter().collect();
        entries.sort_by_key(|e| e.offset);
        let mut counts = Counts::default();
        let mut folders = BTreeSet::new();
        let mut checked = BTreeSet::new();
        let mut failures = Vec::new();
        let mut rows = ContentIndex::default();
        let mut bufs = Buffers::new();
        let top = self.top.clone();
        let mut seen = BTreeSet::new();
        ssx_files::committing(COMMIT_THREADS, |commits| -> Result<()> {
            for e in entries {
                self.check_cancel()?;
                let path = e.path();
                if e.offset + e.size as u64 > extent.size as u64 {
                    bail!("{name}.big: {path} ends past the archive");
                }
                if !seen.insert(path.to_ascii_lowercase()) {
                    bail!("{name}.big: {path} is listed twice");
                }
                let rel = safe_relative(&path).with_context(|| format!("{name}.big: unusable member path {path:?}"))?;
                let dest = root.join(rel);
                let dir = dest.parent().context("member path has a parent")?;
                real_folders(&top, dir, &mut checked)?;
                let mut r = self.disc.reader(extent)?;
                r.seek(SeekFrom::Start(e.offset))?;
                let mut head = vec![0u8; (e.size as usize).min(28)];
                r.read_exact(&mut head)?;
                r.seek(SeekFrom::Start(e.offset))?;
                let placed = if chunk::detect(&head).is_some() {
                    let mut stored = vec![0u8; e.size as usize];
                    r.read_exact(&mut stored)?;
                    match chunk::decompress(&stored) {
                        Ok(data) => Some(place_bytes(&dest, &data, &mut bufs.file, commits)?),
                        Err(err) => {
                            failures.push(format!("{path}: {err}"));
                            None
                        }
                    }
                } else {
                    let mut job = Progressing { cancel: self.cancel, p: &mut self.p, progress: &mut *self.progress };
                    let base = job.p.bytes_done;
                    let placed = place_stream(&mut r, e.offset, e.size as u64, &dest, &mut bufs, &mut job, commits)?;
                    self.p.bytes_done = base;
                    Some(placed)
                };
                if let Some(placed) = placed {
                    if placed.outcome != Outcome::Kept {
                        folders.insert(dir.to_path_buf());
                    }
                    counts.add(&placed);
                    rows.entries.insert(format!("big/{name}/{path}"), (placed.size, placed.hash));
                }
                self.p.files_done += 1;
                self.p.bytes_done += e.size as u64;
                self.p.failed = self.summary.failures.len() + failures.len();
                (self.progress)(&self.p);
            }
            Ok(())
        })?;
        sync_folders(&self.top, &folders)?;
        self.summary.members_written += counts.written;
        self.summary.members_kept += counts.kept;
        self.summary.members_replaced += counts.replaced;
        self.summary.bytes_written += counts.bytes;
        self.summary.failures.extend(failures.iter().map(|f| format!("{name}: {f}")));
        let Counts { written, kept, replaced, bytes } = counts;
        let entry = json!({
            "members": big.entries.len(), "written": written, "kept": kept, "replaced": replaced, "bytes": bytes,
            "failures": failures,
        });
        Ok((entry, rows))
    }
}

struct Progressing<'a, 'b> {
    cancel: &'a AtomicBool,
    p: &'a mut Progress,
    progress: &'a mut (dyn FnMut(&Progress) + 'b),
}

impl Progressing<'_, '_> {
    fn advance(&mut self, base: u64, done: u64) -> Result<()> {
        self.p.bytes_done = base + done;
        (self.progress)(&*self.p);
        if self.cancel.load(Ordering::Relaxed) {
            return Err(anyhow::Error::new(Cancelled));
        }
        Ok(())
    }
}

struct Buffers {
    image: Vec<u8>,
    file: Vec<u8>,
}

impl Buffers {
    fn new() -> Self {
        Buffers { image: vec![0u8; COPY_CHUNK], file: vec![0u8; COPY_CHUNK] }
    }
}

enum Existing {
    Nothing,
    File(File, u64),
    Other,
}

fn existing(dest: &Path) -> Result<Existing> {
    match ssx_files::open_plain(dest) {
        Ok(Some(file)) => {
            let len = file.metadata().with_context(|| format!("reading {}", dest.display()))?.len();
            Ok(Existing::File(file, len))
        }
        Ok(None) if fs::symlink_metadata(dest).is_ok_and(|m| m.is_dir()) => {
            bail!("{} is a folder where a file of the setup belongs: refused (remove it)", dest.display())
        }
        Ok(None) => Ok(Existing::Other),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Existing::Nothing),
        Err(e) => Err(e).with_context(|| format!("reading {}", dest.display())),
    }
}

fn place_bytes(dest: &Path, data: &[u8], buf: &mut [u8], commits: &ssx_files::Committer<'_>) -> Result<Placed> {
    let hash = ContentHash::of(data);
    let size = data.len() as u64;
    let outcome = match existing(dest)? {
        Existing::File(f, len) if len == size && same_bytes(&f, data, buf) => {
            return Ok(Placed { outcome: Outcome::Kept, size, hash });
        }
        Existing::Nothing => Outcome::Written,
        Existing::File(..) | Existing::Other => Outcome::Replaced,
    };
    let file = ssx_files::prepare_with(dest, &MEMBER, |f| {
        f.write_all(data).with_context(|| format!("writing {}", dest.display()))
    })?;
    commits.commit(file)?;
    Ok(Placed { outcome, size, hash })
}

fn same_bytes(mut file: &File, data: &[u8], buf: &mut [u8]) -> bool {
    let mut at = 0;
    while at < data.len() {
        let n = (data.len() - at).min(buf.len());
        if file.read_exact(&mut buf[..n]).is_err() || buf[..n] != data[at..at + n] {
            return false;
        }
        at += n;
    }
    file.read(&mut [0u8; 1]).is_ok_and(|n| n == 0)
}

fn place_stream<R: Read + Seek>(
    r: &mut R,
    start: u64,
    size: u64,
    dest: &Path,
    bufs: &mut Buffers,
    job: &mut Progressing,
    commits: &ssx_files::Committer<'_>,
) -> Result<Placed> {
    let outcome = match existing(dest)? {
        Existing::File(mut f, len) if len == size => {
            if let Some(hash) = compare_stream(r, &mut f, size, bufs, job)? {
                return Ok(Placed { outcome: Outcome::Kept, size, hash });
            }
            r.seek(SeekFrom::Start(start))?;
            Outcome::Replaced
        }
        Existing::Nothing => Outcome::Written,
        Existing::File(..) | Existing::Other => Outcome::Replaced,
    };
    let mut hash = None;
    let file = ssx_files::prepare_with(dest, &MEMBER, |f| -> Result<()> {
        hash = Some(copy_hashed(r, f, size, &mut bufs.image, job, dest)?);
        Ok(())
    })?;
    commits.commit(file)?;
    let hash = hash.context("the copy hashed the file")?;
    Ok(Placed { outcome, size, hash })
}

fn compare_stream(
    r: &mut impl Read,
    file: &mut File,
    size: u64,
    bufs: &mut Buffers,
    job: &mut Progressing,
) -> Result<Option<ContentHash>> {
    let base = job.p.bytes_done;
    let mut h = blake3::Hasher::new();
    let mut done = 0u64;
    while done < size {
        let n = (size - done).min(COPY_CHUNK as u64) as usize;
        r.read_exact(&mut bufs.image[..n]).context("reading the image")?;
        if file.read_exact(&mut bufs.file[..n]).is_err() || bufs.image[..n] != bufs.file[..n] {
            job.p.bytes_done = base;
            return Ok(None);
        }
        h.update(&bufs.image[..n]);
        done += n as u64;
        job.advance(base, done)?;
    }
    if !file.read(&mut [0u8; 1]).is_ok_and(|n| n == 0) {
        job.p.bytes_done = base;
        return Ok(None);
    }
    Ok(Some(ContentHash(*h.finalize().as_bytes())))
}

fn copy_hashed(
    r: &mut impl Read,
    f: &mut File,
    size: u64,
    buf: &mut [u8],
    job: &mut Progressing,
    dest: &Path,
) -> Result<ContentHash> {
    let base = job.p.bytes_done;
    let mut h = blake3::Hasher::new();
    let mut done = 0u64;
    while done < size {
        let n = (size - done).min(buf.len() as u64) as usize;
        r.read_exact(&mut buf[..n]).context("reading the image")?;
        h.update(&buf[..n]);
        f.write_all(&buf[..n]).with_context(|| format!("writing {}", dest.display()))?;
        done += n as u64;
        job.advance(base, done)?;
    }
    Ok(ContentHash(*h.finalize().as_bytes()))
}

fn real_folders(top: &Path, dir: &Path, checked: &mut BTreeSet<PathBuf>) -> Result<()> {
    let rel = dir.strip_prefix(top).with_context(|| format!("{} is outside the data folder", dir.display()))?;
    let mut d = top.to_path_buf();
    for part in rel.components() {
        d.push(part);
        if checked.contains(&d) {
            continue;
        }
        match fs::symlink_metadata(&d) {
            Ok(m) if m.is_dir() => {}
            Ok(_) => {
                bail!("{} is a link or a file where a folder of the setup belongs: refused (remove it)", d.display())
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                fs::create_dir(&d).with_context(|| format!("creating {}", d.display()))?;
            }
            Err(e) => return Err(e).with_context(|| format!("reading {}", d.display())),
        }
        checked.insert(d.clone());
    }
    Ok(())
}

fn sync_folders(top: &Path, dirs: &BTreeSet<PathBuf>) -> Result<()> {
    let mut all = BTreeSet::new();
    for dir in dirs {
        for d in dir.ancestors().take_while(|d| d.starts_with(top)) {
            if !all.insert(d.to_path_buf()) {
                break;
            }
        }
    }
    for d in all.iter().rev() {
        ssx_files::sync_folder(d)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests;
