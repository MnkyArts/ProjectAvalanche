use std::collections::{BTreeMap, BTreeSet};

use ssx_content::data::schema::{Class, FieldSchema, FieldType, RecordSchema, Scalar};
use ssx_content::data::vault::{FieldSpec, field_name};
use ssx_formats::attrib::collection::{Field, FieldData};
use ssx_formats::attrib::value::{Value, ValueKind};
use ssx_formats::attrib::{Class as VaultClass, Schema};

pub const SIM_FIELDS: [(u32, &str); 10] = [
    (0xB9C2_CFC5, "the drop (ge_drop) the event is ridden on: the world, its starts and gates"),
    (0x146B_5937, "the event type (UI_EventType: race, trick, survive …)"),
    (0x555E_EBD2, "the drop type (UI_DropType)"),
    (0x0B03_FC19, "the message parameter the objective scripts read"),
    (0x0E24_143E, "the event's scripts (eventtype_*.lua and the drop's)"),
    (0x7E5E_ED53, "the forced loadout (six RefSpecs: five items and the rider)"),
    (0x98E9_EBCC, "the loadout slot counts"),
    (0xB1C0_1F64, "the avalanche scripts"),
    (0xC211_FF58, "the rivals' riders (character)"),
    (0xD4D5_8427, "the required equipment (UI_DropReqEquipment)"),
];

pub const DROP_FIELDS: [u32; 11] = [
    0xB9C2_CFC5,
    0x555E_EBD2,
    0x2318_0B90,
    0x9A4C_11BE,
    0x8296_DA5A,
    0x45B5_F74A,
    0x5A0B_DB1E,
    0xC8B4_8ED3,
    0xF2C4_3D5F,
    0xE5FA_2B74,
    0x6CA1_C684,
];

pub const NAME_FIELD: u32 = 0xAF27_A974;

const MAX_SHOWN_TEXT: usize = ssx_content::limits::MAX_SHORT_TEXT;

#[derive(Default)]
struct Seen {
    ints: BTreeSet<i64>,
    floats: Vec<(f64, f64)>,
    texts: BTreeSet<String>,
    len: (usize, usize),
    any: bool,
}

pub fn specs(
    schema: &Schema,
    class: &VaultClass,
    values: &[Vec<(u32, &Field)>],
    names: &dyn Fn(u32) -> Option<String>,
    refs: &dyn Fn(&ssx_formats::attrib::RefSpec) -> String,
    targets: &dyn Fn(u32) -> Vec<String>,
) -> Vec<FieldSpec> {
    let mut out = Vec::new();
    for d in &class.definitions {
        if d.key == NAME_FIELD {
            continue;
        }
        let Some(t) = schema.type_info(d.type_hash) else { continue };
        let mut seen = Seen::default();
        for v in values {
            if let Some((_, f)) = v.iter().find(|(_, f)| f.key == d.key) {
                record(&mut seen, &f.data, refs);
            }
        }
        let sim = SIM_FIELDS.iter().find(|(k, _)| *k == d.key);
        let fclass = if sim.is_some() { Class::Simulation } else { Class::Presentation };
        let (elem, source) = elem_type(t.kind, &seen, fclass, targets);
        let array = d.is_array();
        let ty = if array {
            let cap = usize::from(d.max_count).min(ssx_content::limits::MAX_LIST);
            let (min, max) = if fclass == Class::Simulation { seen.len } else { (0, cap) };
            FieldType::List { elem: Box::new(elem), min, max: max.min(cap), ascending: false }
        } else {
            elem
        };
        let note = format!("{}{}", t.name, sim.map(|(_, w)| format!(": {w}")).unwrap_or_default());
        let name = field_name(d.key, names(d.key).as_deref());
        out.push(FieldSpec {
            key: d.key,
            name,
            type_hash: d.type_hash,
            kind: t.kind,
            array,
            ty,
            class: fclass,
            source,
            note,
        });
    }
    out
}

fn record(seen: &mut Seen, data: &FieldData, refs: &dyn Fn(&ssx_formats::attrib::RefSpec) -> String) {
    let items: Vec<&Value> = match data {
        FieldData::Single(v) => vec![v],
        FieldData::Array(vs) => vs.iter().collect(),
    };
    seen.len =
        if seen.any { (seen.len.0.min(items.len()), seen.len.1.max(items.len())) } else { (items.len(), items.len()) };
    seen.any = true;
    for v in items {
        match v {
            Value::Text(t) => {
                seen.texts.insert(t.clone().unwrap_or_default());
            }
            Value::Ref(r) => {
                seen.texts.insert(refs(r));
            }
            Value::Float(x) => seen.floats.push((f64::from(*x), 0.0)),
            Value::Vector2(a) => seen.floats.extend(a.iter().map(|x| (f64::from(*x), 0.0))),
            Value::Vector3(a) => seen.floats.extend(a.iter().map(|x| (f64::from(*x), 0.0))),
            Value::Vector4(a) => seen.floats.extend(a.iter().map(|x| (f64::from(*x), 0.0))),
            Value::Raw { bytes, pointers } if pointers.is_empty() && bytes.len() == 4 => {
                seen.ints.insert(u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]).into());
            }
            Value::Bool(_) | Value::Raw { .. } => {}
            other => {
                if let Some(i) = int(other) {
                    seen.ints.insert(i);
                }
            }
        }
    }
}

