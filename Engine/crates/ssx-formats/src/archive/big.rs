use std::borrow::Cow;

use crate::archive::refpack;
use crate::reader::Reader;
use crate::{Error, Result};

const HEADER_SIZE: usize = 0x30;

#[derive(Clone, Debug)]
pub struct BigHeader {
    pub version: u16,
    pub file_count: u32,
    pub flags: u16,
    pub alignment_shift: u8,
    pub base_header_size: u32,
    pub name_header_size: u32,
    pub path_entry_size: u8,
    pub dir_entry_size: u8,
    pub dir_count: u16,
    pub archive_size: u64,
}

#[derive(Clone, Debug)]
pub struct BigEntry {
    pub offset: u64,
    pub size: u32,
    pub zsize: u32,
    pub hash: u32,
    pub flags: u8,
    pub dir: String,
    pub name: String,
}

impl BigEntry {
    pub fn path(&self) -> String {
        if self.dir.is_empty() || self.dir == "." { self.name.clone() } else { format!("{}/{}", self.dir, self.name) }
    }

    pub fn extension(&self) -> &str {
        self.name.rsplit_once('.').map(|(_, ext)| ext).unwrap_or("")
    }
}

#[derive(Clone, Debug)]
pub struct BigArchive {
    pub header: BigHeader,
    pub entries: Vec<BigEntry>,
}

impl BigArchive {
    pub fn parse(data: &[u8]) -> Result<Self> {
        let mut r = Reader::be(data);
        r.expect(b"EB")?;
        let header = BigHeader {
            version: r.u16()?,
            file_count: r.u32()?,
            flags: r.u16()?,
            alignment_shift: r.u8()?,
            base_header_size: {
                let _reserved = r.u8()?;
                r.u32()?
            },
            name_header_size: r.u32()?,
            path_entry_size: r.u8()?,
            dir_entry_size: r.u8()?,
            dir_count: r.u16()?,
            archive_size: r.u64()?,
        };
        if header.version != 3 {
            return Err(Error::invalid(format!("unsupported EB big version {}", header.version)));
        }
        if header.alignment_shift > 31 {
            return Err(Error::invalid(format!(
                "EB big alignment shift {} does not fit a 32-bit offset",
                header.alignment_shift
            )));
        }
        let n = header.file_count as usize;
        if n > data.len().saturating_sub(HEADER_SIZE) / 17 {
            return Err(Error::invalid(format!("EB big claims {n} members, which do not fit in {} bytes", data.len())));
        }
        if header.path_entry_size < 3 {
            return Err(Error::invalid("EB big path entries too small"));
        }
        if !header.base_header_size.is_multiple_of(16) {
            return Err(Error::invalid(format!(
                "EB big base_header_size {:#x} is not 16-aligned",
                header.base_header_size
            )));
        }
        let dir_bytes = usize::from(header.dir_count) * usize::from(header.dir_entry_size);
        if dir_bytes as u32 > header.name_header_size {
            return Err(Error::invalid(format!(
                "EB big directory table ({dir_bytes} bytes) exceeds name_header_size {}",
                header.name_header_size
            )));
        }
        let index_end = n
            .checked_mul(17)
            .and_then(|size| size.checked_add(HEADER_SIZE))
            .ok_or_else(|| Error::invalid("EB big index size overflows"))?;
        if (header.base_header_size as usize) < index_end {
            return Err(Error::invalid(format!(
                "EB big base_header_size {:#x} overlaps its index ending at {index_end:#x}",
                header.base_header_size
            )));
        }
        let paths_end = n
            .checked_mul(usize::from(header.path_entry_size))
            .and_then(|size| size.checked_add(15))
            .map(|size| size & !15)
            .ok_or_else(|| Error::invalid("EB big path table size overflows"))?;
        if paths_end.saturating_add(dir_bytes) > header.name_header_size as usize {
            return Err(Error::invalid(format!(
                "EB big name_header_size {} does not hold {n} path entries and {dir_bytes} directory bytes",
                header.name_header_size
            )));
        }
        r.seek(HEADER_SIZE)?;
        let mut index = Vec::with_capacity(n);
        for _ in 0..n {
            index.push((r.u32()?, r.u32()?, r.u32()?, r.u32()?));
        }
        let flags = r.bytes(n)?;

        r.seek(header.base_header_size as usize)?;
        let mut paths = Vec::with_capacity(n);
        for _ in 0..n {
            let dir = r.u16()? as usize;
            let name = r.fixed_str(header.path_entry_size as usize - 2)?;
            paths.push((dir, name));
        }
        r.align(16)?;
        let mut dirs = Vec::with_capacity(header.dir_count as usize);
        for _ in 0..header.dir_count {
            dirs.push(r.fixed_str(header.dir_entry_size as usize)?);
        }

        let entries = index
            .into_iter()
            .zip(paths)
            .zip(flags)
            .map(|(((offset, zsize, size, hash), (dir, name)), &flag)| {
                let dir = dirs
                    .get(dir)
                    .cloned()
                    .ok_or_else(|| Error::invalid(format!("directory index {dir} out of range for {name}")))?;
                Ok(BigEntry {
                    offset: (offset as u64) << header.alignment_shift,
                    size,
                    zsize,
                    hash,
                    flags: flag,
                    dir,
                    name,
                })
            })
            .collect::<Result<Vec<_>>>()?;

        Ok(Self { header, entries })
    }

