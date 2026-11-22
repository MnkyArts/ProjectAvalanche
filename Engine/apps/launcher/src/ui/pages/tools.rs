use std::time::SystemTime;

use eframe::egui::{self, Ui};

use crate::app::Launcher;
use crate::catalog;
use crate::command;
use crate::jobs::{BenchmarkRun, Program, newest_report};
use crate::settings::{Mode, Shading};
use crate::ui::details::{self, Detail};
use crate::ui::rows::{self, Field, Row, Slide};
use crate::ui::theme;
use crate::ui::widgets::{self, Kind};

const MAX_BOOST: f32 = 150.0;
const BOOST_STEP: f32 = 5.0;

const NOT_THE_GAME: &str = "Not the game: one track without the front end, to look at the data or to test the ride.";
const DEV_APPLIES: &str = "The game and the tools start with these options. They are saved as you change them.";

fn mode_label(m: Mode) -> &'static str {
    match m {
        Mode::Viewer => "Asset viewer (fly camera)",
        Mode::Ride => "Test ride (rider and physics)",
    }
}

fn shading_label(s: Shading) -> &'static str {
    match s {
        Shading::Auto => "Auto (textured when possible)",
        Shading::Textured => "Textured (the game's materials)",
        Shading::Height => "Height tint",
        Shading::Material => "Material debug palette",
    }
}

const BENCHMARK_TEXT: &str = "Plays fixed scenes of the game with your current settings — the front end, six drops \
     ridden by the game's test player and a camera flight, each from a fresh profile (yours is not touched) — and \
     writes a report of the frame times with your graphics card, processor and system. It takes about ten minutes; the \
     game's window opens for each scene. The report can be sent with a report about the frame rate.";

impl Launcher {
    pub fn tools_section(&mut self, ui: &mut Ui) {
        ui.spacing_mut().item_spacing.y = 0.0;
        rows::group(ui, "Viewer", Some("not the game"));
        if !self.data_ready || self.tracks.is_empty() {
            ui.spacing_mut().item_spacing.y = 6.0;
            widgets::status(ui, theme::WARN, "The tools need the game data: set it up under Content, Game data.");
            rows::buttons(ui, |ui| {
                if widgets::button(ui, "Open the setup", Kind::Plain, true).clicked() {
                    self.go_to_setup();
                }
            });
        } else {
            self.viewer_rows(ui);
        }
        ui.spacing_mut().item_spacing.y = 0.0;
        self.developer_rows(ui);
        if self.data_ready && !self.tracks.is_empty() {
            ui.spacing_mut().item_spacing.y = 6.0;
            self.benchmark_section(ui);
        }
    }

    fn viewer_rows(&mut self, ui: &mut Ui) {
        let s = &mut self.settings;
        let now = s.mode;
        let detail =
            || Detail::new("Run").text(NOT_THE_GAME).now(mode_label(now), None).applies(details::APPLIES_TOOLS);
        rows::pick(ui, Row::new("Run"), &mut s.mode, &[Mode::Viewer, Mode::Ride], |m| mode_label(*m).into(), detail);
        self.track_rows(ui);
        let s = &mut self.settings;
        match &self.riders {
            Ok(riders) if !riders.is_empty() => {
                let label =
                    |name: &String| riders.iter().find(|r| r.name == *name).map_or(name.clone(), |r| r.label.clone());
                let names: Vec<String> = riders.iter().map(|r| r.name.clone()).collect();
                let now = label(&s.character);
                let detail = || {
                    Detail::new("Rider").text("Who rides the test ride.").now(now, None).applies(details::APPLIES_TOOLS)
                };
                rows::pick(ui, Row::new("Rider"), &mut s.character, &names, label, detail);
            }
            Ok(_) => {
                let detail = || Detail::new("Rider").text("Who rides the test ride.").applies(details::APPLIES_TOOLS);
                let field = Field { chars: 64, password: false, hint: "" };
                rows::text_field(ui, Row::new("Rider"), &mut s.character, field, detail);
            }
            Err(e) => {
                let text = format!("riders unavailable: {e}");
                let detail = || Detail::new("Rider").warn(Some(text.clone()));
                rows::info(ui, Row::new("Rider"), &text, theme::ERROR, detail);
            }
        }
        let hud = s.hud;
        rows::toggle(ui, Row::new("Viewer text"), &mut s.hud, || {
            Detail::new("Viewer text")
                .text("HUD text of the viewer.")
                .now(if hud { "On" } else { "Off" }, None)
                .applies(details::APPLIES_TOOLS)
        });
        let ride = s.mode == Mode::Ride;
        let shown = format!("{:.0}", s.boost_meter);
        let unused = (!ride).then(|| "Not used by the asset viewer: the test ride only.".to_owned());
        let detail = || {
            Detail::new("Boost meter at the start")
                .text(
                    "Test option (--ride-boost-meter): the game starts with an empty meter and fills it from landed \
                     tricks.",
                )
                .warn(unused)
                .now(shown.clone(), None)
                .applies(details::APPLIES_TOOLS)
        };
        let slide = rows::slider(ui, Row::new("Boost meter").enabled(ride), s.boost_meter / MAX_BOOST, &shown, detail);
        s.boost_meter = match slide {
            Slide::Still => s.boost_meter,
            Slide::Step(steps) => ((s.boost_meter / BOOST_STEP).round() + steps as f32) * BOOST_STEP,
            Slide::To(fraction) => (fraction * MAX_BOOST).round(),
        }
        .clamp(0.0, MAX_BOOST);
        let what = if ride { "the test ride" } else { "the asset viewer" };
        ui.spacing_mut().item_spacing.y = 6.0;
        rows::buttons(ui, |ui| {
            let running = self.running();
            let launch = widgets::button(ui, "Launch", Kind::Primary, !running);
            details::offer(ui, &launch, || {
                Detail::new("Launch").text(format!("Starts {what} on the track chosen above.")).text(NOT_THE_GAME)
            });
            if launch.clicked() {
                let inv = command::viewer(&self.settings, &self.settings.big_root());
                self.start(ui.ctx(), &inv, Program::Viewer);
            }
        });
        self.started_line(ui);
    }

