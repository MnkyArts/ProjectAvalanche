use glam::Vec3;

use super::*;

fn surface_curve() -> Curve {
    Curve::new(vec![
        [0.0, 0.0],
        [0.124_355_7, 0.025_104_5],
        [0.392_544_6, 0.359_832_5],
        [0.557_353_3, 1.0],
        [0.706_430_4, 1.0],
        [0.836_03, 1.0],
        [0.925_925_9, 1.0],
    ])
}

fn avalanche_curve() -> Curve {
    Curve::new(vec![
        [0.0, 0.0],
        [0.478_559_8, 0.037_656_8],
        [1.914_239_2, 0.196_652_7],
        [2.936_488_2, 0.514_644_3],
        [3.5, 1.0],
    ])
}

fn landing_tuning() -> LandingRumbleTuning {
    LandingRumbleTuning {
        min_speed: 0.2,
        max_speed: 1.0,
        min_weight: 0.4,
        max_weight: 0.7,
        min_time: 15.0,
        max_time: 30.0,
        time_curve: Curve::new(vec![[0.0, 0.0], [0.1, 1.0], [0.8, 0.5], [1.0, 0.0]]),
    }
}

fn curves() -> ([Curve; 2], [Curve; 2]) {
    ([surface_curve(), surface_curve()], [avalanche_curve(), avalanche_curve()])
}

#[test]
fn the_clamps_are_the_fsel_pairs() {
    assert_eq!(clamp_unit(-0.5), 0.0);
    assert_eq!(clamp_unit(-0.0).to_bits(), 0.0f32.to_bits());
    assert_eq!(clamp_unit(0.25), 0.25);
    assert_eq!(clamp_unit(1.0), 1.0);
    assert_eq!(clamp_unit(1.4), 1.0);
    assert_eq!(clamp_unit(f32::NAN), 1.0);
    assert_eq!(min_one(0.5), 0.5);
    assert_eq!(min_one(3.0), 1.0);
    assert!(min_one(f32::NAN).is_nan());
}

#[test]
fn rock_raises_both_motors_and_a_grind_gates_them() {
    let (surface, avalanche) = curves();
    let rock = CarvingRumbleInputs { surface: 1.4, speed: 0.392_544_6, ..CarvingRumbleInputs::default() };
    let mut r = PadRumble::default();
    r.carving(&rock, &surface, &avalanche);
    let v = 1.4f32 * 0.359_832_5;
    assert_eq!(r.motors, [v, v]);
    let mut r = PadRumble::default();
    r.carving(&CarvingRumbleInputs { speed: 0.6, ..rock }, &surface, &avalanche);
    assert_eq!(r.motors, [1.0, 1.0]);
    let mut r = PadRumble::default();
    r.carving(&CarvingRumbleInputs { grind_state: 2, ..rock }, &surface, &avalanche);
    assert_eq!(r.motors, [(v).mul_add(0.2, 0.0), 0.0]);
    let mut r = PadRumble::default();
    r.carving(&CarvingRumbleInputs { surface: 0.0, ..rock }, &surface, &avalanche);
    assert_eq!(r.motors, [0.0, 0.0]);
}

#[test]
fn an_uber_slide_rumbles_until_the_rider_lies_still() {
    let (surface, avalanche) = curves();
    let slide = CarvingRumbleInputs { uber_sliding: true, speed: 0.392_544_6, ..CarvingRumbleInputs::default() };
    let mut r = PadRumble::default();
    r.carving(&slide, &surface, &avalanche);
    assert_eq!(r.motors, [0.359_832_5, 0.359_832_5]);
    let mut r = PadRumble::default();
    r.carving(&CarvingRumbleInputs { dead_on_ground: true, ..slide }, &surface, &avalanche);
    assert_eq!(r.motors, [0.0, 0.0]);
}

#[test]
fn the_avalanche_adds_its_flow_times_mass() {
    let (surface, avalanche) = curves();
    let i = CarvingRumbleInputs { avalanche_vel: Vec3::new(0.0, 0.0, 2.0), avalanche_mass: 1.0, ..Default::default() };
    let mut r = PadRumble::default();
    r.carving(&i, &surface, &avalanche);
    let want = avalanche_curve().eval(vmx::mul(crate::intercept::sqrt(4.0), 1.0));
    assert!((want - 0.223_330_5).abs() < 1e-6, "{want}");
    assert_eq!(r.motors, [want, want]);
}

#[test]
fn the_carving_only_raises() {
    let (surface, avalanche) = curves();
    let mut r = PadRumble { motors: [0.9, 0.05], ..PadRumble::default() };
    r.carving(&CarvingRumbleInputs { surface: 1.4, speed: 0.392_544_6, ..Default::default() }, &surface, &avalanche);
    assert_eq!(r.motors, [0.9, 1.4f32 * 0.359_832_5]);
}

#[test]
fn a_landing_plays_the_time_curve_on_motor_0() {
    let t = landing_tuning();
    let mut r = PadRumble::default();
    let fall = Vec3::new(0.0, -0.6, 0.0);
    for _ in 0..2 {
        r.landing(&t, true, false, Vec3::Y, 2.0);
        r.cache_velocity(fall);
        assert_eq!(r.motors, [0.0, 0.0]);
    }
    r.landing(&t, false, true, Vec3::Y, 2.0);
    assert_eq!(r.landing_impact_magnitude, 0.6);
    let x = (0.6f32 - 0.2) / 0.8;
    assert_eq!(r.landing_weight, x.mul_add(0.3, 0.4));
    assert_eq!(r.landing_total_time, 15.0f32.mul_add(x, 15.0));
    assert_eq!((r.landing_timer, r.motors), (2.0, [0.0, 0.0]));
    r.cache_velocity(Vec3::ZERO);
    let (w, total) = (r.landing_weight, r.landing_total_time);
    let mut out = Vec::new();
    for _ in 0..12 {
        r.reset_motors();
        r.landing(&t, false, true, Vec3::Y, 2.0);
        r.cache_velocity(Vec3::ZERO);
        out.push(r.motors[0]);
    }
    let want = |timer: f32| vmx::mul(t.time_curve.eval(timer / total), w) * 0.6;
    assert_eq!(out[0], want(2.0));
    assert_eq!(out[1], want(4.0));
    assert!(out[0] > 0.29 && out[0] < 0.3, "{out:?}");
    assert_eq!(out[10], want(22.0));
    assert!(out[10] > 0.0);
    assert_eq!(r.landing_total_time, -1.0);
    assert_eq!(out[11], 0.0);
}

#[test]
fn a_landing_rumble_gives_way_to_a_stronger_motor_1() {
    let t = landing_tuning();
    let mut r = PadRumble { landing_weight: 0.55, landing_timer: 2.0, landing_total_time: 22.5, ..Default::default() };
    r.motors = [0.0, 0.9];
    r.landing(&t, false, true, Vec3::Y, 2.0);
    assert_eq!(r.motors, [0.0, 0.9]);
}

#[test]
fn a_soft_landing_only_sets_the_magnitude() {
    let t = landing_tuning();
    let mut r =
        PadRumble { prev_in_air: true, velocities: [Vec3::ZERO, Vec3::new(0.0, -0.15, 0.0)], ..Default::default() };
    r.landing(&t, false, true, Vec3::Y, 2.0);
    assert_eq!(r.landing_impact_magnitude, 0.15);
    assert_eq!((r.landing_total_time, r.motors), (0.0, [0.0, 0.0]));
}
