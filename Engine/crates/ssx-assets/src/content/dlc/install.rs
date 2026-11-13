use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use ssx_formats::dlc::{DlcPackage, DlcToc, MAX_INFO_BYTES, MAX_TOC_BYTES};
use ssx_formats::xcontent::XContentPackage;

use super::intake::{self, Candidate, Staging};
use super::{DlcList, EnabledEntry, MAX_MEMBER_PATH};

pub const MAX_PACKAGE_BYTES: u64 = 1 << 30;
pub const MAX_EXTRACT_BYTES: u64 = 8 << 30;
pub const CONTENT_TYPE_TITLE_UPDATE: u32 = 0x000B_0000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Exists {
    pub folder: String,
    pub display: String,
}

#[derive(Debug)]
pub enum InstallError {
    Exists(Exists),
    Failed(String),
}

impl std::fmt::Display for InstallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            InstallError::Exists(e) => write!(f, "{} is already installed", e.display),
            InstallError::Failed(m) => f.write_str(m),
        }
    }
}

impl std::error::Error for InstallError {}

impl From<String> for InstallError {
    fn from(message: String) -> Self {
        InstallError::Failed(message)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Installed {
    pub folder: String,
    pub display: String,
    pub content_version: Option<u32>,
    pub mount_version: Option<u32>,
    pub members: usize,
    pub bytes: u64,
    pub package: String,
}

#[derive(Clone, Debug, Default)]
pub struct InstallReport {
    pub installed: Vec<Installed>,
    pub tu: Option<super::super::title_update::TuInstallReport>,
    pub skipped: Vec<(String, String)>,
    pub notes: Vec<String>,
}

impl InstallReport {
    pub fn lines(&self) -> Vec<String> {
        let mut out = Vec::new();
        for p in &self.installed {
            out.push(format!(
                "installed {} ({}) — contentVersion {}, mountversion {}, {} members, {} bytes",
                p.display,
                p.folder,
                p.content_version.map_or_else(|| "-".to_string(), |v| v.to_string()),
                p.mount_version.map_or_else(|| "-".to_string(), |v| v.to_string()),
                p.members,
                p.bytes
            ));
        }
        if let Some(tu) = &self.tu {
            out.extend(tu.lines());
        }
        out.extend(self.notes.iter().cloned());
        for (file, reason) in &self.skipped {
            out.push(format!("{file}: {reason}"));
        }
        out
    }
}

pub fn install(
    source: &Path,
    dlc_root: &Path,
    schema_root: Option<&Path>,
    replace: bool,
) -> Result<InstallReport, InstallError> {
    ssx_files::create_dir_all(dlc_root).map_err(|e| InstallError::Failed(format!("{}: {e}", dlc_root.display())))?;
    let guard = Staging::new(dlc_root)?;
    let staging = guard.path().to_path_buf();
    let mut report = InstallReport::default();

    let candidates = intake::candidates(source, &staging, MAX_PACKAGE_BYTES, &mut report.skipped)?;

    let mut staged_packages: BTreeMap<String, Staged> = BTreeMap::new();
    let mut tu_candidates: Vec<(String, PathBuf)> = Vec::new();
    for (name, path) in candidates {
        let bytes = match intake::read_package(&path, MAX_PACKAGE_BYTES).map_err(InstallError::Failed)? {
            Candidate::Package(bytes) => bytes,
            Candidate::Skip(why) => {
                report.skipped.push((name, why));
                continue;
            }
        };
        let container = XContentPackage::parse(&bytes)
            .map_err(|e| InstallError::Failed(format!("{name}: a content package that does not parse: {e}")))?;
        container
            .verify()
            .map_err(|e| InstallError::Failed(format!("{name}: a damaged or altered content package: {e}")))?;
        match container.metadata().content_type {
            CONTENT_TYPE_TITLE_UPDATE => {
                stage_file(&staging, &name, &path).map_err(InstallError::Failed)?;
                tu_candidates.push((name.clone(), staging.join("files").join(&name)));
                continue;
            }
            2 => {}
            other => {
                report.skipped.push((name, format!("content type {other:#010x}, not DLC")));
                continue;
            }
        }
        let package = DlcPackage::parse(&bytes).map_err(|e| InstallError::Failed(format!("{name}: {e}")))?;
        let info = package.info();
        let folder = info.name.clone();
        let (content_version, _unique) = match package.package().read("dlc/dimecontentinfo.xml") {
            Ok(xml) => super::dime(&xml),
            Err(_) => (None, None),
        };
        let rank = (content_version.unwrap_or(0), info.mount_version().unwrap_or(0));
        let key = folder.to_ascii_lowercase();
        if let Some(old) = staged_packages.get(&key) {
            if rank <= old.rank {
                report.skipped.push((
                    name,
                    format!(
                        "the same mount name {folder} with contentVersion {}, mountversion {} is newer",
                        old.rank.0, old.rank.1
                    ),
                ));
                continue;
            }
            report.skipped.push((old.package.clone(), "superseded by a newer copy in this intake".into()));
            let _ = std::fs::remove_dir_all(&old.dir);
            staged_packages.remove(&key);
        }
        let installed = intake::installed_folder(dlc_root, &folder);
        if let Some(existing) = &installed
            && !replace
        {
            return Err(InstallError::Exists(Exists {
                folder: existing.clone(),
                display: info.display_name().unwrap_or(&folder).to_string(),
            }));
        }
        let dir = staging.join("out").join(&folder);
        let extracted = extract_package(&package, &dir, schema_root, &mut report.notes)?;
        let package_name = name.clone();
        let display = info.display_name().unwrap_or(&folder).to_string();
        stage_file(&staging, &name, &path).map_err(InstallError::Failed)?;
        staged_packages.insert(
            key,
            Staged {
                folder,
                dir,
                package: package_name,
                rank,
                display,
                members: extracted.members,
                bytes: extracted.bytes,
                content_version,
                mount_version: info.mount_version(),
            },
        );
    }
    if staged_packages.is_empty() && tu_candidates.is_empty() {
        return Err(InstallError::Failed("no DLC package or title update was found in the source".to_string()));
    }
    let retail_header = schema_root.and_then(super::super::title_update::retail_header);
    if staged_packages.is_empty() {
        let mut tu_report = super::super::title_update::TuInstallReport::default();
        super::super::title_update::install_files(
            &tu_candidates,
            dlc_root,
            retail_header.as_deref(),
            replace,
            &mut tu_report,
        )
        .map_err(tu_error)?;
        report.tu = Some(tu_report);
        drop(guard);
        return Ok(report);
    }

    let old_list = std::fs::read(dlc_root.join(super::LIST_FILE)).ok();
    let mut created_dirs: Vec<PathBuf> = Vec::new();
    let mut moved_aside: Vec<(PathBuf, PathBuf)> = Vec::new();
    let mut created_files: Vec<PathBuf> = Vec::new();
    let rollback = |created_dirs: &[PathBuf], moved_aside: &[(PathBuf, PathBuf)], created_files: &[PathBuf]| {
        for path in created_files {
            let _ = std::fs::remove_file(path);
        }
        for path in created_dirs {
            let _ = std::fs::remove_dir_all(path);
        }
        for (target, aside) in moved_aside {
            let _ = std::fs::rename(aside, target);
        }
        match &old_list {
            Some(bytes) => {
                let _ = ssx_files::replace(&dlc_root.join(super::LIST_FILE), bytes, &ssx_files::Options::new());
            }
            None => {
                let _ = std::fs::remove_file(dlc_root.join(super::LIST_FILE));
            }
        }
    };
    let commit = (|| -> Result<(), InstallError> {
        ssx_files::create_dir_all(dlc_root).map_err(|e| InstallError::Failed(e.to_string()))?;
        for staged in staged_packages.values() {
            let target = intake::installed_folder(dlc_root, &staged.folder)
                .map_or_else(|| dlc_root.join(&staged.folder), |name| dlc_root.join(name));
            if target.exists() {
                let aside = staging.join("old").join(&staged.folder);
                ssx_files::create_dir_all(aside.parent().expect("aside has a parent"))
                    .map_err(|e| InstallError::Failed(e.to_string()))?;
                std::fs::rename(&target, &aside)
                    .map_err(|e| InstallError::Failed(format!("{}: {e}", target.display())))?;
                moved_aside.push((target.clone(), aside));
            }
            std::fs::rename(&staged.dir, &target)
                .map_err(|e| InstallError::Failed(format!("{}: {e}", target.display())))?;
            created_dirs.push(target);
        }
        for name in staged_packages.values().map(|s| s.package.clone()).collect::<Vec<_>>() {
            let staged = staging.join("files").join(&name);
            if !staged.is_file() {
                continue;
            }
            let target = dlc_root.join(&name);
            if target.exists() {
                report.notes.push(format!("{name}: a package file of this name stays as it is"));
                continue;
            }
            std::fs::rename(&staged, &target)
                .map_err(|e| InstallError::Failed(format!("{}: {e}", target.display())))?;
            created_files.push(target);
        }
        let mut list = DlcList::read(dlc_root).map_err(InstallError::Failed)?.unwrap_or_default();
        let mut new_entries: Vec<&Staged> = staged_packages.values().collect();
        new_entries.sort_by_key(|s| s.rank);
        for staged in new_entries {
            if !list.entries.iter().any(|e| e.folder == staged.folder) {
                list.entries.push(EnabledEntry { folder: staged.folder.clone(), enabled: true });
            }
        }
        list.write(dlc_root).map_err(InstallError::Failed)
    })();
    if let Err(e) = commit {
        rollback(&created_dirs, &moved_aside, &created_files);
        return Err(e);
    }
    if !tu_candidates.is_empty() {
        let mut tu_report = super::super::title_update::TuInstallReport::default();
        match super::super::title_update::install_files(
            &tu_candidates,
            dlc_root,
            retail_header.as_deref(),
            replace,
            &mut tu_report,
        ) {
            Ok(()) => report.tu = Some(tu_report),
            Err(e) => {
                rollback(&created_dirs, &moved_aside, &created_files);
                return Err(tu_error(e));
            }
        }
    }
    for staged in staged_packages.into_values() {
        report.installed.push(Installed {
            folder: staged.folder,
            display: staged.display,
            content_version: staged.content_version,
            mount_version: staged.mount_version,
            members: staged.members,
            bytes: staged.bytes,
            package: staged.package,
        });
    }
    drop(guard);
    Ok(report)
}

struct Staged {
    folder: String,
    dir: PathBuf,
    package: String,
    rank: (u32, u32),
    display: String,
    members: usize,
    bytes: u64,
    content_version: Option<u32>,
    mount_version: Option<u32>,
}

fn stage_file(staging: &Path, name: &str, path: &Path) -> Result<(), String> {
    let dir = staging.join("files");
    ssx_files::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let target = dir.join(name);
    std::fs::copy(path, &target).map(|_| ()).map_err(|e| format!("{}: {e}", target.display()))
}

fn tu_error(e: super::super::title_update::InstallTuError) -> InstallError {
    match e {
        super::super::title_update::InstallTuError::Exists(e) => {
            InstallError::Exists(Exists { folder: e.folder, display: e.display })
        }
        super::super::title_update::InstallTuError::Failed(m) => InstallError::Failed(format!("the title update: {m}")),
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Extracted {
    pub members: usize,
    pub bytes: u64,
}

pub fn extract_package(
    package: &DlcPackage<'_>,
    dir: &Path,
    schema_root: Option<&Path>,
    notes: &mut Vec<String>,
) -> Result<Extracted, InstallError> {
    let fail = |message: String| InstallError::Failed(message);
    if dir.exists() {
        return Err(fail(format!("{}: already exists", dir.display())));
    }
    ssx_files::create_dir_all(dir).map_err(|e| fail(e.to_string()))?;
    let write = |rel: &str, bytes: &[u8]| -> Result<(), InstallError> {
        let path = dir.join(rel);
        if let Some(parent) = path.parent() {
            ssx_files::create_dir_all(parent).map_err(|e| fail(e.to_string()))?;
        }
        std::fs::write(&path, bytes).map_err(|e| fail(format!("{}: {e}", path.display())))
    };
    for (name, cap) in [("info.dlc", MAX_INFO_BYTES), ("dlc.toc", MAX_TOC_BYTES)] {
        let bytes = package.package().read(name).map_err(|e| fail(format!("{name}: {e}")))?;
        if bytes.len() > cap {
            return Err(fail(format!("{name}: {} bytes (at most {cap})", bytes.len())));
        }
        write(name, &bytes)?;
    }
    if let Ok(xml) = package.package().read("dlc/dimecontentinfo.xml") {
        if xml.len() as u64 > super::MAX_DIME_BYTES {
            notes.push(format!("dimecontentinfo.xml over {} bytes: not written", super::MAX_DIME_BYTES));
        } else {
            write("dimecontentinfo.xml", &xml)?;
        }
    }
    let mut lower = BTreeSet::new();
    let mut total = 0u64;
    let mut members = 0usize;
    for entry in package.members() {
        let path = entry.path();
        if path.len() > MAX_MEMBER_PATH {
            return Err(fail(format!(
                "member {:?} is over {MAX_MEMBER_PATH} bytes",
                ssx_core::text::shown_max(&path, 120)
            )));
        }
        ssx_content::path::check_package_path(&path, false).map_err(|e| fail(format!("member {path:?}: {e}")))?;
        if path.starts_with("bootcache/") || path == "info.dlc" || path == "dlc.toc" || path == "dimecontentinfo.xml" {
            return Err(fail(format!("member {path:?} uses a name the extracted package keeps for itself")));
        }
        if !lower.insert(path.to_ascii_lowercase()) {
            return Err(fail(format!("member {path:?} differs from another only in case")));
        }
        total = total.checked_add(u64::from(entry.size)).ok_or_else(|| fail("member bytes overflow".into()))?;
        if total > MAX_EXTRACT_BYTES {
            return Err(fail(format!("the package's members are over {MAX_EXTRACT_BYTES} bytes")));
        }
        let bytes = package.read_entry(entry).map_err(|e| fail(format!("{path}: {e}")))?;
        if bytes.len() != entry.size as usize {
            return Err(fail(format!("{path}: {} bytes decoded, {} declared", bytes.len(), entry.size)));
        }
        write(&path, &bytes)?;
        members += 1;
    }
    match schema_root {
        Some(schema) => match super::prepare_decode_root(dir, schema) {
            Ok(()) => {}
            Err(e) => notes.push(format!("{}: records are not decoded: {e}", package.info().name)),
        },
        None => notes.push(format!(
            "{}: no disc schema given, so its object records are not decoded (extract it with --schema)",
            package.info().name
        )),
    }
    Ok(Extracted { members, bytes: total })
}

pub fn prepare_decode_root(package_root: &Path, schema_root: &Path) -> Result<(), String> {
    let schema = schema_root.join("bootcache/data/xenon/attribdb");
    let target = package_root.join("bootcache/data/xenon/attribdb");
    ssx_files::create_dir_all(&target).map_err(|e| e.to_string())?;
    let copy = |src: &Path, dst: &Path| -> Result<(), String> {
        let bytes = std::fs::read(src).map_err(|e| format!("{}: {e}", src.display()))?;
        ssx_files::replace(dst, &bytes, &ssx_files::Options::new()).map_err(|e| e.to_string())
    };
    for name in ["attribdb_schema.vlt", "attribdb_schema.bin"] {
        copy(&schema.join(name), &target.join(name))?;
    }
    let attr = package_root.join("data/xenon/attribdb");
    if attr.join("ge_attribdb.vlt").is_file() {
        for name in ["ge_attribdb.vlt", "ge_attribdb.bin"] {
            copy(&attr.join(name), &target.join(name))?;
        }
    }
    let shaders = package_root.join("data/xenon/shaders");
    if shaders.is_dir() {
        let out = package_root.join("bootcache/data/xenon/shaders");
        ssx_files::create_dir_all(&out).map_err(|e| e.to_string())?;
        let mut seen = BTreeSet::new();
        for path in crate::content::vaults::files_with_ext(&shaders, "fxo").map_err(|e| format!("{e:#}"))? {
            let name = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
            if seen.insert(name.to_ascii_lowercase()) {
                copy(&path, &out.join(&name))?;
            }
        }
    }
    Ok(())
}

pub fn read_installed_toc(root: &Path) -> Result<DlcToc, String> {
    let path = root.join("dlc.toc");
    let bytes = intake::read_file_at_most(&path, MAX_TOC_BYTES as u64)?;
    DlcToc::parse(&bytes).map_err(|e| format!("{}: {e}", path.display()))
}
