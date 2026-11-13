use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context, Result};
use glam::{Quat, Vec3};
use ssx_assets::vault_db::{single_text, with_track_db};
use ssx_formats::attrib::{FieldData, Value};
use ssx_formats::spdt::{SpdtCells, SpdtFile};
use ssx_rules::locators::{instance_placements, instance_refs, object_volumes, place_volumes};

use crate::character::{GearStats, Tuning};
use crate::collision::CollisionWorld;
use crate::rider::World;
use crate::triggers::{TriggerScope, TriggerSet};
use crate::{props, terrain};

pub use ssx_rules::locators::{
    CLASS_DROP, CLASS_GAMEOBJECT, DropStart, END_LOCATOR_OBJECT, FIELD_POB, START_LOCATOR_OBJECT, decode_placements,
    drop_scene_instances, load_drop_starts, load_end_locator, track_drop_keys, track_scene_instances,
    trigger_volume_record,
};

pub fn load_trigger_volumes(big_root: &Path, track: &str, drop_key: Option<u32>) -> Result<TriggerSet> {
    with_track_db(big_root, track, |db| {
        let mut set = TriggerSet::default();
        for si in track_scene_instances(db, track)? {
            add_instance_volumes(db, &mut set, si, TriggerScope::Track)?;
        }
        if let Some(key) = drop_key {
            for si in drop_scene_instances(db, key)? {
                add_instance_volumes(db, &mut set, si, TriggerScope::Drop(key))?;
            }
        }
        let mut objects: Vec<u32> = set.entities.iter().map(|e| e.object).collect();
        objects.sort_unstable();
        objects.dedup();
        let packages = ssx_assets::content::GameContent::of(big_root).track_packages(track);
        let overrides: HashMap<u32, ssx_assets::content::resolve::Found<ssx_content::object::ObjectDef>> =
            objects.iter().filter_map(|&o| packages.object(o).map(|f| (o, f.clone()))).collect();
        let disc: Vec<u32> = objects.iter().copied().filter(|o| !overrides.contains_key(o)).collect();
        let mut scripts: Vec<(u32, String)> =
            ssx_assets::track_particles::object_controllers(db, &disc)?.into_iter().collect();
        for (&object, found) in &overrides {
            if let Some(id) = &found.value.controller {
                scripts.push((object, format!("{}.lua", id.path())));
            }
        }
        for (object, script) in scripts {
            let Some(path) = ssx_assets::track_objects::asset_file(big_root, &script) else { continue };
            if let Ok(src) = std::fs::read_to_string(path) {
                set.scripts.insert(object, (script, std::sync::Arc::from(src)));
            }
        }
        for &object in &objects {
            if let Some(found) = overrides.get(&object) {
                if let Some(body) = &found.value.rigid_body {
                    set.bodies.insert(object, crate::rigid::RigidBodyDef::from(body));
                }
                continue;
            }
            let Some(pob) = db.field(CLASS_GAMEOBJECT, object, FIELD_POB)?.and_then(|f| single_text(&f.data)) else {
                continue;
            };
            let Some(path) = ssx_assets::track_objects::asset_file(big_root, &pob) else { continue };
            let Ok(data) = std::fs::read(&path) else { continue };
            let pob = ssx_formats::pob::PobFile::parse(&data).with_context(|| path.display().to_string())?;
            if let Some(body) = ssx_assets::object_collision::pob_rigid_body(&pob) {
                set.bodies.insert(object, crate::rigid::RigidBodyDef::from(&body));
            }
        }
        Ok(set)
    })
}

