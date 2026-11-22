use eframe::egui::{RichText, Ui};
use ssx_assets::port_options::{
    BootMovie, GraphicsLevers, GraphicsPreset, MenuFocus, RangeMovies, ResolutionScale, ShadowQuality, ViewDistance,
};

use crate::app::Launcher;
use crate::settings::Music;
use crate::ui::details::{self, Detail};
use crate::ui::rows::{self, Row, Slide};
use crate::ui::{theme, widgets};

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

pub fn scale_label(r: ResolutionScale) -> &'static str {
    match r {
        ResolutionScale::Native => "Native (the original's)",
        ResolutionScale::UltraQuality => "Ultra Quality (÷1.3)",
        ResolutionScale::Quality => "Quality (÷1.5)",
        ResolutionScale::Balanced => "Balanced (÷1.7)",
        ResolutionScale::Performance => "Performance (÷2)",
    }
}

pub fn shadow_label(q: ShadowQuality) -> String {
    let what = format!("{}² over {} m", q.map_size(), q.reach());
    match q {
        ShadowQuality::Original => format!("Original ({what})"),
        ShadowQuality::Medium => format!("Medium ({what})"),
        ShadowQuality::Low => format!("Low ({what})"),
    }
}

pub fn on_off(on: bool) -> &'static str {
    if on { "On" } else { "Off" }
}

pub fn switch_facts(detail: Detail, now: bool, original: bool) -> Detail {
    detail.now(on_off(now), Some((on_off(original).to_owned(), now == original)))
}

fn distance_fraction(meters: u32) -> f32 {
    let (min, max) = (f64::from(ViewDistance::MIN), f64::from(ViewDistance::MAX));
    ((f64::from(meters) / min).ln() / (max / min).ln()) as f32
}

fn distance_at(fraction: f32) -> Option<ViewDistance> {
    let (min, max) = (f64::from(ViewDistance::MIN), f64::from(ViewDistance::MAX));
    ViewDistance::nearest(min * (max / min).powf(f64::from(fraction)))
}

pub fn volume(value: &mut f32, slide: Slide) -> bool {
    let next = match slide {
        Slide::Still => return false,
        Slide::Step(steps) => ((*value * 20.0).round() + steps as f32) / 20.0,
        Slide::To(fraction) => (fraction * 20.0).round() / 20.0,
    }
    .clamp(0.0, 1.0);
    let changed = next != *value;
    *value = next;
    changed
}

pub fn percent(value: f32) -> String {
    format!("{:.0} %", value.clamp(0.0, 1.0) * 100.0)
}

