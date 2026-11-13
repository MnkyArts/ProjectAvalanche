use std::path::Path;

use eframe::egui::{self, RichText, Ui};
use ssx_assets::content::mods::{MODS_DIR, ModFolder};

use crate::app::Launcher;
use crate::mod_list::{self, Confirm};
use crate::ui::layout::Layout;
use crate::ui::theme::{self, Face, size};
use crate::ui::widgets::{self, Kind};
use crate::ui::{ModsTab, panel};

enum RowAction {
    Enable(usize, bool),
    Move(usize, isize),
    Remove(String, String),
    Forget(String),
}

impl Launcher {
    pub fn mods_page(&mut self, ui: &mut Ui, l: &Layout) {
        let body = panel::frame(ui.painter(), l, "MODS");
        panel::tabs(ui, l, &ModsTab::ALL, &mut self.mods_tab);
        let tab = self.mods_tab;
        panel::body(ui, body, "mods", |ui| match tab {
            ModsTab::Installed => self.mods_installed(ui),
            ModsTab::Report => self.mods_report(ui),
        });
    }

    pub fn refresh_mods(&mut self, ctx: &egui::Context) {
        let big = self.data_ready.then(|| self.settings.big_root());
        self.mods.refresh(ctx, self.app_dir.clone(), big);
    }

