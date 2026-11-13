use bevy::asset::RenderAssetUsages;
use bevy::ecs::system::SystemParam;
use bevy::mesh::MeshTag;
use bevy::prelude::*;
use bevy::render::storage::ShaderBuffer;
use ssx_assets::track_objects::Placement;

use super::load::{ModelClass, ObjectLod};
use super::{LoadedObjects, MaterialHandle, PendingObjects, ReloadedObjects};
use crate::render::lighting::game::GameLightAssets;
use crate::render::objects::foreground::ForegroundMaterial;
use crate::render::objects::material::{ObjectMaterial, PLACEMENT_BUFFER, placement_bytes};

const VIEW_SIZE_CULL: f32 = 0.002;

const FADE_SLOPE: f32 = 50.0;

#[derive(Resource)]
pub struct ObjectLods {
    objects: Vec<ObjectLod>,
    slots: Vec<PlacementSlot>,
    buffer: Vec<[f32; 2]>,
    opaque: Vec<Option<Handle<ObjectMaterial>>>,
    low: Vec<Option<Handle<ObjectMaterial>>>,
    blend: Vec<Option<Handle<ObjectMaterial>>>,
    logged: (Vec<usize>, f32),
    by_position: std::collections::HashMap<[u32; 3], Vec<usize>>,
    source: (std::path::PathBuf, super::ObjectsSource),
}

impl ObjectLods {
    pub fn source(&self) -> &(std::path::PathBuf, super::ObjectsSource) {
        &self.source
    }
}

struct PlacementSlot {
    centre: Vec3,
    radius: f32,
    object: usize,
    placement: Placement,
    current: Transform,
    entities: Vec<Vec<(Entity, usize)>>,
    state: Option<(usize, Draw)>,
    /// A foreground mountain (terrain_foreground.fx): scenery past the course.
    foreground: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Draw {
    Base,
    LowLod,
    Fading,
}

const SPLIT_DEPTH: f32 = 400.0;

fn placement_bounds(lod: &ObjectLod, p: &Placement) -> (Vec3, f32) {
    transform_bounds(lod, &placement_transform(p))
}

fn transform_bounds(lod: &ObjectLod, t: &Transform) -> (Vec3, f32) {
    let centre = t.translation + t.rotation * (t.scale * lod.centre);
    let r = Mat3::from_quat(t.rotation);
    let e = t.scale * lod.half;
    let half = r.x_axis.abs() * e.x + r.y_axis.abs() * e.y + r.z_axis.abs() * e.z;
    (centre, half.length())
}

pub(super) fn follow_bodies(
    ride: Option<Res<crate::ride::Ride>>,
    lods: Option<ResMut<ObjectLods>>,
    mut transforms: Query<&mut Transform>,
) {
    let (Some(ride), Some(mut lods)) = (ride, lods) else { return };
    let Some(c) = ride.session.controllers.as_ref() else { return };
    let lods = &mut *lods;
    for e in c.entities.iter().filter(|e| e.body.is_some()) {
        let Some(slots) = lods.by_position.get(&e.placement_position.to_array().map(f32::to_bits)) else { continue };
        for &i in slots {
            let slot = &mut lods.slots[i];
            if slot.current.translation == e.position && slot.current.rotation == e.rotation {
                continue;
            }
            slot.current = Transform { translation: e.position, rotation: e.rotation, scale: slot.current.scale };
            (slot.centre, slot.radius) = transform_bounds(&lods.objects[slot.object], &slot.current);
            for &(entity, _) in slot.entities.iter().flatten() {
                if let Ok(mut t) = transforms.get_mut(entity) {
                    *t = slot.current;
                }
            }
        }
    }
}

pub fn placement_transform(p: &Placement) -> Transform {
    let [x, y, z, w] = p.rotation;
    Transform {
        translation: Vec3::from_array(p.position),
        rotation: Quat::from_xyzw(x, y, z, w),
        scale: Vec3::from_array(p.scale),
    }
}

#[derive(SystemParam)]
pub(super) struct SpawnAssets<'w> {
    meshes: ResMut<'w, Assets<Mesh>>,
    materials: ResMut<'w, Assets<ObjectMaterial>>,
    foreground_materials: ResMut<'w, Assets<ForegroundMaterial>>,
    images: ResMut<'w, Assets<Image>>,
    buffers: ResMut<'w, Assets<ShaderBuffer>>,
    light: Res<'w, GameLightAssets>,
    post: Option<Res<'w, crate::render::post::PostSettings>>,
}

