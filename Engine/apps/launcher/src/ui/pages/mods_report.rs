use eframe::egui::{RichText, Ui};
use ssx_assets::content::mods::{ModsReport, ROUTED_KINDS};
use ssx_content::Layer;

use crate::app::Launcher;
use crate::ui::theme;
use crate::ui::widgets;

const MAX_ROWS: usize = 40;

fn capped(ui: &mut Ui, colour: eframe::egui::Color32, lines: impl ExactSizeIterator<Item = String>) {
    let n = lines.len();
    for line in lines.take(MAX_ROWS) {
        ui.label(RichText::new(ssx_core::text::Text::new(&line).into_string()).color(colour));
    }
    if n > MAX_ROWS {
        widgets::note(ui, &format!("… and {} more (all of them in the game's log at its start)", n - MAX_ROWS));
    }
}

impl Launcher {
    pub fn mods_report(&mut self, ui: &mut Ui) {
        ui.spacing_mut().item_spacing.y = 4.0;
        let Some(s) = &self.mods.snapshot else {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Working out what the game will mount…");
            });
            return;
        };
        let Some(r) = &s.report else {
            widgets::paragraph(ui, "No mod is installed or listed: the game starts with your disc's content only.");
            return;
        };
        let problems = !r.skipped.is_empty() || !r.registry.conflicts.is_empty() || r.list_error.is_some();
        widgets::status(ui, if problems { theme::WARN } else { theme::GOOD }, summary(r));
        if !s.base {
            widgets::note(ui, "Set up the game data to see which of the game's textures the mods replace.");
        }
        if !r.skipped.is_empty() {
            widgets::heading(ui, "Skipped at the start");
            let lines = r
                .skipped
                .iter()
                .map(|k| format!("{}: {}", k.folder, super::mods::below_mods(&self.app_dir, &k.reason)));
            capped(ui, theme::ERROR, lines.collect::<Vec<_>>().into_iter());
        }
        if !r.blocklists.is_empty() {
            widgets::heading(ui, "Blocklists followed");
            capped(ui, theme::TEXT_SOFT, r.blocklists.clone().into_iter());
        }
        if !r.registry.conflicts.is_empty() {
            widgets::heading(ui, "Conflicts (one id from several mods)");
            let lines = r
                .registry
                .conflicts
                .iter()
                .map(|c| format!("{}: {} wins over {}", c.id, c.winner, c.losers.join(", ")));
            capped(ui, theme::WARN, lines.collect::<Vec<_>>().into_iter());
        }
        if !s.unknown.is_empty() {
            widgets::heading(ui, "Ids nothing below them has");
            widgets::note(ui, "A typo, or an id of another version: the game uses these files nowhere.");
            capped(
                ui,
                theme::WARN,
                s.unknown.iter().map(|(p, id)| format!("{p}: {id}")).collect::<Vec<_>>().into_iter(),
            );
        }
        overrides(ui, r);
        idle(ui, r);
        if !r.registry.dangling_patches.is_empty() {
            widgets::heading(ui, "Patches of ids nobody provides");
            let lines = r.registry.dangling_patches.iter().map(|d| format!("{} patches {}", d.source, d.id));
            capped(ui, theme::WARN, lines.collect::<Vec<_>>().into_iter());
        }
        widgets::note(ui, &format!("Worked out in {:.2} s, as the game does at its start.", s.seconds));
    }
}

pub fn summary(r: &ModsReport) -> String {
    if let Some(e) = &r.list_error {
        return format!("No mod is mounted: mods.toml cannot be read ({e}).");
    }
    let plural = |n: usize, one: &str, many: &str| format!("{n} {}", if n == 1 { one } else { many });
    let mut s = format!("The game mounts {}", plural(r.mounted.len(), "mod", "mods"));
    if !r.skipped.is_empty() {
        s += &format!(", skips {}", plural(r.skipped.len(), "package", "packages"));
    }
    let c = r.registry.conflicts.len();
    if c > 0 {
        s += &format!("; {} between mods", plural(c, "conflict", "conflicts"));
    }
    s + "."
}

fn overrides(ui: &mut Ui, r: &ModsReport) {
    let mine: Vec<_> = r.registry.overrides.iter().filter(|o| o.winner.0 == Layer::ClientMods).collect();
    if mine.is_empty() {
        return;
    }
    widgets::heading(ui, "Replaced in the game");
    for m in &r.mounted {
        let ids: Vec<String> = mine.iter().filter(|o| o.winner.1 == m.name).map(|o| o.id.to_string()).collect();
        if ids.is_empty() {
            continue;
        }
        let n = ids.len();
        ui.label(RichText::new(format!("{} — {n} id{}", m.name, if n == 1 { "" } else { "s" })).color(theme::TEXT));
        capped(ui, theme::TEXT_SOFT, ids.into_iter());
    }
}

fn idle(ui: &mut Ui, r: &ModsReport) {
    let lines: Vec<String> = r
        .mounted
        .iter()
        .flat_map(|m| {
            let kinds: Vec<String> =
                m.kinds.iter().filter(|(k, _)| !ROUTED_KINDS.contains(k)).map(|(k, n)| format!("{k} ×{n}")).collect();
            let mut out = Vec::new();
            if !kinds.is_empty() {
                out.push(format!("{}: {} — not read from mods in this version", m.name, kinds.join(", ")));
            }
            if m.scripts > 0 {
                out.push(format!(
                    "{}: client scripts ({} patterns) — run in single player; in a network session only where its \
                     server allows client scripts",
                    m.name, m.scripts
                ));
            }
            out
        })
        .collect();
    if !lines.is_empty() {
        widgets::heading(ui, "No effect yet");
        capped(ui, theme::TEXT_DIM, lines.into_iter());
    }
}
