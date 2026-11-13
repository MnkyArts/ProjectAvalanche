use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result};
use bevy::asset::{RenderAssetUsages, embedded_asset};
use bevy::light::NotShadowCaster;
use bevy::mesh::{Indices, MeshVertexBufferLayoutRef, PrimitiveTopology};
use bevy::pbr::{Material, MaterialPipeline, MaterialPipelineKey, MaterialPlugin};
use bevy::prelude::{
    AlphaMode, App, Asset, Assets, Camera, Camera3d, Commands, Entity, GlobalTransform, Handle, Image,
    IntoScheduleConfigs, Mesh, Mesh3d, MeshMaterial3d, Name, PostUpdate, Quat, Query, Reflect, Res, ResMut, Resource,
    Time, Transform, Vec3, Vec4, Visibility, With, World, debug, default, info, resource_exists, warn,
};
use bevy::render::render_resource::{
    AsBindGroup, BlendComponent, BlendFactor, BlendOperation, BlendState, RenderPipelineDescriptor, ShaderType,
    SpecializedMeshPipelineError,
};
use bevy::shader::ShaderRef;
use ssx_assets::particles::ParticleNode;
use ssx_assets::track_objects::geotags::{DistanceScale, GeotagAssets, distance_scale, load};
use ssx_physics::session::geotags::{GeotagData, GeotagRun};

use crate::render::objects::material::{LDR_FRAME_FORMAT, LDR_PIPELINE_LABEL};
use crate::render::terrain::textures::{Texels, load_chain, texture_2d};

const SHADER_PATH: &str = "embedded://ssx_bevy/render/geotag.wesl";

#[derive(Clone, Copy, Debug, Default, Reflect, ShaderType)]
pub struct GeotagParams {
    pub base_colour: Vec4,
    pub exterior: Vec4,
    pub core: Vec4,
    pub specular: Vec4,
    pub pulse: Vec4,
    pub tile: Vec4,
    pub light: Vec4,
}

// TODO(guess): the effect's `Time` is Bevy's `globals.time`; settle: the effect-parameter registrations of `Time` (`FUN_82A90978` callers) and their per-frame write.
#[derive(Asset, AsBindGroup, Reflect, Debug, Clone)]
#[bind_group_data(GeotagPipelineKey)]
pub struct GeotagMaterial {
    #[uniform(0)]
    pub params: GeotagParams,
    #[texture(1)]
    #[sampler(2)]
    pub diffuse: Handle<Image>,
    pub ldr: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Reflect)]
pub struct GeotagPipelineKey {
    pub ldr: bool,
}

impl From<&GeotagMaterial> for GeotagPipelineKey {
    fn from(m: &GeotagMaterial) -> Self {
        GeotagPipelineKey { ldr: m.ldr }
    }
}

impl Material for GeotagMaterial {
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

