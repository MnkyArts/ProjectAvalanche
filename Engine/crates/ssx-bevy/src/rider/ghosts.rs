use std::path::Path;

use anyhow::{Context, Result};
use bevy::asset::{RenderAssetUsages, embedded_asset};
use bevy::camera::visibility::DynamicSkinnedMeshBounds;
use bevy::light::NotShadowCaster;
use bevy::mesh::skinning::{SkinnedMesh, SkinnedMeshInverseBindposes};
use bevy::mesh::{Indices, MeshVertexBufferLayoutRef, PrimitiveTopology, VertexAttributeValues};
use bevy::pbr::{Material, MaterialPipeline, MaterialPipelineKey, MaterialPlugin};
use bevy::prelude::{
    AlphaMode, App, Asset, Assets, ChildOf, Commands, Entity, Handle, Image, IntoScheduleConfigs, Mat4, Mesh, Mesh3d,
    MeshMaterial3d, Name, Plugin, PostUpdate, Quat, Query, Reflect, Res, ResMut, Resource, Time, Transform, Vec3, Vec4,
    Visibility, World, info, resource_exists, warn,
};
use bevy::render::render_resource::{
    AsBindGroup, BlendComponent, BlendFactor, BlendOperation, BlendState, Extent3d, RenderPipelineDescriptor,
    ShaderType, SpecializedMeshPipelineError, TextureDimension, TextureFormat,
};
use bevy::shader::ShaderRef;
use ssx_assets::anim_db::AssetDb;
use ssx_assets::ghosts::{
    EXPLORE_GHOST_TINT, GhostAnimation, GhostModel, GhostPose, GhostReplay, GhostRig, MEDAL_LABELS, tint_rgba,
};
use ssx_physics::event::{Course, EventType};

use crate::render::objects::material::{LDR_FRAME_FORMAT, LDR_PIPELINE_LABEL};
use crate::rider::anim::{JointRecord, world_records};

const SHADER_PATH: &str = "embedded://ssx_bevy/rider/ghost.wesl";

pub struct GhostMaterialPlugin;

impl Plugin for GhostMaterialPlugin {
    fn build(&self, app: &mut App) {
        embedded_asset!(app, "ghost.wesl");
        app.add_plugins(MaterialPlugin::<GhostMaterial>::default());
    }
}

#[derive(Clone, Copy, Debug, Default, Reflect, ShaderType)]
pub struct GhostParams {
    pub ghost_color: Vec4,
    pub ghost_base_color: Vec4,
    pub shading: Vec4,
    pub tint: Vec4,
    pub hologlitch: Vec4,
}

#[derive(Asset, AsBindGroup, Reflect, Debug, Clone)]
#[bind_group_data(GhostPipelineKey)]
pub struct GhostMaterial {
    #[uniform(0)]
    pub params: GhostParams,
    #[texture(1)]
    #[sampler(2)]
    pub normal: Handle<Image>,
    pub ldr: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Reflect)]
pub struct GhostPipelineKey {
    pub ldr: bool,
}

impl From<&GhostMaterial> for GhostPipelineKey {
    fn from(m: &GhostMaterial) -> Self {
        GhostPipelineKey { ldr: m.ldr }
    }
}

impl Material for GhostMaterial {
    fn fragment_shader() -> ShaderRef {
        SHADER_PATH.into()
    }

    fn alpha_mode(&self) -> AlphaMode {
        AlphaMode::Blend
    }

    fn enable_prepass() -> bool {
        false
    }

    fn enable_shadows() -> bool {
        false
    }

    fn specialize(
        _pipeline: &MaterialPipeline,
        descriptor: &mut RenderPipelineDescriptor,
        _layout: &MeshVertexBufferLayoutRef,
        key: MaterialPipelineKey<Self>,
    ) -> Result<(), SpecializedMeshPipelineError> {
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
            descriptor.label = Some(LDR_PIPELINE_LABEL.into());
        }
        Ok(())
    }
}

pub struct Rival {
    pub label: &'static str,
    pub value: u32,
    pub replay: GhostReplay,
}

impl Rival {
    pub fn short(&self) -> &'static str {
        self.label.trim_start_matches("TXT_EXPLORE_GHOST_MEDAL_")
    }
}

struct GhostRigInfo {
    ghost: GhostRig,
    axes: Vec<(usize, usize, usize)>,
}

