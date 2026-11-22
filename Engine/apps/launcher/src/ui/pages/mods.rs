use std::path::Path;

use eframe::egui::{self, RichText, Ui};
use ssx_assets::content::mods::{MODS_DIR, ModFolder};

use crate::app::Launcher;
use crate::mod_list::{self, Confirm};
use crate::ui::ModsView;
use crate::ui::details::{self, Detail};
use crate::ui::rows::{self, Item};
use crate::ui::theme;
use crate::ui::widgets::{self, Kind};

pub const INTRO: &str = "Switched-on mods load in this order: a higher priority wins on the same id; on equal \
     priority, the lower one in this list.";

enum RowAction {
    Enable(usize, bool),
    Move(usize, isize),
    Remove(String, String),
    Forget(String),
}

fn title_of(row: &ModFolder) -> String {
    let title = match &row.package {
        Ok(m) if m.resource.title.is_empty() => m.resource.name.clone(),
        Ok(m) => m.resource.title.clone(),
        Err(_) => row.folder.clone(),
    };
    ssx_core::text::shown_max(&title, 256).to_string()
}

impl Launcher {
    pub fn refresh_mods(&mut self, ctx: &egui::Context) {
        let big = self.data_ready.then(|| self.settings.big_root());
        self.mods.refresh(ctx, self.app_dir.clone(), big);
    }

    pub fn mods_section(&mut self, ui: &mut Ui) {
        if self.mods_view == ModsView::Mount {
            rows::buttons(ui, |ui| {
                if widgets::button(ui, "Back to the installed mods", Kind::Plain, true).clicked() {
                    self.mods_view = ModsView::Installed;
                }
            });
            self.mods_report(ui);
            return;
        }
        self.mods_installed(ui);
    }

