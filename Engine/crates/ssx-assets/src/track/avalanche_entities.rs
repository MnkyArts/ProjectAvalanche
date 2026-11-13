use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context, Result, bail};

use crate::particles::{CLASS_PARTICLE_NODE, ParticleNode, particle_node};
use crate::track_objects::asset_file;
use crate::vault_db::{Db, Material, fnv1, refs, single, with_track_db};

pub const SCRIPTS_DIR: &str = "memresident/data/common/scripts";
pub const SCHEMAS_SCRIPT: &str = "world_entity_schemas_default.lua";
pub const SCENE_SCRIPT: &str = "default_scene.lua";

const CLASS_GAMEOBJECT: u32 = fnv1("ge_scene_gameobject");
const CLASS_GEOMETRY: u32 = fnv1("ge_geometry");
const CLASS_MATERIAL: u32 = fnv1("ge_material");
const FIELD_MODELS: u32 = 0xB43A_3598;
const FIELD_MESHES: u32 = fnv1("Meshes");
const FIELD_MATERIAL: u32 = fnv1("Material");
const FIELD_GEOMETRY: u32 = fnv1("Geometry");
const FIELD_PARTICLE_NODES: u32 = fnv1("ParticleNodes");

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Binding {
    pub row: u8,
    pub sim_type: u8,
    pub set: String,
}

pub fn batches(
    big_root: &Path,
    scene_scripts: &[String],
) -> Result<(ssx_lua::game::scene::Schemas, ssx_lua::game::scene::Scene)> {
    use ssx_lua::game::Binder;
    use ssx_lua::game::graph::run_batch;
    use ssx_lua::game::scene::{Scene, Schemas};
    let read = |n: &str| read_script(big_root, n).map(|t| (n.to_string(), t.into_bytes()));
    let schemas = Schemas::from_graph(&run_batch(Binder::Schemas, &[read(SCHEMAS_SCRIPT)?], None));
    let mut batch = vec![read(SCENE_SCRIPT)?];
    for s in scene_scripts {
        batch.push(read(s)?);
    }
    let scene = Scene::from_graph(&run_batch(Binder::Scene, &batch, None));
    for (name, error) in schemas.scripts.iter().chain(&scene.scripts) {
        if let Some(e) = error {
            bail!("{name}: {e}");
        }
    }
    Ok((schemas, scene))
}

pub fn schema_game_object(schemas: &ssx_lua::game::scene::Schemas, schema: &str) -> Option<String> {
    schemas.schemas.get(schema)?.factories.iter().filter(|f| f.kind == "GameObject").find_map(|f| {
        f.calls.iter().rev().find(|(m, _)| m == "SetGameObjectName").and_then(|(_, a)| Some(a.first()?.str()?.into()))
    })
}

pub fn spray_bindings(schemas: &ssx_lua::game::scene::Schemas) -> Vec<Binding> {
    let Some(s) = schemas.schemas.get("Snowboarder") else { return Vec::new() };
    s.factories
        .iter()
        .filter(|f| f.kind == "MeshParticles")
        .flat_map(|f| &f.calls)
        .filter(|(m, _)| m == "BindSpraySimParticles")
        .filter_map(|(_, a)| {
            let n = |i: usize| a.get(i).and_then(ssx_lua::game::graph::Arg::num);
            Some(Binding { row: n(0)? as u8, sim_type: n(1)? as u8, set: a.get(2)?.str()?.to_string() })
        })
        .collect()
}

pub type SceneLists = (Vec<(String, u32, String)>, HashMap<String, u32>);

pub fn scene_lists(scene: &ssx_lua::game::scene::Scene) -> SceneLists {
    let lists =
        scene.entities.iter().filter_map(|e| Some((e.schema.clone(), e.count as u32, e.set.clone()?))).collect();
    let sizes = scene.sets.iter().map(|(n, c)| (n.clone(), *c as u32)).collect();
    (lists, sizes)
}

#[derive(Clone, Debug)]
pub struct EntityModel {
    pub schema: String,
    pub game_object: String,
    pub geometry: String,
    pub material: Material,
    pub nodes: Vec<ParticleNode>,
}

#[derive(Clone, Debug)]
pub struct BoundSet {
    pub binding: Binding,
    pub capacity: u32,
    pub entities: Vec<usize>,
}

