use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result};
use bevy::asset::embedded_asset;
use bevy::camera::visibility::DynamicSkinnedMeshBounds;
use bevy::mesh::MeshVertexBufferLayoutRef;
use bevy::mesh::skinning::{SkinnedMesh, SkinnedMeshInverseBindposes};
use bevy::pbr::{ExtendedMaterial, MaterialExtension, MaterialExtensionKey, MaterialExtensionPipeline, MaterialPlugin};
use bevy::prelude::{
    AlphaMode, App, Asset, Assets, ChildOf, Commands, Entity, FixedUpdate, Handle, Image, IntoScheduleConfigs, Mat4,
    Mesh, Mesh3d, MeshMaterial3d, Name, Plugin, PostUpdate, Quat, Query, Reflect, Res, ResMut, Resource,
    StandardMaterial, Transform, Update, Vec3, Vec4, Visibility, With, World, info, resource_exists, warn,
};
use bevy::render::render_resource::{
    AsBindGroup, BlendComponent, BlendFactor, BlendOperation, BlendState, RenderPipelineDescriptor, ShaderType,
    SpecializedMeshPipelineError,
};
use bevy::render::storage::ShaderBuffer;
use bevy::shader::ShaderRef;
use ssx_assets::helicopter::{HeliAnimator, HeliModel, HeliPose};
use ssx_assets::particles::ParticleNode;
use ssx_assets::track_lights::SceneLight;
use ssx_formats::crsf::SceneGraph;
use ssx_physics::helicopter::downwash::Downwash;
use ssx_physics::nis::IntroAssets;

use crate::render::lighting::game::{GameLightAssets, GameLightUniform};
use crate::render::objects::foreground::ForegroundMaterial;
use crate::render::objects::material::{LDR_FRAME_FORMAT, LDR_PIPELINE_LABEL, LDR_TONEMAP_BUFFER, ObjectMaterial};
use crate::render::objects::{LoadedPart, MaterialHandle, part_from_mesh};
use crate::rider::anim::{JointRecord, interpolate_records, world_records};

pub const HELI_LAYER: usize = 2;

type HeliPartLoad = (LoadedPart, Vec<usize>, Vec<Mat4>, String, Option<(f32, f32)>);

struct HeliLight {
    group: String,
    joint: usize,
    local: JointRecord,
    light: SceneLight,
}

pub struct LoadedHeli {
    model: HeliModel,
    animator: HeliAnimator,
    parts: Vec<HeliPartLoad>,
    lights: Vec<HeliLight>,
    particles: Vec<ParticleNode>,
    wash: Option<(usize, usize)>,
}

#[derive(Resource)]
struct PendingHeli(Option<LoadedHeli>);

#[derive(Resource)]
pub struct HeliVisual {
    model: HeliModel,
    animator: HeliAnimator,
    root: Entity,
    nodes: Vec<Entity>,
    seen_steps: u64,
    handover: Option<HeliPose>,
    steps: (Vec<JointRecord>, Vec<JointRecord>),
    pub drawn_floor: Option<Vec3>,
    lights: Vec<HeliLight>,
    pub particle_nodes: Vec<ParticleNode>,
    wash: Option<(usize, usize)>,
    pub newest: Vec<JointRecord>,
    drawn: Vec<JointRecord>,
    pub downwash: Option<Downwash>,
}

impl HeliVisual {
    pub fn rig(&self) -> &SceneGraph {
        &self.model.rig
    }
}

