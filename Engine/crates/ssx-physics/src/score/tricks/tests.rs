use super::*;

fn tuning() -> TrickTuning {
    TrickTuning { tweak_switch_threshold: FRAC_PI_4, virtual_stick_persist_time: 15.0, added: Default::default() }
}

fn air() -> TrickContext {
    TrickContext { in_air: true, enable_air_tricks: true, ticks_to_land: 1e35, ..TrickContext::default() }
}

const OPEN_GATES: Gates = Gates { in_air: true, up_start: true, left_start: true, right_start: true };

const UP: usize = 0;
const LEFT: usize = 4;
const RIGHT: usize = 8;
const DOWN: usize = 12;

fn stick_at(deg: f32, mag: f32) -> TrickInput {
    let a = deg.to_radians();
    TrickInput { right_stick: mag * Vec2::new(a.cos(), a.sin()), ..TrickInput::default() }
}
fn stick(deg: f32) -> TrickInput {
    stick_at(deg, 1.0)
}
fn none() -> TrickInput {
    TrickInput::default()
}
fn btn(y: bool, x: bool, b: bool, a: bool) -> TrickInput {
    TrickInput { y, x, b, a, ..TrickInput::default() }
}

fn seq(runs: &[(TrickInput, usize)]) -> Vec<TrickInput> {
    runs.iter().flat_map(|&(i, n)| std::iter::repeat_n(i, n)).collect()
}

fn ticks_where(inputs: &[TrickInput], gates: Gates, signal: impl Fn(&GrabGestures) -> bool) -> Vec<usize> {
    let mut g = GrabGestures::default();
    let mut on = Vec::new();
    for (t, i) in inputs.iter().enumerate() {
        g.tick(PadSample::new(i), &gates, &AddedAirTricks::default(), false);
        if signal(&g) {
            on.push(t);
        }
    }
    on
}

fn until_change(s: &mut TrickState, input: &TrickInput, ctx: &TrickContext, max: usize) -> usize {
    let node = (s.phase, s.level, s.tweak_node);
    for i in 1..=max {
        s.update(&tuning(), input, ctx, 2.0);
        if (s.phase, s.level, s.tweak_node) != node {
            return i;
        }
    }
    max + 1
}

fn to_loop(input: &TrickInput, ctx: &TrickContext, level: TrickLevel) -> TrickState {
    let mut s = TrickState::default();
    for _ in 0..60 {
        s.update(&tuning(), input, ctx, 2.0);
        if (s.phase, s.level, s.tweak_node) == (TrickPhase::Loop, level, false) {
            return s;
        }
    }
    panic!("no {level:?} loop: {:?} {:?}", s.phase, s.level);
}

#[test]
fn up_needs_8_ticks_and_restarts() {
    let up = |g: &GrabGestures| g.stick[UP];
    assert_eq!(ticks_where(&seq(&[(stick(90.0), 12)]), OPEN_GATES, up), (7..12).collect::<Vec<_>>());
    let broken = seq(&[(stick(90.0), 5), (none(), 1), (stick(90.0), 10)]);
    assert_eq!(ticks_where(&broken, OPEN_GATES, up), (13..16).collect::<Vec<_>>());
    assert!(ticks_where(&seq(&[(stick(90.0), 20)]), Gates::default(), up).is_empty());
    let (t, mut s) = (tuning(), TrickState::default());
    (0..7).for_each(|_| s.update(&t, &stick(90.0), &air(), 2.0));
    assert_eq!((s.phase, s.has_valid_input), (TrickPhase::None, false));
    s.update(&t, &stick(90.0), &air(), 2.0);
    assert_eq!(s.phase, TrickPhase::Anticipate);
}

