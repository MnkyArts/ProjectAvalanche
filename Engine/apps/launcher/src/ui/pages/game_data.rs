use eframe::egui::{self, Ui};
use ssx_assets::content::tuner::TUNER_FILE;

use crate::app::Launcher;
use crate::data_state;
use crate::setup_flow::gb;
use crate::ui::details::{self, Detail};
use crate::ui::rows::{self, Item, Row};
use crate::ui::theme;
use crate::ui::widgets::{self, Kind};

pub const INTRO: &str = "Project Avalanche runs on the data of your own SSX (2012) Xbox 360 disc. Choose the disc image \
     (.iso) once: the game archives and movies are extracted from it into the data folder (about 4.7 GB). The image \
     itself is only read, never changed.";

const TUNER_TEXT: &str = "SSX's level-15 update came as a tuner pack, not a title update: a level-15 profile carries it \
     in its save (chunk 7). A level-15 save, its 0x57800-byte buffer or the tuner pack itself is accepted here; the \
     pack is copied to the Tuner folder, and a save is also copied as your profile (Saves/ssxvprofile) so the game \
     loads it. The game merges the pack over the disc's data (15 levels, the store's level tables, the changed drops). \
     Nothing is downloaded or shipped, and a profile without a tuner keeps 10 levels. Your original file is never \
     changed.";

pub const DLC_INTRO: &str = "SSX's DLC packages (the Japan range, Mount Eddie, the Presell Pack, the Eddie and Mac packs) \
     add their tracks, drops, outfits and unlockables as the game's own base content above the disc; a title update in \
     the same folder or .zip goes to the Title update section. Install your own dump's folder (DLC- 4541096D) or its \
     .zip here. Nothing is downloaded, and no game content is shared.";

pub const TU_INTRO: &str = "SSX's title updates are Xbox content packages of their own (not DLC): one update is \
     applied above the disc and the DLC packages. 3.02 (July 2012) brings a patch of the front end, the objective \
     scripts, the localisation databases and a New Zealand track fix; 1.02 (February 2012) is a code-only build. Only \
     one applies at a time — the newest enabled — because they are alternative rebuilds, not cumulative. Install your \
     own dump's Title Update folder, its .zip or its package file here; nothing is downloaded.";

impl Launcher {
    pub fn game_data_section(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        self.check_remembered_image(&ctx);
        ui.spacing_mut().item_spacing.y = 6.0;
        if self.data_ready {
            self.data_ready_rows(ui);
        } else {
            self.setup_steps(ui);
        }
        ui.add_space(4.0);
        self.tuner_section(ui);
    }

    fn setup_steps(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        let busy = self.job.is_some();
        let data = self.settings.game_data();
        let notice = if busy { None } else { data_state::notice(&self.data_state) };
        if let Some((colour, text)) = notice.clone().filter(|_| self.data_state.needs_repair()) {
            widgets::status(ui, colour, text);
        }
        rows::group(ui, "First start", Some("three steps"));
        let image_ok = matches!(self.check, Some(Ok(_)));
        rows::step_card(ui, 1, image_ok, "Your disc image", |ui| {
            match &self.settings.iso {
                Some(iso) => widgets::path(ui, iso),
                None => widgets::note(ui, "The .iso of your own SSX (2012) Xbox 360 disc. It is only read."),
            }
            self.image_check_line(ui);
            rows::buttons(ui, |ui| {
                let label = if image_ok { "Choose another…" } else { "Select SSX (2012) Xbox 360 ISO…" };
                let kind = if image_ok { Kind::Plain } else { Kind::Primary };
                if widgets::button(ui, label, kind, !busy).clicked() {
                    self.pick_iso(&ctx);
                }
            });
            if self.checking.is_some() {
                widgets::busy(ui, "Checking the image…");
            }
        });
        rows::step_card(ui, 2, image_ok, "Where the game data goes", |ui| {
            widgets::path(ui, &data);
            widgets::note(ui, "About 4.7 GB.");
            self.data_folder_buttons(ui, busy);
            let (colour, label) = data_state::status(&self.data_state);
            widgets::status(ui, colour, label);
            if let Some((_, text)) = notice.filter(|_| self.data_state == ssx_assets::folder::State::Unfinished) {
                widgets::note(ui, &text);
            }
        });
        rows::step_card(ui, 3, self.data_ready, "Extract", |ui| {
            if self.setup_progress().is_none() {
                widgets::note(ui, "Unpacks the game's archives and movies from the image into the data folder, once.");
            }
            self.setup_controls(ui);
            self.setup_message_line(ui);
        });
    }

