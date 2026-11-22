pub mod model;
pub mod raster;

use std::sync::{Arc, Mutex};
use std::time::Instant;

use bevy::asset::RenderAssetUsages;
use bevy::ecs::system::NonSendMarker;
use bevy::mesh::skinning::{SkinnedMesh, SkinnedMeshInverseBindposes};
use bevy::mesh::{Indices, VertexAttributeValues};
use bevy::prelude::*;
use bevy::render::extract_resource::{ExtractResource, ExtractResourcePlugin};
use bevy::render::render_asset::RenderAssets;
use bevy::render::render_resource::{
    Extent3d, Origin3d, ShaderType, TexelCopyBufferLayout, TexelCopyTextureInfo, TextureAspect, TextureDimension,
    TextureFormat, TextureViewDescriptor, TextureViewDimension,
};
use bevy::render::renderer::RenderQueue;
use bevy::render::texture::GpuImage;
use bevy::render::{Render, RenderApp, RenderSystems};
use ssx_formats::subd::SubdFile;
use ssx_physics::surface::SNOW_COVERAGE;

use crate::frame_task::FrameTask;
use crate::render::snow_tracks::model::{
    Caster, CasterTransforms, Changes, MAX_TILES, Noise, TEXELS, TILE_WORLD, TileSystem, UNTOUCHED, disp_map,
};
use crate::render::uploads::{LevelImage, LevelUploads};
use crate::ride::Ride;
use crate::rider::visual::RiderRoot;

const RENDER_HZ: f32 = 60.0;

#[derive(Clone, Copy, Debug, Default, Reflect, ShaderType)]
pub struct SnowTrackParams {
    pub tile: Vec4,
    pub size: Vec4,
}

#[derive(Clone, Debug)]
pub struct SnowTrackTextures {
    pub lookup: Handle<Image>,
    pub tiles: Handle<Image>,
    pub params: SnowTrackParams,
}

enum Upload {
    Lookup { index: usize, value: u16 },
    Slice { layer: u32, x: u32, y: u32, w: u32, h: u32, bytes: Vec<u8> },
}

#[derive(Resource, Clone, ExtractResource)]
#[extract_app(RenderApp)]
pub struct SnowTrackGpu {
    lookup: Handle<Image>,
    tiles: Handle<Image>,
    lookup_width: u32,
    uploads: Arc<Mutex<Vec<Upload>>>,
}

#[derive(Clone, Copy, Debug)]
struct History {
    step: u64,
    caster: Caster,
    active: bool,
}

#[derive(Resource)]
pub struct SnowTracks {
    sim: FrameTask<TrackSim, Passes>,
    lookup_width: u32,
    history: Option<History>,
    ticks: u64,
    pub update_ms: f32,
    pub fragments: usize,
    frames: u64,
}

struct TrackSim {
    tiles: TileSystem,
    noise: Vec<f32>,
}

struct Passes {
    fragments: Option<usize>,
    system_ms: f32,
    task_ms: f32,
}

pub fn drop_world_box<'a>(files: impl IntoIterator<Item = &'a SubdFile>) -> (Vec3, Vec3) {
    let (mut min, mut max) = (Vec3::ZERO, Vec3::ZERO);
    for node in files.into_iter().flat_map(|f| &f.nodes).filter(|n| n.children == [0, 0]) {
        let (c, h) = (Vec3::from_array(node.center), Vec3::from_array(node.half_extents));
        min = min.min(c - h);
        max = max.max(c + h);
    }
    (min, max)
}

impl SnowTracks {
    pub fn set_noise(&mut self, noise: Noise) {
        if let Some(sim) = self.sim.settle() {
            sim.noise = noise.tile_sum();
        }
    }

    pub fn new(min: Vec3, max: Vec3, noise: Noise, images: &mut Assets<Image>) -> (SnowTracks, SnowTrackTextures) {
        SnowTracks::from_ahead(TracksAhead::build(min, max, &noise), images)
    }

