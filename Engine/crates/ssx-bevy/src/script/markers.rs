use std::collections::BTreeMap;
use std::sync::Arc;

use bevy::asset::RenderAssetUsages;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;
use ssx_assets::content::markers::MarkerLook;
use ssx_content::ContentId;
use ssx_content::marker::Facing;
use ssx_script::host::game::MarkerView;
use ssx_script::value::ElementId;

#[derive(Component)]
pub(super) struct MarkerVisual {
    key: MarkerKey,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum MarkerKey {
    Script(ElementId),
    Level(usize),
    Gate,
}

#[derive(Resource, Default)]
pub(crate) struct MarkerScene {
    looks: Arc<BTreeMap<ContentId, Arc<MarkerLook>>>,
    meshes: BTreeMap<(bool, ContentId), Handle<Mesh>>,
    pending: Option<Vec<MarkerView>>,
    placed: BTreeMap<MarkerKey, (Entity, MarkerView, Handle<StandardMaterial>)>,
    script: Vec<MarkerView>,
    level: Vec<MarkerView>,
    level_looks: BTreeMap<ContentId, Arc<MarkerLook>>,
    level_changed: bool,
    gate: Option<(GateAt, MarkerView)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum GateAt {
    Checkpoint(usize),
    Finish,
}

impl MarkerScene {
    pub(super) fn set_looks(&mut self, looks: Arc<BTreeMap<ContentId, Arc<MarkerLook>>>) {
        self.looks = looks;
        if let Some((at, mut v)) = self.gate.take() {
            self.colour_gate(&mut v);
            self.gate = Some((at, v));
            self.level_changed = true;
        }
        self.meshes.retain(|(level, _), _| *level);
        let all = self.script.clone();
        self.pending.get_or_insert(all);
    }

    pub(super) fn set_level(&mut self, package: &str, markers: &[crate::ride::MapMarker]) {
        self.level_looks = markers.iter().map(|m| (m.look.id.clone(), Arc::clone(&m.look))).collect();
        self.level = markers
            .iter()
            .enumerate()
            .map(|(k, m)| {
                let mut v = MarkerView::placed(ElementId(k as u64), package, m.look.id.clone(), m.position);
                (v.heading, v.size, v.colour) = (m.heading, m.look.def.size, m.look.def.colour);
                v
            })
            .collect();
        self.meshes.retain(|(level, _), _| !*level);
        self.level_changed = true;
    }

    fn colour_gate(&self, v: &mut MarkerView) {
        if let Some(l) = self.looks.get(&v.id) {
            v.colour = l.def.colour;
        }
    }

    fn look(&self, key: MarkerKey, id: &ContentId) -> Option<&Arc<MarkerLook>> {
        match key {
            MarkerKey::Script(_) | MarkerKey::Gate => self.looks.get(id),
            MarkerKey::Level(_) => self.level_looks.get(id),
        }
    }

    pub(super) fn set(&mut self, mut markers: Vec<MarkerView>) {
        markers.retain(|m| !m.is_level());
        self.pending = Some(markers);
    }

    pub(super) fn icon_points(&self) -> Vec<(ContentId, Vec3)> {
        self.placed
            .iter()
            .filter(|(_, (_, m, _))| m.visible)
            .filter_map(|(k, (_, m, _))| {
                let icon = self.look(*k, &m.id)?.def.icon.clone()?;
                Some((icon, Vec3::from_array(m.position) + Vec3::Y * 0.8))
            })
            .collect()
    }

    #[cfg(test)]
    pub(super) fn pending(&self) -> Option<usize> {
        self.pending.as_ref().map(Vec::len)
    }
}

fn mesh_of(look: &MarkerLook) -> Mesh {
    let m = &look.mesh;
    let mut out = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::RENDER_WORLD)
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, m.positions.clone())
        .with_inserted_indices(Indices::U32(m.indices.clone()));
    match &m.normals {
        Some(n) if n.len() == m.positions.len() => out.insert_attribute(Mesh::ATTRIBUTE_NORMAL, n.clone()),
        _ => out.compute_flat_normals(),
    }
    out
}