    fn data_ready_rows(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        let busy = self.job.is_some();
        rows::group(ui, "Game data", None);
        let (colour, word) = data_state::status(&self.data_state);
        let folder = data_state::folder_line(&self.data_state);
        let detail = || {
            Detail::new("Status")
                .text("The data your own SSX (2012) disc gave. The disc image is only read, never changed.")
                .now(folder.clone(), None)
        };
        rows::state(ui, Row::new("Status"), word, colour, detail);
        if let Some((colour, text)) =
            data_state::notice(&self.data_state).filter(|_| !busy && self.data_state.needs_repair())
        {
            widgets::status(ui, colour, text);
        }
        ui.add_space(4.0);
        widgets::field(ui, "Disc image");
        match &self.settings.iso {
            Some(iso) => widgets::path(ui, iso),
            None => widgets::note(ui, "(none selected)"),
        }
        self.image_check_line(ui);
        rows::buttons(ui, |ui| {
            let label =
                if self.settings.iso.is_some() { "Another disc image…" } else { "Select SSX (2012) Xbox 360 ISO…" };
            if widgets::button(ui, label, Kind::Plain, !busy).clicked() {
                self.pick_iso(&ctx);
            }
        });
        if self.checking.is_some() {
            widgets::busy(ui, "Checking the image…");
        }
        widgets::field(ui, "Data folder");
        widgets::path(ui, &self.settings.game_data());
        widgets::note(ui, &folder);
        self.data_folder_buttons(ui, busy);
        if busy || self.data_state.needs_repair() || self.other_image() || self.image_for_unmarked() {
            self.setup_controls(ui);
        }
        self.setup_message_line(ui);
    }

    fn other_image(&self) -> bool {
        matches!((&self.check, &self.data_disc), (Some(Ok(c)), Some(disc)) if *disc != c.fingerprint)
    }

    fn image_for_unmarked(&self) -> bool {
        matches!(self.check, Some(Ok(_)))
            && matches!(self.data_state, ssx_assets::folder::State::Complete { marked: false, .. })
    }

    fn image_check_line(&self, ui: &mut Ui) {
        match &self.check {
            Some(Ok(c)) => widgets::status(
                ui,
                theme::GOOD,
                format!(
                    "SSX (2012), Xbox 360 image ({:?} layout, {} files): {} archives, {} movies, {} to read",
                    c.layout,
                    c.files,
                    c.archives.len(),
                    c.movies.len(),
                    gb(c.archive_bytes() + c.movie_bytes())
                ),
            ),
            Some(Err(e)) => widgets::status(ui, theme::ERROR, e.clone()),
            None => {}
        }
        if self.other_image() && self.job.is_none() {
            widgets::note(
                ui,
                "This image is not the one the folder was set up from: every file that differs from it is written again.",
            );
        } else if self.image_for_unmarked() && self.job.is_none() {
            widgets::note(
                ui,
                "The folder has no setup marker: Extract compares every file with this image, keeps what matches and \
                 writes the marker.",
            );
        }
    }

    fn data_folder_buttons(&mut self, ui: &mut Ui, busy: bool) {
        rows::buttons(ui, |ui| {
            if widgets::button(ui, "Change…", Kind::Plain, !busy).clicked() {
                self.pick_data_dir();
            }
            if widgets::button(ui, "Default", Kind::Plain, !busy && self.settings.game_data.is_some()).clicked() {
                self.settings.game_data = None;
                self.data_changed();
            }
        });
    }