pub fn add_instance_volumes(
    db: &ssx_assets::vault_db::Db,
    set: &mut TriggerSet,
    (class, key): (u32, u32),
    scope: TriggerScope,
) -> Result<()> {
    let Some((obj, data)) = instance_refs(db, (class, key))? else { return Ok(()) };
    let volumes = object_volumes(db, obj)?;
    let controlled =
        || -> Result<bool> { Ok(!ssx_assets::track_particles::object_controllers(db, &[obj.collection])?.is_empty()) };
    if volumes.is_empty() && !controlled()? {
        return Ok(());
    }
    let placements = instance_placements(db, data)?;
    for (i, pl) in placements.iter().enumerate() {
        set.entities.push(crate::triggers::TriggerEntity {
            object: obj.collection,
            instance: key,
            placement: i,
            scope,
            position: Vec3::from_array(pl.position),
            rotation: Quat::from_array(pl.rotation).normalize(),
        });
    }
    set.volumes.extend(place_volumes(db, &volumes, &placements, (obj.collection, key), scope)?);
    Ok(())
}

pub fn track_world(big_root: &Path, track_dir: &Path, track: &str, gear_mix: f32, with_props: bool) -> Result<World> {
    build_world(big_root, track_dir, track, gear_mix, with_props, None, &mut |_| Ok(()))
}

pub fn track_world_for_drop(
    big_root: &Path,
    track_dir: &Path,
    track: &str,
    gear_mix: f32,
    with_props: bool,
    drop: usize,
) -> Result<World> {
    build_world(big_root, track_dir, track, gear_mix, with_props, Some(drop), &mut |_| Ok(()))
}

pub fn track_world_for_drop_with(
    big_root: &Path,
    track_dir: &Path,
    track: &str,
    gear_mix: f32,
    drop: usize,
    add: &mut dyn FnMut(&mut CollisionWorld) -> Result<()>,
) -> Result<World> {
    build_world(big_root, track_dir, track, gear_mix, true, Some(drop), add)
}

fn build_world(
    big_root: &Path,
    track_dir: &Path,
    track: &str,
    gear_mix: f32,
    with_props: bool,
    drop: Option<usize>,
    add: &mut dyn FnMut(&mut CollisionWorld) -> Result<()>,
) -> Result<World> {
    let starts = load_drop_starts(big_root, track).unwrap_or_default();
    let files = match drop {
        Some(d) => Some(
            starts.get(d).with_context(|| format!("{track} has no drop {d} with a start"))?.terrain_files.as_slice(),
        ),
        None => None,
    };
    let mut collision = CollisionWorld::default();
    terrain::add_track_terrain_files(&mut collision, track_dir, files)?;
    if with_props {
        let drop_key = drop.and_then(|d| starts.get(d)).map(|s| s.drop_key);
        props::add_track_props(&mut collision, big_root, track, drop_key)?;
    }
    add(&mut collision)?;
    collision.finish();
    let bundle = crate::tuning::load(big_root)?;
    let tuning = Tuning::load(&bundle)?;
    let props = tuning.resolve(gear_mix)?;
    let drop_key = starts.get(drop.unwrap_or(0)).map(|d| d.drop_key);
    let triggers = load_trigger_volumes(big_root, track, drop_key)
        .with_context(|| format!("{track}: trigger volumes of drop {:?}", drop_key))?;
    let (events, end_locator) = match drop_key {
        Some(k) => load_drop_events(big_root, track, k)?,
        None => (Vec::new(), None),
    };
    let survival = match drop_key {
        Some(k) => load_drop_survival(big_root, track, k).with_context(|| format!("{track}: survival of {k:08x}"))?,
        None => Default::default(),
    };
    Ok(World { collision, tuning, props, starts, triggers, events, end_locator, survival })
}

pub fn load_drop_events(
    big_root: &Path,
    track: &str,
    drop_key: u32,
) -> Result<(Vec<crate::event::EventDef>, Option<Vec3>)> {
    let mut events: Vec<crate::event::EventDef> =
        crate::event::load_events(big_root)?.into_iter().filter(|e| e.drop_key == drop_key).collect();
    let drop = crate::objectives::DropScripts::load(big_root, drop_objective_scripts(big_root, track, drop_key)?);
    for e in &mut events {
        e.objectives = Some(std::sync::Arc::new(crate::objectives::EventObjectives::new(big_root, &drop, e)));
    }
    Ok((events, load_end_locator(big_root, track, drop_key)?))
}