fn int(v: &Value) -> Option<i64> {
    Some(match v {
        Value::Int8(x) => (*x).into(),
        Value::UInt8(x) => (*x).into(),
        Value::Int16(x) => (*x).into(),
        Value::UInt16(x) => (*x).into(),
        Value::Int32(x) => (*x).into(),
        Value::UInt32(x) => (*x).into(),
        Value::Int64(x) => *x,
        Value::Key(x) => (*x).into(),
        _ => return None,
    })
}

fn elem_type(kind: ValueKind, seen: &Seen, class: Class, targets: &dyn Fn(u32) -> Vec<String>) -> (FieldType, String) {
    let s = |x: Scalar| FieldType::Scalar(x);
    let disc = "the disc's values of this field over every ui_drop".to_string();
    let range = || {
        let (lo, hi) = seen.ints.iter().fold((i64::MAX, i64::MIN), |(a, b), &x| (a.min(x), b.max(x)));
        let (lo, hi) = if seen.ints.is_empty() { (0, 0) } else { (lo, hi) };
        let (min, max) = ssx_content::data::ant_schema::float_range(lo as f64, hi as f64);
        (min.ceil() as i64, max.floor() as i64)
    };
    let (lo, hi) = width(kind);
    match kind {
        ValueKind::Bool => (s(Scalar::Bool), String::new()),
        // PORT(choice): a ui_drop's float or presentation integer may lie within 4x of the disc's values of its field,
        // a simulation integer only at one of them; the readers of the 30 event and 11 drop fields were not traced for
        // their limits; tuning: each field's reader (Results_Process 0x8251D5D8, the flow manager's drop setup) traced
        // for the values it handles; the game checks nothing (it reads the vault).
        ValueKind::Float | ValueKind::Vector2 | ValueKind::Vector3 | ValueKind::Vector4 => {
            let (a, b) = seen.floats.iter().fold((f64::MAX, f64::MIN), |(a, b), &(x, _)| (a.min(x), b.max(x)));
            let (a, b) = if seen.floats.is_empty() { (0.0, 0.0) } else { (a, b) };
            let (min, max) = ssx_content::data::ant_schema::float_range(a, b);
            let f = Scalar::Float { min, max, f32: true };
            let n = match kind {
                ValueKind::Vector2 => 2,
                ValueKind::Vector3 => 3,
                ValueKind::Vector4 => 4,
                _ => return (s(f), format!("{disc}: {a} … {b}, ×4 with the sign kept")),
            };
            (FieldType::Tuple(vec![s(f); n]), format!("{disc}: {a} … {b}, ×4 with the sign kept"))
        }
        ValueKind::Text if class == Class::Simulation => {
            (s(Scalar::Text { max: MAX_SHOWN_TEXT, one_of: seen.texts.iter().cloned().collect() }), disc)
        }
        ValueKind::Text => (s(Scalar::Text { max: MAX_SHOWN_TEXT, one_of: Vec::new() }), String::new()),
        ValueKind::RefSpec | ValueKind::ClassRefSpec(_) => {
            let mut one_of: BTreeSet<String> = seen.texts.clone();
            if let ValueKind::ClassRefSpec(c) = kind
                && class == Class::Presentation
            {
                one_of.extend(targets(c));
            }
            let src =
                if class == Class::Simulation { disc } else { format!("{disc} and every collection of the class") };
            (s(Scalar::Text { max: MAX_SHOWN_TEXT, one_of: one_of.into_iter().collect() }), src)
        }
        ValueKind::Raw => (s(Scalar::IntOf(seen.ints.iter().copied().collect())), format!("{disc} (an enum)")),
        _ if class == Class::Simulation => (s(Scalar::IntOf(seen.ints.iter().copied().collect())), disc),
        _ => {
            let (min, max) = range();
            (s(Scalar::Int { min: min.max(lo), max: max.min(hi) }), format!("{disc}, ×4 with the sign kept"))
        }
    }
}

fn width(kind: ValueKind) -> (i64, i64) {
    match kind {
        ValueKind::Int8 => (i8::MIN.into(), i8::MAX.into()),
        ValueKind::UInt8 => (0, u8::MAX.into()),
        ValueKind::Int16 => (i16::MIN.into(), i16::MAX.into()),
        ValueKind::UInt16 => (0, u16::MAX.into()),
        ValueKind::Int32 => (i32::MIN.into(), i32::MAX.into()),
        ValueKind::UInt32 | ValueKind::Key | ValueKind::Raw => (0, u32::MAX.into()),
        _ => (i64::MIN, i64::MAX),
    }
}