#[test]
fn right_200_tick_limit_is_a_no_op() {
    let held = seq(&[(stick(0.0), 450)]);
    assert_eq!(ticks_where(&held, OPEN_GATES, |g| g.stick[RIGHT]).len(), 450);
    assert_eq!(ticks_where(&seq(&[(stick(180.0), 300)]), OPEN_GATES, |g| g.stick[LEFT]).len(), 300);
    assert_eq!(ticks_where(&seq(&[(stick(270.0), 300)]), OPEN_GATES, |g| g.stick[DOWN]).len(), 300);
    assert_eq!(ticks_where(&seq(&[(stick_at(0.0, 0.59), 5)]), OPEN_GATES, |g| g.stick[RIGHT]).len(), 0);
    assert_eq!(ticks_where(&seq(&[(stick_at(0.0, 0.6), 5)]), OPEN_GATES, |g| g.stick[RIGHT]).len(), 5);
}

#[test]
fn stick_doubles_hold_a_at_most_10_ticks_then_b_within_10() {
    const LEFT_RIGHT: usize = 5;
    let lr = |g: &GrabGestures| g.stick[LEFT_RIGHT];
    let (l, r) = (stick(180.0), stick(0.0));
    assert_eq!(ticks_where(&seq(&[(none(), 1), (l, 10), (r, 3)]), OPEN_GATES, lr), vec![11, 12, 13]);
    assert!(ticks_where(&seq(&[(none(), 1), (l, 11), (r, 3)]), OPEN_GATES, lr).is_empty());
    assert_eq!(ticks_where(&seq(&[(none(), 1), (l, 4), (none(), 10), (r, 2)]), OPEN_GATES, lr), vec![15, 16]);
    assert!(ticks_where(&seq(&[(none(), 1), (l, 4), (none(), 11), (r, 2)]), OPEN_GATES, lr).is_empty());
}

#[test]
fn up_left_and_up_right_only_as_rolls() {
    let (up_left, up_right) = (|g: &GrabGestures| g.stick[1], |g: &GrabGestures| g.stick[2]);
    let roll_left = seq(&[(none(), 1), (stick(90.0), 3), (stick(180.0), 3)]);
    assert_eq!(ticks_where(&roll_left, OPEN_GATES, up_left), vec![4, 5, 6]);
    assert_eq!(ticks_where(&roll_left, OPEN_GATES, |g| g.stick[LEFT]), vec![4, 5, 6], "Left is also held");
    let roll_right = seq(&[(none(), 1), (stick(90.0), 3), (stick(0.0), 3)]);
    assert_eq!(ticks_where(&roll_right, OPEN_GATES, up_right), vec![4, 5, 6]);
    let via_centre = seq(&[(none(), 1), (stick(90.0), 3), (none(), 1), (stick(180.0), 3)]);
    assert!(ticks_where(&via_centre, OPEN_GATES, up_left).is_empty());
    let weak = TrickInput { right_stick: Vec2::new(-0.6, 0.0), ..none() };
    let weak_roll = seq(&[(none(), 1), (stick(90.0), 3), (weak, 3)]);
    assert!(ticks_where(&weak_roll, OPEN_GATES, up_left).is_empty());
    assert_eq!(ticks_where(&weak_roll, OPEN_GATES, |g| g.stick[LEFT]).len(), 3);
}

#[test]
fn button_doubles_and_the_11_tick_re_arm() {
    const UP_LEFT: usize = 1;
    let (y, x) = (btn(true, false, false, false), btn(false, true, false, false));
    let up_left = |g: &GrabGestures| g.button[UP_LEFT];
    let quick = seq(&[(none(), 1), (y, 3), (none(), 2), (x, 3)]);
    assert_eq!(ticks_where(&quick, OPEN_GATES, up_left), vec![6, 7, 8]);
    assert_eq!(ticks_where(&quick, OPEN_GATES, |g| g.button[LEFT]), vec![6, 7, 8]);
    for hold in [10, 11, 12, 21, 22, 23] {
        let s = seq(&[(none(), 1), (y, hold), (none(), 2), (x, 3)]);
        assert_eq!(!ticks_where(&s, OPEN_GATES, up_left).is_empty(), hold % 11 != 0, "Y held {hold} ticks");
    }
    assert!(ticks_where(&quick, Gates { up_start: false, ..OPEN_GATES }, up_left).is_empty());
    let yb = seq(&[(none(), 1), (btn(true, false, true, false), 4), (btn(false, false, true, false), 3)]);
    assert_eq!(ticks_where(&yb, OPEN_GATES, |g| g.button[UP] && g.button[RIGHT]), vec![1, 2, 3, 4]);
    assert_eq!(ticks_where(&yb, OPEN_GATES, |g| g.button[2]), vec![5, 6, 7]);
}