fn material(colour: u32, glow: f32) -> StandardMaterial {
    let [a, r, g, b] = colour.to_be_bytes();
    let base = Color::srgba_u8(r, g, b, a).to_linear();
    let lit = LinearRgba::new(base.red * (1.0 + glow), base.green * (1.0 + glow), base.blue * (1.0 + glow), base.alpha);
    StandardMaterial {
        base_color: lit.into(),
        unlit: true,
        alpha_mode: if a == 255 { AlphaMode::Opaque } else { AlphaMode::Blend },
        cull_mode: None,
        double_sided: true,
        ..default()
    }
}

pub(super) fn sync(
    mut scene: ResMut<MarkerScene>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    alive: Query<(), With<MarkerVisual>>,
) {
    let s = &mut *scene;
    s.placed.retain(|_, (e, _, _)| alive.contains(*e));
    match s.pending.take() {
        Some(list) => s.script = list,
        None if !s.level_changed => return,
        None => {}
    }
    s.level_changed = false;
    let script = s.script.iter().map(|m| (MarkerKey::Script(m.element), m.clone()));
    let level = s.level.iter().enumerate().map(|(k, m)| (MarkerKey::Level(k), m.clone()));
    let gate = s.gate.iter().map(|(_, m)| (MarkerKey::Gate, m.clone()));
    let list: BTreeMap<MarkerKey, MarkerView> = script.chain(level).chain(gate).collect();
    let gone: Vec<MarkerKey> = s.placed.keys().filter(|k| !list.contains_key(k)).copied().collect();
    for el in gone {
        if let Some((e, _, mat)) = s.placed.remove(&el) {
            commands.entity(e).despawn();
            materials.remove(&mat);
        }
    }
    for (el, m) in list {
        let Some(look) = s.look(el, &m.id).cloned() else { continue };
        let level = matches!(el, MarkerKey::Level(_));
        let mesh = s.meshes.entry((level, m.id.clone())).or_insert_with(|| meshes.add(mesh_of(&look))).clone();
        let base = Transform::from_translation(Vec3::from_array(m.position))
            .with_rotation(Quat::from_rotation_y(m.heading.to_radians()))
            .with_scale(Vec3::splat(m.size));
        let vis = if m.visible { Visibility::Inherited } else { Visibility::Hidden };
        match s.placed.get_mut(&el) {
            Some((e, old, mat)) => {
                if old.colour != m.colour
                    && let Some(mut x) = materials.get_mut(&*mat)
                {
                    *x = material(m.colour, look.def.glow);
                }
                commands.entity(*e).insert((base, vis, Mesh3d(mesh)));
                *old = m;
            }
            None => {
                let mat = materials.add(material(m.colour, look.def.glow));
                let name = match el {
                    MarkerKey::Script(e) => Name::new(format!("script marker {} ({})", e.0, m.id)),
                    MarkerKey::Level(k) => Name::new(format!("level marker {k} ({})", m.id)),
                    MarkerKey::Gate => Name::new(format!("next gate ({})", m.id)),
                };
                let e = commands
                    .spawn((name, MarkerVisual { key: el }, Mesh3d(mesh), MeshMaterial3d(mat.clone()), base, vis))
                    .id();
                s.placed.insert(el, (e, m, mat));
            }
        }
    }
}

pub(super) fn level(mut scene: ResMut<MarkerScene>, ride: Option<Res<crate::ride::Ride>>) {
    match ride {
        Some(r) if r.is_added() => match &r.map {
            Some(m) => {
                info!("markers: the map {}'s {} markers drawn with the scripts'", m.id, m.markers.len());
                scene.set_level(m.id.namespace(), &m.markers);
            }
            None => scene.set_level("", &[]),
        },
        None if !scene.level.is_empty() => scene.set_level("", &[]),
        _ => {}
    }
}

const GATE_LOOK: &str = "ava:marker/gate";

