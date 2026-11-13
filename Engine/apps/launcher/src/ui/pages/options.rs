use eframe::egui::{self, RichText, Ui};
use ssx_assets::port_options::{BootMovie, MenuFocus, RangeMovies};

use crate::app::Launcher;
use crate::settings::{Music, Shading};
use crate::ui::theme;
use crate::ui::widgets;

fn shading_label(s: Shading) -> &'static str {
    match s {
        Shading::Auto => "Auto (textured when possible)",
        Shading::Textured => "Textured (the game's materials)",
        Shading::Height => "Height tint",
        Shading::Material => "Material debug palette",
    }
}

fn music_label(m: Music) -> &'static str {
    match m {
        Music::Auto => "Auto (the drop's Deadly Descent, else licensed)",
        Music::DeadlyDescent => "Deadly Descent",
        Music::Licensed => "Licensed tracks",
        Music::Off => "Off",
    }
}

fn boot_movie_label(b: BootMovie) -> &'static str {
    match b {
        BootMovie::EveryStart => "Every start (as the original game)",
        BootMovie::Skip => "Skip (as the game's restart after a sign-out)",
    }
}

fn range_movies_label(r: RangeMovies) -> &'static str {
    match r {
        RangeMovies::EveryTime => "Every time (as the original game)",
        RangeMovies::FirstTimeOnly => "First time only (once per profile)",
    }
}

fn menu_focus_label(m: MenuFocus) -> &'static str {
    match m {
        MenuFocus::FirstItem => "On the first item (as the original game)",
        MenuFocus::LastItem => "On the item picked last",
    }
}

impl Launcher {
    pub fn graphics_section(&mut self, ui: &mut Ui) {
        let s = &mut self.settings;
        ui.columns(2, |columns| {
            let ui = &mut columns[0];
            widgets::heading(ui, "Terrain");
            egui::Grid::new("terrain").num_columns(2).spacing([16.0, 10.0]).show(ui, |ui| {
                widgets::field(ui, "Subdivision");
                ui.add(egui::Slider::new(&mut s.subdivide, 0..=3).text("levels (2 = the game's)")).on_hover_text(
                    "How finely the terrain is smoothed: higher is smoother but heavier, 0 is the raw shape.",
                );
                ui.end_row();
                widgets::field(ui, "Shading");
                let combo =
                    egui::ComboBox::from_id_salt("shading").selected_text(shading_label(s.shading)).width(250.0);
                widgets::combo(ui, combo, |ui| {
                    for v in [Shading::Auto, Shading::Textured, Shading::Height, Shading::Material] {
                        ui.selectable_value(&mut s.shading, v, shading_label(v));
                    }
                })
                .response
                .on_hover_text(
                    "How the terrain surface is coloured; Height and Material are debug views, not the game's look.",
                );
                ui.end_row();
            });
            ui.add_space(8.0);
            widgets::heading(ui, "Rendering");
            ui.checkbox(&mut s.vsync, "VSync")
                .on_hover_text("Caps the frame rate at the display's refresh rate; off removes the cap but may tear.");
            ui.checkbox(&mut s.objects, "Placed objects")
                .on_hover_text("Draws the trees, rocks, rails and buildings on the track.");
            widgets::note(ui, "Off hides them, but you still collide with them. For performance testing only.");
            ui.checkbox(&mut s.wireframe, "Wireframe (F)")
                .on_hover_text("Draws the triangle mesh over everything, for debugging.");
            widgets::note(ui, "A debug overlay of the triangle mesh. Press F in the game to toggle it.");

            let ui = &mut columns[1];
            widgets::heading(ui, "Effects");
            ui.checkbox(&mut s.particles, "Particles")
                .on_hover_text("Snow spray and other particle effects; off can be faster.");
            ui.checkbox(&mut s.snow_tracks, "Snow tracks")
                .on_hover_text("The trails riders cut into the snow; off can be faster.");
            ui.checkbox(&mut s.local_lights, "Local lights")
                .on_hover_text("The lights placed along the track; off can be faster.");
            ui.checkbox(&mut s.game_post, "Post-processing")
                .on_hover_text("The game's screen effects such as bloom and colour grading.");
            widgets::note(ui, "Off also removes screen warnings, such as the low-oxygen blackout.");
            ui.checkbox(&mut s.game_sky, "Sky")
                .on_hover_text("The game's sky and clouds; off shows a flat colour instead.");
            ui.add_space(8.0);
            widgets::note(ui, "The game and the tools start with these options. They are saved as you change them.");
        });
    }

