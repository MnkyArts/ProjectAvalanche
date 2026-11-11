pub mod geotags;
pub mod tombstones;

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use ssx_formats::attrib::{FieldData, Value};
use ssx_rules::locators;

use crate::vault_db::{Db, Material, fnv1, refs, single, single_text, with_track_db};

const CLASS_SCENE_INSTANCE: u32 = fnv1("ge_scene_instance");
const CLASS_GEOMETRY: u32 = fnv1("ge_geometry");
const CLASS_MATERIAL: u32 = fnv1("ge_material");
const FIELD_INSTANCE_DATA: u32 = 0xEE66_32AC;
const FIELD_GAMEOBJECT: u32 = 0x3ED7_B5CE;
const FIELD_MODELS: u32 = 0xB43A_3598;
const FIELD_LOD_THRESHOLDS: u32 = fnv1("LODThresholds");
const FIELD_MESHES: u32 = fnv1("Meshes");
const FIELD_MATERIAL: u32 = fnv1("Material");
const FIELD_GEOMETRY: u32 = fnv1("Geometry");
const FIELD_SKELETON: u32 = fnv1("SkeletonFileRef");
const FIELD_CONTROLLER: u32 = fnv1("Controller");
const FIELD_POB: u32 = fnv1("POBFileRef");
const FIELD_AUDIO_DATA: u32 = 0x7DCA_51D1;
const FIELD_AUDIO_TYPE: u32 = 0x8403_C6C2;
const FIELD_AUDIO_SUBTYPE: u32 = 0x83F8_6C09;

pub use ssx_rules::placement::Placement;

#[derive(Clone, Debug)]
pub struct ObjectMesh {
    pub geometry: String,
    pub skeleton: Option<String>,
    pub material: Material,
    pub material_key: u32,
}

#[derive(Clone, Debug, Default)]
pub struct GameObject {
    pub name: String,
    pub key: u32,
    pub lod_thresholds: Vec<f32>,
    pub lods: Vec<Vec<ObjectMesh>>,
    pub has_controller: bool,
    pub controller: Option<String>,
    pub pob: Option<String>,
    pub audio: Option<AudioName>,
}

pub use ssx_content::object::AudioName;

#[derive(Clone, Debug, Default)]
pub struct TrackObjects {
    pub objects: Vec<GameObject>,
    pub instances: Vec<(usize, Vec<Placement>)>,
    pub selection: Option<SceneSelection>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SceneSelection {
    pub drop_key: u32,
    pub total: usize,
    pub kept_track: usize,
    pub kept_drop: usize,
    pub skipped: Vec<u32>,
    pub fallback: Option<String>,
}

impl SceneSelection {
    pub fn kept(&self) -> usize {
        self.total - self.skipped.len()
    }

