use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use anyhow::{Context, Result, bail};
use ssx_formats::attrib::{Field, FieldData, Value};

use crate::track_objects::Placement;
use crate::vault_db::{Db, fnv1, refs, single, with_track_db};

pub mod text;
pub mod truetype;

const CLASS_SCENE_INSTANCE: u32 = fnv1("ge_scene_instance");
const CLASS_LIGHT_NODE: u32 = fnv1("ge_lightnode");
const CLASS_POINT: u32 = fnv1("ge_scene_pointlight");
const CLASS_SPOT: u32 = fnv1("ge_scene_spotlight");
const CLASS_BOX: u32 = fnv1("ge_scene_boxlight");
const CLASS_TEXTURE: u32 = fnv1("ge_texture");
const FIELD_INSTANCE_DATA: u32 = 0xEE66_32AC;
const FIELD_GAMEOBJECT: u32 = 0x3ED7_B5CE;
const FIELD_MODELS: u32 = 0xB43A_3598;
const FIELD_LIGHT_NODES: u32 = fnv1("LightNodes");
const FIELD_CONTROLLER: u32 = fnv1("Controller");

#[derive(Clone, Debug, PartialEq)]
pub struct SceneLight {
    pub name: String,
    pub debug_name: String,
    pub color: [f32; 4],
    pub diffuse_intensity: f32,
    pub specular_intensity: f32,
    pub fall_off: f32,
    pub enable_specular: bool,
    pub enable: bool,
    pub shape: LightShape,
}

#[derive(Clone, Debug, PartialEq)]
pub enum LightShape {
    Point {
        outer_radius: f32,
        inner_radius: f32,
    },
    Spot {
        cone_angle: f32,
        penumbra_angle: f32,
        outer_length: f32,
        inner_length: f32,
        drop_off: f32,
    },
    Box {
        width: f32,
        height: f32,
        penumbra: f32,
        outer_length: f32,
        inner_length: f32,
        projected_texture: Option<String>,
        unk_fe912176: String,
        unk_ebeca5b9: bool,
    },
}

impl LightShape {
    pub fn kind(&self) -> &'static str {
        match self {
            LightShape::Point { .. } => "point",
            LightShape::Spot { .. } => "spot",
            LightShape::Box { projected_texture: Some(_), .. } => "box+texture",
            LightShape::Box { .. } => "box",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct LightNode {
    pub scale: [f32; 4],
    pub position: [f32; 4],
    pub orientation: [f32; 4],
    pub light: Option<SceneLight>,
    pub joint_name: String,
    pub group_name: String,
    pub unk_77e1adcd: i64,
}

#[derive(Clone, Debug, Default)]
pub struct ObjectLights {
    pub object: String,
    pub key: u32,
    pub lods: Vec<Vec<LightNode>>,
    pub controller_script: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct TrackLights {
    pub objects: Vec<ObjectLights>,
    pub instances: Vec<(usize, Vec<Placement>)>,
}

impl TrackLights {
    pub fn placement_count(&self) -> usize {
        self.instances.iter().map(|(_, p)| p.len()).sum()
    }
}

pub fn load_track_lights(big_root: &Path, track: &str) -> Result<TrackLights> {
    with_track_db(big_root, track, |db| {
        let mut out = TrackLights::default();
        let mut by_key: HashMap<u32, Option<usize>> = HashMap::new();
        for (key, _) in db.collections_of(0, CLASS_SCENE_INSTANCE).collect::<Vec<_>>() {
            let fields = db.fields(CLASS_SCENE_INSTANCE, key)?;
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
                    let o = object_lights(db, obj.class, obj.collection)
                        .with_context(|| format!("game object {}", db.collection_name(obj.collection)))?;
                    let i = o.lods.iter().any(|l| !l.is_empty()).then(|| {
                        out.objects.push(o);
                        out.objects.len() - 1
                    });
                    by_key.insert(obj.collection, i);
                    i
                }
            };
            let Some(idx) = idx else { continue };
            let placements = match db.field(data.class, data.collection, FIELD_INSTANCE_DATA)? {
                Some(f) => placements(&f.data),
                None => Vec::new(),
            };
            out.instances.push((idx, placements));
        }
        Ok(out)
    })
}

pub fn map_lights(big_root: &Path, placed: &[(u32, Placement)]) -> Result<TrackLights> {
    crate::vault_db::with_attribdb(big_root, |db| {
        let mut out = TrackLights::default();
        let mut by_key: HashMap<u32, Option<usize>> = HashMap::new();
        for &(key, placement) in placed {
            let idx = match by_key.get(&key) {
                Some(i) => *i,
                None => {
                    let o = object_lights(db, crate::track_objects::CLASS_GAMEOBJECT, key)
                        .with_context(|| format!("game object {key:#010x}"))?;
                    let i = o.lods.iter().any(|l| !l.is_empty()).then(|| {
                        out.objects.push(o);
                        out.objects.len() - 1
                    });
                    by_key.insert(key, i);
                    i
                }
            };
            if let Some(idx) = idx {
                out.instances.push((idx, vec![placement]));
            }
        }
        Ok(out)
    })
}

impl TrackLights {
    pub fn extend(&mut self, other: TrackLights) {
        let offset = self.objects.len();
        self.objects.extend(other.objects);
        self.instances.extend(other.instances.into_iter().map(|(i, p)| (i + offset, p)));
    }
}

pub fn all_lights(db: &Db) -> Result<BTreeMap<&'static str, Vec<SceneLight>>> {
    let mut out = BTreeMap::new();
    for (class, name) in
        [(CLASS_POINT, "ge_scene_pointlight"), (CLASS_SPOT, "ge_scene_spotlight"), (CLASS_BOX, "ge_scene_boxlight")]
    {
        let keys: Vec<u32> = db.collections_of_all(class).into_iter().map(|(k, _)| k).collect();
        let lights = keys.into_iter().map(|k| scene_light(db, class, k)).collect::<Result<Vec<_>>>()?;
        out.insert(name, lights);
    }
    Ok(out)
}