    pub fn audio_section(&mut self, ui: &mut Ui) {
        let s = &mut self.settings;
        widgets::heading(ui, "Music");
        egui::Grid::new("music").num_columns(2).spacing([16.0, 10.0]).show(ui, |ui| {
            widgets::field(ui, "Source");
            let combo = egui::ComboBox::from_id_salt("music").selected_text(music_label(s.music)).width(330.0);
            widgets::combo(ui, combo, |ui| {
                for v in [Music::Auto, Music::DeadlyDescent, Music::Licensed, Music::Off] {
                    ui.selectable_value(&mut s.music, v, music_label(v));
                }
            })
            .response
            .on_hover_text("Which soundtrack plays in the menus and on the mountain.");
            ui.end_row();
            widgets::field(ui, "Volume");
            ui.add_enabled(s.music != Music::Off, egui::Slider::new(&mut s.music_volume, 0.0..=1.0).fixed_decimals(2))
                .on_hover_text("Music loudness, 1.00 is full volume.");
            ui.end_row();
        });
        ui.add_space(8.0);
        widgets::heading(ui, "Sound effects");
        egui::Grid::new("sfx").num_columns(2).spacing([16.0, 10.0]).show(ui, |ui| {
            widgets::field(ui, "Effects");
            ui.checkbox(&mut s.sfx, "On").on_hover_text("Board, rider and menu sounds.");
            ui.end_row();
            widgets::field(ui, "Volume");
            ui.add_enabled(s.sfx, egui::Slider::new(&mut s.sfx_volume, 0.0..=1.0).fixed_decimals(2))
                .on_hover_text("Sound effect loudness, 1.00 is full volume.");
            ui.end_row();
        });
    }

    pub fn movies_section(&mut self, ui: &mut Ui) {
        let s = &mut self.settings;
        widgets::heading(ui, "Range movies");
        egui::Grid::new("range_movies").num_columns(2).spacing([16.0, 10.0]).show(ui, |ui| {
            widgets::field(ui, "Play");
            let combo = egui::ComboBox::from_id_salt("range_movies")
                .selected_text(range_movies_label(s.range_movies))
                .width(330.0);
            widgets::combo(ui, combo, |ui| {
                for v in RangeMovies::ALL {
                    ui.selectable_value(&mut s.range_movies, v, range_movies_label(v));
                }
            })
            .response
            .on_hover_text("The video of a mountain range, when you choose the range in World Tour.");
            ui.end_row();
        });
        ui.add_space(8.0);
        widgets::note(
            ui,
            "The original game plays a range's video every time you choose the range in World Tour. First time only \
             plays it the first time in a profile; a new profile plays them again. The game starts with this option, \
             and its Options (Movie Settings) change it too.",
        );
        ui.add_space(8.0);
        widgets::heading(ui, "Boot movie");
        egui::Grid::new("boot_movie").num_columns(2).spacing([16.0, 10.0]).show(ui, |ui| {
            widgets::field(ui, "Play");
            let combo =
                egui::ComboBox::from_id_salt("boot_movie").selected_text(boot_movie_label(s.boot_movie)).width(330.0);
            widgets::combo(ui, combo, |ui| {
                for v in BootMovie::ALL {
                    ui.selectable_value(&mut s.boot_movie, v, boot_movie_label(v));
                }
            })
            .response
            .on_hover_text("The video the game starts with, before the start screen.");
            ui.end_row();
        });
        ui.add_space(8.0);
        widgets::note(
            ui,
            "The original game plays its boot video at every start. Skip starts the way the game restarts itself \
             after a sign-out: straight to the start screen; the start screen and the sign-in stay as they are. The \
             game's Options (Movie Settings) change it too.",
        );
    }

