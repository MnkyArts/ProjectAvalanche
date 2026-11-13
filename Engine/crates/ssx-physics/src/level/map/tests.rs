use std::collections::BTreeMap;
use std::sync::Arc;

use glam::Vec3;
use ssx_content::map::{BoundaryArea, Map, VolumeShape, read_map, template};
use ssx_content::model::FileReader;

use super::*;
use crate::collision::CollisionWorld;
use crate::session::Session;

struct Scene(String);

impl FileReader for Scene {
    fn read(&self, path: &str, _max: u64) -> Result<Vec<u8>, String> {
        if path == "maps/course.gltf" { Ok(self.0.clone().into_bytes()) } else { Err(format!("{path}: absent")) }
    }
    fn size(&self, path: &str) -> Result<u64, String> {
        self.read(path, u64::MAX).map(|b| b.len() as u64)
    }
    fn read_range(&self, path: &str, offset: u64, len: u64) -> Result<Vec<u8>, String> {
        let b = self.read(path, u64::MAX)?;
        let o = usize::try_from(offset).map_err(|e| e.to_string())?;
        let l = usize::try_from(len).map_err(|e| e.to_string())?;
        b.get(o..o.saturating_add(l)).map(<[u8]>::to_vec).ok_or_else(|| "short".into())
    }
}

fn template_objects() -> BTreeMap<ssx_content::ContentId, ssx_assets::content::maps::MapObject> {
    let id: ssx_content::ContentId = template::FLARE.parse().expect("an id");
    let object = ssx_assets::content::maps::MapObject {
        id: id.clone(),
        source: "ssx".into(),
        layer: ssx_content::Layer::Base,
        collision: None,
        pob: None,
        model: None,
        audio: None,
        key: None,
        controller: None,
        rigid_body: None,
    };
    BTreeMap::from([(id, object)])
}

fn template_map() -> Map {
    read_map(template::MAP_TOML.as_bytes(), "map.toml", &Scene(template::gltf())).expect("the template reads")
}

#[test]
fn the_template_becomes_terrain_and_rail_clusters_as_the_disc_has_them() {
    let map = template_map();
    let mut c = CollisionWorld::default();
    let stats = add_map_collision(&mut c, &map.definition, &template_objects()).expect("collision");
    c.finish();
    assert_eq!((stats.terrain_triangles, stats.rail_triangles, stats.dropped), (432, 12, 0));
    let terrain: Vec<_> = c.clusters.iter().filter(|k| k.surface_id == TERRAIN_SURFACE).collect();
    assert_eq!(terrain.len(), stats.terrain_clusters);
    assert!(terrain.iter().all(|k| k.is_terrain() && k.class == 0 && usize::from(k.tri_count) <= CLUSTER_TRIANGLES));
    let rails: Vec<_> = c.clusters.iter().filter(|k| k.class == 12).collect();
    let metal = ssx_content::model::collision::surface_id("metal").expect("a surface");
    assert!(!rails.is_empty() && rails.iter().all(|k| k.is_terrain() && k.surface_id == metal));
    let hit = c.ray_cast(Vec3::new(3.0, 100.0, 47.0), -Vec3::Y, 500.0).expect("ground");
    assert!((hit.point.y - template::ground(47.0)).abs() < 0.02, "{}", hit.point);
    assert!(hit.normal.y > 0.9, "the welded normals face up: {}", hit.normal);
    let first = terrain[0].vert_start as usize;
    assert_eq!(c.nibbles[first], 0x000F_0F0F);
}

#[test]
fn the_triangle_cap_counts_the_placed_objects() {
    let mut map = template_map();
    let id: ssx_content::ContentId = "pack:object/rock".parse().expect("an id");
    map.definition.props.push(ssx_content::map::Prop {
        node: "course/rock".into(),
        look: ssx_content::map::PropLook::Object(id.clone()),
        placement: ssx_content::map::Placement { scale: [1.0; 3], rotation: [0.0, 0.0, 0.0, 1.0], position: [0.0; 3] },
        ao: 1.0,
        gltf_node: 0,
        material: None,
    });
    let mut c = CollisionWorld::default();
    let err = add_map_collision(&mut c, &map.definition, &template_objects()).expect_err("unresolved");
    assert!(err.to_string().contains("course/rock"), "{err}");
    let part = ssx_content::model::CollisionCluster {
        vertices: vec![[0.0; 3], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]],
        triangles: vec![[0, 2, 1]; 60_000],
        surface_id: 10,
        group: 0,
        edges: vec![ssx_content::model::TriangleEdges { flags: 0x113, edge_cos: [1.0; 3] }; 60_000],
        material: [0.2, 0.2, 0.1],
    };
    let rock = ssx_content::model::Collision { clusters: vec![part; 31], cylinders: 0 };
    let object = ssx_assets::content::maps::MapObject {
        id: id.clone(),
        source: "pack".into(),
        layer: ssx_content::Layer::ClientMods,
        collision: Some(Arc::new(rock)),
        pob: None,
        model: None,
        audio: None,
        key: None,
        controller: None,
        rigid_body: None,
    };
    let mut objects = template_objects();
    objects.insert(id, object);
    let err = add_map_collision(&mut c, &map.definition, &objects).expect_err("over the cap");
    assert!(err.to_string().contains("at most 1824266"), "{err}");
    assert!(c.clusters.is_empty(), "refused before anything is added");
}