pub const RIVAL_COLUMNS: [&str; 15] = [
    "startingidx",
    "ghostid",
    "score",
    "tuned_score",
    "characterid",
    "catchup",
    "split0",
    "split1",
    "split2",
    "split3",
    "split4",
    "split5",
    "split6",
    "split7",
    "split8",
];

pub fn rival_schema(rows: &[Vec<i64>]) -> RecordSchema {
    let mut rs = RecordSchema::new("ghostdb row");
    for (c, name) in RIVAL_COLUMNS.iter().enumerate() {
        let values: BTreeSet<i64> = rows.iter().filter_map(|r| r.get(c).copied()).collect();
        let (ty, source) = match *name {
            // PORT(choice): a rival's score within 4x of the disc's 728 rows' scores (ms on a race, points or mm
            // otherwise), every other column one of the disc's values; the readers (GhostManager, Results_Process
            // 0x8251D5D8: a rank by comparison) handle any u32, so the ranking needs none; the bound keeps a package
            // from making a medal impossible or free; the game loads no packages.
            "score" | "tuned_score" => {
                let pos: Vec<i64> = values.iter().copied().filter(|&v| v >= 0).collect();
                let (lo, hi) = (pos.first().copied().unwrap_or(0), pos.last().copied().unwrap_or(0));
                let (min, max) = ssx_content::data::ant_schema::float_range(lo as f64, hi as f64);
                let min = if *name == "tuned_score" { -1 } else { min.ceil() as i64 };
                let none = if *name == "tuned_score" { "; -1: no tuned score" } else { "" };
                let src = format!("the disc's scores {lo} … {hi} (ms or points), ×4{none}");
                (Scalar::Int { min, max: (max.floor() as i64).min(u32::MAX.into()) }, src)
            }
            _ => (Scalar::IntOf(values.into_iter().collect()), "the disc's values of this column".to_string()),
        };
        let note = if *name == "score" { "ms for races, points or mm otherwise" } else { "" };
        let fs = FieldSchema {
            ty: FieldType::Scalar(ty),
            class: Class::Simulation,
            source,
            note: note.into(),
            optional: false,
        };
        rs.fields.insert(name.to_string(), fs);
    }
    rs
}

pub fn rivals_field(rows: &[Vec<i64>], count: usize) -> FieldSchema {
    let row = rival_schema(rows);
    let mut table = RecordSchema::new("rivals");
    for i in 0..count {
        let fs = FieldSchema {
            ty: FieldType::Table(Box::new(row.clone())),
            class: Class::Simulation,
            source: "a ghostdb.csv row of this ui_drop".into(),
            note: String::new(),
            optional: false,
        };
        table.fields.insert(i.to_string(), fs);
    }
    FieldSchema {
        ty: FieldType::Table(Box::new(table)),
        class: Class::Simulation,
        source: "ghostdb.csv's rows of this ui_drop (GhostManager_RivalsForDrop 0x824B6648); their count kept".into(),
        note: "the rivals by row (0 first): their scores are the medal times / points".into(),
        optional: false,
    }
}

pub fn rivals_table(rows: &[Vec<i64>]) -> toml::Table {
    rows.iter().enumerate().map(|(i, r)| (i.to_string(), toml::Value::Table(row_table(r)))).collect()
}

pub fn rivals_rows(t: &toml::Table) -> Option<Vec<Vec<i64>>> {
    let mut rows: Vec<(usize, Vec<i64>)> = Vec::with_capacity(t.len().min(RIVALS_MAX));
    for (k, v) in t {
        rows.push((k.parse().ok()?, table_row(v.as_table()?)?));
    }
    rows.sort_by_key(|(i, _)| *i);
    Some(rows.into_iter().map(|(_, r)| r).collect())
}

const RIVALS_MAX: usize = 64;

pub fn split(specs: Vec<FieldSpec>) -> (Vec<FieldSpec>, Vec<FieldSpec>) {
    specs.into_iter().partition(|f| DROP_FIELDS.contains(&f.key))
}

pub fn row_values(cols: &[&str]) -> Option<Vec<i64>> {
    let v: Option<Vec<i64>> = cols.iter().take(RIVAL_COLUMNS.len()).map(|s| s.trim().parse::<i64>().ok()).collect();
    v.filter(|v| v.len() == RIVAL_COLUMNS.len())
}

pub fn row_table(v: &[i64]) -> toml::Table {
    RIVAL_COLUMNS.iter().zip(v).map(|(k, x)| (k.to_string(), toml::Value::Integer(*x))).collect()
}

pub fn table_row(t: &toml::Table) -> Option<Vec<i64>> {
    RIVAL_COLUMNS.iter().map(|k| t.get(*k).and_then(toml::Value::as_integer)).collect()
}

pub fn by_name(specs: &[FieldSpec]) -> BTreeMap<&str, &FieldSpec> {
    specs.iter().map(|f| (f.name.as_str(), f)).collect()
}
