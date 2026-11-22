use std::any::TypeId;
use std::sync::Arc;

use bevy::asset::UntypedAssetId;
use bevy::camera::primitives::{Aabb, CascadesFrusta, Frustum, Sphere};
use bevy::camera::visibility::{CascadesVisibleEntities, NoAutoAabb, NoCpuCulling, VisibilitySystems, VisibleEntities};
use bevy::ecs::entity::EntityHashMap;
use bevy::light::{NotShadowCaster, SimulationLightSystems};
use bevy::pbr::wireframe::{NoWireframe, WireframeConfig};
use bevy::prelude::*;
use bevy::render::view::RetainedViewEntity;

use super::lods::{SlotView, TerrainLods, TerrainPart};
use super::material::TerrainMaterial;
use crate::render::shadows::GameSun;

pub(super) struct PathPart {
    mesh: Handle<Mesh>,
    aabb: Option<Aabb>,
    slot: Option<TerrainPart>,
    carrier: usize,
}

pub(super) enum CarrierMaterial {
    Terrain(Handle<TerrainMaterial>),
    Grey(Handle<StandardMaterial>),
}

impl CarrierMaterial {
    fn id(&self) -> UntypedAssetId {
        match self {
            CarrierMaterial::Terrain(h) => h.id().untyped(),
            CarrierMaterial::Grey(h) => h.id().untyped(),
        }
    }
}

pub(super) struct PathView {
    pub view: RetainedViewEntity,
    pub draws: Vec<(u32, u32)>,
}

#[derive(Resource, Default)]
pub(super) struct TerrainPath {
    parts: Vec<PathPart>,
    free: Vec<usize>,
    pub(super) meshes: Arc<[AssetId<Mesh>]>,
    materials_by_carrier: Vec<CarrierMaterial>,
    carrier_entities: Vec<Entity>,
    pub(super) materials: Arc<[UntypedAssetId]>,
    wireframes: EntityHashMap<usize>,
    pub(super) views: Vec<PathView>,
}

impl TerrainPath {
    pub(super) fn add_part(
        &mut self,
        mesh: Handle<Mesh>,
        aabb: Option<Aabb>,
        slot: Option<TerrainPart>,
        carrier: usize,
    ) -> usize {
        if slot.is_none() {
            self.free.push(self.parts.len());
        }
        self.parts.push(PathPart { mesh, aabb, slot, carrier });
        self.parts.len() - 1
    }

    pub(super) fn carrier(&mut self, material: CarrierMaterial) -> usize {
        let id = material.id();
        if let Some(i) = self.materials_by_carrier.iter().position(|m| m.id() == id) {
            return i;
        }
        self.materials_by_carrier.push(material);
        self.materials_by_carrier.len() - 1
    }

    pub(super) fn spawn_carriers(&mut self, commands: &mut Commands, mut layout: impl FnMut() -> Handle<Mesh>) {
        let boxes: Option<Vec<Aabb>> = self.parts.iter().map(|p| p.aabb).collect();
        let union = boxes.map(|boxes| {
            let (min, max) = boxes.iter().fold((Vec3::INFINITY, Vec3::NEG_INFINITY), |(min, max), b| {
                (min.min(b.min().into()), max.max(b.max().into()))
            });
            Aabb::from_min_max(min, max)
        });
        self.carrier_entities.clear();
        for (i, material) in self.materials_by_carrier.iter().enumerate() {
            let mut e =
                commands.spawn((Name::new(format!("terrain carrier {i}")), Mesh3d(layout()), NoAutoAabb, NoWireframe));
            match material {
                CarrierMaterial::Terrain(h) => e.insert(MeshMaterial3d(h.clone())),
                CarrierMaterial::Grey(h) => e.insert(MeshMaterial3d(h.clone())),
            };
            if let Some(aabb) = union {
                e.insert(aabb);
            }
            self.carrier_entities.push(e.id());
        }
        self.meshes = self.parts.iter().map(|p| p.mesh.id()).collect();
        self.materials = self.materials_by_carrier.iter().map(CarrierMaterial::id).collect();
    }