fn load(root: &Path, track: Option<&str>, intro: &IntroAssets) -> Result<LoadedHeli> {
    let choice = intro.heli.as_ref().map_err(|e| anyhow::anyhow!("{e}"))?;
    // TODO(guess): LOD 0 at every distance (`LODThresholds` 1.6, 1.15, 0.02 not applied); settle: run the helicopter through `objects::update_object_lods` with its LOD 1 / 2 meshes.
    let model = ssx_assets::helicopter::load_model_in(root, track, choice.gameobject)?;
    let animator = HeliAnimator::load(root, &choice.name)?;
    let mut parts = Vec::new();
    for p in &model.parts {
        let Some(skin) = &p.skin else { continue };
        let material = ssx_assets::content::GameContent::of(root)
            .material_for(&p.material, ssx_assets::content::resolve::MaterialUse::Riders);
        let part = part_from_mesh(root, p.mesh.clone(), &p.mesh_name, &material)
            .and_then(|part| part.with_skin(skin.joints.clone(), skin.weights.clone()))
            .with_context(|| p.mesh_name.clone())?;
        let nodes = skin.palette.iter().map(|j| j.node).collect();
        let inv = skin.palette.iter().map(|j| Mat4::from_cols_array_2d(&j.inverse_bind)).collect();
        let m = &p.material;
        let translucent = (m.technique.as_deref() == Some("translucent"))
            .then(|| (m.float("glowEnable").unwrap_or(0.0), m.float("glowIntensity").unwrap_or(0.0)));
        parts.push((part, nodes, inv, p.mesh_name.clone(), translucent));
    }
    let (particles, light_nodes) = match ssx_assets::vault_db::with_level_db(root, track, |db| {
        ssx_assets::track_particles::game_object_nodes(db, choice.gameobject)
    }) {
        Ok((p, l)) => (p.lods.into_iter().next().unwrap_or_default(), l.into_iter().next().unwrap_or_default()),
        Err(e) => {
            warn!("helicopter effects unavailable: {e:#}");
            Default::default()
        }
    };
    let mut lights = Vec::new();
    for n in light_nodes {
        let (Some(light), Some(joint)) = (n.light.clone(), model.rig.find(&n.joint_name)) else {
            warn!("helicopter light node on {:?}: no light or no such joint", n.joint_name);
            continue;
        };
        let [x, y, z, w] = n.orientation;
        let local = JointRecord {
            s: Vec3::new(n.scale[0], n.scale[1], n.scale[2]),
            q: Quat::from_xyzw(x, y, z, w),
            p: Vec3::new(n.position[0], n.position[1], n.position[2]),
        };
        lights.push(HeliLight { group: n.group_name, joint, local, light });
    }
    let wash = match ssx_physics::helicopter::downwash::wash_joints(root) {
        Ok((rotor, wash)) => model.rig.find(&rotor).zip(model.rig.find(&wash)),
        Err(e) => {
            warn!("helicopter downwash query unavailable: {e:#}");
            None
        }
    };
    Ok(LoadedHeli { model, animator, parts, lights, particles, wash })
}

pub type Loaded = LoadedHeli;

pub fn register(app: &mut App) {
    let (ride, heli) = (resource_exists::<crate::ride::Ride>, resource_exists::<HeliVisual>);
    app.add_plugins(HeliTranslucentPlugin)
        .add_systems(crate::shell::level::LevelStartup, spawn.run_if(resource_exists::<PendingHeli>))
        .add_systems(FixedUpdate, snapshot.after(crate::ride::RideStepSet).run_if(ride).run_if(heli))
        .add_systems(Update, snapshot.after(crate::ride::RideStepSet).run_if(ride).run_if(heli))
        .add_systems(
            PostUpdate,
            update
                .before(bevy::transform::TransformSystems::Propagate)
                .before(crate::render::lighting::local::DynamicLightsUpload),
        )
        .add_systems(
            PostUpdate,
            follow_heli
                .after(update)
                .before(bevy::transform::TransformSystems::Propagate)
                .run_if(resource_exists::<HeliCam>),
        );
    crate::shell::level::scoped::<PendingHeli>(app);
    crate::shell::level::scoped::<HeliCam>(app);
    crate::shell::level::scoped::<HeliVisual>(app);
}

pub fn insert(world: &mut World, loaded: Loaded) {
    if let Some(mut reserve) = world.get_resource_mut::<crate::render::lighting::local::DynamicLightReserve>() {
        reserve.0 += loaded.lights.len() as u32;
    }
    world.insert_resource(PendingHeli(Some(loaded)));
    let offset = std::env::var("SSX_HELI_CAM").ok().and_then(|v| {
        let p: Vec<f32> = v.split(',').filter_map(|x| x.trim().parse().ok()).collect();
        (p.len() == 3).then(|| Vec3::new(p[0], p[1], p[2]))
    });
    if let Some(offset) = offset {
        world.insert_resource(HeliCam(offset));
    }
}

