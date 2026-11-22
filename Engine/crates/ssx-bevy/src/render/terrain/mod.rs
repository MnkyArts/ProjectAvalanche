mod draw;
mod lods;
pub mod look;
pub mod material;
mod path;
pub(crate) mod proxy;
pub mod textures;

pub use lods::{Reach, TerrainLods, reach};

#[derive(Resource, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ViewDistance(pub ssx_assets::port_options::ViewDistance);

impl ViewDistance {
    pub fn meters(self) -> f32 {
        self.0.meters() as f32
    }
}

mod map;
pub use map::{MapTerrain, map_textured};

use std::collections::BTreeMap;
use std::path::Path;
use std::time::Instant;

use anyhow::{Context, Result, bail};
use bevy::asset::RenderAssetUsages;
use bevy::camera::primitives::{Aabb, MeshAabb};
use bevy::mesh::{Indices, MeshVertexAttributeId};
use bevy::prelude::{
    App, Assets, Color, ColorToComponents, Commands, Handle, Image, IntoScheduleConfigs, Mesh, Mesh3d, MeshMaterial3d,
    Mix, Name, Res, ResMut, Resource, StandardMaterial, Vec3, World, default, info, resource_exists, warn,
};
use bevy::render::render_resource::PrimitiveTopology;
use ssx_assets::lod::{LOD_COUNT, LodList, face_group, lod_lists};
use ssx_assets::object_mesh::ObjectMesh;
use ssx_assets::subdiv::{GAME_LEVELS, SubdLevel};
use ssx_assets::terrain::{TriMesh, cage_trimesh, is_hidden_face, load_track_files};
use ssx_assets::terrain_vertex;
use ssx_formats::subd::SubdMesh;

use crate::render::lighting::game::GameLightAssets;
use crate::render::snow_tracks::model::Noise;
use crate::render::snow_tracks::{SnowTracks, TracksAhead};
use crate::render::terrain::look::TrackLooks;
use crate::render::terrain::material::{PreparedLooks, SIMPLE_SET, TerrainFades, TerrainMaterial};

const LOW_TINT: Color = Color::srgb(0.55, 0.62, 0.74);
const HIGH_TINT: Color = Color::WHITE;

#[derive(Resource, Clone, Debug)]
pub struct TrackStats {
    pub name: String,
    pub files: usize,
    pub meshes: usize,
    pub vertices: usize,
    pub triangles: usize,
    pub load_secs: f32,
    pub surface: String,
}

#[derive(Clone, Copy, Debug)]
pub struct Bounds {
    pub min: Vec3,
    pub max: Vec3,
}

impl Bounds {
    pub fn center(&self) -> Vec3 {
        (self.min + self.max) * 0.5
    }

    pub fn size(&self) -> Vec3 {
        self.max - self.min
    }

    fn union(self, other: Bounds) -> Bounds {
        Bounds { min: self.min.min(other.min), max: self.max.max(other.max) }
    }
}

pub struct LoadedTrack {
    pub stats: TrackStats,
    pub bounds: Bounds,
    pieces: Vec<Piece>,
    draws: Option<Vec<PieceDraws>>,
    layer_counts: (usize, usize),
    snow_box: (Vec3, Vec3),
    proxy: Option<ObjectMesh>,
    proxy_cut: Option<proxy::ProxyCut>,
    tracks: Option<TracksAhead>,
}

impl LoadedTrack {
    pub fn vertices(&self) -> impl Iterator<Item = Vec3> + '_ {
        self.pieces.iter().flat_map(|p| p.mesh.positions.iter().map(|&p| Vec3::from_array(p)))
    }

    // PORT(choice): the proxy's groups cut into their meshes, each draw's vertex attributes and box (`MeshAabb`) and
    // the snow tracks' tiles, noise sums and two textures are made on the level's loading thread with the draws, not in
    // the spawn's frame on the game thread, where the terrain's spawn took 9.1-35.5 ms (alps0101, three runs) and
    // 24.4-35.0 ms (Mount Eddie, two) and the proxy's 2.1-2.7 and 10.7-12.6 ms (development laptop, untraced marks);
    // with them made here 1.4-3.0 and 5.2-9.8 ms, 0.2-0.4 ms (ten runs). The spawn makes the same meshes, entities and
    // textures from the same data in the same order (the meshes' places in the allocator's slabs hashed equal). The
    // console's level loading is not decoded.
    pub fn cut_draws(&mut self, shading: &Shading) {
        if let Shading::Textured(textured) = shading {
            let pieces = std::mem::take(&mut self.pieces);
            self.draws = Some(crate::load_pool::ordered_owned("terrain draws", pieces, cut_piece));
            self.proxy_cut = self.proxy.take().map(proxy::cut);
            self.tracks = Some(TracksAhead::build(self.snow_box.0, self.snow_box.1, &textured.noise));
        }
    }

    pub fn queue_textures(&mut self, uploads: &mut crate::render::uploads::LevelUploads, images: &mut Assets<Image>) {
        if let Some(t) = self.tracks.as_mut() {
            t.queue(uploads, images);
        }
    }
}

