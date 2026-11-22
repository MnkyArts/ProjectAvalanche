use eframe::egui::{self, RichText, Ui};

use crate::app::Launcher;
use crate::command;
use crate::ui::details::Detail;
use crate::ui::rows::{self, Row};
use crate::ui::theme;
use crate::ui::widgets::{self, Kind};

impl Launcher {
    pub fn output_section(&mut self, ui: &mut Ui) {
        ui.spacing_mut().item_spacing.y = 0.0;
        let shown = self.show_command;
        rows::toggle(ui, Row::new("Show commands"), &mut self.show_command, || {
            Detail::new("Show commands")
                .text("The command lines Play and the tools' Launch run, to copy (with the options chosen here).")
                .now(if shown { "On" } else { "Off" }, None)
        });
        ui.spacing_mut().item_spacing.y = 6.0;
        self.started_line(ui);
        if self.show_command {
            let big = self.settings.big_root();
            let (game, tool) = (command::game(&self.settings, &big), command::viewer(&self.settings, &big));
            let mut text = format!("{}\n{}", game.display(), tool.display());
            ui.horizontal(|ui| {
                if widgets::button(ui, "Copy", Kind::Plain, true).clicked() {
                    ui.ctx().copy_text(text.clone());
                }
                let field = egui::TextEdit::multiline(&mut text).font(egui::TextStyle::Monospace);
                ui.add(field.desired_rows(2).desired_width(f32::INFINITY));
            });
        }
        ui.add_space(4.0);
        let title = format!("{} output", self.started.name());
        widgets::heading(ui, &title);
        match &self.viewer {
            Some(v) => {
                let lines = v.log.lock().map(|l| l.join("\n")).unwrap_or_default();
                let height = ui.available_height().max(120.0);
                egui::ScrollArea::vertical().id_salt("log").max_height(height).stick_to_bottom(true).show(ui, |ui| {
                    ui.add(
                        egui::Label::new(RichText::new(lines).monospace().size(11.0).color(theme::TEXT_SOFT)).wrap(),
                    );
                });
            }
            None => {
                widgets::note(ui, "Nothing was started yet. Play, or Launch under Tools, and the output appears here.")
            }
        }
    }
}
