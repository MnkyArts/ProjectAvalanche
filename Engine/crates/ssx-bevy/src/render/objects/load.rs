use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use anyhow::{Context, Result};
use bevy::asset::RenderAssetUsages;
use bevy::mesh::Indices;
use bevy::prelude::{Mesh, Vec3, info, warn};
use bevy::render::render_resource::PrimitiveTopology;
use ssx_assets::content::GameContent;
use ssx_assets::content::resolve::Found;
use ssx_assets::content::track_packages::{MeshPass, Usage};
use ssx_assets::object_mesh::ObjectMesh;
use ssx_assets::track_objects::{Placement, load_track_objects};
use ssx_content::map::Map;

use super::gather::{Gather, MeshJob};
use super::specs::{MaterialInput, prepare_materials};
use super::{LoadedObjects, ObjectStats};

#[derive(Clone, Debug)]
pub(super) struct ObjectLod {
    pub(super) thresholds: Vec<f32>,
    pub(super) centre: Vec3,
    pub(super) half: Vec3,
    pub(super) class: ModelClass,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ModelClass {
    Prelight = 0,
    Forward = 1,
    Translucent = 2,
    Empty = 3,
}

impl ModelClass {
    fn of<'a>(techniques: impl IntoIterator<Item = Option<&'a str>>) -> ModelClass {
        let (mut forward, mut translucent, mut other) = (false, false, false);
        for t in techniques {
            forward |= t == Some("forward");
            translucent |= t == Some("translucent");
            other |= !forward && !translucent;
            if forward && translucent && other {
                break;
            }
        }
        match (other, translucent, forward) {
            (true, _, _) => ModelClass::Prelight,
            (false, true, _) => ModelClass::Translucent,
            (false, false, true) => ModelClass::Forward,
            _ => ModelClass::Empty,
        }
    }
}

impl ObjectLod {
    const EMPTY_HALF: f32 = 1.192_092_9e-7;

    pub(super) fn from_box(thresholds: Vec<f32>, lo_hi: Option<(Vec3, Vec3)>, class: ModelClass) -> Self {
        match lo_hi {
            Some((lo, hi)) => {
                let centre = (lo + hi) * 0.5;
                ObjectLod { thresholds, centre, half: hi - centre, class }
            }
            None => ObjectLod { thresholds, centre: Vec3::ZERO, half: Vec3::splat(Self::EMPTY_HALF), class },
        }
    }
}

#[derive(Clone)]
pub enum ObjectsSource {
    Track { track: String, drop: Option<u32> },
    Map { base: Option<String>, map: Found<Map>, terrain: Usage },
}

pub fn load(big_root: &Path, track: &str, drop: Option<u32>) -> Result<LoadedObjects> {
    load_from(big_root, &ObjectsSource::Track { track: track.to_string(), drop })
}

