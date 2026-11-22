#![allow(clippy::neg_cmp_op_on_partial_ord)]

use glam::{Mat4, Quat, Vec3, Vec4};

use crate::collision::{CollisionWorld, class_passes};

mod raster;

pub use raster::*;

pub const GRID: usize = 256;
const EPS: f32 = 1.192_092_9e-7;

#[derive(Clone, Debug)]
pub struct SurfaceAnalysis {
    pub to_grid: Mat4,
    pub to_world: Mat4,
    pub anchor: Vec3,
    pub dir: Vec3,
    pub heights: Vec<u8>,
    ridges: Ridges,
}

impl PartialEq for SurfaceAnalysis {
    fn eq(&self, o: &Self) -> bool {
        (self.to_grid, self.to_world, self.anchor, self.dir) == (o.to_grid, o.to_world, o.anchor, o.dir)
            && self.heights == o.heights
    }
}

pub fn initial() -> &'static SurfaceAnalysis {
    static INITIAL: std::sync::LazyLock<SurfaceAnalysis> = std::sync::LazyLock::new(SurfaceAnalysis::default);
    &INITIAL
}

impl Default for SurfaceAnalysis {
    fn default() -> Self {
        Self {
            to_grid: Mat4::IDENTITY,
            to_world: Mat4::IDENTITY,
            anchor: Vec3::ZERO,
            dir: Vec3::ZERO,
            heights: vec![0; GRID * GRID],
            ridges: Ridges::default(),
        }
    }
}

fn apply(m: &Mat4, v: Vec3) -> Vec3 {
    let (r0, r1, r2, r3) = (m.x_axis, m.y_axis, m.z_axis, m.w_axis);
    let f = |k: usize| v.x * r0[k] + (v.y * r1[k] + (v.z * r2[k] + r3[k]));
    Vec3::new(f(0), f(1), f(2))
}

fn apply_dir(m: &Mat4, v: Vec3) -> Vec3 {
    let (r0, r1, r2) = (m.x_axis, m.y_axis, m.z_axis);
    let f = |k: usize| v.x * r0[k] + (v.y * r1[k] + v.z * r2[k]);
    Vec3::new(f(0), f(1), f(2))
}

impl SurfaceAnalysis {
    pub fn from_rows(rows: [Vec4; 4]) -> Self {
        let to_grid = Mat4::from_cols(rows[0], rows[1], rows[2], rows[3]);
        Self { to_grid, to_world: to_grid.inverse(), ..Self::default() }
    }

    pub fn grid(&self, p: Vec3) -> Vec3 {
        apply(&self.to_grid, p)
    }

    pub fn grid_dir(&self, v: Vec3) -> Vec3 {
        apply_dir(&self.to_grid, v)
    }

    pub fn world(&self, g: Vec3) -> Vec3 {
        apply(&self.to_world, g)
    }

    pub fn world_dir(&self, g: Vec3) -> Vec3 {
        apply_dir(&self.to_world, g)
    }

    pub fn cell_world(&self, x: usize, y: usize) -> Vec3 {
        self.world(Vec3::new(x as f32, y as f32, f32::from(self.height(x, y))))
    }

    pub fn cell(&self, p: Vec3) -> Option<(usize, usize)> {
        let g = self.grid(p);
        let (ix, iy) = ((g.x + 0.5).floor(), (g.y + 0.5).floor());
        let inside = |v: f32| (0.0..GRID as f32).contains(&v);
        (inside(ix) && inside(iy)).then_some((ix as usize, iy as usize))
    }

    pub fn height(&self, ix: usize, iy: usize) -> u8 {
        self.heights[iy * GRID + ix]
    }

    pub fn ridge(&self, ix: usize, iy: usize) -> u8 {
        self.ridge_row(iy)[ix]
    }

    pub fn ridge_row(&self, iy: usize) -> &[u8; GRID] {
        self.ridges.rows[iy].get_or_init(|| {
            let t = self.ridges.tables.get_or_init(|| std::sync::Arc::new(RidgeTables::new(&self.to_world)));
            t.row(self.heights[iy * GRID..(iy + 1) * GRID].try_into().expect("a raster row"), iy)
        })
    }

    pub fn ridges(&self) -> Vec<u8> {
        (0..GRID).flat_map(|y| *self.ridge_row(y)).collect()
    }

    pub fn ridgiprocity_at(&self, p: Vec3) -> f32 {
        let g = self.grid(p);
        let inside = |v: f32| (0.0..GRID as f32).contains(&v);
        if !(inside(g.x) && inside(g.y)) {
            return 0.0;
        }
        f32::from(self.ridge(g.x.floor() as usize, g.y.floor() as usize)) / 256.0
    }
}

impl SurfaceAnalysis {
    pub fn cell_normal(&self, x: usize, y: usize, s: usize) -> Vec3 {
        let (hl, hr) = (self.height(x - s, y), self.height(x + s, y));
        let (hu, hd) = (self.height(x, y - s), self.height(x, y + s));
        if hl == 0 || hr == 0 || hu == 0 || hd == 0 {
            return Vec3::ZERO;
        }
        let ds = (2 * s) as f32;
        let a = self.world_dir(Vec3::new(ds, 0.0, f32::from(hr) - f32::from(hl)));
        let b = self.world_dir(Vec3::new(0.0, ds, f32::from(hd) - f32::from(hu)));
        b.cross(a).normalize_or_zero()
    }