pub struct LoadedGhosts {
    model: GhostModel,
    info: GhostRigInfo,
    rivals: Vec<(Rival, GhostAnimation)>,
    normal: Image,
    normal_source: (std::path::PathBuf, String),
    course: Option<Course>,
    event: String,
    riding: bool,
    slot: Option<(Vec3, Quat)>,
}

#[derive(Resource)]
struct PendingGhosts(Option<LoadedGhosts>);

fn load(root: &Path, track: &str, drop_key: u32, riding: bool) -> Result<Option<LoadedGhosts>> {
    let (events, end) = ssx_physics::spawn::load_drop_events(root, track, drop_key)?;
    let want = std::env::var("SSX_RIDE_EVENT").unwrap_or_else(|_| "race".into());
    let def = if want == "none" { None } else { ssx_physics::event::choose_event(events.iter(), &want) };
    let Some(def) = def else { return Ok(None) };
    let board = def.ghost_board();
    let rows: Vec<(usize, &ssx_physics::event::Rival)> =
        board.iter().enumerate().filter_map(|(k, g)| Some((k, def.rivals.get(g.row?)?))).collect();
    if rows.is_empty() {
        return Ok(None);
    }
    let model = ssx_assets::ghosts::load_model(root, track)?;
    let ghost = GhostRig::new(root, &model)?;
    let rig = &ghost.rig;
    let axes = [("LeftAxeAux", "LeftIceAxe"), ("RightAxeAux", "RightIceAxe")]
        .iter()
        .filter_map(|&(s, t)| {
            let (s, t) = (rig.find(s)?, rig.find(t)?);
            Some((s, t, rig.nodes[t].parent?))
        })
        .collect();
    let info = GhostRigInfo { ghost, axes };
    let floats = ssx_assets::rig_anim::float_dofs(root)?;
    let db = AssetDb::load(&root.join(ssx_assets::rig_anim::ANIM_DIR), &[ssx_assets::rig_anim::RIG_BUNDLE])?;
    let mut rivals = Vec::new();
    for (k, row) in rows.into_iter().filter(|(k, _)| *k < MEDAL_LABELS.len()) {
        let id = row.ghost_id as u32;
        let replay = match GhostReplay::load(root, id) {
            Ok(r) => r,
            Err(e) => {
                warn!("ghost {id}: {e:#}");
                continue;
            }
        };
        let (anim, problems) = GhostAnimation::new(root, &db, &info.ghost, &floats)?;
        if rivals.is_empty() {
            for p in problems {
                info!("ghost animation: {p}");
            }
        }
        rivals.push((Rival { label: MEDAL_LABELS[k], value: row.value(), replay }, anim));
    }
    let normal_rel = model.parts.first().and_then(|p| p.material.textures.get("normalTexture").cloned());
    let normal_rel = normal_rel.context("ghost_character_mat has no normalTexture")?;
    let normal = load_normal(root, &normal_rel)?;
    let normal_source = (root.to_path_buf(), normal_rel);
    let slot = ssx_physics::spawn::load_drop_starts(root, track)?
        .iter()
        .find(|s| s.drop_key == drop_key)
        .and_then(|s| s.slot(0));
    let start_y = slot.map(|(p, _)| p.y);
    let course = start_y.zip(end).map(|(start_y, e)| Course { start_y, end_y: e.y });
    Ok(Some(LoadedGhosts { model, info, rivals, normal, normal_source, course, event: def.name.clone(), riding, slot }))
}

fn load_normal(root: &Path, rel: &str) -> Result<Image> {
    let id = ssx_assets::content::textures::texture_id(rel).with_context(|| format!("{rel:?}: not a texture id"))?;
    let found = ssx_assets::content::GameContent::of(root).texture(&id, &ssx_content::texture::Want::NORMAL)?;
    let levels = found.texture.levels;
    let first = levels.first().context("no levels")?;
    let (w, h) = (first.width, first.height);
    let mips = levels
        .iter()
        .enumerate()
        .take_while(|(i, l)| l.width == (w >> i).max(1) && l.height == (h >> i).max(1))
        .count();
    let bytes: Vec<u8> = levels[..mips].iter().flat_map(|l| l.images[0].iter().copied()).collect();
    let size = Extent3d { width: w, height: h, depth_or_array_layers: 1 };
    let mut image =
        Image::new_uninit(size, TextureDimension::D2, TextureFormat::Rgba8Unorm, RenderAssetUsages::RENDER_WORLD);
    image.data = Some(bytes);
    image.texture_descriptor.mip_level_count = mips as u32;
    Ok(image)
}

