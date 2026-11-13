use bevy::input::gamepad::{GamepadInput, GamepadSettings};
use bevy::prelude::*;
use ssx_ui::game::input::{Button, PADS};

use super::GameUiRes;

// PORT(choice): a PC keyboard map (Enter START, Space A, Backspace B, X, Y, arrows D-pad, Q / E bumpers, Z / C and Page
// Up / Page Down triggers — the lists' paging —, V Back), read as a pad device of its own (`KEYBOARD_DEVICE`); a
// gamepad's buttons are the pad's; the console reads pads only.
pub fn pad_presses(
    keys: &ButtonInput<KeyCode>,
    pads: &Query<(Entity, &Gamepad)>,
    slots: &PadSlots,
    sticks: &mut StickDpad,
    search: bool,
) -> Vec<(Button, u8)> {
    let map = [
        (KeyCode::Enter, Button::Start),
        (KeyCode::Space, Button::DigitalOne),
        (KeyCode::Backspace, Button::DigitalTwo),
        (KeyCode::KeyY, Button::DigitalThree),
        (KeyCode::KeyX, Button::DigitalFour),
        (KeyCode::ArrowUp, Button::DpadUp),
        (KeyCode::ArrowDown, Button::DpadDown),
        (KeyCode::ArrowLeft, Button::DpadLeft),
        (KeyCode::ArrowRight, Button::DpadRight),
        (KeyCode::KeyQ, Button::LeftBumper),
        (KeyCode::KeyE, Button::RightBumper),
        (KeyCode::KeyZ, Button::LeftTrigger),
        (KeyCode::KeyC, Button::RightTrigger),
        (KeyCode::PageUp, Button::LeftTrigger),
        (KeyCode::PageDown, Button::RightTrigger),
        (KeyCode::KeyV, Button::Select),
    ];
    let pad_map = [
        (GamepadButton::Start, Button::Start),
        (GamepadButton::Select, Button::Select),
        (GamepadButton::South, Button::DigitalOne),
        (GamepadButton::East, Button::DigitalTwo),
        (GamepadButton::North, Button::DigitalThree),
        (GamepadButton::West, Button::DigitalFour),
        (GamepadButton::DPadUp, Button::DpadUp),
        (GamepadButton::DPadDown, Button::DpadDown),
        (GamepadButton::DPadLeft, Button::DpadLeft),
        (GamepadButton::DPadRight, Button::DpadRight),
        (GamepadButton::LeftTrigger, Button::LeftBumper),
        (GamepadButton::RightTrigger, Button::RightBumper),
    ];
    let keyboard = ssx_ui::game::input::KEYBOARD_DEVICE;
    let mut out: Vec<(Button, u8)> =
        map.iter().filter(|(k, _)| keys.just_pressed(*k)).map(|&(_, b)| (b, keyboard)).collect();
    // PORT(choice): F and the right stick's click are the server browser's search (`RAS`), the one key the browser
    // needs beyond the game's controller map (which maps no stick click); the port's screen, none in the original.
    if search && keys.just_pressed(KeyCode::KeyF) {
        out.push((Button::Ras, keyboard));
    }
    for (entity, pad) in pads {
        let Some(device) = slots.device(entity) else { continue };
        let state = &mut sticks.pads[usize::from(device)];
        out.extend(pad_map.iter().filter(|(g, _)| pad.just_pressed(*g)).map(|&(_, b)| (b, device)));
        if search && pad.just_pressed(GamepadButton::RightThumb) {
            out.push((Button::Ras, device));
        }
        let value = |b: GamepadButton| pad.get(b).unwrap_or(0.0);
        let stick = Vec2::new(value_axis(pad, GamepadAxis::LeftStickX), value_axis(pad, GamepadAxis::LeftStickY));
        let presses = state.step(stick, [value(GamepadButton::LeftTrigger2), value(GamepadButton::RightTrigger2)]);
        out.extend(presses.into_iter().map(|b| (b, device)));
    }
    out
}

