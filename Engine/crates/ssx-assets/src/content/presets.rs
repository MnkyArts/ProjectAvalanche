use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, OnceLock};

use anyhow::{Context, Result};
use ssx_content::lighting::{FieldType, LightingRecord, misfit};
use ssx_content::postfx::{Blend, PostFxRecord};
use ssx_content::{Asset, ContentId, Kind, Layer};

use super::store::GameContent;
use super::textures::texture_id;
use crate::particles::ParticleNode;
use crate::postfx::{ColourCube, ColourCubeBlend, CubeBuild, PostFxPreset};
use crate::track_lighting::LightingPreset;
use crate::vault_db::{fnv1, with_attribdb};

pub(super) const CLASS_LIGHTING: u32 = fnv1("ge_globallightingparams");

pub fn preset_texture_id(stored: &str) -> Option<ContentId> {
    match stored.parse::<ContentId>() {
        Ok(id) if id.kind() == Kind::Texture => Some(id),
        _ => texture_id(stored),
    }
}

pub fn lighting_record(p: &LightingPreset) -> Result<LightingRecord> {
    let mut textures = BTreeMap::new();
    for (k, path) in &p.textures {
        let id = preset_texture_id(path).with_context(|| format!("{}: {k} {path:?} forms no texture id", p.name))?;
        textures.insert(k.clone(), id);
    }
    Ok(LightingRecord {
        floats: p.floats.clone(),
        vectors: p.vectors.clone(),
        textures,
        sky: p.sky.clone(),
        sun_node: p.sun_node.as_ref().map(|n| n.key),
    })
}

pub(super) fn disc_lighting_record(big_root: &Path, key: u32) -> Result<Asset> {
    let p = with_attribdb(big_root, |db| crate::track_lighting::preset(db, key, None))?;
    Ok(Asset::Record(lighting_record(&p)?.to_table()))
}

pub fn preset_from_record(
    base: &LightingPreset,
    r: &LightingRecord,
    node: &dyn Fn(u32) -> Result<ParticleNode>,
) -> Result<LightingPreset> {
    let textures = r
        .textures
        .iter()
        .map(|(k, id)| {
            let kept = base.textures.get(k).filter(|path| preset_texture_id(path).as_ref() == Some(id));
            (k.clone(), kept.cloned().unwrap_or_else(|| id.to_string()))
        })
        .collect();
    let sun_node = match (r.sun_node, &base.sun_node) {
        (None, _) => None,
        (Some(k), Some(n)) if n.key == k => Some(n.clone()),
        (Some(k), _) => Some(node(k).with_context(|| format!("sun_node 0x{k:08x}"))?),
    };
    Ok(LightingPreset {
        name: base.name.clone(),
        floats: r.floats.clone(),
        vectors: r.vectors.clone(),
        textures,
        sun_node,
        sky: r.sky.clone(),
    })
}

pub(super) fn disc_postfx_record(big_root: &Path, key: u32, name: &str) -> Result<Asset> {
    let p = crate::postfx::load_preset_by_key(big_root, key, name)?;
    Ok(Asset::Record(postfx_record(&p).to_table()))
}

pub fn postfx_record(p: &PostFxPreset) -> PostFxRecord {
    PostFxRecord {
        floats: p.floats.clone(),
        vectors: p.vectors.clone(),
        layer: p.layer.clone(),
        colour_cubes: p
            .colour_cubes
            .iter()
            .map(|b| Blend { stack: b.stack, mode: b.mode, group: b.group.clone(), weight: b.weight })
            .collect(),
        cube: None,
    }
}

#[derive(Default)]
pub struct PresetCache {
    schema: OnceLock<Result<Arc<BTreeMap<String, FieldType>>, String>>,
}