    pub fn summary(&self) -> String {
        const SHOWN: usize = 12;
        if let Some(reason) = &self.fallback {
            return format!("objects: drop {:#010x} keeps all {} scene instances: {reason}", self.drop_key, self.total);
        }
        let mut list: Vec<String> = self.skipped.iter().take(SHOWN).map(|k| format!("{k:#010x}")).collect();
        if self.skipped.len() > SHOWN {
            list.push(format!("… {} more", self.skipped.len() - SHOWN));
        }
        let list = if list.is_empty() { "none".to_string() } else { list.join(", ") };
        format!(
            "objects: drop {:#010x} keeps {} of {} scene instances ({} track + {} drop); skipped: {list}",
            self.drop_key,
            self.kept(),
            self.total,
            self.kept_track,
            self.kept_drop
        )
    }
}

pub fn select_scene_instances(
    drop_key: u32,
    all: &[u32],
    track: Result<Vec<u32>>,
    drop: Result<Vec<u32>>,
) -> (Option<BTreeSet<u32>>, SceneSelection) {
    let mut selection = SceneSelection { drop_key, total: all.len(), ..Default::default() };
    let (track, drop) = match (track, drop) {
        (Err(e), _) => {
            selection.fallback = Some(format!("the track's scene instances are unresolved ({e:#})"));
            return (None, selection);
        }
        (Ok(t), Err(e)) => {
            selection.fallback =
                Some(format!("the drop's scene instances are unresolved ({e:#}); track has {}", t.len()));
            return (None, selection);
        }
        (Ok(t), Ok(d)) => (t.into_iter().collect::<BTreeSet<u32>>(), d.into_iter().collect::<BTreeSet<u32>>()),
    };
    if track.is_empty() && drop.is_empty() {
        selection.fallback = Some("neither the track nor the drop lists any".to_string());
        return (None, selection);
    }
    let keep: BTreeSet<u32> = track.union(&drop).copied().collect();
    for &key in all {
        if track.contains(&key) {
            selection.kept_track += 1;
        } else if drop.contains(&key) {
            selection.kept_drop += 1;
        } else {
            selection.skipped.push(key);
        }
    }
    (Some(keep), selection)
}

impl TrackObjects {
    pub fn placement_count(&self) -> usize {
        self.instances.iter().map(|(_, p)| p.len()).sum()
    }
}

pub fn asset_file(big_root: &Path, rel: &str) -> Option<PathBuf> {
    let rel = rel.replace('\\', "/");
    std::fs::read_dir(big_root)
        .ok()?
        .flatten()
        .map(|e| e.path().join("data/xenon/gameexplorer").join(&rel))
        .find(|p| p.is_file())
}

pub fn load_track_objects(big_root: &Path, track: &str, drop_key: Option<u32>) -> Result<TrackObjects> {
    with_track_db(big_root, track, |db| {
        let mut out = TrackObjects::default();
        let mut by_key: HashMap<u32, usize> = HashMap::new();
        let all: Vec<u32> = db.collections_of(0, CLASS_SCENE_INSTANCE).map(|(key, _)| key).collect();
        let keep = drop_key.and_then(|drop_key| {
            let keys = |list: Vec<(u32, u32)>| list.into_iter().map(|(_, key)| key).collect();
            let (keep, selection) = select_scene_instances(
                drop_key,
                &all,
                locators::track_scene_instances(db, track).map(keys),
                locators::drop_scene_instances(db, drop_key).map(keys),
            );
            out.selection = Some(selection);
            keep
        });
        for key in all {
            if keep.as_ref().is_some_and(|keep| !keep.contains(&key)) {
                continue;
            }
            let fields = db.fields(CLASS_SCENE_INSTANCE, key)?;
            let reference = |k: u32| fields.iter().find(|f| f.key == k).and_then(|f| refs(&f.data).first().copied());
            let (Some(data), Some(obj)) = (reference(FIELD_INSTANCE_DATA), reference(FIELD_GAMEOBJECT)) else {
                continue;
            };
            if data.collection == 0 || obj.collection == 0 {
                continue;
            }
            let placements = match db.field(data.class, data.collection, FIELD_INSTANCE_DATA)? {
                Some(f) => decode_placements(&f.data),
                None => Vec::new(),
            };
            let idx = match by_key.get(&obj.collection) {
                Some(&i) => i,
                None => {
                    let o = game_object(db, obj.class, obj.collection)
                        .with_context(|| format!("game object {}", db.collection_name(obj.collection)))?;
                    out.objects.push(o);
                    by_key.insert(obj.collection, out.objects.len() - 1);
                    out.objects.len() - 1
                }
            };
            out.instances.push((idx, placements));
        }
        Ok(out)
    })
}

fn decode_placements(d: &FieldData) -> Vec<Placement> {
    let vals: Vec<&Value> = match d {
        FieldData::Single(v) => vec![v],
        FieldData::Array(vs) => vs.iter().collect(),
    };
    vals.into_iter()
        .filter_map(|v| if let Value::Raw { bytes, .. } = v { Placement::decode(bytes) } else { None })
        .collect()
}

pub fn game_object(db: &Db<'_>, class: u32, key: u32) -> Result<GameObject> {
    let mut o = GameObject { name: db.collection_name(key), key, ..Default::default() };
    for f in db.fields(class, key)? {
        match f.key {
            FIELD_LOD_THRESHOLDS => {
                if let FieldData::Array(vs) = &f.data {
                    o.lod_thresholds =
                        vs.iter().filter_map(|v| if let Value::Float(x) = v { Some(*x) } else { None }).collect();
                }
            }
            FIELD_MODELS => {
                for m in refs(&f.data).into_iter().filter(|r| r.collection != 0) {
                    o.lods.push(model_meshes(db, m.class, m.collection)?);
                }
            }
            FIELD_CONTROLLER => {
                o.has_controller = refs(&f.data).iter().any(|r| r.collection != 0);
                if let Some(r) = refs(&f.data).into_iter().find(|r| r.collection != 0)
                    && db.find(r.class, r.collection).is_some()
                {
                    o.controller = db.text_of(r.class, r.collection)?;
                }
            }
            FIELD_POB => o.pob = single_text(&f.data).filter(|s| !s.is_empty()),
            FIELD_AUDIO_DATA => {
                if let Some(r) = refs(&f.data).into_iter().find(|r| r.collection != 0) {
                    let text = |k: u32| -> Result<String> {
                        Ok(db.field(r.class, r.collection, k)?.and_then(|f| single_text(&f.data)).unwrap_or_default())
                    };
                    let type_name = text(FIELD_AUDIO_TYPE)?;
                    if !type_name.is_empty() {
                        o.audio = Some(AudioName { type_name, sub_type: text(FIELD_AUDIO_SUBTYPE)? });
                    }
                }
            }
            _ => {}
        }
    }
    Ok(o)
}

fn model_meshes(db: &Db<'_>, class: u32, key: u32) -> Result<Vec<ObjectMesh>> {
    let mut meshes = Vec::new();
    let Some(field) = db.field(class, key, FIELD_MESHES)? else { return Ok(meshes) };
    for r in refs(&field.data).into_iter().filter(|r| r.collection != 0) {
        let fields = db.fields(r.class, r.collection)?;
        let get = |k: u32| fields.iter().find(|f| f.key == k);
        let geometry = get(FIELD_GEOMETRY)
            .and_then(|f| single(&f.data))
            .and_then(|v| if let Value::Ref(g) = v { Some(*g) } else { None })
            .filter(|g| g.class == CLASS_GEOMETRY && g.collection != 0);
        let Some(g) = geometry else { continue };
        let Some(path) = db.text_of(g.class, g.collection)? else { continue };
        let (material, material_key) = match get(FIELD_MATERIAL).and_then(|f| single(&f.data)) {
            Some(Value::Ref(m)) if m.class == CLASS_MATERIAL && m.collection != 0 => {
                (db.material(m.class, m.collection)?, m.collection)
            }
            _ => (Material::default(), 0),
        };
        let skeleton = get(FIELD_SKELETON).and_then(|f| single_text(&f.data)).filter(|s| !s.is_empty());
        meshes.push(ObjectMesh { geometry: path, skeleton, material, material_key });
    }
    Ok(meshes)
}

const CLASS_TRACK: u32 = fnv1("ge_track");
pub const CLASS_GAMEOBJECT: u32 = fnv1("ge_scene_gameobject");
const FIELD_TRACK_MG_SCALE: u32 = 0xA874_5E3A;
const FIELD_TRACK_MG_POSITION: u32 = 0x0EAB_1CDD;
const FIELD_TRACK_MG_ROTATION: u32 = 0x5ACF_C16A;
const FIELD_TRACK_MG_OBJECT: u32 = 0xED33_083C;

#[derive(Clone, Debug)]
pub struct TrackMidground {
    pub scale: [f32; 3],
    pub position: [f32; 3],
    pub rotation: [f32; 4],
    pub object: GameObject,
}

pub fn load_track_midground(big_root: &Path, track: &str) -> Result<Option<TrackMidground>> {
    with_track_db(big_root, track, |db| {
        let key = fnv1(&format!("{track}_track"));
        let fields = db.fields(CLASS_TRACK, key).with_context(|| format!("ge_track {track}_track"))?;
        let vector = |k: u32| match fields.iter().find(|f| f.key == k).and_then(|f| single(&f.data)) {
            Some(Value::Vector4(v)) => Some(*v),
            _ => None,
        };
        let object = fields
            .iter()
            .find(|f| f.key == FIELD_TRACK_MG_OBJECT)
            .and_then(|f| refs(&f.data).first().copied())
            .filter(|r| r.class == CLASS_GAMEOBJECT && r.collection != 0);
        let (Some(scale), Some(position), Some(rotation), Some(object)) =
            (vector(FIELD_TRACK_MG_SCALE), vector(FIELD_TRACK_MG_POSITION), vector(FIELD_TRACK_MG_ROTATION), object)
        else {
            return Ok(None);
        };
        Ok(Some(TrackMidground {
            scale: [scale[0], scale[1], scale[2]],
            position: [position[0], position[1], position[2]],
            rotation,
            object: game_object(db, object.class, object.collection)?,
        }))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [u32; 7] = [1, 2, 3, 10, 20, 30, 40];

    #[test]
    fn keeps_the_track_and_the_drop_instances_only() {
        let (keep, sel) = select_scene_instances(0xfaba_6d1b, &ALL, Ok(vec![1, 2, 3]), Ok(vec![10, 99]));
        assert_eq!(keep, Some(BTreeSet::from([1, 2, 3, 10, 99])));
        assert_eq!((sel.total, sel.kept(), sel.kept_track, sel.kept_drop), (7, 4, 3, 1));
        assert_eq!(sel.skipped, vec![20, 30, 40]);
        assert!(sel.fallback.is_none());
        assert_eq!(
            sel.summary(),
            "objects: drop 0xfaba6d1b keeps 4 of 7 scene instances (3 track + 1 drop); skipped: 0x00000014, 0x0000001e, 0x00000028"
        );
    }

    #[test]
    fn an_instance_listed_by_both_counts_once_as_track() {
        let (keep, sel) = select_scene_instances(5, &ALL, Ok(vec![1, 10]), Ok(vec![10, 20]));
        assert_eq!(keep, Some(BTreeSet::from([1, 10, 20])));
        assert_eq!((sel.kept_track, sel.kept_drop, sel.skipped.len()), (2, 1, 4));
    }

    #[test]
    fn a_drop_without_its_own_instances_keeps_the_track_ones() {
        let (keep, sel) = select_scene_instances(5, &ALL, Ok(vec![1, 2]), Ok(Vec::new()));
        assert_eq!(keep, Some(BTreeSet::from([1, 2])));
        assert_eq!(sel.kept(), 2);
    }

    #[test]
    fn empty_lists_keep_everything() {
        let (keep, sel) = select_scene_instances(5, &ALL, Ok(Vec::new()), Ok(Vec::new()));
        assert!(keep.is_none());
        assert!(sel.fallback.is_some());
        assert!(sel.skipped.is_empty());
        assert!(sel.summary().contains("keeps all 7"));
    }

    #[test]
    fn an_unresolved_track_list_keeps_everything() {
        let (keep, sel) = select_scene_instances(5, &ALL, Err(anyhow::anyhow!("no ge_track")), Ok(vec![10]));
        assert!(keep.is_none());
        assert!(sel.fallback.as_deref().is_some_and(|r| r.contains("no ge_track")));
    }

    #[test]
    fn many_skipped_instances_are_capped_in_the_summary() {
        let all: Vec<u32> = (0..40).collect();
        let (_, sel) = select_scene_instances(5, &all, Ok(vec![0]), Ok(Vec::new()));
        assert_eq!(sel.skipped.len(), 39);
        assert!(sel.summary().ends_with("… 27 more"));
    }
}