#[test]
fn the_boundary_grid_reads_as_a_disc_grid() {
    let area = BoundaryArea {
        triangles: vec![[[0.0, 0.0], [100.0, 0.0], [100.0, 100.0]], [[0.0, 0.0], [100.0, 100.0], [0.0, 100.0]]],
        ..BoundaryArea::default()
    };
    let raster = ssx_content::map::boundary_raster(&area).expect("a grid");
    let g = boundary_grid(&raster);
    assert_eq!(g.cell(Vec3::new(50.0, 7.0, 50.0)), 0);
    let far = g.cell(Vec3::new(-500.0, 0.0, 50.0));
    assert_ne!(far & 0xC000, 0);
    let d = crate::rider::boundary::cell_direction(far);
    assert!(d.x > 0.99, "{d}");
}

fn checkpoint(z: f32, order: u32) -> ssx_content::map::Volume {
    let shape = VolumeShape::Sphere { centre: [0.0, 0.0, z], radius: 5.0 };
    ssx_content::map::Volume { node: format!("cp{order}"), name: "checkpoint".into(), order, shape }
}

#[test]
fn a_start_inside_a_death_volume_names_it() {
    let mut world = crate::testutil::world(CollisionWorld::default());
    let below = ssx_content::map::Volume {
        node: "course/below".into(),
        name: "death".into(),
        order: 0,
        shape: VolumeShape::Box { centre: [0.0, -1.5, 5.0], rotation: [0.0, 0.0, 0.0, 1.0], half_extents: [10.0; 3] },
    };
    world.triggers.volumes.push(trigger_volume(&below, "death", 0));
    let deaths = [below];
    let inside = deaths_at(&world.triggers, &deaths, Vec3::new(0.0, -0.4, 5.0));
    assert_eq!((inside.nodes.as_slice(), inside.base), (["course/below".to_string()].as_slice(), 0));
    assert!(!inside.is_empty());
    let beside =
        deaths_at(&world.triggers, &deaths, Vec3::new(0.0, 8.5 + crate::triggers::PLAYER_TOUCH_RADIUS + 0.01, 5.0));
    assert!(beside.is_empty(), "{beside:?}");
    let base = ssx_content::map::Volume {
        node: String::new(),
        name: "death".into(),
        order: 0,
        shape: VolumeShape::Sphere { centre: [100.0, 0.0, 0.0], radius: 5.0 },
    };
    world.triggers.volumes.push(trigger_volume(&base, "death", 1));
    let at_base = deaths_at(&world.triggers, &deaths, Vec3::new(100.0, 0.0, 0.0));
    assert_eq!((at_base.nodes.len(), at_base.base), (0, 1));
    let gate = trigger_volume(&checkpoint(300.0, 0), "", 0);
    world.triggers.volumes.push(gate);
    assert!(deaths_at(&world.triggers, &deaths, Vec3::new(0.0, 0.0, 300.0)).is_empty());
}

