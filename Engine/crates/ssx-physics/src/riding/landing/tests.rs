use std::f32::consts::{FRAC_PI_2, PI};

use super::*;
use crate::testutil::{balance, tuning};

const DEG: f32 = PI / 180.0;

fn board(yaw: f32, pitch: f32) -> Quat {
    let level = Quat::from_mat3(&glam::Mat3::from_cols(Vec3::Z, Vec3::Y, Vec3::Z.cross(Vec3::Y)));
    level * Quat::from_rotation_y(yaw) * Quat::from_rotation_z(pitch)
}

fn land(yaw: f32, pitch: f32, uber: bool) -> (LandingState, Quat) {
    land_rotating(yaw, pitch, 0.0, uber)
}

fn land_rotating(yaw: f32, pitch: f32, pitch_rate: f32, uber: bool) -> (LandingState, Quat) {
    let t = balance();
    let mut steering = SteeringState::new(&tuning().carving);
    let mut a = board(yaw, pitch);
    let w = Quat::from_scaled_axis((a * Vec3::Z) * pitch_rate);
    let mut l = LandingState::default();
    l.touchdown(
        &t,
        &tuning().carving,
        &mut a,
        &mut steering,
        Vec3::Z * 0.3,
        Vec3::Y,
        w,
        uber,
        true,
        &ImpactKick::default(),
        2.0,
    );
    (l, a)
}

#[test]
fn touchdown_snaps_yaw_to_the_velocity_and_pitch_to_the_ground() {
    let (l, a) = land(20.0 * DEG, 0.0, false);
    assert!((a * Vec3::X).distance(Vec3::Z) < 1e-4 && (a * Vec3::Y).distance(Vec3::Y) < 1e-4);
    assert!((l.spin - 20.0 * DEG).abs() < 1e-4 && l.pitch.abs() < 1e-4);
    let (l, a) = land(0.0, 30.0 * DEG, false);
    assert!((a * Vec3::X).distance(Vec3::Z) < 1e-4 && (a * Vec3::Y).distance(Vec3::Y) < 1e-4);
    assert!(l.spin.abs() < 1e-4 && (l.pitch - 30.0 * DEG).abs() < 1e-4);
    assert!(!l.bail_on_landing && l.quality == LandingQuality::Sloppy && !l.landed_switch);
    let (l, a) = land(20.0 * DEG, 30.0 * DEG, false);
    let roll = (a * Vec3::Z).dot(Vec3::Y).asin();
    assert!((l.spin - 20.0 * DEG).abs() < 1e-4, "spin {}°", l.spin / DEG);
    assert!(twist(crate::steering::ground_frame(a, Vec3::Y).conjugate() * a, Vec3::Z).abs() < 1e-4);
    assert!(roll.abs() < 15.0 * DEG && l.pitch > 25.0 * DEG, "roll {}°, pitch {}°", roll / DEG, l.pitch / DEG);
    eprintln!("20° slip + 30° nose up: residual roll {:.1}°, kept pitch {:.1}°", roll / DEG, l.pitch / DEG);
    let (l, a) = land(PI - 10.0 * DEG, 0.0, false);
    assert!(l.landed_switch && (a * Vec3::X).distance(-Vec3::Z) < 1e-4 && (l.spin.abs() - 10.0 * DEG).abs() < 1e-4);
}

#[test]
fn bail_only_past_135_degrees() {
    let (l, a) = land(0.0, 150.0 * DEG, false);
    assert!(l.bail_on_landing && l.quality == LandingQuality::Bail);
    assert!((a * Vec3::Y).distance(Vec3::Y) < 1e-4, "the board is snapped flat even when bailing");
    let (l, _) = land(0.0, -140.0 * DEG, false);
    assert!(l.bail_on_landing);
    let (l, _) = land(0.0, 130.0 * DEG, false);
    assert!(!l.bail_on_landing && l.quality == LandingQuality::Sloppy);
    let (l, a) = land(0.0, 40.0 * DEG, true);
    assert!(l.uber_crash && l.quality == LandingQuality::UberCrash && l.is_bailing());
    assert!(l.pitch == 0.0 && l.spin == 0.0 && (a * Vec3::Y).distance(Vec3::Y) > 0.5, "not snapped");
}