pub fn load_from(big_root: &Path, source: &ObjectsSource) -> Result<LoadedObjects> {
    let start = Instant::now();
    let content = GameContent::of(big_root);
    let mut stats = ObjectStats::default();
    let mut gather = Gather::default();
    let (mut parts, mut thresholds_of, mut placements) = (Vec::new(), Vec::new(), Vec::new());
    let (track, drop) = match source {
        ObjectsSource::Track { track, drop } => (Some(track.as_str()), *drop),
        ObjectsSource::Map { base, .. } => (base.as_deref(), None),
    };
    let mut disc = None;
    if let Some(track) = track {
        let vault = load_track_objects(big_root, track, drop)
            .with_context(|| format!("loading the objects of {track} from {}", big_root.display()))?;
        if let Some(selection) = &vault.selection {
            info!("{}", selection.summary());
        }
        stats.objects = vault.objects.len();
        let packages = content.track_packages(track);
        if packages.world_triangles.is_some() {
            stats.decision = Some(packages.summary());
        }
        let meshes_pass = packages.meshes(&content, &vault);
        for object in &vault.objects {
            let package =
                packages.object(object.key).and_then(|f| gather.package_object(&content, &packages, f, &mut stats));
            let o = package.unwrap_or_else(|| gather.disc_object(&content, meshes_pass, object, &mut stats));
            stats.objects_drawn += usize::from(o.lods.iter().any(|l| !l.is_empty()));
            parts.push(o.lods);
            thresholds_of.push(o.thresholds);
        }
        placements.extend(vault.instances.iter().flat_map(|(object, list)| list.iter().map(move |p| (*object, *p))));
        let used = meshes_pass.used;
        disc = Some((packages, vault, used));
    }
    let mut beside = Usage::default();
    if let ObjectsSource::Map { map, terrain, .. } = source {
        let none = MeshPass::default();
        let base = match &disc {
            Some((packages, vault, used)) => (packages.meshes(&content, vault), *used),
            None => (&none, Usage::default()),
        };
        let m = super::map::gather(&mut gather, &content, map, base, *terrain, &mut stats)?;
        let offset = parts.len();
        stats.objects += m.objects.len();
        for o in m.objects {
            stats.objects_drawn += usize::from(o.lods.iter().any(|l| !l.is_empty()));
            parts.push(o.lods);
            thresholds_of.push(o.thresholds);
        }
        placements.extend(m.placements.into_iter().map(|(o, p)| (o + offset, p)));
        info!("{}", m.summary);
        stats.map_drawn = Some(m.used);
        beside = m.used;
    }

    let loaded: Vec<(String, Mesh, (Vec3, Vec3))> = parallel_map(&gather.meshes, |job| -> Result<_> {
        let (mesh, label) = match job {
            MeshJob::Disc { id, label } => {
                let found = content.base_model(id)?;
                let mesh = found.value.lods.first().and_then(|l| l.parts.first()).map(|p| (*p.mesh).clone());
                (mesh.with_context(|| format!("{id}: no mesh"))?, label)
            }
            MeshJob::Ready { mesh, label } => ((**mesh).clone(), label),
        };
        let (lo, hi) = mesh.bounds.unwrap_or(([0.0; 3], [0.0; 3]));
        Ok((label.clone(), to_bevy_mesh(mesh, label), (Vec3::from_array(lo), Vec3::from_array(hi))))
    })
    .into_iter()
    .collect::<Result<_>>()?;
    let geom_bounds: Vec<(Vec3, Vec3)> = loaded.iter().map(|(_, _, b)| *b).collect();
    let meshes: Vec<(String, Mesh)> = loaded.into_iter().map(|(s, m, _)| (s, m)).collect();
    let lods: Vec<ObjectLod> = thresholds_of
        .iter()
        .zip(&parts)
        .map(|(object_thresholds, object_parts)| {
            let thresholds =
                (0..object_parts.len()).map(|k| object_thresholds.get(k).copied().unwrap_or(0.0)).collect();
            let lod0 = object_parts.first().map(Vec::as_slice).unwrap_or_default();
            let union = lod0.iter().map(|&(g, _)| geom_bounds[g]).reduce(|(lo, hi), (l, h)| (lo.min(l), hi.max(h)));
            let techniques = object_parts.iter().flatten().map(|&(_, m)| gather.materials[m].0.technique.as_deref());
            ObjectLod::from_box(thresholds, union, ModelClass::of(techniques))
        })
        .collect();

    let inputs: Vec<MaterialInput<'_>> =
        gather.materials.iter().map(|(def, tangent_sign)| MaterialInput { def, tangent_sign: *tangent_sign }).collect();
    let track_pass = disc.as_ref().map(|(packages, vault, _)| (&**packages, vault, beside));
    let (images, materials) = prepare_materials(big_root, &inputs, track_pass, &mut stats.problems)?;

    let placements: Vec<(usize, Placement)> = placements;
    for (object, _) in &placements {
        for &(_, material) in parts[*object].iter().flatten() {
            let effect = &gather.materials[material].0.effect;
            let effect = if effect.is_empty() { "(no effect)".to_string() } else { effect.clone() };
            *stats.effects.entry(effect).or_default() += 1;
            stats.entities += 1;
        }
    }
    stats.placements = placements.len();
    stats.meshes = meshes.len();
    stats.materials = materials.len();
    stats.textures = images.len();
    stats.load_secs = start.elapsed().as_secs_f32();
    let source = (big_root.to_path_buf(), source.clone());
    Ok(LoadedObjects { stats, meshes, images, materials, parts, lods, placements, source })
}

pub(super) fn parallel_map<T: Sync, R: Send>(items: &[T], f: impl Fn(&T) -> R + Sync) -> Vec<R> {
    let workers = std::thread::available_parallelism().map_or(4, |n| n.get()).min(items.len()).max(1);
    let next = AtomicUsize::new(0);
    let mut out: Vec<Option<R>> = (0..items.len()).map(|_| None).collect();
    std::thread::scope(|scope| {
        let jobs: Vec<_> = (0..workers)
            .map(|_| {
                scope.spawn(|| {
                    let mut done = Vec::new();
                    loop {
                        let i = next.fetch_add(1, Ordering::Relaxed);
                        let Some(item) = items.get(i) else { break };
                        done.push((i, f(item)));
                    }
                    done
                })
            })
            .collect();
        for job in jobs {
            for (i, r) in job.join().unwrap_or_else(|e| std::panic::resume_unwind(e)) {
                out[i] = Some(r);
            }
        }
    });
    out.into_iter().map(|r| r.unwrap_or_else(|| unreachable!("every index is taken once"))).collect()
}

pub(super) fn to_bevy_mesh(mesh: ObjectMesh, label: &str) -> Mesh {
    let mut out = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::default())
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, mesh.positions)
        .with_inserted_indices(Indices::U32(mesh.indices));
    if let Some(colours) = &mesh.colours {
        let lanes: Vec<[f32; 2]> = colours.iter().map(|c| [f32::from(c[2]) / 255.0, f32::from(c[1]) / 255.0]).collect();
        out.insert_attribute(Mesh::ATTRIBUTE_UV_1, lanes);
    }
    match mesh.normals {
        Some(normals) => {
            let unit = |n: [f32; 3]| Vec3::from_array(n).try_normalize().unwrap_or(Vec3::Y).to_array();
            out.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals.into_iter().map(unit).collect::<Vec<_>>());
        }
        None => out.compute_smooth_normals(),
    }
    if let Some(uv0) = mesh.uv0 {
        out.insert_attribute(Mesh::ATTRIBUTE_UV_0, uv0);
    }
    match mesh.tangents {
        Some(tangents) => {
            let signs = mesh.tangent_signs.unwrap_or_default();
            let tangents: Vec<[f32; 4]> = tangents
                .into_iter()
                .enumerate()
                .map(|(i, [x, y, z])| [x, y, z, signs.get(i).copied().unwrap_or(1.0)])
                .collect();
            out.insert_attribute(Mesh::ATTRIBUTE_TANGENT, tangents);
        }
        None if out.contains_attribute(Mesh::ATTRIBUTE_UV_0) => warn!("{label}: no g0; drawn without its normal map"),
        None => {}
    }
    out
}