pub(super) fn gates(mut scene: ResMut<MarkerScene>, ride: Option<Res<crate::ride::Ride>>) {
    let at = ride.as_deref().and_then(next_gate);
    if at == scene.gate.as_ref().map(|(a, _)| *a) {
        return;
    }
    let mut view = match (at, ride.as_deref()) {
        (Some(a), Some(r)) => gate_view(r, a).map(|v| (a, v)),
        _ => None,
    };
    if let Some((_, v)) = view.as_mut() {
        scene.colour_gate(v);
    }
    scene.gate = view;
    scene.level_changed = true;
}

fn next_gate(ride: &crate::ride::Ride) -> Option<GateAt> {
    let m = ride.map.as_ref()?;
    if m.gates.checkpoints.is_empty() || ride.session.event.result.is_some() {
        return None;
    }
    if m.gates.next < m.gates.checkpoints.len() {
        return Some(GateAt::Checkpoint(m.gates.next));
    }
    m.gates.finish.map(|_| GateAt::Finish)
}

// PORT(choice): the base set's gate look at the volume's bottom centre, as wide as the volume (a box's X extent, a
// sphere's diameter; the look scales uniformly, so as tall), facing a box's +Z or, for a sphere, the way from the gate
// before it (else to the one after it, else +Z); tuning: how map authors want their gates shown (a marker look per map
// or gamemode would be the user's call); the game has no checkpoint.
fn gate_view(ride: &crate::ride::Ride, at: GateAt) -> Option<MarkerView> {
    use ssx_physics::triggers::VolumeShape;
    let m = ride.map.as_ref()?;
    let world = ride.world();
    let finish = || m.gates.finish.and_then(|i| world.triggers.volumes.get(i));
    let (v, before, after) = match at {
        GateAt::Checkpoint(k) => {
            let before = k.checked_sub(1).and_then(|b| m.gates.checkpoints.get(b)).map(|b| b.centre);
            let after = m.gates.checkpoints.get(k + 1).or_else(finish).map(|a| a.centre);
            (m.gates.checkpoints.get(k)?, before, after)
        }
        GateAt::Finish => (finish()?, m.gates.checkpoints.last().map(|b| b.centre), None),
    };
    let id = ContentId::parse(GATE_LOOK).ok()?;
    let (position, size, heading) = match v.shape {
        VolumeShape::Box { half_extents } => {
            let forward = v.rotation * Vec3::Z;
            let bottom = v.centre - v.rotation * Vec3::new(0.0, half_extents.y, 0.0);
            (bottom, 2.0 * half_extents.x, forward.x.atan2(forward.z).to_degrees())
        }
        VolumeShape::Sphere { radius } => {
            let way = before.map(|b| v.centre - b).or_else(|| after.map(|a| a - v.centre)).unwrap_or(Vec3::Z);
            let heading = if way.x.hypot(way.z) > 1e-3 { way.x.atan2(way.z).to_degrees() } else { 0.0 };
            (v.centre - Vec3::new(0.0, radius, 0.0), 2.0 * radius, heading)
        }
    };
    if !(size.is_finite() && size > 0.0 && position.is_finite() && heading.is_finite()) {
        return None;
    }
    let mut view = MarkerView::placed(ElementId(0), "", id, position.to_array());
    (view.heading, view.size) = (heading, size);
    Some(view)
}

pub(super) fn animate(
    scene: Res<MarkerScene>,
    time: Res<Time<Real>>,
    cameras: Query<&GlobalTransform, With<Camera3d>>,
    mut q: Query<(&MarkerVisual, &mut Transform, &mut Visibility)>,
) {
    let t = time.elapsed_secs();
    let eye = cameras.iter().next().map(GlobalTransform::translation);
    for (v, mut tr, mut vis) in &mut q {
        let Some((_, m, _)) = scene.placed.get(&v.key) else { continue };
        let Some(look) = scene.look(v.key, &m.id) else { continue };
        let d = &look.def;
        let a = &d.animation;
        let pos = Vec3::from_array(m.position);
        let bob = if a.bob > 0.0 { a.bob * (t * std::f32::consts::TAU / a.bob_period).sin() } else { 0.0 };
        let pulse =
            1.0 + if a.pulse > 0.0 { a.pulse * (t * std::f32::consts::TAU / a.pulse_period).sin() } else { 0.0 };
        let yaw = match (d.facing, eye) {
            (Facing::Camera, Some(e)) => (e.x - pos.x).atan2(e.z - pos.z),
            _ => (m.heading + a.spin * t).to_radians(),
        };
        tr.translation = pos + Vec3::Y * bob;
        tr.rotation = Quat::from_rotation_y(yaw);
        tr.scale = Vec3::splat(m.size * pulse);
        let near = eye.is_none_or(|e| e.distance(pos) <= d.draw_distance);
        let want = if m.visible && near { Visibility::Inherited } else { Visibility::Hidden };
        if *vis != want {
            *vis = want;
        }
    }
}