pub fn load_level(big_root: Option<&Path>, track: Option<&str>, intro: Option<Arc<IntroAssets>>) -> Option<Loaded> {
    if crate::render::post::env_is("SSX_HELI", "off") {
        info!("helicopter: off (SSX_HELI=off)");
        return None;
    }
    let (Some(root), Some(intro)) = (big_root, intro) else { return None };
    let loaded = match load(root, track, &intro) {
        Ok(l) => l,
        Err(e) => {
            warn!("helicopter unavailable: {e:#}");
            return None;
        }
    };
    if let Ok(c) = &intro.heli {
        info!(
            "helicopter: {} \"{}\" ({:?}; game object {:08x}): {} parts, rig {} nodes | {}",
            c.name,
            c.display,
            c.source,
            c.gameobject,
            loaded.parts.len(),
            loaded.model.rig.nodes.len(),
            loaded.animator.problems.join("; ")
        );
        info!(
            "helicopter effects: {} light nodes, {} particle nodes, downwash query {}",
            loaded.lights.len(),
            loaded.particles.len(),
            if loaded.wash.is_some() { "on its joints" } else { "unavailable" }
        );
    }
    for p in &loaded.model.problems {
        warn!("helicopter model: {p}");
    }
    Some(loaded)
}

#[derive(Resource)]
struct HeliCam(Vec3);

fn follow_heli(
    cam: Res<HeliCam>,
    heli: Option<Res<HeliVisual>>,
    mode: Option<Res<crate::camera::CameraMode>>,
    mut cameras: Query<&mut Transform, With<crate::camera::FlyCam>>,
) {
    let (Some(heli), Some(mode)) = (heli, mode) else { return };
    let Some(at) = heli.drawn_floor else { return };
    if *mode != crate::camera::CameraMode::Fly {
        return;
    }
    for mut t in &mut cameras {
        *t = Transform::from_translation(at + cam.0).looking_at(at, Vec3::Y);
    }
}

#[allow(clippy::too_many_arguments)]
fn spawn(
    mut commands: Commands,
    mut pending: ResMut<PendingHeli>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut images: ResMut<Assets<Image>>,
    mut objects: ResMut<Assets<ObjectMaterial>>,
    mut foreground: ResMut<Assets<ForegroundMaterial>>,
    mut bindposes: ResMut<Assets<SkinnedMeshInverseBindposes>>,
    mut translucent: ResMut<Assets<HeliTranslucentMaterial>>,
    light: Res<GameLightAssets>,
    post: Option<Res<crate::render::post::PostSettings>>,
) {
    let Some(loaded) = pending.0.take() else { return };
    let root = commands.spawn((Name::new("helicopter"), Transform::default(), Visibility::Hidden)).id();
    let rig_root = commands.spawn((Name::new("helicopter rig"), Transform::default(), ChildOf(root))).id();
    let rig = &loaded.model.rig;
    let bind = world_records(rig, &loaded.model.bind, Vec3::ZERO);
    let nodes: Vec<Entity> = rig
        .nodes
        .iter()
        .zip(&bind)
        .map(|(n, r)| commands.spawn((Name::new(n.name.clone()), record_transform(r), ChildOf(rig_root))).id())
        .collect();
    for (part, palette, inv, name, translucent_glow) in loaded.parts {
        let joints = palette.iter().map(|&n| nodes[n]).collect();
        let inverse_bindposes = bindposes.add(SkinnedMeshInverseBindposes::from(inv));
        let (mesh, material) = part.add(&mut meshes, &mut images, &light, &mut objects, &mut foreground);
        let mut e = commands.spawn((
            Name::new(format!("helicopter {name}")),
            Mesh3d(mesh),
            SkinnedMesh { inverse_bindposes, joints },
            DynamicSkinnedMeshBounds,
            Transform::default(),
            ChildOf(root),
            bevy::camera::visibility::RenderLayers::from_layers(&[0, HELI_LAYER]),
        ));
        let object = match &material {
            MaterialHandle::Object(m) if translucent_glow.is_some() => objects.get(m).cloned(),
            _ => None,
        };
        if let (Some(o), Some((glow_enable, glow_intensity))) = (object, translucent_glow) {
            let mut base = o.base.clone();
            base.alpha_mode = AlphaMode::Blend;
            let m = translucent.add(HeliTranslucentMaterial {
                base,
                extension: HeliTranslucentExtension {
                    light: light.uniform,
                    params: HeliTranslucentParams {
                        shading: Vec4::new(
                            o.extension.params.shading.y,
                            o.extension.params.shading.z,
                            glow_enable,
                            glow_intensity,
                        ),
                        tiles: o.extension.params.tiles,
                    },
                    normal: o.extension.normal.clone(),
                    specular: o.extension.specular.clone(),
                    ldr_tonemap: LDR_TONEMAP_BUFFER,
                    ldr: post.is_some(),
                },
            });
            e.insert((MeshMaterial3d(m), bevy::light::NotShadowCaster));
            continue;
        }
        match material {
            MaterialHandle::Object(m) => e.insert(MeshMaterial3d(m)),
            MaterialHandle::Foreground(m) => e.insert(MeshMaterial3d(m)),
        };
    }
    commands.insert_resource(HeliVisual {
        model: loaded.model,
        animator: loaded.animator,
        root,
        nodes,
        seen_steps: 0,
        handover: None,
        steps: (Vec::new(), Vec::new()),
        drawn_floor: None,
        lights: loaded.lights,
        particle_nodes: loaded.particles,
        wash: loaded.wash,
        newest: Vec::new(),
        drawn: Vec::new(),
        downwash: None,
    });
}

