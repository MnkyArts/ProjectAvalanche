use bevy::prelude::*;
use ssx_physics::rewind::RewindPad;
use ssx_physics::rider::{Input, TrickInput};

use super::{L2_HOLD_MIN, Ride, TRIGGER_THRESHOLD};
use crate::camera::CameraMode;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct PadLock {
    prev: bool,
    seen: bool,
    at_pause: bool,
    locked: bool,
}

impl PadLock {
    fn pause(&mut self) {
        self.at_pause = self.seen;
    }

    fn resume(&mut self) {
        self.locked = !self.at_pause;
        self.seen &= !self.locked;
    }

    fn update(&mut self, held: bool) -> bool {
        if held && !self.prev {
            self.locked = false;
        }
        self.prev = held;
        self.seen = held && !self.locked;
        self.seen
    }
}

impl Ride {
    pub fn pause(&mut self) {
        self.hold = true;
        self.paused = true;
        self.a_lock.pause();
        self.session.pause_begin();
    }

    pub fn resume(&mut self) {
        self.hold = false;
        self.paused = false;
        self.session.pause_end();
        self.a_lock.resume();
        if !self.a_lock.seen {
            (self.input.jump, self.tricks.a) = (false, false);
        }
        if self.a_lock.locked {
            info!("Ride step {}: the pause ends, A is locked until its next press", self.steps + 1);
        }
    }

    pub fn hold_pads(&mut self, on: bool) {
        self.pads_held = on;
        if on {
            self.a_lock.pause();
        } else {
            self.a_lock.resume();
            if !self.a_lock.seen {
                (self.input.jump, self.tricks.a) = (false, false);
            }
        }
    }

    pub fn hold_for_chat(&mut self, on: bool) {
        if self.chat_held == on {
            return;
        }
        self.chat_held = on;
        if on {
            self.a_lock.pause();
        } else {
            self.a_lock.resume();
            if !self.a_lock.seen {
                (self.input.jump, self.tricks.a) = (false, false);
            }
        }
    }

    fn take_live_controls(&mut self, mut input: Input, mut tricks: TrickInput) {
        if !self.a_lock.update(input.jump || tricks.a) {
            (input.jump, tricks.a) = (false, false);
        }
        (self.input, self.tricks) = (input, tricks);
    }

    pub(super) fn locked_controls(&mut self) -> (Input, TrickInput) {
        let (mut input, mut tricks) = (self.input, self.tricks);
        if self.scripted() && !self.a_lock.update(input.jump || tricks.a) {
            (input.jump, tricks.a) = (false, false);
        }
        (input, tricks)
    }
}

pub(super) fn read_controls(
    keys: Res<ButtonInput<KeyCode>>,
    gamepads: Query<&Gamepad>,
    mode: Res<CameraMode>,
    mut ride: ResMut<Ride>,
) {
    if keys.just_pressed(KeyCode::KeyR) && ride.viewer_keys {
        ride.respawn();
    }
    if keys.just_pressed(KeyCode::KeyN) && ride.viewer_keys {
        ride.rail_debug = !ride.rail_debug;
    }
    if ride.scripted() {
        return;
    }
    read_live_controls(&mut ride, &keys, &gamepads, *mode == CameraMode::Chase);
}

fn enter_held(keys: &ButtonInput<KeyCode>) -> bool {
    keys.pressed(KeyCode::Enter) && !crate::shell::game_mode::alt_held(keys)
}

#[derive(Clone, Copy, Debug, Default)]
pub(super) struct LiveRead {
    pub input: Input,
    pub tricks: TrickInput,
    pub l2: f32,
    pub r1: bool,
    pub rewind: bool,
}

pub(super) fn live_read(
    keys: &ButtonInput<KeyCode>,
    gamepads: &Query<&Gamepad>,
    chase: bool,
    classic: bool,
    nis: bool,
) -> LiveRead {
    let rewind = keys.pressed(KeyCode::Backspace)
        || gamepads.iter().any(|p| p.pressed(if classic { GamepadButton::East } else { GamepadButton::LeftTrigger }));
    let mut input = live_input(keys, gamepads, chase, classic);
    input.jump |= nis && enter_held(keys);
    let tricks = live_tricks(keys, gamepads, chase, input.jump, classic);
    let (l2, r1) = (live_l2_travel(keys, gamepads, chase, classic), live_r1(keys, gamepads, chase));
    LiveRead { input, tricks, l2, r1, rewind }
}