pub fn all_light_nodes(db: &Db) -> Result<Vec<LightNode>> {
    let keys: Vec<u32> = db.collections_of_all(CLASS_LIGHT_NODE).into_iter().map(|(k, _)| k).collect();
    keys.into_iter().map(|k| light_node(db, k)).collect()
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

fn object_lights(db: &Db, class: u32, key: u32) -> Result<ObjectLights> {
    let mut o = ObjectLights { object: db.collection_name(key), key, lods: Vec::new(), controller_script: None };
    if let Some(f) = db.field(class, key, FIELD_CONTROLLER)?
        && let Some(r) = refs(&f.data).into_iter().find(|r| r.collection != 0)
    {
        o.controller_script = db.text_of(r.class, r.collection)?;
    }
    let Some(models) = db.field(class, key, FIELD_MODELS)? else { return Ok(o) };
    for m in refs(&models.data).into_iter().filter(|r| r.collection != 0) {
        let mut nodes = Vec::new();
        if let Some(f) = db.field(m.class, m.collection, FIELD_LIGHT_NODES)? {
            for r in refs(&f.data).into_iter().filter(|r| r.collection != 0) {
                if r.class != CLASS_LIGHT_NODE {
                    bail!("LightNodes entry of class {}", db.class_name(r.class));
                }
                nodes.push(light_node(db, r.collection)?);
            }
        }
        o.lods.push(nodes);
    }
    Ok(o)
}

struct Fields<'a> {
    what: String,
    fields: Vec<&'a Field>,
}

impl<'a> Fields<'a> {
    fn new(db: &'a Db, class: u32, key: u32) -> Result<Self> {
        Ok(Fields {
            what: format!("{}/{}", db.class_name(class), db.collection_name(key)),
            fields: db.fields(class, key)?,
        })
    }

