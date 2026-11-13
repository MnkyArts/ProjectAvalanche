use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};

use anyhow::{Context, Result};
use ssx_content::data::vault::{self as vrec, FieldSpec};
use ssx_content::data::{Class, Mode, RecordSchema, check_table};
use ssx_content::{Asset, ContentId, Kind, Layer, LoadError, Source};
use ssx_formats::attrib::collection::{self, Collection, DecodedCollection};
use ssx_formats::attrib::{RefSpec, Schema, Vault};

use super::super::store::GameContent;
use super::event_fields as ef;

pub const CLASS_UI_DROP: u32 = 0xB11E_A551;
pub const GHOST_DB: &str = "bootcache/data/common/ghosts/ghostdb.csv";
const ATTRIBDB: &str = "bootcache/data/xenon/attribdb/attribdb.vlt";
const SCHEMA: &str = "bootcache/data/xenon/attribdb/attribdb_schema.vlt";

pub const KINDS: [Kind; 2] = [Kind::Event, Kind::Drop];

pub struct DiscEvents {
    pub by_path: BTreeMap<String, (u32, usize)>,
    pub drop_fields: Vec<FieldSpec>,
    pub event_fields: Vec<FieldSpec>,
    cols: Vec<Collection>,
    decoded: Vec<DecodedCollection>,
    index: std::collections::HashMap<(u32, u32), usize>,
    ref_names: BTreeMap<(u32, u32), String>,
    ref_keys: BTreeMap<String, RefSpec>,
    rivals: BTreeMap<u32, Rows>,
    all_rows: Vec<Vec<i64>>,
}

impl DiscEvents {
    pub fn load(big: &Path) -> Result<DiscEvents> {
        let read = |rel: &str| -> Result<(Vec<u8>, Vec<u8>)> {
            let p = big.join(rel);
            let bin = p.with_extension("bin");
            Ok((ssx_rules::read::read_file(&p)?, ssx_rules::read::read_file(&bin)?))
        };
        let (svlt, sbin) = read(SCHEMA)?;
        let schema = Schema::from_vault(&Vault::parse(&svlt, &sbin)?).context("reading the schema")?;
        let (vlt, bin) = read(ATTRIBDB)?;
        let vault = Vault::parse(&vlt, &bin).context("parsing attribdb.vlt")?;
        let cols = collection::collections(&vault)?;
        let decoded = cols.iter().map(|c| c.decode(&vault, &schema)).collect::<ssx_formats::Result<Vec<_>>>()?;
        let index = collection::index(&cols);
        let class = schema.class(CLASS_UI_DROP).context("the schema has no ui_drop class")?;
        let keys: Vec<u32> = class.definitions.iter().map(|d| d.key).collect();
        let (ids, names) = super::super::records::ids_and_names(big, &[(CLASS_UI_DROP, Kind::Event)], &keys)?;
        let mut by_path = BTreeMap::new();
        for (i, c) in cols.iter().enumerate().filter(|(_, c)| c.class == CLASS_UI_DROP) {
            if let Some(id) = ids.get(&(CLASS_UI_DROP, c.key)) {
                by_path.insert(id.path().to_string(), (c.key, i));
            }
        }
        let ref_names = names.refs.clone();
        let ref_keys = ref_names
            .iter()
            .map(|(&(class, collection), n)| (n.clone(), RefSpec { class, collection }))
            .collect::<BTreeMap<_, _>>();
        let refs = |r: &RefSpec| ref_text(&ref_names, r);
        let values: Vec<Vec<(u32, &collection::Field)>> = cols
            .iter()
            .enumerate()
            .filter(|(_, c)| c.class == CLASS_UI_DROP)
            .map(|(i, _)| collection::with_inherited(&cols, &decoded, &index, i))
            .collect::<ssx_formats::Result<_>>()?;
        let field_names = |k: u32| names.fields.get(&k).cloned();
        let targets = |c: u32| names.refs.iter().filter(|((cc, _), _)| *cc == c).map(|(_, n)| n.clone()).collect();
        let specs = ef::specs(&schema, class, &values, &field_names, &refs, &targets);
        let (drop_fields, event_fields) = ef::split(specs);
        let ghosts = big.join(GHOST_DB);
        let text = std::fs::read_to_string(&ghosts).with_context(|| format!("reading {}", ghosts.display()))?;
        let (rivals, all_rows) = parse_rows(&text);
        Ok(DiscEvents {
            by_path,
            drop_fields,
            event_fields,
            cols,
            decoded,
            index,
            ref_names,
            ref_keys,
            rivals,
            all_rows,
        })
    }