impl Launcher {
    pub fn graphics_section(&mut self, ui: &mut Ui) {
        ui.spacing_mut().item_spacing.y = 0.0;
        let s = &mut self.settings;
        let levers = GraphicsLevers {
            resolution_scale: s.resolution_scale,
            shadow_quality: s.shadow_quality,
            bloom: s.bloom,
            fxaa: s.fxaa,
            view_distance: s.view_distance,
        };
        let preset = GraphicsPreset::of(&levers);
        const QUALITY_NOTE: &str = "The game's Options change these at once. Render resolution (Settings, Display) \
             scales everything, the menus too; the resolution scale only the mountain.";
        rows::group(ui, "Quality", None);
        let shown = preset.map_or("Custom", GraphicsPreset::name);
        let mut chosen = preset;
        let all: Vec<Option<GraphicsPreset>> = GraphicsPreset::ALL.into_iter().map(Some).collect();
        let row = Row::new("Preset").original(preset == Some(GraphicsPreset::Original));
        let detail = || {
            Detail::new("Preset")
                .text("Original is the original game's picture; High, Medium and Low trade some of it for speed.")
                .text(QUALITY_NOTE)
                .now(
                    shown,
                    Some((GraphicsPreset::Original.name().to_owned(), preset == Some(GraphicsPreset::Original))),
                )
                .applies(details::APPLIES_GAME)
        };
        let name = |p: &Option<GraphicsPreset>| p.map_or("Custom", GraphicsPreset::name).to_owned();
        if rows::pick(ui, row, &mut chosen, &all, name, detail)
            && let Some(p) = chosen
        {
            let l = p.levers();
            (s.resolution_scale, s.shadow_quality, s.bloom, s.fxaa, s.view_distance) =
                (l.resolution_scale, l.shadow_quality, l.bloom, l.fxaa, l.view_distance);
        }
        let original = ResolutionScale::default();
        let row = Row::new("Resolution scale").original(s.resolution_scale == original);
        let now = scale_label(s.resolution_scale);
        let detail = || {
            Detail::new("Resolution scale")
                .text(
                    "The mountain drawn at fewer pixels and brought up to the screen by AMD FidelityFX Super \
                     Resolution 1; the menus and the HUD stay at the screen's resolution.",
                )
                .now(now, Some((scale_label(original).to_owned(), now == scale_label(original))))
                .applies(details::APPLIES_GAME)
        };
        rows::pick(ui, row, &mut s.resolution_scale, &ResolutionScale::ALL, |r| scale_label(*r).to_owned(), detail);
        let meters = s.view_distance.meters();
        let original = ViewDistance::ORIGINAL.meters();
        let row = Row::new("View distance").original(s.view_distance.is_original());
        let detail = || {
            Detail::new("View distance")
                .text(
                    "How far the terrain's detail reaches. Nearer is lighter; farther draws the far slopes in detail \
                     instead of the coarse far terrain, at a cost.",
                )
                .now(format!("{meters} m"), Some((format!("{original} m"), meters == original)))
                .applies(details::APPLIES_GAME)
        };
        let slide = rows::slider(ui, row, distance_fraction(meters), &format!("{meters} m"), detail);
        let moved = match slide {
            Slide::Still => None,
            Slide::Step(steps) => {
                let next = i64::from(meters) + i64::from(steps) * i64::from(ViewDistance::STEP);
                ViewDistance::nearest(next.clamp(i64::from(ViewDistance::MIN), i64::from(ViewDistance::MAX)) as f64)
            }
            Slide::To(fraction) => distance_at(fraction),
        };
        if let Some(v) = moved {
            s.view_distance = v;
        }
        let original = ShadowQuality::default();
        let row = Row::new("Shadows").original(s.shadow_quality == original);
        let now = shadow_label(s.shadow_quality);
        let detail = || {
            Detail::new("Shadows")
                .text("The sun's shadows: their maps' resolution and how far they reach.")
                .now(now.clone(), Some((shadow_label(original), s.shadow_quality == original)))
                .applies(details::APPLIES_GAME)
        };
        let mut shadows = s.shadow_quality;
        rows::pick(ui, row, &mut shadows, &ShadowQuality::ALL, |q| shadow_label(*q), detail);
        s.shadow_quality = shadows;
        let bloom = s.bloom;
        rows::toggle(ui, Row::new("Bloom").original(bloom), &mut s.bloom, || {
            switch_facts(Detail::new("Bloom").text("The glow around bright snow and lights."), bloom, true)
                .applies(details::APPLIES_GAME)
        });
        let fxaa = s.fxaa;
        rows::toggle(ui, Row::new("FXAA").original(fxaa), &mut s.fxaa, || {
            switch_facts(Detail::new("FXAA").text("The original's anti-aliasing of the edges."), fxaa, true)
                .applies(details::APPLIES_GAME)
        });
        self.effects_rows(ui);
    }

    fn effects_rows(&mut self, ui: &mut Ui) {
        const APPLIES: &str = "The game and the tools start with these options. They are saved as you change them.";
        rows::group(ui, "Effects", Some("on is the original"));
        let s = &mut self.settings;
        let effects: [(&str, &mut bool, &str, Option<&str>); 5] = [
            (
                "Particles",
                &mut s.particles,
                "Snow spray, mist, falling snow and the other particle effects; off can be faster.",
                Some(
                    "Off keeps the effects that tell you something: tombstones, geotag pickups, the avalanche, the \
                     course markers' flares and smoke, and the clouds over deadly drops.",
                ),
            ),
            ("Snow tracks", &mut s.snow_tracks, "The trails riders cut into the snow; off can be faster.", None),
            (
                "Local lights",
                &mut s.local_lights,
                "The lights placed along the track; off can be faster.",
                Some("Off keeps the course markings and warnings projected onto the snow, and your head lamp."),
            ),
            (
                "Post-processing",
                &mut s.game_post,
                "The game's bloom, anti-aliasing (FXAA) and colour grading; off can be faster.",
                Some(
                    "Off keeps the screen warnings (low oxygen, cold, damage), the pulse goggles' grid and the game's \
                     exposure.",
                ),
            ),
            (
                "Sky",
                &mut s.game_sky,
                "The game's sky, clouds and distant mountains; off shows a flat colour instead.",
                Some("The fog stays, so whiteouts look as in the game."),
            ),
        ];
        for (label, on, what, keeps) in effects {
            let now = *on;
            rows::toggle(ui, Row::new(label).original(now), on, || {
                let mut detail = Detail::new(label).text(what);
                if let Some(keeps) = keeps {
                    detail = detail.text(keeps);
                }
                switch_facts(detail, now, true).applies(APPLIES)
            });
        }
    }