pub type Loaded = LoadedGhosts;

pub fn register(app: &mut App) {
    app.add_plugins(GhostMaterialPlugin)
        .init_resource::<GhostHud>()
        .add_systems(crate::shell::level::LevelStartup, spawn.run_if(resource_exists::<PendingGhosts>))
        .add_systems(PostUpdate, update.before(bevy::transform::TransformSystems::Propagate));
    crate::shell::level::scoped::<PendingGhosts>(app);
    crate::shell::level::scoped::<Ghosts>(app);
}

pub fn reset_level(world: &mut World) {
    world.insert_resource(GhostHud::default());
}

pub fn insert(world: &mut World, loaded: Loaded) {
    world.insert_resource(PendingGhosts(Some(loaded)));
}

pub fn load_level(big_root: Option<&Path>, track: &str, drop_key: Option<u32>, riding: bool) -> Option<Loaded> {
    if crate::render::post::env_is("SSX_GHOSTS", "off") {
        info!("ghosts: off (SSX_GHOSTS=off)");
        return None;
    }
    if !riding && !crate::render::post::env_is("SSX_GHOSTS", "on") && std::env::var("SSX_GHOST_TIME").is_err() {
        return None;
    }
    let (Some(root), Some(drop_key)) = (big_root, drop_key) else { return None };
    let loaded = match load(root, track, drop_key, riding) {
        Ok(Some(l)) => l,
        Ok(None) => {
            info!("ghosts: the drop's event has no ranked rivals");
            return None;
        }
        Err(e) => {
            warn!("ghosts unavailable: {e:#}");
            return None;
        }
    };
    info!(
        "ghosts: {} rivals of {} | model {} ({} parts, {} problems) | {}",
        loaded.rivals.len(),
        loaded.event,
        loaded.model.rig_file.display(),
        loaded.model.parts.len(),
        loaded.model.problems.len(),
        if riding { "on the race clock" } else { "fly camera: app clock" }
    );
    for (r, _) in &loaded.rivals {
        let h = &r.replay.header;
        info!(
            "ghost {}: {} by {} (build {}, gear {:#x}), value {}, {} samples over {:.2} s ({} rejected)",
            r.short(),
            r.replay.id,
            h.gamertag,
            h.build,
            h.gear_flags,
            r.value,
            r.replay.times.len(),
            r.replay.duration(),
            r.replay.rejected
        );
    }
    for p in &loaded.model.problems {
        warn!("ghost model: {p}");
    }
    Some(loaded)
}

struct GhostEntity {
    rival: Rival,
    anim: GhostAnimation,
    root: Entity,
    nodes: Vec<Entity>,
}

#[derive(Resource)]
pub struct Ghosts {
    list: Vec<GhostEntity>,
    info: GhostRigInfo,
    course: Option<Course>,
    riding: bool,
    run: GhostRun,
    slot: Option<(Vec3, Quat)>,
    idle_at: Option<(Vec3, Quat)>,
}

#[derive(Clone, Copy, Debug, Default)]
struct GhostRun {
    started: bool,
    stopped_at: Option<(u64, f32)>,
    last_steps: u64,
    epoch: Option<f32>,
}

#[derive(Resource, Default)]
pub struct GhostHud {
    pub line: String,
}

fn part_mesh(p: &ssx_assets::ghosts::GhostPart) -> Result<Mesh> {
    let m = &p.mesh;
    let unit = |n: &[f32; 3]| Vec3::from_array(*n).try_normalize().unwrap_or(Vec3::Y).to_array();
    let mut mesh = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::default())
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, m.positions.clone())
        .with_inserted_indices(Indices::U32(m.indices.clone()));
    match &m.normals {
        Some(n) => mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, n.iter().map(unit).collect::<Vec<_>>()),
        None => mesh.compute_smooth_normals(),
    }
    if let Some(uv) = &m.uv0 {
        mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uv.clone());
    }
    if let Some(t) = &m.tangents {
        mesh.insert_attribute(Mesh::ATTRIBUTE_TANGENT, t.iter().map(|&[x, y, z]| [x, y, z, 1.0]).collect::<Vec<_>>());
    }
    mesh.insert_attribute(Mesh::ATTRIBUTE_JOINT_INDEX, VertexAttributeValues::Uint16x4(p.skin.joints.clone()));
    mesh.insert_attribute(Mesh::ATTRIBUTE_JOINT_WEIGHT, p.skin.weights.clone());
    mesh.generate_skinned_mesh_bounds()?;
    Ok(mesh)
}

