use std::path::Path;

use anyhow::Result;
use glam::Vec3;
use ssx_assets::subdiv::{GAME_LEVELS, SubdLevel};
use ssx_formats::subd::SubdMesh;

use crate::collision::{BuildStats, CollisionWorld, class_byte};
use crate::surface::pack_terrain_nibbles;

pub const GRANULARITY: f32 = 0.008;
pub const QUANTISATION_SCALE: f32 = 125.0;

pub fn quantize(x: f32) -> i32 {
    (x * QUANTISATION_SCALE) as i32
}

pub fn dequantize(q: i32) -> f32 {
    q as f32 * GRANULARITY
}

fn corner(level: &SubdLevel, f: usize, i: usize) -> u32 {
    let face = &level.faces[f];
    let [a, b] = level.edges[face.edges[i] as usize];
    if face.reversed_mask >> i & 1 == 1 { b } else { a }
}

fn position(level: &SubdLevel, v: u32) -> Vec3 {
    Vec3::from_array(level.vertices[v as usize].position.map(|c| c as f32))
}

pub fn vertex_normals(level: &SubdLevel) -> Vec<Vec3> {
    let mut sum = vec![Vec3::ZERO; level.vertices.len()];
    let mut count = vec![0u32; level.vertices.len()];
    for (f, face) in level.faces.iter().enumerate() {
        let n = face.len as usize;
        let c: Vec<u32> = (0..n).map(|i| corner(level, f, i)).collect();
        let p: Vec<Vec3> = c.iter().map(|&v| position(level, v)).collect();
        let raw = if n == 4 { (p[1] - p[3]).cross(p[2] - p[0]) } else { (p[1] - p[0]).cross(p[2] - p[0]) };
        let Some(unit) = raw.try_normalize() else { continue };
        for &v in &c {
            sum[v as usize] += unit;
            count[v as usize] += 1;
        }
    }
    sum.into_iter().zip(count).map(|(s, k)| if k > 0 { s / k as f32 } else { Vec3::ZERO }).collect()
}

pub fn add_mesh(world: &mut CollisionWorld, mesh: &SubdMesh) {
    let level = SubdLevel::from_subd(mesh).refined(GAME_LEVELS);
    let normals = vertex_normals(&level);
    let mut stats = BuildStats { terrain_meshes: 1, ..BuildStats::default() };
    let class = class_byte(u32::from(mesh.unk_tail[0]));
    let material = u32::from(mesh.unk_tail[1]);
    stats.terrain_class_meshes[class as usize] += 1;
    let mut first_child = 0usize;
    let mut verts: Vec<Vec3> = Vec::with_capacity(25);
    let mut vnormals: Vec<Vec3> = Vec::with_capacity(25);
    let mut nibbles: Vec<u32> = Vec::with_capacity(25);
    let mut tris: Vec<[u8; 3]> = Vec::with_capacity(32);
    let mut ids: Vec<(u32, [i32; 3])> = Vec::with_capacity(25);
    for face in &mesh.faces {
        let children = first_child..first_child + 4 * face.edges.len();
        first_child = children.end;
        if ssx_assets::terrain::is_hidden_face(face.unk_a) {
            stats.hidden_faces += 1;
            continue;
        }
        stats.kept_faces += 1;
        stats.terrain_class_faces[class as usize] += 1;
        verts.clear();
        vnormals.clear();
        nibbles.clear();
        tris.clear();
        ids.clear();
        let mut quads = 0u32;
        for child in children {
            let mut local = [0u8; 4];
            let mut q = [[0i32; 3]; 4];
            for (k, slot) in local.iter_mut().enumerate() {
                let v = corner(&level, child, k);
                let idx = match ids.iter().position(|&(id, _)| id == v) {
                    Some(i) => i,
                    None => {
                        let p = position(&level, v);
                        let qp = p.to_array().map(quantize);
                        ids.push((v, qp));
                        verts.push(Vec3::from_array(qp.map(dequantize)));
                        vnormals.push(normals[v as usize]);
                        nibbles.push(pack_terrain_nibbles(&level.vertices[v as usize].attrs.map(|a| a as f32)));
                        ids.len() - 1
                    }
                };
                *slot = idx as u8;
                q[k] = ids[idx].1;
            }
            let [v0, v1, v2, v3] = local;
            if q[1] == q[3] {
                stats.dropped_units += 1;
                continue;
            }
            let v1_on = q[1] == q[0] || q[1] == q[2];
            let v3_on = q[3] == q[0] || q[3] == q[2];
            match (v1_on, v3_on) {
                (false, false) => {
                    quads |= 1 << tris.len();
                    tris.extend([[v1, v2, v0], [v3, v0, v2]]);
                }
                (true, false) => {
                    tris.push([v3, v0, v2]);
                    stats.triangle_units += 1;
                }
                (false, true) => {
                    tris.push([v1, v2, v0]);
                    stats.triangle_units += 1;
                }
                (true, true) => stats.dropped_units += 1,
            }
        }
        world.push_cluster(&verts, Some(&vnormals), Some(&nibbles), &tris, material, class);
        if !tris.is_empty() {
            world.set_quad_units(quads);
        }
    }
    world.stats.add(&stats);
}

pub fn add_track_terrain(world: &mut CollisionWorld, dir: &Path) -> Result<()> {
    add_track_terrain_files(world, dir, None)
}

