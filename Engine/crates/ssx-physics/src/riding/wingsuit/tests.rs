use glam::{Quat, Vec2, Vec3};

use super::*;
use crate::testutil::tuning;

struct Case {
    j: Quat,
    v: Vec3,
    w: Quat,
    wind: Vec3,
    stick: Vec2,
    eff: f32,
    touched: bool,
    time: f32,
    t: Vec3,
    dq: Quat,
    lift: Vec3,
    time_out: f32,
    use_: f32,
}

#[rustfmt::skip]
#[allow(clippy::excessive_precision, clippy::unreadable_literal)]
fn cases() -> Vec<Vec<Case>> {
    vec![
    vec![
        Case { j: Quat::from_xyzw(0.0514496508, -0.0291402084, -0.020730306, 0.998035088), v: Vec3::new(-0.567971304, 0.0341066866, 0.615708604), w: Quat::from_xyzw(-0.00390363879, -0.0073477936, -0.00190717664, 0.999963566), wind: Vec3::new(-0.0430735237, 0.0, 0.0293751022), stick: Vec2::new(0.0249124793, 0.0101683069), eff: 1.0, touched: false, time: 5.0,
               t: Vec3::new(-1.06404352, 0.0841237679, 1.28906512), dq: Quat::from_xyzw(-0.00209090789, -0.0189659856, -0.00629739976, 0.999798119), lift: Vec3::new(-0.0, 0.0, -0.0), time_out: 7.0, use_: 0.0218749996 },
        Case { j: Quat::from_xyzw(0.0491428011, -0.0476957001, -0.0280478694, 0.997257952), v: Vec3::new(-0.535494567, -0.413844039, -0.915329167), w: Quat::from_xyzw(-0.0105303184, -0.00325394916, 0.0130823296, 0.999853678), wind: Vec3::new(-0.000244768192, 0.0, -0.0384190189), stick: Vec2::new(0.995122418, 0.477375504), eff: 0.6, touched: false, time: 7.0,
               t: Vec3::new(-1.18598986, -1.06545174, -1.62415206), dq: Quat::from_xyzw(-0.00673565594, -0.0091739241, -0.0134147387, 0.999845207), lift: Vec3::new(-0.0, 0.0, -0.0), time_out: 9.0, use_: 0.046875 },
        Case { j: Quat::from_xyzw(0.0428005257, -0.0559889294, -0.0421935784, 0.996620819), v: Vec3::new(0.903928234, -0.305242687, -0.690373887), w: Quat::from_xyzw(-0.0155011504, 0.00169783066, -0.00188366852, 0.999876634), wind: Vec3::new(-0.00152963612, 0.0, -0.0313091236), stick: Vec2::new(0.635018886, -0.750245798), eff: 0.0003, touched: false, time: 9.0,
               t: Vec3::new(1.51333833, -0.625944018, -1.38466096), dq: Quat::from_xyzw(-0.00782969687, -0.00630151154, -0.0174007919, 0.999798059), lift: Vec3::new(-0.0, 0.0, -0.0), time_out: 11.0, use_: 1.0 },
    ],
    vec![
        Case { j: Quat::from_xyzw(-0.533213198, 0.421498583, -0.305433062, 0.666883254), v: Vec3::new(-0.58287557, 0.175015198, 0.787931661), w: Quat::from_xyzw(-0.000425609915, -0.0012107492, 0.00198942302, 0.999997198), wind: Vec3::new(0.0327018085, 0.0, -0.0298920577), stick: Vec2::new(0.509584903, 0.525585292), eff: 1.0, touched: true, time: 100.0,
               t: Vec3::new(-0.907766163, -1.47640836, -0.917854309), dq: Quat::from_xyzw(-0.00450471463, 0.00033223824, 0.00270440429, 0.999986172), lift: Vec3::new(0.00182229723, -0.00340860314, -0.000940621889), time_out: 0.0, use_: 0.318749994 },
        Case { j: Quat::from_xyzw(-0.534968548, 0.424532218, -0.301903731, 0.665158016), v: Vec3::new(0.846334892, 0.110047773, -0.180796619), w: Quat::from_xyzw(0.0103823316, -0.0449791377, 0.0103081976, 0.998880786), wind: Vec3::new(-0.0406737468, 0.0, 0.0218092566), stick: Vec2::new(-0.269558907, -0.0973567441), eff: 0.6, touched: false, time: 0.0,
               t: Vec3::new(0.859239101, 0.260419279, 1.47470057), dq: Quat::from_xyzw(-0.00106008409, -0.000101703845, 0.00047987132, 0.999999344), lift: Vec3::new(-0.0, 0.0, 0.0), time_out: 2.0, use_: 0.010416667 },
        Case { j: Quat::from_xyzw(-0.53550029, 0.425041039, -0.301079886, 0.664778502), v: Vec3::new(0.985646814, -0.203191388, -0.016920789), w: Quat::from_xyzw(-0.0112179871, 0.00439496233, 0.0022796609, 0.999924819), wind: Vec3::new(-0.0319322641, 0.0, -0.032990815), stick: Vec2::new(0.100861797, 0.49635204), eff: 0.0003, touched: false, time: 2.0,
               t: Vec3::new(1.18846834, -0.162050903, 1.32934737), dq: Quat::from_xyzw(0.00238363864, 0.000951418304, -0.00323363766, 0.999991417), lift: Vec3::new(-0.0, 0.0, 0.0), time_out: 4.0, use_: 1.0 },
    ],
    vec![
        Case { j: Quat::from_xyzw(-0.0912315009, 0.256682692, -0.0921894481, 0.957753577), v: Vec3::new(-0.386772092, -0.517381417, -0.868712113), w: Quat::from_xyzw(-0.0176092868, 0.0258038649, 0.0247553937, 0.999205306), wind: Vec3::new(0.0458280118, 0.0, -0.00929853401), stick: Vec2::new(0.627391684, 0.554037175), eff: 1.0, touched: false, time: 250.0,
               t: Vec3::new(0.385840774, -0.735430121, -2.00867844), dq: Quat::from_xyzw(-0.0632357821, 0.0345549434, 0.0243558828, 0.997102797), lift: Vec3::new(-0.0, 0.0, 0.0), time_out: 252.0, use_: 0.787500083 },
        Case { j: Quat::from_xyzw(-0.142094146, 0.297085846, -0.0555163912, 0.942585372), v: Vec3::new(0.002, -0.9, 0.001), w: Quat::from_xyzw(-0.00286014351, -0.012107101, 0.00028716008, 0.999922575), wind: Vec3::new(-0.025279221, 0.0, 0.0376144605), stick: Vec2::new(0.978219258, 0.922859531), eff: 0.6, touched: false, time: 252.0,
               t: Vec3::new(0.335311353, -1.74104619, -0.441445231), dq: Quat::from_xyzw(-0.0639155358, 0.038064111, 0.012302205, 0.997153342), lift: Vec3::new(-0.0, 0.0, 0.0), time_out: 254.0, use_: 1.0 },
        Case { j: Quat::from_xyzw(-0.196167489, 0.337415215, -0.0301827598, 0.920194702), v: Vec3::new(-0.675414867, -0.544531879, 0.610563552), w: Quat::from_xyzw(-0.0115984463, 0.0154127849, 0.00832968905, 0.999779245), wind: Vec3::new(-0.00804486629, 0.0, -0.0329370324), stick: Vec2::new(0.547833245, -0.0336541525), eff: 0.0003, touched: false, time: 254.0,
               t: Vec3::new(-1.39061558, -1.35528052, -0.390155435), dq: Quat::from_xyzw(-0.065220423, 0.0414898656, 0.0173982102, 0.996856213), lift: Vec3::new(0.00910524372, -0.00222086045, -0.00719851302), time_out: 256.0, use_: 1.0 },
    ],    ]
}