fn lighting_schema(big_root: &Path) -> Result<BTreeMap<String, FieldType>> {
    with_attribdb(big_root, |db| {
        let mut out = BTreeMap::new();
        let keys: Vec<u32> = db.collections_of_all(CLASS_LIGHTING).into_iter().map(|(k, _)| k).collect();
        for key in keys {
            let p = crate::track_lighting::preset(db, key, None)?;
            for k in p.floats.keys() {
                out.insert(k.clone(), FieldType::Float);
            }
            for k in p.vectors.keys() {
                out.insert(k.clone(), FieldType::Vector4);
            }
            for k in p.textures.keys() {
                out.insert(k.clone(), FieldType::Texture);
            }
        }
        Ok(out)
    })
}

impl GameContent {
    pub fn lighting_schema(&self) -> Result<Arc<BTreeMap<String, FieldType>>, String> {
        self.presets
            .schema
            .get_or_init(|| lighting_schema(self.big_root()).map(Arc::new).map_err(|e| format!("{e:#}")))
            .clone()
    }

    pub fn packages_provide(&self, kind: Kind) -> bool {
        self.with_registry(|r| super::store::provides_kind(r, kind))
    }

    pub(in crate::content) fn routed_record(
        &self,
        id: &ContentId,
        disc: toml::Table,
    ) -> Result<(toml::Table, String, Layer), String> {
        self.with_registry(|r| {
            let winner = r.winner(id).ok_or_else(|| format!("{id}: no source provides it"))?;
            let patches = r.patches_of(id).map_err(|e| e.to_string())?;
            let by: Vec<&str> = patches.iter().map(|(n, _)| n.as_str()).filter(|n| *n != winner.name).collect();
            let described = |source: &str| {
                if by.is_empty() { source.to_string() } else { format!("{source}, patched by {}", by.join(", ")) }
            };
            if winner.layer == Layer::Base {
                let mut t = disc;
                for (_, p) in &patches {
                    ssx_content::patch::merge(&mut t, p);
                }
                Ok((t, described(winner.name), winner.layer))
            } else {
                let got = r.load(id).map_err(|e| e.to_string())?;
                let t =
                    got.asset.record().cloned().ok_or_else(|| format!("{id}: the winner's asset is not a record"))?;
                Ok((t, described(&got.source), got.layer))
            }
        })
    }

    pub fn route_lighting(&self, key: u32, disc: LightingPreset) -> LightingPreset {
        if !self.packages_provide(Kind::Lighting) {
            return disc;
        }
        let Some(id) = self.disc_records().and_then(|r| r.names().ok()).and_then(|n| n.lighting(key).cloned()) else {
            return disc;
        };
        let changed = self.with_registry(|r| {
            r.winner(&id).is_some_and(|w| w.layer > Layer::Base)
                || r.patches_of(&id).map(|p| !p.is_empty()).unwrap_or(true)
        });
        if !changed {
            return disc;
        }
        match self.lighting_from_registry(&id, &disc) {
            Ok((p, source)) => {
                self.info(format!("content: {id}: the lighting preset from {source}"));
                p
            }
            Err(why) => {
                self.notice(format!("content: {id} not used ({why}); the disc's preset is"));
                disc
            }
        }
    }

    fn lighting_from_registry(
        &self,
        id: &ContentId,
        disc: &LightingPreset,
    ) -> Result<(LightingPreset, String), String> {
        let base = lighting_record(disc).map_err(|e| format!("{e:#}"))?.to_table();
        let (mut table, source, _) = self.routed_record(id, base.clone())?;
        if self.sim_policy() != ssx_content::policy::ClientMods::Any {
            let (sim, _, _) = self.sim_record(id, base)?;
            for k in super::data::LIGHTING_SIM_FIELDS {
                match sim.get(k) {
                    Some(v) => table.insert(k.to_string(), v.clone()),
                    None => table.remove(k),
                };
            }
        }
        let record = LightingRecord::from_table(&table).map_err(|ps| ps.join("; "))?;
        let schema = self.lighting_schema()?;
        let mut own: BTreeMap<String, FieldType> = schema.as_ref().clone();
        for (k, ty) in lighting_record(disc).map_err(|e| format!("{e:#}"))?.fields() {
            own.insert(k.to_string(), ty);
        }
        let problems: Vec<String> = record.fields().filter_map(|(k, ty)| misfit(k, ty, &own)).collect();
        if !problems.is_empty() {
            return Err(problems.join("; "));
        }
        let big = self.big_root().to_path_buf();
        let node =
            move |k: u32| -> Result<ParticleNode> { with_attribdb(&big, |db| crate::particles::particle_node(db, k)) };
        let p = preset_from_record(disc, &record, &node).map_err(|e| format!("{e:#}"))?;
        Ok((p, source))
    }

