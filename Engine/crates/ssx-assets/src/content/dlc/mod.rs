mod install;
pub(crate) mod intake;
#[cfg(test)]
mod tests;
pub(crate) mod zip;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::{OnceLock, RwLock};

use serde::{Deserialize, Serialize};
use ssx_content::path::check_package_path;
use ssx_formats::dlc::{DlcInfo, DlcToc, TITLE_ID};

pub use install::{
    Exists, InstallError, InstallReport, Installed, extract_package, install, prepare_decode_root, read_installed_toc,
};

pub const DLC_DIR: &str = "DLC";
pub const LIST_FILE: &str = "dlc.toml";
pub const LIST_FORMAT: u32 = 1;
pub const MAX_LIST_BYTES: u64 = 256 << 10;
pub const MAX_PACKAGES: usize = 64;
pub const MAX_UNLOCKABLES_BYTES: u64 = 64 << 10;
pub const MAX_UNLOCKABLE_TOKENS: usize = 256;
pub const MAX_UNLOCKABLES: usize = 1024;
pub const MAX_MEMBER_PATH: usize = 512;
pub const MAX_DIME_BYTES: u64 = 16 << 10;

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EnabledEntry {
    pub folder: String,
    pub enabled: bool,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ListFile {
    format: u32,
    #[serde(default)]
    packages: Vec<EnabledEntry>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DlcList {
    pub entries: Vec<EnabledEntry>,
}

impl DlcList {
    pub fn parse(bytes: &[u8], file: &str) -> Result<Self, String> {
        if bytes.len() as u64 > MAX_LIST_BYTES {
            return Err(format!("{file}: over {MAX_LIST_BYTES} bytes"));
        }
        let text = std::str::from_utf8(bytes).map_err(|e| format!("{file}: not UTF-8: {e}"))?;
        let parsed: ListFile = toml::from_str(text).map_err(|e| format!("{file}: {e}"))?;
        if parsed.format != LIST_FORMAT {
            let f = parsed.format;
            return Err(format!("{file}: format {f}; this build reads format {LIST_FORMAT}"));
        }
        if parsed.packages.len() > MAX_PACKAGES {
            return Err(format!("{file}: {} entries (at most {MAX_PACKAGES})", parsed.packages.len()));
        }
        let mut seen = BTreeSet::new();
        for e in &parsed.packages {
            if e.folder.contains('/') {
                return Err(format!("{file}: {:?} is not a folder name below {DLC_DIR}/", e.folder));
            }
            check_package_path(&e.folder, false).map_err(|m| format!("{file}: {m}"))?;
            if !seen.insert(e.folder.to_ascii_lowercase()) {
                return Err(format!("{file}: {:?} is listed twice", e.folder));
            }
        }
        Ok(DlcList { entries: parsed.packages })
    }

    pub fn to_toml(&self) -> String {
        toml::to_string(&ListFile { format: LIST_FORMAT, packages: self.entries.clone() }).unwrap_or_default()
    }

    pub fn write(&self, root: &Path) -> Result<(), String> {
        let text = self.to_toml();
        Self::parse(text.as_bytes(), LIST_FILE)?;
        ssx_files::create_dir_all(root).map_err(|e| e.to_string())?;
        ssx_files::replace(&root.join(LIST_FILE), text.as_bytes(), &ssx_files::Options::new())
            .map_err(|e| e.to_string())
    }

    pub fn read(root: &Path) -> Result<Option<Self>, String> {
        let path = root.join(LIST_FILE);
        let name = path.display().to_string();
        match std::fs::symlink_metadata(&path) {
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(format!("{name}: {e}")),
        }
        let bytes = intake::read_file_at_most(&path, MAX_LIST_BYTES)?;
        Self::parse(&bytes, &name).map(Some)
    }
}

#[derive(Clone, Debug)]
pub struct DlcEntry {
    pub folder: String,
    pub root: PathBuf,
    pub enabled: bool,
    pub info: DlcInfo,
    pub toc: DlcToc,
    pub content_version: Option<u32>,
    pub unique_dlc_id: Option<String>,
    pub ready: bool,
    pub damaged: Option<String>,
}

impl DlcEntry {
    pub fn mount_version(&self) -> Option<u32> {
        self.info.mount_version()
    }

    pub fn display_name(&self) -> &str {
        self.info.display_name().unwrap_or(&self.info.name)
    }

    fn rank(&self) -> (u32, u32, &str, &str) {
        (self.content_version.unwrap_or(0), self.mount_version().unwrap_or(0), &self.info.name, &self.folder)
    }

    pub fn mount_order(&self) -> u32 {
        let mut h = 0x811C_9DC5u32;
        for b in self.info.name.bytes() {
            h = h.wrapping_mul(0x0100_0193) ^ u32::from(b);
        }
        h
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DlcSkip {
    pub folder: String,
    pub reason: String,
}

#[derive(Clone, Debug, Default)]
pub struct DlcSet {
    pub root: PathBuf,
    pub packages: Vec<DlcEntry>,
    pub skipped: Vec<DlcSkip>,
    pub list_error: Option<String>,
}

impl DlcSet {
    pub fn load(root: &Path) -> Self {
        let mut skipped = Vec::new();
        let list_result = DlcList::read(root);
        let (list, list_error) = match list_result {
            Ok(list) => (list, None),
            Err(e) => {
                skipped.push(DlcSkip { folder: LIST_FILE.to_string(), reason: e.clone() });
                (None, Some(e))
            }
        };
        let mut candidates: Vec<(String, bool)> = Vec::new();
        if list_error.is_none() {
            match &list {
                Some(list) => {
                    for e in &list.entries {
                        candidates.push((e.folder.clone(), e.enabled));
                    }
                }
                None => {
                    if let Ok(read) = std::fs::read_dir(root) {
                        for e in read.flatten() {
                            let name = e.file_name().to_string_lossy().into_owned();
                            if e.path().is_dir() && e.path().join("info.dlc").is_file() {
                                candidates.push((name, true));
                            }
                        }
                    }
                    candidates.sort();
                }
            }
        }
        let mut found: Vec<DlcEntry> = Vec::new();
        for (folder, enabled) in candidates {
            if folder.contains('/') || folder == "." || folder == ".." || check_package_path(&folder, false).is_err() {
                skipped.push(DlcSkip { folder, reason: "not a folder name below the DLC root".into() });
                continue;
            }
            match load_folder(&root.join(&folder), &folder, enabled) {
                Ok(entry) => found.push(entry),
                Err(reason) => skipped.push(DlcSkip { folder, reason }),
            }
        }
        let mut by_name: std::collections::BTreeMap<String, DlcEntry> = std::collections::BTreeMap::new();
        for entry in found {
            let key = entry.info.name.to_ascii_lowercase();
            match by_name.get(&key) {
                None => {
                    by_name.insert(key, entry);
                }
                Some(old) if entry.rank() > old.rank() => {
                    let reason = format!(
                        "duplicate of {} (contentVersion {}, mountversion {}): the newer one is mounted",
                        old.info.name,
                        old.content_version.unwrap_or(0),
                        old.mount_version().unwrap_or(0)
                    );
                    let old = std::mem::replace(by_name.get_mut(&key).expect("just read"), entry);
                    skipped.push(DlcSkip { folder: old.folder, reason });
                }
                Some(newer) => skipped.push(DlcSkip {
                    folder: entry.folder,
                    reason: format!(
                        "duplicate of {} (contentVersion {}, mountversion {}): the newer one is mounted",
                        newer.info.name,
                        newer.content_version.unwrap_or(0),
                        newer.mount_version().unwrap_or(0)
                    ),
                }),
            }
        }
        let mut packages: Vec<DlcEntry> = by_name.into_values().collect();
        packages.sort_by(|a, b| a.mount_order().cmp(&b.mount_order()).then_with(|| a.folder.cmp(&b.folder)));
        check_vaults(&mut packages);
        skipped.sort_by(|a, b| a.folder.cmp(&b.folder));
        DlcSet { root: root.to_path_buf(), packages, skipped, list_error }
    }

    pub fn roots(&self) -> Vec<PathBuf> {
        self.mounted().into_iter().map(|p| p.root.clone()).collect()
    }

    pub fn mounted(&self) -> Vec<&DlcEntry> {
        self.packages.iter().filter(|p| p.enabled && p.ready && p.damaged.is_none()).collect()
    }

    pub fn unlockables(&self) -> Unlockables {
        unlockables_from_roots(&self.roots())
    }

    pub fn lines(&self) -> Vec<ssx_core::text::Text> {
        self.line_strings().into_iter().map(ssx_core::text::Text::from).collect()
    }

    pub(super) fn line_strings(&self) -> Vec<String> {
        let mut out = Vec::new();
        for e in &self.packages {
            let state = if e.enabled { "on" } else { "off" };
            let ready = if e.ready { "" } else { ", NOT READY (reinstall it)" };
            let damaged = match &e.damaged {
                Some(why) => format!(", DAMAGED, not mounted: its vaults do not decode ({why}); install it again"),
                None => String::new(),
            };
            out.push(format!(
                "dlc: {} {} — contentVersion {}, mountversion {}, folder {}{ready}{damaged}",
                e.info.name,
                state,
                e.content_version.map_or_else(|| "-".to_string(), |v| v.to_string()),
                e.mount_version().map_or_else(|| "-".to_string(), |v| v.to_string()),
                e.folder
            ));
        }
        for s in &self.skipped {
            out.push(format!("dlc: skipped {}: {}", s.folder, s.reason));
        }
        out
    }
}

#[derive(Clone, Debug, Default)]
pub struct Unlockables {
    pub tokens: Vec<String>,
    pub problems: Vec<String>,
}

pub fn unlockables_from_roots(roots: &[PathBuf]) -> Unlockables {
    let mut out = Unlockables::default();
    let mut seen = BTreeSet::new();
    let mut files_seen = BTreeSet::new();
    for root in roots {
        let label = root.file_name().unwrap_or_default().to_string_lossy().into_owned();
        let dir = root.join("data/common/dlc");
        let Ok(read) = std::fs::read_dir(&dir) else { continue };
        let mut files: Vec<PathBuf> = read
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                p.is_file()
                    && p.file_name()
                        .and_then(|n| n.to_str())
                        .is_some_and(|n| n.starts_with("unlockables_") && n.ends_with(".txt"))
            })
            .collect();
        files.sort();
        for path in files {
            let name = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
            if !files_seen.insert(name.clone()) {
                continue;
            }
            let bytes = match intake::read_file_at_most(&path, MAX_UNLOCKABLES_BYTES) {
                Ok(b) => b,
                Err(e) => {
                    out.problems.push(format!("{label}/{name}: {e}"));
                    continue;
                }
            };
            let Ok(text) = std::str::from_utf8(&bytes) else {
                out.problems.push(format!("{label}/{name}: not UTF-8"));
                continue;
            };
            for (index, token) in text.split(',').enumerate() {
                if index >= MAX_UNLOCKABLE_TOKENS {
                    out.problems.push(format!("{label}/{name}: over {MAX_UNLOCKABLE_TOKENS} tokens"));
                    break;
                }
                let token = token.trim_matches([' ', '\t', '\n', '\r']);
                if token.is_empty() {
                    continue;
                }
                let portable = token.len() <= 128
                    && token.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'));
                if !portable {
                    out.problems.push(format!(
                        "{label}/{name}: {:?} is not a token of [A-Za-z0-9_.-]",
                        ssx_core::text::shown_max(token, 80)
                    ));
                    continue;
                }
                if out.tokens.len() >= MAX_UNLOCKABLES {
                    out.problems.push(format!("{label}/{name}: over {MAX_UNLOCKABLES} tokens in all"));
                    break;
                }
                if seen.insert(token.to_ascii_lowercase()) {
                    out.tokens.push(token.to_string());
                }
            }
        }
    }
    out
}

fn load_folder(root: &Path, folder: &str, enabled: bool) -> Result<DlcEntry, String> {
    let info_path = root.join("info.dlc");
    let toc_path = root.join("dlc.toc");
    let read = intake::read_file_at_most;
    let info = DlcInfo::parse(&read(&info_path, ssx_formats::dlc::MAX_INFO_BYTES as u64)?)
        .map_err(|e| format!("{}: {e}", info_path.display()))?;
    let toc = DlcToc::parse(&read(&toc_path, ssx_formats::dlc::MAX_TOC_BYTES as u64)?)
        .map_err(|e| format!("{}: {e}", toc_path.display()))?;
    let dime_path = root.join("dimecontentinfo.xml");
    let (content_version, unique_dlc_id) =
        if dime_path.exists() { dime(&read(&dime_path, MAX_DIME_BYTES)?) } else { (None, None) };
    let ready = root.join("data").is_dir();
    Ok(DlcEntry {
        folder: folder.to_string(),
        root: root.to_path_buf(),
        enabled,
        info,
        toc,
        content_version,
        unique_dlc_id,
        ready,
        damaged: None,
    })
}

const CHECK_THREADS: usize = 4;

fn check_vaults(packages: &mut [DlcEntry]) {
    let due: Vec<usize> = (0..packages.len()).filter(|&i| packages[i].enabled && packages[i].ready).collect();
    if due.is_empty() {
        return;
    }
    let next = std::sync::atomic::AtomicUsize::new(0);
    let workers = std::thread::available_parallelism().map_or(1, |n| n.get()).min(CHECK_THREADS).min(due.len());
    let roots: Vec<&Path> = due.iter().map(|&i| packages[i].root.as_path()).collect();
    let results: Vec<(usize, Option<String>)> = std::thread::scope(|scope| {
        let jobs: Vec<_> = (0..workers)
            .map(|_| {
                scope.spawn(|| {
                    let mut done = Vec::new();
                    loop {
                        let k = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        let Some(root) = roots.get(k) else { break };
                        let damaged = ssx_rules::vault_db::check_package_vaults(root)
                            .err()
                            .map(|e| ssx_core::text::shown_max(&format!("{e:#}"), 240).to_string());
                        done.push((k, damaged));
                    }
                    done
                })
            })
            .collect();
        jobs.into_iter().flat_map(|job| job.join().unwrap_or_else(|e| std::panic::resume_unwind(e))).collect()
    });
    for (k, damaged) in results {
        packages[due[k]].damaged = damaged;
    }
}

pub fn dime(xml: &[u8]) -> (Option<u32>, Option<String>) {
    let text = String::from_utf8_lossy(xml);
    let element = |name: &str| -> Option<String> {
        let open = format!("<{name}>");
        let close = format!("</{name}>");
        let start = text.find(&open)? + open.len();
        let end = text[start..].find(&close)? + start;
        Some(text[start..end].trim().to_string())
    };
    let version = element("contentVersion")
        .and_then(|v| (!v.is_empty() && v.bytes().all(|b| b.is_ascii_digit())).then(|| v.parse().ok()).flatten());
    (version, element("uniqueDlcId"))
}

pub const DLC_TITLE: u32 = TITLE_ID;

static ACTIVE: OnceLock<RwLock<Vec<PathBuf>>> = OnceLock::new();

pub fn set_active_roots(roots: Vec<PathBuf>) {
    ssx_rules::data_roots::set_overlays(roots.clone());
    let lock = ACTIVE.get_or_init(|| RwLock::new(Vec::new()));
    *lock.write().unwrap_or_else(std::sync::PoisonError::into_inner) = roots;
}

pub fn active_roots() -> Vec<PathBuf> {
    ACTIVE.get().map_or_else(Vec::new, |l| l.read().unwrap_or_else(std::sync::PoisonError::into_inner).clone())
}
