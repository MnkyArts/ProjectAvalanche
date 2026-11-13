use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use super::super::GameUi;
use super::super::input::{Button, Input, SCRIPT_DEVICE};
use super::super::ride::{RideDriver, RideOptions};

#[derive(Clone, Debug, PartialEq)]
pub enum Step {
    Press(Button),
    SkipMovie,
    Shot(String),
    Nav(String),
    Stick(f32, f32),
    TutorialPopup(String),
    Pad(bool),
    Focus(bool),
    Text(String),
    TextKey(super::ava::keyboard::TextInput),
    Chat(bool),
    PointerMove(f32, f32),
    WindowMove(f32, f32),
    Click(crate::game::pointer::MouseButton),
    Wheel(i32),
    Display(crate::game::display_class::DisplayClass),
}

const MAX_POINTER_COORD: f32 = 16_384.0;

fn pointer_point(x: &str, y: &str) -> Result<(f32, f32)> {
    let p = |v: &str| -> Result<f32> {
        let n: f32 = v.trim().parse().with_context(|| format!("pointer coordinate {v:?}"))?;
        anyhow::ensure!(n.is_finite() && n.abs() <= MAX_POINTER_COORD, "pointer coordinate {v:?} out of range");
        Ok(n)
    };
    Ok((p(x)?, p(y)?))
}

pub fn parse_script(s: &str) -> Result<Vec<(f64, Step)>> {
    let mut out = Vec::new();
    let mut pieces = s.split(',').map(str::trim).filter(|x| !x.is_empty());
    while let Some(item) = pieces.next() {
        let (t, what) = item.split_once(':').with_context(|| format!("script item {item:?}: <seconds>:<step>"))?;
        if let Some(x) = what.strip_prefix("move:").or_else(|| what.strip_prefix("wmove:")) {
            let t: f64 = t.parse().with_context(|| format!("script time {t:?}"))?;
            let y = pieces.next().with_context(|| format!("script step {what:?}: <x>,<y>"))?;
            let (x, y) = pointer_point(x, y)?;
            out.push((t, if what.starts_with('w') { Step::WindowMove(x, y) } else { Step::PointerMove(x, y) }));
            continue;
        }
        let t: f64 = t.parse().with_context(|| format!("script time {t:?}"))?;
        let step = match what {
            "start" => Step::Press(Button::Start),
            "a" => Step::Press(Button::DigitalOne),
            "b" => Step::Press(Button::DigitalTwo),
            "x" => Step::Press(Button::DigitalFour),
            "y" => Step::Press(Button::DigitalThree),
            "up" => Step::Press(Button::DpadUp),
            "down" => Step::Press(Button::DpadDown),
            "left" => Step::Press(Button::DpadLeft),
            "right" => Step::Press(Button::DpadRight),
            "lb" => Step::Press(Button::LeftBumper),
            "rb" => Step::Press(Button::RightBumper),
            "lt" => Step::Press(Button::LeftTrigger),
            "rt" => Step::Press(Button::RightTrigger),
            "back" => Step::Press(Button::Select),
            "rs" => Step::Press(Button::Ras),
            "skip" => Step::SkipMovie,
            "pad_off" => Step::Pad(false),
            "pad_on" => Step::Pad(true),
            "focus_off" => Step::Focus(false),
            "focus_on" => Step::Focus(true),
            "chat" => Step::Chat(false),
            "quickchat" => Step::Chat(true),
            "click" => Step::Click(crate::game::pointer::MouseButton::Left),
            "rclick" => Step::Click(crate::game::pointer::MouseButton::Right),
            w if w.starts_with("wheel:") => {
                let n: i32 = w[6..].parse().with_context(|| format!("script step {w:?}: wheel:<notches>"))?;
                anyhow::ensure!((-64..=64).contains(&n), "script step {w:?}: at most 64 notches");
                Step::Wheel(n)
            }
            w if w.starts_with("shot=") => Step::Shot(w[5..].to_string()),
            w if w.starts_with("nav=") => Step::Nav(w[4..].to_string()),
            w if w.starts_with("popup=") => Step::TutorialPopup(w[6..].to_string()),
            w if w.starts_with("text=") => Step::Text(w[5..].to_string()),
            w if w.starts_with("key=") => {
                use super::ava::keyboard::TextInput as T;
                Step::TextKey(match &w[4..] {
                    "backspace" => T::Backspace,
                    "delete" => T::Delete,
                    "left" => T::Left,
                    "right" => T::Right,
                    "home" => T::Home,
                    "end" => T::End,
                    "enter" => T::Enter,
                    "escape" => T::Escape,
                    k => anyhow::bail!("script step key={k}: backspace, delete, left, right, home, end, enter, escape"),
                })
            }
            w if w.starts_with("display=") => Step::Display(crate::game::display_class::DisplayClass {
                widescreen: match &w[8..] {
                    "wide" => true,
                    "4x3" => false,
                    d => anyhow::bail!("script step display={d}: 4x3 or wide"),
                },
            }),
            w if w.starts_with("stick=") => {
                let (x, y) = w[6..].split_once('/').with_context(|| format!("script step {w:?}: stick=<x>/<y>"))?;
                Step::Stick(x.parse()?, y.parse()?)
            }
            w => anyhow::bail!("script step {w:?}"),
        };
        out.push((t, step));
    }
    out.sort_by(|a, b| a.0.total_cmp(&b.0));
    Ok(out)
}