    // TODO(guess): drawn in the `TRAN` pass into the tone-mapped 8-bit frame like the translucent instances; settle: the render entity's draw list (vtable 0x8210CEF4) against main.scrape's passes.
    fn specialize(
        _pipeline: &MaterialPipeline,
        descriptor: &mut RenderPipelineDescriptor,
        _layout: &MeshVertexBufferLayoutRef,
        key: MaterialPipelineKey<Self>,
    ) -> Result<(), SpecializedMeshPipelineError> {
        descriptor.primitive.cull_mode = None;
        if let Some(depth) = descriptor.depth_stencil.as_mut() {
            depth.depth_write_enabled = Some(false);
        }
        let Some(fragment) = descriptor.fragment.as_mut() else { return Ok(()) };
        let Some(Some(target)) = fragment.targets.first_mut() else { return Ok(()) };
        let add = BlendComponent {
            src_factor: BlendFactor::SrcAlpha,
            dst_factor: BlendFactor::One,
            operation: BlendOperation::Add,
        };
        target.blend = Some(BlendState { color: add, alpha: add });
        if key.bind_group_data.ldr {
            target.format = LDR_FRAME_FORMAT;
            descriptor.label = Some(LDR_PIPELINE_LABEL.into());
        }
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub struct GeotagBurst {
    pub entity: usize,
    pub node: ParticleNode,
    pub position: Vec3,
    pub rotation: Quat,
}

#[derive(Resource, Default)]
pub struct GeotagBursts {
    pub pending: Vec<(f32, GeotagBurst)>,
}

pub struct Loaded {
    assets: GeotagAssets,
    meshes: Vec<Mesh>,
    diffuse: Image,
    diffuse_id: ssx_content::ContentId,
    root: std::path::PathBuf,
    scale: DistanceScale,
    track_key: u32,
    fly: Option<GeotagRun>,
}

#[derive(Resource)]
struct PendingGeotags(Option<Loaded>);

#[derive(Resource)]
pub struct Geotags {
    tiers: Vec<(Handle<Mesh>, Handle<GeotagMaterial>)>,
    bursts: Vec<ParticleNode>,
    entities: Vec<Entity>,
    seen: Vec<bool>,
    scale: DistanceScale,
    track_key: u32,
    fly: Option<GeotagRun>,
    profile: ssx_assets::career::Profile,
}

pub fn seed() -> Option<u32> {
    std::env::var("SSX_GEOTAG_SEED").ok().and_then(|v| v.trim().parse().ok())
}

fn to_mesh(m: ssx_assets::object_mesh::ObjectMesh) -> Mesh {
    let mut out = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::default())
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, m.positions)
        .with_inserted_indices(Indices::U32(m.indices));
    match m.normals {
        Some(n) => {
            let unit = |n: [f32; 3]| Vec3::from_array(n).try_normalize().unwrap_or(Vec3::Y).to_array();
            out.insert_attribute(Mesh::ATTRIBUTE_NORMAL, n.into_iter().map(unit).collect::<Vec<_>>());
        }
        None => out.compute_smooth_normals(),
    }
    if let Some(uv) = m.uv0 {
        out.insert_attribute(Mesh::ATTRIBUTE_UV_0, uv);
    }
    if let Some(c) = m.colours {
        let lanes: Vec<[f32; 4]> = c.iter().map(|b| [3, 2, 1, 0].map(|i| f32::from(b[i]) / 255.0)).collect();
        out.insert_attribute(Mesh::ATTRIBUTE_COLOR, lanes);
    }
    out
}

fn load_all(root: &Path, track: &str, riding: bool) -> Result<Option<Loaded>> {
    let Some(data) = GeotagData::load(root, track)? else { return Ok(None) };
    let assets = load(root, track)?;
    let meshes = assets
        .objects
        .iter()
        .map(|o| Ok(to_mesh(crate::render::sky::midground::static_mesh(root, &o.geometry)?)))
        .collect::<Result<Vec<_>>>()?;
    let tex = assets.objects[0].material.textures.get("diffuseTexture").context("no diffuseTexture")?;
    let id = ssx_assets::content::textures::texture_id(tex).with_context(|| format!("{tex:?}: not a texture id"))?;
    let diffuse = texture_2d(&load_chain(root, &id, &ssx_content::texture::Want::PLANE)?, Texels::Linear, default());
    let track_key = data.track_key;
    let fly = (!riding)
        .then(|| GeotagRun::new(Arc::new(data), |_| false, &mut ssx_assets::career::random::Source::seeded(seed())));
    let (diffuse_id, root_dir) = (id, root.to_path_buf());
    Ok(Some(Loaded {
        assets,
        meshes,
        diffuse,
        diffuse_id,
        root: root_dir,
        scale: distance_scale(root)?,
        track_key,
        fly,
    }))
}

