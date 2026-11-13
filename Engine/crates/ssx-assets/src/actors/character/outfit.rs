use super::*;

pub(super) struct MeshRef {
    pub(super) slot: String,
    pub(super) mesh_name: String,
    pub(super) geom: String,
    pub(super) crsf: String,
    pub(super) skeleton: String,
    pub(super) material: Material,
}

pub(super) struct Outfit {
    pub(super) chain: String,
    pub(super) meshes: Vec<MeshRef>,
    pub(super) left_out: Vec<String>,
    pub(super) snowboard: Option<String>,
    pub(super) light_nodes: Vec<GearLightNode>,
}

pub(super) const SLOT_ORDER: [&str; 16] = [
    "head",
    "hair",
    "eyes",
    "headgear",
    "jackets",
    "pants",
    "gloves",
    "boots",
    "goggles",
    "headlamp",
    "protection",
    "thermal",
    "oxygentank",
    "iceaxe",
    "wingsuit",
    "snowboard",
];

pub(super) const BOARDER_FIELDS: [&str; 16] = [
    "Boots",
    "Character",
    "Common",
    "Gloves",
    "Goggles",
    "Head",
    "HeadLamp",
    "IceAxe",
    "Jackets",
    "OxygenTank",
    "Pants",
    "Protection",
    "Snowboard",
    "Thermal",
    "Tricky",
    "Wingsuit",
];

pub(super) const HEAD_SLOTS: [(&str, &str); 4] =
    [("Head", "head"), ("Hair", "hair"), ("Eyes", "eyes"), ("HeadGear", "headgear")];

