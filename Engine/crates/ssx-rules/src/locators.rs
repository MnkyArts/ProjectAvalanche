use std::path::Path;

use anyhow::Result;
use glam::{Quat, Vec3};
use ssx_formats::attrib::{FieldData, RefSpec, Value};

use crate::placement::Placement;
use crate::triggers::{TriggerScope, TriggerVolume, TriggerVolumeRecord};
use crate::vault_db::{Db, fnv1, refs, single, single_text, with_track_db};

pub const CLASS_DROP: u32 = fnv1("ge_drop");
pub const CLASS_GAMEOBJECT: u32 = fnv1("ge_scene_gameobject");
pub const FIELD_POB: u32 = fnv1("POBFileRef");
pub const CLASS_SCENE_INSTANCE: u32 = fnv1("ge_scene_instance");
pub const FIELD_TRACK: u32 = fnv1("Track");
pub const FIELD_DROP_SCENE_INSTANCES: u32 = 0x5050_DE8B;
pub const FIELD_GAMEOBJECT: u32 = 0x3ED7_B5CE;
pub const FIELD_INSTANCE_DATA: u32 = 0xEE66_32AC;
pub const FIELD_DROP_MESHES: u32 = fnv1("Meshes");
pub const CLASS_MESHSUBD: u32 = 0xB9DD_6F89;
pub const FIELD_FILE_REF: u32 = 0xDF9E_F8B2;
pub const START_LOCATOR_OBJECT: u32 = 0x1343_4084;
pub const CLASS_TRACK: u32 = fnv1("ge_track");
pub const FIELD_TRACK_TERRAIN: u32 = 0x3132_67E8;
pub const FIELD_TERRAIN_INSTANCES: u32 = 0x0903_EF8F;
pub const FIELD_TRIGGER_VOLUMES: u32 = fnv1("TriggerVolumes");
pub const FIELD_TV_SCALE: u32 = fnv1("Scale");
pub const FIELD_TV_POSITION: u32 = fnv1("Position");
pub const FIELD_TV_ORIENTATION: u32 = fnv1("Orientation");
pub const FIELD_TV_NAME: u32 = 0xBF29_BFFE;
pub const FIELD_TV_TYPE: u32 = 0x7F6F_3C0B;
pub const FIELD_TV_SHAPE: u32 = 0xCC65_CA4E;

#[derive(Clone, Debug, PartialEq)]
pub struct DropStart {
    pub drop_key: u32,
    pub slots: Vec<Placement>,
    pub terrain_files: Vec<String>,
}

impl DropStart {
    pub fn slot(&self, i: usize) -> Option<(Vec3, Quat)> {
        let p = self.slots.get(i)?;
        let q = Quat::from_xyzw(p.rotation[0], p.rotation[1], p.rotation[2], p.rotation[3]).normalize();
        Some((Vec3::from_array(p.position), q))
    }
}

pub fn load_drop_starts(big_root: &Path, track: &str) -> Result<Vec<DropStart>> {
    with_track_db(big_root, track, |db| drop_starts(db, track))
}

pub fn drop_starts(db: &Db, track: &str) -> Result<Vec<DropStart>> {
    let track_key = fnv1(&format!("{track}_track"));
    {
        let mut out = Vec::new();
        for (key, _) in db.collections_of_all(CLASS_DROP) {
            let fields = db.fields(CLASS_DROP, key)?;
            let field = |k: u32| fields.iter().find(|f| f.key == k);
            if !matches!(field(FIELD_TRACK).and_then(|f| single(&f.data)), Some(Value::Ref(r)) if r.collection == track_key)
            {
                continue;
            }
            let mut terrain_files = Vec::new();
            for m in field(FIELD_DROP_MESHES).map(|f| refs(&f.data)).unwrap_or_default() {
                if let Some(path) =
                    db.field(CLASS_MESHSUBD, m.collection, FIELD_FILE_REF)?.and_then(|f| single_text(&f.data))
                {
                    terrain_files.push(path.rsplit(['\\', '/']).next().unwrap_or(&path).to_owned());
                }
            }
            let Some(instances) = field(FIELD_DROP_SCENE_INSTANCES) else { continue };
            for si in refs(&instances.data).into_iter().filter(|r| r.collection != 0) {
                let sif = db.fields(CLASS_SCENE_INSTANCE, si.collection)?;
                let reference = |k: u32| sif.iter().find(|f| f.key == k).and_then(|f| refs(&f.data).first().copied());
                if reference(FIELD_GAMEOBJECT).is_none_or(|o| o.collection != START_LOCATOR_OBJECT) {
                    continue;
                }
                let Some(data) = reference(FIELD_INSTANCE_DATA).filter(|d| d.collection != 0) else { continue };
                let slots = match db.field(data.class, data.collection, FIELD_INSTANCE_DATA)? {
                    Some(f) => decode_placements(&f.data),
                    None => Vec::new(),
                };
                if !slots.is_empty() {
                    out.push(DropStart { drop_key: key, slots, terrain_files });
                    break;
                }
            }
        }
        Ok(out)
    }
}