pub fn track_world_with_gear(
    big_root: &Path,
    track_dir: &Path,
    track: &str,
    gear: &GearStats,
    with_props: bool,
) -> Result<World> {
    let mut w = track_world(big_root, track_dir, track, crate::character::DEFAULT_GEAR_MIX, with_props)?;
    w.props = w.tuning.resolve_gear(gear)?;
    Ok(w)
}

pub struct Boundary(SpdtFile);

impl Boundary {
    pub fn load(track_dir: &Path, track: &str) -> Option<Self> {
        let name = format!("{track}_track_boundary.spdt");
        let data = match ssx_assets::data_roots::overlay_track_file(track, &name) {
            Some(path) => std::fs::read(path).ok()?,
            None => std::fs::read(track_dir.join(&name)).ok()?,
        };
        let file = SpdtFile::parse(&data).ok()?;
        matches!(file.cells, SpdtCells::Boundary(_)).then_some(Self(file))
    }

    pub fn cell(&self, p: Vec3) -> Option<u16> {
        let f = &self.0;
        let [cx, cz] = f.cell_size();
        let max_x = f.bbox_min[0] + cx * f32::from(f.width) - 0.01;
        let max_z = f.bbox_min[2] + cz * f32::from(f.height) - 0.01;
        let (x, z) = (p.x.clamp(f.bbox_min[0], max_x), p.z.clamp(f.bbox_min[2], max_z));
        let ix = (((x - f.bbox_min[0]) / cx).floor() as usize).min(usize::from(f.width) - 1);
        let iz = (((z - f.bbox_min[2]) / cz).floor() as usize).min(usize::from(f.height) - 1);
        let SpdtCells::Boundary(cells) = &f.cells else { return None };
        cells.get(iz * f.width as usize + ix).copied()
    }

    pub fn in_bounds(&self, p: Vec3) -> bool {
        self.cell(p).is_some_and(|c| c & 0xC000 == 0)
    }

    pub fn push_direction(&self, p: Vec3) -> Option<Vec3> {
        let c = self.cell(p)?;
        let a = f32::from((c >> 8) & 0x3F) * std::f32::consts::TAU / 63.0;
        Some(Vec3::new(a.sin(), 0.0, a.cos()))
    }
}

pub fn drop_start(world: &World, drop: usize) -> Option<(Vec3, Vec3)> {
    let (at, q) = world.starts.get(drop)?.slot(0)?;
    let heading = Vec3::new((q * Vec3::Z).x, 0.0, (q * Vec3::Z).z).try_normalize()?;
    Some((at, heading))
}

pub fn spawn_pose(
    world: &World,
    _bounds: Option<&Boundary>,
    position: Option<Vec3>,
    heading: Option<Vec3>,
) -> Result<(Vec3, Vec3)> {
    spawn_pose_for_drop(world, 0, position, heading)
}

pub fn spawn_pose_for_drop(
    world: &World,
    drop: usize,
    position: Option<Vec3>,
    heading: Option<Vec3>,
) -> Result<(Vec3, Vec3)> {
    let start = drop_start(world, drop);
    let at = match (position, start) {
        (Some(p), _) => p,
        (None, Some((p, _))) => p,
        (None, None) => highest_terrain_vertex(&world.collision).context("no terrain")?,
    };
    let fall_line = || {
        world
            .collision
            .ray_cast(at + Vec3::Y * 1.0, -Vec3::Y, 101.0)
            .and_then(|h| Vec3::new(h.normal.x, 0.0, h.normal.z).try_normalize())
            .unwrap_or(Vec3::Z)
    };
    let heading = heading.or(start.filter(|_| position.is_none()).map(|s| s.1)).unwrap_or_else(fall_line);
    Ok((at, heading))
}

pub fn level_start_pose(
    world: &World,
    drop: usize,
    position: Option<Vec3>,
    heading: Option<Vec3>,
) -> Result<(Vec3, Quat)> {
    let (at, h) = spawn_pose_for_drop(world, drop, position, heading)?;
    Ok((at, level_start_orientation(world, at, h)))
}