fn settle(l: &mut LandingState, stick_y: f32, steps: usize) -> Option<usize> {
    let t = balance();
    let mut stable_at = None;
    for i in 0..steps {
        l.balance_update(&t, 1.0, stick_y, 0.3, false, true, false, false, false, false, false, 2.0);
        if l.stable && stable_at.is_none() {
            stable_at = Some(i);
        }
    }
    stable_at
}

#[test]
fn small_pitch_settles_to_a_clean_landing() {
    let (mut l, _) = land(5.0 * DEG, 15.0 * DEG, false);
    let at = settle(&mut l, 0.0, 120).expect("stable");
    assert!(l.quality == LandingQuality::Clean && l.landing_score == 1.0, "{:?}", l.quality);
    assert!(l.pitch.abs() < 25.0 * DEG && l.spin_vel.abs() < 1.0 * DEG && at <= 2, "stable at step {at}");
    eprintln!("15° nose up, 5° yaw: stable after {} ticks, pitch {:.1}°", 2 * at, l.pitch / DEG);
}

#[test]
fn over_pitching_tumbles_and_recovers() {
    let (mut l, _) = land(0.0, -70.0 * DEG, false);
    assert!(!l.bail_on_landing);
    let t = balance();
    l.balance_update(&t, 1.0, 0.0, 0.3, false, true, false, false, false, false, false, 2.0);
    assert_eq!(l.landing_score, 0.0);
    assert_eq!(l.quality, LandingQuality::Tumble);
    assert!(l.pitch > PI && l.tumbling, "{}", l.pitch / DEG);
    assert!(!l.is_upright(&t) && t.tumble_friction_curve.eval(l.pitch) > 0.2, "tumbling brakes by TumbleFrictionCurve");
    for _ in 0..60 {
        l.balance_update(&t, 1.0, 0.0, 0.3, false, true, false, false, false, false, false, 2.0);
    }
    assert!((l.pitch - PI / 2.0).abs() < 3.0 * DEG && !l.stable, "on the back: {}°", l.pitch / DEG);
    l.balance_update(&t, 1.0, 0.0, 0.3, true, true, false, false, false, false, false, 2.0);
    assert!(l.auto_recover_backslide);
    let at = settle(&mut l, 0.0, 600).expect("recovers");
    assert_eq!(l.quality, LandingQuality::Tumble, "the landing stays a tumble");
    eprintln!("70° nose down: tumble, stable again after {} ticks", 2 * (at + 1));
}

#[test]
fn a_backslide_ends_by_itself_unless_the_stick_holds_it() {
    let t = balance();
    let on_back = || {
        let (mut l, _) = land(0.0, -70.0 * DEG, false);
        l.balance_update(&t, 1.0, 0.0, 0.3, false, true, false, false, false, false, false, 2.0);
        for _ in 0..90 {
            l.balance_update(&t, 1.0, -1.0, 0.3, false, true, false, false, false, false, false, 2.0);
        }
        assert!(l.backslide && (l.pitch - PI / 2.0).abs() < 3.0 * DEG, "on the back: {}°", l.pitch / DEG);
        l
    };
    let mut l = on_back();
    for _ in 0..600 {
        l.balance_update(&t, 1.0, -1.0, 0.0, false, true, false, false, false, false, false, 2.0);
    }
    assert!(l.backslide && !l.auto_recover_backslide && !l.stable && l.backslide_ticks <= 2.0, "{}°", l.pitch / DEG);
    let mut l = on_back();
    let mut recovered_at = None;
    for k in 0..600 {
        l.balance_update(&t, 1.0, 0.0, 0.0, false, true, false, false, false, false, false, 2.0);
        if l.auto_recover_backslide && recovered_at.is_none() {
            recovered_at = Some(2.0 * (k + 1) as f32);
        }
    }
    let at = recovered_at.expect("recovers without input");
    assert!((at - t.max_ticks_in_auto_backslide).abs() <= 2.0, "at {at} ticks");
    assert!(!l.backslide && l.pitch.abs() < 25.0 * DEG, "upright again: {}°", l.pitch / DEG);
    let mut l = on_back();
    for _ in 0..600 {
        l.balance_update(&t, 1.0, 1.0, 0.0, false, true, false, false, false, false, false, 2.0);
    }
    assert!(!l.backslide && l.pitch.abs() < 25.0 * DEG, "upright with the helping stick: {}°", l.pitch / DEG);
}