    pub fn from_ahead(ahead: TracksAhead, images: &mut Assets<Image>) -> (SnowTracks, SnowTrackTextures) {
        let TracksAhead { tiles, noise, lookup, array, params, lookup_width } = ahead;
        let textures =
            SnowTrackTextures { lookup: lookup.into_handle(images), tiles: array.into_handle(images), params };
        let tracks = SnowTracks {
            sim: FrameTask::new(TrackSim { tiles, noise }),
            lookup_width,
            history: None,
            ticks: 0,
            update_ms: 0.0,
            fragments: 0,
            frames: 0,
        };
        (tracks, textures)
    }
}

pub struct TracksAhead {
    tiles: TileSystem,
    noise: Vec<f32>,
    lookup: LevelImage,
    array: LevelImage,
    params: SnowTrackParams,
    lookup_width: u32,
}

impl TracksAhead {
    pub fn build(min: Vec3, max: Vec3, noise: &Noise) -> TracksAhead {
        let tiles = TileSystem::new(min, max);
        let (w, h) = (tiles.size.x as u32, tiles.size.y as u32);
        let mut lookup = Image::new(
            Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            TextureDimension::D2,
            vec![0xFF; (w * h * 2) as usize],
            TextureFormat::R16Uint,
            RenderAssetUsages::RENDER_WORLD,
        );
        lookup.sampler = bevy::image::ImageSampler::nearest();
        let texel_bytes: Vec<u8> = UNTOUCHED.repeat(TEXELS * TEXELS * MAX_TILES);
        let mut array = Image::new(
            Extent3d { width: TEXELS as u32, height: TEXELS as u32, depth_or_array_layers: MAX_TILES as u32 },
            TextureDimension::D2,
            texel_bytes,
            TextureFormat::Rgba8Unorm,
            RenderAssetUsages::RENDER_WORLD,
        );
        array.texture_view_descriptor =
            Some(TextureViewDescriptor { dimension: Some(TextureViewDimension::D2Array), ..default() });
        array.sampler = crate::render::fx_states::sampler(
            "terrain_default",
            "prelight_gbuffer_4SnowTypes_2BaseSurfaces",
            "normalAndAccumulatedDispMapSampler",
        )
        .and_then(|s| s.image_sampler())
        .unwrap_or_else(|_| bevy::image::ImageSampler::linear());
        let params = SnowTrackParams {
            tile: Vec4::new(1.0 / TILE_WORLD, tiles.offset.x as f32, tiles.offset.y as f32, 0.0),
            size: Vec4::new(w as f32, h as f32, 0.0, 0.0),
        };
        TracksAhead {
            tiles,
            noise: noise.tile_sum(),
            lookup: LevelImage::built(lookup, None),
            array: LevelImage::built(array, None),
            params,
            lookup_width: w,
        }
    }

    pub fn queue(&mut self, uploads: &mut LevelUploads, images: &mut Assets<Image>) {
        self.lookup.queue(uploads, images);
        self.array.queue(uploads, images);
    }
}

pub struct SnowTracksPlugin;

impl Plugin for SnowTracksPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(ExtractResourcePlugin::<SnowTrackGpu>::default());
        if std::env::var("SSX_SNOW_TRACKS").is_ok_and(|v| v == "off") {
            return;
        }
        app.add_systems(PostUpdate, update.after(bevy::transform::TransformSystems::Propagate))
            .add_systems(Last, finish);
        if let Some(render) = app.get_sub_app_mut(RenderApp) {
            render.add_systems(Render, upload.in_set(RenderSystems::PrepareResources));
        }
    }
}

pub fn gpu_resource(textures: &SnowTrackTextures, tracks: &SnowTracks) -> SnowTrackGpu {
    SnowTrackGpu {
        lookup: textures.lookup.clone(),
        tiles: textures.tiles.clone(),
        lookup_width: tracks.lookup_width,
        uploads: Arc::new(Mutex::new(Vec::new())),
    }
}

