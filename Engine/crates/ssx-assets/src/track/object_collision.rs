use anyhow::{Context, Result, bail};
use glam::Vec3;
use ssx_content::model::{Collision, CollisionCluster, TriangleEdges};
use ssx_content::object::RigidBody;
use ssx_formats::pob::{ObjectData, PobFile, Volume, VolumeKind};

pub const DEFAULT_PART_MATERIAL: [f32; 3] = [0.5, 0.3, 0.4];

pub fn unit_triangle_edges(
    unit: &ssx_formats::pob::Unit,
    mesh_flags: u16,
    vertex: impl Fn(u8) -> Vec3,
) -> Result<Vec<TriangleEdges>> {
    use ssx_formats::pob::edge_cosine;
    let codes = &unit.edge_codes;
    if codes.is_empty() {
        bail!("cluster unit without edge codes (none on the disc: every unit is 0x21 or 0x22)");
    }
    let base = 0x103 | u32::from(mesh_flags & 0x10);
    let make = |vc: [u8; 3], ec: [Option<u8>; 3], diag: (f32, bool)| {
        let mut flags = base;
        let mut cos = [1.0f32; 3];
        for k in 0..3 {
            if vc[k] & 0x40 != 0 {
                flags |= 0x200 << k;
            }
            let (c, convex) = match ec[k] {
                Some(code) => (edge_cosine(code), code & 0x20 != 0),
                None => diag,
            };
            if convex {
                flags |= 0x20 << k;
                cos[k] = c;
            }
        }
        TriangleEdges { flags, edge_cos: cos }
    };
    let i = &unit.indices;
    Ok(match unit.kind() {
        1 => vec![make([codes[0], codes[1], codes[2]], [Some(codes[0]), Some(codes[1]), Some(codes[2])], (1.0, false))],
        2 => {
            let (v0, v1, v2, v3) = (vertex(i[0]), vertex(i[1]), vertex(i[2]), vertex(i[3]));
            let nb = (v1 - v0).cross(v2 - v0);
            let na = (v2 - v3).cross(v1 - v3);
            let convex = (v2 - v1).dot(nb.cross(na)) > 0.0;
            let (lb, la) = (nb.length_squared(), na.length_squared());
            let cos = if lb > f32::MIN_POSITIVE && la > f32::MIN_POSITIVE {
                nb.dot(na) / (lb.sqrt() * la.sqrt())
            } else {
                1.0
            };
            vec![
                make([codes[0], codes[1], codes[2]], [Some(codes[0]), None, Some(codes[2])], (cos, convex)),
                make([codes[3], codes[2], codes[1]], [Some(codes[3]), None, Some(codes[1])], (cos, convex)),
            ]
        }
        k => bail!("cluster unit kind {k} not handled (none on the disc)"),
    })
}

pub fn pob_part_material(pob: &PobFile) -> [f32; 3] {
    pob.objects
        .iter()
        .find_map(|o| match &o.data {
            ObjectData::AssemblyDefinition(d) => d.parts.first().map(|p| p.material),
            _ => None,
        })
        .unwrap_or(DEFAULT_PART_MATERIAL)
}

pub fn pob_collision(pob: &PobFile) -> Result<Collision> {
    let mut out = Collision::default();
    let material = pob_part_material(pob);
    let outer: Vec<&Volume> =
        pob.objects.iter().filter_map(|o| if let ObjectData::Volume(v) = &o.data { Some(v) } else { None }).collect();
    for ov in outer {
        let Some(sma_at) = ov.aggregate else { bail!("outer volume is a {:?}, expected an aggregate", ov.kind) };
        let Some(ObjectData::SimpleMappedArray(sma)) = pob.object_at(sma_at).map(|o| &o.data) else {
            bail!("outer volume does not point at a SimpleMappedArray");
        };
        for child in &sma.volumes {
            match child.kind {
                VolumeKind::Aggregate => {
                    let at = child.aggregate.context("aggregate child without target")?;
                    let Some(ObjectData::ClusteredMesh(cm)) = pob.object_at(at).map(|o| &o.data) else {
                        bail!("SMA child aggregate is not a ClusteredMesh");
                    };
                    for c in &cm.clusters {
                        let vertices = c.vertices.clone();
                        let mut triangles = Vec::new();
                        let mut edges = Vec::new();
                        for u in &c.units {
                            triangles.extend(u.triangles().unwrap_or_default());
                            let raw = |k: u8| Vec3::from_array(c.vertices[k as usize]);
                            edges.extend(unit_triangle_edges(u, cm.flags, raw)?);
                        }
                        out.clusters.push(CollisionCluster {
                            vertices,
                            triangles,
                            surface_id: child.surface_id,
                            group: child.group_id,
                            edges,
                            material,
                        });
                    }
                }
                VolumeKind::Cylinder => {
                    child.cylinder().context("cylinder parameters")?;
                    out.cylinders += 1;
                }
                other => bail!("SMA child volume kind {other:?} not handled (none on the disc)"),
            }
        }
    }
    Ok(out)
}

pub fn pob_rigid_body(pob: &PobFile) -> Option<RigidBody> {
    let def = pob.objects.iter().find_map(|o| match &o.data {
        ObjectData::AssemblyDefinition(d) => d.parts.first().cloned(),
        _ => None,
    })?;
    let outer = match &pob.object_at(def.volume)?.data {
        ObjectData::Volume(v) => v.clone(),
        _ => return None,
    };
    let sma = match &pob.object_at(outer.aggregate?)?.data {
        ObjectData::SimpleMappedArray(s) => s.clone(),
        _ => return None,
    };
    let child = sma.volumes.iter().find(|v| v.kind == VolumeKind::Cylinder)?;
    let (half_height, radius) = child.cylinder()?;
    let m = child.transform;
    let [d0, d1, d2] = def.diagonal_terms;
    let [inv_mass, max_speed, max_angular_speed, linear_damping, angular_damping] = def.mass_params;
    Some(RigidBody {
        offset: m[3],
        axis: m[2],
        half_height,
        radius,
        fat: child.radius,
        inv_mass,
        inv_inertia: (Vec3::new(d0, d1, d2) / inv_mass).to_array(),
        max_speed,
        max_angular_speed,
        linear_damping,
        angular_damping,
        material: def.material,
    })
}

pub fn load_pob(path: &std::path::Path) -> Result<(Collision, Option<RigidBody>)> {
    let data = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    let pob = PobFile::parse(&data).with_context(|| format!("parsing {}", path.display()))?;
    let collision = pob_collision(&pob).with_context(|| path.display().to_string())?;
    Ok((collision, pob_rigid_body(&pob)))
}
