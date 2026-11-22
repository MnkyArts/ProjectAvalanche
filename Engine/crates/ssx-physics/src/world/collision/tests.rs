use super::*;
use crate::testutil::plane;

#[test]
fn the_chunk_query_hands_out_a_quads_second_triangle_first() {
    let mut w = CollisionWorld::default();
    let v = [Vec3::ZERO, Vec3::X, Vec3::Z, Vec3::ONE];
    w.push_cluster(&v, None, None, &[[1, 2, 0], [3, 0, 2], [0, 1, 2], [1, 2, 0], [3, 0, 2]], 0, 0);
    w.set_quad_units(0b01001);
    assert_eq!(w.query_order(0).collect::<Vec<_>>(), [1, 0, 2, 4, 3]);
    w.push_cluster(&v, None, None, &[[0, 1, 2], [1, 2, 3]], 0, 0);
    w.push_cluster(&v, None, None, &[], 0, 0);
    assert_eq!(w.query_order(1).collect::<Vec<_>>(), [0, 1]);
    assert_eq!(w.quad_units, [0b01001, 0]);
}

#[test]
fn ray_hits_plane_with_barycentrics() {
    let w = plane(10.0, 0.0);
    let h = w.ray_cast(Vec3::new(1.0, 5.0, 2.0), -Vec3::Y, 10.0).expect("hit");
    assert!((h.t - 5.0).abs() < 1e-5 && h.point.distance(Vec3::new(1.0, 0.0, 2.0)) < 1e-5);
    assert!(h.normal.distance(Vec3::Y) < 1e-6);
    assert!((h.bary.iter().sum::<f32>() - 1.0).abs() < 1e-5);
    assert!(w.ray_cast(Vec3::new(1.0, -5.0, 2.0), Vec3::Y, 10.0).is_none());
    assert!(w.sphere_cast(Vec3::new(1.0, -5.0, 2.0), Vec3::Y, 10.0, 0.5).is_none());
    assert!(w.ray_cast(Vec3::new(1.0, 5.0, 2.0), -Vec3::Y, 4.9).is_none());
    assert!(w.ray_cast(Vec3::new(11.0, 5.0, 2.0), -Vec3::Y, 10.0).is_none());
}

#[test]
fn particle_sweep_is_one_sided_and_keeps_the_last_triangle() {
    let w = plane(10.0, 0.0);
    let (p, n) = w.particle_sweep(Vec3::new(1.0, 0.5, 2.0), Vec3::new(0.0, -1.0, 0.0)).expect("hit");
    assert!(p.distance(Vec3::new(1.0, 0.0, 2.0)) < 1e-6 && n.distance(Vec3::Y) < 1e-6);
    assert!(w.particle_sweep(Vec3::new(1.0, -0.5, 2.0), Vec3::Y).is_none(), "from below");
    assert!(w.particle_sweep(Vec3::new(1.0, 1.0, 2.0), -Vec3::Y).is_none(), "ends on the plane: t = 1");
    assert!(w.particle_sweep(Vec3::new(1.0, 0.5, 2.0), Vec3::new(0.0, -1e-5, 0.0)).is_none(), "too short");
    let mut w = CollisionWorld::default();
    let v =
        [Vec3::new(-1.0, 0.0, -1.0), Vec3::new(-1.0, 0.0, 1.0), Vec3::new(1.0, 0.0, 1.0), Vec3::new(1.0, 0.2, -1.0)];
    w.push_cluster(&v, None, None, &[[0, 1, 2], [0, 2, 3]], 0, 11);
    w.finish();
    let (_, n) = w.particle_sweep(Vec3::new(0.0, 0.5, 0.0), -Vec3::Y).expect("hit on the diagonal");
    let second = face_normal(v[0], v[2], v[3]).unwrap();
    assert!(n.distance(second) < 1e-6 && n.distance(Vec3::Y) > 1e-3, "{n}");
}