#[test]
fn the_finish_opens_after_the_gates_in_order() {
    let mut world = crate::testutil::world(CollisionWorld::default());
    let finish = ssx_content::map::Volume {
        node: "finish".into(),
        name: "end".into(),
        order: 0,
        shape: VolumeShape::Box { centre: [0.0, 0.0, 100.0], rotation: [0.0, 0.0, 0.0, 1.0], half_extents: [9.0; 3] },
    };
    world.triggers.volumes.push(trigger_volume(&finish, FINISH_GROUPS, 0));
    let gates = vec![trigger_volume(&checkpoint(20.0, 0), "", 0), trigger_volume(&checkpoint(60.0, 1), "", 1)];
    let mut g = Gates::new(gates, Some(0), Some(30_000));
    let mut rs = Session::default();
    g.reset(&mut rs);
    let in_finish = Vec3::new(0.0, 0.0, 100.0);
    let touches_end = |rs: &mut Session, w: &World| {
        rs.tracker.update(&w.triggers, in_finish);
        rs.tracker.touching_name(&w.triggers, "end")
    };
    assert!(!touches_end(&mut rs, &world), "closed while a gate is left");
    assert_eq!(g.update(&mut rs, Vec3::new(0.0, 0.0, 22.0)).passed, None, "not before the start");
    let run = |rs: &mut Session| {
        rs.event.state = crate::event::EventState::Running;
        rs.event.clock.running = true;
    };
    run(&mut rs);
    assert_eq!(g.update(&mut rs, Vec3::new(0.0, 0.0, 60.0)).passed, None, "the second gate first");
    assert_eq!(g.update(&mut rs, Vec3::new(0.0, 0.0, 22.0)).passed, Some(0));
    let step = g.update(&mut rs, Vec3::new(4.0, 0.0, 60.0));
    assert_eq!((step.passed, step.opened), (Some(1), true));
    assert!(touches_end(&mut rs, &world), "open after the last gate");
    assert!(world.triggers.volumes[0].groups != 0 && world.triggers.volumes[0].interacts != 0);
    rs.event.loops += 1;
    assert_eq!(g.update(&mut rs, Vec3::new(0.0, 0.0, 60.0)).passed, None, "the loop's first gate first");
    assert!(!touches_end(&mut rs, &world) && g.next == 0, "closed again by the loop");
    assert_eq!(g.update(&mut rs, Vec3::new(0.0, 0.0, 22.0)).passed, Some(0));
    rs.reset();
    g.reset(&mut rs);
    assert!(!touches_end(&mut rs, &world) && g.next == 0);
    run(&mut rs);
    rs.event.clock.seconds = 30.0;
    assert!(g.update(&mut rs, Vec3::ZERO).time_up && rs.event.time_up && !rs.event_ended);
    assert!(!g.update(&mut rs, Vec3::ZERO).time_up, "once");
    assert_eq!(g.update(&mut rs, Vec3::new(0.0, 0.0, 22.0)).passed, None, "not after the limit");
    let mut timer = crate::death::DeathTimer::default();
    let out = rs.event.update(&mut timer, &crate::event::EventInputs::default());
    let r = rs.event.result.as_ref().expect("a result");
    assert!(out.results && r.failed && r.time_ms == 30_000 && !rs.event.time_up);
}