struct Piece {
    label: String,
    mesh: TriMesh,
    layers: Vec<MeshLayers>,
    groups: Vec<u8>,
    parts: Vec<(usize, usize)>,
    lods: Vec<Option<UnitLods>>,
}

type LodDraws = [Vec<(usize, PartMesh)>; LOD_COUNT];

struct PartMesh {
    mesh: Mesh,
    attributes: Vec<MeshVertexAttributeId>,
    aabb: Option<Aabb>,
}

impl PartMesh {
    fn new(mesh: Mesh) -> PartMesh {
        let attributes = mesh.attributes().map(|(a, _)| a.id).collect();
        let aabb = mesh.get_aabb();
        PartMesh { mesh, attributes, aabb }
    }
}

struct PieceDraws {
    label: String,
    meshes: Vec<(LodDraws, Option<UnitLods>)>,
}

struct UnitLods {
    lists: [LodList; LOD_COUNT],
    stream: (Vec3, f32),
    centre: Vec3,
    half: Vec3,
    radius: f32,
    record: (usize, usize),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColorMode {
    Height,
    Material,
}

pub enum Shading {
    Vertex(ColorMode),
    Textured(TexturedTerrain),
}

pub struct TexturedTerrain {
    prepared: PreparedLooks,
    by_file: BTreeMap<String, usize>,
    summary: String,
    noise: Noise,
}

impl Shading {
    pub fn queue_textures(&mut self, uploads: &mut crate::render::uploads::LevelUploads, images: &mut Assets<Image>) {
        if let Shading::Textured(t) = self {
            t.prepared.queue_textures(uploads, images);
        }
    }
}

#[derive(Resource, Clone, Debug)]
pub struct ShadingLabel(pub String);

pub fn load_textured(big_root: Option<&Path>, track: &str) -> Result<TexturedTerrain> {
    let start = Instant::now();
    let big_root =
        big_root.context("no extracted archive root (Game-Data/Extracted/big with cache1/data/xenon/attribdb)")?;
    let looks = TrackLooks::load(big_root, track)
        .with_context(|| format!("loading the terrain materials of {track} from {}", big_root.display()))?;
    let prepared = PreparedLooks::decode(big_root, &looks.looks)?;
    let noise = load_snow_noise(big_root)?;
    let summary =
        format!("textured (materials + textures loaded in {:.2}s)\n{}", start.elapsed().as_secs_f32(), looks.summary());
    Ok(TexturedTerrain { prepared, by_file: looks.by_file, summary, noise })
}

pub(crate) fn load_snow_noise(big_root: &Path) -> Result<Noise> {
    let id =
        ssx_assets::content::textures::texture_id("worlds/textures/snow_noise_displace").context("the noise's id")?;
    let chain = crate::render::terrain::textures::rgba_chain(big_root, &id, &ssx_content::texture::Want::PLANE)?;
    let level = &chain.levels[0];
    let size = level.width as usize;
    anyhow::ensure!(level.width == level.height, "{id}: not square");
    Ok(Noise { size, texels: level.rgba.as_chunks::<4>().0.iter().map(|p| f32::from(p[0]) / 255.0).collect() })
}

#[derive(Resource)]
struct PendingTerrain {
    pieces: Vec<Piece>,
    draws: Option<Vec<PieceDraws>>,
    bounds: Bounds,
    snow_box: (Vec3, Vec3),
    tracks: Option<TracksAhead>,
    shading: Option<Shading>,
}

#[derive(Resource)]
struct PendingProxy(Option<proxy::ProxyCut>);

pub fn track_name(dir: &Path) -> String {
    std::fs::canonicalize(dir)
        .ok()
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
        .unwrap_or_else(|| dir.display().to_string())
}

pub fn load(dir: &Path, big_root: Option<&Path>, subdivide: u32, drop_files: Option<&[String]>) -> Result<LoadedTrack> {
    let start = Instant::now();
    let files = load_track_files(dir, drop_files)?;
    if files.is_empty() {
        bail!("no *_mesh_subd.subd files in {}", dir.display());
    }

    let name = track_name(dir);
    let meshes = files.iter().map(|f| f.subd.meshes.len()).sum();
    let snow_depth = big_root.context("no extracted archive root").and_then(terrain_vertex::snow_depths);
    let displacement = match &snow_depth {
        Ok(d) => format!("displaced along N as the exe does, SnowDepth {d:?} m"),
        Err(e) => {
            warn!("{name}: the terrain's snow-depth displacement is not applied: {e:#}");
            format!("CPU displacement not applied: {e:#}")
        }
    };
    let surface = match subdivide as usize {
        0 => format!("control cage as stored; {displacement}"),
        GAME_LEVELS => format!("catmull-clark x{subdivide} per patch (game rules); {displacement}"),
        _ => format!("catmull-clark x{subdivide} per patch (game uses x{GAME_LEVELS}); {displacement}"),
    };
    let indexed: Vec<(usize, &ssx_assets::terrain::TerrainPiece)> = files.iter().enumerate().collect();
    let built =
        crate::load_pool::ordered("terrain refine", &indexed, |&(sorted, file)| -> Result<(Piece, (usize, usize))> {
            let mut counts = (0, 0);
            let label = file.path.file_stem().unwrap_or_default().to_string_lossy().into_owned();
            let name = file.path.file_name().unwrap_or_default().to_string_lossy();
            let list = match drop_files {
                Some(list) => list
                    .iter()
                    .position(|f| f.eq_ignore_ascii_case(&name))
                    .with_context(|| format!("{label}: not in the drop's Meshes list"))?,
                // PORT(choice): without a drop (the asset viewer's whole track) the files' sorted order; the game
                // always streams a drop's `Meshes` list.
                None => sorted,
            };
            let mut walk = vec![usize::MAX; file.subd.meshes.len()];
            let tree = file.subd.meshes_in_tree_order().with_context(|| format!("{label}: the mesh tree"))?;
            for (rank, &mesh) in tree.iter().enumerate() {
                if let Some(w) = walk.get_mut(mesh) {
                    *w = rank;
                }
            }
            let mut piece = Piece {
                label,
                mesh: TriMesh::default(),
                layers: Vec::new(),
                groups: Vec::new(),
                parts: Vec::new(),
                lods: Vec::new(),
            };
            for (m, subd_mesh) in file.subd.meshes.iter().enumerate() {
                let fan = |unk_a: u8, len: usize| std::iter::repeat_n(face_group(unk_a), len.saturating_sub(2));
                let mut lods = None;
                let mut mesh = match subdivide {
                    0 => {
                        let drawn = subd_mesh.faces.iter().filter(|f| !is_hidden_face(f.unk_a));
                        piece.groups.extend(drawn.flat_map(|f| fan(f.unk_a, subd_mesh.face_vertices(f).count())));
                        let mut mesh = cage_trimesh(subd_mesh);
                        let loops: Vec<Vec<u32>> = subd_mesh
                            .faces
                            .iter()
                            .map(|f| subd_mesh.face_vertices(f).map(u32::from).collect())
                            .collect();
                        mesh.normals = game_vertex_normals(&mesh.positions, &loops);
                        mesh
                    }
                    levels => {
                        let cage = SubdLevel::from_subd(subd_mesh);
                        let level = cage.clone().refined(levels as usize);
                        let drawn = level.faces.iter().filter(|f| !is_hidden_face(f.unk_a));
                        piece.groups.extend(drawn.flat_map(|f| fan(f.unk_a, usize::from(f.len))));
                        let mut mesh = level.to_trimesh();
                        if levels as usize == GAME_LEVELS {
                            let lists = lod_lists(&cage, &level).with_context(|| format!("{}: LODs", piece.label))?;
                            let (lo, hi) = mesh.bounds().context("a refined mesh without vertices")?;
                            let (lo, hi) = (Vec3::from_array(lo), Vec3::from_array(hi));
                            let centre = (lo + hi) * 0.5;
                            let (cage_lo, cage_hi) = cage.vertices.iter().fold(
                                (Vec3::splat(f32::INFINITY), Vec3::splat(f32::NEG_INFINITY)),
                                |(lo, hi), v| {
                                    let p = Vec3::from_array(v.position.map(|c| c as f32));
                                    (lo.min(p), hi.max(p))
                                },
                            );
                            let stream = ((cage_lo + cage_hi) * 0.5, (cage_hi - cage_lo).length() * 0.5);
                            let half = hi - centre;
                            let record = (list, walk[m]);
                            lods = Some(UnitLods { lists, stream, centre, half, radius: half.length(), record });
                        }
                        mesh.normals = game_vertex_normals(&mesh.positions, &level.face_loops().collect::<Vec<_>>());
                        mesh
                    }
                };
                if let Ok(depth) = &snow_depth {
                    for ((p, n), a) in mesh.positions.iter_mut().zip(&mesh.normals).zip(&mesh.attributes) {
                        *p = terrain_vertex::displaced_position(*p, *n, a, depth);
                    }
                }
                let layers = MeshLayers::classify(subd_mesh);
                counts.0 += usize::from(layers.dropped > 0);
                counts.1 += usize::from(layers.base == 0);
                piece.layers.extend(std::iter::repeat_n(layers, mesh.positions.len()));
                piece.parts.push((mesh.positions.len(), mesh.triangle_count()));
                piece.lods.push(lods);
                piece.mesh.append(&mesh);
            }
            if piece.groups.len() != piece.mesh.triangle_count() {
                bail!(
                    "{}: {} face-group entries for {} triangles",
                    piece.label,
                    piece.groups.len(),
                    piece.mesh.triangle_count()
                );
            }
            Ok((piece, counts))
        });
    let mut layer_counts = (0, 0);
    let mut pieces = Vec::with_capacity(files.len());
    for b in built {
        let (piece, counts) = b?;
        (layer_counts.0, layer_counts.1) = (layer_counts.0 + counts.0, layer_counts.1 + counts.1);
        pieces.push(piece);
    }
    let snow_box = crate::render::snow_tracks::drop_world_box(files.iter().map(|f| &f.subd));
    let bounds = pieces
        .iter()
        .filter_map(|p| p.mesh.bounds())
        .map(|(lo, hi)| Bounds { min: Vec3::from_array(lo), max: Vec3::from_array(hi) })
        .reduce(Bounds::union)
        .with_context(|| format!("the terrain in {} has no vertices", dir.display()))?;

    let proxy = match (subdivide as usize == GAME_LEVELS, big_root) {
        (false, _) => None,
        (true, None) => {
            warn!("{name}: no extracted archive root; no terrain proxy");
            None
        }
        (true, Some(root)) => proxy::load(root, dir, &name)
            .inspect_err(|e| warn!("{name}: terrain proxy not loaded: {e:#}"))
            .ok()
            .flatten(),
    };
    let stats = TrackStats {
        name,
        files: files.len(),
        meshes,
        vertices: pieces.iter().map(|p| p.mesh.positions.len()).sum(),
        triangles: pieces.iter().map(|p| p.mesh.triangle_count()).sum(),
        load_secs: start.elapsed().as_secs_f32(),
        surface,
    };
    Ok(LoadedTrack { stats, bounds, pieces, draws: None, layer_counts, snow_box, proxy, proxy_cut: None, tracks: None })
}

pub fn register(app: &mut App) {
    app.add_systems(crate::shell::level::LevelStartup, spawn_terrain.run_if(resource_exists::<PendingTerrain>));
    app.add_systems(crate::shell::level::LevelStartup, spawn_proxy.run_if(resource_exists::<PendingProxy>));
    lods::register(app);
    path::register(app);
    app.add_systems(bevy::prelude::PostUpdate, proxy::follow_view_distance);
    crate::shell::level::scoped::<PendingTerrain>(app);
    crate::shell::level::scoped::<PendingProxy>(app);
    crate::shell::level::scoped::<TrackStats>(app);
    crate::shell::level::scoped::<ShadingLabel>(app);
    crate::shell::level::scoped::<TerrainFades>(app);
    crate::shell::level::scoped::<crate::render::snow_tracks::SnowTracks>(app);
    crate::shell::level::scoped::<crate::render::snow_tracks::SnowTrackGpu>(app);
}

pub fn insert(world: &mut World, track: LoadedTrack, shading: Shading) {
    let label = match &shading {
        Shading::Vertex(ColorMode::Height) => "height tint".to_string(),
        Shading::Vertex(ColorMode::Material) => "material-attribute debug palette (not the game's shading)".to_string(),
        Shading::Textured(textured) => {
            let (dropped, no_base) = track.layer_counts;
            format!(
                "{}\nsubd meshes: {dropped} use >4 snow layers (layers past the 4th dropped, as in the game), \
                 {no_base} show no base surface (drawn as snow)",
                textured.summary
            )
        }
    };
    if matches!(shading, Shading::Textured(_)) {
        world.insert_resource(TerrainFades(true));
        if let Some(cut) = track.proxy_cut.or_else(|| track.proxy.map(proxy::cut)) {
            world.insert_resource(PendingProxy(Some(cut)));
        }
    }
    world.insert_resource(track.stats);
    world.insert_resource(ShadingLabel(label));
    world.insert_resource(PendingTerrain {
        pieces: track.pieces,
        draws: track.draws,
        bounds: track.bounds,
        snow_box: track.snow_box,
        tracks: track.tracks,
        shading: Some(shading),
    });
}

#[allow(clippy::too_many_arguments)]
fn spawn_terrain(
    mut commands: Commands,
    mut pending: ResMut<PendingTerrain>,
    stats: Res<TrackStats>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut terrain_materials: ResMut<Assets<TerrainMaterial>>,
    mut images: ResMut<Assets<Image>>,
    light: Res<GameLightAssets>,
    mut uploads: Option<ResMut<crate::render::uploads::LevelUploads>>,
) {
    let bounds = pending.bounds;
    let snow_box = pending.snow_box;
    info!(
        "Loaded track {} in {:.2}s: {} files, {} meshes, {} vertices, {} triangles; bounds {:.0} .. {:.0}; {}",
        stats.name,
        stats.load_secs,
        stats.files,
        stats.meshes,
        stats.vertices,
        stats.triangles,
        bounds.min,
        bounds.max,
        stats.surface,
    );

    let pieces = std::mem::take(&mut pending.pieces);
    let draws = pending.draws.take();
    match pending.shading.take() {
        Some(Shading::Textured(textured)) => {
            let (tracks, track_textures) = match pending.tracks.take() {
                Some(ahead) => SnowTracks::from_ahead(ahead, &mut images),
                None => SnowTracks::new(snow_box.0, snow_box.1, textured.noise, &mut images),
            };
            commands.insert_resource(crate::render::snow_tracks::gpu_resource(&track_textures, &tracks));
            commands.insert_resource(tracks);
            let handles: Vec<[Handle<TerrainMaterial>; SIMPLE_SET + 1]> = textured
                .prepared
                .into_materials(&mut images, &light, &track_textures)
                .into_iter()
                .map(|set| set.map(|m| terrain_materials.add(m)))
                .collect();
            let fallback = materials.add(StandardMaterial { base_color: Color::srgb(0.5, 0.5, 0.5), ..default() });
            let mut warm = std::collections::HashSet::new();
            let mut lods = TerrainLods::default();
            let mut path = path::TerrainPath::default();
            let mut grey = None;
            let mut draws = draws.unwrap_or_default();
            draws.extend(pieces.into_iter().map(cut_piece));
            for PieceDraws { label, meshes: cut } in draws {
                let set = textured.by_file.get(&label);
                if set.is_none() {
                    warn!("{label}: no terrain material in the track vault; drawn grey");
                }
                let carriers: Option<[usize; SIMPLE_SET + 1]> = set.map(|&i| {
                    std::array::from_fn(|g| path.carrier(path::CarrierMaterial::Terrain(handles[i][g].clone())))
                });
                // TODO(guess): the views cull by Bevy's frustum test on the refined, displaced mesh's box; settle: the visibility test before the terrain draw 0x829BCF90 and the ShadowObjects bin's per-cascade cull.
                for (parts, unit) in cut {
                    let mut unit_parts: [Vec<(usize, usize)>; LOD_COUNT] = Default::default();
                    for (lod, parts) in parts.into_iter().enumerate() {
                        for (group, PartMesh { mesh, attributes, aabb }) in parts {
                            let part = crate::render::uploads::add_mesh(uploads.as_deref_mut(), &mut meshes, mesh);
                            let own = match &carriers {
                                Some(c) => c[group],
                                None => *grey
                                    .get_or_insert_with(|| path.carrier(path::CarrierMaterial::Grey(fallback.clone()))),
                            };
                            if let Some(&i) = set {
                                let simple = unit.as_ref().map(|_| handles[i][SIMPLE_SET].clone());
                                for h in std::iter::once(handles[i][group].clone()).chain(simple) {
                                    if warm.insert((h.id(), attributes.clone())) {
                                        commands.spawn((
                                            Mesh3d(part.clone()),
                                            MeshMaterial3d(h),
                                            crate::render::warm_up::draw(),
                                        ));
                                    }
                                }
                            }
                            let slot = unit.as_ref().map(|_| lods::TerrainPart::new(lods.len(), lod, group));
                            let index = path.add_part(part, aabb, slot, own);
                            unit_parts[lod].push((index, group));
                        }
                    }
                    if let Some(unit) = unit {
                        let triangles = unit.lists.each_ref().map(LodList::triangle_count);
                        let bounds = (unit.centre, unit.half, unit.radius);
                        lods.push(lods::TerrainUnit::new(
                            unit.stream,
                            bounds,
                            unit.record,
                            unit_parts,
                            carriers,
                            triangles,
                        ));
                    }
                }
            }
            path.spawn_carriers(&mut commands, || {
                crate::render::uploads::add_mesh(uploads.as_deref_mut(), &mut meshes, carrier_mesh())
            });
            commands.insert_resource(path);
            commands.insert_resource(lods);
        }
        Some(Shading::Vertex(mode)) => {
            let material =
                materials.add(StandardMaterial { base_color: Color::WHITE, perceptual_roughness: 0.85, ..default() });
            for Piece { label, mesh, .. } in pieces {
                let mesh = to_bevy_mesh(mesh, bounds.min.y, bounds.max.y, mode);
                commands.spawn((
                    Name::new(label),
                    Mesh3d(crate::render::uploads::add_mesh(uploads.as_deref_mut(), &mut meshes, mesh)),
                    MeshMaterial3d(material.clone()),
                ));
            }
        }
        None => {}
    }
    commands.remove_resource::<PendingTerrain>();
}

fn spawn_proxy(
    mut commands: Commands,
    mut pending: ResMut<PendingProxy>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<proxy::ProxyMaterial>>,
    mut uploads: Option<ResMut<crate::render::uploads::LevelUploads>>,
    view_distance: Option<Res<ViewDistance>>,
) {
    if let Some(mesh) = pending.0.take() {
        let (name, triangles) = (mesh.name.clone(), mesh.triangles);
        let mut meshes = (&mut *meshes, uploads.as_deref_mut());
        let reach = reach(view_distance.map_or(lods::CUT_OFF, |d| d.meters()));
        let groups = proxy::spawn(&mut commands, mesh, &mut meshes, &mut materials, &reach);
        info!("terrain proxy {name}: {triangles} triangles in {groups} groups");
    }
    commands.remove_resource::<PendingProxy>();
}

fn to_bevy_mesh(mesh: TriMesh, low_y: f32, high_y: f32, mode: ColorMode) -> Mesh {
    let colors: Vec<[f32; 4]> = match mode {
        ColorMode::Height => {
            let (low, high) = (LOW_TINT.to_linear(), HIGH_TINT.to_linear());
            let span = (high_y - low_y).max(f32::EPSILON);
            mesh.positions
                .iter()
                .map(|p| low.mix(&high, ((p[1] - low_y) / span).clamp(0.0, 1.0)).to_f32_array())
                .collect()
        }
        ColorMode::Material => mesh.attributes.iter().map(material_debug_color).collect(),
    };

    Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::default())
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, mesh.positions)
        .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, mesh.normals)
        .with_inserted_attribute(Mesh::ATTRIBUTE_COLOR, colors)
        .with_inserted_indices(Indices::U32(mesh.indices))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MeshLayers {
    lanes: [u8; 4],
    count: u8,
    base: u8,
    dropped: u8,
}