#[test]
fn the_triangle_rejection_changes_no_query() {
    let mut seed = 0x51ed_270b_27a3_c4e1u64;
    let mut f = |lo: f32, hi: f32| {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        lo + (hi - lo) * ((seed >> 40) as f32 / (1u64 << 24) as f32)
    };
    let mut w = CollisionWorld::default();
    for k in 0..12 {
        let size = [0.5, 3.0, 40.0][k % 3];
        let centre = Vec3::new(f(-20.0, 20.0), f(-5.0, 5.0), f(-20.0, 20.0));
        let v: Vec<Vec3> =
            (0..40).map(|_| centre + Vec3::new(f(-size, size), f(-size, size) * 0.3, f(-size, size))).collect();
        let mut tris: Vec<[u8; 3]> = (0..60).map(|_| [0; 3].map(|_| (f(0.0, 39.99) as u8).min(39))).collect();
        tris.push([3, 3, 4]);
        w.push_cluster(&v, None, None, &tris, 0, (k % 4) as u8);
    }
    w.finish();
    let (mut hits, mut n) = (0, 0);
    for i in 0..6000 {
        let start = Vec3::new(f(-30.0, 30.0), f(-8.0, 8.0), f(-30.0, 30.0));
        let len = [0.01, 0.4, 3.0, 30.0][i % 4];
        let dir = Vec3::new(f(-1.0, 1.0), f(-1.0, 1.0), f(-1.0, 1.0)).normalize_or(Vec3::Y);
        let radius = [0.0, 0.05, 0.3, 2.0][(i / 4) % 4];
        let mut q = SegmentQuery::sphere(start, start + dir * len, radius, if i % 5 == 0 { 0x2 } else { 0 });
        q.two_sided = i % 3 == 0;
        let (a, b) = (w.segment_query_with(&q, true), w.segment_query_with(&q, false));
        assert_eq!(a, b, "query {i}: {q:?}");
        hits += usize::from(a.is_some());
        n += 1;
    }
    assert!(hits > n / 10, "{hits} hits of {n}");
}

#[test]
fn the_cluster_cull_tests_the_bare_segment() {
    let (lo, hi) = (Vec3::new(1.0, -1.0, -1.0), Vec3::new(2.0, 1.0, 1.0));
    let pierces = |start: Vec3, len: f32| SegmentCull::new(start, start + Vec3::X * len).pierces(lo, hi);
    assert!(pierces(Vec3::ZERO, 1.5) && !pierces(Vec3::ZERO, 0.99));
    assert!(pierces(Vec3::new(0.95, 0.0, 0.0), 0.01) && !pierces(Vec3::new(0.85, 0.0, 0.0), 0.01));
    assert!(pierces(Vec3::new(0.0, 1.0, 0.0), 3.0) && !pierces(Vec3::new(0.0, 1.001, 0.0), 3.0));
    let far = |y: f32| SegmentCull::new(Vec3::new(1000.0, y, 0.0), Vec3::new(1003.0, y, 0.0));
    let (lo, hi) = (Vec3::new(1001.0, 990.0, -1.0), Vec3::new(1002.0, 1000.0, 1.0));
    assert!(far(1000.0005).pierces(lo, hi) && !far(1000.002).pierces(lo, hi));
}

#[test]
fn a_sphere_beside_a_clusters_box_never_reaches_its_triangles() {
    let mut w = CollisionWorld::default();
    crate::testutil::add_floor(&mut w, [-5.0, 5.0], [-5.0, 5.0], 0.0, 0);
    w.finish();
    let beside = SegmentQuery::sphere(Vec3::new(-6.0, 0.25, 0.0), Vec3::new(6.0, 0.05, 0.0), 0.3, 0);
    assert_eq!(w.segment_query(&beside), None);
    let into = SegmentQuery::sphere(Vec3::new(-6.0, 0.2, 0.0), Vec3::new(6.0, -0.2, 0.0), 0.3, 0);
    assert!(w.segment_query(&into).is_some_and(|h| h.t < 6.0));
}