fn close(a: Vec3, b: Vec3, tol: f32) -> bool {
    (a - b).abs().max_element() <= tol
}

#[test]
fn the_controller_matches_the_emulated_update() {
    let t = tuning();
    let r = ResolvedCharacterProperties::resolve(&t.cp_max).expect("properties");
    for (n, seq) in cases().iter().enumerate() {
        let mut c = WingsuitController::new(&t.wingsuit);
        for (k, e) in seq.iter().enumerate() {
            let i = ControllerInputs {
                j: e.j,
                velocity: e.v,
                angular_velocity: e.w,
                wind: e.wind,
                stick: e.stick,
                effectiveness: e.eff,
                time_scalar: 1.0,
            };
            let mut g = ControllerGameState { time: e.time, has_touched_ground: e.touched, goofy: true };
            let o = c.update(&t.wingsuit, &r, &i, &mut g, 2.0).expect("a step of time");
            let at = format!("case {n} update {k}");
            assert!(close(o.t, e.t, 2e-4), "{at}: t {} vs {}", o.t, e.t);
            let dq = if o.dq.dot(e.dq) < 0.0 { -o.dq } else { o.dq };
            assert!((Vec4from(dq) - Vec4from(e.dq)).abs().max_element() < 2e-5, "{at}: dq {dq} vs {}", e.dq);
            assert!(close(o.lift, e.lift, 2e-6), "{at}: lift {} vs {}", o.lift, e.lift);
            assert!((g.time - e.time_out).abs() < 1e-4 && (o.use_percentage - e.use_).abs() < 1e-5, "{at}");
            assert_eq!(g.goofy, k != 0, "{at}: the first update clears BLN_Goofy, later ones leave it");
            assert!(!g.has_touched_ground);
        }
    }
}

#[allow(non_snake_case)]
fn Vec4from(q: Quat) -> glam::Vec4 {
    glam::Vec4::new(q.x, q.y, q.z, q.w)
}

