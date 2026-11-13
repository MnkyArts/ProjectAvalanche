use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::Instant;

use ssx_content::material::{Family, MaterialDef, TextureRef};
use ssx_content::object::{MeshRecord, ObjectCollision, ObjectDef, ObjectLods};
use ssx_content::{Asset, BASE_NAMESPACE, ContentId, Kind, LoadError, Source};

use super::models::{DiscModels, model_id};
use super::records::{CLASS_KEYS, record_ids};
use super::textures::texture_id;
use crate::object_collision::load_pob;
use crate::track_objects::{CLASS_GAMEOBJECT, GameObject, asset_file, game_object};
use crate::vault_db::{CLASS_MATERIAL, Material as VaultMaterial, with_attribdb, with_local_attribdb};

pub struct RecordNames {
    pub by_key: HashMap<(u32, u32), ContentId>,
    pub by_id: BTreeMap<ContentId, (u32, u32)>,
    pub seconds: f64,
}

impl RecordNames {
    pub fn object(&self, key: u32) -> Option<&ContentId> {
        self.by_key.get(&(CLASS_GAMEOBJECT, key))
    }

    pub fn material(&self, key: u32) -> Option<&ContentId> {
        self.by_key.get(&(CLASS_MATERIAL, key))
    }

    pub fn lighting(&self, key: u32) -> Option<&ContentId> {
        self.by_key.get(&(super::presets::CLASS_LIGHTING, key))
    }
}

pub fn is_record_kind(kind: Kind) -> bool {
    matches!(kind, Kind::Object | Kind::Material | Kind::Lighting | Kind::Postfx)
}

pub fn script_id(stored: &str) -> Option<ContentId> {
    let p = stored.replace('\\', "/").to_ascii_lowercase();
    let p = p.strip_suffix(".lua").unwrap_or(&p);
    ContentId::new(BASE_NAMESPACE, Kind::Script, &super::path_segments(p)?).ok()
}

pub fn material_def(m: &VaultMaterial) -> MaterialDef {
    let effect = m.effect.clone().unwrap_or_default();
    let textures = m
        .textures
        .iter()
        .map(|(p, path)| {
            let r = texture_id(path).map_or_else(|| TextureRef::Path(path.clone()), TextureRef::Id);
            (p.clone(), r)
        })
        .collect();
    MaterialDef {
        name: m.name.clone(),
        family: Family::of_effect(&effect),
        effect,
        technique: m.technique.clone(),
        textures,
        floats: m.floats.clone(),
        vectors: m.vectors.clone(),
        double_sided: None,
        base_colour: None,
        shader: None,
    }
}

pub fn object_def(o: &GameObject, names: &RecordNames, big_root: &Path) -> anyhow::Result<ObjectDef> {
    let lods = o
        .lods
        .iter()
        .map(|lod| {
            lod.iter()
                .map(|m| MeshRecord {
                    geometry: m.geometry.clone(),
                    model: model_id(&m.geometry),
                    material: (m.material_key != 0).then(|| names.material(m.material_key).cloned()).flatten(),
                    skeleton: m.skeleton.clone(),
                })
                .collect()
        })
        .collect();
    let (collision, rigid_body) = match o.pob.as_ref().map(|p| (p, asset_file(big_root, p))) {
        Some((path, Some(file))) => {
            let (c, body) = load_pob(&file)?;
            (ObjectCollision::Decoded { path: path.clone(), collision: Arc::new(c) }, body)
        }
        _ => (ObjectCollision::None, None),
    };
    Ok(ObjectDef {
        name: o.name.clone(),
        lod_thresholds: o.lod_thresholds.clone(),
        lods: ObjectLods::Records(lods),
        collision,
        controller: o.controller.as_deref().and_then(script_id),
        rigid_body,
        audio: o.audio.clone(),
    })
}

pub struct DiscRecords {
    big_root: PathBuf,
    assets_root: PathBuf,
    _models: Arc<DiscModels>,
    names: OnceLock<Result<Arc<RecordNames>, String>>,
}

impl DiscRecords {
    pub fn new(big_root: &Path, models: Arc<DiscModels>) -> Self {
        Self::with_assets(big_root, big_root, models)
    }

    pub fn with_assets(vault_root: &Path, assets_root: &Path, models: Arc<DiscModels>) -> Self {
        DiscRecords {
            big_root: vault_root.to_path_buf(),
            assets_root: assets_root.to_path_buf(),
            _models: models,
            names: OnceLock::new(),
        }
    }

    pub fn usable(&self) -> Result<std::collections::BTreeMap<u32, usize>, String> {
        if !self.big_root.join("bootcache/data/xenon/attribdb/ge_attribdb.vlt").is_file() {
            return Err(format!("{}: no bootcache ge_attribdb.vlt", self.big_root.display()));
        }
        self.names()?;
        with_local_attribdb(&self.big_root, |db| Ok(db.unknown.clone())).map_err(|e| format!("{e:#}"))
    }

    pub fn names(&self) -> Result<Arc<RecordNames>, String> {
        self.names
            .get_or_init(|| {
                let start = Instant::now();
                let by_key = record_ids(&self.big_root, &CLASS_KEYS).map_err(|e| format!("{e:#}"))?;
                let by_id = by_key.iter().map(|(k, id)| (id.clone(), *k)).collect();
                let seconds = start.elapsed().as_secs_f64();
                Ok(Arc::new(RecordNames { by_key: by_key.into_iter().collect(), by_id, seconds }))
            })
            .clone()
    }

    pub fn named(&self) -> bool {
        self.names.get().is_some()
    }

    fn key_of(&self, id: &ContentId) -> Result<(u32, u32), LoadError> {
        let names = self.names().map_err(|message| LoadError::Failed { id: id.clone(), message })?;
        names.by_id.get(id).copied().ok_or_else(|| LoadError::Missing(id.clone()))
    }

    fn decode(&self, id: &ContentId) -> Result<Asset, LoadError> {
        let (class, key) = self.key_of(id)?;
        let names = self.names().map_err(|message| LoadError::Failed { id: id.clone(), message })?;
        let failed = |e: anyhow::Error| LoadError::Failed { id: id.clone(), message: format!("{e:#}") };
        match id.kind() {
            Kind::Lighting => return super::presets::disc_lighting_record(&self.big_root, key).map_err(failed),
            Kind::Postfx => return super::presets::disc_postfx_record(&self.big_root, key, id.path()).map_err(failed),
            _ => {}
        }
        with_attribdb(&self.big_root, |db| {
            if class == CLASS_MATERIAL {
                let mut def = material_def(&db.material(class, key)?);
                def.name = id.path().to_string();
                return Ok(Asset::Value(Arc::new(def)));
            }
            let o = game_object(db, class, key)?;
            let mut def = object_def(&o, &names, &self.assets_root)?;
            def.name = id.path().to_string();
            Ok(Asset::Value(Arc::new(def)))
        })
        .map_err(failed)
    }
}

impl Source for DiscRecords {
    fn name(&self) -> &str {
        BASE_NAMESPACE
    }

    fn has(&self, id: &ContentId) -> bool {
        is_record_kind(id.kind()) && self.key_of(id).is_ok()
    }

    fn load(&self, id: &ContentId) -> Result<Asset, LoadError> {
        self.decode(id)
    }

    fn list(&self, kind: Kind) -> Vec<ContentId> {
        if !is_record_kind(kind) {
            return Vec::new();
        }
        match self.names() {
            Ok(n) => n.by_id.keys().filter(|id| id.kind() == kind).cloned().collect(),
            Err(_) => Vec::new(),
        }
    }
}
