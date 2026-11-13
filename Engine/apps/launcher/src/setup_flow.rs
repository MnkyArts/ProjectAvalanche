use eframe::egui;

use crate::app::Launcher;
use crate::jobs::{self, Background, SetupEnd, SetupJob};
use crate::ui::Page;

pub fn gb(bytes: u64) -> String {
    format!("{:.2} GB", bytes as f64 / 1e9)
}

pub struct SetupProgress {
    pub fraction: f32,
    pub line: String,
}

impl Launcher {
    pub fn poll(&mut self, ctx: &egui::Context) {
        self.update.poll(ctx);
        if let Some(result) = self.checking.as_ref().and_then(Background::take) {
            self.checking = None;
            if result.is_ok()
                && self.auto_extract
                && self.job.is_none()
                && let Some(iso) = self.settings.iso.clone()
            {
                self.job = Some(SetupJob::start(ctx, iso, self.settings.game_data()));
            }
            self.check = Some(result);
        }
        if let Some(end) = self.job.as_ref().and_then(SetupJob::end) {
            self.job = None;
            self.setup_message = Some(match end {
                SetupEnd::Done(s) => {
                    self.data_changed();
                    self.go(Page::Play);
                    let failed = if s.failures.is_empty() {
                        String::new()
                    } else {
                        format!(", {} member(s) skipped (truncated on the disc itself)", s.failures.len())
                    };
                    (
                        true,
                        format!(
                            "Setup finished: {} archives read, {} files written, {} kept, {} repaired, {} movies \
                             copied{failed}.",
                            s.extracted.len(),
                            s.members_written,
                            s.members_kept + s.movies_kept,
                            s.members_replaced + s.movies_replaced,
                            s.movies_written
                        ),
                    )
                }
                SetupEnd::Cancelled => (false, "Setup cancelled. Start it again to resume where it stopped.".into()),
                SetupEnd::Failed(e) => (false, format!("Setup failed: {e}")),
            });
        }
        self.displays.poll();
        if self.job.is_some() || self.checking.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(250));
        }
    }

    pub fn pick_iso(&mut self, ctx: &egui::Context) {
        let mut dialog = rfd::FileDialog::new()
            .set_title("Select the SSX (2012) Xbox 360 disc image")
            .add_filter("Disc image", &["iso", "ISO"])
            .add_filter("All files", &["*"]);
        if let Some(dir) = self.settings.iso.as_ref().and_then(|p| p.parent()) {
            dialog = dialog.set_directory(dir);
        }
        if let Some(path) = dialog.pick_file() {
            self.settings.iso = Some(path.clone());
            self.check = None;
            self.checking = Some(jobs::inspect(ctx, path));
        }
    }

    pub fn pick_data_dir(&mut self) {
        let start = self.settings.game_data();
        let dialog = rfd::FileDialog::new().set_title("Folder for the game data (about 4.7 GB)");
        let dialog = match start.parent().filter(|p| p.is_dir()) {
            Some(p) => dialog.set_directory(p),
            None => dialog,
        };
        if let Some(dir) = dialog.pick_folder() {
            self.settings.game_data = Some(dir);
            self.data_changed();
        }
    }

    pub fn check_remembered_image(&mut self, ctx: &egui::Context) {
        if self.check.is_none()
            && self.checking.is_none()
            && self.job.is_none()
            && let Some(iso) = self.settings.iso.clone().filter(|p| p.is_file())
        {
            self.checking = Some(jobs::inspect(ctx, iso));
        }
    }

    pub fn start_setup(&mut self, ctx: &egui::Context) {
        if let Some(iso) = self.settings.iso.clone() {
            self.setup_message = None;
            self.job = Some(SetupJob::start(ctx, iso, self.settings.game_data()));
        }
    }

    pub fn setup_progress(&self) -> Option<SetupProgress> {
        let job = self.job.as_ref()?;
        let p = job.progress();
        let fraction = if p.bytes_total == 0 { 0.0 } else { p.bytes_done as f32 / p.bytes_total as f32 };
        let secs = job.started.elapsed().as_secs_f32().max(0.1);
        let rate = p.bytes_done as f32 / secs;
        let left = if rate > 0.0 { (p.bytes_total - p.bytes_done) as f32 / rate } else { 0.0 };
        let line = format!(
            "{} ({}/{})  ·  {} / {} files  ·  {} / {}  ·  {:.0} MB/s  ·  about {:.0} s left",
            p.archive,
            (p.archive_index + 1).min(p.archive_count.max(1)),
            p.archive_count,
            p.files_done,
            p.files_total,
            gb(p.bytes_done),
            gb(p.bytes_total),
            rate / 1e6,
            left
        );
        Some(SetupProgress { fraction, line })
    }
}