#[test]
fn the_velocity_frame_lies_along_the_travel_with_wings_level() {
    let t = tuning().wingsuit;
    let v = Vec3::new(0.5, -0.2, 0.1);
    let q = velocity_frame(&t, Quat::IDENTITY, v, v.length());
    assert!(close(q * Vec3::Y, v.normalize(), 1e-5));
    assert!((q * Vec3::Z).y.abs() < 1e-6 && (q * Vec3::X).y < 0.0);
    let j = Quat::from_rotation_z(0.7);
    assert_eq!(velocity_frame(&t, j, Vec3::new(0.01, 0.0, 0.0), 0.01), j);
    let r = rates_to_rotation(&t, 2.0, 0.4, -0.6, 0.8);
    let e = Quat::from_xyzw(0.0, 0.0, 0.0, 1.0)
        * Quat::from_rotation_y(t.max_roll_vel * 0.8)
        * Quat::from_rotation_z(-t.max_pitch_vel * 1.2)
        * Quat::from_rotation_x(t.max_yaw_vel * 1.6);
    assert!(r.abs_diff_eq(e, 1e-6));
}

#[test]
fn the_retract_keeps_the_stance_of_the_travel() {
    let r = ResolvedCharacterProperties::resolve(&tuning().cp_max).expect("properties");
    let j = joint_of(Quat::IDENTITY);
    let (_, fwd) = ballistic_step(&r, Vec3::new(0.3, 0.0, 0.0), Vec3::ZERO, j, true, 2.0);
    let (_, back) = ballistic_step(&r, Vec3::new(-0.3, 0.0, 0.0), Vec3::ZERO, j, false, 2.0);
    let (_, kept) = ballistic_step(&r, Vec3::new(0.0, -0.3, 0.0), Vec3::ZERO, j, true, 2.0);
    assert!(!fwd && back && kept);
    let (v, _) = ballistic_step(&r, Vec3::ZERO, Vec3::ZERO, j, false, 2.0);
    assert!((v.y - r.g_air.y * 2.0).abs() < 1e-7);
}

#[test]
fn the_wear_follows_durability_and_the_handles_decay() {
    let t = tuning();
    let r = ResolvedCharacterProperties::resolve(&t.cp_max).expect("properties");
    let mut s = WingsuitState { signal: true, handle_weight: 1.0, ..WingsuitState::default() };
    s.wear(&t.wingsuit, &r, true, 2.0);
    assert!((s.effectiveness - (1.0 - 0.000_138_333_33 / 3.0 * 2.0)).abs() < 1e-7);
    s.wear(&t.wingsuit, &r, false, 2.0);
    s.decay_handles(&t.wingsuit, 2.0);
    assert_eq!(s.handle_weight, 1.0, "no decay in the wingsuit");
    s.signal = false;
    s.decay_handles(&t.wingsuit, 2.0);
    assert!((s.handle_weight - 0.75).abs() < 1e-6);
}

#[test]
fn the_sequences_raise_and_lower_the_handles() {
    use crate::wingsuit::WingsuitNode::{Enter, Exit, Fly};
    let f = crate::testutil::wingsuit_flow();
    let mut s = WingsuitState::default();
    let seen: Vec<f32> = (1..=17)
        .map(|k| {
            s.handle_tags(&f, Some(Enter), k as f32);
            s.handle_weight
        })
        .collect();
    assert_eq!(&seen[..4], &[0.0; 4]);
    assert!((seen[5] - 0.316_406_25).abs() < 1e-6, "t 6 = w ⅓: {seen:?}");
    assert!(seen[6] > seen[5] && seen[9] == 1.0 && seen[16] == 1.0, "{seen:?}");
    s.handle_tags(&f, Some(Fly), 1.0);
    assert!(s.handle_tags.is_none() && s.handle_weight == 1.0);
    let seen: Vec<f32> = (1..=20)
        .map(|k| {
            s.handle_tags(&f, Some(Exit), k as f32);
            s.handle_weight
        })
        .collect();
    assert_eq!(seen[9], 1.0, "t 10: w 0");
    assert!((seen[11] - (1.0 - 0.389_489_53)).abs() < 1e-6, "{seen:?}");
    assert!(seen[14] == 0.0 && seen[19] == 0.0, "{seen:?}");
    s.handle_tags(&f, None, 0.0);
    assert!(s.handle_tags.is_none());
}

fn airborne(w: &crate::rider::World) -> crate::rider::RiderState {
    use crate::rider::{RideState, RiderState};
    let mut s = RiderState::spawn(w, Vec3::ZERO, Vec3::Z).expect("ground");
    s.traj += Vec3::Y * 150.0;
    (s.traj_cached, s.traj_cached_prev) = (s.traj, s.traj);
    (s.velocity, s.traj_velocity) = (Vec3::Z * 0.6, Vec3::Z * 0.6);
    (s.state, s.on_ground, s.first_air_tick) = (RideState::Airborne, false, true);
    s.close = crate::probe::CloseToGround::default();
    s
}