    fn fields(&self, kind: Kind) -> &[FieldSpec] {
        if kind == Kind::Drop { &self.drop_fields } else { &self.event_fields }
    }

    pub fn schema_of(&self, id: &ContentId) -> Option<RecordSchema> {
        let &(key, _) = self.by_path.get(id.path())?;
        let mut rs = vrec::schema(
            if id.kind() == Kind::Drop { "drop (ui_drop)" } else { "event (ui_drop)" },
            self.fields(id.kind()),
        );
        let own = self.record(id).ok()?;
        for (name, f) in rs.fields.iter_mut() {
            f.optional = !own.contains_key(name);
        }
        if id.kind() == Kind::Event {
            let n = self.rivals.get(&key).map_or(0, Vec::len);
            rs.fields.insert("rivals".into(), ef::rivals_field(&self.all_rows, n));
        }
        Some(rs)
    }

    pub fn record(&self, id: &ContentId) -> Result<toml::Table, String> {
        let &(key, i) = self.by_path.get(id.path()).ok_or_else(|| format!("{id}: no such ui_drop"))?;
        let values =
            collection::with_inherited(&self.cols, &self.decoded, &self.index, i).map_err(|e| e.to_string())?;
        let mut t = vrec::to_table(self.fields(id.kind()), &values, &|r| ref_text(&self.ref_names, r));
        if id.kind() == Kind::Event {
            let rows = self.rivals.get(&key).map_or(&[][..], Vec::as_slice);
            t.insert("rivals".into(), toml::Value::Table(ef::rivals_table(rows)));
        }
        Ok(t)
    }

    pub fn ref_of(&self, class: u32, s: &str) -> Option<RefSpec> {
        if s.is_empty() {
            return Some(RefSpec { class, collection: 0 });
        }
        self.ref_keys.get(s).copied().filter(|r| class == 0 || r.class == class)
    }

    pub fn checked(&self, id: &ContentId, table: &toml::Table) -> Result<(), String> {
        let schema = self.schema_of(id).ok_or_else(|| format!("{id}: no such ui_drop"))?;
        let problems = check_table(&schema, table, &Default::default(), Mode::Whole);
        if problems.is_empty() {
            return Ok(());
        }
        Err(problems.iter().take(8).map(|p| format!("{}: {}", p.path, p.message)).collect::<Vec<_>>().join("; "))
    }

    pub fn round_trips(&self, id: &ContentId, table: &toml::Table) -> Result<bool, String> {
        let &(_, i) = self.by_path.get(id.path()).ok_or_else(|| format!("{id}: no such ui_drop"))?;
        let mut decoded = self.decoded.clone();
        let mut t = table.clone();
        t.remove("rivals");
        let refs = |class: u32, s: &str| self.ref_of(class, s);
        let d = decoded.get_mut(i).ok_or_else(|| format!("{id}: no collection {i}"))?;
        vrec::apply(self.fields(id.kind()), &mut d.fields, &t, &refs)?;
        let values = collection::with_inherited(&self.cols, &decoded, &self.index, i).map_err(|e| e.to_string())?;
        let back = vrec::to_table(self.fields(id.kind()), &values, &|r| ref_text(&self.ref_names, r));
        Ok(back == t)
    }

    pub fn key_of(&self, id: &ContentId) -> Option<u32> {
        self.by_path.get(id.path()).map(|&(k, _)| k)
    }
}

fn ref_text(names: &BTreeMap<(u32, u32), String>, r: &RefSpec) -> String {
    if r.collection == 0 {
        return String::new();
    }
    names.get(&(r.class, r.collection)).cloned().unwrap_or_else(|| format!("0x{:08x}/0x{:08x}", r.class, r.collection))
}

