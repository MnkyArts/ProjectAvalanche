use glam::{Quat, Vec3};

use crate::bvh::{Bvh, segment_hits_box, sphere_hits_box};
use crate::surface::{self, TERRAIN_SURFACE_ID, Weights};

mod primitives;
mod queries;

pub use primitives::*;
pub use queries::*;

#[derive(Clone, Copy, Debug)]
pub struct Cluster {
    pub min: Vec3,
    pub max: Vec3,
    pub entry_min: Vec3,
    pub entry_max: Vec3,
    pub vert_start: u32,
    pub vert_count: u16,
    pub tri_start: u32,
    pub tri_count: u16,
    pub surface_id: u32,
    pub terrain: bool,
    pub class: u8,
}

pub const RIDABLE_CLASS_MASK: u32 = 0x802;

pub fn class_byte(group: u32) -> u8 {
    let b = group as u8;
    if b < 32 { b } else { 0 }
}

pub fn class_passes(mask: u32, class: u8) -> bool {
    mask.checked_shr(u32::from(class)).unwrap_or(0) & 1 == 0
}

impl Cluster {
    pub fn is_terrain(&self) -> bool {
        self.terrain
    }
}

#[derive(Clone, Debug, Default)]
pub struct BuildStats {
    pub terrain_files: usize,
    pub terrain_meshes: usize,
    pub kept_faces: usize,
    pub hidden_faces: usize,
    pub triangle_units: usize,
    pub dropped_units: usize,
    pub prop_instances: usize,
    pub prop_files: usize,
    pub prop_missing: usize,
    pub prop_missing_paths: Vec<String>,
    pub prop_cylinders: usize,
    pub prop_packages: Vec<String>,
    pub prop_decision: Option<String>,
    pub terrain_class_faces: [usize; 32],
    pub terrain_class_meshes: [usize; 32],
    pub prop_class_clusters: [usize; 32],
}

impl BuildStats {
    pub fn add(&mut self, o: &BuildStats) {
        self.terrain_files += o.terrain_files;
        self.terrain_meshes += o.terrain_meshes;
        self.kept_faces += o.kept_faces;
        self.hidden_faces += o.hidden_faces;
        self.triangle_units += o.triangle_units;
        self.dropped_units += o.dropped_units;
        self.prop_instances += o.prop_instances;
        self.prop_files += o.prop_files;
        self.prop_missing += o.prop_missing;
        self.prop_missing_paths.extend(o.prop_missing_paths.iter().cloned());
        self.prop_cylinders += o.prop_cylinders;
        self.prop_packages.extend(o.prop_packages.iter().cloned());
        if o.prop_decision.is_some() {
            self.prop_decision.clone_from(&o.prop_decision);
        }
        for i in 0..32 {
            self.terrain_class_faces[i] += o.terrain_class_faces[i];
            self.terrain_class_meshes[i] += o.terrain_class_meshes[i];
            self.prop_class_clusters[i] += o.prop_class_clusters[i];
        }
    }

    pub fn histogram(h: &[usize; 32]) -> String {
        let parts: Vec<String> =
            h.iter().enumerate().filter(|(_, n)| **n > 0).map(|(c, n)| format!("{c}:{n}")).collect();
        if parts.is_empty() { "-".into() } else { parts.join(" ") }
    }
}

#[derive(Clone, Debug, Default)]
pub struct CollisionWorld {
    pub vertices: Vec<Vec3>,
    pub normals: Vec<Vec3>,
    pub nibbles: Vec<u32>,
    pub triangles: Vec<[u8; 3]>,
    pub clusters: Vec<Cluster>,
    pub bvh: Bvh,
    pub stats: BuildStats,
    pub boundary: Option<crate::rider::boundary::BoundaryGrid>,
    pub edge_start: Vec<u32>,
    pub tri_edges: Vec<TriEdges>,
    pub cluster_material: Vec<[f32; 3]>,
    pub quad_units: Vec<u32>,
    pub cluster_entity: Vec<u32>,
    pub entities: Vec<PropEntity>,
    pub cluster_frame: Vec<Quat>,
}

