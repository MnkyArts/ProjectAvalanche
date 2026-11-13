use bevy::prelude::*;
use ssx_ui::game::GameUi;
use ssx_ui::game::prompts::{Pad, chords};
use ssx_ui::game::tutorial::{TRICK_BUTTON_STRING, TRICK_HELP, TRICK_IS_ENABLED, TRICK_SUCCESS, TRICK_SUMMARY};
use ssx_ui::game::values::id;

use crate::ride::Ride;

const READ: f64 = 2.0;
const REACT: f64 = 0.6;
const TAP: f64 = 0.2;
const GAP: f64 = 0.07;
const RETRY: f64 = 3.0;
const LATE: f64 = 0.15;
const PUSH_EVERY: f64 = 1.2;
const PUSH_HOLD: f64 = 1.0;

fn key(p: Pad) -> Option<KeyCode> {
    Some(match p {
        Pad::A => KeyCode::KeyK,
        Pad::B => KeyCode::KeyL,
        Pad::X => KeyCode::KeyJ,
        Pad::Y => KeyCode::KeyI,
        Pad::RightTrigger => KeyCode::ShiftLeft,
        Pad::LeftTrigger => KeyCode::ControlLeft,
        Pad::RightBumper => KeyCode::KeyE,
        Pad::LeftBumper => KeyCode::Backspace,
        Pad::LeftStick(_, y) if y > 0 => KeyCode::ArrowUp,
        Pad::LeftStick(_, y) if y < 0 => KeyCode::ArrowDown,
        Pad::LeftStick(x, _) if x < 0 => KeyCode::ArrowLeft,
        Pad::LeftStick(..) => KeyCode::ArrowRight,
        Pad::RightStick(_, y) if y > 0 => KeyCode::Numpad8,
        Pad::RightStick(_, y) if y < 0 => KeyCode::Numpad2,
        Pad::RightStick(x, _) if x < 0 => KeyCode::Numpad4,
        Pad::RightStick(..) => KeyCode::Numpad6,
        Pad::Back | Pad::Start | Pad::LeftThumb | Pad::RightThumb | Pad::Dpad(..) => return None,
    })
}

fn trigger(p: Pad) -> bool {
    matches!(p, Pad::LeftTrigger | Pad::RightTrigger)
}

fn chord_keys(chord: &[Pad], t: f64) -> Vec<KeyCode> {
    let triggers_first = chord.contains(&Pad::LeftTrigger);
    let mixed = chord.iter().any(|&p| trigger(p)) && chord.iter().any(|&p| !trigger(p));
    chord.iter().filter(|&&p| !mixed || t >= LATE || trigger(p) == triggers_first).filter_map(|&p| key(p)).collect()
}

#[derive(Default)]
pub struct PromptPlayer {
    prompt: String,
    since: f64,
    lesson: String,
    passed: bool,
    success: bool,
    held: Vec<KeyCode>,
    taps: Vec<(KeyCode, f64)>,
    standing: Option<f64>,
    ride_clock: f64,
    popup: Option<String>,
    popup_since: f64,
    popup_step: u8,
    count: u32,
    shots: Vec<(String, f64)>,
}

impl PromptPlayer {
    fn tap(&mut self, keys: &mut ButtonInput<KeyCode>, k: KeyCode, clock: f64) {
        let hold = std::env::var("SSX_PROMPT_TAP").ok().and_then(|v| v.parse::<f64>().ok()).unwrap_or(0.1);
        keys.press(k);
        self.taps.push((k, clock + hold));
    }

    pub fn frame(&mut self, clock: f64, ui: &GameUi, keys: &mut ButtonInput<KeyCode>) -> Vec<String> {
        for (k, _) in self.taps.extract_if(.., |(_, until)| *until <= clock) {
            keys.release(k);
        }
        let popup = ui.fe.as_ref().filter(|f| f.tutorial_pause.shown && !f.tutorial_pause.hidden);
        let popup = popup.and_then(|f| f.tutorial_pause.current.as_ref());
        if let Some(p) = popup {
            if self.popup.as_deref() != Some(p.message.as_str()) {
                info!("prompts: popup {:?} (menu type {})", p.message, p.menu_type);
                (self.popup, self.popup_since, self.popup_step) = (Some(p.message.clone()), clock, 0);
                self.count += 1;
                self.shots.push((format!("prompt_{:02}_popup", self.count), clock + 1.0));
            }
            let t = clock - self.popup_since;
            match (p.menu_type, self.popup_step) {
                (1, 0) if t >= READ => {
                    self.tap(keys, KeyCode::ArrowDown, clock);
                    self.popup_step = 1;
                }
                (1, 1) if t >= READ + 0.5 => {
                    self.tap(keys, KeyCode::Space, clock);
                    self.popup_step = 2;
                }
                (_, 0) if t >= READ => {
                    self.tap(keys, KeyCode::Space, clock);
                    self.popup_step = 2;
                }
                (_, 2) if t >= READ + 2.5 => (self.popup_since, self.popup_step) = (clock, 0),
                _ => {}
            }
            for k in self.held.drain(..) {
                keys.release(k);
            }
        } else {
            self.popup = None;
        }
        self.shots.extract_if(.., |(_, at)| *at <= clock).map(|(name, _)| name).collect()
    }