impl MeshLayers {
    const NO_LAYER: u8 = 7;

    fn classify(mesh: &SubdMesh) -> MeshLayers {
        let used = terrain_vertex::used_layers(mesh);
        let mut lanes = [Self::NO_LAYER; 4];
        for (lane, &layer) in lanes.iter_mut().zip(&used) {
            *lane = layer;
        }
        let mut base = 0;
        for face in mesh.faces.iter().filter(|f| f.unk_a & 1 == 0) {
            for v in mesh.face_vertices(face) {
                let Some(vertex) = mesh.vertices.get(usize::from(v)) else { continue };
                if vertex.attributes[5] < 1.0 {
                    base |= if vertex.attributes[6] >= 0.5 { 2 } else { 1 };
                }
            }
        }
        let count = used.len().min(terrain_vertex::SHADER_LAYERS);
        MeshLayers { lanes, count: count as u8, base, dropped: (used.len() - count) as u8 }
    }

    fn code(&self) -> f32 {
        let lanes = self.lanes.iter().enumerate().map(|(i, &l)| u32::from(l) << (3 * i)).sum::<u32>();
        (lanes | u32::from(self.base) << 12) as f32
    }
}

fn textured_mesh(mesh: TriMesh, layers: &[MeshLayers]) -> Mesh {
    let n = mesh.positions.len();
    let (mut color, mut uv0, mut uv1) = (Vec::with_capacity(n), Vec::with_capacity(n), Vec::with_capacity(n));
    let unorm = |b: [u8; 4]| b.map(|b| f32::from(b) / 255.0);
    for (a, layers) in mesh.attributes.iter().zip(layers) {
        let (c0, c1) = terrain_vertex::pack_colors(a, &layers.lanes[..usize::from(layers.count)]);
        let c1 = unorm(c1);
        color.push(unorm(c0));
        uv0.push([c1[0], c1[1]]);
        uv1.push([c1[2], layers.code()]);
    }
    Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::default())
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, mesh.positions)
        .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, mesh.normals)
        .with_inserted_attribute(Mesh::ATTRIBUTE_COLOR, color)
        .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, uv0)
        .with_inserted_attribute(Mesh::ATTRIBUTE_UV_1, uv1)
        .with_inserted_indices(Indices::U32(mesh.indices))
}

