use std::collections::HashMap;
use std::sync::Arc;

use bevy::math::{Mat3, Quat, Vec3, Vec4};

mod actions;
mod defs;
mod draw;
mod emit;
mod params;

use actions::{ActionPrivate, accelerate, drag, kill_age, periodic_box, speed_limit, turbulence, weather};
pub use defs::{EffectDef, Kind};
#[cfg(test)]
pub use draw::RenderConsts;
pub use draw::{Draw, DrawBuffers, REGISTRY_NAMES, Registry, VertexRecord, registry_slot};
use emit::{EmitEnv, emit_box, emit_count, emit_sphere, emit_spheroid};
use params::name;
pub use params::{Params, View, Weights};

#[derive(Clone, Debug)]
pub struct Rng([u32; 4]);

pub const RNG_SEEDS: [u32; 4] = [0x00F1_FBDC, 0x43BB_7EB2, 0x5CAD_6A98, 0x53DF_C8C1];

fn xorshift(mut x: u32) -> u32 {
    x ^= x << 13;
    x ^= x >> 17;
    x ^= x << 5;
    x
}

fn unit_float(x: u32) -> f32 {
    f32::from_bits(0x3F80_0000 | (x & 0x007F_FFFF)) - 1.0
}

impl Default for Rng {
    fn default() -> Self {
        Rng(RNG_SEEDS)
    }
}

