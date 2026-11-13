use std::path::{Path, PathBuf};
use std::time::Instant;

use eframe::egui;
use ssx_assets::content::title_update::{self, InstallTuError, TuInstallReport, TuSet};

use crate::app::Launcher;
use crate::dlc_list::Confirm;
use crate::jobs::Background;

impl Launcher {
    pub fn refresh_tu(&mut self, ctx: &egui::Context) {
        let big = self.data_ready.then(|| self.settings.big_root());
        self.tu.refresh(ctx, self.tu_root.clone(), big);
    }
}

pub struct Snapshot {
    pub set: TuSet,
    pub seconds: f64,
}

pub enum InstallEnd {
    Done(TuInstallReport),
    Exists { source: PathBuf, folder: String, name: String },
    Failed(String),
}

#[derive(Default)]
pub struct TuPage {
    pub snapshot: Option<Snapshot>,
    job: Option<Background<Snapshot>>,
    stale: bool,
    pub install: Option<Background<InstallEnd>>,
    pub confirm: Option<Confirm>,
    pub message: Option<(bool, String)>,
    base: Option<(PathBuf, Option<PathBuf>)>,
}

fn snapshot(tu_root: PathBuf, big_root: Option<PathBuf>) -> Snapshot {
    let start = Instant::now();
    let header = big_root.as_deref().and_then(title_update::retail_header);
    let set = TuSet::load(&tu_root, header.as_deref());
    Snapshot { set, seconds: start.elapsed().as_secs_f64() }
}

impl TuPage {
    pub fn root(app_dir: &Path) -> PathBuf {
        crate::dlc_list::DlcPage::root(app_dir).join("Title Updates")
    }

    pub fn busy(&self) -> bool {
        self.job.is_some() || self.install.is_some()
    }

    pub fn refresh(&mut self, ctx: &egui::Context, tu_root: PathBuf, big_root: Option<PathBuf>) {
        self.base = Some((tu_root.clone(), big_root.clone()));
        if self.job.is_some() {
            self.stale = true;
            return;
        }
        self.job = Some(Background::spawn(ctx, move || snapshot(tu_root, big_root)));
    }

    pub fn needs_refresh(&self, tu_root: &Path, big_root: &Option<PathBuf>) -> bool {
        self.base.as_ref().is_none_or(|(r, b)| r != tu_root || b != big_root)
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
                InstallEnd::Done(report) => self.message = Some((true, report.lines().join("\n"))),
                InstallEnd::Exists { source, folder, name } => {
                    self.confirm = Some(Confirm::Replace { source, folder, name });
                }
                InstallEnd::Failed(e) => self.message = Some((false, e)),
            }
        }
        due
    }

    pub fn set_enabled(&mut self, folder: &str, on: bool) -> bool {
        let Some(snapshot) = &self.snapshot else { return false };
        match snapshot.set.set_enabled(folder, on) {
            Ok(()) => true,
            Err(e) => {
                self.message = Some((false, format!("tu.toml not written: {e}")));
                false
            }
        }
    }

    pub fn remove(&mut self, tu_root: &Path, folder: &str, permanently: bool) -> bool {
        let path = tu_root.join(folder);
        let done = if permanently {
            std::fs::remove_dir_all(&path).map_err(|e| e.to_string())
        } else {
            trash::delete(&path).map_err(|e| e.to_string())
        };
        match done {
            Ok(()) => {
                let how = if permanently { "deleted" } else { "moved to the trash" };
                self.message = Some((true, format!("{folder}: {how}")));
                self.forget(folder)
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

    pub fn forget(&mut self, folder: &str) -> bool {
        let Some(snapshot) = &self.snapshot else { return false };
        match snapshot.set.forget(folder) {
            Ok(()) => true,
            Err(e) => {
                self.message = Some((false, format!("tu.toml not written: {e}")));
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
            self.message = Some((
                false,
                "the game data must be set up first (it holds the retail XEX the header patch decodes against)".into(),
            ));
            return;
        };
        self.install = Some(Background::spawn(ctx, move || {
            let again = source.clone();
            let header = title_update::retail_header(&big_root);
            match title_update::install(&source, &dlc_root, header.as_deref(), replace) {
                Ok(report) => InstallEnd::Done(report),
                Err(InstallTuError::Exists(exists)) => {
                    InstallEnd::Exists { source: again, folder: exists.folder, name: exists.display }
                }
                Err(e) => InstallEnd::Failed(format!("{e}")),
            }
        }));
    }
}
