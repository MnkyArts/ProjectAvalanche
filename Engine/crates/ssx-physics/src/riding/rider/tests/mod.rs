use super::*;
use crate::collision::CollisionWorld;
use crate::testutil::{add_floor, add_quad, plane, world};
use crate::{DT_TICKS, KMH_PER_M_PER_TICK};

mod air;
mod boost_press;
mod prewind;
mod reverse_camera;

fn ride(w: &World, s: &mut RiderState, input: &Input, steps: usize, mut each: impl FnMut(&RiderState)) {
    for _ in 0..steps {
        step(s, w, input, DT_TICKS);
        each(s);
    }
}

fn kmh(v: f32) -> f32 {
    v / KMH_PER_M_PER_TICK
}

fn moving(w: &World, at: Vec3, dir: Vec3, v: f32) -> RiderState {
    let mut s = RiderState::spawn(w, at, dir).expect("ground");
    let ground = s.traj - s.ground_normal * s.surface.snow_depth;
    s.traj = ground + s.ground_normal * w.tuning.carving.riding_height_neutral.abs();
    s.traj_cached = s.traj;
    s.carving = CarvingState {
        have_made_first_contact: true,
        use_leg_spring: true,
        uls_ticks: 120.0,
        riding_height: w.tuning.carving.riding_height_neutral,
        initialized: true,
        ..CarvingState::default()
    };
    (s.landing.stable, s.landing.stable_ticks, s.nested_stable) = (true, 60.0, true);
    (s.velocity, s.traj_velocity) = (s.heading * v, s.heading * v);
    s.level_start = s.traj + Vec3::Y * 100.0;
    spq(&mut s, w, DT_TICKS, None);
    s.position = s.board_point();
    s
}

fn traj_height(w: &World, s: &RiderState) -> f32 {
    w.collision.ray_cast(s.traj + Vec3::Y * 0.5, -Vec3::Y, 50.0).map_or(f32::MAX, |h| h.t - 0.5)
}

#[test]
fn rests_on_flat_ground_at_the_riding_height() {
    let w = world(plane(100.0, 0.0));
    let mut s = RiderState::spawn(&w, Vec3::new(0.0, 1.0, 0.0), Vec3::Z).expect("ground");
    assert!((s.traj.y - 0.15).abs() < 1e-4, "{:?}", s.traj);
    ride(&w, &mut s, &Input::default(), 300, |s| {
        assert!(s.on_ground && s.traj.y > 0.37, "the floor keeps P ≥ 0.38 + c: {:?}", s.traj);
        assert!(!s.janked.janked && !s.impact.colliding, "resting on open ground is not janked");
    });
    assert!((0.78..0.84).contains(&s.traj.y), "trajectory at {}", s.traj.y);
    assert!(s.position.y.abs() < 0.06, "board point at {:?}", s.position);
    assert!(s.speed() < 1e-6 && s.stopped, "at rest on flat ground the stop latch holds");
}

#[test]
fn decelerates_by_friction_then_stops_on_flat_ground() {
    let w = world(plane(500.0, 0.0));
    let mut s = moving(&w, Vec3::new(0.0, 1.0, -400.0), Vec3::Z, 10.0 / 60.0);
    let v0 = s.speed();
    let mut last = v0;
    let mut speeds = Vec::new();
    ride(&w, &mut s, &Input::default(), 900, |s| {
        assert!(s.speed() <= last + 1e-6 && s.on_ground, "{} after {}", s.speed(), last);
        last = s.speed();
        speeds.push(s.speed());
    });
    let dv = v0 - speeds[29];
    assert!((0.034..0.044).contains(&dv), "Δv in 1 s = {dv}");
    let stop = speeds.iter().position(|&v| v == 0.0).expect("stops");
    assert!(speeds[stop - 1] <= kmh(5.0) + 1e-6, "{} km/h before the stop", speeds[stop - 1] * KMH_PER_M_PER_TICK);
    assert!(speeds[stop - 2] > kmh(5.0), "the first step under the threshold requests the stop");
    assert!((stop as f32 / 30.0) < 12.0, "stopped after {} s", stop as f32 / 30.0);
    assert!(speeds[stop..].iter().all(|&v| v == 0.0));
}