type SpawnPart = (Handle<Mesh>, Handle<SkinnedMeshInverseBindposes>, Vec<usize>, String);

#[allow(clippy::too_many_arguments)]
fn spawn(
    mut commands: Commands,
    mut pending: ResMut<PendingGhosts>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut images: ResMut<Assets<Image>>,
    mut materials: ResMut<Assets<GhostMaterial>>,
    mut bindposes: ResMut<Assets<SkinnedMeshInverseBindposes>>,
    post: Option<Res<crate::render::post::PostSettings>>,
) {
    let Some(loaded) = pending.0.take() else { return };
    let mut normal = loaded.normal;
    match crate::render::fx_states::sampler("player_ghost.fx", "translucent", "normalTextureSampler")
        .and_then(|s| s.image_sampler())
    {
        Ok(s) => normal.sampler = s,
        Err(e) => warn!("ghost normal sampler: {e:#}"),
    }
    let ((root, rel), sampler) = (loaded.normal_source.clone(), normal.sampler.clone());
    let id = ssx_assets::content::textures::texture_id(&rel);
    let recipe = id.map(|id| {
        crate::render::reload::Recipe::new(vec![id], move || {
            let mut image = load_normal(&root, &rel)?;
            image.sampler = sampler.clone();
            Ok(image)
        })
    });
    let normal = crate::render::reload::add(&mut images, normal, recipe);
    let mat = &loaded.model.parts[0].material;
    let vec4 = |name: &str, d: Vec4| mat.vectors.get(name).filter(|v| v.len() == 4).map_or(d, |v| Vec4::from_slice(v));
    let float = |name: &str| mat.float(name).unwrap_or(0.0);
    let params = GhostParams {
        ghost_color: vec4("ghostColor", Vec4::ONE),
        ghost_base_color: vec4("ghostBaseColor", Vec4::splat(0.35)),
        shading: Vec4::new(float("ghostIntensity"), float("fresnelExponent"), float("fresnelSpread"), 0.0),
        tint: Vec4::from_array(tint_rgba(EXPLORE_GHOST_TINT)),
        hologlitch: Vec4::ZERO,
    };
    let ldr = post.is_some();
    let parts: Vec<SpawnPart> = loaded
        .model
        .parts
        .iter()
        .filter_map(|p| match part_mesh(p) {
            Ok(m) => {
                let inv: Vec<Mat4> = p.skin.palette.iter().map(|j| Mat4::from_cols_array_2d(&j.inverse_bind)).collect();
                let nodes = p.skin.palette.iter().map(|j| j.node).collect();
                Some((meshes.add(m), bindposes.add(SkinnedMeshInverseBindposes::from(inv)), nodes, p.mesh_name.clone()))
            }
            Err(e) => {
                warn!("ghost part {}: {e:#}", p.mesh_name);
                None
            }
        })
        .collect();
    let bind = world_records(&loaded.info.ghost.rig, &loaded.info.ghost.bind, Vec3::ZERO);
    let mut list = Vec::new();
    for (rival, anim) in loaded.rivals {
        let shading = Vec4::new(params.shading.x, params.shading.y, params.shading.z, if ldr { 1.0 } else { 0.0 });
        let material =
            materials.add(GhostMaterial { params: GhostParams { shading, ..params }, normal: normal.clone(), ldr });
        let root = commands
            .spawn((Name::new(format!("ghost {}", rival.short())), Transform::default(), Visibility::Hidden))
            .id();
        let rig_root = commands.spawn((Name::new("ghost rig"), Transform::default(), ChildOf(root))).id();
        let nodes: Vec<Entity> = loaded
            .info
            .ghost
            .rig
            .nodes
            .iter()
            .enumerate()
            .map(|(i, n)| {
                let r = bind.get(i).copied().unwrap_or(JointRecord { s: Vec3::ONE, q: Quat::IDENTITY, p: Vec3::ZERO });
                commands.spawn((Name::new(n.name.clone()), record_transform(&r), ChildOf(rig_root))).id()
            })
            .collect();
        for (mesh, inverse_bindposes, palette, name) in &parts {
            let joints = palette.iter().map(|&n| nodes[n]).collect();
            commands.spawn((
                Name::new(format!("ghost {} {name}", rival.short())),
                Mesh3d(mesh.clone()),
                MeshMaterial3d(material.clone()),
                SkinnedMesh { inverse_bindposes: inverse_bindposes.clone(), joints },
                DynamicSkinnedMeshBounds,
                NotShadowCaster,
                Transform::default(),
                ChildOf(root),
            ));
        }
        list.push(GhostEntity { rival, anim, root, nodes });
    }
    commands.insert_resource(Ghosts {
        list,
        info: loaded.info,
        course: loaded.course,
        riding: loaded.riding,
        run: GhostRun::default(),
        slot: loaded.slot,
        idle_at: None,
    });
}

