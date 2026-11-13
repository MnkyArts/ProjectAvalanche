use super::collection::ENTRY_INLINE;
use super::container::{FILE_BIN, FILE_VLT};
use super::*;

const DATN_PAYLOAD: usize = 88;

pub struct Builder {
    strings: Vec<u8>,
    datn: Vec<u8>,
    bin_data: Vec<u8>,
    exports: Vec<[u32; 4]>,
    fixups: Vec<(u16, u32, u32)>,
}

impl Builder {
    pub fn new(strings: &[&str]) -> Self {
        let mut s: Vec<u8> = strings.iter().flat_map(|x| x.bytes().chain([0])).collect();
        s.resize(s.len().next_multiple_of(16), 0);
        Builder { strings: s, datn: Vec::new(), bin_data: Vec::new(), exports: Vec::new(), fixups: Vec::new() }
    }
    pub fn bin_base(&self) -> u32 {
        (8 + self.strings.len() + 16) as u32
    }
    pub fn string_at(&self, s: &str) -> u32 {
        let pos = self.strings.windows(s.len() + 1).position(|w| &w[..s.len()] == s.as_bytes() && w[s.len()] == 0);
        8 + pos.unwrap() as u32
    }
    pub fn export(&mut self, key: u32, ty: u32, bytes: &[u8]) -> u32 {
        self.datn.resize(self.datn.len().next_multiple_of(8), 0);
        let off = (DATN_PAYLOAD + self.datn.len()) as u32;
        self.datn.extend_from_slice(bytes);
        self.exports.push([key, ty, bytes.len() as u32, off]);
        off
    }
    pub fn bin(&mut self, bytes: &[u8]) -> u32 {
        self.bin_data.resize(self.bin_data.len().next_multiple_of(16), 0);
        let off = self.bin_base() + self.bin_data.len() as u32;
        self.bin_data.extend_from_slice(bytes);
        off
    }
    pub fn ptr(&mut self, file: u16, slot: u32, target: u32) {
        self.fixups.push((file, slot, target));
    }
    pub fn build(&self) -> (Vec<u8>, Vec<u8>) {
        let mut v = Vec::new();
        let chunk = |v: &mut Vec<u8>, tag: &[u8], payload: &[u8]| {
            v.extend_from_slice(tag);
            v.extend_from_slice(&(payload.len() as u32 + 8).to_be_bytes());
            v.extend_from_slice(payload);
        };
        chunk(&mut v, b"Vers", &[0x2c, 0x82, 0x1e, 0x4b, 0x12, 0x0c, 0x4f, 0xd6]);
        chunk(&mut v, b"StrN", &[0; 8]);
        let mut dep = be(&[2, fnv1(b"t.vlt"), fnv1(b"t.bin"), 0, 6]);
        dep.extend_from_slice(b"t.vlt\0t.bin\0");
        dep.resize(40, 0);
        chunk(&mut v, b"DepN", &dep);
        assert_eq!(v.len() + 8, DATN_PAYLOAD);
        let mut datn = self.datn.clone();
        datn.resize(datn.len().next_multiple_of(16), 0);
        chunk(&mut v, b"DatN", &datn);
        let mut exp = be(&[self.exports.len() as u32]);
        self.exports.iter().for_each(|e| exp.extend(be(e)));
        exp.resize((exp.len() + 8).next_multiple_of(16) - 8, 0);
        chunk(&mut v, b"ExpN", &exp);
        let mut ptr = Vec::new();
        for file in [FILE_BIN, FILE_VLT] {
            ptr.extend(be(&[0, (2 << 16) | file as u32, 0]));
            for &(_, slot, target) in self.fixups.iter().filter(|x| x.0 == file) {
                ptr.extend(be(&[slot, (3 << 16) | FILE_BIN as u32, target]));
            }
        }
        ptr.extend([0; 12]);
        chunk(&mut v, b"PtrN", &ptr);
        chunk(&mut v, b"EndC", &[0; 8]);
        let mut b = Vec::new();
        chunk(&mut b, b"StrE", &self.strings);
        chunk(&mut b, b"EndC", &[0; 8]);
        b.extend_from_slice(&self.bin_data);
        (v, b)
    }
}

pub fn be(words: &[u32]) -> Vec<u8> {
    words.iter().flat_map(|w| w.to_be_bytes()).collect()
}

pub fn minimal_pair() -> (Vec<u8>, Vec<u8>) {
    let mut b = Builder::new(&[]);
    let key = fnv1(b"c/minimal");
    let mut col = be(&[key, fnv1(b"c"), 0, 0, 0, 0, 0, 0]);
    col.resize(0x20, 0);
    b.export(key, EXPORT_COLLECTION, &col);
    b.build()
}

pub const CLASS: u32 = fnv1(b"c");
pub const TYPES: [(&str, u32); 5] = [
    ("EA::Reflection::Float", 4),
    ("EA::Reflection::Text", 4),
    ("Attrib::Types::Vector3", 12),
    ("Attrib::RefSpec", 12),
    ("EA::Reflection::Bool", 1),
];

pub fn th(i: usize) -> u32 {
    fnv1(TYPES[i].0.as_bytes())
}

pub fn schema_pair() -> (Vec<u8>, Vec<u8>) {
    let mut b = Builder::new(&[]);
    let names: Vec<u8> = TYPES.iter().flat_map(|t| t.0.bytes().chain([0])).collect();
    let names_at = b.bin(&names);
    let mut db = be(&[207, 216, TYPES.len() as u32, 0]);
    db.extend(be(&TYPES.map(|t| t.1)));
    let db_at = b.export(EXPORT_DATABASE ^ 1, EXPORT_DATABASE, &db);
    b.ptr(FILE_VLT, db_at + 12, names_at);
    let def = |key: &[u8], ty: usize, off: u32, size: u32, max: u32, flags: u32, align: u32| {
        be(&[fnv1(key), th(ty), (off << 16) | size, (max << 16) | (flags << 8) | align])
    };
    let defs = [
        def(b"name", 1, 0, 4, 1, 0x06, 2),
        def(b"link", 3, 4, 12, 1, 0x26, 2),
        def(b"speed", 0, 0, 4, 1, 0x04, 2),
        def(b"dir", 2, 0, 12, 1, 0x04, 2),
        def(b"refs", 3, 0, 12, 0xffff, 0x25, 2),
        def(b"on", 4, 0, 1, 1, 0x04, 0),
    ]
    .concat();
    let defs_at = b.bin(&defs);
    let class_at = b.export(CLASS, EXPORT_CLASS, &be(&[CLASS, 2, 6, 0, 0, 0, 16, 2]));
    b.ptr(FILE_VLT, class_at + 12, defs_at);
    b.build()
}

pub fn one_collection_pair(name: &str) -> (Vec<u8>, Vec<u8>) {
    let key = fnv1(name.as_bytes());
    let mut b = Builder::new(&[]);
    let layout = b.bin(&be(&[0, CLASS, 0, 0]));
    let mut col = be(&[key, CLASS, 0, 1, 0, 1, (1 << 16) | 2, 0, th(0), 0]);
    col.extend(be(&[fnv1(b"speed"), 2.0f32.to_bits(), (ENTRY_INLINE as u32) << 8]));
    let at = b.export(fnv1(format!("c/{name}").as_bytes()), EXPORT_COLLECTION, &col);
    b.ptr(FILE_VLT, at + 0x1c, layout);
    b.build()
}
