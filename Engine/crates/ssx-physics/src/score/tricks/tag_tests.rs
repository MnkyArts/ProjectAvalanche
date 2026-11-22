use super::*;

fn tuning() -> TrickTuning {
    TrickTuning { tweak_switch_threshold: FRAC_PI_4, virtual_stick_persist_time: 15.0, added: Default::default() }
}

fn air() -> TrickContext {
    TrickContext { in_air: true, enable_air_tricks: true, ticks_to_land: 1e35, ..TrickContext::default() }
}

fn btn(y: bool, x: bool, b: bool, a: bool) -> TrickInput {
    TrickInput { y, x, b, a, ..TrickInput::default() }
}

fn airborne() -> TrickState {
    let mut s = TrickState::default();
    s.enter_air_node(false);
    s
}

fn step(s: &mut TrickState, inp: &TrickInput) {
    s.scene_op(&tuning(), inp, &air(), 2.0);
    s.air_node_update(2.0);
    s.flow(&air(), 2.0);
    s.script_tags();
}

#[test]
fn y_then_x_holds_the_flying_squirrel_loop() {
    let (y, x) = (btn(true, false, false, false), btn(false, true, false, false));
    let mut s = airborne();
    for i in [y, y, y, TrickInput::default(), x] {
        step(&mut s, &i);
    }
    assert_eq!((s.trick_type, s.input_index), (TrickType::FlyingSquirrel, 1));
    let mut nodes = Vec::new();
    for _ in 0..30 {
        step(&mut s, &x);
        nodes.push(s.flow_node_name());
    }
    assert_eq!(s.input_index_on_enter, 1);
    assert_eq!(s.flow_node_name(), "state_trick_air_loop");
    assert_eq!(s.trick_type, TrickType::FlyingSquirrel);
    let first_loop = nodes.iter().position(|n| *n == "state_trick_air_loop").expect("a loop");
    assert!(nodes[first_loop..].iter().all(|n| *n == "state_trick_air_loop"), "{nodes:?}");
    assert!(!nodes.contains(&"state_trick_air_exit"));
}

#[test]
fn on_air_trick_enter_latches_at_the_take_after_the_scoring() {
    let y = btn(true, false, false, false);
    let mut s = airborne();
    s.input_index_on_enter = 7;
    step(&mut s, &y);
    assert_eq!((s.flow_node_name(), s.input_index, s.input_index_on_enter), ("state_trick_air_anticipate", 0, 7));
    for _ in 0..20 {
        s.scene_op(&tuning(), &y, &air(), 2.0);
        s.air_node_update(2.0);
        s.flow(&air(), 2.0);
        if s.flow_node_name() == "state_trick_air_enter" {
            assert_eq!(s.input_index_on_enter, 7);
            assert!(s.scripts.now.air_trick_enter);
            s.script_tags();
            assert_eq!(s.input_index_on_enter, 0);
            return;
        }
        s.script_tags();
    }
    panic!("no enter");
}

#[test]
fn exit_reset_waits_for_the_script_tags_and_a_chained_grab_resets_now() {
    let (y, b) = (btn(true, false, false, false), btn(false, false, true, false));
    let mut s = airborne();
    (0..20).for_each(|_| step(&mut s, &y));
    assert_eq!(s.flow_node_name(), "state_trick_air_loop");
    step(&mut s, &TrickInput::default());
    assert_eq!(s.flow_node_name(), "state_trick_air_exit");
    s.scene_op(&tuning(), &b, &air(), 2.0);
    s.air_node_update(2.0);
    s.flow(&air(), 2.0);
    assert_eq!(s.flow_node_name(), "state_trick_air_anticipate");
    assert_eq!((s.trick_type, s.hand), (TrickType::Rocket, TrickType::Rocket.hand()), "the scoring view");
    assert!(s.scripts.now.grab_reset && !s.scripts.next.grab_reset);
    s.script_tags();
    assert_eq!((s.trick_type, s.hand, s.gesture_len), (TrickType::None, TrickHand::None, -1));
    step(&mut s, &b);
    assert_eq!(s.trick_type, TrickType::Stiffy);
}

