use std::collections::{BTreeMap, HashMap};

use anyhow::{Result, bail};
use glam::{Quat, Vec3};
use ssx_assets::content::maps::MapObject;
use ssx_content::ContentId;
use ssx_content::map::{CollisionMesh, MAX_COLLISION_TRIANGLES, MapDefinition, PropLook, TerrainMesh};
use ssx_content::model::Collision;
use ssx_content::model::collision::build as build_parts;
use ssx_rules::placement::Placement;

use crate::collision::{CollisionWorld, PropEntity, TriEdges, class_byte};
use crate::props::{place, prop_material_id};
use crate::surface::pack_terrain_nibbles;
use crate::terrain::{dequantize, quantize};

pub const TERRAIN_SURFACE: u32 = 0xFFFF;
pub const CLUSTER_TRIANGLES: usize = 32;
pub const CLUSTER_CELL: f32 = 16.0;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MapCollisionStats {
    pub terrain_clusters: usize,
    pub terrain_triangles: usize,
    pub rail_clusters: usize,
    pub rail_triangles: usize,
    pub dropped: usize,
    pub collision_clusters: usize,
    pub collision_triangles: usize,
    pub degenerate: usize,
    pub object_placements: usize,
    pub object_triangles: usize,
}

impl MapCollisionStats {
    pub fn triangles(&self) -> usize {
        self.terrain_triangles + self.rail_triangles + self.collision_triangles + self.object_triangles
    }
}

pub fn add_map_collision(
    world: &mut CollisionWorld,
    def: &MapDefinition,
    objects: &BTreeMap<ContentId, MapObject>,
) -> Result<MapCollisionStats> {
    add_map_collision_with_cap(world, def, objects, MAX_COLLISION_TRIANGLES)
}

pub fn add_map_collision_with_cap(
    world: &mut CollisionWorld,
    def: &MapDefinition,
    objects: &BTreeMap<ContentId, MapObject>,
    cap: usize,
) -> Result<MapCollisionStats> {
    let mut object_triangles = 0usize;
    for p in &def.props {
        if let PropLook::Object(id) = &p.look {
            let Some(o) = objects.get(id) else { bail!("node \"{}\": {id} is not resolved", p.node) };
            object_triangles += o.collision.as_ref().map_or(0, |c| c.triangle_count());
        }
    }
    let total = def.collision_triangles().saturating_add(object_triangles);
    if total > cap {
        bail!(
            "the map's collision is {total} triangles with its placed objects' ({object_triangles}); at most \
             {cap}, the disc's largest track"
        );
    }
    let mut stats = MapCollisionStats::default();
    for t in &def.terrain {
        add_terrain(world, t, &mut stats);
    }
    for r in &def.rails {
        add_rail(world, r, &mut stats);
    }
    for c in &def.collision {
        let (parts, degenerate) = build_parts(&c.triangles);
        stats.degenerate += degenerate;
        let entity = world.add_entity(PropEntity { object: c.node.clone(), audio: None });
        // TODO(guess): a port choice — a map's collision mesh is a static part in the world frame (its node's transform
        // already applied), so its part frame is the identity and its AO multiplier 1.0 (the AO byte 255); the disc's
        // props take their placement's quaternion and word 12, and the game has no map collision mesh; settle: the frame
        // and AO the game would give a world-space part (SCMComponent_BuildPartsFromPob 0x82945300's insertion, not
        // followed).
        crate::guess_hit!();
        add_parts(world, &parts, &|v| v, 1.0, Quat::IDENTITY, entity, &mut stats.collision_clusters);
        stats.collision_triangles += parts.triangle_count();
    }
    for p in &def.props {
        let PropLook::Object(id) = &p.look else { continue };
        let Some(o) = objects.get(id) else { continue };
        let Some(collision) = &o.collision else { continue };
        let pl = Placement {
            scale: p.placement.scale,
            rotation: p.placement.rotation,
            position: p.placement.position,
            unk: [1.0, 1.0, p.ao, 0.0, 0.0, 0.0],
        };
        let entity = world.add_entity(PropEntity { object: id.to_string(), audio: o.audio.clone() });
        let frame = Quat::from_xyzw(pl.rotation[0], pl.rotation[1], pl.rotation[2], pl.rotation[3]);
        let at = |v: Vec3| place(&pl, v);
        add_parts(world, collision, &at, pl.ao_multiplier(), frame, entity, &mut stats.collision_clusters);
        stats.object_placements += 1;
        stats.object_triangles += collision.triangle_count();
    }
    Ok(stats)
}

fn add_parts(
    world: &mut CollisionWorld,
    c: &Collision,
    at: &dyn Fn(Vec3) -> Vec3,
    ao: f32,
    frame: Quat,
    entity: u32,
    clusters: &mut usize,
) {
    for part in &c.clusters {
        let v: Vec<Vec3> = part.vertices.iter().map(|&p| at(Vec3::from_array(p))).collect();
        let class = class_byte(part.group);
        world.push_cluster(&v, None, None, &part.triangles, prop_material_id(part.surface_id, ao), class);
        world.set_cluster_entity(entity);
        world.set_cluster_frame(frame);
        let edges: Vec<TriEdges> =
            part.edges.iter().map(|e| TriEdges { flags: e.flags, edge_cos: e.edge_cos }).collect();
        world.set_triangle_edges(&edges, part.material);
        world.stats.prop_class_clusters[class as usize] += 1;
        *clusters += 1;
    }
}

