use sha1::{Digest, Sha1};

use crate::archive::big::{BigArchive, BigEntry};
use crate::archive::dlc::TITLE_ID;
use crate::archive::lzx::LzxDecoder;
use crate::archive::xcontent::XContentPackage;
use crate::reader::Reader;
use crate::{Error, Result};

pub const CONTENT_TYPE_TITLE_UPDATE: u32 = 0x000B_0000;
pub const MODULE_PATCH_DELTA: u32 = 0x40;
pub const KEY_FILE_FORMAT_INFO: u32 = 0x0000_03FF;
pub const KEY_DELTA_PATCH_DESCRIPTOR: u32 = 0x0000_05FF;
pub const KEY_EXECUTION_INFO: u32 = 0x0004_0006;
pub const KEY_CHECKSUM_TIMESTAMP: u32 = 0x0001_8002;
pub const COMPRESSION_DELTA: u16 = 3;
pub const MAX_XEXP_BYTES: usize = 64 << 20;
pub const MAX_PATCH_BYTES: usize = 64 << 20;
pub const MAX_PATCH_MEMBER_BYTES: usize = 16 << 20;
pub const MAX_PATCH_TOTAL_BYTES: u64 = 32 << 20;
pub const MAX_TARGET_HEADER_BYTES: usize = 0x10_000;
pub const MAX_WINDOW_BYTES: usize = 1 << 21;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct XexExecution {
    pub media_id: u32,
    pub version: u32,
    pub base_version: u32,
    pub title_id: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct XexIdentity {
    pub header_size: u32,
    pub execution: XexExecution,
    pub pe_timestamp: Option<u32>,
    pub header_sha1: [u8; 20],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HeaderRecord {
    pub old_address: u32,
    pub new_address: u32,
    pub uncompressed_length: u32,
    pub compressed_length: u32,
    pub data_offset: usize,
}

#[derive(Clone, Debug)]
pub struct XexpPatch {
    header: Vec<u8>,
    pub module_flags: u32,
    pub header_size: u32,
    pub window_size: u32,
    pub target_version: u32,
    pub source_version: u32,
    pub digest_source: [u8; 20],
    pub image_key_source: [u8; 16],
    pub target_header_size: u32,
    pub image_source_offset: u32,
    pub image_source_size: u32,
    pub image_target_offset: u32,
    pub records: Vec<HeaderRecord>,
}

impl XexpPatch {
    pub fn parse(data: &[u8]) -> Result<Self> {
        let mut r = Reader::be(data);
        r.expect(b"XEX2")?;
        let module_flags = r.u32()?;
        let header_size = r.u32()?;
        let _reserved = r.u32()?;
        let _security_offset = r.u32()?;
        let count = r.u32()? as usize;
        if count > data.len().saturating_sub(0x18) / 8 {
            return Err(Error::invalid(format!("XEXP optional header count {count} exceeds the file")));
        }
        if header_size as usize > data.len() || header_size as usize > MAX_XEXP_BYTES {
            return Err(Error::invalid(format!("XEXP header size {header_size:#x} exceeds the file")));
        }
        let mut opt = Vec::with_capacity(count.min(64));
        for _ in 0..count {
            opt.push((r.u32()?, r.u32()?));
        }
        if module_flags & MODULE_PATCH_DELTA == 0 {
            return Err(Error::invalid(format!("XEXP module flags {module_flags:#x} are not a delta patch")));
        }
        let find = |key: u32| opt.iter().find(|(k, _)| *k == key).map(|(_, v)| *v);
        let format =
            find(KEY_FILE_FORMAT_INFO).ok_or_else(|| Error::invalid("XEXP has no file-format info (0x3ff)"))?;
        let format = format as usize;
        let within =
            |offset: usize, size: usize| offset.checked_add(size).is_some_and(|end| end <= header_size as usize);
        if !within(format, 0x20) {
            return Err(Error::invalid(format!("XEXP file-format info at {format:#x} is outside the header")));
        }
        let mut fr = Reader::be(data);
        fr.seek(format)?;
        let info_size = fr.u32()?;
        let encryption = fr.u16()?;
        let compression = fr.u16()?;
        let window_size = fr.u32()?;
        if info_size != 0x24 {
            return Err(Error::invalid(format!("XEXP file-format info size {info_size:#x}, expected 0x24")));
        }
        if encryption != 1 {
            return Err(Error::invalid(format!("XEXP encryption type {encryption}, expected normal (1)")));
        }
        if compression != COMPRESSION_DELTA {
            return Err(Error::invalid(format!("XEXP compression type {compression}, expected delta (3)")));
        }
        if window_size == 0 || window_size as usize > MAX_WINDOW_BYTES || !window_size.is_power_of_two() {
            return Err(Error::invalid(format!("XEXP LZX window size {window_size:#x} is not a power of two")));
        }

        let descriptor = find(KEY_DELTA_PATCH_DESCRIPTOR)
            .ok_or_else(|| Error::invalid("XEXP has no delta-patch descriptor (0x5ff)"))?;
        let descriptor = descriptor as usize;
        if !within(descriptor, 0x4C) {
            return Err(Error::invalid(format!("XEXP delta descriptor at {descriptor:#x} is outside the header")));
        }
        let mut dr = Reader::be(data);
        dr.seek(descriptor)?;
        let size = dr.u32()?;
        let target_version = dr.u32()?;
        let source_version = dr.u32()?;
        let digest_source: [u8; 20] = dr.array()?;
        let image_key_source: [u8; 16] = dr.array()?;
        let target_header_size = dr.u32()?;
        let _headers_source_offset = dr.u32()?;
        let _headers_source_size = dr.u32()?;
        let _headers_target_offset = dr.u32()?;
        let image_source_offset = dr.u32()?;
        let image_source_size = dr.u32()?;
        let image_target_offset = dr.u32()?;
        if size < 0x4C {
            return Err(Error::invalid(format!("XEXP delta descriptor size {size:#x} holds no records")));
        }
        let end = descriptor
            .checked_add(size as usize)
            .filter(|end| *end <= header_size as usize && *end <= data.len())
            .ok_or_else(|| Error::invalid(format!("XEXP delta descriptor size {size:#x} exceeds the header")))?;
        if target_header_size == 0 || target_header_size as usize > MAX_TARGET_HEADER_BYTES {
            return Err(Error::invalid(format!(
                "XEXP target header size {target_header_size:#x} is not 1…{MAX_TARGET_HEADER_BYTES:#x}"
            )));
        }
        let records_start = descriptor + 0x4C;
        let records = parse_records(data, records_start, end)?;
        Ok(Self {
            header: data[..header_size as usize].to_vec(),
            module_flags,
            header_size,
            window_size,
            target_version,
            source_version,
            digest_source,
            image_key_source,
            target_header_size,
            image_source_offset,
            image_source_size,
            image_target_offset,
            records,
        })
    }

    pub fn patched_header(&self, base: &[u8]) -> Result<Vec<u8>> {
        let base = header_slice(base)?;
        let target = self.target_header_size as usize;
        let mut out = vec![0u8; target];
        let copy = base.len().min(target);
        out[..copy].copy_from_slice(&base[..copy]);
        for record in &self.records {
            apply_record(&mut out, record, self.window_size as usize, &self.header)?;
        }
        reset_file_format(&mut out)?;
        Ok(out)
    }

    pub fn version_text(&self) -> String {
        version_text(self.target_version)
    }
}

pub fn version_text(version: u32) -> String {
    format!("{}.{:02}", version >> 8, version & 0xFF)
}

pub fn timestamp_text(timestamp: u32) -> String {
    let days = i64::from(timestamp / 86_400);
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}")
}

fn parse_records(data: &[u8], start: usize, end: usize) -> Result<Vec<HeaderRecord>> {
    let mut records = Vec::new();
    let mut at = start;
    while at < end {
        if at + 12 > end {
            return Err(Error::invalid(format!("XEXP delta record at {at:#x} is cut by the descriptor's end")));
        }
        let mut r = Reader::be(data);
        r.seek(at)?;
        let old_address = r.u32()?;
        let new_address = r.u32()?;
        let uncompressed_length = u32::from(r.u16()?);
        let compressed_length = u32::from(r.u16()?);
        if old_address == 0 && new_address == 0 && uncompressed_length == 0 && compressed_length == 0 {
            if data[at..end].iter().any(|b| *b != 0) {
                return Err(Error::invalid(format!("XEXP delta descriptor has data after its end record at {at:#x}")));
            }
            at = end;
            break;
        }
        let data_offset = at + 12;
        let data_end = data_offset
            .checked_add(if compressed_length > 1 { compressed_length as usize } else { 0 })
            .ok_or_else(|| Error::invalid("XEXP delta record data overflows"))?;
        if data_end > end {
            return Err(Error::invalid(format!(
                "XEXP delta record at {at:#x} (compressed {compressed_length}) runs past the descriptor"
            )));
        }
        records.push(HeaderRecord { old_address, new_address, uncompressed_length, compressed_length, data_offset });
        if compressed_length > 1 {
            at = data_end;
        } else {
            at += 12;
        }
        if records.len() > 4096 {
            return Err(Error::invalid("XEXP delta descriptor has over 4096 records"));
        }
    }
    if at != end {
        return Err(Error::invalid(format!("XEXP delta records end at {at:#x}, the descriptor at {end:#x}")));
    }
    if records.is_empty() {
        return Err(Error::invalid("XEXP delta descriptor holds no record"));
    }
    Ok(records)
}

fn apply_record(out: &mut [u8], record: &HeaderRecord, window: usize, source: &[u8]) -> Result<()> {
    let len = record.uncompressed_length as usize;
    let new = record.new_address as usize;
    let old = record.old_address as usize;
    let dest = new
        .checked_add(len)
        .filter(|end| *end <= out.len())
        .ok_or_else(|| Error::invalid(format!("XEXP record target {new:#x}+{len:#x} is outside the header")))?;
    if record.compressed_length == 0 {
        out[new..dest].fill(0);
        return Ok(());
    }
    if record.compressed_length == 1 {
        let source = old
            .checked_add(len)
            .filter(|end| *end <= out.len())
            .ok_or_else(|| Error::invalid(format!("XEXP move source {old:#x}+{len:#x} is outside the header")))?;
        out.copy_within(old..source, new);
        return Ok(());
    }
    if old + len > out.len() {
        return Err(Error::invalid(format!(
            "XEXP LZX reference {old:#x}+{len:#x} is outside the {}-byte header",
            out.len()
        )));
    }
    let csz = record.compressed_length as usize;
    let stream = source.get(record.data_offset..record.data_offset + csz).ok_or_else(|| {
        Error::invalid(format!("XEXP LZX stream {:#x}+{csz:#x} is outside the header", record.data_offset))
    })?;
    let mut decoder = LzxDecoder::with_reference(window_bits(window)?, &out[old..old + len])?;
    let mut decoded = Vec::with_capacity(len);
    decoder.decode_frame(stream, len, &mut decoded)?;
    if decoded.len() != len {
        return Err(Error::invalid(format!("XEXP LZX record decoded {} of {len} bytes", decoded.len())));
    }
    out[new..dest].copy_from_slice(&decoded);
    Ok(())
}

fn window_bits(window: usize) -> Result<u32> {
    let bits = usize::BITS - window.leading_zeros() - 1;
    if !window.is_power_of_two() || !(15..=21).contains(&bits) {
        return Err(Error::invalid(format!("XEXP LZX window {window:#x} is no 2^15…2^21 window")));
    }
    Ok(bits)
}

fn header_slice(data: &[u8]) -> Result<&[u8]> {
    let mut r = Reader::be(data);
    r.expect(b"XEX2")?;
    let _flags = r.u32()?;
    let header_size = r.u32()?;
    let size = header_size as usize;
    if size < 0x18 || size > data.len() {
        return Err(Error::invalid(format!("XEX header size {header_size:#x} exceeds {} bytes", data.len())));
    }
    Ok(&data[..size])
}

pub fn xex_identity(header: &[u8]) -> Result<XexIdentity> {
    let header = header_slice(header)?;
    let mut r = Reader::be(header);
    r.seek(0x14)?;
    let count = r.u32()? as usize;
    if count > (header.len() - 0x18) / 8 {
        return Err(Error::invalid(format!("XEX optional header count {count} exceeds the header")));
    }
    let mut opt = Vec::with_capacity(count.min(64));
    for _ in 0..count {
        opt.push((r.u32()?, r.u32()?));
    }
    let find = |key: u32| opt.iter().find(|(k, _)| *k == key).map(|(_, v)| *v as usize);
    let exec = find(KEY_EXECUTION_INFO).ok_or_else(|| Error::invalid("XEX has no execution info"))?;
    let mut er = Reader::be(header);
    er.seek(exec)?;
    let execution =
        XexExecution { media_id: er.u32()?, version: er.u32()?, base_version: er.u32()?, title_id: er.u32()? };
    let pe_timestamp = match find(KEY_CHECKSUM_TIMESTAMP) {
        Some(at) => {
            let mut tr = Reader::be(header);
            tr.seek(at)?;
            let _checksum = tr.u32()?;
            Some(tr.u32()?)
        }
        None => None,
    };
    let header_sha1: [u8; 20] = Sha1::digest(header).into();
    Ok(XexIdentity { header_size: header.len() as u32, execution, pe_timestamp, header_sha1 })
}

fn reset_file_format(header: &mut [u8]) -> Result<()> {
    let mut r = Reader::be(header);
    r.expect(b"XEX2")?;
    r.seek(0x8)?;
    let header_size = r.u32()? as usize;
    if header_size > header.len() {
        return Err(Error::invalid(format!(
            "patched XEX header declares {header_size:#x} bytes, the patch wrote {:#x}",
            header.len()
        )));
    }
    let count = {
        r.seek(0x14)?;
        r.u32()? as usize
    };
    if count > header_size.saturating_sub(0x18) / 8 {
        return Err(Error::invalid("patched XEX optional header count exceeds the header"));
    }
    r.seek(0x18)?;
    let mut found = None;
    for _ in 0..count {
        let key = r.u32()?;
        let value = r.u32()? as usize;
        if key == KEY_FILE_FORMAT_INFO {
            found = Some(value);
        }
    }
    let at = found.ok_or_else(|| Error::invalid("patched XEX header has no file-format info"))?;
    let fields = at
        .checked_add(4)
        .and_then(|start| start.checked_add(4).map(|end| start..end))
        .filter(|range| range.end <= header_size)
        .ok_or_else(|| Error::invalid("patched XEX file-format info is outside the header"))?;
    header.get_mut(fields).ok_or_else(|| Error::invalid("patched XEX file-format info is outside the buffer"))?.fill(0);
    Ok(())
}

#[derive(Debug)]
pub struct PatchBig {
    big: BigArchive,
    data: Vec<u8>,
}

impl PatchBig {
    pub fn big(&self) -> &BigArchive {
        &self.big
    }

    pub fn members(&self) -> &[BigEntry] {
        &self.big.entries
    }

    pub fn data(&self) -> &[u8] {
        &self.data
    }

    pub fn read(&self, entry: &BigEntry) -> Result<std::borrow::Cow<'_, [u8]>> {
        if entry.size as usize > MAX_PATCH_MEMBER_BYTES {
            return Err(Error::invalid(format!(
                "patch0.big member {:?} is {} bytes (at most {MAX_PATCH_MEMBER_BYTES})",
                entry.path(),
                entry.size
            )));
        }
        let bytes = self.big.member(&self.data, entry)?;
        if bytes.len() != entry.size as usize {
            return Err(Error::invalid(format!(
                "patch0.big member {:?}: {} bytes decoded, {} declared",
                entry.path(),
                bytes.len(),
                entry.size
            )));
        }
        Ok(bytes)
    }

    pub fn decode(&self, entry: &BigEntry) -> Result<std::borrow::Cow<'_, [u8]>> {
        let bytes = self.read(entry)?;
        if crate::archive::chunk::detect(&bytes).is_none() {
            return Ok(bytes);
        }
        let header = crate::archive::chunk::read_header(&bytes)
            .map_err(|e| Error::invalid(format!("patch0.big member {:?}: its container: {e}", entry.path())))?;
        if header.full_size as usize > MAX_PATCH_MEMBER_BYTES {
            return Err(Error::invalid(format!(
                "patch0.big member {:?}: its container says {} bytes (at most {MAX_PATCH_MEMBER_BYTES})",
                entry.path(),
                header.full_size
            )));
        }
        let decoded = crate::archive::chunk::decompress(&bytes)
            .map_err(|e| Error::invalid(format!("patch0.big member {:?}: does not decode: {e}", entry.path())))?;
        if decoded.len() != header.full_size as usize {
            return Err(Error::invalid(format!(
                "patch0.big member {:?}: {} bytes decoded, its container says {}",
                entry.path(),
                decoded.len(),
                header.full_size
            )));
        }
        Ok(std::borrow::Cow::Owned(decoded))
    }

    pub fn verify(&self) -> Result<PatchReport> {
        let mut report = PatchReport { members: self.big.entries.len(), ..PatchReport::default() };
        for entry in &self.big.entries {
            let stored = self.read(entry)?;
            report.stored_bytes += stored.len() as u64;
            let container = crate::archive::chunk::detect(&stored).is_some();
            let bytes = if container { self.decode(entry)? } else { stored };
            report.bytes += bytes.len() as u64;
            if entry.flags & 1 != 0 {
                report.compressed += 1;
            } else {
                report.raw += 1;
            }
            if container {
                report.containers += 1;
            }
        }
        Ok(report)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PatchReport {
    pub members: usize,
    pub compressed: usize,
    pub raw: usize,
    pub containers: usize,
    pub stored_bytes: u64,
    pub bytes: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TuIdentity {
    pub target_version: u32,
    pub source_version: u32,
    pub execution: XexExecution,
    pub pe_timestamp: Option<u32>,
    pub header_sha1: [u8; 20],
}

#[derive(Debug)]
pub struct TitleUpdatePackage<'a> {
    package: XContentPackage<'a>,
    xexp: Vec<u8>,
    patch: Option<PatchBig>,
}

impl<'a> TitleUpdatePackage<'a> {
    pub fn parse(data: &'a [u8]) -> Result<Self> {
        let package = XContentPackage::parse(data)?;
        let metadata = package.metadata();
        if metadata.content_type != CONTENT_TYPE_TITLE_UPDATE {
            return Err(Error::invalid(format!(
                "content type {:#010x}, not a title update ({CONTENT_TYPE_TITLE_UPDATE:#010x})",
                metadata.content_type
            )));
        }
        if metadata.execution.title_id != TITLE_ID {
            return Err(Error::invalid(format!(
                "title id {:#010x}, not {TITLE_ID:#010x}",
                metadata.execution.title_id
            )));
        }
        let xexp = package.read_root("default.xexp", MAX_XEXP_BYTES)?;
        let patch = match package.find("patch0.big") {
            Some(id) if !package.entries()[id].is_directory() => {
                let length = package.entry(id)?.length as usize;
                if length > MAX_PATCH_BYTES {
                    return Err(Error::invalid(format!("patch0.big is {length} bytes (at most {MAX_PATCH_BYTES})")));
                }
                Some(parse_patch_big(package.read_id(id)?)?)
            }
            _ => None,
        };
        Ok(Self { package, xexp, patch })
    }

    pub fn package(&self) -> &XContentPackage<'a> {
        &self.package
    }

    pub fn data(&self) -> &'a [u8] {
        self.package.data()
    }

    pub fn xexp(&self) -> &[u8] {
        &self.xexp
    }

    pub fn patch(&self) -> Option<&PatchBig> {
        self.patch.as_ref()
    }

    pub fn header_patch(&self) -> Result<XexpPatch> {
        XexpPatch::parse(&self.xexp)
    }

    pub fn identify(&self, retail_header: Option<&[u8]>) -> Result<TuIdentity> {
        let patch = self.header_patch()?;
        let header = match retail_header {
            Some(base) => patch.patched_header(base)?,
            None => return Err(Error::invalid("the retail XEX header is needed to decode the patched header")),
        };
        let identity = xex_identity(&header)?;
        Ok(TuIdentity {
            target_version: patch.target_version,
            source_version: patch.source_version,
            execution: identity.execution,
            pe_timestamp: identity.pe_timestamp,
            header_sha1: identity.header_sha1,
        })
    }

    pub fn target_version(&self) -> Result<u32> {
        Ok(self.header_patch()?.target_version)
    }
}