    fn tuner_section(&mut self, ui: &mut Ui) {
        use crate::tuner_page::Confirm;
        let ctx = ui.ctx().clone();
        let root = self.tuner_root.clone();
        let save = self.save_path.clone();
        let installing = self.tuner.install.is_some();
        rows::group(ui, "Level 15 data", Some("tuner pack and profile"));
        let installed = self.tuner.snapshot.as_ref().and_then(|s| s.installed.as_ref()).is_some();
        let about = || Detail::new("Level 15 data").text(TUNER_TEXT);
        rows::buttons(ui, |ui| {
            let install = widgets::button(ui, "Install a level-15 save or tuner pack…", Kind::Primary, !installing);
            details::offer(ui, &install, about);
            if install.clicked()
                && let Some(file) =
                    rfd::FileDialog::new().set_title("Install a level-15 save (ssxvprofile) or tuner pack").pick_file()
            {
                self.tuner.start_install(&ctx, root.clone(), save.clone(), file, false);
            }
            if installed {
                let remove = widgets::button(ui, "Remove…", Kind::Plain, !installing);
                details::offer(ui, &remove, about);
                if remove.clicked() {
                    self.tuner.confirm = Some(Confirm::Remove);
                }
            }
            let open = widgets::button(ui, "Open the tuner folder", Kind::Plain, true);
            details::offer(ui, &open, about);
            if open.clicked()
                && let Err(e) = crate::mod_list::open_folder(&root)
            {
                self.tuner.message = Some((false, e));
            }
        });
        if installing {
            widgets::busy(ui, "Checking and copying…");
        }
        if let Some((ok, text)) = &self.tuner.message {
            widgets::status(ui, if *ok { theme::GOOD } else { theme::ERROR }, text.clone());
        }
        if self.tuner_confirm(ui) {
            self.refresh_tuner(&ctx);
        }
        match &self.tuner.snapshot {
            Some(snapshot) => match &snapshot.installed {
                Some(loaded) => {
                    widgets::status(ui, theme::GOOD, format!("{} — installed", loaded.line()));
                    if let Some(bytes) = snapshot.bytes {
                        widgets::note(ui, &format!("{} ({} bytes)", root.join(TUNER_FILE).display(), bytes));
                    }
                }
                None => {
                    widgets::note(ui, "No level-15 data installed: the game keeps the disc's 10 levels.");
                    if let Some(e) = &snapshot.error {
                        widgets::status(ui, theme::ERROR, e.clone());
                    }
                }
            },
            None => {
                widgets::busy(ui, "Reading the tuner folder…");
            }
        }
    }

    fn tuner_confirm(&mut self, ui: &mut Ui) -> bool {
        let ctx = ui.ctx().clone();
        let Some(confirm) = self.tuner.confirm.take() else { return false };
        let root = self.tuner_root.clone();
        let save = self.save_path.clone();
        let (title, text, yes) = match &confirm {
            crate::tuner_page::Confirm::Replace { source, tuner, profile } => {
                let name = source
                    .file_name()
                    .map_or_else(|| source.display().to_string(), |n| n.to_string_lossy().into_owned());
                let what = match (tuner, profile) {
                    (true, true) => format!(
                        "A tuner file is installed and your profile {} exists already. Replace both with {name}?",
                        save.display()
                    ),
                    (true, false) => format!("A tuner file is installed already. Replace it with {name}?"),
                    (false, true) => format!("Your profile {} exists already. Replace it with {name}?", save.display()),
                    (false, false) => format!("Replace the installed level-15 data with {name}?"),
                };
                ("Replace the level-15 data?".to_string(), what, "Replace")
            }
            crate::tuner_page::Confirm::Remove => (
                "Remove the level-15 data?".to_string(),
                "The copy in the Tuner folder is deleted; the game keeps the disc's 10 levels (and a profile's own \
                 chunk 7, if it has one). Your original file is not touched."
                    .to_string(),
                "Remove",
            ),
        };
        let asked = widgets::Question { title: &title, text: &text, yes, yes_enabled: !self.tuner.busy() };
        let Some(answer) = widgets::question(&ctx, egui::Id::new("tuner-confirm"), Some(asked)) else {
            self.tuner.confirm = Some(confirm);
            return false;
        };
        if !answer {
            return false;
        }
        match confirm {
            crate::tuner_page::Confirm::Replace { source, .. } => {
                if self.tuner.install.is_none() {
                    self.tuner.start_install(&ctx, root, save, source, true);
                }
                false
            }
            crate::tuner_page::Confirm::Remove => self.tuner.remove(&root),
        }
    }

