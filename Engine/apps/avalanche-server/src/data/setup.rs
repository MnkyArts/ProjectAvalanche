use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::Instant;

use serde_json::json;
use ssx_assets::setup::Progress;
use ssx_assets::subset::{self, MAX_MEMBER, Member};

use super::index::{self, INDEX_FILE, Index};
use super::manifest::{Class, Entry, KNOWN_PRESSING, hex, manifest};
use super::{guard, md5check};

#[derive(Clone, Debug)]
pub enum Source {
    Iso(PathBuf),
    Extraction(PathBuf),
}

#[derive(Clone, Debug)]
pub struct Report {
    pub members: usize,
    pub bytes: u64,
    pub written: usize,
    pub kept: usize,
    pub known_checked: usize,
    pub md5_checked: usize,
    pub index_checked: usize,
    pub pressing: String,
    pub seconds: f64,
}

// PORT(choice): 4 MiB (the index is 280 KB at most, the marker under 1 KB, one partial file at most 16 MiB is already
// counted as its member); tuning: a file system's reserved blocks; the original has no dedicated server.
const HEADROOM: u64 = 4 << 20;

pub fn run(
    source: &Source,
    out: &Path,
    configured: &guard::Configured<'_>,
    classes: &BTreeSet<Class>,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(&Progress),
) -> Result<Report, String> {
    let started = Instant::now();
    let list = manifest()?;
    let entries = list.of(classes);
    if classes.contains(&Class::NotServer) {
        return Err("the class not-server is never taken (the simulation does not read it)".into());
    }
    let wanted: BTreeSet<String> = entries.iter().map(|e| e.member.data_path()).collect();
    guard::prepare(out, &wanted, configured)?;
    match std::fs::remove_file(out.join(INDEX_FILE)) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
            return Err(format!("{}: {e}", out.join(INDEX_FILE).display()));
        }
        _ => {}
    }
    let needed: u64 = entries
        .iter()
        .filter(|e| std::fs::symlink_metadata(out.join(e.member.data_path())).map_or(true, |m| m.len() != e.size))
        .map(|e| e.size)
        .sum();
    let free = guard::free_bytes(out)?;
    if free < needed.saturating_add(HEADROOM) {
        return Err(format!(
            "{} has {:.1} MB free; the subset needs {:.1} MB more (and {} MB of room): free some space or choose another folder",
            out.display(),
            free as f64 / 1e6,
            needed as f64 / 1e6,
            HEADROOM >> 20
        ));
    }
    let mut report = match source {
        Source::Iso(iso) => from_iso(iso, out, &entries, classes, cancel, progress)?,
        Source::Extraction(dir) => from_extraction(dir, out, &entries, classes, cancel, progress)?,
    };
    report.seconds = started.elapsed().as_secs_f64();
    Ok(report)
}

struct Checker<'a> {
    by_path: BTreeMap<String, &'a Entry>,
    known: bool,
    md5: BTreeMap<String, BTreeMap<String, [u8; 16]>>,
    reference: Option<Index>,
    index: Index,
    known_checked: BTreeSet<String>,
    md5_checked: BTreeSet<String>,
    index_checked: BTreeSet<String>,
}

