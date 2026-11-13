use super::collection::{self, ENTRY_ARRAY, ENTRY_INLINE, ENTRY_REFERENCE};
use super::container::{FILE_BIN, FILE_VLT, PtrTarget};
pub(crate) use super::fixtures::minimal_pair;
use super::fixtures::{Builder, CLASS, be, schema_pair, th};
use super::*;

#[test]
fn container_and_pointer_fixups() {
    let mut bld = Builder::new(&["", "hello"]);
    let hello = bld.string_at("hello");
    let obj = bld.export(0x1234, 0x5678, &[0; 8]);
    bld.ptr(FILE_VLT, obj + 4, hello);
    let (vlt, bin) = bld.build();
    let v = Vault::parse(&vlt, &bin).unwrap();
    assert_eq!(v.deps[1].name, "t.bin");
    assert_eq!(v.exports, vec![Export { key: 0x1234, type_hash: 0x5678, size: 8, offset: obj }]);
    assert_eq!(v.bin_strings.iter().map(|s| s.1.as_str()).collect::<Vec<_>>(), ["", "hello"]);
    let t = v.pointer(FILE_VLT, obj + 4).unwrap();
    assert_eq!(t, PtrTarget { file: FILE_BIN, offset: hello });
    assert_eq!(v.cstr(t).unwrap(), "hello");
    assert_eq!(v.pointer(FILE_VLT, obj), None);

    let mut bad = vlt.clone();
    bad[obj as usize + 7] = 1;
    assert!(Vault::parse(&bad, &bin).is_err());
}

#[test]
fn schema_collection_decode_and_inheritance() {
    let (svlt, sbin) = schema_pair();
    let sv = Vault::parse(&svlt, &sbin).unwrap();
    let schema = Schema::from_vault(&sv).unwrap();
    assert_eq!(schema.types.len(), 5);
    assert_eq!(schema.class(CLASS).unwrap().layout_size, 16);

    let (x, child) = (fnv1(b"x"), fnv1(b"child"));
    let mut b = Builder::new(&["xname"]);
    let name_at = b.string_at("xname");
    let layout = b.bin(&be(&[0, CLASS, child, 0]));
    b.ptr(FILE_BIN, layout, name_at);
    let child_layout = b.bin(&be(&[0, CLASS, 0, 0]));
    let dir = b.bin(&be(&[0, 1.0f32.to_bits(), 0]));
    let refs = b.bin(&[be(&[0x0001_0001, 0x000c_0000]), be(&[CLASS, x, 0])].concat());
    let mut col = be(&[x, CLASS, 0, 4, 0, 4, (4 << 16) | 4, 0, th(0), th(2), th(3), th(4)]);
    col.extend(be(&[fnv1(b"speed"), 1.5f32.to_bits(), (ENTRY_INLINE as u32) << 8]));
    col.extend(be(&[fnv1(b"dir"), 0, 1 << 16]));
    col.extend(be(&[fnv1(b"refs"), 0, (2 << 16) | ((ENTRY_ARRAY | ENTRY_REFERENCE) as u32) << 8]));
    col.extend(be(&[fnv1(b"on"), 0x0100_0000, (3 << 16) | (ENTRY_INLINE as u32) << 8]));
    let at = b.export(fnv1(b"c/x"), EXPORT_COLLECTION, &col);
    b.ptr(FILE_VLT, at + 0x1c, layout);
    b.ptr(FILE_VLT, at + 0x20 + 16 + 12 + 4, dir);
    b.ptr(FILE_VLT, at + 0x20 + 16 + 24 + 4, refs);
    let mut ccol = be(&[child, CLASS, x, 1, 0, 1, (1 << 16) | 2, 0, th(0), 0]);
    ccol.extend(be(&[fnv1(b"speed"), 2.0f32.to_bits(), (ENTRY_INLINE as u32) << 8]));
    let cat = b.export(fnv1(b"c/child"), EXPORT_COLLECTION, &ccol);
    b.ptr(FILE_VLT, cat + 0x1c, child_layout);
    let (vlt, bin) = b.build();

    let v = Vault::parse(&vlt, &bin).unwrap();
    let cols = collection::collections(&v).unwrap();
    let dec: Vec<_> = cols.iter().map(|c| c.decode(&v, &schema).unwrap()).collect();
    let get = |d: &DecodedCollection, k: &[u8]| d.field(fnv1(k)).unwrap().data.clone();
    assert_eq!(get(&dec[0], b"name"), FieldData::Single(Value::Text(Some("xname".into()))));
    assert_eq!(get(&dec[0], b"link"), FieldData::Single(Value::Ref(RefSpec { class: CLASS, collection: child })));
    assert_eq!(get(&dec[0], b"speed"), FieldData::Single(Value::Float(1.5)));
    assert_eq!(get(&dec[0], b"dir"), FieldData::Single(Value::Vector3([0.0, 1.0, 0.0])));
    assert_eq!(get(&dec[0], b"refs"), FieldData::Array(vec![Value::Ref(RefSpec { class: CLASS, collection: x })]));
    assert_eq!(get(&dec[0], b"on"), FieldData::Single(Value::Bool(true)));
    assert_eq!(get(&dec[1], b"name"), FieldData::Single(Value::Text(None)));
    assert_eq!(dec.iter().map(|d| d.pointer_slots.len()).sum::<usize>(), v.fixup_count());

    let index = collection::index(&cols);
    let all = collection::with_inherited(&cols, &dec, &index, 1).unwrap();
    let speed = all.iter().find(|(_, f)| f.key == fnv1(b"speed")).unwrap();
    assert_eq!((speed.0, &speed.1.data), (child, &FieldData::Single(Value::Float(2.0))));
    assert!(all.iter().any(|(from, f)| *from == x && f.key == fnv1(b"dir")));
    assert!(!all.iter().any(|(from, f)| *from == x && f.key == fnv1(b"name")), "layout is not inherited");

    let mut bad = vlt.clone();
    let flags = (at + 0x20 + 16 + 10) as usize;
    bad[flags] = 0;
    let v = Vault::parse(&bad, &bin).unwrap();
    assert!(collection::collections(&v).unwrap()[0].decode(&v, &schema).is_err());
}
