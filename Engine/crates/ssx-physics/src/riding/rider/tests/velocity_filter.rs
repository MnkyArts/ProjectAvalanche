use super::*;
use crate::steering::update_concavity;

fn filter(v: Vec3, f: Vec3) -> Vec3 {
    v + (f - v) * (-0.1 * DT_TICKS).exp()
}

fn towards_a_wall(z: f32, v: f32) -> (World, RiderState) {
    let mut c = plane(100.0, 0.0);
    let p = |x: f32, y: f32| Vec3::new(x, y, z);
    add_quad(&mut c, [p(-10.0, 0.0), p(-10.0, 3.0), p(10.0, 3.0), p(10.0, 0.0)], 0);
    c.finish();
    let w = world(c);
    let s = moving(&w, Vec3::new(0.0, 1.0, 0.0), Vec3::Z, v);
    (w, s)
}

fn rig_ops_then_spq(w: &World, s: &mut RiderState, f0: Vec3, dp: Vec3) -> Vec3 {
    s.filtered_velocity = f0;
    s.traj_cached = s.traj;
    super::graph_io::rig_ops(s, w, dp, None, DT_TICKS);
    let f1 = s.filtered_velocity;
    super::spq::spq(s, w, DT_TICKS, Some(()));
    f1
}

#[test]
fn a_bounce_leaves_the_velocity_filters_state_alone() {
    let (w, mut s) = towards_a_wall(0.7, 0.5);
    let f0 = Vec3::new(0.0, -0.3, 0.3);
    let dp = Vec3::Z * 1.0;
    let f1 = rig_ops_then_spq(&w, &mut s, f0, dp);
    assert_eq!(f1, filter(dp / DT_TICKS, f0), "the input is this update's displacement / dt");
    assert_eq!(s.impacts, 1, "the step into the wall bounces");
    let b = s.traj_velocity;
    assert!(b.z < 0.0 && s.velocity == b, "the component and the carving velocity take the bounce: {b:?}");
    assert_eq!(s.filtered_velocity, f1, "the bounce does not reach the filter's channel");
    let dp2 = b * DT_TICKS;
    super::graph_io::rig_ops(&mut s, &w, dp2, None, DT_TICKS);
    assert_eq!(s.filtered_velocity, filter(dp2 / DT_TICKS, f1));
}

#[test]
fn the_concavity_after_a_bounce_reads_the_rig_ops_filter() {
    let (w, mut s) = towards_a_wall(0.7, 0.5);
    let f1 = rig_ops_then_spq(&w, &mut s, Vec3::new(0.0, -0.3, 0.3), Vec3::Z * 1.0);
    assert_eq!(s.impacts, 1);
    let mut expected = s.steering;
    update_concavity(&mut expected, &w.tuning.carving, true, f1, s.ground_normal, DT_TICKS);
    assert!(expected.concavity > 0.5, "the filter still points into the ground: {}", expected.concavity);
    super::ride::ride(&mut s, &w, &Input::default(), (false, false), DT_TICKS);
    assert!(
        (s.steering.concavity - expected.concavity).abs() < 1e-6,
        "{} vs {}",
        s.steering.concavity,
        expected.concavity
    );
}

fn beside_a_wall(floor_y: f32, x: f32) -> World {
    let mut c = plane(100.0, 0.0);
    let p = |x: f32, y: f32, z: f32| Vec3::new(x, floor_y + y, z);
    let v = [
        p(x, -5.0, -10.0),
        p(x, -5.0, 10.0),
        p(x, 5.0, 10.0),
        p(x, 5.0, -10.0),
        p(x - 0.3, 5.0, 10.0),
        p(x - 0.3, 5.0, -10.0),
    ];
    c.push_cluster(&v, None, None, &[[0, 1, 2], [0, 2, 3], [3, 2, 4], [3, 4, 5]], 0, 0);
    c.finish();
    world(c)
}

#[test]
fn a_soft_deflection_reaches_the_carving_velocity_only_on_the_ground() {
    let w = beside_a_wall(50.0, 0.12);
    let mut s = moving(&w, Vec3::new(0.0, 1.0, 0.0), Vec3::Z, 0.5);
    s.traj = Vec3::new(0.0, 50.0, 0.0);
    (s.state, s.contact) = (RideState::Airborne, None);
    let aerial = Vec3::new(0.0, -0.1, 0.5);
    s.velocity = aerial;
    let dp = Vec3::Z * 1.0;
    let f1 = rig_ops_then_spq(&w, &mut s, Vec3::new(0.0, -0.3, 0.3), dp);
    assert!(s.impact.in_soft_collision && s.impacts == 0, "a soft contact without a bounce");
    assert_eq!((s.traj_velocity, s.velocity, s.filtered_velocity), (dp / DT_TICKS, aerial, f1));
    let mut s = moving(&world(plane(100.0, 0.0)), Vec3::new(0.0, 1.0, 0.0), Vec3::Z, 0.5);
    let w = beside_a_wall(0.0, s.traj.x + 0.12);
    let v = s.velocity;
    let f1 = rig_ops_then_spq(&w, &mut s, Vec3::new(0.0, -0.3, 0.3), dp);
    assert!(s.impact.in_soft_collision && s.impacts == 0);
    assert_eq!((s.traj_velocity, s.filtered_velocity), (dp / DT_TICKS, f1));
    assert!(s.velocity.x < -1e-3 && (s.velocity.length() - v.length()).abs() < 1e-6, "{:?}", s.velocity);
}

#[test]
fn a_bot_bounce_leaves_the_velocity_filters_state_alone() {
    let w = world(plane(100.0, 0.0));
    let mut s = moving(&w, Vec3::new(0.0, 1.0, 0.0), Vec3::Z, 0.5);
    s.ghost_impact = Some(Vec3::new(0.0, 0.0, -1.0));
    let f1 = rig_ops_then_spq(&w, &mut s, Vec3::new(0.0, -0.3, 0.3), Vec3::Z * 1.0);
    assert!(s.traj_velocity != Vec3::Z * 0.5 && s.velocity == s.traj_velocity, "bounced: {:?}", s.traj_velocity);
    assert_eq!(s.filtered_velocity, f1);
}