    pub(super) fn carrier_entities(&self) -> impl Iterator<Item = Entity> + '_ {
        self.carrier_entities.iter().copied()
    }
}

fn part_carrier(lods: Option<&TerrainLods>, part: &PathPart, view: SlotView) -> Option<usize> {
    let Some(slot) = &part.slot else { return Some(part.carrier) };
    lods?.carrier(slot, view, part.carrier)
}

fn camera_keeps(frustum: &Frustum, aabb: &Aabb) -> bool {
    let transform = GlobalTransform::IDENTITY;
    let world_from_local = transform.affine();
    let sphere = Sphere {
        center: world_from_local.transform_point3a(aabb.center),
        radius: transform.radius_vec3a(aabb.half_extents),
    };
    frustum.intersects_sphere(&sphere, false) && frustum.intersects_obb(aabb, &world_from_local, true, false)
}

fn cascade_keeps(frustum: &Frustum, aabb: &Aabb) -> bool {
    frustum.intersects_obb(aabb, &GlobalTransform::IDENTITY.affine(), false, true)
}

#[derive(Clone, Copy)]
enum Cull<'a> {
    Camera { frustum: &'a Frustum, all: bool },
    Cascade(&'a Frustum),
}

impl Cull<'_> {
    fn keeps(self, aabb: &Aabb) -> bool {
        match self {
            Cull::Camera { frustum, all } => all || camera_keeps(frustum, aabb),
            Cull::Cascade(frustum) => cascade_keeps(frustum, aabb),
        }
    }
}

pub(super) fn terrain_views(
    lods: Option<Res<TerrainLods>>,
    path: Option<ResMut<TerrainPath>>,
    mut cameras: Query<(Entity, &Frustum, Has<NoCpuCulling>, &mut VisibleEntities), With<Camera3d>>,
    mut lights: Query<(Entity, &CascadesFrusta, &mut CascadesVisibleEntities, Has<GameSun>)>,
) {
    let Some(mut path) = path else { return };
    let path = &mut *path;
    let mut views = std::mem::take(&mut path.views);
    views.clear();
    let lods = lods.as_deref();
    let chosen = lods.and_then(TerrainLods::camera);
    for (camera, frustum, no_cpu_culling, mut visible) in &mut cameras {
        let Some(list) = visible.entities.get_mut(&TypeId::of::<Mesh3d>()) else { continue };
        let view = if chosen == Some(camera) { SlotView::Main } else { SlotView::Other };
        let draws = view_draws(path, lods, view, list, Cull::Camera { frustum, all: no_cpu_culling });
        views.push(PathView { view: RetainedViewEntity::new(camera.into(), None, 0), draws });
    }
    for (light, frusta, mut visible, is_sun) in &mut lights {
        for (camera, cascades) in visible.entities.iter_mut() {
            let Some(frusta) = frusta.frusta.get(camera) else { continue };
            for (i, (cascade, frustum)) in cascades.iter_mut().zip(frusta).enumerate() {
                let view = if is_sun && chosen == Some(*camera) { SlotView::Cascade(i) } else { SlotView::Other };
                let draws = view_draws(path, lods, view, &mut cascade.entities, Cull::Cascade(frustum));
                let view = RetainedViewEntity::new(light.into(), Some((*camera).into()), i as u32);
                views.push(PathView { view, draws });
            }
        }
    }
    path.views = views;
}

