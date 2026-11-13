use std::path::{Path, PathBuf};

use std::sync::Arc;

use anyhow::{Context, Result, bail};
use ssx_formats::attrib::{FieldData, Value};
use ssx_formats::csvdb::Geotag;
use ssx_lua::game::controllers::{CondDef, StateSets};

use crate::particles::{CLASS_PARTICLE_NODE, ParticleNode, particle_node};
use crate::vault_db::{Db, Material, fnv1, refs, single, single_text, with_track_db};

const SCRIPTS_DIR: &str = "memresident/data/common/scripts";
const CONTROLLERS_SCRIPT: &str = "world_entity_controllers_default.lua";
const GEOCACHE_DIR: &str = "cache1/data/common/geocaches";

pub const OFFLINE_SCHEMAS: [&str; 3] = ["OfflineGeoCacheTier1", "OfflineGeoCacheTier2", "OfflineGeoCacheTier3"];
pub const TIER2_LEVEL: u32 = 6;
pub const TIER3_LEVEL: u32 = 8;

pub fn offline_tier(level: u32) -> usize {
    if level >= TIER3_LEVEL {
        2
    } else if level >= TIER2_LEVEL {
        1
    } else {
        0
    }
}

const CLASS_GAMEOBJECT: u32 = fnv1("ge_scene_gameobject");
const CLASS_GEOMETRY: u32 = fnv1("ge_geometry");
const CLASS_MATERIAL: u32 = fnv1("ge_material");
const CLASS_TRIGGER_VOLUME: u32 = fnv1("ge_scene_triggervolume");
const CLASS_AUDIO_DATA: u32 = fnv1("ge_scene_audiodata");
const FIELD_MODELS: u32 = 0xB43A_3598;
const FIELD_LOD_THRESHOLDS: u32 = fnv1("LODThresholds");
const FIELD_TRIGGER_VOLUMES: u32 = fnv1("TriggerVolumes");
const FIELD_AUDIO_DATA: u32 = 0x7DCA_51D1;
const FIELD_AUDIO_NAME: u32 = 0x83F8_6C09;
const FIELD_GAMEOBJECT_UNK: u32 = 0x4FB2_359A;
const FIELD_MESHES: u32 = fnv1("Meshes");
const FIELD_PARTICLE_NODES: u32 = fnv1("ParticleNodes");
const FIELD_MATERIAL: u32 = fnv1("Material");
const FIELD_GEOMETRY: u32 = fnv1("Geometry");
const FIELD_SKELETON: u32 = fnv1("SkeletonFileRef");

#[derive(Clone, Debug, PartialEq)]
pub struct GeotagController {
    pub sets: Arc<StateSets>,
    pub set: usize,
    pub default_states: [Option<String>; 3],
    pub collect_volume: String,
}

pub fn geotag_controller(sets: Arc<StateSets>) -> Result<GeotagController> {
    let set = sets.set("GeoCache").context("no GeoCache state set")?;
    let volumes: Vec<&str> = sets
        .conditions
        .iter()
        .filter_map(|c| match c {
            CondDef::TriggerVolume { volume, entity, client } if entity == "Player1" && client == "Touch" => {
                Some(volume.as_str())
            }
            _ => None,
        })
        .filter(|v| v.contains("geotag"))
        .collect();
    let [volume] = volumes.as_slice() else { bail!("GeoCache: collect volumes {volumes:?}") };
    let collect_volume = volume.to_string();
    Ok(GeotagController { sets, set, default_states: Default::default(), collect_volume })
}

#[derive(Clone, Debug)]
pub struct GeotagObject {
    pub schema: String,
    pub game_object: String,
    pub lod_thresholds: Vec<f32>,
    pub geometry: String,
    pub skeleton: Option<String>,
    pub material: Material,
    pub burst: ParticleNode,
    pub trigger_volume: (u32, u32),
    pub audio_name: Option<String>,
    pub unk_4fb2359a: Option<i32>,
}

#[derive(Clone, Debug)]
pub struct GeotagAssets {
    pub objects: Vec<GeotagObject>,
    pub controller: GeotagController,
}

