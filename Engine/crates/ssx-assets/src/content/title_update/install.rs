use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use ssx_formats::tu::{TitleUpdatePackage, version_text};

use crate::content::dlc::intake::{self, Candidate, Staging};

use super::{BuildDate, EnabledUpdate, MAX_PACKAGE_BYTES, MAX_UPDATES, TuList, root};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TuInstalled {
    pub folder: String,
    pub name: String,
    pub version: u32,
    pub build_date: BuildDate,
    pub has_patch: bool,
    pub members: usize,
    pub bytes: u64,
    pub replaced: bool,
}

impl TuInstalled {
    pub fn version_text(&self) -> String {
        version_text(self.version)
    }
}

#[derive(Clone, Debug, Default)]
pub struct TuInstallReport {
    pub installed: Vec<TuInstalled>,
    pub skipped: Vec<(String, String)>,
    pub notes: Vec<String>,
}

impl TuInstallReport {
    pub fn lines(&self) -> Vec<String> {
        let mut out = Vec::new();
        for u in &self.installed {
            out.push(format!(
                "installed title update {} ({}, XEX {}, {}) — patch0.big {}",
                u.name,
                u.folder,
                u.version_text(),
                u.build_date,
                if u.has_patch { format!("{} members, {} bytes", u.members, u.bytes) } else { "none".to_string() }
            ));
        }
        out.extend(self.notes.iter().cloned());
        for (file, reason) in &self.skipped {
            out.push(format!("{file}: {reason}"));
        }
        out
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Exists {
    pub folder: String,
    pub display: String,
}

#[derive(Debug)]
pub enum InstallTuError {
    Exists(Exists),
    Failed(String),
}

impl std::fmt::Display for InstallTuError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            InstallTuError::Exists(e) => write!(f, "{} is already installed", e.display),
            InstallTuError::Failed(m) => f.write_str(m),
        }
    }
}

impl std::error::Error for InstallTuError {}

impl From<String> for InstallTuError {
    fn from(message: String) -> Self {
        InstallTuError::Failed(message)
    }
}

pub fn install(
    source: &Path,
    dlc_root: &Path,
    retail_header: Option<&[u8]>,
    replace: bool,
) -> Result<TuInstallReport, InstallTuError> {
    let tu_root = root(dlc_root);
    ssx_files::create_dir_all(&tu_root).map_err(|e| InstallTuError::Failed(format!("{}: {e}", tu_root.display())))?;
    let staging = Staging::new(&tu_root)?;
    let mut report = TuInstallReport::default();
    let candidates = intake::candidates(source, staging.path(), MAX_PACKAGE_BYTES, &mut report.skipped)?;
    stage_and_commit(candidates, &tu_root, retail_header, replace, staging.path(), &mut report)?;
    drop(staging);
    Ok(report)
}

pub fn install_files(
    files: &[(String, PathBuf)],
    dlc_root: &Path,
    retail_header: Option<&[u8]>,
    replace: bool,
    report: &mut TuInstallReport,
) -> Result<(), InstallTuError> {
    let tu_root = root(dlc_root);
    ssx_files::create_dir_all(&tu_root).map_err(|e| InstallTuError::Failed(format!("{}: {e}", tu_root.display())))?;
    let staging = Staging::new(&tu_root)?;
    stage_and_commit(files.to_vec(), &tu_root, retail_header, replace, staging.path(), report)?;
    drop(staging);
    Ok(())
}

pub fn installed_folders(dlc_root: &Path) -> Vec<String> {
    super::folders(&root(dlc_root))
}

struct Staged {
    folder: String,
    name: String,
    dir: PathBuf,
    version: u32,
    build_date: BuildDate,
    has_patch: bool,
    members: usize,
    bytes: u64,
}