#[test]
fn class_mask_skips_excluded_classes() {
    use crate::testutil::add_floor;
    let mut w = CollisionWorld::default();
    add_floor(&mut w, [-5.0, 5.0], [-5.0, 5.0], 1.0, 11);
    add_floor(&mut w, [-5.0, 5.0], [-5.0, 5.0], 0.0, 2);
    w.finish();
    let down = |mask| w.ray_cast_masked(Vec3::new(0.0, 5.0, 0.0), -Vec3::Y, 10.0, mask).map(|h| h.point.y);
    assert_eq!(down(0), Some(1.0));
    assert_eq!(down(RIDABLE_CLASS_MASK), Some(0.0));
    let sphere = w.sphere_cast_masked(Vec3::new(0.0, 5.0, 0.0), -Vec3::Y, 10.0, 0.25, RIDABLE_CLASS_MASK);
    assert!(sphere.is_some_and(|h| h.point.y.abs() < 1e-5));
    assert_eq!((class_byte(11), class_byte(0x10B), class_byte(40)), (11, 11, 0));
    assert!(class_passes(RIDABLE_CLASS_MASK, 0) && class_passes(RIDABLE_CLASS_MASK, 12));
    assert!(!class_passes(RIDABLE_CLASS_MASK, 1) && !class_passes(RIDABLE_CLASS_MASK, 11));
}

#[test]
fn sphere_cast_face_edge_vertex_and_overlap() {
    let flat = plane(10.0, 0.0);
    let h = flat.sphere_cast(Vec3::new(0.0, 3.0, 0.0), -Vec3::Y, 10.0, 0.5).expect("face");
    assert!((h.t - 2.5).abs() < 1e-5 && h.normal.distance(Vec3::Y) < 1e-5 && h.point.y.abs() < 1e-5);
    assert!((h.contact_normal - Vec3::Y).length() < 1e-5);
    let two_in = |w: &CollisionWorld, o: Vec3, d: Vec3| {
        w.segment_query(&SegmentQuery { start: o, end: o + d * 10.0, radius: 0.5, two_sided: true, class_mask: 0 })
    };
    assert!(two_in(&flat, Vec3::new(13.0, 0.3, 0.0), -Vec3::X).is_none());
    assert!(flat.sphere_cast(Vec3::new(10.3, 2.0, 0.0), -Vec3::Y, 10.0, 0.5).is_none());
    assert!(flat.sphere_cast(Vec3::new(0.0, -0.2, 0.0), -Vec3::Y, 1.0, 0.5).is_none());
    let mut w = plane(10.0, 0.0);
    w.set_cluster_entry_box(Vec3::new(-11.0, -1.0, -11.0), Vec3::new(11.0, 1.0, 11.0));
    let two = |o: Vec3, d: Vec3| two_in(&w, o, d);
    assert!(w.sphere_cast(Vec3::new(13.0, 0.3, 0.0), -Vec3::X, 10.0, 0.5).is_none());
    let h = two(Vec3::new(13.0, 0.3, 0.0), -Vec3::X).expect("edge");
    assert!((h.t - 2.6).abs() < 1e-4, "{}", h.t);
    assert!(h.point.distance(Vec3::new(10.0, 0.0, 0.0)) < 1e-4);
    assert!(h.contact_normal.distance(Vec3::new(0.4, 0.3, 0.0) / 0.5) < 1e-4);
    let d = Vec3::new(-1.0, 0.0, -1.0).normalize();
    let h = two(Vec3::new(12.0, 0.0, 12.0), d).expect("vertex");
    assert!((h.t - (8.0f32.sqrt() - 0.5)).abs() < 1e-4, "{}", h.t);
    let h = w.sphere_cast(Vec3::new(10.3, 2.0, 0.0), -Vec3::Y, 10.0, 0.5).expect("edge from above");
    assert!(h.point.distance(Vec3::new(10.0, 0.0, 0.0)) < 1e-4 && h.contact_normal.x > 0.5);
    let c = Vec3::new(0.0, 0.499, 0.0);
    assert!(w.sphere_cast(c, Vec3::X, 1.0, 0.5).is_none());
    assert_eq!(w.sphere_cast(c, -Vec3::Y, 1.0, 0.5).map(|h| h.t), Some(0.0));
    assert_eq!(w.sphere_cast(Vec3::new(0.0, -0.2, 0.0), -Vec3::Y, 1.0, 0.5).map(|h| h.t), Some(0.0));
    let h = w.ray_cast(Vec3::new(0.0, 1.0, 0.0), -Vec3::Y, 2.0).expect("line");
    assert!((h.t - 1.0).abs() < 1e-6);
    let mut contacts = Vec::new();
    w.overlap_sphere(c, 0.5, &mut contacts);
    assert!(!contacts.is_empty() && contacts.iter().all(|k| (k.depth - 0.001).abs() < 1e-5));
}