pub(super) fn spawn_objects(mut commands: Commands, mut pending: ResMut<PendingObjects>, assets: SpawnAssets) {
    let Some(loaded) = pending.0.take() else { return };
    spawn(&mut commands, loaded, assets);
    commands.remove_resource::<PendingObjects>();
}

pub(super) fn respawn_objects(
    mut commands: Commands,
    mut reloaded: ResMut<ReloadedObjects>,
    old: Option<Res<ObjectLods>>,
    assets: SpawnAssets,
) {
    let Some(loaded) = reloaded.0.take() else { return };
    for slot in old.iter().flat_map(|o| &o.slots) {
        for &(entity, _) in slot.entities.iter().flatten() {
            commands.entity(entity).despawn();
        }
    }
    spawn(&mut commands, loaded, assets);
    commands.remove_resource::<ReloadedObjects>();
}

fn spawn(commands: &mut Commands, loaded: LoadedObjects, mut a: SpawnAssets) {
    let source = loaded.source.clone();
    let stats = &loaded.stats;
    info!(
        "Loaded track objects in {:.2}s: {} of {} game objects drawn, {} placements -> {} entities (all LODs), \
         {} meshes, {} materials, {} textures",
        stats.load_secs,
        stats.objects_drawn,
        stats.objects,
        stats.placements,
        stats.entities,
        stats.meshes,
        stats.materials,
        stats.textures,
    );
    for problem in &stats.problems {
        warn!("{problem}");
    }
    if let Some(d) = &stats.decision {
        info!("track objects: {d}");
    }
    for p in &stats.package_objects {
        info!("track objects: drawn from a package: {p}");
    }
    let image_handles: Vec<Handle<Image>> = loaded
        .images
        .into_iter()
        .map(|(image, recipe)| crate::render::reload::add(&mut a.images, image, Some(recipe)))
        .collect();
    let material_handles: Vec<MaterialHandle> = loaded
        .materials
        .iter()
        .map(|m| m.add(&image_handles, &a.light, &mut a.materials, &mut a.foreground_materials))
        .collect();
    let blend: Vec<Option<Handle<ObjectMaterial>>> = loaded
        .materials
        .iter()
        .map(|m| m.add_low(&image_handles, &a.light, &mut a.materials, true, a.post.is_some()))
        .collect();
    let low: Vec<Option<Handle<ObjectMaterial>>> =
        loaded.materials.iter().map(|m| m.add_low(&image_handles, &a.light, &mut a.materials, false, false)).collect();
    let opaque: Vec<Option<Handle<ObjectMaterial>>> = material_handles
        .iter()
        .map(|h| if let MaterialHandle::Object(m) = h { Some(m.clone()) } else { None })
        .collect();
    let mesh_handles: Vec<(Name, Handle<Mesh>)> =
        loaded.meshes.into_iter().map(|(label, mesh)| (Name::new(label), a.meshes.add(mesh))).collect();

    let mut buffer = vec![[1.0, 1.0]];
    let mut slots = Vec::with_capacity(loaded.placements.len());
    for (object, placement) in &loaded.placements {
        let lods = &loaded.parts[*object];
        if lods.iter().all(Vec::is_empty) {
            continue;
        }
        let transform = placement_transform(placement);
        let slot = buffer.len();
        buffer.push([placement.unk[2].max(0.0), 1.0]);
        let tag = MeshTag::new(slot as u32);
        let mut entities = Vec::with_capacity(lods.len());
        for lod in lods {
            let mut lod_entities = Vec::with_capacity(lod.len());
            for &(mesh, material) in lod {
                let (name, mesh) = (mesh_handles[mesh].0.clone(), Mesh3d(mesh_handles[mesh].1.clone()));
                let id = match &material_handles[material] {
                    MaterialHandle::Object(m) => commands
                        .spawn((name, mesh, MeshMaterial3d(m.clone()), transform, tag.clone(), Visibility::Hidden))
                        .id(),
                    MaterialHandle::Foreground(m) => {
                        commands.spawn((name, mesh, MeshMaterial3d(m.clone()), transform, Visibility::Hidden)).id()
                    }
                };
                lod_entities.push((id, material));
            }
            entities.push(lod_entities);
        }
        let (centre, radius) = placement_bounds(&loaded.lods[*object], placement);
        let foreground =
            lods.iter().flatten().any(|&(_, m)| matches!(material_handles[m], MaterialHandle::Foreground(_)));
        slots.push(PlacementSlot {
            foreground,
            centre,
            radius,
            object: *object,
            placement: *placement,
            current: transform,
            entities,
            state: None,
        });
    }
    let pairs: std::collections::BTreeSet<(usize, usize)> =
        loaded.parts.iter().flatten().flatten().map(|&(mesh, material)| (mesh, material)).collect();
    let warm = super::warm_up::spawn(
        commands,
        pairs.into_iter().map(|(mesh, m)| {
            (mesh_handles[mesh].1.clone(), material_handles[m].clone(), [low[m].clone(), blend[m].clone()])
        }),
    );
    info!("object pipelines warmed up by {warm} draws of {} meshes", mesh_handles.len());
    let mut by_position: std::collections::HashMap<[u32; 3], Vec<usize>> = std::collections::HashMap::new();
    for (i, slot) in slots.iter().enumerate() {
        by_position.entry(slot.placement.position.map(f32::to_bits)).or_default().push(i);
    }
    let data = ShaderBuffer::new(placement_bytes(&buffer), RenderAssetUsages::default());
    if let Err(e) = a.buffers.insert(PLACEMENT_BUFFER.id(), data) {
        error!("placement buffer: {e}");
    }
    let logged = (Vec::new(), f32::NEG_INFINITY);
    let classes = [ModelClass::Prelight, ModelClass::Forward, ModelClass::Translucent, ModelClass::Empty]
        .map(|c| slots.iter().filter(|s| loaded.lods[s.object].class == c).count());
    info!(
        "object placements by instance bucket: prelight {}, forward {}, translucent {}, empty (not drawn) {}",
        classes[0], classes[1], classes[2], classes[3]
    );
    commands.insert_resource(ObjectLods {
        objects: loaded.lods,
        slots,
        buffer,
        opaque,
        low,
        blend,
        logged,
        by_position,
        source,
    });
}

