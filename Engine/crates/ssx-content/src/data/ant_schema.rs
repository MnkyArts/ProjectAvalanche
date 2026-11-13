use std::collections::{BTreeSet, HashMap};

use ssx_formats::ant::{Guid, Object, Schema, Stream, TypeDesc, Value, builtin};

use super::ant::{HEADER, MAX_NESTING, chain};
use super::schema::{Class, FieldSchema, FieldType, RecordSchema, Scalar};

// PORT(choice): a package's float may lie within 4x of the disc's values of its field (sign and a zero bound kept); the
// readers of the 1985 float fields were not traced for their own limits; the fuzz target `data_ride` rode 35 min (21
// 392 inputs) of accepted records without a NaN or a panic; tuning: each field's bound from the code that reads it
// (ssx-physics params/*, score/scoring/*); the game checks nothing (it reads what the bundle holds).
pub const FLOAT_FACTOR: f64 = 4.0;
pub const LIST_FACTOR: usize = 2;
const MAX_TEXTS: usize = 4096;
const MAX_INTS: usize = 4096;

#[derive(Clone, Debug, Default)]
pub struct Stat {
    pub n: usize,
    pub min: f64,
    pub max: f64,
    pub ints: BTreeSet<i64>,
    pub texts: BTreeSet<String>,
    pub len_min: usize,
    pub len_max: usize,
    pub targets: BTreeSet<String>,
    pub null: bool,
    pub guids: BTreeSet<Guid>,
    pub guids_over: bool,
    pub ascending: bool,
}

impl Stat {
    fn number(&mut self, x: f64) {
        if self.n == 0 {
            (self.min, self.max) = (x, x);
        } else {
            self.min = self.min.min(x);
            self.max = self.max.max(x);
        }
        self.n += 1;
    }
}

#[derive(Clone, Debug, Default)]
pub struct Ranges {
    pub by: HashMap<(String, String), Stat>,
    pub classes: HashMap<Guid, String>,
    pub dynamic: BTreeSet<(String, String)>,
}

impl Ranges {
    pub fn of(stream: &Stream) -> Ranges {
        let schema = &stream.schema;
        let mut r = Ranges::default();
        for a in &stream.assets {
            if let Some(g) = a.object.guid(schema) {
                r.classes.insert(g, a.object.type_desc(schema).name.clone());
            }
        }
        for a in &stream.assets {
            let class = a.object.type_desc(schema).name.clone();
            r.object(schema, &class, "", &a.object, 0);
        }
        r
    }

    pub fn get(&self, class: &str, path: &str) -> Option<&Stat> {
        self.by.get(&(class.to_string(), path.to_string()))
    }

    fn stat(&mut self, class: &str, path: &str) -> &mut Stat {
        self.by
            .entry((class.to_string(), path.to_string()))
            .or_insert_with(|| Stat { ascending: true, ..Stat::default() })
    }

    fn object(&mut self, schema: &Schema, class: &str, path: &str, obj: &Object, depth: usize) {
        for o in chain(schema, obj) {
            for (f, v) in o.body.fields(schema) {
                if !HEADER.contains(&f.name.as_str()) {
                    let p = super::spans::child(path, &f.name);
                    if f.is_dynamic_array() {
                        self.dynamic.insert((class.to_string(), p.clone()));
                    }
                    self.value(schema, class, &p, v, depth + 1);
                }
            }
        }
    }