#[test]
fn the_bail_limit_counts_from_the_bail_and_spares_avalanche_tumbles() {
    let mut l = LandingState { ticks: 10.0 * BAIL_ON_LANDING_MAX_TICKS, bail_on_landing: true, ..Default::default() };
    l.landing_bail_limit(false, 2.0);
    assert!(l.bail_on_landing && l.landing_bail_ticks == 2.0);
    let mut tumble = LandingState { bail_on_landing: true, ..Default::default() };
    for _ in 0..1000 {
        tumble.landing_bail_limit(true, 2.0);
    }
    assert!(tumble.bail_on_landing && tumble.landing_bail_ticks == 0.0);
    l.bail_on_landing = false;
    l.landing_bail_limit(false, 2.0);
    assert_eq!(l.landing_bail_ticks, 0.0);
}

#[test]
fn bail_springs_onto_the_back_until_a_fresh_controller_or_the_bail_limit() {
    let (mut l, _) = land_rotating(0.0, 150.0 * DEG, 0.1, false);
    let t = balance();
    assert_eq!(l.adhesion_factor(&tuning().carving.balance_surface_adhesion_modifier_curve), 0.0);
    for _ in 0..30 {
        l.balance_update(&t, 1.0, 0.0, 0.3, false, true, false, false, false, false, false, 2.0);
    }
    assert!(l.bail_on_landing && (l.pitch - PI).abs() < 5.0 * DEG, "pitch {}°", l.pitch / DEG);
    while l.landing_bail_ticks + 2.0 < BAIL_ON_LANDING_MAX_TICKS {
        l.landing_bail_limit(false, 2.0);
        l.balance_update(&t, 1.0, 0.0, 0.3, true, true, false, false, false, false, false, 2.0);
    }
    assert!(l.bail_on_landing && l.tumbling && (l.pitch - PI).abs() < 1.0 * DEG, "pitch {}°", l.pitch / DEG);
    let mut held = l;
    held.fresh_controller();
    assert!(!held.bail_on_landing && !held.is_bailing() && held.pitch == 0.0 && held.landing_score == 1.0);
    assert_eq!(held.quality, LandingQuality::Bail);
    l.landing_bail_limit(false, 2.0);
    l.balance_update(&t, 1.0, 0.0, 0.3, false, true, false, false, false, false, false, 2.0);
    assert!(!l.bail_on_landing && !l.is_bailing());
    for _ in 0..300 {
        l.balance_update(&t, 1.0, 0.0, 0.3, false, true, false, false, false, false, false, 2.0);
    }
    assert!(!l.tumbling && l.pitch.abs() < 25.0 * DEG, "upright after the bail: {}°", l.pitch / DEG);
    assert_eq!(l.quality, LandingQuality::Bail);
}

#[test]
fn spring_acceleration_prediction_is_the_springs_second_derivative() {
    let (k, e0, rate, dt) = (0.012f32.sqrt(), 0.7, -0.05, 2.0);
    let h = 1e-2;
    let r = |t: f32| crit_damped(e0, rate, k, t).1;
    let numeric = (r(dt + h) - r(dt - h)) / (2.0 * h);
    let decoded = spring_accel_after(k, e0, rate, 0.0, dt);
    assert!((numeric - decoded).abs() < 1e-5, "{numeric} vs {decoded}");
}

#[test]
fn spin_target_takes_the_calmer_spring() {
    let (k, dt) = (0.012f32.sqrt(), 2.0);
    let (spin, target) = spin_target(k, 20.0 * DEG, -0.05, 0.0, 1.0, dt);
    assert!(spin == 20.0 * DEG && target == 0.0);
    let (s, v) = (80.0 * DEG, 10.0 * DEG);
    let (_, target) = spin_target(k, s, v, 0.0, 1.0, dt);
    let calmer =
        if spring_accel_after(k, s, v, PI, dt).abs() <= spring_accel_after(k, s, v, 0.0, dt).abs() { PI } else { 0.0 };
    assert_eq!(target, calmer);
    let (spin, target) = spin_target(k, -80.0 * DEG, -10.0 * DEG, 0.0, 1.0, dt);
    if spring_accel_after(k, -80.0 * DEG, -10.0 * DEG, -PI, dt).abs()
        <= spring_accel_after(k, -80.0 * DEG, -10.0 * DEG, 0.0, dt).abs()
    {
        assert!((target - PI).abs() < 1e-5 && (spin - (TAU - 80.0 * DEG)).abs() < 1e-5, "{spin} {target}");
    }
}