fn carrier_mesh() -> Mesh {
    Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::default())
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, vec![[0.0f32; 3]; 3])
        .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, vec![[0.0f32, 1.0, 0.0]; 3])
        .with_inserted_attribute(Mesh::ATTRIBUTE_COLOR, vec![[0.0f32; 4]; 3])
        .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, vec![[0.0f32; 2]; 3])
        .with_inserted_attribute(Mesh::ATTRIBUTE_UV_1, vec![[0.0f32; 2]; 3])
        .with_inserted_indices(Indices::U32(vec![0, 1, 2]))
}

fn game_vertex_normals(positions: &[[f32; 3]], faces: &[Vec<u32>]) -> Vec<[f32; 3]> {
    let mut sum = vec![Vec3::ZERO; positions.len()];
    let mut count = vec![0u32; positions.len()];
    let p = |i: u32| Vec3::from_array(positions[i as usize]);
    for face in faces {
        let n = match *face.as_slice() {
            [a, b, c] => (p(b) - p(a)).cross(p(c) - p(a)),
            [a, b, c, d] => (p(b) - p(d)).cross(p(c) - p(a)),
            _ => continue,
        };
        let unit = n.normalize_or_zero();
        for &i in face {
            sum[i as usize] += unit;
            count[i as usize] += 1;
        }
    }
    sum.iter().zip(&count).map(|(s, &k)| if k == 0 { [0.0, 1.0, 0.0] } else { (*s / k as f32).to_array() }).collect()
}

