use eframe::egui::{self, RichText, Ui};
use ssx_assets::content::tuner::TUNER_FILE;

use crate::app::Launcher;
use crate::data_state;
use crate::setup_flow::gb;
use crate::ui::theme::{self, Face, size};
use crate::ui::widgets::{self, Kind};

impl Launcher {
    pub fn game_data_section(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        self.check_remembered_image(&ctx);
        let busy = self.job.is_some();
        let data = self.settings.game_data();
        ui.spacing_mut().item_spacing.y = 6.0;
        let notice = if busy { None } else { data_state::notice(&self.data_state) };
        if let Some((colour, text)) = notice.clone().filter(|_| self.data_state.needs_repair()) {
            widgets::status(ui, colour, text);
        } else {
            widgets::paragraph(
                ui,
                "Project Avalanche runs on the data of your own SSX (2012) Xbox 360 disc. Choose the disc image \
                 (.iso) once: the game archives and movies are extracted from it into the data folder (about 4.7 GB). \
                 The image itself is only read, never changed.",
            );
        }
        ui.add_space(6.0);
        widgets::heading(ui, "Disc image");
        match &self.settings.iso {
            Some(iso) => widgets::path(ui, iso),
            None => widgets::note(ui, "(none selected)"),
        }
        ui.horizontal(|ui| {
            if widgets::button(ui, "Select SSX (2012) Xbox 360 ISO…", Kind::Plain, !busy).clicked() {
                self.pick_iso(&ctx);
            }
            if self.checking.is_some() {
                ui.spinner();
                ui.label("Checking the image…");
            }
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
        });
        ui.add_space(6.0);
        widgets::heading(ui, "Data folder");
        widgets::path(ui, &data);
        ui.horizontal(|ui| {
            if widgets::button(ui, "Change…", Kind::Plain, !busy).clicked() {
                self.pick_data_dir();
            }
            if widgets::button(ui, "Default", Kind::Plain, !busy && self.settings.game_data.is_some()).clicked() {
                self.settings.game_data = None;
                self.data_changed();
            }
            let (colour, label) = data_state::status(&self.data_state);
            widgets::status(ui, colour, label);
        });
        if let Some((_, text)) = notice.filter(|_| self.data_state == ssx_assets::folder::State::Unfinished) {
            widgets::note(ui, &text);
        }
        if let (Some(Ok(c)), Some(disc)) = (&self.check, &self.data_disc)
            && *disc != c.fingerprint
            && !busy
        {
            widgets::note(
                ui,
                "This image is not the one the folder was set up from: every file that differs from it is written again.",
            );
        }
        ui.add_space(6.0);
        widgets::heading(ui, "Setup");
        self.setup_controls(ui);
        ui.add_space(10.0);
        self.language_section(ui);
        ui.add_space(10.0);
        self.dlc_section(ui);
        ui.add_space(10.0);
        self.tuner_section(ui);
        self.tu_section(ui);
    }

    pub fn language_section(&mut self, ui: &mut Ui) {
        widgets::heading(ui, "Language");
        if self.languages.is_empty() {
            widgets::note(
                ui,
                "The disc's text languages are read from the data folder; set up the game data to choose one.",
            );
            return;
        }
        let languages = self.languages.clone();
        let current = self.settings.language.as_deref().unwrap_or("eng_us");
        let shown = languages.iter().find(|(code, _)| *code == current).map_or("English (default)", |(_, name)| *name);
        egui::Grid::new("language").num_columns(2).spacing([16.0, 10.0]).show(ui, |ui| {
            widgets::field(ui, "Text & speech");
            let combo = egui::ComboBox::from_id_salt("language").selected_text(shown).width(250.0);
            widgets::combo(ui, combo, |ui| {
                for &(code, name) in &languages {
                    ui.selectable_value(&mut self.settings.language, Some(code.to_string()), name);
                }
            })
            .response
            .on_hover_text(
                "The game's text and speech language, from the databases on your disc. French and German have their \
                 own spoken lines; the other languages use the English voice.",
            );
            ui.end_row();
        });
        widgets::note(ui, "Takes effect the next time the game starts.");
    }

