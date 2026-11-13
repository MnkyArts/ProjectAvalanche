use eframe::egui::{self, RichText, Ui};

use crate::app::Launcher;
use crate::art::{Slot, Source};
use crate::settings::Settings;
use crate::ui::footer;
use crate::ui::theme::{self, Face, size};
use crate::ui::widgets::{self, Kind};

const DISCORD: &str = "https://discord.com/invite/XrFGQEfwdJ";
const RELEASES: &str = "https://github.com/MnkyArts/ProjectAvalanche/releases";
const ISSUES: &str = "https://github.com/MnkyArts/ProjectAvalanche/issues";

impl Launcher {
    pub fn about_section(&self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            ui.label(RichText::new("Project Avalanche").font(Face::Title.id(size::TITLE)).color(theme::TEXT));
            ui.label(RichText::new(footer::version()).color(theme::TEXT_DIM));
        });
        widgets::paragraph(
            ui,
            "A native PC port of SSX (2012), written from the ground up in Rust on Bevy. No emulation and no \
             recompilation: the original data formats and the game's behaviour are reverse-engineered and \
             re-implemented. The port is unfinished.",
        );
        ui.add_space(6.0);
        egui::Grid::new("about-version").num_columns(2).spacing([16.0, 4.0]).show(ui, |ui| {
            widgets::field(ui, "Version");
            ui.label(RichText::new(footer::version()).color(theme::TEXT));
            ui.end_row();
            widgets::field(ui, "Updates");
            ui.label(
                RichText::new("Extras, Updates: newer releases come from the project's GitHub releases.")
                    .color(theme::TEXT_DIM),
            );
            ui.end_row();
        });
        ui.add_space(6.0);
        ui.horizontal_wrapped(|ui| {
            if widgets::button(ui, "Join the Discord", Kind::Primary, true).clicked() {
                ui.ctx().open_url(egui::OpenUrl::new_tab(DISCORD));
            }
            if widgets::button(ui, "Releases", Kind::Plain, true).clicked() {
                ui.ctx().open_url(egui::OpenUrl::new_tab(RELEASES));
            }
            if widgets::button(ui, "Report an issue", Kind::Plain, true).clicked() {
                ui.ctx().open_url(egui::OpenUrl::new_tab(ISSUES));
            }
        });
        ui.add_space(8.0);
        widgets::heading(ui, "Your game data");
        widgets::paragraph(
            ui,
            "No game data is included. The launcher unpacks your own SSX (2012) Xbox 360 disc image into the data \
             folder; the image is only read. The pictures of this window come from that data too, or are painted \
             in code: none is shipped.",
        );
        ui.add_space(8.0);
        widgets::heading(ui, "Credits");
        widgets::paragraph(
            ui,
            "Built on the work of others. Their code was read as a lead or used as a reference to check against; \
             every claim was re-checked on the retail disc and the port's code is our own.",
        );

        group(ui, "SSX community");
        credit(
            ui,
            "SSX-Library",
            Some("https://github.com/GlitcherOG/SSX-Library"),
            "the community's research into the SSX (2012) formats on the PS3 disc: BIG archives, GEOM models, \
             attribute vaults, the skinned models' scene files (CRSF).",
        );
        credit(
            ui,
            "SSX Collection Multitool",
            Some("https://github.com/GlitcherOG/SSX-Collection-Multitool"),
            "the same research as a tool.",
        );
        credit(
            ui,
            "bigfile",
            Some("https://github.com/SSXModding/bigfile"),
            "EA BIG archives and RefPack compression.",
        );
        credit(
            ui,
            "SSXTools",
            Some("https://github.com/SSXModding/SSXTools"),
            "EA BIG archives and RefPack compression.",
        );
        credit(ui, "SSX wiki", Some("https://ssx.computernewb.com/wiki/Formats/Common:BIG"), "the BIG format.");

        group(ui, "Xbox 360");
        credit(
            ui,
            "Xenia",
            Some("https://github.com/xenia-canary/xenia-canary"),
            "GPU texture tiling and formats, XPR, LZX, XEX loading, XMA packet headers, kernel import tables, \
             and the PowerPC / VMX128 opcode tables.",
        );
        credit(ui, "Velocity", Some("https://github.com/hetelek/Velocity"), "STFS and XEX notes.");
        credit(
            ui,
            "extract-xiso",
            Some("https://github.com/XboxDev/extract-xiso"),
            "the lead for our XDVDFS disc reader, and its cross-check.",
        );
        credit(
            ui,
            "XeXtractor",
            Some("https://github.com/XboxChef/XeXtractor"),
            "XEX2 structures, the base file's decryption and decompression.",
        );

        group(ui, "Audio and video");
        credit(
            ui,
            "vgmstream",
            Some("https://github.com/vgmstream/vgmstream"),
            "the EA audio containers (SNR / SNS, ABK / AST, SBR / SBS, Pathfinder MPF / MUS), EALayer3 and the \
             EA movies' audio; through it, Zench's ealayer3.",
        );
        credit(
            ui,
            "FFmpeg",
            Some("https://ffmpeg.org"),
            "the EA movie container, the VP6 decoder and its codec tables, the WMA Pro / XMA decoder; the \
             reference our movie and MP3 decoding was checked against.",
        );
        credit(ui, "mpg123", Some("https://www.mpg123.de"), "a reference decoder for our MPEG audio decoding.");
        credit(
            ui,
            "minimp3",
            Some("https://github.com/lieff/minimp3"),
            "a reference decoder for our MPEG audio decoding.",
        );
        credit(ui, "nanomp3", None, "a minimp3 port; a reference decoder for our MPEG audio decoding.");
        credit(
            ui,
            "symphonia",
            Some("https://github.com/pdeljanov/Symphonia"),
            "its tables were the lead when transcribing the MP3 layer III tables.",
        );

        group(ui, "Flash and Scaleform");
        credit(ui, "SWF 10 file format specification", None, "the tags, the shapes and the AVM1 actions (Adobe).");
        credit(
            ui,
            "Ruffle",
            Some("https://github.com/ruffle-rs/ruffle"),
            "its AVM1 implementation as reading material.",
        );
        credit(
            ui,
            "JPEXS Free Flash Decompiler",
            Some("https://github.com/jindrapetrik/jpexs-decompiler"),
            "tag lists for Scaleform's own tags.",
        );

        group(ui, "Other references");
        credit(
            ui,
            "Lua 5.1",
            Some("https://www.lua.org"),
            "the reference manual and the sources our Lua implementation follows (Lua.org, PUC-Rio).",
        );
        credit(
            ui,
            "Microsoft DDS documentation",
            Some("https://learn.microsoft.com/en-us/windows/win32/direct3ddds/dx-graphics-dds-pguide"),
            "DDS_HEADER, DDS_PIXELFORMAT.",
        );
        credit(
            ui,
            "libmspack",
            Some("https://github.com/kyz/libmspack"),
            "the LZX conventions our LZX decoder was compared with.",
        );
        credit(ui, "lzxd", Some("https://crates.io/crates/lzxd"), "the cross-check of our LZX decoder.");

        group(ui, "Reverse engineering");
        credit(ui, "Ghidra", Some("https://ghidra-sre.org"), "the analysis of the game's executable (NSA).");
        credit(ui, "GhidraMCP", Some("https://github.com/bethington/ghidra-mcp"), "served to our tools by GhidraMCP.");
        credit(ui, "ImHex", Some("https://github.com/WerWolv/ImHex"), "hex work (WerWolv).");
        credit(ui, "Kaitai Struct", Some("https://kaitai.io"), "structure work.");
        credit(
            ui,
            "skate-3-rust-engine",
            Some("https://github.com/SK8-ENGINE/skate-3-rust-engine"),
            "ideas from a comparable Rust / Bevy port of an EA Xbox 360 game.",
        );
        credit(
            ui,
            "skate-3-trick-toolkit",
            Some("https://github.com/chasmlol/skate-3-trick-toolkit"),
            "the ABIN animation importer.",
        );
        credit(ui, "Skate 3 Custom Engine Layer contributors", None, "the collision mesh reader.");

        group(ui, "The engine");
        credit(ui, "Bevy", Some("https://bevy.org"), "the engine.");
        credit(ui, "egui / eframe", Some("https://github.com/emilk/egui"), "the launcher.");
        credit(ui, "The Rust crates in Engine/Cargo.lock", None, "the other libraries the port builds on.");
        credit(
            ui,
            "Saira Condensed",
            Some("https://fonts.google.com/specimen/Saira+Condensed"),
            "the launcher's font, SIL Open Font License (OFL.txt).",
        );

        ui.add_space(8.0);
        widgets::heading(ui, "AI assistance");
        widgets::paragraph(
            ui,
            "AI tools assisted parts of the development and documentation. Findings are verified against the \
             retail disc wherever possible — whole-disc verification for every parser, cross-checks against the \
             game's code, comparisons with reference decoders — but errors may remain. Assumptions not yet \
             decoded are marked TODO(guess) in the code. All analysis used the maintainer's own copy of the game; \
             no game data, assets or proprietary code are included.",
        );
        ui.add_space(8.0);
        widgets::heading(ui, "Licences and names");
        widgets::paragraph(
            ui,
            "The port's own code is MIT; the Rust crates keep their own licences. Type: Saira Condensed, \
             © 2016 The Saira Project Authors, SIL Open Font License 1.1 (OFL.txt beside the fonts in the \
             source). SSX (2012) and everything on its disc belong to Electronic Arts and are not part of this \
             project or its builds. SSX is a trademark of Electronic Arts Inc.; Project Avalanche is not \
             affiliated with or endorsed by Electronic Arts.",
        );
    }

    pub fn files_section(&mut self, ui: &mut Ui) {
        if self.files_frame != self.frame.checked_sub(1) {
            self.save_exists = self.save_path.is_file();
        }
        self.files_frame = Some(self.frame);
        let app = self.app_dir.clone();
        let save = self.save_path.clone();
        let save_note = if self.save_exists { "the game's profile" } else { "none yet: Play starts a new profile" };
        let rows = [
            ("App folder", app.clone(), "settings, saves, the log"),
            ("Settings", Settings::path(), "every choice made here"),
            ("Game data", self.settings.game_data(), "the unpacked archives (big/)"),
            ("Profile save", save, save_note),
            ("Game log", app.join(crate::jobs::GAME_LOG), "the game's output since Play last started it"),
            ("Tools log", app.join(crate::jobs::VIEWER_LOG), "the asset viewer's output (Extras, Tools)"),
            ("Your pictures", app.join("LauncherArt"), "your own <slot>.png replaces a picture below"),
            ("Picture cache", app.join("LauncherCache"), "decoded from the game data; safe to delete"),
        ];
        egui::Grid::new("files").num_columns(3).spacing([16.0, 2.0]).min_row_height(22.0).show(ui, |ui| {
            for (name, path, what) in rows {
                widgets::field(ui, name);
                let text = path.display().to_string();
                let label = egui::Label::new(RichText::new(&text).monospace().color(theme::TEXT)).truncate();
                ui.scope(|ui| {
                    ui.set_max_width(420.0);
                    if ui.add(label.sense(egui::Sense::click())).on_hover_text("Click to copy").clicked() {
                        ui.ctx().copy_text(text.clone());
                    }
                });
                widgets::note(ui, what);
                ui.end_row();
            }
        });
        ui.add_space(4.0);
        widgets::heading(ui, "Pictures of this window");
        egui::Grid::new("art").num_columns(3).spacing([16.0, 0.0]).min_row_height(22.0).show(ui, |ui| {
            for slot in Slot::ALL {
                widgets::field(ui, slot.name());
                let (source, note) = match self.gallery.get(slot) {
                    Some(art) => (
                        match art.source {
                            Source::User => "your file",
                            Source::Disc => "game data",
                            Source::BuiltIn => "built in",
                        },
                        art.note.as_str(),
                    ),
                    None if slot == Slot::Logo => ("none", "shown only from your file or your game data"),
                    None => ("loading", ""),
                };
                ui.label(RichText::new(source).color(theme::TEXT));
                ui.add(egui::Label::new(RichText::new(note).color(theme::TEXT_DIM)).truncate()).on_hover_text(note);
                ui.end_row();
            }
        });
        for problem in &self.gallery.problems {
            widgets::status(ui, theme::WARN, problem.clone());
        }
    }
}

fn group(ui: &mut Ui, name: &str) {
    ui.add_space(6.0);
    ui.label(RichText::new(name).color(theme::TEXT).strong());
}

fn credit(ui: &mut Ui, name: &str, url: Option<&str>, what: &str) {
    ui.horizontal_wrapped(|ui| {
        match url {
            Some(url) => {
                ui.hyperlink_to(RichText::new(name).color(theme::CYAN), url);
            }
            None => {
                ui.label(RichText::new(name).color(theme::TEXT));
            }
        }
        ui.label(RichText::new(format!("— {what}")).color(theme::TEXT_SOFT));
    });
}