fn step_ws(s: &mut crate::rider::RiderState, w: &crate::rider::World, r1: bool, flow: crate::death::GameFlowFlags) {
    use crate::rider::{Input, StepContext, TrickInput, step_full};
    s.wingsuit.pad_r1 = r1;
    let x = StepContext { flow, ..StepContext::default() };
    step_full(s, w, &Input::default(), &TrickInput::default(), crate::DT_TICKS, x);
}

#[test]
fn r1_deploys_flies_and_retracts_through_the_nodes() {
    use crate::death::GameFlowFlags;
    use crate::rider::RideState;
    let w = crate::testutil::world(crate::testutil::plane(3000.0, 0.3));
    let started = GameFlowFlags { race_proper_started: true, ..GameFlowFlags::default() };
    let mut s = airborne(&w);
    step_ws(&mut s, &w, false, GameFlowFlags::default());
    step_ws(&mut s, &w, true, GameFlowFlags::default());
    assert_eq!(s.state, RideState::Airborne);
    let mut s = airborne(&w);
    step_ws(&mut s, &w, false, started);
    step_ws(&mut s, &w, true, started);
    assert!(s.state == RideState::WingsuitEnter && s.wingsuit.signal);
    for _ in 0..7 {
        step_ws(&mut s, &w, true, started);
    }
    assert_eq!(s.state, RideState::WingsuitEnter, "16 ticks into the deploy (the entry update ran 2)");
    step_ws(&mut s, &w, true, started);
    assert_eq!(s.state, RideState::WingsuitFly, "the End window (17) falls in the node's 9th update");
    let mut fall = airborne(&w);
    for _ in 0..90 {
        step_ws(&mut s, &w, true, started);
        step_ws(&mut fall, &w, false, started);
    }
    assert_eq!(s.state, RideState::WingsuitFly);
    assert!(s.traj.y > fall.traj.y + 20.0, "the wingsuit falls slower: {} vs {}", s.traj.y, fall.traj.y);
    assert!(s.traj_velocity.y <= 1e-6, "MaximumYVelocity 0: no climbing ({})", s.traj_velocity.y);
    assert!(s.wingsuit.time > 180.0 && s.wingsuit.use_percentage > 0.5);
    step_ws(&mut s, &w, false, started);
    assert_eq!(s.state, RideState::WingsuitExit);
    for _ in 0..8 {
        step_ws(&mut s, &w, false, started);
    }
    assert_eq!(s.state, RideState::WingsuitExit, "18 ticks into the retract");
    step_ws(&mut s, &w, false, started);
    assert_eq!(s.state, RideState::WingsuitExit);
    step_ws(&mut s, &w, false, started);
    assert!(s.state == RideState::Airborne && !s.wingsuit.signal && s.wingsuit.blend.is_some());
    assert!(s.wingsuit.time > 180.0);
}

#[test]
fn a_restored_wingsuit_flag_resumes_the_flight_and_a_low_deploy_lands_at_once() {
    use crate::death::GameFlowFlags;
    use crate::rider::RideState;
    let w = crate::testutil::world(crate::testutil::plane(3000.0, 0.3));
    let started = GameFlowFlags { race_proper_started: true, ..GameFlowFlags::default() };
    let mut s = airborne(&w);
    (s.wingsuit.signal, s.wingsuit.hold) = (true, true);
    step_ws(&mut s, &w, true, started);
    assert_eq!(s.state, RideState::WingsuitFly);
    step_ws(&mut s, &w, false, started);
    assert_eq!(s.state, RideState::WingsuitExit);
    let mut s = airborne(&w);
    s.traj -= Vec3::Y * 149.0;
    (s.traj_cached, s.traj_cached_prev) = (s.traj, s.traj);
    step_ws(&mut s, &w, false, started);
    assert!(s.close.is_very_close, "1 m above the slope");
    step_ws(&mut s, &w, true, started);
    assert_eq!(s.state, RideState::Riding, "deploy → land within the update");
    assert!(!s.wingsuit.signal);
}

#[test]
fn the_wingsuit_needs_the_item() {
    use crate::character::{GearStats, SLOT_WINGSUIT};
    use crate::death::GameFlowFlags;
    use crate::rider::RideState;
    let started = GameFlowFlags { race_proper_started: true, ..GameFlowFlags::default() };
    let mut no_suit = GearStats::default_gear();
    no_suit.slots[SLOT_WINGSUIT] = false;
    let deploys = |gear: &GearStats, script: Option<bool>| {
        let mut w = crate::testutil::world(crate::testutil::plane(3000.0, 0.3));
        w.equip(gear).expect("gear");
        let mut s = airborne(&w);
        step_ws(&mut s, &w, false, started);
        if let Some(v) = script {
            s.wingsuit.enabled = v;
        }
        step_ws(&mut s, &w, false, started);
        step_ws(&mut s, &w, true, started);
        (s.state == RideState::WingsuitEnter, s.wingsuit.enabled)
    };
    assert_eq!(deploys(&no_suit, None), (false, false));
    assert_eq!(deploys(&GearStats::default_gear(), None), (true, true));
    assert_eq!(deploys(&GearStats::none(), None), (true, true), "no gear list: every item flag true");
    assert_eq!(deploys(&no_suit, Some(true)), (true, true), "the tutorial's enable");
    assert_eq!(deploys(&GearStats::default_gear(), Some(false)), (false, false), "the tutorial's disable");
}

