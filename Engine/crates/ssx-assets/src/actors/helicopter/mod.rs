mod animation;

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use ssx_formats::attrib::collection::{self, Collection, DecodedCollection, Field, FieldData};
use ssx_formats::attrib::{Schema, Value, Vault, fnv1};
use ssx_formats::crsf::{CrsfFile, SceneGraph};

use crate::character::{Skin, Srt};
use crate::ghosts::{CLASS_GAMEOBJECT, FIELD_LOD_THRESHOLDS, FIELD_MODELS, bind_pose, part_refs, skin_of};
use crate::object_mesh::ObjectMesh;
use crate::track_objects::asset_file;
use crate::vault_db::{Material, refs, with_level_db};

pub use animation::*;

const CLASS_UI_DROP: u32 = fnv1(b"ui_drop");
const CLASS_GE_DROP: u32 = fnv1(b"ge_drop");
const CLASS_PEAK: u32 = fnv1(b"peak");
const CLASS_RANGE: u32 = fnv1(b"range");
const CLASS_HELICOPTER: u32 = fnv1(b"helicopter");
const FIELD_UI_DROP_GE_DROP: u32 = 0xB9C2_CFC5;
const FIELD_UI_DROP_HELICOPTER: u32 = 0x2101_12EA;
const FIELD_UI_DROP_PEAK: u32 = 0x8296_DA5A;
const FIELD_GE_DROP_PEAK: u32 = 0x17E8_41D0;
const FIELD_PEAK_RANGE: u32 = 0xAB6B_2547;
const FIELD_RANGE_HELICOPTER: u32 = fnv1(b"helicopter");
const FIELD_HELI_GAMEOBJECT: u32 = 0x58F5_AE0E;
const FIELD_HELI_NAME: u32 = 0x8097_DCBE;
const FIELD_HELI_DISPLAY: u32 = 0x5A0F_251D;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HeliSource {
    Drop,
    Range,
    Default,
}

pub const DEFAULT_HELICOPTER: u32 = 0xB48A_C47B;

#[derive(Clone, Debug, PartialEq)]
pub struct HeliChoice {
    pub key: u32,
    pub source: HeliSource,
    pub peak: u32,
    pub range: u32,
    pub name: String,
    pub display: String,
    pub gameobject: u32,
}

pub(crate) struct Vaults {
    cols: Vec<Vec<Collection>>,
    decoded: Vec<Vec<DecodedCollection>>,
    index: Vec<HashMap<(u32, u32), usize>>,
}

impl Vaults {
    pub(crate) fn load(big_root: &Path) -> Result<Self> {
        let dir = big_root.join("bootcache/data/xenon/attribdb");
        let read = |path: &Path| -> Result<(Vec<u8>, Vec<u8>)> {
            Ok((
                std::fs::read(path).with_context(|| format!("reading {}", path.display()))?,
                std::fs::read(path.with_extension("bin")).with_context(|| format!("reading {}", path.display()))?,
            ))
        };
        let (svlt, sbin) = read(&dir.join("attribdb_schema.vlt"))?;
        let schema = Schema::from_vault(&Vault::parse(&svlt, &sbin)?).context("reading the schema")?;
        let known: std::collections::HashSet<u32> = schema.classes.iter().map(|c| c.key).collect();
        let mut sources: Vec<(PathBuf, bool)> =
            crate::vault_db::dlc_vaults(big_root).into_iter().map(|p| (p, false)).collect();
        sources.push((dir.join("attribdb.vlt"), true));
        let mut out = Self { cols: Vec::new(), decoded: Vec::new(), index: Vec::new() };
        for (path, route) in sources {
            let (vlt, bin) = read(&path)?;
            let vault = Vault::parse(&vlt, &bin).with_context(|| format!("parsing {}", path.display()))?;
            let cols = collection::collections(&vault)?;
            let mut kept = Vec::with_capacity(cols.len());
            let mut decoded = Vec::with_capacity(cols.len());
            for c in cols {
                if !known.contains(&c.class) {
                    continue;
                }
                decoded.push(c.decode(&vault, &schema).with_context(|| format!("collection {:#010x}", c.key))?);
                kept.push(c);
            }
            if route {
                crate::content::data::events::route_ui_drops(big_root, &kept, &mut decoded);
            }
            out.index.push(collection::index(&kept));
            out.cols.push(kept);
            out.decoded.push(decoded);
        }
        Ok(out)
    }