fn stage_and_commit(
    candidates: Vec<(String, PathBuf)>,
    tu_root: &Path,
    retail_header: Option<&[u8]>,
    replace: bool,
    staging: &Path,
    report: &mut TuInstallReport,
) -> Result<(), InstallTuError> {
    let mut staged: BTreeMap<String, Staged> = BTreeMap::new();
    for (name, path) in candidates {
        let bytes = match intake::read_package(&path, MAX_PACKAGE_BYTES).map_err(InstallTuError::Failed)? {
            Candidate::Package(bytes) => bytes,
            Candidate::Skip(why) => {
                report.skipped.push((name, why));
                continue;
            }
        };
        let update = match TitleUpdatePackage::parse(&bytes) {
            Ok(u) => u,
            Err(e) => {
                report.skipped.push((name, format!("not a title update: {e}")));
                continue;
            }
        };
        update
            .package()
            .verify()
            .map_err(|e| InstallTuError::Failed(format!("{name}: a damaged or altered title update: {e}")))?;
        let version = update.target_version().map_err(|e| InstallTuError::Failed(format!("{name}: {e}")))?;
        let (_, build_date) = BuildDate::identify(&update, retail_header);
        let folder = folder_name(&name, version);
        let key = folder.to_ascii_lowercase();
        if let Some(old) = staged.get(&key) {
            if old.version >= version {
                report.skipped.push((
                    name,
                    format!(
                        "the same update {} with XEX version {} is in this intake",
                        old.folder,
                        version_text(old.version)
                    ),
                ));
                continue;
            }
            report.skipped.push((old.name.clone(), "superseded by a newer copy in this intake".into()));
            let _ = std::fs::remove_dir_all(&old.dir);
            staged.remove(&key);
        }
        if staged.len() >= MAX_UPDATES {
            report.skipped.push((name, format!("more than {MAX_UPDATES} title updates")));
            continue;
        }
        let dir = staging.join("out").join(&folder);
        let extracted = extract(&update, &dir).map_err(|e| InstallTuError::Failed(format!("{name}: {e}")))?;
        let installed = intake::installed_folder(tu_root, &folder);
        if let Some(existing) = &installed
            && !replace
        {
            return Err(InstallTuError::Exists(Exists { folder: existing.clone(), display: name }));
        }
        staged.insert(
            key,
            Staged {
                folder,
                name,
                dir,
                version,
                build_date,
                has_patch: extracted.has_patch,
                members: extracted.members,
                bytes: extracted.bytes,
            },
        );
    }
    if staged.is_empty() {
        return Err(InstallTuError::Failed("no title update was found in the source".into()));
    }

    let mut installed_dirs: Vec<PathBuf> = Vec::new();
    let mut moved_aside: Vec<(PathBuf, PathBuf)> = Vec::new();
    let commit = (|| -> Result<(), InstallTuError> {
        for staged in staged.values() {
            let target = intake::installed_folder(tu_root, &staged.folder)
                .map_or_else(|| tu_root.join(&staged.folder), |name| tu_root.join(name));
            if target.exists() {
                let aside = staging.join("old").join(&staged.folder);
                ssx_files::create_dir_all(aside.parent().expect("aside has a parent"))
                    .map_err(|e| InstallTuError::Failed(e.to_string()))?;
                std::fs::rename(&target, &aside)
                    .map_err(|e| InstallTuError::Failed(format!("{}: {e}", target.display())))?;
                moved_aside.push((target.clone(), aside));
            }
            ssx_files::install_folder(&staged.dir, &target)
                .map_err(|e| InstallTuError::Failed(format!("{}: {e}", target.display())))?;
            installed_dirs.push(target);
        }
        let mut list = TuList::read(tu_root).map_err(InstallTuError::Failed)?.unwrap_or_default();
        for staged in staged.values() {
            match list.entries.iter_mut().find(|e| e.folder.eq_ignore_ascii_case(&staged.folder)) {
                Some(e) => {
                    e.name = staged.name.clone();
                    e.enabled = true;
                }
                None => list.entries.push(EnabledUpdate {
                    folder: staged.folder.clone(),
                    name: staged.name.clone(),
                    enabled: true,
                }),
            }
        }
        list.write(tu_root).map_err(InstallTuError::Failed)
    })();
    if let Err(e) = commit {
        for path in &installed_dirs {
            let _ = std::fs::remove_dir_all(path);
        }
        for (target, aside) in &moved_aside {
            let _ = std::fs::rename(aside, target);
        }
        return Err(e);
    }
    for staged in staged.into_values() {
        let replaced = moved_aside.iter().any(|(target, _)| target.ends_with(&staged.folder));
        report.installed.push(TuInstalled {
            folder: staged.folder,
            name: staged.name,
            version: staged.version,
            build_date: staged.build_date,
            has_patch: staged.has_patch,
            members: staged.members,
            bytes: staged.bytes,
            replaced,
        });
    }
    Ok(())
}

struct Extracted {
    has_patch: bool,
    members: usize,
    bytes: u64,
}

fn extract(update: &TitleUpdatePackage<'_>, dir: &Path) -> Result<Extracted, String> {
    if dir.exists() {
        return Err(format!("{}: already exists", dir.display()));
    }
    ssx_files::create_dir_all(dir).map_err(|e| e.to_string())?;
    let write = |rel: &str, bytes: &[u8]| -> Result<(), String> {
        let path = dir.join(rel);
        if let Some(parent) = path.parent() {
            ssx_files::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        std::fs::write(&path, bytes).map_err(|e| format!("{}: {e}", path.display()))
    };
    write("package", update.data())?;
    write("default.xexp", update.xexp())?;
    let mut members = 0usize;
    let mut total = 0u64;
    let has_patch = match update.patch() {
        Some(patch) => {
            write("patch0.big", patch.data())?;
            let mut lower = BTreeSet::new();
            for entry in patch.members() {
                let path = entry.path();
                ssx_formats::tu::check_member_path(&path).map_err(|e| e.to_string())?;
                if !path.starts_with("data/") {
                    return Err(format!("member {path:?} is not a game data path"));
                }
                ssx_content::path::check_package_path(&path, false).map_err(|e| format!("member {path:?}: {e}"))?;
                if !lower.insert(path.to_ascii_lowercase()) {
                    return Err(format!("member {path:?} differs from another only in case"));
                }
                let bytes = patch.decode(entry).map_err(|e| e.to_string())?;
                total = total.checked_add(bytes.len() as u64).ok_or("member bytes overflow")?;
                if total > ssx_formats::tu::MAX_PATCH_TOTAL_BYTES {
                    return Err(format!(
                        "the patch's members are over {} bytes",
                        ssx_formats::tu::MAX_PATCH_TOTAL_BYTES
                    ));
                }
                write(&path, &bytes)?;
                members += 1;
            }
            true
        }
        None => false,
    };
    Ok(Extracted { has_patch, members, bytes: total })
}

fn folder_name(name: &str, version: u32) -> String {
    let mut out: String =
        name.chars().map(|c| if c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.') { c } else { '_' }).collect();
    if out.len() > 64 {
        out.truncate(64);
    }
    while out.ends_with(['.', ' ']) {
        out.pop();
    }
    if ssx_content::path::check_package_path(&out, false).is_err() {
        out = format!("tu-{}", version_text(version).replace('.', "_"));
    }
    out
}