#[derive(Clone, Debug, Default)]
pub struct PropEntity {
    pub object: String,
    pub audio: Option<ssx_assets::track_objects::AudioName>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TriEdges {
    pub flags: u32,
    pub edge_cos: [f32; 3],
}

pub const TERRAIN_TRIANGLE_FLAGS: u32 = 0x12;

pub use ssx_assets::object_collision::DEFAULT_PART_MATERIAL;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hit {
    pub t: f32,
    pub point: Vec3,
    pub normal: Vec3,
    pub contact_normal: Vec3,
    pub cluster: u32,
    pub tri: u32,
    pub bary: [f32; 3],
}

#[derive(Clone, Copy, Debug)]
pub struct Contact {
    pub point: Vec3,
    pub normal: Vec3,
    pub depth: f32,
    pub cluster: u32,
    pub tri: u32,
    pub bary: [f32; 3],
}

impl CollisionWorld {
    pub fn push_cluster(
        &mut self,
        vertices: &[Vec3],
        normals: Option<&[Vec3]>,
        nibbles: Option<&[u32]>,
        triangles: &[[u8; 3]],
        surface_id: u32,
        class: u8,
    ) {
        if triangles.is_empty() || vertices.is_empty() {
            return;
        }
        debug_assert!(vertices.len() <= 256 && triangles.len() <= u16::MAX as usize);
        let (mut min, mut max) = (Vec3::splat(f32::INFINITY), Vec3::splat(f32::NEG_INFINITY));
        for v in vertices {
            min = min.min(*v);
            max = max.max(*v);
        }
        self.clusters.push(Cluster {
            min,
            max,
            entry_min: min,
            entry_max: max,
            vert_start: self.vertices.len() as u32,
            vert_count: vertices.len() as u16,
            tri_start: self.triangles.len() as u32,
            tri_count: triangles.len() as u16,
            surface_id,
            terrain: nibbles.is_some(),
            class,
        });
        self.vertices.extend_from_slice(vertices);
        match normals {
            Some(n) => self.normals.extend_from_slice(n),
            None => self.normals.extend(std::iter::repeat_n(Vec3::ZERO, vertices.len())),
        }
        match nibbles {
            Some(n) => self.nibbles.extend_from_slice(n),
            None => self.nibbles.extend(std::iter::repeat_n(0, vertices.len())),
        }
        self.triangles.extend_from_slice(triangles);
        self.edge_start.push(u32::MAX);
        self.cluster_material.push(DEFAULT_PART_MATERIAL);
        self.quad_units.push(0);
        self.cluster_entity.push(0);
        self.cluster_frame.push(Quat::IDENTITY);
    }

    pub fn add_entity(&mut self, e: PropEntity) -> u32 {
        self.entities.push(e);
        self.entities.len() as u32
    }

    pub fn set_cluster_entity(&mut self, id: u32) {
        if let Some(e) = self.cluster_entity.last_mut() {
            *e = id;
        }
    }

    pub fn set_cluster_entry_box(&mut self, min: Vec3, max: Vec3) {
        if let Some(c) = self.clusters.last_mut() {
            (c.entry_min, c.entry_max) = (min, max);
        }
    }

    pub fn set_cluster_frame(&mut self, frame: Quat) {
        if let Some(f) = self.cluster_frame.last_mut() {
            *f = frame;
        }
    }

    pub fn entity(&self, id: u32) -> Option<&PropEntity> {
        id.checked_sub(1).and_then(|i| self.entities.get(i as usize))
    }

    pub fn set_quad_units(&mut self, mask: u32) {
        if let Some(m) = self.quad_units.last_mut() {
            *m = mask;
        }
    }

    pub fn query_order(&self, ci: u32) -> impl Iterator<Item = u32> + '_ {
        let c = &self.clusters[ci as usize];
        let mask = self.quad_units.get(ci as usize).copied().unwrap_or(0);
        let n = u32::from(c.tri_count);
        let mut i = 0u32;
        let mut pending: Option<u32> = None;
        std::iter::from_fn(move || {
            if let Some(p) = pending.take() {
                return Some(p);
            }
            if i >= n {
                return None;
            }
            let t = i;
            if t < 32 && mask & (1 << t) != 0 && t + 1 < n {
                pending = Some(t);
                i += 2;
                Some(t + 1)
            } else {
                i += 1;
                Some(t)
            }
        })
    }