#[test]
fn x_to_b_needs_a_first_tap() {
    const LEFT_RIGHT: usize = 5;
    let (x, b, xb) = (btn(false, true, false, false), btn(false, false, true, false), btn(false, true, true, false));
    let lr = |g: &GrabGestures| g.button[LEFT_RIGHT];
    assert_eq!(ticks_where(&seq(&[(none(), 1), (x, 5), (none(), 2), (b, 3)]), OPEN_GATES, lr), vec![8, 9, 10]);
    assert!(ticks_where(&seq(&[(none(), 1), (x, 15), (none(), 2), (b, 3)]), OPEN_GATES, lr).is_empty());
    assert_eq!(ticks_where(&seq(&[(none(), 1), (x, 4), (xb, 2), (b, 3)]), OPEN_GATES, lr), vec![7, 8, 9]);
}

#[test]
fn input_index_and_scan_order() {
    let t = tuning();
    let mut s = TrickState::default();
    assert_eq!((s.input_index, s.input_index_on_enter), (0, 0));
    s.update(&t, &stick(0.0), &air(), 2.0);
    assert_eq!((s.has_valid_input, s.input_index, s.input_index_on_enter), (true, -1, -1));
    assert_eq!(s.phase, TrickPhase::Anticipate);
    s.update(&t, &TrickInput { y: true, ..stick(0.0) }, &air(), 2.0);
    assert_eq!(s.input_index, 0);
    assert!(s.pad_grab_end());
    let mut s = TrickState::default();
    (0..2).for_each(|_| s.update(&t, &btn(true, false, true, false), &air(), 2.0));
    assert!(s.gestures.button[UP] && s.gestures.button[RIGHT]);
    assert_eq!((s.input_index, s.trick_type), (0, TrickType::Rocket));
}

#[test]
fn enter_exits_on_lost_input_before_tweaking() {
    let (t, ctx) = (tuning(), air());
    let a = btn(false, false, false, true);
    let mut s = TrickState::default();
    assert_eq!(until_change(&mut s, &a, &ctx, 1), 1);
    assert_eq!(until_change(&mut s, &a, &ctx, 10), 7);
    assert_eq!((s.phase, s.level, s.trick_type), (TrickPhase::Enter, TrickLevel::Normal, TrickType::Truckdriver));
    let mut released = s;
    released.update(&t, &TrickInput { tweak: true, ..none() }, &ctx, 2.0);
    assert!(released.is_tweaking && !released.has_valid_input);
    assert_eq!((released.phase, released.tweak_node), (TrickPhase::Exit, false));
    s.update(&t, &TrickInput { tweak: true, ..a }, &ctx, 2.0);
    assert_eq!((s.phase, s.tweak_node), (TrickPhase::Enter, true));
}

#[test]
fn exit_lengths_per_level() {
    let y = btn(true, false, false, false);
    let uber = TrickContext { awesome: true, ..air() };
    let super_uber = TrickContext { super_awesome: true, ..uber };
    let exit_steps = |ctx: &TrickContext, level: TrickLevel, during: &TrickContext| {
        let mut s = to_loop(&y, ctx, level);
        assert_eq!(until_change(&mut s, &none(), during, 1), 1);
        assert_eq!((s.phase, s.level), (TrickPhase::Exit, level));
        let n = until_change(&mut s, &none(), during, 30);
        assert_eq!((s.phase, s.trick_type), (TrickPhase::None, TrickType::None), "OnGrabReset");
        n
    };
    let near = |ctx: &TrickContext, ticks: f32| TrickContext { ticks_to_land: ticks, ..*ctx };
    assert_eq!(exit_steps(&air(), TrickLevel::Normal, &air()), 5);
    assert_eq!(exit_steps(&air(), TrickLevel::Normal, &near(&air(), 0.0)), 5);
    assert_eq!(exit_steps(&uber, TrickLevel::Uber, &uber), 7);
    assert_eq!(exit_steps(&uber, TrickLevel::Uber, &near(&uber, 0.0)), 7);
    assert_eq!(exit_steps(&super_uber, TrickLevel::SuperUber, &super_uber), 15);
    assert_eq!(exit_steps(&super_uber, TrickLevel::SuperUber, &near(&super_uber, 10.0)), 10);
    assert_eq!(exit_steps(&super_uber, TrickLevel::SuperUber, &near(&super_uber, 0.0)), 7);
}

