use bevy::input::gamepad::{GamepadConnection, GamepadConnectionEvent, RawGamepadButtonChangedEvent, RawGamepadEvent};
use bevy::prelude::*;
use ssx_physics::rewind::RewindState;
use ssx_physics::rider::{Input, RideState, TrickInput};

use super::{Ride, TRIGGER_THRESHOLD};

const NUMPAD: [KeyCode; 4] = [KeyCode::Numpad8, KeyCode::Numpad4, KeyCode::Numpad6, KeyCode::Numpad2];

const SECTOR: f32 = 0.383;

#[derive(Clone, Copy, Debug, Default)]
pub(super) struct Buttons {
    pub l2: bool,
    pub r1: bool,
    pub r2: f32,
    pub rewind: bool,
    pub takeover: bool,
}

fn enabled() -> bool {
    super::scripted::env_flag("SSX_AUTOPLAY_KEYS")
}

fn heli_prepare() -> bool {
    super::scripted::env_flag("SSX_AUTOPLAY_HELI_PREPARE")
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct KeyRewind {
    at: f32,
    back: f32,
    key: KeyCode,
    after: f32,
}

fn key_named(name: &str) -> Option<KeyCode> {
    Some(match name {
        "backspace" => KeyCode::Backspace,
        "space" => KeyCode::Space,
        "i" => KeyCode::KeyI,
        "j" => KeyCode::KeyJ,
        "k" => KeyCode::KeyK,
        "l" => KeyCode::KeyL,
        "e" => KeyCode::KeyE,
        "q" => KeyCode::KeyQ,
        "x" => KeyCode::KeyX,
        "z" => KeyCode::KeyZ,
        "c" => KeyCode::KeyC,
        "shift" => KeyCode::ShiftLeft,
        "ctrl" => KeyCode::ControlLeft,
        "up" => KeyCode::ArrowUp,
        "down" => KeyCode::ArrowDown,
        "left" => KeyCode::ArrowLeft,
        "right" => KeyCode::ArrowRight,
        "num8" => KeyCode::Numpad8,
        "num2" => KeyCode::Numpad2,
        "num4" => KeyCode::Numpad4,
        "num6" => KeyCode::Numpad6,
        _ => return None,
    })
}

fn parse_key_rewind(v: &str) -> Result<KeyRewind, String> {
    let parts: Vec<&str> = v.split(',').map(str::trim).collect();
    let number = |i: usize| -> Result<f32, String> {
        let p = parts.get(i).ok_or_else(|| format!("part {} missing", i + 1))?;
        p.parse::<f32>().ok().filter(|x| x.is_finite() && *x >= 0.0).ok_or_else(|| format!("{p:?} is no time"))
    };
    if !(3..=4).contains(&parts.len()) {
        return Err("AT_S,BACK_S,KEY[,AFTER_S] expected".into());
    }
    let key = key_named(&parts[2].to_ascii_lowercase()).ok_or_else(|| format!("{:?} is no key it knows", parts[2]))?;
    let after = if parts.len() == 4 { number(3)? } else { 0.5 };
    Ok(KeyRewind { at: number(0)?, back: number(1)?, key, after })
}

fn key_rewind() -> Option<KeyRewind> {
    static SCRIPT: std::sync::OnceLock<Option<KeyRewind>> = std::sync::OnceLock::new();
    *SCRIPT.get_or_init(|| {
        let v = std::env::var("SSX_AUTOPLAY_KEY_REWIND").ok()?;
        parse_key_rewind(&v).map_err(|e| warn!("SSX_AUTOPLAY_KEY_REWIND: {e}; not used")).ok()
    })
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct KeyHold {
    at: f32,
    key: KeyCode,
    dur: f32,
}

fn parse_key_holds(v: &str) -> Result<Vec<KeyHold>, String> {
    let windows: Vec<&str> = v.split(',').map(str::trim).filter(|w| !w.is_empty()).collect();
    if windows.is_empty() || windows.len() > 32 {
        return Err("1 … 32 windows AT_S:KEY:DUR_S expected".into());
    }
    windows
        .into_iter()
        .map(|w| {
            let parts: Vec<&str> = w.split(':').map(str::trim).collect();
            let [at, key, dur] = parts[..] else { return Err(format!("{w:?} is no AT_S:KEY:DUR_S")) };
            let time = |p: &str| p.parse::<f32>().ok().filter(|x| x.is_finite() && *x >= 0.0);
            let (Some(at), Some(dur)) = (time(at), time(dur)) else { return Err(format!("{w:?}: no times")) };
            let key = key_named(&key.to_ascii_lowercase()).ok_or_else(|| format!("{key:?} is no key it knows"))?;
            Ok(KeyHold { at, key, dur })
        })
        .collect()
}

fn key_holds() -> &'static [KeyHold] {
    static SCRIPT: std::sync::OnceLock<Vec<KeyHold>> = std::sync::OnceLock::new();
    SCRIPT.get_or_init(|| {
        let Ok(v) = std::env::var("SSX_AUTOPLAY_KEY_HOLDS") else { return Vec::new() };
        parse_key_holds(&v).unwrap_or_else(|e| {
            warn!("SSX_AUTOPLAY_KEY_HOLDS: {e}; not used");
            Vec::new()
        })
    })
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct PadL2 {
    at: f32,
    travel: f32,
    dur: f32,
}

fn parse_pad_l2(v: &str) -> Result<Vec<PadL2>, String> {
    let windows: Vec<&str> = v.split(',').map(str::trim).filter(|w| !w.is_empty()).collect();
    if windows.is_empty() || windows.len() > 32 {
        return Err("1 … 32 windows AT_S:TRAVEL:DUR_S expected".into());
    }
    windows
        .into_iter()
        .map(|w| {
            let parts: Vec<f32> = w.split(':').filter_map(|p| p.trim().parse::<f32>().ok()).collect();
            let [at, travel, dur] = parts[..] else { return Err(format!("{w:?} is no AT_S:TRAVEL:DUR_S")) };
            let time = |x: f32| x.is_finite() && x >= 0.0;
            if !(time(at) && time(dur) && (0.0..=1.0).contains(&travel)) {
                return Err(format!("{w:?}: times ≥ 0 and a travel in [0, 1] expected"));
            }
            Ok(PadL2 { at, travel, dur })
        })
        .collect()
}

fn pad_l2() -> &'static [PadL2] {
    static SCRIPT: std::sync::OnceLock<Vec<PadL2>> = std::sync::OnceLock::new();
    SCRIPT.get_or_init(|| {
        let Ok(v) = std::env::var("SSX_AUTOPLAY_PAD_L2") else { return Vec::new() };
        parse_pad_l2(&v).unwrap_or_else(|e| {
            warn!("SSX_AUTOPLAY_PAD_L2: {e}; not used");
            Vec::new()
        })
    })
}

#[derive(Resource)]
pub(super) struct TestPad {
    id: Entity,
    travel: f32,
}

pub(super) fn test_pad(
    ride: Res<Ride>,
    pad: Option<ResMut<TestPad>>,
    mut commands: Commands,
    (mut connect, mut raw): (MessageWriter<GamepadConnectionEvent>, MessageWriter<RawGamepadEvent>),
) {
    let script = pad_l2();
    if script.is_empty() || !ride.autoplay || !enabled() {
        return;
    }
    let Some(mut pad) = pad else {
        let id = commands
            .spawn((Name::new("test pad"), crate::shell::game_mode::ScriptPad, crate::shell::level::LevelEntity))
            .id();
        let connection = GamepadConnection::Connected { name: "test pad".into(), vendor_id: None, product_id: None };
        connect.write(GamepadConnectionEvent::new(id, connection));
        commands.insert_resource(TestPad { id, travel: 0.0 });
        info!("Ride step {}: test pad: connected", ride.steps);
        return;
    };
    let time = (ride.steps - ride.autoplay_waited) as f32 / ssx_physics::SIM_HZ;
    let travel = script.iter().find(|w| time >= w.at && time < w.at + w.dur).map_or(0.0, |w| w.travel);
    if travel != pad.travel {
        info!("Ride step {}: test pad: L2 travel {travel} (was {})", ride.steps + 1, pad.travel);
        raw.write(RawGamepadEvent::Button(RawGamepadButtonChangedEvent::new(
            pad.id,
            GamepadButton::LeftTrigger2,
            travel,
        )));
        pad.travel = travel;
    }
}

pub(super) fn test_pad_end(pad: Option<Res<TestPad>>, mut commands: Commands) {
    if let Some(pad) = pad {
        commands.entity(pad.id).despawn();
        commands.remove_resource::<TestPad>();
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct StepRead {
    grind_button: bool,
    sa_l2: bool,
    boosting: bool,
    rewind: RewindState,
    wingsuit_hold: bool,
    in_air: bool,
    bail_on_landing: bool,
    r2: bool,
    shoulders: [bool; 2],
}

#[derive(Clone, Debug, Default)]
pub(super) struct KeyHoldsRun {
    down: Vec<KeyCode>,
    read: Option<StepRead>,
    live: (bool, [bool; 2]),
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) enum KeyRewindRun {
    #[default]
    Waiting,
    Holding(u32),
    Released {
        replay: u32,
        held: u32,
    },
    Done,
}

pub(super) fn keys_for(input: &Input, tricks: &TrickInput, b: Buttons, classic: bool) -> Vec<KeyCode> {
    let mut k = Vec::new();
    let mut add = |key: KeyCode, on: bool| {
        if on && !k.contains(&key) {
            k.push(key);
        }
    };
    let dpad = if classic { input.dpad } else { Vec2::ZERO };
    add(KeyCode::ArrowRight, input.steer > 0.0 || dpad.x > 0.0);
    add(KeyCode::ArrowLeft, input.steer < 0.0 || dpad.x < 0.0);
    add(KeyCode::ArrowUp, input.tuck > 0.0 || dpad.y > 0.0);
    add(KeyCode::ArrowDown, input.brake > 0.0 || dpad.y < 0.0);
    add(KeyCode::Space, input.jump);
    let rs = tricks.right_stick;
    add(KeyCode::Numpad6, rs.x >= SECTOR);
    add(KeyCode::Numpad4, rs.x <= -SECTOR);
    add(KeyCode::Numpad8, rs.y >= SECTOR);
    add(KeyCode::Numpad2, rs.y <= -SECTOR || (b.takeover && rs.length() < SECTOR));
    if classic {
        add(KeyCode::ControlLeft, tricks.y);
        add(KeyCode::KeyJ, tricks.x || tricks.tweak);
        add(KeyCode::KeyL, tricks.b || b.rewind);
        add(KeyCode::KeyQ, tricks.lb);
        add(KeyCode::KeyE, tricks.rb || b.r1);
        add(KeyCode::KeyZ, tricks.lt || b.l2);
        add(KeyCode::KeyX, tricks.rt);
    } else {
        add(KeyCode::KeyI, tricks.y);
        add(KeyCode::KeyJ, tricks.x);
        add(KeyCode::KeyL, tricks.b);
        add(KeyCode::KeyK, tricks.a && !input.jump);
        add(KeyCode::ShiftLeft, tricks.tweak || b.r2 >= TRIGGER_THRESHOLD);
        add(KeyCode::ControlLeft, b.l2);
        add(KeyCode::KeyE, b.r1);
        add(KeyCode::Backspace, b.rewind);
    }
    add(KeyCode::ShiftLeft, tricks.signature);
    add(KeyCode::ControlLeft, tricks.signature);
    k
}

impl Ride {
    pub(super) fn through_keys(&mut self, gamepads: &Query<&Gamepad>, chase: bool) {
        if !self.autoplay || !enabled() {
            return;
        }
        let mut tricks = self.tricks;
        let ground = self.state.state == RideState::Riding && self.state.start.is_none() && !self.autoplay_sat_out;
        if heli_prepare() && ground && tricks.right_stick == Vec2::ZERO {
            tricks.right_stick = Vec2::NEG_Y;
        }
        let b = Buttons {
            l2: self.l2,
            r1: self.r1,
            r2: self.r2,
            rewind: self.rewind_pad.rewind_held,
            takeover: self.rewind_pad.takeover,
        };
        let mut keys = ButtonInput::<KeyCode>::default();
        let mut pressed = keys_for(&self.input, &tricks, b, self.classic);
        if let Some(script) = self.key_rewind_keys() {
            pressed = script;
        }
        if let Some(script) = self.key_holds_keys() {
            pressed = script;
        }
        for &key in &pressed {
            keys.press(key);
        }
        let numpad: Vec<KeyCode> = pressed.into_iter().filter(|k| NUMPAD.contains(k)).collect();
        if numpad != self.numpad_held {
            info!(
                "Ride step {}: autoplay keys: the numpad {:?} ({}, the rider {:?})",
                self.steps + 1,
                numpad,
                if numpad.is_empty() { "released" } else { "held" },
                self.state.state
            );
            self.numpad_held = numpad;
        }
        super::controls::read_live_controls(self, &keys, gamepads, chase);
        self.key_holds.live = (self.r2 >= TRIGGER_THRESHOLD, [self.tricks.lt, self.tricks.rt]);
        if matches!(self.key_rewind, KeyRewindRun::Released { .. }) && self.rewind_pad.takeover {
            info!("Ride step {}: key rewind: the live controls read a takeover input", self.steps + 1);
        }
    }

    fn key_holds_keys(&mut self) -> Option<Vec<KeyCode>> {
        let (script, pad) = (key_holds(), pad_l2());
        if script.is_empty() && pad.is_empty() {
            return None;
        }
        let read = StepRead {
            grind_button: self.state.grind.button,
            sa_l2: self.state.grind.l2_hold,
            boosting: self.state.boost.is_boosting,
            rewind: self.last_exec.as_ref().map_or(RewindState::Idle, |e| e.rewind.state),
            wingsuit_hold: self.state.wingsuit.hold,
            in_air: self.state.state != RideState::Riding,
            bail_on_landing: self.state.landing.bail_on_landing,
            r2: self.key_holds.live.0,
            shoulders: self.key_holds.live.1,
        };
        if self.key_holds.read != Some(read) && self.steps > 0 {
            info!(
                "Ride step {}: key holds: the step read the grind button {}, the surface analysis' next L2 {}, boosting \
                 {} (meter {:.3}), the rewind {:?}, the wingsuit hold {}, in the air {}, the landing bail {}, \
                 R2 held {}, the classic shoulders LT {} RT {}",
                self.steps,
                read.grind_button,
                read.sa_l2,
                read.boosting,
                self.state.score.boost,
                read.rewind,
                read.wingsuit_hold,
                read.in_air,
                read.bail_on_landing,
                read.r2,
                read.shoulders[0],
                read.shoulders[1]
            );
        }
        self.key_holds.read = Some(read);
        let time = (self.steps - self.autoplay_waited) as f32 / ssx_physics::SIM_HZ;
        let mut down: Vec<KeyCode> = Vec::new();
        for w in script.iter().filter(|w| time >= w.at && time < w.at + w.dur) {
            if !down.contains(&w.key) {
                down.push(w.key);
            }
        }
        if down != self.key_holds.down {
            info!("Ride step {}: key holds: {:?} held (was {:?})", self.steps + 1, down, self.key_holds.down);
            self.key_holds.down.clone_from(&down);
        }
        let pad_open = pad.iter().any(|w| time >= w.at && time < w.at + w.dur);
        (!down.is_empty() || pad_open).then_some(down)
    }

    fn key_rewind_keys(&mut self) -> Option<Vec<KeyCode>> {
        let script = key_rewind()?;
        let step = self.steps + 1;
        let state = self.last_exec.as_ref().map_or(RewindState::Idle, |e| e.rewind.state);
        if state != self.key_rewind_state {
            if self.key_rewind != KeyRewindRun::Waiting {
                info!("Ride step {step}: key rewind: the rewind {:?} -> {state:?}", self.key_rewind_state);
            }
            self.key_rewind_state = state;
        }
        let time = (self.steps - self.autoplay_waited) as f32 / ssx_physics::SIM_HZ;
        match self.key_rewind {
            KeyRewindRun::Waiting if time >= script.at && self.state.start.is_none() => {
                let hold = (30.0 * script.back + 12.0).round() as u32;
                info!("Ride step {step}: key rewind: Backspace held for {hold} executions ({:.1} s back)", script.back);
                self.key_rewind = KeyRewindRun::Holding(hold);
                Some(vec![KeyCode::Backspace])
            }
            KeyRewindRun::Waiting | KeyRewindRun::Done => None,
            KeyRewindRun::Holding(n) if n > 1 => {
                self.key_rewind = KeyRewindRun::Holding(n - 1);
                Some(vec![KeyCode::Backspace])
            }
            KeyRewindRun::Holding(_) => {
                info!("Ride step {step}: key rewind: Backspace released");
                self.key_rewind = KeyRewindRun::Released { replay: 0, held: 0 };
                Some(Vec::new())
            }
            KeyRewindRun::Released { replay, held } => {
                if state == RewindState::Idle {
                    info!(
                        "Ride step {step}: key rewind: the replay ended after {replay} executions, {:?} held for {held}",
                        script.key
                    );
                    self.key_rewind = KeyRewindRun::Done;
                    return None;
                }
                let replay = replay + u32::from(state == RewindState::Replay);
                let press = replay as f32 > script.after * ssx_physics::SIM_HZ;
                if press && held == 0 {
                    info!("Ride step {step}: key rewind: {:?} pressed at the replay's execution {replay}", script.key);
                }
                self.key_rewind = KeyRewindRun::Released { replay, held: held + u32::from(press) };
                Some(if press { vec![script.key] } else { Vec::new() })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::controls::{TakeoverPad, live_read};
    use super::*;

    #[test]
    fn the_keys_read_back_as_the_controls() {
        use bevy::ecs::system::RunSystemOnce;
        let read = |pressed: Vec<KeyCode>, classic: bool| {
            let mut app = App::new();
            app.add_plugins(bevy::input::InputPlugin);
            let mut keys = ButtonInput::<KeyCode>::default();
            for k in pressed {
                keys.press(k);
            }
            app.world_mut().insert_resource(keys);
            app.world_mut()
                .run_system_once(move |keys: Res<ButtonInput<KeyCode>>, pads: Query<&Gamepad>| {
                    let r = live_read(&keys, &pads, true, classic, false);
                    (r.input, r.tricks, r)
                })
                .expect("the live controls")
        };
        let input = Input { steer: 1.0, tuck: 1.0, jump: true, ..Input::default() };
        let tricks = TrickInput { right_stick: Vec2::NEG_Y, x: true, a: true, ..TrickInput::default() };
        let (i, t, _) = read(keys_for(&input, &tricks, Buttons::default(), false), false);
        assert_eq!((i.steer, i.tuck, i.brake, i.jump), (1.0, 1.0, 0.0, true));
        assert_eq!(t.right_stick, Vec2::NEG_Y, "Numpad 2: the right stick down");
        assert!(t.x && t.a && !t.y && !t.b && !t.tweak);
        let diagonal = TrickInput { right_stick: Vec2::new(-0.7, 0.7), tweak: true, ..TrickInput::default() };
        let (_, t, _) = read(keys_for(&Input::default(), &diagonal, Buttons::default(), false), false);
        assert!((t.right_stick - Vec2::new(-1.0, 1.0).normalize()).length() < 1e-6, "Numpad 8 + 4: up-left");
        assert!(t.tweak, "Shift: R2");
        let none = (Input::default(), TrickInput::default());
        for classic in [false, true] {
            let takeover = Buttons { takeover: true, ..Buttons::default() };
            let (i, t, r) = read(keys_for(&none.0, &none.1, takeover, classic), classic);
            assert_eq!(t.right_stick, Vec2::NEG_Y, "the takeover: the right stick down");
            assert!(TakeoverPad::of(&i, &t, r.l2, r.r1, classic).takeover(), "read as a takeover, classic={classic}");
            let (i, t, r) = read(keys_for(&none.0, &none.1, Buttons::default(), classic), classic);
            assert!(!TakeoverPad::of(&i, &t, r.l2, r.r1, classic).takeover(), "nothing held, classic={classic}");
            let l2 = Buttons { l2: true, ..Buttons::default() };
            let (_, t, r) = read(keys_for(&none.0, &none.1, l2, classic), classic);
            assert!(r.l2 == 1.0 && !t.y, "L2: Ctrl (standard) / Z (classic), classic={classic}");
            let rewind = Buttons { rewind: true, ..Buttons::default() };
            let (_, t, r) = read(keys_for(&none.0, &none.1, rewind, classic), classic);
            let rewound = if classic { t.b } else { r.rewind && !t.b };
            assert!(rewound, "the rewind: Backspace (L1) / L (B), classic={classic}");
        }
        let classic = TrickInput { rt: true, lb: true, x: true, tweak: true, ..TrickInput::default() };
        let air = Input { dpad: Vec2::X, dpad_held: true, steer: 1.0, ..Input::default() };
        let (i, t, _) = read(keys_for(&air, &classic, Buttons::default(), true), true);
        assert_eq!((i.dpad, i.dpad_held, i.steer), (Vec2::X, true, 1.0));
        assert!(t.rt && t.lb && t.x && t.tweak && !t.rb && !t.lt);
    }

    #[test]
    fn the_pad_l2_script_parses() {
        let w = parse_pad_l2("6:0.35:2, 10:1:0.5").expect("a script");
        assert_eq!(w[0], PadL2 { at: 6.0, travel: 0.35, dur: 2.0 });
        assert_eq!(w[1], PadL2 { at: 10.0, travel: 1.0, dur: 0.5 });
        for bad in ["", "6:0.3", "6:1.5:1", "6:-0.1:1", "-1:0.3:1", "6:0.3:inf", "6:x:1"] {
            assert!(parse_pad_l2(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn the_key_holds_script_parses() {
        let w = parse_key_holds("6:ctrl:1.5, 9.25:L:0.4").expect("a script");
        assert_eq!(w[0], KeyHold { at: 6.0, key: KeyCode::ControlLeft, dur: 1.5 });
        assert_eq!(w[1], KeyHold { at: 9.25, key: KeyCode::KeyL, dur: 0.4 });
        for bad in ["", "6:ctrl", "6:ctrl:1:2", "x:ctrl:1", "6:f13:1", "6:ctrl:-1", "6:ctrl:inf"] {
            assert!(parse_key_holds(bad).is_err(), "{bad}");
        }
        assert!(parse_key_holds(&vec!["1:j:1"; 33].join(",")).is_err(), "at most 32 windows");
    }

    #[test]
    fn the_key_rewind_script_parses() {
        let r = parse_key_rewind("4, 3, J").expect("a script");
        assert_eq!(r, KeyRewind { at: 4.0, back: 3.0, key: KeyCode::KeyJ, after: 0.5 });
        assert_eq!(parse_key_rewind("4,3,num2,1").map(|r| (r.key, r.after)), Ok((KeyCode::Numpad2, 1.0)));
        for bad in ["4,3", "4,3,j,1,2", "x,3,j", "4,-1,j", "4,3,f13", "4,NaN,j"] {
            assert!(parse_key_rewind(bad).is_err(), "{bad}");
        }
    }
}