    fn field(&self, class: u32, coll: u32, key: u32) -> Result<Option<&Field>> {
        for v in 0..self.cols.len() {
            if let Some(&i) = self.index[v].get(&(class, coll)) {
                let fields = collection::with_inherited(&self.cols[v], &self.decoded[v], &self.index[v], i)?;
                return Ok(fields.into_iter().map(|(_, f)| f).find(|f| f.key == key));
            }
        }
        bail!("collection {class:#010x}/{coll:#010x} not found")
    }

    pub(crate) fn reference(&self, class: u32, coll: u32, key: u32) -> Result<u32> {
        Ok(self.field(class, coll, key)?.map_or(0, |f| refs(&f.data).first().map_or(0, |r| r.collection)))
    }

    fn text(&self, class: u32, coll: u32, key: u32) -> Result<Option<String>> {
        Ok(self.field(class, coll, key)?.and_then(|f| match &f.data {
            FieldData::Single(Value::Text(t)) => t.clone(),
            FieldData::Array(v) => v.iter().find_map(|x| if let Value::Text(t) = x { t.clone() } else { None }),
            _ => None,
        }))
    }
}

pub fn drop_helicopter(big_root: &Path, ui_drop: &str) -> Result<HeliChoice> {
    choice(&Vaults::load(big_root)?, fnv1(ui_drop.as_bytes())).with_context(|| format!("ui_drop {ui_drop}"))
}

pub fn default_helicopter(big_root: &Path) -> Result<HeliChoice> {
    helicopter(&Vaults::load(big_root)?, DEFAULT_HELICOPTER, HeliSource::Default, 0, 0)
}

pub fn survey(big_root: &Path) -> Result<Vec<(u32, Result<HeliChoice>)>> {
    let v = Vaults::load(big_root)?;
    let keys: Vec<u32> = v.cols[0].iter().filter(|c| c.class == CLASS_UI_DROP).map(|c| c.key).collect();
    Ok(keys.into_iter().map(|k| (k, choice(&v, k))).collect())
}

pub fn drop_range(big_root: &Path, ui_drop: &str) -> Result<(u32, u32)> {
    peak_range(&Vaults::load(big_root)?, fnv1(ui_drop.as_bytes())).with_context(|| format!("ui_drop {ui_drop}"))
}

fn peak_range(v: &Vaults, drop: u32) -> Result<(u32, u32)> {
    let mut peak = v.reference(CLASS_UI_DROP, drop, FIELD_UI_DROP_PEAK)?;
    if peak == 0 {
        let ge_drop = v.reference(CLASS_UI_DROP, drop, FIELD_UI_DROP_GE_DROP)?;
        let name = v
            .text(CLASS_GE_DROP, ge_drop, FIELD_GE_DROP_PEAK)?
            .with_context(|| format!("ge_drop {ge_drop:08x} names no peak"))?;
        peak = fnv1(name.as_bytes());
    }
    Ok((peak, v.reference(CLASS_PEAK, peak, FIELD_PEAK_RANGE)?))
}

fn choice(v: &Vaults, drop: u32) -> Result<HeliChoice> {
    let own = v.reference(CLASS_UI_DROP, drop, FIELD_UI_DROP_HELICOPTER)?;
    let (peak, range) = peak_range(v, drop)?;
    let (key, source) = if own != 0 {
        (own, HeliSource::Drop)
    } else {
        let r = v.reference(CLASS_RANGE, range, FIELD_RANGE_HELICOPTER)?;
        anyhow::ensure!(r != 0, "neither the drop nor its range {range:08x} names a helicopter");
        (r, HeliSource::Range)
    };
    helicopter(v, key, source, peak, range)
}

fn helicopter(v: &Vaults, key: u32, source: HeliSource, peak: u32, range: u32) -> Result<HeliChoice> {
    let gameobject = v.reference(CLASS_HELICOPTER, key, FIELD_HELI_GAMEOBJECT)?;
    anyhow::ensure!(gameobject != 0, "helicopter {key:08x} has no game object");
    Ok(HeliChoice {
        key,
        source,
        peak,
        range,
        name: v.text(CLASS_HELICOPTER, key, FIELD_HELI_NAME)?.unwrap_or_default(),
        display: v.text(CLASS_HELICOPTER, key, FIELD_HELI_DISPLAY)?.unwrap_or_default(),
        gameobject,
    })
}

