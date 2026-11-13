use bevy::prelude::*;

use crate::camera::graph;
use crate::ride::Ride;

pub(super) const TICKS_SINCE_COLLISION: f32 = 200.0;

const MAX_DEGREES_FOR_COLLISION_TEST: f32 = 10.0;
const MAX_DISTANCE_FOR_COLLISION_TEST: f32 = 1.0;

const QUERY_RADIUS: f32 = 0.25;
const QUERY_CLASS_MASK: u32 = 0x1C00;

const RAY_GRID: [Vec2; 9] = [
    Vec2::ZERO,
    Vec2::new(0.0, -1.0),
    Vec2::new(0.0, 1.0),
    Vec2::new(-1.0, 0.0),
    Vec2::new(1.0, 0.0),
    Vec2::new(0.0, -2.0),
    Vec2::new(0.0, 2.0),
    Vec2::new(-2.0, 0.0),
    Vec2::new(2.0, 0.0),
];

#[derive(Clone, Copy, Debug, Default)]
pub(super) struct Queries {
    local: Vec2,
    pub(super) collided: bool,
    pub(super) was_colliding: bool,
    pub(super) collided_all: bool,
    pub(super) was_colliding_all: bool,
}

pub(super) fn physics_queries(ride: &Ride, q: &mut Queries, cam: Vec3, p: Vec3, up: Vec3, dt: f32) -> Vec3 {
    let to_player = p - cam;
    let len = to_player.length();
    let f = if len > 0.0 { to_player / len } else { Vec3::ZERO };
    let dist = len.max(2.0);
    let d = if q.collided_all { graph::DISTANCE } else { dist };
    let side = up.cross(f).try_normalize().unwrap_or(Vec3::ZERO);
    let up2 = f.cross(side).try_normalize().unwrap_or(Vec3::ZERO);
    let a = (MAX_DISTANCE_FOR_COLLISION_TEST / dist).atan().min(0.5 * MAX_DEGREES_FOR_COLLISION_TEST.to_radians());
    let ray = -f * d * 1.1;
    let turns = [(side, 0.0), (side, -a), (side, a), (up2, a), (up2, -a), (side, -2.0 * a), (side, 2.0 * a)];
    let turns = turns.into_iter().chain([(up2, 2.0 * a), (up2, -2.0 * a)]);
    let collision = &ride.world().collision;
    let mut hit = [false; 9];
    let (mut free, mut best, mut best_fraction) = (None, 0, -1.0);
    for (i, (axis, angle)) in turns.enumerate() {
        let r = Quat::from_axis_angle(axis, angle) * ray;
        let l = r.length();
        let h = (l > 0.0).then(|| collision.sphere_cast_masked(p, r / l, l, QUERY_RADIUS, QUERY_CLASS_MASK)).flatten();
        hit[i] = h.is_some();
        let fraction = h.map_or(1.0, |h| (h.t / l).clamp(0.0, 1.0));
        if !hit[i] && free.is_none() {
            free = Some(i);
        }
        if fraction > best_fraction {
            (best, best_fraction) = (i, fraction);
        }
    }
    let chosen = free.unwrap_or(best);
    let mut local: Vec2 =
        (0..9).filter(|&i| hit[i] && i != chosen).map(|i| (RAY_GRID[chosen] - RAY_GRID[i]).normalize_or_zero()).sum();
    if q.was_colliding {
        local += (q.local - local) * (-0.2 * dt).exp();
    }
    let factor = 0.02 + 0.78 * (dist.clamp(1.0, 20.0) - 1.0) / 19.0;
    *q = Queries {
        local,
        collided: hit.iter().any(|&h| h),
        was_colliding: q.collided,
        collided_all: free.is_none(),
        was_colliding_all: q.collided_all,
    };
    cam + (side * local.x + up2 * local.y) * factor
}

const NEAR_QUERY_EPSILON: f32 = 0.3;
const MIN_NEAR_CLIPPING_PLANE: f32 = 0.05;
pub(super) const DEFAULT_NEAR_CLIPPING_PLANE: f32 = 0.5;
const NEAR_QUERY_ASPECT: f32 = 16.0 / 9.0;

pub(super) fn post_update_near(
    collision: &ssx_physics::collision::CollisionWorld,
    position: Vec3,
    rotation: Quat,
    fov_deg: f32,
    near: f32,
) -> f32 {
    let depth = near + NEAR_QUERY_EPSILON;
    let half_width = depth * (fov_deg.to_radians() * 0.5).tan();
    let half_height = half_width / NEAR_QUERY_ASPECT;
    let corner = |y: f32, z: f32| position + rotation * Vec3::new(depth, y, z);
    let c = [
        corner(half_height, -half_width),
        corner(half_height, half_width),
        corner(-half_height, -half_width),
        corner(-half_height, half_width),
    ];
    let ends =
        [(c[0] + c[1]) * 0.5, (c[0] + c[2]) * 0.5, (c[1] + c[3]) * 0.5, (c[2] + c[3]) * 0.5, c[0], c[1], c[2], c[3]];
    let fraction = ends
        .into_iter()
        .filter_map(|end| {
            let len = (end - position).length();
            let q = ssx_physics::collision::SegmentQuery::line(position, end, QUERY_CLASS_MASK);
            collision.segment_query(&q).map(|h| (h.t / len).clamp(0.0, 1.0))
        })
        .fold(1.0_f32, f32::min);
    (fraction * depth - NEAR_QUERY_EPSILON).max(MIN_NEAR_CLIPPING_PLANE)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ssx_physics::collision::CollisionWorld;

    fn wall(at: f32, facing_camera: bool) -> CollisionWorld {
        let p = |y: f32, z: f32| Vec3::new(at, y, z);
        let mut v = [p(-1.0, -1.0), p(-1.0, 1.0), p(1.0, 1.0), p(1.0, -1.0)];
        if !facing_camera {
            v.reverse();
        }
        let mut w = CollisionWorld::default();
        w.push_cluster(&v, None, None, &[[0, 1, 2], [0, 2, 3]], 0, 0);
        w.finish();
        w
    }

    #[test]
    fn the_near_plane_moves_in_front_of_the_nearest_surface() {
        let near = |w: &CollisionWorld| post_update_near(w, Vec3::ZERO, Quat::IDENTITY, 90.0, 0.5);
        assert!((near(&wall(0.4, true)) - 0.1).abs() < 1e-5, "{}", near(&wall(0.4, true)));
        assert_eq!(near(&wall(0.2, true)), MIN_NEAR_CLIPPING_PLANE);
        assert_eq!(near(&wall(0.9, true)), 0.5);
        assert_eq!(near(&wall(0.4, false)), 0.5);
        assert_eq!(near(&CollisionWorld::default()), 0.5);
    }

    #[test]
    fn the_sweeps_reach_the_near_rectangles_edges() {
        let p = |x: f32, y: f32| Vec3::new(x, y, 0.6);
        let mut w = CollisionWorld::default();
        let v = [p(0.0, 1.0), p(2.0, 1.0), p(2.0, -1.0), p(0.0, -1.0)];
        w.push_cluster(&v, None, None, &[[0, 1, 2], [0, 2, 3]], 0, 0);
        w.finish();
        let near = post_update_near(&w, Vec3::ZERO, Quat::IDENTITY, 90.0, 0.5);
        assert!((near - (0.75 * 0.8 - 0.3)).abs() < 1e-5, "{near}");
    }
}
