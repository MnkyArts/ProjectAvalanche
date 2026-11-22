use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use eframe::egui::{self, Key};

use crate::ui::nav::{Dir, PadStyle};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PadButton {
    Up,
    Down,
    Left,
    Right,
    A,
    B,
    Lb,
    Rb,
}

impl PadButton {
    pub const NAMES: [(&'static str, PadButton); 8] = [
        ("up", PadButton::Up),
        ("down", PadButton::Down),
        ("left", PadButton::Left),
        ("right", PadButton::Right),
        ("a", PadButton::A),
        ("b", PadButton::B),
        ("lb", PadButton::Lb),
        ("rb", PadButton::Rb),
    ];

    pub fn parse(name: &str) -> Option<PadButton> {
        Self::NAMES.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)).map(|(_, b)| *b)
    }

    // PORT(choice): a pad in the launcher enters as these keys (Microsoft's gamepad-to-keyboard mapping for XY focus
    // navigation; LB / RB for tabs), so pad and keyboard share every move; the console had no launcher.
    pub fn key(self) -> Key {
        match self {
            PadButton::Up => Key::ArrowUp,
            PadButton::Down => Key::ArrowDown,
            PadButton::Left => Key::ArrowLeft,
            PadButton::Right => Key::ArrowRight,
            PadButton::A => Key::Enter,
            PadButton::B => Key::Escape,
            PadButton::Lb => Key::Q,
            PadButton::Rb => Key::E,
        }
    }

    fn repeats(self) -> bool {
        Dir::of(self.key()).is_some()
    }

    fn of_dir(dir: Dir) -> PadButton {
        match dir {
            Dir::Up => PadButton::Up,
            Dir::Down => PadButton::Down,
            Dir::Left => PadButton::Left,
            Dir::Right => PadButton::Right,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PadEvent {
    pub button: PadButton,
    pub pressed: bool,
    pub style: PadStyle,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PadInfo {
    pub name: String,
    pub style: PadStyle,
    pub vendor: Option<u16>,
    pub product: Option<u16>,
}

const SONY: u16 = 0x054C;

pub fn style_of(vendor: Option<u16>, name: &str) -> PadStyle {
    let name = name.to_ascii_lowercase();
    let named = ["playstation", "dualsense", "dualshock"].iter().any(|n| name.contains(n));
    if vendor == Some(SONY) || named { PadStyle::PlayStation } else { PadStyle::Xbox }
}

pub const STICK: f32 = 0.4;

pub fn stick_dir(held: Option<Dir>, x: f32, y: f32) -> Option<Dir> {
    let past = |v: f32| v.abs() > STICK;
    let vertical = |y: f32| if y > 0.0 { Dir::Up } else { Dir::Down };
    let horizontal = |x: f32| if x > 0.0 { Dir::Right } else { Dir::Left };
    match held {
        Some(Dir::Up | Dir::Down) if past(y) => return Some(vertical(y)),
        Some(Dir::Left | Dir::Right) if past(x) => return Some(horizontal(x)),
        _ => {}
    }
    match (past(x), past(y)) {
        (_, true) if y.abs() >= x.abs() => Some(vertical(y)),
        (true, _) => Some(horizontal(x)),
        (false, true) => Some(vertical(y)),
        (false, false) => None,
    }
}

// PORT(choice): a held D-pad or stick repeats its arrow after 380 ms, then every 110 ms (the approved prototype's
// timing, about a desktop's key repeat); the console had no launcher.
pub const REPEAT_FIRST: Duration = Duration::from_millis(380);
pub const REPEAT_NEXT: Duration = Duration::from_millis(110);

pub struct Pads {
    events: Receiver<PadEvent>,
    scripted: VecDeque<PadEvent>,
    connected: Arc<Mutex<Vec<PadInfo>>>,
    held: Option<(PadButton, Instant)>,
    error: Arc<Mutex<Option<String>>>,
    focused: Arc<AtomicBool>,
}

impl Pads {
    pub fn start(ctx: &egui::Context) -> Pads {
        let (tx, rx) = mpsc::channel();
        let pads = Pads::with(rx);
        let thread = Thread { ctx: ctx.clone(), tx, list: pads.connected.clone(), error: pads.error.clone() };
        let focused = pads.focused.clone();
        let spawned = std::thread::Builder::new().name("pads".into()).spawn(move || read_pads(&thread, &focused));
        if let Err(e) = spawned {
            ssx_core::eprintln!("pads: the thread did not start: {e}");
            if let Ok(mut slot) = pads.error.lock() {
                *slot = Some(e.to_string());
            }
        }
        pads
    }

    fn with(events: Receiver<PadEvent>) -> Pads {
        let (connected, error) = (Arc::new(Mutex::new(Vec::new())), Arc::new(Mutex::new(None)));
        let focused = Arc::new(AtomicBool::new(true));
        Pads { events, scripted: VecDeque::new(), connected, held: None, error, focused }
    }

    pub fn script(&mut self, button: PadButton, style: PadStyle) {
        for pressed in [true, false] {
            self.scripted.push_back(PadEvent { button, pressed, style });
        }
    }

    pub fn connected(&self) -> Vec<PadInfo> {
        self.connected.lock().map(|l| l.clone()).unwrap_or_default()
    }

    pub fn error(&self) -> Option<String> {
        self.error.lock().ok().and_then(|e| e.clone())
    }

    pub fn take(&mut self, ctx: &egui::Context, focused: bool, now: Instant) -> (Vec<egui::Event>, Option<PadStyle>) {
        let (mut out, mut style) = (Vec::new(), None);
        let key = |button: PadButton, pressed: bool, repeat: bool| egui::Event::Key {
            key: button.key(),
            physical_key: None,
            pressed,
            repeat,
            modifiers: egui::Modifiers::NONE,
        };
        self.focused.store(focused, Ordering::Relaxed);
        let pads: Vec<PadEvent> = self.events.try_iter().filter(|_| focused).collect();
        if !focused {
            self.held = None;
        }
        for ev in self.scripted.drain(..).chain(pads) {
            style = Some(ev.style);
            out.push(key(ev.button, ev.pressed, false));
            match (ev.pressed, self.held) {
                (true, _) if ev.button.repeats() => self.held = Some((ev.button, now + REPEAT_FIRST)),
                (false, Some((held, _))) if held == ev.button => self.held = None,
                _ => {}
            }
        }
        if let Some((button, next)) = &mut self.held {
            if now >= *next {
                out.push(key(*button, true, true));
                *next = (*next + REPEAT_NEXT).max(now + REPEAT_NEXT / 2);
            }
            ctx.request_repaint_after(next.saturating_duration_since(now));
        }
        (out, style)
    }
}

struct Thread {
    ctx: egui::Context,
    tx: Sender<PadEvent>,
    list: Arc<Mutex<Vec<PadInfo>>>,
    error: Arc<Mutex<Option<String>>>,
}

fn read_pads(t: &Thread, focused: &AtomicBool) {
    let mut gilrs = match gilrs::Gilrs::new() {
        Ok(g) => g,
        Err(e) => {
            ssx_core::eprintln!("pads: {e}");
            if let Ok(mut slot) = t.error.lock() {
                *slot = Some(e.to_string());
            }
            t.ctx.request_repaint();
            return;
        }
    };
    let info = |pad: gilrs::Gamepad<'_>| PadInfo {
        name: ssx_core::text::shown_max(pad.name(), 80).to_string(),
        style: style_of(pad.vendor_id(), pad.name()),
        vendor: pad.vendor_id(),
        product: pad.product_id(),
    };
    let relist = |gilrs: &gilrs::Gilrs| {
        if let Ok(mut l) = t.list.lock() {
            *l = gilrs.gamepads().map(|(_, pad)| info(pad)).collect();
        }
    };
    let named = |pad: &PadInfo| {
        let id = match (pad.vendor, pad.product) {
            (Some(v), Some(p)) => format!("{v:04x}:{p:04x}"),
            _ => "no USB id".to_owned(),
        };
        let glyphs = match pad.style {
            PadStyle::Xbox => "Xbox prompts",
            PadStyle::PlayStation => "PlayStation prompts",
        };
        ssx_core::eprintln!("pads: {} ({id}, {glyphs})", pad.name);
    };
    let mut named_ids: Vec<gilrs::GamepadId> = Vec::new();
    for (id, pad) in gilrs.gamepads() {
        named(&info(pad));
        named_ids.push(id);
    }
    relist(&gilrs);
    t.ctx.request_repaint();
    let mut sticks: Vec<(gilrs::GamepadId, f32, f32, Option<Dir>)> = Vec::new();
    let mut down: Vec<(gilrs::GamepadId, PadButton)> = Vec::new();
    let mut idle = Duration::ZERO;
    loop {
        let Some(ev) = gilrs.next_event_blocking(None) else {
            idle = (idle * 2).clamp(Duration::from_millis(1), Duration::from_secs(1));
            std::thread::sleep(idle);
            continue;
        };
        idle = Duration::ZERO;
        let pad = gilrs.gamepad(ev.id);
        let style = style_of(pad.vendor_id(), pad.name());
        let mut presses: Vec<(PadButton, bool)> = Vec::new();
        match ev.event {
            gilrs::EventType::ButtonPressed(b, _) => presses.extend(button_of(b).map(|b| (b, true))),
            gilrs::EventType::ButtonReleased(b, _) => presses.extend(button_of(b).map(|b| (b, false))),
            gilrs::EventType::AxisChanged(axis @ (gilrs::Axis::LeftStickX | gilrs::Axis::LeftStickY), value, _) => {
                let at = match sticks.iter().position(|s| s.0 == ev.id) {
                    Some(at) => at,
                    None => {
                        sticks.push((ev.id, 0.0, 0.0, None));
                        sticks.len() - 1
                    }
                };
                let stick = &mut sticks[at];
                if axis == gilrs::Axis::LeftStickX {
                    stick.1 = value;
                } else {
                    stick.2 = value;
                }
                let dir = stick_dir(stick.3, stick.1, stick.2);
                let was = std::mem::replace(&mut stick.3, dir);
                if was != dir {
                    presses.extend(was.map(|d| (PadButton::of_dir(d), false)));
                    presses.extend(dir.map(|d| (PadButton::of_dir(d), true)));
                }
            }
            gilrs::EventType::Connected => {
                if !named_ids.contains(&ev.id) {
                    named(&info(pad));
                    named_ids.push(ev.id);
                }
                relist(&gilrs);
                t.ctx.request_repaint();
            }
            gilrs::EventType::Disconnected => {
                sticks.retain(|s| s.0 != ev.id);
                named_ids.retain(|id| *id != ev.id);
                presses.extend(down.iter().filter(|(id, _)| *id == ev.id).map(|(_, b)| (*b, false)));
                relist(&gilrs);
                t.ctx.request_repaint();
            }
            _ => {}
        }
        if presses.is_empty() || !focused.load(Ordering::Relaxed) {
            continue;
        }
        for (button, pressed) in presses {
            if t.tx.send(PadEvent { button, pressed, style }).is_err() {
                return;
            }
            down.retain(|d| *d != (ev.id, button));
            if pressed && button.repeats() {
                down.push((ev.id, button));
            }
        }
        t.ctx.request_repaint();
    }
}

fn button_of(b: gilrs::Button) -> Option<PadButton> {
    Some(match b {
        gilrs::Button::DPadUp => PadButton::Up,
        gilrs::Button::DPadDown => PadButton::Down,
        gilrs::Button::DPadLeft => PadButton::Left,
        gilrs::Button::DPadRight => PadButton::Right,
        gilrs::Button::South => PadButton::A,
        gilrs::Button::East => PadButton::B,
        gilrs::Button::LeftTrigger => PadButton::Lb,
        gilrs::Button::RightTrigger => PadButton::Rb,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(events: &[egui::Event]) -> Vec<(Key, bool, bool)> {
        events
            .iter()
            .filter_map(|e| match e {
                egui::Event::Key { key, pressed, repeat, .. } => Some((*key, *pressed, *repeat)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_sony_pad_or_a_playstation_name_shows_playstation_glyphs() {
        assert_eq!(style_of(Some(0x054C), "Wireless Controller"), PadStyle::PlayStation);
        let dualsense = "Sony Interactive Entertainment DualSense Wireless Controller";
        assert_eq!(style_of(None, dualsense), PadStyle::PlayStation);
        assert_eq!(style_of(Some(0x045E), "Xbox Wireless Controller"), PadStyle::Xbox);
        assert_eq!(style_of(Some(0x28DE), "Steam Virtual Gamepad"), PadStyle::Xbox);
        assert_eq!(style_of(None, "PS5 controller"), PadStyle::Xbox, "only the names the drivers give");
    }

    #[test]
    fn the_stick_is_an_arrow_past_the_games_threshold() {
        assert_eq!(stick_dir(None, 0.0, 0.0), None);
        assert_eq!(stick_dir(None, 0.0, 0.39), None);
        assert_eq!(stick_dir(None, 0.0, 0.41), Some(Dir::Up));
        assert_eq!(stick_dir(None, 0.0, -0.9), Some(Dir::Down));
        assert_eq!(stick_dir(None, -0.9, 0.2), Some(Dir::Left));
        assert_eq!(stick_dir(None, 0.6, 0.5), Some(Dir::Right), "the farther axis");
        assert_eq!(stick_dir(Some(Dir::Up), 0.8, 0.5), Some(Dir::Up));
        assert_eq!(stick_dir(Some(Dir::Up), 0.8, 0.3), Some(Dir::Right));
        assert_eq!(stick_dir(Some(Dir::Right), 0.1, 0.1), None);
    }

    #[test]
    fn a_held_direction_repeats_and_a_button_does_not() {
        let ctx = egui::Context::default();
        let (tx, rx) = mpsc::channel();
        let mut pads = Pads::with(rx);
        let send = |button, pressed| tx.send(PadEvent { button, pressed, style: PadStyle::Xbox }).unwrap();
        let t0 = Instant::now();
        send(PadButton::Down, true);
        send(PadButton::A, true);
        let (events, style) = pads.take(&ctx, true, t0);
        assert_eq!(style, Some(PadStyle::Xbox));
        assert_eq!(keys(&events), [(Key::ArrowDown, true, false), (Key::Enter, true, false)]);
        assert!(keys(&pads.take(&ctx, true, t0 + Duration::from_millis(379)).0).is_empty());
        assert_eq!(keys(&pads.take(&ctx, true, t0 + REPEAT_FIRST).0), [(Key::ArrowDown, true, true)]);
        assert!(keys(&pads.take(&ctx, true, t0 + REPEAT_FIRST + Duration::from_millis(100)).0).is_empty());
        let next = t0 + REPEAT_FIRST + REPEAT_NEXT;
        assert_eq!(keys(&pads.take(&ctx, true, next).0), [(Key::ArrowDown, true, true)]);
        send(PadButton::Down, false);
        assert_eq!(keys(&pads.take(&ctx, true, next).0), [(Key::ArrowDown, false, false)]);
        assert!(keys(&pads.take(&ctx, true, next + REPEAT_FIRST).0).is_empty(), "released: no more repeats");
        send(PadButton::Up, true);
        let (events, style) = pads.take(&ctx, false, next);
        assert!(events.is_empty() && style.is_none());
        assert!(keys(&pads.take(&ctx, true, next + REPEAT_FIRST).0).is_empty());
        pads.script(PadButton::Rb, PadStyle::PlayStation);
        let (events, style) = pads.take(&ctx, false, next);
        assert_eq!(keys(&events), [(Key::E, true, false), (Key::E, false, false)]);
        assert_eq!(style, Some(PadStyle::PlayStation));
    }

    #[test]
    fn the_pad_names_are_the_mapping() {
        for (name, b) in PadButton::NAMES {
            assert_eq!(PadButton::parse(name), Some(b));
            assert_eq!(PadButton::parse(&name.to_uppercase()), Some(b));
        }
        assert_eq!(PadButton::parse("x"), None);
        assert_eq!(PadButton::A.key(), Key::Enter);
        assert_eq!(PadButton::B.key(), Key::Escape);
        assert_eq!(PadButton::Lb.key(), Key::Q);
        assert_eq!(PadButton::Rb.key(), Key::E);
    }
}
