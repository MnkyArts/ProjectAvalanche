use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use anyhow::{Context, Result, bail};
use ssx_formats::attrib::{
    Collection, DecodedCollection, Field, FieldData, FieldSource, NameDict, Schema, Value, Vault, collection,
    container::cstr_in,
};

pub const fn fnv1(s: &str) -> u32 {
    ssx_formats::attrib::fnv1(s.as_bytes())
}

pub const CLASS_MATERIAL: u32 = fnv1("ge_material");
pub const CLASS_TEXTURE: u32 = fnv1("ge_texture");
pub const CLASS_EFFECT: u32 = fnv1("ge_effect");
const FIELD_EFFECT: u32 = fnv1("Effect");
const FIELD_TECHNIQUE: u32 = fnv1("Technique");

#[derive(Clone, Debug, Default)]
pub struct Material {
    pub name: String,
    pub key: u32,
    pub effect: Option<String>,
    pub technique: Option<String>,
    pub textures: BTreeMap<String, String>,
    pub floats: BTreeMap<String, f32>,
    pub vectors: BTreeMap<String, Vec<f32>>,
}

impl Material {
    pub fn float(&self, name: &str) -> Option<f32> {
        self.floats.get(name).copied()
    }

    pub fn texture_file(&self, big_root: &Path, param: &str) -> Option<PathBuf> {
        let rel = self.textures.get(param)?.replace('\\', "/");
        if crate::data_roots::overlays().is_empty() {
            let disc = big_root.join("cache3/data/xenon/gameexplorer").join(format!("{rel}.xpr"));
            if disc.is_file() {
                return Some(disc);
            }
        }
        crate::data_roots::find(big_root, &format!("data/xenon/gameexplorer/{rel}.xpr"))
    }
}

struct Files {
    vlt: Vec<u8>,
    bin: Vec<u8>,
}

#[derive(Clone)]
enum Pair {
    File(PathBuf),
    Tuner(Arc<crate::data_roots::TunerVault>),
}

fn read_pair(pair: &Pair) -> Result<Files> {
    match pair {
        Pair::File(vlt) => {
            let bin = vlt.with_extension("bin");
            Ok(Files { vlt: crate::read::read_file(vlt)?, bin: crate::read::read_file(&bin)? })
        }
        Pair::Tuner(t) => Ok(Files { vlt: t.vlt.to_vec(), bin: t.bin.to_vec() }),
    }
}

fn tuner_pair() -> Option<Pair> {
    crate::data_roots::tuner().map(Pair::Tuner)
}

// PORT(choice): 1 024 files and 64 MiB in all, port limits about ten times the disc's 92 files and 6.2 MB, so that a
// damaged or hostile data folder cannot grow the dictionary without bound; tuning: a name dictionary that no longer
// needs the shader files (the rules subset's own list of names); the original has no dedicated server.
pub const MAX_SHADER_FILES: usize = 1024;
pub const MAX_SHADER_BYTES: u64 = 64 << 20;

const MAX_CACHED_DBS: usize = 4;
const MAX_CACHED_BYTES: usize = 256 << 20;

pub struct Loaded {
    pub cols: Vec<Collection>,
    pub decoded: Vec<DecodedCollection>,
    pub local: HashMap<(u32, u32), usize>,
    files: Files,
}

pub struct Db {
    pub dict: NameDict,
    pub vaults: Vec<Loaded>,
    pub unknown: BTreeMap<u32, usize>,
}

fn disc_attribdb(big_root: &Path) -> PathBuf {
    big_root.join("bootcache/data/xenon/attribdb")
}

fn schema_path(big_root: &Path) -> PathBuf {
    disc_attribdb(big_root).join("attribdb_schema.vlt")
}

pub fn with_local_attribdb<R>(root: &Path, f: impl FnOnce(&Db) -> Result<R>) -> Result<R> {
    let dir = root.join("bootcache/data/xenon/attribdb");
    with_vaults(root, &[Pair::File(dir.join("ge_attribdb.vlt")), Pair::File(dir.join("attribdb_schema.vlt"))], f)
}

pub fn dlc_vaults(big_root: &Path) -> Vec<PathBuf> {
    let mut out = dlc_delta_vaults();
    if let Some(ge) = ge_attribdb_vault(big_root) {
        out.push(ge);
    }
    out
}