type Part = (TriMesh, Vec<MeshLayers>, Vec<u8>, Option<UnitLods>);

fn cut_piece(piece: Piece) -> PieceDraws {
    let label = piece.label.clone();
    let meshes = per_mesh(piece)
        .into_iter()
        .map(|(mesh, layers, groups, unit)| {
            let parts = match &unit {
                Some(unit) => lod_parts(&mesh, &layers, unit),
                None => {
                    let mut parts: [Vec<LodPart>; LOD_COUNT] = Default::default();
                    parts[0] = split_by_group(&mesh, &layers, &groups);
                    parts
                }
            };
            let parts = parts.map(|lod| {
                lod.into_iter()
                    .map(|(group, (part, part_layers))| (group, PartMesh::new(textured_mesh(part, &part_layers))))
                    .collect()
            });
            (parts, unit)
        })
        .collect();
    PieceDraws { label, meshes }
}

fn per_mesh(piece: Piece) -> Vec<Part> {
    let (mut v0, mut t0) = (0, 0);
    let mut out = Vec::with_capacity(piece.parts.len());
    let mut lods = piece.lods.into_iter();
    for &(nv, nt) in &piece.parts {
        let m = &piece.mesh;
        let mesh = TriMesh {
            positions: m.positions[v0..v0 + nv].to_vec(),
            normals: m.normals[v0..v0 + nv].to_vec(),
            attributes: m.attributes[v0..v0 + nv].to_vec(),
            indices: m.indices[3 * t0..3 * (t0 + nt)].iter().map(|&i| i - v0 as u32).collect(),
        };
        let unit = lods.next().flatten();
        out.push((mesh, piece.layers[v0..v0 + nv].to_vec(), piece.groups[t0..t0 + nt].to_vec(), unit));
        v0 += nv;
        t0 += nt;
    }
    out
}

