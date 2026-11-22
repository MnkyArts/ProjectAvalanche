use bevy::math::{Mat3, Vec3};

use super::params::{Name, name};
use super::{ActionState, Context, Locator, Particle, Rng, View};

pub(super) struct EmitEnv<'a> {
    pub(super) ctx: &'a Context,
    pub(super) inst_start: i32,
    pub(super) gate: bool,
    pub(super) dist: f32,
    pub(super) lod: f32,
    pub(super) free: usize,
}

pub(super) fn emit_count(p: &View, st: &mut ActionState, env: &EmitEnv, rng: &mut Rng) -> usize {
    let ctx = env.ctx;
    let mut restarted = false;
    if st.time_started_ms == 0 || (env.inst_start == 0 && p.b(name!("bRespectExternalTrigger"))) {
        st.time_started_ms = ctx.time_ms;
        restarted = true;
    }
    let centers = p.count(name!("vCenter")) as i64;
    let per_center = if p.b(name!("bEmitRatePerCenter")) { centers } else { 1 };
    let (rate, dur, cycles) =
        (p.i(name!("iEmitRate")) as i64, p.i(name!("iEmitDurationMs")) as i64, p.i(name!("iEmitCycles")));
    if cycles == 1 && dur > 0 && dur < 16 {
        let n = if restarted { rate * dur * per_center / 1000 } else { 0 };
        return n.clamp(0, env.free as i64) as usize;
    }
    if rate == 0
        || env.dist > env.lod
        || !env.gate
        || centers == 0
        || env.free == 0
        || st.time_started_ms < 0
        || ctx.dt == 0.0
    {
        return 0;
    }
    let r = rng.next4().x;
    let elapsed = ctx.time_ms - st.time_started_ms;
    let delay = (p.i(name!("iEmitDelayVarianceMs")) as f32 * r + 0.5) as i32 + p.i(name!("iEmitDelayMs"));
    let dur = dur as i32;
    if dur > 0 {
        if cycles != 0 {
            let period = dur + delay;
            if period == 0 {
                return 0;
            }
            let (cyc, rem) = (elapsed / period, elapsed % period);
            if (cycles > 0 && cyc > cycles) || rem < delay {
                return 0;
            }
        } else if elapsed > dur || delay > elapsed {
            return 0;
        }
    } else if delay > elapsed {
        return 0;
    }
    let per_frame = p.b(name!("bEmitRatePerFrame")) || p.b(name!("bInject"));
    let f = if per_frame { rate as f32 } else { (rate * per_center) as f32 * ctx.dt_smoothed };
    let n = f as i64;
    let frac = f - n as f32;
    let iv = if frac == 0.0 { 1000 } else { ((1.0 / frac + 0.5) as i64).clamp(1, 1000) };
    let extra = (ctx.frame as i64 % iv == 0 && !per_frame) as i64;
    (n + extra).clamp(0, env.free as i64) as usize
}

#[allow(clippy::too_many_arguments)]
fn emit_tail(
    p: &View,
    loc: &Locator,
    ctx: &Context,
    u: f32,
    local: (Vec3, Vec3, Vec3, Vec3),
    inv_mass: f32,
    size: f32,
    birth: f32,
    rng: &mut Rng,
) -> Particle {
    let (pos_l, vel_l, ori_l, av_l) = local;
    let identity = Locator { rows: Mat3::IDENTITY, pos: Vec3::ZERO, prev_pos: Vec3::ZERO, ..*loc };
    let frame = if p.i(name!("iObeyLocator")) != 0 { loc } else { &identity };
    let ori = frame.rot(ori_l);
    let vloc = loc.prev_vel.lerp(loc.vel, u);
    let adopt = p.f(name!("fVelocityAdoption")) + p.f(name!("fVelocityAdoptionSpread")) * rng.spread4().x;
    let mut v = vloc * adopt + frame.rot(vel_l);
    let av = frame.rot(av_l);
    let base = match p.i(name!("iSubFrameTechnique")) {
        1 => frame.prev_pos.lerp(frame.pos, u),
        2 => {
            let (u2, u3) = (u * u, u * u * u);
            let (h00, h10, h01, h11) = (2.0 * u3 - 3.0 * u2 + 1.0, u3 - 2.0 * u2 + u, 3.0 * u2 - 2.0 * u3, u3 - u2);
            frame.prev_pos * h00 + loc.prev_vel * (h10 * 0.016667) + frame.pos * h01 + loc.vel * (h11 * 0.016667)
        }
        _ => frame.pos,
    };
    let pos = frame.rot(pos_l) + base - v * (u * ctx.dt);
    let ang_adopt =
        p.f(name!("fAngularVelocityAdoption")) + p.f(name!("fAngularVelocityAdoptionSpread")) * rng.spread4().x;
    v += loc.ang_axis.cross(pos - frame.pos) * loc.ang_rate * ang_adopt;
    Particle { pos, inv_mass, vel: v, size, ori, birth, ang_vel: av, stamp: 0.0 }
}

