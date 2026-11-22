use bevy::camera::visibility::RenderLayers;
use bevy::light::cascade::Cascade;
use bevy::light::{CascadeShadowConfig, Cascades, DirectionalLightShadowMap, SimulationLightSystems};
use bevy::mesh::skinning::SkinnedMesh;
use bevy::pbr::MeshPipelineKey;
use bevy::prelude::*;
use bevy::render::render_resource::{Face, RenderPipelineDescriptor};
use bevy::shader::ShaderDefVal;
use ssx_assets::port_options::ShadowQuality;
use ssx_formats::fxo::state;

use crate::camera::GameProjection;
use crate::render::fx_states;
use crate::rider::visual::RiderRoot;

const SPLIT_LAMBDA: f32 = 0.65;
pub const CASCADES: usize = 3;
const DEPTH_RANGE: f32 = 1000.0;

pub(crate) fn light_view(doff: Vec3) -> Mat3 {
    let r2 = doff.normalize();
    let r0 = Vec3::Y.cross(r2).normalize();
    let r1 = r2.cross(r0);
    Mat3::from_cols(r0, r1, r2).transpose()
}

fn cascades_of(doff: Vec3, boxes: &[(Vec3, Vec3, f32)]) -> Vec<Cascade> {
    let world_from_light = Mat4::from_mat3(light_view(doff).transpose());
    boxes.iter().map(|&(min, max, t)| cascade(world_from_light, ortho(min, max, 1.0), t)).collect()
}

pub fn game_cascade_boxes(
    doff: Vec3,
    camera: &GlobalTransform,
    (near, far): (f32, f32),
    reach: f32,
    map_size: u32,
) -> [(Vec3, Vec3, f32); CASCADES] {
    let lv = light_view(doff);
    let axis = |z: f32| lv * camera.transform_point(Vec3::new(0.0, 0.0, z));
    let (n, a) = (-near, -far);
    let f = n - reach;
    let split = |p: f32| {
        let c = SPLIT_LAMBDA * n * (f / n).powf(p) + (1.0 - SPLIT_LAMBDA) * (n + (f - n) * p);
        (c - n) / (f - n)
    };
    let s = [0.0, split(1.0 / 3.0), split(2.0 / 3.0), 1.0];
    let ratio = reach / (a - n).abs();
    let zf = a * ratio;
    let zr = axis((zf + n) / 2.0).z - (zf - n).abs() / 2.0;
    std::array::from_fn(|i| {
        let (zf_i, zn_i) = (a * s[i + 1] * ratio, if i == 0 { n } else { 0.0 });
        let span = (zf_i - zn_i).abs();
        let r = span / 2.0;
        let c = axis((zf_i + zn_i) / 2.0);
        let t = span / map_size as f32;
        let snap = |v: Vec3| (v / t).floor() * t;
        let min = snap(Vec3::new(c.x - r, c.y - r, -zr - DEPTH_RANGE));
        let max = snap(Vec3::new(c.x + r, c.y + r, -zr));
        (min, max, t)
    })
}

fn ortho(min: Vec3, max: Vec3, viewport: f32) -> Mat4 {
    let (w, h, dz, s) = (max.x - min.x, max.y - min.y, min.z - max.z, viewport);
    Mat4::from_cols(
        Vec4::new(2.0 * s / w, 0.0, 0.0, 0.0),
        Vec4::new(0.0, 2.0 * s / h, 0.0, 0.0),
        Vec4::new(0.0, 0.0, -1.0 / dz, 0.0),
        Vec4::new(-s * (min.x + max.x) / w + s - 1.0, -s * (min.y + max.y) / h + 1.0 - s, 1.0 - min.z / dz, 1.0),
    )
}

fn cascade(world_from_light: Mat4, clip_from_light: Mat4, texel_size: f32) -> Cascade {
    Cascade {
        world_from_cascade: world_from_light,
        clip_from_cascade: clip_from_light,
        clip_from_world: clip_from_light * world_from_light.transpose(),
        texel_size,
    }
}

pub const CHARACTER_MAP_SIZE: usize = 128;
const CHARACTER_MARGIN: f32 = 1.5;
const CHARACTER_DEPTH: f32 = 500.0;
pub const CHARACTER_LAYER: usize = 1;

