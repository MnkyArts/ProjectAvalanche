use std::collections::BTreeMap;

use anyhow::{Context, Result};
use bevy::math::{Affine3A, DVec2, DVec3};
use bevy::prelude::{IVec2, Quat, UVec2, Vec2, Vec3};
use ssx_assets::light_script::LightBehaviour;
use ssx_assets::track_lights::text::Strings;
use ssx_assets::track_lights::{LightNode, LightShape, load_track_lights};

use super::{CELL, Kind, Placed, Projected, TEXT_LAYER, light_words, sphere_cos};
use crate::render::lighting::game::GameLightUniform;

#[derive(Default)]
pub(super) struct Words(pub(super) Vec<[u32; 4]>);

impl Words {
    fn f(&mut self, v: [f32; 4]) {
        self.0.push(v.map(f32::to_bits));
    }
    fn u(&mut self, v: [u32; 4]) {
        self.0.push(v);
    }
    pub(super) fn bytes(&self) -> Vec<u8> {
        self.0.iter().flatten().flat_map(|w| w.to_le_bytes()).collect()
    }
}

const HEADER: u32 = 4;

const STACKS: u32 = 10;
const SLICES: u32 = 10;
const RECORD: u32 = 10;

pub(super) fn collect(
    big_root: &std::path::Path,
    track: Option<&str>,
    map: &[(u32, ssx_assets::track_objects::Placement)],
) -> Result<(Vec<Placed>, String)> {
    let mut lights = match track {
        Some(t) => load_track_lights(big_root, t).with_context(|| format!("{t}: light nodes"))?,
        None => Default::default(),
    };
    if !map.is_empty() {
        lights.extend(ssx_assets::track_lights::map_lights(big_root, map).context("the map's placed objects' lights")?);
    }
    let mut placed = Vec::new();
    let (mut entity, mut off, mut text) = (0u32, 0usize, 0usize);
    let mut strings: Option<Strings> = None;
    let text_off = std::env::var("SSX_TEXT_PROJECTIONS").is_ok_and(|v| v == "off");
    for (set, (object, placements)) in lights.instances.iter().enumerate() {
        let object = &lights.objects[*object];
        // TODO(guess): "controller ⇒ dynamic light" is inferred (0x8298FA18; 0x829241C8 registers static lights only for descriptor type 0 / 5, byte +0x18 0); settle: the writer of the descriptor's +0x14 / +0x18 (the instance-set build).
        let behaviour = object.controller_script.as_ref().map(|_| LightBehaviour::Off);
        let object_key = object.key;
        let nodes = object.lods.first().map(Vec::as_slice).unwrap_or_default();
        for (pi, placement) in placements.iter().enumerate() {
            let instance = crate::render::objects::placement_transform(placement).compute_affine();
            for (ni, node) in nodes.iter().enumerate() {
                let Some(light) = &node.light else { continue };
                if !light.enable {
                    off += 1;
                    continue;
                }
                let (kind, texture) = match &light.shape {
                    LightShape::Point { .. } => (Kind::Point, None),
                    LightShape::Spot { .. } => (Kind::Spot, None),
                    LightShape::Box { unk_ebeca5b9: true, .. } if text_off => {
                        off += 1;
                        continue;
                    }
                    LightShape::Box { unk_ebeca5b9: true, unk_fe912176: key, .. } => {
                        if strings.is_none() {
                            strings = Some(Strings::load(big_root)?);
                        }
                        let string = strings.as_ref().map_or_else(|| key.clone(), |s| s.text(key));
                        let colour = [light.color[0], light.color[1], light.color[2]].map(f32::to_bits);
                        text += 1;
                        (Kind::Textured, Some(Projected::Text { string, colour }))
                    }
                    LightShape::Box { projected_texture: Some(t), .. } => {
                        (Kind::Textured, Some(Projected::File(t.clone())))
                    }
                    LightShape::Box { .. } => (Kind::Box, None),
                };
                let bin = kind as u32 * 2 + light.enable_specular as u32;
                // TODO(guess): dynamic lights within a bin follow the track's order, the rider's after them; settle: the DynamicLightsEntity registration order (0x8297FDF0 callers).
                let order = match behaviour {
                    None => (bin, 0, set, ni, pi),
                    Some(_) => (bin, 1, set, pi, ni),
                };
                placed.push(Placed {
                    world: instance * node_affine(node),
                    light: light.clone(),
                    factor: None,
                    ctrl: behaviour.map(|_| (object_key, placement.position.map(f32::to_bits))),
                    node: node_affine(node),
                    scale: Vec3::from_array(placement.scale),
                    order,
                    behaviour,
                    entity,
                    texture,
                    texture_layer: 0,
                });
            }
            if behaviour.is_some() {
                entity += 1;
            }
        }
    }
    placed.sort_by_key(|p| p.order);
    let mut kinds: BTreeMap<&str, usize> = BTreeMap::new();
    for p in &placed {
        *kinds.entry(p.light.shape.kind()).or_default() += 1;
    }
    let summary = format!(
        "local lights: {} placed ({kinds:?}), {} on entities' controllers; {off} off (Enable), {text} text projections",
        placed.len(),
        placed.iter().filter(|p| p.behaviour.is_some()).count()
    );
    Ok((placed, summary))
}

