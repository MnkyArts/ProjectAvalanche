use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result};
use glam::{Quat, Vec3};
use ssx_assets::content::GameContent;
use ssx_assets::object_collision::load_pob;
use ssx_assets::track_objects::{AudioName, Placement, asset_file, load_track_objects};
use ssx_content::model::Collision;
use ssx_content::object::ObjectCollision;

use crate::collision::{CollisionWorld, PropEntity, TriEdges, class_byte};

pub fn prop_material_id(child_surface_id: u32, ao: f32) -> u32 {
    let low = if -ao >= 0.0 { 0.0 } else { ao };
    let clamped = if 1.0 - low >= 0.0 { low } else { 1.0 };
    (child_surface_id & 0xFF) | (((clamped * 255.0) as u32) << 8)
}

pub fn place(pl: &Placement, p: Vec3) -> Vec3 {
    let q = Quat::from_xyzw(pl.rotation[0], pl.rotation[1], pl.rotation[2], pl.rotation[3]);
    Vec3::from_array(pl.position) + q * (Vec3::from_array(pl.scale) * p)
}

struct PropCollision {
    collision: Option<Arc<Collision>>,
    audio: Option<AudioName>,
}

pub fn add_track_props(world: &mut CollisionWorld, big_root: &Path, track: &str, drop_key: Option<u32>) -> Result<()> {
    let objects = load_track_objects(big_root, track, drop_key)?;
    let content = GameContent::of(big_root);
    let packages = content.track_packages(track);
    if packages.world_triangles.is_some() {
        world.stats.prop_decision = Some(packages.summary());
    }
    let mut by_pob: HashMap<String, Option<Arc<Collision>>> = HashMap::new();
    let mut by_object: HashMap<usize, PropCollision> = HashMap::new();
    for (idx, placements) in &objects.instances {
        let Some(object) = objects.objects.get(*idx) else { continue };
        if !by_object.contains_key(idx) {
            let resolved = match packages.object(object.key) {
                Some(found) => {
                    let collision = match &found.value.collision {
                        ObjectCollision::None => None,
                        ObjectCollision::Decoded { collision, .. } => Some(collision.clone()),
                        ObjectCollision::Model(id) => packages
                            .object_model(id)
                            .with_context(|| format!("{}: its collision model {id} was not decoded", found.id))?
                            .value
                            .collision
                            .clone(),
                    };
                    let size = collision.as_ref().map_or_else(
                        || "no collision".to_string(),
                        |c| {
                            let triangles: usize = c.clusters.iter().map(|k| k.triangles.len()).sum();
                            format!("{} clusters, {triangles} triangles", c.clusters.len())
                        },
                    );
                    world.stats.prop_packages.push(format!("{} from {}: {size}", found.id, found.source));
                    PropCollision { collision, audio: found.value.audio.clone() }
                }
                None => {
                    let collision = match &object.pob {
                        Some(pob_path) => {
                            if !by_pob.contains_key(pob_path) {
                                let loaded = match asset_file(big_root, pob_path) {
                                    Some(path) => {
                                        let (c, _) = load_pob(&path)?;
                                        world.stats.prop_files += 1;
                                        world.stats.prop_cylinders += c.cylinders;
                                        Some(Arc::new(c))
                                    }
                                    None => {
                                        world.stats.prop_missing += 1;
                                        world.stats.prop_missing_paths.push(pob_path.clone());
                                        None
                                    }
                                };
                                by_pob.insert(pob_path.clone(), loaded);
                            }
                            by_pob.get(pob_path).cloned().flatten()
                        }
                        None => None,
                    };
                    PropCollision { collision, audio: object.audio.clone() }
                }
            };
            by_object.insert(*idx, resolved);
        }
        let Some(PropCollision { collision: Some(collision), audio }) = by_object.get(idx) else { continue };
        for pl in placements {
            world.stats.prop_instances += 1;
            let entity = world.add_entity(PropEntity { object: object.name.clone(), audio: audio.clone() });
            for c in &collision.clusters {
                let v: Vec<Vec3> = c.vertices.iter().map(|&p| place(pl, Vec3::from_array(p))).collect();
                let material = prop_material_id(c.surface_id, pl.ao_multiplier());
                let class = class_byte(c.group);
                world.push_cluster(&v, None, None, &c.triangles, material, class);
                world.set_cluster_entity(entity);
                // TODO(guess): a prop's part frame taken as its placement's rotation quaternion; the game's static slot holds the frame of the prop's part as the world takes it from the entity's matrix (SCMComponent_BuildPartsFromPob 0x82945300 → the part's insertion; not followed); the rigid setup 0x82E753D0 reads only that orientation of a static side, whose mass terms are zero, so it signs those zeros, which reach K only where the body's own term is an exact zero; settle: the slot's quaternion of a placed, rotated and scaled prop in the emulator.
                world.set_cluster_frame(Quat::from_xyzw(
                    pl.rotation[0],
                    pl.rotation[1],
                    pl.rotation[2],
                    pl.rotation[3],
                ));
                let edges: Vec<TriEdges> =
                    c.edges.iter().map(|e| TriEdges { flags: e.flags, edge_cos: e.edge_cos }).collect();
                world.set_triangle_edges(&edges, c.material);
                world.stats.prop_class_clusters[class as usize] += 1;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_material_id_carries_the_placement_ao_byte() {
        assert_eq!(prop_material_id(10, 1.0), 0xFF0A);
        assert_eq!(prop_material_id(41, 0.5), (127 << 8) | 41);
        assert_eq!(prop_material_id(0x10A, 0.0), 0x0A);
        assert_eq!(prop_material_id(10, 3.2) >> 8, 255);
        assert_eq!(prop_material_id(10, -0.0) >> 8, 0);
        assert_eq!(prop_material_id(10, f32::NAN) >> 8, 255);
        let w = crate::surface::object_contact_weights(prop_material_id(10, 0.4), 1.0);
        assert!((w[crate::surface::AMBIENT_OCCLUSION] - 102.0 / 255.0).abs() < 1e-6);
    }
}