    fn value(&mut self, schema: &Schema, class: &str, path: &str, v: &Value, depth: usize) {
        if depth > MAX_NESTING {
            return;
        }
        let comps = |me: &mut Ranges, xs: &[f32]| {
            for (k, x) in xs.iter().enumerate() {
                me.stat(class, &format!("{path}#{k}")).number(f64::from(*x));
            }
        };
        match v {
            Value::Float(x) => self.stat(class, path).number(f64::from(*x)),
            Value::Vector2(a) => comps(self, a),
            Value::Vector3(a) => comps(self, a),
            Value::Vector4(a) | Value::Quaternion(a) => comps(self, a),
            Value::Matrix44(m) => m.iter().flatten().enumerate().for_each(|(k, x)| {
                self.stat(class, &format!("{path}#{k}")).number(f64::from(*x));
            }),
            Value::String(s) => {
                let st = self.stat(class, path);
                st.n += 1;
                if st.texts.len() < MAX_TEXTS {
                    st.texts.insert(s.clone());
                }
            }
            Value::Guid(g) => {
                let target = if g.is_null() { None } else { Some(self.classes.get(g).cloned().unwrap_or("?".into())) };
                let st = self.stat(class, path);
                st.n += 1;
                match target {
                    None => st.null = true,
                    Some(c) => {
                        st.targets.insert(c);
                        if st.guids.len() < MAX_TEXTS {
                            st.guids.insert(*g);
                        } else if !st.guids.contains(g) {
                            st.guids_over = true;
                        }
                    }
                }
            }
            Value::DataRef(Some(o)) => self.object(schema, class, path, o, depth + 1),
            Value::Struct(s) => {
                for (f, x) in s.fields(schema) {
                    let p = super::spans::child(path, &f.name);
                    if f.is_dynamic_array() {
                        self.dynamic.insert((class.to_string(), p.clone()));
                    }
                    self.value(schema, class, &p, x, depth + 1);
                }
            }
            Value::Array(items) => {
                let ascending =
                    items.windows(2).all(|w| matches!(w, [Value::Vector2(a), Value::Vector2(b)] if a[0] <= b[0]));
                let st = self.stat(class, path);
                st.len_min = if st.n == 0 { items.len() } else { st.len_min.min(items.len()) };
                st.len_max = st.len_max.max(items.len());
                st.ascending &= ascending || items.len() < 2;
                st.n += 1;
                let item = format!("{path}[]");
                items.iter().for_each(|x| self.value(schema, class, &item, x, depth + 1));
            }
            Value::Bytes(b) => {
                let st = self.stat(class, path);
                st.len_min = if st.n == 0 { b.len() } else { st.len_min.min(b.len()) };
                st.len_max = st.len_max.max(b.len());
                st.n += 1;
            }
            Value::DataRef(None) => {}
            other => {
                if let Some(x) = other.as_i64() {
                    let st = self.stat(class, path);
                    st.number(x as f64);
                    if st.ints.len() < MAX_INTS {
                        st.ints.insert(x);
                    }
                }
            }
        }
    }
}

// PORT(choice): the 244 integer fields other than these two accept only the disc's values of their field (an integer
// may index a table or pick a branch; two readers traced: banks.rs TrickComboMaxCount, jump.rs obstacle_fan
// 0x82CE128C); tuning: each integer's reader traced for the values it handles; the game checks nothing (it reads what
// the bundle holds).
pub const INT_RANGES: &[(&str, &str, i64, i64, &str)] = &[
    (
        "ScoringSceneOpAsset",
        "TrickComboMaxCount",
        0,
        10_000,
        "ssx-physics score/scoring/banks.rs: the combo count is (count + 1).min(this), never an index; the combo bonus \
         table (Lua kCOMBO_BONUS) stops growing at 500",
    ),
    (
        "SnowboarderPhysicsQueriesSceneOpAsset",
        "NumJumpLaunchProbeRays",
        0,
        16,
        "ssx-physics riding/jump.rs obstacle_fan (0x82CE128C): n / 2 + 1 fan steps of two rays each, 0 skips the fan; \
         16 holds a launch to 18 probes (the disc's 5: 6)",
    ),
];

pub const FLOAT_RANGES: &[(&str, &str, f64, f64, &str)] = &[(
    "GameLoopSceneOpAsset",
    "TimeDilationHeliTrickChance",
    0.99,
    1.0,
    "ssx-physics level/session/dilation.rs: the port holds the heli trick's time dilation to the disc's (one curve, \
     chance 1: the RAND_Value draws 0x82CFF844 / 0x82CFF888 decide nothing) and refuses a chance below 0.99",
)];

pub const LINKED_FLOATS: &[(&str, &str, &str)] = &[(
    "ssx:tuning/aerialcontrollerasset/aerial_airborne",
    "ssx:tuning/aerialcontrollerasset/aerial_jump",
    "ssx-physics riding/air.rs AerialTuning::load reads the two variants' floats as one (equal on the disc) and \
     refuses them unequal: change both alike",
)];