pub fn character_cascade(doff: Vec3, min: Vec3, max: Vec3, map_size: u32) -> Cascade {
    let lv = light_view(doff);
    let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
    for i in 0..8 {
        let corner = Vec3::select(BVec3::new(i & 1 != 0, i & 2 != 0, i & 4 != 0), max, min);
        lo = lo.min(lv * corner);
        hi = hi.max(lv * corner);
    }
    let (centre, half) = ((lo + hi) * 0.5, (hi - lo).abs() * 0.5 * CHARACTER_MARGIN);
    let (lo, hi) = (centre - half, centre + half);
    let top = hi.z;
    let min = Vec3::new(lo.x, lo.y, -top);
    let max = Vec3::new(hi.x, hi.y, -top + CHARACTER_DEPTH);
    let viewport = CHARACTER_MAP_SIZE as f32 / map_size as f32;
    cascade(Mat4::from_mat3(lv.transpose()), ortho(min, max, viewport), (max.x - min.x) / CHARACTER_MAP_SIZE as f32)
}

pub const CONTACT_MAP_SIZE: usize = 128;
const CONTACT_DEPTH: f32 = 0.55;
const CONTACT_MARGIN: f32 = 1.5;
pub const CONTACT_LIGHT_MARK: f32 = 1.0;

fn ground_frame(n: Vec3) -> (Vec3, Vec3, Vec3) {
    let n = n.normalize();
    let reference = if n.z >= n.x { Vec3::X } else { Vec3::Z };
    let z = reference.cross(n).normalize();
    (n.cross(z), n, z)
}

pub fn contact_cascade(p: Vec3, n: Vec3, depth: f32, (min, max): (Vec3, Vec3), map_size: u32) -> Cascade {
    let (x, n, z) = ground_frame(n);
    let rotation = Mat4::from_mat3(Mat3::from_cols(x, z, -n).transpose());
    let light_from_world = Mat4::from_translation(Vec3::new(0.0, 0.0, depth)) * rotation * Mat4::from_translation(-p);
    let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN_POSITIVE));
    for i in 0..8 {
        let corner = Vec3::select(BVec3::new(i & 1 != 0, i & 2 != 0, i & 4 != 0), max, min);
        let c = light_from_world.transform_point3(corner);
        lo = lo.min(c);
        hi = hi.max(c);
    }
    let half = (hi - lo).abs() * 0.5 * CONTACT_MARGIN;
    let viewport = CONTACT_MAP_SIZE as f32 / map_size as f32;
    let clip_from_light = ortho(Vec3::new(-half.x, -half.y, 0.0), Vec3::new(half.x, half.y, CONTACT_DEPTH), viewport);
    Cascade {
        world_from_cascade: light_from_world.inverse(),
        clip_from_cascade: clip_from_light,
        clip_from_world: clip_from_light * light_from_world,
        texel_size: 2.0 * half.x / CONTACT_MAP_SIZE as f32,
    }
}

pub fn sun_components(travel: Vec3, illuminance: f32) -> impl Bundle {
    (
        DirectionalLight {
            illuminance,
            shadow_maps_enabled: true,
            shadow_depth_bias: 0.0,
            shadow_normal_bias: 0.0,
            ..default()
        },
        Transform::default().looking_to(travel, Vec3::Y),
        CascadeShadowConfig {
            bounds: vec![ShadowQuality::GAME_REACH; CASCADES],
            overlap_proportion: 0.0,
            minimum_distance: 0.5,
        },
        GameSun { doff: -travel },
        CascadeFit::default(),
    )
}

#[derive(Component, Clone, Copy, Debug, Default)]
pub struct CascadeFit {
    pub camera: Option<Entity>,
    pub light: Mat3,
    pub boxes: [(Vec3, Vec3); CASCADES],
}

#[derive(Component, Clone, Copy, Debug)]
pub struct GameSun {
    pub doff: Vec3,
}

pub fn character_light_components(travel: Vec3) -> impl Bundle {
    (
        DirectionalLight {
            illuminance: 0.0,
            shadow_maps_enabled: false,
            shadow_depth_bias: 0.0,
            shadow_normal_bias: 0.0,
            ..default()
        },
        Transform::default().looking_to(travel, Vec3::Y),
        CascadeShadowConfig { bounds: vec![CHARACTER_DEPTH], overlap_proportion: 0.0, minimum_distance: 0.5 },
        RenderLayers::layer(CHARACTER_LAYER),
        GameCharacterShadow { doff: -travel },
    )
}