impl Rng {
    pub fn next4(&mut self) -> Vec4 {
        let mut out = [0.0; 4];
        for (lane, o) in self.0.iter_mut().zip(&mut out) {
            *lane = xorshift(*lane);
            *o = unit_float(*lane);
        }
        Vec4::from_array(out)
    }
    pub fn spread4(&mut self) -> Vec4 {
        self.next4() - Vec4::splat(0.5)
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Particle {
    pub pos: Vec3,
    pub inv_mass: f32,
    pub vel: Vec3,
    pub size: f32,
    pub ori: Vec3,
    pub birth: f32,
    pub ang_vel: Vec3,
    pub stamp: f32,
}

#[derive(Clone, Copy, Debug)]
pub struct Locator {
    pub rows: Mat3,
    pub pos: Vec3,
    pub prev_pos: Vec3,
    pub vel: Vec3,
    pub prev_vel: Vec3,
    pub ang_axis: Vec3,
    pub ang_rate: f32,
}

impl Locator {
    fn local() -> Locator {
        Locator {
            rows: Mat3::IDENTITY,
            pos: Vec3::ZERO,
            prev_pos: Vec3::ZERO,
            vel: Vec3::ZERO,
            prev_vel: Vec3::ZERO,
            ang_axis: Vec3::ZERO,
            ang_rate: 0.0,
        }
    }

    fn rot(&self, v: Vec3) -> Vec3 {
        self.rows.x_axis * v.x + self.rows.y_axis * v.y + self.rows.z_axis * v.z
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Context {
    pub time_ms: i32,
    pub dt: f32,
    pub frame: u32,
    pub dt_smoothed: f32,
    pub camera: Vec3,
    pub player_shadow: f32,
    pub weather: Weather,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Weather {
    pub wind: Vec3,
    pub precipitation: f32,
}

#[derive(Clone, Copy, Debug, Default)]
struct ActionState {
    time_started_ms: i32,
}

#[derive(Clone, Debug, Default)]
pub struct ParticleSet {
    pub particles: Vec<Particle>,
    first_new: usize,
    last_frame: Option<u32>,
}

#[derive(Default)]
pub struct SetArena {
    pub sets: Vec<ParticleSet>,
    shared: HashMap<(usize, usize), usize>,
    privates: HashMap<(usize, usize, usize), ActionPrivate>,
}

impl SetArena {
    fn alloc(&mut self) -> usize {
        self.sets.push(ParticleSet::default());
        self.sets.len() - 1
    }
}

pub struct EffectInstance {
    pub def: Arc<EffectDef>,
    pub sets: Vec<usize>,
    actions: Vec<Vec<ActionState>>,
    rendered: Vec<(usize, usize, usize)>,
    pub state: u8,
    pub start_ms: i32,
    last_start: f32,
    cur: (Mat3, Vec3),
    prev: (Mat3, Vec3),
    vel: Vec3,
    prev_vel: Vec3,
    pub ao: f32,
    pub weights: Weights,
}

#[derive(Clone, Debug)]
pub struct RecordView {
    pub scale: Vec3,
    pub rotation: Quat,
    pub position: Vec3,
    pub start: f32,
    pub stop: f32,
    pub instant_off: bool,
    pub ao: f32,
    pub blend: Vec<(u8, f32)>,
}

fn rows_of(scale: Vec3, q: Quat) -> Mat3 {
    Mat3::from_cols(q * Vec3::X * scale.x, q * Vec3::Y * scale.y, q * Vec3::Z * scale.z)
}

#[derive(Clone, Copy, Debug)]
pub struct Visibility {
    pub metric: f32,
    pub visible: bool,
}

pub const CULL_METRIC: f32 = 0.05;

impl EffectInstance {
    pub fn new(def: Arc<EffectDef>, arena: &mut SetArena) -> Self {
        let key = Arc::as_ptr(&def) as usize;
        let w = Weights::default();
        let sets = def
            .groups
            .iter()
            .enumerate()
            .map(|(gi, g)| {
                if View::new(&g.params, &w).i(name!("iInstancing")) != 0 {
                    match arena.shared.get(&(key, gi)) {
                        Some(&s) => s,
                        None => {
                            let s = arena.alloc();
                            arena.shared.insert((key, gi), s);
                            s
                        }
                    }
                } else {
                    arena.alloc()
                }
            })
            .collect();
        let actions = def.groups.iter().map(|g| vec![ActionState::default(); g.actions.len()]).collect();
        EffectInstance {
            def,
            sets,
            actions,
            rendered: Vec::new(),
            state: 4,
            start_ms: 0,
            last_start: -1.0,
            cur: (Mat3::IDENTITY, Vec3::ZERO),
            prev: (Mat3::IDENTITY, Vec3::ZERO),
            vel: Vec3::ZERO,
            prev_vel: Vec3::ZERO,
            ao: 1.0,
            weights: Weights::default(),
        }
    }

    pub fn cull_params(&self) -> (f32, i32) {
        let base = Weights::default();
        let v = View::new(&self.def.params, &base);
        (v.f(name!("fRadius")), v.i(name!("iCullBehavior")))
    }

    pub fn make_dormant(&mut self, arena: &mut SetArena) {
        self.state = 4;
        self.kill(arena);
        (self.last_start, self.start_ms) = (-1.0, 0);
    }

    fn kill(&self, arena: &mut SetArena) {
        for &s in &self.sets {
            arena.sets[s].particles.clear();
        }
    }

    pub fn apply_record(&mut self, rec: &RecordView, time: f32, dt_frame: f32, vis: Visibility, arena: &mut SetArena) {
        let active = rec.start != -1.0 && time >= rec.start && (time < rec.stop || rec.stop == -1.0);
        let mut restart = false;
        if active {
            if rec.start > self.last_start && time - rec.start < 0.1 {
                restart = true;
                self.start_ms = 0;
            } else {
                self.start_ms = (rec.start * 1000.0) as i32;
            }
            self.last_start = rec.start;
            self.state = 0;
        } else if self.state == 0 || self.state == 1 {
            if rec.instant_off {
                self.state = 4;
                self.kill(arena);
            } else {
                self.state = 2;
            }
        }
        self.weights = Weights::from_list(&rec.blend);
        if self.state != 4 {
            let (_, behaviour) = self.cull_params();
            if vis.metric < CULL_METRIC {
                self.state = match behaviour {
                    1 => 1,
                    3 => 3,
                    4 => {
                        self.kill(arena);
                        4
                    }
                    _ => 2,
                };
            } else if !vis.visible && behaviour != 0 {
                self.state = behaviour.clamp(0, 255) as u8;
            }
        }
        if self.state != 4 {
            let m = (rows_of(rec.scale, rec.rotation), rec.position);
            if restart {
                (self.cur, self.prev, self.vel, self.prev_vel) = (m, m, Vec3::ZERO, Vec3::ZERO);
            } else {
                self.prev = self.cur;
                self.cur = m;
                self.prev_vel = self.vel;
                if dt_frame > 1.5e-5 {
                    self.vel = (self.cur.1 - self.prev.1) / dt_frame;
                }
            }
        }
        self.ao = rec.ao;
    }

    pub fn update(&mut self, ctx: &Context, rng: &mut Rng, arena: &mut SetArena) {
        self.rendered.clear();
        if self.state == 4 {
            return;
        }
        let world_loc = self.locator(ctx.dt);
        let local_loc = Locator::local();
        let dist = (self.cur.1 - ctx.camera).length();
        let gate = self.state == 0 || self.state == 1;
        let def = self.def.clone();
        let w = self.weights;
        let effect_space = View::new(&def.params, &w).i(name!("iSpace"));
        for (gi, gd) in def.groups.iter().enumerate() {
            let gp = View::new(&gd.params, &w);
            let instancing = gp.i(name!("iInstancing"));
            let space = match (instancing, gp.i(name!("iSpace"))) {
                (1, _) | (_, 0) => 0,
                (2, _) | (_, 1) => 1,
                _ => effect_space,
            };
            let loc = if space == 1 { &world_loc } else { &local_loc };
            let def_key = Arc::as_ptr(&def) as usize;
            let SetArena { sets, privates, .. } = &mut *arena;
            let set = &mut sets[self.sets[gi]];
            let first = instancing == 0 || set.last_frame != Some(ctx.frame);
            let cap = gp.i(name!("iNumParticlesMax")).max(0) as usize;
            set.particles.truncate(cap);
            set.first_new = set.particles.len();
            let env =
                EmitEnv { ctx, inst_start: self.start_ms, gate, dist, lod: gp.f(name!("fLevelOfDetail")), free: 0 };
            for (ai, (ad, st)) in gd.actions.iter().zip(&mut self.actions[gi]).enumerate() {
                let (per_local, per_world) = ad.kind.instancing_flags();
                if !(first || (instancing == 1 && per_local) || (instancing == 2 && per_world)) {
                    continue;
                }
                let p = View::new(&ad.params, &w);
                if !p.b(name!("bActive")) {
                    continue;
                }
                let ps = &mut set.particles;
                match ad.kind {
                    Kind::EmitBox | Kind::EmitSphere | Kind::EmitSpheroid => {
                        let env = EmitEnv { free: cap.saturating_sub(ps.len()), ..env };
                        let n = emit_count(&p, st, &env, rng);
                        if n > 0 {
                            match ad.kind {
                                Kind::EmitBox => emit_box(&p, loc, ctx, n, rng, ps),
                                Kind::EmitSphere => emit_sphere(&p, loc, ctx, n, rng, ps),
                                _ => emit_spheroid(&p, loc, ctx, n, rng, ps),
                            }
                        }
                    }
                    Kind::Evolve => {
                        let zero = p.i(name!("iZeroVelocity")) != 0;
                        for q in ps.iter_mut() {
                            q.pos += q.vel * ctx.dt;
                            q.ori += q.ang_vel * ctx.dt;
                            if zero {
                                q.vel = Vec3::ZERO;
                            }
                        }
                    }
                    Kind::Accelerate => accelerate(&p, ps, ctx.dt),
                    Kind::Drag => drag(&p, ps, ctx.dt),
                    Kind::SpeedLimit => speed_limit(&p, ps),
                    Kind::KillAge => kill_age(&p, ps, ctx.time_ms),
                    Kind::Weather => weather(&p, ps, ctx, privates.entry((def_key, gi, ai)).or_default(), rng),
                    Kind::Turbulence => turbulence(&p, ps, ctx, privates.entry((def_key, gi, ai)).or_default(), rng),
                    Kind::PeriodicBox => periodic_box(&p, ps),
                    Kind::Shadow | Kind::Ao => {
                        let ao = self.ao.clamp(0.0, 0.999);
                        let stamp =
                            if ad.kind == Kind::Shadow { (ctx.player_shadow * 1000.0).floor() + ao } else { ao };
                        let from = set.first_new / 64 * 64;
                        for q in ps.iter_mut().skip(from).filter(|q| q.stamp == 0.0) {
                            q.stamp = stamp;
                        }
                    }
                    Kind::Render => {
                        if !ps.is_empty() && !matches!(self.state, 1 | 3 | 4) {
                            self.rendered.push((gi, ai, ps.len()));
                        }
                    }
                    Kind::CopyToParameter | Kind::Other => {}
                }
            }
            set.last_frame = Some(ctx.frame);
        }
        if self.start_ms == 0 {
            self.start_ms = ctx.time_ms;
        }
    }

    fn locator(&self, dt: f32) -> Locator {
        let unscaled = |m: Mat3| {
            Quat::from_mat3(&Mat3::from_cols(m.x_axis.normalize(), m.y_axis.normalize(), m.z_axis.normalize()))
        };
        let rel = unscaled(self.cur.0) * unscaled(self.prev.0).inverse();
        let (mut axis, mut angle) = rel.to_axis_angle();
        if angle > std::f32::consts::PI {
            (axis, angle) = (-axis, std::f32::consts::TAU - angle);
        }
        let rate = if dt != 0.0 && angle.is_finite() { angle / dt } else { 0.0 };
        Locator {
            rows: self.cur.0,
            pos: self.cur.1,
            prev_pos: self.prev.1,
            vel: self.vel,
            prev_vel: self.prev_vel,
            ang_axis: if axis.is_finite() { axis } else { Vec3::ZERO },
            ang_rate: rate,
        }
    }
}

#[cfg(test)]
mod tests;
