use std::collections::BTreeMap;

use serde::Deserialize;

use crate::id::ContentId;
use crate::manifest::line_col;
use crate::model::{AlphaMode, ModelMaterial};
use crate::texture::TextureKind;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Family {
    Object,
    Foreground,
    Sky,
    Character,
}

pub const UNREAD_FAMILIES: [(&str, &str); 3] = [
    ("terrain", "a map's terrain is drawn with the disc's terrain materials"),
    ("geotag", "the geotags are drawn with the disc's materials"),
    ("midground", "the mid-ground mountains are drawn with the disc's materials"),
];

pub const OBJECT_EFFECTS: [&str; 9] = [
    "object_default",
    "object_default_anim",
    "object_alpha",
    "object_alpha_anim",
    "object_foliage",
    "object_foliage_anim",
    "object_advanced",
    "object_texture_anim",
    "object_shadow",
];
pub const FOREGROUND_EFFECTS: [&str; 1] = ["terrain_foreground"];
pub const SKY_EFFECTS: [&str; 1] = ["sky_dome"];
pub const CHARACTER_EFFECTS: [&str; 7] = [
    "player_body",
    "player_hair",
    "player_eye",
    "player_apparel",
    "player_board",
    "player_wingsuit",
    "player_secondary",
];
pub const CHARACTER_TEXTURES: [&str; 4] = ["diffuseTexture", "normalTexture", "specularTexture", "ambOcclusionTexture"];
pub const CHARACTER_PARAMETERS: [&str; 6] =
    ["specularIntensity", "specularEccentricity", "glowEnable", "glowIntensity", "glowColour", "rimMult"];

pub const OBJECT_TEXTURES: [&str; 5] =
    ["diffuseTexture", "normalTexture", "specularTexture", "scatterNormalMap", "sparkleNormalMap"];
pub const FOREGROUND_TEXTURES: [&str; 6] = [
    "maskTexture",
    "baseChannelTexture",
    "redChannelTexture",
    "greenChannelTexture",
    "ambOcclusionTexture",
    "normalTexture",
];
pub const SKY_TEXTURES: [&str; 1] = ["skyTexture"];
pub const OBJECT_PARAMETERS: [&str; 16] = [
    "specularIntensity",
    "specularEccentricity",
    "glowEnable",
    "glowIntensity",
    "normalTile",
    "specularTile",
    "diffuseTile",
    "snowFallDirection",
    "snowBlendOffset",
    "snowBlendPoint",
    "snowBlendRange",
    "snowSpecularIntensity",
    "snowEccentricity",
    "snowScatterTile",
    "snowScatterAmount",
    "snowNormalBlendOffset",
];
pub const FOREGROUND_PARAMETERS: [&str; 5] =
    ["baseChannelTile", "redChannelTile", "greenChannelTile", "aoTile", "snowBias"];
pub const SKY_PARAMETERS: [&str; 3] = ["skyTile", "tint", "exposure"];
pub const OBJECT_TEXTURE_TILES: [(&str, &str); 4] = [
    ("diffuseTexture", "diffuseTile"),
    ("normalTexture", "normalTile"),
    ("specularTexture", "specularTile"),
    ("scatterNormalMap", "snowScatterTile"),
];
pub const FOREGROUND_TEXTURE_TILES: [(&str, &str); 4] = [
    ("baseChannelTexture", "baseChannelTile"),
    ("redChannelTexture", "redChannelTile"),
    ("greenChannelTexture", "greenChannelTile"),
    ("ambOcclusionTexture", "aoTile"),
];

pub fn missing_tiles(
    m: &MaterialDef,
    effect_parameters: &std::collections::BTreeSet<String>,
) -> Vec<(&'static str, &'static str)> {
    let tiles: &[(&str, &str)] = match m.family {
        Some(f) => f.texture_tiles(),
        None => &[],
    };
    tiles
        .iter()
        .filter(|(texture, tile)| {
            m.textures.contains_key(*texture)
                && effect_parameters.contains(*tile)
                && !m.floats.contains_key(*tile)
                && !m.vectors.contains_key(*tile)
        })
        .copied()
        .collect()
}

pub const TECHNIQUES: [&str; 3] = ["prelight", "forward", "translucent"];