#[test]
fn only_plain_uber_and_super_uber_exits_chain_a_new_grab() {
    let (t, y) = (tuning(), btn(true, false, false, false));
    let uber = TrickContext { awesome: true, ..air() };
    let super_uber = TrickContext { super_awesome: true, ..uber };
    for (ctx, level) in [(air(), TrickLevel::Normal), (uber, TrickLevel::Uber), (super_uber, TrickLevel::SuperUber)] {
        let mut s = to_loop(&y, &ctx, level);
        s.update(&t, &none(), &ctx, 2.0);
        assert!(s.ending());
        s.update(&t, &y, &ctx, 2.0);
        assert_eq!(s.phase, TrickPhase::Anticipate, "{level:?} exit → anticipate on PadGrabStart");
    }
    let mut s = to_loop(&y, &air(), TrickLevel::Normal);
    let yr2 = TrickInput { tweak: true, ..y };
    assert_eq!(until_change(&mut s, &yr2, &air(), 1), 1);
    assert_eq!(until_change(&mut s, &yr2, &air(), 10), 7);
    assert_eq!((s.phase, s.tweak_node), (TrickPhase::Loop, true));
    assert_eq!(until_change(&mut s, &none(), &air(), 1), 1);
    assert_eq!((s.phase, s.tweak_node), (TrickPhase::Exit, true));
    assert_eq!(until_change(&mut s, &y, &air(), 10), 5);
    assert_eq!((s.phase, s.trick_ticks), (TrickPhase::Anticipate, 2.0));
    let sig = TrickInput { signature: true, ..y };
    let mut s = TrickState::default();
    (0..8).for_each(|_| s.update(&t, &sig, &super_uber, 2.0));
    assert_eq!((s.phase, s.level), (TrickPhase::Enter, TrickLevel::Signature));
    let steps = until_change(&mut s, &none(), &super_uber, 60);
    assert_eq!((s.phase, s.level, s.sig_exit), (TrickPhase::Exit, TrickLevel::Signature, 1));
    assert_eq!(steps, 38, "sig_enter τ 1 + 2·37 = 75: due at the 37th step's end, fires on the 38th");
    assert_eq!(until_change(&mut s, &y, &super_uber, 20), 15, "30 ticks; through the air pose");
    assert_eq!(s.phase, TrickPhase::Anticipate);
}

#[test]
fn defaults_from_the_assets() {
    let s = TrickState::default();
    assert_eq!((s.ticks_since_sig_held, s.virtual_stick_timer), (0.0, 0.0));
    assert_eq!((s.input_index, s.input_index_on_enter, s.gesture_len), (0, 0, -1));
    assert!(!s.valid_up_start && !s.valid_left_start && !s.valid_right_start);
    assert_eq!(s.signature, SignatureTiming::for_rider("mac").unwrap());
    assert_eq!(SignatureTiming::for_rider("kaori").unwrap().out_blend_1, (62.0, 2.0));
    let mut s = TrickState::default();
    s.update(&tuning(), &none(), &TrickContext::default(), 2.0);
    assert_eq!((s.ticks_since_sig_held, s.virtual_stick_timer), (2.0, -2.0));
    assert!(!s.is_tweaking);
}

