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

pub fn transform_box(rows: [Vec3; 3], t: Vec3, min: Vec3, max: Vec3) -> (Vec3, Vec3) {
    use ssx_formats::vmx;
    let (min, max) = (min.to_array(), max.to_array());
    let c: [f32; 3] = std::array::from_fn(|i| vmx::mul(vmx::add(max[i], min[i]), 0.5));
    let h: [f32; 3] = std::array::from_fn(|i| vmx::mul(vmx::sub(max[i], min[i]), 0.5));
    let rows = rows.map(|r| r.to_array());
    let t = t.to_array();
    let mut lo = [0.0f32; 3];
    let mut hi = [0.0f32; 3];
    for i in 0..3 {
        let centre = vmx::madd(rows[2][i], c[2], vmx::madd(rows[1][i], c[1], vmx::madd(rows[0][i], c[0], t[i])));
        let half = vmx::madd(
            vmx::abs(rows[2][i]),
            h[2],
            vmx::madd(vmx::abs(rows[1][i]), h[1], vmx::mul(vmx::abs(rows[0][i]), h[0])),
        );
        (lo[i], hi[i]) = (vmx::sub(centre, half), vmx::add(centre, half));
    }
    (Vec3::from_array(lo), Vec3::from_array(hi))
}

pub fn part_matrix(pl: &Placement) -> ([Vec3; 3], Vec3) {
    use ssx_formats::vmx;
    let root2 = f32::from_bits(0x3FB5_04F3);
    let [px, py, pz, pw] = pl.rotation.map(|c| vmx::mul(root2, c));
    let [dx, dy, dz] = [px, py, pz].map(|c| vmx::nmsub(c, c, 0.5));
    let (xy, yz, zx) = (vmx::mul(px, py), vmx::mul(py, pz), vmx::mul(pz, px));
    let (wz, wx, wy) = (vmx::mul(pw, pz), vmx::mul(pw, px), vmx::mul(pw, py));
    let rows = [
        [vmx::add(dy, dz), vmx::add(xy, wz), vmx::sub(zx, wy)],
        [vmx::sub(xy, wz), vmx::add(dz, dx), vmx::add(yz, wx)],
        [vmx::add(zx, wy), vmx::sub(yz, wx), vmx::add(dx, dy)],
    ];
    let scale = pl.scale[0];
    (rows.map(|r| Vec3::from_array(r.map(|c| vmx::mul(c, scale)))), Vec3::from_array(pl.position))
}

fn placed_entry_box(pl: &Placement, vertices: &[[f32; 3]]) -> (Vec3, Vec3) {
    let (mut min, mut max) = (Vec3::splat(f32::INFINITY), Vec3::splat(f32::NEG_INFINITY));
    for &v in vertices {
        min = min.min(Vec3::from_array(v));
        max = max.max(Vec3::from_array(v));
    }
    let (rows, t) = part_matrix(pl);
    transform_box(rows, t, min, max)
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
                let (lo, hi) = placed_entry_box(pl, &c.vertices);
                world.set_cluster_entry_box(lo, hi);
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
    fn a_box_through_a_matrix_holds_the_transformed_box() {
        let (min, max) = (Vec3::new(-1.0, 0.0, -2.0), Vec3::new(3.0, 1.0, 2.0));
        let (lo, hi) = transform_box([Vec3::Z * -1.0, Vec3::Y * 2.0, Vec3::X], Vec3::new(10.0, 0.0, 0.0), min, max);
        assert_eq!((lo, hi), (Vec3::new(8.0, 0.0, -3.0), Vec3::new(12.0, 2.0, 1.0)));
        let h = std::f32::consts::FRAC_1_SQRT_2;
        let (lo, hi) = transform_box([Vec3::new(h, 0.0, -h), Vec3::Y, Vec3::new(h, 0.0, h)], Vec3::ZERO, min, max);
        let corners = (0..8).map(|k| {
            let p = Vec3::new([min.x, max.x][k & 1], [min.y, max.y][(k >> 1) & 1], [min.z, max.z][k >> 2]);
            Vec3::new(h * p.x + h * p.z, p.y, -h * p.x + h * p.z)
        });
        for c in corners {
            assert!(c.cmpge(lo - 1e-5).all() && c.cmple(hi + 1e-5).all(), "{c} outside {lo} … {hi}");
        }
        assert!((hi.x - lo.x - 8.0 * h).abs() < 1e-5 && (hi.z - lo.z - 8.0 * h).abs() < 1e-5, "{lo} … {hi}");
    }

    #[test]
    fn a_part_matrix_is_the_quaternions_rotation_times_the_scale() {
        let at = |rotation: [f32; 4], scale: f32| Placement {
            scale: [scale; 3],
            rotation,
            position: [1.0, -2.0, 3.0],
            unk: [1.0, 1.0, 1.0, 0.0, 0.0, 0.0],
        };
        let (rows, t) = part_matrix(&at([0.0, 0.0, 0.0, 1.0], 2.5));
        assert_eq!(rows, [Vec3::X * 2.5, Vec3::Y * 2.5, Vec3::Z * 2.5]);
        assert_eq!(t, Vec3::new(1.0, -2.0, 3.0));
        let h = std::f32::consts::FRAC_1_SQRT_2;
        let (rows, _) = part_matrix(&at([0.0, h, 0.0, h], 1.0));
        assert!(rows[0].abs_diff_eq(-Vec3::Z, 1e-6) && rows[2].abs_diff_eq(Vec3::X, 1e-6), "{rows:?}");
        let q = Quat::from_xyzw(0.3, -0.5, 0.1, 0.8).normalize();
        let (rows, _) = part_matrix(&at(q.to_array(), 0.75));
        for (row, axis) in rows.iter().zip([Vec3::X, Vec3::Y, Vec3::Z]) {
            assert!(row.abs_diff_eq(q * axis * 0.75, 1e-6), "{row} vs {}", q * axis * 0.75);
        }
    }

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