#[test]
fn accelerates_towards_terminal_velocity_on_a_slope() {
    let theta = 20f32.to_radians();
    let w = world(plane(2500.0, theta));
    let mut s = moving(&w, Vec3::new(0.0, 1500.0 * theta.tan() + 1.0, -1500.0), Vec3::Z, 0.1);
    let (mut min_h, mut max_h) = (f32::MAX, f32::MIN);
    ride(&w, &mut s, &Input::default(), 30 * 60, |s| {
        let h = traj_height(&w, s) * theta.cos();
        (min_h, max_h) = (min_h.min(h), max_h.max(h));
        assert!(s.on_ground && !s.stopped && !s.janked.janked);
    });
    assert!(min_h > 0.6 && max_h < 0.9, "trajectory {min_h} … {max_h} m above the slope");
    let r = &w.props;
    let (sn, cs) = theta.sin_cos();
    let drive = r.gravity_ground.abs() * sn - 0.03 * 0.015;
    let v_term = (drive / (r.k_h_ground * cs.powi(3) + r.k_v_ground * sn.powi(3))).sqrt();
    let v = s.speed();
    assert!((v / v_term - 1.0).abs() < 0.03, "{} vs {} km/h", v * KMH_PER_M_PER_TICK, v_term * KMH_PER_M_PER_TICK);
    assert!(s.velocity.normalize().x.abs() < 1e-3);
    assert!(s.heading.x.abs() < 1e-3 && s.lean.abs() < 1e-3);
}

#[test]
fn brake_and_steer() {
    let theta = 15f32.to_radians();
    let w = world(plane(3000.0, theta));
    let at = Vec3::new(0.0, 2000.0 * theta.tan() + 1.0, -2000.0);
    let (mut free, mut braking) = (moving(&w, at, Vec3::Z, 0.1), moving(&w, at, Vec3::Z, 0.1));
    ride(&w, &mut free, &Input::default(), 300, |_| {});
    ride(&w, &mut braking, &Input { brake: 1.0, ..Input::default() }, 300, |_| {});
    assert!(braking.speed() < 0.5 * free.speed(), "{} vs {}", braking.speed(), free.speed());
    let mut turning = moving(&w, at, Vec3::Z, 0.25);
    ride(&w, &mut turning, &Input { steer: 1.0, ..Input::default() }, 60, |s| assert!(s.on_ground));
    assert!(turning.traj.x < -1.0, "{:?}", turning.traj);
}

fn ledge(drop: f32) -> World {
    let mut c = CollisionWorld::default();
    add_floor(&mut c, [-50.0, 50.0], [-200.0, 0.0], 0.0, 0);
    add_floor(&mut c, [-50.0, 50.0], [0.0, 300.0], -drop, 0);
    c.finish();
    world(c)
}

#[test]
fn ground_air_hysteresis_on_a_ledge() {
    use crate::probe::{CLOSE_TO_GROUND, VERY_CLOSE_TO_GROUND};
    for (drop, expect_air) in [(3.0f32, true), (0.4, false)] {
        let w = ledge(drop);
        let mut s = moving(&w, Vec3::new(0.0, 0.5, -20.0), Vec3::Z, 0.25);
        let (mut prev, mut takeoff_z, mut landing_h) = (s.state, None, None);
        ride(&w, &mut s, &Input::default(), 150, |s| {
            match (prev, s.state) {
                (RideState::Riding, RideState::Airborne) => takeoff_z = Some(s.traj.z),
                (RideState::Airborne, RideState::Riding) => landing_h = Some(s.traj.y + drop),
                _ => {}
            }
            prev = s.state;
        });
        assert_eq!(takeoff_z.is_some(), expect_air, "drop {drop}");
        assert_eq!(s.state, RideState::Riding);
        assert!(s.close.d <= CLOSE_TO_GROUND);
        if expect_air {
            assert!(takeoff_z.is_some_and(|z| (0.0..1.5).contains(&z)), "take-off at z = {takeoff_z:?}");
            assert!(
                landing_h.is_some_and(|h| h <= VERY_CLOSE_TO_GROUND + 0.15 && h > 0.38),
                "landing at {landing_h:?} m"
            );
        }
    }
}

fn steep_slope(class: u8) -> World {
    let mut c = CollisionWorld::default();
    let p = |x: f32, z: f32| Vec3::new(x, z * 60f32.to_radians().tan(), z);
    add_quad(&mut c, [p(-10.0, 0.0), p(-10.0, 10.0), p(10.0, 10.0), p(10.0, 0.0)], class);
    add_floor(&mut c, [-50.0, 50.0], [-50.0, 50.0], -20.0, 0);
    add_floor(&mut c, [20.0, 40.0], [-10.0, 10.0], 0.0, class);
    c.finish();
    world(c)
}

