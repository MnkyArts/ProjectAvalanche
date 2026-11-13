use std::collections::BTreeMap;
use std::path::Path;
use std::time::Instant;

use anyhow::{Context, Result};
use bevy::prelude::Vec3;
use ssx_assets::terrain::TriMesh;
use ssx_assets::terrain_vertex;
use ssx_content::ContentId;
use ssx_content::map::{DEFAULT_RAIL_MATERIAL, DEFAULT_TERRAIN_MATERIAL, MapDefinition};

use super::look::{TerrainLook, TrackLooks};
use super::material::PreparedLooks;
use super::{
    Bounds, LoadedTrack, MeshLayers, Piece, TexturedTerrain, TrackStats, game_vertex_normals, load_snow_noise,
};

// PORT(choice): a map's rail has no baked occlusion, so a7 is 1.0 (no occlusion); an author's occlusion for a rail node
// (an attribute like the terrain's `_AVA_TERRAIN_B`) would set it; the disc's a7 is baked per vertex (101 values on the
// rail meshes, 1.0 the most common: 22 996 of terrain_rail_red_mat's 49 160 vertices).
const RAIL_WEIGHTS: [f32; 8] = [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 1.0];

const RAIL_MATERIAL_A6_ZERO: &str = "terrain_rail_damn_pipe_mat";

// TODO(guess): a rail drawn with a terrain material that no disc rail names (any `terrain_default.fx` material other than the four rail ones) gets the default rail's a6 1: the disc's 55 rail files name only the four rail materials, so there is no rail of another material to read; settle: a disc rail with a non-rail terrain material, or the terrain shader's use of a6 (terrain_default.fxo) showing which value such a rail needs.
fn rail_weights(material: &ContentId) -> [f32; 8] {
    let mut w = RAIL_WEIGHTS;
    if material.namespace() == ssx_content::BASE_NAMESPACE && material.path() == RAIL_MATERIAL_A6_ZERO {
        w[6] = 0.0;
    }
    w
}

pub struct MapTerrain {
    pieces: Vec<(Piece, ContentId)>,
    cage_box: Option<(Vec3, Vec3)>,
    vertices: usize,
    triangles: usize,
}

impl MapTerrain {
    pub fn build(def: &MapDefinition, big_root: &Path) -> Result<MapTerrain> {
        let depth = terrain_vertex::snow_depths(big_root).context("the terrain's snow depths")?;
        let default_terrain: ContentId = DEFAULT_TERRAIN_MATERIAL.parse().context("the default terrain material")?;
        let default_rail: ContentId = DEFAULT_RAIL_MATERIAL.parse().context("the default rail material")?;
        let mut out = MapTerrain { pieces: Vec::new(), cage_box: None, vertices: 0, triangles: 0 };
        for (k, t) in def.terrain.iter().enumerate() {
            let mesh = TriMesh {
                positions: t.positions.clone(),
                normals: Vec::new(),
                attributes: t.weights.clone(),
                indices: t.triangles.iter().flatten().copied().collect(),
            };
            let (lo, hi) = cage_box(&t.positions);
            out.cage_box = Some(out.cage_box.map_or((lo, hi), |(a, b)| (a.min(lo), b.max(hi))));
            let material = t.material.clone().unwrap_or_else(|| default_terrain.clone());
            out.push(format!("map terrain {k}"), mesh, &depth, material);
        }
        for (k, r) in def.rails.iter().enumerate().filter(|(_, r)| !r.hidden) {
            let positions: Vec<[f32; 3]> = r.triangles.iter().flat_map(|t| t.corners).collect();
            let n = positions.len();
            let material = r.material.clone().unwrap_or_else(|| default_rail.clone());
            let mesh = TriMesh {
                positions,
                normals: Vec::new(),
                attributes: vec![rail_weights(&material); n],
                indices: (0..n as u32).collect(),
            };
            out.push(format!("map rail {k}"), mesh, &depth, material);
        }
        Ok(out)
    }