fn record_transform(r: &JointRecord) -> Transform {
    Transform { translation: r.p, rotation: r.q, scale: r.s }
}

fn ghost_records(
    anim: &mut GhostAnimation,
    info: &GhostRigInfo,
    p: &GhostPose,
    dt_ticks: f32,
    axe_weight: f32,
) -> Vec<JointRecord> {
    let pose = anim.pose(&info.ghost, p, dt_ticks);
    let mut records = world_records(&info.ghost.rig, &pose.nodes, Vec3::from_array(p.trajectory_position));
    for &op in &info.axes {
        crate::rider::skin::prop_in_hands(&info.ghost.rig, &mut records, op, axe_weight);
    }
    records
}

fn ghost_time(g: &mut Ghosts, ride: Option<&crate::ride::Ride>, time: &Time) -> Option<f32> {
    if let Some(ride) = ride.filter(|_| g.riding) {
        return g.drawn_time(ride);
    }
    if let Some(t) = std::env::var("SSX_GHOST_TIME").ok().and_then(|v| v.parse::<f32>().ok()) {
        return Some(t);
    }
    let now = time.elapsed_secs();
    let epoch = *g.run.epoch.get_or_insert(now);
    let longest = g.list.iter().map(|e| e.rival.replay.duration()).fold(0.0f32, f32::max);
    Some((now - epoch) % (longest + 3.0))
}

impl Ghosts {
    pub fn drawn_time(&mut self, ride: &crate::ride::Ride) -> Option<f32> {
        if !self.riding {
            return None;
        }
        let (run, ev) = (&mut self.run, &ride.session.event);
        let idle = ev.state == ssx_physics::event::EventState::Idle && !ev.clock.running && ev.clock.seconds == 0.0;
        if ride.steps < run.last_steps || idle {
            *run = GhostRun::default();
        }
        run.last_steps = ride.steps;
        // TODO(guess): the slot clock's scale (racer+8 vtable +0x68, 0x824B5E50; `FLT_GhostTimeScale` only with `BLN_GhostAvoidGrindCollision`, no writer found) is taken as 1; settle: the interface FUN_82439538 queries and its slot 0x68 for a ghost racer.
        let newest = if ev.clock.running {
            (run.started, run.stopped_at) = (true, None);
            ev.clock.seconds
        } else if run.started {
            let (s0, t0) = *run.stopped_at.get_or_insert((ride.steps, ev.clock.seconds));
            t0 + (ride.steps - s0) as f32 / ssx_physics::SIM_HZ
        } else {
            return None;
        };
        let lag = (ride.steps as f32 / ssx_physics::SIM_HZ - ride.render_time).max(0.0);
        Some((newest - lag).max(0.0))
    }

    pub fn drawn(&self, t: Option<f32>) -> Vec<(u64, Vec3)> {
        if t.is_none()
            && self.riding
            && let Some((at, _)) = self.idle_at
        {
            return self.list.iter().map(|e| (u64::from(e.rival.replay.id), at)).collect();
        }
        let shown = t.unwrap_or(0.0);
        self.list
            .iter()
            .filter(|e| !e.rival.replay.finished(shown))
            .map(|e| {
                let p = e.rival.replay.pose(shown).trajectory_position;
                (u64::from(e.rival.replay.id), Vec3::from_array(p))
            })
            .collect()
    }
}