#[test]
fn ridable_filter_on_steep_surfaces() {
    let n = Vec3::new(0.0, 0.5, -60f32.to_radians().sin());
    for (class, ridable) in [(0u8, true), (2, true), (1, false), (11, false)] {
        let w = steep_slope(class);
        let on_slope = Vec3::new(0.0, 5.0 * 60f32.to_radians().tan(), 5.0);
        let probe = ProbeInputs {
            traj: on_slope + n * 0.83,
            joint: on_slope,
            prev_normal: n,
            velocity: Vec3::ZERO,
            in_air: false,
            short: false,
            grinding: false,
            uber_in_air: false,
            wind: Vec3::ZERO,
        };
        let g = w.ground_probe(&probe, &BallisticRay::default());
        if ridable {
            let g = g.expect("steep ridable ground");
            assert!(g.class == class && g.normal.distance(n) < 1e-4 && (g.distance - 0.83).abs() < 1e-3, "{g:?}");
        } else {
            assert!(g.is_none(), "class {class}: the ridable pass looks through the slope: {g:?}");
        }
        let mut s = RiderState::spawn(&w, on_slope + Vec3::Y * 0.1, Vec3::Z).expect("slope");
        step(&mut s, &w, &Input::default(), DT_TICKS);
        assert_eq!(s.state == RideState::Riding, ridable, "class {class}");
        let mut s = RiderState::spawn(&w, Vec3::new(30.0, 1.0, 0.0), Vec3::X).expect("top");
        step(&mut s, &w, &Input::default(), DT_TICKS);
        assert_eq!(s.state, RideState::Riding, "flat class {class}");
        assert!(s.contact.is_some_and(|g| g.class == class && !g.ridable_pass));
    }
}

fn launch_at(v: Vec3, in_air: bool, compression: f32) -> LaunchInputs {
    LaunchInputs {
        grinding: false,
        stick_angle: 0.0,
        stick_mag: 0.0,
        velocity: v,
        in_air,
        ground_normal: Vec3::Y,
        traj_up: Vec3::Y,
        traj_pos: Vec3::new(0.0, 500.0, 0.0),
        compression,
        dt: DT_TICKS,
        wind: Vec3::ZERO,
    }
}

#[test]
fn jump_launch_speed_and_direction_from_the_curves() {
    let w = world(plane(10.0, 0.0));
    let q = &w.tuning.queries;
    let v = Vec3::Z * (100.0 / KMH_PER_M_PER_TICK);
    let d = jump_launch(&w, &launch_at(v, false, 0.0)).expect("moving");
    let s = q.jump_launch_speed_vs_speed.eval(v.length());
    assert!((s - 0.2).abs() < 1e-6);
    let launch_dir = Vec3::new(0.0, 0.7, 0.3).normalize();
    let target = Quat::from_axis_angle(-Vec3::X, 10f32.to_radians()) * (v + launch_dir * s);
    assert!((v + d).distance(target) < 1e-5, "{:?} vs {target:?}", v + d);
    let d1 = jump_launch(&w, &launch_at(v, false, 1.0)).expect("moving");
    let t1 = Quat::from_axis_angle(-Vec3::X, 10f32.to_radians()) * (v + launch_dir * s * 1.25);
    assert!((v + d1).distance(t1) < 1e-5);
    let da = jump_launch(&w, &launch_at(v, true, 0.0)).expect("moving");
    assert!((v + da).distance(v + launch_dir * s) < 1e-5);
    let du = jump_launch(&w, &launch_at(Vec3::Y * 0.1, false, 0.0)).expect("moving");
    assert!(du.distance(Vec3::Y * q.jump_launch_speed_vs_speed.eval(0.1)) < 1e-6);
    assert!(jump_launch(&w, &launch_at(Vec3::ZERO, false, 0.0)).is_none());
    assert!(jump_launch(&w, &LaunchInputs { dt: 0.0, ..launch_at(v, false, 0.0) }).is_none());
}

#[test]
fn jump_launch_fan_steers_around_an_obstacle() {
    let mut c = CollisionWorld::default();
    let wall = |x0: f32, x1: f32| {
        [Vec3::new(x0, 0.0, 6.0), Vec3::new(x0, 8.0, 6.0), Vec3::new(x1, 8.0, 6.0), Vec3::new(x1, 0.0, 6.0)]
    };
    add_quad(&mut c, wall(-0.6, 0.6), 0);
    c.finish();
    let w = world(c);
    let v = Vec3::Z * 0.3;
    let base = LaunchInputs { traj_pos: Vec3::new(0.0, 0.8, 0.0), ..launch_at(v, true, 0.0) };
    let free = jump_launch(&world(CollisionWorld::default()), &base).expect("moving");
    let fan = jump_launch(&w, &base).expect("moving");
    assert!((v + fan).length() - (v + free).length() < 1e-4, "the fan keeps the speed");
    assert!(fan.distance(free) > 1e-3, "blocked target replaced: {fan:?} vs {free:?}");
}