fn node_affine(n: &LightNode) -> Affine3A {
    let [x, y, z, w] = n.orientation;
    Affine3A::from_scale_rotation_translation(
        Vec3::new(n.scale[0], n.scale[1], n.scale[2]),
        Quat::from_xyzw(x, y, z, w),
        Vec3::new(n.position[0], n.position[1], n.position[2]),
    )
}

pub(super) fn assign_layers(placed: &mut [Placed]) -> Vec<Projected> {
    let mut paths: Vec<Projected> = Vec::new();
    for p in placed.iter_mut() {
        if let Some(t) = &p.texture {
            p.texture_layer = match paths.iter().position(|q| q == t) {
                Some(i) => i as u32,
                None => {
                    paths.push(t.clone());
                    paths.len() as u32 - 1
                }
            };
        }
    }
    paths
}

fn constants(p: &Placed) -> ([f32; 3], [f32; 3], Vec3, f32, [f32; 2]) {
    let m = p.world.matrix3;
    let scale = m.x_axis.length().max(m.y_axis.length()).max(m.z_axis.length());
    let origin = Vec3::from(p.world.translation);
    match p.light.shape {
        LightShape::Point { outer_radius, inner_radius } => {
            let a1 = [inner_radius, outer_radius, 1.0 / p.light.fall_off];
            let s = outer_radius / sphere_cos();
            (a1, [0.0; 3], origin, outer_radius * scale, [s, s])
        }
        LightShape::Spot { cone_angle, penumbra_angle, outer_length, inner_length, drop_off } => {
            let half = cone_angle * 0.5;
            let outer = penumbra_angle.max(0.0) + half;
            let inner = penumbra_angle.min(0.0) + half;
            let a1 = [inner_length, outer_length, 1.0 / p.light.fall_off];
            let a2 = [(inner as f64).cos() as f32, (outer as f64).cos() as f32, drop_off];
            let r = ((outer as f64).tan() as f32) * outer_length;
            (a1, a2, origin, outer_length * scale, [r, outer_length])
        }
        LightShape::Box { width, height, penumbra, outer_length, inner_length, .. } => {
            let a1 = [inner_length, outer_length, 1.0 / p.light.fall_off];
            let a2 = [width * 0.5, height * 0.5, penumbra];
            let centre = p.world.transform_point3(Vec3::new(0.0, 0.0, -0.5 * outer_length));
            let radius = 0.5 * Vec3::new(width, height, outer_length).length() * scale;
            (a1, a2, centre, radius, [0.0, 0.0])
        }
    }
}

fn vis_radius(r: f32) -> f32 {
    Vec3::splat(r).length()
}

pub(super) fn switched_off(p: &Placed) -> bool {
    p.factor.is_none() && p.behaviour == Some(LightBehaviour::Off)
}

fn flags(p: &Placed, kind: u32) -> u32 {
    let mode = match (p.factor, p.behaviour) {
        (Some(_), _) | (None, Some(LightBehaviour::Off)) => 3,
        (None, None | Some(LightBehaviour::On)) => 0,
        (None, Some(LightBehaviour::Flicker { .. })) => 1,
        (None, Some(LightBehaviour::Blink { .. })) => 2,
    };
    let text = if matches!(p.texture, Some(Projected::Text { .. })) { TEXT_LAYER } else { 0 };
    kind | (p.light.enable_specular as u32) << 2 | text | mode << 4 | p.texture_layer << 8
}