#[test]
fn twist_is_the_rotation_about_the_axis() {
    let q = Quat::from_rotation_z(0.4) * Quat::from_rotation_x(0.0);
    assert!((twist(q, Vec3::Z) - 0.4).abs() < 1e-6);
    assert!(twist(Quat::from_rotation_y(-2.0), Vec3::Y) + 2.0 < 1e-6);
    assert!((twist(Quat::from_rotation_z(3.5), Vec3::Z) - (3.5 - TAU)).abs() < 1e-5);
}

#[test]
fn ground_frame_projects_the_board_side() {
    use crate::steering::ground_frame;
    let s = 0.5f32.sqrt();
    let n = Vec3::new(0.0, s, s);
    let a = Quat::from_rotation_y(0.5);
    let g = ground_frame(a, n);
    assert!((g * Vec3::Z).distance(Vec3::new(0.6114, -0.5596, 0.5596)) < 1e-3);
    assert!((g * Vec3::X).distance(Vec3::new(0.7913, 0.4323, -0.4323)) < 1e-3);
    assert!((g * Vec3::Y).distance(n) < 1e-5);
    let a = Quat::from_rotation_x(-60f32.to_radians());
    let g = ground_frame(a, Vec3::Y);
    assert!((g * Vec3::X).distance(Vec3::X) < 1e-5 && (g * Vec3::Y).distance(Vec3::Y) < 1e-5);
}

#[test]
fn an_impact_kicks_the_spin_sideways_and_softens_the_springs() {
    let t = balance();
    let c = tuning().carving;
    let kick = |n: Vec3, s: f32| {
        let mut steering = SteeringState::new(&c);
        let mut a = board(0.0, 0.0);
        let mut l = LandingState::default();
        let k = ImpactKick { pre_orientation: Quat::IDENTITY, pre_velocity: Vec3::Z * 0.3, velocity: n * s };
        l.touchdown(&t, &c, &mut a, &mut steering, Vec3::Z * 0.3, Vec3::Y, Quat::IDENTITY, false, true, &k, 2.0);
        l
    };
    let l = kick(Vec3::X, 0.2);
    assert!(l.use_impact_tuning && (l.spin_kick + t.spin_impact.eval(0.2)).abs() < 1e-6, "{}", l.spin_kick);
    assert!(l.spin_kick < -0.05 && l.spin_vel == l.spin_kick && l.pitch_kick == 0.0);
    let r = kick(-Vec3::X, 0.2);
    assert!((r.spin_kick + l.spin_kick).abs() < 1e-6);
    let small = kick(Vec3::X, 0.04);
    assert!(small.use_impact_tuning && small.spin_kick == 0.0);
    let none = kick(Vec3::X, 0.0);
    assert!(!none.use_impact_tuning && none.spin_kick == 0.0);
    let mut a = l;
    let mut b = LandingState { use_impact_tuning: false, ..l };
    for _ in 0..5 {
        a.balance_update(&t, 1.0, 0.0, 0.3, false, true, false, false, false, false, false, 2.0);
        b.balance_update(&t, 1.0, 0.0, 0.3, false, true, false, false, false, false, false, 2.0);
    }
    assert!((a.spin - a.spin_target).abs() > (b.spin - b.spin_target).abs());
}

#[test]
fn a_fall_onto_a_slope_snaps_the_board_onto_the_slope_travel() {
    let (t, c) = (balance(), tuning().carving);
    let n = Vec3::new(0.0, 0.92, 0.39).normalize();
    let v = Vec3::new(0.1, -0.8, 0.0);
    let mut steering = SteeringState::new(&c);
    let mut a = board(FRAC_PI_2, 0.0);
    assert!((a * Vec3::X).distance(Vec3::X) < 1e-5);
    let mut l = LandingState::default();
    l.touchdown(&t, &c, &mut a, &mut steering, v, n, Quat::IDENTITY, false, true, &ImpactKick::default(), 2.0);
    let slip_after = |a: Quat| {
        let g = crate::steering::ground_frame(a, n);
        let vl = g.conjugate() * v;
        vl.z.atan2(vl.x).abs()
    };
    assert!(l.spin.abs() > 60.0 * DEG, "spin {}°", l.spin / DEG);
    assert!(slip_after(a) < 10.0 * DEG, "slip after the snap {}°", slip_after(a) / DEG);
    assert!(slip_after(board(FRAC_PI_2, 0.0)) > 60.0 * DEG);
}