    fn value(&self, name: &str, key: u32) -> Result<&'a Value> {
        let f = self.fields.iter().find(|f| f.key == key).with_context(|| format!("{}: no field {name}", self.what))?;
        single(&f.data).with_context(|| format!("{}: {name} is an array", self.what))
    }

    fn float(&self, name: &str) -> Result<f32> {
        match self.value(name, fnv1(name))? {
            Value::Float(x) => Ok(*x),
            v => bail!("{}: {name} = {v:?}, not a float", self.what),
        }
    }

    fn boolean(&self, name: &str, key: u32) -> Result<bool> {
        match self.value(name, key)? {
            Value::Bool(b) => Ok(*b),
            v => bail!("{}: {name} = {v:?}, not a bool", self.what),
        }
    }

    fn vec4(&self, name: &str) -> Result<[f32; 4]> {
        match self.value(name, fnv1(name))? {
            Value::Vector4(v) => Ok(*v),
            v => bail!("{}: {name} = {v:?}, not a Vector4", self.what),
        }
    }

    fn text(&self, name: &str, key: u32) -> Result<String> {
        let f = self.fields.iter().find(|f| f.key == key).with_context(|| format!("{}: no field {name}", self.what))?;
        match single(&f.data) {
            Some(Value::Text(t)) => Ok(t.clone().unwrap_or_default()),
            v => bail!("{}: {name} = {v:?}, not a text", self.what),
        }
    }
}

pub fn light_node(db: &Db, key: u32) -> Result<LightNode> {
    let f = Fields::new(db, CLASS_LIGHT_NODE, key)?;
    let light = match f.value("Light", fnv1("Light"))? {
        Value::Ref(r) if r.collection == 0 => None,
        Value::Ref(r) => Some(scene_light(db, r.class, r.collection)?),
        v => bail!("{}: Light = {v:?}", f.what),
    };
    let unk_77e1adcd = match f.value("0x77E1ADCD", 0x77E1_ADCD)? {
        Value::Int64(x) => *x,
        v => bail!("{}: 0x77E1ADCD = {v:?}", f.what),
    };
    Ok(LightNode {
        scale: f.vec4("Scale")?,
        position: f.vec4("Position")?,
        orientation: f.vec4("Orientation")?,
        light,
        joint_name: f.text("JointName", fnv1("JointName"))?,
        group_name: f.text("GroupName", fnv1("GroupName"))?,
        unk_77e1adcd,
    })
}

fn scene_light(db: &Db, class: u32, key: u32) -> Result<SceneLight> {
    let f = Fields::new(db, class, key)?;
    let shape = match class {
        CLASS_POINT => {
            LightShape::Point { outer_radius: f.float("OuterRadius")?, inner_radius: f.float("InnerRadius")? }
        }
        CLASS_SPOT => LightShape::Spot {
            cone_angle: f.float("ConeAngle")?,
            penumbra_angle: f.float("PenumbraAngle")?,
            outer_length: f.float("OuterLength")?,
            inner_length: f.float("InnerLength")?,
            drop_off: f.float("DropOff")?,
        },
        CLASS_BOX => {
            let projected_texture = match f.value("ProjectedTexture", fnv1("ProjectedTexture"))? {
                Value::Ref(r) if r.collection == 0 => None,
                Value::Ref(r) if r.class == CLASS_TEXTURE => db.text_of(r.class, r.collection)?,
                v => bail!("{}: ProjectedTexture = {v:?}", f.what),
            };
            LightShape::Box {
                width: f.float("Width")?,
                height: f.float("Height")?,
                penumbra: f.float("Penumbra")?,
                outer_length: f.float("OuterLength")?,
                inner_length: f.float("InnerLength")?,
                projected_texture,
                unk_fe912176: f.text("0xFE912176", 0xFE91_2176)?,
                unk_ebeca5b9: f.boolean("0xEBECA5B9", 0xEBEC_A5B9)?,
            }
        }
        _ => bail!("{}: not a light class", f.what),
    };
    Ok(SceneLight {
        name: db.collection_name(key),
        debug_name: f.text("DebugName", fnv1("DebugName"))?,
        color: f.vec4("Color")?,
        diffuse_intensity: f.float("DiffuseIntensity")?,
        specular_intensity: f.float("SpecularIntensity")?,
        fall_off: f.float("FallOff")?,
        enable_specular: f.boolean("EnableSpecular", fnv1("EnableSpecular"))?,
        enable: f.boolean("Enable", fnv1("Enable"))?,
        shape,
    })
}