pub fn floats_differ(a: &toml::Table, b: &toml::Table) -> Option<String> {
    let narrow = |v: &toml::Value| v.as_float().map(|x| (x as f32).to_bits());
    let one_way = |p: &toml::Table, q: &toml::Table| {
        p.iter().filter(|(_, v)| v.is_float()).find_map(|(k, v)| {
            (narrow(v) != q.get(k).and_then(narrow)).then(|| {
                let other = q.get(k).map_or_else(|| "nothing".to_string(), |w| w.to_string());
                format!(
                    "{}: {} against {}",
                    ssx_core::text::quoted(k),
                    ssx_core::text::shown(&v.to_string()),
                    ssx_core::text::shown(&other)
                )
            })
        })
    };
    one_way(a, b).or_else(|| one_way(b, a))
}

pub fn record_schema(schema: &Schema, obj: &Object, ranges: &Ranges, class: Class) -> RecordSchema {
    let owner = obj.type_desc(schema).name.clone();
    let mut rs = RecordSchema::new(&owner);
    let b = Builder { schema, ranges, owner: &owner, class };
    b.object(&mut rs, "", obj, 0);
    rs
}

struct Builder<'a> {
    schema: &'a Schema,
    ranges: &'a Ranges,
    owner: &'a str,
    class: Class,
}