#[derive(Component, Clone, Copy, Debug)]
pub struct GameCharacterShadow {
    pub doff: Vec3,
}

fn contact_light_components() -> impl Bundle {
    (
        Name::new("contact shadow light"),
        DirectionalLight {
            illuminance: 0.0,
            shadow_maps_enabled: false,
            shadow_depth_bias: CONTACT_LIGHT_MARK,
            shadow_normal_bias: 0.0,
            ..default()
        },
        Transform::default().looking_to(Vec3::NEG_Y, Vec3::Z),
        CascadeShadowConfig { bounds: vec![CHARACTER_DEPTH], overlap_proportion: 0.0, minimum_distance: 0.5 },
        RenderLayers::layer(CHARACTER_LAYER),
        GameContactShadow,
    )
}

#[derive(Component, Clone, Copy, Debug)]
pub struct GameContactShadow;

#[derive(Resource, Default)]
struct CharacterBoardJoint(Option<Entity>);

#[derive(Resource, Default)]
struct CharacterAabb(Option<(Vec3, Vec3)>);

#[derive(Resource, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ShadowSettings(pub ShadowQuality);

pub struct GameShadowPlugin;

impl Plugin for GameShadowPlugin {
    fn build(&self, app: &mut App) {
        let quality = app.world().get_resource::<ShadowSettings>().copied().unwrap_or_default();
        app.insert_resource(quality)
            .insert_resource(DirectionalLightShadowMap { size: quality.0.map_size() as usize })
            .add_systems(Update, follow_shadow_settings.run_if(resource_changed::<ShadowSettings>))
            .init_resource::<CharacterBoardJoint>()
            .init_resource::<CharacterAabb>()
            .init_resource::<DepthBelowSnow>()
            .add_systems(crate::shell::level::LevelStartup, |mut commands: Commands| {
                commands.spawn(contact_light_components());
            })
            .add_systems(
                PostUpdate,
                (tag_character_casters, fit_cascades, fit_character, fit_contact)
                    .chain()
                    .after(SimulationLightSystems::UpdateDirectionalLightCascades)
                    .before(SimulationLightSystems::UpdateLightFrusta),
            );
        use crate::render::lighting::fade::{LightingFadeSet, faded};
        app.add_systems(Update, follow_lighting.after(LightingFadeSet).run_if(faded));
    }
}

fn follow_shadow_settings(settings: Res<ShadowSettings>, mut map: ResMut<DirectionalLightShadowMap>) {
    let size = settings.0.map_size() as usize;
    if map.size != size {
        map.size = size;
        info!("shadows: {} ({size}² maps, {} m)", settings.0, settings.0.reach());
    }
}

fn follow_lighting(
    lighting: Res<crate::render::lighting::GameLighting>,
    mut suns: Query<(&mut GameSun, &mut Transform), Without<GameCharacterShadow>>,
    mut characters: Query<(&mut GameCharacterShadow, &mut Transform), Without<GameSun>>,
) {
    let doff = lighting.sun_offset;
    for (mut sun, mut transform) in &mut suns {
        sun.doff = doff;
        *transform = Transform::default().looking_to(-doff, Vec3::Y);
    }
    for (mut light, mut transform) in &mut characters {
        light.doff = doff;
        *transform = Transform::default().looking_to(-doff, Vec3::Y);
    }
}

pub fn reset_level(world: &mut World) {
    world.insert_resource(CharacterBoardJoint::default());
    world.insert_resource(CharacterAabb::default());
    world.insert_resource(DepthBelowSnow::default());
}

fn tag_character_casters(
    mut commands: Commands,
    meshes: Query<Entity, (Added<Mesh3d>, Without<RenderLayers>)>,
    cameras: Query<Entity, (Added<Camera3d>, Without<RenderLayers>)>,
    names: Query<(Entity, &Name), Added<Name>>,
    parents: Query<&ChildOf>,
    roots: Query<(), With<RiderRoot>>,
    mut board: ResMut<CharacterBoardJoint>,
) {
    let layers = || RenderLayers::from_layers(&[0, CHARACTER_LAYER]);
    let in_rider = |e: Entity| parents.iter_ancestors(e).any(|a| roots.contains(a));
    for e in meshes.iter().filter(|&e| in_rider(e)) {
        commands.entity(e).insert(RenderLayers::layer(CHARACTER_LAYER));
    }
    for e in &cameras {
        commands.entity(e).insert(layers());
    }
    if let Some((e, _)) = names.iter().find(|(e, n)| n.as_str() == "Board" && in_rider(*e)) {
        board.0 = Some(e);
    }
}