impl Checker<'_> {
    fn check(&mut self, m: &Member, data: &[u8]) -> Result<(), String> {
        let path = m.data_path();
        let entry = self.by_path.get(&path).ok_or("not in the server's member list")?;
        let (size, hash) = (data.len() as u64, index::hash(data));
        if self.known {
            if size != entry.size || hash != entry.blake3 {
                return Err(format!(
                    "{size} bytes, BLAKE3 {}…, but the known disc's member has {} bytes, {}…: a damaged image or extraction?",
                    &hex(&hash)[..16],
                    entry.size,
                    &hex(&entry.blake3)[..16]
                ));
            }
            self.known_checked.insert(path.clone());
        } else if let Some(reference) = &self.reference {
            match reference.rows.get(&path) {
                Some(&(s, h)) if (s, h) == (size, hash) => {
                    self.index_checked.insert(path.clone());
                }
                Some(_) => return Err("differs from the extraction's own content.tsv: a damaged extraction?".into()),
                None => return Err("not in the extraction's content.tsv".into()),
            }
        }
        if let Some(track) = md5check::track_of(m) {
            let name = m.path.rsplit('/').next().unwrap_or_default();
            let expected =
                self.md5.get(track).and_then(|l| l.get(name)).ok_or("not in its track's MD5 list on the disc")?;
            if md5check::md5(data) != *expected {
                return Err("its MD5 is not the one the disc's list gives: a damaged image?".into());
            }
            self.md5_checked.insert(path.clone());
        }
        self.index.rows.insert(path, (size, hash));
        Ok(())
    }

    fn report(self, members: usize, bytes: u64, written: usize, kept: usize, pressing: String) -> (Report, Index) {
        let r = Report {
            members,
            bytes,
            written,
            kept,
            known_checked: self.known_checked.len(),
            md5_checked: self.md5_checked.len(),
            index_checked: self.index_checked.len(),
            pressing,
            seconds: 0.0,
        };
        (r, self.index)
    }
}

fn checker<'a>(entries: &[&'a Entry], known: bool) -> Checker<'a> {
    Checker {
        by_path: entries.iter().map(|e| (e.member.data_path(), *e)).collect(),
        known,
        md5: BTreeMap::new(),
        reference: None,
        index: Index::default(),
        known_checked: BTreeSet::new(),
        md5_checked: BTreeSet::new(),
        index_checked: BTreeSet::new(),
    }
}

fn tracks(entries: &[&Entry]) -> BTreeSet<String> {
    entries.iter().filter_map(|e| md5check::track_of(&e.member).map(str::to_string)).collect()
}

fn classes_label(classes: &BTreeSet<Class>) -> serde_json::Value {
    let mut names = vec![Class::Rules.name()];
    names.extend(classes.iter().filter(|c| **c != Class::Rules).map(|c| c.name()));
    json!(names)
}

fn from_iso(
    iso: &Path,
    out: &Path,
    entries: &[&Entry],
    classes: &BTreeSet<Class>,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(&Progress),
) -> Result<Report, String> {
    let disc = ssx_assets::setup::inspect(iso).map_err(|e| format!("{e:#}"))?;
    let known = disc.fingerprint == KNOWN_PRESSING.fingerprint && disc.image_size == KNOWN_PRESSING.iso_size;
    let mut c = checker(entries, known);
    let tracks = tracks(entries);
    let lists: Vec<Member> = tracks.iter().map(|t| md5check::list_member(t)).collect();
    if !lists.is_empty() {
        let bytes = subset::read(iso, &lists).map_err(|e| format!("the disc's MD5 lists: {e:#}"))?;
        for (track, xml) in tracks.iter().zip(bytes) {
            let list =
                md5check::parse(&xml).map_err(|e| format!("{}: {e}", md5check::list_member(track).data_path()))?;
            c.md5.insert(track.clone(), list);
        }
    }
    let mut selection = subset::Selection::default();
    for e in entries {
        selection.insert(e.member.clone()).map_err(|e| format!("{e:#}"))?;
    }
    let label = json!({ "classes": classes_label(classes), "source": "image", "known_pressing": known });
    let mut check = |m: &Member, data: &[u8]| c.check(m, data).map_err(anyhow::Error::msg);
    let run = subset::run(iso, out, &selection, label, &mut check, cancel, progress).map_err(|e| format!("{e:#}"))?;
    let pressing = if known {
        format!("the known pressing ({})", disc.fingerprint)
    } else {
        format!("another pressing ({}): checked by the disc's MD5 lists only", disc.fingerprint)
    };
    let (report, index) = c.report(run.members, run.bytes, run.written, run.kept, pressing);
    subset::write_synced(out, INDEX_FILE, index.to_tsv().as_bytes()).map_err(|e| format!("{e:#}"))?;
    Ok(report)
}

