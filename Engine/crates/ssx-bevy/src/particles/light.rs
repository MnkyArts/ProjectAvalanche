use std::path::Path;

use anyhow::Result;
use bevy::math::{Affine3A, Mat4, Quat, Vec3, Vec4};
use ssx_assets::light_script::LightBehaviour;
use ssx_assets::track_lights::{LightNode, LightShape, load_track_lights};
use ssx_physics::controllers::{Controllers, LightUpdate};

use crate::particles::ParticleLight;

struct Candidate {
    position: Vec3,
    radius: f32,
    shape: Vec3,
    color: Vec4,
    diffuse: f32,
    ctrl: Option<(u32, [u32; 3])>,
}

#[derive(Clone, Copy)]
struct Smoothed {
    shape: Vec3,
    color: Vec4,
    diffuse: f32,
}

pub struct NearestLight {
    candidates: Vec<Candidate>,
    live: bevy::platform::collections::HashMap<(u32, [u32; 3]), (LightBehaviour, f32)>,
    smoothed: Option<Smoothed>,
    rng: crate::particles::sim::Rng,
}

fn node_affine(n: &LightNode) -> Affine3A {
    let [x, y, z, w] = n.orientation;
    Affine3A::from_scale_rotation_translation(
        Vec3::new(n.scale[0], n.scale[1], n.scale[2]),
        Quat::from_xyzw(x, y, z, w),
        Vec3::new(n.position[0], n.position[1], n.position[2]),
    )
}

impl NearestLight {
    pub fn load(big_root: &Path, track: &str) -> Result<Self> {
        let lights = load_track_lights(big_root, track)?;
        let mut candidates = Vec::new();
        for (object, placements) in &lights.instances {
            let object = &lights.objects[*object];
            let key = object.key;
            let dynamic = object.controller_script.is_some();
            let nodes = object.lods.first().map(Vec::as_slice).unwrap_or_default();
            for placement in placements {
                let ctrl = dynamic.then(|| (key, placement.position.map(f32::to_bits)));
                let instance = crate::render::objects::placement_transform(placement).compute_affine();
                for node in nodes {
                    let Some(light) = &node.light else { continue };
                    let (shape, dynamic_ok) = match &light.shape {
                        LightShape::Point { outer_radius, inner_radius } => {
                            (Vec3::new(*inner_radius, *outer_radius, light.fall_off), true)
                        }
                        LightShape::Spot { outer_length, inner_length, .. } => {
                            (Vec3::new(*inner_length, *outer_length, light.fall_off), false)
                        }
                        LightShape::Box { .. } => continue,
                    };
                    if !light.enable || (dynamic && !dynamic_ok) {
                        continue;
                    }
                    let world = instance * node_affine(node);
                    let scale = world
                        .matrix3
                        .x_axis
                        .length()
                        .max(world.matrix3.y_axis.length())
                        .max(world.matrix3.z_axis.length());
                    candidates.push(Candidate {
                        position: world.translation.into(),
                        radius: shape.y * scale,
                        shape,
                        color: Vec4::from_array(light.color),
                        diffuse: light.diffuse_intensity,
                        ctrl,
                    });
                }
            }
        }
        Ok(NearestLight { candidates, live: Default::default(), smoothed: None, rng: Default::default() })
    }
}

fn visible(centre: Vec3, radius: f32, clip_from_world: &Mat4, camera: Vec3, tan_half_fov: f32) -> bool {
    let m = clip_from_world.transpose();
    let (r0, r1, r2, r3) = (m.x_axis, m.y_axis, m.z_axis, m.w_axis);
    let planes = [r3 + r0, r3 - r0, r3 + r1, r3 - r1, r3 - r2];
    let inside = planes.iter().all(|p| {
        let n = p.truncate();
        (n.dot(centre) + p.w) / n.length() >= -radius
    });
    let d = (centre - camera).length();
    inside && (d <= radius || radius / (d * tan_half_fov) > 0.002)
}

impl NearestLight {
    pub fn pick(
        &mut self,
        point: Vec3,
        clip_from_world: &Mat4,
        camera: Vec3,
        tan_half_fov: f32,
        controllers: Option<&Controllers>,
    ) -> ParticleLight {
        // PORT(choice): the map is kept between updates (cleared, its memory kept) and hashed with Bevy's fixed hasher
        // (foldhash); it was a new `std` map (SipHash) each update (the whole pick 0.06 ms of the main thread a ride
        // step on Mount Eddie, 0.10 on Vorassay: a traced build with timers, round perf-x1). The same entries; only
        // its lookups read it. The console's storage of the entities' lights is not decoded.
        let live = &mut self.live;
        live.clear();
        for e in controllers.map_or(&[][..], |c| &c.entities).iter().filter(|e| e.owner.is_none()) {
            let Some(run) = e.fx.lights.get("").filter(|r| !r.group && r.on) else { continue };
            let b = match run.update {
                LightUpdate::Blink { strength, frequency, gap } => LightBehaviour::Blink { strength, frequency, gap },
                LightUpdate::Flicker { strength } => LightBehaviour::Flicker { strength },
                _ => LightBehaviour::On,
            };
            live.insert((e.object, e.position.to_array().map(f32::to_bits)), (b, run.time));
        }
        let mut best: Option<(f32, usize, f32)> = None;
        for dynamic in [true, false] {
            for (i, c) in self.candidates.iter().enumerate().filter(|(_, c)| c.ctrl.is_some() == dynamic) {
                let factor = match c.ctrl {
                    Some(k) => match live.get(&k) {
                        Some((b, t)) => b.factor(*t, self.rng.next4().x),
                        None => continue,
                    },
                    None => 1.0,
                };
                let d2 = (c.position - point).length_squared();
                if d2 < best.map_or(400.0, |b| b.0)
                    && visible(c.position, c.radius, clip_from_world, camera, tan_half_fov)
                {
                    best = Some((d2, i, factor));
                }
            }
        }
        let Some((_, i, factor)) = best else {
            return ParticleLight {
                position: Vec3::ZERO,
                colour: Vec4::new(1.0, 1.0, 1.0, 0.0),
                radius: 1.0,
                falloff: 1.0,
            };
        };
        let c = &self.candidates[i];
        let found = Smoothed { shape: c.shape, color: c.color, diffuse: c.diffuse * factor };
        let prev = *self.smoothed.get_or_insert(found);
        let out = Smoothed {
            shape: found.shape.lerp(prev.shape, 0.6),
            color: found.color.lerp(prev.color, 0.6),
            diffuse: found.diffuse + (prev.diffuse - found.diffuse) * 0.6,
        };
        self.smoothed = Some(out);
        let tiny = 1.0 / 65536.0;
        ParticleLight {
            position: c.position,
            colour: (out.color.truncate() * out.diffuse).extend(0.0),
            radius: out.shape.y.max(tiny),
            falloff: 1.0 / out.shape.z.max(tiny),
        }
    }
}