pub(super) fn bin_of(p: &Placed) -> u32 {
    let kind = match (&p.light.shape, &p.texture) {
        (LightShape::Point { .. }, _) => Kind::Point,
        (LightShape::Spot { .. }, _) => Kind::Spot,
        (LightShape::Box { .. }, Some(_)) => Kind::Textured,
        (LightShape::Box { .. }, None) => Kind::Box,
    };
    kind as u32 * 2 + p.light.enable_specular as u32
}

pub(super) fn record(w: &mut Words, p: &Placed) {
    let (a1, a2, centre, radius, vol) = constants(p);
    let inv = p.world.inverse();
    let m = inv.matrix3;
    let t = Vec3::from(inv.translation);
    for k in 0..3 {
        let row = m.row(k);
        w.f([row.x, row.y, row.z, t[k]]);
    }
    let kind = bin_of(p) / 2;
    let o = Vec3::from(p.world.translation);
    w.u([o.x.to_bits(), o.y.to_bits(), o.z.to_bits(), flags(p, kind)]);
    let c = p.light.color;
    w.f([c[0], c[1], c[2], p.light.diffuse_intensity]);
    w.f([a1[0], a1[1], a1[2], p.light.specular_intensity]);
    w.f([a2[0], a2[1], a2[2], vis_radius(radius)]);
    let axis = Vec3::from(p.world.matrix3.z_axis).normalize_or_zero();
    w.u([axis.x.to_bits(), axis.y.to_bits(), axis.z.to_bits(), p.entity]);
    let (strength, frequency, gap) = match (p.factor, p.behaviour) {
        (Some(f), _) => (f, 0.0, 0.0),
        (None, Some(LightBehaviour::Flicker { strength })) => (strength, 0.0, 0.0),
        (None, Some(LightBehaviour::Blink { strength, frequency, gap })) => (strength, frequency, gap),
        _ => (0.0, 0.0, 0.0),
    };
    w.f([centre.x, centre.y, centre.z, strength]);
    w.f([frequency, gap, vol[0], vol[1]]);
}

pub(super) fn bounds(p: &Placed) -> [f32; 4] {
    let m = p.world.matrix3;
    let gram = m.transpose() * m;
    let stretch =
        [gram.x_axis, gram.y_axis, gram.z_axis].map(|c| c.abs().element_sum()).into_iter().fold(0.0, f32::max);
    let stretch = stretch.sqrt();
    let (centre, extent, ends) = match p.light.shape {
        LightShape::Point { outer_radius, inner_radius } => (Vec3::ZERO, outer_radius, inner_radius <= outer_radius),
        LightShape::Spot { cone_angle, penumbra_angle, outer_length, inner_length, .. } => {
            let (centre, extent) = spot_reach(cone_angle, penumbra_angle, outer_length);
            (centre, extent, inner_length <= outer_length)
        }
        LightShape::Box { width, height, outer_length, inner_length, penumbra, .. } => (
            Vec3::new(0.0, 0.0, -0.5 * outer_length),
            0.5 * Vec3::new(width, height, outer_length).length(),
            inner_length <= outer_length && penumbra >= 0.0,
        ),
    };
    let centre = p.world.transform_point3(centre);
    let radius = extent.abs() * stretch * 1.0001 + 1e-4;
    let bounded = ends && radius.is_finite() && centre.is_finite();
    [centre.x, centre.y, centre.z, if bounded { radius } else { f32::MAX }]
}

fn spot_reach(cone_angle: f32, penumbra_angle: f32, l: f32) -> (Vec3, f32) {
    let half = cone_angle * 0.5;
    let (outer, inner) = (penumbra_angle.max(0.0) + half, penumbra_angle.min(0.0) + half);
    let (cos_outer, cos_inner) = ((outer as f64).cos() as f32, (inner as f64).cos() as f32);
    let cos = cos_outer - 0.001;
    if cos_inner >= cos_outer && cos > 0.5 && l >= 0.0 {
        let r = l / (2.0 * cos);
        (Vec3::new(0.0, 0.0, -r), r)
    } else {
        (Vec3::ZERO, l)
    }
}

const MAX_CELLS: u32 = 1024;

// PORT(choice): a 2-D cell grid of light lists, 32 m cells, at most 1024 per side, sized once per level by the lights
// as placed, with an outside list; a light is in the lists of the cells its bounds (a sphere holding every point its
// attenuation reaches) touch, so any light that can reach a fragment is in the list the fragment reads, and the
// shader's bounds test skips the rest: the lists change no result. The console drew each visible light's volume
// into the L-buffer (DL_LightBufferPass) and had no such lists.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Grid {
    lo: Vec2,
    cell: f32,
    dims: UVec2,
}

