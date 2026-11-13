mod readers;
mod records;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use ssx_formats::attrib::collection::{self, Collection, DecodedCollection, Field, FieldData, FieldSource};
use ssx_formats::attrib::{NameDict, Schema, Value, Vault, fnv1};

pub use readers::*;
pub use records::*;

pub const CLASS_RANGE: u32 = fnv1(b"range");
pub const CLASS_PEAK: u32 = fnv1(b"peak");
pub const CLASS_UI_DROP: u32 = fnv1(b"ui_drop");
pub const CLASS_GE_DROP: u32 = fnv1(b"ge_drop");
pub const CLASS_GE_TRACK: u32 = fnv1(b"ge_track");
pub const CLASS_CHARACTER: u32 = fnv1(b"character");
pub const CLASS_EQUIPMENT: u32 = fnv1(b"equipment");
pub const CLASS_EXPLORE_REWARDS: u32 = fnv1(b"explorerewards");

struct Loaded {
    vlt: Vec<u8>,
    bin: Vec<u8>,
    cols: Vec<Collection>,
    decoded: Vec<DecodedCollection>,
    local: HashMap<(u32, u32), usize>,
}

pub(crate) struct Vaults {
    loaded: Vec<Loaded>,
    pub dict: NameDict,
    pub unknown: BTreeMap<u32, usize>,
}

fn read_pair(vlt: &Path) -> Result<(Vec<u8>, Vec<u8>)> {
    let bin = vlt.with_extension("bin");
    Ok((
        std::fs::read(vlt).with_context(|| format!("reading {}", vlt.display()))?,
        std::fs::read(&bin).with_context(|| format!("reading {}", bin.display()))?,
    ))
}

pub(crate) struct Rec<'a> {
    fields: Vec<(u32, &'a Field, &'a Loaded)>,
}

impl Vaults {
    pub fn load(big_root: &Path) -> Result<Self> {
        Self::read(big_root, true)
    }

    pub fn load_disc(big_root: &Path) -> Result<Self> {
        Self::read(big_root, false)
    }

    fn read(big_root: &Path, routed: bool) -> Result<Self> {
        let dir = big_root.join("bootcache/data/xenon/attribdb");
        let (svlt, sbin) = read_pair(&dir.join("attribdb_schema.vlt"))?;
        let schema = Schema::from_vault(&Vault::parse(&svlt, &sbin)?).context("reading the schema")?;
        let known: HashSet<u32> = schema.classes.iter().map(|c| c.key).collect();
        let mut loaded = Vec::new();
        let mut dict = NameDict::new();
        let mut seen: HashSet<(u32, u32)> = HashSet::new();
        let mut dlc_files: HashSet<String> = HashSet::new();
        let mut unknown: BTreeMap<u32, usize> = BTreeMap::new();
        let load = |name: &str,
                    (vlt, bin): (Vec<u8>, Vec<u8>),
                    route: bool,
                    dict: &mut NameDict,
                    loaded: &mut Vec<Loaded>,
                    seen: &mut HashSet<(u32, u32)>,
                    unknown: &mut BTreeMap<u32, usize>|
         -> Result<()> {
            dict.add_strings_from(&vlt);
            dict.add_strings_from(&bin);
            let v = Vault::parse(&vlt, &bin).with_context(|| format!("parsing {name}"))?;
            let cols = collection::collections(&v)?;
            let mut kept = Vec::with_capacity(cols.len());
            let mut decoded = Vec::with_capacity(cols.len());
            for c in cols {
                if !seen.insert((c.class, c.key)) {
                    continue;
                }
                if !known.contains(&c.class) {
                    *unknown.entry(c.class).or_default() += 1;
                    continue;
                }
                decoded.push(c.decode(&v, &schema).with_context(|| format!("decoding collection {:#010x}", c.key))?);
                kept.push(c);
            }
            if route {
                crate::content::data::events::route_ui_drops(big_root, &kept, &mut decoded);
            }
            if kept.is_empty() {
                return Ok(());
            }
            let local = collection::index(&kept);
            loaded.push(Loaded { vlt, bin, cols: kept, decoded, local });
            Ok(())
        };
        if routed {
            if let Some(t) = crate::data_roots::tuner() {
                load(
                    &format!("the tuner pack ({})", t.source),
                    (t.vlt.to_vec(), t.bin.to_vec()),
                    false,
                    &mut dict,
                    &mut loaded,
                    &mut seen,
                    &mut unknown,
                )?;
            }
            for root in crate::content::dlc::active_roots() {
                let dir = root.join("data/xenon/attribdb");
                let Ok(read) = std::fs::read_dir(&dir) else { continue };
                let mut vlts: Vec<PathBuf> = read
                    .flatten()
                    .map(|e| e.path())
                    .filter(|p| p.is_file() && p.extension().is_some_and(|e| e.eq_ignore_ascii_case("vlt")))
                    .collect();
                vlts.sort();
                for path in vlts {
                    let name = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
                    if name.eq_ignore_ascii_case("ge_attribdb.vlt")
                        || name.eq_ignore_ascii_case("attribdb.vlt")
                        || name.to_ascii_lowercase().ends_with("_track.vlt")
                    {
                        continue;
                    }
                    if !dlc_files.insert(name.clone()) {
                        continue;
                    }
                    load(&name, read_pair(&path)?, false, &mut dict, &mut loaded, &mut seen, &mut unknown)?;
                }
            }
        }
        let attribdb = crate::data_roots::find(big_root, "data/xenon/attribdb/attribdb.vlt")
            .unwrap_or_else(|| dir.join("attribdb.vlt"));
        load("attribdb.vlt", read_pair(&attribdb)?, true, &mut dict, &mut loaded, &mut seen, &mut unknown)?;
        let ge = crate::data_roots::find(big_root, "data/xenon/attribdb/ge_attribdb.vlt")
            .unwrap_or_else(|| dir.join("ge_attribdb.vlt"));
        let ge_name = ge.file_name().unwrap_or_default().to_string_lossy().into_owned();
        load(&ge_name, read_pair(&ge)?, false, &mut dict, &mut loaded, &mut seen, &mut unknown)?;
        Ok(Vaults { loaded, dict, unknown })
    }