pub fn level_start_orientation(world: &World, at: Vec3, heading: Vec3) -> Quat {
    let flat = Vec3::new(heading.x, 0.0, heading.z).try_normalize().unwrap_or(Vec3::Z);
    for d in &world.starts {
        if let Some((p, q)) = d.slot(0)
            && p.distance(at) < 1e-3
            && Vec3::new((q * Vec3::Z).x, 0.0, (q * Vec3::Z).z).normalize_or_zero().dot(flat) > 0.9999
        {
            return q;
        }
    }
    Quat::from_rotation_y(flat.x.atan2(flat.z))
}

pub fn highest_terrain_vertex(world: &CollisionWorld) -> Option<Vec3> {
    world
        .clusters
        .iter()
        .filter(|c| c.is_terrain())
        .flat_map(|c| &world.vertices[c.vert_start as usize..c.vert_start as usize + c.vert_count as usize])
        .copied()
        .max_by(|a, b| a.y.total_cmp(&b.y))
}

const FIELD_DROP_OBJECTIVE_SCRIPTS: u32 = 0xB3E0_D079;
const FIELD_DROP_SCENE_SCRIPTS: u32 = 0x085A_9D8A;
pub const SCRIPTS_DIR: &str = "memresident/data/common/scripts";

pub fn drop_objective_scripts(big_root: &Path, track: &str, drop_key: u32) -> Result<Vec<String>> {
    drop_script_list(big_root, track, drop_key, FIELD_DROP_OBJECTIVE_SCRIPTS)
}

pub fn drop_scene_scripts(big_root: &Path, track: &str, drop_key: u32) -> Result<Vec<String>> {
    drop_script_list(big_root, track, drop_key, FIELD_DROP_SCENE_SCRIPTS)
}

fn drop_script_list(big_root: &Path, track: &str, drop_key: u32, field_hash: u32) -> Result<Vec<String>> {
    with_track_db(big_root, track, |db| {
        let field = db.field(CLASS_DROP, drop_key, field_hash)?;
        let values = match field.map(|f| &f.data) {
            Some(FieldData::Array(vs)) => vs.iter().collect::<Vec<_>>(),
            Some(FieldData::Single(v)) => vec![v],
            None => Vec::new(),
        };
        Ok(values
            .into_iter()
            .filter_map(|v| if let Value::Text(Some(t)) = v { Some(t.clone()) } else { None })
            .collect())
    })
}

pub fn load_drop_survival(big_root: &Path, track: &str, drop_key: u32) -> Result<crate::survival::DropSurvival> {
    use crate::objectives::DropScripts;
    let mut scripts = DropScripts::load(big_root, drop_objective_scripts(big_root, track, drop_key)?);
    scripts.add_scene(big_root, drop_scene_scripts(big_root, track, drop_key)?);
    let mut d = survival_of_scripts(scripts);
    let drops = ssx_assets::track_lighting::load_track_drops(big_root, track)?;
    let preset = drops.iter().find(|x| x.key == drop_key).map(|x| &x.lighting);
    apply_lighting(&mut d, preset);
    Ok(d)
}

pub fn survival_of_scripts(scripts: crate::objectives::DropScripts) -> crate::survival::DropSurvival {
    use crate::objectives::{GsValue, Probe};
    let p = Probe::run(&scripts.level_batch(None), None, crate::character::GearStats::default_gear().slots);
    let objective_tuning_set = match p.value("Player1", "INT_AvalancheTuningSet") {
        Some(GsValue::Int(v)) => Some(v),
        _ => None,
    };
    crate::survival::DropSurvival {
        cold: p.flag("ColdDrop"),
        low_oxygen: p.flag("LowOxygenDrop"),
        rock_damage: p.set_true("Player1", "BLN_EnableRockDamage"),
        tripping: p.set_true("Player1", "BLN_EnableTripping"),
        avalanche: crate::avalanche::DropAvalanche {
            objective_tuning_set,
            avalanche_drop: p.flag("AvalancheDrop"),
            reverse_cam: p.value("TheCamera", "INT_CameraType") == Some(GsValue::Int(2)),
        },
        objectives: Some(std::sync::Arc::new(scripts)),
        ..Default::default()
    }
}

