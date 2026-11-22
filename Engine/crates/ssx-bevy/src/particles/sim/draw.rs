use std::ops::Range;
use std::sync::Arc;

use bevy::math::{Mat4, Vec3, Vec4};

use super::params::{ParamDef, name};
use super::{Context, EffectInstance, SetArena, View};

pub type VertexRecord = [Vec4; 3];

pub const REGISTRY_NAMES: [&str; 42] = [
    "fTimeSpeed",
    "vColorMultSpread",
    "vColorRampTimes",
    "vSizeRamp",
    "vSizeRampTimes",
    "bUseColorRamp",
    "bUseColorMult",
    "bUseSizeRamp",
    "bUseSizeMult",
    "fSizeMultX",
    "fSizeMultY",
    "iAlign",
    "iScreenSpace",
    "vPivotShift",
    "fNearFadeStart",
    "fNearFadeEnd",
    "fZBias",
    "fContactShadowFalloff",
    "fContactShadowOffset",
    "bUseAnimTexture",
    "iAnimTextureMode",
    "iAnimTextureCount",
    "iAnimTextureRows",
    "fAnimTextureSpeed",
    "iAnimTextureRandomStart",
    "iTile",
    "fCameraPush",
    "iNoFadeFar",
    "fGlareBloomScale",
    "fGlareBloomSpread",
    "fGlareSensitivityCenter",
    "fGlareSensitivityEdge",
    "fGlareSensitivityPower",
    "fGlareRotationRate",
    "fGlareDirCenterPower",
    "fGlareDirEdgePower",
    "fGlareDirOrientation",
    "fGlareSizeMultSpread",
    "fFlareOffsetScale",
    "fFlareEndScale",
    "bUseGlareSeed",
    "fFlareMovementRate",
];

pub const fn registry_slot(name: &str) -> Option<usize> {
    let mut i = 0;
    while i < REGISTRY_NAMES.len() {
        let (a, b) = (REGISTRY_NAMES[i].as_bytes(), name.as_bytes());
        if a.len() == b.len() {
            let mut j = 0;
            while j < a.len() && a[j] == b[j] {
                j += 1;
            }
            if j == a.len() {
                return Some(i);
            }
        }
        i += 1;
    }
    None
}

pub type Registry = [Vec4; REGISTRY_NAMES.len()];

pub struct RenderConsts {
    pub shader: Arc<str>,
    pub technique: Arc<str>,
    pub texture: Arc<str>,
    extras: Vec<(usize, ParamDef)>,
}

impl RenderConsts {
    pub(super) fn of(params: &super::Params) -> RenderConsts {
        let w = super::Weights::default();
        let p = View::new(params, &w);
        let text = |name: &str| Arc::<str>::from(p.text(name).unwrap_or_default());
        let mut extras: Vec<(usize, ParamDef)> = params
            .iter()
            .filter(|(n, _)| !p.built_in(n))
            .filter_map(|(n, def)| registry_slot(n).map(|slot| (slot, def.clone())))
            .collect();
        extras.sort_by_key(|(slot, _)| *slot);
        RenderConsts { shader: text("sShader"), technique: text("sTechnique"), texture: text("sTexture"), extras }
    }

    #[cfg(test)]
    pub fn extras_slots(&self) -> impl Iterator<Item = usize> + '_ {
        self.extras.iter().map(|(slot, _)| *slot)
    }
}

#[derive(Clone)]
pub struct Draw {
    pub effect: usize,
    pub world: Mat4,
    pub layer: i32,
    pub half: bool,
    pub tonemap: bool,
    pub render: Arc<RenderConsts>,
    pub time_now: i32,
    pub age_max: i32,
    pub age_warp_inv: f32,
    pub stretch: Vec3,
    pub stretch_per_particle: bool,
    pub color_mult: Vec4,
    pub color_ramp: [Vec4; 4],
    pub z_feather: [f32; 4],
    pub use_global_lighting: bool,
    pub light: (Vec3, Vec4, f32, f32),
    pub modulo_position: bool,
    pub dimensions: Vec3,
    pub stretch_per_camera: bool,
    pub mesh_type: i32,
    pub extras: Range<usize>,
    pub records: Range<usize>,
    pub aabb: (Vec3, Vec3),
}

#[derive(Default)]
pub struct DrawBuffers {
    pub draws: Vec<Draw>,
    pub records: Vec<VertexRecord>,
    pub extras: Vec<(usize, Vec4)>,
}

impl EffectInstance {
    fn reference(&self, path: &str) -> Option<Vec4> {
        let mut it = path.split('/');
        let (g, a, p) = (it.next()?, it.next()?, it.next()?);
        let gd = self.def.groups.iter().find(|x| x.name == g)?;
        let ad = gd.actions.iter().find(|x| x.class == a || x.instance_name.as_deref() == Some(a))?;
        Some(View::new(&ad.params, &self.weights).v(p, 0))
    }

    fn group_space(&self, gi: usize) -> i32 {
        let gp = View::new(&self.def.groups[gi].params, &self.weights);
        match (gp.i(name!("iInstancing")), gp.i(name!("iSpace"))) {
            (1, _) | (_, 0) => 0,
            (2, _) | (_, 1) => 1,
            _ => View::new(&self.def.params, &self.weights).i(name!("iSpace")),
        }
    }