fn split_by_group(mesh: &TriMesh, layers: &[MeshLayers], groups: &[u8]) -> Vec<(usize, (TriMesh, Vec<MeshLayers>))> {
    let (triangles, _) = mesh.indices.as_chunks::<3>();
    (0..3u8)
        .filter_map(|group| {
            let picked = triangles.iter().zip(groups).filter(|(_, g)| **g == group).flat_map(|(t, _)| t);
            let part = sub_mesh(mesh, layers, picked.copied());
            (!part.0.indices.is_empty()).then_some((usize::from(group), part))
        })
        .collect()
}

fn sub_mesh(mesh: &TriMesh, layers: &[MeshLayers], indices: impl Iterator<Item = u32>) -> (TriMesh, Vec<MeshLayers>) {
    let mut part = TriMesh::default();
    let mut part_layers = Vec::new();
    let mut remap = vec![u32::MAX; mesh.positions.len()];
    for v in indices {
        let v = v as usize;
        if remap[v] == u32::MAX {
            remap[v] = part.positions.len() as u32;
            part.positions.push(mesh.positions[v]);
            part.normals.push(mesh.normals[v]);
            part.attributes.push(mesh.attributes[v]);
            part_layers.push(layers[v]);
        }
        part.indices.push(remap[v]);
    }
    (part, part_layers)
}