#[test]
fn right_stick_magnitude_is_rescaled() {
    for (raw, mag) in [(0.0, 0.0), (0.25, 0.0), (0.475, 0.3), (0.625, 0.5), (1.0, 1.0), (1.5, 1.0)] {
        assert!((rstick_mag(Vec2::new(raw, 0.0)) - mag).abs() < 1e-6, "{raw}");
    }
    let looping = || TrickState {
        phase: TrickPhase::Loop,
        trick_type: TrickType::Melon,
        hand: TrickHand::Forehand,
        gesture_len: 1,
        virtual_stick_timer: -1.0,
        ..TrickState::default()
    };
    for (raw, tweaking) in [(0.45, false), (0.5, true)] {
        let mut s = looping();
        s.update(&tuning(), &TrickInput { tweak: true, ..stick_at(0.0, raw) }, &air(), 2.0);
        assert_eq!((s.tweak_blend, s.is_tweaking), (1.0, tweaking), "|s| {raw}");
    }
    for (raw, blend) in [(0.3, 0.0), (0.4, 1.0)] {
        let mut s = looping();
        s.update(&tuning(), &stick_at(90.0, raw), &air(), 2.0);
        assert_eq!(s.tweak_blend, blend, "|s| {raw}");
    }
}

#[test]
fn reset_trick_state_keeps_inputs_and_freezes_the_sig_counter() {
    let (t, ctx) = (tuning(), TrickContext { awesome: true, super_awesome: true, ..air() });
    let sig = TrickInput { y: true, signature: true, ..none() };
    let mut s = TrickState::default();
    (0..10).for_each(|_| s.update(&t, &sig, &ctx, 2.0));
    (0..3).for_each(|_| s.update(&t, &none(), &ctx, 2.0));
    let before = s;
    assert!(before.ticks_since_sig_held > 0.0 && before.is_tricking);
    s.reset_trick_state();
    assert_eq!(
        (s.trick_type, s.trick_type_locked, s.hand, s.gesture_len),
        (TrickType::None, TrickType::None, TrickHand::None, -1)
    );
    assert_eq!((s.tweak_blend, s.virtual_stick_timer, s.virtual_stick), (0.0, 0.0, Vec2::ZERO));
    assert!(!s.is_signature && !s.trick_locked);
    assert_eq!(
        (s.has_valid_input, s.input_index, s.input_index_on_enter),
        (before.has_valid_input, before.input_index, before.input_index_on_enter)
    );
    assert_eq!(
        (s.ticks_since_sig_held, s.is_tricking, s.is_tweaking),
        (before.ticks_since_sig_held, true, before.is_tweaking)
    );
    (0..3).for_each(|_| s.update_reset(&tuning(), &sig, &ctx, 2.0));
    assert_eq!(s.ticks_since_sig_held, before.ticks_since_sig_held);
    assert!(s.gestures.button[UP] && s.gestures.signature);
    assert_eq!(s.phase_ticks, before.phase_ticks + 6.0);
}

#[test]
fn grab_timing_reach_enter_loop_exit() {
    let (t, ctx) = (tuning(), air());
    let mut s = TrickState::default();
    let y = btn(true, false, false, false);
    s.update(&t, &y, &ctx, 2.0);
    assert_eq!((s.phase, s.phase_ticks), (TrickPhase::Anticipate, 2.0));
    assert_eq!(until_change(&mut s, &y, &ctx, 20), 7);
    assert_eq!((s.phase, s.level, s.phase_ticks), (TrickPhase::Enter, TrickLevel::Normal, 1.0));
    assert_eq!((s.trick_type, s.hand, s.gesture_len), (TrickType::Rocket, TrickHand::Both, 1));
    assert_eq!(until_change(&mut s, &y, &ctx, 20), 8);
    assert_eq!((s.phase, s.phase_ticks, s.trick_ticks), (TrickPhase::Loop, 2.0, 32.0));
    assert_eq!(until_change(&mut s, &y, &ctx, 30), 31, "the loop lasts while the grab is held");
    assert_eq!(s.trick_type_locked, TrickType::Rocket);
    assert!(s.trick_locked && s.is_tricking);
    assert_eq!(until_change(&mut s, &none(), &ctx, 5), 1);
    assert!(s.ending());
    assert_eq!(until_change(&mut s, &none(), &ctx, 10), 5);
    assert_eq!((s.phase, s.trick_type, s.hand), (TrickPhase::None, TrickType::None, TrickHand::None));
}

