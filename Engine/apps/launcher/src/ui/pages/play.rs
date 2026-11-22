use eframe::egui::{self, Color32, CursorIcon, Sense, Ui};
use ssx_assets::port_options::{GraphicsLevers, GraphicsPreset};

use crate::app::Launcher;
use crate::art::Slot;
use crate::catalog;
use crate::jobs::Program;
use crate::ui::icons::Icon;
use crate::ui::layout::Layout;
use crate::ui::tiles::{self, Hero, Tile};
use crate::ui::{Page, theme};

const TILES: [(Slot, Icon, &str, Page, &str); 3] = [
    (Slot::TileExtras, Icon::Globe, "ONLINE", Page::Online, "tile_online"),
    (Slot::TileMods, Icon::Layers, "CONTENT", Page::Content, "tile_content"),
    (Slot::TileSettings, Icon::Gear, "SETTINGS", Page::Settings, "tile_settings"),
];

struct HeroText {
    title: &'static str,
    line: (String, Color32),
    tab: (&'static str, Color32),
    enabled: bool,
}

impl Launcher {
    fn hero_text(&self) -> HeroText {
        let soft = theme::TEXT_SOFT;
        if let Some(p) = self.setup_progress() {
            let line = format!("{:.0} %  ·  {}", p.fraction * 100.0, p.line);
            return HeroText {
                title: "SETTING UP GAME DATA",
                line: (line, soft),
                tab: ("SETTING UP", theme::CYAN),
                enabled: true,
            };
        }
        if !self.data_ready {
            let (title, line, tab) = crate::data_state::hero_unplayable(&self.data_state);
            return HeroText { title, line: (line.to_owned(), soft), tab: (tab, theme::WARN), enabled: true };
        }
        if self.tracks.is_empty() {
            let dir = self.settings.big_root().join(catalog::TRACKS_DIR);
            let line = (format!("No tracks found under {}", dir.display()), theme::ERROR);
            return HeroText { title: "GAME DATA INCOMPLETE", line, tab: ("SETUP NEEDED", theme::WARN), enabled: true };
        }
        let title = "PLAY SSX (2012)";
        if let Some(e) = &self.launch_error {
            return HeroText {
                title,
                line: (e.clone(), theme::ERROR),
                tab: ("START FAILED", theme::ERROR),
                enabled: true,
            };
        }
        let name = self.started.name();
        match self.viewer.as_ref().map(|v| (v.pid, v.status())) {
            Some((pid, None)) => {
                let more = match self.started {
                    Program::Game => "The boot movies come first; F1 skips one.",
                    Program::Viewer => "Loading a track takes a few seconds.",
                    Program::Benchmark => "Its scenes open the game's window one after another.",
                };
                let line = (format!("{name} running (process {pid}). {more}"), theme::CYAN_SOFT);
                HeroText { title, line, tab: ("RUNNING", theme::CYAN), enabled: false }
            }
            Some((_, Some(status))) => {
                let ok = status == "exited normally";
                let more = if ok { "" } else { " See Extras, Log." };
                let line = (format!("{name} {status}.{more}"), if ok { theme::GOOD } else { theme::ERROR });
                HeroText { title, line, tab: ("READY", theme::GOOD), enabled: true }
            }
            None => {
                if let Some(p) = self.update.progress() {
                    let line = format!("Downloading the update… {:.0} %", p.fraction * 100.0);
                    return HeroText {
                        title,
                        line: (line, theme::CYAN_SOFT),
                        tab: ("UPDATING", theme::CYAN),
                        enabled: true,
                    };
                }
                if let Some(release) = self.update.ready() {
                    let line = match release.reinstall.is_empty() {
                        true => format!("Update v{} ready: restart in Extras, Updates.", release.version),
                        false => "The repair is ready: restart in Extras, Updates.".to_owned(),
                    };
                    return HeroText {
                        title,
                        line: (line, theme::CYAN_SOFT),
                        tab: ("UPDATE READY", theme::CYAN),
                        enabled: true,
                    };
                }
                if let Some(release) = self.update.available().filter(|r| r.reinstall.is_empty()) {
                    let line = format!("A new version, v{}, is available: Extras, Updates.", release.version);
                    return HeroText {
                        title,
                        line: (line, theme::CYAN_SOFT),
                        tab: ("UPDATE", theme::CYAN),
                        enabled: true,
                    };
                }
                if let [first, rest @ ..] = self.program_missing.as_slice() {
                    let line = match rest.len() {
                        0 => format!("This installation lacks {first}: repair it in Extras, Updates."),
                        more => format!(
                            "This installation lacks {first} and {more} more files: repair it in Extras, Updates."
                        ),
                    };
                    return HeroText {
                        title,
                        line: (line, theme::WARN),
                        tab: ("REPAIR NEEDED", theme::WARN),
                        enabled: true,
                    };
                }
                if !self.data_state.is_complete() {
                    let line = (crate::data_state::HERO_JOIN_REPAIR.to_owned(), theme::WARN);
                    return HeroText { title, line, tab: ("REPAIR NEEDED", theme::WARN), enabled: true };
                }
                let line = match (&self.setup_message, self.mods_line()) {
                    (Some((true, message)), _) => (message.clone(), theme::GOOD),
                    (_, Some(line)) => line,
                    _ => ("Experience the original game, now on PC.".to_owned(), soft),
                };
                HeroText { title, line, tab: ("READY", theme::GOOD), enabled: true }
            }
        }
    }