    pub fn keys_of(&self, class: u32) -> Vec<u32> {
        let mut seen = std::collections::HashSet::new();
        self.loaded
            .iter()
            .flat_map(|l| l.cols.iter().filter(|c| c.class == class).map(|c| c.key))
            .filter(|k| seen.insert(*k))
            .collect()
    }

    pub fn parent(&self, class: u32, key: u32) -> u32 {
        self.loaded.iter().find_map(|l| l.local.get(&(class, key)).map(|&i| l.cols[i].parent)).unwrap_or(0)
    }

    pub fn rec(&self, class: u32, key: u32) -> Result<Rec<'_>> {
        let (v, i) = self
            .loaded
            .iter()
            .enumerate()
            .find_map(|(v, l)| l.local.get(&(class, key)).map(|&i| (v, i)))
            .with_context(|| format!("collection {class:#010x}/{key:#010x} not found"))?;
        Ok(Rec { fields: self.with_inherited(v, i)? })
    }

    fn find(&self, class: u32, key: u32) -> Option<(usize, usize)> {
        self.loaded.iter().enumerate().find_map(|(v, l)| l.local.get(&(class, key)).map(|&i| (v, i)))
    }

    fn with_inherited(&self, v: usize, i: usize) -> Result<Vec<(u32, &Field, &Loaded)>> {
        let l = &self.loaded[v];
        let mut out: Vec<(u32, &Field, &Loaded)> = l.decoded[i].fields.iter().map(|f| (l.cols[i].key, f, l)).collect();
        let mut cur = (l.cols[i].class, l.cols[i].parent);
        let total: usize = self.loaded.iter().map(|l| l.cols.len()).sum();
        for _ in 0..total {
            if cur.1 == 0 {
                return Ok(out);
            }
            let (pv, pi) = self.find(cur.0, cur.1).with_context(|| {
                format!("collection {:#010x}: parent {:#010x} not in any vault", l.cols[i].key, cur.1)
            })?;
            let pl = &self.loaded[pv];
            for f in pl.decoded[pi].fields.iter().filter(|f| f.source != FieldSource::Layout) {
                if !out.iter().any(|(_, o, _)| o.key == f.key) {
                    out.push((pl.cols[pi].key, f, pl));
                }
            }
            cur = (pl.cols[pi].class, pl.cols[pi].parent);
        }
        Err(anyhow::anyhow!("collection {:#010x}: parent chain loops", l.cols[i].key))
    }

    pub fn name(&self, class_name: &str, key: u32) -> String {
        let class = fnv1(class_name.as_bytes());
        let confirmed = self.loaded.iter().find_map(|l| l.local.get(&(class, key)).map(|&i| l.cols[i].export_key));
        match (self.dict.get(key), confirmed) {
            (Some(n), Some(e)) if fnv1(format!("{class_name}/{n}").as_bytes()) == e => n.to_string(),
            _ => format!("{key:08x}"),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnlockKind {
    Rider,
    Item,
    Geotag,
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unlockable {
    pub token: String,
    pub key: u32,
    pub kind: UnlockKind,
}

pub(crate) fn unlockables(v: &Vaults, read: crate::content::dlc::Unlockables) -> (Vec<Unlockable>, Vec<String>) {
    let characters: HashSet<u32> = v.keys_of(CLASS_CHARACTER).into_iter().collect();
    let equipment: HashSet<u32> = v.keys_of(CLASS_EQUIPMENT).into_iter().collect();
    let mut problems = read.problems;
    let mut out = Vec::new();
    for token in read.tokens {
        let key = fnv1(token.as_bytes());
        let kind = if characters.contains(&key) {
            UnlockKind::Rider
        } else if equipment.contains(&key) {
            let kind = v.rec(CLASS_EQUIPMENT, key).ok().and_then(|r| r.int(fnv1(b"type"))).unwrap_or(-1);
            if kind == 4 { UnlockKind::Geotag } else { UnlockKind::Item }
        } else {
            problems.push(format!("unlockable {token:?} names no character or equipment collection"));
            UnlockKind::Unknown
        };
        out.push(Unlockable { token, key, kind });
    }
    (out, problems)
}

fn values(d: &FieldData) -> Vec<&Value> {
    match d {
        FieldData::Single(v) => vec![v],
        FieldData::Array(vs) => vs.iter().collect(),
    }
}

fn int_of(v: &Value) -> Option<i64> {
    Some(match v {
        Value::Bool(b) => i64::from(*b),
        Value::Int8(x) => i64::from(*x),
        Value::UInt8(x) => i64::from(*x),
        Value::Int16(x) => i64::from(*x),
        Value::UInt16(x) => i64::from(*x),
        Value::Int32(x) => i64::from(*x),
        Value::UInt32(x) => i64::from(*x),
        Value::Int64(x) => *x,
        Value::UInt64(x) => *x as i64,
        Value::Raw { bytes, .. } if bytes.len() == 4 => {
            i64::from(i32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
        }
        _ => return None,
    })
}

impl Rec<'_> {
    fn data(&self, key: u32) -> Option<&FieldData> {
        self.fields.iter().find(|(_, f, _)| f.key == key).map(|(_, f, _)| &f.data)
    }

    pub fn keys(&self) -> Vec<u32> {
        self.fields.iter().map(|(_, f, _)| f.key).collect()
    }

    pub fn int(&self, key: u32) -> Option<i64> {
        self.data(key).and_then(|d| values(d).first().and_then(|v| int_of(v)))
    }

    pub fn ints(&self, key: u32) -> Vec<i64> {
        self.data(key).map_or_else(Vec::new, |d| values(d).into_iter().filter_map(int_of).collect())
    }

    pub fn float(&self, key: u32) -> Option<f32> {
        self.data(key).and_then(|d| match values(d).first() {
            Some(Value::Float(x)) => Some(*x),
            _ => None,
        })
    }

    pub fn floats(&self, key: u32) -> Vec<f32> {
        self.data(key).map_or_else(Vec::new, |d| {
            values(d).into_iter().filter_map(|v| if let Value::Float(x) = v { Some(*x) } else { None }).collect()
        })
    }

    pub fn vector(&self, key: u32) -> Option<Vec<f32>> {
        self.data(key).and_then(|d| match values(d).first() {
            Some(Value::Vector2(v)) => Some(v.to_vec()),
            Some(Value::Vector3(v)) => Some(v.to_vec()),
            Some(Value::Vector4(v)) => Some(v.to_vec()),
            _ => None,
        })
    }

    pub fn text(&self, key: u32) -> Option<String> {
        self.texts(key).into_iter().next()
    }

    pub fn texts(&self, key: u32) -> Vec<String> {
        self.data(key).map_or_else(Vec::new, |d| {
            values(d)
                .into_iter()
                .filter_map(|v| if let Value::Text(Some(t)) = v { Some(t.clone()) } else { None })
                .collect()
        })
    }

    pub fn refs(&self, key: u32) -> Vec<u32> {
        self.data(key).map_or_else(Vec::new, |d| {
            values(d)
                .into_iter()
                .filter_map(|v| if let Value::Ref(r) = v { Some(r.collection) } else { None })
                .collect()
        })
    }

    pub fn reference(&self, key: u32) -> u32 {
        self.refs(key).first().copied().unwrap_or(0)
    }

    pub fn raws(&self, key: u32) -> Vec<Vec<u8>> {
        self.data(key).map_or_else(Vec::new, |d| {
            values(d)
                .into_iter()
                .filter_map(|v| if let Value::Raw { bytes, .. } = v { Some(bytes.clone()) } else { None })
                .collect()
        })
    }

    pub fn raw_text(&self, key: u32, offset: u32) -> Option<String> {
        let (_, f, vault) = self.fields.iter().find(|(_, f, _)| f.key == key)?;
        let Some(Value::Raw { pointers, .. }) = values(&f.data).first().copied() else { return None };
        let p = pointers.iter().find(|p| p.offset == offset)?;
        let file = if p.target.file == 0 { &vault.vlt } else { &vault.bin };
        let rest = file.get(p.target.offset as usize..)?;
        let end = rest.iter().position(|&b| b == 0)?;
        Some(String::from_utf8_lossy(&rest[..end]).into_owned())
    }
}