pub fn read_live_controls(ride: &mut Ride, keys: &ButtonInput<KeyCode>, gamepads: &Query<&Gamepad>, chase: bool) {
    let classic = ride.classic;
    let released = ride.pads_held || ride.chat_held;
    ride.back_held =
        !released && (keys.pressed(KeyCode::KeyC) || gamepads.iter().any(|p| p.pressed(GamepadButton::Select)));
    let live = live_read(keys, gamepads, chase, classic, ride.state.start.is_some());
    (ride.l2, ride.l2_hold) = (live.l2 >= TRIGGER_THRESHOLD, live.l2 >= L2_HOLD_MIN);
    ride.r1 = live.r1;
    let pads = gamepads.iter().map(|p| p.get(GamepadButton::RightTrigger2).unwrap_or(0.0)).fold(0.0, f32::max);
    ride.r2 = if live.tricks.tweak && pads < TRIGGER_THRESHOLD { 1.0 } else { pads };
    ride.take_live_controls(live.input, live.tricks);
    let takeover = TakeoverPad::of(&ride.input, &ride.tricks, live.l2, live.r1, classic).takeover();
    ride.rewind_pad = RewindPad { rewind_held: live.rewind, takeover, ..RewindPad::default() };
}

pub(super) fn late_controls(
    keys: Res<ButtonInput<KeyCode>>,
    gamepads: Query<&Gamepad>,
    mode: Res<CameraMode>,
    mut ride: ResMut<Ride>,
) {
    if ride.scripted() {
        return;
    }
    let classic = ride.classic;
    let mut input = live_input(&keys, &gamepads, *mode == CameraMode::Chase, classic);
    input.jump |= ride.state.start.is_some() && enter_held(&keys);
    let a = input.jump || live_tricks(&keys, &gamepads, *mode == CameraMode::Chase, input.jump, classic).a;
    if a != ride.a_lock.prev {
        ride.a_lock.update(a);
    }
}

pub(super) fn numpad_stick(keys: &ButtonInput<KeyCode>) -> Vec2 {
    let held = |k: KeyCode| f32::from(u8::from(keys.pressed(k)));
    Vec2::new(held(KeyCode::Numpad6) - held(KeyCode::Numpad4), held(KeyCode::Numpad8) - held(KeyCode::Numpad2))
        .normalize_or_zero()
}

type StickSector = (f32, f32, f32);

const fn sectors(up: f32, left: f32, down: f32, right: f32) -> [StickSector; 4] {
    use std::f32::consts::FRAC_PI_4;
    [
        (FRAC_PI_4, 3.0 * FRAC_PI_4, up),
        (3.0 * FRAC_PI_4, 5.0 * FRAC_PI_4, left),
        (5.0 * FRAC_PI_4, 7.0 * FRAC_PI_4, down),
        (7.0 * FRAC_PI_4, FRAC_PI_4, right),
    ]
}

const LEFT_STICK_TAKEOVER: [StickSector; 4] = sectors(0.57, 0.47, 0.47, 0.46);
const RIGHT_STICK_TAKEOVER: [StickSector; 4] = sectors(0.54, 0.49, 0.49, 0.52);