pub fn load(big_root: &Path, track: &str) -> Result<GeotagAssets> {
    let (schemas, _) = crate::avalanche_entities::batches(big_root, &[]).context("the schemas batch")?;
    let path = big_root.join(SCRIPTS_DIR).join(CONTROLLERS_SCRIPT);
    let src = std::fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
    let graph = ssx_lua::game::graph::run_batch(
        ssx_lua::game::Binder::StateSets,
        &[(CONTROLLERS_SCRIPT.to_string(), src)],
        None,
    );
    let mut controller = geotag_controller(Arc::new(StateSets::from_graph(&graph)))?;
    let mut tiers = Vec::new();
    for (i, s) in OFFLINE_SCHEMAS.iter().enumerate() {
        let schema = schemas.schemas.get(*s).with_context(|| format!("no schema {s}"))?;
        anyhow::ensure!(schema.controller.as_deref() == Some("GeoCache"), "{s}: controller {:?}", schema.controller);
        let object = schema
            .factories
            .iter()
            .filter(|f| f.kind == "GameObject")
            .flat_map(|f| &f.calls)
            .find(|(m, _)| m == "SetGameObjectName")
            .and_then(|(_, a)| a.first().and_then(|a| a.str()).map(str::to_string))
            .with_context(|| format!("{s}: no game object"))?;
        controller.default_states[i] = schema.default_state.clone();
        tiers.push(object);
    }
    let objects = with_track_db(big_root, track, |db| {
        OFFLINE_SCHEMAS
            .iter()
            .zip(&tiers)
            .map(|(s, object)| game_object(db, s, object).with_context(|| format!("{s} → {object}")))
            .collect::<Result<Vec<_>>>()
    })?;
    Ok(GeotagAssets { objects, controller })
}

fn first_ref(d: &FieldData) -> Option<ssx_formats::attrib::RefSpec> {
    refs(d).into_iter().find(|r| r.collection != 0)
}

fn game_object(db: &Db, schema: &str, name: &str) -> Result<GeotagObject> {
    let key = fnv1(name);
    let field = |k: u32| db.field(CLASS_GAMEOBJECT, key, k);
    let lod_thresholds = match field(FIELD_LOD_THRESHOLDS)?.map(|f| &f.data) {
        Some(FieldData::Array(vs)) => {
            vs.iter().filter_map(|v| if let Value::Float(x) = v { Some(*x) } else { None }).collect()
        }
        _ => Vec::new(),
    };
    let models = field(FIELD_MODELS)?.context("no models")?;
    let model = first_ref(&models.data).context("no LOD-0 model")?;
    let nodes = db.field(model.class, model.collection, FIELD_PARTICLE_NODES)?.context("no particle node")?;
    let node = first_ref(&nodes.data).filter(|r| r.class == CLASS_PARTICLE_NODE).context("no particle node")?;
    let burst = particle_node(db, node.collection)?;
    let meshes = db.field(model.class, model.collection, FIELD_MESHES)?.context("model without meshes")?;
    let mesh = first_ref(&meshes.data).context("no mesh")?;
    let fields = db.fields(mesh.class, mesh.collection)?;
    let get = |k: u32| fields.iter().find(|f| f.key == k);
    let Some(Value::Ref(g)) = get(FIELD_GEOMETRY).and_then(|f| single(&f.data)) else { bail!("mesh without geometry") };
    if g.class != CLASS_GEOMETRY {
        bail!("geometry of class {}", db.class_name(g.class));
    }
    let geometry = db.text_of(g.class, g.collection)?.context("geometry without a path")?;
    let material = match get(FIELD_MATERIAL).and_then(|f| single(&f.data)) {
        Some(Value::Ref(m)) if m.class == CLASS_MATERIAL && m.collection != 0 => db.material(m.class, m.collection)?,
        other => bail!("mesh material {other:?}"),
    };
    let skeleton = get(FIELD_SKELETON).and_then(|f| single_text(&f.data)).filter(|s| !s.is_empty());
    let volumes = field(FIELD_TRIGGER_VOLUMES)?.context("no trigger volume")?;
    let tv = first_ref(&volumes.data).filter(|r| r.class == CLASS_TRIGGER_VOLUME).context("no trigger volume")?;
    let audio_name = match field(FIELD_AUDIO_DATA)?.and_then(|f| first_ref(&f.data)) {
        Some(a) if a.class == CLASS_AUDIO_DATA => {
            db.field(a.class, a.collection, FIELD_AUDIO_NAME)?.and_then(|f| single_text(&f.data))
        }
        _ => None,
    };
    let unk_4fb2359a = match field(FIELD_GAMEOBJECT_UNK)?.and_then(|f| single(&f.data)) {
        Some(Value::Int32(x)) => Some(*x),
        _ => None,
    };
    Ok(GeotagObject {
        schema: schema.into(),
        game_object: name.into(),
        lod_thresholds,
        geometry,
        skeleton,
        material,
        burst,
        trigger_volume: (tv.class, tv.collection),
        audio_name,
        unk_4fb2359a,
    })
}

