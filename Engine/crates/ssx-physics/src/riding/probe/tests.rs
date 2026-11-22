use super::*;
use crate::collision::CollisionWorld;
use crate::testutil::{add_floor, add_quad, plane, world};

fn on(traj: Vec3, joint: Vec3, v: Vec3, in_air: bool) -> ProbeInputs {
    ProbeInputs {
        traj,
        joint,
        prev_normal: Vec3::Y,
        velocity: v,
        in_air,
        short: false,
        grinding: false,
        uber_in_air: false,
        wind: Vec3::ZERO,
        query_enabled: true,
    }
}

#[test]
fn flat_ground_distance_and_close_flags() {
    let w = world(plane(100.0, 0.0));
    let g = w.ground_probe(&on(Vec3::Y * 0.83, Vec3::ZERO, Vec3::Z * 0.3, false), &BallisticRay::default());
    let g = g.expect("ground");
    assert!((g.distance - 0.83).abs() < 1e-5 && g.normal.distance(Vec3::Y) < 1e-6 && g.class == 0);
    assert!(g.contact.distance(Vec3::ZERO) < 1e-5 && g.height.abs() < 1e-5);
    let c = close_to_ground(Some(&g), 0.15);
    assert!((c.d - 0.68).abs() < 1e-5 && c.is_close && c.is_very_close);
    let far = GroundContact { distance: 1.5, ..g };
    let c = close_to_ground(Some(&far), 0.15);
    assert!(c.is_close && !c.is_very_close);
    assert_eq!(close_to_ground(None, 0.15), CloseToGround::default());
}

#[test]
fn the_probe_reach_and_the_ballistic_fallback() {
    let w = world(plane(100.0, 0.0));
    let high = ProbeInputs { short: true, ..on(Vec3::Y * 2.0, Vec3::Y * 1.17, Vec3::Z * 0.05, true) };
    assert!(w.ground_probe(&high, &BallisticRay::default()).is_none());
    let hit = w.collision.ray_cast(Vec3::new(0.0, 2.0, 1.0), -Vec3::Y, 10.0);
    let near = BallisticRay { valid: true, hit, ..BallisticRay::default() };
    let g = w.ground_probe(&high, &near).expect("ballistic fallback");
    assert!(g.point.distance(Vec3::new(0.0, 0.0, 0.8)) < 1e-3, "{:?}", g.point);
    let far_hit = w.collision.ray_cast(Vec3::new(0.0, 2.0, 3.0), -Vec3::Y, 10.0);
    let far = BallisticRay { valid: true, hit: far_hit, ..BallisticRay::default() };
    assert!(w.ground_probe(&high, &far).is_none(), "contact 3.6 m away");
    assert!(w.ground_probe(&ProbeInputs { in_air: false, ..high }, &near).is_none());
}

#[test]
fn the_first_valid_query_wins() {
    let mut c = CollisionWorld::default();
    add_floor(&mut c, [-10.0, 10.0], [-10.0, 3.0], 0.0, 0);
    let p = |x: f32, y: f32, z: f32| Vec3::new(x, y, z);
    add_quad(&mut c, [p(-10.0, 0.0, 3.0), p(-10.0, 0.5, 3.0), p(10.0, 0.5, 3.0), p(10.0, 0.0, 3.0)], 0);
    add_floor(&mut c, [-10.0, 10.0], [3.0, 20.0], 0.5, 0);
    c.finish();
    let w = world(c);
    let g = w.ground_probe(&on(Vec3::Y * 0.83, Vec3::ZERO, Vec3::Z * 0.4, false), &BallisticRay::default());
    assert!(g.expect("floor").point.y.abs() < 1e-5);
}

#[test]
fn the_chord_is_never_a_ground_candidate() {
    let mut c = CollisionWorld::default();
    add_floor(&mut c, [-10.0, 10.0], [-10.0, 10.0], 0.0, 0);
    let p = |x: f32, y: f32| Vec3::new(x, y, 1.0);
    add_quad(&mut c, [p(-10.0, 0.0), p(-10.0, 10.0), p(10.0, 10.0), p(10.0, 0.0)], 0);
    c.finish();
    let w = world(c);
    let i = on(Vec3::Y * 3.5, Vec3::Y * 2.7, Vec3::Z * 0.5, true);
    let chord_end = Vec3::new(0.0, 3.44, 1.5);
    let chord = w.collision.segment_query(&SegmentQuery::sphere(i.traj, chord_end, 0.4, 0));
    assert!(chord.is_some_and(|h| h.normal.z < -0.99), "the chord reaches the wall: {chord:?}");
    let pass = w.probe_pass(&i, 0, Some(&BallisticRay::default()));
    assert!(pass.hit.is_none(), "pass A: {:?}", pass.hit);
    assert!(w.ground_probe(&i, &BallisticRay::default()).is_none());
}

#[test]
fn a_probe_starting_under_a_ceiling_publishes_its_underside() {
    let mut c = CollisionWorld::default();
    add_floor(&mut c, [-10.0, 10.0], [-10.0, 10.0], 0.0, 0);
    let p = |x: f32, y: f32, z: f32| Vec3::new(x, y, z);
    let slab = [p(-10.0, 1.2, -10.0), p(10.0, 1.2, -10.0), p(10.0, 1.2, 10.0), p(-10.0, 1.2, 10.0)];
    let side = [p(-10.0, 0.5, -10.0), p(-10.0, 0.5, 10.0)];
    let v = [slab[0], slab[1], slab[2], slab[3], side[0], side[1]];
    c.push_cluster(&v, None, None, &[[0, 1, 2], [0, 2, 3], [0, 3, 5], [0, 5, 4]], 0, 0);
    c.finish();
    let w = world(c);
    let i = on(Vec3::Y * 0.85, Vec3::Y * 0.05, Vec3::Z * 0.3, false);
    let g = w.ground_probe(&i, &BallisticRay::default()).expect("the underside");
    assert!(g.normal.distance(-Vec3::Y) < 1e-6 && (g.point.y - 1.2).abs() < 1e-5, "{g:?}");
    assert!((g.distance - 0.35).abs() < 1e-5 && g.ridable_pass, "{g:?}");
    assert!(close_to_ground(Some(&g), 0.0).is_very_close);
}

#[test]
fn selection_between_the_passes() {
    let cos45 = std::f32::consts::FRAC_1_SQRT_2;
    let steep = Vec3::new(0.0, 0.5, -0.866);
    assert!(use_unfiltered(None, Some(Vec3::Y), Vec3::Y, cos45));
    assert!(use_unfiltered(Some(Vec3::Y), Some(Vec3::Y), Vec3::Y, cos45));
    assert!(use_unfiltered(Some(Vec3::Y), None, Vec3::Y, cos45));
    assert!(!use_unfiltered(Some(steep), Some(steep), Vec3::Y, cos45));
    assert!(!use_unfiltered(Some(Vec3::Y), None, steep, cos45));
}