fn parse_patch_big(bytes: Vec<u8>) -> Result<PatchBig> {
    let big = BigArchive::parse(&bytes)?;
    if big.header.archive_size != bytes.len() as u64 {
        return Err(Error::invalid(format!(
            "patch0.big declares {} bytes, the file has {}",
            big.header.archive_size,
            bytes.len()
        )));
    }
    if big.entries.is_empty() {
        return Err(Error::invalid("patch0.big has no member"));
    }
    let mut seen = std::collections::BTreeSet::new();
    let mut total = 0u64;
    for entry in &big.entries {
        let path = entry.path();
        check_member_path(&path)?;
        if !seen.insert(path.to_ascii_lowercase()) {
            return Err(Error::invalid(format!("patch0.big member {path:?} is listed twice")));
        }
        if entry.size as usize > MAX_PATCH_MEMBER_BYTES {
            return Err(Error::invalid(format!(
                "patch0.big member {path:?} is {} bytes (at most {MAX_PATCH_MEMBER_BYTES})",
                entry.size
            )));
        }
        total = total.checked_add(u64::from(entry.size)).ok_or_else(|| Error::invalid("patch0.big size overflows"))?;
        if total > MAX_PATCH_TOTAL_BYTES {
            return Err(Error::invalid(format!("patch0.big members are over {MAX_PATCH_TOTAL_BYTES} bytes")));
        }
    }
    Ok(PatchBig { big, data: bytes })
}