    fn tuner_section(&mut self, ui: &mut Ui) {
        use crate::tuner_page::Confirm;
        let ctx = ui.ctx().clone();
        let root = self.tuner_root.clone();
        let save = self.save_path.clone();
        let installing = self.tuner.install.is_some();
        widgets::heading(ui, "Level 15 data (tuner pack and profile)");
        widgets::paragraph(
            ui,
            "SSX's level-15 update came as a tuner pack, not a title update: a level-15 profile carries it in its save \
             (chunk 7). A level-15 save, its 0x57800-byte buffer or the tuner pack itself is accepted here; the pack is \
             copied to the Tuner folder, and a save is also copied as your profile (Saves/ssxvprofile) so the game \
             loads it. The game merges the pack over the disc's data (15 levels, the store's level tables, the changed \
             drops). Nothing is downloaded or shipped, and a profile without a tuner keeps 10 levels. Your original \
             file is never changed.",
        );
        let installed = self.tuner.snapshot.as_ref().and_then(|s| s.installed.as_ref()).is_some();
        ui.horizontal(|ui| {
            if widgets::button(ui, "Install a level-15 save or tuner pack…", Kind::Primary, !installing).clicked()
                && let Some(file) =
                    rfd::FileDialog::new().set_title("Install a level-15 save (ssxvprofile) or tuner pack").pick_file()
            {
                self.tuner.start_install(&ctx, root.clone(), save.clone(), file, false);
            }
            if installed && widgets::button(ui, "Remove…", Kind::Plain, !installing).clicked() {
                self.tuner.confirm = Some(Confirm::Remove);
            }
            if widgets::button(ui, "Open the tuner folder", Kind::Plain, true).clicked()
                && let Err(e) = crate::mod_list::open_folder(&root)
            {
                self.tuner.message = Some((false, e));
            }
            if installing {
                ui.spinner();
                ui.label("Checking and copying…");
            }
        });
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
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Reading the tuner folder…");
                });
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

    fn dlc_section(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        let root = self.dlc_root.clone();
        let big = self.data_ready.then(|| self.settings.big_root());
        let installing = self.dlc.install.is_some();
        widgets::heading(ui, "Downloadable content");
        widgets::paragraph(
            ui,
            "SSX's DLC packages (the Japan range, Mount Eddie, the Presell Pack, the Eddie and Mac packs) add their \
             tracks, drops, outfits and unlockables as the game's own base content above the disc; a title update in \
             the same folder or .zip goes to the Title Update section below. Install your own dump's folder \
             (DLC- 4541096D) or its .zip here. Nothing is downloaded, and no game content is shared.",
        );
        ui.horizontal(|ui| {
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
            if installing {
                ui.spinner();
                ui.label("Installing and extracting…");
            }
            if !self.data_ready {
                widgets::note(ui, "Set the game data up first: its vault schema decodes the packages.");
            }
        });
        if let Some((ok, text)) = &self.dlc.message {
            widgets::status(ui, if *ok { theme::GOOD } else { theme::ERROR }, text.clone());
        }
        if self.dlc_confirm(ui) {
            self.refresh_dlc(&ctx);
        }
        let Some(snapshot) = &self.dlc.snapshot else {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Reading the DLC folder…");
            });
            return;
        };
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
            return;
        }
        widgets::note(
            ui,
            &format!(
                "Read in {:.2} s. The packages load oldest first — contentVersion, then mountversion — so the newer one \
                 wins a path two provide (the Japan Pack over the Presell Pack). A package is the game's base \
                 content: it is not a mod.",
                snapshot.seconds
            ),
        );
        let mut action = None;
        let mut forget = None;
        for row in &snapshot.set.packages {
            if let Some(a) = ui.push_id(&row.folder, |ui| dlc_row(ui, row, installing)).inner {
                action = Some(a);
            }
        }
        for skipped in &snapshot.set.skipped {
            if skipped.folder == ssx_assets::content::dlc::LIST_FILE {
                continue;
            }
            if ui.push_id(&skipped.folder, |ui| skipped_row(ui, &skipped.folder, &skipped.reason, installing)).inner {
                forget = Some(skipped.folder.clone());
            }
        }
        if let Some(folder) = forget {
            let forgotten = self.dlc.forget(&root, &folder);
            if forgotten {
                self.refresh_dlc(&ctx);
            }
        }
        match action {
            Some(DlcAction::Enable(folder, on)) => {
                self.dlc.set_enabled(&root, &folder, on);
                self.refresh_dlc(&ctx);
            }
            Some(DlcAction::Remove(folder, _name)) => {
                let removed = self.dlc.remove(&root, &folder, false);
                if removed {
                    self.refresh_dlc(&ctx);
                }
            }
            None => {}
        }
    }
    fn tu_section(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        let dlc_root = self.dlc_root.clone();
        let root = self.tu_root.clone();
        let big = self.data_ready.then(|| self.settings.big_root());
        let installing = self.tu.install.is_some();
        widgets::heading(ui, "Title update");
        widgets::paragraph(
            ui,
            "SSX's title updates are Xbox content packages of their own (not DLC): one update is applied above the \
             disc and the DLC packages. 3.02 (July 2012) brings a patch of the front end, the objective scripts, the \
             localisation databases and a New Zealand track fix; 1.02 (February 2012) is a code-only build. Only one \
             applies at a time — the newest enabled — because they are alternative rebuilds, not cumulative. Install \
             your own dump's Title Update folder, its .zip or its package file here; nothing is downloaded.",
        );
        ui.horizontal(|ui| {
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
            if installing {
                ui.spinner();
                ui.label("Installing and extracting…");
            }
        });
        if let Some((ok, text)) = &self.tu.message {
            widgets::status(ui, if *ok { theme::GOOD } else { theme::ERROR }, text.clone());
        }
        if self.tu_confirm(ui) {
            self.refresh_tu(&ctx);
        }
        let Some(snapshot) = &self.tu.snapshot else {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Reading the title updates' folder…");
            });
            return;
        };
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
            return;
        }
        widgets::note(
            ui,
            &format!(
                "Read in {:.2} s. One update applies: the newest enabled one. Its files answer before the DLC packages \
                 and the disc; the other installed updates stay off.",
                snapshot.seconds
            ),
        );
        let mut action = None;
        let mut forget = None;
        for row in &snapshot.set.entries {
            let applied = snapshot.set.mounted().is_some_and(|m| m.folder == row.folder);
            if let Some(a) = ui.push_id(&row.folder, |ui| tu_row(ui, row, applied, installing)).inner {
                action = Some(a);
            }
        }
        for skipped in &snapshot.set.skipped {
            if skipped.folder == ssx_assets::content::title_update::LIST_FILE {
                continue;
            }
            if ui.push_id(&skipped.folder, |ui| skipped_row(ui, &skipped.folder, &skipped.reason, installing)).inner {
                forget = Some(skipped.folder.clone());
            }
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
            let cancelling = self.job.as_ref().is_some_and(|j| j.cancelling());
            let label = if cancelling { "Cancelling…" } else { "Cancel" };
            ui.horizontal(|ui| {
                if widgets::button(ui, label, Kind::Plain, !cancelling).clicked()
                    && let Some(job) = &self.job
                {
                    job.cancel();
                }
                ui.label(format!("{:.0} %  ·  {}", p.fraction * 100.0, p.line));
            });
        } else {
            let ready = matches!(self.check, Some(Ok(_))) && !self.running();
            ui.horizontal(|ui| {
                if widgets::button(ui, data_state::button(&self.data_state), Kind::Primary, ready).clicked() {
                    self.start_setup(ui.ctx());
                }
                if !ready {
                    widgets::note(ui, "Select a valid image first; the game must not be running.");
                }
            });
        }
        if let Some((ok, message)) = &self.setup_message {
            widgets::status(ui, if *ok { theme::GOOD } else { theme::ERROR }, message.clone());
        }
    }
}

