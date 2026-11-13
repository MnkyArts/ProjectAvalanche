pub mod cache;
#[cfg(test)]
mod tests;

use std::collections::BTreeSet;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use ssx_core::text::{PATH_CHARS, VALUE_CHARS, quoted_max, shown_max};

use crate::hash::ContentHash;
use crate::limits::MAX_FILE_BYTES;
use crate::path::PackageTree;

pub use cache::Cached;

pub const CACHE_FOLDER: &str = "Blocklists";
pub const MAX_ENTRIES: usize = 100_000;
pub const MAX_LISTS: usize = 9;
// PORT(choice): 30 days: a master down for days keeps ruling; one gone for a month stops (it may have unblocked files
// meanwhile). Nothing measured; tuning: the outages of real masters; the original has no master server.
pub const MAX_AGE: Duration = Duration::from_secs(30 * 24 * 3600);
// PORT(choice): one day (a clock set back by hours, a time zone's error, keeps the list; a file dated days ahead is not
// one this game wrote). Nothing measured; tuning: the clock errors players' machines show; the original has no master
// server.
pub const FUTURE_SLACK: Duration = Duration::from_secs(24 * 3600);

// PORT(choice): 10 minutes (a list changes when an operator edits a file; masters answer a 304 cheaply); tuning: the
// request rates real masters see; the original has no master server.
pub const REFRESH_INTERVAL: Duration = Duration::from_secs(10 * 60);

pub fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct List {
    pub source: String,
    hashes: Arc<[ContentHash]>,
}

impl List {
    pub fn new(source: String, hashes: Vec<ContentHash>) -> Result<Self, String> {
        if hashes.len() > MAX_ENTRIES {
            return Err(format!("{} entries, a list holds at most {MAX_ENTRIES}", hashes.len()));
        }
        if let Some(w) = hashes.windows(2).find(|w| w[0] >= w[1]) {
            return Err(format!("not sorted or {} twice", w[1]));
        }
        Ok(Self { source, hashes: hashes.into() })
    }

    pub fn len(&self) -> usize {
        self.hashes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.hashes.is_empty()
    }

    pub fn contains(&self, hash: &ContentHash) -> bool {
        self.hashes.binary_search(hash).is_ok()
    }

    pub fn hashes(&self) -> &[ContentHash] {
        &self.hashes
    }
}

pub fn from_hex_list(list: &[String]) -> Result<Vec<ContentHash>, String> {
    if list.len() > MAX_ENTRIES {
        return Err(format!("{} entries, a list holds at most {MAX_ENTRIES}", list.len()));
    }
    let mut out: Vec<ContentHash> = Vec::with_capacity(list.len());
    for (i, h) in list.iter().enumerate() {
        out.push(h.parse().map_err(|e| format!("entry {i}: {e}"))?);
    }
    out.sort_unstable();
    if let Some(w) = out.windows(2).find(|w| w[0] == w[1]) {
        return Err(format!("{} twice", w[0]));
    }
    Ok(out)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hit {
    pub path: String,
    pub hash: ContentHash,
    pub source: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Blocked {
    lists: Vec<List>,
}

impl Blocked {
    pub fn new(lists: Vec<List>) -> Self {
        Self { lists }
    }

    pub fn lists(&self) -> &[List] {
        &self.lists
    }

    pub fn is_empty(&self) -> bool {
        self.lists.iter().all(List::is_empty)
    }

    pub fn entries(&self) -> usize {
        self.lists.iter().map(List::len).sum()
    }

    pub fn source_of(&self, hash: &ContentHash) -> Option<&str> {
        self.lists.iter().find(|l| l.contains(hash)).map(|l| l.source.as_str())
    }

    pub fn hits<'a>(&self, files: impl IntoIterator<Item = (&'a str, &'a ContentHash)>) -> Vec<Hit> {
        if self.is_empty() {
            return Vec::new();
        }
        files
            .into_iter()
            .filter_map(|(path, hash)| {
                let source = self.source_of(hash)?;
                Some(Hit { path: path.to_string(), hash: *hash, source: source.to_string() })
            })
            .collect()
    }

    pub fn blocked_of<'a>(&self, hashes: impl IntoIterator<Item = &'a ContentHash>) -> BTreeSet<ContentHash> {
        hashes.into_iter().filter(|h| self.source_of(h).is_some()).copied().collect()
    }
}

pub fn describe(what: &str, hits: &[Hit]) -> String {
    let Some(first) = hits.first() else { return format!("{what}: nothing blocked") };
    let more = hits.len() - 1;
    let short: String = first.hash.to_string().chars().take(16).collect();
    format!(
        "{what} holds a file the blocklist of {} names: {} (BLAKE3 {short}…){}",
        shown_max(&first.source, PATH_CHARS),
        quoted_max(&first.path, VALUE_CHARS * 4),
        if more > 0 { format!(", and {more} more blocked files") } else { String::new() }
    )
}

