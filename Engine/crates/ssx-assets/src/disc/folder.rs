use std::collections::{HashMap, HashSet};
use std::fmt;
use std::io::{ErrorKind, Read};
use std::path::{Path, PathBuf};

use serde_json::Value;
use ssx_content::hash::{ContentHash, ContentIndex};

use super::setup::{ARCHIVES, MOVIE_DIR, big_root, marker, safe_relative};

pub const INDEX_FILE: &str = "content.tsv";
// PORT(choice): 32 MiB, 17 times the index of the one pressing measured (12 928 rows, 1 879 517 bytes; its longest path
// 91 bytes); tuning: the indexes of the disc's other pressings (regions); a bound of the port's setup reader.
pub const MAX_INDEX_BYTES: u64 = 32 << 20;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum State {
    Complete { marked: bool, files: usize },
    Missing,
    Unfinished,
    NoMovies,
    NoIndex,
    IndexDamaged(String),
    MarkerDamaged(String),
    FilesDamaged(String),
}

impl State {
    pub fn playable(&self) -> bool {
        matches!(self, State::Complete { .. } | State::NoIndex | State::IndexDamaged(_))
    }

    pub fn is_complete(&self) -> bool {
        matches!(self, State::Complete { .. })
    }

    pub fn needs_repair(&self) -> bool {
        matches!(
            self,
            State::NoMovies
                | State::NoIndex
                | State::IndexDamaged(_)
                | State::MarkerDamaged(_)
                | State::FilesDamaged(_)
        )
    }
}