    pub fn dlc_section(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        let root = self.dlc_root.clone();
        let big = self.data_ready.then(|| self.settings.big_root());
        let installing = self.dlc.install.is_some();
        ui.spacing_mut().item_spacing.y = 6.0;
        if let Some((ok, text)) = &self.dlc.message {
            widgets::status(ui, if *ok { theme::GOOD } else { theme::ERROR }, text.clone());
        }
        if self.dlc_confirm(ui) {
            self.refresh_dlc(&ctx);
        }
        let mut action = None;
        let mut forget = None;
        match &self.dlc.snapshot {
            None => {
                widgets::busy(ui, "Reading the DLC folder…");
            }
            Some(snapshot) => {
                if let Some(e) = &snapshot.set.list_error {
                    widgets::status(
                        ui,
                        theme::ERROR,
                        format!("dlc.toml cannot be read: {e}. No listed package is mounted until it can."),
                    );
                }
                if snapshot.set.packages.is_empty() {
                    widgets::note(ui, "No DLC package is installed.");
                    widgets::path(ui, &root);
                } else {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    rows::group(ui, "Installed", Some("oldest first: the newer wins"));
                    for row in &snapshot.set.packages {
                        let selected = self.selected_dlc.as_deref() == Some(row.folder.as_str());
                        let id = ui.id().with(("dlc", &row.folder));
                        let item = dlc_item(ui, id, row, installing, selected, snapshot.seconds);
                        if item.response.has_focus() || item.response.clicked() {
                            self.selected_dlc = Some(row.folder.clone());
                        }
                        if item.toggle {
                            action = Some(DlcAction::Enable(row.folder.clone(), !row.enabled));
                        }
                    }
                    ui.spacing_mut().item_spacing.y = 6.0;
                    let chosen = snapshot.set.packages.iter().find(|r| self.selected_dlc.as_deref() == Some(&r.folder));
                    rows::buttons(ui, |ui| {
                        if widgets::button(ui, "Remove", Kind::Plain, chosen.is_some() && !installing).clicked()
                            && let Some(row) = chosen
                        {
                            action = Some(DlcAction::Remove(row.folder.clone()));
                        }
                    });
                }
                for skipped in &snapshot.set.skipped {
                    if skipped.folder == ssx_assets::content::dlc::LIST_FILE {
                        continue;
                    }
                    let forgot = ui
                        .push_id(&skipped.folder, |ui| skipped_row(ui, &skipped.folder, &skipped.reason, installing))
                        .inner;
                    if forgot {
                        forget = Some(skipped.folder.clone());
                    }
                }
            }
        }
        rows::buttons(ui, |ui| {
            if widgets::button(ui, "Install .zip…", Kind::Primary, self.data_ready && !installing).clicked()
                && let Some(zip) = rfd::FileDialog::new()
                    .set_title("Install SSX DLC packages (.zip)")
                    .add_filter("DLC dump", &["zip", "ZIP"])
                    .pick_file()
            {
                self.dlc.start_install(&ctx, root.clone(), big.clone(), zip, false);
            }
            if widgets::button(ui, "Install folder…", Kind::Plain, self.data_ready && !installing).clicked()
                && let Some(dir) =
                    rfd::FileDialog::new().set_title("Install a folder of SSX DLC packages").pick_folder()
            {
                self.dlc.start_install(&ctx, root.clone(), big.clone(), dir, false);
            }
            if widgets::button(ui, "Open the DLC folder", Kind::Plain, true).clicked()
                && let Err(e) = crate::mod_list::open_folder(&root)
            {
                self.dlc.message = Some((false, e));
            }
        });
        if installing {
            widgets::busy(ui, "Installing and extracting…");
        }
        if !self.data_ready {
            widgets::note(ui, "Set the game data up first: its vault schema decodes the packages.");
        }
        if let Some(folder) = forget
            && self.dlc.forget(&root, &folder)
        {
            self.refresh_dlc(&ctx);
        }
        match action {
            Some(DlcAction::Enable(folder, on)) => {
                self.dlc.set_enabled(&root, &folder, on);
                self.refresh_dlc(&ctx);
            }
            Some(DlcAction::Remove(folder)) => {
                let removed = self.dlc.remove(&root, &folder, false);
                if removed {
                    self.refresh_dlc(&ctx);
                }
            }
            None => {}
        }
    }