    pub fn set_triangle_edges(&mut self, edges: &[TriEdges], material: [f32; 3]) {
        let (Some(start), Some(mat), Some(c)) =
            (self.edge_start.last_mut(), self.cluster_material.last_mut(), self.clusters.last())
        else {
            return;
        };
        debug_assert_eq!(edges.len(), c.tri_count as usize);
        *start = self.tri_edges.len() as u32;
        *mat = material;
        self.tri_edges.extend_from_slice(edges);
    }

    pub fn triangle_edges(&self, cluster: u32, tri: u32) -> TriEdges {
        let c = &self.clusters[cluster as usize];
        match self.edge_start.get(cluster as usize) {
            Some(&s) if s != u32::MAX => self.tri_edges[(s + tri - c.tri_start) as usize],
            _ => TriEdges { flags: TERRAIN_TRIANGLE_FLAGS, edge_cos: [0.0; 3] },
        }
    }

    pub fn clusters_in_box(&self, min: Vec3, max: Vec3) -> Vec<u32> {
        let mut out = Vec::new();
        let hit = |a: Vec3, b: Vec3| a.cmple(max).all() && b.cmpge(min).all();
        self.bvh.traverse(hit, |i| {
            let c = &self.clusters[i as usize];
            if hit(c.min, c.max) {
                out.push(i);
            }
        });
        out.sort_unstable();
        out
    }

    pub fn append(&mut self, other: CollisionWorld) {
        let (v0, t0) = (self.vertices.len() as u32, self.triangles.len() as u32);
        self.clusters.extend(other.clusters.into_iter().map(|mut c| {
            c.vert_start += v0;
            c.tri_start += t0;
            c
        }));
        self.vertices.extend(other.vertices);
        self.normals.extend(other.normals);
        self.nibbles.extend(other.nibbles);
        self.triangles.extend(other.triangles);
        let e0 = self.tri_edges.len() as u32;
        self.edge_start.extend(other.edge_start.into_iter().map(|s| if s == u32::MAX { s } else { s + e0 }));
        self.tri_edges.extend(other.tri_edges);
        self.cluster_material.extend(other.cluster_material);
        self.quad_units.extend(other.quad_units);
        let n0 = self.entities.len() as u32;
        self.cluster_entity.extend(other.cluster_entity.into_iter().map(|e| if e == 0 { 0 } else { e + n0 }));
        self.entities.extend(other.entities);
        self.cluster_frame.extend(other.cluster_frame);
        self.stats.add(&other.stats);
    }

    pub fn finish(&mut self) {
        let boxes: Vec<(Vec3, Vec3)> = self.clusters.iter().map(|c| (c.min, c.max)).collect();
        self.bvh = Bvh::build(&boxes);
    }

    pub fn triangle_count(&self) -> usize {
        self.triangles.len()
    }

    pub fn memory_bytes(&self) -> usize {
        self.vertices.len() * (12 + 12 + 4)
            + self.triangles.len() * 3
            + self.clusters.len() * std::mem::size_of::<Cluster>()
            + self.bvh.memory_bytes()
    }

    pub fn bounds(&self) -> Option<(Vec3, Vec3)> {
        let n = self.bvh.nodes.first()?;
        Some((n.min, n.max))
    }

    pub fn triangle_vertices(&self, cluster: u32, tri: u32) -> [usize; 3] {
        let c = &self.clusters[cluster as usize];
        self.triangles[tri as usize].map(|i| c.vert_start as usize + i as usize)
    }

    fn corners(&self, cluster: &Cluster, tri: usize) -> [Vec3; 3] {
        self.triangles[tri].map(|i| self.vertices[cluster.vert_start as usize + i as usize])
    }
}

#[cfg(test)]
mod tests;