#[derive(Clone, Debug, Default)]
pub struct AvalancheEntities {
    pub models: Vec<EntityModel>,
    pub sets: Vec<BoundSet>,
    pub scripts: Vec<String>,
}

fn read_script(big_root: &Path, rel: &str) -> Result<String> {
    let p = big_root.join(SCRIPTS_DIR).join(rel.replace('\\', "/"));
    std::fs::read_to_string(&p).with_context(|| format!("reading {}", p.display()))
}

pub fn load_bound(
    big_root: &Path,
    track: &str,
    scene_scripts: &[String],
    sim_types: &[u8],
) -> Result<AvalancheEntities> {
    let (schemas, scene) = batches(big_root, scene_scripts)?;
    let (lists, sizes) = scene_lists(&scene);
    let bindings: Vec<Binding> =
        spray_bindings(&schemas).into_iter().filter(|b| sim_types.contains(&b.sim_type)).collect();
    let mut out = AvalancheEntities { scripts: scene_scripts.to_vec(), ..Default::default() };
    with_track_db(big_root, track, |db| {
        let mut by_schema: HashMap<String, usize> = HashMap::new();
        for b in bindings {
            let mut entities = Vec::new();
            for (schema, n, _) in lists.iter().filter(|(_, _, set)| *set == b.set) {
                let m = match by_schema.get(schema) {
                    Some(&m) => m,
                    None => {
                        let object = schema_game_object(&schemas, schema)
                            .with_context(|| format!("schema {schema} has no game object"))?;
                        out.models.push(entity_model(db, schema, &object)?);
                        by_schema.insert(schema.clone(), out.models.len() - 1);
                        out.models.len() - 1
                    }
                };
                entities.extend(std::iter::repeat_n(m, *n as usize));
            }
            let capacity = sizes.get(&b.set).copied().unwrap_or(0);
            if entities.len() > capacity as usize {
                bail!("{} entities in {} (capacity {capacity})", entities.len(), b.set);
            }
            out.sets.push(BoundSet { binding: b, capacity, entities });
        }
        Ok(())
    })?;
    Ok(out)
}

pub fn load(big_root: &Path, track: &str, avalanche_scripts: &[String]) -> Result<AvalancheEntities> {
    load_bound(big_root, track, avalanche_scripts, &[2, 3])
}

fn entity_model(db: &Db, schema: &str, object: &str) -> Result<EntityModel> {
    let key = fnv1(object);
    let models = db.field(CLASS_GAMEOBJECT, key, FIELD_MODELS)?.with_context(|| format!("{object}: no models"))?;
    let model = refs(&models.data).into_iter().find(|r| r.collection != 0).context("no LOD-0 model")?;
    let mut nodes = Vec::new();
    if let Some(f) = db.field(model.class, model.collection, FIELD_PARTICLE_NODES)? {
        for r in refs(&f.data).into_iter().filter(|r| r.collection != 0) {
            if r.class != CLASS_PARTICLE_NODE {
                bail!("{object}: ParticleNodes entry of class {}", db.class_name(r.class));
            }
            nodes.push(particle_node(db, r.collection)?);
        }
    }
    let meshes = db.field(model.class, model.collection, FIELD_MESHES)?.context("model without meshes")?;
    for r in refs(&meshes.data).into_iter().filter(|r| r.collection != 0) {
        let fields = db.fields(r.class, r.collection)?;
        let get = |k: u32| fields.iter().find(|f| f.key == k).and_then(|f| single(&f.data));
        let Some(ssx_formats::attrib::Value::Ref(g)) = get(FIELD_GEOMETRY) else { continue };
        if g.class != CLASS_GEOMETRY || g.collection == 0 {
            continue;
        }
        let Some(geometry) = db.text_of(g.class, g.collection)? else { continue };
        let material = match get(FIELD_MATERIAL) {
            Some(ssx_formats::attrib::Value::Ref(m)) if m.class == CLASS_MATERIAL && m.collection != 0 => {
                db.material(m.class, m.collection)?
            }
            _ => Material::default(),
        };
        return Ok(EntityModel { schema: schema.into(), game_object: object.into(), geometry, material, nodes });
    }
    bail!("{object}: no mesh with a geometry")
}

pub fn geometry_file(big_root: &Path, model: &EntityModel) -> Option<std::path::PathBuf> {
    asset_file(big_root, &model.geometry)
}
