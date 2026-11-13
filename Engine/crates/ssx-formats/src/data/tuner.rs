use crate::archive::big::BigArchive;
use crate::data::attrib::{Vault, collection};
use crate::data::save::chunks::TUNING_BYTES;
use crate::{Error, Result};

pub const TUNER_VERSION: u32 = 4;

pub const MAX_MEMBER_BYTES: u32 = 1 << 20;
const MAX_VERSION_BYTES: u32 = 16;

const VLT: &str = "updates/updates.vlt";
const BIN: &str = "updates/updates.bin";
const VERSION: &str = "updates/version.txt";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TunerPack {
    pub version: u32,
    pub archive_size: u32,
    pub vlt: Vec<u8>,
    pub bin: Vec<u8>,
}

impl TunerPack {
    pub fn parse(payload: &[u8]) -> Result<Self> {
        let data = if payload.starts_with(b"EB") {
            payload
        } else if payload.len() >= 4 && payload[4..].starts_with(b"EB") {
            &payload[4..]
        } else {
            return Err(Error::invalid("tuner: neither an EB v3 archive nor a chunk-7 payload with one at +4"));
        };
        let big = BigArchive::parse(data)?;
        if big.header.version != 3 {
            return Err(Error::invalid(format!("tuner: EB version {}, expected 3", big.header.version)));
        }
        let size = usize::try_from(big.header.archive_size)
            .map_err(|_| Error::invalid("tuner: archive size overflows this platform"))?;
        if size > data.len() {
            return Err(Error::invalid(format!(
                "tuner: the archive declares {size} bytes, only {} are there",
                data.len()
            )));
        }
        for e in &big.entries {
            if e.size > MAX_MEMBER_BYTES {
                return Err(Error::invalid(format!(
                    "tuner: member {:?} declares {} bytes, at most {MAX_MEMBER_BYTES} are read",
                    e.path(),
                    e.size
                )));
            }
            let stored = if e.flags & 0x01 != 0 { e.zsize } else { e.size } as usize;
            let end = usize::try_from(e.offset)
                .ok()
                .and_then(|o| o.checked_add(stored))
                .ok_or_else(|| Error::invalid(format!("tuner: member {:?} overflows", e.path())))?;
            if end > size {
                return Err(Error::invalid(format!(
                    "tuner: member {:?} ends at {end:#x}, past the archive's {size:#x}",
                    e.path()
                )));
            }
        }
        if big.entries.len() != 3 {
            return Err(Error::invalid(format!(
                "tuner: {} members, expected {VLT}, {BIN} and {VERSION}",
                big.entries.len()
            )));
        }
        let member = |path: &str| -> Result<Vec<u8>> {
            let e = big.find(path).ok_or_else(|| Error::invalid(format!("tuner: no member {path}")))?;
            Ok(big.member(data, e)?.into_owned())
        };
        let vlt = member(VLT)?;
        let bin = member(BIN)?;
        let version_bytes = member(VERSION)?;
        if version_bytes.len() as u32 > MAX_VERSION_BYTES {
            return Err(Error::invalid(format!(
                "tuner: updates/version.txt is {} bytes, at most {MAX_VERSION_BYTES}",
                version_bytes.len()
            )));
        }
        let text = std::str::from_utf8(&version_bytes)
            .map_err(|_| Error::invalid("tuner: updates/version.txt is not ASCII"))?
            .trim();
        let version: u32 = text
            .parse()
            .map_err(|_| Error::invalid(format!("tuner: updates/version.txt is {text:?}, not a version")))?;
        if version != TUNER_VERSION {
            return Err(Error::invalid(format!("tuner: version {version}, this port reads {TUNER_VERSION}")));
        }
        let vault = Vault::parse(&vlt, &bin)?;
        let collections = collection::collections(&vault)?;
        if collections.is_empty() {
            return Err(Error::invalid("tuner: the delta vault has no collections"));
        }
        Ok(TunerPack {
            version,
            archive_size: u32::try_from(big.header.archive_size)
                .map_err(|_| Error::invalid("tuner: archive size does not fit a u32"))?,
            vlt,
            bin,
        })
    }

    pub fn collections(&self) -> Result<usize> {
        let vault = Vault::parse(&self.vlt, &self.bin)?;
        Ok(collection::collections(&vault)?.len())
    }