    fn mods_installed(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        let app = self.app_dir.clone();
        let installing = self.mods.install.is_some();
        ui.spacing_mut().item_spacing.y = 6.0;
        if let Some((ok, text)) = &self.mods.message {
            widgets::status(ui, if *ok { theme::GOOD } else { theme::ERROR }, text.clone());
        }
        if self.mods.remarks.count > mod_list::INLINE_REMARKS {
            remarks(ui, &self.mods.remarks);
        }
        if self.mods_confirm(ui) {
            self.refresh_mods(&ctx);
        }
        let mut action = None;
        match &self.mods.snapshot {
            None => {
                widgets::busy(ui, "Reading the mods folder…");
            }
            Some(snapshot) => {
                if let Err(e) = &snapshot.list {
                    widgets::status(
                        ui,
                        theme::ERROR,
                        format!(
                            "mods.toml cannot be read: {e}. The game mounts no mod until it can; a change here \
                             rewrites it."
                        ),
                    );
                }
                if snapshot.rows.is_empty() {
                    widgets::heading(ui, "No mods installed");
                    widgets::paragraph(
                        ui,
                        "A mod is a package — a folder or a .zip with a resource.toml — made of your own files: \
                         textures, models, skies, sounds, music, riders, screens, strings, maps, gamemodes, scripts and \
                         more. Install one with the buttons below: it is checked as it is installed. To make one, see \
                         \"Making a Mod\" in the Project Avalanche wiki; the game's --mod-dev <folder> shows your \
                         package live while you edit it.",
                    );
                    widgets::path(ui, &app.join(MODS_DIR));
                } else {
                    let skipped = |folder: &str| {
                        snapshot
                            .report
                            .as_ref()
                            .and_then(|r| r.skipped.iter().find(|s| s.folder == folder))
                            .map(|s| s.reason.clone())
                    };
                    ui.spacing_mut().item_spacing.y = 0.0;
                    rows::group(ui, "Installed", Some("in load order"));
                    for (i, row) in snapshot.rows.iter().enumerate() {
                        let reason = if row.enabled { skipped(&row.folder) } else { None };
                        let selected = self.selected_mod.as_deref() == Some(row.folder.as_str());
                        let id = ui.id().with(("mod", &row.folder));
                        let item = mod_item(ui, &app, id, row, reason, installing, selected);
                        if item.response.has_focus() || item.response.clicked() {
                            self.selected_mod = Some(row.folder.clone());
                        }
                        if item.toggle {
                            action = Some(RowAction::Enable(i, !row.enabled));
                        }
                    }
                    ui.spacing_mut().item_spacing.y = 6.0;
                    let at = snapshot.rows.iter().position(|r| self.selected_mod.as_deref() == Some(r.folder.as_str()));
                    let n = snapshot.rows.len();
                    rows::buttons(ui, |ui| {
                        let chosen = at.filter(|_| !installing);
                        if widgets::button(ui, "Move up", Kind::Plain, chosen.is_some_and(|i| i > 0)).clicked()
                            && let Some(i) = chosen
                        {
                            action = Some(RowAction::Move(i, -1));
                        }
                        if widgets::button(ui, "Move down", Kind::Plain, chosen.is_some_and(|i| i + 1 < n)).clicked()
                            && let Some(i) = chosen
                        {
                            action = Some(RowAction::Move(i, 1));
                        }
                        let row = chosen.map(|i| &snapshot.rows[i]);
                        match row {
                            Some(row) if row.package.is_err() && !row.present => {
                                if widgets::button(ui, "Forget", Kind::Plain, true).clicked() {
                                    action = Some(RowAction::Forget(row.folder.clone()));
                                }
                            }
                            _ => {
                                if widgets::button(ui, "Remove…", Kind::Plain, row.is_some()).clicked()
                                    && let Some(row) = row
                                {
                                    let title = if row.package.is_ok() { title_of(row) } else { row.folder.clone() };
                                    action = Some(RowAction::Remove(row.folder.clone(), title));
                                }
                            }
                        }
                    });
                }
            }
        }
        let summary = self.mods.snapshot.as_ref().and_then(|s| s.report.as_ref()).map(super::mods_report::summary);
        rows::buttons(ui, |ui| {
            if widgets::button(ui, "Install .zip…", Kind::Primary, !installing).clicked()
                && let Some(zip) = rfd::FileDialog::new()
                    .set_title("Install a mod package (.zip)")
                    .add_filter("Mod package", &["zip", "ZIP"])
                    .pick_file()
            {
                self.mods.start_install(&ctx, app.clone(), zip, false);
            }
            if widgets::button(ui, "Install folder…", Kind::Plain, !installing).clicked()
                && let Some(dir) = rfd::FileDialog::new().set_title("Install a mod package folder").pick_folder()
            {
                self.mods.start_install(&ctx, app.clone(), dir, false);
            }
            if widgets::button(ui, "Open the mods folder", Kind::Plain, true).clicked()
                && let Err(e) = mod_list::open_folder(&app.join(MODS_DIR))
            {
                self.mods.message = Some((false, e));
            }
            let mount = widgets::button(ui, "What the game will mount", Kind::Plain, true);
            details::offer(ui, &mount, || {
                let d = Detail::new("What the game will mount").text(
                    "The game's own mount, worked out now: skipped packages, conflicts, unknown ids and what each \
                     package replaces.",
                );
                match &summary {
                    Some(line) => d.fact("NOW", line.clone()),
                    None => d,
                }
            });
            if mount.clicked() {
                self.mods_view = ModsView::Mount;
            }
        });
        if installing {
            widgets::busy(ui, "Installing and checking…");
        }
        let changed = match action {
            Some(RowAction::Enable(i, on)) => self.mods.set_enabled(&app, i, on),
            Some(RowAction::Move(i, step)) => self.mods.move_row(&app, i, step),
            Some(RowAction::Forget(folder)) => self.mods.forget(&app, &folder),
            Some(RowAction::Remove(folder, title)) => {
                self.mods.confirm = Some(Confirm::Remove { folder, title });
                false
            }
            None => false,
        };
        if changed {
            self.refresh_mods(&ctx);
        }
    }

    fn mods_confirm(&mut self, ui: &mut Ui) -> bool {
        let question = self.mods.confirm.as_ref().map(question_texts);
        let asked = question.as_ref().map(|(title, text, yes)| widgets::Question {
            title,
            text,
            yes,
            yes_enabled: self.mods.install.is_none(),
        });
        let ctx = ui.ctx().clone();
        let Some(yes) = widgets::question(&ctx, egui::Id::new("mods-confirm"), asked) else { return false };
        let app = self.app_dir.clone();
        match self.mods.confirm.take().filter(|_| yes) {
            Some(Confirm::Remove { folder, .. }) => self.mods.remove(&app, &folder, false),
            Some(Confirm::Delete { folder, .. }) => self.mods.remove(&app, &folder, true),
            Some(Confirm::Replace { package, .. }) => {
                self.mods.start_install(&ctx, app, package, true);
                false
            }
            None => false,
        }
    }
}

fn question_texts(confirm: &Confirm) -> (String, String, &'static str) {
    match confirm {
        Confirm::Remove { folder, title } => (
            format!("Remove {title}?"),
            format!("The folder {folder} goes to the system's trash, and its entry leaves the list."),
            "Move to the trash",
        ),
        Confirm::Delete { folder, why } => (
            format!("Delete {folder} for good?"),
            format!("The system's trash is not available here ({why}). Deleting cannot be undone."),
            "Delete permanently",
        ),
        Confirm::Replace { name, installed, .. } => (
            format!("Replace {name} {installed}?"),
            "A package of this name is installed. The installed version goes to the system's trash (where there is \
             none, it is deleted), the new one takes its place in the list."
                .to_string(),
            "Replace",
        ),
    }
}