    pub fn step(&mut self, ride_clock: f64, clock: f64, ui: &GameUi, ride: &Ride, keys: &mut ButtonInput<KeyCode>) {
        if ride_clock < self.ride_clock {
            (self.prompt, self.lesson, self.passed, self.standing) = (String::new(), String::new(), false, None);
        }
        self.ride_clock = ride_clock;
        let want = self.follow(ride_clock, clock, ui, ride);
        for k in self.held.extract_if(.., |k| !want.contains(k)) {
            keys.release(k);
        }
        for k in want {
            keys.press(k);
            if !self.held.contains(&k) {
                self.held.push(k);
            }
        }
    }

    pub fn release(&mut self, keys: &mut ButtonInput<KeyCode>) {
        for k in self.held.drain(..).chain(self.taps.drain(..).map(|(k, _)| k)) {
            keys.release(k);
        }
    }

    fn follow(&mut self, clock: f64, shot_clock: f64, ui: &GameUi, ride: &Ride) -> Vec<KeyCode> {
        let v = &ui.values;
        let s = &ride.state;
        let waiting = s.stopped || s.start.as_ref().is_some_and(|l| l.idle());
        let hud = v.bool(TRICK_IS_ENABLED);
        let button = v.list(TRICK_BUTTON_STRING).iter().find(|b| !chords(b).is_empty());
        let asked = button.cloned().unwrap_or_else(|| v.str(TRICK_HELP).to_string());
        let (prompt, lesson) = if hud && !asked.is_empty() {
            (asked, v.str(TRICK_SUMMARY).to_string())
        } else if waiting && v.bool(id::HUDMESSAGE_IS_ENABLED) {
            (v.str(id::HUDMESSAGE_MESSAGE).to_string(), String::new())
        } else {
            (String::new(), String::new())
        };
        let success = v.bool(TRICK_SUCCESS);
        if hud && lesson == self.lesson && success && !std::mem::replace(&mut self.success, success) && !self.passed {
            self.passed = true;
            info!("prompts: lesson {lesson} passed {:.2} s after its prompt", clock - self.since);
            self.shots.push((format!("prompt_{:02}_passed", self.count), shot_clock + 0.3));
        }
        self.success = success;
        if prompt != self.prompt || lesson != self.lesson {
            (self.prompt, self.lesson, self.since, self.passed) = (prompt, lesson, clock, false);
            if !self.prompt.is_empty() {
                self.count += 1;
                self.shots.push((format!("prompt_{:02}", self.count), shot_clock + REACT - 0.05));
                let help = if hud { v.str(TRICK_HELP) } else { "" };
                let unknown: Vec<&str> = ssx_ui::game::prompts::tokens(&self.prompt)
                    .into_iter()
                    .filter(|t| *t != "NEXT" && ssx_ui::game::prompts::pad(t).and_then(key).is_none())
                    .collect();
                info!(
                    "prompts: at ride time {:.2} s the screen asks {:?} (lesson {:?}, help {help:?}, objective {:?}) → \
                     {:?}{}",
                    ride.seconds(),
                    self.prompt,
                    self.lesson,
                    v.str(ssx_ui::game::tutorial::TRICK_OBJECTIVE),
                    self.steps(),
                    if unknown.is_empty() { String::new() } else { format!("; no key for {unknown:?}") }
                );
            }
        }
        if self.prompt.is_empty() || self.passed {
            self.standing = Self::rests(self.standing, clock, s);
            return match self.standing {
                Some(since) if clock - since >= REACT && (clock - since - REACT) % PUSH_EVERY < PUSH_HOLD => {
                    vec![KeyCode::ArrowUp]
                }
                _ => Vec::new(),
            };
        }
        self.standing = None;
        let steps = self.steps();
        let t = clock - self.since - REACT;
        if t < 0.0 {
            return Vec::new();
        }
        match steps.as_slice() {
            [] => Vec::new(),
            [one] => chord_keys(one, t),
            [first, second, ..] => match t % RETRY {
                p if p < TAP => chord_keys(first, p),
                p if p < TAP + GAP => Vec::new(),
                p => chord_keys(second, p - TAP - GAP),
            },
        }
    }

    fn rests(since: Option<f64>, clock: f64, s: &ssx_physics::rider::RiderState) -> Option<f64> {
        let ground = s.start.is_none() && s.state == ssx_physics::rider::RideState::Riding;
        let kmh = s.speed() * ssx_physics::KMH_PER_M_PER_TICK;
        match since {
            Some(t) if ground && kmh < 15.0 => Some(t),
            None if ground && (s.stopped || kmh < 3.0) => Some(clock),
            _ => None,
        }
    }

    fn steps(&self) -> Vec<Vec<Pad>> {
        let mut steps = chords(&self.prompt);
        for c in &mut steps {
            if c.contains(&Pad::LeftTrigger) && c.contains(&Pad::RightTrigger) && c.iter().all(|&p| trigger(p)) {
                c.push(Pad::Y);
            }
        }
        steps
    }
}