    pub const fn buffer_size() -> usize {
        TUNING_BYTES
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::attrib::tests_synthetic::minimal_pair;

    fn pack(version: &[u8], vlt: &[u8], bin: &[u8]) -> Vec<u8> {
        const N: usize = 3;
        const PATH_ENTRY: usize = 0x10;
        const DIR_ENTRY: usize = 0x10;
        const BASE: usize = 0x70;
        let mut packed = vec![0x10, 0xFB, 0, 0, version.len() as u8, 0xFC | (version.len() as u8)];
        packed.extend_from_slice(version);
        type Member<'a> = (&'a str, &'a [u8], Option<Vec<u8>>);
        let mut members: Vec<Member<'_>> =
            vec![("updates.vlt", vlt, None), ("updates.bin", bin, None), ("version.txt", version, Some(packed))];
        members.sort_by_key(|(name, _, _)| crate::archive::big::path_hash(&format!("updates/{name}")));
        let mut data = vec![0u8; BASE + N * PATH_ENTRY + DIR_ENTRY];
        let mut at = BASE + N * PATH_ENTRY + DIR_ENTRY;
        let mut offsets = Vec::new();
        for (_, raw, packed) in &members {
            at = at.next_multiple_of(16);
            let bytes: &[u8] = packed.as_deref().unwrap_or(raw);
            offsets.push((at, bytes.len()));
            if data.len() < at + bytes.len() {
                data.resize(at + bytes.len(), 0);
            }
            data[at..at + bytes.len()].copy_from_slice(bytes);
            at += bytes.len();
        }
        data[0..2].copy_from_slice(b"EB");
        data[2..4].copy_from_slice(&3u16.to_be_bytes());
        data[4..8].copy_from_slice(&(N as u32).to_be_bytes());
        data[8..10].copy_from_slice(&0x10u16.to_be_bytes());
        data[0x0A] = 4;
        data[0x0C..0x10].copy_from_slice(&(BASE as u32).to_be_bytes());
        data[0x10..0x14].copy_from_slice(&((N * PATH_ENTRY + DIR_ENTRY) as u32).to_be_bytes());
        data[0x14] = PATH_ENTRY as u8;
        data[0x15] = DIR_ENTRY as u8;
        data[0x16..0x18].copy_from_slice(&1u16.to_be_bytes());
        let archive_size = data.len() as u64;
        data[0x18..0x20].copy_from_slice(&archive_size.to_be_bytes());
        for (i, ((name, raw, packed), (offset, len))) in members.iter().zip(&offsets).enumerate() {
            let at = 0x30 + i * 16;
            data[at..at + 4].copy_from_slice(&((*offset as u32) >> 4).to_be_bytes());
            let zsize = if packed.is_some() { *len as u32 } else { 0 };
            data[at + 4..at + 8].copy_from_slice(&zsize.to_be_bytes());
            let size = packed.as_ref().map_or(*len as u32, |_| raw.len() as u32);
            data[at + 8..at + 12].copy_from_slice(&size.to_be_bytes());
            data[at + 12..at + 16]
                .copy_from_slice(&crate::archive::big::path_hash(&format!("updates/{name}")).to_be_bytes());
            data[0x30 + N * 16 + i] = u8::from(packed.is_some());
            let p = BASE + i * PATH_ENTRY;
            data[p + 2..p + 2 + name.len()].copy_from_slice(name.as_bytes());
        }
        data[BASE + N * PATH_ENTRY..BASE + N * PATH_ENTRY + 7].copy_from_slice(b"updates");
        data
    }

    #[test]
    fn parses_a_synthetic_pack_and_reads_its_vault() {
        let (vlt, bin) = minimal_pair();
        let bytes = pack(b"4", &vlt, &bin);
        let p = TunerPack::parse(&bytes).unwrap();
        assert_eq!((p.version, p.archive_size), (4, bytes.len() as u32));
        assert_eq!((&p.vlt, &p.bin), (&vlt, &bin));
        assert_eq!(p.collections().unwrap(), 1);
        let mut chunk = 4u32.to_be_bytes().to_vec();
        chunk.extend_from_slice(&bytes);
        chunk.resize(TUNING_BYTES, 0);
        assert_eq!(TunerPack::parse(&chunk).unwrap(), p);
        assert_eq!(TunerPack::buffer_size(), TUNING_BYTES);
        let mut longer = bytes.clone();
        longer.push(0xFF);
        assert_eq!(TunerPack::parse(&longer).unwrap(), p);
    }

    #[test]
    fn refuses_other_versions_members_and_sizes() {
        let (vlt, bin) = minimal_pair();
        assert!(TunerPack::parse(&pack(b"3", &vlt, &bin)).is_err());
        assert!(TunerPack::parse(&pack(b"x", &vlt, &bin)).is_err());
        let mut bytes = pack(b"4", &vlt, &bin);
        let longer = bytes.len() as u64 + 16;
        bytes[0x18..0x20].copy_from_slice(&longer.to_be_bytes());
        assert!(TunerPack::parse(&bytes).is_err());
        let mut bytes = pack(b"4", &vlt, &bin);
        let size = u64::from_be_bytes(bytes[0x18..0x20].try_into().unwrap()) - 8;
        bytes[0x18..0x20].copy_from_slice(&size.to_be_bytes());
        assert!(TunerPack::parse(&bytes).is_err());
        assert!(TunerPack::parse(&vec![0u8; TUNING_BYTES]).is_err());
        assert!(TunerPack::parse(&[]).is_err());
    }
}