    fn mods_line(&self) -> Option<(String, Color32)> {
        let r = self.mods.snapshot.as_ref()?.report.as_ref()?;
        let problems = !r.skipped.is_empty() || !r.registry.conflicts.is_empty() || r.list_error.is_some();
        let colour = if problems { theme::WARN } else { theme::TEXT_SOFT };
        let more = if problems { " See Content, Mods: What the game will mount." } else { "" };
        Some((format!("{}{more}", super::mods_report::summary(r)), colour))
    }

    pub fn hero_clicked(&mut self, ctx: &egui::Context) {
        if !self.data_ready || self.tracks.is_empty() || self.job.is_some() {
            self.go_to_setup();
        } else if !self.running() {
            self.play(ctx);
        }
    }

    fn tile_line(&self, page: Page) -> String {
        match page {
            Page::Online => {
                let server = match self.host.server_running() {
                    true => format!("Server running on port {}.", self.settings.host.port),
                    false => "Server: off.".to_owned(),
                };
                format!("Host a server for friends. {server}")
            }
            Page::Content => {
                let mods = self.mods.snapshot.as_ref().map_or(0, |s| s.rows.iter().filter(|r| r.enabled).count());
                let mut parts = vec![match mods {
                    0 => "No mods on".to_owned(),
                    1 => "1 mod on".to_owned(),
                    n => format!("{n} mods on"),
                }];
                let dlc = self.dlc.snapshot.as_ref().map_or(0, |s| s.set.packages.iter().filter(|p| p.enabled).count());
                if dlc > 0 {
                    parts.push(format!("{dlc} DLC"));
                }
                if let Some(tu) = self.tu.snapshot.as_ref().and_then(|s| s.set.mounted()) {
                    parts.push(format!("title update {}", tu.version_text()));
                }
                if !self.data_ready {
                    parts.push("game data: set up first".to_owned());
                }
                parts.join("  ·  ")
            }
            _ => {
                let s = &self.settings;
                let levers = GraphicsLevers {
                    resolution_scale: s.resolution_scale,
                    shadow_quality: s.shadow_quality,
                    bloom: s.bloom,
                    fxaa: s.fxaa,
                    view_distance: s.view_distance,
                };
                let preset = GraphicsPreset::of(&levers).map_or("Custom", GraphicsPreset::name);
                let mode = match s.display.mode {
                    ssx_assets::display::WindowMode::Windowed => "window",
                    ssx_assets::display::WindowMode::Borderless => "borderless",
                    ssx_assets::display::WindowMode::Exclusive => "exclusive fullscreen",
                };
                format!("{preset} preset  ·  {mode}  ·  VSync {}", if s.vsync { "on" } else { "off" })
            }
        }
    }

    pub fn play_page(&mut self, ui: &mut Ui, l: &Layout) {
        let painter = ui.painter().clone();
        let forced = self.forced_hover.clone();
        let words = self.hero_text();
        let mut response = ui.interact(l.hero, ui.id().with("hero"), Sense::click());
        if words.enabled {
            response = response.on_hover_cursor(CursorIcon::PointingHand);
        }
        if response.clicked() {
            self.hero_clicked(ui.ctx());
            if !response.clicked_by(egui::PointerButton::Primary) && self.page != Page::Play {
                self.enter_page = true;
            }
        }
        let hero = Hero {
            title: words.title,
            line: (&words.line.0, words.line.1),
            tab: words.tab,
            hovered: response.hovered() || response.has_focus() || forced.as_deref() == Some("hero"),
            enabled: words.enabled,
        };
        tiles::hero(&painter, l, l.hero, self.gallery.get(Slot::Hero), &hero);
        for (i, (slot, icon, title, page, name)) in TILES.into_iter().enumerate() {
            let rect = l.tiles[i];
            let response =
                ui.interact(rect, ui.id().with(name), Sense::click()).on_hover_cursor(CursorIcon::PointingHand);
            if response.clicked() {
                self.enter_page = !response.clicked_by(egui::PointerButton::Primary);
                self.go(page);
            }
            let hovered = response.hovered() || response.has_focus() || forced.as_deref() == Some(name);
            let text = self.tile_line(page);
            tiles::tile(&painter, l, rect, self.gallery.get(slot), &Tile { icon, title, text: &text, hovered });
        }
    }
}
