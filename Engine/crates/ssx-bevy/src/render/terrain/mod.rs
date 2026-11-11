pub mod look;
pub mod material;
pub mod textures;

mod map;
pub use map::{MapTerrain, map_textured};

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{Context, Result, bail};
use bevy::asset::RenderAssetUsages;
use bevy::mesh::Indices;
use bevy::prelude::{
    App, Assets, Color, ColorToComponents, Commands, Handle, Image, IntoScheduleConfigs, Mesh, Mesh3d, MeshMaterial3d,
    Mix, Name, Res, ResMut, Resource, StandardMaterial, Vec3, World, default, info, resource_exists, warn,
};
use bevy::render::render_resource::PrimitiveTopology;
use ssx_assets::subdiv::{GAME_LEVELS, SubdLevel};
use ssx_assets::terrain::{TriMesh, cage_trimesh, is_hidden_face, load_track_files};
use ssx_assets::terrain_vertex;
use ssx_formats::subd::SubdMesh;

use crate::render::lighting::game::GameLightAssets;
use crate::render::snow_tracks::SnowTracks;
use crate::render::snow_tracks::model::Noise;
use crate::render::terrain::look::{TrackLooks, find_big_root};
use crate::render::terrain::material::{PreparedLooks, TerrainFades, TerrainMaterial};

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
    layer_counts: (usize, usize),
    snow_box: (Vec3, Vec3),
}

impl LoadedTrack {
    pub fn vertices(&self) -> impl Iterator<Item = Vec3> + '_ {
        self.pieces.iter().flat_map(|p| p.mesh.positions.iter().map(|&p| Vec3::from_array(p)))
    }
}

struct Piece {
    label: String,
    mesh: TriMesh,
    layers: Vec<MeshLayers>,
    groups: Vec<u8>,
    parts: Vec<(usize, usize)>,
}

