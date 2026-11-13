pub mod rider;
pub mod spray_chunks;

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use ssx_formats::attrib::Value;
use ssx_formats::lynx::{Behaviour, ParticleSystem};

use crate::vault_db::{Db, fnv1, refs, single};

pub const CLASS_SNOW_SPRAY: u32 = fnv1("ge_snowspray");
pub const CLASS_PARTICLE_NODE: u32 = fnv1("ge_particlenode");
pub const CLASS_LYNX_VFX: u32 = fnv1("ge_lynxvfx");
pub const CLASS_BEHAVIOUR_FILE: u32 = fnv1("ge_particlebehaviourfile");
pub const BASE_SNOW_SPRAY: u32 = fnv1("base_snowspray");
pub const LOD_SNOW_SPRAY: u32 = fnv1("lod_snowspray");

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SpraySurface {
    Snow = 0,
    Packed = 1,
    Ice = 2,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SprayKind {
    Carve = 0,
    Impact = 1,
    ImpactAwesome = 2,
    Kickoff = 3,
    KickoffAwesome = 4,
    Trail = 5,
}

pub const SURFACES: [SpraySurface; 3] = [SpraySurface::Snow, SpraySurface::Packed, SpraySurface::Ice];
pub const KINDS: [SprayKind; 6] = [
    SprayKind::Carve,
    SprayKind::Impact,
    SprayKind::ImpactAwesome,
    SprayKind::Kickoff,
    SprayKind::KickoffAwesome,
    SprayKind::Trail,
];

pub const SLOT_KEYS: [[u32; 6]; 3] = [
    [0x01C4_5447, 0xE4C0_5DBE, 0x8BCA_C69E, 0x36D2_F697, 0xC677_17B3, 0x8781_3896],
    [0x4F70_E581, 0xC7EE_0F54, 0x30FC_ACF4, 0x9AD2_F251, 0x229E_7105, 0xC02D_C34C],
    [0xA6DD_CE03, 0x70D4_A4F0, 0x24A9_2A50, 0x6444_D6D3, 0x0812_D1CF, 0xE150_96B8],
];

#[derive(Clone, Debug)]
pub struct ParticleNode {
    pub key: u32,
    pub scale: [f32; 4],
    pub position: [f32; 4],
    pub orientation: [f32; 4],
    pub joint_name: String,
    pub group_name: String,
    pub lynx: Option<LynxVfx>,
    pub behaviour_path: Option<String>,
    pub unk_77e1adcd: i64,
    pub locator_type: i32,
    pub unk_a20a4677: bool,
    pub unk_d8f68fb4: bool,
}

#[derive(Clone, Debug)]
pub struct LynxVfx {
    pub collection: String,
    pub name: String,
    pub path: String,
    pub definition: String,
}

#[derive(Clone, Debug)]
pub struct SnowSpraySet {
    pub slots: [[Option<ParticleNode>; 6]; 3],
}

impl SnowSpraySet {
    pub fn get(&self, s: SpraySurface, k: SprayKind) -> Option<&ParticleNode> {
        self.slots[s as usize][k as usize].as_ref()
    }
}

pub fn load_snow_spray(db: &Db, collection: u32) -> Result<SnowSpraySet> {
    let fields = db.fields(CLASS_SNOW_SPRAY, collection)?;
    let mut set = SnowSpraySet { slots: Default::default() };
    for (s, row) in SLOT_KEYS.iter().enumerate() {
        for (k, &key) in row.iter().enumerate() {
            let f =
                fields.iter().find(|f| f.key == key).with_context(|| format!("ge_snowspray: no field {key:#010x}"))?;
            set.slots[s][k] = match refs(&f.data).first() {
                Some(r) if r.collection != 0 => {
                    if r.class != CLASS_PARTICLE_NODE {
                        bail!("ge_snowspray {key:#010x} refers to class {}", db.class_name(r.class));
                    }
                    Some(particle_node(db, r.collection)?)
                }
                _ => None,
            };
        }
    }
    Ok(set)
}

fn value<'a>(fields: &[&'a ssx_formats::attrib::Field], key: u32, what: &str) -> Result<&'a Value> {
    let f = fields.iter().find(|f| f.key == key).with_context(|| format!("{what}: no field {key:#010x}"))?;
    single(&f.data).with_context(|| format!("{what}: {key:#010x} is an array"))
}

fn text(fields: &[&ssx_formats::attrib::Field], name: &str) -> Result<String> {
    match value(fields, fnv1(name), name)? {
        Value::Text(t) => Ok(t.clone().unwrap_or_default()),
        v => bail!("{name} = {v:?}, not a text"),
    }
}

fn vec4(fields: &[&ssx_formats::attrib::Field], name: &str) -> Result<[f32; 4]> {
    match value(fields, fnv1(name), name)? {
        Value::Vector4(v) => Ok(*v),
        v => bail!("{name} = {v:?}, not a Vector4"),
    }
}