    fn developer_rows(&mut self, ui: &mut Ui) {
        let s = &mut self.settings;
        rows::group(ui, "Developer options", Some("the game uses these too"));
        let original = ssx_assets::subdiv::GAME_LEVELS as u32;
        let now = s.subdivide;
        let name = |n: &u32| if *n == original { format!("{n} (the game's)") } else { n.to_string() };
        let detail = || {
            Detail::new("Subdivision")
                .text("How finely the terrain is smoothed: higher is smoother but heavier, 0 is the raw shape.")
                .now(name(&now), Some((name(&original), now == original)))
                .applies(DEV_APPLIES)
        };
        let row = Row::new("Subdivision").original(now == original);
        rows::pick(ui, row, &mut s.subdivide, &[0, 1, 2, 3], name, detail);
        let (now, original) = (s.shading, Shading::default());
        let detail = || {
            Detail::new("Shading")
                .text("How the terrain surface is coloured; Height and Material are debug views, not the game's look.")
                .now(shading_label(now), Some((shading_label(original).to_owned(), now == original)))
                .applies(DEV_APPLIES)
        };
        let all = [Shading::Auto, Shading::Textured, Shading::Height, Shading::Material];
        let row = Row::new("Shading").original(now == original);
        rows::pick(ui, row, &mut s.shading, &all, |v| shading_label(*v).into(), detail);
        let wireframe = s.wireframe;
        rows::toggle(ui, Row::new("Wireframe").original(!wireframe), &mut s.wireframe, || {
            let d = Detail::new("Wireframe")
                .text("Draws the triangle mesh over everything, for debugging.")
                .text("A debug overlay of the triangle mesh. Press F in the game to toggle it.");
            super::options::switch_facts(d, wireframe, false).applies(DEV_APPLIES)
        });
    }

    // PORT(choice): the launcher runs the game's benchmark (`avalanche-game --benchmark`, the port's own tool) with
    // the options Play uses and shows the report it wrote (`<app dir>/Benchmarks`); the console has no benchmark.
    fn benchmark_section(&mut self, ui: &mut Ui) {
        rows::group(ui, "Benchmark", None);
        let about = || Detail::new("Benchmark").text(BENCHMARK_TEXT);
        rows::buttons(ui, |ui| {
            let running = self.running();
            let run = widgets::button(ui, "Run the benchmark", Kind::Primary, !running);
            details::offer(ui, &run, about);
            if run.clicked() {
                let inv = command::benchmark(&self.settings, &self.settings.big_root());
                self.benchmark = BenchmarkRun { started: Some(SystemTime::now()), ..BenchmarkRun::default() };
                self.start(ui.ctx(), &inv, Program::Benchmark);
            }
            let open = widgets::button(ui, "Open the reports folder", Kind::Plain, true);
            details::offer(ui, &open, about);
            if open.clicked()
                && let Err(e) = crate::mod_list::open_folder(&command::benchmark_dir())
            {
                self.launch_error = Some(e);
            }
        });
        let ended = self.started == Program::Benchmark && self.viewer.is_some() && !self.running();
        if ended && !self.benchmark.looked {
            self.benchmark.looked = true;
            self.benchmark.report = self.benchmark.started.and_then(|t| newest_report(&command::benchmark_dir(), t));
        }
        if let Some((path, text)) = &self.benchmark.report {
            ui.add_space(6.0);
            widgets::path(ui, path);
            egui::ScrollArea::both().id_salt("benchmark-report").max_height(260.0).show(ui, |ui| {
                ui.add(egui::Label::new(egui::RichText::new(text).monospace()).wrap_mode(egui::TextWrapMode::Extend));
            });
        }
    }