fn stick_matches(s: Vec2, sectors: &[StickSector; 4]) -> bool {
    let angle = s.y.atan2(s.x).rem_euclid(std::f32::consts::TAU);
    let magnitude = s.length().min(1.0);
    sectors.iter().any(|&(min, max, at_least)| {
        let inside = if min == max {
            true
        } else if min < max {
            (min..=max).contains(&angle)
        } else {
            angle >= min || angle <= max
        };
        inside && magnitude >= at_least
    })
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct TakeoverPad {
    pub x: bool,
    pub y: bool,
    pub a: bool,
    pub r1: bool,
    pub r2: bool,
    pub l2: f32,
    pub left_stick: Vec2,
    pub right_stick: Vec2,
    pub other_rewind: bool,
}

impl TakeoverPad {
    pub(super) fn of(input: &Input, tricks: &TrickInput, l2: f32, r1: bool, classic: bool) -> Self {
        TakeoverPad {
            x: tricks.x,
            y: tricks.y,
            a: tricks.a || input.jump,
            r1: if classic { tricks.rb } else { r1 },
            r2: if classic { tricks.rt } else { tricks.tweak },
            l2,
            left_stick: input.stick(),
            right_stick: tricks.right_stick,
            other_rewind: if classic { tricks.lb } else { tricks.b },
        }
    }

    pub(super) fn takeover(&self) -> bool {
        self.x
            || self.y
            || self.a
            || self.r1
            || self.r2
            || self.l2 >= L2_HOLD_MIN
            || stick_matches(self.left_stick, &LEFT_STICK_TAKEOVER)
            || stick_matches(self.right_stick, &RIGHT_STICK_TAKEOVER)
            || self.other_rewind
    }
}

pub(super) fn live_input(keys: &ButtonInput<KeyCode>, gamepads: &Query<&Gamepad>, wasd: bool, classic: bool) -> Input {
    let held =
        |arrow: KeyCode, letter: KeyCode| f32::from(u8::from(keys.pressed(arrow) || (wasd && keys.pressed(letter))));
    let arrow_x = held(KeyCode::ArrowRight, KeyCode::KeyD) - held(KeyCode::ArrowLeft, KeyCode::KeyA);
    let arrow_y = held(KeyCode::ArrowUp, KeyCode::KeyW) - held(KeyCode::ArrowDown, KeyCode::KeyS);
    // PORT(choice): the arrows / WASD are the keyboard's left stick in both schemes and, under the classic scheme, also
    // its D-pad (one keyboard cannot be two sticks; the carve sums the two, which for ±1 keys clamps to the same full
    // deflection a pad reaches); a second keyboard binding for the classic D-pad, if the two must be separate; the
    // console reads pads only.
    let mut dpad = if classic { Vec2::new(arrow_x, arrow_y) } else { Vec2::ZERO };
    let mut dpad_held = classic && (arrow_x != 0.0 || arrow_y != 0.0);
    let (mut steer, mut tuck, mut brake) = (arrow_x, arrow_y.max(0.0), (-arrow_y).max(0.0));
    let mut jump = keys.pressed(KeyCode::Space);
    for pad in gamepads {
        let stick = pad.left_stick();
        steer = (steer + stick.x).clamp(-1.0, 1.0);
        tuck = tuck.max(stick.y);
        brake = brake.max(-stick.y);
        if classic {
            let left = f32::from(u8::from(pad.pressed(GamepadButton::DPadLeft)));
            let right = f32::from(u8::from(pad.pressed(GamepadButton::DPadRight)));
            let up = f32::from(u8::from(pad.pressed(GamepadButton::DPadUp)));
            let down = f32::from(u8::from(pad.pressed(GamepadButton::DPadDown)));
            dpad += Vec2::new(right - left, up - down);
            dpad_held |= left + right + up + down > 0.0;
        }
        jump |= pad.pressed(GamepadButton::South);
    }
    Input { steer, brake, tuck, dpad: dpad.clamp(Vec2::splat(-1.0), Vec2::splat(1.0)), dpad_held, jump }
}

fn live_l2_travel(keys: &ButtonInput<KeyCode>, gamepads: &Query<&Gamepad>, chase: bool, classic: bool) -> f32 {
    let key = if classic {
        keys.pressed(KeyCode::KeyZ)
    } else {
        chase && keys.any_pressed([KeyCode::ControlLeft, KeyCode::ControlRight])
    };
    let pads = gamepads.iter().map(|p| {
        let pressed = f32::from(u8::from(p.pressed(GamepadButton::LeftTrigger2)));
        p.get(GamepadButton::LeftTrigger2).unwrap_or(0.0).max(pressed)
    });
    if key { 1.0 } else { pads.fold(0.0, f32::max).clamp(0.0, 1.0) }
}

fn live_r1(keys: &ButtonInput<KeyCode>, gamepads: &Query<&Gamepad>, chase: bool) -> bool {
    (chase && keys.pressed(KeyCode::KeyE)) || gamepads.iter().any(|p| p.pressed(GamepadButton::RightTrigger))
}

pub(super) fn live_tricks(
    keys: &ButtonInput<KeyCode>,
    gamepads: &Query<&Gamepad>,
    chase: bool,
    jump: bool,
    classic: bool,
) -> TrickInput {
    let shift = chase && keys.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]);
    let ctrl = chase && keys.any_pressed([KeyCode::ControlLeft, KeyCode::ControlRight]);
    if classic {
        let mut t = TrickInput {
            right_stick: numpad_stick(keys),
            y: chase && keys.any_pressed([KeyCode::ControlLeft, KeyCode::ControlRight]),
            x: keys.pressed(KeyCode::KeyJ),
            b: keys.pressed(KeyCode::KeyL),
            a: jump,
            lb: keys.pressed(KeyCode::KeyQ),
            rb: keys.pressed(KeyCode::KeyE),
            lt: keys.pressed(KeyCode::KeyZ),
            rt: keys.pressed(KeyCode::KeyX),
            tweak: keys.pressed(KeyCode::KeyJ),
            signature: shift && ctrl,
        };
        for pad in gamepads {
            let stick = pad.right_stick();
            if stick.length() > t.right_stick.length() {
                t.right_stick = stick;
            }
            let west = pad.pressed(GamepadButton::West);
            let east = pad.pressed(GamepadButton::East);
            let north = pad.pressed(GamepadButton::North);
            let south = pad.pressed(GamepadButton::South);
            let lb = pad.pressed(GamepadButton::LeftTrigger);
            let rb = pad.pressed(GamepadButton::RightTrigger);
            let lt = pad.get(GamepadButton::LeftTrigger2).unwrap_or(0.0) >= TRIGGER_THRESHOLD
                || pad.pressed(GamepadButton::LeftTrigger2);
            let rt = pad.get(GamepadButton::RightTrigger2).unwrap_or(0.0) >= TRIGGER_THRESHOLD
                || pad.pressed(GamepadButton::RightTrigger2);
            t.x |= west;
            t.tweak |= west;
            t.y |= north;
            t.b |= east;
            t.a |= south;
            (t.lb, t.rb, t.lt, t.rt) = (t.lb || lb, t.rb || rb, t.lt || lt, t.rt || rt);
            t.signature |= lb && rb && lt && rt;
        }
        return t;
    }
    let mut t = TrickInput {
        right_stick: numpad_stick(keys),
        y: keys.pressed(KeyCode::KeyI),
        x: keys.pressed(KeyCode::KeyJ),
        b: keys.pressed(KeyCode::KeyL),
        a: keys.pressed(KeyCode::KeyK) || jump,
        tweak: shift,
        signature: shift && ctrl,
        ..TrickInput::default()
    };
    for pad in gamepads {
        let stick = pad.right_stick();
        if stick.length() > t.right_stick.length() {
            t.right_stick = stick;
        }
        let trigger = |b: GamepadButton| pad.get(b).unwrap_or(0.0) >= TRIGGER_THRESHOLD || pad.pressed(b);
        let (l2, r2) = (trigger(GamepadButton::LeftTrigger2), trigger(GamepadButton::RightTrigger2));
        t.y |= pad.pressed(GamepadButton::North);
        t.x |= pad.pressed(GamepadButton::West);
        t.b |= pad.pressed(GamepadButton::East);
        t.tweak |= r2;
        t.signature |= l2 && r2;
    }
    t
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::input::gamepad::GamepadAxis;

    fn live_buttons(
        classic: bool,
        buttons: &[(GamepadButton, f32)],
        axes: &[(GamepadAxis, f32)],
    ) -> (bool, TrickInput) {
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
        for &(button, value) in buttons {
            app.world_mut()
                .write_message(RawGamepadEvent::Button(RawGamepadButtonChangedEvent::new(id, button, value)));
        }
        for &(axis, value) in axes {
            app.world_mut().write_message(RawGamepadEvent::Axis(RawGamepadAxisChangedEvent::new(id, axis, value)));
        }
        app.update();
        app.world_mut()
            .run_system_once(move |keys: Res<ButtonInput<KeyCode>>, pads: Query<&Gamepad>| {
                let grind = live_l2_travel(&keys, &pads, false, classic) >= TRIGGER_THRESHOLD;
                (grind, live_tricks(&keys, &pads, false, false, classic))
            })
            .expect("the live trigger and trick systems")
    }

    #[test]
    fn the_standard_layout_reads_independent_analog_triggers() {
        for (lt, rt, expected_grind, expected_tweak, expected_signature) in [
            (0.0, 0.0, false, false, false),
            (0.49, 0.49, false, false, false),
            (0.5, 0.49, true, false, false),
            (0.49, 0.5, false, true, false),
            (0.5, 0.5, true, true, true),
            (1.0, 0.0, true, false, false),
            (0.0, 1.0, false, true, false),
            (1.0, 1.0, true, true, true),
        ] {
            let (grind, tricks) =
                live_buttons(false, &[(GamepadButton::LeftTrigger2, lt), (GamepadButton::RightTrigger2, rt)], &[]);
            assert_eq!(
                (grind, tricks.tweak, tricks.signature),
                (expected_grind, expected_tweak, expected_signature),
                "LT={lt}, RT={rt}"
            );
            assert_eq!(
                (tricks.x, tricks.y, tricks.lb, tricks.rb, tricks.lt, tricks.rt),
                (false, false, false, false, false, false)
            );
        }
        let (grind, tricks) =
            live_buttons(false, &[(GamepadButton::LeftTrigger, 1.0), (GamepadButton::RightTrigger, 1.0)], &[]);
        assert!(!grind && !tricks.tweak && !tricks.signature, "the bumpers are not the analog triggers");
    }

    #[test]
    fn the_classic_layout_keeps_all_four_shoulder_channels_and_face_roles() {
        for mask in 0u8..16 {
            let held = [mask & 1 != 0, mask & 2 != 0, mask & 4 != 0, mask & 8 != 0];
            let (_, tricks) = live_buttons(
                true,
                &[
                    (GamepadButton::LeftTrigger, if held[0] { 1.0 } else { 0.0 }),
                    (GamepadButton::RightTrigger, if held[1] { 1.0 } else { 0.0 }),
                    (GamepadButton::LeftTrigger2, if held[2] { 0.5 } else { 0.49 }),
                    (GamepadButton::RightTrigger2, if held[3] { 0.5 } else { 0.49 }),
                ],
                &[],
            );
            assert_eq!([tricks.lb, tricks.rb, tricks.lt, tricks.rt], held, "shoulder mask {mask:04b}");
            assert_eq!(tricks.signature, mask == 15, "all four shoulders are required, mask {mask:04b}");
            assert!(!tricks.tweak && !tricks.x && !tricks.y && !tricks.b && !tricks.a);
        }
        let (_, x) = live_buttons(true, &[(GamepadButton::West, 1.0)], &[]);
        assert!(x.x && x.tweak, "Classic X remains the boost/tweak input");
        assert!(!x.y && !x.signature && !x.lt && !x.rt);
        let (_, y) = live_buttons(true, &[(GamepadButton::North, 1.0)], &[]);
        assert!(y.y, "Classic Y remains the grind input");
        assert!(!y.x && !y.tweak && !y.signature && !y.lt && !y.rt);
    }

    #[test]
    fn unassigned_z_axes_are_not_globally_reinterpreted_as_triggers() {
        for classic in [false, true] {
            let (grind, tricks) = live_buttons(classic, &[], &[(GamepadAxis::LeftZ, 1.0), (GamepadAxis::RightZ, 1.0)]);
            assert!(!grind, "an unrelated Z axis is not the grind button, Classic={classic}");
            assert_eq!(tricks, TrickInput::default(), "Z axes need a device mapping, Classic={classic}");
        }
    }

    fn live(classic: bool, stick: Vec2, (right, up): (bool, bool)) -> Input {
        use bevy::ecs::system::RunSystemOnce;
        use bevy::input::gamepad::{
            GamepadAxis, GamepadButton, GamepadConnection, GamepadConnectionEvent, RawGamepadAxisChangedEvent,
            RawGamepadButtonChangedEvent, RawGamepadEvent,
        };
        let mut app = App::new();
        app.add_plugins(bevy::input::InputPlugin);
        let id = app.world_mut().spawn_empty().id();
        app.world_mut().write_message(GamepadConnectionEvent::new(
            id,
            GamepadConnection::Connected { name: "test pad".into(), vendor_id: None, product_id: None },
        ));
        app.update();
        let world = app.world_mut();
        world.write_message(RawGamepadEvent::Axis(RawGamepadAxisChangedEvent::new(
            id,
            GamepadAxis::LeftStickX,
            stick.x,
        )));
        world.write_message(RawGamepadEvent::Axis(RawGamepadAxisChangedEvent::new(
            id,
            GamepadAxis::LeftStickY,
            stick.y,
        )));
        if right {
            world.write_message(RawGamepadEvent::Button(RawGamepadButtonChangedEvent::new(
                id,
                GamepadButton::DPadRight,
                1.0,
            )));
        }
        if up {
            world.write_message(RawGamepadEvent::Button(RawGamepadButtonChangedEvent::new(
                id,
                GamepadButton::DPadUp,
                1.0,
            )));
        }
        app.update();
        app.world_mut()
            .run_system_once(move |keys: Res<ButtonInput<KeyCode>>, pads: Query<&Gamepad>| {
                live_input(&keys, &pads, false, classic)
            })
            .expect("the live input system")
    }

    #[test]
    fn the_classic_live_input_reads_the_left_stick_and_the_dpad() {
        let stick = Vec2::new(0.5, -0.25);
        let standard = live(false, stick, (false, false));
        assert_eq!((standard.steer, standard.tuck, standard.brake), (0.5, 0.0, 0.25));
        assert_eq!(standard.dpad, Vec2::ZERO);
        let classic = live(true, stick, (true, true));
        assert_eq!((classic.steer, classic.tuck, classic.brake), (0.5, 0.0, 0.25), "the left stick still carves");
        assert_eq!(classic.dpad, Vec2::ONE, "the D-pad is the classic air input");
        assert!(classic.dpad_held, "BLN_Pad_DPad*_Hold: the classic prewind's condition");
        assert_eq!(live(true, stick, (false, false)).dpad, Vec2::ZERO);
        assert!(!live(true, stick, (false, false)).dpad_held);
        assert_eq!(live(false, stick, (true, true)).dpad, Vec2::ZERO, "the standard scheme has no D-pad input");
    }

    #[test]
    fn the_keyboard_is_the_left_stick_and_the_classic_dpad() {
        use bevy::ecs::system::RunSystemOnce;
        let mut app = App::new();
        app.add_plugins(bevy::input::InputPlugin);
        let mut keys = ButtonInput::<KeyCode>::default();
        keys.press(KeyCode::ArrowUp);
        keys.press(KeyCode::ArrowRight);
        app.world_mut().insert_resource(keys);
        let read = |app: &mut App, classic: bool| {
            app.world_mut()
                .run_system_once(move |keys: Res<ButtonInput<KeyCode>>, pads: Query<&Gamepad>| {
                    live_input(&keys, &pads, false, classic)
                })
                .expect("the live input system")
        };
        let classic = read(&mut app, true);
        assert_eq!((classic.steer, classic.tuck, classic.brake), (1.0, 1.0, 0.0), "the left stick");
        assert_eq!(classic.dpad, Vec2::ONE, "the classic D-pad: the air's stick and the carve's second stick");
        let standard = read(&mut app, false);
        assert_eq!((standard.steer, standard.tuck, standard.brake), (1.0, 1.0, 0.0));
        assert_eq!(standard.dpad, Vec2::ZERO, "the standard scheme has no D-pad input");
    }

    #[test]
    fn the_numpad_is_the_keyboards_right_stick() {
        use bevy::ecs::system::RunSystemOnce;
        let read = |pressed: &'static [KeyCode], classic: bool| {
            let mut app = App::new();
            app.add_plugins(bevy::input::InputPlugin);
            let mut keys = ButtonInput::<KeyCode>::default();
            for &k in pressed {
                keys.press(k);
            }
            app.world_mut().insert_resource(keys);
            app.world_mut()
                .run_system_once(move |keys: Res<ButtonInput<KeyCode>>, pads: Query<&Gamepad>| {
                    let r = live_read(&keys, &pads, true, classic, false);
                    (r.tricks.right_stick, TakeoverPad::of(&r.input, &r.tricks, r.l2, r.r1, classic).takeover())
                })
                .expect("the live tricks system")
        };
        for classic in [false, true] {
            assert_eq!(read(&[KeyCode::Numpad8], classic), (Vec2::Y, true), "up");
            assert_eq!(read(&[KeyCode::Numpad2], classic), (Vec2::NEG_Y, true), "down: the helicopter's prepare");
            assert_eq!(read(&[KeyCode::Numpad4], classic), (Vec2::NEG_X, true), "left");
            assert_eq!(read(&[KeyCode::Numpad6], classic), (Vec2::X, true), "right");
            let (d, takeover) = read(&[KeyCode::Numpad6, KeyCode::Numpad2], classic);
            assert!((d - Vec2::new(1.0, -1.0).normalize()).length() < 1e-6 && takeover, "a diagonal, magnitude 1");
            assert_eq!(read(&[KeyCode::Numpad4, KeyCode::Numpad6], classic), (Vec2::ZERO, false), "opposites cancel");
            assert_eq!(read(&[KeyCode::ArrowUp], classic).0, Vec2::ZERO, "the arrows are the left stick");
        }
    }

    fn takeover_of(
        classic: bool,
        keys: &[KeyCode],
        buttons: &[(GamepadButton, f32)],
        axes: &[(GamepadAxis, f32)],
    ) -> bool {
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
        for &(button, value) in buttons {
            app.world_mut()
                .write_message(RawGamepadEvent::Button(RawGamepadButtonChangedEvent::new(id, button, value)));
        }
        for &(axis, value) in axes {
            app.world_mut().write_message(RawGamepadEvent::Axis(RawGamepadAxisChangedEvent::new(id, axis, value)));
        }
        app.update();
        let mut held = ButtonInput::<KeyCode>::default();
        for &k in keys {
            held.press(k);
        }
        app.world_mut().insert_resource(held);
        app.world_mut()
            .run_system_once(move |keys: Res<ButtonInput<KeyCode>>, pads: Query<&Gamepad>| {
                let r = live_read(&keys, &pads, true, classic, false);
                TakeoverPad::of(&r.input, &r.tricks, r.l2, r.r1, classic).takeover()
            })
            .expect("the live read")
    }

    #[test]
    fn the_keyboard_takes_over_as_the_pads_buttons_do() {
        use GamepadButton as P;
        use KeyCode as K;
        let none: &[(GamepadAxis, f32)] = &[];
        let standard_keys = [
            (K::KeyJ, "X"),
            (K::KeyI, "Y"),
            (K::KeyK, "A"),
            (K::Space, "A (the jump)"),
            (K::KeyE, "R1"),
            (K::ShiftLeft, "R2"),
            (K::ControlLeft, "L2"),
            (K::KeyL, "B: BLN_Pad_Classic_Rewind"),
            (K::ArrowUp, "left stick up"),
            (K::ArrowLeft, "left stick left"),
            (K::ArrowDown, "left stick down"),
            (K::ArrowRight, "left stick right"),
            (K::Numpad8, "right stick up"),
            (K::Numpad2, "right stick down"),
        ];
        for (key, what) in standard_keys {
            assert!(takeover_of(false, &[key], &[], none), "standard: {key:?} = {what}");
        }
        for key in [K::Backspace, K::KeyC, K::KeyQ, K::KeyZ, K::KeyX, K::KeyR, K::Enter] {
            assert!(!takeover_of(false, &[key], &[], none), "standard: {key:?} is none of the takeover's inputs");
        }
        let standard_pad = [(P::West, true), (P::North, true), (P::South, true), (P::RightTrigger, true)]
            .into_iter()
            .chain([(P::East, true), (P::LeftTrigger, false), (P::Select, false), (P::Start, false)]);
        for (button, takes_over) in standard_pad {
            assert_eq!(takeover_of(false, &[], &[(button, 1.0)], none), takes_over, "standard pad {button:?}");
        }
        let classic_keys = [
            (K::ControlLeft, "Y"),
            (K::KeyJ, "X"),
            (K::Space, "A"),
            (K::KeyE, "R1 (RB)"),
            (K::KeyX, "R2 (RT)"),
            (K::KeyZ, "L2 (LT)"),
            (K::KeyQ, "L1 (LB): BLN_Pad_Rewind"),
            (K::ArrowUp, "left stick up"),
            (K::Numpad4, "right stick left"),
        ];
        for (key, what) in classic_keys {
            assert!(takeover_of(true, &[key], &[], none), "classic: {key:?} = {what}");
        }
        for key in [K::KeyL, K::Backspace, K::KeyI, K::KeyK, K::ShiftLeft, K::KeyC] {
            assert!(!takeover_of(true, &[key], &[], none), "classic: {key:?} is none of the takeover's inputs");
        }
        for (button, takes_over) in [(P::East, false), (P::LeftTrigger, true), (P::North, true), (P::South, true)] {
            assert_eq!(takeover_of(true, &[], &[(button, 1.0)], none), takes_over, "classic pad {button:?}");
        }
        for classic in [false, true] {
            for (button, value, takes_over) in
                [(P::LeftTrigger2, 0.2, true), (P::LeftTrigger2, 0.19, false), (P::RightTrigger2, 0.5, true)]
                    .into_iter()
                    .chain([(P::RightTrigger2, 0.49, false)])
            {
                let got = takeover_of(classic, &[], &[(button, value)], none);
                assert_eq!(got, takes_over, "classic={classic}: {button:?} at {value}");
            }
        }
    }

    #[test]
    fn the_takeovers_stick_sectors_are_the_matchers() {
        use std::f32::consts::FRAC_PI_4;
        let at = |angle: f32, m: f32| Vec2::new(angle.cos(), angle.sin()) * m;
        let left = |s: Vec2| TakeoverPad { left_stick: s, ..TakeoverPad::default() }.takeover();
        let right = |s: Vec2| TakeoverPad { right_stick: s, ..TakeoverPad::default() }.takeover();
        for (dir, l, r) in [(2.0, 0.57, 0.54), (4.0, 0.47, 0.49), (6.0, 0.47, 0.49), (0.0, 0.46, 0.52)] {
            let angle = dir * FRAC_PI_4;
            assert!(left(at(angle, l)) && !left(at(angle, l - 0.01)), "left stick at {dir}·45°");
            assert!(right(at(angle, r)) && !right(at(angle, r - 0.01)), "right stick at {dir}·45°");
        }
        assert!(!left(at(FRAC_PI_4 + 0.01, 0.5)) && left(at(FRAC_PI_4 - 0.01, 0.5)));
        assert!(stick_matches(Vec2::new(-0.5, 0.0), &[(1.0, 1.0, 0.4); 4]), "min = max: any angle");
        assert!(stick_matches(Vec2::new(0.5, -0.01), &sectors(1.1, 1.1, 1.1, 0.46)), "the right sector wraps 0");
        assert!(!left(Vec2::ZERO) && !right(Vec2::ZERO), "a centred stick");
        assert!(left(Vec2::new(-3.0, 0.0)), "the magnitude is min(|s|, 1)");
        let pad = TakeoverPad::default();
        assert!(
            !pad.takeover() && TakeoverPad { l2: 0.2, ..pad }.takeover() && !TakeoverPad { l2: 0.19, ..pad }.takeover()
        );
    }

    #[test]
    fn the_pause_locks_a_button_pressed_during_it() {
        let mut l = PadLock::default();
        assert!(!l.update(false) && l.update(true) && l.update(true), "no pause: A as given");
        assert!(!l.update(false));
        l.pause();
        assert!(l.update(true), "the popup's A, read during the pause: its edge is spent here");
        l.resume();
        assert!(!l.seen, "out of the held word at the pause's end");
        assert!(!l.update(true) && !l.update(true), "held through the end: locked");
        assert!(!l.update(false));
        assert!(l.update(true), "the next press edge is seen");
        let mut held = PadLock::default();
        assert!(held.update(true));
        held.pause();
        held.resume();
        assert!(held.seen && held.update(true), "held when the pause began: not locked");
        let mut late = PadLock::default();
        late.pause();
        late.resume();
        assert!(!late.update(false));
        assert!(late.update(true), "first read after the end: seen at once");
    }

    fn popup_answer(hold: f64, fps: f64, injected: bool, fe_frames: u32, late: bool) -> (Vec<u32>, bool) {
        #[derive(Clone, Copy, Default)]
        struct Key {
            down: bool,
            just: bool,
        }
        let set = |k: &mut Key, down: bool| {
            (k.just, k.down) = (k.just || (down && !k.down), down);
        };
        let (press, release) = (1.0, 1.0 + hold);
        let (again, end) = (release + 0.5, release + 1.0);
        let want = |t: f64| (press..release).contains(&t) || (again..end).contains(&t);
        let (mut lock, mut key) = (PadLock::default(), Key::default());
        let (mut popup, mut held, mut answered) = (true, false, None::<u32>);
        let (mut early, mut second, mut steps, mut carry) = (Vec::new(), false, 0u32, 0.0);
        for frame in 0.. {
            let t = f64::from(frame) / fps;
            if t > end + 0.2 {
                break;
            }
            key.just = false;
            if !injected {
                set(&mut key, want(t));
            }
            if frame == 2 {
                lock.pause();
                held = true;
            }
            if held && !popup {
                lock.resume();
                held = false;
            }
            carry += 1.0 / fps;
            while carry >= 1.0 / 30.0 {
                carry -= 1.0 / 30.0;
                if !held {
                    steps += 1;
                    if lock.seen && t < again {
                        early.push(steps);
                    }
                    second |= lock.seen && t >= again;
                }
            }
            lock.update(key.down);
            if injected {
                set(&mut key, want(t));
            }
            if popup && key.just {
                answered.get_or_insert(frame + fe_frames);
            }
            popup &= answered.is_none_or(|f| frame < f);
            if late && key.down != lock.prev {
                lock.update(key.down);
            }
        }
        assert!(!popup && !held && steps > 30, "the popup was answered and the ride went on");
        (early, second)
    }

    #[test]
    fn the_popups_answer_does_not_reach_the_ride() {
        for hold in [0.02, 0.1, 0.6] {
            for fps in [20.0, 30.0, 60.0, 75.0, 144.0, 300.0] {
                for (injected, fe_frames) in [(false, 0), (false, 1), (false, 3), (true, 0), (true, 1), (true, 3)] {
                    let (early, second) = popup_answer(hold, fps, injected, fe_frames, true);
                    let case = format!("{hold} s at {fps} fps, injected {injected}, the front end {fe_frames} later");
                    assert!(early.is_empty(), "{case}: A seen on steps {early:?}");
                    assert!(second, "{case}: the second press was not seen");
                }
            }
        }
        let (early, _) = popup_answer(0.1, 60.0, true, 0, false);
        assert_eq!(early.first(), Some(&3), "one read per frame: {early:?}");
    }
}
