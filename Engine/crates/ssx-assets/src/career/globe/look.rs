use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context, Result, bail};
use ssx_formats::attrib::collection::{self, FieldData};
use ssx_formats::attrib::{Schema, Value, Vault};

use crate::postfx::{
    CLASS_POSTFX, ColourCube, ColourCubeBlend, ColourStack, CubeBuild, PostFxPreset, build_cube, load_presets,
    parse_stack,
};
use crate::vault_db::fnv1;

pub const FIELD_VIGNETTE_ON: &str = "0x9d897880";
const FIELD_COLOUR_CUBE_STACK: u32 = 0x92F1_952A;

#[derive(Clone, Debug, Default)]
pub struct FePostFx {
    pub default: PostFxPreset,
    pub presets: BTreeMap<String, PostFxPreset>,
    pub stacks: Vec<ColourStack>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct PostFxState {
    pub floats: BTreeMap<String, f32>,
    pub vectors: BTreeMap<String, Vec<f32>>,
    pub cubes: Vec<ColourCubeBlend>,
}

impl FePostFx {
    pub fn load(big_root: &Path, names: &[&str]) -> Result<Self> {
        let mut all: Vec<&str> = vec!["default"];
        all.extend(names.iter().copied().filter(|n| !n.is_empty() && *n != "default"));
        all.sort_unstable();
        all.dedup();
        let mut presets: BTreeMap<String, PostFxPreset> =
            load_presets(big_root, &all)?.into_iter().map(|(p, _)| (p.name.clone(), p)).collect();
        let default = presets.remove("default").context("no postfx `default`")?;
        Ok(FePostFx { default, presets, stacks: load_stacks(big_root)? })
    }

    pub fn state(&self, weights: &[(String, f32)]) -> PostFxState {
        let mut s = PostFxState::default();
        s.blend(&self.default, 1.0);
        for (name, w) in weights {
            if let Some(p) = self.presets.get(name) {
                s.blend(p, *w);
            }
        }
        s
    }

    pub fn cube(&self, state: &PostFxState) -> Result<CubeBuild> {
        build_cube(&self.stacks, &state.cubes)
    }
}

impl PostFxState {
    fn blend(&mut self, p: &PostFxPreset, w: f32) {
        for (k, v) in &p.floats {
            let e = self.floats.entry(k.clone()).or_insert(*v);
            *e += (v - *e) * w;
        }
        for (k, v) in &p.vectors {
            match self.vectors.get_mut(k) {
                Some(acc) => {
                    for (a, b) in acc.iter_mut().zip(v) {
                        *a += (b - *a) * w;
                    }
                }
                None => {
                    self.vectors.insert(k.clone(), v.clone());
                }
            }
        }
        for c in &p.colour_cubes {
            match self.cubes.iter_mut().find(|e| e.stack == c.stack && e.mode == c.mode && e.group == c.group) {
                Some(e) => e.weight += (c.weight - e.weight) * w,
                None => self.cubes.push(ColourCubeBlend { weight: c.weight * w, ..c.clone() }),
            }
        }
    }

    fn f(&self, name: &str) -> f32 {
        self.floats.get(name).copied().unwrap_or(0.0)
    }

    pub fn bloom(&self) -> bool {
        self.f(&format!("{:#010x}", crate::postfx::FIELD_BLOOM_ACTIVE)) > 0.001
    }

    pub fn blur(&self) -> [f32; 2] {
        [self.f("bloom_blur_x"), self.f("bloom_blur_y")]
    }

    pub fn vignette(&self) -> Option<([f32; 3], [f32; 2])> {
        if self.f(FIELD_VIGNETTE_ON) <= 0.001 {
            return None;
        }
        let c = self.vectors.get("vignette_colour").cloned().unwrap_or_default();
        let b = self.vectors.get("vignette_fadeoffbounds").cloned().unwrap_or_default();
        let at = |v: &Vec<f32>, i: usize| v.get(i).copied().unwrap_or(0.0);
        Some(([at(&c, 0), at(&c, 1), at(&c, 2)], [at(&b, 0), at(&b, 1)]))
    }

    pub fn final_modifier(&self) -> [f32; 2] {
        let (black, white) = (self.f("0x2dd8d98e"), self.f("0xf1c5c1fc"));
        if black > 0.001 {
            [1.0 - black, 0.0]
        } else if white > 0.001 {
            [1.0 - white, white]
        } else {
            [1.0, 0.0]
        }
    }
}

fn load_stacks(big_root: &Path) -> Result<Vec<ColourStack>> {
    let dir = big_root.join("bootcache/data/xenon/attribdb");
    let read = |n: &str| -> Result<(Vec<u8>, Vec<u8>)> {
        let vlt = dir.join(n);
        let bin = vlt.with_extension("bin");
        Ok((
            std::fs::read(&vlt).with_context(|| format!("reading {}", vlt.display()))?,
            std::fs::read(&bin).with_context(|| format!("reading {}", bin.display()))?,
        ))
    };
    let (svlt, sbin) = read("attribdb_schema.vlt")?;
    let schema = Schema::from_vault(&Vault::parse(&svlt, &sbin)?).context("reading the schema")?;
    let (vlt, bin) = read("attribdb.vlt")?;
    let vault = Vault::parse(&vlt, &bin)?;
    let cols = collection::collections(&vault)?;
    let index = collection::index(&cols);
    let i = *index.get(&(CLASS_POSTFX, fnv1("default"))).context("no postfx `default`")?;
    let decoded = cols[i].decode(&vault, &schema)?;
    let field = decoded.field(FIELD_COLOUR_CUBE_STACK).context("`default` without a ColourCubeStack")?;
    let values: Vec<&Value> = match &field.data {
        FieldData::Single(v) => vec![v],
        FieldData::Array(vs) => vs.iter().collect(),
    };
    let files = big_root.join("memresident/data/common/postfx/colourcube");
    let mut out = Vec::new();
    for v in values {
        let Value::Raw { bytes, pointers } = v else { bail!("ColourCubeStack is not a raw value") };
        if bytes.len() != 8 {
            bail!("ColourCubeStack entry of {} bytes", bytes.len());
        }
        let p = pointers.iter().find(|p| p.offset == 4).context("ColourCubeStack entry without a name")?;
        let name = vault.cstr(p.target)?;
        let rel = format!("data/common/postfx/colourcube/{name}.xml");
        let file = crate::data_roots::find(big_root, &rel).unwrap_or_else(|| files.join(format!("{name}.xml")));
        let text = std::fs::read_to_string(&file).with_context(|| format!("reading {}", file.display()))?;
        let xml = parse_stack(&text).with_context(|| format!("parsing {}", file.display()))?;
        let index = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
        out.push(ColourStack { index, file, xml });
    }
    out.sort_by_key(|s| s.index);
    Ok(out)
}

pub fn identity_cube() -> ColourCube {
    ColourCube::identity()
}
