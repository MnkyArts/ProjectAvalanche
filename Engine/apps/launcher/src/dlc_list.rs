use std::path::{Path, PathBuf};
use std::time::Instant;

use eframe::egui;
use ssx_assets::content::dlc::{self, DLC_DIR, DlcList, DlcSet, EnabledEntry, InstallError, InstallReport};

use crate::app::Launcher;
use crate::jobs::Background;

impl Launcher {
    pub fn refresh_dlc(&mut self, ctx: &egui::Context) {
        let big = self.data_ready.then(|| self.settings.big_root());
        self.dlc.refresh(ctx, self.dlc_root.clone(), big);
    }
}

pub struct Snapshot {
    pub set: DlcSet,
    pub seconds: f64,
}

pub enum Confirm {
    Replace { source: PathBuf, folder: String, name: String },
    Delete { folder: String, why: String },
}

pub enum InstallEnd {
    Done(InstallReport),
    Exists { source: PathBuf, folder: String, name: String },
    Failed(String),
}

#[derive(Default)]
pub struct DlcPage {
    pub snapshot: Option<Snapshot>,
    job: Option<Background<Snapshot>>,
    stale: bool,
    pub install: Option<Background<InstallEnd>>,
    pub confirm: Option<Confirm>,
    pub message: Option<(bool, String)>,
    base: Option<(PathBuf, Option<PathBuf>)>,
}

fn snapshot(dlc_root: PathBuf) -> Snapshot {
    let start = Instant::now();
    let set = DlcSet::load(&dlc_root);
    Snapshot { set, seconds: start.elapsed().as_secs_f64() }
}

impl DlcPage {
    pub fn root(app_dir: &Path) -> PathBuf {
        app_dir.join(DLC_DIR)
    }

    pub fn busy(&self) -> bool {
        self.job.is_some() || self.install.is_some()
    }

    pub fn refresh(&mut self, ctx: &egui::Context, dlc_root: PathBuf, big_root: Option<PathBuf>) {
        self.base = Some((dlc_root.clone(), big_root));
        if self.job.is_some() {
            self.stale = true;
            return;
        }
        self.job = Some(Background::spawn(ctx, move || snapshot(dlc_root)));
    }

    pub fn needs_refresh(&self, dlc_root: &Path, big_root: &Option<PathBuf>) -> bool {
        self.base.as_ref().is_none_or(|(r, b)| r != dlc_root || b != big_root)
    }

    pub fn poll(&mut self) -> bool {
        let mut due = false;
        if let Some(s) = self.job.as_ref().and_then(Background::take) {
            self.snapshot = Some(s);
            self.job = None;
            due = std::mem::take(&mut self.stale);
        }
        if let Some(end) = self.install.as_ref().and_then(Background::take) {
            self.install = None;
            due = true;
            match end {
                InstallEnd::Done(report) => {
                    self.message = Some((true, report.lines().join("\n")));
                }
                InstallEnd::Exists { source, folder, name } => {
                    self.confirm = Some(Confirm::Replace { source, folder, name });
                }
                InstallEnd::Failed(e) => self.message = Some((false, e)),
            }
        }
        due
    }

    pub fn set_enabled(&mut self, dlc_root: &Path, folder: &str, on: bool) -> bool {
        let Some(snapshot) = &self.snapshot else { return false };
        let mut list = match DlcList::read(dlc_root) {
            Ok(Some(list)) => list,
            Ok(None) => DlcList {
                entries: snapshot
                    .set
                    .packages
                    .iter()
                    .map(|p| EnabledEntry { folder: p.folder.clone(), enabled: p.enabled })
                    .collect(),
            },
            Err(e) => {
                self.message = Some((false, format!("dlc.toml not written: {e}")));
                return false;
            }
        };
        match list.entries.iter_mut().find(|e| e.folder == folder) {
            Some(e) => e.enabled = on,
            None => return false,
        }
        match list.write(dlc_root) {
            Ok(()) => true,
            Err(e) => {
                self.message = Some((false, format!("dlc.toml not written: {e}")));
                false
            }
        }
    }

    pub fn remove(&mut self, dlc_root: &Path, folder: &str, permanently: bool) -> bool {
        let path = dlc_root.join(folder);
        let done = if permanently {
            std::fs::remove_dir_all(&path).map_err(|e| e.to_string())
        } else {
            trash::delete(&path).map_err(|e| e.to_string())
        };
        match done {
            Ok(()) => {
                let how = if permanently { "deleted" } else { "moved to the trash" };
                self.message = Some((true, format!("{folder}: {how}")));
                self.forget(dlc_root, folder)
            }
            Err(why) if !permanently => {
                self.confirm = Some(Confirm::Delete { folder: folder.to_string(), why });
                false
            }
            Err(e) => {
                self.message = Some((false, format!("{folder}: not deleted: {e}")));
                false
            }
        }
    }

    pub fn forget(&mut self, dlc_root: &Path, folder: &str) -> bool {
        let Some(snapshot) = &self.snapshot else { return false };
        let list = match DlcList::read(dlc_root) {
            Ok(Some(list)) => DlcList { entries: list.entries.into_iter().filter(|e| e.folder != folder).collect() },
            Ok(None) => DlcList {
                entries: snapshot
                    .set
                    .packages
                    .iter()
                    .filter(|p| p.folder != folder)
                    .map(|p| EnabledEntry { folder: p.folder.clone(), enabled: p.enabled })
                    .collect(),
            },
            Err(e) => {
                self.message = Some((false, format!("dlc.toml not written: {e}")));
                return false;
            }
        };
        match list.write(dlc_root) {
            Ok(()) => true,
            Err(e) => {
                self.message = Some((false, format!("dlc.toml not written: {e}")));
                false
            }
        }
    }

    pub fn start_install(
        &mut self,
        ctx: &egui::Context,
        dlc_root: PathBuf,
        big_root: Option<PathBuf>,
        source: PathBuf,
        replace: bool,
    ) {
        self.message = None;
        let Some(big_root) = big_root else {
            self.message =
                Some((false, "the game data must be set up first (its vault schema decodes the packages)".into()));
            return;
        };
        self.install = Some(Background::spawn(ctx, move || {
            let again = source.clone();
            match dlc::install(&source, &dlc_root, Some(&big_root), replace) {
                Ok(report) => InstallEnd::Done(report),
                Err(InstallError::Exists(exists)) => {
                    InstallEnd::Exists { source: again, folder: exists.folder, name: exists.display }
                }
                Err(e) => InstallEnd::Failed(format!("{e}")),
            }
        }));
    }
}
