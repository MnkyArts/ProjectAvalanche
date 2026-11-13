pub mod focus;
pub mod look;

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context, Result};
use ssx_formats::attrib::{Value, fnv1};

use super::data::{CLASS_PEAK, CLASS_RANGE, CLASS_UI_DROP, Vaults};
use crate::track_lighting::{LightingPreset, RECOVERED_FIELD_NAMES};
use crate::vault_db::{Db, Material, refs, single, with_attribdb, with_track_db};

pub const CLASS_GLOBE: u32 = fnv1(b"globe");
const CLASS_LIGHTING: u32 = fnv1(b"ge_globallightingparams");
const CLASS_TEXTURE: u32 = fnv1(b"ge_texture");
const CLASS_GAMEOBJECT: u32 = fnv1(b"ge_scene_gameobject");
const CLASS_MODEL: u32 = fnv1(b"ge_scene_model");
const CLASS_GEOMETRY: u32 = fnv1(b"ge_geometry");
const CLASS_MATERIAL: u32 = fnv1(b"ge_material");
pub const EARTH: u32 = 0xDF61_7463;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Shot {
    pub offset: [f32; 3],
    pub look: [f32; 3],
    pub target: [f32; 3],
    pub fov: f32,
}

#[derive(Clone, Debug, Default)]
pub struct Globe {
    pub radius: f32,
    pub atmosphere_radius: f32,
    pub shots: [Shot; 5],
    pub up: [f32; 3],
    pub lighting: u32,
    pub lighting_dd: u32,
    pub postfx: String,
    pub object: u32,
    pub name: String,
}

#[derive(Clone, Debug, Default)]
pub struct RangeView {
    pub key: u32,
    pub name: String,
    pub lon_lat: [f32; 2],
    pub shot: Shot,
    pub midground: u32,
    pub midground_position: [f32; 3],
    pub midground_rotation: [f32; 3],
    pub midground_scale: [f32; 3],
    pub lighting: u32,
    pub lighting_dd: u32,
    pub postfx: String,
    pub postfx_dd: String,
    pub unk_offset: [f32; 3],
    pub drop_scale: f32,
}

#[derive(Clone, Debug, Default)]
pub struct PeakView {
    pub key: u32,
    pub range: u32,
    pub position: [f32; 3],
    pub shot_peak: Shot,
    pub shot_peak_wide: Shot,
    pub shot_drop: Shot,
    pub dd: bool,
}

#[derive(Clone, Debug, Default)]
pub struct DropView {
    pub key: u32,
    pub peak: u32,
    pub drop_type: i64,
    pub custom_status: i64,
    pub event_type: i64,
    pub position: [f32; 3],
    pub title: String,
    pub selectable: bool,
}

#[derive(Clone, Debug, Default)]
pub struct ObjectMesh {
    pub geometry: String,
    pub material: Material,
}

#[derive(Clone, Debug, Default)]
pub struct GlobeData {
    pub globe: Globe,
    pub ranges: Vec<RangeView>,
    pub peaks: Vec<PeakView>,
    pub drops: Vec<DropView>,
    pub globe_meshes: Vec<ObjectMesh>,
    pub midground_meshes: BTreeMap<u32, Vec<ObjectMesh>>,
    pub lighting: BTreeMap<u32, LightingPreset>,
    pub postfx: look::FePostFx,
    pub globe_particles: Vec<ParticleFx>,
    pub midground_particles: BTreeMap<u32, Vec<ParticleFx>>,
    pub ui_drop_order: Vec<u32>,
}

#[derive(Clone, Debug)]
pub struct ParticleFx {
    pub node: crate::particles::ParticleNode,
    pub effect: ssx_formats::lynx::Effect,
}