#[test]
fn table_columns_and_gestures() {
    let t = tuning();
    let first = |input: TrickInput, goofy: bool| {
        let ctx = TrickContext { goofy, ..air() };
        let mut s = TrickState::default();
        (0..6).for_each(|_| s.update(&t, &input, &ctx, 2.0));
        (s.trick_type, s.gesture_len)
    };
    let x = btn(false, true, false, false);
    assert_eq!(first(x, false), (TrickType::Melon, 1));
    assert_eq!(first(x, true), (TrickType::Indy, 1), "goofy column");
    assert_eq!(first(stick_at(6.0, 0.9), false), (TrickType::Indy, 1));
    assert_eq!(first(btn(false, false, false, true), false), (TrickType::Truckdriver, 2), "Down is a length-2 entry");
    assert_eq!(first(stick_at(0.0, 0.5), false), (TrickType::None, -1));
}

#[test]
fn double_gesture_upgrades_during_the_anticipation() {
    let t = tuning();
    let (up, left) = (stick(90.0), stick(180.0));
    let mut s = TrickState::default();
    (0..8).for_each(|_| s.update(&t, &up, &air(), 2.0));
    assert_eq!(s.phase, TrickPhase::Anticipate);
    s.update(&t, &up, &air(), 2.0);
    assert_eq!(s.trick_type, TrickType::Rocket);
    let mut via_centre = s;
    s.update(&t, &left, &air(), 2.0);
    assert_eq!((s.trick_type, s.gesture_len), (TrickType::FlyingSquirrel, 2));
    assert!(s.trick_locked && s.trick_type_locked == TrickType::FlyingSquirrel);
    via_centre.update(&t, &none(), &air(), 2.0);
    (0..3).for_each(|_| via_centre.update(&t, &left, &air(), 2.0));
    assert_eq!((via_centre.trick_type, via_centre.gesture_len), (TrickType::Rocket, 1));
}

#[test]
fn tweak_with_r2_and_air_tricks_gate() {
    let t = tuning();
    let b = btn(false, false, true, false);
    let mut s = TrickState::default();
    (0..20).for_each(|_| s.update(&t, &b, &air(), 2.0));
    assert_eq!((s.phase, s.tweak_node, s.trick_type), (TrickPhase::Loop, false, TrickType::Indy));
    let tweak = TrickInput { tweak: true, ..b };
    s.update(&t, &tweak, &air(), 2.0);
    assert!(s.is_tweaking && s.tweak_blend == 1.0);
    assert_eq!((s.phase, s.tweak_node), (TrickPhase::Enter, true), "same step: conditions after TrickSceneOp");
    (0..8).for_each(|_| s.update(&t, &tweak, &air(), 2.0));
    assert_eq!((s.phase, s.tweak_node), (TrickPhase::Loop, true));
    s.update(&t, &b, &air(), 2.0);
    assert_eq!((s.phase, s.tweak_node), (TrickPhase::Exit, true), "tweak released → tweak exit");
    for ctx in [TrickContext { enable_air_tricks: false, ..air() }, TrickContext { in_air: false, ..air() }] {
        let mut s = TrickState::default();
        (0..10).for_each(|_| s.update(&t, &b, &ctx, 2.0));
        assert_eq!(s.phase, TrickPhase::None);
    }
}

#[test]
fn classic_x_tweaks_the_grab() {
    let t = tuning();
    let grab = classic(false, false, false, true);
    let mut s = TrickState { classic: true, ..TrickState::default() };
    (0..20).for_each(|_| s.update(&t, &grab, &air(), 2.0));
    assert!(s.is_tricking && !s.is_tweaking, "a classic grab first");
    let x = TrickInput { x: true, tweak: true, ..grab };
    s.update(&t, &x, &air(), 2.0);
    assert!(s.is_tweaking && s.tweak_blend == 1.0, "X held: tweaking (blend {:?})", s.tweak_blend);
    assert_eq!((s.phase, s.tweak_node), (TrickPhase::Enter, true));
    (0..8).for_each(|_| s.update(&t, &x, &air(), 2.0));
    assert_eq!((s.phase, s.tweak_node), (TrickPhase::Loop, true));
    s.update(&t, &grab, &air(), 2.0);
    assert_eq!((s.phase, s.tweak_node), (TrickPhase::Exit, true), "X released → tweak exit");
    assert!(!s.is_tweaking);
}