#[test]
fn an_exit_left_after_a_part_of_the_update_resets_in_the_next_step() {
    let mut q = ScriptQueue::default();
    let mut t = NodeTags::new(NodeScript::OnExit(Script::GrabReset));
    t.update(2.0, &mut q.now);
    t.begin_step();
    let mut early = t;
    early.exit(&mut q);
    assert!(q.now.grab_reset && !q.next.grab_reset);
    let mut q = ScriptQueue::default();
    t.update(1.0, &mut q.now);
    t.exit(&mut q);
    assert!(!q.now.grab_reset && q.next.grab_reset);
    assert!(q.take() == script_tags::ScriptCalls::default() && q.take().grab_reset);
}

#[test]
fn uber_tweak_exit_resets_past_tick_1() {
    let mut q = ScriptQueue::default();
    let mut t = NodeTags::new(Node::TweakExit(TrickLevel::Uber).script());
    t.update(2.0, &mut q.now);
    assert!(!q.now.grab_reset, "first update (clock 0): active");
    t.update(2.0, &mut q.now);
    assert!(q.now.grab_reset, "clock 2 > 1: left");
    let mut q = ScriptQueue::default();
    t.exit(&mut q);
    assert_eq!(q, ScriptQueue::default(), "left once");
    let mut t = NodeTags::new(Node::TweakExit(TrickLevel::SuperUber).script());
    t.update(0.5, &mut q.now);
    t.update(2.0, &mut q.now);
    assert!(!q.now.grab_reset);
    t.begin_step();
    t.exit(&mut q);
    assert!(q.now.grab_reset, "exited before the window's leave");
}

#[test]
fn air_states_resets_at_the_flows_first_update() {
    let mut s = airborne();
    (s.hand, s.trick_type, s.virtual_stick_timer) = (TrickHand::Backhand, TrickType::Indy, 4.0);
    s.scene_op(&tuning(), &TrickInput::default(), &air(), 2.0);
    s.flow(&air(), 2.0);
    assert!(s.scripts.now.grab_reset);
    s.script_tags();
    assert_eq!((s.hand, s.trick_type, s.virtual_stick_timer), (TrickHand::None, TrickType::None, -1.0));
    step(&mut s, &TrickInput::default());
    assert!(!s.scripts.now.grab_reset);
}

#[test]
fn choose_air_trick_resets_the_tweak_only_when_it_finds_an_entry() {
    let pad = PadSample::new(&TrickInput::default());
    let mut s = TrickState { tweak_blend: 1.0, tweak_enter: 0.5, tweak_wind: 0.7, ..TrickState::default() };
    s.choose_air_trick(&tuning(), &TrickInput::default(), &pad, &air(), 2.0);
    assert_eq!((s.tweak_blend, s.tweak_enter, s.tweak_wind), (1.0, 0.5, 0.7), "nothing found");
    s.gestures.button[0] = true;
    s.choose_air_trick(&tuning(), &TrickInput::default(), &pad, &air(), 2.0);
    assert_eq!(s.trick_type, TrickType::Rocket);
    assert_eq!(s.tweak_blend, 0.0);
}

#[test]
fn the_tweak_resets_on_the_step_after_a_start() {
    let stick = |deg: f32| TrickInput { right_stick: Vec2::from_angle(deg.to_radians()), ..TrickInput::default() };
    let mut s = airborne();
    for _ in 0..30 {
        step(&mut s, &stick(90.0));
        if s.flow_node_name() == "state_trick_air_enter" {
            break;
        }
    }
    assert_eq!(s.flow_node_name(), "state_trick_air_enter");
    assert!(s.was_air_starting);
    let mut loop_steps = 0;
    while s.flow_node_name() != "state_trick_air_loop" {
        step(&mut s, &stick(90.0));
        loop_steps += 1;
        assert!(loop_steps < 20);
    }
    step(&mut s, &stick(100.0));
    assert!(!s.was_air_starting);
    assert!((s.tweak_enter - 100f32.to_radians()).abs() < 1e-5 && s.tweak_wind == s.tweak_enter);
    step(&mut s, &stick(150.0));
    assert!((s.tweak_enter - 100f32.to_radians()).abs() < 1e-5);
    assert_eq!(s.tweak_blend, 1.0);
}