pub fn hash_tree(tree: &PackageTree) -> Result<Vec<(String, ContentHash)>, String> {
    let paths: Vec<String> = tree.files().map(str::to_string).collect();
    let mut out = Vec::with_capacity(paths.len());
    for p in paths {
        let (_, hash) = tree
            .copy_to(&p, MAX_FILE_BYTES, &mut std::io::sink())
            .map_err(|e| format!("{}: {e}", quoted_max(&p, VALUE_CHARS * 4)))?;
        out.push((p, hash));
    }
    Ok(out)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum State {
    Applied { entries: usize, age: Duration, fetched: u64 },
    NotYet,
    Expired { entries: usize, age: Duration },
    Unreadable(String),
}

#[derive(Clone, Debug, Default)]
pub struct Followed {
    pub blocked: Blocked,
    pub states: Vec<(String, State)>,
    pub problem: Option<String>,
}

impl Followed {
    pub fn lines(&self) -> Vec<String> {
        let mut out: Vec<String> = self.states.iter().map(|(m, s)| format!("blocklist of {}", s.line(m))).collect();
        if let Some(p) = &self.problem {
            out.push(format!("blocklists: none followed: {p}"));
        } else if self.states.is_empty() {
            out.push("blocklists: no master followed".into());
        }
        out
    }
}

pub fn player_master() -> Result<String, String> {
    let master = ssx_core::master::master();
    master.url().map(str::to_string).ok_or_else(|| format!("no master: {}", master.describe()))
}

pub fn for_player(app_dir: &Path, now: u64) -> Followed {
    match player_master() {
        Ok(master) => load(&app_dir.join(CACHE_FOLDER), &[master], now),
        Err(problem) => Followed { problem: Some(problem), ..Followed::default() },
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Follow {
    #[default]
    Nothing,
    Player(std::path::PathBuf),
    Masters {
        dir: std::path::PathBuf,
        masters: Vec<String>,
    },
}

impl Follow {
    pub fn load(&self, now: u64) -> Followed {
        match self {
            Follow::Nothing => Followed::default(),
            Follow::Player(app_dir) => for_player(app_dir, now),
            Follow::Masters { dir, masters } => load(dir, masters, now),
        }
    }

    pub fn masters(&self) -> (std::path::PathBuf, Vec<String>) {
        match self {
            Follow::Nothing => (std::path::PathBuf::new(), Vec::new()),
            Follow::Player(app_dir) => (app_dir.join(CACHE_FOLDER), player_master().into_iter().collect()),
            Follow::Masters { dir, masters } => (dir.clone(), masters.clone()),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Answer {
    List { hashes: Vec<ContentHash>, etag: Option<String> },
    NotModified,
}

pub fn etag_of(dir: &Path, master: &str) -> Option<String> {
    cache::read(dir, master).ok().flatten().and_then(|c| c.etag)
}

pub fn due(dir: &Path, master: &str, now: u64) -> bool {
    match cache::read(dir, master) {
        Ok(Some(c)) => c.age(now).map_or(true, |age| age >= REFRESH_INTERVAL) || c.checked > now,
        _ => true,
    }
}

pub fn record(dir: &Path, master: &str, answer: Answer, now: u64) -> Result<usize, String> {
    let kept = match answer {
        Answer::List { hashes, etag } => {
            let etag = etag.filter(|e| {
                !e.is_empty() && e.len() <= cache::MAX_ETAG && e.bytes().all(|b| (0x21..=0x7E).contains(&b))
            });
            Cached { master: master.to_string(), etag, fetched: now, checked: now, hashes }
        }
        Answer::NotModified => match cache::read(dir, master) {
            Ok(Some(mut c)) => {
                c.checked = now.max(c.fetched);
                c
            }
            Ok(None) | Err(_) => return Err("answered 304 Not Modified, but no list of it is kept".into()),
        },
    };
    let n = kept.hashes.len();
    cache::write(dir, &kept)?;
    Ok(n)
}

pub fn load(dir: &Path, masters: &[String], now: u64) -> Followed {
    let mut out = Followed::default();
    for master in masters.iter().take(MAX_LISTS) {
        let state = match cache::read(dir, master) {
            Ok(None) => State::NotYet,
            Err(e) => State::Unreadable(e),
            Ok(Some(c)) => match c.age(now) {
                Err(e) => State::Unreadable(e),
                Ok(age) if age > MAX_AGE => State::Expired { entries: c.hashes.len(), age },
                Ok(age) => {
                    let (entries, fetched) = (c.hashes.len(), c.fetched);
                    match List::new(master.clone(), c.hashes) {
                        Ok(list) => {
                            out.blocked.lists.push(list);
                            State::Applied { entries, age, fetched }
                        }
                        Err(e) => State::Unreadable(e),
                    }
                }
            },
        };
        out.states.push((master.clone(), state));
    }
    out
}

impl State {
    pub fn line(&self, master: &str) -> String {
        let days = |d: &Duration| d.as_secs_f64() / 86_400.0;
        match self {
            State::Applied { entries, age, .. } => {
                format!("{}: {entries} blocked files, checked {:.2} days ago", shown_max(master, PATH_CHARS), days(age))
            }
            State::NotYet => format!("{}: no list kept yet", shown_max(master, PATH_CHARS)),
            State::Expired { entries, age } => format!(
                "{}: its list of {entries} files is {:.1} days old, past {} days: not applied",
                shown_max(master, PATH_CHARS),
                days(age),
                MAX_AGE.as_secs() / 86_400
            ),
            State::Unreadable(e) => {
                format!("{}: its kept list does not read ({e}): not applied", shown_max(master, PATH_CHARS))
            }
        }
    }
}
