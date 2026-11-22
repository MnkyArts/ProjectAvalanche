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
        sticks.presses.extend(presses.into_iter().map(|b| (b, device)));
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
    presses: Vec<(Button, u8)>,
}

impl StickDpad {
    pub fn take(&mut self) -> Vec<(Button, u8)> {
        std::mem::take(&mut self.presses)
    }
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

// PORT(choice): every gamepad while the keyboard (the PC's) is signed in; none while the window has lost the focus
// (outside a test run, as the system UI) or the exit waits, so a motor never runs behind the player's back. The
// console's simulation module sends to `1 << USER_controllerid` (0x82AB2DE8), its signed-in pad, and the guide button's
// pause stops the posts.
pub(super) fn rumble_target(
    slots: Res<PadSlots>,
    ui: Res<GameUiRes>,
    shell: Res<super::Shell>,
    gamepads: Query<Entity, crate::ride::rumble::RealPad>,
    window: Query<&Window, With<bevy::window::PrimaryWindow>>,
    mut target: ResMut<crate::ride::rumble::RumbleTarget>,
) {
    use crate::ride::rumble::RumbleTarget;
    let focused = !slots.focus_follows || window.iter().all(|w| w.focused);
    let next = if shell.exiting.is_some() || !focused {
        RumbleTarget::Pads(Vec::new())
    } else if slots.script_pad {
        RumbleTarget::Unattended
    } else {
        RumbleTarget::Pads(match ui.active_pad() {
            None => gamepads.iter().collect(),
            Some(d) => slots.slots.get(d).copied().flatten().filter(|e| gamepads.contains(*e)).into_iter().collect(),
        })
    };
    if *target != next {
        *target = next;
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

    fn read_pad(x: f32, dpad_right: bool) -> (Vec<(Button, u8)>, StickDpad) {
        use bevy::ecs::system::RunSystemOnce;
        use bevy::input::gamepad::{
            GamepadConnection, GamepadConnectionEvent, RawGamepadAxisChangedEvent, RawGamepadButtonChangedEvent,
            RawGamepadEvent,
        };
        let mut app = App::new();
        app.add_plugins(bevy::input::InputPlugin);
        let id = app.world_mut().spawn_empty().id();
        app.world_mut().write_message(GamepadConnectionEvent::new(
            id,
            GamepadConnection::Connected { name: "test pad".into(), vendor_id: None, product_id: None },
        ));
        app.update();
        let axis = RawGamepadAxisChangedEvent::new(id, GamepadAxis::LeftStickX, x);
        app.world_mut().write_message(RawGamepadEvent::Axis(axis));
        if dpad_right {
            let button = RawGamepadButtonChangedEvent::new(id, GamepadButton::DPadRight, 1.0);
            app.world_mut().write_message(RawGamepadEvent::Button(button));
        }
        app.update();
        let mut slots = PadSlots { synced: true, ..Default::default() };
        slots.update_with(&[(id, true, false)]);
        app.world_mut()
            .run_system_once(move |keys: Res<ButtonInput<KeyCode>>, pads: Query<(Entity, &Gamepad)>| {
                let mut sticks = StickDpad::default();
                let presses = pad_presses(&keys, &pads, &slots, &mut sticks, false);
                (presses, sticks)
            })
            .expect("the pad read")
    }

    #[test]
    fn the_left_stick_is_no_pad_d_pad_press() {
        let (presses, _) = read_pad(0.9, false);
        assert!(presses.iter().all(|p| p.0 != Button::DpadRight), "{presses:?}");
        let (presses, _) = read_pad(0.0, true);
        assert_eq!(presses, [(Button::DpadRight, 0)], "the D-pad itself");
    }

    #[test]
    fn a_tap_within_one_frame_is_a_press() {
        use bevy::ecs::system::RunSystemOnce;
        use bevy::input::ButtonState;
        use bevy::input::gamepad::{
            GamepadConnection, GamepadConnectionEvent, RawGamepadButtonChangedEvent, RawGamepadEvent,
        };
        use bevy::input::keyboard::{Key, KeyboardInput};
        let mut app = App::new();
        app.add_plugins(bevy::input::InputPlugin);
        let (id, window) = (app.world_mut().spawn_empty().id(), app.world_mut().spawn_empty().id());
        app.world_mut().write_message(GamepadConnectionEvent::new(
            id,
            GamepadConnection::Connected { name: "test pad".into(), vendor_id: None, product_id: None },
        ));
        app.update();
        for (value, state) in [(1.0, ButtonState::Pressed), (0.0, ButtonState::Released)] {
            let button = RawGamepadButtonChangedEvent::new(id, GamepadButton::South, value);
            app.world_mut().write_message(RawGamepadEvent::Button(button));
            let (key_code, logical_key, text, repeat) = (KeyCode::ArrowDown, Key::ArrowDown, None, false);
            app.world_mut().write_message(KeyboardInput { key_code, logical_key, state, text, repeat, window });
        }
        app.update();
        let mut slots = PadSlots { synced: true, ..Default::default() };
        slots.update_with(&[(id, true, false)]);
        let presses = app
            .world_mut()
            .run_system_once(move |keys: Res<ButtonInput<KeyCode>>, pads: Query<(Entity, &Gamepad)>| {
                assert!(!keys.pressed(KeyCode::ArrowDown), "released within the frame");
                pad_presses(&keys, &pads, &slots, &mut StickDpad::default(), false)
            })
            .expect("the pad read");
        let keyboard = ssx_ui::game::input::KEYBOARD_DEVICE;
        assert_eq!(presses, [(Button::DpadDown, keyboard), (Button::DigitalOne, 0)]);
    }

    #[test]
    fn a_tap_through_steams_virtual_pad_alone_is_one_press() {
        use bevy::ecs::system::RunSystemOnce;
        use bevy::input::gamepad::{
            GamepadConnection, GamepadConnectionEvent, RawGamepadButtonChangedEvent, RawGamepadEvent,
        };
        let tap = |offered: &[(Option<u16>, Option<u16>)]| {
            let mut app = App::new();
            app.add_plugins(bevy::input::InputPlugin);
            let ids: Vec<Entity> = offered.iter().map(|_| app.world_mut().spawn_empty().id()).collect();
            for (&id, &(vendor_id, product_id)) in ids.iter().zip(offered) {
                let connection = GamepadConnection::Connected { name: "pad".into(), vendor_id, product_id };
                app.world_mut().write_message(GamepadConnectionEvent::new(id, connection));
            }
            app.update();
            for &id in &ids {
                for (button, value) in [
                    (GamepadButton::LeftTrigger2, 0.0),
                    (GamepadButton::RightTrigger2, 0.0),
                    (GamepadButton::South, 1.0),
                    (GamepadButton::South, 0.0),
                ] {
                    let event = RawGamepadButtonChangedEvent::new(id, button, value);
                    app.world_mut().write_message(RawGamepadEvent::Button(event));
                }
            }
            app.update();
            let mut slots = PadSlots { synced: true, ..Default::default() };
            let devices: Vec<(Entity, bool, bool)> = ids.iter().map(|&id| (id, true, false)).collect();
            slots.update_with(&devices);
            app.world_mut()
                .run_system_once(move |keys: Res<ButtonInput<KeyCode>>, pads: Query<(Entity, &Gamepad)>| {
                    let mut sticks = StickDpad::default();
                    let mut presses = pad_presses(&keys, &pads, &slots, &mut sticks, false);
                    presses.sort_by_key(|&(_, device)| device);
                    let held = pads.iter().any(|(_, pad)| {
                        pad.pressed(GamepadButton::LeftTrigger2)
                            || pad.pressed(GamepadButton::RightTrigger2)
                            || pad.get(GamepadButton::LeftTrigger2).unwrap_or(0.0) > 0.0
                            || pad.get(GamepadButton::RightTrigger2).unwrap_or(0.0) > 0.0
                    });
                    (presses, sticks.take(), held)
                })
                .expect("the pads read")
        };
        let (steam, dualsense) = ((Some(0x28de), Some(0x11ff)), (Some(0x054c), Some(0x0ce6)));
        let (presses, trigger_presses, held) = tap(&[steam, dualsense]);
        assert_eq!(presses, [(Button::DigitalOne, 0), (Button::DigitalOne, 1)], "both pads offered: two presses");
        assert_eq!((trigger_presses, held), (vec![], false));
        let (presses, trigger_presses, held) = tap(&[steam]);
        assert_eq!(presses, [(Button::DigitalOne, 0)], "Steam's pad alone: one press");
        assert_eq!((trigger_presses, held), (vec![], false), "the triggers at rest are released");
    }

    #[test]
    fn the_left_sticks_d_pad_is_kept_for_the_movies() {
        let (presses, mut sticks) = read_pad(0.9, false);
        assert_eq!(presses, []);
        assert_eq!(sticks.take(), [(Button::DpadRight, 0)]);
        assert_eq!(sticks.take(), []);
    }
}
