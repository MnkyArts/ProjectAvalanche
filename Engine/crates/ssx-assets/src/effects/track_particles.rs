use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context, Result, bail};
use ssx_formats::attrib::{FieldData, Value};

use crate::particles::{CLASS_PARTICLE_NODE, ParticleNode, particle_node};
use crate::track_objects::Placement;
use crate::vault_db::{Db, fnv1, refs, single, with_track_db};

const CLASS_TRACK: u32 = fnv1("ge_track");
const CLASS_DROP: u32 = fnv1("ge_drop");
const FIELD_TRACK: u32 = fnv1("Track");
const FIELD_TRACK_TERRAIN: u32 = 0x3132_67E8;
const FIELD_TERRAIN_INSTANCES: u32 = 0x0903_EF8F;
const FIELD_SCENE_INSTANCES: u32 = 0x5050_DE8B;
const FIELD_INSTANCE_DATA: u32 = 0xEE66_32AC;
const FIELD_GAMEOBJECT: u32 = 0x3ED7_B5CE;
const FIELD_MODELS: u32 = 0xB43A_3598;
const FIELD_CONTROLLER: u32 = fnv1("Controller");
pub const FIELD_PARTICLE_NODES: u32 = fnv1("ParticleNodes");

#[derive(Clone, Debug, Default)]
pub struct ObjectParticles {
    pub key: u32,
    pub object: String,
    pub lods: Vec<Vec<ParticleNode>>,
    pub controller_script: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum InstanceScope {
    Track,
    Drop(u32),
    Map,
}

#[derive(Clone, Debug)]
pub struct ParticleInstance {
    pub object: usize,
    pub key: u32,
    pub scope: InstanceScope,
    pub placements: Vec<Placement>,
}

#[derive(Clone, Debug, Default)]
pub struct TrackParticles {
    pub objects: Vec<ObjectParticles>,
    pub instances: Vec<ParticleInstance>,
}

impl TrackParticles {
    pub fn placement_count(&self) -> usize {
        self.instances.iter().map(|i| i.placements.len()).sum()
    }
}

pub fn load_track_particles(big_root: &Path, track: &str, drop: Option<u32>) -> Result<TrackParticles> {
    with_track_db(big_root, track, |db| track_particles(db, track, drop))
}

pub fn drop_keys(db: &Db, track: &str) -> Result<Vec<u32>> {
    let track_key = fnv1(&format!("{track}_track"));
    let mut out = Vec::new();
    for (key, _) in db.collections_of_all(CLASS_DROP) {
        if matches!(db.field(CLASS_DROP, key, FIELD_TRACK)?.and_then(|f| single(&f.data)),
            Some(Value::Ref(r)) if r.collection == track_key)
        {
            out.push(key);
        }
    }
    Ok(out)
}

pub fn track_particles(db: &Db, track: &str, drop: Option<u32>) -> Result<TrackParticles> {
    let mut scene = Vec::new();
    let track_key = fnv1(&format!("{track}_track"));
    if let Some(terrains) = db.field(CLASS_TRACK, track_key, FIELD_TRACK_TERRAIN)? {
        for t in refs(&terrains.data).into_iter().filter(|r| r.collection != 0) {
            let Some(cols) = db.field(t.class, t.collection, FIELD_TERRAIN_INSTANCES)? else { continue };
            for c in refs(&cols.data).into_iter().filter(|r| r.collection != 0) {
                if let Some(list) = db.field(c.class, c.collection, FIELD_SCENE_INSTANCES)? {
                    let it = refs(&list.data).into_iter().filter(|r| r.collection != 0);
                    scene.extend(it.map(|r| (r.class, r.collection, InstanceScope::Track)));
                }
            }
        }
    }
    if let Some(d) = drop
        && let Some(list) = db.field(CLASS_DROP, d, FIELD_SCENE_INSTANCES)?
    {
        let it = refs(&list.data).into_iter().filter(|r| r.collection != 0);
        scene.extend(it.map(|r| (r.class, r.collection, InstanceScope::Drop(d))));
    }
    let mut out = TrackParticles::default();
    let mut by_key: HashMap<u32, Option<usize>> = HashMap::new();
    for (class, key, scope) in scene {
        let fields = db.fields(class, key)?;
        let reference = |k: u32| fields.iter().find(|f| f.key == k).and_then(|f| refs(&f.data).first().copied());
        let (Some(data), Some(obj)) = (reference(FIELD_INSTANCE_DATA), reference(FIELD_GAMEOBJECT)) else {
            continue;
        };
        if data.collection == 0 || obj.collection == 0 {
            continue;
        }
        let idx = match by_key.get(&obj.collection) {
            Some(i) => *i,
            None => {
                let o = object_particles(db, obj.class, obj.collection)
                    .with_context(|| format!("game object {}", db.collection_name(obj.collection)))?;
                let i = o.lods.iter().any(|l| !l.is_empty()).then(|| {
                    out.objects.push(o);
                    out.objects.len() - 1
                });
                by_key.insert(obj.collection, i);
                i
            }
        };
        let Some(object) = idx else { continue };
        let placements = match db.field(data.class, data.collection, FIELD_INSTANCE_DATA)? {
            Some(f) => placements(&f.data),
            None => Vec::new(),
        };
        out.instances.push(ParticleInstance { object, key, scope, placements });
    }
    Ok(out)
}

pub fn map_particles(big_root: &Path, placed: &[(u32, u32, Placement)]) -> Result<TrackParticles> {
    crate::vault_db::with_attribdb(big_root, |db| {
        let mut out = TrackParticles::default();
        let mut by_key: HashMap<u32, Option<usize>> = HashMap::new();
        for &(instance, key, placement) in placed {
            let idx = match by_key.get(&key) {
                Some(i) => *i,
                None => {
                    let o = object_particles(db, crate::track_objects::CLASS_GAMEOBJECT, key)
                        .with_context(|| format!("game object {key:#010x}"))?;
                    let i = o.lods.iter().any(|l| !l.is_empty()).then(|| {
                        out.objects.push(o);
                        out.objects.len() - 1
                    });
                    by_key.insert(key, i);
                    i
                }
            };
            if let Some(object) = idx {
                let scope = InstanceScope::Map;
                out.instances.push(ParticleInstance { object, key: instance, scope, placements: vec![placement] });
            }
        }
        Ok(out)
    })
}

fn placements(d: &FieldData) -> Vec<Placement> {
    let vals: Vec<&Value> = match d {
        FieldData::Single(v) => vec![v],
        FieldData::Array(vs) => vs.iter().collect(),
    };
    vals.into_iter()
        .filter_map(|v| if let Value::Raw { bytes, .. } = v { Placement::decode(bytes) } else { None })
        .collect()
}

fn object_particles(db: &Db, class: u32, key: u32) -> Result<ObjectParticles> {
    let mut o = ObjectParticles { key, object: db.collection_name(key), lods: Vec::new(), controller_script: None };
    if let Some(f) = db.field(class, key, FIELD_CONTROLLER)?
        && let Some(r) = refs(&f.data).into_iter().find(|r| r.collection != 0)
    {
        o.controller_script = db.text_of(r.class, r.collection)?;
    }
    let Some(models) = db.field(class, key, FIELD_MODELS)? else { return Ok(o) };
    for m in refs(&models.data).into_iter().filter(|r| r.collection != 0) {
        let mut nodes = Vec::new();
        if let Some(f) = db.field(m.class, m.collection, FIELD_PARTICLE_NODES)? {
            for r in refs(&f.data).into_iter().filter(|r| r.collection != 0) {
                if r.class != CLASS_PARTICLE_NODE {
                    bail!("ParticleNodes entry of class {}", db.class_name(r.class));
                }
                nodes.push(particle_node(db, r.collection)?);
            }
        }
        o.lods.push(nodes);
    }
    Ok(o)
}

pub fn game_object_nodes(db: &Db, key: u32) -> Result<(ObjectParticles, Vec<Vec<crate::track_lights::LightNode>>)> {
    let class = fnv1("ge_scene_gameobject");
    let particles = object_particles(db, class, key)?;
    let mut lights = Vec::new();
    if let Some(models) = db.field(class, key, FIELD_MODELS)? {
        for m in refs(&models.data).into_iter().filter(|r| r.collection != 0) {
            let mut nodes = Vec::new();
            if let Some(f) = db.field(m.class, m.collection, fnv1("LightNodes"))? {
                for r in refs(&f.data).into_iter().filter(|r| r.collection != 0) {
                    nodes.push(crate::track_lights::light_node(db, r.collection)?);
                }
            }
            lights.push(nodes);
        }
    }
    Ok((particles, lights))
}

pub fn object_controllers(db: &Db, keys: &[u32]) -> Result<HashMap<u32, String>> {
    let class = fnv1("ge_scene_gameobject");
    let mut out = HashMap::new();
    for &k in keys {
        if db.find(class, k).is_none() {
            continue;
        }
        if let Some(f) = db.field(class, k, FIELD_CONTROLLER)?
            && let Some(r) = refs(&f.data).into_iter().find(|r| r.collection != 0)
            && let Some(s) = db.text_of(r.class, r.collection)?
        {
            out.insert(k, s);
        }
    }
    Ok(out)
}