type LodPart = (usize, (TriMesh, Vec<MeshLayers>));

fn lod_parts(mesh: &TriMesh, layers: &[MeshLayers], unit: &UnitLods) -> [Vec<LodPart>; LOD_COUNT] {
    std::array::from_fn(|lod| {
        let list = &unit.lists[lod];
        let (triangles, _) = list.indices.as_chunks::<3>();
        let mut covered = vec![false; triangles.len()];
        let mut out = Vec::new();
        for (g, range) in list.groups.iter().enumerate() {
            let Some((first, n)) = *range else { continue };
            let span = first / 3..first / 3 + n;
            let Some(picked) = triangles.get(span.clone()).filter(|t| !t.is_empty()) else { continue };
            covered[span].fill(true);
            out.push((g, sub_mesh(mesh, layers, picked.iter().flatten().copied())));
        }
        let rest = triangles.iter().zip(&covered).filter(|(_, c)| !**c).flat_map(|(t, _)| t);
        let rest = sub_mesh(mesh, layers, rest.copied());
        if !rest.0.indices.is_empty() {
            out.push((SIMPLE_SET, rest));
        }
        out
    })
}

fn material_debug_color(a: &[f32; 8]) -> [f32; 4] {
    const LAYERS: [[f32; 3]; 6] = [
        [1.0, 0.55, 0.55],
        [0.55, 1.0, 0.55],
        [0.55, 0.65, 1.0],
        [1.0, 1.0, 0.5],
        [0.85, 0.5, 1.0],
        [0.97, 0.97, 0.97],
    ];
    const BASE: [[f32; 3]; 2] = [[0.45, 0.32, 0.2], [0.3, 0.33, 0.38]];
    let mut w = [a[0], a[1], a[2], a[3], a[4], 0.0];
    w[5] = (1.0 - w[..5].iter().sum::<f32>()).max(0.0);
    let total = w.iter().sum::<f32>().max(f32::EPSILON);
    let mut snow = [0.0f32; 3];
    for (layer, weight) in LAYERS.iter().zip(w) {
        for (c, l) in snow.iter_mut().zip(layer) {
            *c += l * weight / total;
        }
    }
    let base = BASE[usize::from(a[6] >= 0.5)];
    let snow_amount = a[5].clamp(0.0, 1.0);
    let rgb: [f32; 3] = std::array::from_fn(|i| base[i] + (snow[i] - base[i]) * snow_amount);
    let lin = Color::srgb(rgb[0], rgb[1], rgb[2]).to_linear();
    [lin.red, lin.green, lin.blue, 1.0]
}

