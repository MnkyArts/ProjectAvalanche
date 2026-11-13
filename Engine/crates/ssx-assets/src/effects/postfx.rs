mod cube;
pub(crate) mod xml;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use ssx_formats::attrib::collection::Field;
use ssx_formats::attrib::{FieldData, Value};

pub use cube::{BlendMode, ColourCube, LayerKind, N as CUBE_SIZE, Stage, apply_stage, quantise};
pub use xml::{XmlGroup, XmlLayer, XmlStack, parse_stack};

use crate::vault_db::{Db, fnv1};

pub const CLASS_POSTFX: u32 = fnv1("postfx");
const CLASS_DROP: u32 = fnv1("ge_drop");
const FIELD_DROP_POSTFX: u32 = 0x5595_A25C;
const FIELD_LAYER: u32 = 0xE8B4_B707;
pub const FIELD_BLOOM_ACTIVE: u32 = 0x8E58_1409;
const FIELD_COLOUR_CUBE_BLEND: u32 = 0x8E8B_3DA4;
const FIELD_COLOUR_CUBE_STACK: u32 = 0x92F1_952A;

pub const FIELD_NAMES: [&str; 16] = [
    "bloom_blur_x",
    "bloom_blur_y",
    "colourcube_depth_near",
    "colourcube_depth_far",
    "motion_blur_intensity",
    "motion_blur_origin",
    "topogrid_colour_grid",
    "topogrid_pulse_falloff",
    "topogrid_dist_start",
    "topogrid_dist_end",
    "topogrid_dist_fadein",
    "topogrid_dist_fadeout",
    "topogrid_min_intensities",
    "topogrid_max_intensities",
    "vignette_colour",
    "vignette_fadeoffbounds",
];

#[derive(Clone, Debug, PartialEq)]
pub struct ColourCubeBlend {
    pub stack: u32,
    pub mode: u32,
    pub group: String,
    pub weight: f32,
}

#[derive(Clone, Debug, Default)]
pub struct PostFxPreset {
    pub name: String,
    pub chain: Vec<String>,
    pub layer: Option<String>,
    pub floats: BTreeMap<String, f32>,
    pub vectors: BTreeMap<String, Vec<f32>>,
    pub colour_cubes: Vec<ColourCubeBlend>,
}

impl PostFxPreset {
    pub fn float(&self, name: &str) -> Option<f32> {
        self.floats.get(name).copied()
    }

    pub fn bloom_active(&self) -> bool {
        self.float(&format!("{FIELD_BLOOM_ACTIVE:#010x}")).is_some_and(|v| v > 0.001)
    }

    pub fn bloom_blur(&self) -> Option<[f32; 2]> {
        Some([self.float("bloom_blur_x")?, self.float("bloom_blur_y")?])
    }
}

#[derive(Clone, Debug)]
pub struct ColourStack {
    pub index: u32,
    pub file: PathBuf,
    pub xml: XmlStack,
}

#[derive(Clone, Debug)]
pub struct CubeBuild {
    pub cube: ColourCube,
    pub stages: Vec<String>,
    pub needs_second_lut: bool,
}

fn values(d: &FieldData) -> Vec<&Value> {
    match d {
        FieldData::Single(v) => vec![v],
        FieldData::Array(vs) => vs.iter().collect(),
    }
}