struct Surface<'a> {
    positions: &'a [[f32; 3]],
    triangles: &'a [[u32; 3]],
    nibbles: Vec<u32>,
}

#[derive(Default)]
struct RailPart {
    positions: Vec<[f32; 3]>,
    triangles: Vec<[u32; 3]>,
}

fn add_terrain(world: &mut CollisionWorld, t: &TerrainMesh, stats: &mut MapCollisionStats) {
    let nibbles = t.weights.iter().map(pack_terrain_nibbles).collect();
    let s = Surface { positions: &t.positions, triangles: &t.triangles, nibbles };
    let (clusters, triangles) = add_surface(world, &s, TERRAIN_SURFACE, 0, &mut stats.dropped);
    stats.terrain_clusters += clusters;
    stats.terrain_triangles += triangles;
}

fn add_rail(world: &mut CollisionWorld, r: &CollisionMesh, stats: &mut MapCollisionStats) {
    let mut by_surface: BTreeMap<(u32, u32), RailPart> = BTreeMap::new();
    for tri in &r.triangles {
        let part = by_surface.entry((tri.surface_id, tri.group)).or_default();
        let base = part.positions.len() as u32;
        part.positions.extend(tri.corners);
        part.triangles.push([base, base + 1, base + 2]);
    }
    for ((surface, group), part) in &by_surface {
        let nibbles = vec![0; part.positions.len()];
        let s = Surface { positions: &part.positions, triangles: &part.triangles, nibbles };
        let (clusters, n) = add_surface(world, &s, *surface, class_byte(*group), &mut stats.dropped);
        stats.rail_clusters += clusters;
        stats.rail_triangles += n;
    }
}

fn add_surface(
    world: &mut CollisionWorld,
    s: &Surface<'_>,
    surface: u32,
    class: u8,
    dropped: &mut usize,
) -> (usize, usize) {
    let key = |v: u32| s.positions[v as usize].map(quantize);
    let mut sums: HashMap<[i32; 3], (Vec3, u32)> = HashMap::new();
    let mut kept: Vec<(usize, [u32; 3])> = Vec::with_capacity(s.triangles.len());
    for (k, t) in s.triangles.iter().enumerate() {
        let q = t.map(key);
        if q[0] == q[1] || q[1] == q[2] || q[0] == q[2] {
            *dropped += 1;
            continue;
        }
        let p = t.map(|v| Vec3::from_array(s.positions[v as usize]));
        let Some(n) = (p[1] - p[0]).cross(p[2] - p[0]).try_normalize() else {
            *dropped += 1;
            continue;
        };
        for c in q {
            let e = sums.entry(c).or_insert((Vec3::ZERO, 0));
            e.0 += n;
            e.1 += 1;
        }
        kept.push((k, *t));
    }
    // TODO(guess): a port choice — a map's terrain is cut into clusters by the 16 m square of each triangle's centroid,
    // at most 32 triangles (and 256 vertices) each, with no quad units; the disc's terrain clusters are one cage face
    // each after two refinement levels (PhysicsChunkJob 0x82A71F20: ≤ 32 triangles, 25 vertices), which a triangle
    // mesh does not have; settle: nothing in the game builds clusters from a triangle mesh (a cell size measured
    // against the disc's cluster extents would only tune the BVH).
    crate::guess_hit!();
    let cell = |t: &[u32; 3]| {
        let c = t.iter().fold(Vec3::ZERO, |a, &v| a + Vec3::from_array(s.positions[v as usize])) / 3.0;
        ((c.z / CLUSTER_CELL).floor() as i64, (c.x / CLUSTER_CELL).floor() as i64)
    };
    kept.sort_by_key(|(k, t)| (cell(t), *k));
    let (mut clusters, mut triangles) = (0, 0);
    let mut i = 0;
    while i < kept.len() {
        let here = cell(&kept[i].1);
        let (mut verts, mut normals, mut nibbles) = (Vec::new(), Vec::new(), Vec::new());
        let mut local: HashMap<[i32; 3], u8> = HashMap::new();
        let mut tris: Vec<[u8; 3]> = Vec::new();
        while i < kept.len() && tris.len() < CLUSTER_TRIANGLES && cell(&kept[i].1) == here {
            let t = kept[i].1;
            let new = t.iter().filter(|&&v| !local.contains_key(&key(v))).count();
            if local.len() + new > 256 {
                break;
            }
            let mut idx = [0u8; 3];
            for (slot, &v) in idx.iter_mut().zip(&t) {
                let q = key(v);
                *slot = match local.get(&q) {
                    Some(&j) => j,
                    None => {
                        let j = verts.len() as u8;
                        local.insert(q, j);
                        verts.push(Vec3::from_array(q.map(dequantize)));
                        let (sum, n) = sums.get(&q).copied().unwrap_or((Vec3::ZERO, 1));
                        normals.push(sum / n as f32);
                        nibbles.push(s.nibbles.get(v as usize).copied().unwrap_or(0));
                        j
                    }
                };
            }
            tris.push(idx);
            i += 1;
        }
        triangles += tris.len();
        world.push_cluster(&verts, Some(&normals), Some(&nibbles), &tris, surface, class);
        clusters += 1;
    }
    (clusters, triangles)
}