impl Grid {
    pub(super) fn around(placed: &[Placed]) -> Self {
        let (mut lo, mut hi) = (Vec2::splat(f32::MAX), Vec2::splat(f32::MIN));
        for (_, _, c, r, _) in placed.iter().map(constants) {
            let (a, b) = (Vec2::new(c.x - r, c.z - r), Vec2::new(c.x + r, c.z + r));
            if a.is_finite() && b.is_finite() {
                (lo, hi) = (lo.min(a), hi.max(b));
            }
        }
        if lo.x > hi.x {
            return Self { lo: Vec2::ZERO, cell: CELL, dims: UVec2::ONE };
        }
        let span = hi.as_dvec2() - lo.as_dvec2();
        let cell = (span.max_element() / f64::from(MAX_CELLS)).max(f64::from(CELL));
        let dims = (span / cell).ceil().max(DVec2::ONE).min(DVec2::splat(f64::from(MAX_CELLS))).as_uvec2();
        Self { lo, cell: cell as f32, dims }
    }

    fn cover(&self, min: Vec2, max: Vec2) -> (IVec2, IVec2, bool) {
        let last = self.dims.as_ivec2() - 1;
        if !(min.is_finite() && max.is_finite()) {
            return (IVec2::ZERO, last, true);
        }
        let a = ((min - self.lo) / self.cell).floor().as_ivec2();
        let b = ((max - self.lo) / self.cell).floor().as_ivec2();
        let beyond = a.cmplt(IVec2::ZERO).any() || b.cmpgt(last).any();
        (a.max(IVec2::ZERO), b.min(last), beyond)
    }
}

pub(super) fn encode(placed: &[Placed], grid: &Grid, slots: u32, light: &GameLightUniform) -> (Words, usize) {
    let reach: Vec<[f32; 4]> = placed.iter().map(bounds).collect();
    let dims = grid.dims;
    let outside = (dims.x * dims.y) as usize;
    let mut cells: Vec<Vec<u32>> = vec![Vec::new(); outside + 1];
    for (i, &[x, _, z, r]) in reach.iter().enumerate() {
        if switched_off(&placed[i]) {
            continue;
        }
        let r = if r == f32::MAX { f32::INFINITY } else { r };
        let (a, b, beyond) = grid.cover(Vec2::new(x - r, z - r), Vec2::new(x + r, z + r));
        for z in a.y..=b.y {
            for x in a.x..=b.x {
                cells[(z as u32 * dims.x + x as u32) as usize].push(i as u32);
            }
        }
        if beyond {
            cells[outside].push(i as u32);
        }
    }
    let flat: Vec<u32> = cells.concat();
    let planes = sphere_planes();
    let lights_off = HEADER + planes.len() as u32;
    let cells_off = lights_off + RECORD * placed.len() as u32;
    let list_off = cells_off + cells.len() as u32;
    let list_bounds_off = list_off + flat.len().div_ceil(4) as u32;
    let mut w = Words::default();
    w.u([grid.lo.x.to_bits(), grid.lo.y.to_bits(), grid.cell.to_bits(), placed.len() as u32]);
    w.u([dims.x, dims.y, cells_off, list_off]);
    w.u([lights_off, HEADER, list_bounds_off, 0]);
    w.u([0; 4]);
    for plane in planes {
        w.f(plane);
    }
    for p in placed {
        record(&mut w, p);
    }
    let mut start = 0u32;
    for cell in &cells {
        w.u([start, cell.len() as u32, 0, 0]);
        start += cell.len() as u32;
    }
    for chunk in flat.chunks(4) {
        let mut v = [0u32; 4];
        v[..chunk.len()].copy_from_slice(chunk);
        w.u(v);
    }
    for &i in &flat {
        w.f(reach[i as usize]);
    }
    let dynamic_at = w.0.len();
    w.0.resize(dynamic_at + (RECORD * slots) as usize, [0; 4]);
    w.0[3] = [dynamic_at as u32, 0, 0, w.0.len() as u32];
    w.0.extend(light_words(light));
    w.f(union(&[]));
    w.0.resize(w.0.len() + slots as usize, [0; 4]);
    (w, dynamic_at)
}