pub fn apply_lighting(
    d: &mut crate::survival::DropSurvival,
    preset: Option<&ssx_assets::track_lighting::LightingPreset>,
) {
    d.sun = preset.and_then(|p| {
        let v = p.vector("dirLight_Direction")?;
        let sun = Vec3::new(v[0], v[1], v[2]).normalize_or_zero();
        (sun + Vec3::Y * p.float("dirLight_Offset").unwrap_or(0.0)).try_normalize()
    });
    // TODO(guess): the drop's preset is the lighting service's current one when the objectives test it and `HeadLampGearTrack` reads `headLampScaleFactor` (an override or blend could change it); settle: the service's +0x20.
    crate::guess_hit!();
    d.night = preset.and_then(|p| p.float("isNightCondition")).is_some_and(|v| v > 0.0);
    d.head_lamp_scale = preset.and_then(|p| p.float("headLampScaleFactor")).unwrap_or(0.0);
}

#[cfg(test)]
mod tests {
    use super::*;
    use ssx_assets::track_objects::Placement;
    use ssx_formats::spdt::SpdtKind;

    fn grid(cells: Vec<u16>) -> Boundary {
        Boundary(SpdtFile {
            kind: SpdtKind::Boundary,
            type_name: "snowboun".into(),
            version: 1,
            bbox_min: [-15.0, 0.0, 0.0],
            bbox_max: [15.0, 100.0, 20.0],
            width: 3,
            height: 2,
            cells: SpdtCells::Boundary(cells),
        })
    }

    #[test]
    fn boundary_bits_clamping_and_push_direction() {
        let b = grid(vec![0x0012, 0x4000, 0x8000, 0x3F00 & (21 << 8), 0x00FF, 0xC000]);
        assert!(b.in_bounds(Vec3::new(-10.0, 0.0, 5.0)));
        assert!(!b.in_bounds(Vec3::new(0.0, 0.0, 5.0)) && !b.in_bounds(Vec3::new(10.0, 0.0, 5.0)));
        assert!(b.in_bounds(Vec3::new(-10.0, 0.0, 15.0)) && b.in_bounds(Vec3::new(0.0, 0.0, 15.0)));
        assert!(!b.in_bounds(Vec3::new(10.0, 0.0, 15.0)));
        assert_eq!(b.cell(Vec3::new(-500.0, 0.0, -500.0)), Some(0x0012));
        assert_eq!(b.cell(Vec3::new(500.0, 0.0, 500.0)), Some(0xC000));
        let d = b.push_direction(Vec3::new(-10.0, 0.0, 15.0)).expect("cell");
        let a = 21.0 * std::f32::consts::TAU / 63.0;
        assert!(d.distance(Vec3::new(a.sin(), 0.0, a.cos())) < 1e-6);
        assert!(b.push_direction(Vec3::new(-10.0, 0.0, 5.0)).is_some_and(|d| d.distance(Vec3::Z) < 1e-6));
    }

    #[test]
    fn the_start_locator_slot_gives_position_and_heading() {
        let mut w = crate::testutil::world(crate::testutil::plane(100.0, 0.2));
        let q = Quat::from_rotation_y(std::f32::consts::FRAC_PI_2);
        let slot = Placement { scale: [1.0; 3], rotation: q.to_array(), position: [3.0, 1.0, 4.0], unk: [0.0; 6] };
        w.starts = vec![DropStart { drop_key: 1, slots: vec![slot], terrain_files: Vec::new() }];
        let (at, heading) = spawn_pose_for_drop(&w, 0, None, None).expect("start");
        assert!(at.distance(Vec3::new(3.0, 1.0, 4.0)) < 1e-6 && heading.distance(Vec3::X) < 1e-5);
        let (_, h) = spawn_pose_for_drop(&w, 0, Some(Vec3::new(0.0, 5.0, 0.0)), None).expect("pose");
        assert!(h.distance(Vec3::Z) < 1e-5, "{h:?}");
        w.starts.clear();
        assert!(spawn_pose(&w, None, None, None).is_err());
    }
}