fn open_and_close(hold_ticks: usize) -> (Vec<f32>, f32) {
    use crate::death::GameFlowFlags;
    use crate::rider::RideState;
    let w = crate::testutil::world(crate::testutil::plane(3000.0, 0.3));
    let started = GameFlowFlags { race_proper_started: true, ..GameFlowFlags::default() };
    let v = 80.0 / crate::KMH_PER_M_PER_TICK;
    let mut s = airborne(&w);
    (s.velocity, s.traj_velocity) = (Vec3::Z * v, Vec3::Z * v);
    let mut fall = s.clone();
    let mut speeds = Vec::new();
    for k in 0..hold_ticks + 61 {
        step_ws(&mut s, &w, (1..=hold_ticks).contains(&k), started);
        step_ws(&mut fall, &w, false, started);
        speeds.push(s.traj_velocity.with_y(0.0).length());
        if k == hold_ticks {
            assert!(s.state.is_wingsuit(), "{:?}", s.state);
        }
    }
    assert!(s.state == RideState::Airborne && s.wingsuit.blend.is_none(), "{:?}", s.state);
    assert_eq!(fall.state, RideState::Airborne);
    (speeds, fall.traj_velocity.with_y(0.0).length())
}

#[test]
fn closing_the_wingsuit_right_after_opening_keeps_the_speed() {
    for hold in [1, 3, 8] {
        let (speeds, fall) = open_and_close(hold);
        let min = speeds.iter().copied().fold(f32::MAX, f32::min);
        assert!(min > 0.8 * fall, "hold {hold}: {min} vs {fall} m/tick without the wingsuit ({speeds:?})");
    }
}

#[test]
fn closing_the_wingsuit_after_a_flight_keeps_the_speed() {
    let (speeds, fall) = open_and_close(45);
    let min = speeds.iter().copied().fold(f32::MAX, f32::min);
    assert!(min > 0.8 * fall, "{min} vs {fall} m/tick without the wingsuit");
}

#[test]
fn the_exited_aerial_keeps_its_outputs_off_the_entity() {
    use crate::death::GameFlowFlags;
    use crate::rider::{Input, RideState, StepContext, TrickInput, step_full};
    let w = crate::testutil::world(crate::testutil::plane(3000.0, 0.3));
    let started = GameFlowFlags { race_proper_started: true, ..GameFlowFlags::default() };
    let mut s = airborne(&w);
    step_ws(&mut s, &w, false, started);
    let before = s.air.outputs;
    let stick = Input { steer: 1.0, ..Input::default() };
    let x = || StepContext { flow: started, ..StepContext::default() };
    s.wingsuit.pad_r1 = true;
    step_full(&mut s, &w, &stick, &TrickInput::default(), crate::DT_TICKS, x());
    assert_eq!(s.state, RideState::WingsuitEnter);
    assert_eq!(s.air.outputs, before, "the deploy's update: the aerial already exited");
    let spring0 = s.air.spin_wiggle;
    for _ in 0..2 {
        step_full(&mut s, &w, &stick, &TrickInput::default(), crate::DT_TICKS, x());
        assert!(s.wingsuit.blend.is_some(), "within the 8-tick blend");
        assert_eq!(s.air.outputs, before, "the entity's outputs stay as the aerial left them");
        assert!(s.wingsuit.aerial_layer.is_none(), "the layer goes to the graph in the step");
    }
    assert_ne!(s.air.spin_wiggle, spring0, "the slave kept updating its spring on the stick");
}

#[test]
fn the_deploy_raises_the_uber_trigger_and_the_close_lowers_it() {
    use crate::death::GameFlowFlags;
    use crate::rider::RideState;
    let w = crate::testutil::world(crate::testutil::plane(3000.0, 0.3));
    let started = GameFlowFlags { race_proper_started: true, ..GameFlowFlags::default() };
    let mut s = airborne(&w);
    step_ws(&mut s, &w, false, started);
    assert!(!s.uber_trigger);
    step_ws(&mut s, &w, true, started);
    assert!(s.state == RideState::WingsuitEnter && s.uber_trigger, "airborne → wingsuit_enter writes true");
    for _ in 0..40 {
        step_ws(&mut s, &w, true, started);
    }
    assert!(s.state == RideState::WingsuitFly && s.uber_trigger, "kept through the deploy's End and the flight");
    step_ws(&mut s, &w, false, started);
    assert!(s.state == RideState::WingsuitExit && !s.uber_trigger, "fly → exit writes false");
    let mut s = airborne(&w);
    (s.wingsuit.signal, s.wingsuit.hold) = (true, true);
    step_ws(&mut s, &w, true, started);
    assert!(s.state == RideState::WingsuitFly && s.uber_trigger, "airborne → wingsuit_fly writes true");
}