type Rows = Vec<Vec<i64>>;

fn parse_rows(text: &str) -> (BTreeMap<u32, Rows>, Rows) {
    let mut by = BTreeMap::<u32, Vec<Vec<i64>>>::new();
    let mut all = Vec::new();
    for line in text.lines().skip(1) {
        let f: Vec<&str> = line.split(',').collect();
        let (Some(id), Some(row)) =
            (f.first().and_then(|s| s.trim().parse::<u32>().ok()), ef::row_values(&f[1.min(f.len())..]))
        else {
            continue;
        };
        all.push(row.clone());
        by.entry(id).or_default().push(row);
    }
    (by, all)
}

impl Source for DiscEvents {
    fn name(&self) -> &str {
        ssx_content::BASE_NAMESPACE
    }

    fn has(&self, id: &ContentId) -> bool {
        KINDS.contains(&id.kind())
            && id.namespace() == ssx_content::BASE_NAMESPACE
            && self.by_path.contains_key(id.path())
    }

    fn load(&self, id: &ContentId) -> Result<Asset, LoadError> {
        self.record(id).map(Asset::Record).map_err(|message| LoadError::Failed { id: id.clone(), message })
    }

    fn list(&self, kind: Kind) -> Vec<ContentId> {
        if !KINDS.contains(&kind) {
            return Vec::new();
        }
        let id = |p: &String| ContentId::new(ssx_content::BASE_NAMESPACE, kind, p).ok();
        self.by_path.keys().filter_map(id).collect()
    }
}

pub struct Routed {
    pub tables: BTreeMap<u32, (Option<toml::Table>, Option<toml::Table>)>,
}

#[derive(Default)]
pub struct EventCache {
    pub(in crate::content) disc: OnceLock<Option<Arc<DiscEvents>>>,
    pub(in crate::content) mounted: std::sync::atomic::AtomicBool,
    routed: Mutex<Option<(Generation, Arc<Routed>)>>,
}

pub(in crate::content) type Generation = (u64, u8);

impl GameContent {
    pub fn disc_events(&self) -> Option<Arc<DiscEvents>> {
        self.data
            .events
            .disc
            .get_or_init(|| match DiscEvents::load(self.big_root()) {
                Ok(d) => Some(Arc::new(d)),
                Err(e) => {
                    self.notice(format!("content: the disc's events and drops: {e:#}"));
                    None
                }
            })
            .clone()
    }

    pub(in crate::content) fn mount_events(&self, registry: &mut ssx_content::Registry) -> bool {
        use std::sync::atomic::Ordering::AcqRel;
        if !KINDS.iter().any(|&k| super::super::store::provides_kind(registry, k))
            || self.data.events.mounted.load(std::sync::atomic::Ordering::Acquire)
        {
            return false;
        }
        let Some(d) = self.disc_events() else { return false };
        if self.data.events.mounted.swap(true, AcqRel) {
            return false;
        }
        registry.mount(Layer::Base, 0, Box::new(super::super::store::Shared(d)));
        true
    }

    pub(in crate::content) fn mount_data_preview(&self, registry: &mut ssx_content::Registry) -> bool {
        let mut any = false;
        if super::tuning::KINDS.iter().any(|&k| super::super::store::provides_kind(registry, k))
            && let Some(d) = self.disc_tuning()
        {
            registry.mount(Layer::Base, 0, Box::new(super::super::store::Shared(d)));
            any = true;
        }
        if KINDS.iter().any(|&k| super::super::store::provides_kind(registry, k))
            && let Some(d) = self.disc_events()
        {
            registry.mount(Layer::Base, 0, Box::new(super::super::store::Shared(d)));
            any = true;
        }
        any
    }

    pub fn routed_events(&self) -> Arc<Routed> {
        let key = (self.generation(), self.sim_policy() as u8);
        let mut slot = self.data.events.routed.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some((k, r)) = slot.as_ref()
            && *k == key
        {
            return r.clone();
        }
        let r = Arc::new(self.route_events());
        *slot = Some((key, r.clone()));
        r
    }