#[cfg(test)]
mod tests {
    use bevy::mesh::{MeshVertexBufferLayouts, VertexAttributeValues};

    use super::*;

    #[test]
    fn carrier_mesh_has_the_parts_layout() {
        let part = TriMesh {
            positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]],
            normals: vec![[0.0, 1.0, 0.0]; 3],
            attributes: vec![[0.25, 0.0, 0.0, 0.0, 0.0, 0.5, 0.0, 0.0]; 3],
            indices: vec![0, 1, 2],
        };
        let layers =
            MeshLayers { lanes: [0, 1, MeshLayers::NO_LAYER, MeshLayers::NO_LAYER], count: 2, base: 1, dropped: 0 };
        let part = textured_mesh(part, &[layers; 3]);
        let carrier = carrier_mesh();
        let mut layouts = MeshVertexBufferLayouts::default();
        let (a, b) =
            (part.get_mesh_vertex_buffer_layout(&mut layouts), carrier.get_mesh_vertex_buffer_layout(&mut layouts));
        assert_eq!(*a.0, *b.0);
        assert_eq!(part.primitive_topology(), carrier.primitive_topology());
        assert!(matches!((part.indices(), carrier.indices()), (Some(Indices::U32(_)), Some(Indices::U32(_)))));
        let Some(VertexAttributeValues::Float32x3(positions)) = carrier.attribute(Mesh::ATTRIBUTE_POSITION) else {
            panic!("the carrier mesh has no positions");
        };
        assert!(positions.iter().all(|p| *p == positions[0]));
    }
}