pub(super) fn union(bounds: &[[f32; 4]]) -> [f32; 4] {
    if bounds.is_empty() {
        return [0.0, 0.0, 0.0, -1.0];
    }
    if bounds.iter().any(|b| b[3] == f32::MAX) {
        return [0.0, 0.0, 0.0, f32::MAX];
    }
    let spheres: Vec<(DVec3, f64)> =
        bounds.iter().map(|b| (DVec3::new(b[0].into(), b[1].into(), b[2].into()), f64::from(b[3]))).collect();
    let (mut lo, mut hi) = (DVec3::splat(f64::MAX), DVec3::splat(f64::MIN));
    for &(c, r) in &spheres {
        (lo, hi) = (lo.min(c - r), hi.max(c + r));
    }
    let centre = ((lo + hi) * 0.5).as_vec3();
    let reach = spheres.iter().map(|&(c, r)| c.distance(centre.as_dvec3()) + r).fold(0.0, f64::max);
    let radius = (reach * 1.0001 + 1e-4) as f32;
    [centre.x, centre.y, centre.z, if radius.is_finite() { radius } else { f32::MAX }]
}

fn sphere_planes() -> Vec<[f32; 4]> {
    let v = |i: u32, j: u32| {
        let (t, f) =
            (std::f64::consts::PI * i as f64 / STACKS as f64, std::f64::consts::TAU * j as f64 / SLICES as f64);
        bevy::math::DVec3::new(f.cos() * t.sin(), f.sin() * t.sin(), t.cos())
    };
    let mut planes = Vec::new();
    for i in 0..STACKS {
        for j in 0..SLICES {
            let (a, b, c, d) = (v(i, j), v(i + 1, j), v(i, (j + 1) % SLICES), v(i + 1, (j + 1) % SLICES));
            let (p, q, r) = if i == 0 { (a, b, d) } else { (a, b, c) };
            let mut n = (q - p).cross(r - p).normalize();
            if n.dot(a + b + c + d) < 0.0 {
                n = -n;
            }
            planes.push([n.x as f32, n.y as f32, n.z as f32, n.dot(p) as f32]);
        }
    }
    planes
}

#[cfg(test)]
mod tests {
    use bevy::prelude::Vec3A;

    use super::super::{DYNAMIC_SLOTS, LIGHT_WORDS};
    use super::*;

    #[test]
    fn sphere_volume_planes() {
        let planes = sphere_planes();
        assert_eq!(planes.len(), (STACKS * SLICES) as usize);
        let min = planes.iter().map(|p| p[3]).fold(f32::MAX, f32::min);
        assert!((min - 0.940_447).abs() < 1e-5, "{min}");
        assert!((min / sphere_cos() - 0.988_844).abs() < 1e-5);
        for i in 0..=STACKS {
            for j in 0..SLICES {
                let (t, f) = (std::f32::consts::PI * i as f32 / 10.0, std::f32::consts::TAU * j as f32 / 10.0);
                let v = Vec3::new(f.cos() * t.sin(), f.sin() * t.sin(), t.cos());
                assert!(planes.iter().all(|p| Vec3::new(p[0], p[1], p[2]).dot(v) <= p[3] + 1e-5));
            }
        }
    }

    #[test]
    fn buffer_header_without_lights() {
        let (w, dynamic_at) = encode(&[], &Grid::around(&[]), DYNAMIC_SLOTS, &GameLightUniform::default());
        assert_eq!(w.0[0][3], 0);
        assert_eq!(w.0[2][0], HEADER + STACKS * SLICES);
        assert_eq!(dynamic_at as u32, HEADER + STACKS * SLICES + 2);
        let light_at = HEADER + STACKS * SLICES + 2 + RECORD * DYNAMIC_SLOTS;
        assert_eq!(w.0[3], [dynamic_at as u32, 0, 0, light_at]);
        assert_eq!(w.0[2][2], dynamic_at as u32);
        assert_eq!(w.0[light_at as usize + LIGHT_WORDS], [0, 0, 0, (-1.0f32).to_bits()]);
        assert_eq!(w.0.len(), light_at as usize + LIGHT_WORDS + 1 + DYNAMIC_SLOTS as usize);
    }

