use std::collections::{HashMap, HashSet};

use sha1::{Digest, Sha1};

use crate::reader::Reader;
use crate::{Error, Result};

pub const BLOCK_SIZE: usize = 0x1000;
pub const METADATA_OFFSET: usize = 0x344;
pub const METADATA_SIZE: usize = 0x93D6;
pub const METADATA_END: usize = METADATA_OFFSET + METADATA_SIZE;
const L0_BLOCKS: u64 = 170;
const L1_BLOCKS: u64 = 28_900;
const L2_BLOCKS: u64 = 4_913_000;
const ENTRIES_PER_BLOCK: usize = 0x40;
const ENTRY_SIZE: usize = 0x40;
pub const END_OF_CHAIN: u32 = 0x00FF_FFFF;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Magic {
    Live,
    Con,
    Pirs,
}

impl Magic {
    pub fn as_str(self) -> &'static str {
        match self {
            Magic::Live => "LIVE",
            Magic::Con => "CON ",
            Magic::Pirs => "PIRS",
        }
    }

    fn parse(data: &[u8]) -> Result<Self> {
        match data.get(..4) {
            Some(b"LIVE") => Ok(Magic::Live),
            Some(b"CON ") => Ok(Magic::Con),
            Some(b"PIRS") => Ok(Magic::Pirs),
            Some(other) => Err(Error::BadMagic { offset: 0, expected: b"LIVE", found: other.to_vec() }),
            None => Err(Error::UnexpectedEof { offset: 0, wanted: 4, available: data.len() }),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct License {
    pub licensee_id: u64,
    pub license_bits: u32,
    pub license_flags: u32,
}

#[derive(Clone, Debug)]
pub struct Header {
    pub magic: Magic,
    pub signature: Vec<u8>,
    pub licenses: Vec<License>,
    pub content_id: [u8; 20],
    pub header_size: u32,
    pub data_start: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExecutionInfo {
    pub media_id: u32,
    pub version: u32,
    pub base_version: u32,
    pub title_id: u32,
    pub platform: u8,
    pub executable_table: u8,
    pub disc_number: u8,
    pub disc_count: u8,
    pub savegame_id: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VolumeDescriptor {
    pub descriptor_length: u8,
    pub version: u8,
    pub flags: u8,
    pub file_table_block_count: u16,
    pub file_table_block_number: u32,
    pub top_hash: [u8; 20],
    pub total_block_count: u32,
    pub free_block_count: u32,
}

impl VolumeDescriptor {
    pub fn read_only(&self) -> bool {
        self.flags & 0x01 != 0
    }
}

#[derive(Clone, Debug)]
pub struct Metadata {
    pub content_type: u32,
    pub metadata_version: u32,
    pub content_size: u64,
    pub execution: ExecutionInfo,
    pub console_id: [u8; 5],
    pub profile_id: u64,
    pub volume: VolumeDescriptor,
    pub data_file_count: u32,
    pub data_file_size: u64,
    pub volume_type: u32,
    pub online_creator: u64,
    pub category: u32,
    pub device_id: [u8; 20],
    pub display_names: Vec<String>,
    pub descriptions: Vec<String>,
    pub publisher: String,
    pub title_name: String,
    pub attributes: u8,
    pub thumbnail_size: u32,
    pub title_thumbnail_size: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DirectoryEntry {
    pub name: String,
    pub flags: u8,
    pub valid_blocks: u32,
    pub alloc_blocks: u32,
    pub start_block: u32,
    pub parent: u16,
    pub length: u32,
    pub create_date: u16,
    pub create_time: u16,
    pub modified_date: u16,
    pub modified_time: u16,
    pub path: String,
}

impl DirectoryEntry {
    pub fn is_directory(&self) -> bool {
        self.flags & 0x80 != 0
    }

    pub fn is_contiguous(&self) -> bool {
        self.flags & 0x40 != 0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HashEntry {
    pub sha1: [u8; 20],
    pub info: u32,
}

impl HashEntry {
    pub fn next_block(&self) -> u32 {
        self.info & 0x00FF_FFFF
    }

    pub fn state(&self) -> u8 {
        (self.info >> 30) as u8
    }

    pub fn active_index(&self) -> bool {
        self.info & 0x4000_0000 != 0
    }

    pub fn writeable(&self) -> bool {
        self.info & 0x8000_0000 != 0
    }
}

#[derive(Clone, Debug, Default)]
pub struct VerifyReport {
    pub data_blocks: u32,
    pub blocks_verified: u32,
    pub tables_verified: u32,
    pub top_hash_ok: bool,
    pub content_id_ok: bool,
    pub files: usize,
    pub directories: usize,
    pub chains: usize,
}

#[derive(Clone, Debug)]
pub struct XContentPackage<'a> {
    data: &'a [u8],
    header: Header,
    metadata: Metadata,
    entries: Vec<DirectoryEntry>,
    file_table_blocks: Vec<u32>,
    by_path: HashMap<String, usize>,
}

impl<'a> XContentPackage<'a> {
    pub fn parse(data: &'a [u8]) -> Result<Self> {
        let magic = Magic::parse(data)?;
        let mut r = Reader::be(data);

        r.seek(0x04)?;
        let signature = r.bytes(0x228)?.to_vec();
        r.seek(0x22C)?;
        let mut licenses = Vec::with_capacity(16);
        for _ in 0..16 {
            licenses.push(License { licensee_id: r.u64()?, license_bits: r.u32()?, license_flags: r.u32()? });
        }
        let content_id: [u8; 20] = r.array()?;
        let header_size = r.u32()?;
        if (header_size as usize) < METADATA_END {
            return Err(Error::invalid(format!(
                "header_size {header_size:#x} does not hold the 0x93d6-byte metadata at 0x344"
            )));
        }
        let data_start = usize::try_from((header_size as u64).next_multiple_of(BLOCK_SIZE as u64))
            .map_err(|_| Error::invalid(format!("header_size {header_size:#x} gives no data start")))?;
        if data_start > data.len() {
            return Err(Error::UnexpectedEof { offset: data_start, wanted: 0, available: data.len() });
        }

        r.seek(METADATA_OFFSET)?;
        let content_type = r.u32()?;
        let metadata_version = r.u32()?;
        let content_size = r.u64()?;
        let execution = ExecutionInfo {
            media_id: r.u32()?,
            version: r.u32()?,
            base_version: r.u32()?,
            title_id: r.u32()?,
            platform: r.u8()?,
            executable_table: r.u8()?,
            disc_number: r.u8()?,
            disc_count: r.u8()?,
            savegame_id: r.u32()?,
        };
        let console_id: [u8; 5] = r.array()?;
        let profile_id = r.u64()?;
        let volume = VolumeDescriptor {
            descriptor_length: r.u8()?,
            version: r.u8()?,
            flags: r.u8()?,
            file_table_block_count: u16::from_le_bytes(r.array()?),
            file_table_block_number: read_u24_le(&mut r)?,
            top_hash: r.array()?,
            total_block_count: r.u32()?,
            free_block_count: r.u32()?,
        };
        if volume.descriptor_length != 0x24 {
            return Err(Error::invalid(format!(
                "STFS volume descriptor length {}, expected 0x24 (file offset {:#x})",
                volume.descriptor_length,
                METADATA_OFFSET + 0x35
            )));
        }
        if !volume.read_only() {
            return Err(Error::invalid(
                "writable CON/PIRS package (read_only_format = 0): its two-backing-block hash layout is not decoded",
            ));
        }

        let data_file_count = r.u32()?;
        let data_file_size = r.u64()?;
        let volume_type = r.u32()?;
        let online_creator = r.u64()?;
        let category = r.u32()?;
        r.skip(0x20)?;
        r.skip(0x24)?;
        let device_id: [u8; 20] = r.array()?;

        let mut display_names = Vec::with_capacity(12);
        for _ in 0..9 {
            display_names.push(read_utf16be(&mut r, 0x100)?);
        }
        let mut descriptions = Vec::with_capacity(12);
        for _ in 0..9 {
            descriptions.push(read_utf16be(&mut r, 0x100)?);
        }
        let publisher = read_utf16be(&mut r, 0x80)?;
        let title_name = read_utf16be(&mut r, 0x80)?;
        let attributes = r.u8()?;
        let thumbnail_size = r.u32()?;
        let title_thumbnail_size = r.u32()?;
        r.skip(0x3D00)?;
        for _ in 0..3 {
            display_names.push(read_utf16be(&mut r, 0x100)?);
        }
        r.skip(0x3D00)?;
        for _ in 0..3 {
            descriptions.push(read_utf16be(&mut r, 0x100)?);
        }
        if r.pos() != METADATA_END {
            return Err(Error::invalid(format!("metadata ends at {:#x}, expected {METADATA_END:#x}", r.pos())));
        }

        let data_area = data.len() - data_start;
        if content_size != data_area as u64 {
            return Err(Error::invalid(format!(
                "content_size {content_size} != data area {data_area} bytes (file offset {data_start:#x})"
            )));
        }
        if content_size % BLOCK_SIZE as u64 != 0 {
            return Err(Error::invalid(format!("content_size {content_size} is not a multiple of 0x1000")));
        }
        let total = volume.total_block_count;
        if total == 0 {
            return Err(Error::invalid("STFS volume has no data blocks (file offset 0x393)"));
        }
        if u64::from(total) > L2_BLOCKS {
            return Err(Error::invalid(format!(
                "STFS volume has {total} data blocks, three hash levels hold {L2_BLOCKS}"
            )));
        }
        if volume.file_table_block_count == 0 {
            return Err(Error::invalid("STFS volume has no file-table blocks (file offset 0x37c)"));
        }
        if u64::from(volume.file_table_block_count) > u64::from(total) {
            return Err(Error::invalid(format!(
                "file_table_block_count {} exceeds total_block_count {total}",
                volume.file_table_block_count
            )));
        }
        let last_slot = slot(total - 1);
        let blocks_used = last_slot + 1;
        if blocks_used * BLOCK_SIZE as u64 != content_size {
            return Err(Error::invalid(format!(
                "{total} data blocks plus their hash tables occupy {blocks_used} data-area blocks, but content_size \
                 is {}",
                content_size / BLOCK_SIZE as u64
            )));
        }

        let (entries, file_table_blocks) = parse_file_table(data, data_start, &volume)?;
        let mut package = Self {
            data,
            header: Header { magic, signature, licenses, content_id, header_size, data_start },
            metadata: Metadata {
                content_type,
                metadata_version,
                content_size,
                execution,
                console_id,
                profile_id,
                volume,
                data_file_count,
                data_file_size,
                volume_type,
                online_creator,
                category,
                device_id,
                display_names,
                descriptions,
                publisher,
                title_name,
                attributes,
                thumbnail_size,
                title_thumbnail_size,
            },
            entries,
            file_table_blocks,
            by_path: HashMap::new(),
        };
        package.build_paths()?;
        Ok(package)
    }

    fn build_paths(&mut self) -> Result<()> {
        let mut paths: Vec<String> = Vec::with_capacity(self.entries.len());
        for (index, entry) in self.entries.iter().enumerate() {
            let path = if entry.parent == 0xFFFF {
                entry.name.clone()
            } else {
                let parent = entry.parent as usize;
                if parent >= index {
                    return Err(Error::invalid(format!(
                        "entry {index} ({:?}) names parent {parent}, which is not an earlier entry",
                        entry.name
                    )));
                }
                if !self.entries[parent].is_directory() {
                    return Err(Error::invalid(format!(
                        "entry {index} ({:?}) names parent {parent} ({:?}), which is not a directory",
                        entry.name, self.entries[parent].name
                    )));
                }
                format!("{}/{}", paths[parent], entry.name)
            };
            let key = path.to_ascii_lowercase();
            if let Some(previous) = self.by_path.insert(key, index) {
                return Err(Error::invalid(format!("entries {previous} and {index} share the path {path:?}")));
            }
            paths.push(path);
        }
        for (entry, path) in self.entries.iter_mut().zip(paths) {
            entry.path = path;
        }
        Ok(())
    }

    pub fn data(&self) -> &'a [u8] {
        self.data
    }

    pub fn header(&self) -> &Header {
        &self.header
    }

    pub fn metadata(&self) -> &Metadata {
        &self.metadata
    }

    pub fn entries(&self) -> &[DirectoryEntry] {
        &self.entries
    }

    pub fn file_table_blocks(&self) -> &[u32] {
        &self.file_table_blocks
    }

    pub fn entry(&self, id: usize) -> Result<&DirectoryEntry> {
        self.entries
            .get(id)
            .ok_or_else(|| Error::invalid(format!("entry id {id} out of range ({} entries)", self.entries.len())))
    }

    pub fn find(&self, path: &str) -> Option<usize> {
        let normalized = path.trim_start_matches(['/', '\\']).replace('\\', "/");
        self.by_path.get(&normalized.to_ascii_lowercase()).copied()
    }

    pub fn read(&self, path: &str) -> Result<Vec<u8>> {
        let id = self.find(path).ok_or_else(|| Error::invalid(format!("{path:?} is not in the package")))?;
        self.read_id(id)
    }

    pub fn read_root(&self, path: &str, cap: usize) -> Result<Vec<u8>> {
        let id = self
            .find(path)
            .filter(|&id| !self.entries[id].is_directory())
            .ok_or_else(|| Error::invalid(format!("the package has no {path} at its root")))?;
        let length = self.entry(id)?.length as usize;
        if length > cap {
            return Err(Error::invalid(format!("{path} is {length} bytes (at most {cap})")));
        }
        self.read_id(id)
    }

    pub fn read_id(&self, id: usize) -> Result<Vec<u8>> {
        let entry = self.entry(id)?;
        let chain = self.chain(id)?;
        let length = entry.length as usize;
        let mut out = Vec::with_capacity(length);
        for block in chain {
            let take = (length - out.len()).min(BLOCK_SIZE);
            out.extend_from_slice(&self.block(block)?[..take]);
        }
        Ok(out)
    }

    pub fn read_chain_range(&self, chain: &[u32], length: u64, offset: u64, len: usize) -> Result<Vec<u8>> {
        let end = offset
            .checked_add(len as u64)
            .filter(|&end| end <= length)
            .ok_or_else(|| Error::invalid(format!("bytes {offset:#x}+{len:#x} are past the file's {length:#x}")))?;
        let mut out = Vec::with_capacity(len);
        let mut at = offset;
        while at < end {
            let index = usize::try_from(at / BLOCK_SIZE as u64).map_err(|_| Error::invalid("file offset overflows"))?;
            let block = *chain
                .get(index)
                .ok_or_else(|| Error::invalid(format!("byte {at:#x} is past the chain's {} blocks", chain.len())))?;
            let within = (at % BLOCK_SIZE as u64) as usize;
            let take = usize::try_from(end - at).map_or(BLOCK_SIZE - within, |left| left.min(BLOCK_SIZE - within));
            out.extend_from_slice(&self.block(block)?[within..within + take]);
            at += take as u64;
        }
        Ok(out)
    }

    pub fn chain(&self, id: usize) -> Result<Vec<u32>> {
        let entry = self.entry(id)?;
        if entry.is_directory() {
            return Err(Error::invalid(format!("entry {id} ({:?}) is a directory", entry.path)));
        }
        let length = entry.length as usize;
        let alloc = entry.alloc_blocks as usize;
        let mut chain = Vec::with_capacity(alloc);
        if length == 0 {
            return Ok(chain);
        }
        let mut block = entry.start_block;
        let mut seen = HashSet::with_capacity(alloc);
        let mut delivered = 0usize;
        while delivered < length {
            if block == END_OF_CHAIN {
                return Err(Error::invalid(format!(
                    "chain of {:?} ended after {delivered} of {length} bytes",
                    entry.path
                )));
            }
            if block >= self.metadata.volume.total_block_count {
                return Err(Error::invalid(format!(
                    "chain of {:?} uses block {block}, beyond {} data blocks",
                    entry.path, self.metadata.volume.total_block_count
                )));
            }
            if !seen.insert(block) {
                return Err(Error::invalid(format!("chain of {:?} loops at block {block}", entry.path)));
            }
            chain.push(block);
            delivered += (length - delivered).min(BLOCK_SIZE);
            block = self.hash_entry(block)?.next_block();
        }
        if chain.len() != alloc {
            return Err(Error::invalid(format!(
                "chain of {:?} has {} blocks, its entry allocates {alloc}",
                entry.path,
                chain.len()
            )));
        }
        if block != END_OF_CHAIN {
            return Err(Error::invalid(format!(
                "chain of {:?} continues to block {block} after its {alloc} allocated blocks",
                entry.path
            )));
        }
        Ok(chain)
    }

    pub fn block_offset(&self, block: u32) -> Result<usize> {
        block_offset(self.header.data_start, self.metadata.volume.total_block_count, block)
    }

    pub fn block(&self, block: u32) -> Result<&'a [u8]> {
        let offset = self.block_offset(block)?;
        self.data.get(offset..offset + BLOCK_SIZE).ok_or(Error::UnexpectedEof {
            offset,
            wanted: BLOCK_SIZE,
            available: self.data.len().saturating_sub(offset),
        })
    }

    pub fn hash_entry(&self, block: u32) -> Result<HashEntry> {
        hash_entry(self.data, self.header.data_start, block)
    }

    pub fn top_table_block(&self) -> u64 {
        top_table_block(self.metadata.volume.total_block_count)
    }

    pub fn verify_top_hash(&self) -> Result<()> {
        let slot = self.top_table_block();
        let offset = self.table_offset(slot)?;
        let table = self.data.get(offset..offset + BLOCK_SIZE).ok_or(Error::UnexpectedEof {
            offset,
            wanted: BLOCK_SIZE,
            available: self.data.len().saturating_sub(offset),
        })?;
        let digest: [u8; 20] = Sha1::digest(table).into();
        if digest != self.metadata.volume.top_hash {
            return Err(Error::invalid(format!(
                "top hash table at data-area block {slot}: SHA-1 {} != descriptor's {}",
                hex(&digest),
                hex(&self.metadata.volume.top_hash)
            )));
        }
        Ok(())
    }

    pub fn verify_content_id(&self) -> Result<()> {
        let digest: [u8; 20] = Sha1::digest(&self.data[METADATA_OFFSET..self.header.data_start]).into();
        if digest != self.header.content_id {
            return Err(Error::invalid(format!(
                "content id {} != SHA-1 of the metadata region {}",
                hex(&self.header.content_id),
                hex(&digest)
            )));
        }
        Ok(())
    }

    pub fn verify(&self) -> Result<VerifyReport> {
        let total = self.metadata.volume.total_block_count;
        let mut report = VerifyReport { data_blocks: total, ..VerifyReport::default() };
        for block in 0..total {
            let expected = self.hash_entry(block)?.sha1;
            let digest: [u8; 20] = Sha1::digest(self.block(block)?).into();
            if digest != expected {
                return Err(Error::invalid(format!(
                    "data block {block} at offset {:#x}: SHA-1 {} != hash entry {}",
                    self.block_offset(block)?,
                    hex(&digest),
                    hex(&expected)
                )));
            }
            report.blocks_verified += 1;
        }
        report.tables_verified = self.verify_tables()?;
        self.verify_top_hash()?;
        report.top_hash_ok = true;
        self.verify_content_id()?;
        report.content_id_ok = true;
        let mut owner: HashMap<u32, usize> = HashMap::with_capacity(total as usize);
        for &block in &self.file_table_blocks {
            owner.insert(block, usize::MAX);
        }
        for (id, entry) in self.entries.iter().enumerate() {
            if entry.is_directory() {
                report.directories += 1;
                continue;
            }
            for block in self.chain(id)? {
                if let Some(other) = owner.insert(block, id) {
                    let other =
                        if other == usize::MAX { "the file table".to_string() } else { format!("entry {other}") };
                    return Err(Error::invalid(format!("block {block} of {:?} is also used by {other}", entry.path)));
                }
            }
            report.files += 1;
            report.chains += 1;
        }
        Ok(report)
    }

    fn verify_tables(&self) -> Result<u32> {
        let total = u64::from(self.metadata.volume.total_block_count);
        let level = top_level(self.metadata.volume.total_block_count);
        let mut checked = 0u32;
        for (child_level, covered) in [(0u8, L0_BLOCKS), (1u8, L1_BLOCKS)] {
            if level <= child_level {
                break;
            }
            for first in (0..total).step_by(covered as usize) {
                let first = u32::try_from(first).map_err(|_| Error::invalid("data block number overflows"))?;
                let (child, parent) = if child_level == 0 {
                    (l0_table_block(first), l1_table_block(first))
                } else {
                    (l1_table_block(first), L2_TABLE_BLOCK)
                };
                let record = (u64::from(first) / covered) % L0_BLOCKS;
                let expected = self.table_entry(parent, record)?.sha1;
                let offset = self.table_offset(child)?;
                let digest: [u8; 20] = Sha1::digest(&self.data[offset..offset + BLOCK_SIZE]).into();
                if digest != expected {
                    return Err(Error::invalid(format!(
                        "level-{child_level} hash table at data-area block {child}: SHA-1 {} != level-{} entry {record} \
                         at block {parent} ({})",
                        hex(&digest),
                        child_level + 1,
                        hex(&expected)
                    )));
                }
                checked += 1;
            }
        }
        Ok(checked)
    }

    fn table_entry(&self, table: u64, record: u64) -> Result<HashEntry> {
        let offset = self.table_offset(table)? + (record % L0_BLOCKS) as usize * 0x18;
        let raw = &self.data[offset..offset + 0x18];
        let mut sha1 = [0u8; 20];
        sha1.copy_from_slice(&raw[..20]);
        Ok(HashEntry { sha1, info: u32::from_be_bytes([raw[0x14], raw[0x15], raw[0x16], raw[0x17]]) })
    }

    fn table_offset(&self, slot: u64) -> Result<usize> {
        table_offset(self.header.data_start, self.data.len(), slot)
    }
}

fn parse_file_table(
    data: &[u8],
    data_start: usize,
    volume: &VolumeDescriptor,
) -> Result<(Vec<DirectoryEntry>, Vec<u32>)> {
    let total = volume.total_block_count;
    let mut entries = Vec::new();
    let mut blocks = Vec::with_capacity(volume.file_table_block_count as usize);
    let mut seen = HashSet::with_capacity(volume.file_table_block_count as usize);
    let mut block = volume.file_table_block_number;
    for index in 0..volume.file_table_block_count {
        if block >= total {
            return Err(Error::invalid(format!(
                "file-table block {block} is beyond {total} data blocks (table entry {index})"
            )));
        }
        if !seen.insert(block) {
            return Err(Error::invalid(format!("file-table chain loops at block {block}")));
        }
        blocks.push(block);
        let offset = block_offset(data_start, total, block)?;
        let raw = data.get(offset..offset + BLOCK_SIZE).ok_or(Error::UnexpectedEof {
            offset,
            wanted: BLOCK_SIZE,
            available: data.len().saturating_sub(offset),
        })?;
        for slot in 0..ENTRIES_PER_BLOCK {
            let raw = &raw[slot * ENTRY_SIZE..(slot + 1) * ENTRY_SIZE];
            let flags = raw[0x28];
            let name_len = (flags & 0x3F) as usize;
            if name_len == 0 {
                break;
            }
            if name_len > 0x28 {
                return Err(Error::invalid(format!(
                    "file-table entry {} has name length {name_len}, the field holds 0x28",
                    entries.len()
                )));
            }
            let name = decode_cp1252(&raw[..name_len]);
            validate_name(&name, entries.len())?;
            let entry = DirectoryEntry {
                name,
                flags,
                valid_blocks: read_u24_le_bytes(&raw[0x29..0x2C]),
                alloc_blocks: read_u24_le_bytes(&raw[0x2C..0x2F]),
                start_block: read_u24_le_bytes(&raw[0x2F..0x32]),
                parent: u16::from_be_bytes([raw[0x32], raw[0x33]]),
                length: u32::from_be_bytes([raw[0x34], raw[0x35], raw[0x36], raw[0x37]]),
                create_date: u16::from_be_bytes([raw[0x38], raw[0x39]]),
                create_time: u16::from_be_bytes([raw[0x3A], raw[0x3B]]),
                modified_date: u16::from_be_bytes([raw[0x3C], raw[0x3D]]),
                modified_time: u16::from_be_bytes([raw[0x3E], raw[0x3F]]),
                path: String::new(),
            };
            if entry.valid_blocks > entry.alloc_blocks {
                return Err(Error::invalid(format!(
                    "entry {:?} has {} valid blocks but only {} allocated",
                    entry.name, entry.valid_blocks, entry.alloc_blocks
                )));
            }
            if entry.is_directory() {
                if entry.length != 0 || entry.alloc_blocks != 0 {
                    return Err(Error::invalid(format!(
                        "directory {:?} has length {} and {} allocated blocks",
                        entry.name, entry.length, entry.alloc_blocks
                    )));
                }
            } else {
                let wanted = u64::from(entry.length).div_ceil(BLOCK_SIZE as u64);
                if u64::from(entry.alloc_blocks) != wanted {
                    return Err(Error::invalid(format!(
                        "file {:?} has length {} but allocates {} blocks, not {wanted}",
                        entry.name, entry.length, entry.alloc_blocks
                    )));
                }
                if entry.alloc_blocks > 0 && entry.start_block >= total {
                    return Err(Error::invalid(format!(
                        "file {:?} starts at block {}, beyond {total} data blocks",
                        entry.name, entry.start_block
                    )));
                }
            }
            entries.push(entry);
        }
        let next = hash_entry(data, data_start, block)?.next_block();
        if index + 1 == volume.file_table_block_count {
            if next != END_OF_CHAIN {
                return Err(Error::invalid(format!(
                    "file table continues to block {next} after {} blocks",
                    volume.file_table_block_count
                )));
            }
        } else if next == END_OF_CHAIN {
            return Err(Error::invalid(format!(
                "file table ends after {} of {} blocks",
                index + 1,
                volume.file_table_block_count
            )));
        } else {
            block = next;
        }
    }
    Ok((entries, blocks))
}

fn validate_name(name: &str, index: usize) -> Result<()> {
    if name == "." || name == ".." {
        return Err(Error::invalid(format!("file-table entry {index} has the name {name:?}")));
    }
    if name.contains('/') || name.contains('\\') {
        return Err(Error::invalid(format!(
            "file-table entry {index} has the name {name:?}, which contains a path separator"
        )));
    }
    if name.chars().any(char::is_control) {
        return Err(Error::invalid(format!(
            "file-table entry {index} has the name {name:?}, which contains a control character"
        )));
    }
    Ok(())
}

fn slot(block: u32) -> u64 {
    let mut slot = u64::from(block);
    for level_base in [L0_BLOCKS, L1_BLOCKS, L2_BLOCKS] {
        slot += (u64::from(block) + level_base) / level_base;
        if u64::from(block) < level_base {
            break;
        }
    }
    slot
}

fn l0_table_block(block: u32) -> u64 {
    if block < L0_BLOCKS as u32 {
        return 0;
    }
    let block = u64::from(block);
    (block / L0_BLOCKS) * (L0_BLOCKS + 1) + (block / L1_BLOCKS + 1) + u64::from(block >= L1_BLOCKS)
}

const L2_TABLE_BLOCK: u64 = L1_BLOCKS + L0_BLOCKS + 1;

fn top_level(total: u32) -> u8 {
    // TODO(guess): the boundary itself (exactly 170 / 28 900 blocks) has no instance: the 9 STFS volumes checked
    // hold 5, 5, 195, 268, 2 207, 2 219, 2 249, 2 815 and 62 630 blocks (the 7 player packages, the disc's nxeart and
    // system update); Velocity's CalcualateTopLevel agrees (`<= 0xAA` → 0, `<= 0x70E4` → 1), Xenia's
    // GetAmountOfHashLevelsToCheck reads both one level higher (`total < 170` → 0); the console's STFS code is in its
    // kernel, not in the game's image; settle: a volume of exactly 170 or 28 900 blocks.
    if u64::from(total) > L1_BLOCKS {
        2
    } else if u64::from(total) > L0_BLOCKS {
        1
    } else {
        0
    }
}

fn top_table_block(total: u32) -> u64 {
    match top_level(total) {
        2 => L2_TABLE_BLOCK,
        1 => L0_BLOCKS + 1,
        _ => 0,
    }
}

fn l1_table_block(block: u32) -> u64 {
    let block = u64::from(block);
    if block < L1_BLOCKS { L0_BLOCKS + 1 } else { 1 + (block / L1_BLOCKS) * L2_TABLE_BLOCK }
}

fn block_offset(data_start: usize, total: u32, block: u32) -> Result<usize> {
    if block >= total {
        return Err(Error::invalid(format!("data block {block} is beyond {total} blocks")));
    }
    let slot = slot(block);
    let relative = slot
        .checked_mul(BLOCK_SIZE as u64)
        .ok_or_else(|| Error::invalid(format!("data block {block} offset overflows")))?;
    let offset = u64::try_from(data_start)
        .ok()
        .and_then(|start| start.checked_add(relative))
        .and_then(|offset| usize::try_from(offset).ok())
        .ok_or_else(|| Error::invalid(format!("data block {block} offset overflows")))?;
    Ok(offset)
}

fn table_offset(data_start: usize, data_len: usize, slot: u64) -> Result<usize> {
    let relative = slot
        .checked_mul(BLOCK_SIZE as u64)
        .ok_or_else(|| Error::invalid(format!("hash table block {slot} offset overflows")))?;
    let offset = data_start
        .checked_add(usize::try_from(relative).map_err(|_| Error::invalid("hash table offset overflows"))?)
        .ok_or_else(|| Error::invalid("hash table offset overflows"))?;
    if offset + BLOCK_SIZE > data_len {
        return Err(Error::UnexpectedEof { offset, wanted: BLOCK_SIZE, available: data_len.saturating_sub(offset) });
    }
    Ok(offset)
}

fn hash_entry(data: &[u8], data_start: usize, block: u32) -> Result<HashEntry> {
    let table = l0_table_block(block);
    let entry_offset = table
        .checked_mul(BLOCK_SIZE as u64)
        .and_then(|offset| offset.checked_add(u64::from(block % L0_BLOCKS as u32) * 0x18))
        .and_then(|offset| u64::try_from(data_start).ok().and_then(|start| start.checked_add(offset)))
        .and_then(|offset| usize::try_from(offset).ok())
        .ok_or_else(|| Error::invalid(format!("hash entry of data block {block} offset overflows")))?;
    let raw = data.get(entry_offset..entry_offset + 0x18).ok_or(Error::UnexpectedEof {
        offset: entry_offset,
        wanted: 0x18,
        available: data.len().saturating_sub(entry_offset),
    })?;
    let mut sha1 = [0u8; 20];
    sha1.copy_from_slice(&raw[..20]);
    Ok(HashEntry { sha1, info: u32::from_be_bytes([raw[0x14], raw[0x15], raw[0x16], raw[0x17]]) })
}

fn read_u24_le(r: &mut Reader<'_>) -> Result<u32> {
    Ok(read_u24_le_bytes(r.bytes(3)?))
}

fn read_u24_le_bytes(bytes: &[u8]) -> u32 {
    u32::from(bytes[0]) | (u32::from(bytes[1]) << 8) | (u32::from(bytes[2]) << 16)
}

fn read_utf16be(r: &mut Reader<'_>, byte_len: usize) -> Result<String> {
    let raw = r.bytes(byte_len)?;
    let (pairs, _) = raw.as_chunks::<2>();
    let units: Vec<u16> = pairs.iter().map(|pair| u16::from_be_bytes(*pair)).take_while(|&unit| unit != 0).collect();
    Ok(String::from_utf16_lossy(&units))
}

fn decode_cp1252(bytes: &[u8]) -> String {
    const HIGH: [char; 32] = [
        '€', '\u{81}', '‚', 'ƒ', '„', '…', '†', '‡', 'ˆ', '‰', 'Š', '‹', 'Œ', '\u{8d}', 'Ž', '\u{8f}', '\u{90}', '‘',
        '’', '“', '”', '•', '–', '—', '˜', '™', 'š', '›', 'œ', '\u{9d}', 'ž', 'Ÿ',
    ];
    bytes
        .iter()
        .map(|&byte| if (0x80..=0x9F).contains(&byte) { HIGH[(byte - 0x80) as usize] } else { char::from(byte) })
        .collect()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn synthetic_package() -> Vec<u8> {
        let mut data = vec![0u8; 0xB000 + 3 * BLOCK_SIZE];
        data[..4].copy_from_slice(b"LIVE");
        data[0x340..0x344].copy_from_slice(&0xAD0Eu32.to_be_bytes());
        let metadata_start = METADATA_OFFSET;
        let meta = &mut data[metadata_start..metadata_start + METADATA_SIZE];
        meta[0..4].copy_from_slice(&2u32.to_be_bytes());
        meta[4..8].copy_from_slice(&2u32.to_be_bytes());
        meta[8..16].copy_from_slice(&(3 * 0x1000u64).to_be_bytes());
        meta[0x1C..0x20].copy_from_slice(&0x4541_096Du32.to_be_bytes());
        meta[0x35] = 0x24;
        meta[0x37] = 0x01;
        meta[0x38..0x3A].copy_from_slice(&1u16.to_le_bytes());
        meta[0x51..0x55].copy_from_slice(&2u32.to_be_bytes());

        let data_start = 0xB000;
        let block0 = data_start + BLOCK_SIZE;
        data[block0..block0 + 9].copy_from_slice(b"hello.txt");
        data[block0 + 0x28] = 9;
        data[block0 + 0x29..block0 + 0x2C].copy_from_slice(&[1, 0, 0]);
        data[block0 + 0x2C..block0 + 0x2F].copy_from_slice(&[1, 0, 0]);
        data[block0 + 0x2F..block0 + 0x32].copy_from_slice(&[1, 0, 0]);
        data[block0 + 0x32..block0 + 0x34].copy_from_slice(&0xFFFFu16.to_be_bytes());
        data[block0 + 0x34..block0 + 0x38].copy_from_slice(&4u32.to_be_bytes());
        let block1 = data_start + 2 * BLOCK_SIZE;
        data[block1..block1 + 4].copy_from_slice(b"data");

        let table = data_start;
        let sha0: [u8; 20] = Sha1::digest(&data[block0..block0 + BLOCK_SIZE]).into();
        let sha1: [u8; 20] = Sha1::digest(&data[block1..block1 + BLOCK_SIZE]).into();
        data[table..table + 20].copy_from_slice(&sha0);
        data[table + 20..table + 24].copy_from_slice(&END_OF_CHAIN.to_be_bytes());
        data[table + 0x18..table + 0x18 + 20].copy_from_slice(&sha1);
        data[table + 0x18 + 20..table + 0x18 + 24].copy_from_slice(&END_OF_CHAIN.to_be_bytes());
        let top: [u8; 20] = Sha1::digest(&data[data_start..data_start + BLOCK_SIZE]).into();
        data[0x381..0x381 + 20].copy_from_slice(&top);
        let content: [u8; 20] = Sha1::digest(&data[METADATA_OFFSET..data_start]).into();
        data[0x32C..0x340].copy_from_slice(&content);
        data
    }

    fn package_with(total: u32, shared: bool) -> Vec<u8> {
        let area = slot(total - 1) + 1;
        let data_start = 0xB000;
        let mut data = vec![0u8; data_start + area as usize * BLOCK_SIZE];
        data[..4].copy_from_slice(b"LIVE");
        data[0x340..0x344].copy_from_slice(&0xAD0Eu32.to_be_bytes());
        let meta = &mut data[METADATA_OFFSET..METADATA_OFFSET + METADATA_SIZE];
        meta[0..4].copy_from_slice(&2u32.to_be_bytes());
        meta[8..16].copy_from_slice(&(area * BLOCK_SIZE as u64).to_be_bytes());
        meta[0x35] = 0x24;
        meta[0x37] = 0x01;
        meta[0x38..0x3A].copy_from_slice(&1u16.to_le_bytes());
        meta[0x51..0x55].copy_from_slice(&total.to_be_bytes());
        let at = |block: u32| data_start + slot(block) as usize * BLOCK_SIZE;
        let length = (total - 1) * BLOCK_SIZE as u32;
        let alloc = (total - 1).to_le_bytes();
        let ft = at(0);
        for (i, name) in [&b"f.bin"[..], b"twin"].iter().enumerate().take(if shared { 2 } else { 1 }) {
            let e = ft + i * ENTRY_SIZE;
            data[e..e + name.len()].copy_from_slice(name);
            data[e + 0x28] = name.len() as u8 | 0x40;
            data[e + 0x29..e + 0x2C].copy_from_slice(&alloc[..3]);
            data[e + 0x2C..e + 0x2F].copy_from_slice(&alloc[..3]);
            data[e + 0x2F..e + 0x32].copy_from_slice(&[1, 0, 0]);
            data[e + 0x32..e + 0x34].copy_from_slice(&0xFFFFu16.to_be_bytes());
            data[e + 0x34..e + 0x38].copy_from_slice(&length.to_be_bytes());
        }
        for block in 1..total {
            let b = at(block);
            data[b..b + 4].copy_from_slice(&block.to_be_bytes());
        }
        let put = |data: &mut Vec<u8>, table: u64, record: u64, sha: [u8; 20], info: u32| {
            let e = data_start + table as usize * BLOCK_SIZE + record as usize * 0x18;
            data[e..e + 20].copy_from_slice(&sha);
            data[e + 20..e + 24].copy_from_slice(&info.to_be_bytes());
        };
        for block in 0..total {
            let sha: [u8; 20] = Sha1::digest(&data[at(block)..at(block) + BLOCK_SIZE]).into();
            let next = if block == 0 || block + 1 == total { END_OF_CHAIN } else { block + 1 };
            put(&mut data, l0_table_block(block), u64::from(block) % L0_BLOCKS, sha, next);
        }
        if top_level(total) >= 1 {
            for first in (0..total).step_by(L0_BLOCKS as usize) {
                let t = data_start + l0_table_block(first) as usize * BLOCK_SIZE;
                let sha: [u8; 20] = Sha1::digest(&data[t..t + BLOCK_SIZE]).into();
                put(&mut data, l1_table_block(first), u64::from(first) / L0_BLOCKS % L0_BLOCKS, sha, 0);
            }
        }
        let top = data_start + top_table_block(total) as usize * BLOCK_SIZE;
        let top: [u8; 20] = Sha1::digest(&data[top..top + BLOCK_SIZE]).into();
        data[0x381..0x381 + 20].copy_from_slice(&top);
        let content: [u8; 20] = Sha1::digest(&data[METADATA_OFFSET..data_start]).into();
        data[0x32C..0x340].copy_from_slice(&content);
        data
    }

    #[test]
    fn the_hash_levels_follow_the_block_count() {
        assert_eq!((top_level(1), top_level(170), top_level(171)), (0, 0, 1));
        assert_eq!((top_level(28_900), top_level(28_901)), (1, 2));
        assert_eq!((top_table_block(170), top_table_block(171), top_table_block(28_901)), (0, 171, 29_071));
        assert_eq!((l1_table_block(0), l1_table_block(28_899), l1_table_block(28_900)), (171, 171, 29_072));
        let data = package_with(170, false);
        assert_eq!(data.len(), 0xB000 + 171 * BLOCK_SIZE);
        let report = XContentPackage::parse(&data).unwrap().verify().unwrap();
        assert_eq!((report.blocks_verified, report.tables_verified, report.files), (170, 0, 1));
        let data = package_with(171, false);
        let package = XContentPackage::parse(&data).unwrap();
        assert_eq!(package.top_table_block(), 171);
        let report = package.verify().unwrap();
        assert_eq!((report.blocks_verified, report.tables_verified), (171, 2));
        assert_eq!(package.read_id(0).unwrap().len(), 170 * BLOCK_SIZE);
    }

    #[test]
    fn a_level_0_table_that_its_level_1_entry_does_not_name_is_refused() {
        let mut data = package_with(171, false);
        let t = 0xB000 + l0_table_block(170) as usize * BLOCK_SIZE;
        data[t + 0xFF0] ^= 1;
        let err = XContentPackage::parse(&data).unwrap().verify().unwrap_err().to_string();
        assert!(err.contains("level-0 hash table at data-area block 172"), "{err}");
    }

    #[test]
    fn two_files_sharing_blocks_are_refused() {
        let data = package_with(4, true);
        let package = XContentPackage::parse(&data).unwrap();
        assert_eq!(package.read_id(0).unwrap(), package.read_id(1).unwrap());
        let err = package.verify().unwrap_err().to_string();
        assert!(err.contains("is also used by entry 0"), "{err}");
    }

    #[test]
    fn parses_and_reads_a_synthetic_package() {
        let data = synthetic_package();
        let package = XContentPackage::parse(&data).unwrap();
        assert_eq!(package.header().magic, Magic::Live);
        assert_eq!(package.metadata().volume.total_block_count, 2);
        assert_eq!(package.entries().len(), 1);
        assert_eq!(package.entries()[0].path, "hello.txt");
        assert_eq!(package.read("hello.txt").unwrap(), b"data");
        assert_eq!(package.read("HELLO.TXT").unwrap(), b"data");
        assert_eq!(package.read_id(0).unwrap(), b"data");
        let report = package.verify().unwrap();
        assert_eq!(report.blocks_verified, 2);
        assert!(report.top_hash_ok && report.content_id_ok);
        assert_eq!(report.files, 1);
    }

    #[test]
    fn rejects_a_bad_block_hash() {
        let mut data = synthetic_package();
        let block1 = 0xB000 + 2 * BLOCK_SIZE;
        data[block1] ^= 0xFF;
        let package = XContentPackage::parse(&data).unwrap();
        assert!(package.verify().is_err());
    }

    #[test]
    fn rejects_a_looping_chain() {
        let mut data = synthetic_package();
        let table = 0xB000;
        let entry = table + BLOCK_SIZE;
        data[entry + 0x2C..entry + 0x2F].copy_from_slice(&[2, 0, 0]);
        data[entry + 0x34..entry + 0x38].copy_from_slice(&4097u32.to_be_bytes());
        data[table + 0x18 + 0x14..table + 0x18 + 0x18].copy_from_slice(&1u32.to_be_bytes());
        let package = XContentPackage::parse(&data).unwrap();
        assert!(package.read_id(0).is_err());
    }

    #[test]
    fn rejects_a_short_chain() {
        let mut data = synthetic_package();
        let table = 0xB000;
        let entry = table + BLOCK_SIZE;
        data[entry + 0x2C..entry + 0x2F].copy_from_slice(&[2, 0, 0]);
        data[entry + 0x34..entry + 0x38].copy_from_slice(&4097u32.to_be_bytes());
        let package = XContentPackage::parse(&data).unwrap();
        assert!(package.read_id(0).is_err());
    }

    #[test]
    fn rejects_a_bad_structure() {
        let mut data = synthetic_package();
        data[0x379 + 2] = 0x00;
        assert!(XContentPackage::parse(&data).is_err());

        let mut data = synthetic_package();
        data[..4].copy_from_slice(b"CON ");
        assert!(XContentPackage::parse(&data).is_ok());
        data[..4].copy_from_slice(b"PIRS");
        assert!(XContentPackage::parse(&data).is_ok());

        let mut data = synthetic_package();
        data[0x379] = 0x23;
        assert!(XContentPackage::parse(&data).is_err());

        let mut data = synthetic_package();
        data[0x344 + 8..0x344 + 16].copy_from_slice(&0x4000u64.to_be_bytes());
        assert!(XContentPackage::parse(&data).is_err());
    }
}