fn record_step(tracks: &mut SnowTracks, ride: &Ride) {
    let s = &ride.state;
    if tracks.history.is_some_and(|h| h.step == ride.steps) {
        return;
    }
    let orientation = (s.traj_orientation * Quat::from_rotation_y(-std::f32::consts::FRAC_PI_2)).normalize();
    let active = s.contact.is_some() && !ride.rewind_backwards && s.weights[SNOW_COVERAGE] > 0.0;
    let before = tracks.history.filter(|h| ride.steps > h.step);
    let mut c = before.map_or(
        Caster {
            orientation,
            prev_orientation: orientation,
            contact: Vec3::NAN,
            prev_contact: Vec3::NAN,
            normal: Vec3::Y,
            depth: 0.0,
            spray: Vec3::ZERO,
        },
        |h| h.caster,
    );
    c.prev_orientation = c.orientation;
    c.orientation = orientation;
    if active && let Some(g) = &s.contact {
        c.prev_contact = if c.contact.is_nan() { g.contact } else { c.contact };
        c.contact = g.contact;
        c.normal = g.normal;
        c.depth = s.surface.snow_depth - g.distance - s.carving.riding_height;
        c.spray = s.spray.carving;
    }
    tracks.history = Some(History { step: ride.steps, caster: c, active });
}

#[allow(clippy::too_many_arguments)]
fn update(
    ride: Option<Res<Ride>>,
    tracks: Option<ResMut<SnowTracks>>,
    gpu: Option<Res<SnowTrackGpu>>,
    roots: Query<Entity, With<RiderRoot>>,
    children: Query<&Children>,
    parts: Query<(&Mesh3d, &GlobalTransform, Option<&SkinnedMesh>, &InheritedVisibility)>,
    joints: Query<&GlobalTransform>,
    meshes: Res<Assets<Mesh>>,
    bindposes: Res<Assets<SkinnedMeshInverseBindposes>>,
) {
    let (Some(ride), Some(mut tracks), Some(gpu)) = (ride, tracks, gpu) else { return };
    let start = Instant::now();
    record_step(&mut tracks, &ride);
    let target = (ride.render_time.max(0.0) * RENDER_HZ) as u64;
    let n = target.saturating_sub(tracks.ticks).min(4);
    tracks.ticks = target;
    if n == 0 {
        return;
    }
    let caster = tracks.history.filter(|h| h.active && !h.caster.contact.is_nan()).map(|h| h.caster);
    let triangles = caster.map(|_| {
        let (mut positions, mut tris) = (Vec::new(), Vec::new());
        for root in &roots {
            rider_triangles(root, &children, &parts, &joints, &meshes, &bindposes, &mut positions, &mut tris);
        }
        (positions, tris)
    });
    let uploads = gpu.uploads.clone();
    let system_ms = start.elapsed().as_secs_f32() * 1000.0;
    // PORT(choice): the frame's displacement passes (`VelDispMap` and `DispMap` from the rider's triangles, the tiles'
    // passes, the texel copies queued) run as a task of Bevy's compute pool beside the frame's other systems, joined by
    // `finish` in `Last`: the same work in the same order on the same tiles. Run in the system they held PostUpdate's
    // later systems and command syncs behind them; the console ran them in its `DisplacementRenderObj`.
    tracks.sim.spawn(move |sim| {
        let start = Instant::now();
        let disp = caster.zip(triangles.as_ref()).map(|(c, (positions, tris))| {
            let t = CasterTransforms::new(&c);
            let (vel, fragments) = crate::render::snow_tracks::raster::draw(positions, tris, &t);
            (disp_map(&vel), fragments)
        });
        let mut changes = Changes::default();
        let TrackSim { tiles, noise } = sim;
        for _ in 0..n {
            tiles.begin_frame(&mut changes);
            if let (Some(c), Some((d, _))) = (caster, &disp) {
                tiles.update(&c, d, noise, &mut changes);
            }
        }
        queue_uploads(tiles, &changes, &uploads);
        Passes { fragments: disp.map(|(_, f)| f), system_ms, task_ms: start.elapsed().as_secs_f32() * 1000.0 }
    });
}