pub fn register(app: &mut App) {
    embedded_asset!(app, "geotag.wesl");
    app.add_plugins(MaterialPlugin::<GeotagMaterial>::default())
        .init_resource::<GeotagBursts>()
        .add_systems(crate::shell::level::LevelStartup, spawn.run_if(resource_exists::<PendingGeotags>))
        .add_systems(PostUpdate, update.before(bevy::transform::TransformSystems::Propagate));
    crate::shell::level::scoped::<PendingGeotags>(app);
    crate::shell::level::scoped::<Geotags>(app);
}

pub fn reset_level(world: &mut World) {
    world.insert_resource(GeotagBursts::default());
}

pub fn insert(world: &mut World, loaded: Loaded) {
    world.insert_resource(PendingGeotags(Some(loaded)));
}

pub fn load_level(big_root: Option<&Path>, track: &str, riding: bool) -> Option<Loaded> {
    if crate::render::post::env_is("SSX_GEOTAGS", "off")
        || (!riding && !crate::render::post::env_is("SSX_GEOTAGS", "on"))
    {
        return None;
    }
    let root = big_root?;
    let loaded = match load_all(root, track, riding) {
        Ok(Some(l)) => l,
        Ok(None) => {
            info!("geotags: {track} has none");
            return None;
        }
        Err(e) => {
            warn!("geotags unavailable: {e:#}");
            return None;
        }
    };
    let c = &loaded.assets.controller;
    info!(
        "geotags: tiers {} | controller: the GeoCache set ({} states, collect volume {:?}) | \
         distance scale {}…{} m ×{} | {}",
        loaded.assets.objects.iter().map(|o| o.game_object.as_str()).collect::<Vec<_>>().join(", "),
        c.sets.sets[c.set].states.len(),
        c.collect_volume,
        loaded.scale.start,
        loaded.scale.end,
        loaded.scale.size,
        if riding { "the ride's session" } else { "fly camera: app clock" }
    );
    Some(loaded)
}

fn spawn(
    mut commands: Commands,
    mut pending: ResMut<PendingGeotags>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut images: ResMut<Assets<Image>>,
    mut materials: ResMut<Assets<GeotagMaterial>>,
    post: Option<Res<crate::render::post::PostSettings>>,
    lighting: Option<Res<crate::render::lighting::GameLighting>>,
) {
    let Some(loaded) = pending.0.take() else { return };
    let mut diffuse = loaded.diffuse;
    match crate::render::fx_states::sampler("object_geotag.fx", "translucent", "diffuseTextureSampler")
        .and_then(|s| s.image_sampler())
    {
        Ok(s) => diffuse.sampler = s,
        Err(e) => warn!("geotag diffuse sampler: {e:#}"),
    }
    let recipe = crate::render::terrain::textures::plane_recipe(
        &loaded.root,
        &loaded.diffuse_id,
        ssx_content::texture::Want::PLANE,
        Texels::Linear,
        diffuse.sampler.clone(),
    );
    let diffuse = crate::render::reload::add(&mut images, diffuse, Some(recipe));
    let light = lighting.map_or(Vec3::Y, |l| l.sun_offset).extend(0.0);
    let ldr = post.is_some();
    let tiers: Vec<(Handle<Mesh>, Handle<GeotagMaterial>)> = loaded
        .assets
        .objects
        .iter()
        .zip(loaded.meshes)
        .map(|(o, mesh)| {
            let m = &o.material;
            let f = |n: &str| m.float(n).unwrap_or(0.0);
            let v = |n: &str| {
                let mut a = [0.0f32; 4];
                for (d, x) in a.iter_mut().zip(m.vectors.get(n).into_iter().flatten()) {
                    *d = *x;
                }
                Vec4::from_array(a)
            };
            let params = GeotagParams {
                base_colour: v("baseColour"),
                exterior: Vec4::new(f("exteriorIntensity"), f("exteriorExponent"), f("exteriorSpread"), 0.0),
                core: Vec4::new(f("coreIntensity"), f("coreExponent"), f("coreSpread"), 0.0),
                specular: Vec4::new(f("specularIntensity"), f("specularEccentricity"), 0.0, 0.0),
                pulse: Vec4::new(f("pulseSpeed"), f("pulseAmount"), f("spinSpeed"), 0.0),
                tile: v("diffuseTile"),
                light,
            };
            (meshes.add(mesh), materials.add(GeotagMaterial { params, diffuse: diffuse.clone(), ldr }))
        })
        .collect();
    for (mesh, material) in &tiers {
        let warm = crate::render::warm_up::draw();
        commands.spawn((Mesh3d(mesh.clone()), MeshMaterial3d(material.clone()), NotShadowCaster, warm));
    }
    let bursts = loaded.assets.objects.iter().map(|o| o.burst.clone()).collect();
    commands.insert_resource(Geotags {
        tiers,
        bursts,
        entities: Vec::new(),
        seen: Vec::new(),
        scale: loaded.scale,
        track_key: loaded.track_key,
        fly: loaded.fly,
        profile: ssx_assets::career::Profile::new(),
    });
}