    fn push(&mut self, label: String, mut mesh: TriMesh, depth: &[f32; 6], material: ContentId) {
        let loops: Vec<Vec<u32>> = mesh.indices.as_chunks::<3>().0.iter().map(|t| t.to_vec()).collect();
        mesh.normals = game_vertex_normals(&mesh.positions, &loops);
        for ((p, n), a) in mesh.positions.iter_mut().zip(&mesh.normals).zip(&mesh.attributes) {
            *p = terrain_vertex::displaced_position(*p, *n, a, depth);
        }
        let layers = MeshLayers::of_weights(&mesh.attributes);
        let (v, t) = (mesh.positions.len(), mesh.triangle_count());
        (self.vertices, self.triangles) = (self.vertices + v, self.triangles + t);
        let piece = Piece { label, layers: vec![layers; v], groups: vec![0; t], parts: vec![(v, t)], mesh };
        self.pieces.push((piece, material));
    }

    pub fn counts(&self) -> (usize, usize) {
        (self.vertices, self.triangles)
    }

    pub fn into_track(self, name: &str) -> Result<(LoadedTrack, BTreeMap<String, ContentId>)> {
        let start = Instant::now();
        let snow_box = self.cage_box.context("a new track without terrain draws no ground")?;
        let mut track = LoadedTrack {
            stats: TrackStats {
                name: name.to_string(),
                files: 0,
                meshes: 0,
                vertices: 0,
                triangles: 0,
                load_secs: 0.0,
                surface: "a map's terrain meshes as stored (triangles; no subdivision); displaced along N as the exe \
                          does"
                    .into(),
            },
            bounds: Bounds { min: snow_box.0, max: snow_box.1 },
            pieces: Vec::new(),
            layer_counts: (0, 0),
            snow_box,
        };
        let materials = self.add_to(&mut track);
        track.stats.load_secs = start.elapsed().as_secs_f32();
        Ok((track, materials))
    }

    pub fn add_to(self, track: &mut LoadedTrack) -> BTreeMap<String, ContentId> {
        let mut materials = BTreeMap::new();
        for (piece, material) in self.pieces {
            if let Some((lo, hi)) = piece.mesh.bounds() {
                let b = Bounds { min: Vec3::from_array(lo), max: Vec3::from_array(hi) };
                track.bounds = track.bounds.union(b);
            }
            track.stats.files += 1;
            track.stats.meshes += 1;
            track.stats.vertices += piece.mesh.positions.len();
            track.stats.triangles += piece.mesh.triangle_count();
            track.layer_counts.1 += usize::from(piece.layers.first().is_some_and(|l| l.base == 0));
            materials.insert(piece.label.clone(), material);
            track.pieces.push(piece);
        }
        materials
    }
}

fn cage_box(positions: &[[f32; 3]]) -> (Vec3, Vec3) {
    positions.iter().map(|&p| Vec3::from_array(p)).fold((Vec3::ZERO, Vec3::ZERO), |(lo, hi), p| (lo.min(p), hi.max(p)))
}

impl MeshLayers {
    fn of_weights(weights: &[[f32; 8]]) -> MeshLayers {
        let mut used = [false; 6];
        let mut base = 0;
        for w in weights {
            let mut a = *w;
            terrain_vertex::normalise_layer_weights(&mut a);
            for (k, x) in terrain_vertex::layer_weights(&a).into_iter().enumerate() {
                used[k] |= x > terrain_vertex::LAYER_USED_THRESHOLD;
            }
            if w[5] < 1.0 {
                base |= if w[6] >= 0.5 { 2 } else { 1 };
            }
        }
        let used: Vec<u8> = (0..6u8).filter(|&k| used[k as usize]).collect();
        let mut lanes = [Self::NO_LAYER; 4];
        for (lane, &layer) in lanes.iter_mut().zip(&used) {
            *lane = layer;
        }
        let count = used.len().min(terrain_vertex::SHADER_LAYERS);
        MeshLayers { lanes, count: count as u8, base, dropped: (used.len() - count) as u8 }
    }
}

