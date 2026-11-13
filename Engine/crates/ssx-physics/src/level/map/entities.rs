use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context, Result, bail};
use glam::{Quat, Vec3};
use ssx_assets::content::maps::MapObject;
use ssx_content::ContentId;
use ssx_content::map::{MapDefinition, Prop, PropLook};
use ssx_formats::attrib::RefSpec;

use crate::triggers::{TriggerEntity, TriggerScope, TriggerSet};

pub use ssx_content::map::{MAX_BODIES as MAX_MAP_BODIES, MAX_ENTITIES as MAX_MAP_ENTITIES};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MapEntityStats {
    pub entities: usize,
    pub scripted: usize,
    pub bodies: usize,
    pub volumes: usize,
}

pub fn prop_placement(p: &Prop) -> ssx_rules::placement::Placement {
    let t = &p.placement;
    ssx_rules::placement::Placement {
        scale: t.scale,
        rotation: t.rotation,
        position: t.position,
        unk: [1.0, 1.0, p.ao, 0.0, 0.0, 0.0],
    }
}

fn object_key(o: &MapObject) -> u32 {
    o.key.unwrap_or_else(|| ssx_formats::attrib::fnv1(o.id.to_string().as_bytes()))
}

fn controller_source(big_root: &Path, o: &MapObject) -> Result<Option<(String, std::sync::Arc<str>)>> {
    let Some(id) = &o.controller else { return Ok(None) };
    let script = format!("{}.lua", id.path());
    let path = ssx_assets::track_objects::asset_file(big_root, &script)
        .with_context(|| format!("{}: its controller {id} is not on the disc", o.id))?;
    let src = std::fs::read_to_string(&path).with_context(|| format!("{}: {}", o.id, path.display()))?;
    Ok(Some((script, std::sync::Arc::from(src))))
}

pub fn place_entities(
    set: &mut TriggerSet,
    big_root: &Path,
    def: &MapDefinition,
    objects: &BTreeMap<ContentId, MapObject>,
    map_volumes: usize,
) -> Result<MapEntityStats> {
    let disc = def.props.iter().any(|p| match &p.look {
        PropLook::Object(id) => objects.get(id).is_some_and(|o| o.key.is_some()),
        PropLook::Mesh { .. } => false,
    });
    if disc {
        ssx_assets::vault_db::with_attribdb(big_root, |db| {
            place_with(set, Some(db), big_root, def, objects, map_volumes)
        })
    } else {
        place_with(set, None, big_root, def, objects, map_volumes)
    }
}

fn place_with(
    set: &mut TriggerSet,
    db: Option<&ssx_assets::vault_db::Db>,
    big_root: &Path,
    def: &MapDefinition,
    objects: &BTreeMap<ContentId, MapObject>,
    map_volumes: usize,
) -> Result<MapEntityStats> {
    let mut stats = MapEntityStats::default();
    let mut records: BTreeMap<u32, Vec<RefSpec>> = BTreeMap::new();
    let mut keys: BTreeMap<u32, &ContentId> = BTreeMap::new();
    let mut sources: BTreeMap<&ContentId, Option<(String, std::sync::Arc<str>)>> = BTreeMap::new();
    let props = def.props.iter().enumerate().filter_map(|(i, p)| match &p.look {
        PropLook::Object(id) => Some((i, p, id)),
        PropLook::Mesh { .. } => None,
    });
    for (index, prop, id) in props {
        let o = objects.get(id).with_context(|| format!("node \"{}\": {id} is not resolved", prop.node))?;
        let key = object_key(o);
        if let Some(other) = keys.insert(key, id).filter(|other| *other != id) {
            bail!("node \"{}\": {id} and {other} take the same object key {key:#010x}", prop.node);
        }
        if o.key.is_none() && set.entities.iter().any(|e| e.object == key && e.scope != TriggerScope::Map) {
            bail!("node \"{}\": {id}'s object key {key:#010x} is a disc object's of the drop", prop.node);
        }
        let instance = u32::try_from(index).context("the map's props")?;
        if set.entities.iter().any(|e| e.instance == instance && e.scope != TriggerScope::Map) {
            bail!("node \"{}\": the prop's instance {instance} is a scene instance's key of the drop", prop.node);
        }
        let placement = prop_placement(prop);
        let volumes = match (o.key, db) {
            (Some(k), Some(db)) => {
                let mut placed = || -> Result<Vec<crate::triggers::TriggerVolume>> {
                    let refs = match records.entry(k) {
                        std::collections::btree_map::Entry::Occupied(e) => e.into_mut(),
                        std::collections::btree_map::Entry::Vacant(e) => {
                            let obj = RefSpec { class: crate::spawn::CLASS_GAMEOBJECT, collection: k };
                            e.insert(ssx_rules::locators::object_volumes(db, obj)?)
                        }
                    };
                    ssx_rules::locators::place_volumes(db, refs, &[placement], (k, instance), TriggerScope::Map)
                };
                placed().with_context(|| format!("node \"{}\": {id}'s trigger volumes", prop.node))?
            }
            (Some(_), None) => bail!("node \"{}\": {id}: the attribute database was not read", prop.node),
            (None, _) => Vec::new(),
        };
        if !sources.contains_key(id) {
            let source = controller_source(big_root, o).with_context(|| format!("node \"{}\"", prop.node))?;
            if let Some((name, src)) = &source {
                crate::controllers::script_sets_within(name, src, crate::controllers::MAP_CONTROLLER_BUDGET).map_err(
                    |e| {
                        anyhow::anyhow!(
                            "node \"{}\": {id}'s controller {name} does not run as a controller: {e}",
                            prop.node
                        )
                    },
                )?;
            }
            sources.insert(id, source);
        }
        let script = sources[id].clone();
        if volumes.is_empty() && script.is_none() {
            continue;
        }
        stats.entities += 1;
        stats.volumes += volumes.len();
        if stats.entities > MAX_MAP_ENTITIES {
            bail!(
                "the map places more than {MAX_MAP_ENTITIES} entities (objects with trigger volumes or a controller)"
            );
        }
        if map_volumes + stats.volumes > ssx_content::map::MAX_VOLUMES {
            bail!(
                "the map's volumes and its objects' trigger volumes are more than {} (node \"{}\", {id})",
                ssx_content::map::MAX_VOLUMES,
                prop.node
            );
        }
        if let Some((name, src)) = script {
            stats.scripted += 1;
            match set.scripts.get(&key) {
                Some((n, s)) if (n, s) != (&name, &src) => {
                    bail!("node \"{}\": {id}'s controller differs from the drop's own {n}", prop.node)
                }
                _ => {
                    set.scripts.insert(key, (name, src));
                }
            }
            if let Some(body) = &o.rigid_body {
                stats.bodies += 1;
                if stats.bodies > MAX_MAP_BODIES {
                    bail!("the map places more than {MAX_MAP_BODIES} objects with a rigid body and a controller");
                }
                let def = crate::rigid::RigidBodyDef::from(body);
                if set.bodies.get(&key).is_some_and(|d| *d != def) {
                    bail!("node \"{}\": {id}'s rigid body differs from the drop's own placements'", prop.node);
                }
                set.bodies.insert(key, def);
            }
        }
        set.entities.push(TriggerEntity {
            object: key,
            instance,
            placement: 0,
            scope: TriggerScope::Map,
            position: Vec3::from_array(placement.position),
            rotation: Quat::from_array(placement.rotation).normalize(),
        });
        set.volumes.extend(volumes);
    }
    Ok(stats)
}