#[test]
fn signature_needs_super_tricky_and_counts_ticks_since_release() {
    let t = tuning();
    let sig = TrickInput { y: true, signature: true, ..TrickInput::default() };
    let mut s = TrickState::default();
    (0..6).for_each(|_| s.update(&t, &sig, &air(), 2.0));
    assert_eq!(s.trick_type, TrickType::Rocket, "not super-tricky: a plain grab");
    let ctx = TrickContext { awesome: true, super_awesome: true, ..air() };
    let mut s = TrickState::default();
    (0..12).for_each(|_| s.update(&t, &sig, &ctx, 2.0));
    assert!(s.is_signature && s.level == TrickLevel::Signature && s.trick_type == TrickType::Signature);
    assert_eq!(s.ticks_since_sig_held, 0.0);
    for k in 1..=11 {
        s.update(&t, &TrickInput::default(), &ctx, 2.0);
        assert_eq!(s.ticks_since_sig_held <= 16.0, k <= 9, "step {k}: {}", s.ticks_since_sig_held);
    }
    let ctx = TrickContext { awesome: true, ..air() };
    let mut s = TrickState::default();
    (0..10).for_each(|_| s.update(&t, &btn(false, true, false, false), &ctx, 2.0));
    assert_eq!((s.phase, s.level), (TrickPhase::Enter, TrickLevel::Uber));
}

#[test]
fn air_jump_and_uber_recovery_nodes_hold_the_tricks_off() {
    let (t, y) = (tuning(), btn(true, false, false, false));
    let mut s = TrickState::default();
    s.update(&t, &y, &TrickContext { air_jump: true, ..air() }, 2.0);
    assert_eq!(s.phase, TrickPhase::AirJump);
    let steps = until_change(&mut s, &y, &air(), 10);
    assert_eq!((steps, s.phase), (7, TrickPhase::Anticipate), "τ 2 + 2·6 = 14: the window at 15 opens in the 7th");
    let mut s = TrickState::default();
    let recovering = TrickContext { recovering: true, ..air() };
    (0..3).for_each(|_| s.update(&t, &y, &recovering, 2.0));
    assert_eq!(s.phase, TrickPhase::Recover);
    s.update(&t, &y, &air(), 2.0);
    assert_eq!(s.phase, TrickPhase::Anticipate, "back to the air pose and on to the grab in one update");
}

#[test]
fn signature_timing_by_bundle_name() {
    assert_eq!(SignatureTiming::for_rider("travis"), SignatureTiming::for_rider("travisrice"));
    assert_eq!(SignatureTiming::for_rider("tyler").map(|t| t.enter_end.0), Some(190.0));
    assert!(SignatureTiming::for_rider("wizard").is_none(), "a helicopter pilot");
}

fn classic(lb: bool, rb: bool, lt: bool, rt: bool) -> TrickInput {
    TrickInput { lb, rb, lt, rt, ..TrickInput::default() }
}