    pub fn audio_section(&mut self, ui: &mut Ui) {
        ui.spacing_mut().item_spacing.y = 0.0;
        let s = &mut self.settings;
        rows::group(ui, "Music", None);
        let original = Music::default();
        let now = s.music;
        let detail = || {
            Detail::new("Music")
                .text("Which soundtrack plays in the menus and on the mountain.")
                .now(music_label(now), Some((music_label(original).to_owned(), now == original)))
                .applies(details::APPLIES_NEXT_START)
        };
        let all = [Music::Auto, Music::DeadlyDescent, Music::Licensed, Music::Off];
        rows::pick(
            ui,
            Row::new("Music").original(now == original),
            &mut s.music,
            &all,
            |m| music_label(*m).into(),
            detail,
        );
        let row = Row::new("Music volume").enabled(s.music != Music::Off);
        let off = (s.music == Music::Off).then(|| "Not used while the music is off.".to_owned());
        let shown = percent(s.music_volume);
        let detail = || {
            Detail::new("Music volume")
                .text("Music loudness, 100 % is full volume.")
                .warn(off)
                .now(shown.clone(), None)
                .applies(details::APPLIES_NEXT_START)
        };
        let slide = rows::slider(ui, row, s.music_volume, &shown, detail);
        volume(&mut s.music_volume, slide);
        rows::group(ui, "Sound effects", None);
        let sfx = s.sfx;
        rows::toggle(ui, Row::new("Sound effects").original(sfx), &mut s.sfx, || {
            switch_facts(Detail::new("Sound effects").text("Board, rider and menu sounds."), sfx, true)
                .applies(details::APPLIES_NEXT_START)
        });
        let row = Row::new("Effects volume").enabled(s.sfx);
        let off = (!s.sfx).then(|| "Not used while the sound effects are off.".to_owned());
        let shown = percent(s.sfx_volume);
        let detail = || {
            Detail::new("Effects volume")
                .text("Sound effect loudness, 100 % is full volume.")
                .warn(off)
                .now(shown.clone(), None)
                .applies(details::APPLIES_NEXT_START)
        };
        let slide = rows::slider(ui, row, s.sfx_volume, &shown, detail);
        volume(&mut s.sfx_volume, slide);
    }
}