#[test]
fn classic_signature_is_all_four_shoulders() {
    let ctx = TrickContext { awesome: true, super_awesome: true, ..air() };
    let shoulders = |lb, rb, lt, rt| TrickInput { lb, rb, lt, rt, signature: lt && rt, ..TrickInput::default() };
    let run = |inp: TrickInput| {
        let mut s = TrickState { classic: true, ..airborne() };
        for _ in 0..3 {
            s.scene_op(&tuning(), &inp, &ctx, 2.0);
            s.flow(&ctx, 2.0);
            s.script_tags();
        }
        s.trick_type
    };
    assert_eq!(run(shoulders(true, true, true, true)), TrickType::Signature);
    assert_ne!(run(shoulders(false, false, true, true)), TrickType::Signature);
}

#[test]
fn reset_trick_state_clears_the_grab_node_signals() {
    let y = btn(true, false, false, false);
    let mut s = airborne();
    (0..20).for_each(|_| step(&mut s, &y));
    assert!(s.in_loop());
    s.grind.gesture_len_locked = 2;
    s.reset_trick_state();
    assert!(!s.in_loop() && !s.anticipating() && !s.was_air_starting);
    assert_eq!(s.grind.gesture_len_locked, -1);
    s.scene_op(&tuning(), &y, &air(), 2.0);
    assert!(!s.trick_locked);
}

#[test]
fn is_double_input_gesture_is_the_lua_expression() {
    let mut s = TrickState { gesture_len: 1, ..TrickState::default() };
    assert!(s.double_input_gesture());
    s.classic = true;
    assert!(!s.double_input_gesture());
    s.trick_type = TrickType::grind(23).expect("Grind_4");
    assert!(s.double_input_gesture());
}

#[test]
fn a_left_air_node_keeps_its_sets_while_the_graph_updates_it() {
    let y = btn(true, false, false, false);
    let mut s = airborne();
    (0..20).for_each(|_| step(&mut s, &y));
    step(&mut s, &TrickInput::default());
    assert_eq!(s.flow_node_name(), "state_trick_air_exit");
    s.begin_step();
    s.land();
    assert!(s.scripts.now.grab_reset && s.scripts.now.grab_exit);
    let path = [ssx_assets::anim_eval::PathNode {
        flow: "SF_AirTricks".into(),
        name: "state_trick_air_exit".into(),
        time: 4.0,
        serial: 42,
    }];
    s.left_capture(&path);
    s.left_step(Some(&[42]), 2.0);
    s.script_tags();
    assert_eq!(s.left_sets.len(), 1);
    s.begin_step();
    s.left_step(Some(&[]), 2.0);
    assert!(s.left_sets.is_empty() && s.scripts.now.grab_reset);
}

#[test]
fn a_mid_air_stance_switch_restarts_the_flow() {
    let mut s = airborne();
    step(&mut s, &TrickInput::default());
    s.virtual_stick_timer = 5.0;
    s.begin_step();
    s.mirror_rebuild(false);
    assert_eq!(s.left_sets.len(), 1);
    s.flow(&air(), 2.0);
    s.left_step(None, 2.0);
    s.script_tags();
    assert_eq!(s.virtual_stick_timer, -1.0, "the new flow's `state_air_states` reset");
}

#[test]
fn a_new_hand_or_grab_restarts_the_every_update_choosers_sequence() {
    let x = btn(false, true, false, false);
    let mut s = airborne();
    step(&mut s, &TrickInput::default());
    step(&mut s, &x);
    assert_eq!((s.flow_node_name(), s.hand, s.phase_ticks), ("state_trick_air_anticipate", TrickHand::None, 2.0));
    step(&mut s, &x);
    assert_eq!((s.hand, s.phase_ticks), (TrickHand::Forehand, 2.0), "restarted");
    let mut steps = 1;
    while s.flow_node_name() == "state_trick_air_anticipate" {
        step(&mut s, &x);
        steps += 1;
    }
    assert_eq!((steps, s.flow_node_name()), (8, "state_trick_air_enter"));
    let y = btn(true, false, false, false);
    let mut s = airborne();
    step(&mut s, &TrickInput::default());
    step(&mut s, &y);
    step(&mut s, &y);
    assert_eq!((s.hand, s.phase_ticks), (TrickHand::Both, 4.0));
    assert_eq!(Node::Enter(TrickLevel::Normal).chooser_entry(TrickHand::Both, TrickType::Rocket), Some(2));
    assert_eq!(Node::Enter(TrickLevel::Uber).chooser_entry(TrickHand::Both, TrickType::FlyingSquirrel), Some(4));
    assert_eq!(Node::Enter(TrickLevel::Normal).chooser_entry(TrickHand::None, TrickType::None), Some(11));
    assert_eq!(Node::Loop(TrickLevel::Normal).chooser_entry(TrickHand::Both, TrickType::Rocket), None);
}