#[test]
fn classic_air_tricks_are_exact_shoulder_sets() {
    let sets = [
        (true, false, false, false),
        (false, true, false, false),
        (false, false, true, false),
        (false, false, false, true),
        (true, true, false, false),
        (true, false, true, false),
        (true, false, false, true),
        (false, true, false, true),
        (false, true, true, false),
        (false, false, true, true),
        (true, true, true, false),
        (true, true, false, true),
        (true, false, true, true),
        (false, true, true, true),
        (true, true, true, true),
    ];
    for (i, &(lb, rb, lt, rt)) in sets.iter().enumerate() {
        let mut g = GrabGestures::default();
        g.tick(PadSample::new(&classic(lb, rb, lt, rt)), &OPEN_GATES, &AddedAirTricks::default(), true);
        assert!(g.button[i], "entry {i} is {:?}", (lb, rb, lt, rt));
        assert_eq!(g.button.iter().filter(|&&b| b).count(), 1, "entry {i} matches no other entry");
        assert!(!g.stick.iter().any(|&b| b), "the classic stick mappings are never on");
    }
    let mut g = GrabGestures::default();
    g.tick(PadSample::new(&classic(true, true, false, false)), &OPEN_GATES, &AddedAirTricks::default(), true);
    assert!(g.button[4] && !g.button[0], "LB+RB is Truckdriver, not LB's Nose");
}

#[test]
fn classic_air_choice_uses_the_fifteen_entry_table() {
    let mut s = TrickState { classic: true, ..TrickState::default() };
    s.gestures.button[4] = true;
    s.choose_air_trick(&tuning(), &none(), &PadSample::default(), &air(), 2.0);
    assert_eq!(s.trick_type, TrickType::Truckdriver);
    assert_eq!(s.gesture_len, 1);
    let mut s = TrickState { classic: true, ..TrickState::default() };
    s.gestures.button[14] = true;
    s.choose_air_trick(&tuning(), &none(), &PadSample::default(), &air(), 2.0);
    assert_eq!(s.trick_type, TrickType::Truckdriver);
    assert_eq!(s.gesture_len, 2);
}

#[test]
fn classic_grind_tables_match_the_executables_groups() {
    let cases = [
        (0, (false, false, true, false), 0, 14),
        (1, (true, false, false, false), 0, 14),
        (2, (true, false, true, false), 1, 23),
        (3, (true, true, true, false), 2, 23),
        (5, (false, false, false, true), 0, 17),
        (6, (false, true, false, false), 0, 17),
        (7, (false, true, false, true), 1, 26),
        (10, (true, true, false, false), 1, 20),
        (14, (true, true, true, true), 2, 29),
    ];
    for (i, (lb, rb, lt, rt), len, plain) in cases {
        let mut g = GrindTricks::default();
        let cur = PadSample { lb, rb, lt, rt, ..PadSample::default() };
        g.tick_gestures(&PadSample::default(), &cur, true, true);
        assert!(g.button[i], "classic grind entry {i}");
        assert_eq!(g.button.iter().filter(|&&b| b).count(), 1, "entry {i} is the exact set");
        assert_eq!((GRIND_TRICKS_CLASSIC[i].len, GRIND_TRICKS_CLASSIC[i].plain), (len, plain));
    }
}

#[test]
fn the_wipeout_recovery_takes_the_schemes_boost_button() {
    let x = TrickInput { x: true, ..TrickInput::default() };
    let r2 = TrickInput { tweak: true, ..TrickInput::default() };
    let none = TrickInput::default();
    assert!(
        wipeout_recovery_input(false, false, true, &x, 1.0) && !wipeout_recovery_input(false, false, true, &r2, 1.0)
    );
    assert!(
        wipeout_recovery_input(false, false, false, &r2, 1.0) && !wipeout_recovery_input(false, false, false, &x, 1.0)
    );
    assert!(!wipeout_recovery_input(false, false, true, &x, 0.0), "no boost in the meter");
    assert!(
        wipeout_recovery_input(true, false, false, &none, 0.0) && wipeout_recovery_input(false, true, true, &none, 0.0)
    );
    assert!(!wipeout_recovery_input(false, false, false, &none, 5.0));
}

#[test]
fn the_classic_boost_is_x_held_from_its_first_update() {
    let x = TrickInput { x: true, ..TrickInput::default() };
    let r2 = TrickInput { tweak: true, ..TrickInput::default() };
    assert!(boost_requested(true, &x) && !boost_requested(true, &r2), "classic: X (`BLN_Pad_Classic_Boost`)");
    assert!(boost_requested(false, &r2) && !boost_requested(false, &x), "standard: R2 (`BLN_Pad_R2_Hold`)");
}