impl fmt::Display for State {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            State::Complete { marked: true, files } => write!(f, "complete ({files} files in its content index)"),
            State::Complete { marked: false, files } => {
                write!(f, "an archive tree without a setup marker ({files} files in its content index)")
            }
            State::Missing => f.write_str("not set up"),
            State::Unfinished => f.write_str("its setup has not finished"),
            State::NoMovies => f.write_str("set up by an earlier version: no movies and no content index"),
            State::NoIndex => f.write_str("set up by an earlier version: no content index"),
            State::IndexDamaged(why) => write!(f, "its content index is damaged ({why})"),
            State::MarkerDamaged(why) => write!(f, "its setup marker is damaged ({why})"),
            State::FilesDamaged(why) => write!(f, "files of it are missing or damaged ({why})"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Incomplete {
    pub data: PathBuf,
    pub state: State,
}

impl fmt::Display for Incomplete {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let todo = match self.state {
            State::Missing => "set it up in the launcher (Game Data, Extract game data)",
            State::Unfinished => "finish it in the launcher (Game Data, Extract game data)",
            _ => "repair it in the launcher (Game Data, Repair game data)",
        };
        write!(f, "the game data folder {}: {}: {todo}", self.data.display(), self.state)
    }
}

impl std::error::Error for Incomplete {}

#[derive(Clone, Debug)]
pub struct Folder {
    pub state: State,
    pub index: Option<ContentIndex>,
}

pub fn check(data: &Path) -> Folder {
    let only = |state| Folder { state, index: None };
    let marker = match marker(data) {
        Ok(m) => m,
        Err(why) => return only(State::MarkerDamaged(why)),
    };
    let Some(marker) = marker else {
        if !is_tree(data) {
            return only(State::Missing);
        }
        return match read_index(data) {
            Ok(None) => only(State::NoIndex),
            Ok(Some(text)) => verified(data, &text, None),
            Err(why) => only(State::IndexDamaged(why)),
        };
    };
    match marker["format"].as_u64() {
        Some(1 | 2) => {}
        Some(n) if n > super::setup::MARKER_FORMAT => {
            return only(State::MarkerDamaged(format!("format {n}, of a newer version")));
        }
        _ => return only(State::MarkerDamaged("no known format".into())),
    }
    if marker["complete"] != Value::Bool(true) {
        return only(State::Unfinished);
    }
    if marker["format"].as_u64() == Some(1) {
        return only(State::NoMovies);
    }
    if let Some(missing) = ARCHIVES.iter().find(|a| !marker["finished"][**a].is_object()) {
        return only(State::MarkerDamaged(format!("complete, without {missing} among the finished archives")));
    }
    if !marker["movies"].is_object() {
        return only(State::MarkerDamaged("complete, without the movies".into()));
    }
    let recorded = &marker["content_index"];
    if recorded.is_null() {
        return only(State::NoIndex);
    }
    let recorded = match Recorded::of(recorded) {
        Ok(r) => r,
        Err(why) => return only(State::MarkerDamaged(why)),
    };
    match read_index(data) {
        Ok(Some(text)) => verified(data, &text, Some(&recorded)),
        Ok(None) => only(State::IndexDamaged(format!("{INDEX_FILE} is missing"))),
        Err(why) => only(State::IndexDamaged(why)),
    }
}

pub fn state(data: &Path) -> State {
    check(data).state
}

pub fn content_index(data: &Path) -> Result<ContentIndex, Incomplete> {
    let folder = check(data);
    folder.index.ok_or_else(|| Incomplete { data: data.to_path_buf(), state: folder.state })
}

fn is_tree(data: &Path) -> bool {
    let big = big_root(data);
    big.join("cache1/data/xenon/attribdb").is_dir() && big.join("data0").is_dir()
}

fn verified(data: &Path, text: &str, recorded: Option<&Recorded>) -> Folder {
    match verify_index(text, recorded) {
        Ok(index) => {
            let damage = check_files(data, &index);
            if !damage.is_empty() {
                return Folder { state: State::FilesDamaged(damage.to_string()), index: None };
            }
            let state = State::Complete { marked: recorded.is_some(), files: index.entries.len() };
            Folder { state, index: Some(index) }
        }
        Err(why) => Folder { state: State::IndexDamaged(why), index: None },
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Fault {
    Missing,
    NotPlain,
    Size(u64),
    Unreadable(String),
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FileDamage {
    pub rows: Vec<(String, Fault)>,
}

impl FileDamage {
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    pub fn below(&self, prefix: &str) -> bool {
        self.rows.iter().any(|(path, _)| path.starts_with(prefix))
    }
}

impl fmt::Display for FileDamage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let count = |pick: fn(&Fault) -> bool| self.rows.iter().filter(|(_, fault)| pick(fault)).count();
        let counts = [
            (count(|x| *x == Fault::Missing), "missing"),
            (count(|x| *x == Fault::NotPlain), "not a plain file"),
            (count(|x| matches!(x, Fault::Size(_))), "of another size"),
            (count(|x| matches!(x, Fault::Unreadable(_))), "unreadable"),
        ];
        let shown: Vec<_> = counts.iter().filter(|(n, _)| *n > 0).map(|(n, what)| format!("{n} {what}")).collect();
        let files = if self.rows.len() == 1 { "file" } else { "files" };
        write!(f, "{} {files}: {}", self.rows.len(), shown.join(", "))?;
        if let Some((path, fault)) = self.rows.first() {
            let path = ssx_core::text::quoted(path);
            match fault {
                Fault::Missing => write!(f, "; first {path}, missing")?,
                Fault::NotPlain => write!(f, "; first {path}, not a plain file")?,
                Fault::Size(n) => write!(f, "; first {path}, {n} bytes")?,
                Fault::Unreadable(why) => write!(f, "; first {path}: {}", ssx_core::text::quoted(why))?,
            }
        }
        Ok(())
    }
}

pub fn check_files(data: &Path, index: &ContentIndex) -> FileDamage {
    let mut folders: HashMap<PathBuf, Option<Fault>> = HashMap::new();
    let mut damage = FileDamage::default();
    for (path, (size, _)) in &index.entries {
        if let Some(fault) = file_fault(data, path, *size, &mut folders) {
            damage.rows.push((path.clone(), fault));
        }
    }
    damage
}

fn file_fault(data: &Path, path: &str, size: u64, folders: &mut HashMap<PathBuf, Option<Fault>>) -> Option<Fault> {
    let Some(rel) = safe_relative(path) else {
        return Some(Fault::Unreadable("no path of the setup's".into()));
    };
    let mut dir = data.to_path_buf();
    let mut parts = rel.components().peekable();
    while let Some(part) = parts.next() {
        if parts.peek().is_none() {
            break;
        }
        dir.push(part);
        let fault = folders.entry(dir.clone()).or_insert_with(|| match std::fs::symlink_metadata(&dir) {
            Ok(m) if m.is_dir() => None,
            Ok(_) => Some(Fault::NotPlain),
            Err(e) if e.kind() == ErrorKind::NotFound => Some(Fault::Missing),
            Err(e) => Some(Fault::Unreadable(e.to_string())),
        });
        if fault.is_some() {
            return fault.clone();
        }
    }
    match std::fs::symlink_metadata(data.join(&rel)) {
        Ok(m) if m.is_file() && m.len() == size => None,
        Ok(m) if m.is_file() => Some(Fault::Size(m.len())),
        Ok(_) => Some(Fault::NotPlain),
        Err(e) if e.kind() == ErrorKind::NotFound => Some(Fault::Missing),
        Err(e) => Some(Fault::Unreadable(e.to_string())),
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Recorded {
    pub files: u64,
    pub bytes: u64,
    pub blake3: Option<ContentHash>,
}

impl Recorded {
    pub fn of(v: &Value) -> Result<Self, String> {
        let bad = || "its content_index is malformed".to_string();
        let files = v["files"].as_u64().ok_or_else(bad)?;
        let bytes = v["bytes"].as_u64().ok_or_else(bad)?;
        let blake3 = match &v["blake3"] {
            Value::Null => None,
            Value::String(s) => Some(s.parse().map_err(|_| bad())?),
            _ => return Err(bad()),
        };
        Ok(Recorded { files, bytes, blake3 })
    }
}

pub fn read_index(data: &Path) -> Result<Option<String>, String> {
    read_text(data, INDEX_FILE)
}

pub fn read_text(dir: &Path, name: &str) -> Result<Option<String>, String> {
    let path = dir.join(name);
    let file = match ssx_files::open_plain(&path) {
        Ok(Some(file)) => file,
        Ok(None) => return Err(format!("{name} is a link or a folder, not a file")),
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.to_string()),
    };
    let mut bytes = Vec::new();
    file.take(MAX_INDEX_BYTES + 1).read_to_end(&mut bytes).map_err(|e| format!("{}: {e}", path.display()))?;
    if bytes.len() as u64 > MAX_INDEX_BYTES {
        return Err(format!("{name} is larger than {MAX_INDEX_BYTES} bytes"));
    }
    String::from_utf8(bytes).map(Some).map_err(|_| format!("{name} is not UTF-8 text"))
}

pub fn verify_index(text: &str, recorded: Option<&Recorded>) -> Result<ContentIndex, String> {
    let index = ContentIndex::from_tsv(text).map_err(|e| format!("{INDEX_FILE}: {e}"))?;
    if index.to_tsv() != text {
        let line = index.to_tsv().lines().zip(text.lines()).position(|(a, b)| a != b);
        let at = line.map_or_else(|| "at its end".to_string(), |l| format!("at line {}", l + 1));
        return Err(format!("{INDEX_FILE} is not in its one form (sorted rows, each ended by a line feed) {at}"));
    }
    let mut folded = HashSet::with_capacity(index.entries.len());
    let mut bytes = 0u64;
    for (path, (size, _)) in &index.entries {
        let shown = ssx_core::text::quoted(path);
        if !setup_path(path) {
            return Err(format!("{INDEX_FILE} lists {shown}, which is no file of the setup's"));
        }
        if !folded.insert(path.to_ascii_lowercase()) {
            return Err(format!("{INDEX_FILE} lists {shown} twice but for case"));
        }
        bytes = bytes.checked_add(*size).ok_or_else(|| format!("{INDEX_FILE}'s sizes overflow"))?;
    }
    if let Some(r) = recorded {
        if r.files != index.entries.len() as u64 || r.bytes != bytes {
            return Err(format!(
                "{INDEX_FILE} lists {} files of {bytes} bytes, the marker records {} of {}",
                index.entries.len(),
                r.files,
                r.bytes
            ));
        }
        if r.blake3.is_some_and(|h| h != ContentHash::of(text.as_bytes())) {
            return Err(format!("{INDEX_FILE} is not the one the setup wrote (its BLAKE3 differs from the marker's)"));
        }
    }
    Ok(index)
}

pub fn setup_path(path: &str) -> bool {
    if let Some(rest) = path.strip_prefix("big/") {
        let Some((archive, member)) = rest.split_once('/') else { return false };
        return ARCHIVES.contains(&archive) && safe_relative(member).is_some();
    }
    if let Some(rest) = path.strip_prefix("disc/") {
        let lower = rest.to_ascii_lowercase();
        let name = lower.strip_prefix(MOVIE_DIR).and_then(|r| r.strip_prefix('/'));
        return name.is_some_and(|n| !n.contains('/') && n.ends_with(".vp6")) && safe_relative(rest).is_some();
    }
    false
}

#[cfg(test)]
pub(crate) mod tests;