enum TuAction {
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

fn tu_row(
    ui: &mut Ui,
    row: &ssx_assets::content::title_update::TuEntry,
    applied: bool,
    busy: bool,
) -> Option<TuAction> {
    let mut action = None;
    let line = if applied {
        theme::GOOD
    } else if row.enabled {
        theme::LINE
    } else {
        theme::LINE_FAINT
    };
    egui::Frame::new().fill(theme::PANEL).stroke(egui::Stroke::new(1.0, line)).inner_margin(6.0).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.spacing_mut().item_spacing.y = 2.0;
        let title = ssx_core::text::shown_max(row.display_name(), 256).to_string();
        let (left, right) = egui::containers::Sides::new().shrink_left().truncate().show(
            ui,
            |ui| {
                let mut on = row.enabled;
                let changed = ui.add_enabled(!busy, egui::Checkbox::without_text(&mut on)).changed();
                let colour = if row.enabled { theme::TEXT } else { theme::TEXT_SOFT };
                ui.add(
                    egui::Label::new(RichText::new(&title).font(Face::Title.id(size::TEXT)).color(colour)).truncate(),
                );
                changed.then_some(TuAction::Enable(row.folder.clone(), on))
            },
            |ui| {
                if widgets::button(ui, "Remove", Kind::Plain, !busy).clicked() {
                    return Some(TuAction::Remove(row.folder.clone()));
                }
                None
            },
        );
        action = left.or(right);
        let patch = if row.has_patch {
            format!("patch0.big {} members", row.members)
        } else {
            "no patch0.big (code-only)".to_string()
        };
        let facts = format!(
            "XEX {}  ·  {}  ·  {patch}  ·  folder {}",
            row.version_text(),
            row.build_date,
            ssx_core::text::shown_max(&row.folder, 128)
        );
        ui.add(egui::Label::new(RichText::new(&facts).color(theme::TEXT_DIM)).truncate());
        if applied {
            widgets::status(ui, theme::GOOD, "applied: its files answer before the DLC packages and the disc");
        } else if !row.enabled {
            widgets::status(ui, theme::WARN, "off: the enabled update applies instead");
        } else if !row.ready {
            widgets::status(ui, theme::WARN, "its extracted data is missing: install the package again");
        }
    });
    action
}

