use std::path::{Component, Path};

use anyhow::{Result, anyhow, bail};
use bevy::asset::RenderAssetUsages;
use bevy::light::{NotShadowCaster, NotShadowReceiver};
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::pbr::{ExtendedMaterial, MaterialExtension};
use bevy::prelude::*;
use bevy::render::render_resource::AsBindGroup;
use bevy::render::storage::ShaderBuffer;
use bevy::shader::ShaderRef;
use ssx_assets::object_mesh::ObjectMesh;

use crate::render::lighting::game::LEVEL_LIGHT;

const SHADER_PATH: &str = "embedded://ssx_bevy/render/terrain/proxy.wesl";
const VERTEX_SHADER_PATH: &str = "embedded://ssx_bevy/render/terrain/proxy_vertex.wesl";
const EFFECT: &str = "terrain_proxy";
const TECHNIQUE: &str = "forward";

pub type ProxyMaterial = ExtendedMaterial<StandardMaterial, ProxyExtension>;

#[derive(Asset, AsBindGroup, Reflect, Debug, Clone)]
pub struct ProxyExtension {
    #[storage(100, read_only)]
    pub light: Handle<ShaderBuffer>,
    #[uniform(101)]
    pub reach: Vec4,
}

fn reach_uniform(reach: &super::Reach) -> Vec4 {
    Vec4::new(reach.proxy_push / super::lods::PROXY_BAND, reach.proxy_clip, 0.0, 0.0)
}

pub(super) fn follow_view_distance(
    distance: Option<Res<super::ViewDistance>>,
    mut materials: ResMut<Assets<ProxyMaterial>>,
) {
    let Some(distance) = distance.filter(|d| d.is_changed()) else { return };
    let wanted = reach_uniform(&super::reach(distance.meters()));
    let ids: Vec<_> = materials.iter().filter(|(_, m)| m.extension.reach != wanted).map(|(id, _)| id).collect();
    for id in ids {
        if let Some(mut m) = materials.get_mut(id) {
            m.extension.reach = wanted;
        }
    }
}

impl MaterialExtension for ProxyExtension {
    fn vertex_shader() -> ShaderRef {
        VERTEX_SHADER_PATH.into()
    }

    fn fragment_shader() -> ShaderRef {
        SHADER_PATH.into()
    }

    fn enable_prepass() -> bool {
        false
    }

    fn enable_shadows() -> bool {
        false
    }
}

pub(super) fn load(big_root: &Path, track_dir: &Path, track: &str) -> Result<Option<ObjectMesh>> {
    let Some(geometry) = ssx_assets::track_objects::load_track_proxy(big_root, track)? else { return Ok(None) };
    let parts: Vec<&str> = geometry.split(['\\', '/']).collect();
    let plain = |p: &&str| matches!(Path::new(p).components().collect::<Vec<_>>()[..], [Component::Normal(_)]);
    if !parts.iter().all(plain) {
        bail!("{track}: proxy path {geometry:?} is not a relative path of plain names");
    }
    let explorer = track_dir.parent().ok_or_else(|| anyhow!("{} has no parent folder", track_dir.display()))?;
    let path = parts.iter().fold(explorer.to_path_buf(), |p, part| p.join(part));
    let mesh = ssx_assets::object_mesh::load(&path)?;
    if mesh.normals.is_none() {
        bail!("{}: no n0 attribute", path.display());
    }
    Ok(Some(mesh))
}

pub(crate) struct ProxyCut {
    pub name: String,
    pub triangles: usize,
    groups: Vec<std::result::Result<Mesh, String>>,
}

pub(crate) fn cut(proxy: ObjectMesh) -> ProxyCut {
    let whole = std::iter::once(0..(proxy.indices.len() / 3) as u32);
    let groups: Vec<_> = if proxy.groups.is_empty() { whole.collect() } else { proxy.groups.clone() };
    let groups = groups
        .iter()
        .enumerate()
        .map(|(g, range)| {
            let Some(triangles) = proxy.indices.get(range.start as usize * 3..range.end as usize * 3) else {
                return Err(format!("proxy group {g} ({range:?}) past the {} indices; left out", proxy.indices.len()));
            };
            group_mesh(&proxy, triangles).ok_or_else(|| {
                format!("proxy group {g}: an index past the {} vertices; left out", proxy.positions.len())
            })
        })
        .collect();
    ProxyCut { name: proxy.name.clone(), triangles: proxy.indices.len() / 3, groups }
}

pub(super) fn spawn(
    commands: &mut Commands,
    proxy: ProxyCut,
    (meshes, uploads): &mut (&mut Assets<Mesh>, Option<&mut crate::render::uploads::LevelUploads>),
    materials: &mut Assets<ProxyMaterial>,
    reach: &super::Reach,
) -> usize {
    let cull = crate::render::fx_states::cull(EFFECT, TECHNIQUE).unwrap_or_else(|e| {
        warn!("{EFFECT} {TECHNIQUE}: {e:#}; back faces culled");
        Some(bevy::render::render_resource::Face::Back)
    });
    let material = materials.add(ProxyMaterial {
        base: StandardMaterial { double_sided: cull.is_none(), cull_mode: cull, ..default() },
        extension: ProxyExtension { light: LEVEL_LIGHT, reach: reach_uniform(reach) },
    });
    let mut spawned = 0;
    for (g, group) in proxy.groups.into_iter().enumerate() {
        let mesh = match group {
            Ok(mesh) => mesh,
            Err(why) => {
                warn!("{why}");
                continue;
            }
        };
        commands.spawn((
            Name::new(format!("{} group {g}", proxy.name)),
            Mesh3d(crate::render::uploads::add_mesh(uploads.as_deref_mut(), meshes, mesh)),
            MeshMaterial3d(material.clone()),
            Transform::IDENTITY,
            NotShadowCaster,
            NotShadowReceiver,
        ));
        spawned += 1;
    }
    spawned
}

fn group_mesh(proxy: &ObjectMesh, triangles: &[u32]) -> Option<Mesh> {
    let normals = proxy.normals.as_ref()?;
    let mut remap = std::collections::HashMap::new();
    let mut order = Vec::new();
    let mut indices = Vec::with_capacity(triangles.len());
    for &i in triangles {
        if i as usize >= proxy.positions.len() || i as usize >= normals.len() {
            return None;
        }
        let next = remap.len() as u32;
        let local = *remap.entry(i).or_insert_with(|| {
            order.push(i as usize);
            next
        });
        indices.push(local);
    }
    let colours: Vec<[f32; 4]> = order
        .iter()
        .map(|&i| proxy.colours.as_ref().and_then(|c| c.get(i)).map_or([1.0; 4], |c| c.map(|b| f32::from(b) / 255.0)))
        .collect();
    let mut mesh = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::RENDER_WORLD);
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, order.iter().map(|&i| proxy.positions[i]).collect::<Vec<_>>());
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, order.iter().map(|&i| normals[i]).collect::<Vec<_>>());
    mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, colours);
    mesh.insert_indices(Indices::U32(indices));
    Some(mesh)
}