pub fn unread_effect(effect: &str) -> &'static str {
    let e = effect.strip_suffix(".fx").unwrap_or(effect);
    if e == "object_geotag" || e == "object_default_skinned" {
        "the geotags' and the skinned objects' materials"
    } else if e.starts_with("terrain_midground") || e.starts_with("terrain_oceanwave") {
        "the mid-ground mountains' materials"
    } else if e.starts_with("terrain_") {
        "the terrain's materials"
    } else if e.starts_with("globe_") || e.starts_with("ui_") {
        "the front end's materials"
    } else {
        "a family packages do not provide"
    }
}

impl Family {
    pub fn parse(name: &str) -> Result<Family, String> {
        match name {
            "object" => Ok(Family::Object),
            "foreground" => Ok(Family::Foreground),
            "sky" => Ok(Family::Sky),
            "character" => Ok(Family::Character),
            _ => match UNREAD_FAMILIES.iter().find(|(n, _)| *n == name) {
                Some((n, why)) => Err(format!("the {n} family is not read from packages ({why})")),
                None => Err(format!(
                    "family {}: not one of object, foreground, sky, character (terrain, geotag and midground are not \
                     read from packages)",
                    ssx_core::text::quoted(name)
                )),
            },
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Family::Object => "object",
            Family::Foreground => "foreground",
            Family::Sky => "sky",
            Family::Character => "character",
        }
    }