#[allow(clippy::too_many_arguments)]
fn fit_character(
    cameras: Query<(Entity, &Camera), With<Projection>>,
    mut lights: Query<(&GameCharacterShadow, &mut Cascades, &mut DirectionalLight)>,
    parts: Query<(Entity, &SkinnedMesh, &InheritedVisibility)>,
    transforms: Query<&GlobalTransform>,
    parents: Query<&ChildOf>,
    roots: Query<(), With<RiderRoot>>,
    (board, map): (Res<CharacterBoardJoint>, Res<DirectionalLightShadowMap>),
    mut aabb: ResMut<CharacterAabb>,
) {
    let (mut min, mut max) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
    let mut skinned = false;
    let in_rider = |e: Entity| parents.iter_ancestors(e).any(|a| roots.contains(a));
    for (_, mesh, visibility) in parts.iter().filter(|(e, ..)| in_rider(*e)) {
        if !visibility.get() {
            continue;
        }
        skinned = true;
        for t in mesh.joints.iter().chain(board.0.iter()).filter_map(|&j| transforms.get(j).ok()) {
            min = min.min(t.translation());
            max = max.max(t.translation());
        }
    }
    aabb.0 = (skinned && min.x <= max.x).then_some((min, max));
    let enabled = std::env::var("SSX_CHARACTER_SHADOW").map_or(true, |v| v != "off");
    let active = enabled && aabb.0.is_some();
    for (light, mut cascades, mut directional) in &mut lights {
        if directional.shadow_maps_enabled != active {
            directional.shadow_maps_enabled = active;
        }
        if !active {
            continue;
        }
        let cascade = character_cascade(light.doff, min, max, map.size as u32);
        for (entity, camera) in &cameras {
            if camera.is_active {
                cascades.cascades.insert(entity, vec![cascade.clone()]);
            }
        }
    }
}

fn contact_of(ride: &crate::ride::Ride, depth: f32) -> Option<(Vec3, Vec3, f32)> {
    use ssx_physics::surface::SNOW_COVERAGE;
    let s = &ride.state;
    let g = s.contact.as_ref()?;
    if ride.rewind_backwards || s.weights[SNOW_COVERAGE] <= 0.0 {
        return None;
    }
    let shift = ride.render.board - s.position;
    Some((g.contact + shift, g.normal, depth))
}

#[derive(Resource, Default)]
struct DepthBelowSnow(f32);

fn fit_contact(
    ride: Option<Res<crate::ride::Ride>>,
    aabb: Res<CharacterAabb>,
    mut depth: ResMut<DepthBelowSnow>,
    cameras: Query<(Entity, &Camera), With<Projection>>,
    mut lights: Query<(&mut Cascades, &mut DirectionalLight), With<GameContactShadow>>,
    map: Res<DirectionalLightShadowMap>,
) {
    if let Some(s) = ride.as_deref().map(|r| &r.state).filter(|s| s.on_ground) {
        let h = -s.contact.as_ref().map_or(0.0, |g| g.distance);
        depth.0 = s.surface.snow_depth + h - s.carving.riding_height;
    }
    let enabled = std::env::var("SSX_CONTACT_SHADOW").map_or(true, |v| v != "off");
    let contact = ride.as_deref().and_then(|r| contact_of(r, depth.0)).filter(|_| enabled);
    let fit = contact.zip(aabb.0);
    for (mut cascades, mut directional) in &mut lights {
        if directional.shadow_maps_enabled != fit.is_some() {
            directional.shadow_maps_enabled = fit.is_some();
        }
        let Some(((p, n, depth), (min, max))) = fit else { continue };
        let cascade = contact_cascade(p, n, depth, (min, max), map.size as u32);
        for (entity, camera) in &cameras {
            if camera.is_active {
                cascades.cascades.insert(entity, vec![cascade.clone()]);
            }
        }
    }
}