fn update(
    mut commands: Commands,
    geotags: Option<ResMut<Geotags>>,
    ride: Option<Res<crate::ride::Ride>>,
    cameras: Query<(&Camera, &GlobalTransform), With<Camera3d>>,
    mut placed: Query<(&mut Transform, &mut Visibility)>,
    mut bursts: ResMut<GeotagBursts>,
    time: Res<Time>,
) {
    let Some(mut g) = geotags else { return };
    let g = &mut *g;
    if let Some(run) = g.fly.as_mut() {
        run.update(time.delta_secs(), Vec3::new(0.0, 1.0e7, 0.0), false);
    }
    let Some(run) = ride.as_ref().and_then(|r| r.session.geotags.as_ref()).or(g.fly.as_ref()) else { return };
    let eye = cameras.iter().find(|(c, _)| c.is_active).map(|(_, t)| t.translation());
    let now = time.elapsed_secs();
    bursts.pending.retain(|(t, b)| {
        let keep = now - t < 1.0;
        if !keep {
            let effect = b.node.lynx.as_ref().map_or("-", |l| l.name.as_str());
            debug!("geotag burst {effect} at {:.1} ({:?}) not taken by the particle system", b.position, b.rotation);
        }
        keep
    });
    for (i, e) in run.entities.iter().enumerate() {
        if i == g.entities.len() {
            let (mesh, material) = g.tiers[e.record.tier].clone();
            let id = commands
                .spawn((
                    Name::new(format!("geotag {}", e.record.row.id)),
                    Mesh3d(mesh),
                    MeshMaterial3d(material),
                    Transform::from_translation(e.position),
                    Visibility::Hidden,
                    NotShadowCaster,
                ))
                .id();
            g.entities.push(id);
            g.seen.push(false);
            continue;
        }
        if let Ok((mut t, mut vis)) = placed.get_mut(g.entities[i]) {
            let s = eye.map_or(1.0, |c| g.scale.at(c.distance(e.position)));
            *t = Transform { translation: e.position, rotation: e.rotation, scale: Vec3::splat(s) };
            *vis = if e.visible { Visibility::Visible } else { Visibility::Hidden };
        }
        let collected = e.collected;
        if collected && !g.seen[i] {
            g.seen[i] = true;
            let paid = g.profile.collect_geotag(g.track_key, &e.record.row);
            info!(
                "geotag {} collected (tier {}, ride step {:?}): value {}, paid {paid:?} -> credits {}, collected {} of {}, \
                 GEOCACHE_ResultList {:?}",
                e.record.row.id,
                e.record.tier + 1,
                ride.as_ref().map(|r| r.steps),
                e.record.value,
                g.profile.credits(),
                g.profile.collected_geotags.len(),
                run.data.disc_total,
                run.result_list
            );
            let node = g.bursts[e.record.tier].clone();
            bursts.pending.push((now, GeotagBurst { entity: i, node, position: e.position, rotation: e.rotation }));
        }
    }
}