impl Builder<'_> {
    fn object(&self, rs: &mut RecordSchema, path: &str, obj: &Object, depth: usize) {
        for o in chain(self.schema, obj) {
            for (f, v) in o.body.fields(self.schema) {
                if HEADER.contains(&f.name.as_str()) || rs.fields.contains_key(&f.name) {
                    continue;
                }
                let p = super::spans::child(path, &f.name);
                let (ty, source) = self.field(&p, f.type_index, f.is_dynamic_array() || f.count > 1, v, depth + 1);
                let note = self.schema.types.get(f.type_index).map_or_else(String::new, |t| t.name.clone());
                rs.fields.insert(f.name.clone(), FieldSchema { ty, class: self.class, source, note, optional: false });
            }
        }
    }

    fn stat(&self, path: &str) -> Option<&Stat> {
        self.ranges.get(self.owner, path)
    }

    fn field(&self, path: &str, ti: usize, list: bool, v: &Value, depth: usize) -> (FieldType, String) {
        if depth > MAX_NESTING {
            return (FieldType::Scalar(Scalar::Fixed("nested too deeply")), String::new());
        }
        let Some(t) = self.schema.types.get(ti) else {
            return (FieldType::Scalar(Scalar::Fixed("a type the schema lacks")), String::new());
        };
        if !list {
            return self.single(path, t, v, depth);
        }
        let st = self.stat(path);
        let (lo, hi) = st.map_or((0, 0), |s| (s.len_min, s.len_max));
        if let Value::Bytes(b) = v {
            let max = (hi * LIST_FACTOR).min(crate::limits::MAX_LIST).max(b.len());
            return (FieldType::Scalar(Scalar::Hex { max }), format!("the disc's {lo} … {hi} bytes, × {LIST_FACTOR}"));
        }
        let items: &[Value] = match v {
            Value::Array(a) => a,
            _ => &[],
        };
        if t.id == builtin::GUID {
            return self.own_references(items);
        }
        let fixed = !self.dynamic(path);
        let (min, max, source) = if fixed {
            (items.len(), items.len(), format!("a fixed array of {}", items.len()))
        } else if items.is_empty() {
            (0, 0, "the disc's list is empty here: no item to grow it from".to_string())
        } else {
            let max = (hi * LIST_FACTOR).clamp(items.len(), crate::limits::MAX_LIST.max(items.len()));
            (lo.min(items.len()).max(1), max, format!("the disc's {lo} … {hi} items, × {LIST_FACTOR}"))
        };
        let item = format!("{path}[]");
        let (elem, esource) = match items.first() {
            Some(x) => self.single(&item, t, x, depth + 1),
            None => (FieldType::Scalar(Scalar::Fixed("no item on the disc")), String::new()),
        };
        let ascending = st.is_some_and(|s| s.ascending) && t.id == builtin::VECTOR2 && !fixed;
        let source = if esource.is_empty() { source } else { format!("{source}; items: {esource}") };
        let source =
            if ascending { format!("{source}; ascending x (CurvePoints_EvaluateLinear 0x82CB6CC0)") } else { source };
        (FieldType::List { elem: Box::new(elem), min, max, ascending }, source)
    }

    fn dynamic(&self, path: &str) -> bool {
        self.ranges.dynamic.contains(&(self.owner.to_string(), path.to_string()))
    }

    fn single(&self, path: &str, t: &TypeDesc, v: &Value, depth: usize) -> (FieldType, String) {
        let s = |x: Scalar| FieldType::Scalar(x);
        let comps = |n: usize| -> (FieldType, String) {
            let items = (0..n).map(|k| s(self.float(&format!("{path}#{k}")).0)).collect();
            (FieldType::Tuple(items), self.float(&format!("{path}#0")).1)
        };
        match (t.id, v) {
            (builtin::BOOL, _) => (s(Scalar::Bool), String::new()),
            (builtin::FLOAT, _) => {
                let (x, src) = self.float(path);
                (s(x), src)
            }
            (builtin::VECTOR2, _) => comps(2),
            (builtin::VECTOR3, _) => comps(3),
            (builtin::VECTOR4 | builtin::QUATERNION, _) => comps(4),
            (builtin::MATRIX44, _) => {
                let rows = (0..4)
                    .map(|r| {
                        FieldType::Tuple((0..4).map(|c| s(self.float(&format!("{path}#{}", r * 4 + c)).0)).collect())
                    })
                    .collect();
                (FieldType::Tuple(rows), self.float(&format!("{path}#0")).1)
            }
            (builtin::GUID, _) => self.guid(path, v),
            (builtin::STRING, _) => {
                let texts: Vec<String> = self.stat(path).map(|s| s.texts.iter().cloned().collect()).unwrap_or_default();
                let full = texts.len() < MAX_TEXTS;
                let max = texts.iter().map(String::len).max().unwrap_or(0).max(1) * 2;
                let one_of = if full { texts } else { Vec::new() };
                (s(Scalar::Text { max: max.min(crate::limits::MAX_LONG_TEXT), one_of }), "the disc's texts here".into())
            }
            (builtin::DATA_REF, Value::DataRef(Some(o))) => {
                let mut rs = RecordSchema::new(&o.type_desc(self.schema).name);
                self.object(&mut rs, path, o, depth + 1);
                (FieldType::Table(Box::new(rs)), String::new())
            }
            (builtin::DATA_REF, _) => (s(Scalar::Fixed("null on the disc: no object to set")), String::new()),
            (id, _) if id < builtin::COUNT => self.int(path, id),
            (_, Value::Struct(st)) => {
                let mut rs = RecordSchema::new(&t.name);
                for (f, x) in st.fields(self.schema) {
                    let p = super::spans::child(path, &f.name);
                    let (ty, source) = self.field(&p, f.type_index, f.is_dynamic_array() || f.count > 1, x, depth + 1);
                    let note = self.schema.types.get(f.type_index).map_or_else(String::new, |t| t.name.clone());
                    rs.fields
                        .insert(f.name.clone(), FieldSchema { ty, class: self.class, source, note, optional: false });
                }
                (FieldType::Table(Box::new(rs)), String::new())
            }
            _ => (s(Scalar::Fixed("a value of another type than its descriptor's")), String::new()),
        }
    }

    // PORT(choice): a package's reference may name only an asset the disc names at the same field of the same class, of
    // the class its own names; the port's readers were written for the disc's references and one checks which asset it
    // gets (world/surface.rs BoardChatter: FLT_carving_state_GrindBlendValue …, found by the data_ride fuzz target:
    // another blackboard variable of the same class failed the load); tuning: each reference field's reader traced for
    // the assets it handles; the game loads no packages.
    fn guid(&self, path: &str, v: &Value) -> (FieldType, String) {
        let st = self.stat(path);
        let null = st.is_none_or(|s| s.null);
        let class_of = |g: &Guid| self.ranges.classes.get(g).cloned();
        let own = match v {
            Value::Guid(g) if !g.is_null() => class_of(g),
            _ => None,
        };
        let classes: Vec<String> = match &own {
            Some(c) => vec![c.clone()],
            None => st.map(|s| s.targets.iter().filter(|c| *c != "?").cloned().collect()).unwrap_or_default(),
        };
        let one_of: Option<Vec<[u8; 16]>> = st.filter(|s| !s.guids_over).map(|s| {
            s.guids.iter().filter(|g| class_of(g).is_some_and(|c| classes.contains(&c))).map(|g| g.0).collect()
        });
        let src = match &one_of {
            Some(v) => format!("the disc's {} reference(s) here to {}", v.len(), classes.join(", ")),
            None => format!("the classes the disc's references here point to: {}", classes.join(", ")),
        };
        (FieldType::Scalar(Scalar::Guid { classes, null, one_of }), src)
    }

    fn own_references(&self, items: &[Value]) -> (FieldType, String) {
        let item = |x: &Value| match x {
            Value::Guid(g) if !g.is_null() => {
                let classes = self.ranges.classes.get(g).cloned().into_iter().collect();
                FieldType::Scalar(Scalar::Guid { classes, null: false, one_of: Some(vec![g.0]) })
            }
            _ => FieldType::Scalar(Scalar::Guid { classes: Vec::new(), null: true, one_of: Some(Vec::new()) }),
        };
        let src =
            format!("the record's own {} reference(s), in order: a list of references stays the disc's", items.len());
        (FieldType::Tuple(items.iter().map(item).collect()), src)
    }

    fn float(&self, path: &str) -> (Scalar, String) {
        if let Some(&(_, _, lo, hi, src)) = FLOAT_RANGES.iter().find(|(c, p, ..)| *c == self.owner && *p == path) {
            return (Scalar::Float { min: lo, max: hi, f32: true }, src.to_string());
        }
        let Some(st) = self.stat(path).filter(|s| s.n > 0) else {
            return (Scalar::Float { min: 0.0, max: 0.0, f32: true }, "no value on the disc".into());
        };
        let (min, max) = float_range(st.min, st.max);
        let (lo, hi) = (st.min as f32, st.max as f32);
        let src = format!("the disc's {lo} … {hi} over {} values; {FLOAT_FACTOR}× with the sign kept", st.n);
        (Scalar::Float { min, max, f32: true }, src)
    }

    fn int(&self, path: &str, id: u32) -> (FieldType, String) {
        let s = |x: Scalar| FieldType::Scalar(x);
        if let Some(&(_, _, lo, hi, src)) = INT_RANGES.iter().find(|(c, p, ..)| *c == self.owner && *p == path) {
            return (s(Scalar::Int { min: lo, max: hi }), src.to_string());
        }
        match self.stat(path).filter(|s| !s.ints.is_empty() && s.ints.len() < MAX_INTS) {
            Some(st) => (s(Scalar::IntOf(st.ints.iter().copied().collect())), "the disc's values here".into()),
            None => {
                let (min, max) = int_width(id);
                (s(Scalar::Int { min, max }), "the type's width".into())
            }
        }
    }
}