#[test]
fn jump_fires_on_release_after_ten_ground_ticks() {
    let w = world(plane(2000.0, 0.0));
    let mut s = moving(&w, Vec3::new(0.0, 1.0, -1500.0), Vec3::Z, 60.0 / KMH_PER_M_PER_TICK);
    s.jump.on_ground_ticks = 0.0;
    for i in 0..4 {
        step(&mut s, &w, &Input { jump: i < 3, ..Input::default() }, DT_TICKS);
        assert_eq!(s.state, RideState::Riding, "step {i}");
    }
    let expected = s.jump.launch;
    let compression = s.jump.compression;
    assert!((compression - (1.0 - (-0.66f32).exp()) * (-0.22f32).exp()).abs() < 1e-5, "{compression}");
    let li = LaunchInputs {
        velocity: s.traj_velocity,
        in_air: false,
        ground_normal: s.ground_normal,
        traj_up: s.traj_orientation * Vec3::Y,
        traj_pos: s.traj,
        compression,
        dt: DT_TICKS,
        grinding: false,
        stick_angle: 0.0,
        stick_mag: 0.0,
        wind: Vec3::ZERO,
    };
    let launch = jump_launch(&w, &li).expect("moving");
    assert!(expected.distance(launch) < 1e-6);
    let (v0, y0) = (s.traj_velocity, s.traj.y);
    step(&mut s, &w, &Input::default(), DT_TICKS);
    assert_eq!(s.state, RideState::Jump);
    assert!(s.last_launch.distance(launch) < 1e-6);
    let u_y = (v0 + launch).y;
    let peak = u_y * u_y / (2.0 * w.props.gravity_air.abs());
    let (mut top, mut landed_at) = (0.0f32, None);
    for i in 0..90 {
        step(&mut s, &w, &Input::default(), DT_TICKS);
        top = top.max(s.traj.y - y0);
        if s.state == RideState::Riding && landed_at.is_none() {
            landed_at = Some(i);
        }
    }
    assert!((0.9 * peak..=1.0 * peak + 0.05).contains(&top), "peak {top} m vs {peak} m");
    assert!(peak > 2.0, "60 km/h jump: {peak} m");
    let air = landed_at.expect("lands") as f32 * DT_TICKS;
    assert!((air - 2.0 * u_y / w.props.gravity_air.abs()).abs() < 12.0, "{air} ticks in the air");
    let mut s = moving(&w, Vec3::new(0.0, 1.0, -1000.0), Vec3::Z, 0.3);
    ride(&w, &mut s, &Input { jump: true, ..Input::default() }, 20, |s| assert_eq!(s.state, RideState::Riding));
}

#[test]
fn air_jump_within_eight_ticks_launches_from_the_air() {
    let w = ledge(10.0);
    for (late, jumps) in [(3usize, true), (5, false)] {
        let mut s = moving(&w, Vec3::new(0.0, 0.5, -10.0), Vec3::Z, 0.3);
        let mut since_air = None;
        let mut launched = false;
        for _ in 0..60 {
            let release = since_air.is_some_and(|n| n >= late);
            step(&mut s, &w, &Input { jump: !release, ..Input::default() }, DT_TICKS);
            assert_ne!(s.state, RideState::Jump);
            launched |= s.last_launch != Vec3::ZERO;
            if s.state != RideState::Riding {
                since_air = Some(since_air.map_or(1, |n| n + 1));
            }
        }
        assert_eq!(launched, jumps, "release {late} steps after take-off");
    }
}

#[test]
fn landing_from_a_jump_waits_for_the_early_window() {
    let w = world(plane(500.0, 0.0));
    let mut s = moving(&w, Vec3::ZERO, Vec3::Z, 0.03);
    s.jump.on_ground_ticks = 20.0;
    s.jump.allow = true;
    step(&mut s, &w, &Input { jump: true, ..Input::default() }, DT_TICKS);
    step(&mut s, &w, &Input::default(), DT_TICKS);
    assert_eq!(s.state, RideState::Jump);
    let mut steps = 0;
    while s.state == RideState::Jump && steps < 30 {
        step(&mut s, &w, &Input::default(), DT_TICKS);
        steps += 1;
    }
    assert!(s.state == RideState::Riding && (9..=11).contains(&steps), "landed after {steps} steps");
}