pub fn ge_attribdb_vault(big_root: &Path) -> Option<PathBuf> {
    crate::data_roots::find(big_root, "data/xenon/attribdb/ge_attribdb.vlt")
}

pub fn dlc_delta_vaults() -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut names = std::collections::BTreeSet::new();
    for root in crate::data_roots::overlays() {
        for path in crate::data_roots::files_in(&root, "data/xenon/attribdb", "vlt") {
            let name = path.file_name().unwrap_or_default().to_string_lossy().to_ascii_lowercase();
            if name == "ge_attribdb.vlt" || name == "attribdb.vlt" || name.ends_with("_track.vlt") {
                continue;
            }
            if names.insert(name) {
                out.push(path);
            }
        }
    }
    out
}

pub fn with_track_db<R>(big_root: &Path, track: &str, f: impl FnOnce(&Db) -> Result<R>) -> Result<R> {
    let track_vlt = crate::data_roots::find(big_root, &format!("data/xenon/attribdb/{track}_track.vlt"))
        .with_context(|| format!("{track}: no track vault"))?;
    let mut pairs = vec![Pair::File(track_vlt)];
    pairs.extend(tuner_pair());
    let dlc = !crate::data_roots::overlays().is_empty();
    if dlc {
        pairs.extend(dlc_delta_vaults().into_iter().map(Pair::File));
    }
    let ge = ge_attribdb_vault(big_root).with_context(|| format!("{}: no ge_attribdb vault", big_root.display()))?;
    pairs.push(Pair::File(ge));
    pairs.push(Pair::File(schema_path(big_root)));
    let key = CacheKey::new(big_root, 1, track);
    with_vaults_maybe_cached(big_root, &pairs, (dlc || crate::data_roots::tuner().is_some()).then_some(key), f)
}

pub fn with_level_db<R>(big_root: &Path, track: Option<&str>, f: impl FnOnce(&Db) -> Result<R>) -> Result<R> {
    match track {
        Some(t) => with_track_db(big_root, t, f),
        None => with_attribdb(big_root, f),
    }
}

pub fn with_attribdb<R>(big_root: &Path, f: impl FnOnce(&Db) -> Result<R>) -> Result<R> {
    let mut pairs = Vec::new();
    pairs.extend(tuner_pair());
    let dlc = !crate::data_roots::overlays().is_empty();
    if dlc {
        pairs.extend(dlc_delta_vaults().into_iter().map(Pair::File));
    }
    let ge = ge_attribdb_vault(big_root).with_context(|| format!("{}: no ge_attribdb vault", big_root.display()))?;
    pairs.push(Pair::File(ge));
    pairs.push(Pair::File(schema_path(big_root)));
    let key = CacheKey::new(big_root, 2, "");
    with_vaults_maybe_cached(big_root, &pairs, (dlc || crate::data_roots::tuner().is_some()).then_some(key), f)
}

pub fn with_all_db<R>(big_root: &Path, f: impl FnOnce(&Db) -> Result<R>) -> Result<R> {
    let mut pairs = Vec::new();
    pairs.extend(tuner_pair());
    let dlc = !crate::data_roots::overlays().is_empty();
    if dlc {
        pairs.extend(dlc_delta_vaults().into_iter().map(Pair::File));
    }
    pairs.push(Pair::File(
        crate::data_roots::find(big_root, "data/xenon/attribdb/attribdb.vlt")
            .unwrap_or_else(|| disc_attribdb(big_root).join("attribdb.vlt")),
    ));
    let ge = ge_attribdb_vault(big_root).with_context(|| format!("{}: no ge_attribdb vault", big_root.display()))?;
    pairs.push(Pair::File(ge));
    pairs.push(Pair::File(schema_path(big_root)));
    let key = CacheKey::new(big_root, 3, "");
    with_vaults_maybe_cached(big_root, &pairs, (dlc || crate::data_roots::tuner().is_some()).then_some(key), f)
}

