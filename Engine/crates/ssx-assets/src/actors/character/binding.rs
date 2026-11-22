use ssx_formats::attrib::value::ValueKind;
use ssx_formats::attrib::{Field, RefSpec};

use super::*;
use crate::vault_db::FieldOverrides;

const CLASS_PART_BINDING: u32 = fnv1("ge_partbinding");
const FIELD_CLASS_KEY: u32 = fnv1("classkey");
const FIELD_SOURCE_PATH: u32 = fnv1("sourceattributepath");
const FIELD_SOURCE_INDICES: u32 = fnv1("sourceattributepathindices");
const FIELD_TARGET_PATH: u32 = fnv1("targetattributepath");
const FIELD_TARGET_INDICES: u32 = fnv1("targetattributepathindices");
pub(crate) const FIELD_SNOWBOARD_ID: u32 = fnv1("ID");
const VECTOR_TOLERANCE: f32 = 0.000_392_156_86;
// PORT(choice): a path of more than 64 steps or more than 256 bindings of one class is refused (hostile package
// vaults; the disc's paths have 2 and 5 steps, one binding); the console walked any length its vault held.
const MAX_PATH_STEPS: usize = 64;
const MAX_BINDINGS: usize = 256;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Step {
    field: u32,
    index: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct PartBinding {
    key: u32,
    source: Vec<Step>,
    target: Vec<Step>,
}

pub(crate) trait Store {
    fn field(&self, class: u32, coll: u32, key: u32) -> Result<Option<&Field>>;
    fn kind(&self, type_hash: u32) -> Option<ValueKind>;
    fn has(&self, class: u32, coll: u32) -> bool;
    fn of_class(&self, class: u32) -> Result<Vec<(u32, Vec<&Field>)>>;
    fn name(&self, key: u32) -> String;
}

impl Store for Db {
    fn field(&self, class: u32, coll: u32, key: u32) -> Result<Option<&Field>> {
        Db::field(self, class, coll, key)
    }

    fn kind(&self, type_hash: u32) -> Option<ValueKind> {
        Db::kind(self, type_hash)
    }

    fn has(&self, class: u32, coll: u32) -> bool {
        self.find(class, coll).is_some()
    }

    fn of_class(&self, class: u32) -> Result<Vec<(u32, Vec<&Field>)>> {
        self.collections_of_all(class).into_iter().map(|(k, _)| Ok((k, self.fields(class, k)?))).collect()
    }

    fn name(&self, key: u32) -> String {
        self.collection_name(key)
    }
}

fn keys(f: &Field) -> Vec<u32> {
    let values: Vec<&Value> = match &f.data {
        ssx_formats::attrib::FieldData::Single(v) => vec![v],
        ssx_formats::attrib::FieldData::Array(vs) => vs.iter().collect(),
    };
    values.into_iter().map(|v| if let Value::Key(k) = v { *k } else { 0 }).collect()
}

fn path(fields: &[&Field], path: u32, indices: u32) -> Vec<Step> {
    let get = |k: u32| fields.iter().find(|f| f.key == k).map(|f| keys(f)).unwrap_or_default();
    let index = get(indices);
    get(path)
        .into_iter()
        .enumerate()
        .map(|(i, field)| Step { field, index: index.get(i).copied().unwrap_or(0) })
        .collect()
}

fn bindings_of(db: &impl Store, class: u32, problems: &mut Vec<String>) -> Result<Vec<PartBinding>> {
    let mut out = Vec::new();
    // PORT(choice): bindings with targets of one length apply in the database's collection order (a track's vault, the
    // mounted packages', `ge_attribdb`); the console took its class table's slot order (0x83147AD0), and only a
    // mod's second binding of a class meets the difference.
    for (key, fields) in db.of_class(CLASS_PART_BINDING)? {
        let class_key = fields.iter().find(|f| f.key == FIELD_CLASS_KEY).and_then(|f| keys(f).first().copied());
        if class_key != Some(class) {
            continue;
        }
        let binding = PartBinding {
            key,
            source: path(&fields, FIELD_SOURCE_PATH, FIELD_SOURCE_INDICES),
            target: path(&fields, FIELD_TARGET_PATH, FIELD_TARGET_INDICES),
        };
        if binding.source.len() > MAX_PATH_STEPS || binding.target.len() > MAX_PATH_STEPS {
            problems.push(format!("part binding {}: a path of more than {MAX_PATH_STEPS} steps", db.name(key)));
            continue;
        }
        if out.len() == MAX_BINDINGS {
            problems.push(format!("more than {MAX_BINDINGS} part bindings of {}", db.name(class)));
            break;
        }
        out.push(binding);
    }
    out.sort_by_key(|b| b.target.len());
    Ok(out)
}

type At = (u32, u32);

fn index(
    db: &impl Store,
    overrides: &FieldOverrides,
    root: At,
    key: u32,
    problems: &mut Vec<String>,
) -> Result<Option<u32>> {
    let Some(f) = db.field(root.0, root.1, key)? else { return Ok(Some(key)) };
    let first = |v: &Value| -> Option<u32> {
        Some(match (v, db.kind(f.type_hash)?) {
            (Value::Int32(x), _) => u32::from_be_bytes(x.to_be_bytes()),
            (Value::UInt32(x) | Value::Key(x), _) => *x,
            (Value::Float(x), _) => x.to_bits(),
            (Value::Int64(x), _) => u32::from_be_bytes(x.to_be_bytes()[..4].try_into().ok()?),
            (Value::UInt64(x), _) => u32::from_be_bytes(x.to_be_bytes()[..4].try_into().ok()?),
            (Value::Vector2([x, ..]) | Value::Vector3([x, ..]) | Value::Vector4([x, ..]), _) => x.to_bits(),
            (Value::Ref(r), ValueKind::RefSpec) => r.class,
            (Value::Ref(r), ValueKind::ClassRefSpec(_)) => r.collection,
            _ => return None,
        })
    };
    match overrides.element(root.0, root.1, f, 0) {
        None => Ok(Some(0)),
        Some(v) => match first(v) {
            Some(i) => Ok(Some(i)),
            None => {
                // PORT(choice): an index read from a field narrower than four bytes, a text or a raw value is refused;
                // the console read the four bytes at the value, past a narrower one into its neighbours.
                problems.push(format!(
                    "part binding index {}: {}/{} holds {v:?}, not a four-byte value",
                    db.name(key),
                    db.name(root.0),
                    db.name(root.1)
                ));
                Ok(None)
            }
        },
    }
}

fn element_ref(db: &impl Store, overrides: &FieldOverrides, at: At, f: &Field, index: u32) -> Option<RefSpec> {
    if !db.kind(f.type_hash).is_some_and(ValueKind::is_reference) {
        return None;
    }
    match overrides.element(at.0, at.1, f, index)? {
        Value::Ref(r) => Some(*r),
        _ => None,
    }
}

fn valid(r: Option<RefSpec>) -> Option<RefSpec> {
    r.filter(|r| r.class != 0 && r.collection != 0)
}

fn walk(
    db: &impl Store,
    overrides: &FieldOverrides,
    root: At,
    steps: &[Step],
    problems: &mut Vec<String>,
) -> Result<Option<At>> {
    let mut at = root;
    for s in steps {
        let Some(f) = db.field(at.0, at.1, s.field)? else { return Ok(None) };
        let Some(i) = index(db, overrides, root, s.index, problems)? else { return Ok(None) };
        let Some(r) = valid(element_ref(db, overrides, at, f, i)) else { return Ok(None) };
        if !db.has(r.class, r.collection) {
            return Ok(None);
        }
        at = (r.class, r.collection);
    }
    Ok(Some(at))
}

fn stored(f: &Field, index: u32) -> bool {
    match &f.data {
        ssx_formats::attrib::FieldData::Single(_) => index == 0,
        ssx_formats::attrib::FieldData::Array(vs) => usize::try_from(index).is_ok_and(|i| i < vs.len()),
    }
}

fn apply(
    db: &impl Store,
    b: &PartBinding,
    root: At,
    overrides: &mut FieldOverrides,
    problems: &mut Vec<String>,
) -> Result<()> {
    let (Some((s_last, s_steps)), Some((t_last, t_steps))) = (b.source.split_last(), b.target.split_last()) else {
        return Ok(());
    };
    let Some(src) = walk(db, overrides, root, s_steps, problems)? else { return Ok(()) };
    let Some(tgt) = walk(db, overrides, root, t_steps, problems)? else { return Ok(()) };
    let (Some(sf), Some(tf)) = (db.field(src.0, src.1, s_last.field)?, db.field(tgt.0, tgt.1, t_last.field)?) else {
        return Ok(());
    };
    let Some(si) = index(db, overrides, root, s_last.index, problems)? else { return Ok(()) };
    let Some(ti) = index(db, overrides, root, t_last.index, problems)? else { return Ok(()) };
    let what =
        || format!("part binding {}: {}/{} {}[{ti}]", db.name(b.key), db.name(tgt.0), db.name(tgt.1), db.name(tf.key));
    let value = if let Some(r) = valid(element_ref(db, overrides, src, sf, si)) {
        let current = element_ref(db, overrides, tgt, tf, ti).map_or((0, 0), |c| (c.class, c.collection));
        if current == (r.class, r.collection) {
            return Ok(());
        }
        if db.kind(tf.type_hash) != Some(ValueKind::RefSpec) {
            // PORT(choice): a reference is written only into a `RefSpec` field; the console copied the 12-byte
            // reference over the target type's own bytes (0x8313D0B8), garbage in any other type.
            problems.push(format!("{}: not a RefSpec field, nothing written", what()));
            return Ok(());
        }
        Value::Ref(r)
    } else if db.kind(sf.type_hash) == Some(ValueKind::Vector4) {
        let vector = |at: At, f: &Field| match overrides.element(at.0, at.1, f, 0) {
            Some(Value::Vector4(v)) => *v,
            _ => [0.0; 4],
        };
        if db.kind(tf.type_hash) != Some(ValueKind::Vector4) {
            // PORT(choice): a Vector4 is written only into a Vector4 field; the console copied its 16 bytes over the
            // target type's own (0x83002130).
            problems.push(format!("{}: not a Vector4 field, nothing written", what()));
            return Ok(());
        }
        let (s, t) = (vector(src, sf), vector(tgt, tf));
        if s.iter().zip(t).all(|(a, b)| (a - b).abs() <= VECTOR_TOLERANCE) {
            return Ok(());
        }
        Value::Vector4(s)
    } else {
        return Ok(());
    };
    if stored(tf, ti) {
        overrides.set(tgt.0, tgt.1, tf.key, ti, value);
    }
    Ok(())
}

pub(crate) fn override_field(
    db: &impl Store,
    class: u32,
    key: u32,
    field: u32,
    value: u32,
    overrides: &mut FieldOverrides,
    problems: &mut Vec<String>,
) -> Result<()> {
    let Some(f) = db.field(class, key, field)? else { return Ok(()) };
    let set = match single(&f.data) {
        Some(Value::Int32(_)) => Value::Int32(i32::from_be_bytes(value.to_be_bytes())),
        Some(Value::UInt32(_)) => Value::UInt32(value),
        Some(Value::Key(_)) => Value::Key(value),
        _ => {
            problems.push(format!(
                "{}/{}: {} is no four-byte integer, the loadout's {value} not set",
                db.name(class),
                db.name(key),
                db.name(field)
            ));
            return Ok(());
        }
    };
    overrides.set(class, key, field, 0, set);
    for b in bindings_of(db, class, problems)? {
        apply(db, &b, (class, key), overrides, problems)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use ssx_formats::attrib::{FieldData, FieldSource};

    use super::*;

    const T_REF: u32 = 1;
    const T_KEY: u32 = 2;
    const T_INT: u32 = 3;
    const T_VEC4: u32 = 4;
    const T_CREF: u32 = 5;
    const T_BOOL: u32 = 6;

    const PACK: u32 = fnv1("ge_charactertexturepack");
    const MESH: u32 = fnv1("ge_mesh");
    const MATERIAL: u32 = fnv1("ge_material");
    const MATERIAL_TYPE: u32 = fnv1("ge_materialtype_player_board");
    const PARAMS: u32 = fnv1("ge_materialtype_default_player_board");
    const TEXTURE: u32 = fnv1("ge_texture");
    const F_PACK: u32 = 0xE8C1_A3DD;
    const F_ITEMS: u32 = fnv1("Items");
    const F_MESH: u32 = fnv1("Mesh");
    const F_MATERIAL: u32 = fnv1("Material");
    const F_TYPE: u32 = 0x6408_6147;
    const F_PARAMS: u32 = 0xC8E1_9255;
    const F_DIFFUSE: u32 = 0xF2EA_DA8A;
    const BOARD: u32 = 0xB0A2;
    const COMMON: u32 = fnv1("common_board_diffuse");

    #[derive(Default)]
    struct Fake {
        cols: Vec<((u32, u32), Vec<Field>)>,
    }

    impl Store for Fake {
        fn field(&self, class: u32, coll: u32, key: u32) -> Result<Option<&Field>> {
            let (_, fields) = self.cols.iter().find(|(at, _)| *at == (class, coll)).context("no collection")?;
            Ok(fields.iter().find(|f| f.key == key))
        }

        fn kind(&self, type_hash: u32) -> Option<ValueKind> {
            Some(match type_hash {
                T_REF => ValueKind::RefSpec,
                T_KEY => ValueKind::Key,
                T_INT => ValueKind::Int32,
                T_VEC4 => ValueKind::Vector4,
                T_CREF => ValueKind::ClassRefSpec(TEXTURE),
                T_BOOL => ValueKind::Bool,
                _ => return None,
            })
        }

        fn has(&self, class: u32, coll: u32) -> bool {
            self.cols.iter().any(|(at, _)| *at == (class, coll))
        }

        fn of_class(&self, class: u32) -> Result<Vec<(u32, Vec<&Field>)>> {
            Ok(self.cols.iter().filter(|((c, _), _)| *c == class).map(|((_, k), f)| (*k, f.iter().collect())).collect())
        }

        fn name(&self, key: u32) -> String {
            format!("{key:#010x}")
        }
    }

    impl Fake {
        fn add(&mut self, class: u32, key: u32, fields: Vec<Field>) {
            self.cols.push(((class, key), fields));
        }

        fn fields_mut(&mut self, class: u32, key: u32) -> &mut Vec<Field> {
            &mut self.cols.iter_mut().find(|(at, _)| *at == (class, key)).expect("collection").1
        }
    }

    fn one(key: u32, type_hash: u32, v: Value) -> Field {
        Field { key, type_hash, source: FieldSource::Layout, data: FieldData::Single(v) }
    }

    fn many(key: u32, type_hash: u32, vs: Vec<Value>) -> Field {
        Field { key, type_hash, source: FieldSource::OutOfLine, data: FieldData::Array(vs) }
    }

    fn r(class: u32, collection: u32) -> Value {
        Value::Ref(RefSpec { class, collection })
    }

    fn texture(i: u32) -> u32 {
        0x7E00 + i
    }

    fn key_list(keys: &[u32]) -> Vec<Value> {
        keys.iter().map(|&k| Value::Key(k)).collect()
    }

    fn binding(key: u32, class: u32, source: &[u32], source_indices: &[u32], target: &[u32]) -> Vec<Field> {
        vec![
            one(FIELD_CLASS_KEY, T_KEY, Value::Key(class)),
            many(FIELD_SOURCE_PATH, T_KEY, key_list(source)),
            many(FIELD_SOURCE_INDICES, T_KEY, key_list(source_indices)),
            many(FIELD_TARGET_PATH, T_KEY, key_list(target)),
            many(FIELD_TARGET_INDICES, T_KEY, key_list(&vec![0; target.len()])),
            one(fnv1("unused"), T_INT, Value::Int32(i32::try_from(key).unwrap_or(0))),
        ]
    }

    fn disc() -> Fake {
        let mut f = Fake::default();
        f.add(
            CLASS_SNOWBOARD,
            BOARD,
            vec![
                one(F_PACK, T_REF, r(PACK, 0x9A)),
                one(F_MESH, T_REF, r(MESH, 0x3E)),
                one(FIELD_SNOWBOARD_ID, T_INT, Value::Int32(0)),
            ],
        );
        f.add(PACK, 0x9A, vec![many(F_ITEMS, T_REF, (0..10).map(|i| r(TEXTURE, texture(i))).collect())]);
        f.add(MESH, 0x3E, vec![one(F_MATERIAL, T_REF, r(MATERIAL, 0xA1))]);
        f.add(MATERIAL, 0xA1, vec![one(F_TYPE, T_REF, r(MATERIAL_TYPE, 0xA2))]);
        f.add(MATERIAL_TYPE, 0xA2, vec![one(F_PARAMS, T_REF, r(PARAMS, 0xA3))]);
        f.add(PARAMS, 0xA3, vec![one(F_DIFFUSE, T_REF, r(TEXTURE, COMMON))]);
        f.add(
            CLASS_PART_BINDING,
            0xFD64_7957,
            binding(
                0xFD64_7957,
                CLASS_SNOWBOARD,
                &[F_PACK, F_ITEMS],
                &[0, FIELD_SNOWBOARD_ID],
                &[F_MESH, F_MATERIAL, F_TYPE, F_PARAMS, F_DIFFUSE],
            ),
        );
        f
    }

    fn equip(f: &Fake, id: u32) -> (Option<Value>, Vec<String>) {
        let (mut o, mut problems) = (FieldOverrides::new(), Vec::new());
        override_field(f, CLASS_SNOWBOARD, BOARD, FIELD_SNOWBOARD_ID, id, &mut o, &mut problems).expect("applies");
        (o.get(PARAMS, 0xA3, F_DIFFUSE, 0).cloned(), problems)
    }

    #[test]
    fn names_are_the_vaults_keys() {
        assert_eq!(CLASS_PART_BINDING, 0x40D3_8F7E);
        assert_eq!(FIELD_CLASS_KEY, 0x67D1_E558);
        assert_eq!(FIELD_SOURCE_PATH, 0x287D_769F);
        assert_eq!(FIELD_SOURCE_INDICES, 0xAD68_C61E);
        assert_eq!(FIELD_TARGET_PATH, 0x6B5C_7427);
        assert_eq!(FIELD_TARGET_INDICES, 0x9DB5_E166);
        assert_eq!(FIELD_SNOWBOARD_ID, 0x4876_EE26);
        assert_eq!(F_DIFFUSE, fnv1("diffuseTexture"));
    }

    #[test]
    fn the_design_is_the_packs_item_of_the_id() {
        let f = disc();
        for id in 0..10 {
            let (drawn, problems) = equip(&f, id);
            assert_eq!(drawn, Some(r(TEXTURE, texture(id))), "id {id}");
            assert!(problems.is_empty(), "{problems:?}");
        }
    }

    #[test]
    fn an_id_past_the_designs_keeps_the_material() {
        let f = disc();
        for id in [10, 0x7FFF_FFFF, u32::MAX] {
            assert_eq!(equip(&f, id), (None, Vec::new()), "id {id}");
        }
    }

    #[test]
    fn the_same_texture_is_not_written() {
        let mut f = disc();
        f.fields_mut(PARAMS, 0xA3)[0] = one(F_DIFFUSE, T_REF, r(TEXTURE, texture(4)));
        assert_eq!(equip(&f, 4).0, None);
        assert_eq!(equip(&f, 5).0, Some(r(TEXTURE, texture(5))));
    }

    #[test]
    fn a_broken_step_stops_the_binding() {
        for broken in
            [vec![], vec![one(F_MATERIAL, T_REF, r(MATERIAL, 0))], vec![one(F_MATERIAL, T_REF, r(MATERIAL, 0x77))]]
        {
            let mut f = disc();
            *f.fields_mut(MESH, 0x3E) = broken;
            assert_eq!(equip(&f, 3), (None, Vec::new()));
        }
    }

    #[test]
    fn only_the_bindings_of_the_changed_class_run() {
        let mut f = disc();
        f.fields_mut(CLASS_PART_BINDING, 0xFD64_7957)[0] = one(FIELD_CLASS_KEY, T_KEY, Value::Key(MESH));
        assert_eq!(equip(&f, 3).0, None);
    }

    #[test]
    fn a_class_ref_target_is_not_written() {
        let mut f = disc();
        f.fields_mut(PARAMS, 0xA3)[0] = one(F_DIFFUSE, T_CREF, r(TEXTURE, COMMON));
        let (drawn, problems) = equip(&f, 3);
        assert_eq!(drawn, None);
        assert_eq!(problems.len(), 1, "{problems:?}");
    }

    #[test]
    fn an_index_from_a_narrow_field_is_refused() {
        let mut f = disc();
        let narrow = fnv1("narrow");
        f.fields_mut(CLASS_SNOWBOARD, BOARD).push(one(narrow, T_BOOL, Value::Bool(true)));
        f.fields_mut(CLASS_PART_BINDING, 0xFD64_7957)[2] = many(FIELD_SOURCE_INDICES, T_KEY, key_list(&[0, narrow]));
        let (drawn, problems) = equip(&f, 3);
        assert_eq!(drawn, None);
        assert_eq!(problems.len(), 1, "{problems:?}");
        f.fields_mut(CLASS_PART_BINDING, 0xFD64_7957)[2] = many(FIELD_SOURCE_INDICES, T_KEY, key_list(&[0, 2]));
        assert_eq!(equip(&f, 3), (Some(r(TEXTURE, texture(2))), Vec::new()));
    }

    #[test]
    fn shorter_targets_apply_first() {
        let mut f = disc();
        f.cols.rotate_right(1);
        let other = fnv1("OtherPack");
        f.fields_mut(CLASS_SNOWBOARD, BOARD).push(one(other, T_REF, r(PACK, 0x9B)));
        f.add(PACK, 0x9B, vec![many(F_ITEMS, T_REF, (0..10).map(|i| r(TEXTURE, texture(100 + i))).collect())]);
        f.add(CLASS_PART_BINDING, 0x5A, binding(0x5A, CLASS_SNOWBOARD, &[other], &[0], &[F_PACK]));
        assert_eq!(equip(&f, 3).0, Some(r(TEXTURE, texture(103))));
    }

    #[test]
    fn a_vector_is_copied_beyond_the_tolerance() {
        let mut f = disc();
        let (tint, colour) = (fnv1("tint"), fnv1("colour"));
        f.fields_mut(CLASS_SNOWBOARD, BOARD).push(one(tint, T_VEC4, Value::Vector4([1.0, 0.5, 0.25, 1.0])));
        f.fields_mut(PARAMS, 0xA3).push(one(colour, T_VEC4, Value::Vector4([1.0, 0.5, 0.25 + 0.0003, 1.0])));
        f.add(
            CLASS_PART_BINDING,
            0x5B,
            binding(0x5B, CLASS_SNOWBOARD, &[tint], &[0], &[F_MESH, F_MATERIAL, F_TYPE, F_PARAMS, colour]),
        );
        let get = |f: &Fake| {
            let (mut o, mut p) = (FieldOverrides::new(), Vec::new());
            override_field(f, CLASS_SNOWBOARD, BOARD, FIELD_SNOWBOARD_ID, 1, &mut o, &mut p).expect("applies");
            o.get(PARAMS, 0xA3, colour, 0).cloned()
        };
        assert_eq!(get(&f), None);
        f.fields_mut(PARAMS, 0xA3)[1] = one(colour, T_VEC4, Value::Vector4([1.0, 0.5, 0.2, 1.0]));
        assert_eq!(get(&f), Some(Value::Vector4([1.0, 0.5, 0.25, 1.0])));
    }

    #[test]
    fn a_hostile_path_is_refused() {
        let mut f = disc();
        let long = vec![F_PACK; MAX_PATH_STEPS + 1];
        f.add(CLASS_PART_BINDING, 0x5C, binding(0x5C, CLASS_SNOWBOARD, &long, &[], &long));
        let (drawn, problems) = equip(&f, 3);
        assert_eq!(drawn, Some(r(TEXTURE, texture(3))));
        assert_eq!(problems.len(), 1, "{problems:?}");
    }
}