fn v3(v: Option<Vec<f32>>) -> [f32; 3] {
    let v = v.unwrap_or_default();
    [0, 1, 2].map(|i| v.get(i).copied().unwrap_or(0.0))
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn add(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

const DEG: f32 = 0.017_453_29;

impl GlobeData {
    pub fn load(big_root: &Path) -> Result<Self> {
        let v = Vaults::load(big_root)?;
        let g = v.rec(CLASS_GLOBE, EARTH)?;
        let up = v3(g.vector(0x4571_CCE8));
        let shot = |a: u32, b: u32, fov: u32| {
            let (a, b) = (v3(g.vector(a)), v3(g.vector(b)));
            Shot { offset: sub(b, a), look: [0.0; 3], target: add(a, up), fov: g.float(fov).unwrap_or(0.0) * DEG }
        };
        let globe = Globe {
            radius: g.float(0x2B8B_D88D).unwrap_or(0.0),
            atmosphere_radius: g.float(0x5BF2_17F6).unwrap_or(0.0),
            shots: [
                shot(0x2545_4A85, 0x95CA_45B3, 0xA9F8_4843),
                shot(0x45D7_7C9F, 0x8718_C569, 0x564F_FA79),
                shot(0xDE16_708F, 0xAAAA_84B9, 0x40D4_C669),
                shot(0xF211_59D5, 0x83A9_7563, 0x419D_B353),
                shot(0xC15D_E019, 0x26E0_E6E7, 0xAB53_6697),
            ],
            up,
            lighting: g.reference(0xBB8E_71C4),
            lighting_dd: g.reference(0x34DE_693D),
            postfx: postfx_name(&v, g.reference(fnv1(b"postfx"))),
            object: g.reference(0x610C_844D),
            name: g.text(0x7D8F_357D).unwrap_or_default(),
        };
        let mut ranges = Vec::new();
        for key in v.keys_of(CLASS_RANGE) {
            let r = v.rec(CLASS_RANGE, key)?;
            // TODO(guess): the scene's range nodes come from a "ranges" node list (0x8249C918 → 0x8249C168) whose data is not traced; the port takes the Earth's `range` collections with index 0x2AACC36E set (1…9); settle: the source of 0x8249C168's "ranges" node.
            if r.reference(0x4963_5FFF) != EARTH || r.int(0x2AAC_C36E).unwrap_or(0) == 0 {
                continue;
            }
            let ll = r.vector(0x5A70_806E).unwrap_or_default();
            ranges.push(RangeView {
                key,
                name: v.name("range", key),
                lon_lat: [ll.first().copied().unwrap_or(0.0), ll.get(1).copied().unwrap_or(0.0)],
                shot: Shot {
                    offset: v3(r.vector(0xE423_D1BB)),
                    look: v3(r.vector(0xFAE6_CA59)),
                    target: [0.0; 3],
                    fov: r.float(0xC447_C4DC).unwrap_or(0.0) * DEG,
                },
                midground: r.reference(0xE188_29B5),
                midground_position: v3(r.vector(0xD59A_C472)),
                midground_rotation: v3(r.vector(0x8C40_9A7B)),
                midground_scale: v3(r.vector(0x05FD_6665)),
                lighting: r.reference(0x4D73_0B2F),
                lighting_dd: r.reference(0xA25F_0A8C),
                postfx: postfx_name(&v, r.reference(fnv1(b"postfx"))),
                postfx_dd: postfx_name(&v, r.reference(0x869A_E05C)),
                unk_offset: v3(r.vector(0x4571_CCE8)),
                drop_scale: r.float(0xC5A9_AF41).unwrap_or(0.0),
            });
        }
        let mut peaks = Vec::new();
        for key in v.keys_of(CLASS_PEAK) {
            let p = v.rec(CLASS_PEAK, key)?;
            let shot = |offset: u32, look: u32, fov: u32| Shot {
                offset: v3(p.vector(offset)),
                look: v3(p.vector(look)),
                target: [0.0; 3],
                fov: p.float(fov).unwrap_or(0.0) * DEG,
            };
            peaks.push(PeakView {
                key,
                range: p.reference(0xAB6B_2547),
                position: v3(p.vector(fnv1(b"position"))),
                shot_peak: shot(0xBD46_56E0, 0xBAD1_AEA6, 0xBD23_C427),
                shot_peak_wide: shot(0x9746_39E4, 0x8ECD_EEEA, 0x970E_8CEB),
                shot_drop: shot(0x9EF1_4AB2, 0x630F_3BF4, 0x9EC4_2B55),
                dd: p.int(0x1B2B_75B8) == Some(1),
            });
        }
        let mut drops = Vec::new();
        for key in v.keys_of(super::data::CLASS_UI_DROP) {
            let r = v.rec(super::data::CLASS_UI_DROP, key)?;
            let drop_type = r.int(0x555E_EBD2).unwrap_or(-1);
            if drop_type == -1 {
                continue;
            }
            let ge = v.rec(super::data::CLASS_GE_DROP, r.reference(0xB9C2_CFC5)).ok();
            let peak = match r.reference(0x8296_DA5A) {
                0 => ge.as_ref().and_then(|g| g.text(0x17E8_41D0)).map_or(0, |t| fnv1(t.as_bytes())),
                p => p,
            };
            let own = v3(r.vector(0x45B5_F74A));
            let position = if own.iter().all(|c| c.abs() <= 1e-6) {
                ge.as_ref().map_or([0.0; 3], |g| v3(g.vector(0xBD7C_27D4)))
            } else {
                own
            };
            drops.push(DropView {
                key,
                peak,
                drop_type,
                custom_status: r.int(0xB29D_587A).unwrap_or(0),
                event_type: r.int(0x146B_5937).unwrap_or(-1),
                position,
                title: ge.as_ref().and_then(|g| g.text(0x1A32_9A87)).unwrap_or_default(),
                selectable: ge.as_ref().and_then(|g| g.int(fnv1(b"Selectable"))) == Some(1),
            });
        }
        let capacity = schema_class(big_root, CLASS_UI_DROP)?.map_or(0, |c| c.unk_collection_capacity);
        let ui_drop_order = focus::class_order(&v.keys_of(CLASS_UI_DROP), capacity);
        let mut out = GlobeData { globe, ranges, peaks, drops, ui_drop_order, ..Default::default() };
        out.load_ge(big_root)?;
        let mut names: Vec<&str> = vec![out.globe.postfx.as_str()];
        names.extend(out.ranges.iter().flat_map(|r| [r.postfx.as_str(), r.postfx_dd.as_str()]));
        out.postfx = look::FePostFx::load(big_root, &names)?;
        Ok(out)
    }

    fn load_ge(&mut self, big_root: &Path) -> Result<()> {
        let mut presets: Vec<u32> = vec![self.globe.lighting, self.globe.lighting_dd];
        presets.extend(self.ranges.iter().flat_map(|r| [r.lighting, r.lighting_dd]));
        let objects: Vec<(Option<u32>, u32)> = std::iter::once((None, self.globe.object))
            .chain(self.ranges.iter().map(|r| (Some(r.key), r.midground)))
            .collect();
        let (meshes, lighting) = with_attribdb(big_root, |db| {
            let mut meshes = Vec::new();
            for (range, object) in &objects {
                if *object != 0 {
                    let m = object_meshes(db, *object).with_context(|| format!("object {object:#x}"))?;
                    let p = object_particles(db, big_root, *object).with_context(|| format!("object {object:#x}"))?;
                    meshes.push((*range, m, p));
                }
            }
            let mut lighting = BTreeMap::new();
            for key in presets.iter().copied().filter(|&k| k != 0) {
                if let std::collections::btree_map::Entry::Vacant(e) = lighting.entry(key) {
                    e.insert(preset(db, key)?);
                }
            }
            Ok((meshes, lighting))
        })?;
        let content = crate::content::GameContent::of(big_root);
        let lighting = lighting.into_iter().map(|(k, p)| (k, content.route_lighting(k, p))).collect();
        for (range, m, p) in meshes {
            match range {
                None => {
                    self.globe_meshes = m;
                    self.globe_particles = p;
                }
                Some(r) => {
                    self.midground_meshes.insert(r, m);
                    self.midground_particles.insert(r, p);
                }
            }
        }
        self.lighting = lighting;
        Ok(())
    }

    pub fn range(&self, key: u32) -> Option<&RangeView> {
        self.ranges.iter().find(|r| r.key == key)
    }

    pub fn globe_mesh(&self, technique: &str) -> Option<&ObjectMesh> {
        self.globe_meshes.iter().find(|m| m.material.technique.as_deref() == Some(technique))
    }
}

fn schema_class(big_root: &Path, class: u32) -> Result<Option<ssx_formats::attrib::schema::Class>> {
    let dir = big_root.join("bootcache/data/xenon/attribdb");
    let read = |n: &str| std::fs::read(dir.join(n)).with_context(|| format!("reading {n}"));
    let (vlt, bin) = (read("attribdb_schema.vlt")?, read("attribdb_schema.bin")?);
    let schema = ssx_formats::attrib::Schema::from_vault(&ssx_formats::attrib::Vault::parse(&vlt, &bin)?)?;
    Ok(schema.class(class).cloned())
}

fn postfx_name(v: &Vaults, key: u32) -> String {
    if key == 0 { String::new() } else { v.name("postfx", key) }
}

fn object_meshes(db: &Db, object: u32) -> Result<Vec<ObjectMesh>> {
    let mut out = Vec::new();
    let Some(models) = db.field(CLASS_GAMEOBJECT, object, 0xB43A_3598)? else { return Ok(out) };
    let Some(model) = refs(&models.data).into_iter().find(|r| r.class == CLASS_MODEL && r.collection != 0) else {
        return Ok(out);
    };
    let Some(list) = db.field(CLASS_MODEL, model.collection, fnv1(b"Meshes"))? else { return Ok(out) };
    for m in refs(&list.data).into_iter().filter(|r| r.collection != 0) {
        let fields = db.fields(m.class, m.collection)?;
        let get = |k: u32| fields.iter().find(|f| f.key == k).and_then(|f| single(&f.data));
        let Some(Value::Ref(g)) = get(fnv1(b"Geometry")) else { continue };
        if g.class != CLASS_GEOMETRY || g.collection == 0 {
            continue;
        }
        let Some(geometry) = db.text_of(g.class, g.collection)? else { continue };
        let material = match get(fnv1(b"Material")) {
            Some(Value::Ref(r)) if r.class == CLASS_MATERIAL && r.collection != 0 => {
                db.material(r.class, r.collection)?
            }
            _ => Material::default(),
        };
        out.push(ObjectMesh { geometry, material });
    }
    Ok(out)
}

fn object_particles(db: &Db, big_root: &Path, object: u32) -> Result<Vec<ParticleFx>> {
    let mut out = Vec::new();
    let Some(models) = db.field(CLASS_GAMEOBJECT, object, 0xB43A_3598)? else { return Ok(out) };
    let Some(model) = refs(&models.data).into_iter().find(|r| r.class == CLASS_MODEL && r.collection != 0) else {
        return Ok(out);
    };
    let Some(list) = db.field(CLASS_MODEL, model.collection, fnv1(b"ParticleNodes"))? else { return Ok(out) };
    for r in refs(&list.data).into_iter().filter(|r| r.collection != 0) {
        let node = crate::particles::particle_node(db, r.collection)?;
        let Some((sys, _)) = crate::particles::load_node_files_optional(big_root, &node)? else { continue };
        let vfx = node.lynx.as_ref().expect("a loaded node has its effect");
        let effect = sys.effect(&vfx.definition).cloned().with_context(|| format!("no effect {}", vfx.definition))?;
        out.push(ParticleFx { node, effect });
    }
    Ok(out)
}

pub fn lighting_presets(big_root: &Path, track: &str) -> Result<std::collections::BTreeMap<u32, LightingPreset>> {
    let presets = disc_lighting_presets(big_root, track)?;
    let content = crate::content::GameContent::of(big_root);
    Ok(presets.into_iter().map(|(k, p)| (k, content.route_lighting(k, p))).collect())
}

fn disc_lighting_presets(big_root: &Path, track: &str) -> Result<std::collections::BTreeMap<u32, LightingPreset>> {
    with_track_db(big_root, track, |db| {
        let mut out = std::collections::BTreeMap::new();
        for (key, _) in db.collections_of_all(CLASS_LIGHTING) {
            if let std::collections::btree_map::Entry::Vacant(e) = out.entry(key) {
                e.insert(preset(db, key)?);
            }
        }
        Ok(out)
    })
}

fn preset(db: &Db, key: u32) -> Result<LightingPreset> {
    let mut p = LightingPreset { name: db.collection_name(key), ..Default::default() };
    for f in db.fields(CLASS_LIGHTING, key)? {
        let name = match RECOVERED_FIELD_NAMES.iter().find(|(k, _)| *k == f.key) {
            Some((_, n)) => (*n).to_string(),
            None => db.field_name(f.key),
        };
        match single(&f.data) {
            Some(Value::Float(x)) => {
                p.floats.insert(name, *x);
            }
            Some(Value::Vector4(v)) => {
                p.vectors.insert(name, *v);
            }
            Some(Value::Ref(r)) if r.class == CLASS_TEXTURE && r.collection != 0 => {
                if let Some(path) = db.text_of(r.class, r.collection)? {
                    p.textures.insert(name, path);
                }
            }
            _ => {}
        }
    }
    Ok(p)
}