    pub fn tu_section(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        let dlc_root = self.dlc_root.clone();
        let root = self.tu_root.clone();
        let big = self.data_ready.then(|| self.settings.big_root());
        let installing = self.tu.install.is_some();
        ui.spacing_mut().item_spacing.y = 6.0;
        if let Some((ok, text)) = &self.tu.message {
            widgets::status(ui, if *ok { theme::GOOD } else { theme::ERROR }, text.clone());
        }
        if self.tu_confirm(ui) {
            self.refresh_tu(&ctx);
        }
        let mut action = None;
        let mut forget = None;
        match &self.tu.snapshot {
            None => {
                widgets::busy(ui, "Reading the title updates' folder…");
            }
            Some(snapshot) => {
                if let Some(e) = &snapshot.set.list_error {
                    widgets::status(
                        ui,
                        theme::ERROR,
                        format!("tu.toml cannot be read: {e}. No update is applied until it can."),
                    );
                }
                if snapshot.set.entries.is_empty() {
                    widgets::note(ui, "No title update is installed.");
                    widgets::path(ui, &root);
                } else {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    rows::group(ui, "Installed", Some("one applies: the newest switched on"));
                    for row in &snapshot.set.entries {
                        let applied = snapshot.set.mounted().is_some_and(|m| m.folder == row.folder);
                        let selected = self.selected_tu.as_deref() == Some(row.folder.as_str());
                        let id = ui.id().with(("tu", &row.folder));
                        let item = tu_item(ui, id, row, applied, installing, selected, snapshot.seconds);
                        if item.response.has_focus() || item.response.clicked() {
                            self.selected_tu = Some(row.folder.clone());
                        }
                        if item.toggle {
                            action = Some(TuAction::Enable(row.folder.clone(), !row.enabled));
                        }
                    }
                    ui.spacing_mut().item_spacing.y = 6.0;
                    let chosen = snapshot.set.entries.iter().find(|r| self.selected_tu.as_deref() == Some(&r.folder));
                    rows::buttons(ui, |ui| {
                        if widgets::button(ui, "Remove", Kind::Plain, chosen.is_some() && !installing).clicked()
                            && let Some(row) = chosen
                        {
                            action = Some(TuAction::Remove(row.folder.clone()));
                        }
                    });
                }
                for skipped in &snapshot.set.skipped {
                    if skipped.folder == ssx_assets::content::title_update::LIST_FILE {
                        continue;
                    }
                    let forgot = ui
                        .push_id(&skipped.folder, |ui| skipped_row(ui, &skipped.folder, &skipped.reason, installing))
                        .inner;
                    if forgot {
                        forget = Some(skipped.folder.clone());
                    }
                }
            }
        }
        rows::buttons(ui, |ui| {
            if widgets::button(ui, "Install .zip…", Kind::Primary, self.data_ready && !installing).clicked()
                && let Some(zip) = rfd::FileDialog::new()
                    .set_title("Install an SSX title update (.zip)")
                    .add_filter("Title update", &["zip", "ZIP"])
                    .pick_file()
            {
                self.tu.start_install(&ctx, dlc_root.clone(), big.clone(), zip, false);
            }
            if widgets::button(ui, "Install folder…", Kind::Plain, self.data_ready && !installing).clicked()
                && let Some(dir) =
                    rfd::FileDialog::new().set_title("Install a folder holding an SSX title update").pick_folder()
            {
                self.tu.start_install(&ctx, dlc_root.clone(), big.clone(), dir, false);
            }
            if widgets::button(ui, "Install package…", Kind::Plain, self.data_ready && !installing).clicked()
                && let Some(file) = rfd::FileDialog::new().set_title("Install an SSX title update package").pick_file()
            {
                self.tu.start_install(&ctx, dlc_root.clone(), big.clone(), file, false);
            }
        });
        if installing {
            widgets::busy(ui, "Installing and extracting…");
        }
        if let Some(folder) = forget
            && self.tu.forget(&folder)
        {
            self.refresh_tu(&ctx);
        }
        match action {
            Some(TuAction::Enable(folder, on)) => {
                self.tu.set_enabled(&folder, on);
                self.refresh_tu(&ctx);
            }
            Some(TuAction::Remove(folder)) => {
                let removed = self.tu.remove(&root, &folder, false);
                if removed {
                    self.refresh_tu(&ctx);
                }
            }
            None => {}
        }
    }