#[derive(Clone, Debug)]
pub struct HeliPart {
    pub mesh_name: String,
    pub mesh: ObjectMesh,
    pub skin: Option<Skin>,
    pub material: Material,
}

#[derive(Clone, Debug)]
pub struct HeliModel {
    pub lod_thresholds: Vec<f32>,
    pub rig: SceneGraph,
    pub bind: Vec<Srt>,
    pub parts: Vec<HeliPart>,
    pub problems: Vec<String>,
}

pub fn load_model(big_root: &Path, track: &str, key: u32) -> Result<HeliModel> {
    load_model_in(big_root, Some(track), key)
}

pub fn load_model_in(big_root: &Path, track: Option<&str>, key: u32) -> Result<HeliModel> {
    let (lod_thresholds, lod0) = with_level_db(big_root, track, |db| {
        let mut thresholds = Vec::new();
        let mut models = Vec::new();
        for f in db.fields(CLASS_GAMEOBJECT, key)? {
            match f.key {
                FIELD_LOD_THRESHOLDS => {
                    let vs: Vec<&Value> = match &f.data {
                        FieldData::Array(vs) => vs.iter().collect(),
                        FieldData::Single(v) => vec![v],
                    };
                    thresholds =
                        vs.into_iter().filter_map(|v| if let Value::Float(x) = v { Some(*x) } else { None }).collect();
                }
                FIELD_MODELS => {
                    for m in refs(&f.data).into_iter().filter(|r| r.collection != 0) {
                        models.push(part_refs(db, m.class, m.collection)?);
                    }
                }
                _ => {}
            }
        }
        Ok((thresholds, models.into_iter().next().with_context(|| format!("game object {key:08x} has no model"))?))
    })?;
    let mut problems = Vec::new();
    let mut loaded = Vec::new();
    let mut rig: Option<SceneGraph> = None;
    for p in lod0 {
        let read = || -> Result<(ObjectMesh, CrsfFile)> {
            let crsf_rel = p.crsf.as_deref().context("no FileRef (.crsf)")?;
            let mesh = crate::content::models::rider_mesh(big_root, &p.geom)?;
            let crsf = asset_file(big_root, crsf_rel).with_context(|| format!("{crsf_rel} not found"))?;
            let file = CrsfFile::parse(&std::fs::read(&crsf).with_context(|| format!("reading {}", crsf.display()))?)
                .with_context(|| format!("parsing {}", crsf.display()))?;
            Ok((mesh, file))
        };
        match read() {
            Ok((mesh, file)) => {
                if let Some(scene) = &file.scene {
                    let joints = |g: &SceneGraph| -> Vec<(String, Option<String>)> {
                        g.nodes
                            .iter()
                            .filter(|n| matches!(n.kind, ssx_formats::crsf::NodeKind::Joint(_)))
                            .map(|n| (n.name.clone(), n.parent.map(|q| g.nodes[q].name.clone())))
                            .collect()
                    };
                    match &rig {
                        None => rig = Some(scene.clone()),
                        Some(r) if joints(r) != joints(scene) => {
                            problems.push(format!("{}: its skeleton differs from the first part's", p.name));
                        }
                        Some(_) => {}
                    }
                }
                loaded.push((p, mesh, file));
            }
            Err(e) => problems.push(format!("{}: {e:#}", p.name)),
        }
    }
    let rig = rig.context("no helicopter part has a scene graph")?;
    let mut face_offsets = HashMap::new();
    for (_, _, file) in &loaded {
        for o in file.face_offsets.iter().flatten() {
            face_offsets.insert(o.joint.clone(), o.translate);
        }
    }
    let bind = bind_pose(&rig, &face_offsets);
    let mut parts = Vec::new();
    for (p, mesh, file) in loaded {
        let skin = if file.weights.is_some() {
            match skin_of(&file, &rig, mesh.positions.len()) {
                Ok(s) => Some(s),
                Err(e) => {
                    problems.push(format!("{}: {e:#}", p.name));
                    continue;
                }
            }
        } else {
            None
        };
        parts.push(HeliPart { mesh_name: p.name, mesh, skin, material: p.material });
    }
    if parts.is_empty() {
        bail!("game object {key:08x}: no part loaded ({})", problems.join("; "));
    }
    Ok(HeliModel { lod_thresholds, rig, bind, parts, problems })
}