fn be_u32(b: &[u8], at: usize) -> u32 {
    u32::from_be_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

fn colour_cube_blend(db: &Db, class: u32, source: u32, v: &Value) -> Result<ColourCubeBlend> {
    let Value::Raw { bytes, .. } = v else { bail!("ColourCubeBlend is not a raw value") };
    if bytes.len() != 16 || be_u32(bytes, 8) != 0 {
        bail!("ColourCubeBlend: unexpected layout {bytes:02x?}");
    }
    Ok(ColourCubeBlend {
        stack: be_u32(bytes, 0),
        mode: be_u32(bytes, 4),
        group: db.value_text(class, source, v, 8)?.context("ColourCubeBlend without a group name")?,
        weight: f32::from_bits(be_u32(bytes, 12)),
    })
}

fn preset(db: &Db, key: u32, name: &str) -> Result<PostFxPreset> {
    db.find(CLASS_POSTFX, key).with_context(|| format!("no postfx collection {name:?}"))?;
    let mut p = PostFxPreset { name: name.to_string(), ..Default::default() };
    let mut cur = Some(key);
    while let Some(k) = cur {
        p.chain.push(db.collection_name(k));
        let parent = db.parent(CLASS_POSTFX, k);
        cur = (parent != 0).then_some(parent);
        if p.chain.len() > db.vaults.iter().map(|v| v.cols.len()).sum() {
            bail!("{name}: parent chain loops");
        }
    }
    for (source, f) in db.fields_with_source(CLASS_POSTFX, key)? {
        let field = FIELD_NAMES
            .iter()
            .find(|n| fnv1(n) == f.key)
            .map_or_else(|| format!("{:#010x}", f.key), |n| (*n).to_string());
        let vals = values(&f.data);
        match vals.first() {
            Some(Value::Float(x)) => {
                p.floats.insert(field, *x);
            }
            Some(Value::Vector2(v)) => {
                p.vectors.insert(field, v.to_vec());
            }
            Some(Value::Vector3(v)) => {
                p.vectors.insert(field, v.to_vec());
            }
            Some(Value::Vector4(v)) => {
                p.vectors.insert(field, v.to_vec());
            }
            Some(Value::Ref(r)) if f.key == FIELD_LAYER && r.collection != 0 => {
                p.layer = Some(match db.find(r.class, r.collection) {
                    Some(_) => db.collection_name(r.collection),
                    None => format!("{:08x}", r.collection),
                });
            }
            Some(_) if f.key == FIELD_COLOUR_CUBE_BLEND => {
                p.colour_cubes =
                    vals.into_iter().map(|v| colour_cube_blend(db, CLASS_POSTFX, source, v)).collect::<Result<_>>()?;
            }
            _ => {}
        }
    }
    Ok(p)
}

fn stacks(db: &Db, big_root: &Path) -> Result<Vec<ColourStack>> {
    let mut found: Option<(u32, &Field)> = None;
    for (key, _) in db.collections_of_all(CLASS_POSTFX) {
        if let Some(f) = db.own_field(CLASS_POSTFX, key, FIELD_COLOUR_CUBE_STACK) {
            if found.is_some() {
                bail!("more than one postfx collection has a ColourCubeStack ({key:08x})");
            }
            found = Some((key, f));
        }
    }
    let (source, field) = found.context("no postfx collection has a ColourCubeStack")?;
    let fallback = big_root.join("memresident/data/common/postfx/colourcube");
    let mut out = Vec::new();
    for v in values(&field.data) {
        let Value::Raw { bytes, .. } = v else { bail!("ColourCubeStack is not a raw value") };
        if bytes.len() != 8 {
            bail!("ColourCubeStack entry of {} bytes", bytes.len());
        }
        let name = db.value_text(CLASS_POSTFX, source, v, 4)?.context("ColourCubeStack entry without a name")?;
        let rel = format!("data/common/postfx/colourcube/{name}.xml");
        let file = crate::data_roots::find(big_root, &rel).unwrap_or_else(|| fallback.join(format!("{name}.xml")));
        let text = std::fs::read_to_string(&file).with_context(|| format!("reading {}", file.display()))?;
        let xml = parse_stack(&text).with_context(|| format!("parsing {}", file.display()))?;
        if xml.name != name {
            bail!("{} declares stack {:?}", file.display(), xml.name);
        }
        out.push(ColourStack { index: be_u32(bytes, 0), file, xml });
    }
    out.sort_by_key(|s| s.index);
    Ok(out)
}

pub fn build_cube(stacks: &[ColourStack], blends: &[ColourCubeBlend]) -> Result<CubeBuild> {
    for b in blends {
        let stack = stacks.iter().find(|s| s.index == b.stack).with_context(|| format!("no stack {}", b.stack))?;
        if !stack.xml.groups.iter().any(|g| g.name == b.group) {
            bail!("stack {} has no group {:?}", stack.xml.name, b.group);
        }
        if b.mode > 2 {
            bail!("ColourCubeBlend mode {} (expected 0..2)", b.mode);
        }
    }
    let mut cube = ColourCube::identity();
    let mut stages = Vec::new();
    for stack in stacks {
        for group in &stack.xml.groups {
            let blend = blends.iter().find(|b| b.stack == stack.index && b.group == group.name && b.mode != 2);
            let (enabled, opacity) = match blend {
                Some(b) => {
                    let o = ((b.weight * 255.0) as i32).clamp(0, 255) as u32;
                    (o > 0, o)
                }
                None => (group.enabled, group.opacity),
            };
            for layer in group.layers.iter().filter(|l| enabled && l.enabled) {
                let opacity = layer.opacity * opacity / 255;
                if opacity == 0 {
                    continue;
                }
                let kind = LayerKind::parse(layer).with_context(|| format!("{}/{}", stack.xml.name, group.name))?;
                let blend = BlendMode::parse(&layer.blend_mode).with_context(|| {
                    format!(
                        "{}/{}/{}: blend mode {} is not decoded",
                        stack.xml.name, group.name, layer.name, layer.blend_mode
                    )
                })?;
                let label = format!(
                    "{}/{}/{} ({}, {}, {opacity})",
                    stack.xml.name, group.name, layer.name, layer.type_name, layer.blend_mode
                );
                if kind != LayerKind::LoadColourCube {
                    apply_stage(&mut cube, &Stage { label: label.clone(), kind, blend, opacity });
                    stages.push(label);
                }
            }
        }
    }
    let needs_second_lut = blends.iter().any(|b| b.mode != 0 && b.weight > 0.0);
    Ok(CubeBuild { cube, stages, needs_second_lut })
}

pub fn load_presets(big_root: &Path, names: &[&str]) -> Result<Vec<(PostFxPreset, CubeBuild)>> {
    let presets = disc_presets(big_root, names)?;
    let content = crate::content::GameContent::of(big_root);
    Ok(presets.into_iter().map(|(p, b)| content.route_postfx(p, b)).collect())
}

fn disc_presets(big_root: &Path, names: &[&str]) -> Result<Vec<(PostFxPreset, CubeBuild)>> {
    crate::vault_db::with_career_db(big_root, |db| {
        let stacks = stacks(db, big_root)?;
        names
            .iter()
            .map(|name| {
                let p = preset(db, fnv1(name), name)?;
                let build = build_cube(&stacks, &p.colour_cubes).with_context(|| format!("colour cube of {name}"))?;
                Ok((p, build))
            })
            .collect()
    })
}

pub fn load_preset_by_key(big_root: &Path, key: u32, name: &str) -> Result<PostFxPreset> {
    crate::vault_db::with_career_db(big_root, |db| preset(db, key, name))
}

pub fn build_cube_for(big_root: &Path, blends: &[ColourCubeBlend]) -> Result<CubeBuild> {
    crate::vault_db::with_career_db(big_root, |db| build_cube(&stacks(db, big_root)?, blends))
}

pub fn stack_groups(big_root: &Path) -> Result<Vec<(u32, String, Vec<String>)>> {
    crate::vault_db::with_career_db(big_root, |db| {
        Ok(stacks(db, big_root)?
            .into_iter()
            .map(|s| (s.index, s.xml.name.clone(), s.xml.groups.iter().map(|g| g.name.clone()).collect()))
            .collect())
    })
}

pub fn drop_postfx_names(big_root: &Path) -> Result<Vec<String>> {
    crate::vault_db::with_all_db(big_root, |db| {
        let mut out = Vec::new();
        for (key, _) in db.collections_of_all(CLASS_DROP) {
            let text = db.fields(CLASS_DROP, key)?.into_iter().find(|f| f.key == FIELD_DROP_POSTFX).and_then(|f| {
                match &f.data {
                    FieldData::Single(Value::Text(Some(t))) => Some(t.clone()),
                    _ => None,
                }
            });
            out.extend(text);
        }
        Ok(out)
    })
}

pub fn check_base_cube(big_root: &Path) -> Result<(usize, usize)> {
    use ssx_formats::xpr::XprFile;
    let path = big_root.join("bootcache/data/xenon/gameexplorer/postfx/textures/postfx_colourcube_base.xpr");
    let data = std::fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
    let xpr = XprFile::parse(&data)?;
    let res = xpr.resources.first().context("no resource")?;
    let levels = crate::texture::decode_resource(&data, &xpr, res, false, crate::texture::DxnExport::Raw)?;
    let img = &levels.first().context("no level")?.image;
    if (img.width as usize, img.height as usize) != (CUBE_SIZE * CUBE_SIZE, CUBE_SIZE) {
        bail!("base cube decodes to {}x{}, expected a 1024x32 strip", img.width, img.height);
    }
    let id = ColourCube::identity();
    let mut matching = 0;
    for b in 0..CUBE_SIZE {
        for g in 0..CUBE_SIZE {
            for r in 0..CUBE_SIZE {
                let at = (g * CUBE_SIZE * CUBE_SIZE + b * CUBE_SIZE + r) * 4;
                let want = id.texels[r + CUBE_SIZE * g + CUBE_SIZE * CUBE_SIZE * b];
                matching += usize::from(img.rgba[at..at + 3] == want);
            }
        }
    }
    Ok((matching, CUBE_SIZE.pow(3)))
}

pub fn write_cube_png(path: &Path, cube: &ColourCube) -> Result<()> {
    let (w, h) = (CUBE_SIZE * CUBE_SIZE, CUBE_SIZE);
    let mut rgba = vec![0u8; w * h * 4];
    for (i, t) in cube.texels.iter().enumerate() {
        let (r, g, b) = (i % CUBE_SIZE, i / CUBE_SIZE % CUBE_SIZE, i / (CUBE_SIZE * CUBE_SIZE));
        let at = (g * w + b * CUBE_SIZE + r) * 4;
        rgba[at..at + 4].copy_from_slice(&[t[0], t[1], t[2], 255]);
    }
    crate::texture::write_png(path, &crate::texture::Image { width: w as u32, height: h as u32, rgba })
}

pub fn run(big_root: &Path, names: &[String], png_dir: Option<&Path>) -> Result<()> {
    let (matching, total) = check_base_cube(big_root)?;
    println!("base cube postfx_colourcube_base.xpr: {matching}/{total} texels = floor(i*255/31)");
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    let drops = drop_postfx_names(big_root)?;
    for n in &drops {
        *counts.entry(n.clone()).or_default() += 1;
    }
    let wanted: Vec<&str> = if names.is_empty() {
        counts.keys().map(String::as_str).collect()
    } else {
        names.iter().map(String::as_str).collect()
    };
    if names.is_empty() {
        println!("{} ge_drops name a post-FX preset; {} distinct", drops.len(), counts.len());
    }
    let built = load_presets(big_root, &wanted)?;
    let mut non_identity = 0;
    for (p, b) in &built {
        let dev = b.cube.max_deviation();
        non_identity += usize::from(dev > 0);
        println!(
            "\n{} ({} drops): chain {} | layer {} | bloom {} blur {:?}",
            p.name,
            counts.get(&p.name).copied().unwrap_or(0),
            p.chain.join(" -> "),
            p.layer.as_deref().unwrap_or("-"),
            if p.bloom_active() { "on" } else { "off" },
            p.bloom_blur()
        );
        for c in &p.colour_cubes {
            println!("  colour cube: stack {} mode {} group {} weight {}", c.stack, c.mode, c.group, c.weight);
        }
        for s in &b.stages {
            println!("  stage {s}");
        }
        let ramp: Vec<[u8; 3]> = [0, 8, 16, 24, 31].iter().map(|&i| b.cube.texels[i * (1 + 32 + 1024)]).collect();
        println!(
            "  LUT: max |delta| to identity {dev}, grey ramp (i=0,8,16,24,31) {ramp:?}{}",
            if b.needs_second_lut { " | NEEDS the depth-blended second LUT (not built)" } else { "" }
        );
        if let Some(dir) = png_dir {
            std::fs::create_dir_all(dir)?;
            write_cube_png(&dir.join(format!("{}_lut.png", p.name)), &b.cube)?;
        }
    }
    println!("\n{} presets resolved, {} LUTs built ({non_identity} differ from identity)", built.len(), built.len());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recovered_field_names_hash_to_their_keys() {
        let known = [
            ("bloom_blur_x", 0xCA95_B073),
            ("bloom_blur_y", 0xCA95_B072),
            ("colourcube_depth_near", 0xFA03_885D),
            ("colourcube_depth_far", 0xF408_EB22),
            ("vignette_colour", 0xAF70_D8F2),
            ("vignette_fadeoffbounds", 0x18EC_F294),
        ];
        for (name, key) in known {
            assert_eq!(fnv1(name), key, "{name}");
            assert!(FIELD_NAMES.contains(&name));
        }
        assert_eq!(fnv1("day_sunny_postfx"), 0xF6F1_7684);
        assert_eq!(CLASS_POSTFX, 0x3D70_D4ED);
    }

    #[test]
    fn blend_replaces_group_opacity() {
        let text = r#"<?xml version="1.0"?>
<stack name="s">
  <group name="DO_NOT_EDIT" enabled="true" opacity="255">
    <layer name="LOAD_PREVIOUS" enabled="true" type="LoadColourCube" blendmode="Normal" opacity="255"/>
  </group>
  <group name="g" enabled="false" opacity="130">
    <layer name="c" enabled="true" type="BrightnessContrast" blendmode="Normal" opacity="255">
      <parameter name="brightness" value="0"/>
      <parameter name="contrast" value="15"/>
    </layer>
  </group>
</stack>"#;
        let xml = parse_stack(text).unwrap();
        assert_eq!(xml.groups[1].layers[0].parameters.len(), 2);
        let stacks = [ColourStack { index: 0, file: PathBuf::new(), xml }];
        let none = build_cube(&stacks, &[]).unwrap();
        assert_eq!(none.cube, ColourCube::identity());
        let blend = ColourCubeBlend { stack: 0, mode: 0, group: "g".into(), weight: 1.0 };
        let full = build_cube(&stacks, &[blend]).unwrap();
        assert!(full.stages[0].ends_with("255)"), "{:?}", full.stages);
        assert!(!full.needs_second_lut);
        let bad = ColourCubeBlend { stack: 0, mode: 0, group: "missing".into(), weight: 1.0 };
        assert!(build_cube(&stacks, &[bad]).is_err());
    }
}