impl Launcher {
    pub fn game_section(&mut self, ui: &mut Ui) {
        ui.spacing_mut().item_spacing.y = 0.0;
        rows::group(ui, "Text and speech", None);
        const LANGUAGE: &str = "The game's text and speech language, from the databases on your disc. French and \
             German have their own spoken lines; the other languages use the English voice.";
        if self.languages.is_empty() {
            let detail = || {
                Detail::new("Language").text(LANGUAGE).text(
                    "The disc's text languages are read from the data folder; set up the game data to choose one.",
                )
            };
            rows::info(ui, Row::new("Language"), "Set up the game data first", theme::TEXT_DIM, detail);
        } else {
            let languages = self.languages.clone();
            let name = |code: &Option<String>| {
                let code = code.as_deref().unwrap_or("eng_us");
                languages.iter().find(|(c, _)| *c == code).map_or("English (default)", |(_, n)| *n).to_owned()
            };
            let now = name(&self.settings.language);
            let all: Vec<Option<String>> = languages.iter().map(|(code, _)| Some((*code).to_owned())).collect();
            let detail = || {
                Detail::new("Language")
                    .text(LANGUAGE)
                    .now(now.clone(), None)
                    .applies("Takes effect the next time the game starts.")
            };
            rows::pick(ui, Row::new("Language"), &mut self.settings.language, &all, name, detail);
        }
        let s = &mut self.settings;
        rows::group(ui, "Movies", None);
        let (now, original) = (s.range_movies, RangeMovies::default());
        let detail = || {
            Detail::new("Range movies")
                .text("The video of a mountain range, when you choose the range in World Tour.")
                .text(
                    "The original game plays a range's video every time you choose the range in World Tour. First \
                     time only plays it the first time in a profile; a new profile plays them again. The game starts \
                     with this option, and its Options (Movie Settings) change it too.",
                )
                .now(range_movies_label(now), Some((range_movies_label(original).to_owned(), now == original)))
                .applies(details::APPLIES_GAME)
        };
        let row = Row::new("Range movies").original(now == original);
        rows::pick(ui, row, &mut s.range_movies, &RangeMovies::ALL, |v| range_movies_label(*v).into(), detail);
        let (now, original) = (s.boot_movie, BootMovie::default());
        let detail = || {
            Detail::new("Boot movie")
                .text("The video the game starts with, before the start screen.")
                .text(
                    "The original game plays its boot video at every start. Skip starts the way the game restarts \
                     itself after a sign-out: straight to the start screen; the start screen and the sign-in stay as \
                     they are. The game's Options (Movie Settings) change it too.",
                )
                .now(boot_movie_label(now), Some((boot_movie_label(original).to_owned(), now == original)))
                .applies(details::APPLIES_GAME)
        };
        let row = Row::new("Boot movie").original(now == original);
        rows::pick(ui, row, &mut s.boot_movie, &BootMovie::ALL, |v| boot_movie_label(*v).into(), detail);
        rows::group(ui, "Menus", None);
        let (now, original) = (s.menu_focus, MenuFocus::default());
        let detail = || {
            Detail::new("Open menus")
                .text("Where the highlight is when you come back to a menu.")
                .text(
                    "The original game opens its main menu, pause menu and settings menu on their first item every \
                     time. With \"On the item picked last\" they, and Project Avalanche's own menus, open on the item \
                     you picked there last. The game starts with this option, and its Options (Menu Settings) change \
                     it too.",
                )
                .now(menu_focus_label(now), Some((menu_focus_label(original).to_owned(), now == original)))
                .applies(details::APPLIES_GAME)
        };
        let row = Row::new("Open menus").original(now == original);
        rows::pick(ui, row, &mut s.menu_focus, &MenuFocus::ALL, |v| menu_focus_label(*v).into(), detail);
    }

    pub fn controls_section(&self, ui: &mut Ui) {
        for (what, keys) in CONTROLS {
            widgets::heading(ui, what);
            ui.label(RichText::new(keys).color(theme::TEXT_SOFT));
            ui.add_space(8.0);
        }
        widgets::heading(ui, "Pads in this launcher");
        ui.label(RichText::new(LAUNCHER_PAD).color(theme::TEXT_SOFT));
        ui.add_space(4.0);
        let pads = self.pads.connected();
        if let Some(e) = self.pads.error() {
            widgets::status(ui, theme::WARN, format!("Pads cannot be read: {e}"));
        } else if pads.is_empty() {
            widgets::note(ui, "No pad is connected.");
        }
        for pad in pads {
            let glyphs = match pad.style {
                crate::ui::nav::PadStyle::Xbox => "Xbox buttons",
                crate::ui::nav::PadStyle::PlayStation => "PlayStation buttons",
            };
            let id = match (pad.vendor, pad.product) {
                (Some(v), Some(p)) => format!("  ·  {v:04x}:{p:04x}"),
                _ => String::new(),
            };
            widgets::status(ui, theme::GOOD, format!("{}  ·  {glyphs}{id}", pad.name));
        }
    }
}

const LAUNCHER_PAD: &str = "D-pad or left stick = move (held: repeats), A (cross) = select or change, B (circle) = \
     back, LB / RB (L1 / R1) = the section before / after; on the Play page A starts the game. The prompts at the \
     bottom follow the pad or the keys used last. A pad works here while this window has the focus.";

const CONTROLS: [(&str, &str); 3] = [
    ("Gamepad", "An Xbox-style pad with the game's own layout (recommended: the game was made for a pad)."),
    (
        "Keys in the menus",
        "Enter = START, Space = A, Backspace = B, X, Y, arrow keys = D-pad, Q / E = LB / RB, W A S D = left stick \
         (the globe), F1 skips a movie.",
    ),
    (
        "Keys on the mountain",
        "Arrow keys or W A S D = left stick (steer, tuck, brake), Numpad 8 / 4 / 6 / 2 = right stick (grabs, up = \
         jump, down = ready for the helicopter), Numpad 5 = right stick click (classic controls: oxygen), Space = A \
         (jump), I / J / L / K = Y / X / B / A, Shift = RT, Ctrl = LT, E = RB (standard controls: wingsuit and \
         oxygen), Backspace = LB (rewind), Esc = START (pause).",
    ),
];