fn face_group(unk_a: u8) -> u8 {
    if unk_a & 2 != 0 {
        1
    } else if unk_a & 4 != 0 {
        2
    } else {
        0
    }
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

#[derive(Resource, Clone, Debug)]
pub struct ShadingLabel(pub String);

pub fn load_textured(track_dir: &Path, track: &str, big_root: Option<PathBuf>) -> Result<TexturedTerrain> {
    let start = Instant::now();
    let big_root = big_root
        .or_else(|| find_big_root(track_dir))
        .context("no extracted archive root (Game-Data/Extracted/big with cache1/data/xenon/attribdb) found")?;
    let looks = TrackLooks::load(&big_root, track)
        .with_context(|| format!("loading the terrain materials of {track} from {}", big_root.display()))?;
    let prepared = PreparedLooks::decode(&big_root, &looks.looks)?;
    let noise = load_snow_noise(&big_root)?;
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
    bounds: Bounds,
    snow_box: (Vec3, Vec3),
    shading: Option<Shading>,
}

pub fn track_name(dir: &Path) -> String {
    std::fs::canonicalize(dir)
        .ok()
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
        .unwrap_or_else(|| dir.display().to_string())
}

pub fn load(dir: &Path, subdivide: u32, drop_files: Option<&[String]>) -> Result<LoadedTrack> {
    let start = Instant::now();
    let files = load_track_files(dir, drop_files)?;
    if files.is_empty() {
        bail!("no *_mesh_subd.subd files in {}", dir.display());
    }

    let meshes = files.iter().map(|f| f.subd.meshes.len()).sum();
    let snow_depth = find_big_root(dir)
        .context("no extracted archive root (Game-Data/Extracted/big) found")
        .and_then(|root| terrain_vertex::snow_depths(&root));
    let displacement = match &snow_depth {
        Ok(d) => format!("displaced along N as the exe does, SnowDepth {d:?} m"),
        Err(e) => format!("CPU displacement not applied: {e:#}"),
    };
    let surface = match subdivide as usize {
        0 => format!("control cage as stored; {displacement}"),
        GAME_LEVELS => format!("catmull-clark x{subdivide} per patch (game rules); {displacement}"),
        _ => format!("catmull-clark x{subdivide} per patch (game uses x{GAME_LEVELS}); {displacement}"),
    };
    let mut layer_counts = (0, 0);
    let mut pieces = Vec::with_capacity(files.len());
    for file in &files {
        let label = file.path.file_stem().unwrap_or_default().to_string_lossy().into_owned();
        let mut piece =
            Piece { label, mesh: TriMesh::default(), layers: Vec::new(), groups: Vec::new(), parts: Vec::new() };
        for subd_mesh in &file.subd.meshes {
            let fan = |unk_a: u8, len: usize| std::iter::repeat_n(face_group(unk_a), len.saturating_sub(2));
            let mut mesh = match subdivide {
                0 => {
                    let drawn = subd_mesh.faces.iter().filter(|f| !is_hidden_face(f.unk_a));
                    piece.groups.extend(drawn.flat_map(|f| fan(f.unk_a, subd_mesh.face_vertices(f).count())));
                    let mut mesh = cage_trimesh(subd_mesh);
                    let loops: Vec<Vec<u32>> =
                        subd_mesh.faces.iter().map(|f| subd_mesh.face_vertices(f).map(u32::from).collect()).collect();
                    mesh.normals = game_vertex_normals(&mesh.positions, &loops);
                    mesh
                }
                levels => {
                    let level = SubdLevel::from_subd(subd_mesh).refined(levels as usize);
                    let drawn = level.faces.iter().filter(|f| !is_hidden_face(f.unk_a));
                    piece.groups.extend(drawn.flat_map(|f| fan(f.unk_a, usize::from(f.len))));
                    let mut mesh = level.to_trimesh();
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
            layer_counts.0 += usize::from(layers.dropped > 0);
            layer_counts.1 += usize::from(layers.base == 0);
            piece.layers.extend(std::iter::repeat_n(layers, mesh.positions.len()));
            piece.parts.push((mesh.positions.len(), mesh.triangle_count()));
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
        pieces.push(piece);
    }
    let snow_box = crate::render::snow_tracks::drop_world_box(files.iter().map(|f| &f.subd));
    let bounds = pieces
        .iter()
        .filter_map(|p| p.mesh.bounds())
        .map(|(lo, hi)| Bounds { min: Vec3::from_array(lo), max: Vec3::from_array(hi) })
        .reduce(Bounds::union)
        .with_context(|| format!("the terrain in {} has no vertices", dir.display()))?;

    let name = track_name(dir);
    let stats = TrackStats {
        name,
        files: files.len(),
        meshes,
        vertices: pieces.iter().map(|p| p.mesh.positions.len()).sum(),
        triangles: pieces.iter().map(|p| p.mesh.triangle_count()).sum(),
        load_secs: start.elapsed().as_secs_f32(),
        surface,
    };
    Ok(LoadedTrack { stats, bounds, pieces, layer_counts, snow_box })
}

pub fn register(app: &mut App) {
    app.add_systems(crate::shell::level::LevelStartup, spawn_terrain.run_if(resource_exists::<PendingTerrain>));
    crate::shell::level::scoped::<PendingTerrain>(app);
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
    }
    world.insert_resource(track.stats);
    world.insert_resource(ShadingLabel(label));
    world.insert_resource(PendingTerrain {
        pieces: track.pieces,
        bounds: track.bounds,
        snow_box: track.snow_box,
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
) {
    let bounds = pending.bounds;
    let snow_box = pending.snow_box;
    info!(
        "Loaded track {} in {:.2}s: {} files, {} meshes, {} vertices, {} triangles; bounds {:.0} .. {:.0}",
        stats.name, stats.load_secs, stats.files, stats.meshes, stats.vertices, stats.triangles, bounds.min, bounds.max,
    );

    let pieces = std::mem::take(&mut pending.pieces);
    match pending.shading.take() {
        Some(Shading::Textured(textured)) => {
            let (tracks, track_textures) = SnowTracks::new(snow_box.0, snow_box.1, textured.noise, &mut images);
            commands.insert_resource(crate::render::snow_tracks::gpu_resource(&track_textures, &tracks));
            commands.insert_resource(tracks);
            let handles: Vec<[Handle<TerrainMaterial>; 3]> = textured
                .prepared
                .into_materials(&mut images, &light, &track_textures)
                .into_iter()
                .map(|set| set.map(|m| terrain_materials.add(m)))
                .collect();
            let fallback = materials.add(StandardMaterial { base_color: Color::srgb(0.5, 0.5, 0.5), ..default() });
            let mut warm = std::collections::HashSet::new();
            for piece in pieces {
                let set = textured.by_file.get(&piece.label);
                if set.is_none() {
                    warn!("{}: no terrain material in the track vault; drawn grey", piece.label);
                }
                let label = piece.label.clone();
                // TODO(guess): the views cull by Bevy's frustum test on the refined, displaced mesh's box; settle: the visibility test before the terrain draw 0x829BCF90 and the ShadowObjects bin's per-cascade cull.
                for (k, (mesh, layers, groups)) in per_mesh(piece).into_iter().enumerate() {
                    for (group, (part, part_layers)) in split_by_group(&mesh, &layers, &groups) {
                        let name = Name::new(format!("{label} mesh {k} group {group}"));
                        let mesh = textured_mesh(part, &part_layers);
                        let attributes: Vec<_> = mesh.attributes().map(|(a, _)| a.id).collect();
                        let part = meshes.add(mesh);
                        match set {
                            Some(&i) => {
                                let handle = handles[i][group].clone();
                                if warm.insert((handle.id(), attributes)) {
                                    commands.spawn((
                                        Mesh3d(part.clone()),
                                        MeshMaterial3d(handle.clone()),
                                        crate::render::warm_up::draw(),
                                    ));
                                }
                                commands.spawn((name, Mesh3d(part), MeshMaterial3d(handle)))
                            }
                            None => commands.spawn((name, Mesh3d(part), MeshMaterial3d(fallback.clone()))),
                        };
                    }
                }
            }
        }
        Some(Shading::Vertex(mode)) => {
            let material =
                materials.add(StandardMaterial { base_color: Color::WHITE, perceptual_roughness: 0.85, ..default() });
            for Piece { label, mesh, .. } in pieces {
                commands.spawn((
                    Name::new(label),
                    Mesh3d(meshes.add(to_bevy_mesh(mesh, bounds.min.y, bounds.max.y, mode))),
                    MeshMaterial3d(material.clone()),
                ));
            }
        }
        None => {}
    }
    commands.remove_resource::<PendingTerrain>();
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

fn per_mesh(piece: Piece) -> Vec<(TriMesh, Vec<MeshLayers>, Vec<u8>)> {
    let (mut v0, mut t0) = (0, 0);
    let mut out = Vec::with_capacity(piece.parts.len());
    for &(nv, nt) in &piece.parts {
        let m = &piece.mesh;
        let mesh = TriMesh {
            positions: m.positions[v0..v0 + nv].to_vec(),
            normals: m.normals[v0..v0 + nv].to_vec(),
            attributes: m.attributes[v0..v0 + nv].to_vec(),
            indices: m.indices[3 * t0..3 * (t0 + nt)].iter().map(|&i| i - v0 as u32).collect(),
        };
        out.push((mesh, piece.layers[v0..v0 + nv].to_vec(), piece.groups[t0..t0 + nt].to_vec()));
        v0 += nv;
        t0 += nt;
    }
    out
}

fn split_by_group(mesh: &TriMesh, layers: &[MeshLayers], groups: &[u8]) -> Vec<(usize, (TriMesh, Vec<MeshLayers>))> {
    let mut out = Vec::new();
    for group in 0..3u8 {
        let mut part = TriMesh::default();
        let mut part_layers = Vec::new();
        let mut remap = vec![u32::MAX; mesh.positions.len()];
        let (triangles, _) = mesh.indices.as_chunks::<3>();
        for (tri, _) in triangles.iter().zip(groups).filter(|(_, g)| **g == group) {
            for &v in tri {
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
        }
        if !part.indices.is_empty() {
            out.push((usize::from(group), (part, part_layers)));
        }
    }
    out
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