    fn tu_confirm(&mut self, ui: &mut Ui) -> bool {
        let (title, text, yes) = match &self.tu.confirm {
            Some(crate::dlc_list::Confirm::Replace { name, folder, .. }) => (
                format!("Replace {}?", ssx_core::text::shown_max(name, 256)),
                format!(
                    "The installed title update {} is replaced by this one.",
                    ssx_core::text::shown_max(folder, 256)
                ),
                "Replace",
            ),
            Some(crate::dlc_list::Confirm::Delete { folder, why }) => (
                format!("Delete {} for good?", ssx_core::text::shown_max(folder, 256)),
                format!(
                    "The system's trash is not available here ({}). Deleting cannot be undone.",
                    ssx_core::text::shown_max(why, 256)
                ),
                "Delete permanently",
            ),
            None => return false,
        };
        let asked = widgets::Question { title: &title, text: &text, yes, yes_enabled: self.tu.install.is_none() };
        let ctx = ui.ctx().clone();
        let Some(answer) = widgets::question(&ctx, egui::Id::new("tu-confirm"), Some(asked)) else { return false };
        let dlc_root = self.dlc_root.clone();
        let root = self.tu_root.clone();
        let big = self.data_ready.then(|| self.settings.big_root());
        match self.tu.confirm.take().filter(|_| answer) {
            Some(crate::dlc_list::Confirm::Replace { source, .. }) => {
                self.tu.start_install(&ctx, dlc_root, big, source, true);
                false
            }
            Some(crate::dlc_list::Confirm::Delete { folder, .. }) => self.tu.remove(&root, &folder, true),
            None => false,
        }
    }

    fn dlc_confirm(&mut self, ui: &mut Ui) -> bool {
        let (title, text, yes) = match &self.dlc.confirm {
            Some(crate::dlc_list::Confirm::Replace { name, folder, .. }) => (
                format!("Replace {}?", ssx_core::text::shown_max(name, 256)),
                format!("The installed package {} is replaced by this one.", ssx_core::text::shown_max(folder, 256)),
                "Replace",
            ),
            Some(crate::dlc_list::Confirm::Delete { folder, why }) => (
                format!("Delete {} for good?", ssx_core::text::shown_max(folder, 256)),
                format!(
                    "The system's trash is not available here ({}). Deleting cannot be undone.",
                    ssx_core::text::shown_max(why, 256)
                ),
                "Delete permanently",
            ),
            None => return false,
        };
        let asked = widgets::Question { title: &title, text: &text, yes, yes_enabled: self.dlc.install.is_none() };
        let ctx = ui.ctx().clone();
        let Some(answer) = widgets::question(&ctx, egui::Id::new("dlc-confirm"), Some(asked)) else { return false };
        let root = self.dlc_root.clone();
        let big = self.data_ready.then(|| self.settings.big_root());
        match self.dlc.confirm.take().filter(|_| answer) {
            Some(crate::dlc_list::Confirm::Replace { source, .. }) => {
                self.dlc.start_install(&ctx, root, big, source, true);
                false
            }
            Some(crate::dlc_list::Confirm::Delete { folder, .. }) => self.dlc.remove(&root, &folder, true),
            None => false,
        }
    }

    fn setup_controls(&mut self, ui: &mut Ui) {
        if let Some(p) = self.setup_progress() {
            widgets::progress(ui, p.fraction);
            ui.label(format!("{:.0} %  ·  {}", p.fraction * 100.0, p.line));
            let cancelling = self.job.as_ref().is_some_and(|j| j.cancelling());
            let label = if cancelling { "Cancelling…" } else { "Cancel" };
            rows::buttons(ui, |ui| {
                if widgets::button(ui, label, Kind::Plain, !cancelling).clicked()
                    && let Some(job) = &self.job
                {
                    job.cancel();
                }
            });
        } else {
            let ready = matches!(self.check, Some(Ok(_))) && !self.running();
            rows::buttons(ui, |ui| {
                if widgets::button(ui, data_state::button(&self.data_state), Kind::Primary, ready).clicked() {
                    self.start_setup(ui.ctx());
                }
            });
            if !ready {
                widgets::note(ui, "Select a valid image first; the game must not be running.");
            }
        }
    }

    fn setup_message_line(&self, ui: &mut Ui) {
        if let Some((ok, message)) = &self.setup_message {
            widgets::status(ui, if *ok { theme::GOOD } else { theme::ERROR }, message.clone());
        }
    }
}

enum TuAction {
    Enable(String, bool),
    Remove(String),
}

enum DlcAction {
    Enable(String, bool),
    Remove(String),
}

fn skipped_row(ui: &mut Ui, folder: &str, reason: &str, busy: bool) -> bool {
    let text = format!("skipped {}: {}", ssx_core::text::shown_max(folder, 256), reason);
    let (_, forget) = egui::containers::Sides::new().shrink_left().truncate().show(
        ui,
        |ui| widgets::status_cut(ui, theme::WARN, text),
        |ui| widgets::button(ui, "Forget", Kind::Plain, !busy).clicked(),
    );
    forget
}