    fn mods_installed(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        let app = self.app_dir.clone();
        let installing = self.mods.install.is_some();
        ui.spacing_mut().item_spacing.y = 6.0;
        ui.horizontal(|ui| {
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
            if installing {
                ui.spinner();
                ui.label("Installing and checking…");
            }
        });
        if let Some((ok, text)) = &self.mods.message {
            widgets::status(ui, if *ok { theme::GOOD } else { theme::ERROR }, text.clone());
        }
        if self.mods.remarks.count > mod_list::INLINE_REMARKS {
            remarks(ui, &self.mods.remarks);
        }
        if self.mods_confirm(ui) {
            self.refresh_mods(&ctx);
        }
        let Some(snapshot) = &self.mods.snapshot else {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Reading the mods folder…");
            });
            return;
        };
        if let Err(e) = &snapshot.list {
            widgets::status(
                ui,
                theme::ERROR,
                format!(
                    "mods.toml cannot be read: {e}. The game mounts no mod until it can; a change here rewrites it."
                ),
            );
        }
        if snapshot.rows.is_empty() {
            widgets::heading(ui, "No mods installed");
            widgets::paragraph(
                ui,
                "A mod is a package — a folder or a .zip with a resource.toml — made of your own files: textures, \
                 models, skies, sounds, music, riders, screens, strings, maps, gamemodes, scripts and more. Install \
                 one with the buttons above: it is checked as it is installed. To make one, see \"Making a Mod\" in \
                 the Project Avalanche wiki; the game's --mod-dev <folder> shows your package live while you edit it.",
            );
            widgets::path(ui, &app.join(MODS_DIR));
            return;
        }
        widgets::note(
            ui,
            "Switched-on mods load in this order: a higher priority wins on the same id; on equal priority, the lower \
             one in this list.",
        );
        let skipped = |folder: &str| {
            snapshot
                .report
                .as_ref()
                .and_then(|r| r.skipped.iter().find(|s| s.folder == folder))
                .map(|s| s.reason.clone())
        };
        let n = snapshot.rows.len();
        let mut action = None;
        for (i, row) in snapshot.rows.iter().enumerate() {
            let reason = if row.enabled { skipped(&row.folder) } else { None };
            if let Some(a) = ui.push_id(&row.folder, |ui| mod_row(ui, &app, i, n, row, reason, installing)).inner {
                action = Some(a);
            }
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

fn mod_row(
    ui: &mut Ui,
    app: &Path,
    i: usize,
    n: usize,
    row: &ModFolder,
    skipped: Option<String>,
    busy: bool,
) -> Option<RowAction> {
    let mut action = None;
    let line = if row.enabled { theme::LINE } else { theme::LINE_FAINT };
    egui::Frame::new().fill(theme::PANEL).stroke(egui::Stroke::new(1.0, line)).inner_margin(6.0).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.spacing_mut().item_spacing.y = 2.0;
        let (title, version) = match &row.package {
            Ok(m) => {
                let t = if m.resource.title.is_empty() { m.resource.name.clone() } else { m.resource.title.clone() };
                (t, m.resource.version.to_string())
            }
            Err(_) => (row.folder.clone(), String::new()),
        };
        let title = ssx_core::text::shown_max(&title, 256).to_string();
        let (left, right) = egui::containers::Sides::new().shrink_left().truncate().show(
            ui,
            |ui| {
                let mut on = row.enabled;
                let usable = row.package.is_ok();
                let changed =
                    ui.add_enabled(!busy && (usable || row.enabled), egui::Checkbox::without_text(&mut on)).changed();
                let colour = if row.enabled { theme::TEXT } else { theme::TEXT_SOFT };
                ui.add(egui::Label::new(RichText::new(&title).font(Face::Title.id(size::TEXT)).color(colour)).truncate());
                changed.then_some(RowAction::Enable(i, on))
            },
            |ui| {
                let mut action = None;
                match &row.package {
                    Ok(_) => {
                        if widgets::button(ui, "Remove", Kind::Plain, !busy).clicked() {
                            action = Some(RowAction::Remove(row.folder.clone(), title.clone()));
                        }
                    }
                    Err(_) if row.present => {
                        if widgets::button(ui, "Remove", Kind::Plain, !busy).clicked() {
                            action = Some(RowAction::Remove(row.folder.clone(), row.folder.clone()));
                        }
                    }
                    Err(_) => {
                        if widgets::button(ui, "Forget", Kind::Plain, !busy).clicked() {
                            action = Some(RowAction::Forget(row.folder.clone()));
                        }
                    }
                }
                if widgets::button(ui, "Down", Kind::Plain, !busy && i + 1 < n).clicked() {
                    action = Some(RowAction::Move(i, 1));
                }
                if widgets::button(ui, "Up", Kind::Plain, !busy && i > 0).clicked() {
                    action = Some(RowAction::Move(i, -1));
                }
                if let Ok(m) = &row.package {
                    ui.label(RichText::new(format!("priority {}", m.resource.priority)).color(theme::TEXT_DIM));
                }
                ui.label(RichText::new(version).color(theme::TEXT_DIM));
                action
            },
        );
        if let Some(a) = left.or(right) {
            action = Some(a);
        }
        match &row.package {
            Ok(m) => {
                let r = &m.resource;
                let mut kinds = std::collections::BTreeMap::new();
                for id in m.content_ids() {
                    *kinds.entry(id.kind()).or_insert(0usize) += 1;
                }
                let content: Vec<String> = kinds.iter().map(|(k, n)| format!("{k} ×{n}")).collect();
                let authors = if r.authors.is_empty() {
                    "no authors named".to_string()
                } else {
                    format!("by {}", ssx_core::text::shown_max(&r.authors.join(", "), 512))
                };
                let folder = ssx_core::text::shown_max(&row.folder, 256);
                let mut facts = vec![authors, format!("{:?}", r.kind).to_lowercase(), format!("folder {folder}")];
                if !content.is_empty() {
                    facts.push(content.join(", "));
                }
                if !m.patches.is_empty() {
                    facts.push(format!("{} patches", m.patches.len()));
                }
                if !row.listed {
                    facts.push("new: off until switched on".into());
                }
                if !r.description.is_empty() {
                    facts.push(ssx_core::text::shown_max(&r.description.replace(['\n', '\t'], " "), 512).to_string());
                }
                let text = facts.join("  ·  ");
                ui.add(egui::Label::new(RichText::new(&text).color(theme::TEXT_DIM)).truncate())
                    .on_hover_text(widgets::shown(&r.description));
                if m.gpu_code() {
                    widgets::status(
                        ui,
                        theme::WARN,
                        "contains GPU code: shader hooks run on your graphics card (client mods only; servers refuse them)",
                    );
                }
                if m.actionscript() {
                    widgets::status(
                        ui,
                        theme::WARN,
                        "contains ActionScript: its screens run code in the game's menus, within the sandbox's caps \
                         (client mods only; servers refuse them)",
                    );
                }
            }
            Err(e) => widgets::status(ui, theme::ERROR, format!("cannot be used: {}", below_mods(app, e))),
        }
        if let Some(reason) = skipped.filter(|_| row.package.is_ok()) {
            widgets::status(ui, theme::WARN, format!("skipped by the game: {}", below_mods(app, &reason)));
        }
    });
    action
}