    pub fn effects(self) -> &'static [&'static str] {
        match self {
            Family::Object => &OBJECT_EFFECTS,
            Family::Foreground => &FOREGROUND_EFFECTS,
            Family::Sky => &SKY_EFFECTS,
            Family::Character => &CHARACTER_EFFECTS,
        }
    }

    pub fn textures(self) -> &'static [&'static str] {
        match self {
            Family::Object => &OBJECT_TEXTURES,
            Family::Foreground => &FOREGROUND_TEXTURES,
            Family::Sky => &SKY_TEXTURES,
            Family::Character => &CHARACTER_TEXTURES,
        }
    }

    pub fn texture_tiles(self) -> &'static [(&'static str, &'static str)] {
        match self {
            Family::Object => &OBJECT_TEXTURE_TILES,
            Family::Foreground => &FOREGROUND_TEXTURE_TILES,
            Family::Sky | Family::Character => &[],
        }
    }

    pub fn parameters(self) -> &'static [&'static str] {
        match self {
            Family::Object => &OBJECT_PARAMETERS,
            Family::Foreground => &FOREGROUND_PARAMETERS,
            Family::Sky => &SKY_PARAMETERS,
            Family::Character => &CHARACTER_PARAMETERS,
        }
    }

    pub fn hooks(self) -> Option<crate::shader::HookFamily> {
        match self {
            Family::Object => Some(crate::shader::HookFamily::Object),
            Family::Sky => Some(crate::shader::HookFamily::Sky),
            Family::Foreground | Family::Character => None,
        }
    }

    pub fn is_game(self) -> bool {
        !matches!(self, Family::Sky)
    }

    pub fn of_effect(effect: &str) -> Option<Family> {
        let stem = effect.strip_suffix(".fx").unwrap_or(effect);
        [Family::Object, Family::Foreground, Family::Character].into_iter().find(|f| f.effects().contains(&stem))
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum TextureRef {
    Id(ContentId),
    ModelImage { model: ContentId, image: usize, kind: TextureKind },
    Path(String),
}

#[derive(Clone, Debug, PartialEq)]
pub struct MaterialDef {
    pub name: String,
    pub family: Option<Family>,
    pub effect: String,
    pub technique: Option<String>,
    pub textures: BTreeMap<String, TextureRef>,
    pub floats: BTreeMap<String, f32>,
    pub vectors: BTreeMap<String, Vec<f32>>,
    pub double_sided: Option<bool>,
    pub base_colour: Option<[f32; 4]>,
    pub shader: Option<ShaderRef>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ShaderRef {
    pub file: String,
    pub source: Option<std::sync::Arc<str>>,
    pub hash: Option<crate::hash::ContentHash>,
    pub problem: Option<String>,
}

impl MaterialDef {
    pub fn float(&self, name: &str) -> Option<f32> {
        self.floats.get(name).copied()
    }
}

pub const VECTOR_PARAMETERS: [&str; 11] = [
    "glowColour",
    "normalTile",
    "specularTile",
    "diffuseTile",
    "snowFallDirection",
    "baseChannelTile",
    "redChannelTile",
    "greenChannelTile",
    "aoTile",
    "skyTile",
    "tint",
];

#[derive(Deserialize)]
#[serde(untagged)]
enum Param {
    Number(f64),
    Vector(Vec<f64>),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawMaterial {
    family: toml::Spanned<String>,
    effect: toml::Spanned<String>,
    technique: Option<toml::Spanned<String>>,
    double_sided: Option<bool>,
    #[serde(default)]
    textures: BTreeMap<toml::Spanned<String>, toml::Spanned<String>>,
    #[serde(default)]
    parameters: BTreeMap<toml::Spanned<String>, toml::Spanned<Param>>,
    shader: Option<toml::Spanned<String>>,
}

pub type Problem = (usize, usize, String);

pub fn parse_toml(bytes: &[u8], name: &str) -> Result<MaterialDef, Vec<Problem>> {
    crate::package::parse_record_at(bytes).map_err(|p| vec![p])?;
    let text = std::str::from_utf8(bytes).map_err(|e| vec![(0, 0, format!("not UTF-8: {e}"))])?;
    let raw: RawMaterial = toml::from_str(text).map_err(|e| {
        let (line, column) = e.span().map_or((0, 0), |s| line_col(text, s.start));
        vec![(line, column, ssx_core::text::message(e.message()).into_string())]
    })?;
    let mut problems = Vec::new();
    let mut at = |span: std::ops::Range<usize>, m: String| {
        let (line, column) = line_col(text, span.start);
        problems.push((line, column, m));
    };
    let family = match Family::parse(raw.family.get_ref()) {
        Ok(f) => f,
        Err(e) => {
            at(raw.family.span(), e);
            return Err(problems);
        }
    };
    let effect = raw.effect.get_ref();
    if !family.effects().contains(&effect.as_str()) {
        at(
            raw.effect.span(),
            format!(
                "effect {}: the {} family's are {}",
                ssx_core::text::quoted(effect),
                family.as_str(),
                family.effects().join(", ")
            ),
        );
    }
    if let Some(t) = &raw.technique
        && !TECHNIQUES.contains(&t.get_ref().as_str())
    {
        at(t.span(), format!("technique {}: one of {}", ssx_core::text::quoted(t.get_ref()), TECHNIQUES.join(", ")));
    }
    let mut textures = BTreeMap::new();
    for (k, v) in &raw.textures {
        if !family.textures().contains(&k.get_ref().as_str()) {
            at(
                k.span(),
                format!(
                    "{} is not a texture of the {} family ({})",
                    ssx_core::text::quoted(k.get_ref()),
                    family.as_str(),
                    family.textures().join(", ")
                ),
            );
            continue;
        }
        match v.get_ref().parse::<ContentId>() {
            Ok(id) if id.kind() == crate::Kind::Texture => {
                textures.insert(k.get_ref().clone(), TextureRef::Id(id));
            }
            Ok(id) => at(v.span(), format!("{id} is not a texture id")),
            Err(e) => at(v.span(), e.to_string()),
        }
    }
    let (mut floats, mut vectors) = (BTreeMap::new(), BTreeMap::new());
    for (k, v) in &raw.parameters {
        let key = k.get_ref().as_str();
        if !family.parameters().contains(&key) {
            at(
                k.span(),
                format!(
                    "{} is not a parameter of the {} family ({})",
                    ssx_core::text::quoted(key),
                    family.as_str(),
                    family.parameters().join(", ")
                ),
            );
            continue;
        }
        let finite = |x: f64| x.is_finite() && x.abs() <= f64::from(f32::MAX);
        match (v.get_ref(), VECTOR_PARAMETERS.contains(&key)) {
            (Param::Number(x), false) if finite(*x) => {
                floats.insert(key.to_string(), *x as f32);
            }
            (Param::Vector(xs), true) if (2..=4).contains(&xs.len()) && xs.iter().all(|x| finite(*x)) => {
                vectors.insert(key.to_string(), xs.iter().map(|x| *x as f32).collect());
            }
            (_, true) => at(v.span(), format!("{key} is a vector: 2 to 4 finite numbers")),
            (_, false) => at(v.span(), format!("{key} is one finite number")),
        }
    }
    let shader = match &raw.shader {
        Some(f) if family.hooks().is_none() => {
            at(
                f.span(),
                format!(
                    "the {} family has no shader hooks (the object family's and the sky dome's have)",
                    family.as_str()
                ),
            );
            None
        }
        Some(f) => match crate::path::check_package_path(f.get_ref(), false) {
            Err(e) => {
                at(f.span(), format!("shader: {e}"));
                None
            }
            Ok(()) if !f.get_ref().to_ascii_lowercase().ends_with(".wgsl") => {
                at(f.span(), "shader: a .wgsl file".into());
                None
            }
            Ok(()) => Some(ShaderRef { file: f.get_ref().clone(), source: None, hash: None, problem: None }),
        },
        None => None,
    };
    if family == Family::Sky
        && let Some(t) = &raw.technique
    {
        at(t.span(), "the sky family has no techniques".into());
    }
    if !problems.is_empty() {
        return Err(problems);
    }
    let effect = if family.is_game() { format!("{effect}.fx") } else { effect.clone() };
    Ok(MaterialDef {
        name: name.to_string(),
        family: Some(family),
        effect,
        technique: raw.technique.map(|t| t.into_inner()),
        textures,
        floats,
        vectors,
        double_sided: raw.double_sided,
        base_colour: None,
        shader,
    })
}

pub fn from_gltf(model: &ContentId, m: &ModelMaterial, slot: usize) -> (MaterialDef, Vec<String>) {
    let name = m.name.clone().unwrap_or_else(|| format!("material {slot}"));
    let mut notes = Vec::new();
    let mut textures = BTreeMap::new();
    let image = |r: &Option<crate::model::ImageRef>, param: &str, kind: TextureKind, notes: &mut Vec<String>| {
        let r = (*r)?;
        if r.tex_coord != 0 {
            notes.push(format!(
                "{name}: {param} samples TEXCOORD_{}; the object family samples UV 0: not used",
                r.tex_coord
            ));
            return None;
        }
        Some(TextureRef::ModelImage { model: model.clone(), image: r.image, kind })
    };
    if let Some(t) = image(&m.base_colour_texture, "baseColorTexture", TextureKind::Color, &mut notes) {
        textures.insert("diffuseTexture".to_string(), t);
    }
    if let Some(t) = image(&m.normal_texture, "normalTexture", TextureKind::Normal, &mut notes) {
        textures.insert("normalTexture".to_string(), t);
    }
    if m.normal_texture.is_some() && m.normal_scale != 1.0 {
        notes.push(format!(
            "{name}: normalTexture.scale {} is not read (the family has no normal scale)",
            m.normal_scale
        ));
    }
    for (slot, what) in [
        (
            &m.metallic_roughness_texture,
            "metallicRoughnessTexture (the family's specular map is a mask, not roughness)",
        ),
        (&m.occlusion_texture, "occlusionTexture (the family's ambient occlusion is the vertex colour's G)"),
        (&m.emissive_texture, "emissiveTexture"),
    ] {
        if slot.is_some() {
            notes.push(format!("{name}: {what} is not read"));
        }
    }
    if m.emissive != [0.0; 3] {
        notes.push(format!("{name}: emissiveFactor is not read (the family's glow is its own)"));
    }
    let effect = match m.alpha {
        AlphaMode::Opaque => "object_default.fx",
        AlphaMode::Mask | AlphaMode::Blend => "object_alpha.fx",
    };
    if m.alpha == AlphaMode::Blend {
        notes.push(format!("{name}: alphaMode BLEND is drawn alpha-tested (the family has no blended effect)"));
    }
    if m.alpha == AlphaMode::Mask && m.alpha_cutoff != 0.25 {
        notes.push(format!("{name}: alphaCutoff {} is not read: the family tests at 0.25", m.alpha_cutoff));
    }
    let mut vectors = BTreeMap::new();
    for (texture, tile) in OBJECT_TEXTURE_TILES {
        if textures.contains_key(texture) {
            vectors.insert(tile.to_string(), vec![1.0, 1.0]);
        }
    }
    let mut floats = BTreeMap::new();
    // PORT(choice): a glTF metallic-roughness material on the game's Phong family: roughness r → α = r², the Blinn
    // exponent 2/α² − 2 halved twice for Phong's reflect form (clamped 1…512) as specularEccentricity; the reflectance
    // at normal incidence F0 = 0.04 (dielectric) mixed to the base colour's luma by metallic, its square root as
    // specularIntensity (the game squares specularIntensity · specular.g); no glow; tuning: fit the two on renders of
    // the glTF sample materials against a reference PBR renderer; no mapping is in the game.
    let alpha = (m.roughness.clamp(0.0, 1.0) * m.roughness.clamp(0.0, 1.0)).max(1e-3);
    floats.insert("specularEccentricity".to_string(), ((2.0 / (alpha * alpha) - 2.0) / 4.0).clamp(1.0, 512.0));
    let [r, g, b, _] = m.base_colour;
    let luma = 0.2126 * r + 0.7152 * g + 0.0722 * b;
    let metallic = m.metallic.clamp(0.0, 1.0);
    floats.insert("specularIntensity".to_string(), (0.04 * (1.0 - metallic) + luma.clamp(0.0, 1.0) * metallic).sqrt());
    floats.insert("glowEnable".to_string(), 0.0);
    floats.insert("glowIntensity".to_string(), 0.0);
    let base = m.base_colour.map(|c| c.clamp(0.0, 1.0));
    let base_colour = Some([base[0].sqrt(), base[1].sqrt(), base[2].sqrt(), base[3]]);
    let def = MaterialDef {
        name,
        family: Some(Family::Object),
        effect: effect.to_string(),
        technique: None,
        textures,
        floats,
        vectors,
        double_sided: Some(m.double_sided),
        base_colour,
        shader: None,
    };
    (def, notes)
}

pub fn character_effect(slot: &str) -> &'static str {
    match slot {
        "head" => "player_body.fx",
        "hair" => "player_hair.fx",
        "eyes" => "player_eye.fx",
        "wingsuit" => "player_wingsuit.fx",
        "snowboard" => "player_board.fx",
        _ => "player_apparel.fx",
    }
}

pub fn from_gltf_character(
    owner: &ContentId,
    m: &ModelMaterial,
    slot: usize,
    part_slot: &str,
) -> (MaterialDef, Vec<String>) {
    let (mut def, mut notes) = from_gltf(owner, m, slot);
    def.family = Some(Family::Character);
    def.effect = character_effect(part_slot).to_string();
    for (_, tile) in OBJECT_TEXTURE_TILES {
        def.vectors.remove(tile);
    }
    if m.alpha != AlphaMode::Opaque {
        notes.push(format!("{}: alphaMode {:?}: {} cuts out by its own rule", def.name, m.alpha, def.effect));
    }
    notes.retain(|n| !n.contains("the family tests at 0.25") && !n.contains("drawn alpha-tested"));
    (def, notes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_material_toml_reads_and_refuses_with_lines() {
        let ok = br#"
family = "object"
effect = "object_advanced"
technique = "prelight"
[textures]
diffuseTexture = "rocks:texture/boulder_d"
normalTexture = "ssx:texture/worlds/textures/rock_basalt_norm"
[parameters]
specularIntensity = 0.4
normalTile = [2.0, 2.0]
snowFallDirection = [0.0, -1.0, 0.0]
"#;
        let m = parse_toml(ok, "m").unwrap();
        assert_eq!((m.family, m.effect.as_str()), (Some(Family::Object), "object_advanced.fx"));
        assert_eq!(m.vectors["snowFallDirection"], [0.0, -1.0, 0.0]);
        let bad = br#"family = "object"
effect = "player_body"
technique = "forwardHalo"
[textures]
ambOcclusionTexture = "a:texture/x"
normalTexture = "a:model/x"
[parameters]
specularIntensity = [1.0, 2.0]
normalTile = 1.0
glowColour = [1, 1, 1]
"#;
        let p = parse_toml(bad, "m").unwrap_err();
        let lines: Vec<usize> = p.iter().map(|(l, _, _)| *l).collect();
        assert_eq!(lines, [2, 3, 5, 6, 10, 9, 8], "{p:?}");
        let later = parse_toml(b"family = \"geotag\"\neffect = \"object_geotag\"\n", "m").unwrap_err();
        assert!(later[0].2.contains("the geotag family is not read from packages (the geotags"), "{later:?}");
        let rider = parse_toml(
            b"family = \"character\"\neffect = \"player_apparel\"\n[parameters]\nglowColour = [1.0, 0.5, 0.0]\n",
            "m",
        )
        .unwrap();
        assert_eq!((rider.family, rider.effect.as_str()), (Some(Family::Character), "player_apparel.fx"));
        assert!(
            parse_toml(b"family = \"object\"\neffect = \"object_default\"\nglow = 1\n", "m").unwrap_err()[0]
                .2
                .contains("unknown field")
        );
        let hooked =
            parse_toml(b"family = \"object\"\neffect = \"object_default\"\nshader = \"shaders/g.wgsl\"\n", "m");
        assert_eq!(hooked.unwrap().shader.unwrap().file, "shaders/g.wgsl");
        let fg = parse_toml(b"family = \"foreground\"\neffect = \"terrain_foreground\"\nshader = \"s.wgsl\"\n", "m");
        assert!(fg.unwrap_err()[0].2.contains("no shader hooks"));
        let sky = parse_toml(
            b"family = \"sky\"\neffect = \"sky_dome\"\n[textures]\nskyTexture = \"a:texture/s\"\n[parameters]\nexposure = 2.0\ntint = [1.0, 0.5, 0.5]\n",
            "m",
        )
        .unwrap();
        assert_eq!((sky.family, sky.effect.as_str()), (Some(Family::Sky), "sky_dome"));
    }

    #[test]
    fn a_pbr_material_maps_onto_the_object_family() {
        let model: ContentId = "rocks:model/boulder".parse().unwrap();
        let m = ModelMaterial {
            name: Some("Rock".into()),
            base_colour: [0.25, 1.0, 0.0, 1.0],
            base_colour_texture: Some(crate::model::ImageRef { image: 0, tex_coord: 0 }),
            metallic: 0.0,
            roughness: 1.0,
            metallic_roughness_texture: None,
            normal_texture: Some(crate::model::ImageRef { image: 1, tex_coord: 1 }),
            normal_scale: 1.0,
            occlusion_texture: None,
            emissive: [0.0; 3],
            emissive_texture: None,
            alpha: AlphaMode::Mask,
            alpha_cutoff: 0.5,
            double_sided: true,
            extras: Default::default(),
        };
        let (def, notes) = from_gltf(&model, &m, 0);
        assert_eq!(def.effect, "object_alpha.fx");
        assert_eq!(def.base_colour, Some([0.5, 1.0, 0.0, 1.0]), "the game squares the multiplier");
        assert!(def.textures.contains_key("diffuseTexture") && !def.textures.contains_key("normalTexture"));
        assert_eq!(notes.len(), 2, "{notes:?}");
        assert_eq!(def.float("specularEccentricity"), Some(1.0), "roughness 1: the widest lobe");
        assert!((def.float("specularIntensity").unwrap() - 0.2).abs() < 1e-6, "a dielectric's F0 0.04");
        assert_eq!(def.vectors["diffuseTile"], [1.0, 1.0], "glTF samples its texture once across UV 0");
        assert!(!def.vectors.contains_key("normalTile"), "no normal map mapped, no tile");
    }

    #[test]
    fn a_texture_without_its_tile_is_found() {
        let text = br#"family = "object"
effect = "object_default"
[textures]
diffuseTexture = "a:texture/d"
normalTexture = "a:texture/n"
specularTexture = "a:texture/s"
[parameters]
normalTile = [2.0, 2.0]
"#;
        let m = parse_toml(text, "m").unwrap();
        let fx: std::collections::BTreeSet<String> =
            ["diffuseTile", "normalTile", "specularTile"].map(String::from).into();
        assert_eq!(missing_tiles(&m, &fx), [("diffuseTexture", "diffuseTile"), ("specularTexture", "specularTile")]);
        assert!(missing_tiles(&m, &Default::default()).is_empty());
        let fg = parse_toml(
            b"family = \"foreground\"\neffect = \"terrain_foreground\"\n[textures]\nnormalTexture = \"a:texture/n\"\nredChannelTexture = \"a:texture/r\"\n",
            "fg",
        )
        .unwrap();
        let fx: std::collections::BTreeSet<String> = ["normalTile", "redChannelTile"].map(String::from).into();
        assert_eq!(missing_tiles(&fg, &fx), [("redChannelTexture", "redChannelTile")]);
    }
}