    pub fn started_line(&self, ui: &mut Ui) {
        let name = self.started.name();
        if let Some(e) = &self.launch_error {
            widgets::status(ui, theme::ERROR, e.clone());
        } else if let Some((pid, status)) = self.viewer.as_ref().map(|v| (v.pid, v.status())) {
            match status {
                None => widgets::status(ui, theme::CYAN_SOFT, format!("{name} running (process {pid}).")),
                Some(s) if s == "exited normally" => widgets::status(ui, theme::GOOD, format!("{name} {s}.")),
                Some(s) => widgets::status(ui, theme::ERROR, format!("{name} {s}. See Extras, Log.")),
            }
        } else {
            widgets::note(ui, "Starts in its own window; this one stays open.");
        }
    }

    fn track_rows(&mut self, ui: &mut Ui) {
        let big = self.settings.big_root();
        let facts = self.drops.get(&big, &self.settings.track).as_ref().ok().map(|drops| {
            let peak = drops.first().map_or("", |d| d.peak.as_str());
            format!("{peak}  ·  {} drop(s)", drops.len())
        });
        let now = self.settings.track.clone();
        let count = self.tracks.len();
        let detail = || {
            let mut d = Detail::new("Track").text(format!("The tracks of your game data ({count})."));
            if let Some(facts) = facts {
                d = d.fact("PEAK", facts);
            }
            d.now(now, None).applies(details::APPLIES_TOOLS)
        };
        let tracks = self.tracks.clone();
        if rows::pick(ui, Row::new("Track"), &mut self.settings.track, &tracks, |t| t.clone(), detail) {
            self.settings.drop = 0;
            self.settings.lighting = None;
        }
        let drops = match self.drops.get(&big, &self.settings.track) {
            Ok(d) => d.clone(),
            Err(e) => {
                let text = format!("drops unavailable: {e}");
                let detail = || Detail::new("Drop").warn(Some(text.clone()));
                rows::info(ui, Row::new("Drop"), &text, theme::ERROR, detail);
                return;
            }
        };
        let s = &mut self.settings;
        if s.drop > drops.len() {
            s.drop = 0;
        }
        if s.lighting.as_ref().is_some_and(|l| !drops.iter().any(|d| &d.lighting == l)) {
            s.lighting = None;
        }
        let name = |i: usize| drops.get(i).map_or("-".to_string(), |d| format!("{}  ·  {}", d.text_id, d.lighting));
        let drop_name = |n: &usize| match *n {
            0 => format!("Default: 1. {}", name(0)),
            n => format!("{n}. {}", name(n - 1)),
        };
        let unused =
            s.lighting.is_some().then(|| "A lighting preset chooses the drop: the first one it lights.".to_owned());
        let now = drop_name(&s.drop);
        let detail = || {
            Detail::new("Drop")
                .text("Which of the track's drops, with its lighting; the default is the first drop.")
                .warn(unused)
                .now(now, None)
                .applies(details::APPLIES_TOOLS)
        };
        let all: Vec<usize> = (0..=drops.len()).collect();
        rows::pick(ui, Row::new("Drop").enabled(s.lighting.is_none()), &mut s.drop, &all, drop_name, detail);
        let presets: Vec<Option<String>> =
            std::iter::once(None).chain(catalog::presets(&drops).into_iter().map(Some)).collect();
        let lighting_name = |l: &Option<String>| l.clone().unwrap_or_else(|| "The drop's own".into());
        let now = lighting_name(&s.lighting);
        let chosen = s.lighting.is_some();
        let detail = || {
            let d = Detail::new("Lighting").text("A lighting preset instead of the drop's own.");
            let d = if chosen { d.text("Uses the first drop lit by this preset.") } else { d };
            d.now(now, None).applies(details::APPLIES_TOOLS)
        };
        rows::pick(ui, Row::new("Lighting"), &mut s.lighting, &presets, lighting_name, detail);
    }
}