fn extraction_pressing(dir: &Path) -> Option<(String, u64)> {
    let path = dir.join(ssx_assets::setup::MARKER);
    std::fs::symlink_metadata(&path).ok().filter(|m| m.is_file() && m.len() <= 1 << 20)?;
    let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()?;
    Some((v["fingerprint"].as_str()?.to_string(), v["iso_size"].as_u64()?))
}

fn read_source(dir: &Path, rel: &str) -> Result<Vec<u8>, String> {
    let mut p = dir.to_path_buf();
    for part in rel.split('/') {
        p.push(part);
        let m = std::fs::symlink_metadata(&p).map_err(|e| format!("{}: {e}", p.display()))?;
        if m.file_type().is_symlink() {
            return Err(format!("{} is a link: refused", p.display()));
        }
    }
    let m = std::fs::symlink_metadata(&p).map_err(|e| format!("{}: {e}", p.display()))?;
    if !m.is_file() || m.len() > MAX_MEMBER {
        return Err(format!("{}: not a regular file of at most {MAX_MEMBER} bytes", p.display()));
    }
    use std::io::Read;
    let mut data = Vec::new();
    std::fs::File::open(&p)
        .and_then(|f| f.take(MAX_MEMBER).read_to_end(&mut data))
        .map_err(|e| format!("{}: {e}", p.display()))?;
    Ok(data)
}

