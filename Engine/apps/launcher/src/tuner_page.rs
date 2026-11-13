use std::path::{Path, PathBuf};

use eframe::egui;
use ssx_assets::content::tuner::{self, InstallError, Loaded, Report, TUNER_DIR, TUNER_FILE};

use crate::app::Launcher;
use crate::jobs::Background;

impl Launcher {
    pub fn refresh_tuner(&mut self, ctx: &egui::Context) {
        self.tuner.refresh(ctx, self.tuner_root.clone());
    }
}

pub struct Snapshot {
    pub installed: Option<Loaded>,
    pub bytes: Option<u64>,
    pub error: Option<String>,
}

#[derive(Default)]
pub struct TunerPage {
    pub snapshot: Option<Snapshot>,
    job: Option<Background<Snapshot>>,
    pub install: Option<Background<InstallEnd>>,
    pub message: Option<(bool, String)>,
    pub confirm: Option<Confirm>,
    base: Option<PathBuf>,
}

pub enum Confirm {
    Replace { source: PathBuf, tuner: bool, profile: bool },
    Remove,
}

pub enum InstallEnd {
    Done(Report),
    Exists { source: PathBuf, tuner: bool, profile: bool },
    Failed(String),
}

fn snapshot(dir: PathBuf) -> Snapshot {
    let Some(path) = tuner::installed(&dir) else {
        return Snapshot { installed: None, bytes: None, error: None };
    };
    let bytes = std::fs::metadata(&path).ok().map(|m| m.len());
    match tuner::read(&path) {
        Ok(Some(loaded)) => Snapshot { installed: Some(loaded), bytes, error: None },
        Ok(None) => Snapshot { installed: None, bytes, error: Some("the file holds no tuner pack".into()) },
        Err(e) => Snapshot { installed: None, bytes, error: Some(format!("{e:#}")) },
    }
}

impl TunerPage {
    pub fn root(app_dir: &Path) -> PathBuf {
        app_dir.join(TUNER_DIR)
    }

    pub fn busy(&self) -> bool {
        self.job.is_some() || self.install.is_some()
    }

    pub fn refresh(&mut self, ctx: &egui::Context, dir: PathBuf) {
        self.base = Some(dir.clone());
        if self.job.is_some() {
            return;
        }
        self.job = Some(Background::spawn(ctx, move || snapshot(dir)));
    }

    pub fn needs_refresh(&self, dir: &Path) -> bool {
        self.base.as_deref() != Some(dir)
    }

    pub fn poll(&mut self) -> bool {
        let mut due = false;
        if let Some(s) = self.job.as_ref().and_then(Background::take) {
            self.snapshot = Some(s);
            self.job = None;
        }
        if let Some(end) = self.install.as_ref().and_then(Background::take) {
            self.install = None;
            due = true;
            match end {
                InstallEnd::Done(report) => self.message = Some((true, report.lines().join("\n"))),
                InstallEnd::Exists { source, tuner, profile } => {
                    self.confirm = Some(Confirm::Replace { source, tuner, profile })
                }
                InstallEnd::Failed(e) => self.message = Some((false, e)),
            }
        }
        due
    }

    pub fn start_install(&mut self, ctx: &egui::Context, dir: PathBuf, save: PathBuf, source: PathBuf, replace: bool) {
        self.message = None;
        self.install = Some(Background::spawn(ctx, move || {
            let again = source.clone();
            match tuner::install(&source, &dir, Some(&save), replace) {
                Ok(report) => InstallEnd::Done(report),
                Err(InstallError::Exists { tuner, profile }) => InstallEnd::Exists { source: again, tuner, profile },
                Err(e) => InstallEnd::Failed(format!("{e}")),
            }
        }));
    }

    pub fn remove(&mut self, dir: &Path) -> bool {
        let path = dir.join(TUNER_FILE);
        match std::fs::remove_file(&path) {
            Ok(()) => {
                self.message = Some((true, format!("{} removed", path.display())));
                true
            }
            Err(e) => {
                self.message = Some((false, format!("{} not removed: {e}", path.display())));
                false
            }
        }
    }
}