pub fn with_career_db<R>(big_root: &Path, f: impl FnOnce(&Db) -> Result<R>) -> Result<R> {
    let mut pairs = Vec::new();
    pairs.extend(tuner_pair());
    let dlc = !crate::data_roots::overlays().is_empty();
    if dlc {
        pairs.extend(dlc_delta_vaults().into_iter().map(Pair::File));
    }
    pairs.push(Pair::File(disc_attribdb(big_root).join("attribdb.vlt")));
    pairs.push(Pair::File(schema_path(big_root)));
    let key = CacheKey::new(big_root, 4, "");
    with_vaults_maybe_cached(big_root, &pairs, (dlc || crate::data_roots::tuner().is_some()).then_some(key), f)
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct CacheKey {
    big_root: PathBuf,
    kind: u8,
    track: String,
    generation: u64,
}

impl CacheKey {
    fn new(big_root: &Path, kind: u8, track: &str) -> Self {
        CacheKey {
            big_root: std::fs::canonicalize(big_root).unwrap_or_else(|_| big_root.to_path_buf()),
            kind,
            track: track.to_string(),
            generation: crate::data_roots::generation(),
        }
    }
}

struct Slot {
    db: Arc<Db>,
    bytes: usize,
    used: u64,
}

#[derive(Default)]
struct Cache {
    slots: BTreeMap<CacheKey, Slot>,
    tick: u64,
}

impl Cache {
    fn get(&mut self, key: &CacheKey) -> Option<Arc<Db>> {
        self.tick += 1;
        let slot = self.slots.get_mut(key)?;
        slot.used = self.tick;
        Some(slot.db.clone())
    }

    fn insert(&mut self, key: CacheKey, db: Arc<Db>, bytes: usize, now: u64) {
        self.slots.retain(|k, _| k.generation == now);
        if key.generation != now || bytes > MAX_CACHED_BYTES {
            return;
        }
        self.slots.remove(&key);
        loop {
            let used: usize = self.slots.values().map(|s| s.bytes).sum();
            if self.slots.len() < MAX_CACHED_DBS && used.saturating_add(bytes) <= MAX_CACHED_BYTES {
                break;
            }
            let Some(oldest) = self.slots.iter().min_by_key(|(_, s)| s.used).map(|(k, _)| k.clone()) else { break };
            self.slots.remove(&oldest);
        }
        self.tick += 1;
        self.slots.insert(key, Slot { db, bytes, used: self.tick });
    }
}

static CACHE: OnceLock<Mutex<Cache>> = OnceLock::new();

fn with_vaults_maybe_cached<R>(
    big_root: &Path,
    pairs: &[Pair],
    key: Option<CacheKey>,
    f: impl FnOnce(&Db) -> Result<R>,
) -> Result<R> {
    let Some(key) = key else { return with_vaults(big_root, pairs, f) };
    let cache = CACHE.get_or_init(|| Mutex::new(Cache::default()));
    let hit = cache.lock().unwrap_or_else(std::sync::PoisonError::into_inner).get(&key);
    if let Some(db) = hit {
        return f(&db);
    }
    let files: Vec<Files> = pairs.iter().map(read_pair).collect::<Result<_>>()?;
    let bytes: usize = files.iter().map(|f| f.vlt.len().saturating_add(f.bin.len())).sum();
    let db = Arc::new(build_db(big_root, files)?);
    cache.lock().unwrap_or_else(std::sync::PoisonError::into_inner).insert(
        key,
        db.clone(),
        bytes,
        crate::data_roots::generation(),
    );
    f(&db)
}

fn with_vaults<R>(big_root: &Path, pairs: &[Pair], f: impl FnOnce(&Db) -> Result<R>) -> Result<R> {
    let files: Vec<Files> = pairs.iter().map(read_pair).collect::<Result<_>>()?;
    let db = build_db(big_root, files)?;
    f(&db)
}

fn build_db(big_root: &Path, files: Vec<Files>) -> Result<Db> {
    let vaults: Vec<Vault> = files.iter().map(|f| Vault::parse(&f.vlt, &f.bin)).collect::<ssx_formats::Result<_>>()?;
    let schema = Schema::from_vault(vaults.last().context("no schema vault")?).context("reading the schema")?;
    let known: HashSet<u32> = schema.classes.iter().map(|c| c.key).collect();

    let mut dict = NameDict::new();
    for f in &files {
        dict.add_strings_from(&f.vlt);
        dict.add_strings_from(&f.bin);
    }
    let (mut files_read, mut bytes_read) = (0usize, 0u64);
    for path in crate::data_roots::files_in(big_root, "data/xenon/shaders", "fxo") {
        if files_read == MAX_SHADER_FILES {
            bail!("{}: more than {MAX_SHADER_FILES} shader files", big_root.display());
        }
        if let Ok(data) = crate::read::read_file(&path) {
            files_read += 1;
            bytes_read += data.len() as u64;
            if bytes_read > MAX_SHADER_BYTES {
                bail!("{}: shader files of more than {MAX_SHADER_BYTES} bytes", big_root.display());
            }
            dict.add_strings_from(&data);
        }
    }
    for t in &schema.types {
        dict.insert(&t.name);
    }

    let mut tables = Vec::with_capacity(vaults.len());
    let mut unknown: BTreeMap<u32, usize> = BTreeMap::new();
    for v in &vaults {
        let (kept, decoded) = decode_vault(v, &schema, &known, &mut unknown)?;
        let local = collection::index(&kept);
        tables.push((kept, decoded, local));
    }
    drop(vaults);
    let loaded = tables
        .into_iter()
        .zip(files)
        .map(|((cols, decoded, local), files)| Loaded { cols, decoded, local, files })
        .collect();
    Ok(Db { dict, vaults: loaded, unknown })
}

fn decode_vault(
    v: &Vault,
    schema: &Schema,
    known: &HashSet<u32>,
    unknown: &mut BTreeMap<u32, usize>,
) -> Result<(Vec<Collection>, Vec<DecodedCollection>)> {
    let cols = collection::collections(v)?;
    let mut kept = Vec::with_capacity(cols.len());
    let mut decoded = Vec::with_capacity(cols.len());
    for c in cols {
        if !known.contains(&c.class) {
            *unknown.entry(c.class).or_default() += 1;
            continue;
        }
        decoded.push(c.decode(v, schema).with_context(|| format!("collection {:#010x}", c.key))?);
        kept.push(c);
    }
    Ok((kept, decoded))
}

pub const MAX_PACKAGE_VAULTS: usize = 64;

pub fn check_package_vaults(root: &Path) -> Result<BTreeMap<u32, usize>> {
    let decode_root = root.join("bootcache/data/xenon/attribdb");
    let mut paths = Vec::new();
    for dir in [root.join("data/xenon/attribdb"), decode_root.clone()] {
        let read = match std::fs::read_dir(&dir) {
            Ok(read) => read,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e).with_context(|| dir.display().to_string()),
        };
        for entry in read {
            let path = entry.with_context(|| dir.display().to_string())?.path();
            let name = path.file_name().unwrap_or_default().to_string_lossy().to_ascii_lowercase();
            if !name.ends_with(".vlt") || (dir == decode_root && name == "attribdb_schema.vlt") {
                continue;
            }
            if paths.len() == MAX_PACKAGE_VAULTS {
                bail!("{}: more than {MAX_PACKAGE_VAULTS} vault files", root.display());
            }
            paths.push(path);
        }
    }
    let mut unknown = BTreeMap::new();
    if paths.is_empty() {
        return Ok(unknown);
    }
    paths.sort();
    let schema_files = read_pair(&Pair::File(decode_root.join("attribdb_schema.vlt")))
        .context("the decode root's attribdb_schema (install the package again)")?;
    let schema_vault = Vault::parse(&schema_files.vlt, &schema_files.bin).context("parsing attribdb_schema")?;
    let schema = Schema::from_vault(&schema_vault).context("reading the schema")?;
    let known: HashSet<u32> = schema.classes.iter().map(|c| c.key).collect();
    for path in paths {
        let shown = path.strip_prefix(root).unwrap_or(&path).display().to_string();
        let files = read_pair(&Pair::File(path)).with_context(|| shown.clone())?;
        let vault = Vault::parse(&files.vlt, &files.bin).with_context(|| format!("parsing {shown}"))?;
        decode_vault(&vault, &schema, &known, &mut unknown).with_context(|| shown.clone())?;
    }
    Ok(unknown)
}