pub fn map_textured(
    big_root: &Path,
    base_track: Option<&str>,
    pieces: &BTreeMap<String, ContentId>,
) -> Result<TexturedTerrain> {
    let start = Instant::now();
    let content = ssx_assets::content::GameContent::of(big_root);
    let mut looks = match base_track {
        Some(t) => TrackLooks::load(big_root, t)?,
        None => TrackLooks { looks: Vec::new(), by_file: BTreeMap::new() },
    };
    for (label, id) in pieces {
        let m = content.map_terrain_material(id).with_context(|| format!("{label}: its terrain material"))?;
        let index = match looks.looks.iter().position(|l| l.material == m.name) {
            Some(i) => i,
            None => {
                looks.looks.push(TerrainLook::from_vault(big_root, &m)?);
                looks.looks.len() - 1
            }
        };
        looks.by_file.insert(label.clone(), index);
    }
    let prepared = PreparedLooks::decode(big_root, &looks.looks)?;
    let noise = load_snow_noise(big_root)?;
    let summary = format!(
        "textured (a map's terrain: materials + textures loaded in {:.2}s)\n{}",
        start.elapsed().as_secs_f32(),
        looks.summary()
    );
    Ok(TexturedTerrain { prepared, by_file: looks.by_file, summary, noise })
}

#[cfg(test)]
mod tests {
    use ssx_formats::subd::{SubdFace, SubdMesh, SubdVertex};

    use super::*;

    fn cage(weights: &[[f32; 8]]) -> SubdMesh {
        let n = weights.len() as u16;
        let vertices = weights.iter().map(|&attributes| SubdVertex { position: [0.0; 3], attributes }).collect();
        let edges = (0..n).map(|k| [k, (k + 1) % n]).collect();
        let faces =
            (0..n / 3).map(|f| SubdFace { unk_a: 0, reversed_mask: 0, edges: vec![3 * f, 3 * f + 1, 3 * f + 2] });
        SubdMesh {
            version: 0,
            unk_header: [0x20, 1, 2],
            vertices,
            edges,
            faces: faces.collect(),
            unk_face_flags: Vec::new(),
            unk_tail: [0, 0xFFFF],
        }
    }

    #[test]
    fn a_map_meshs_layers_are_the_classifiers() {
        let powder = [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0];
        let groomed_ice = [0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 1.0];
        let covered = [0.3, 0.3, 0.3, 0.3, 0.3, 1.0, 0.0, 1.0];
        let crud = [0.0, 0.0, 0.0, 0.0, 0.9, 0.2, 0.0, 0.5];
        for set in [
            vec![powder; 3],
            vec![RAIL_WEIGHTS; 3],
            vec![rail_weights(&"ssx:material/terrain_rail_damn_pipe_mat".parse().unwrap()); 3],
            vec![powder, groomed_ice, covered],
            vec![covered, covered, crud, powder, groomed_ice, covered],
        ] {
            assert_eq!(MeshLayers::of_weights(&set), MeshLayers::classify(&cage(&set)), "{set:?}");
        }
        let a6 = |m: &str| rail_weights(&m.parse().unwrap())[6];
        assert_eq!(a6("ssx:material/terrain_rail_damn_pipe_mat"), 0.0);
        for m in [
            "ssx:material/terrain_rail_red_mat",
            "ssx:material/terrain_default_mat",
            "pkg:material/terrain_rail_damn_pipe_mat",
        ] {
            assert_eq!(a6(m), 1.0, "{m}");
        }
        let windswept_crusty = [0.5, 0.5, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0];
        let five = MeshLayers::of_weights(&[crud, groomed_ice, windswept_crusty]);
        assert_eq!(
            (five.lanes, five.count, five.dropped),
            ([0, 1, 3, 4], 4, 1),
            "five layers: four lanes, one dropped"
        );
    }

    #[test]
    fn the_cage_box_grows_from_the_origin() {
        let (lo, hi) = cage_box(&[[10.0, 5.0, 2.0], [12.0, 7.0, 3.0]]);
        assert_eq!((lo, hi), (Vec3::ZERO, Vec3::new(12.0, 7.0, 3.0)));
    }
}
