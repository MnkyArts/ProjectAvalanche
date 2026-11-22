use super::*;

fn pad() -> Entity {
    Entity::from_raw_u32(7).expect("an index")
}

#[test]
fn the_words_are_the_rumblers() {
    assert_eq!(motor_word(0.0), 0);
    assert_eq!(motor_word(1.0), 65_535);
    assert_eq!(motor_word(0.5), 32_767);
    assert_eq!(motor_word(0.293_333_34), 19_223);
    assert_eq!(motor_word(1.5), (98_302u32 & 0xFFFF) as u16);
    assert_eq!(motor_word(-0.25), (-16_383i64) as u16);
    assert_eq!(motor_word(f32::NAN), 0);
    assert_eq!(motor_word(f32::INFINITY), 0xFFFF);
}

#[test]
fn bevy_gives_every_word_back() {
    let bevy = |ratio: f32| (ratio * u16::MAX as f32) as u16;
    for w in 0..=u16::MAX {
        assert_eq!(bevy(word_ratio(w)), w, "{w}");
    }
    assert_eq!(word_ratio(0), 0.0);
}

#[test]
fn a_post_runs_three_steps_then_stops() {
    let mut out = PadRumbleOut::default();
    let mut sends = Vec::new();
    out.update(LOOP_STEP_S, &mut sends);
    out.start(pad(), LOOP_STEP_S, [1.0, 0.5], &mut sends);
    assert_eq!(sends, [Send(pad(), [65_535, 32_767])]);
    for step in 1..=3 {
        sends.clear();
        out.update(LOOP_STEP_S, &mut sends);
        assert_eq!(sends.is_empty(), step < 3, "step {step}");
    }
    assert_eq!(sends, [Send(pad(), [0, 0])]);
    assert!(!out.pads[0].active);
    sends.clear();
    out.update(LOOP_STEP_S, &mut sends);
    assert!(sends.is_empty());
}

#[test]
fn every_posting_step_sends_and_keeps_the_pad_on() {
    let mut out = PadRumbleOut::default();
    let mut sends = Vec::new();
    for _ in 0..10 {
        out.update(LOOP_STEP_S, &mut sends);
        out.start(pad(), LOOP_STEP_S, [0.25, 0.0], &mut sends);
    }
    assert_eq!(sends.len(), 10);
    assert!(sends.iter().all(|s| *s == Send(pad(), [16_383, 0])));
    assert_eq!(out.pads[0].timer, LOOP_STEP_S);
}

#[test]
fn a_restart_and_a_lost_target_stop_at_once() {
    let mut out = PadRumbleOut::default();
    let mut sends = Vec::new();
    out.start(pad(), LOOP_STEP_S, [0.5, 0.5], &mut sends);
    sends.clear();
    out.stop_all(&mut sends);
    assert_eq!(sends, [Send(pad(), [0, 0])]);
    out.start(pad(), LOOP_STEP_S, [0.5, 0.5], &mut sends);
    sends.clear();
    out.retarget(&[pad()], &[pad()], &mut sends);
    assert!(sends.is_empty());
    out.retarget(&[pad()], &[], &mut sends);
    assert_eq!(sends, [Send(pad(), [0, 0])]);
    sends.clear();
    out.retarget(&[pad()], &[], &mut sends);
    assert!(sends.is_empty());
    out.start(pad(), LOOP_STEP_S, [0.5, 0.5], &mut sends);
    sends.clear();
    out.retarget(&[], &[], &mut sends);
    assert!(sends.is_empty() && out.pads.is_empty());
}

#[test]
fn the_post_needs_a_motor_above_zero() {
    let mut p = RumblePost::default();
    let motors = |m: [f32; 2]| {
        let mut r = PadRumble::default();
        r.motors = m;
        r
    };
    p.executed(&motors([0.0, 0.0]), false);
    assert_eq!((p.executed.is_some(), p.motors), (true, None));
    p.executed(&motors([0.0, 0.1]), false);
    assert_eq!(p.motors, Some([0.0, 0.1]));
    p.executed(&motors([f32::NAN, -1.0]), true);
    assert_eq!(p.motors, None);
    p.restart();
    assert!(p.stop_all && p.executed.is_none());
}

#[test]
fn an_unattended_run_reaches_only_the_named_pad() {
    use bevy::ecs::system::SystemState;
    let mut world = World::new();
    let desk = world.spawn((Gamepad::default(), Name::new("Nacon GC-100"))).id();
    let test = world.spawn((Gamepad::default(), Name::new("test pad"))).id();
    world.spawn((Gamepad::default(), Name::new("test pad"), crate::shell::game_mode::ScriptPad));
    let get = |world: &mut World, target: RumbleTarget, name: Option<&str>, screenshot: bool| {
        world.insert_resource(target);
        world.insert_resource(RumbleTestPad(name.map(str::to_string)));
        let mut state = SystemState::<RumblePads>::new(world);
        state.get(world).expect("the rumble's resources").get(screenshot)
    };
    let both = vec![desk, test];
    assert_eq!(get(&mut world, RumbleTarget::Unattended, None, false), (both.clone(), vec![]));
    assert_eq!(get(&mut world, RumbleTarget::Unattended, Some("test pad"), false), (both.clone(), vec![test]));
    assert_eq!(get(&mut world, RumbleTarget::Focused, None, true), (both.clone(), vec![]));
    assert_eq!(get(&mut world, RumbleTarget::Focused, None, false), (both.clone(), both.clone()));
    assert_eq!(get(&mut world, RumbleTarget::Pads(vec![desk]), Some("test pad"), false), (both.clone(), vec![desk]));
    world.despawn(desk);
    assert_eq!(get(&mut world, RumbleTarget::Pads(vec![desk]), None, false), (vec![test], vec![]));
}