fn value_axis(pad: &Gamepad, axis: GamepadAxis) -> f32 {
    pad.get(axis).unwrap_or(0.0)
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PadState {
    stick: Vec2,
    triggers: [f32; 2],
}

#[derive(Clone, Debug, Default)]
pub struct StickDpad {
    pads: [PadState; PADS],
}

const STICK_DPAD: f32 = 0.4;

impl PadState {
    pub fn step(&mut self, cur: Vec2, triggers: [f32; 2]) -> Vec<Button> {
        let t = STICK_DPAD;
        let past = |v: f32| !(-t..=t).contains(&v);
        let (prev, mut x, mut y) = (self.stick, cur.x, cur.y);
        let y_now = past(y);
        let mut y_wins = y_now;
        if past(prev.y) && y_now {
            x = 0.0;
        }
        if past(prev.x) && y_now {
            y = 0.0;
            y_wins = false;
        }
        if y_wins {
            x = 0.0;
        }
        let mut out = Vec::new();
        for (was, now, b) in [
            (prev.y < -t, y < -t, Button::DpadDown),
            (prev.y > t, y > t, Button::DpadUp),
            (prev.x < -t, x < -t, Button::DpadLeft),
            (prev.x > t, x > t, Button::DpadRight),
        ] {
            if now && !was {
                out.push(b);
            }
        }
        for (k, b) in [Button::LeftTrigger, Button::RightTrigger].into_iter().enumerate() {
            if triggers[k] >= 0.5 && self.triggers[k] < 0.5 {
                out.push(b);
            }
        }
        (self.stick, self.triggers) = (Vec2::new(x, y), triggers);
        out
    }
}

#[derive(Component)]
pub struct ScriptPad;

type PadQuery<'w, 's> = Query<'w, 's, (Entity, Has<Gamepad>, Has<ScriptPad>), With<GamepadSettings>>;

#[derive(Resource, Default)]
pub struct PadSlots {
    slots: [Option<Entity>; PADS],
    reported: [bool; PADS],
    synced: bool,
    script_pad: bool,
    focus_follows: bool,
    focus_armed: bool,
}

impl PadSlots {
    pub(super) fn new(script_pad: bool, focus_follows: bool) -> Self {
        Self { script_pad, focus_follows, ..Default::default() }
    }

    pub fn device(&self, pad: Entity) -> Option<u8> {
        self.slots.iter().position(|s| *s == Some(pad)).map(|d| d as u8)
    }

    // PORT(choice): a new pad takes the lowest free device and a returning one its old device; the console has its user
    // indices.
    fn update(&mut self, pads: &PadQuery) -> Vec<(usize, bool)> {
        let gamepads: Vec<(Entity, bool, bool)> = pads.iter().collect();
        self.update_with(&gamepads)
    }

    fn update_with(&mut self, gamepads: &[(Entity, bool, bool)]) -> Vec<(usize, bool)> {
        let first = usize::from(self.script_pad);
        let connected = |e: Option<Entity>| e.is_some_and(|e| gamepads.iter().any(|&(g, c, _)| g == e && c));
        for &(e, c, script) in gamepads {
            if !c || self.slots.contains(&Some(e)) {
                continue;
            }
            if script && self.script_pad {
                info!("shell: gamepad {e} is the scripts' pad, device 0");
                self.slots[0] = Some(e);
                continue;
            }
            if let Some(d) = (first..PADS).find(|&d| !connected(self.slots[d])) {
                info!("shell: gamepad {e} is pad device {d}");
                self.slots[d] = Some(e);
            }
        }
        let mut changes = Vec::new();
        for d in first..PADS {
            let c = connected(self.slots[d]);
            if c != self.reported[d] || !self.synced {
                self.reported[d] = c;
                changes.push((d, c));
            }
        }
        changes
    }
}

// PORT(choice): the window's focus stands for the console's system UI (`XN_SYS_UI`, `fe::pads::system_ui`).
// PORT(choice): a start on the keyboard leaves every gamepad live; the console reads pads only.
pub fn pads_frame(
    mut slots: ResMut<PadSlots>,
    gamepads: PadQuery,
    mut states: Query<(Entity, &mut Gamepad)>,
    mut focus: MessageReader<bevy::window::WindowFocused>,
    mut ui: ResMut<GameUiRes>,
) {
    let changes = slots.update(&gamepads);
    let first_sync = !std::mem::replace(&mut slots.synced, true);
    for (d, c) in changes {
        if first_sync {
            if let Some(fe) = ui.fe.as_mut() {
                fe.pads.connected[d] = c;
            }
        } else {
            info!("shell: pad device {d} {}", if c { "connected" } else { "disconnected" });
            ui.pad_connection(d, c);
        }
    }
    for ev in focus.read() {
        slots.focus_armed |= ev.focused;
        if slots.focus_follows && slots.focus_armed {
            info!(
                "shell: the window {} the focus: the system UI {}",
                if ev.focused { "has" } else { "lost" },
                if ev.focused { "closes" } else { "opens" }
            );
            ui.system_ui(!ev.focused);
        }
    }
    if let Some(active) = ui.active_pad() {
        for (e, mut pad) in &mut states {
            if slots.device(e) != Some(active as u8) {
                pad.digital_mut().reset_all();
                let axes: Vec<GamepadInput> = pad.analog().all_axes().copied().collect();
                for a in axes {
                    pad.analog_mut().set(a, 0.0);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_gamepad_takes_the_lowest_free_device_and_its_own_again() {
        let (a, b, c) =
            (Entity::from_raw_u32(10).unwrap(), Entity::from_raw_u32(11).unwrap(), Entity::from_raw_u32(12).unwrap());
        let mut s = PadSlots::default();
        assert_eq!(s.update_with(&[]), vec![(0, false), (1, false), (2, false), (3, false)]);
        s.synced = true;
        assert_eq!(s.update_with(&[(a, true, false), (b, true, false)]), vec![(0, true), (1, true)]);
        assert_eq!(s.device(b), Some(1));
        assert_eq!(s.update_with(&[(a, false, false), (b, true, false)]), vec![(0, false)]);
        assert_eq!(s.update_with(&[(a, true, false), (b, true, false)]), vec![(0, true)]);
        assert_eq!(s.device(a), Some(0));
        assert_eq!(s.update_with(&[(a, false, false), (b, true, false), (c, true, false)]), vec![]);
        assert_eq!(s.device(c), Some(0));
        let mut t = PadSlots { script_pad: true, synced: true, ..Default::default() };
        assert_eq!(t.update_with(&[(a, true, false)]), vec![(1, true)]);
        assert_eq!(t.device(a), Some(1));
    }

    #[test]
    fn the_scripts_gamepad_is_device_zero_in_a_test_run() {
        let (a, p) = (Entity::from_raw_u32(10).unwrap(), Entity::from_raw_u32(11).unwrap());
        let mut t = PadSlots { script_pad: true, synced: true, ..Default::default() };
        assert_eq!(t.update_with(&[(a, true, false), (p, true, true)]), vec![(1, true)]);
        assert_eq!((t.device(p), t.device(a)), (Some(0), Some(1)));
        let mut s = PadSlots { synced: true, ..Default::default() };
        assert_eq!(s.update_with(&[(p, true, true)]), vec![(0, true)]);
    }

    #[test]
    fn the_left_stick_is_the_d_pad() {
        let mut p = PadState::default();
        assert_eq!(p.step(Vec2::new(0.0, 0.5), [0.0; 2]), [Button::DpadUp]);
        assert_eq!(p.step(Vec2::new(0.9, 0.6), [0.0; 2]), []);
        assert_eq!(p.step(Vec2::new(0.9, 0.1), [0.0; 2]), [Button::DpadRight]);
        assert_eq!(p.step(Vec2::new(0.9, -0.9), [0.0; 2]), []);
        assert_eq!(p.step(Vec2::ZERO, [0.0; 2]), []);
        assert_eq!(p.step(Vec2::new(-0.8, -0.8), [0.0; 2]), [Button::DpadDown]);
        assert_eq!(p.step(Vec2::ZERO, [0.6, 0.4]), [Button::LeftTrigger]);
        assert_eq!(p.step(Vec2::ZERO, [0.7, 0.5]), [Button::RightTrigger]);
    }
}