fn from_extraction(
    dir: &Path,
    out: &Path,
    entries: &[&Entry],
    classes: &BTreeSet<Class>,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(&Progress),
) -> Result<Report, String> {
    let meta = std::fs::symlink_metadata(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    if !meta.is_dir() || !dir.join("big").is_dir() {
        return Err(format!("{} is not the game's data folder (no big/ in it, or a link)", dir.display()));
    }
    let same = |a: &Path, b: &Path| match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    };
    if same(dir, out) {
        return Err("the extraction and the server's data folder are one folder".into());
    }
    let marker = extraction_pressing(dir);
    let known = marker.as_ref().is_some_and(|(f, s)| f == KNOWN_PRESSING.fingerprint && *s == KNOWN_PRESSING.iso_size);
    let mut c = checker(entries, known || marker.is_none());
    if marker.is_some() && !known {
        let theirs = ssx_assets::folder::content_index(dir).map_err(|e| format!("the extraction's index: {e}"))?;
        c.reference = Some(Index { rows: theirs.entries.into_iter().map(|(p, (size, h))| (p, (size, h.0))).collect() });
    }
    for track in tracks(entries) {
        let rel = md5check::list_member(&track).data_path();
        let xml = read_source(dir, &rel)?;
        c.md5.insert(track, md5check::parse(&xml).map_err(|e| format!("{rel}: {e}"))?);
    }
    let fingerprint = marker.as_ref().map(|(f, _)| f.clone());
    let iso_size = marker.as_ref().map(|(_, s)| *s);
    let label = json!({ "classes": classes_label(classes), "source": "extraction", "known_pressing": c.known });
    let mut state = json!({
        "format": 1,
        "extraction": dir.display().to_string(),
        "fingerprint": fingerprint,
        "iso_size": iso_size,
        "selection": label,
        "members": entries.len(),
        "complete": false,
    });
    let write_state = |v: &serde_json::Value| {
        let text = serde_json::to_string_pretty(v).map_err(|e| e.to_string())?;
        subset::write_synced(out, subset::MARKER, text.as_bytes()).map_err(|e| format!("{e:#}"))
    };
    write_state(&state)?;
    let mut p = Progress { archive: "extraction".into(), files_total: entries.len(), ..Default::default() };
    let (mut written, mut kept, mut bytes) = (0usize, 0usize, 0u64);
    for e in entries {
        if cancel.load(std::sync::atomic::Ordering::Relaxed) {
            return Err("setup cancelled".into());
        }
        let m = &e.member;
        let have = subset::existing(out, m).map_err(|e| format!("{e:#}"))?;
        if let Some(data) = have
            && c.check(m, &data).is_ok()
        {
            kept += 1;
            bytes += data.len() as u64;
            p.files_done += 1;
            progress(&p);
            continue;
        }
        let data = read_source(dir, &m.data_path())?;
        c.check(m, &data).map_err(|why| format!("{}: refused: {why}", m.data_path()))?;
        subset::put(out, m, &data).map_err(|e| format!("{e:#}"))?;
        written += 1;
        bytes += data.len() as u64;
        p.files_done += 1;
        p.bytes_done = bytes;
        progress(&p);
    }
    let pressing = match &marker {
        Some((f, _)) if known => format!("the known pressing ({f}), from the game's extraction"),
        Some((f, _)) => format!("another pressing ({f}): checked by the extraction's own index and the MD5 lists"),
        None => "an extraction without the game's marker: checked against the known pressing".to_string(),
    };
    let (report, index) = c.report(entries.len(), bytes, written, kept, pressing);
    subset::write_synced(out, INDEX_FILE, index.to_tsv().as_bytes()).map_err(|e| format!("{e:#}"))?;
    state["bytes"] = json!(bytes);
    state["complete"] = json!(true);
    write_state(&state)?;
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ssx_content::hash::{ContentHash, ContentIndex};

    struct Temp(PathBuf);
    impl Temp {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("ssx-server-extraction-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(dir.join("big/data0/data")).unwrap();
            Temp(dir)
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn marker(dir: &Path, index: Option<&str>) {
        let archives =
            ssx_assets::setup::ARCHIVES.iter().map(|a| (a.to_string(), json!({}))).collect::<serde_json::Map<_, _>>();
        let mut m = json!({ "format": 2, "fingerprint": "0123456789abcdef", "iso_size": 1, "complete": true,
            "finished": archives, "movies": {} });
        if let Some(text) = index {
            let rows = text.lines().count() - 1;
            m["content_index"] = json!({ "files": rows, "bytes": rows as u64 * 3, "blake3": ContentHash::of(text.as_bytes()).to_string() });
        }
        std::fs::write(dir.join(ssx_assets::setup::MARKER), m.to_string()).unwrap();
    }

    fn run(dir: &Path, out: &Path) -> Result<Report, String> {
        from_extraction(dir, out, &[], &BTreeSet::new(), &AtomicBool::new(false), &mut |_| {})
    }

    #[test]
    fn an_extraction_without_its_index_names_the_repair() {
        let (t, out) = (Temp::new("noindex"), Temp::new("noindex-out"));
        marker(&t.0, None);
        let e = run(&t.0, &out.0).unwrap_err();
        assert!(e.contains("Repair game data") && !e.contains("os error"), "{e}");
    }

    #[test]
    fn a_whole_extractions_index_is_the_reference() {
        let (t, out) = (Temp::new("whole"), Temp::new("whole-out"));
        let mut index = ContentIndex::default();
        for i in 0..5000 {
            index.entries.insert(format!("big/data0/data/m{i:05}.bin"), (3, ContentHash::of(b"abc")));
        }
        index.entries.insert("disc/data/common/fmv/alps.vp6".into(), (3, ContentHash::of(b"abc")));
        for path in index.entries.keys() {
            let file = t.0.join(path);
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(file, b"abc").unwrap();
        }
        let text = index.to_tsv();
        std::fs::write(t.0.join("content.tsv"), &text).unwrap();
        marker(&t.0, Some(&text));
        let report = run(&t.0, &out.0).unwrap();
        assert_eq!(report.written, 0);
    }
}