    pub fn route_postfx(&self, disc: PostFxPreset, build: CubeBuild) -> (PostFxPreset, CubeBuild) {
        if !self.packages_provide(Kind::Postfx) {
            return (disc, build);
        }
        let Some(id) =
            super::segment(&disc.name).and_then(|s| ContentId::new(ssx_content::BASE_NAMESPACE, Kind::Postfx, &s).ok())
        else {
            return (disc, build);
        };
        let named = self.disc_records().and_then(|r| r.names().ok()).is_some_and(|n| n.by_id.contains_key(&id));
        let changed = named
            && self.with_registry(|r| {
                r.winner(&id).is_some_and(|w| w.layer > Layer::Base)
                    || r.patches_of(&id).map(|p| !p.is_empty()).unwrap_or(true)
            });
        if !changed {
            return (disc, build);
        }
        match self.postfx_from_registry(&id, &disc, &build) {
            Ok((p, b, source)) => {
                self.info(format!("content: {id}: the post-FX preset from {source}"));
                (p, b)
            }
            Err(why) => {
                self.notice(format!("content: {id} not used ({why}); the disc's preset is"));
                (disc, build)
            }
        }
    }

    fn postfx_from_registry(
        &self,
        id: &ContentId,
        disc: &PostFxPreset,
        build: &CubeBuild,
    ) -> Result<(PostFxPreset, CubeBuild, String), String> {
        let (table, source, _) = self.routed_record(id, postfx_record(disc).to_table())?;
        let r = PostFxRecord::from_table(&table).map_err(|ps| ps.join("; "))?;
        let colour_cubes: Vec<ColourCubeBlend> = r
            .colour_cubes
            .iter()
            .map(|b| ColourCubeBlend { stack: b.stack, mode: b.mode, group: b.group.clone(), weight: b.weight })
            .collect();
        let preset = PostFxPreset {
            name: disc.name.clone(),
            chain: disc.chain.clone(),
            layer: r.layer.clone(),
            floats: r.floats.clone(),
            vectors: r.vectors.clone(),
            colour_cubes: colour_cubes.clone(),
        };
        let build = match &r.cube {
            Some(cube) => {
                let texels = self.cube_strip(cube)?;
                CubeBuild {
                    cube: ColourCube { texels },
                    stages: vec![format!("the cube {cube}")],
                    needs_second_lut: false,
                }
            }
            None if colour_cubes == disc.colour_cubes => build.clone(),
            None => crate::postfx::build_cube_for(self.big_root(), &colour_cubes).map_err(|e| format!("{e:#}"))?,
        };
        Ok((preset, build, source))
    }

    pub fn cube_strip(&self, id: &ContentId) -> Result<Vec<[u8; 3]>, String> {
        let (file, got) = self.with_registry(|r| (r.providers(id).next().and_then(|(_, s)| s.file_of(id)), r.load(id)));
        let got = got.map_err(|e| e.to_string())?;
        let bytes = got.asset.bytes().ok_or_else(|| format!("{id}: not a PNG strip (a .png entry)"))?;
        let file = file.ok_or_else(|| format!("{id}: not a PNG strip (a .png entry)"))?;
        ssx_content::postfx::cube_from_strip(&file, bytes)
    }
}