#[cfg(test)]
mod tests {
    use bevy::ecs::system::RunSystemOnce;

    use super::*;
    use crate::ride::MapMarker;

    fn look(name: &str) -> Arc<MarkerLook> {
        let id: ContentId = format!("pack:marker/{name}").parse().unwrap();
        let def = ssx_content::marker::parse_toml(b"shape = \"ring\"\nsize = 6.0\ncolor = \"#FF000080\"").unwrap();
        let mesh = ssx_content::model::Mesh {
            positions: vec![[0.0; 3], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            normals: Some(vec![[0.0, 0.0, 1.0]; 3]),
            indices: vec![0, 1, 2],
            ..Default::default()
        };
        let layer = ssx_content::Layer::ClientMods;
        Arc::new(MarkerLook { id, def, mesh: Arc::new(mesh), source: "pack".into(), layer })
    }

    fn world() -> World {
        let mut w = World::new();
        w.init_resource::<Assets<Mesh>>();
        w.init_resource::<Assets<StandardMaterial>>();
        w.init_resource::<MarkerScene>();
        w
    }

    fn drawn(w: &mut World) -> Vec<String> {
        let mut q = w.query_filtered::<&Name, With<MarkerVisual>>();
        let mut names: Vec<String> = q.iter(w).map(|n| n.as_str().to_string()).collect();
        names.sort();
        names
    }

    #[test]
    fn a_levels_markers_are_drawn_beside_the_scripts_and_go_with_it() {
        let mut w = world();
        let (ring, gate) = (look("ring"), look("gate"));
        let markers = vec![
            MapMarker { node: "ring".into(), look: Arc::clone(&ring), position: [1.0, 2.0, 3.0], heading: 90.0 },
            MapMarker { node: "gate".into(), look: Arc::clone(&gate), position: [0.0; 3], heading: 0.0 },
        ];
        w.resource_mut::<MarkerScene>().set_level("pack", &markers);
        w.run_system_once(sync).unwrap();
        assert_eq!(drawn(&mut w), ["level marker 0 (pack:marker/ring)", "level marker 1 (pack:marker/gate)"]);
        {
            let s = w.resource::<MarkerScene>();
            let (_, v, _) = &s.placed[&MarkerKey::Level(0)];
            assert_eq!((v.heading, v.size, v.colour, v.position), (90.0, 6.0, 0x80FF_0000, [1.0, 2.0, 3.0]));
        }
        let looks: BTreeMap<ContentId, Arc<MarkerLook>> = [(ring.id.clone(), Arc::clone(&ring))].into();
        let mut scene = w.resource_mut::<MarkerScene>();
        scene.set_looks(Arc::new(looks));
        scene.set(vec![MarkerView::placed(ElementId(0), "script", ring.id.clone(), [5.0; 3])]);
        w.run_system_once(sync).unwrap();
        assert_eq!(drawn(&mut w).len(), 3);
        w.resource_mut::<MarkerScene>().set(Vec::new());
        w.run_system_once(sync).unwrap();
        assert_eq!(drawn(&mut w).len(), 2);
        w.resource_mut::<MarkerScene>().set_level("", &[]);
        w.run_system_once(sync).unwrap();
        w.flush();
        assert!(drawn(&mut w).is_empty());
    }
}
