use std::path::Path;

use anyhow::{Context, Result, bail};
use ssx_content::ContentId;
use ssx_content::rider::OutfitSlot;
use ssx_formats::crsf::SceneGraph;

use super::LookSource;
use super::dress::{item_look, parts_of_lod};
use super::look::{self, LookKind, Skeletons};
use crate::character::{CharacterPart, Srt, bind_pose_of};
use crate::content::GameContent;

pub struct BotLook {
    pub lods: Vec<Vec<CharacterPart>>,
    pub rig: SceneGraph,
    pub bind: Vec<Srt>,
    pub notes: Vec<String>,
}

pub fn bot_look(big_root: &Path, id: &ContentId, at: &LookSource, lods: usize) -> Result<BotLook> {
    let content = GameContent::of(big_root);
    let def = content.character_def(id).map_err(anyhow::Error::from)?.value;
    let face = look::face_set(id, &def).with_context(|| format!("{id}: no disc rider to take the face offsets of"))?;
    let plain = content.riders.skeletons.get(big_root, None)?;
    let sk = content.riders.skeletons.get(big_root, Some(&face))?;
    let skeletons = Skeletons { face: Some(&sk.skeleton), plain: &plain.skeleton };
    let model = content.look_model(id, at)?;
    let built = look::build(&content, id, at, model, LookKind::Character, &def.materials, &skeletons)
        .map_err(|p| anyhow::anyhow!("{id}: {}", p.join("; ")))?;
    let have = built.lods();
    if have == 0 {
        bail!("{id}: the look has no LOD");
    }
    let mut notes = built.notes.clone();
    if have < lods {
        notes.push(format!("{id}: {have} LODs for the bot's {lods}; its LOD {} is drawn for the rest", have - 1));
    }
    let mut out = Vec::new();
    for k in 0..lods.max(1) {
        // PORT(choice): a package look's LOD k is drawn at the game object's LOD k thresholds (`LODThresholds` of the
        // character's game object, field 0x030BB788's model list), its last LOD for the rest; a record field of its own
        // (`character.toml`) would let the author set them; the game has no package LODs to compare with.
        let lod = k.min(have - 1);
        let mut parts = parts_of_lod(&built, lod, &sk.rig, &sk.bind);
        for (slot, o) in &def.outfit {
            parts.retain(|p| p.slot != *slot);
            let OutfitSlot::Gear(g) = o else { continue };
            match item_look(&content, g, &skeletons) {
                Ok(l) if l.lods() > 0 => {
                    let k = lod.min(l.lods() - 1);
                    parts.extend(parts_of_lod(&l, k, &sk.rig, &sk.bind).into_iter().filter(|p| p.slot == *slot));
                }
                Ok(_) => notes.push(format!("outfit {slot}: {g}: no LOD")),
                Err(e) => notes.push(format!("outfit {slot}: {g}: {e}")),
            }
        }
        out.push(parts);
    }
    let bind = bind_pose_of(&sk.base_rig, &sk.face_offsets);
    Ok(BotLook { lods: out, rig: sk.base_rig.clone(), bind, notes })
}