pub const END_LOCATOR_OBJECT: u32 = 0x9297_7821;

pub fn load_end_locator(big_root: &Path, track: &str, drop_key: u32) -> Result<Option<Vec3>> {
    with_track_db(big_root, track, |db| end_locator(db, track, drop_key))
}

pub fn end_locator(db: &Db, track: &str, drop_key: u32) -> Result<Option<Vec3>> {
    {
        let mut list = drop_scene_instances(db, drop_key)?;
        list.extend(track_scene_instances(db, track)?);
        for (class, key) in list {
            let Ok(sif) = db.fields(class, key) else { continue };
            let reference = |k: u32| sif.iter().find(|f| f.key == k).and_then(|f| refs(&f.data).first().copied());
            if reference(FIELD_GAMEOBJECT).is_none_or(|o| o.collection != END_LOCATOR_OBJECT) {
                continue;
            }
            let Some(data) = reference(FIELD_INSTANCE_DATA).filter(|d| d.collection != 0) else { continue };
            if let Some(f) = db.field(data.class, data.collection, FIELD_INSTANCE_DATA)?
                && let Some(p) = decode_placements(&f.data).first()
            {
                return Ok(Some(Vec3::from_array(p.position)));
            }
        }
        Ok(None)
    }
}

pub fn decode_placements(d: &FieldData) -> Vec<Placement> {
    let vals: Vec<&Value> = match d {
        FieldData::Single(v) => vec![v],
        FieldData::Array(vs) => vs.iter().collect(),
    };
    vals.into_iter()
        .filter_map(|v| if let Value::Raw { bytes, .. } = v { Placement::decode(bytes) } else { None })
        .collect()
}

pub fn track_drop_keys(db: &Db, track: &str) -> Result<Vec<u32>> {
    let track_key = fnv1(&format!("{track}_track"));
    let mut out = Vec::new();
    for (key, _) in db.collections_of_all(CLASS_DROP) {
        if matches!(db.field(CLASS_DROP, key, FIELD_TRACK)?.and_then(|f| single(&f.data)),
            Some(Value::Ref(r)) if r.collection == track_key)
        {
            out.push(key);
        }
    }
    Ok(out)
}

pub fn track_scene_instances(db: &Db, track: &str) -> Result<Vec<(u32, u32)>> {
    let track_key = fnv1(&format!("{track}_track"));
    let mut out = Vec::new();
    let Some(terrains) = db.field(CLASS_TRACK, track_key, FIELD_TRACK_TERRAIN)? else { return Ok(out) };
    for t in refs(&terrains.data).into_iter().filter(|r| r.collection != 0) {
        let Some(cols) = db.field(t.class, t.collection, FIELD_TERRAIN_INSTANCES)? else { continue };
        for c in refs(&cols.data).into_iter().filter(|r| r.collection != 0) {
            if let Some(list) = db.field(c.class, c.collection, FIELD_DROP_SCENE_INSTANCES)? {
                out.extend(refs(&list.data).into_iter().filter(|r| r.collection != 0).map(|r| (r.class, r.collection)));
            }
        }
    }
    Ok(out)
}

pub fn drop_scene_instances(db: &Db, drop_key: u32) -> Result<Vec<(u32, u32)>> {
    Ok(match db.field(CLASS_DROP, drop_key, FIELD_DROP_SCENE_INSTANCES)? {
        Some(f) => refs(&f.data).into_iter().filter(|r| r.collection != 0).map(|r| (r.class, r.collection)).collect(),
        None => Vec::new(),
    })
}