#[test]
fn landing_in_the_suit_is_an_uber_crash_and_closing_first_is_not() {
    use crate::death::GameFlowFlags;
    use crate::rider::RideState;
    let w = crate::testutil::world(crate::testutil::plane(3000.0, 0.3));
    let started = GameFlowFlags { race_proper_started: true, ..GameFlowFlags::default() };
    let fly = |close_after: Option<usize>| {
        let mut s = airborne(&w);
        s.traj -= Vec3::Y * 125.0;
        (s.traj_cached, s.traj_cached_prev) = (s.traj, s.traj);
        step_ws(&mut s, &w, false, started);
        let mut flown = 0;
        while s.state != RideState::Riding && flown < 900 {
            let r1 = close_after.is_none_or(|n| flown < n);
            step_ws(&mut s, &w, r1, started);
            flown += 1;
        }
        assert_eq!(s.state, RideState::Riding, "landed within {flown} updates");
        s
    };
    let held = fly(None);
    assert!(held.landing.uber_crash, "landed from a wingsuit node with the trigger up");
    let closed = fly(Some(20));
    assert!(!closed.landing.uber_crash && !closed.uber_trigger, "closed before the ground");
}

#[test]
fn a_node_blend_runs_its_curve() {
    let old = OldNode { side: OldSide::Ballistic { ticks: 0.0 }, blend: None };
    let mut linear = NodeBlend::new(old.clone(), BlendCurve { time: 8.0, curve: 3 });
    let mut cosine = NodeBlend::new(old, BlendCurve { time: 8.0, curve: 0 });
    assert_eq!(linear.advance(2.0), Some(0.75));
    let w = cosine.advance(2.0).expect("blending");
    assert!((w - std::f32::consts::FRAC_PI_8.cos()).abs() < 1e-6, "{w}");
    assert_eq!(linear.advance(6.0), None);
    assert_eq!(cosine.advance(6.0), None);
}

#[test]
fn the_flow_runs_on_the_graphs_windows_blends_and_tags() {
    use crate::death::GameFlowFlags;
    use crate::rider::RideState;
    let started = GameFlowFlags { race_proper_started: true, ..GameFlowFlags::default() };
    let mut w = crate::testutil::world(crate::testutil::plane(3000.0, 0.3));
    let fly_after = |w: &crate::rider::World| {
        let mut s = airborne(w);
        step_ws(&mut s, w, false, started);
        let mut updates = 0;
        while s.state != RideState::WingsuitFly && updates < 60 {
            step_ws(&mut s, w, true, started);
            updates += 1;
        }
        (updates, s)
    };
    let (disc, s) = fly_after(&w);
    assert_eq!(disc, 9, "the entry's update and eight more (End 17, 2-tick updates)");
    assert_eq!(s.wingsuit.blend.as_ref().map(|b| b.blend), Some(BlendCurve { time: 30.0, curve: 0 }));
    w.tuning.wingsuit.flow.deploy_end = 9.0;
    w.tuning.wingsuit.flow.enter_to_fly.blend = BlendCurve { time: 20.0, curve: 3 };
    let (short, s) = fly_after(&w);
    assert_eq!(short, 5, "End 9");
    assert_eq!(s.wingsuit.blend.as_ref().map(|b| b.blend), Some(BlendCurve { time: 20.0, curve: 3 }));
    for (particle, tag, blend) in [(false, false, 15.0), (true, true, 8.0)] {
        let (_, mut s) = fly_after(&w);
        s.board_particle.enabled = particle;
        step_ws(&mut s, &w, false, started);
        assert_eq!(s.state, RideState::WingsuitExit);
        assert_eq!(s.uber_trigger, tag, "board particle {particle}");
        assert_eq!(s.wingsuit.blend.as_ref().map(|b| b.blend.time), Some(blend), "board particle {particle}");
    }
}