impl Db {
    pub fn find(&self, class: u32, key: u32) -> Option<(usize, usize)> {
        self.vaults.iter().enumerate().find_map(|(v, l)| l.local.get(&(class, key)).map(|&i| (v, i)))
    }

    pub fn fields(&self, class: u32, key: u32) -> Result<Vec<&Field>> {
        Ok(self.fields_with_source(class, key)?.into_iter().map(|(_, f)| f).collect())
    }

    pub fn fields_with_source(&self, class: u32, key: u32) -> Result<Vec<(u32, &Field)>> {
        let (v, i) =
            self.find(class, key).with_context(|| format!("collection {class:#010x}/{key:#010x} not found"))?;
        self.with_inherited(v, i)
    }

    fn with_inherited(&self, v: usize, i: usize) -> Result<Vec<(u32, &Field)>> {
        let l = &self.vaults[v];
        let mut out: Vec<(u32, &Field)> = l.decoded[i].fields.iter().map(|f| (l.cols[i].key, f)).collect();
        let mut cur = (l.cols[i].class, l.cols[i].parent);
        let total: usize = self.vaults.iter().map(|l| l.cols.len()).sum();
        for _ in 0..total {
            if cur.1 == 0 {
                return Ok(out);
            }
            let (pv, pi) = self.find(cur.0, cur.1).with_context(|| {
                format!("collection {:#010x}: parent {:#010x} not in any vault", l.cols[i].key, cur.1)
            })?;
            let pl = &self.vaults[pv];
            for f in pl.decoded[pi].fields.iter().filter(|f| f.source != FieldSource::Layout) {
                if !out.iter().any(|(_, o)| o.key == f.key) {
                    out.push((pl.cols[pi].key, f));
                }
            }
            cur = (pl.cols[pi].class, pl.cols[pi].parent);
        }
        anyhow::bail!("collection {:#010x}: parent chain loops", l.cols[i].key)
    }