enum DlcAction {
    Enable(String, bool),
    Remove(String, String),
}

fn dlc_row(ui: &mut Ui, row: &ssx_assets::content::dlc::DlcEntry, busy: bool) -> Option<DlcAction> {
    let mut action = None;
    let line = if row.enabled { theme::LINE } else { theme::LINE_FAINT };
    egui::Frame::new().fill(theme::PANEL).stroke(egui::Stroke::new(1.0, line)).inner_margin(6.0).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.spacing_mut().item_spacing.y = 2.0;
        let title = ssx_core::text::shown_max(row.display_name(), 256).to_string();
        let (left, right) = egui::containers::Sides::new().shrink_left().truncate().show(
            ui,
            |ui| {
                let mut on = row.enabled;
                let changed = ui.add_enabled(!busy, egui::Checkbox::without_text(&mut on)).changed();
                let colour = if row.enabled { theme::TEXT } else { theme::TEXT_SOFT };
                ui.add(
                    egui::Label::new(RichText::new(&title).font(Face::Title.id(size::TEXT)).color(colour)).truncate(),
                );
                changed.then_some(DlcAction::Enable(row.folder.clone(), on))
            },
            |ui| {
                if widgets::button(ui, "Remove", Kind::Plain, !busy).clicked() {
                    return Some(DlcAction::Remove(row.folder.clone(), title.clone()));
                }
                None
            },
        );
        action = left.or(right);
        let version = row.content_version.map_or_else(|| "-".to_string(), |v| v.to_string());
        let mount = row.mount_version().map_or_else(|| "-".to_string(), |v| v.to_string());
        let facts = format!(
            "{}  ·  contentVersion {version}  ·  mountversion {mount}  ·  folder {}",
            ssx_core::text::shown_max(&row.info.name, 128),
            ssx_core::text::shown_max(&row.folder, 128)
        );
        ui.add(egui::Label::new(RichText::new(&facts).color(theme::TEXT_DIM)).truncate());
        if let Some(unique) = &row.unique_dlc_id {
            ui.label(
                RichText::new(format!("uniqueDlcId {}", ssx_core::text::shown_max(unique, 32))).color(theme::TEXT_DIM),
            );
        }
        if !row.ready {
            widgets::status(ui, theme::WARN, "its data folder is missing: install the package again");
        }
        if let Some(why) = &row.damaged {
            widgets::status(
                ui,
                theme::ERROR,
                format!("damaged, not loaded: its vaults do not decode ({why}); install the package again"),
            );
        }
    });
    action
}