fn view_draws(
    path: &TerrainPath,
    lods: Option<&TerrainLods>,
    view: SlotView,
    list: &mut Vec<Entity>,
    cull: Cull,
) -> Vec<(u32, u32)> {
    let kept: Vec<bool> = path.carrier_entities.iter().map(|e| list.binary_search(e).is_ok()).collect();
    let mut draws = Vec::new();
    let mut drawn = vec![false; kept.len()];
    if kept.contains(&true) {
        let candidates = path.free.iter().copied().chain(lods.into_iter().flat_map(|l| l.candidates(view)));
        for i in candidates {
            let Some(part) = path.parts.get(i) else { continue };
            if let Some(c) = part_carrier(lods, part, view)
                && kept.get(c).copied().unwrap_or(false)
                && part.aabb.as_ref().is_none_or(|aabb| cull.keeps(aabb))
            {
                draws.push((c as u32, i as u32));
                drawn[c] = true;
            }
        }
    }
    let mut out: Vec<usize> = path
        .carrier_entities
        .iter()
        .zip(&drawn)
        .filter(|&(_, &drawn)| !drawn)
        .filter_map(|(e, _)| list.binary_search(e).ok())
        .collect();
    out.extend(path.wireframes.iter().filter_map(|(e, &p)| {
        let shown = path.parts.get(p).is_some_and(|part| part_carrier(lods, part, view).is_some());
        if shown { None } else { list.binary_search(e).ok() }
    }));
    if !out.is_empty() {
        out.sort_unstable();
        let mut out = out.into_iter().peekable();
        let mut at = 0;
        list.retain(|_| {
            let taken = out.next_if_eq(&at).is_some();
            at += 1;
            !taken
        });
    }
    draws
}

// PORT(choice): the terrain's wireframe drawn from entities of its own while the debug wireframe (F, the launcher's
// "Wireframe" option) is on; Bevy 0.20's wireframe pass queues mesh entities with its own draw function, which the
// carriers' parts list does not reach. The console had no wireframe view.
fn terrain_wireframes(mut commands: Commands, config: Option<Res<WireframeConfig>>, path: Option<ResMut<TerrainPath>>) {
    let Some(mut path) = path else { return };
    let on = config.is_some_and(|c| c.global);
    if on == !path.wireframes.is_empty() || (on && path.parts.is_empty()) {
        return;
    }
    let path = &mut *path;
    if on {
        for (i, part) in path.parts.iter().enumerate() {
            let e = commands.spawn((
                Name::new(format!("terrain wireframe {i}")),
                Mesh3d(part.mesh.clone()),
                NotShadowCaster,
            ));
            path.wireframes.insert(e.id(), i);
        }
    } else {
        for (e, _) in path.wireframes.drain() {
            commands.entity(e).despawn();
        }
    }
}

pub(super) fn register(app: &mut App) {
    app.add_systems(Update, terrain_wireframes.after(crate::render::scene::toggle_wireframe)).add_systems(
        PostUpdate,
        terrain_views
            .after(SimulationLightSystems::CheckLightVisibility)
            .before(VisibilitySystems::MarkNewlyHiddenEntitiesInvisible),
    );
    app.add_plugins(super::draw::TerrainDrawPlugin);
    crate::shell::level::scoped::<TerrainPath>(app);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entity(index: u32) -> Entity {
        Entity::from_raw_u32(index).expect("a valid entity index")
    }

    #[test]
    fn a_views_list_keeps_its_order_and_loses_only_the_terrains_undrawn() {
        let mut path = TerrainPath::default();
        path.add_part(Handle::default(), None, None, 0);
        path.add_part(Handle::default(), None, None, 2);
        path.add_part(Handle::default(), None, Some(TerrainPart::new(0, 0, 0)), 0);
        path.carrier_entities = vec![entity(5), entity(9), entity(12)];
        path.wireframes.insert(entity(7), 0);
        path.wireframes.insert(entity(8), 2);
        let mut list = vec![entity(1), entity(5), entity(7), entity(8), entity(9), entity(11)];
        list.sort_unstable();
        let mut left = vec![entity(1), entity(5), entity(7), entity(11)];
        left.sort_unstable();
        let frustum = Frustum::default();
        let draws = view_draws(&path, None, SlotView::Main, &mut list, Cull::Camera { frustum: &frustum, all: true });
        assert_eq!(draws, vec![(0, 0)]);
        assert_eq!(list, left);
    }
}
