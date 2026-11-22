use std::path::Path;

use anyhow::{Context, Result};
use ssx_formats::attrib::Value;

use super::{CLASS_PARTICLE_NODE, ParticleNode, particle_node};
use crate::character::{GEAR_SLOTS, Loadout};
use crate::vault_db::{Db, fnv1, refs, single};

const CLASS_CHARACTER: u32 = fnv1("character");
const CLASS_BOARDER: u32 = fnv1("ge_characterboarder");
const FIELD_BOARDER: u32 = 0x9F33_AE3E;

const BOARDER_FIELDS: [&str; 16] = [
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

#[derive(Clone, Debug)]
pub struct RiderNode {
    pub slot: String,
    pub node: ParticleNode,
}

pub fn rider_particle_nodes(
    big_root: &Path,
    track: Option<&str>,
    name: &str,
    loadout: &Loadout,
) -> Result<Vec<RiderNode>> {
    let equipped = crate::character::equipment_refs(big_root, name, loadout)?.refs;
    crate::vault_db::with_level_db(big_root, track, |db| {
        let boarder = boarder_of(db, name)?;
        let mut out = Vec::new();
        for field in db.fields(CLASS_BOARDER, boarder)? {
            let Some(Value::Ref(r)) = single(&field.data) else { continue };
            let slot = BOARDER_FIELDS
                .iter()
                .find(|f| fnv1(f) == field.key)
                .map_or_else(|| format!("{:#010x}", field.key), |f| f.to_lowercase());
            let target = match GEAR_SLOTS.iter().find(|g| fnv1(g.field) == field.key) {
                Some(g) => match equipped.get(&g.item) {
                    Some(&Some(r)) => Some(r),
                    Some(None) => Some((r.class, r.collection)),
                    None if g.keep_default => Some((r.class, r.collection)),
                    None => None,
                },
                None => Some((r.class, r.collection)),
            };
            if let Some((class, key)) = target {
                walk(db, class, key, &slot, &mut Vec::new(), &mut out)?;
            }
        }
        Ok(out)
    })
}

fn boarder_of(db: &Db, name: &str) -> Result<u32> {
    let key = fnv1(name);
    if db.find(CLASS_CHARACTER, key).is_some()
        && let Some(f) = db.field(CLASS_CHARACTER, key, FIELD_BOARDER)?
        && let Some(Value::Ref(r)) = single(&f.data)
        && r.class == CLASS_BOARDER
    {
        return Ok(r.collection);
    }
    db.find(CLASS_BOARDER, key).map(|_| key).with_context(|| format!("no ge_characterboarder for {name}"))
}

fn walk(db: &Db, class: u32, key: u32, slot: &str, path: &mut Vec<(u32, u32)>, out: &mut Vec<RiderNode>) -> Result<()> {
    if key == 0 || db.find(class, key).is_none() || path.contains(&(class, key)) {
        return Ok(());
    }
    if class == CLASS_PARTICLE_NODE {
        let node = particle_node(db, key)?;
        if node.lynx.is_some() {
            out.push(RiderNode { slot: slot.to_owned(), node });
        }
    }
    path.push((class, key));
    for f in db.fields(class, key)? {
        for r in refs(&f.data) {
            walk(db, r.class, r.collection, slot, path, out)?;
        }
    }
    path.pop();
    Ok(())
}