pub(crate) fn fit_cascades(
    cameras: Query<(Entity, &GlobalTransform, &Projection, &Camera)>,
    mut suns: Query<(&GameSun, &mut Cascades, Option<&mut CascadeFit>)>,
    settings: Res<ShadowSettings>,
    map: Res<DirectionalLightShadowMap>,
) {
    for (sun, mut cascades, mut fit) in &mut suns {
        for (entity, transform, projection, camera) in &cameras {
            if !camera.is_active {
                continue;
            }
            let (near, far) = match projection {
                Projection::Custom(c) => match c.get::<GameProjection>() {
                    Some(p) => (p.near, p.far),
                    None => continue,
                },
                Projection::Perspective(p) => (p.near, p.far),
                Projection::Orthographic(_) => continue,
            };
            let boxes = game_cascade_boxes(sun.doff, transform, (near, far), settings.0.reach(), map.size as u32);
            cascades.cascades.insert(entity, cascades_of(sun.doff, &boxes));
            if let Some(fit) = fit.as_deref_mut() {
                *fit = CascadeFit {
                    camera: Some(entity),
                    light: light_view(sun.doff),
                    boxes: boxes.map(|(min, max, _)| (min, max)),
                };
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Reflect)]
pub struct ShadowPassKey {
    cull: u8,
}

impl ShadowPassKey {
    pub fn new(cull: Option<Face>) -> Self {
        ShadowPassKey { cull: cull.map_or(0, |f| if f == Face::Back { 1 } else { 2 }) }
    }

    pub fn of(effect: &str, technique: &str) -> Self {
        match fx_states::cull(effect, technique) {
            Ok(cull) => Self::new(cull),
            Err(e) => {
                warn!("{effect} shadow pass: {e:#}; back faces culled");
                Self::new(Some(Face::Back))
            }
        }
    }

    fn face(self) -> Option<Face> {
        match self.cull {
            0 => None,
            1 => Some(Face::Back),
            _ => Some(Face::Front),
        }
    }
}

pub fn specialize_shadow_pass(
    descriptor: &mut RenderPipelineDescriptor,
    mesh_key: MeshPipelineKey,
    key: ShadowPassKey,
) {
    if crate::render::gbuffer::is_depth_prepass(descriptor, mesh_key) {
        crate::render::gbuffer::depth_prepass_writes(descriptor, false);
        return;
    }
    if !mesh_key.contains(MeshPipelineKey::UNCLIPPED_DEPTH_ORTHO) {
        return;
    }
    descriptor.primitive.cull_mode = key.face();
    descriptor.primitive.unclipped_depth = false;
    let emulation = |defs: &mut Vec<ShaderDefVal>| {
        defs.retain(|d| {
            let (ShaderDefVal::Bool(name, _) | ShaderDefVal::Int(name, _) | ShaderDefVal::UInt(name, _)) = d;
            name != "UNCLIPPED_DEPTH_ORTHO_EMULATION" && name != "PREPASS_FRAGMENT"
        })
    };
    emulation(&mut descriptor.vertex.shader_defs);
    if let Some(fragment) = &mut descriptor.fragment {
        emulation(&mut fragment.shader_defs);
    }
}

pub fn shadow_alpha(effect: &str, technique: &str, glow_enable: f32) -> Vec4 {
    let get = |s| fx_states::render_state(effect, technique, s);
    let (Ok(test), Ok(reference), Ok(func)) =
        (get(state::ALPHATESTENABLE), get(state::ALPHAREF), get(state::ALPHAFUNC))
    else {
        return Vec4::new(-1.0, 0.0, 0.0, 0.0);
    };
    if test == 0 || func != 4 {
        return Vec4::new(-1.0, 0.0, 0.0, 0.0);
    }
    let reads_glow = fx_states::pixel_shader_reads(effect, technique, "glowEnable").unwrap_or(false);
    Vec4::new(reference as f32 / 255.0, if reads_glow { glow_enable } else { 0.0 }, 0.0, 0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pick(cascades: &[Cascade], p: Vec3) -> usize {
        let edge = 0.5 - 2.0 / 1024.0;
        cascades
            .iter()
            .position(|c| {
                let clip = c.clip_from_world * p.extend(1.0);
                let ci = Vec3::new(0.5 * clip.x, -0.5 * clip.y, 0.5 - clip.z);
                ci.x.abs() < edge && ci.y.abs() < edge && ci.z.abs() < 0.5
            })
            .unwrap_or(3)
    }

    #[test]
    fn cascades_cover_the_view_axis() {
        let camera = GlobalTransform::from(
            Transform::from_xyz(0.0, 1000.0, 0.0).looking_to(Vec3::new(0.0, -0.2, -1.0), Vec3::Y),
        );
        let doff = Vec3::new(-0.6, 0.5, 0.6).normalize();
        let cascades = cascades_of(doff, &game_cascade_boxes(doff, &camera, (0.5, 8000.0), 400.0, 1024));
        let forward = camera.forward().as_vec3();
        let at = |d: f32| pick(&cascades, camera.translation() + forward * d);
        assert_eq!((at(5.0), at(30.0)), (0, 0));
        assert_eq!(at(80.0), 1);
        assert_eq!((at(200.0), at(350.0)), (2, 2));
        assert_eq!(at(600.0), 3);
        let p = camera.translation() + forward * 20.0;
        let z = |q: Vec3| 1.0 - (cascades[0].clip_from_world * q.extend(1.0)).z;
        assert!(z(p - doff * 10.0) > z(p));
    }

    #[test]
    fn character_box_fills_its_viewport() {
        let doff = Vec3::new(0.3, 0.8, 0.2).normalize();
        let (min, max) = (Vec3::new(10.0, 100.0, -5.0), Vec3::new(11.0, 101.8, -4.4));
        let c = character_cascade(doff, min, max, 1024);
        let clip = |p: Vec3| (c.clip_from_world * p.extend(1.0)).truncate();
        let edge = -1.0 + 2.0 * CHARACTER_MAP_SIZE as f32 / 1024.0;
        for i in 0..8 {
            let corner = Vec3::select(BVec3::new(i & 1 != 0, i & 2 != 0, i & 4 != 0), max, min);
            let q = clip(corner);
            assert!(q.x > -1.0 && q.x < edge && q.y < 1.0 && q.y > -edge && q.z > 0.0 && q.z < 1.0, "{q}");
        }
        let centre = (min + max) * 0.5;
        assert!(clip(centre - doff).z < clip(centre).z);
        assert!(clip(centre - doff * 600.0).z < 0.0);
    }

    #[test]
    fn ground_frame_matches_the_game() {
        let (x, n, z) = ground_frame(Vec3::new(0.3, 0.9, 0.1));
        assert!(x.abs_diff_eq(Vec3::new(-0.0331, -0.0994, 0.9945), 2e-4), "{x}");
        assert!(n.abs_diff_eq(Vec3::new(0.3145, 0.9435, 0.1048), 2e-4), "{n}");
        assert!(z.abs_diff_eq(Vec3::new(-0.9487, 0.3162, 0.0), 2e-4), "{z}");
        let (x, _, z) = ground_frame(Vec3::new(0.0, 0.8, 0.6));
        assert!(x.abs_diff_eq(Vec3::X, 1e-6) && z.abs_diff_eq(Vec3::new(0.0, -0.6, 0.8), 1e-6));
    }

    #[test]
    fn contact_box_matches_the_emulated_fit() {
        let c = contact_cascade(Vec3::ZERO, Vec3::Y, 0.0, (Vec3::new(-0.5, 0.0, -0.3), Vec3::new(0.5, 1.8, 0.3)), 1024);
        let q = (c.clip_from_world * Vec4::new(0.3, 0.2, -0.1, 1.0)).truncate();
        let s = CONTACT_MAP_SIZE as f32 / 1024.0;
        let (xc, yc, zn) = (0.3 * 4.0 / 3.0, -0.1 / 0.45, 0.2 / 0.55);
        let expected = Vec3::new(s * xc + s - 1.0, s * yc + 1.0 - s, 1.0 - zn);
        assert!(q.abs_diff_eq(expected, 1e-5), "{q} vs {expected}");
        let (p, n, d) = (Vec3::new(10.0, 20.0, 30.0), Vec3::new(0.2, 0.9, -0.1).normalize(), 0.25);
        let c = contact_cascade(p, n, d, (p - Vec3::ONE, p + Vec3::ONE), 1024);
        let z = |h: f32| (c.clip_from_world * (p + n * (d + h)).extend(1.0)).z;
        assert!((z(0.0) - 1.0).abs() < 1e-5 && z(CONTACT_DEPTH).abs() < 1e-5);
        let recomposed = c.clip_from_cascade * c.world_from_cascade.inverse();
        assert!(recomposed.abs_diff_eq(c.clip_from_world, 1e-4));
    }
}