#[test]
fn the_flight_keeps_the_carvings_alignment_frame() {
    use crate::death::GameFlowFlags;
    use crate::rider::{Input, RideState, RiderState, StepContext, TrickInput, reverse_camera_stick, step_full};
    let w = crate::testutil::world(crate::testutil::plane(3000.0, 0.3));
    let started = GameFlowFlags { race_proper_started: true, ..GameFlowFlags::default() };
    let mut s = RiderState::spawn(&w, Vec3::ZERO, Vec3::Z).expect("ground");
    assert_eq!(s.alignment_frame, Quat::IDENTITY, "the asset default before any carving");
    for _ in 0..20 {
        step_ws(&mut s, &w, false, started);
    }
    assert_eq!(s.state, RideState::Riding);
    let frame = s.alignment_frame;
    assert_eq!(frame, s.alignment, "the carving's update writes its AlignmentFrame");
    s.traj += Vec3::Y * 150.0;
    (s.traj_cached, s.traj_cached_prev) = (s.traj, s.traj);
    (s.state, s.on_ground, s.first_air_tick) = (RideState::Airborne, false, true);
    s.close = crate::probe::CloseToGround::default();
    step_ws(&mut s, &w, false, started);
    for _ in 0..40 {
        step_ws(&mut s, &w, true, started);
    }
    assert_eq!(s.state, RideState::WingsuitFly);
    assert_eq!(s.alignment_frame, frame, "no air or wingsuit node writes it");
    assert!(s.alignment.angle_between(frame) > 0.1, "the flight's orientation moved on");
    let camera = Quat::from_rotation_y(2.5);
    (s.velocity, s.traj_velocity, s.reverse_camera) = (Vec3::splat(0.01), Vec3::splat(0.01), Some(camera));
    let input = Input { steer: 0.8, ..Input::default() };
    let matched = crate::graph::stick_axes(input.stick());
    let want = reverse_camera_stick(matched, camera, frame, s.kin.fwd, s.traj_velocity);
    let flight = reverse_camera_stick(matched, camera, s.alignment, s.kin.fwd, s.traj_velocity);
    assert!(want.distance(flight) > 1e-3, "the two frames turn the stick apart: {want} vs {flight}");
    s.wingsuit.pad_r1 = true;
    let x = StepContext { flow: started, ..StepContext::default() };
    step_full(&mut s, &w, &input, &TrickInput::default(), crate::DT_TICKS, x);
    assert_eq!(s.pad_stick, want);
}

#[test]
fn a_press_in_the_retracts_last_update_deploys_again() {
    use crate::death::GameFlowFlags;
    use crate::rider::RideState;
    let w = crate::testutil::world(crate::testutil::plane(3000.0, 0.3));
    let started = GameFlowFlags { race_proper_started: true, ..GameFlowFlags::default() };
    let mut s = airborne(&w);
    step_ws(&mut s, &w, false, started);
    for _ in 0..30 {
        step_ws(&mut s, &w, true, started);
    }
    assert_eq!(s.state, RideState::WingsuitFly);
    step_ws(&mut s, &w, false, started);
    assert!(s.state == RideState::WingsuitExit && !s.uber_trigger);
    let mut probe = s.clone();
    let mut k = 0;
    while probe.state == RideState::WingsuitExit {
        step_ws(&mut probe, &w, false, started);
        k += 1;
    }
    assert_eq!((k, probe.state), (10, RideState::Airborne));
    for _ in 1..k {
        step_ws(&mut s, &w, false, started);
    }
    step_ws(&mut s, &w, true, started);
    assert_eq!(s.state, RideState::WingsuitEnter, "airborne → wingsuit_enter in the window's update");
    assert!(s.wingsuit.signal && s.uber_trigger, "the deploy's tag");
    assert_eq!(s.wingsuit.ticks, 2.0, "the window due at the update's start (20 − 20): the deploy runs all of it");
}

#[test]
fn an_uber_crash_that_slides_off_an_edge_deploys_again() {
    use crate::collision::CollisionWorld;
    use crate::death::GameFlowFlags;
    use crate::landing::UberRecovery;
    use crate::rider::RideState;
    let started = GameFlowFlags { race_proper_started: true, ..GameFlowFlags::default() };
    let (sin, cos) = 0.3f32.sin_cos();
    let ledge = |edge: f32| {
        let p = |x: f32, z: f32| Vec3::new(x, -z * sin, z * cos);
        let mut c = CollisionWorld::default();
        let v = [p(-3000.0, -3000.0), p(-3000.0, edge), p(3000.0, edge), p(3000.0, -3000.0)];
        c.push_cluster(&v, None, None, &[[0, 1, 2], [0, 2, 3]], 0, 0);
        c.finish();
        c
    };
    let land = |w: &crate::rider::World| {
        let mut s = airborne(w);
        s.traj -= Vec3::Y * 125.0;
        (s.traj_cached, s.traj_cached_prev) = (s.traj, s.traj);
        step_ws(&mut s, w, false, started);
        let mut flown = 0;
        while s.state != RideState::Riding && flown < 900 {
            step_ws(&mut s, w, true, started);
            flown += 1;
        }
        assert!(s.state == RideState::Riding && s.landing.uber_crash, "an uber crash within {flown} updates");
        s
    };
    let open = land(&crate::testutil::world(ledge(3000.0)));
    let w = crate::testutil::world(ledge(open.traj.z / cos + 10.0));
    let mut s = land(&w);
    assert!(s.traj.distance(open.traj) < 0.01, "the same touchdown (the shorter slope's plane rounds apart)");
    let (mut slid, mut wiping_out) = (0, false);
    while s.state == RideState::Riding && slid < 120 {
        wiping_out = s.landing.tumbling;
        step_ws(&mut s, &w, false, started);
        slid += 1;
    }
    assert_eq!(s.state, RideState::Airborne, "off the edge within {slid} updates");
    assert!(s.board_particle.enabled && s.landing.uber_recovery == UberRecovery::Sliding, "still the crash's slide");
    assert!(wiping_out && !s.landing.tumbling, "BLN_State_Wipeout {wiping_out} on the ground, cleared at the take");
    step_ws(&mut s, &w, true, started);
    assert_eq!(s.state, RideState::WingsuitEnter, "R1 deploys {slid} updates after the touchdown");
}

