use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use ssx_content::model::Model;
use ssx_content::{Asset, BASE_NAMESPACE, ContentId, Kind, LoadError, Source};

use super::textures::{GAME_EXPLORER, game_archives, game_files};
use super::vaults::rel;
use crate::object_mesh;

pub fn model_id(rel: &str) -> Option<ContentId> {
    let p = rel.replace('\\', "/");
    let p = p.strip_suffix(".geom").unwrap_or(&p);
    ContentId::new(BASE_NAMESPACE, Kind::Model, &super::path_segments(p)?).ok()
}

pub struct DiscModels {
    files: BTreeMap<ContentId, PathBuf>,
    pub collisions: Vec<(ContentId, Vec<PathBuf>)>,
}

impl DiscModels {
    pub fn index(big_root: &Path) -> Result<Self> {
        Self::index_roots(game_archives(big_root)?)
    }

    pub fn index_archive(root: &Path) -> Result<Self> {
        Self::index_roots(vec![root.to_path_buf()])
    }

    fn index_roots(roots: Vec<PathBuf>) -> Result<Self> {
        let mut found: BTreeMap<ContentId, BTreeMap<String, PathBuf>> = BTreeMap::new();
        for a in roots {
            let ge = a.join(GAME_EXPLORER);
            for f in game_files(&ge, "geom")? {
                let below = rel(&ge, &f);
                if let Some(id) = model_id(&below) {
                    found.entry(id).or_default().entry(below).or_insert(f);
                }
            }
        }
        let mut files = BTreeMap::new();
        let mut collisions = Vec::new();
        for (id, mut copies) in found {
            if copies.len() == 1 {
                if let Some((_, f)) = copies.pop_first() {
                    files.insert(id, f);
                }
            } else {
                collisions.push((id, copies.into_values().collect()));
            }
        }
        Ok(DiscModels { files, collisions })
    }

    pub fn file(&self, id: &ContentId) -> Option<&Path> {
        self.files.get(id).map(PathBuf::as_path)
    }

    pub fn len(&self) -> usize {
        self.files.len()
    }

    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    pub fn decode(&self, id: &ContentId) -> Result<Model, LoadError> {
        let path = self.files.get(id).ok_or_else(|| LoadError::Missing(id.clone()))?;
        let mesh =
            object_mesh::load(path).map_err(|e| LoadError::Failed { id: id.clone(), message: format!("{e:#}") })?;
        Ok(Model::single(mesh))
    }
}

impl Source for DiscModels {
    fn name(&self) -> &str {
        BASE_NAMESPACE
    }

    fn has(&self, id: &ContentId) -> bool {
        self.files.contains_key(id)
    }

    fn load(&self, id: &ContentId) -> Result<Asset, LoadError> {
        Ok(Asset::Value(Arc::new(self.decode(id)?)))
    }

    fn list(&self, kind: Kind) -> Vec<ContentId> {
        if kind == Kind::Model { self.files.keys().cloned().collect() } else { Vec::new() }
    }
}

pub fn rider_mesh(big_root: &Path, stored: &str) -> Result<crate::object_mesh::ObjectMesh> {
    let content = super::GameContent::of(big_root);
    let id = model_id(stored).filter(|id| content.has(id)).with_context(|| format!("{stored} not found"))?;
    let found = content.model(&id, super::resolve::ModelUse::Rider)?;
    let (mesh, _) = found.value.lod0_mesh().map_err(|e| anyhow::anyhow!("{id}: {e}"))?;
    Ok(mesh)
}
