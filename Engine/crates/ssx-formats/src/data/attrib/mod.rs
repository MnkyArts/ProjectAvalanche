pub mod collection;
pub mod container;
pub mod names;
pub mod schema;
pub mod value;

pub use collection::{Collection, DecodedCollection, Entry, Field, FieldData, FieldSource};
pub use container::{Export, PtrTarget, Vault};
pub use names::NameDict;
pub use schema::{Class, Definition, Schema, TypeInfo};
pub use value::{RawPointer, RefSpec, Value};

pub const fn fnv1(bytes: &[u8]) -> u32 {
    let mut h: u32 = 0x811C_9DC5;
    let mut i = 0;
    while i < bytes.len() {
        h = h.wrapping_mul(0x0100_0193) ^ bytes[i] as u32;
        i += 1;
    }
    h
}

pub const EXPORT_DATABASE: u32 = fnv1(b"Attrib::DatabaseLoadData");
pub const EXPORT_CLASS: u32 = fnv1(b"Attrib::ClassLoadData");
pub const EXPORT_COLLECTION: u32 = fnv1(b"Attrib::CollectionLoadData");

#[cfg(any(test, feature = "test-fixtures"))]
#[doc(hidden)]
pub mod fixtures;
#[cfg(test)]
pub(crate) mod tests_synthetic;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fnv1_known_values() {
        assert_eq!(fnv1(b""), 0x811C_9DC5);
        assert_eq!(fnv1(b"terrain_rock_alps_mat"), 0xBB14_C3B4);
        assert_eq!(fnv1(b"alps0101_track"), 0x1825_3B85);
        assert_eq!(fnv1(b"Material"), 0x6D04_D9A2);
        assert_eq!(fnv1(b"test_scene_track.vlt"), 0xC6F2_0814);
        assert_eq!(EXPORT_COLLECTION, 0x2234_EDF1);
    }
}