pub fn check_member_path(path: &str) -> Result<()> {
    if path.is_empty() || path.len() > 512 {
        return Err(Error::invalid(format!("patch0.big member {path:?} is not 1…512 bytes")));
    }
    if path.starts_with('/') || path.contains('\\') {
        return Err(Error::invalid(format!("patch0.big member {path:?} is not a relative '/' path")));
    }
    for segment in path.split('/') {
        if segment.is_empty() || segment == "." || segment == ".." {
            return Err(Error::invalid(format!("patch0.big member {path:?} has an empty or climbing segment")));
        }
        if segment.contains([':', '\0']) || segment.ends_with(['.', ' ']) {
            return Err(Error::invalid(format!("patch0.big member {path:?} has a segment Windows would fold")));
        }
        if super::is_reserved_device_name(segment) {
            return Err(Error::invalid(format!("patch0.big member {path:?} names a reserved device")));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_and_dates_are_the_packages() {
        assert_eq!(version_text(0x0302), "3.02");
        assert_eq!(version_text(0x0102), "1.02");
        assert_eq!(timestamp_text(0x4FFB_1EB8), "2012-07-09");
        assert_eq!(timestamp_text(0x4F28_B230), "2012-02-01");
        assert_eq!(timestamp_text(0x4F1A_2B98), "2012-01-21");
    }

    #[test]
    fn a_climbing_or_windows_member_path_is_refused() {
        for good in ["data/xenon/loc/eng_us.db", "credits.txt", "a/b-c_d.e"] {
            check_member_path(good).unwrap();
        }
        for bad in ["", "/data/x", "data\\x", "data/../x", "data//x", "data/CON", "data/nul.txt", "data/x.", "c:/x"] {
            assert!(check_member_path(bad).is_err(), "{bad}");
        }
    }

    fn patched(len: usize, size_word: u32, info: u32) -> Vec<u8> {
        let mut h = vec![0u8; len];
        h[0..4].copy_from_slice(b"XEX2");
        h[0x8..0xC].copy_from_slice(&size_word.to_be_bytes());
        h[0x14..0x18].copy_from_slice(&1u32.to_be_bytes());
        h[0x18..0x1C].copy_from_slice(&KEY_FILE_FORMAT_INFO.to_be_bytes());
        h[0x1C..0x20].copy_from_slice(&info.to_be_bytes());
        h
    }

    #[test]
    fn the_file_format_reset_stays_inside_the_patched_buffer() {
        let mut h = patched(0x40, 0x40, 0x20);
        h[0x24..0x28].copy_from_slice(&[0, 1, 0, 3]);
        reset_file_format(&mut h).unwrap();
        assert_eq!(&h[0x24..0x28], &[0, 0, 0, 0]);
        assert!(reset_file_format(&mut patched(0x40, 0xFFFF_0000, 0x1000)).is_err());
        assert!(reset_file_format(&mut patched(0x40, 0x41, 0x3C)).is_err());
        reset_file_format(&mut patched(0x40, 0x40, 0x38)).unwrap();
        assert!(reset_file_format(&mut patched(0x40, 0x40, 0x39)).is_err());
        assert!(reset_file_format(&mut patched(0x40, 0x40, u32::MAX)).is_err());
    }
}
