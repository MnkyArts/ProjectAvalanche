mod gear;
mod outfit;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use ssx_formats::ant_anim::euler_xyz_to_quat;
use ssx_formats::attrib::Value;
use ssx_formats::crsf::{CrsfFile, Mat4, SceneGraph};

use crate::object_mesh::ObjectMesh;
use crate::track_objects::asset_file;
use crate::vault_db::{Db, Material, fnv1, refs, single, single_text, with_track_db};

pub use gear::{SlotParts, equipped_parts, mesh_part};
pub(crate) use outfit::*;

pub const DEFAULT_CHARACTER: &str = "alex";

const CLASS_CHARACTER: u32 = fnv1("character");
const CLASS_BOARDER: u32 = fnv1("ge_characterboarder");
const CLASS_HEAD: u32 = fnv1("ge_characterhead");
const CLASS_SNOWBOARD: u32 = fnv1("ge_snowboard");
const CLASS_MESH: u32 = fnv1("ge_mesh");
const CLASS_LIGHT_NODE: u32 = fnv1("ge_lightnode");
const CLASS_PARTICLE_NODE: u32 = fnv1("ge_particlenode");
const CLASS_MESHPOLYGON: u32 = fnv1("ge_scene_meshpolygon");
const CLASS_MODEL: u32 = fnv1("ge_model");
const CLASS_EQUIPMENT: u32 = fnv1("equipment_description");
const FIELD_BOARDER: u32 = 0x9F33_AE3E;
const FIELD_FILE_REF: u32 = 0xDF9E_F8B2;
const FIELD_EQ_CHARACTER: u32 = fnv1("character");
const FIELD_EQ_ID: u32 = fnv1("id");
const FIELD_EQ_TYPE: u32 = 0x1779_0AC8;
const FIELD_EQ_ACCESSORY: u32 = fnv1("character_accessory");
const FIELD_EQ_SNOWBOARD: u32 = fnv1("snowboard");
const FIELD_EQ_JACKET: u32 = fnv1("character_jacket");
const FIELD_EQ_PANTS: u32 = fnv1("character_pants");

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum GearItem {
    Board = 0,
    Wingsuit = 1,
    IceAxe = 2,
    Armour = 3,
    Thermal = 4,
    Oxygen = 5,
    HeadLamp = 6,
    PulseGoggles = 7,
    Jacket = 12,
    Pants = 13,
}

impl GearItem {
    pub fn from_slot(slot: usize) -> Option<GearItem> {
        [
            GearItem::Board,
            GearItem::Wingsuit,
            GearItem::IceAxe,
            GearItem::Armour,
            GearItem::Thermal,
            GearItem::Oxygen,
            GearItem::HeadLamp,
            GearItem::PulseGoggles,
        ]
        .get(slot)
        .copied()
    }
}

#[derive(Clone, Copy, Debug)]
pub struct GearSlot {
    pub item: GearItem,
    pub field: &'static str,
    pub keep_default: bool,
}

pub const GEAR_SLOTS: [GearSlot; 10] = [
    GearSlot { item: GearItem::Board, field: "Snowboard", keep_default: false },
    GearSlot { item: GearItem::HeadLamp, field: "HeadLamp", keep_default: true },
    GearSlot { item: GearItem::IceAxe, field: "IceAxe", keep_default: false },
    GearSlot { item: GearItem::Oxygen, field: "OxygenTank", keep_default: false },
    GearSlot { item: GearItem::Wingsuit, field: "Wingsuit", keep_default: false },
    GearSlot { item: GearItem::Armour, field: "Protection", keep_default: true },
    GearSlot { item: GearItem::Thermal, field: "Thermal", keep_default: true },
    GearSlot { item: GearItem::PulseGoggles, field: "Goggles", keep_default: true },
    GearSlot { item: GearItem::Jacket, field: "Jackets", keep_default: true },
    GearSlot { item: GearItem::Pants, field: "Pants", keep_default: true },
];

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Loadout {
    pub items: Vec<(GearItem, u32)>,
    pub descriptions: Vec<(GearItem, u32)>,
    pub head_lamp_stats: [Option<u8>; 2],
}

