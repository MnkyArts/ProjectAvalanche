use std::time::Duration;

use bevy::input::ButtonState;
use bevy::input::gamepad::{GamepadConnection, GamepadConnectionEvent, RawGamepadButtonChangedEvent, RawGamepadEvent};
use bevy::input::keyboard::{Key, KeyboardInput};
use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;

use super::*;
use crate::ride::controls::live_read;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Read {
    Latched,
    PerFrame,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Device {
    Key,
    Pad,
}

#[derive(Resource, Default)]
struct Bench {
    latch: PadLatch,
    frame_a: bool,
    steps: Vec<bool>,
}

fn frame_read(mut b: ResMut<Bench>, keys: Res<ButtonInput<KeyCode>>, pads: Query<(Entity, &Gamepad)>) {
    b.latch.frame(&keys, pads.iter());
}

fn latched_step(mut b: ResMut<Bench>, keys: Res<ButtonInput<KeyCode>>, pads: Query<(Entity, &Gamepad)>) {
    let b = &mut *b;
    b.latch.step(&keys, pads.iter());
    let read = live_read(b.latch.keys(), pads.iter().map(|(e, p)| b.latch.pad(e, p)), true, false, false);
    b.steps.push(read.input.jump);
}

fn per_frame_read(mut b: ResMut<Bench>, keys: Res<ButtonInput<KeyCode>>, pads: Query<(Entity, &Gamepad)>) {
    b.frame_a = live_read(&keys, pads.iter().map(|(_, p)| p), true, false, false).input.jump;
}

fn per_frame_step(mut b: ResMut<Bench>) {
    let a = b.frame_a;
    b.steps.push(a);
}

fn bench(read: Read, frame: Duration) -> (App, Entity, Entity) {
    let mut app = App::new();
    app.add_plugins((bevy::time::TimePlugin, bevy::input::InputPlugin));
    app.insert_resource(Time::<Fixed>::from_hz(f64::from(ssx_physics::SIM_HZ)));
    app.insert_resource(TimeUpdateStrategy::ManualDuration(frame));
    app.init_resource::<Bench>();
    match read {
        Read::Latched => {
            app.add_systems(RunFixedMainLoop, frame_read.in_set(RunFixedMainLoopSystems::BeforeFixedMainLoop));
            app.add_systems(FixedUpdate, latched_step);
        }
        Read::PerFrame => {
            app.add_systems(FixedUpdate, per_frame_step);
            app.add_systems(Update, per_frame_read);
        }
    }
    let window = app.world_mut().spawn_empty().id();
    let pad = app.world_mut().spawn_empty().id();
    let connection = GamepadConnection::Connected { name: "test pad".into(), vendor_id: None, product_id: None };
    app.world_mut().write_message(GamepadConnectionEvent::new(pad, connection));
    app.update();
    (app, window, pad)
}

fn send(app: &mut App, device: Device, window: Entity, pad: Entity, down: bool) {
    let world = app.world_mut();
    match device {
        Device::Key => {
            let state = if down { ButtonState::Pressed } else { ButtonState::Released };
            world.write_message(KeyboardInput {
                key_code: KeyCode::Space,
                logical_key: Key::Space,
                state,
                text: None,
                repeat: false,
                window,
            });
        }
        Device::Pad => {
            let value = if down { 1.0 } else { 0.0 };
            let event = RawGamepadButtonChangedEvent::new(pad, GamepadButton::South, value);
            world.write_message(RawGamepadEvent::Button(event));
        }
    }
}

fn run(read: Read, device: Device, frame_ms: f64, tap_ms: f64, phases: u32) -> (Vec<u32>, Vec<u32>) {
    let frame = Duration::from_secs_f64(frame_ms / 1000.0);
    let (mut app, window, pad) = bench(read, frame);
    let mut events: Vec<(f64, bool, u32)> = Vec::new();
    for k in 0..phases {
        let press = 100.0 + f64::from(k) * (300.0 + 1.9);
        events.push((press, true, k));
        events.push((press + tap_ms, false, k));
    }
    let end = events.last().map_or(0.0, |e| e.0) + 300.0;
    let (mut owner, mut steps_of): (Option<u32>, Vec<Option<u32>>) = (None, Vec::new());
    let (mut t, mut next) = (0.0, 0);
    while t < end {
        t += frame_ms;
        while let Some(&(_, down, k)) = events.get(next).filter(|e| e.0 <= t) {
            send(&mut app, device, window, pad, down);
            if down {
                owner = Some(k);
            }
            next += 1;
        }
        let before = app.world().resource::<Bench>().steps.len();
        app.update();
        let after = app.world().resource::<Bench>().steps.len();
        steps_of.extend(std::iter::repeat_n(owner, after - before));
    }
    let steps = &app.world().resource::<Bench>().steps;
    let mut presses = vec![0u32; phases as usize];
    let mut prev = false;
    for (&a, owner) in steps.iter().zip(&steps_of) {
        if a && !prev
            && let Some(k) = owner
        {
            presses[*k as usize] += 1;
        }
        prev = a;
    }
    let lost = (0..phases).filter(|&k| presses[k as usize] == 0).collect();
    let doubled = (0..phases).filter(|&k| presses[k as usize] > 1).collect();
    (lost, doubled)
}

const FRAMES_MS: [f64; 6] = [16.667, 20.0, 28.0, 33.333, 40.0, 50.0];
const TAPS_MS: [f64; 9] = [20.0, 30.0, 40.0, 50.0, 60.0, 70.0, 80.0, 90.0, 100.0];

#[test]
fn every_tap_reaches_a_step() {
    let mut lost_before = Vec::new();
    for device in [Device::Key, Device::Pad] {
        for frame in FRAMES_MS {
            for tap in TAPS_MS {
                let (lost, doubled) = run(Read::Latched, device, frame, tap, 40);
                assert!(
                    lost.is_empty() && doubled.is_empty(),
                    "{device:?} {frame} ms frames, {tap} ms taps: lost {lost:?}, twice {doubled:?}"
                );
                let (lost, _) = run(Read::PerFrame, device, frame, tap, 40);
                lost_before.push((device, frame, tap, lost.len()));
            }
        }
    }
    for (device, frame, tap, n) in &lost_before {
        ssx_core::println!("before: {device:?} {frame:>6.2} ms frames, {tap:>3} ms taps: {n:>2} of 40 lost");
    }
    let lost =
        |d: Device, f: f64, t: f64| lost_before.iter().find(|x| x.0 == d && x.1 == f && x.2 == t).map_or(0, |x| x.3);
    for d in [Device::Key, Device::Pad] {
        assert!(lost(d, 50.0, 20.0) > 0 && lost(d, 50.0, 40.0) > 0 && lost(d, 16.667, 20.0) > 0, "{lost_before:?}");
    }
}

#[test]
fn a_press_again_between_two_steps_keeps_its_edge() {
    let mut keys = ButtonInput::<KeyCode>::default();
    let mut latch = PadLatch::default();
    let pads = std::iter::empty::<(Entity, &Gamepad)>();
    keys.press(KeyCode::Space);
    latch.frame(&keys, pads.clone());
    latch.step(&keys, pads.clone());
    assert!(latch.keys().pressed(KeyCode::Space));
    keys.clear();
    keys.release(KeyCode::Space);
    keys.press(KeyCode::Space);
    latch.frame(&keys, pads.clone());
    latch.step(&keys, pads.clone());
    assert!(!latch.keys().pressed(KeyCode::Space), "the release");
    keys.clear();
    latch.frame(&keys, pads.clone());
    latch.step(&keys, pads.clone());
    assert!(latch.keys().pressed(KeyCode::Space), "the press again");
    latch.step(&keys, pads.clone());
    assert!(latch.keys().pressed(KeyCode::Space), "held");
}

#[test]
fn the_flips_are_bounded_and_a_flush_drops_them() {
    let mut keys = ButtonInput::<KeyCode>::default();
    let mut latch = PadLatch::default();
    let pads = std::iter::empty::<(Entity, &Gamepad)>();
    keys.press(KeyCode::KeyM);
    latch.step(&keys, pads.clone());
    assert!(latch.keys().pressed(KeyCode::KeyM), "not latched: as held");
    for _ in 0..10 {
        keys.clear();
        keys.press(KeyCode::KeyK);
        keys.release(KeyCode::KeyK);
        latch.frame(&keys, pads.clone());
    }
    let mut read = Vec::new();
    for _ in 0..6 {
        latch.step(&keys, pads.clone());
        read.push(latch.keys().pressed(KeyCode::KeyK));
    }
    assert_eq!(read, [true, false, true, false, false, false], "two taps kept of ten");
    keys.clear();
    keys.press(KeyCode::KeyK);
    keys.release(KeyCode::KeyK);
    latch.frame(&keys, pads.clone());
    latch.flush(&keys, pads.clone());
    latch.step(&keys, pads.clone());
    assert!(!latch.keys().pressed(KeyCode::KeyK), "flushed");
}