// PORT(choice): it runs on the main thread (`NonSendMarker`): a compute thread blocked on the passes could be the one
// they wait for; the console had no task pool.
fn finish(_main_thread: NonSendMarker, tracks: Option<ResMut<SnowTracks>>) {
    let Some(mut tracks) = tracks else { return };
    if !tracks.sim.pending() {
        return;
    }
    let tracks = &mut *tracks;
    let Some(slices) = tracks.sim.settle().map(|s| s.tiles.slices_in_use()) else { return };
    let Some(passes) = tracks.sim.take_done() else { return };
    if let Some(f) = passes.fragments {
        tracks.fragments = f;
    }
    let ms = passes.system_ms + passes.task_ms;
    tracks.update_ms = if tracks.update_ms == 0.0 { ms } else { tracks.update_ms + 0.05 * (ms - tracks.update_ms) };
    tracks.frames += 1;
    if tracks.frames.is_multiple_of(300) {
        info!(
            "snow tracks: {:.2} ms/frame (smoothed), {} VelDispMap fragments, {} slices in use",
            tracks.update_ms, tracks.fragments, slices
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn rider_triangles(
    root: Entity,
    children: &Query<&Children>,
    parts: &Query<(&Mesh3d, &GlobalTransform, Option<&SkinnedMesh>, &InheritedVisibility)>,
    joints: &Query<&GlobalTransform>,
    meshes: &Assets<Mesh>,
    bindposes: &Assets<SkinnedMeshInverseBindposes>,
    positions: &mut Vec<Vec3>,
    triangles: &mut Vec<[u32; 3]>,
) {
    for e in children.iter_descendants(root) {
        let Ok((mesh, global, skin, visible)) = parts.get(e) else { continue };
        let Some(mesh) = meshes.get(&mesh.0).filter(|_| visible.get()) else { continue };
        let Some(VertexAttributeValues::Float32x3(pos)) = mesh.attribute(Mesh::ATTRIBUTE_POSITION) else { continue };
        let base = positions.len() as u32;
        match skin {
            Some(skin) => match skinned(mesh, pos, skin, joints, bindposes) {
                Some(w) => positions.extend(w),
                None => continue,
            },
            None => positions.extend(pos.iter().map(|&p| global.transform_point(Vec3::from(p)))),
        }
        let tri = |t: &[u32]| [base + t[0], base + t[1], base + t[2]];
        match mesh.indices() {
            Some(Indices::U16(ix)) => {
                triangles.extend(ix.as_chunks::<3>().0.iter().map(|t| tri(&[t[0] as u32, t[1] as u32, t[2] as u32])))
            }
            Some(Indices::U32(ix)) => triangles.extend(ix.as_chunks::<3>().0.iter().map(|t| tri(t))),
            None => triangles.extend((0..pos.len() as u32 / 3).map(|t| tri(&[3 * t, 3 * t + 1, 3 * t + 2]))),
        }
    }
}

fn skinned(
    mesh: &Mesh,
    pos: &[[f32; 3]],
    skin: &SkinnedMesh,
    joints: &Query<&GlobalTransform>,
    bindposes: &Assets<SkinnedMeshInverseBindposes>,
) -> Option<Vec<Vec3>> {
    let inverse = bindposes.get(&skin.inverse_bindposes)?;
    let mats: Vec<Mat4> = skin
        .joints
        .iter()
        .zip(inverse.iter())
        .map(|(&j, b)| joints.get(j).ok().map(|g| g.to_matrix() * *b))
        .collect::<Option<_>>()?;
    let Some(VertexAttributeValues::Uint16x4(index)) = mesh.attribute(Mesh::ATTRIBUTE_JOINT_INDEX) else { return None };
    let Some(VertexAttributeValues::Float32x4(weight)) = mesh.attribute(Mesh::ATTRIBUTE_JOINT_WEIGHT) else {
        return None;
    };
    Some(
        pos.iter()
            .zip(index)
            .zip(weight)
            .map(|((&p, j), w)| {
                let p = Vec3::from(p);
                (0..4).fold(Vec3::ZERO, |acc, k| {
                    let m = mats.get(j[k] as usize).copied().unwrap_or(Mat4::IDENTITY);
                    acc + w[k] * m.transform_point3(p)
                })
            })
            .collect(),
    )
}

fn queue_uploads(tiles: &TileSystem, changes: &Changes, uploads: &Mutex<Vec<Upload>>) {
    if changes.lookup.is_empty() && changes.slices.is_empty() {
        return;
    }
    let Ok(mut q) = uploads.lock() else { return };
    q.extend(changes.lookup.iter().map(|&(index, value)| Upload::Lookup { index, value }));
    for &(slice, r) in &changes.slices {
        let mut bytes = Vec::with_capacity((r.x1 - r.x0) * (r.y1 - r.y0) * 4);
        for y in r.y0..r.y1 {
            let row = (slice as usize * TEXELS + y) * TEXELS;
            for t in &tiles.slices[row + r.x0..row + r.x1] {
                bytes.extend_from_slice(t);
            }
        }
        let (x, y, w, h) = (r.x0 as u32, r.y0 as u32, (r.x1 - r.x0) as u32, (r.y1 - r.y0) as u32);
        q.push(Upload::Slice { layer: slice as u32, x, y, w, h, bytes });
    }
}

fn upload(gpu: Option<Res<SnowTrackGpu>>, images: Res<RenderAssets<GpuImage>>, queue: Res<RenderQueue>) {
    let Some(gpu) = gpu else { return };
    let (Some(lookup), Some(tiles)) = (images.get(&gpu.lookup), images.get(&gpu.tiles)) else { return };
    let Ok(mut pending) = gpu.uploads.lock() else { return };
    for u in pending.drain(..) {
        match u {
            Upload::Lookup { index, value } => {
                let (x, y) = (index as u32 % gpu.lookup_width, index as u32 / gpu.lookup_width);
                queue.write_texture(
                    TexelCopyTextureInfo {
                        texture: &lookup.texture,
                        mip_level: 0,
                        origin: Origin3d { x, y, z: 0 },
                        aspect: TextureAspect::All,
                    },
                    &value.to_le_bytes(),
                    TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(2), rows_per_image: Some(1) },
                    Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
                );
            }
            Upload::Slice { layer, x, y, w, h, bytes } => {
                queue.write_texture(
                    TexelCopyTextureInfo {
                        texture: &tiles.texture,
                        mip_level: 0,
                        origin: Origin3d { x, y, z: layer },
                        aspect: TextureAspect::All,
                    },
                    &bytes,
                    TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(4 * w), rows_per_image: Some(h) },
                    Extent3d { width: w, height: h, depth_or_array_layers: 1 },
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ssx_formats::subd::SubdNode;

    fn node(center: [f32; 3], half_extents: [f32; 3], children: [u16; 2]) -> SubdNode {
        SubdNode { center, half_extents, index: 0, leaf_count: if children == [0, 0] { 1 } else { 2 }, children }
    }

    #[test]
    fn drop_box_is_the_leaf_union_with_the_origin() {
        let file = SubdFile {
            nodes: vec![
                node([50.0, 0.0, 0.0], [99.0, 99.0, 99.0], [1, 2]),
                node([20.0, 5.0, -10.0], [5.0, 1.0, 2.0], [0, 0]),
                node([40.0, 8.0, 30.0], [3.0, 2.0, 4.0], [0, 0]),
            ],
            leaf_mesh: vec![0, 1],
            meshes: Vec::new(),
        };
        let (min, max) = drop_world_box([&file]);
        assert_eq!(min, Vec3::new(0.0, 0.0, -12.0));
        assert_eq!(max, Vec3::new(43.0, 10.0, 34.0));
    }
}