pub fn add_track_terrain_files(world: &mut CollisionWorld, dir: &Path, files: Option<&[String]>) -> Result<()> {
    let pieces = ssx_assets::terrain::load_track_files(dir, files)?;
    let parts: Result<Vec<CollisionWorld>> = std::thread::scope(|s| {
        let handles: Vec<_> = pieces
            .iter()
            .map(|piece| {
                s.spawn(move || {
                    let mut part = CollisionWorld::default();
                    for mesh in &piece.subd.meshes {
                        add_mesh(&mut part, mesh);
                    }
                    part.stats.terrain_files = 1;
                    part
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().map_err(|_| anyhow::anyhow!("terrain build thread panicked"))).collect()
    });
    for part in parts? {
        world.append(part);
    }
    if world.boundary.is_none() {
        world.boundary = crate::rider::boundary::BoundaryGrid::load(dir);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ssx_formats::subd::{SubdFace, SubdVertex};

    fn mesh(unk_a: [u8; 2]) -> SubdMesh {
        let v = |x: f32, y: f32, z: f32| SubdVertex {
            position: [x, y, z],
            attributes: [0.0, 0.0, 0.0, 0.5, 0.0, 0.5, 0.0, 0.0],
        };
        let vertices = vec![
            v(-11.2341, -3.1007, -7.5),
            v(-11.2341, -3.1007, -2.5),
            v(-6.2, -3.1007, -2.5),
            v(-6.2, -3.1007, -7.5),
            v(-1.0, -3.1007, -5.0),
        ];
        let edges = vec![[0, 1], [1, 2], [2, 3], [3, 0], [2, 4], [4, 3]];
        let faces = vec![
            SubdFace { unk_a: unk_a[0], reversed_mask: 0, edges: vec![0, 1, 2, 3] },
            SubdFace { unk_a: unk_a[1], reversed_mask: 0b001, edges: vec![2, 4, 5] },
        ];
        SubdMesh {
            version: 0xEA00_0006,
            unk_header: [0x20, 1, 2],
            vertices,
            edges,
            faces,
            unk_face_flags: vec![0, 0],
            unk_tail: [0, 0xFFFF],
        }
    }

    #[test]
    fn clusters_follow_the_game_layout() {
        let m = mesh([0, 0]);
        let mut w = CollisionWorld::default();
        add_mesh(&mut w, &m);
        w.finish();
        assert_eq!(w.clusters.len(), 2);
        let (q, t) = (&w.clusters[0], &w.clusters[1]);
        assert_eq!((q.vert_count, q.tri_count), (25, 32));
        assert_eq!((t.vert_count, t.tri_count), (19, 24));
        assert!(w.clusters.iter().all(|c| c.is_terrain() && c.class == 0));
        let level = SubdLevel::from_subd(&m).refined(GAME_LEVELS);
        for v in &w.vertices {
            let ok = level.vertices.iter().any(|r| {
                let p = r.position.map(|c| c as f32);
                (0..3).all(|k| v[k] == ((p[k] * 125.0) as i32) as f32 * 0.008)
            });
            assert!(ok, "{v:?} is not a quantised refined vertex");
        }
        assert!(w.normals.iter().all(|n| n.distance(Vec3::Y) < 1e-5));
        let h = w.ray_cast(Vec3::new(-9.0, 0.0, -5.0), -Vec3::Y, 10.0).expect("hit");
        assert_eq!(h.cluster, 0);
        let wts = w.surface_weights(h.cluster, h.tri, h.bary, h.normal.y);
        assert!((wts[3] - 7.0 / 15.0).abs() < 1e-5 && (wts[5] - 8.0 / 15.0).abs() < 1e-5 && wts[13] > 0.999);
    }

    #[test]
    fn mesh_class_from_unk_tail() {
        let mut m = mesh([0, 0]);
        m.unk_tail = [12, 0x28];
        let mut w = CollisionWorld::default();
        add_mesh(&mut w, &m);
        assert!(w.clusters.iter().all(|c| c.class == 12));
        assert_eq!((w.stats.terrain_class_faces[12], w.stats.terrain_class_meshes[12]), (2, 1));
        m.unk_tail = [0x101, 0xFFFF];
        let mut w = CollisionWorld::default();
        add_mesh(&mut w, &m);
        assert!(w.clusters.iter().all(|c| c.class == 1));
    }

    #[test]
    fn hidden_faces_have_no_collision() {
        let mut w = CollisionWorld::default();
        add_mesh(&mut w, &mesh([1, 0]));
        assert_eq!((w.clusters.len(), w.stats.kept_faces, w.stats.hidden_faces), (1, 1, 1));
        assert_eq!(w.clusters[0].vert_count, 19);
    }

    fn quad(corners: [[f32; 3]; 4]) -> SubdMesh {
        SubdMesh {
            version: 0xEA00_0006,
            unk_header: [0x20, 1, 2],
            vertices: corners.iter().map(|&position| SubdVertex { position, attributes: [0.0; 8] }).collect(),
            edges: vec![[0, 1], [1, 2], [2, 3], [3, 0]],
            faces: vec![SubdFace { unk_a: 0, reversed_mask: 0, edges: vec![0, 1, 2, 3] }],
            unk_face_flags: vec![0],
            unk_tail: [0, 0xFFFF],
        }
    }

    #[test]
    fn coinciding_corners_become_triangles_or_vanish() {
        let mut w = CollisionWorld::default();
        add_mesh(&mut w, &quad([[0.0, 0.0, 0.0], [0.0, 0.0, 0.06], [0.025, 0.0, 0.06], [0.025, 0.0, 0.0]]));
        let s = &w.stats;
        assert!(s.triangle_units >= 1 && s.dropped_units >= 1, "{s:?}");
        assert_eq!(w.triangles.len(), 2 * (16 - s.triangle_units - s.dropped_units) + s.triangle_units);
    }
}