    pub fn menus_section(&mut self, ui: &mut Ui) {
        let s = &mut self.settings;
        widgets::heading(ui, "Menus");
        egui::Grid::new("menu_focus").num_columns(2).spacing([16.0, 10.0]).show(ui, |ui| {
            widgets::field(ui, "Open menus");
            let combo =
                egui::ComboBox::from_id_salt("menu_focus").selected_text(menu_focus_label(s.menu_focus)).width(330.0);
            widgets::combo(ui, combo, |ui| {
                for v in MenuFocus::ALL {
                    ui.selectable_value(&mut s.menu_focus, v, menu_focus_label(v));
                }
            })
            .response
            .on_hover_text("Where the highlight is when you come back to a menu.");
            ui.end_row();
        });
        ui.add_space(8.0);
        widgets::note(
            ui,
            "The original game opens its main menu, pause menu and settings menu on their first item every time. With \
             \"On the item picked last\" they, and Project Avalanche's own menus, open on the item you picked there \
             last. The game starts with this option, and its Options (Menu Settings) change it too.",
        );
    }

    pub fn controls_section(&self, ui: &mut Ui) {
        for (what, keys) in CONTROLS {
            widgets::heading(ui, what);
            ui.label(RichText::new(keys).color(theme::TEXT_SOFT));
            ui.add_space(8.0);
        }
    }
}

const CONTROLS: [(&str, &str); 3] = [
    ("Gamepad", "An Xbox-style pad with the game's own layout (recommended: the game was made for a pad)."),
    (
        "Keys in the menus",
        "Enter = START, Space = A, Backspace = B, X, Y, arrow keys = D-pad, Q / E = LB / RB, W A S D = left stick \
         (the globe), F1 skips a movie.",
    ),
    (
        "Keys on the mountain",
        "Arrow keys or W A S D = left stick (steer, tuck, brake), Space = A (jump), I / J / L / K = Y / X / B / A, \
         Shift = RT, Ctrl = LT, E = RB, Backspace = LB (rewind), Esc = START (pause).",
    ),
];

impl Launcher {
    pub fn multiplayer_section(&mut self, ui: &mut Ui) {
        use crate::link_handler::{self, Status};
        widgets::heading(ui, "Server links");
        let status = self.link_status.get_or_insert_with(link_handler::status).clone();
        let mut on = status == Status::Ours;
        let available = !matches!(status, Status::Unavailable(_));
        let changed = ui
            .add_enabled(available, egui::Checkbox::new(&mut on, "Open avalanche:// links with Project Avalanche"))
            .changed();
        if changed {
            let done = if on { link_handler::register() } else { link_handler::unregister() };
            self.link_status = None;
            self.link_note = Some(match done {
                Ok(what) => what,
                Err(e) => format!("not changed: {e}"),
            });
        }
        match &status {
            Status::Other(cmd) => widgets::note(ui, &format!("Another program opens these links now: {cmd}")),
            Status::Unavailable(why) => widgets::note(ui, &format!("Not available on this system: {why}")),
            Status::Ours | Status::None => {}
        }
        if let Some(note) = &self.link_note {
            ui.label(RichText::new(note).color(theme::TEXT_SOFT));
        }
        ui.add_space(8.0);
        widgets::note(ui, LINK_NOTE);
        ui.add_space(8.0);
        widgets::heading(ui, "Your name on servers");
        match &self.settings.player_name {
            Some(name) => widgets::paragraph(ui, &widgets::shown(name)),
            None => widgets::paragraph(ui, "None set yet."),
        }
        widgets::note(ui, "Set in the game: Options, Player Name.");
    }
}

const LINK_NOTE: &str = if cfg!(windows) {
    "For your user only: the key HKCU\\Software\\Classes\\avalanche with the command \"<this launcher>\" --link \"%1\". \
     Unticking deletes that key. A link never joins by itself: the game shows the server and asks you first."
} else {
    "For your user only: the file ~/.local/share/applications/project-avalanche-link.desktop and the line \
     x-scheme-handler/avalanche=project-avalanche-link.desktop; in ~/.config/mimeapps.list. Unticking deletes both. \
     A link never joins by itself: the game shows the server and asks you first."
};