    fn route_events(&self) -> Routed {
        let mut tables: BTreeMap<u32, (Option<toml::Table>, Option<toml::Table>)> = BTreeMap::new();
        let Some(disc) = self.disc_events() else { return Routed { tables } };
        for id in self.changed_ids(&KINDS) {
            let Some(key) = disc.key_of(&id).filter(|_| id.namespace() == ssx_content::BASE_NAMESPACE) else {
                self.notice(format!(
                    "content: {id}: no disc ui_drop of that name (a package changes events, not adds)"
                ));
                continue;
            };
            match self.event_record(&disc, &id) {
                Ok(Some((table, source))) => {
                    self.info(format!("content: {id}: from {source}"));
                    let slot = tables.entry(key).or_default();
                    if id.kind() == Kind::Drop { slot.0 = Some(table) } else { slot.1 = Some(table) }
                }
                Ok(None) => {}
                Err(why) => self.notice(format!("content: {id} not used ({why}); the disc's values are")),
            }
        }
        Routed { tables }
    }

    fn event_record(&self, disc: &DiscEvents, id: &ContentId) -> Result<Option<(toml::Table, String)>, String> {
        let base = disc.record(id)?;
        let (all, source, layer) = self.routed_record(id, base.clone())?;
        let (sim, _, _) = self.sim_record(id, base.clone())?;
        let schema = disc.schema_of(id).ok_or_else(|| format!("{id}: no such ui_drop"))?;
        let mut t = toml::Table::new();
        for (name, f) in &schema.fields {
            let from = if f.class == Class::Presentation && self.sim_policy() != ssx_content::policy::ClientMods::None {
                &all
            } else {
                &sim
            };
            if let Some(v) = from.get(name) {
                t.insert(name.clone(), v.clone());
            }
        }
        for (k, v) in all.iter().filter(|(k, _)| !schema.fields.contains_key(*k)) {
            t.insert(k.clone(), v.clone());
        }
        disc.checked(id, &t)?;
        if t == base && layer == Layer::Base {
            return Ok(None);
        }
        Ok(Some((t, source)))
    }
}

pub fn route_ui_drops(big_root: &Path, cols: &[Collection], decoded: &mut [DecodedCollection]) {
    let content = GameContent::of(big_root);
    if !KINDS.iter().any(|&k| content.packages_provide(k)) {
        return;
    }
    let routed = content.routed_events();
    let Some(disc) = content.disc_events() else { return };
    for (key, (drop, event)) in &routed.tables {
        let Some(i) = cols.iter().position(|c| c.class == CLASS_UI_DROP && c.key == *key) else { continue };
        let Some(d) = decoded.get_mut(i) else { continue };
        let refs = |class: u32, s: &str| disc.ref_of(class, s);
        for (fields, table) in [(&disc.drop_fields, drop), (&disc.event_fields, event)] {
            let Some(mut t) = table.clone() else { continue };
            t.remove("rivals");
            let mut own = d.fields.clone();
            match vrec::apply(fields, &mut own, &t, &refs) {
                Ok(()) => d.fields = own,
                Err(e) => content.notice(format!("content: ui_drop 0x{key:08x}: {e}; the disc's values are")),
            }
        }
    }
}

pub fn ghost_db_text(big_root: &Path) -> std::io::Result<String> {
    let text = crate::data_roots::read(big_root, "data/common/ghosts/ghostdb.csv")
        .and_then(|b| {
            String::from_utf8(b).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))
        })
        .or_else(|_| std::fs::read_to_string(big_root.join(GHOST_DB)))?;
    let content = GameContent::of(big_root);
    if !content.packages_provide(Kind::Event) {
        return Ok(text);
    }
    let routed = content.routed_events();
    let changed: BTreeMap<u32, Rows> = routed
        .tables
        .iter()
        .filter_map(|(k, (_, e))| {
            let rows = e.as_ref()?.get("rivals")?.as_table()?;
            Some((*k, ef::rivals_rows(rows)?))
        })
        .collect();
    if changed.is_empty() {
        return Ok(text);
    }
    let mut out = String::with_capacity(text.len());
    let mut written = std::collections::BTreeSet::new();
    for (n, line) in text.lines().enumerate() {
        let id = line.split(',').next().and_then(|s| s.trim().parse::<u32>().ok());
        match id.and_then(|k| changed.get(&k).map(|rows| (k, rows))) {
            Some((k, rows)) if n > 0 => {
                if written.insert(k) {
                    for r in rows {
                        let cols: Vec<String> = r.iter().map(i64::to_string).collect();
                        out.push_str(&format!("{k},{},\n", cols.join(",")));
                    }
                }
            }
            _ => {
                out.push_str(line);
                out.push('\n');
            }
        }
    }
    Ok(out)
}