pub fn geotag_file(big_root: &Path, track: &str) -> PathBuf {
    let rel = format!("data/common/geocaches/{track}_track_geotagdb.csv");
    crate::data_roots::find(big_root, &rel)
        .unwrap_or_else(|| big_root.join(GEOCACHE_DIR).join(format!("{track}_track_geotagdb.csv")))
}

pub fn load_rows(big_root: &Path, track: &str) -> Result<Option<Vec<Geotag>>> {
    let p = geotag_file(big_root, track);
    if !p.is_file() {
        return Ok(None);
    }
    let text = std::fs::read_to_string(&p).with_context(|| format!("reading {}", p.display()))?;
    Ok(Some(ssx_formats::csvdb::parse_geotags(&text).with_context(|| p.display().to_string())?))
}

pub fn disc_total(big_root: &Path) -> Result<usize> {
    let dir = big_root.join(GEOCACHE_DIR);
    let manifest = std::fs::read_to_string(dir.join("manifest.txt")).context("reading geocaches/manifest.txt")?;
    let mut total = 0;
    for name in manifest.lines().map(str::trim).filter(|l| !l.is_empty()) {
        let text = std::fs::read_to_string(dir.join(name)).with_context(|| format!("reading {name}"))?;
        total += text.split('\n').filter(|l| !l.trim().is_empty()).count().saturating_sub(1);
    }
    Ok(total)
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DistanceScale {
    pub start: f32,
    pub end: f32,
    pub size: f32,
}

impl DistanceScale {
    pub fn at(&self, distance: f32) -> f32 {
        1.0 + ((distance - self.start) / (self.end - self.start)).clamp(0.0, 1.0) * (self.size - 1.0)
    }
}

pub const GEOTAG_EFFECT: &str = "cache1/data/xenon/shaders/object_geotag.fxo";

pub fn distance_scale(big_root: &Path) -> Result<DistanceScale> {
    let path = big_root.join(GEOTAG_EFFECT);
    let data = std::fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
    let fx = ssx_formats::fxo::Effect::parse(&data).with_context(|| path.display().to_string())?;
    let image = data.get(12..).context("effect without an image")?;
    let word = |o: usize| -> Result<u32> {
        let b = image.get(o..o + 4).context("effect image too short")?;
        Ok(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    };
    let (offset, size) = (word(0x128)? as usize, word(0x138)? as usize);
    let value = |name: &str| -> Result<f32> {
        let p = fx.parameters.iter().find(|p| p.name.as_deref() == Some(name)).with_context(|| format!("no {name}"))?;
        let at = usize::from(p.value_offset) * 16;
        if at + 4 > size {
            bail!("{name}: slot {} outside the {size}-byte value buffer", p.value_offset);
        }
        Ok(f32::from_bits(word(offset + at)?))
    };
    Ok(DistanceScale { start: value("scaleStart")?, end: value("scaleEnd")?, size: value("scaleSize")? })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SET: &str = r#"
local s = CreateStateSet( "GeoCache" )
local st = s:AddState( "Active" )
st = s:AddState( "CollectionIdle" )
local t = st:AddTransition( "Collecting" )
t:AddCondition( TriggerVolumeCondition( "world_geotag_collect_group", "Player1", "Touch" ) )
st = s:AddState( "Collecting" )
s = CreateStateSet( "Tombstone" )
"#;

    #[test]
    fn the_controller_set_and_the_tiers() {
        let g = ssx_lua::game::graph::run_batch(
            ssx_lua::game::Binder::StateSets,
            &[("c.lua".to_string(), SET.as_bytes().to_vec())],
            None,
        );
        let c = geotag_controller(Arc::new(StateSets::from_graph(&g))).unwrap();
        assert_eq!((c.set, c.collect_volume.as_str()), (0, "world_geotag_collect_group"));
        assert_eq!([1, 5, 6, 7, 8, 11].map(offline_tier), [0, 0, 1, 1, 2, 2]);
    }
}