fn record_transform(r: &JointRecord) -> Transform {
    Transform { translation: r.p, rotation: r.q, scale: r.s }
}

fn snapshot(ride: Res<crate::ride::Ride>, mut heli: ResMut<HeliVisual>) {
    let h = &mut *heli;
    let Some(state) = ride.heli.as_ref() else {
        (h.steps, h.newest) = ((Vec::new(), Vec::new()), Vec::new());
        return;
    };
    if ride.steps < h.seen_steps {
        (h.seen_steps, h.handover, h.steps) = (0, None, (Vec::new(), Vec::new()));
    }
    if ride.steps == h.seen_steps && !h.steps.1.is_empty() {
        return;
    }
    let ticks = (ride.steps - h.seen_steps) as f32 * ssx_physics::DT_TICKS;
    h.seen_steps = ride.steps;
    h.animator.update(ticks);
    let root = Quat::from_array(state.gameplay_pose().q.get("Root").copied().unwrap_or([0.0, 0.0, 0.0, 1.0]));
    let (newest, reference) = pose_records(h, state, root);
    let before = std::mem::replace(&mut h.steps.1, snapshot_block(&newest, reference));
    h.steps.0 = if before.is_empty() { h.steps.1.clone() } else { before };
    h.newest = newest;
    h.downwash = ride.session.downwash;
    if let (Some((_, wash)), Some(d)) = (h.wash, h.downwash)
        && let Some((q, p)) = d.wash
    {
        h.newest[wash] = JointRecord { s: Vec3::ONE, q, p };
    }
}

