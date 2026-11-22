use glam::Vec3;

use super::*;
use crate::collision::CollisionWorld;
use crate::rider::drag_accel;
use crate::testutil::{add_floor, add_quad, world};

fn floor_world() -> World {
    let mut c = CollisionWorld::default();
    add_floor(&mut c, [-400.0, 400.0], [-400.0, 400.0], 0.0, 0);
    c.set_cluster_entry_box(Vec3::new(-400.0, 0.0, -400.0), Vec3::new(400.0, 1.0, 400.0));
    c.finish();
    world(c)
}

fn reference_contact(w: &World, p0: Vec3, v0: Vec3, dt: f32) -> (f32, Vec3) {
    let r = &w.props;
    let (mut p, mut v, mut t) = (p0, v0, 0.0);
    loop {
        let mut a = drag_accel(r.k_h_air, r.k_v_air, v, Vec3::ZERO, dt);
        a.y -= r.g_air.y.abs();
        let v1 = v + a * dt;
        let p1 = p + v1 * dt;
        if p1.y <= BALLISTIC_PROBE_RADIUS {
            return (t + dt * (p.y - BALLISTIC_PROBE_RADIUS) / (p.y - p1.y), v);
        }
        (p, v, t) = (p1, v1, t + dt);
    }
}

#[test]
fn contact_time_is_the_stepped_path_meeting_the_ground() {
    let w = floor_world();
    let (p0, v0) = (Vec3::new(0.0, 12.0, 0.0), Vec3::new(0.3, 0.1, 0.0));
    let b = w.ballistic_probe(p0, v0, Vec3::ZERO, true, 2.0);
    let (t, v_seg) = reference_contact(&w, p0, v0, 2.0);
    assert!(b.valid);
    assert!((b.ticks - t).abs() < 1e-3, "{} vs {t}", b.ticks);
    assert!((b.normal - Vec3::Y).length() < 1e-5);
    assert!((b.end_velocity - v_seg).length() < 1e-6);
    assert!(b.end_position.y.abs() < 1e-4, "contact on the floor: {:?}", b.end_position);
    assert!(b.moving_towards_impact == (v0.y < 0.0));
    assert!((b.normal_proj_distance - 12.0).abs() < 1e-3, "{b:?}");
    assert!(b.landing().is_some_and(|l| l.ticks == b.ticks));
}

#[test]
fn a_flat_floor_is_found_in_the_step_whose_path_reaches_it() {
    let mut c = CollisionWorld::default();
    add_floor(&mut c, [-400.0, 400.0], [-400.0, 400.0], 0.0, 0);
    c.finish();
    let w = world(c);
    let (p0, v0) = (Vec3::new(0.0, 12.0, 0.0), Vec3::new(0.3, 0.1, 0.0));
    let flat = w.ballistic_probe(p0, v0, Vec3::ZERO, true, 2.0);
    let r = &w.props;
    let (mut p, mut v, mut t) = (p0, v0, 0.0f32);
    let (touch, reach) = loop {
        let mut a = drag_accel(r.k_h_air, r.k_v_air, v, Vec3::ZERO, 2.0);
        a.y -= r.g_air.y.abs();
        let v1 = v + a * 2.0;
        let p1 = p + v1 * 2.0;
        if p1.y <= 0.0 {
            break (p.y < BALLISTIC_PROBE_RADIUS, t);
        }
        (p, v, t) = (p1, v1, t + 2.0);
    };
    assert!(touch, "the sphere touches the floor before its path reaches it");
    assert!(flat.valid && flat.ticks == reach, "{} vs {reach}", flat.ticks);
    assert!(
        flat.hit.is_some_and(|h| h.t == 0.0) && flat.normal.y > 0.999 && flat.end_position.y.abs() < 1e-4,
        "{flat:?}"
    );
}

#[test]
fn nothing_in_range_runs_128_steps() {
    let w = world({
        let mut c = CollisionWorld::default();
        add_floor(&mut c, [-1.0, 1.0], [-1.0, 1.0], -5000.0, 0);
        c.finish();
        c
    });
    let b = w.ballistic_probe(Vec3::ZERO, Vec3::new(0.05, 0.0, 0.0), Vec3::ZERO, true, 2.0);
    assert!(!b.valid);
    assert_eq!(b.steps, BALLISTIC_PROBE_MAX_STEPS);
    assert_eq!(b.ticks, 256.0);
    assert_eq!(b.normal, Vec3::Y);
    assert!(b.landing().is_none());
}

#[test]
fn the_path_ends_after_crossing_200_m() {
    let w = world(CollisionWorld::default());
    let v0 = Vec3::new(0.0, -6.0, 0.0);
    let b = w.ballistic_probe(Vec3::ZERO, v0, Vec3::ZERO, true, 2.0);
    assert!(!b.valid && b.steps < BALLISTIC_PROBE_MAX_STEPS);
    assert!(b.end_position.length() >= BALLISTIC_PROBE_MAX_DISTANCE);
    assert!(b.end_position.length() - b.end_velocity.length() * 2.0 < BALLISTIC_PROBE_MAX_DISTANCE);
}

#[test]
fn ground_mode_uses_ground_gravity_and_drag() {
    let w = floor_world();
    let (p0, v0) = (Vec3::new(0.0, 3.0, 0.0), Vec3::new(0.3, 0.0, 0.0));
    let air = w.ballistic_probe(p0, v0, Vec3::ZERO, true, 2.0);
    let ground = w.ballistic_probe(p0, v0, Vec3::ZERO, false, 2.0);
    assert!(w.props.g_ground.y.abs() < w.props.g_air.y.abs());
    assert!(ground.valid && air.valid && ground.ticks > air.ticks);
}

#[test]
fn zero_dt_publishes_nothing() {
    let w = floor_world();
    let b = w.ballistic_probe(Vec3::new(0.0, 1.0, 0.0), Vec3::new(0.1, -0.1, 0.0), Vec3::ZERO, true, 0.0);
    assert!(!b.valid && b.ticks == 0.0 && b.steps == 0 && b.normal == Vec3::Y);
}

#[test]
fn a_wall_ahead_gives_its_normal() {
    let mut c = CollisionWorld::default();
    let p = |y: f32, z: f32| Vec3::new(5.0, y, z);
    add_quad(&mut c, [p(-50.0, -50.0), p(-50.0, 50.0), p(50.0, 50.0), p(50.0, -50.0)], 1);
    c.finish();
    let w = world(c);
    let b = w.ballistic_probe(Vec3::new(0.0, 10.0, 0.0), Vec3::new(0.4, 0.0, 0.0), Vec3::ZERO, true, 2.0);
    assert!(b.valid);
    assert!((b.normal - Vec3::NEG_X).length() < 1e-4, "{:?}", b.normal);
    assert!((b.end_position.x - 5.0).abs() < 1e-4);
    assert!(b.ticks > 0.0 && b.ticks < 14.0);
}