#[test]
fn the_uber_trigger_is_written_by_the_flows_tags() {
    let y = btn(true, false, false, false);
    let mut s = airborne();
    let mut writes = Vec::new();
    for i in 0..40 {
        step(&mut s, &if i < 25 { y } else { TrickInput::default() });
        if let Some(v) = s.uber_trigger_tag.take() {
            writes.push((s.flow_node_name(), v));
        }
    }
    assert_eq!(writes, vec![("state_trick_air_enter", true), ("state_trick_air_exit", false)]);
    use TrickLevel::*;
    assert_eq!(Node::uber_trigger_at_take(Node::Loop(Uber), Node::Enter(SuperUber)), Some(true));
    assert_eq!(Node::uber_trigger_at_take(Node::AirStates, Node::UberCrashRecover), Some(false));
    assert_eq!(Node::uber_trigger_at_take(Node::Enter(Normal), Node::Loop(Normal)), None);
    assert_eq!(Node::uber_trigger_at_take(Node::TweakEnter(Uber), Node::TweakExit(Uber)), None);
    assert_eq!(Node::SigExit(1).uber_trigger_at_first_update(), Some(false));
    assert_eq!(Node::TweakExit(SuperUber).uber_trigger_at_first_update(), Some(false));
    assert_eq!(Node::TweakExit(Uber).uber_trigger_at_first_update(), None);
}

#[test]
fn a_classic_grind_changes_group_only_from_a_short_gesture() {
    let ctx = TrickContext { is_grinding: true, ..TrickContext::default() };
    let rt = 5;
    let run = |classic: bool, gl: i32| {
        let mut s = TrickState {
            classic,
            gesture_len: gl,
            trick_type: TrickType::grind(23).expect("Grind_4"),
            ..TrickState::default()
        };
        (s.grind.group, s.grind.index) = (0, -1);
        s.grind.button[rt] = true;
        s.choose_grind_trick(&tuning(), &TrickInput::default(), &ctx, 2.0);
        (s.grind.group, s.grind.index)
    };
    assert_eq!(run(true, 1), (1, rt as i32));
    assert_eq!(run(true, 2), (0, -1), "classic, LB+RB+LT's length 2: no change");
    assert_eq!(run(false, 2), (2, 5));
}

#[test]
fn a_left_exit_before_its_first_update_still_lowers_the_trigger() {
    let left = |updates: u32, running: bool| {
        let mut s = airborne();
        step(&mut s, &TrickInput::default());
        let exit = Node::SigExit(1);
        s.set_node(exit);
        s.node_tags = NodeTags { updates, ..NodeTags::new(exit.script()) };
        s.begin_step();
        s.leave_air_node();
        let path = [ssx_assets::anim_eval::PathNode {
            flow: "SF_AirTricks".into(),
            name: exit.name().into(),
            time: 0.0,
            serial: 7,
        }];
        s.left_capture(&path);
        s.left_step(Some(if running { &[7] } else { &[] }), 2.0);
        s.uber_trigger_tag
    };
    assert_eq!(left(0, true), Some(false), "its first update as the old side");
    assert_eq!(left(1, true), None, "already written at its own first update");
    assert_eq!(left(0, false), None, "dropped by the blend: exited, never entered");
}

#[test]
fn leaving_state_riding_calls_on_bail_exit() {
    let mut s = TrickState::default();
    s.begin_step();
    s.riding_node_update(2.0);
    assert!(!s.script_tags().any(), "the entry calls nothing");
    s.begin_step();
    s.riding_node_update(2.0);
    assert!(!s.script_tags().any(), "nor the node's updates");
    s.begin_step();
    s.leave_riding_node();
    assert!(s.riding_tags.is_none());
    assert!(s.script_tags().bail_exit, "the take's exit: this step's run");
    s.begin_step();
    s.riding_node_update(2.0);
    s.leave_riding_node();
    assert!(!s.script_tags().bail_exit);
    s.begin_step();
    assert!(s.script_tags().bail_exit, "the set updated in that step exits in the next run");
    s.begin_step();
    s.riding_node_update(2.0);
    s.fresh_ride();
    s.leave_riding_node();
    assert!(!s.script_tags().any());
}