#[test]
fn steering_turns_towards_the_stick() {
    let theta = 10f32.to_radians();
    let w = world(plane(3000.0, theta));
    let down = Vec3::new(0.0, -theta.sin(), theta.cos());
    for steer in [1.0f32, -1.0, 0.0] {
        let mut s =
            moving(&w, Vec3::new(0.0, 2000.0 * theta.tan() + 1.0, -2000.0), Vec3::Z, 100.0 / KMH_PER_M_PER_TICK);
        let (mut max_slip, mut max_lean) = (0.0f32, 0.0f32);
        ride(&w, &mut s, &Input { steer, ..Input::default() }, 30, |s| {
            assert!(s.on_ground);
            max_slip = max_slip.max(s.kin.slip.abs());
            max_lean = max_lean.max(s.lean * steer);
        });
        let turned = s.velocity.normalize().angle_between(down).to_degrees();
        if steer == 0.0 {
            assert!(s.velocity.x.abs() < 1e-4 && s.lean.abs() < 1e-4, "{:?}", s.velocity);
            continue;
        }
        assert!(s.velocity.x * steer < 0.0 && s.heading.x * steer < 0.0, "steer {steer}: {:?}", s.velocity);
        assert!(turned > 20.0, "steer {steer}: turned {turned}°");
        assert!(max_lean > 30f32.to_radians(), "lean {}°", max_lean.to_degrees());
        assert!(max_slip < 45f32.to_radians(), "slip {}°", max_slip.to_degrees());
    }
}

#[test]
fn stop_latch_at_low_speed_and_getting_going() {
    let w = world(plane(500.0, 0.0));
    let mut s = moving(&w, Vec3::new(0.0, 1.0, 0.0), Vec3::Z, 3.0 / KMH_PER_M_PER_TICK);
    ride(&w, &mut s, &Input::default(), 3, |_| {});
    assert!(s.stopped && s.speed() == 0.0);
    let at = s.traj;
    ride(&w, &mut s, &Input::default(), 30, |s| assert!(s.stopped && s.speed() == 0.0));
    assert!((s.traj - at).with_y(0.0).length() < 1e-4, "{at:?} → {:?}", s.traj);
    ride(&w, &mut s, &Input { brake: 1.0, ..Input::default() }, 10, |s| assert!(s.stopped));
    ride(&w, &mut s, &Input::default(), 1, |_| {});
    ride(&w, &mut s, &Input { tuck: 1.0, ..Input::default() }, 30, |_| {});
    assert!(!s.stopped && s.speed() > 5.0 / KMH_PER_M_PER_TICK, "{} km/h", s.speed() * KMH_PER_M_PER_TICK);
    for (deg, rides_off) in [(8.0f32, false), (25.0, true)] {
        let theta = deg.to_radians();
        let w = world(plane(500.0, theta));
        let mut s = moving(&w, Vec3::new(0.0, 1.0, 0.0), Vec3::Z, 0.0);
        ride(&w, &mut s, &Input::default(), 60, |_| {});
        assert_eq!(!s.stopped && s.speed() > 1.0 / KMH_PER_M_PER_TICK, rides_off, "{deg}°: {} m/tick", s.speed());
    }
}

#[test]
fn the_start_waits_for_the_get_going_gesture() {
    let theta = 25f32.to_radians();
    let w = world(plane(500.0, theta));
    let mut s = RiderState::spawn(&w, Vec3::new(0.0, 1.0, 0.0), Vec3::Z).expect("ground");
    ride(&w, &mut s, &Input::default(), 60, |_| {});
    assert!(s.stopped && s.speed() == 0.0);
    assert!(traj_height(&w, &s) * theta.cos() > 0.7, "lifted to the riding height while stopped");
    ride(&w, &mut s, &Input { tuck: 1.0, ..Input::default() }, 1, |_| {});
    ride(&w, &mut s, &Input::default(), 60, |_| {});
    assert!(!s.stopped && s.speed() > 10.0 / KMH_PER_M_PER_TICK, "{} km/h", s.speed() * KMH_PER_M_PER_TICK);
}