fn dlc_item(
    ui: &mut Ui,
    id: egui::Id,
    row: &ssx_assets::content::dlc::DlcEntry,
    busy: bool,
    selected: bool,
    seconds: f64,
) -> rows::ItemResponse {
    let title = ssx_core::text::shown_max(row.display_name(), 256).to_string();
    let version = row.content_version.map_or_else(|| "-".to_string(), |v| v.to_string());
    let mount = row.mount_version().map_or_else(|| "-".to_string(), |v| v.to_string());
    let meta = format!("contentVersion {version}  ·  mountversion {mount}");
    let chip = match (row.enabled, row.ready, &row.damaged) {
        (_, _, Some(_)) => ("DAMAGED", theme::ERROR),
        (_, false, None) => ("MISSING", theme::WARN),
        (true, true, None) => ("ON", theme::GOOD),
        (false, true, None) => ("OFF", theme::TEXT_DIM),
    };
    let item =
        Item { switch: Some(row.enabled), name: &title, meta: &meta, chip: Some(chip), enabled: !busy, selected };
    let detail = || {
        let mut d = Detail::new(title.clone())
            .text(format!(
                "Read in {seconds:.2} s. The packages load oldest first — contentVersion, then mountversion — so the \
                 newer one wins a path two provide (the Japan Pack over the Presell Pack). A package is the game's \
                 base content: it is not a mod."
            ))
            .fact("PACKAGE", ssx_core::text::shown_max(&row.info.name, 128).to_string())
            .fact("VERSION", format!("contentVersion {version}, mountversion {mount}"))
            .fact("FOLDER", ssx_core::text::shown_max(&row.folder, 128).to_string());
        if let Some(unique) = &row.unique_dlc_id {
            d = d.fact("ID", format!("uniqueDlcId {}", ssx_core::text::shown_max(unique, 32)));
        }
        let warn = match (&row.damaged, row.ready) {
            (Some(why), _) => {
                Some(format!("Damaged, not loaded: its vaults do not decode ({why}); install the package again."))
            }
            (None, false) => Some("Its data folder is missing: install the package again.".to_owned()),
            (None, true) => None,
        };
        d.warn(warn)
            .applies("Enter (a pad's A) or its switch turns it on or off; Remove acts on the package chosen here.")
    };
    rows::item(ui, id, item, detail)
}

fn tu_item(
    ui: &mut Ui,
    id: egui::Id,
    row: &ssx_assets::content::title_update::TuEntry,
    applied: bool,
    busy: bool,
    selected: bool,
    seconds: f64,
) -> rows::ItemResponse {
    let title = ssx_core::text::shown_max(row.display_name(), 256).to_string();
    let meta = format!("XEX {}  ·  {}", row.version_text(), row.build_date);
    let chip = match (applied, row.enabled, row.ready) {
        (true, _, _) => ("APPLIED", theme::GOOD),
        (false, _, false) => ("MISSING", theme::WARN),
        (false, false, true) => ("OFF", theme::TEXT_DIM),
        (false, true, true) => ("ON", theme::TEXT_SOFT),
    };
    let item =
        Item { switch: Some(row.enabled), name: &title, meta: &meta, chip: Some(chip), enabled: !busy, selected };
    let detail = || {
        let patch = if row.has_patch {
            format!("patch0.big {} members", row.members)
        } else {
            "no patch0.big (code-only)".to_string()
        };
        let state = if applied {
            Some("Applied: its files answer before the DLC packages and the disc.")
        } else if !row.enabled {
            Some("Off: the enabled update applies instead.")
        } else {
            None
        };
        let mut d = Detail::new(title.clone()).text(format!(
            "Read in {seconds:.2} s. One update applies: the newest enabled one. Its files answer before the DLC \
             packages and the disc; the other installed updates stay off."
        ));
        if let Some(state) = state {
            d = d.text(state);
        }
        let warn = (!row.ready).then(|| "Its extracted data is missing: install the package again.".to_owned());
        d.warn(warn)
            .fact("XEX", row.version_text())
            .fact("BUILD", row.build_date.to_string())
            .fact("PATCH", patch)
            .fact("FOLDER", ssx_core::text::shown_max(&row.folder, 128).to_string())
            .applies("Enter (a pad's A) or its switch turns it on or off; Remove acts on the update chosen here.")
    };
    rows::item(ui, id, item, detail)
}