pub fn head_lamp_scale(stats: [Option<u8>; 2]) -> (f32, f32) {
    let f = |s: Option<u8>| {
        let t = s.map_or(0.0, |v| (f64::from(v) * f64::from(0.01f32)) as f32);
        t.mul_add(t, 1.0) * 0.5
    };
    (f(stats[0]), f(stats[1]))
}

impl Loadout {
    pub fn from_setup(entries: &[(usize, u32)], head_lamp_stats: [Option<u8>; 2]) -> Loadout {
        let descriptions = entries
            .iter()
            .filter_map(|&(t, d)| GEAR_SLOTS.iter().find(|g| g.item as usize == t).map(|g| (g.item, d)))
            .collect();
        Loadout { items: Vec::new(), descriptions, head_lamp_stats }
    }

    pub fn from_slots(slots: &[bool], id: u32) -> Loadout {
        let items =
            (1..slots.len()).filter(|&s| slots[s]).filter_map(GearItem::from_slot).map(|item| (item, id)).collect();
        Loadout { items, ..Loadout::default() }
    }
}

#[derive(Clone, Debug)]
pub struct GearLightNode {
    pub slot: String,
    pub node: crate::track_lights::LightNode,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Srt {
    pub scale: [f32; 3],
    pub rotation: [f32; 4],
    pub translation: [f32; 3],
}

#[derive(Clone, Debug)]
pub struct PaletteJoint {
    pub name: String,
    pub node: usize,
    pub inverse_bind: Mat4,
}

#[derive(Clone, Debug, Default)]
pub struct Skin {
    pub joints: Vec<[u16; 4]>,
    pub weights: Vec<[f32; 4]>,
    pub palette: Vec<PaletteJoint>,
}

#[derive(Clone, Debug)]
pub enum PartMaterial {
    Disc(Material),
    Def(Box<ssx_content::material::MaterialDef>),
}

impl PartMaterial {
    pub fn effect(&self) -> Option<&str> {
        match self {
            PartMaterial::Disc(m) => m.effect.as_deref(),
            PartMaterial::Def(d) => Some(d.effect.as_str()),
        }
    }

    pub fn name(&self) -> &str {
        match self {
            PartMaterial::Disc(m) => &m.name,
            PartMaterial::Def(d) => &d.name,
        }
    }
}

#[derive(Clone, Debug)]
pub struct CharacterPart {
    pub slot: String,
    pub mesh_name: String,
    pub source: String,
    pub mesh: ObjectMesh,
    pub skin: Skin,
    pub material: PartMaterial,
    pub face_offsets: bool,
    pub bind_error: f32,
}

#[derive(Clone, Debug)]
pub struct Character {
    pub name: String,
    pub chain: String,
    pub parts: Vec<CharacterPart>,
    pub left_out: Vec<String>,
    pub rig_file: PathBuf,
    pub rig: SceneGraph,
    pub face_offsets: HashMap<String, [f32; 3]>,
    pub snowboard_mesh: Option<String>,
    pub light_nodes: Vec<GearLightNode>,
    pub problems: Vec<String>,
}

impl Srt {
    pub fn matrix(&self) -> Mat4 {
        let [x, y, z, w] = ssx_formats::ant_anim::normalize(self.rotation);
        let r = [
            [1.0 - 2.0 * (y * y + z * z), 2.0 * (x * y + z * w), 2.0 * (x * z - y * w)],
            [2.0 * (x * y - z * w), 1.0 - 2.0 * (x * x + z * z), 2.0 * (y * z + x * w)],
            [2.0 * (x * z + y * w), 2.0 * (y * z - x * w), 1.0 - 2.0 * (x * x + y * y)],
        ];
        let mut m = [[0.0; 4]; 4];
        for i in 0..3 {
            for j in 0..3 {
                m[i][j] = self.scale[i] * r[i][j];
            }
        }
        let [tx, ty, tz] = self.translation;
        m[3] = [tx, ty, tz, 1.0];
        m
    }
}

pub fn mat_mul(a: &Mat4, b: &Mat4) -> Mat4 {
    std::array::from_fn(|i| std::array::from_fn(|j| (0..4).map(|k| a[i][k] * b[k][j]).sum()))
}

pub fn transform_point(p: [f32; 3], m: &Mat4) -> [f32; 3] {
    std::array::from_fn(|j| p[0] * m[0][j] + p[1] * m[1][j] + p[2] * m[2][j] + m[3][j])
}

pub fn model_matrices(rig: &SceneGraph, pose: &[Srt]) -> Vec<Mat4> {
    let mut out: Vec<Mat4> = Vec::with_capacity(rig.nodes.len());
    for (node, srt) in rig.nodes.iter().zip(pose) {
        let local = srt.matrix();
        out.push(node.parent.map_or(local, |p| mat_mul(&local, &out[p])));
    }
    out
}

pub fn model_matrix(rig: &SceneGraph, pose: &[Srt], node: usize) -> Mat4 {
    let mut m = pose[node].matrix();
    let mut parent = rig.nodes[node].parent;
    while let Some(p) = parent {
        m = mat_mul(&m, &pose[p].matrix());
        parent = rig.nodes[p].parent;
    }
    m
}

impl Character {
    pub fn bind_pose(&self) -> Vec<Srt> {
        bind_pose(&self.rig, &self.face_offsets)
    }