fn remarks(ui: &mut Ui, remarks: &mod_list::Remarks) {
    egui::CollapsingHeader::new(format!("Remarks of the install ({})", remarks.count)).id_salt("install-remarks").show(
        ui,
        |ui| {
            egui::ScrollArea::vertical().id_salt("install-remarks-list").max_height(220.0).show(ui, |ui| {
                for r in &remarks.kept {
                    ui.label(RichText::new(ssx_core::text::Text::new(r).into_string()).color(theme::TEXT_SOFT));
                }
                if remarks.count > remarks.kept.len() {
                    widgets::note(ui, &format!("… and {} more", remarks.count - remarks.kept.len()));
                }
            });
        },
    );
}

pub(super) fn below_mods(app: &Path, text: &str) -> String {
    let mods = format!("{}{}", app.join(MODS_DIR).display(), std::path::MAIN_SEPARATOR);
    text.replace(&mods, "")
}

fn mod_item(
    ui: &mut Ui,
    app: &Path,
    id: egui::Id,
    row: &ModFolder,
    skipped: Option<String>,
    busy: bool,
    selected: bool,
) -> rows::ItemResponse {
    let title = title_of(row);
    let usable = row.package.is_ok();
    let meta = match &row.package {
        Ok(m) => {
            let r = &m.resource;
            let kind = format!("{:?}", r.kind).to_lowercase();
            format!("{}  ·  {kind}  ·  priority {}", r.version, r.priority)
        }
        Err(_) => "cannot be used".to_owned(),
    };
    let chip = match (&row.package, row.enabled, &skipped) {
        (Err(_), _, _) => ("UNUSABLE", theme::ERROR),
        (Ok(_), true, Some(_)) => ("SKIPPED", theme::WARN),
        (Ok(_), true, None) => ("ON", theme::GOOD),
        (Ok(_), false, _) => ("OFF", theme::TEXT_DIM),
    };
    let item = Item {
        switch: Some(row.enabled),
        name: &title,
        meta: &meta,
        chip: Some(chip),
        enabled: !busy && (usable || row.enabled),
        selected,
    };
    rows::item(ui, id, item, || mod_detail(app, row, &title, skipped))
}

fn mod_detail(app: &Path, row: &ModFolder, title: &str, skipped: Option<String>) -> Detail {
    let mut d = Detail::new(title);
    match &row.package {
        Ok(m) => {
            let r = &m.resource;
            if !r.description.is_empty() {
                d = d.text(ssx_core::text::shown_max(&r.description.replace(['\n', '\t'], " "), 512).to_string());
            }
            let mut kinds = std::collections::BTreeMap::new();
            for id in m.content_ids() {
                *kinds.entry(id.kind()).or_insert(0usize) += 1;
            }
            let content: Vec<String> = kinds.iter().map(|(k, n)| format!("{k} ×{n}")).collect();
            let authors = match r.authors.is_empty() {
                true => "no authors named".to_string(),
                false => ssx_core::text::shown_max(&r.authors.join(", "), 512).to_string(),
            };
            d = d
                .fact("VERSION", r.version.to_string())
                .fact("BY", authors)
                .fact("TYPE", format!("{:?}", r.kind).to_lowercase())
                .fact("PRIORITY", r.priority.to_string())
                .fact("FOLDER", ssx_core::text::shown_max(&row.folder, 256).to_string());
            if !content.is_empty() {
                d = d.fact("CONTENT", content.join(", "));
            }
            if !m.patches.is_empty() {
                d = d.fact("PATCHES", m.patches.len().to_string());
            }
            if !row.listed {
                d = d.text("New: off until switched on.");
            }
            let mut warnings = Vec::new();
            if m.gpu_code() {
                warnings.push(
                    "Contains GPU code: shader hooks run on your graphics card (client mods only; servers refuse \
                     them)."
                        .to_owned(),
                );
            }
            if m.actionscript() {
                warnings.push(
                    "Contains ActionScript: its screens run code in the game's menus, within the sandbox's caps \
                     (client mods only; servers refuse them)."
                        .to_owned(),
                );
            }
            if let Some(reason) = skipped {
                warnings.push(format!("Skipped by the game: {}", below_mods(app, &reason)));
            }
            if !warnings.is_empty() {
                d = d.warn(Some(warnings.join(" ")));
            }
        }
        Err(e) => {
            d = d
                .warn(Some(format!("Cannot be used: {}", below_mods(app, e))))
                .fact("FOLDER", ssx_core::text::shown_max(&row.folder, 256).to_string());
        }
    }
    d.applies(
        "Enter (a pad's A) or its switch turns it on or off; the buttons under the list act on the mod chosen here. \
         The game reads the list at its next start.",
    )
}