    fn attenuation(p: &Placed, pl: Vec3) -> f32 {
        let sat = |x: f32| if x.is_nan() { 0.0 } else { x.clamp(0.0, 1.0) };
        let game_pow = |x: f32, y: f32| if x <= 0.0 { 0.0 } else { (y * x.log2()).exp2() };
        let (a1, a2, ..) = constants(p);
        let d = pl.length();
        let falloff = 1.0 - game_pow(sat((d - a1[0]) / (a1[1] - a1[0])), a1[2]);
        match p.light.shape {
            LightShape::Point { .. } => falloff,
            LightShape::Spot { .. } => {
                let c = pl.z / d;
                let u = sat((-c - a2[1]) / (a2[0] - a2[1]));
                falloff * u * u * (3.0 - 2.0 * u) * game_pow(c.abs(), a2[2])
            }
            LightShape::Box { .. } => {
                if pl.z >= 0.0 {
                    return 0.0;
                }
                let sx = sat((pl.x.abs() - (a2[0] - a2[2])) / a2[2]);
                let sy = sat((pl.y.abs() - (a2[1] - a2[2])) / a2[2]);
                let tz = sat((-pl.z - a1[0]) / (a1[1] - a1[0]));
                (1.0 - sx * sx * (3.0 - 2.0 * sx)) * (1.0 - sy * sy * (3.0 - 2.0 * sy)) * (1.0 - game_pow(tz, a1[2]))
            }
        }
    }