fn update(
    ride: Option<Res<crate::ride::Ride>>,
    heli: Option<ResMut<HeliVisual>>,
    mut lights: ResMut<crate::render::lighting::local::DynamicLocalLights>,
    mut transforms: Query<&mut Transform>,
    mut visibility: Query<&mut Visibility>,
) {
    let (Some(ride), Some(mut heli)) = (ride, heli) else { return };
    let h = &mut *heli;
    if ride.heli.is_none() || h.steps.1.is_empty() {
        if let Ok(mut v) = visibility.get_mut(h.root) {
            *v = Visibility::Hidden;
        }
        h.drawn = Vec::new();
        return;
    }
    let lag_ticks = ((ride.steps as f32 / ssx_physics::SIM_HZ - ride.render_time).max(0.0)) * 60.0;
    let frac = 1.0 - (lag_ticks / ssx_physics::DT_TICKS).clamp(0.0, 1.0);
    interpolate_records(&h.steps.0, &h.steps.1, frac, &mut h.drawn);
    for (&e, r) in h.nodes.iter().zip(&h.drawn) {
        if let Ok(mut tr) = transforms.get_mut(e) {
            *tr = record_transform(r);
        }
    }
    h.drawn_floor = h.model.rig.find("Floor").map(|f| h.drawn[f].p);
    if let Ok(mut v) = visibility.get_mut(h.root) {
        *v = Visibility::Inherited;
    }
    if let Some(fx) = drop_heli(&ride) {
        for l in &h.lights {
            let Some(factor) = light_factor(fx, &l.group).filter(|_| l.light.enable) else { continue };
            let Some(j) = h.drawn.get(l.joint) else { continue };
            let r = crate::rider::skin::compose(j, &l.local);
            let world = bevy::math::Affine3A::from_scale_rotation_translation(r.s, r.q, r.p);
            lights.lights.push(crate::render::lighting::local::DynamicLight { world, light: l.light.clone(), factor });
        }
    }
}

pub fn drop_heli(ride: &crate::ride::Ride) -> Option<&ssx_physics::controllers::EntityFx> {
    let c = ride.session.controllers.as_ref()?;
    c.entities.iter().find(|e| e.owner.as_ref().is_some_and(|o| o.is("DropHeli"))).map(|e| &e.fx)
}

fn light_factor(fx: &ssx_physics::controllers::EntityFx, group: &str) -> Option<f32> {
    use ssx_assets::light_script::LightBehaviour;
    use ssx_physics::controllers::LightUpdate;
    let run = fx
        .lights
        .values()
        .find(|r| r.on && !r.head_lamp && if r.group { r.name == group } else { r.name.is_empty() })?;
    match run.update {
        LightUpdate::Blink { strength, frequency, gap } => {
            Some(LightBehaviour::Blink { strength, frequency, gap }.factor(run.time, 0.0))
        }
        LightUpdate::Flicker { .. } => None,
        _ => Some(run.factor),
    }
}

fn snapshot_block(records: &[JointRecord], reference: Vec3) -> Vec<JointRecord> {
    use ssx_formats::vmx::{add, pack_half, sub, unpack_half};
    let half = |x: f32| unpack_half(pack_half(x));
    let r = reference.to_array();
    records
        .iter()
        .map(|j| {
            let p = j.p.to_array();
            JointRecord {
                s: Vec3::from_array(j.s.to_array().map(half)),
                q: Quat::from_array(j.q.to_array().map(half)),
                p: Vec3::from_array(std::array::from_fn(|i| add(half(sub(p[i], r[i])), r[i]))),
            }
        })
        .collect()
}

fn pose_records(
    h: &mut HeliVisual,
    state: &ssx_physics::helicopter::Helicopter,
    root: Quat,
) -> (Vec<JointRecord>, Vec3) {
    let mut base;
    let frame = match state.nis_pose() {
        Some(nis) => {
            base = nis;
            h.handover = Some(base.clone());
            state.frame
        }
        None => {
            let posing = !crate::render::post::env_is("SSX_HELI_POSING", "off");
            base = state.gameplay_pose();
            let clip_node = matches!(
                state.node,
                ssx_physics::helicopter::HeliNode::IdleEnd { .. } | ssx_physics::helicopter::HeliNode::Trick { .. }
            );
            if posing && !clip_node {
                base.q.insert("Root".into(), root.to_array());
            } else if !clip_node {
                let d = h.animator.default_pose();
                for j in ["Root", "Roll"] {
                    base.q.insert(j.into(), d.q.get(j).copied().unwrap_or([0.0, 0.0, 0.0, 1.0]));
                    base.t.insert(j.into(), d.t.get(j).copied().unwrap_or([0.0; 3]));
                }
            }
            if let (Some(last), ssx_physics::helicopter::HeliPhase::Gameplay { ticks, .. }) = (&h.handover, state.phase)
            {
                let w = 1.0 - ssx_assets::anim_eval::curve_weight(ticks.max(0.0) / 20.0, 1);
                let mut blended = last.clone();
                blended.blend_from(&base, |_, _| w);
                base = blended;
            }
            base.q.insert("AITrajectory".into(), [0.0, 0.0, 0.0, 1.0]);
            base.t.insert("AITrajectory".into(), [0.0; 3]);
            state.traj
        }
    };
    let pose = h.animator.pose(&base, state.nis_active());
    let (fq, fp) = frame;
    let nodes = h.model.node_pose(&pose, (fq.to_array(), fp.to_array()));
    (world_records(&h.model.rig, &nodes, Vec3::ZERO), state.traj.1)
}

