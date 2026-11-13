use eframe::egui::{self, RichText, Ui};

use crate::app::Launcher;
use crate::ui::footer;
use crate::ui::theme::{self, Face, size};
use crate::ui::widgets::{self, Kind};
use crate::update::{Update, current_version};

impl Launcher {
    pub fn update_section(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            ui.label(RichText::new("Project Avalanche").font(Face::Title.id(size::TITLE)).color(theme::TEXT));
            ui.label(RichText::new(footer::version()).color(theme::TEXT_DIM));
        });
        widgets::paragraph(
            ui,
            "The launcher asks GitHub for the project's latest release when it starts. An update is downloaded from \
             the project's own releases, checked against the SHA-256 GitHub publishes, and unpacked beside this \
             launcher; settings, saves and game data stay where they are.",
        );
        ui.add_space(8.0);
        let mut repair = false;
        if !self.program_missing.is_empty() {
            let names = self.program_missing.join(", ");
            widgets::status(
                ui,
                theme::ERROR,
                format!(
                    "This installation lacks {} of its files: {names}. (Launcher 0.2.0 did not know them when it \
                     updated.) Without the movie decoder a mod's movies do not play.",
                    self.program_missing.len()
                ),
            );
            let can = matches!(self.update, Update::Idle | Update::UpToDate | Update::Failed(_));
            if matches!(&self.update, Update::Available(r) if r.reinstall.is_empty()) {
                widgets::note(ui, "The update below puts them back too.");
            } else if can && widgets::button(ui, "Repair the installation", Kind::Primary, true).clicked() {
                repair = true;
            }
            ui.add_space(8.0);
        }
        let mut check = false;
        let mut download = false;
        let mut apply = false;
        let mut discard = false;
        let mut open = None;
        let blocker = install_blocker(Busy {
            started: self.running(),
            server: self.host.server_running() || self.host.busy(),
            setup: self.job.is_some(),
            install: self.mods.install.is_some(),
        });
        match &self.update {
            Update::Idle => {
                widgets::status(ui, theme::TEXT_DIM, "Automatic checks are off.");
                if widgets::button(ui, "Check for updates", Kind::Primary, true).clicked() {
                    check = true;
                }
            }
            Update::Checking(_) => {
                widgets::status(ui, theme::CYAN_SOFT, "Asking GitHub for the latest release…");
            }
            Update::UpToDate => {
                widgets::status(ui, theme::GOOD, format!("This is the latest version (v{}).", current_version()));
                if widgets::button(ui, "Check again", Kind::Plain, true).clicked() {
                    check = true;
                }
            }
            Update::Failed(e) => {
                widgets::status(ui, theme::ERROR, e.clone());
                if widgets::button(ui, "Try again", Kind::Primary, true).clicked() {
                    check = true;
                }
            }
            Update::Available(release) if !release.reinstall.is_empty() => {
                widgets::status(
                    ui,
                    theme::CYAN,
                    format!(
                        "The release of v{} holds the missing files: it is downloaded again, checked against its \
                         SHA-256 and installed as an update is.",
                        release.version
                    ),
                );
                ui.add_space(8.0);
                if widgets::button(ui, "Download and repair", Kind::Primary, true).clicked() {
                    download = true;
                }
            }
            Update::Available(release) => {
                widgets::status(
                    ui,
                    theme::CYAN,
                    format!("Version v{} is available (this is v{}).", release.version, current_version()),
                );
                if !release.title.is_empty() && release.title != release.tag {
                    widgets::note(ui, &release.title);
                }
                if !release.notes.is_empty() {
                    ui.add_space(4.0);
                    widgets::heading(ui, "What changed");
                    let notes = truncate(&release.notes, 4000);
                    ui.label(RichText::new(notes).color(theme::TEXT_SOFT));
                }
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if widgets::button(ui, "Download and install", Kind::Primary, true).clicked() {
                        download = true;
                    }
                    if !release.page.is_empty() && widgets::button(ui, "Release page", Kind::Plain, true).clicked() {
                        open = Some(release.page.clone());
                    }
                });
            }
            Update::Downloading { release, progress, .. } => {
                let p = progress.lock().map(|p| p.clone()).unwrap_or_default();
                widgets::status(ui, theme::CYAN_SOFT, format!("Downloading v{}…", release.version));
                widgets::progress(ui, p.fraction);
                widgets::note(ui, &p.detail);
            }
            Update::Ready(staged) => {
                widgets::status(ui, theme::GOOD, format!("v{} is downloaded and verified.", staged.release.version));
                widgets::paragraph(
                    ui,
                    "Restart the launcher to finish the update. It closes, the new files are put in place and it \
                     starts again.",
                );
                if let Some(why) = blocker {
                    widgets::status(ui, theme::WARN, why);
                }
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if widgets::button(ui, "Restart and install", Kind::Primary, blocker.is_none()).clicked() {
                        apply = true;
                    }
                    if widgets::button(ui, "Cancel", Kind::Plain, true).clicked() {
                        discard = true;
                    }
                });
            }
        }
        if check {
            self.update = Update::start_check(ui.ctx());
        }
        if repair {
            let missing = self.program_missing.iter().map(|n| n.to_string()).collect();
            self.update = Update::start_repair(ui.ctx(), missing);
        }
        if download && let Err(e) = self.update.download(ui.ctx()) {
            self.update = Update::Failed(e);
        }
        if apply && blocker.is_none() {
            self.settings_save.flush(&mut self.settings, &mut self.saved);
            if let Err(e) = self.update.apply() {
                self.update = Update::Failed(e);
            }
        }
        if discard {
            self.update.discard();
        }
        if let Some(url) = open {
            ui.ctx().open_url(egui::OpenUrl::new_tab(url));
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct Busy {
    started: bool,
    server: bool,
    setup: bool,
    install: bool,
}

fn install_blocker(busy: Busy) -> Option<&'static str> {
    if busy.started {
        Some("Close the game or tool the launcher started first: the update replaces its program.")
    } else if busy.server {
        Some("Stop the server on the Host page first: the update replaces its program.")
    } else if busy.setup {
        Some("Wait for the game data's setup to end: closing the launcher would stop it.")
    } else if busy.install {
        Some("Wait for the mod install to end: closing the launcher would cut it short.")
    } else {
        None
    }
}

fn truncate(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_string();
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &text[..end])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_restart_waits_for_what_the_launcher_runs() {
        assert_eq!(install_blocker(Busy::default()), None);
        let each = [
            Busy { started: true, ..Busy::default() },
            Busy { server: true, ..Busy::default() },
            Busy { setup: true, ..Busy::default() },
            Busy { install: true, ..Busy::default() },
        ];
        let reasons: Vec<_> = each.iter().map(|b| install_blocker(*b).expect("blocked")).collect();
        for (i, r) in reasons.iter().enumerate() {
            assert!(reasons[i + 1..].iter().all(|other| other != r), "one reason per cause: {r}");
        }
        let all = Busy { started: true, server: true, setup: true, install: true };
        assert_eq!(install_blocker(all), Some(reasons[0]));
    }
}