pub(super) fn emit_box(p: &View, loc: &Locator, ctx: &Context, n: usize, rng: &mut Rng, out: &mut Vec<Particle>) {
    let centers = p.count(name!("vCenter")).max(1);
    let mut ci = ((rng.next4().x * 32767.0) as i64 as usize) % centers;
    let (size_mean, dep) = (p.f(name!("fSizeMean")), p.f(name!("fMassDependenceOnSize")));
    let density = p.f(name!("fMassMean")) / (size_mean * size_mean * size_mean);
    let dt_ms = ctx.dt * 1000.0;
    for i in 0..n {
        let u = i as f32 * (1.0 / n as f32);
        let at = |name: Name| p.v(name, ci).truncate();
        let (s1, s2, s3, s4) = (rng.spread4(), rng.spread4(), rng.spread4(), rng.spread4());
        let (s5, s6) = (rng.spread4().x, rng.spread4().x);
        let vel = at(name!("vVelocityMean")) + at(name!("vVelocitySpread")) * s1.truncate();
        let pos = at(name!("vCenter")) + at(name!("vDimensions")) * s2.truncate();
        let ori = at(name!("vOrientationMean")) + at(name!("vOrientationSpread")) * s3.truncate();
        let av = at(name!("vAngularVelocityMean")) + at(name!("vAngularVelocitySpread")) * s4.truncate();
        let size = size_mean + p.f(name!("fSizeSpread")) * s5;
        let mass = dep * density * size * size * size
            + (1.0 - dep) * (p.f(name!("fMassMean")) + p.f(name!("fMassSpread")) * s6);
        let r7 = rng.next4().x;
        let birth = ctx.time_ms as f32 - p.i(name!("iAgeSpreadMs")) as f32 * r7 - dt_ms * (n - 1 - i) as f32 / n as f32;
        out.push(emit_tail(p, loc, ctx, u, (pos, vel, ori, av), 1.0 / mass, size, birth, rng));
        ci = (ci + 1) % centers;
    }
}

pub(super) fn emit_spheroid(p: &View, loc: &Locator, ctx: &Context, n: usize, rng: &mut Rng, out: &mut Vec<Particle>) {
    let centers = p.count(name!("vCenter")).max(1);
    let mut ci = ((rng.next4().x * 32767.0) as i64 as usize) % centers;
    let shape = p.v(name!("vHemisphereShape"), 0);
    let skew = p.v(name!("vInnerOuterSkew"), 0);
    let d2r = 0.017_453_29_f32;
    let e = (1.0 - skew.z) / 3.0;
    let size_mean = p.f(name!("fSizeMean"));
    for i in 0..n {
        let u = i as f32 * (1.0 / n as f32);
        let at = |name: Name| p.v(name, ci).truncate();
        let d1 = rng.next4();
        let phi = (shape.w - shape.z) * d2r * d1.x + shape.z * d2r;
        let cos_t = d1.y * (shape.y - shape.x) + shape.x;
        let r = skew.x + (skew.y - skew.x) * if d1.z > 0.0 { d1.z.powf(e) } else { 0.0 };
        let size = p.f(name!("fSizeSpread")) * (d1.w - 0.5) + size_mean;
        let rc = cos_t * r;
        let rs = (r * r - rc * rc).max(0.0).sqrt();
        let dims = at(name!("vDimensions"));
        let off = Vec3::new(rs * phi.cos() * dims.x + 1e-10 * (d1.x - 0.5), rc * dims.y, rs * phi.sin() * dims.z);
        let pos = at(name!("vCenter")) + off;
        let mass = (p.f(name!("fMassMean")) * size * size * size / (size_mean * size_mean * size_mean)).abs();
        let d2 = rng.spread4();
        let radial = d2.w * p.f(name!("fRadialVelocitySpread")) + p.f(name!("fRadialVelocityMean"));
        let vel = at(name!("vVelocityMean"))
            + off / off.length() * radial
            + at(name!("vVelocitySpread")) * d2.truncate().normalize();
        let d3 = rng.spread4();
        let ori = at(name!("vOrientationMean")) + at(name!("vOrientationSpread")) * d3.truncate();
        let birth = ctx.time_ms as f32 - p.i(name!("iAgeSpreadMs")) as f32 * (d3.w + 0.5);
        let d4 = rng.spread4();
        let av = at(name!("vAngularVelocityMean")) + at(name!("vAngularVelocitySpread")) * d4.truncate();
        out.push(emit_tail(p, loc, ctx, u, (pos, vel, ori, av), 1.0 / mass, size, birth, rng));
        ci = (ci + 1) % centers;
    }
}

pub(super) fn emit_sphere(p: &View, loc: &Locator, ctx: &Context, n: usize, rng: &mut Rng, out: &mut Vec<Particle>) {
    let centers = p.count(name!("vCenter")).max(1);
    let mut ci = ((rng.next4().x * 32767.0) as i64 as usize) % centers;
    let size_mean = p.f(name!("fSizeMean"));
    let density = p.f(name!("fMassMean")) / (size_mean * size_mean * size_mean);
    let dt_ms = ctx.dt * 1000.0;
    for i in 0..n {
        let u = i as f32 * (1.0 / n as f32);
        let at = |name: Name| p.v(name, ci).truncate();
        let a = rng.spread4();
        let vel = at(name!("vVelocityMean")) + a.truncate().normalize_or_zero() * p.v(name!("vVelocitySpread"), ci).x;
        let b = rng.spread4();
        let size = size_mean + p.f(name!("fSizeSpread")) * b.x;
        let mass = (density * size * size * size).abs();
        let c = rng.spread4();
        let pos = at(name!("vCenter")) + at(name!("vDimensions")) * c.truncate();
        let d = rng.spread4();
        let ori = at(name!("vOrientationMean")) + at(name!("vOrientationSpread")) * d.truncate();
        let birth = ctx.time_ms as f32
            - p.i(name!("iAgeSpreadMs")) as f32 * (d.w + 0.5)
            - dt_ms * (n - 1 - i) as f32 / n as f32;
        let e = rng.spread4();
        let av = at(name!("vAngularVelocityMean")) + at(name!("vAngularVelocitySpread")) * e.truncate();
        out.push(emit_tail(p, loc, ctx, u, (pos, vel, ori, av), 1.0 / mass, size, birth, rng));
        ci = (ci + 1) % centers;
    }
}