    #[test]
    fn bounds_hold_the_attenuation() {
        let spot = |cone_angle, penumbra_angle| LightShape::Spot {
            cone_angle,
            penumbra_angle,
            outer_length: 9.0,
            inner_length: 2.0,
            drop_off: 3.0,
        };
        let shapes = [
            LightShape::Point { outer_radius: 7.0, inner_radius: 1.0 },
            spot(1.2, 0.1),
            spot(0.610_865_2, 0.261_799_4),
            spot(2.0, 0.2),
            spot(0.8, 0.0),
            spot(0.4, -0.5),
            LightShape::Box {
                width: 8.0,
                height: 5.0,
                penumbra: 1.0,
                outer_length: 6.0,
                inner_length: 0.5,
                projected_texture: None,
                unk_fe912176: String::new(),
                unk_ebeca5b9: false,
            },
        ];
        let world = Affine3A::from_cols(
            Vec3A::new(2.0, 0.3, -0.1),
            Vec3A::new(0.9, 0.5, 0.2),
            Vec3A::new(-0.4, 0.2, 1.7),
            Vec3A::new(100.0, -20.0, 35.0),
        );
        let mut seed = 0x2545_f491_u32;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            seed as f32 / u32::MAX as f32 * 2.0 - 1.0
        };
        for shape in shapes {
            let p = placed(world, shape.clone());
            let b = bounds(&p);
            let (centre, radius) = (Vec3::new(b[0], b[1], b[2]), b[3]);
            let inv = world.inverse();
            let (mut reached, mut beyond) = (0, 0);
            for _ in 0..400_000 {
                let q = Vec3::from(world.translation) + Vec3::new(next(), next(), next()) * 20.0;
                let lit = attenuation(&p, inv.transform_point3(q)) > 0.0;
                reached += lit as u32;
                beyond += (lit && q.distance(centre) > radius) as u32;
            }
            assert!(reached > 100, "{shape:?}: {reached}");
            assert_eq!(beyond, 0, "{shape:?}");
        }
        let pole = bounds(&placed(Affine3A::IDENTITY, spot(0.610_865_2, 0.261_799_4)))[3];
        assert!((pole / 9.0 - 0.593).abs() < 0.002, "{pole}");
        let point = |outer_radius, inner_radius| LightShape::Point { outer_radius, inner_radius };
        assert_eq!(bounds(&placed(Affine3A::IDENTITY, point(2.0, 3.0)))[3], f32::MAX);
        let turned = Affine3A::from_scale_rotation_translation(Vec3::splat(3.0), Quat::from_rotation_y(0.7), Vec3::ONE);
        assert!((bounds(&placed(turned, point(2.0, 2.0)))[3] - 6.0).abs() < 2e-3);
    }

    #[test]
    fn the_union_holds_each_sphere() {
        let mut seed = 0x9e37_79b9_u32;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            seed as f32 / u32::MAX as f32
        };
        let inside = |p: Vec3, b: [f32; 4]| p.distance(Vec3::new(b[0], b[1], b[2])) <= b[3];
        for (far, size) in [(0.0, 50.0), (9000.0, 400.0), (20000.0, 0.01)] {
            let spheres: Vec<[f32; 4]> = (0..24)
                .map(|_| [far + next() * size, -far + next() * size, far * 0.5 + next() * size, next() * size * 0.3])
                .collect();
            let u = union(&spheres);
            for b in &spheres {
                for _ in 0..20_000 {
                    let dir = Vec3::new(next() - 0.5, next() - 0.5, next() - 0.5).normalize_or_zero();
                    let p = Vec3::new(b[0], b[1], b[2]) + dir * b[3] * (0.999 + next() * 0.002);
                    assert!(!inside(p, *b) || inside(p, u), "{p} in {b:?}, not in {u:?}");
                }
            }
        }
        assert_eq!(union(&[]), [0.0, 0.0, 0.0, -1.0]);
        assert_eq!(union(&[[1.0, 2.0, 3.0, 4.0], [0.0, 0.0, 0.0, f32::MAX]])[3], f32::MAX);
        let one = union(&[[1.0, 2.0, 3.0, 4.0]]);
        assert_eq!(one[..3], [1.0, 2.0, 3.0]);
        assert!(one[3] >= 4.0 && one[3] < 4.01);
    }

    #[test]
    fn the_size_cull_takes_the_bounds_box() {
        let scaled = Affine3A::from_scale_rotation_translation(Vec3::new(1.0, 2.5, 1.5), Quat::IDENTITY, Vec3::ONE);
        let shapes = [
            (LightShape::Point { outer_radius: 4.0, inner_radius: 1.0 }, 4.0 * 2.5),
            (
                LightShape::Spot {
                    cone_angle: 0.8,
                    penumbra_angle: 0.0,
                    outer_length: 12.0,
                    inner_length: 1.0,
                    drop_off: 1.0,
                },
                12.0 * 2.5,
            ),
            (
                LightShape::Box {
                    width: 6.0,
                    height: 2.0,
                    penumbra: 0.5,
                    outer_length: 3.0,
                    inner_length: 0.0,
                    projected_texture: None,
                    unk_fe912176: String::new(),
                    unk_ebeca5b9: false,
                },
                0.5 * Vec3::new(6.0, 2.0, 3.0).length() * 2.5,
            ),
        ];
        for (shape, r) in shapes {
            let mut w = Words::default();
            record(&mut w, &placed(scaled, shape.clone()));
            let got = f32::from_bits(w.0[6][3]);
            assert!((got - r * 3f32.sqrt()).abs() <= 1e-5 * r, "{shape:?}: {got} for r {r}");
        }
    }

    fn list_at(w: &Words, p: Vec2) -> Vec<u32> {
        list_entries(w, p).into_iter().map(|(i, _)| i).collect()
    }

    fn list_entries(w: &Words, p: Vec2) -> Vec<(u32, [u32; 4])> {
        let (h0, h1) = (w.0[0], w.0[1]);
        let (lo, cell) = (Vec2::new(f32::from_bits(h0[0]), f32::from_bits(h0[1])), f32::from_bits(h0[2]));
        let c = ((p - lo) / cell).floor().as_ivec2();
        let inside = c.cmpge(IVec2::ZERO).all() && c.cmplt(IVec2::new(h1[0] as i32, h1[1] as i32)).all();
        let entry = if inside { c.y as u32 * h1[0] + c.x as u32 } else { h1[0] * h1[1] };
        let [start, len, ..] = w.0[(h1[2] + entry) as usize];
        (start..start + len)
            .map(|at| (w.0[(h1[3] + at / 4) as usize][(at % 4) as usize], w.0[(w.0[2][2] + at) as usize]))
            .collect()
    }

    fn check_lists(w: &Words, placed: &[Placed], points: impl Iterator<Item = Vec2>) -> usize {
        let mut reached = 0;
        for p in points {
            let entries = list_entries(w, p);
            for &(i, b) in &entries {
                assert_eq!(b, bounds(&placed[i as usize]).map(f32::to_bits), "light {i}'s bounds in the list at {p}");
            }
            let list: Vec<u32> = entries.into_iter().map(|(i, _)| i).collect();
            assert!(list.windows(2).all(|i| i[0] < i[1]), "{p}: {list:?}");
            for (i, [x, _, z, r]) in placed.iter().map(bounds).enumerate() {
                let c = Vec2::new(x, z);
                if (p.x - c.x).abs() <= r && (p.y - c.y).abs() <= r {
                    assert!(list.contains(&(i as u32)), "light {i} at {c} (r {r}) misses {p}: {list:?}");
                    reached += 1;
                }
            }
        }
        reached
    }

    fn point(x: f32, z: f32, radius: f32) -> Placed {
        let shape = LightShape::Point { outer_radius: radius, inner_radius: 0.5 * radius };
        placed(Affine3A::from_translation(Vec3::new(x, 10.0, z)), shape)
    }

    #[test]
    fn a_light_carried_away_keeps_the_grid() {
        let mut lights = vec![point(0.0, 0.0, 6.0), point(150.0, 40.0, 12.0), point(-70.0, 300.0, 4.0)];
        let grid = Grid::around(&lights);
        assert_eq!((grid.lo, grid.cell, grid.dims), (Vec2::new(-74.0, -6.0), CELL, UVec2::new(8, 10)));
        let (before, _) = encode(&lights, &grid, DYNAMIC_SLOTS, &GameLightUniform::default());
        let mut seed = 0x9e37_79b9_u32;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            seed as f32 / u32::MAX as f32
        };
        for (i, far) in [(1, Vec3::new(31_000.0, -3.9e6, -27_000.0)), (0, Vec3::new(-180.0, -50.0, 120.0))] {
            lights[i].world.translation = far.into();
            let (w, _) = encode(&lights, &grid, DYNAMIC_SLOTS, &GameLightUniform::default());
            assert_eq!(w.0[..2], before.0[..2], "the grid's header");
            assert!(w.0.len() <= before.0.len() + 1, "{} → {} words", before.0.len(), w.0.len());
            let mut points: Vec<Vec2> =
                (0..4000).map(|_| Vec2::new(next() * 600.0 - 300.0, next() * 600.0 - 200.0)).collect();
            points.extend((0..1000).map(|_| Vec2::new(far.x, far.z) + Vec2::new(next(), next()) * 30.0 - 15.0));
            assert!(check_lists(&w, &lights, points.into_iter()) > 100);
        }
    }

    #[test]
    fn switched_off_lights_are_in_no_list() {
        let mut lights = vec![point(0.0, 0.0, 6.0), point(3.0, 2.0, 6.0), point(-4.0, 1.0, 6.0)];
        lights[1].ctrl = Some((7, [0; 3]));
        lights[2].ctrl = Some((8, [1; 3]));
        let grid = Grid::around(&lights);
        let at = [Vec2::new(0.0, 0.0), Vec2::new(2.0, 1.0), Vec2::new(-3.0, 0.5)];
        for (b1, b2, listed) in [
            (LightBehaviour::Off, LightBehaviour::Off, vec![0]),
            (LightBehaviour::On, LightBehaviour::Off, vec![0, 1]),
            (LightBehaviour::Off, LightBehaviour::Flicker { strength: 0.5 }, vec![0, 2]),
        ] {
            (lights[1].behaviour, lights[2].behaviour) = (Some(b1), Some(b2));
            let (w, _) = encode(&lights, &grid, DYNAMIC_SLOTS, &GameLightUniform::default());
            for p in at {
                assert_eq!(list_at(&w, p), listed, "{b1:?} / {b2:?} at {p}");
            }
            assert_eq!(w.0[0][3], 3);
        }
    }

    #[test]
    fn wide_and_unbounded_lights() {
        let mut lights = vec![point(-2.0e6, 0.0, 5.0), point(3.0e6, 1.0e5, 5.0), point(0.0, 0.0, 5.0)];
        lights.push(point(10.0, 10.0, f32::INFINITY));
        let grid = Grid::around(&lights);
        assert_eq!(grid.dims.x, MAX_CELLS);
        assert!(grid.dims.y < MAX_CELLS && grid.cell > CELL, "{grid:?}");
        let (w, _) = encode(&lights, &grid, DYNAMIC_SLOTS, &GameLightUniform::default());
        let points = [(-2.0e6, 0.0), (3.0e6, 1.0e5), (1.0, 2.0), (-1.0e9, 4.0e8), (2.5e6, -3.0e7)];
        let points = points.map(|(x, z)| Vec2::new(x, z));
        assert!(check_lists(&w, &lights, points.iter().copied()) >= points.len() + 3);
        assert!(points.iter().all(|&p| list_at(&w, p).contains(&3)));
    }

    fn placed(world: Affine3A, shape: LightShape) -> Placed {
        let light = ssx_assets::track_lights::SceneLight {
            name: String::new(),
            debug_name: String::new(),
            color: [1.0; 4],
            diffuse_intensity: 1.0,
            specular_intensity: 0.0,
            fall_off: 1.0,
            enable_specular: false,
            enable: true,
            shape,
        };
        Placed {
            world,
            light,
            factor: None,
            ctrl: None,
            node: Affine3A::IDENTITY,
            scale: Vec3::ONE,
            order: (0, 0, 0, 0, 0),
            behaviour: None,
            entity: 0,
            texture: None,
            texture_layer: 0,
        }
    }
}