/// A port fix: whether the camera is over a foreground mountain's footprint (inside its box across its own ground
/// plane, 5 % wider). Such a mountain is placed over the course (Silverthrone's): it is not drawn while the camera is
/// there, where it stood as ghost mountains around the run (they have no collision), and drawn again from outside.
fn over_footprint(lod: &ObjectLod, t: &Transform, eye: Vec3) -> bool {
    let local = t.compute_affine().inverse().transform_point3(eye) - lod.centre;
    let half = lod.half * 1.05;
    local.x.abs() <= half.x && local.z.abs() <= half.z
}

fn choose_lod(thresholds: &[f32], metric: f32) -> (Option<usize>, f32) {
    let lod = if metric > VIEW_SIZE_CULL { thresholds.iter().position(|&t| t <= metric.max(0.0)) } else { None };
    let fade = thresholds.last().map_or(1.0, |&last| ((metric - last) * FADE_SLOPE).min(1.0));
    (lod, fade)
}

fn fade_alpha(fade: f32) -> f32 {
    f32::from((fade * 1000.0) as i64 as u16) * 0.001
}

pub(super) fn update_object_lods(
    lods: Option<ResMut<ObjectLods>>,
    cameras: Query<(&Camera, &GlobalTransform), With<Camera3d>>,
    mut visibility: Query<&mut Visibility>,
    mut object_materials: Query<&mut MeshMaterial3d<ObjectMaterial>>,
    mut buffers: ResMut<Assets<ShaderBuffer>>,
    time: Res<Time>,
) {
    let Some(mut lods) = lods else { return };
    let Some((camera, view)) = cameras.iter().find(|(c, _)| c.is_active) else { return };
    let clip = camera.clip_from_view();
    let (tan_x, tan_y) = (1.0 / clip.x_axis.x.abs(), 1.0 / clip.y_axis.y.abs());
    let narrower = tan_x < tan_y * crate::camera::GAME_ASPECT * (1.0 - 1e-4);
    let tan_min =
        if narrower { tan_x / crate::camera::GAME_ASPECT } else { 1.0 / clip.x_axis.x.abs().max(clip.y_axis.y.abs()) };
    if !(tan_min.is_finite() && tan_min > 0.0) {
        return;
    }
    let eye = view.translation();
    let forward = view.forward().as_vec3();
    let lods = &mut *lods;
    let max_lods = lods.objects.iter().map(|o| o.thresholds.len()).max().unwrap_or(0);
    let mut counts = vec![0usize; max_lods + 3];
    let mut buffer_changed = false;
    for (i, slot) in lods.slots.iter_mut().enumerate() {
        let distance = slot.centre.distance(eye);
        let metric = if distance > 0.0 { slot.radius / (distance * tan_min) } else { f32::INFINITY };
        let object = &lods.objects[slot.object];
        let (lod, fade) = choose_lod(&object.thresholds, metric);
        let lod = lod.filter(|&k| k < slot.entities.len());
        let lod = lod.filter(|_| !(slot.foreground && over_footprint(object, &slot.current, eye)));
        let near_depth = (slot.centre - eye).dot(forward) - slot.radius;
        let draw = match object.class {
            ModelClass::Prelight if fade < 1.0 => Draw::Fading,
            ModelClass::Prelight if near_depth >= SPLIT_DEPTH => Draw::LowLod,
            _ => Draw::Base,
        };
        let state = lod.map(|k| (k, draw));
        match state {
            Some((k, draw)) => {
                counts[k + 1] += 1;
                counts[max_lods + 1] += usize::from(draw == Draw::Fading);
                counts[max_lods + 2] += usize::from(draw == Draw::LowLod);
            }
            None => counts[0] += 1,
        }
        let alpha = if state.is_some_and(|(_, d)| d == Draw::Fading) { fade_alpha(fade) } else { 1.0 };
        if lods.buffer[i + 1][1] != alpha {
            lods.buffer[i + 1][1] = alpha;
            buffer_changed = true;
        }
        if state == slot.state {
            continue;
        }
        if let Some((k, _)) = slot.state {
            for &(e, _) in &slot.entities[k] {
                if let Ok(mut v) = visibility.get_mut(e) {
                    *v = Visibility::Hidden;
                }
            }
        }
        if let Some((k, draw)) = state {
            for &(e, material) in &slot.entities[k] {
                if let Ok(mut v) = visibility.get_mut(e) {
                    *v = Visibility::Inherited;
                }
                let handle = match draw {
                    Draw::Base => &lods.opaque[material],
                    Draw::LowLod => &lods.low[material],
                    Draw::Fading => &lods.blend[material],
                };
                if let (Some(handle), Ok(mut m)) = (handle, object_materials.get_mut(e))
                    && m.0 != *handle
                {
                    m.0 = handle.clone();
                }
            }
        }
        slot.state = state;
    }
    if buffer_changed && let Some(mut b) = buffers.get_mut(&PLACEMENT_BUFFER) {
        b.clear();
        b.extend_from_slice(&placement_bytes(&lods.buffer));
    }
    let now = time.elapsed_secs();
    if counts != lods.logged.0 && now - lods.logged.1 >= 1.0 {
        let per_lod: Vec<String> = (0..max_lods).map(|k| format!("LOD{k} {}", counts[k + 1])).collect();
        info!(
            "object LODs at {eye:.0}: {} | not drawn {} | fading {} | forward_low_lod {} (of {} placements)",
            per_lod.join(", "),
            counts[0],
            counts[max_lods + 1],
            counts[max_lods + 2],
            lods.slots.len()
        );
        lods.logged = (counts, now);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lod_choice_follows_0x82d474c8() {
        let t = [0.6, 0.4, 0.2, 0.02];
        assert_eq!(choose_lod(&t, 0.7).0, Some(0));
        assert_eq!(choose_lod(&t, 0.4).0, Some(1));
        assert_eq!(choose_lod(&t, 0.03).0, Some(3));
        assert_eq!(choose_lod(&t, 0.019).0, None);
        assert_eq!(choose_lod(&[0.0], 0.0021).0, Some(0));
        assert_eq!(choose_lod(&[0.0], 0.002).0, None);
        assert_eq!(choose_lod(&t, 0.5).1, 1.0);
        assert!((choose_lod(&t, 0.03).1 - 0.5).abs() < 1e-5);
        assert_eq!(fade_alpha(0.5), 0.5);
        assert_eq!(fade_alpha(0.9999), 999.0 * 0.001);
    }

    #[test]
    fn bounds_rotate_the_box() {
        let lod = ObjectLod::from_box(
            vec![0.0],
            Some((Vec3::new(-1.0, 0.0, -2.0), Vec3::new(1.0, 4.0, 2.0))),
            ModelClass::Prelight,
        );
        let q = Quat::from_rotation_y(std::f32::consts::FRAC_PI_2);
        let p = Placement {
            scale: [2.0; 3],
            rotation: [q.x, q.y, q.z, q.w],
            position: [10.0, 0.0, 0.0],
            unk: [1.0, 1.0, 1.0, 0.0, 0.0, 0.0],
        };
        let (c, r) = placement_bounds(&lod, &p);
        assert!(c.abs_diff_eq(Vec3::new(10.0, 4.0, 0.0), 1e-5));
        assert!((r - 6.0).abs() < 1e-5);
    }
}
