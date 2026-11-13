use super::*;
use crate::jump::PrewindGestures;
use crate::tricks::PadSample;

fn stick(angle: f32, mag: f32) -> PadSample {
    PadSample { angle, mag, ..PadSample::default() }
}

#[test]
fn the_flip_acceleration_is_the_prewind_stick_y() {
    let i = Input { tuck: 0.8, dpad: Vec2::new(0.0, -1.0), dpad_held: true, ..Input::default() };
    assert_eq!((i.flip_acceleration(false), i.flip_acceleration(true)), (0.8, -1.0));
    let brake = Input { brake: 0.6, ..Input::default() };
    assert_eq!((brake.flip_acceleration(false), brake.flip_acceleration(true)), (-0.6, 0.0));
}

#[test]
fn the_prewind_mappings_are_the_right_sticks_lower_hold_and_upper_enter() {
    let down = 3.0 * std::f32::consts::FRAC_PI_2;
    let up = std::f32::consts::FRAC_PI_2;
    let mut g = PrewindGestures::default();
    g.tick(stick(0.0, 0.0));
    assert!(!g.hold && !g.release);
    g.tick(stick(down, 0.9));
    assert!(g.hold && !g.release, "pulled down: the crouch");
    g.tick(stick(down, 0.9));
    assert!(g.hold && !g.release, "held: no release (it is an enter edge)");
    g.tick(stick(up, 0.8));
    assert!(!g.hold && g.release, "flicked up: the release");
    g.tick(stick(up, 0.8));
    assert!(!g.hold && !g.release, "one step only");
    g.tick(stick(down, 0.84));
    assert!(!g.hold);
    g.tick(stick(0.0, 0.0));
    g.tick(stick(up, 0.74));
    assert!(!g.release);
    g.tick(stick(0.0, 0.6));
    assert!(!g.hold && !g.release, "a weak sideways stick is neither");
}

#[test]
fn the_right_stick_flick_jumps_under_the_standard_scheme_only() {
    let w = world(plane(4000.0, 0.0));
    let mut standard = moving(&w, Vec3::new(0.0, 1.0, -3000.0), Vec3::Z, 60.0 / KMH_PER_M_PER_TICK);
    standard.jump.on_ground_ticks = 20.0;
    let down = TrickInput { right_stick: Vec2::new(0.0, -1.0), ..TrickInput::default() };
    let up = TrickInput { right_stick: Vec2::new(0.0, 1.0), ..TrickInput::default() };
    for _ in 0..10 {
        step_with(&mut standard, &w, &Input::default(), &down, DT_TICKS);
    }
    assert!(standard.prewind > 0.5, "the crouch builds FLT_Prewind: {}", standard.prewind);
    assert!(standard.jump.compression > 0.5, "and the compression: {}", standard.jump.compression);
    assert_eq!(standard.state, RideState::Riding);
    step_with(&mut standard, &w, &Input::default(), &up, DT_TICKS);
    assert_eq!(standard.state, RideState::Jump, "the up flick requests the jump");

    let mut classic = moving(&w, Vec3::new(0.0, 1.0, -2000.0), Vec3::Z, 60.0 / KMH_PER_M_PER_TICK);
    classic.tricks.classic = true;
    classic.jump.on_ground_ticks = 20.0;
    for _ in 0..10 {
        step_with(&mut classic, &w, &Input::default(), &down, DT_TICKS);
    }
    step_with(&mut classic, &w, &Input::default(), &up, DT_TICKS);
    step_with(&mut classic, &w, &Input::default(), &up, DT_TICKS);
    assert!(classic.state != RideState::Jump, "the classic scheme's up flick is its nose press: {:?}", classic.state);
}

#[test]
fn the_classic_prewind_needs_a_and_a_dpad_button() {
    let w = world(plane(4000.0, 0.0));
    let ride = |x: f32| {
        let mut s = moving(&w, Vec3::new(x, 1.0, -3000.0), Vec3::Z, 60.0 / KMH_PER_M_PER_TICK);
        s.tricks.classic = true;
        s
    };
    let a = Input { jump: true, ..Input::default() };
    let mut a_only = ride(0.0);
    for _ in 0..10 {
        step_with(&mut a_only, &w, &a, &TrickInput::default(), DT_TICKS);
    }
    assert_eq!(a_only.prewind, 0.0, "no D-pad button: FLT_Prewind stays at 0");
    assert!(a_only.prewind_analog > 0.5, "FLT_PrewindAnalog builds by the standard filter: {}", a_only.prewind_analog);

    let mut with_dpad = ride(100.0);
    let a_dpad = Input { dpad_held: true, ..a };
    for _ in 0..10 {
        step_with(&mut with_dpad, &w, &a_dpad, &TrickInput::default(), DT_TICKS);
    }
    assert!(with_dpad.prewind > 0.5, "A and a D-pad button: {}", with_dpad.prewind);
    assert!((with_dpad.prewind - with_dpad.prewind_analog).abs() < 1e-6, "both at the accumulation rate");

    let mut crouch = ride(200.0);
    let down = TrickInput { right_stick: Vec2::new(0.0, -1.0), ..TrickInput::default() };
    for _ in 0..10 {
        step_with(&mut crouch, &w, &Input { dpad_held: true, ..Input::default() }, &down, DT_TICKS);
    }
    assert_eq!((crouch.prewind, crouch.prewind_analog), (0.0, 0.0), "the right stick's hold is standard-only");
}