fn retract_clip() -> RetractRotation {
    use std::f32::consts::FRAC_PI_2;
    let (prone, upright) = (Quat::from_rotation_z(-FRAC_PI_2), Quat::from_rotation_y(-FRAC_PI_2));
    let keys = [(0, prone, Vec3::Y), (10, prone, Vec3::Y), (21, upright, Vec3::Y), (40, upright, Vec3::Y)];
    RetractRotation {
        clip_name: "retract".into(),
        clip: crate::rootmotion::RootClip::from_keys(&keys, 40.0),
        start_tick: 0.0,
        length: 35.0,
        start_time: -5.0,
        begin: 0.0,
        scale: 1.0,
        mirror: false,
    }
}

#[test]
fn the_retract_clip_plays_at_the_sequences_time() {
    use std::f32::consts::FRAC_PI_2;
    let r = retract_clip();
    let still = r.rotation(0.0, 2.0).expect("begun at the node's entry");
    assert!(still.angle_between(Quat::IDENTITY) < 1e-5, "clip 5 → 7 holds: {still}");
    let mut turned = Quat::IDENTITY;
    let mut n = 0.0;
    while n < 35.0 {
        turned *= r.rotation(n, 2.0).expect("playing");
        n += 2.0;
    }
    let want = Quat::from_rotation_z(-FRAC_PI_2).conjugate() * Quat::from_rotation_y(-FRAC_PI_2);
    assert!(turned.angle_between(want) < 1e-4, "{turned} vs {want} (120°)");
    assert!((want.angle_between(Quat::IDENTITY).to_degrees() - 120.0).abs() < 1e-3);
    let mid = r.rotation(10.0, 2.0).expect("playing");
    assert!(mid.angle_between(Quat::IDENTITY) > 0.05, "clip 15 → 17 turns");
    let end = r.rotation(36.0, 2.0).expect("playing");
    assert_eq!(end, Quat::IDENTITY, "the actor's time stops at its Length");
    let mirrored = RetractRotation { mirror: true, ..retract_clip() }.rotation(10.0, 2.0).expect("playing");
    assert!(mirrored.abs_diff_eq(Quat::from_xyzw(-mid.x, -mid.y, mid.z, mid.w), 1e-6));
    let late = RetractRotation { begin: 3.0, ..retract_clip() };
    assert_eq!(late.rotation(0.0, 2.0), None, "the element begins at 3");
    let part = late.rotation(2.0, 2.0).expect("begun within the update");
    let rest = late.clip.joint_delta(8.0, 9.0).expect("sampled").1;
    assert!(part.angle_between(rest) < 1e-6, "clip 8 → 9 only");
}

#[test]
fn the_retract_turns_the_rider_by_its_clip() {
    use crate::death::GameFlowFlags;
    use crate::rider::RideState;
    let started = GameFlowFlags { race_proper_started: true, ..GameFlowFlags::default() };
    let mut w = crate::testutil::world(crate::testutil::plane(3000.0, 0.3));
    let close = |w: &crate::rider::World| {
        let mut s = airborne(w);
        step_ws(&mut s, w, false, started);
        for _ in 0..60 {
            step_ws(&mut s, w, true, started);
        }
        assert_eq!(s.state, RideState::WingsuitFly);
        step_ws(&mut s, w, false, started);
        assert_eq!(s.state, RideState::WingsuitExit);
        let mut checked = 0;
        while s.state == RideState::WingsuitExit {
            let (j0, n0, blending) = (joint_of(s.traj_orientation), s.wingsuit.ticks, s.wingsuit.blend.is_some());
            step_ws(&mut s, w, false, started);
            let r = w.tuning.wingsuit.flow.retract_rotation.as_ref().and_then(|c| c.rotation(n0, 2.0));
            if !blending && s.state == RideState::WingsuitExit {
                let turn = j0.conjugate() * joint_of(s.traj_orientation);
                let want = r.unwrap_or(Quat::IDENTITY);
                assert!(turn.angle_between(want) < 1e-4, "tick {n0}: {turn} vs {want}");
                checked += 1;
            }
        }
        assert_eq!(s.state, RideState::Airborne);
        (checked, (s.traj_orientation * Vec3::Y).angle_between(Vec3::Y).to_degrees())
    };
    let (checked, prone) = close(&w);
    assert!(checked >= 2 && prone > 60.0, "{checked} updates; tilt {prone}° without the clip's turn");
    w.tuning.wingsuit.flow.retract_rotation = Some(retract_clip());
    let (checked, upright) = close(&w);
    assert!(checked >= 2 && upright < 0.6 * prone, "{checked} updates; tilt {upright}° with it (prone {prone}°)");
}
