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
    let mut s = WingsuitState::default();
    let seen: Vec<f32> = (1..=17)
        .map(|k| {
            s.handle_tags(Some(Enter), k as f32);
            s.handle_weight
        })
        .collect();
    assert_eq!(&seen[..4], &[0.0; 4]);
    assert!((seen[5] - 0.316_406_25).abs() < 1e-6, "t 6 = w ⅓: {seen:?}");
    assert!(seen[6] > seen[5] && seen[9] == 1.0 && seen[16] == 1.0, "{seen:?}");
    s.handle_tags(Some(Fly), 1.0);
    assert!(s.handle_tags.is_none() && s.handle_weight == 1.0);
    let seen: Vec<f32> = (1..=20)
        .map(|k| {
            s.handle_tags(Some(Exit), k as f32);
            s.handle_weight
        })
        .collect();
    assert_eq!(seen[9], 1.0, "t 10: w 0");
    assert!((seen[11] - (1.0 - 0.389_489_53)).abs() < 1e-6, "{seen:?}");
    assert!(seen[14] == 0.0 && seen[19] == 0.0, "{seen:?}");
    s.handle_tags(None, 0.0);
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