    pub fn table_size(header_bytes: &[u8; HEADER_SIZE]) -> Result<usize> {
        let mut r = Reader::be(header_bytes);
        r.expect(b"EB")?;
        r.seek(0x0c)?;
        let base = r.u32()? as usize;
        let names = r.u32()? as usize;
        Ok(base + names)
    }

    pub fn find(&self, path: &str) -> Option<&BigEntry> {
        let hash = path_hash(path);
        let start = self.entries.partition_point(|e| e.hash < hash);
        self.entries[start..].iter().take_while(|e| e.hash == hash).find(|e| e.path() == path)
    }

    pub fn member<'a>(&self, data: &'a [u8], entry: &BigEntry) -> Result<Cow<'a, [u8]>> {
        let compressed = entry.flags & 0x01 != 0;
        if compressed != (entry.zsize != 0) {
            return Err(Error::invalid(format!(
                "EB big member {:?}: flag {:#04x} does not match zsize {}",
                entry.path(),
                entry.flags,
                entry.zsize
            )));
        }
        let stored = if compressed { entry.zsize } else { entry.size } as usize;
        let start = usize::try_from(entry.offset)
            .map_err(|_| Error::invalid(format!("EB big member {:?} offset overflows", entry.path())))?;
        let end = start
            .checked_add(stored)
            .ok_or_else(|| Error::invalid(format!("EB big member {:?} stored size overflows", entry.path())))?;
        let bytes = data.get(start..end).ok_or(Error::UnexpectedEof {
            offset: start,
            wanted: stored,
            available: data.len().saturating_sub(start),
        })?;
        Self::decode_stored(entry, bytes)
    }

    pub fn stored_range(entry: &BigEntry) -> Result<(u64, usize)> {
        let compressed = entry.flags & 0x01 != 0;
        if compressed != (entry.zsize != 0) {
            return Err(Error::invalid(format!(
                "EB big member {:?}: flag {:#04x} does not match zsize {}",
                entry.path(),
                entry.flags,
                entry.zsize
            )));
        }
        Ok((entry.offset, if compressed { entry.zsize } else { entry.size } as usize))
    }

    pub fn decode_stored<'a>(entry: &BigEntry, stored: &'a [u8]) -> Result<Cow<'a, [u8]>> {
        let (_, len) = Self::stored_range(entry)?;
        if stored.len() != len {
            return Err(Error::invalid(format!(
                "EB big member {:?}: {} stored bytes, the entry has {len}",
                entry.path(),
                stored.len()
            )));
        }
        if entry.flags & 0x01 == 0 {
            return Ok(Cow::Borrowed(stored));
        }
        Ok(Cow::Owned(refpack::decompress(stored, entry.size as usize)?))
    }
}

pub fn path_hash(path: &str) -> u32 {
    path.bytes().fold(5381u32, |h, b| h.wrapping_mul(33).wrapping_add(b as u32))
}

pub const HEADER_LEN: usize = HEADER_SIZE;

#[cfg(test)]
mod tests {
    use super::*;