    pub fn parent(&self, class: u32, key: u32) -> u32 {
        self.find(class, key).map_or(0, |(v, i)| self.vaults[v].cols[i].parent)
    }

    pub fn own_field(&self, class: u32, key: u32, field: u32) -> Option<&Field> {
        let (v, i) = self.find(class, key)?;
        self.vaults[v].decoded[i].fields.iter().find(|f| f.key == field)
    }

    pub fn value_text(&self, class: u32, key: u32, v: &Value, offset: u32) -> Result<Option<String>> {
        let (vi, _) =
            self.find(class, key).with_context(|| format!("collection {class:#010x}/{key:#010x} not found"))?;
        let Value::Raw { pointers, .. } = v else { bail!("not a raw value") };
        match pointers.iter().find(|p| p.offset == offset) {
            Some(p) => {
                let files = &self.vaults[vi].files;
                Ok(Some(cstr_in(&files.vlt, &files.bin, p.target)?))
            }
            None => Ok(None),
        }
    }

    pub fn field(&self, class: u32, coll: u32, key: u32) -> Result<Option<&Field>> {
        Ok(self.fields(class, coll)?.into_iter().find(|f| f.key == key))
    }

    pub fn collections_of(&self, vault: usize, class: u32) -> impl Iterator<Item = (u32, usize)> + '_ {
        self.vaults[vault].cols.iter().enumerate().filter(move |(_, c)| c.class == class).map(|(i, c)| (c.key, i))
    }

    pub fn collections_of_all(&self, class: u32) -> Vec<(u32, usize)> {
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        for (v, loaded) in self.vaults.iter().enumerate() {
            for c in loaded.cols.iter().filter(|c| c.class == class) {
                if seen.insert(c.key) {
                    out.push((c.key, v));
                }
            }
        }
        out
    }

    pub fn class_name(&self, class: u32) -> String {
        self.dict.get(class).map_or_else(|| format!("{class:#010x}"), str::to_string)
    }

    pub fn field_name(&self, key: u32) -> String {
        self.dict.get(key).map_or_else(|| format!("{key:#010x}"), str::to_string)
    }

    pub fn collection_name(&self, key: u32) -> String {
        self.dict.get(key).map_or_else(|| format!("{key:#010x}"), str::to_string)
    }

    pub fn text_of(&self, class: u32, key: u32) -> Result<Option<String>> {
        Ok(self.fields(class, key)?.iter().find_map(|f| single_text(&f.data)))
    }

    pub fn material(&self, class: u32, key: u32) -> Result<Material> {
        if class != CLASS_MATERIAL {
            bail!("material reference to class {} instead of ge_material", self.class_name(class));
        }
        let mut m = Material { name: self.collection_name(key), key, ..Default::default() };
        let mut queue = vec![(class, key)];
        let mut seen = HashSet::new();
        while let Some((c, k)) = queue.pop() {
            if !seen.insert((c, k)) || self.find(c, k).is_none() {
                continue;
            }
            let is_params = self.class_name(c).starts_with("ge_materialtype_default");
            for f in self.fields(c, k)? {
                match single(&f.data) {
                    Some(Value::Ref(r)) if r.collection != 0 => match r.class {
                        CLASS_EFFECT if f.key == FIELD_EFFECT => m.effect = self.text_of(r.class, r.collection)?,
                        CLASS_TEXTURE if is_params => {
                            if let Some(path) = self.text_of(r.class, r.collection)? {
                                m.textures.insert(self.field_name(f.key), path);
                            }
                        }
                        CLASS_TEXTURE | CLASS_EFFECT => {}
                        _ => queue.push((r.class, r.collection)),
                    },
                    Some(Value::Text(Some(t))) if f.key == FIELD_TECHNIQUE => m.technique = Some(t.clone()),
                    Some(Value::Float(x)) if is_params => {
                        m.floats.insert(self.field_name(f.key), *x);
                    }
                    Some(Value::Vector2(v)) if is_params => {
                        m.vectors.insert(self.field_name(f.key), v.to_vec());
                    }
                    Some(Value::Vector3(v)) if is_params => {
                        m.vectors.insert(self.field_name(f.key), v.to_vec());
                    }
                    Some(Value::Vector4(v)) if is_params => {
                        m.vectors.insert(self.field_name(f.key), v.to_vec());
                    }
                    _ => {}
                }
            }
        }
        Ok(m)
    }
}