fn update(
    time: Res<Time>,
    ride: Option<Res<crate::ride::Ride>>,
    ghosts: Option<ResMut<Ghosts>>,
    mut transforms: Query<&mut Transform>,
    mut visibility: Query<&mut Visibility>,
    mut hud: ResMut<GhostHud>,
) {
    let Some(mut ghosts) = ghosts else { return };
    let t = ghost_time(&mut ghosts, ride.as_deref(), &time);
    let held = ride.as_deref().is_some_and(|r| r.hold);
    let dt_ticks = if held { 0.0 } else { (time.delta_secs() * 60.0).min(4.0) };
    let g = &mut *ghosts;
    if g.idle_at.is_none()
        && let (Some(ride), Some((p, q))) = (ride.as_deref(), g.slot)
    {
        g.idle_at = ssx_physics::levelstart::offset_level_start(ride.world(), p).map(|o| (o, q));
    }
    for e in &mut g.list {
        if t.is_none()
            && g.riding
            && let Some((at, q)) = g.idle_at
            && let Some((pose, p)) = e.anim.idle_pose(&g.info.ghost, dt_ticks, at.to_array(), q.to_array())
        {
            if let Ok(mut v) = visibility.get_mut(e.root) {
                *v = Visibility::Inherited;
            }
            let mut records = world_records(&g.info.ghost.rig, &pose.nodes, Vec3::from_array(p));
            for &op in &g.info.axes {
                crate::rider::skin::prop_in_hands(&g.info.ghost.rig, &mut records, op, 0.0);
            }
            for (&entity, r) in e.nodes.iter().zip(&records) {
                if let Ok(mut tr) = transforms.get_mut(entity) {
                    *tr = record_transform(r);
                }
            }
            continue;
        }
        let shown = t.unwrap_or(0.0);
        let visible = !e.rival.replay.finished(shown);
        if let Ok(mut v) = visibility.get_mut(e.root) {
            *v = if visible { Visibility::Inherited } else { Visibility::Hidden };
        }
        if !visible {
            continue;
        }
        let pose = e.rival.replay.pose(shown);
        let axe = if t.is_some() && e.rival.replay.header.gear_flags & 2 != 0 { 1.0 } else { 0.0 };
        let records = ghost_records(&mut e.anim, &g.info, &pose, dt_ticks, axe);
        for (&entity, r) in e.nodes.iter().zip(&records) {
            if let Ok(mut tr) = transforms.get_mut(entity) {
                *tr = record_transform(r);
            }
        }
    }
    hud.line = hud_line(g, t, ride.as_deref());
}

fn hud_line(g: &Ghosts, t: Option<f32>, ride: Option<&crate::ride::Ride>) -> String {
    let course = ride.and_then(|r| r.session.event.setup.course).or(g.course);
    let race = ride.and_then(|r| r.session.event.setup.event_type).is_none_or(EventType::is_race);
    let split = ride.and_then(|r| r.session.event.split_times.last().copied());
    let clock = |ms: u32| format!("{}:{:06.3}", ms / 60_000, (ms % 60_000) as f32 / 1000.0);
    let items: Vec<String> = g
        .list
        .iter()
        .map(|e| {
            let r = &e.rival;
            let value = if race { clock(r.value) } else { r.value.to_string() };
            let shown = t.unwrap_or(0.0);
            let state = if r.replay.finished(shown) {
                "gone".to_string()
            } else {
                let y = r.replay.position(shown)[1];
                course.map_or(format!("y {y:.0}"), |c| format!("{:.0}%", 100.0 * c.progress(y)))
            };
            let gap = split
                .zip(course)
                .and_then(|((k, ms), c)| {
                    let h = c.splits()[usize::try_from(k).ok()?];
                    r.replay.time_below(h).map(|tg| ms as f32 / 1000.0 - tg)
                })
                .map_or(String::new(), |d| format!(" {:+.2} s", d));
            format!("{} {value} {state}{gap}", r.short())
        })
        .collect();
    let when = t.map_or("waiting for the start".into(), |t| format!("{t:.1} s"));
    let split = split.map_or(String::new(), |(k, _)| format!(" | gaps at split #{}, + = behind", k + 1));
    format!("\nGHOSTS ({when}{split}): {}\n", items.join(" | "))
}

#[cfg(test)]
mod tests {
    #[test]
    fn channel_tables_match_the_rewind() {
        assert_eq!(ssx_formats::ghost::ROTATION_CHANNELS, ssx_physics::graph::REWIND_ROTATIONS);
        assert_eq!(ssx_formats::ghost::TRANSLATION_CHANNELS, ssx_physics::graph::REWIND_TRANSLATIONS);
    }
}