pub fn script_interrupt(ui: &mut GameUi, step: &Step) {
    match *step {
        Step::Pad(on) => ui.fe_call(|fe, env| super::pads::connection(fe, env, usize::from(SCRIPT_DEVICE), on)),
        Step::Focus(on) => ui.fe_call(|fe, env| super::pads::system_ui(fe, env, !on)),
        _ => {}
    }
}

pub fn demo_profile(big_root: &Path, which: &str) -> Result<ssx_assets::career::Profile> {
    use ssx_assets::career::profile::EventOutcome;
    let career = ssx_assets::career::CareerDb::load(big_root)?;
    let rules = career.rules();
    let mut p = ssx_assets::career::Profile::new();
    match which {
        "new" => {}
        "tutorials" => {
            let _ = ssx_assets::career::new_game::run_new_game(&mut p, &rules, |p, d| {
                let _ = p.apply_result(&rules, &EventOutcome { event: d, ..Default::default() });
            });
            let gen1 = ssx_formats::attrib::fnv1(b"chapter_2_drop_gen_1");
            let _ = p.apply_result(&rules, &EventOutcome { event: gen1, ..Default::default() });
        }
        w => anyhow::bail!("profile {w:?}: new or tutorials"),
    }
    Ok(p)
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Phase {
    Menus,
    Loading(f64),
    Riding,
    Results(f64),
}

pub struct Host {
    pub big_root: PathBuf,
    pub script: Vec<(f64, Step)>,
    pub time: f64,
    pub phase: Phase,
    pub ride: Option<RideDriver>,
    pub steps_per_frame: usize,
    pub loading_seconds: f64,
    reported: bool,
    pub external_movies: bool,
    pub external_pause: bool,
    pub movie_cap: f64,
    movie_left: Option<(String, f64, f64)>,
    movie_played: f64,
    pub viewer_movie_frame: Option<i32>,
    release: Vec<(Button, u8)>,
    pointer_release: Vec<crate::game::pointer::MouseButton>,
    pub window_moves: Vec<(f32, f32)>,
    pub displays: Vec<crate::game::display_class::DisplayClass>,
    pub script_pointer: bool,
    pub script_pad: bool,
    pub paused: bool,
    pub log: Vec<String>,
    pub ride_win: Option<f32>,
    ride_rows: Vec<u32>,
    pub exit_asked: bool,
}

pub fn ghost_values(big_root: &Path, drop: u32) -> Vec<u32> {
    let text = ssx_assets::content::data::events::ghost_db_text(big_root).unwrap_or_default();
    ssx_physics::event::parse_ghost_db(&text)
        .into_iter()
        .filter(|(id, _)| *id == drop)
        .map(|(_, r)| r.value())
        .collect()
}

pub fn forced_win(event: &ssx_physics::event::EventManager, rows: &[u32]) -> ssx_physics::event::EventResult {
    let t = event.setup.event_type;
    let clock_ms = event.clock.ms();
    let race = t.is_some_and(|t| t.is_race());
    let trick = t.is_some_and(|t| t.is_trick());
    let beaten = || rows.iter().chain(&event.setup.rivals).copied().filter(|&v| v != u32::MAX);
    let value = if race {
        beaten().min().map_or(clock_ms, |m| m.saturating_sub(1)).max(1)
    } else {
        beaten().max().map_or(1, |m| m.saturating_add(1))
    };
    let s = event.setup.rank(value, false, Vec::new());
    ssx_physics::event::EventResult {
        event_type: t,
        points: if trick { value } else { 0 },
        time_ms: if race { value } else { clock_ms },
        descent_mm: if race || trick { 0 } else { value },
        rewinds: 0,
        loops: 0,
        failed: false,
        value,
        rivals: s.rivals,
        rank: s.rank,
        medal: s.medal,
        place: s.place,
        geotag_credits: 0,
        geotag_count: 0,
    }
}

pub fn movie_info(big_root: &Path, name: &str) -> Option<(u32, f64)> {
    let id = ssx_assets::content::fmv::movie_id(name)?;
    let fmv = ssx_assets::content::GameContent::of(big_root).fmv(&id)?;
    match &fmv {
        ssx_assets::content::fmv::Fmv::Disc(path) => ssx_assets::content::fmv::disc_info(path),
        _ => {
            let (seconds, fps) = (fmv.seconds()?, fmv.fps()?);
            Some(((seconds * fps).round() as u32, fps))
        }
    }
}

pub fn movie_seconds(big_root: &Path, name: &str) -> Option<f64> {
    movie_info(big_root, name).map(|(frames, fps)| f64::from(frames) / fps)
}

impl Host {
    pub fn new(big_root: &Path, script: Vec<(f64, Step)>) -> Self {
        Host {
            big_root: big_root.to_path_buf(),
            script,
            time: 0.0,
            phase: Phase::Menus,
            ride: None,
            steps_per_frame: 1,
            loading_seconds: 2.0,
            reported: false,
            external_movies: false,
            external_pause: false,
            movie_cap: f64::INFINITY,
            movie_left: None,
            movie_played: 0.0,
            viewer_movie_frame: None,
            release: Vec::new(),
            pointer_release: Vec::new(),
            window_moves: Vec::new(),
            displays: Vec::new(),
            script_pointer: false,
            script_pad: false,
            paused: false,
            log: Vec::new(),
            ride_win: None,
            ride_rows: Vec::new(),
            exit_asked: false,
        }
    }

    fn note(&mut self, what: String) {
        self.log.push(format!("[{:7.2}] host: {what}", self.time));
    }

    pub fn finish_movie(&mut self, ui: &mut GameUi) {
        ui.fe_call(|fe, env| fe.movie_finished(env));
        let held = ui.fe.as_ref().and_then(|f| f.movie.as_ref().map(|m| m.name.as_str()));
        match &mut self.movie_left {
            Some((n, left, _)) if held == Some(n.as_str()) => *left = f64::NEG_INFINITY,
            _ => self.movie_left = None,
        }
    }

    pub fn frame(&mut self, ui: &mut GameUi, dt: f64) -> Vec<String> {
        self.time += dt;
        let mut shots = Vec::new();
        for (b, device) in std::mem::take(&mut self.release) {
            ui.input.push_from(Input::Button { button: b, down: false }, device);
        }
        for b in std::mem::take(&mut self.pointer_release) {
            ui.pointer_button(b, false);
        }
        while self.script.first().is_some_and(|(t, _)| *t <= self.time) {
            let (_, step) = self.script.remove(0);
            match step {
                Step::Press(b) => {
                    self.note(format!("press {b:?}"));
                    self.press(ui, b);
                    self.script_pad = true;
                }
                Step::SkipMovie => {
                    if ui.fe.as_ref().is_some_and(|f| f.movie.is_some()) {
                        self.note("skip the movie".into());
                        self.finish_movie(ui);
                    }
                }
                Step::Shot(n) => shots.push(n),
                Step::Nav(e) => {
                    self.note(format!("nav event {e} (script)"));
                    ui.nav.raise(&e);
                }
                Step::Stick(x, y) => {
                    self.note(format!("stick ({x}, {y})"));
                    ui.fe_call(|fe, _| super::globe::stick(fe, x, y));
                }
                Step::TutorialPopup(text) => {
                    self.note(format!("DO_TutorialPopup({text}) (script)"));
                    ui.tutorial_popup(&text);
                }
                step @ (Step::Pad(_) | Step::Focus(_)) => {
                    self.note(format!("{step:?} (script)"));
                    script_interrupt(ui, &step);
                }
                Step::Text(t) => {
                    self.note(format!("type {t:?} (script)"));
                    for c in t.chars() {
                        ui.text_input(super::ava::keyboard::TextInput::Char(c));
                    }
                }
                Step::TextKey(k) => {
                    self.note(format!("key {k:?} (script)"));
                    ui.text_input(k);
                }
                Step::Chat(quick) => self.note(format!("chat key (quick {quick}) outside a level: nothing (script)")),
                Step::PointerMove(x, y) => {
                    self.note(format!("pointer at ({x}, {y}) (script)"));
                    ui.pointer_move(x, y);
                    self.script_pointer = true;
                }
                Step::WindowMove(x, y) => {
                    self.note(format!("pointer at window ({x}, {y}) (script)"));
                    self.window_moves.push((x, y));
                    self.script_pointer = true;
                }
                Step::Click(b) => {
                    self.note(format!("pointer {b:?} click (script)"));
                    ui.pointer_button(b, true);
                    self.pointer_release.push(b);
                    self.script_pointer = true;
                }
                Step::Wheel(n) => {
                    self.note(format!("pointer wheel {n} (script)"));
                    ui.pointer_wheel(n);
                    self.script_pointer = true;
                }
                Step::Display(d) => {
                    self.note(format!("display {} (script)", if d.widescreen { "widescreen" } else { "4:3" }));
                    self.displays.push(d);
                }
            }
        }
        self.movies(ui, dt);
        self.drop(ui);
        shots
    }

    pub fn movie_frame(&self) -> i32 {
        let own = self.movie_left.as_ref().map_or(0, |(_, _, fps)| (self.movie_played * fps) as i32);
        self.viewer_movie_frame.unwrap_or(own)
    }

    pub fn press(&mut self, ui: &mut GameUi, b: Button) {
        self.press_from(ui, b, SCRIPT_DEVICE);
    }

    pub fn press_from(&mut self, ui: &mut GameUi, b: Button, device: u8) {
        if ui.fe.as_ref().is_some_and(super::FrontEnd::movie_queued) {
            if matches!(b, Button::DigitalOne | Button::Start) {
                let frame = self.movie_frame();
                self.note(format!("press {b:?} under a movie (frame {frame}): the movie manager's skip"));
                ui.fe_call(|fe, _| fe.skip_movie(frame));
            }
        } else {
            ui.input.push_from(Input::Button { button: b, down: true }, device);
            self.release.push((b, device));
            if b == Button::Start && self.phase == Phase::Riding {
                ui.game_input(true);
            }
            if matches!(b, Button::DpadLeft | Button::DpadRight) {
                let x = if b == Button::DpadRight { 1.0 } else { -1.0 };
                ui.fe_call(|fe, _| super::globe::dpad(fe, x));
            }
        }
    }

    fn movies(&mut self, ui: &mut GameUi, dt: f64) {
        let current = ui.fe.as_ref().and_then(|f| f.movie.as_ref().map(|m| m.name.clone()));
        let stopped = ui.fe.as_ref().is_some_and(|f| f.movie_stop);
        match (&mut self.movie_left, current) {
            (_, None) => self.movie_left = None,
            (Some((n, left, _)), Some(m)) if *n == m => {
                if !self.external_movies {
                    *left -= dt;
                    self.movie_played += dt;
                    if *left <= 0.0 || stopped {
                        if left.is_finite() {
                            self.note(format!("movie {m} {}", if stopped { "stopped" } else { "played" }));
                        }
                        self.finish_movie(ui);
                    }
                }
            }
            (_, Some(m)) => {
                let info = movie_info(&self.big_root, &m);
                let len = info.map_or(0.0, |(frames, fps)| f64::from(frames) / fps).min(self.movie_cap);
                self.note(format!("movie {m} ({len:.1} s)"));
                self.movie_left = Some((m, len, info.map_or(0.0, |i| i.1)));
                self.movie_played = 0.0;
            }
        }
    }

    fn drop(&mut self, ui: &mut GameUi) {
        use super::pause::GameRequest;
        if let Some(paused) = ui.fe.as_mut().filter(|_| !self.external_pause).and_then(|fe| fe.pause_step()) {
            self.note(format!("the game {}", if paused { "pauses" } else { "runs again" }));
            self.paused = paused;
        }
        if self.phase == Phase::Riding {
            ui.game_input(false);
        }
        for r in ui.take_requests() {
            self.note(format!("game request {r:?}"));
            match r {
                GameRequest::Restart => {
                    self.paused = false;
                    let launch = ui.fe.as_ref().and_then(|f| f.launch.clone());
                    if let Some(l) = launch {
                        match self.load_ride(&l) {
                            Ok(d) => {
                                self.note("ride restarted".into());
                                self.ride = Some(d);
                            }
                            Err(e) => self.note(format!("the ride did not restart: {e:#}")),
                        }
                        self.phase = Phase::Loading(self.time);
                        self.reported = false;
                    }
                }
                GameRequest::SaveSettings | GameRequest::Save => {}
                GameRequest::ExitGame => {
                    self.note("Exit: the session ends".into());
                    self.exit_asked = true;
                }
                GameRequest::Quit => {
                    self.paused = false;
                    if self.phase == Phase::Riding || matches!(self.phase, Phase::Loading(_)) {
                        self.note("left the drop".into());
                        self.ride = None;
                        self.phase = Phase::Menus;
                        ui.fe_call(super::flow::front_end_entered);
                    }
                }
            }
        }
        let launched = ui.fe.as_ref().and_then(|f| f.launch.clone());
        match self.phase {
            Phase::Menus => {
                if let Some(l) = launched {
                    self.phase = Phase::Loading(self.time);
                    self.reported = false;
                    match self.load_ride(&l) {
                        Ok(d) => {
                            let nis = crate::game::feed::nis_is_active(&d.rider, &d.session);
                            ui.set_drop(d.drop_key, d.event_type, l.free_ride, nis);
                            self.ride_rows = ghost_values(&self.big_root, d.drop_key);
                            self.note(format!(
                                "ride loaded: {} drop {} event {:?} (type {}), rider {}",
                                d.opts.track, d.opts.drop, d.event_name, d.event_type, d.opts.character
                            ));
                            self.ride = Some(d);
                        }
                        Err(e) => self.note(format!("the ride did not load: {e:#}")),
                    }
                }
            }
            Phase::Loading(since) => {
                // PORT(choice): a test harness's stand-in — the headless host loads no world (no world to stream here);
                // the load and the streaming report take `loading_seconds`; the game always loads its level.
                if !self.reported && self.time - since >= self.loading_seconds {
                    self.reported = true;
                    self.note("level loaded: the camera's streaming report (0xDAE83AFD {0})".into());
                    ui.fe_call(|fe, _| super::flow::simulation_begins(fe));
                    ui.fe_call(super::flow::stream_ready);
                }
                if self.reported && ui.fe.as_ref().is_some_and(|f| f.screen == super::screens::id::HUD) {
                    self.note("on the HUD (A_GameGameplayStarted → GoToScreen(0x37)): the ride runs".into());
                    self.phase = Phase::Riding;
                }
            }
            Phase::Riding if ui.fe.as_ref().is_some_and(|f| f.level_start.query) => {
                self.note("the survive loop's streaming report (0xDAE83AFD {0})".into());
                ui.fe_call(super::flow::stream_ready);
            }
            Phase::Riding if !self.paused => {
                let Some(d) = self.ride.as_mut() else { return };
                for _ in 0..self.steps_per_frame {
                    let _ = d.step(ui);
                    let popup =
                        ui.fe.as_ref().is_some_and(|f| f.game_pause.asked(super::pause::reason::TUTORIAL_POPUP));
                    if d.results.is_some() || popup {
                        break;
                    }
                }
                if d.results.is_none() && self.ride_win.is_some_and(|s| d.time() >= s) {
                    let r = forced_win(&d.session.event, &self.ride_rows);
                    let t = d.time();
                    if let Some(mut profile) =
                        ui.fe.as_mut().map(|f| std::mem::replace(&mut f.profile, ssx_assets::career::Profile::new()))
                    {
                        use ssx_assets::career::profile::DropState;
                        let drop = ui.fe.as_ref().map_or(0, |f| f.current_drop);
                        let before = profile.drop_state(drop);
                        let success = d.post_results_with(ui, &r, &mut profile);
                        if before == DropState::Unlocked && profile.drop_state(drop) == DropState::Completed {
                            super::flow::campaign_drop_completed(&mut ui.values, drop);
                        }
                        if let Some(f) = ui.fe.as_mut() {
                            f.profile = profile;
                            f.last_success = Some(success);
                            f.last_result_drop = drop;
                        }
                    }
                    d.results = Some((t, r));
                    self.log
                        .push(format!("[{:7.2}] host: --ride-win at {t:.2} s ride time (a test option)", self.time));
                }
                if let Some((t, r)) = d.results.clone() {
                    let death = r.failed;
                    let line = format!(
                        "results at {t:.2} s ride time: {} ms, {} points, place {:?}, failed {}",
                        r.time_ms,
                        r.points,
                        r.rank.map(|k| k + 1),
                        r.failed
                    );
                    self.note(line);
                    ui.fe_call(|fe, env| super::flow::results_posted(fe, env, death));
                    self.phase = Phase::Results(self.time);
                }
            }
            Phase::Results(_) if ui.fe.as_ref().is_some_and(|f| !f.in_drop) => {
                self.note("back in the front end".into());
                self.ride = None;
                self.phase = Phase::Menus;
            }
            _ => {}
        }
    }

    fn load_ride(&self, l: &super::Launch) -> Result<RideDriver> {
        let starts = ssx_physics::spawn::load_drop_starts(&self.big_root, &l.track)?;
        let career = ssx_assets::career::CareerDb::load(&self.big_root)?;
        let ge_drop = career.event(l.drop).map(|e| e.drop).unwrap_or(0);
        let drop = starts.iter().position(|s| s.drop_key == ge_drop).unwrap_or(0);
        let opts = RideOptions {
            track: l.track.clone(),
            drop,
            event: l.event.clone(),
            character: l.rider.clone(),
            intro: true,
            setup: Some(super::super::ride::game_setup(&l.setup)),
            ..Default::default()
        };
        RideDriver::load(&self.big_root, opts)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn script() {
        let s = parse_script("10:a, 7:start,12:shot=fe_menu,13:skip,14:nav=go_to_menu").unwrap();
        assert_eq!(s.len(), 5);
        assert_eq!(s[0], (7.0, Step::Press(Button::Start)));
        assert_eq!(s[1], (10.0, Step::Press(Button::DigitalOne)));
        assert_eq!(s[2], (12.0, Step::Shot("fe_menu".into())));
        assert_eq!(s[3], (13.0, Step::SkipMovie));
        assert_eq!(s[4], (14.0, Step::Nav("go_to_menu".into())));
        assert!(parse_script("7:jump").is_err());
        assert!(parse_script("start").is_err());
        assert!(parse_script("").unwrap().is_empty());
    }

    #[test]
    fn pointer_steps() {
        use crate::game::pointer::MouseButton;
        let s = parse_script("6.5:move:1000,335.5,7:click,7.5:rclick,8:wheel:-3,9:wmove:-20,16384").unwrap();
        assert_eq!(s[0], (6.5, Step::PointerMove(1000.0, 335.5)));
        assert_eq!(s[1], (7.0, Step::Click(MouseButton::Left)));
        assert_eq!(s[2], (7.5, Step::Click(MouseButton::Right)));
        assert_eq!(s[3], (8.0, Step::Wheel(-3)));
        assert_eq!(s[4], (9.0, Step::WindowMove(-20.0, 16_384.0)));
        for bad in [
            "1:move:10",
            "1:move:10,",
            "1:move:NaN,5",
            "1:move:inf,5",
            "1:move:16385,5",
            "1:wmove:5,-16385",
            "1:move:x,5",
            "1:wheel:65",
            "1:wheel:-65",
            "1:wheel:two",
            "1:wheel:",
        ] {
            assert!(parse_script(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn forced_win_beats_the_rows() {
        use ssx_physics::event::{EventManager, EventSetup, EventType, Medal, ResultsFlow};
        let rows = [146_034, 151_000, 120_500];
        let tour = |t| EventSetup { event_type: Some(t), results: Some(ResultsFlow::WorldTour), ..Default::default() };
        let race = EventManager::new(tour(EventType::RaceBots));
        let r = forced_win(&race, &rows);
        assert_eq!((r.value, r.time_ms, r.rank, r.place, r.failed), (120_499, 120_499, Some(0), 0, false));
        assert_eq!((r.rivals.len(), r.medal), (0, None));
        let explore = EventManager::new(EventSetup {
            event_type: Some(EventType::RaceGhosts),
            rivals: vec![77_532, 92_631, 120_965],
            results: Some(ResultsFlow::Explore),
            ..Default::default()
        });
        let e = forced_win(&explore, &rows);
        assert_eq!((e.value, e.rank, e.medal), (77_531, Some(0), Some(Medal::Gold)));
        let trick = EventManager::new(tour(EventType::TrickBots));
        let t = forced_win(&trick, &rows);
        assert_eq!((t.value, t.points, t.rank), (151_001, 151_001, Some(0)));
        let rule = ssx_assets::career::profile::EventRule {
            key: 0,
            name: String::new(),
            drop_type: ssx_assets::career::data::DropType::Campaign,
            event_type: 1,
            reward: 0,
            places: Some(1),
            goal: None,
            cost: 0,
            peak: 0,
            range: 0,
        };
        let o = ssx_assets::career::profile::EventOutcome {
            value: t.value,
            bot_scores: rows.to_vec(),
            ..Default::default()
        };
        assert!(ssx_assets::career::profile::campaign_success(&rule, &o));
        let survive =
            EventManager::new(EventSetup { event_type: Some(EventType::SurviveToBottom), ..Default::default() });
        let v = forced_win(&survive, &[]);
        assert_eq!((v.value, v.rank, v.medal, v.failed), (1, None, None, false));
    }
}