pub fn trigger_volume_record(db: &Db, class: u32, key: u32) -> Result<TriggerVolumeRecord> {
    let fields = db.fields(class, key)?;
    let get = |k: u32| fields.iter().find(|f| f.key == k).and_then(|f| single(&f.data));
    let vec4 = |k: u32| match get(k) {
        Some(Value::Vector4(v)) => Ok(*v),
        other => anyhow::bail!("trigger volume {key:#010x} field {k:#010x}: expected a Vector4, got {other:?}"),
    };
    let text = |k: u32| match get(k) {
        Some(Value::Text(t)) => t.clone().unwrap_or_default(),
        _ => String::new(),
    };
    let shape = match get(FIELD_TV_SHAPE) {
        Some(Value::Int32(s)) => *s,
        other => anyhow::bail!("trigger volume {key:#010x}: shape {other:?}"),
    };
    let (s, p, q) = (vec4(FIELD_TV_SCALE)?, vec4(FIELD_TV_POSITION)?, vec4(FIELD_TV_ORIENTATION)?);
    Ok(TriggerVolumeRecord {
        name: text(FIELD_TV_NAME),
        type_text: text(FIELD_TV_TYPE),
        shape,
        scale: Vec3::new(s[0], s[1], s[2]),
        position: Vec3::new(p[0], p[1], p[2]),
        orientation: Quat::from_xyzw(q[0], q[1], q[2], q[3]),
    })
}

pub fn instance_refs(db: &Db, (class, key): (u32, u32)) -> Result<Option<(RefSpec, RefSpec)>> {
    let sif = db.fields(class, key)?;
    let reference = |k: u32| sif.iter().find(|f| f.key == k).and_then(|f| refs(&f.data).first().copied());
    let (Some(obj), Some(data)) = (reference(FIELD_GAMEOBJECT), reference(FIELD_INSTANCE_DATA)) else {
        return Ok(None);
    };
    if obj.collection == 0 || data.collection == 0 {
        return Ok(None);
    }
    Ok(Some((obj, data)))
}

pub fn object_volumes(db: &Db, obj: RefSpec) -> Result<Vec<RefSpec>> {
    Ok(match db.field(obj.class, obj.collection, FIELD_TRIGGER_VOLUMES)? {
        Some(f) => refs(&f.data).into_iter().filter(|r| r.collection != 0).collect(),
        None => Vec::new(),
    })
}

pub fn instance_placements(db: &Db, data: RefSpec) -> Result<Vec<Placement>> {
    Ok(match db.field(data.class, data.collection, FIELD_INSTANCE_DATA)? {
        Some(f) => decode_placements(&f.data),
        None => Vec::new(),
    })
}

pub fn place_volumes(
    db: &Db,
    volumes: &[RefSpec],
    placements: &[Placement],
    (object, instance): (u32, u32),
    scope: TriggerScope,
) -> Result<Vec<TriggerVolume>> {
    let mut out = Vec::new();
    for v in volumes {
        let rec = trigger_volume_record(db, v.class, v.collection)?;
        for (i, pl) in placements.iter().enumerate() {
            let mut tv = TriggerVolume::place(&rec, pl);
            (tv.object, tv.instance, tv.placement, tv.scope) = (object, instance, i, scope);
            out.push(tv);
        }
    }
    Ok(out)
}

pub fn drop_trigger_volumes(db: &Db, track: &str, drop_key: Option<u32>) -> Result<Vec<TriggerVolume>> {
    let mut out = Vec::new();
    let mut add = |si: (u32, u32), scope: TriggerScope| -> Result<()> {
        let Some((obj, data)) = instance_refs(db, si)? else { return Ok(()) };
        let volumes = object_volumes(db, obj)?;
        if volumes.is_empty() {
            return Ok(());
        }
        let placements = instance_placements(db, data)?;
        out.extend(place_volumes(db, &volumes, &placements, (obj.collection, si.1), scope)?);
        Ok(())
    };
    for si in track_scene_instances(db, track)? {
        add(si, TriggerScope::Track)?;
    }
    if let Some(key) = drop_key {
        for si in drop_scene_instances(db, key)? {
            add(si, TriggerScope::Drop(key))?;
        }
    }
    Ok(out)
}