pub fn single(d: &FieldData) -> Option<&Value> {
    match d {
        FieldData::Single(v) => Some(v),
        FieldData::Array(_) => None,
    }
}

pub fn single_text(d: &FieldData) -> Option<String> {
    match single(d) {
        Some(Value::Text(Some(t))) => Some(t.clone()),
        _ => None,
    }
}

pub fn refs(d: &FieldData) -> Vec<ssx_formats::attrib::RefSpec> {
    let vals: Vec<&Value> = match d {
        FieldData::Single(v) => vec![v],
        FieldData::Array(vs) => vs.iter().collect(),
    };
    vals.into_iter().filter_map(|v| if let Value::Ref(r) = v { Some(*r) } else { None }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(track: &str, generation: u64) -> CacheKey {
        CacheKey { big_root: PathBuf::from("/big"), kind: 1, track: track.into(), generation }
    }

    fn db() -> Arc<Db> {
        Arc::new(Db { dict: NameDict::new(), vaults: Vec::new(), unknown: BTreeMap::new() })
    }

    #[test]
    fn the_cache_drops_older_generations_and_the_least_recently_used() {
        let mut cache = Cache::default();
        for t in ["a", "b", "c", "d"] {
            cache.insert(key(t, 1), db(), 1, 1);
        }
        assert!(cache.get(&key("a", 1)).is_some());
        cache.insert(key("e", 1), db(), 1, 1);
        assert_eq!(cache.slots.len(), MAX_CACHED_DBS);
        assert!(cache.get(&key("b", 1)).is_none());
        assert!(cache.get(&key("a", 1)).is_some());
        cache.insert(key("a", 2), db(), 1, 2);
        assert_eq!(cache.slots.keys().cloned().collect::<Vec<_>>(), vec![key("a", 2)]);
        cache.insert(key("b", 1), db(), 1, 2);
        assert_eq!(cache.slots.len(), 1);
        cache.insert(key("big", 2), db(), MAX_CACHED_BYTES + 1, 2);
        assert!(cache.get(&key("big", 2)).is_none());
        cache.insert(key("half", 2), db(), MAX_CACHED_BYTES / 2 + 1, 2);
        cache.insert(key("other half", 2), db(), MAX_CACHED_BYTES / 2 + 1, 2);
        assert!(cache.get(&key("half", 2)).is_none());
        assert!(cache.get(&key("other half", 2)).is_some());
    }
}