pub type AttribDb = (Vec<Collection>, Vec<DecodedCollection>, std::collections::HashMap<(u32, u32), usize>);

pub fn read_attribdb(big_root: &Path) -> Result<AttribDb> {
    let read = |rel: &str| -> Result<(Vec<u8>, Vec<u8>)> {
        let p = big_root.join(rel);
        let bin = p.with_extension("bin");
        Ok((ssx_rules::read::read_file(&p)?, ssx_rules::read::read_file(&bin)?))
    };
    let (svlt, sbin) = read(SCHEMA)?;
    let schema = Schema::from_vault(&Vault::parse(&svlt, &sbin)?).context("reading the schema")?;
    let (vlt, bin) = read(ATTRIBDB)?;
    let vault = Vault::parse(&vlt, &bin).context("parsing attribdb.vlt")?;
    let cols = collection::collections(&vault)?;
    let mut decoded = cols.iter().map(|c| c.decode(&vault, &schema)).collect::<ssx_formats::Result<Vec<_>>>()?;
    route_ui_drops(big_root, &cols, &mut decoded);
    let index = collection::index(&cols);
    Ok((cols, decoded, index))
}

pub fn ui_drop_records(big_root: &Path) -> Result<Vec<ssx_rules::events::UiDropRecord>> {
    let (cols, decoded, index) = read_attribdb(big_root)?;
    ssx_rules::events::ui_drop_records(&cols, &decoded, &index)
}

pub fn ge_drop_of(big_root: &Path, drop: &str) -> Result<Option<u32>> {
    let key = ssx_formats::attrib::fnv1(drop.as_bytes());
    let (cols, decoded, index) = read_attribdb(big_root)?;
    let Some(i) = cols.iter().position(|c| c.class == CLASS_UI_DROP && c.key == key) else { return Ok(None) };
    let fields = collection::with_inherited(&cols, &decoded, &index, i)?;
    Ok(fields.iter().find(|(_, f)| f.key == ef::SIM_FIELDS[0].0).and_then(|(_, f)| match &f.data {
        collection::FieldData::Single(ssx_formats::attrib::Value::Ref(r)) if r.collection != 0 => Some(r.collection),
        _ => None,
    }))
}

pub fn ge_drops(big_root: &Path) -> Result<Vec<(String, u32)>> {
    let (cols, decoded, index) = read_attribdb(big_root)?;
    let ids = super::super::records::record_ids(big_root, &[(CLASS_UI_DROP, Kind::Event)])?;
    let mut out = Vec::new();
    for (i, c) in cols.iter().enumerate().filter(|(_, c)| c.class == CLASS_UI_DROP) {
        let Some(id) = ids.get(&(CLASS_UI_DROP, c.key)) else { continue };
        let fields = collection::with_inherited(&cols, &decoded, &index, i)?;
        let ge = fields.iter().find(|(_, f)| f.key == ef::SIM_FIELDS[0].0).and_then(|(_, f)| match &f.data {
            collection::FieldData::Single(ssx_formats::attrib::Value::Ref(r)) => Some(r.collection),
            _ => None,
        });
        out.push((id.path().to_string(), ge.unwrap_or(0)));
    }
    Ok(out)
}

pub fn event_id(name: &str) -> Option<ContentId> {
    let path = super::super::segment(name)?;
    ContentId::new(ssx_content::BASE_NAMESPACE, Kind::Event, &path).ok()
}