    pub fn world_matrix(&self) -> Mat4 {
        let (r, t) = self.cur;
        Mat4::from_cols(r.x_axis.extend(0.0), r.y_axis.extend(0.0), r.z_axis.extend(0.0), t.extend(1.0))
    }

    pub fn draws(&self, index: usize, ctx: &Context, view_axis: Vec3, arena: &SetArena, out: &mut DrawBuffers) {
        for &(gi, ai, count) in &self.rendered {
            let particles = &arena.sets[self.sets[gi]].particles;
            let particles = &particles[..count.min(particles.len())];
            let ad = &self.def.groups[gi].actions[ai];
            let Some(render) = &ad.render else { continue };
            let p = View::new(&ad.params, &self.weights);
            let m = if self.group_space(gi) == 0 { self.world_matrix() } else { Mat4::IDENTITY };
            let world = m * Mat4::from_scale(Vec3::splat(p.f(name!("fScale"))));
            let age_max = p.text(name!("rKillAge")).and_then(|r| self.reference(r)).map_or(0, |v| v.x as i32);
            let (sv, ss) = (p.v3(name!("vStretchVelocity")), p.f(name!("fStretchSpeedScale")));
            let stretch = if ss >= 1.192_092_9e-7 && sv.length() > p.f(name!("fStretchSpeedMin")) {
                sv.normalize() * (1.0 + sv.length() / ss)
            } else {
                Vec3::ZERO
            };
            let per_particle = p.b(name!("bStretchPerParticle"));
            let first = out.records.len();
            let mut aabb = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
            let mut push = |q: &super::Particle| {
                let wp = world.transform_point3(q.pos);
                aabb = (aabb.0.min(wp), aabb.1.max(wp));
                let t0 = if per_particle { q.vel * ss } else { q.ori };
                out.records.push([q.pos.extend(q.size), t0.extend(q.birth), Vec4::new(q.stamp, 0.0, 0.0, 0.0)]);
            };
            if p.b(name!("bDepthSort")) {
                let mut order: Vec<(u32, u32)> = (0..particles.len())
                    .map(|i| {
                        (!view_axis.dot(world.transform_point3(particles[i].pos) - ctx.camera).to_bits(), i as u32)
                    })
                    .collect();
                order.sort_unstable();
                order.iter().for_each(|&(_, i)| push(&particles[i as usize]));
            } else {
                particles.iter().for_each(&mut push);
            }
            let records = first..out.records.len();
            let extras_first = out.extras.len();
            out.extras.extend(render.extras.iter().map(|(slot, def)| (*slot, p.blended(def, 0))));
            let draw = Draw {
                effect: index,
                world,
                layer: p.i(name!("iRenderLayer")),
                half: p.b(name!("bHalfSizeRender")),
                tonemap: p.b(name!("bToneMapRender")),
                render: render.clone(),
                time_now: ctx.time_ms,
                age_max,
                age_warp_inv: 1.0 / p.f(name!("fAgeWarp")),
                stretch,
                stretch_per_particle: per_particle,
                color_mult: p.v(name!("vColorMult"), 0),
                color_ramp: [
                    p.v(name!("vColorRamp"), 0),
                    p.v(name!("vColorRamp"), 1),
                    p.v(name!("vColorRamp"), 2),
                    p.v(name!("vColorRamp"), 3),
                ],
                z_feather: [
                    p.f(name!("fZFeatherHeight")),
                    p.f(name!("fZFeatherDist")),
                    p.f(name!("fZFeatherFalloff")),
                    p.f(name!("fZFeatherOffset")),
                ],
                use_global_lighting: p.b(name!("bUseGlobalLighting")),
                light: (
                    p.v3(name!("vLightPosition")),
                    p.v(name!("vLightColor"), 0),
                    p.f(name!("fLightRadius")),
                    p.f(name!("fLightFalloff")),
                ),
                modulo_position: p.b(name!("bModuloPosition")),
                dimensions: p.v3(name!("vDimensions")),
                stretch_per_camera: p.b(name!("bStretchPerCamera")),
                mesh_type: p.i(name!("iType")),
                extras: extras_first..out.extras.len(),
                records,
                aabb,
            };
            if p.b(name!("bFollowCamera")) {
                let n = p.i(name!("iPeriodicBoxNum")).max(1);
                let half = (n - 1) as f32 * 0.5;
                for (i, j, k) in (0..n).flat_map(|i| (0..n).flat_map(move |j| (0..n).map(move |k| (i, j, k)))) {
                    let cell = Vec3::new(i as f32, j as f32, k as f32) - Vec3::splat(half);
                    let t = ctx.camera + draw.dimensions * cell;
                    let world = draw.world * Mat4::from_translation(t);
                    let aabb = (draw.aabb.0 + t, draw.aabb.1 + t);
                    out.draws.push(Draw { world, aabb, ..draw.clone() });
                }
            } else {
                out.draws.push(draw);
            }
        }
    }
}