    const PATHS: usize = 2;
    const PATH_ENTRY: usize = 0x10;
    const DIR_ENTRY: usize = 0x10;
    const BASE: usize = 0x60;
    const RAW_AT: usize = 0x90;
    const PACKED_AT: usize = 0xA0;

    fn archive() -> Vec<u8> {
        let packed = [0x10u8, 0xFB, 0, 0, 5, 0xE0, b'h', b'e', b'l', b'l', 0xFD, b'o'];
        let mut data = vec![0u8; PACKED_AT + packed.len()];
        data[0..2].copy_from_slice(b"EB");
        data[2..4].copy_from_slice(&3u16.to_be_bytes());
        data[4..8].copy_from_slice(&(PATHS as u32).to_be_bytes());
        data[8..10].copy_from_slice(&0x10u16.to_be_bytes());
        data[0x0A] = 4;
        data[0x0C..0x10].copy_from_slice(&(BASE as u32).to_be_bytes());
        data[0x10..0x14].copy_from_slice(&((PATHS * PATH_ENTRY + DIR_ENTRY) as u32).to_be_bytes());
        data[0x14] = PATH_ENTRY as u8;
        data[0x15] = DIR_ENTRY as u8;
        data[0x16..0x18].copy_from_slice(&1u16.to_be_bytes());
        let archive_size = data.len() as u64;
        data[0x18..0x20].copy_from_slice(&archive_size.to_be_bytes());
        for (index, (offset, zsize, size, name, flag)) in
            [(RAW_AT as u32, 0u32, 4u32, "a.bin", 0u8), (PACKED_AT as u32, packed.len() as u32, 5, "b.bin", 1)]
                .into_iter()
                .enumerate()
        {
            let at = HEADER_SIZE + index * 16;
            data[at..at + 4].copy_from_slice(&(offset >> 4).to_be_bytes());
            data[at + 4..at + 8].copy_from_slice(&zsize.to_be_bytes());
            data[at + 8..at + 12].copy_from_slice(&size.to_be_bytes());
            data[at + 12..at + 16].copy_from_slice(&path_hash(name).to_be_bytes());
            data[HEADER_SIZE + PATHS * 16 + index] = flag;
        }
        for (index, name) in ["a.bin", "b.bin"].iter().enumerate() {
            let at = BASE + index * PATH_ENTRY;
            data[at + 2..at + 2 + name.len()].copy_from_slice(name.as_bytes());
        }
        data[BASE + PATHS * PATH_ENTRY] = b'.';
        data[RAW_AT..RAW_AT + 4].copy_from_slice(b"RAW!");
        data[PACKED_AT..PACKED_AT + packed.len()].copy_from_slice(&packed);
        data
    }

    #[test]
    fn reads_raw_and_refpack_members() {
        let data = archive();
        let big = BigArchive::parse(&data).unwrap();
        assert_eq!(big.header.file_count, 2);
        assert_eq!(big.header.archive_size, data.len() as u64);
        let raw = big.member(&data, &big.entries[0]).unwrap();
        assert!(matches!(raw, Cow::Borrowed(_)));
        assert_eq!(raw.as_ref(), b"RAW!");
        let packed = big.member(&data, &big.entries[1]).unwrap();
        assert!(matches!(packed, Cow::Owned(_)));
        assert_eq!(packed.as_ref(), b"hello");
    }

    #[test]
    fn rejects_a_flag_that_disagrees_with_zsize() {
        let data = archive();
        let big = BigArchive::parse(&data).unwrap();
        let mut entry = big.entries[1].clone();
        entry.flags = 0;
        assert!(big.member(&data, &entry).is_err());
        let mut entry = big.entries[0].clone();
        entry.flags = 1;
        entry.zsize = 4;
        assert!(big.member(&data, &entry).is_err());
    }

    #[test]
    fn rejects_hostile_tables() {
        let mut data = archive();
        data[4..8].copy_from_slice(&u32::MAX.to_be_bytes());
        assert!(BigArchive::parse(&data).is_err());
        let mut data = archive();
        data[0x0C..0x10].copy_from_slice(&0x50u32.to_be_bytes());
        assert!(BigArchive::parse(&data).is_err());
    }
}