    pub fn estimate_normal(&self, p: Vec3, fallback_y: f32) -> Option<Vec3> {
        let g = self.grid(p);
        let (cx, cy) = ((g.x + 0.5).floor(), (g.y + 0.5).floor());
        let inside = |v: f32| (0.0..GRID as f32).contains(&v);
        if !(inside(cx) && inside(cy)) {
            return None;
        }
        let (cx, cy) = (cx as i32, cy as i32);
        let mut acc = Vec3::ZERO;
        for j in 0..9 {
            for i in 0..9 {
                let (x, y) = (cx - 16 + 4 * i, cy - 16 + 4 * j);
                if !((2..254).contains(&x) && (2..254).contains(&y)) {
                    continue;
                }
                let d = (((x - cx).pow(2) + (y - cy).pow(2)) as f32).sqrt();
                let w = (1.0 - (d - 8.0) / 14.0).clamp(0.0, 1.0);
                let mut n = self.cell_normal(x as usize, y as usize, 2);
                if n == Vec3::ZERO {
                    n = Vec3::new(0.0, fallback_y, 0.0);
                }
                acc += n * w;
            }
        }
        if acc.abs().max_element() <= EPS { None } else { Some(acc.normalize()) }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SaGround {
    pub contact: Vec3,
    pub contact_valid: bool,
    pub normal: Vec3,
    pub normal_valid: bool,
    pub distance: f32,
}

pub fn sa_ground(prev: &SaGround, sa: &SurfaceAnalysis, traj: Vec3, joint: Vec3) -> SaGround {
    let mut out = SaGround { normal: Vec3::ZERO, normal_valid: false, distance: 0.0, ..*prev };
    let zt = (sa.grid(traj).z + 0.5).floor();
    let Some((ix, iy)) = sa.cell(joint) else { return out };
    let (mut best, mut bx, mut by) = (sa.height(ix, iy), ix, iy);
    for y in iy as i32 - 1..=iy as i32 + 1 {
        for x in ix as i32 - 6..=ix as i32 + 6 {
            if !((0..GRID as i32).contains(&x) && (0..GRID as i32).contains(&y)) {
                continue;
            }
            let h = sa.height(x as usize, y as usize);
            if h > best && f32::from(h) < zt {
                (best, bx, by) = (h, x as usize, y as usize);
            }
        }
    }
    out.contact = sa.world(Vec3::new(bx as f32, by as f32, f32::from(best)));
    out.contact_valid = best > 0 && f32::from(best) < zt;
    let n = sa.estimate_normal(out.contact, 0.0);
    out.normal = n.unwrap_or(Vec3::ZERO);
    out.distance = (traj - out.contact).dot(out.normal);
    out.normal_valid = n.is_some() && out.distance >= 0.0;
    out
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LookAhead {
    pub landing_mode: bool,
    pub ticks: f32,
    pub pos: Vec3,
    pub vel: Vec3,
}

#[derive(Clone, Copy, Debug)]
pub struct DispatchInputs {
    pub traj: Vec3,
    pub velocity: Vec3,
    pub orientation: Quat,
    pub in_air: bool,
    pub grind_steering: Option<(Vec3, Vec3)>,
    pub ballistic_valid: bool,
    pub ballistic_end: (Vec3, Vec3),
    pub ticks_to_land: f32,
    pub l2: bool,
    pub super_grind: bool,
    pub wind: Vec3,
    pub k_h_air: f32,
    pub k_v_air: f32,
    pub g_air: f32,
    pub dt: f32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SaFeature {
    pub look_ahead: LookAhead,
    pub aabb: (Vec3, Vec3),
}

impl SaFeature {
    fn update_look_ahead(&mut self, i: &DispatchInputs) {
        let la = &mut self.look_ahead;
        if i.dt.abs() <= EPS || (la.landing_mode && i.in_air) {
            return;
        }
        if !i.in_air {
            *la = LookAhead { landing_mode: false, ticks: 0.0, pos: i.traj, vel: i.velocity };
            return;
        }
        let dt = i.dt;
        let step = |p: &mut Vec3, v: &mut Vec3| {
            let mut a = crate::rider::drag_accel(i.k_h_air, i.k_v_air, *v, i.wind, dt);
            a.y -= i.g_air;
            *v += a * dt;
            *p += *v * dt;
        };
        let n = ((1.0 / dt) * la.ticks).floor().max(0.0) as u32;
        let (mut p, mut v, mut t) = (i.traj, i.velocity, 0.0);
        let mut matched = false;
        for _ in 0..n {
            step(&mut p, &mut v);
            t += dt;
            if (p - la.pos).abs().max_element() <= 0.05 {
                matched = true;
                break;
            }
        }
        if !matched {
            (p, v, t) = (i.traj, i.velocity, 0.0);
        }
        let (lo, hi) = self.aabb;
        while p.cmpge(lo).all()
            && p.cmple(hi).all()
            && (p - i.traj).length_squared() < 100.0 * 100.0
            && i.ticks_to_land > t
        {
            step(&mut p, &mut v);
            t += dt;
        }
        *la = if i.ballistic_valid && t >= i.ticks_to_land {
            LookAhead { landing_mode: true, ticks: 0.0, pos: i.traj, vel: i.velocity }
        } else {
            LookAhead { landing_mode: false, ticks: t, pos: p, vel: v }
        };
    }
}

struct Region {
    to_grid: Mat4,
    to_raster: Mat4,
    anchor: Vec3,
    dir: Vec3,
    aabb: (Vec3, Vec3),
}

fn region(i: &DispatchInputs, la: &LookAhead) -> Region {
    let (p, v) = if let Some(gs) = i.grind_steering {
        gs
    } else if la.landing_mode {
        i.ballistic_end
    } else if i.in_air {
        (la.pos, la.vel)
    } else {
        (i.traj, i.velocity)
    };
    let flat = i.in_air || i.grind_steering.is_some();
    let d = if flat { Vec3::new(v.x, 0.0, v.z) } else { v };
    let c = Vec3::new(d.z, 0.0, -d.x);
    let (f, side, up) = if c.x * c.x + c.z * c.z == 0.0 {
        let q = i.orientation;
        (q * Vec3::X, -(q * Vec3::Z), q * Vec3::Y)
    } else {
        let f = d.normalize();
        let side = c.normalize();
        (f, side, f.cross(side))
    };
    let top = p + f * if flat { 0.0 } else { 4.0 } + up * 3.0;
    let (sg, sz) = (15.9375, 31.875);
    let row = |k: usize| Vec4::new(side[k] * sg, f[k] * sg, up[k] * sz, 0.0);
    let t = Vec4::new(127.5 - sg * top.dot(side), 127.5 - sg * top.dot(f), 255.0 - sz * top.dot(up), 1.0);
    let to_grid = Mat4::from_cols(row(0), row(1), row(2), t);
    let flip = |r: Vec4| Vec4::new(r.x, r.y, -r.z, r.w);
    let t_r = Vec4::new(t.x, t.y, 255.0 - t.z, 1.0);
    let to_raster = Mat4::from_cols(flip(row(0)), flip(row(1)), flip(row(2)), t_r);
    let mut lo = Vec3::splat(f32::MAX);
    let mut hi = Vec3::splat(f32::MIN);
    for a in [-8.0, 8.0] {
        for b in [-8.0, 8.0] {
            for h in [-8.0, 0.0] {
                let q = top + side * a + f * b + up * h;
                (lo, hi) = (lo.min(q), hi.max(q));
            }
        }
    }
    Region { to_grid, to_raster, anchor: p, dir: d, aabb: (lo, hi) }
}

pub fn gather_mask(grind_steering: bool, in_air: bool, landing_mode: bool, l2: bool, super_grind: bool) -> u32 {
    const RAILS: u32 = !0x1004;
    if grind_steering {
        RAILS
    } else if in_air && landing_mode {
        if l2 { RAILS } else { 0 }
    } else if in_air || super_grind {
        RAILS
    } else if l2 {
        0x802
    } else {
        0
    }
}

impl SaFeature {
    pub fn dispatch(&mut self, c: &CollisionWorld, i: &DispatchInputs) -> SurfaceAnalysis {
        self.prepare(i).run(c)
    }

    pub fn prepare(&mut self, i: &DispatchInputs) -> SaJob {
        self.update_look_ahead(i);
        let la = self.look_ahead;
        let region = region(i, &la);
        self.aabb = region.aabb;
        let mask = gather_mask(i.grind_steering.is_some(), i.in_air, la.landing_mode, i.l2, i.super_grind);
        SaJob { region, mask }
    }
}

pub struct SaJob {
    region: Region,
    mask: u32,
}

impl SaJob {
    pub fn run(&self, c: &CollisionWorld) -> SurfaceAnalysis {
        let (r, mask) = (&self.region, self.mask);
        let to_world = r.to_grid.inverse();
        let mut heights = vec![0u8; GRID * GRID];
        let (lo, hi) = r.aabb;
        let m = &r.to_raster;
        let fused = fused_ops();
        let mut scratch = [Vec3::ZERO; CLUSTER_VERTICES];
        c.bvh.traverse(
            |a, b| a.cmple(hi).all() && b.cmpge(lo).all(),
            |ci| {
                let cl = &c.clusters[ci as usize];
                if !class_passes(mask, cl.class) || !(cl.min.cmple(hi).all() && cl.max.cmpge(lo).all()) {
                    return;
                }
                rasterize_cluster(&mut heights, m, c, cl, &mut scratch, fused);
            },
        );
        SurfaceAnalysis {
            to_grid: r.to_grid,
            to_world,
            anchor: r.anchor,
            dir: r.dir,
            heights,
            ridges: Ridges::default(),
        }
    }
}

#[cfg(test)]
mod tests;