    pub fn summary(&self) -> String {
        let slots: Vec<&str> = self.parts.iter().map(|p| p.slot.as_str()).collect();
        format!(
            "{} ({}): {} parts [{}], {} joints, {} face offsets",
            self.name,
            self.chain,
            self.parts.len(),
            slots.join(" "),
            self.rig.joints().count(),
            self.face_offsets.len()
        )
    }
}

pub fn bind_pose_of(rig: &SceneGraph, face_offsets: &HashMap<String, [f32; 3]>) -> Vec<Srt> {
    bind_pose(rig, face_offsets)
}

pub fn outfit_crsf_files(big_root: &Path, name: &str) -> Result<Vec<String>> {
    crate::vault_db::with_attribdb(big_root, |db| {
        Ok(outfit(db, name, &EquippedRefs::new())?.meshes.into_iter().map(|m| m.crsf).collect())
    })
}

fn bind_pose(rig: &SceneGraph, face_offsets: &HashMap<String, [f32; 3]>) -> Vec<Srt> {
    rig.nodes
        .iter()
        .map(|n| {
            let mut translation = n.attribute("translate").unwrap_or([0.0; 3]);
            if let Some(o) = n.name.strip_prefix("Offset_").and_then(|j| face_offsets.get(j)) {
                translation = *o;
            }
            Srt {
                scale: n.attribute("scale").unwrap_or([1.0; 3]),
                rotation: euler_xyz_to_quat(n.attribute("rotate").unwrap_or([0.0; 3])),
                translation,
            }
        })
        .collect()
}

pub fn load_character(big_root: &Path, track: &str, name: &str) -> Result<Character> {
    load_character_with(big_root, track, name, &Loadout::default())
}

pub fn load_character_with(big_root: &Path, track: &str, name: &str, loadout: &Loadout) -> Result<Character> {
    let (equipped, lamp_accessory) = equipment_refs(big_root, name, loadout)?;
    let outfit = with_track_db(big_root, track, |db| outfit(db, name, &equipped))?;
    dressed(big_root, name, loadout, outfit, lamp_accessory)
}

pub fn load_character_untracked(big_root: &Path, name: &str, loadout: &Loadout) -> Result<Character> {
    let (equipped, lamp_accessory) = equipment_refs(big_root, name, loadout)?;
    let outfit = crate::vault_db::with_attribdb(big_root, |db| outfit(db, name, &equipped))?;
    dressed(big_root, name, loadout, outfit, lamp_accessory)
}

fn dressed(
    big_root: &Path,
    name: &str,
    loadout: &Loadout,
    mut outfit: Outfit,
    lamp_accessory: bool,
) -> Result<Character> {
    if lamp_accessory {
        gear::scale_head_lamp(&mut outfit.light_nodes, loadout.head_lamp_stats);
    }
    let mut items: Vec<String> = loadout.items.iter().map(|(item, id)| format!("{item:?} {id}")).collect();
    items.extend(loadout.descriptions.iter().map(|(item, key)| format!("{item:?} {key:#010x}")));
    outfit.chain +=
        &format!("; loadout: {}", if items.is_empty() { "nothing equipped".into() } else { items.join(", ") });
    outfit.meshes.sort_by_key(|m| SLOT_ORDER.iter().position(|s| *s == m.slot).unwrap_or(SLOT_ORDER.len()));

    let skeleton = outfit.meshes.first().map(|m| m.skeleton.clone()).context("the outfit has no meshes")?;
    let mut problems: Vec<String> = outfit
        .meshes
        .iter()
        .filter(|m| m.skeleton != skeleton)
        .map(|m| {
            format!(
                "{}: rig {} differs from {skeleton}; drawn on the latter (the boarder rigs are identical, crsf.md §4)",
                m.mesh_name, m.skeleton
            )
        })
        .collect();
    let rig_file = asset_file(big_root, &skeleton).with_context(|| format!("rig {skeleton} not found"))?;
    let rig = CrsfFile::parse(&std::fs::read(&rig_file).with_context(|| format!("reading {}", rig_file.display()))?)
        .with_context(|| format!("parsing {}", rig_file.display()))?
        .scene
        .with_context(|| format!("{} has no SGRF scene graph", rig_file.display()))?;

    let mut loaded = Vec::new();
    for m in outfit.meshes {
        match load_mesh_files(big_root, &m) {
            Ok((geom, crsf, mesh, file)) => loaded.push((m, geom, crsf, mesh, file)),
            Err(e) => problems.push(format!("{} ({}): {e:#}", m.mesh_name, m.slot)),
        }
    }
    let parts = disc_parts(&rig, loaded, &mut problems)?;
    let face_offsets = parts.1;
    let parts = parts.0;
    Ok(Character {
        name: name.to_string(),
        chain: outfit.chain,
        parts,
        left_out: outfit.left_out,
        rig_file,
        rig,
        face_offsets,
        snowboard_mesh: outfit.snowboard,
        light_nodes: outfit.light_nodes,
        problems,
    })
}

type LoadedMesh = (MeshRef, PathBuf, PathBuf, ObjectMesh, CrsfFile);

type FaceOffsets = HashMap<String, [f32; 3]>;

fn disc_parts(
    rig: &SceneGraph,
    loaded: Vec<LoadedMesh>,
    problems: &mut Vec<String>,
) -> Result<(Vec<CharacterPart>, FaceOffsets)> {
    let mut face_offsets = HashMap::new();
    for (_, _, _, _, file) in &loaded {
        for o in file.face_offsets.iter().flatten() {
            face_offsets.insert(o.joint.clone(), o.translate);
        }
    }
    let plain = model_matrices(rig, &bind_pose(rig, &HashMap::new()));
    let offset = model_matrices(rig, &bind_pose(rig, &face_offsets));
    let mut parts = Vec::new();
    for (m, geom, crsf, mesh, file) in loaded {
        let has_fpof = file.face_offsets.is_some();
        let skin = match skin_of(&file, rig, mesh.positions.len()) {
            Ok(s) => s,
            Err(e) => {
                problems.push(format!("{} ({}): {e:#}", m.mesh_name, m.slot));
                continue;
            }
        };
        let bind_error = bind_error(&mesh, &skin, if has_fpof { &offset } else { &plain });
        parts.push(CharacterPart {
            slot: m.slot,
            mesh_name: m.mesh_name,
            source: format!("{} + {}", geom.display(), crsf.display()),
            mesh,
            skin,
            material: PartMaterial::Disc(m.material),
            face_offsets: has_fpof,
            bind_error,
        });
    }
    Ok((parts, face_offsets))
}

fn load_mesh_files(big_root: &Path, m: &MeshRef) -> Result<(PathBuf, PathBuf, ObjectMesh, CrsfFile)> {
    let geom = asset_file(big_root, &m.geom).with_context(|| format!("{} not found", m.geom))?;
    let crsf = asset_file(big_root, &m.crsf).with_context(|| format!("{} not found", m.crsf))?;
    let mesh = crate::content::models::rider_mesh(big_root, &m.geom)?;
    let file = CrsfFile::parse(&std::fs::read(&crsf).with_context(|| format!("reading {}", crsf.display()))?)
        .with_context(|| format!("parsing {}", crsf.display()))?;
    Ok((geom, crsf, mesh, file))
}

fn skin_of(file: &CrsfFile, rig: &SceneGraph, vertex_count: usize) -> Result<Skin> {
    let w = file.weights.as_ref().context("no WEIT chunk")?;
    let p = file.palette.as_ref().context("no SKTN chunk")?;
    if w.vertex_count != vertex_count {
        bail!("WEIT has {} vertices, the .geom {vertex_count}", w.vertex_count);
    }
    if w.influences > 4 {
        bail!("{} influences per vertex (4 at most on the disc)", w.influences);
    }
    let palette = p
        .bones
        .iter()
        .map(|b| {
            let node = rig.find(&b.name).with_context(|| format!("palette joint {} is not in the rig", b.name))?;
            Ok(PaletteJoint { name: b.name.clone(), node, inverse_bind: b.inverse_bind })
        })
        .collect::<Result<Vec<_>>>()?;
    let mut skin = Skin { palette, ..Skin::default() };
    for v in 0..w.vertex_count {
        let (ws, bs) = w.vertex(v);
        let (mut joints, mut weights) = ([0u16; 4], [0.0f32; 4]);
        for (k, (&x, &b)) in ws.iter().zip(bs).enumerate().filter(|(_, (x, _))| **x != 0.0) {
            if usize::from(b) >= skin.palette.len() {
                bail!("vertex {v}: palette index {b} of {}", skin.palette.len());
            }
            (joints[k], weights[k]) = (u16::from(b), x);
        }
        skin.joints.push(joints);
        skin.weights.push(weights);
    }
    Ok(skin)
}

pub fn bind_error(mesh: &ObjectMesh, skin: &Skin, model: &[Mat4]) -> f32 {
    let matrices: Vec<Mat4> = skin.palette.iter().map(|j| mat_mul(&j.inverse_bind, &model[j.node])).collect();
    let mut worst = 0.0f32;
    for ((p, joints), weights) in mesh.positions.iter().zip(&skin.joints).zip(&skin.weights) {
        let mut q = [0.0f32; 3];
        for (&j, &w) in joints.iter().zip(weights).filter(|(_, w)| **w != 0.0) {
            let t = transform_point(*p, &matrices[usize::from(j)]);
            (0..3).for_each(|k| q[k] += w * t[k]);
        }
        worst = worst.max((0..3).map(|k| (q[k] - p[k]).powi(2)).sum::<f32>().sqrt());
    }
    worst
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recovered_field_names_match_the_vault_keys() {
        let boarder = [
            ("Jackets", 0x36A2_8462),
            ("Gloves", 0x5D33_ED5B),
            ("Boots", 0x0C16_A800),
            ("OxygenTank", 0x8861_A581),
            ("Protection", 0x7B4E_B650),
            ("Common", 0x3486_94E2),
        ];
        for (name, key) in boarder {
            assert_eq!(fnv1(name), key, "{name}");
            assert_eq!(boarder_slot_name(key), name.to_lowercase());
        }
        let head = [("Hair", 0x401C_29FD), ("Eyes", 0x7C26_4343), ("HeadGear", 0x171E_679A)];
        for (name, key) in head {
            assert!(HEAD_SLOTS.iter().any(|(f, _)| *f == name), "{name}");
            assert_eq!(fnv1(name), key, "{name}");
        }
        assert_eq!(FIELD_EQ_ACCESSORY, 0x0468_9361);
        assert_eq!(FIELD_EQ_SNOWBOARD, 0x01E1_A9EC);
        assert_eq!(FIELD_EQ_JACKET, 0x2D1C_514F);
        assert_eq!(FIELD_EQ_PANTS, 0x24C8_EFB9);
        assert_eq!(fnv1("AccessoryType"), 0x4BFF_D83D);
        assert_eq!(fnv1("LightNodes"), 0x562C_4452);
        assert_eq!(fnv1("ParticleNodes"), 0xB87A_271A);
        assert_eq!(boarder_slot_name(0x1234_5678), "0x12345678");
        assert!(GEAR_SLOTS.iter().all(|g| BOARDER_FIELDS.contains(&g.field)));
    }

    #[test]
    fn gear_item_values() {
        let order = [
            (GearItem::Board, 0),
            (GearItem::Wingsuit, 1),
            (GearItem::IceAxe, 2),
            (GearItem::Armour, 3),
            (GearItem::Thermal, 4),
            (GearItem::Oxygen, 5),
            (GearItem::HeadLamp, 6),
            (GearItem::PulseGoggles, 7),
            (GearItem::Jacket, 12),
            (GearItem::Pants, 13),
        ];
        for (item, value) in order {
            assert_eq!(item as u32, value, "{item:?}");
        }
    }

    #[test]
    fn loadout_overrides() {
        let cleared: Vec<GearItem> = GEAR_SLOTS.iter().filter(|g| !g.keep_default).map(|g| g.item).collect();
        assert_eq!(cleared, [GearItem::Board, GearItem::IceAxe, GearItem::Oxygen, GearItem::Wingsuit]);
        for g in &GEAR_SLOTS {
            let none = slot_after_loadout(g, None);
            assert_eq!(none, if g.keep_default { SlotAfterLoadout::Default } else { SlotAfterLoadout::Cleared });
            assert_eq!(slot_after_loadout(g, Some(Some((7, 9)))), SlotAfterLoadout::Equipped(7, 9));
            assert_eq!(slot_after_loadout(g, Some(None)), SlotAfterLoadout::Default, "an item without a reference");
        }
        assert!(Loadout::default().items.is_empty());
    }

    #[test]
    fn loadout_from_slots() {
        let mut slots = [false; 15];
        (slots[0], slots[1], slots[5], slots[6], slots[12]) = (true, true, true, true, true);
        let l = Loadout::from_slots(&slots, 0);
        assert_eq!(l.items, [(GearItem::Wingsuit, 0), (GearItem::Oxygen, 0), (GearItem::HeadLamp, 0)]);
        assert_eq!(GearItem::from_slot(7), Some(GearItem::PulseGoggles));
        assert_eq!(GearItem::from_slot(8), None);
    }

    #[test]
    fn head_lamp_scale_and_setup_entries() {
        assert_eq!(head_lamp_scale([None, None]), (0.5, 0.5));
        assert_eq!(head_lamp_scale([Some(100), Some(0)]), (1.0, 0.5));
        let (c, d) = head_lamp_scale([Some(64), Some(75)]);
        assert!((c - 0.7048).abs() < 1e-6 && (d - 0.78125).abs() < 1e-6, "{c} {d}");
        let l = Loadout::from_setup(&[(0, 1), (6, 2), (8, 3), (12, 4), (13, 5), (14, 6)], [Some(64), None]);
        assert_eq!(
            l.descriptions,
            [(GearItem::Board, 1), (GearItem::HeadLamp, 2), (GearItem::Jacket, 4), (GearItem::Pants, 5)]
        );
        assert!(l.items.is_empty());
    }
}