pub fn float_range(min: f64, max: f64) -> (f64, f64) {
    let f = FLOAT_FACTOR;
    let lo = if min > 0.0 { min / f } else { min * f };
    let hi = if max < 0.0 { max / f } else { max * f };
    let narrow = |x: f64| f64::from(x.clamp(-f64::from(f32::MAX), f64::from(f32::MAX)) as f32);
    (narrow(lo), narrow(hi))
}

fn int_width(id: u32) -> (i64, i64) {
    match id {
        builtin::INT8 => (i8::MIN.into(), i8::MAX.into()),
        builtin::UINT8 => (0, u8::MAX.into()),
        builtin::INT16 => (i16::MIN.into(), i16::MAX.into()),
        builtin::UINT16 => (0, u16::MAX.into()),
        builtin::INT32 => (i32::MIN.into(), i32::MAX.into()),
        builtin::UINT32 => (0, u32::MAX.into()),
        _ => (i64::MIN, i64::MAX),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn linked_records_compare_their_floats() {
        let a: toml::Table = "x = 0.1\ny = 2.0\nn = 3\n".parse().unwrap();
        let same: toml::Table = "x = 0.10000000000000001\ny = 2.0\nn = 4\n".parse().unwrap();
        assert_eq!(super::floats_differ(&a, &same), None, "the integers are not compared");
        let near: toml::Table = "x = 0.1000000001\ny = 2.0\n".parse().unwrap();
        assert_eq!(super::floats_differ(&a, &near), None, "equal as 32-bit floats");
        let other: toml::Table = "x = 0.1\ny = 2.5\n".parse().unwrap();
        assert_eq!(super::floats_differ(&a, &other).as_deref(), Some("\"y\": 2.0 against 2.5"));
        let more: toml::Table = "x = 0.1\ny = 2.0\nz = 1.0\n".parse().unwrap();
        assert!(super::floats_differ(&a, &more).is_some_and(|m| m.contains("nothing")));
    }
}