pub fn particle_node(db: &Db, key: u32) -> Result<ParticleNode> {
    let f = db.fields(CLASS_PARTICLE_NODE, key)?;
    let lynx = match value(&f, fnv1("LynxVfx"), "LynxVfx")? {
        Value::Ref(r) if r.collection == 0 => None,
        Value::Ref(r) if r.class == CLASS_LYNX_VFX => Some(lynx_vfx(db, r.collection)?),
        v => bail!("ge_particlenode {key:#010x}: LynxVfx = {v:?}"),
    };
    let behaviour_path = match value(&f, fnv1("ParticleBehaviour"), "ParticleBehaviour")? {
        Value::Ref(r) if r.collection != 0 => db.text_of(r.class, r.collection)?,
        _ => None,
    };
    let int64 = |k: u32| match value(&f, k, "unk") {
        Ok(Value::Int64(x)) => Ok(*x),
        v => bail!("ge_particlenode {k:#010x} = {v:?}"),
    };
    let int32 = |k: u32| match value(&f, k, "unk") {
        Ok(Value::Int32(x)) => Ok(*x),
        v => bail!("ge_particlenode {k:#010x} = {v:?}"),
    };
    let boolean = |k: u32| match value(&f, k, "unk") {
        Ok(Value::Bool(b)) => Ok(*b),
        v => bail!("ge_particlenode {k:#010x} = {v:?}"),
    };
    Ok(ParticleNode {
        key,
        scale: vec4(&f, "Scale")?,
        position: vec4(&f, "Position")?,
        orientation: vec4(&f, "Orientation")?,
        joint_name: text(&f, "JointName")?,
        group_name: text(&f, "GroupName")?,
        lynx,
        behaviour_path,
        unk_77e1adcd: int64(0x77E1_ADCD)?,
        locator_type: int32(0xC91A_03A1)?,
        unk_a20a4677: boolean(0xA20A_4677)?,
        unk_d8f68fb4: boolean(0xD8F6_8FB4)?,
    })
}

fn lynx_vfx(db: &Db, key: u32) -> Result<LynxVfx> {
    let f = db.fields(CLASS_LYNX_VFX, key)?;
    let path = match value(&f, 0xDF9E_F8B2, "path")? {
        Value::Text(Some(t)) => t.clone(),
        v => bail!("ge_lynxvfx {key:#010x}: path = {v:?}"),
    };
    Ok(LynxVfx {
        collection: db.collection_name(key),
        name: text(&f, "Name")?,
        path,
        definition: text(&f, "Definition")?,
    })
}

pub fn gameexplorer(big_root: &Path) -> PathBuf {
    big_root.join("memresident/data/xenon/gameexplorer")
}

pub fn fx_file(big_root: &Path, vault_path: &str) -> PathBuf {
    let rel = vault_path.replace('\\', "/");
    crate::data_roots::find(big_root, &format!("data/xenon/gameexplorer/{rel}"))
        .unwrap_or_else(|| gameexplorer(big_root).join(rel))
}

pub fn texture_id(texture: &str) -> Option<ssx_content::ContentId> {
    if texture.contains(':') {
        return texture.parse::<ssx_content::ContentId>().ok().filter(|id| id.kind() == ssx_content::Kind::Texture);
    }
    let stem = texture.strip_suffix(".dds").unwrap_or(texture);
    crate::content::textures::texture_id(&format!("fx/textures/{stem}"))
}

pub fn load_node_files(big_root: &Path, node: &ParticleNode) -> Result<(ParticleSystem, Option<Behaviour>)> {
    let vfx = node.lynx.as_ref().with_context(|| format!("ge_particlenode {:#010x} has no effect", node.key))?;
    let sys = crate::content::GameContent::of(big_root).particle_system(&vfx.path)?.as_ref().clone();
    if sys.effect(&vfx.definition).is_none() {
        bail!("{}: no effect {}", vfx.path, vfx.definition);
    }
    let behaviour = match &node.behaviour_path {
        Some(p) => match std::fs::read(fx_file(big_root, p)) {
            Ok(d) => Some(Behaviour::parse(&d).with_context(|| p.clone())?),
            Err(_) => None,
        },
        None => None,
    };
    Ok((sys, behaviour))
}

pub fn load_node_files_optional(
    big_root: &Path,
    node: &ParticleNode,
) -> Result<Option<(ParticleSystem, Option<Behaviour>)>> {
    let Some(vfx) = node.lynx.as_ref() else { return Ok(None) };
    let sys = crate::content::GameContent::of(big_root).particle_system(&vfx.path)?.as_ref().clone();
    if sys.effect(&vfx.definition).is_none() {
        return Ok(None);
    }
    let behaviour = match &node.behaviour_path {
        Some(p) => match std::fs::read(fx_file(big_root, p)) {
            Ok(d) => Some(Behaviour::parse(&d).with_context(|| p.clone())?),
            Err(_) => None,
        },
        None => None,
    };
    Ok(Some((sys, behaviour)))
}

pub struct LoadedSpray {
    pub set: SnowSpraySet,
    pub files: std::collections::HashMap<u32, (ParticleSystem, Option<Behaviour>)>,
}

pub fn load_snow_spray_files(big_root: &Path, collection: u32) -> Result<LoadedSpray> {
    let dir = big_root.join("cache1/data/xenon/attribdb");
    let track = std::fs::read_dir(&dir)
        .with_context(|| format!("reading {}", dir.display()))?
        .flatten()
        .filter_map(|e| e.file_name().to_str()?.strip_suffix("_track.vlt").map(str::to_owned))
        .min()
        .with_context(|| format!("no *_track.vlt in {}", dir.display()))?;
    crate::vault_db::with_track_db(big_root, &track, |db| {
        let set = load_snow_spray(db, collection)?;
        let set = crate::content::GameContent::of(big_root).route_spray(collection, set);
        let mut files = std::collections::HashMap::new();
        for node in set.slots.iter().flatten().flatten() {
            if let std::collections::hash_map::Entry::Vacant(e) = files.entry(node.key) {
                e.insert(load_node_files(big_root, node)?);
            }
        }
        Ok(LoadedSpray { set, files })
    })
}