pub(super) fn boarder_slot_name(key: u32) -> String {
    BOARDER_FIELDS.iter().find(|f| fnv1(f) == key).map_or_else(|| format!("{key:#010x}"), |f| f.to_lowercase())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SlotAfterLoadout {
    Default,
    Equipped(u32, u32),
    Cleared,
}

pub(super) fn slot_after_loadout(g: &GearSlot, equipped: Option<Option<(u32, u32)>>) -> SlotAfterLoadout {
    match equipped {
        Some(Some((c, k))) => SlotAfterLoadout::Equipped(c, k),
        Some(None) => SlotAfterLoadout::Default,
        None if g.keep_default => SlotAfterLoadout::Default,
        None => SlotAfterLoadout::Cleared,
    }
}

#[derive(Default)]
struct Reach {
    meshes: Vec<u32>,
    other_items: Vec<String>,
    light_nodes: Vec<u32>,
    particle_nodes: usize,
}

fn walk(db: &Db, class: u32, key: u32, seen: &mut HashSet<(u32, u32)>, reach: &mut Reach) -> Result<()> {
    if key == 0 || db.find(class, key).is_none() || !seen.insert((class, key)) {
        return Ok(());
    }
    match class {
        CLASS_MESH => reach.meshes.push(key),
        CLASS_MESHPOLYGON | CLASS_MODEL => {
            reach.other_items.push(format!("{}/{}", db.class_name(class), db.collection_name(key)))
        }
        CLASS_LIGHT_NODE => reach.light_nodes.push(key),
        CLASS_PARTICLE_NODE => reach.particle_nodes += 1,
        _ => {}
    }
    for f in db.fields(class, key)? {
        for r in refs(&f.data) {
            walk(db, r.class, r.collection, seen, reach)?;
        }
    }
    Ok(())
}

fn reach_of(db: &Db, class: u32, key: u32) -> Result<Reach> {
    let mut reach = Reach::default();
    walk(db, class, key, &mut HashSet::new(), &mut reach)?;
    Ok(reach)
}

fn field_ref(db: &Db, class: u32, coll: u32, key: u32) -> Result<Option<(u32, u32)>> {
    Ok(db.field(class, coll, key)?.and_then(|f| match single(&f.data) {
        Some(Value::Ref(r)) if r.collection != 0 => Some((r.class, r.collection)),
        _ => None,
    }))
}

fn field_text(db: &Db, class: u32, coll: u32, key: u32) -> Result<Option<String>> {
    Ok(db.field(class, coll, key)?.and_then(|f| single_text(&f.data)))
}

pub(super) fn mesh_ref(db: &Db, slot: &str, key: u32) -> Result<MeshRef> {
    let mesh_name = db.collection_name(key);
    let what = |f: &str| format!("ge_mesh/{mesh_name} has no {f}");
    let (gc, gk) = field_ref(db, CLASS_MESH, key, fnv1("Geometry"))?.with_context(|| what("Geometry"))?;
    let (mc, mk) = field_ref(db, CLASS_MESH, key, fnv1("Material"))?.with_context(|| what("Material"))?;
    Ok(MeshRef {
        slot: slot.to_string(),
        geom: db.text_of(gc, gk)?.with_context(|| what("geometry path"))?,
        crsf: field_text(db, CLASS_MESH, key, FIELD_FILE_REF)?.with_context(|| what("FileRef"))?,
        skeleton: field_text(db, CLASS_MESH, key, fnv1("SkeletonFileRef"))?.with_context(|| what("SkeletonFileRef"))?,
        material: db.material(mc, mk)?,
        mesh_name,
    })
}

pub(super) fn collect(db: &Db, slot: &str, class: u32, key: u32, out: &mut Outfit) -> Result<()> {
    let reach = reach_of(db, class, key)?;
    let nodes = match (reach.light_nodes.len(), reach.particle_nodes) {
        (0, 0) => String::new(),
        (l, p) => format!("; {l} light-node (loaded) and {p} particle-node collections (not drawn here)"),
    };
    for &k in &reach.light_nodes {
        let node = crate::track_lights::light_node(db, k)?;
        out.light_nodes.push(GearLightNode { slot: slot.to_string(), node });
    }
    if reach.meshes.is_empty() {
        out.left_out.push(format!("{slot} (no mesh{nodes})"));
    } else if !nodes.is_empty() {
        out.left_out.push(format!("{slot} nodes{nodes}"));
    }
    for item in reach.other_items {
        out.left_out.push(format!("{slot}: {item} (render item kind not loaded here)"));
    }
    for m in reach.meshes {
        if class == CLASS_SNOWBOARD {
            out.snowboard = Some(db.collection_name(m));
            out.meshes.push(mesh_ref(db, "snowboard", m)?);
        } else {
            out.meshes.push(mesh_ref(db, slot, m)?);
        }
    }
    Ok(())
}

pub(super) fn outfit(db: &Db, name: &str, equipped: &EquippedRefs) -> Result<Outfit> {
    let key = fnv1(name);
    let (boarder, chain) = if db.find(CLASS_CHARACTER, key).is_some() {
        let (class, boarder) = field_ref(db, CLASS_CHARACTER, key, FIELD_BOARDER)?
            .with_context(|| format!("character/{name} has no ge_characterboarder reference"))?;
        if class != CLASS_BOARDER {
            bail!(
                "character/{name} field {FIELD_BOARDER:#010x} references {}, not ge_characterboarder",
                db.class_name(class)
            );
        }
        (boarder, format!("vault: character/{name} -> ge_characterboarder/{}", db.collection_name(boarder)))
    } else if db.find(CLASS_BOARDER, key).is_some() {
        (key, format!("vault: ge_characterboarder/{name} (character/{name} is in attribdb.vlt)"))
    } else {
        let mut boarders: Vec<String> = db
            .collections_of(db.vaults.len().saturating_sub(2), CLASS_BOARDER)
            .map(|(k, _)| db.collection_name(k))
            .collect();
        boarders.sort();
        bail!("no character/{name} or ge_characterboarder/{name} in the vaults (boarders: {})", boarders.join(", "));
    };
    let mut out = Outfit { chain, meshes: Vec::new(), left_out: Vec::new(), snowboard: None, light_nodes: Vec::new() };
    for field in db.fields(CLASS_BOARDER, boarder)? {
        let Some(Value::Ref(r)) = single(&field.data) else { continue };
        let slot = boarder_slot_name(field.key);
        let gear = GEAR_SLOTS.iter().find(|g| fnv1(g.field) == field.key);
        let (class, coll) = match gear.map(|g| (g, slot_after_loadout(g, equipped.get(&g.item).copied()))) {
            Some((g, SlotAfterLoadout::Cleared)) => {
                let meshes: Vec<String> =
                    reach_of(db, r.class, r.collection)?.meshes.iter().map(|&m| db.collection_name(m)).collect();
                let default = if meshes.is_empty() { "empty".to_string() } else { meshes.join(", ") };
                out.left_out.push(format!("{slot} ({default}): cleared, no {:?} item", g.item));
                continue;
            }
            Some((_, SlotAfterLoadout::Equipped(c, k))) => (c, k),
            _ => (r.class, r.collection),
        };
        if coll == 0 {
            out.left_out.push(format!("{slot} (empty)"));
        } else if class == CLASS_HEAD {
            for (f, part) in HEAD_SLOTS {
                if let Some((c, k)) = field_ref(db, CLASS_HEAD, coll, fnv1(f))? {
                    collect(db, part, c, k, &mut out)?;
                }
            }
        } else {
            collect(db, &slot, class, coll, &mut out)?;
        }
    }
    Ok(out)
}

pub(crate) type EquippedRefs = HashMap<GearItem, Option<(u32, u32)>>;

pub(crate) fn equipment_refs(big_root: &Path, name: &str, loadout: &Loadout) -> Result<(EquippedRefs, bool)> {
    let mut out: EquippedRefs = HashMap::new();
    let mut lamp_accessory = false;
    if loadout.items.is_empty() && loadout.descriptions.is_empty() {
        return Ok((out, lamp_accessory));
    }
    crate::vault_db::with_all_db(big_root, |db| {
        for &(item, _) in &loadout.descriptions {
            out.insert(item, None);
        }
        for (key, _) in db.collections_of_all(CLASS_EQUIPMENT) {
            let fields = db.fields(CLASS_EQUIPMENT, key)?;
            let value = |k: u32| fields.iter().find(|f| f.key == k).and_then(|f| single(&f.data));
            let reference = |k: u32| match value(k) {
                Some(Value::Ref(r)) if r.collection != 0 => Some((k, (r.class, r.collection))),
                _ => None,
            };
            let target = || {
                [FIELD_EQ_ACCESSORY, FIELD_EQ_SNOWBOARD, FIELD_EQ_JACKET, FIELD_EQ_PANTS]
                    .into_iter()
                    .find_map(reference)
            };
            for &(item, _) in loadout.descriptions.iter().filter(|(_, want)| *want == key) {
                let t = target();
                if item == GearItem::HeadLamp {
                    lamp_accessory = t.is_some_and(|(k, _)| k == FIELD_EQ_ACCESSORY);
                }
                out.insert(item, t.map(|(_, r)| r));
            }
            if loadout.items.is_empty() || reference(FIELD_EQ_CHARACTER).map(|(_, (_, k))| k) != Some(fnv1(name)) {
                continue;
            }
            let (Some(Value::UInt32(id)), Some(Value::Raw { bytes, .. })) = (value(FIELD_EQ_ID), value(FIELD_EQ_TYPE))
            else {
                continue;
            };
            let Ok(ty) = <[u8; 4]>::try_from(bytes.as_slice()).map(u32::from_be_bytes) else { continue };
            for &(item, want) in loadout.items.iter().filter(|(item, want)| *item as u32 == ty && want == id) {
                let (_, r) = target().with_context(|| format!("{name}'s {item:?} item {want} references nothing"))?;
                out.insert(item, Some(r));
            }
        }
        Ok(())
    })?;
    if let Some((item, id)) = loadout.items.iter().find(|(item, _)| !out.contains_key(item)) {
        bail!("no equipment_description for {name}: {item:?} id {id}");
    }
    Ok((out, lamp_accessory))
}
