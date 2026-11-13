use std::collections::BTreeMap;
use std::path::Path;

use anyhow::Result;
use ssx_formats::attrib::Value;

use crate::vault_db::{Db, fnv1, single, single_text, with_track_db};

const CLASS_DROP: u32 = fnv1("ge_drop");
const CLASS_LIGHTING: u32 = fnv1("ge_globallightingparams");
const CLASS_TEXTURE: u32 = fnv1("ge_texture");
const FIELD_TRACK: u32 = fnv1("Track");
const FIELD_SELECTABLE: u32 = fnv1("Selectable");
const FIELD_LIGHTING: u32 = 0xC0EC_ED82;
const FIELD_POSTFX: u32 = 0x5595_A25C;
const FIELD_PEAK: u32 = 0x17E8_41D0;
const FIELD_TEXT_ID: u32 = 0x1A32_9A87;
pub const FIELD_SUN_NODE: u32 = 0xE6AB_EB94;

pub const RECOVERED_FIELD_NAMES: [(u32, &str); 31] = [
    (0xC1E5_0D54, "dirLight_Offset"),
    (0x4167_465C, "rampBounceOffset"),
    (0x7FEC_FFE0, "fillOffsetPitchAngle"),
    (0xA20F_FD75, "fillOffsetYawAngle"),
    (0x3431_8863, "rimOffsetAngle"),
    (0x02FE_A98B, "heliInteriorDynamicExposureKeyValue"),
    (0xC184_A7AD, "heliInteriorFillIntensity"),
    (0x4EF3_63FE, "heliInteriorRimIntensity"),
    (0xE7A5_5D34, "isNightCondition"),
    (0xCE6C_6854, "headLampScaleFactor"),
    (0xC9A7_EA2A, "toneMappingShoulderStrength"),
    (0x4007_3461, "toneMappingLinearStrength"),
    (0x7140_B71B, "toneMappingLinearAngle"),
    (0x5395_CC48, "toneMappingToeStrength"),
    (0xDCAE_B954, "toneMappingToeNumerator"),
    (0x4BD0_BA19, "toneMappingToeDenominator"),
    (0xF332_EAC5, "toneMappingLinearWhite"),
    (0xFCEB_05E1, "atmosExtinctionMultiplier"),
    (0x876E_F823, "atmosInScatteringMultiplier"),
    (0xA846_6B7A, "atmosRayleighColour"),
    (0x612B_626F, "atmosRayleighMultiplier"),
    (0x6444_CB2C, "atmosMieColour"),
    (0xB28D_A48D, "atmosMieMultiplier"),
    (0x8118_17BB, "atmosRayleighHeightFalloff"),
    (0x18CA_0582, "atmosRayleighHeightOffset"),
    (0x719E_6865, "atmosMieHeightFalloff"),
    (0xC162_E208, "atmosMieHeightOffset"),
    (0xE557_6304, "atmosMieHeightFalloffSky"),
    (0xCAB2_D8F7, "atmosMieHeightOffsetSky"),
    (0x8A3B_21E5, "atmosPlanetRadius"),
    (0xCEAF_CAE0, "atmosHenyeyGreensteinParam"),
];

#[derive(Clone, Debug, Default)]
pub struct LightingPreset {
    pub name: String,
    pub floats: BTreeMap<String, f32>,
    pub vectors: BTreeMap<String, [f32; 4]>,
    pub textures: BTreeMap<String, String>,
    pub sun_node: Option<crate::particles::ParticleNode>,
    pub sky: Option<ssx_content::ContentId>,
}

impl LightingPreset {
    pub fn float(&self, name: &str) -> Option<f32> {
        self.floats.get(name).copied()
    }

    pub fn vector(&self, name: &str) -> Option<[f32; 4]> {
        self.vectors.get(name).copied()
    }
}

#[derive(Clone, Debug)]
pub struct Drop {
    pub key: u32,
    pub text_id: Option<String>,
    pub peak: Option<String>,
    pub postfx: Option<String>,
    pub selectable: bool,
    pub lighting: LightingPreset,
    pub lighting_key: Option<u32>,
}

pub fn load_track_drops(big_root: &Path, track: &str) -> Result<Vec<Drop>> {
    let mut drops = disc_track_drops(big_root, track)?;
    let content = crate::content::GameContent::of(big_root);
    for d in &mut drops {
        if let Some(key) = d.lighting_key {
            d.lighting = content.route_lighting(key, std::mem::take(&mut d.lighting));
        }
    }
    Ok(drops)
}

pub fn disc_track_drops(big_root: &Path, track: &str) -> Result<Vec<Drop>> {
    let track_key = fnv1(&format!("{track}_track"));
    with_track_db(big_root, track, |db| {
        let mut out = Vec::new();
        for (key, _) in db.collections_of_all(CLASS_DROP) {
            let fields = db.fields(CLASS_DROP, key)?;
            let get = |k: u32| fields.iter().find(|f| f.key == k).and_then(|f| single(&f.data));
            let text = |k: u32| fields.iter().find(|f| f.key == k).and_then(|f| single_text(&f.data));
            if !matches!(get(FIELD_TRACK), Some(Value::Ref(r)) if r.collection == track_key) {
                continue;
            }
            let postfx = text(FIELD_POSTFX);
            let (lighting, lighting_key) = match get(FIELD_LIGHTING) {
                Some(Value::Ref(r)) if r.class == CLASS_LIGHTING && r.collection != 0 => {
                    (preset(db, r.collection, postfx.as_deref())?, Some(r.collection))
                }
                _ => (LightingPreset::default(), None),
            };
            out.push(Drop {
                key,
                text_id: text(FIELD_TEXT_ID),
                peak: text(FIELD_PEAK),
                postfx,
                selectable: matches!(get(FIELD_SELECTABLE), Some(Value::Bool(true))),
                lighting,
                lighting_key,
            });
        }
        Ok(out)
    })
}

pub(crate) fn preset(db: &Db, key: u32, postfx: Option<&str>) -> Result<LightingPreset> {
    let mut name = db.collection_name(key);
    if let Some(base) = postfx.and_then(|p| p.strip_suffix("_postfx")) {
        let candidates = [format!("{base}_lighting"), format!("{base}_pre_lighting")];
        if let Some(c) = candidates.into_iter().find(|c| fnv1(c) == key) {
            name = c;
        }
    }
    let mut p = LightingPreset { name, ..Default::default() };
    for f in db.fields(CLASS_LIGHTING, key)? {
        let field = match RECOVERED_FIELD_NAMES.iter().find(|(k, _)| *k == f.key) {
            Some((_, name)) => (*name).to_string(),
            None => db.field_name(f.key),
        };
        match single(&f.data) {
            Some(Value::Float(x)) => {
                p.floats.insert(field, *x);
            }
            Some(Value::Vector4(v)) => {
                p.vectors.insert(field, *v);
            }
            Some(Value::Ref(r)) if r.class == CLASS_TEXTURE && r.collection != 0 => {
                if let Some(path) = db.text_of(r.class, r.collection)? {
                    p.textures.insert(field, path);
                }
            }
            Some(Value::Ref(r))
                if f.key == FIELD_SUN_NODE && r.class == crate::particles::CLASS_PARTICLE_NODE && r.collection != 0 =>
            {
                p.sun_node = Some(crate::particles::particle_node(db, r.collection)?);
            }
            _ => {}
        }
    }
    Ok(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recovered_names_hash_to_their_keys() {
        for (key, name) in RECOVERED_FIELD_NAMES {
            assert_eq!(fnv1(name), key, "{name}");
        }
    }
}