#[test]
fn pid_drives_the_rate_to_the_set_point() {
    use crate::character::PidGains;
    use crate::steering::Pid;
    let mut p = Pid::new(PidGains { p: 1.5, i: 0.0, d: 1.0 }, 0.32);
    for _ in 0..60 {
        p.update(2.0, 0.5);
    }
    assert!((p.x - 0.5).abs() < 0.02, "{}", p.x);
    for _ in 0..60 {
        p.update(2.0, 5.0);
    }
    assert!((p.x - 1.0).abs() < 1e-6);
}

#[test]
fn the_classic_carve_adds_the_dpad_to_the_left_stick() {
    let stick = Input { steer: 1.0, tuck: 0.5, ..Input::default() };
    let dpad = Input { dpad: Vec2::new(1.0, 1.0), ..Input::default() };
    let both = Input { steer: 1.0, tuck: 0.5, dpad: Vec2::new(1.0, 1.0), ..Input::default() };
    assert_eq!(dpad.carve_stick(false), Vec2::ZERO);
    assert_eq!(dpad.carve_stick_y(false), 0.0);
    assert!((stick.carve_stick(false) - Vec2::new(1.0, 0.5).normalize()).length() < 1e-6);
    assert_eq!(stick.carve_stick_y(false), 0.5);
    assert_eq!(stick.carve_stick(true), stick.carve_stick(false));
    assert!((dpad.carve_stick(true) - Vec2::new(1.0, 1.0).normalize()).length() < 1e-6);
    assert_eq!(dpad.carve_stick_y(true), 1.0);
    assert!((both.carve_stick(true) - Vec2::new(1.0, 1.0).normalize()).length() < 1e-6);
    assert_eq!(both.carve_stick_y(true), 1.0, "the disc clamp does not reach the y axis function");
    let cancel = Input { dpad: Vec2::new(-1.0, -0.5), ..stick };
    assert!(cancel.carve_stick(true).length() < 1e-6 && cancel.carve_stick_y(true) == 0.0);
}

#[test]
fn the_classic_air_reads_the_dpad_only() {
    let stick = Input { steer: 0.5, brake: 0.25, ..Input::default() };
    let dpad = Input { dpad: Vec2::new(-1.0, 1.0), ..Input::default() };
    assert_eq!(stick.air_stick(false), Vec2::new(0.5, -0.25));
    assert_eq!(dpad.air_stick(false), Vec2::ZERO);
    assert_eq!(stick.air_stick(true), Vec2::ZERO);
    assert_eq!(dpad.air_stick(true), Vec2::new(-1.0, 1.0));
}

#[test]
fn the_classic_scheme_steers_with_the_left_stick() {
    let theta = 10f32.to_radians();
    let w = world(plane(3000.0, theta));
    let at = Vec3::new(0.0, 2000.0 * theta.tan() + 1.0, -2000.0);
    let turn = |classic: bool, input: Input| {
        let mut s = moving(&w, at, Vec3::Z, 0.25);
        s.tricks.classic = classic;
        let start = s.heading;
        ride(&w, &mut s, &input, 40, |_| {});
        start.angle_between(s.heading)
    };
    let stick = turn(true, Input { steer: 1.0, ..Input::default() });
    let dpad = turn(true, Input { dpad: Vec2::X, ..Input::default() });
    let standard = turn(false, Input { steer: 1.0, ..Input::default() });
    assert!(stick > 0.1, "the classic left stick carves: {stick}");
    assert!(dpad > 0.05, "the classic D-pad carves: {dpad}");
    assert!(standard > 0.1, "the standard left stick carves: {standard}");
    assert!(
        turn(false, Input { dpad: Vec2::X, ..Input::default() }) < 1e-3,
        "the standard D-pad is not a carving input"
    );
}

#[test]
fn l2s_two_matchers_reach_the_step_apart() {
    let theta = 5f32.to_radians();
    let w = world(plane(2500.0, theta));
    let at = Vec3::new(0.0, 2000.0 * theta.tan() + 1.0, -2000.0);
    let mut s = moving(&w, at, Vec3::Z, 0.1);
    s.grind.pad_l2_hold = true;
    step(&mut s, &w, &Input::default(), DT_TICKS);
    assert!(s.grind.l2_hold && !s.grind.button, "L2 ∈ [0.2, 0.5): the hold, not the grind button");
    s.grind.set_l2(true);
    step(&mut s, &w, &Input::default(), DT_TICKS);
    assert!(s.grind.l2_hold && s.grind.button, "a full press: both");
    s.grind.release_l2();
    step(&mut s, &w, &Input::default(), DT_TICKS);
    assert!(!s.grind.l2_hold && !s.grind.button, "released: neither");
}