const TRANSLUCENT_SHADER: &str = "embedded://ssx_bevy/ride/heli_translucent.wesl";

pub type HeliTranslucentMaterial = ExtendedMaterial<StandardMaterial, HeliTranslucentExtension>;

struct HeliTranslucentPlugin;

impl Plugin for HeliTranslucentPlugin {
    fn build(&self, app: &mut App) {
        embedded_asset!(app, "heli_translucent.wesl");
        app.add_plugins(MaterialPlugin::<HeliTranslucentMaterial>::default());
    }
}

#[derive(Clone, Copy, Debug, Default, Reflect, ShaderType)]
pub struct HeliTranslucentParams {
    pub shading: Vec4,
    pub tiles: Vec4,
}

#[derive(Asset, AsBindGroup, Reflect, Debug, Clone)]
#[bind_group_data(HeliTranslucentKey)]
pub struct HeliTranslucentExtension {
    #[uniform(100)]
    pub light: GameLightUniform,
    #[uniform(101)]
    pub params: HeliTranslucentParams,
    #[texture(102)]
    #[sampler(103)]
    pub normal: Handle<Image>,
    #[texture(104)]
    pub specular: Handle<Image>,
    #[storage(122, read_only)]
    pub ldr_tonemap: Handle<ShaderBuffer>,
    pub ldr: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Reflect)]
pub struct HeliTranslucentKey {
    pub ldr: bool,
}

impl From<&HeliTranslucentExtension> for HeliTranslucentKey {
    fn from(e: &HeliTranslucentExtension) -> Self {
        HeliTranslucentKey { ldr: e.ldr }
    }
}

impl MaterialExtension for HeliTranslucentExtension {
    fn vertex_shader() -> ShaderRef {
        crate::render::lighting::game::GAME_VERTEX_PATH.into()
    }

    fn fragment_shader() -> ShaderRef {
        TRANSLUCENT_SHADER.into()
    }

    fn specialize(
        _pipeline: &MaterialExtensionPipeline,
        descriptor: &mut RenderPipelineDescriptor,
        _layout: &MeshVertexBufferLayoutRef,
        key: MaterialExtensionKey<Self>,
    ) -> Result<(), SpecializedMeshPipelineError> {
        crate::render::lighting::game::specialize_game_vertex(descriptor, 100);
        if let Some(depth) = descriptor.depth_stencil.as_mut() {
            depth.depth_write_enabled = Some(true);
        }
        let Some(fragment) = descriptor.fragment.as_mut() else { return Ok(()) };
        let Some(Some(target)) = fragment.targets.first_mut() else { return Ok(()) };
        target.blend = Some(BlendState {
            color: BlendComponent {
                src_factor: BlendFactor::SrcAlpha,
                dst_factor: BlendFactor::OneMinusSrcAlpha,
                operation: BlendOperation::Add,
            },
            alpha: BlendComponent {
                src_factor: BlendFactor::One,
                dst_factor: BlendFactor::One,
                operation: BlendOperation::Add,
            },
        });
        if key.bind_group_data.ldr {
            target.format = LDR_FRAME_FORMAT;
            fragment.shader_defs.push("SSX_LDR_TRANSLUCENT".into());
            descriptor.label = Some(LDR_PIPELINE_LABEL.into());
        }
        Ok(())
    }
}