#[test]
fn map_entities_are_placed_as_scene_instances_and_hostile_ones_refused() {
    use entities::{MAX_MAP_BODIES, MAX_MAP_ENTITIES, place_entities};
    use ssx_content::map::{MapDefinition, Placement, Prop, PropLook};
    use ssx_rules::triggers::TriggerScope;

    use crate::triggers::{TriggerEntity, TriggerSet};

    let root = std::env::temp_dir().join(format!("ssx-map-entities-{}", std::process::id()));
    let scripts = root.join("data0/data/xenon/gameexplorer/scripts");
    std::fs::create_dir_all(&scripts).expect("a scratch folder");
    for (name, src) in [("ok", "local a = 1\n"), ("other", "local b = 2\n"), ("fails", "error(\"no\")\n")] {
        std::fs::write(scripts.join(format!("{name}.lua")), src).expect("a script");
    }
    std::fs::write(scripts.join("loops.lua"), "while true do end\n").expect("a script");
    let body = ssx_content::object::RigidBody {
        offset: [0.0; 3],
        axis: [0.0, 1.0, 0.0],
        half_height: 0.5,
        radius: 0.2,
        fat: 0.05,
        inv_mass: 1.0,
        inv_inertia: [1.0; 3],
        max_speed: 30.0,
        max_angular_speed: 10.0,
        linear_damping: 0.1,
        angular_damping: 0.1,
        material: [0.5, 0.4, 0.2],
    };
    let object = |id: &str, controller: Option<&str>, body: Option<ssx_content::object::RigidBody>| {
        let id: ssx_content::ContentId = id.parse().expect("an id");
        let o = ssx_assets::content::maps::MapObject {
            id: id.clone(),
            source: "pack".into(),
            layer: ssx_content::Layer::ClientMods,
            collision: None,
            pob: None,
            model: None,
            audio: None,
            key: None,
            controller: controller.map(|c| c.parse().expect("a script id")),
            rigid_body: body,
        };
        (id, o)
    };
    let prop = |i: usize, id: &str| Prop {
        node: format!("p{i}"),
        look: PropLook::Object(id.parse().expect("an id")),
        placement: Placement { scale: [1.0; 3], rotation: [0.0, 0.0, 0.0, 1.0], position: [i as f32, 0.0, 0.0] },
        ao: 1.0,
        gltf_node: i,
        material: None,
    };
    let objects: BTreeMap<_, _> = [
        object("pack:object/flare", Some("ssx:script/scripts/ok"), Some(body)),
        object("pack:object/lamp", Some("ssx:script/scripts/other"), None),
        object("pack:object/rock", None, Some(body)),
        object("pack:object/bad", Some("ssx:script/scripts/fails"), None),
        object("pack:object/runaway", Some("ssx:script/scripts/loops"), None),
        object("pack:object/gone", Some("ssx:script/scripts/absent"), None),
    ]
    .into_iter()
    .collect();
    let def = |ids: &[&str]| MapDefinition {
        props: ids.iter().enumerate().map(|(i, id)| prop(i, id)).collect(),
        ..MapDefinition::default()
    };
    let place = |set: &mut TriggerSet, ids: &[&str]| place_entities(set, &root, &def(ids), &objects, 0);

    let mut set = TriggerSet::default();
    let ids = ["pack:object/flare", "pack:object/rock", "pack:object/flare", "pack:object/lamp"];
    let stats = place(&mut set, &ids).expect("placed");
    assert_eq!((stats.entities, stats.scripted, stats.bodies, stats.volumes), (3, 3, 2, 0));
    let placed: Vec<(u32, TriggerScope, usize)> =
        set.entities.iter().map(|e| (e.instance, e.scope, e.placement)).collect();
    assert_eq!(placed, vec![(0, TriggerScope::Map, 0), (2, TriggerScope::Map, 0), (3, TriggerScope::Map, 0)]);
    assert_eq!(set.entities[1].position, Vec3::new(2.0, 0.0, 0.0));
    let flare = set.entities[0].object;
    assert_eq!(flare, ssx_formats::attrib::fnv1(b"pack:object/flare"));
    assert_eq!((set.scripts.len(), set.bodies.len()), (2, 1));
    assert_eq!(set.scripts[&flare].0, "scripts/ok.lua");
    let c = crate::controllers::Controllers::offline(&set);
    assert!(c.entities.is_empty());

    let refused = |set: &mut TriggerSet, ids: &[&str], want: &str| {
        let e = format!("{:#}", place(set, ids).expect_err(want));
        assert!(e.contains(want), "{want}: {e}");
    };
    let mut fresh = TriggerSet::default();
    refused(
        &mut fresh,
        &["pack:object/bad"],
        "node \"p0\": pack:object/bad's controller scripts/fails.lua does not run",
    );
    refused(&mut fresh, &["pack:object/runaway"], "instruction budget exhausted");
    refused(&mut fresh, &["pack:object/gone"], "its controller ssx:script/scripts/absent is not on the disc");
    refused(&mut fresh, &["pack:object/x"], "node \"p0\": pack:object/x is not resolved");
    let drop = |object: u32, instance: u32| TriggerEntity {
        object,
        instance,
        placement: 0,
        scope: TriggerScope::Drop(7),
        position: Vec3::ZERO,
        rotation: glam::Quat::IDENTITY,
    };
    let mut with_drop = TriggerSet { entities: vec![drop(flare, 99)], ..TriggerSet::default() };
    refused(&mut with_drop, &["pack:object/flare"], "is a disc object's of the drop");
    let mut with_drop = TriggerSet { entities: vec![drop(1, 0)], ..TriggerSet::default() };
    refused(&mut with_drop, &["pack:object/flare"], "the prop's instance 0 is a scene instance's key of the drop");
    let mut with_script = TriggerSet::default();
    with_script.scripts.insert(flare, ("scripts/other.lua".into(), Arc::from("local b = 2\n")));
    refused(&mut with_script, &["pack:object/flare"], "controller differs from the drop's own scripts/other.lua");
    let many = vec!["pack:object/lamp"; MAX_MAP_ENTITIES + 1];
    refused(&mut TriggerSet::default(), &many, &format!("more than {MAX_MAP_ENTITIES} entities"));
    let bodies = vec!["pack:object/flare"; MAX_MAP_BODIES + 1];
    refused(&mut TriggerSet::default(), &bodies, &format!("more than {MAX_MAP_BODIES} objects with a rigid body"));
    let at_cap = vec!["pack:object/flare"; MAX_MAP_BODIES];
    let stats = place(&mut TriggerSet::default(), &at_cap).expect("the cap itself is placed");
    assert_eq!(stats.bodies, MAX_MAP_BODIES);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_free_ride_lists_the_title_updates_outro_only_with_the_helicopter() {
    use crate::event::EventType;
    use ssx_content::map::Intro;
    let has = |t, intro, tu| heli::event_scripts(t, intro, tu).iter().any(|s| s == heli::OUTRO_STANDARD);
    assert!(has(EventType::FreeRide, Intro::Heli, true));
    assert!(!has(EventType::FreeRide, Intro::Heli, false));
    assert!(!has(EventType::FreeRide, Intro::Spawn, true));
    assert!(!has(EventType::RaceClock, Intro::Heli, true));
    assert!(!has(EventType::SurviveToDeath, Intro::Heli, true));
    assert_eq!(
        heli::event_scripts(EventType::FreeRide, Intro::Spawn, true),
        vec![crate::objectives::EVENT_TYPE_SCRIPT]
    );
}
