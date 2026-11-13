use crate::archive::big::{BigArchive, BigEntry, HEADER_LEN};
use crate::archive::xcontent::{DirectoryEntry, XContentPackage};
use crate::reader::Reader;
use crate::{Error, Result};

pub const TITLE_ID: u32 = 0x4541_096D;
pub const CONTENT_TYPE_DLC: u32 = 2;
pub const INFO_VERSION: u8 = 2;
pub const TOC_VERSION: u32 = 1;
pub const MAX_TOC_ENTRIES: usize = 0x40;
pub const MAX_TOC_BYTES: usize = 0x10 + MAX_TOC_ENTRIES * 12;
pub const MAX_INFO_BYTES: usize = 4096;
pub const MAX_LANGUAGE_SLOTS: u8 = 12;
pub const MAX_MEMBER_BYTES: usize = 4 * 25_600_364;

pub fn check_mount_name(name: &str) -> Result<()> {
    if name.is_empty() || name.len() > 64 {
        return Err(Error::invalid(format!("DLC mount name {name:?} is not 1…64 bytes")));
    }
    if name == "." || name == ".." {
        return Err(Error::invalid(format!("DLC mount name {name:?} is not a folder name")));
    }
    if let Some(c) = name.chars().find(|c| !(c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))) {
        return Err(Error::invalid(format!("DLC mount name {name:?} contains {c:?}")));
    }
    if name.ends_with('.') {
        return Err(Error::invalid(format!("DLC mount name {name:?} ends in '.'")));
    }
    if super::is_reserved_device_name(name) {
        return Err(Error::invalid(format!("DLC mount name {name:?} is a reserved device name")));
    }
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DlcLanguage {
    pub slot: u8,
    pub name: String,
    pub description: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DlcInfo {
    pub version: u8,
    pub name: String,
    pub unk_01: u8,
    pub unk_00: u8,
    pub category: String,
    pub pairs: Vec<(String, String)>,
    pub languages: Vec<DlcLanguage>,
}

impl DlcInfo {
    pub fn parse(data: &[u8]) -> Result<Self> {
        if data.len() > MAX_INFO_BYTES {
            return Err(Error::invalid(format!(
                "info.dlc is {} bytes (at most {MAX_INFO_BYTES}; the five packages have 149…354)",
                data.len()
            )));
        }
        let mut r = Reader::be(data);
        r.skip(8)?;
        let version = r.u8()?;
        let name = read_len_string(&mut r, "the mount name")?;
        check_mount_name(&name)?;
        let unk_01 = r.u8()?;
        let unk_00 = r.u8()?;
        let category = read_len_string(&mut r, "the category")?;
        let kv_count = signed_count(r.u8()?);
        let mut pairs: Vec<(String, String)> = Vec::with_capacity(kv_count.min(16));
        for _ in 0..kv_count {
            let key = read_len_string(&mut r, "a key/value key")?;
            let value = read_len_string(&mut r, "a key/value value")?;
            match pairs.iter_mut().find(|(k, _)| path_hash(k) == path_hash(&key)) {
                Some(pair) => *pair = (key, value),
                None => pairs.push((key, value)),
            }
        }
        let lang_count = signed_count(r.u8()?);
        let mut languages: Vec<DlcLanguage> = Vec::with_capacity(lang_count.min(MAX_LANGUAGE_SLOTS as usize));
        for _ in 0..lang_count {
            let slot = r.u8()?;
            if slot >= MAX_LANGUAGE_SLOTS {
                return Err(Error::invalid(format!(
                    "info.dlc names language slot {slot}: the game's record holds slots 0…{} and would be \
                     overwritten",
                    MAX_LANGUAGE_SLOTS - 1
                )));
            }
            let entry = DlcLanguage {
                slot,
                name: read_len_string(&mut r, "a language name")?,
                description: read_len_string(&mut r, "a language description")?,
            };
            match languages.iter_mut().find(|l| l.slot == slot) {
                Some(l) => *l = entry,
                None => languages.push(entry),
            }
        }
        if r.pos() != data.len() {
            return Err(Error::invalid(format!(
                "info.dlc ends at {:#x}, but the file is {} bytes",
                r.pos(),
                data.len()
            )));
        }
        Ok(DlcInfo { version, name, unk_01, unk_00, category, pairs, languages })
    }

    pub fn pair(&self, key: &str) -> Option<&str> {
        let hash = path_hash(key);
        self.pairs.iter().find(|(k, _)| path_hash(k) == hash).map(|(_, v)| v.as_str())
    }

    pub fn mount_version(&self) -> Option<u32> {
        let text = self.pair("mountversion")?;
        if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        text.parse().ok()
    }

    pub fn display_name(&self) -> Option<&str> {
        self.languages.iter().find(|l| l.slot == 0).or_else(|| self.languages.first()).map(|l| l.name.as_str())
    }
}

fn signed_count(byte: u8) -> usize {
    usize::try_from(byte as i8).unwrap_or(0)
}

fn read_len_string(r: &mut Reader<'_>, what: &str) -> Result<String> {
    let mut len = 0usize;
    for group in 0.. {
        if group == 5 {
            return Err(Error::invalid(format!("{what}'s length at offset {:#x} has more than five groups", r.pos())));
        }
        let byte = r.u8()?;
        len |= usize::from(byte & 0x7f) << (7 * group);
        if byte & 0x80 == 0 {
            break;
        }
    }
    let bytes = r.bytes(len.checked_add(1).ok_or_else(|| Error::invalid(format!("{what}: length {len}")))?)?;
    if bytes[len] != 0 {
        return Err(Error::invalid(format!("{what} at offset {:#x} is not NUL-terminated", r.pos() - 1)));
    }
    if bytes[..len].contains(&0) {
        return Err(Error::invalid(format!("{what} holds a NUL inside its {len} bytes")));
    }
    std::str::from_utf8(&bytes[..len])
        .map(str::to_string)
        .map_err(|e| Error::invalid(format!("{what} is not UTF-8: {e}")))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DlcTocEntry {
    pub hash_hi: u32,
    pub hash_lo: u32,
    pub size: u32,
}

impl DlcTocEntry {
    pub fn hash(self) -> u64 {
        u64::from(self.hash_hi) << 32 | u64::from(self.hash_lo)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DlcToc {
    pub version: u32,
    pub entries: Vec<DlcTocEntry>,
}

impl DlcToc {
    pub fn parse(data: &[u8]) -> Result<Self> {
        if data.len() > MAX_TOC_BYTES {
            return Err(Error::invalid(format!(
                "dlc.toc is {} bytes (at most {MAX_TOC_BYTES}: a 0x10-byte header and {MAX_TOC_ENTRIES} entries)",
                data.len()
            )));
        }
        let mut r = Reader::be(data);
        r.expect(b"DLC_TOC_")?;
        let version = r.u32()?;
        if version != TOC_VERSION {
            return Err(Error::invalid(format!("dlc.toc version {version}, this parser knows {TOC_VERSION}")));
        }
        let count = r.u32()? as usize;
        if count > MAX_TOC_ENTRIES {
            return Err(Error::invalid(format!("dlc.toc has {count} entries (at most {MAX_TOC_ENTRIES})")));
        }
        let mut entries = Vec::with_capacity(count);
        for _ in 0..count {
            entries.push(DlcTocEntry { hash_hi: r.u32()?, hash_lo: r.u32()?, size: r.u32()? });
        }
        if r.pos() != data.len() {
            return Err(Error::invalid(format!(
                "dlc.toc ends at {:#x} after {count} entries, but the file is {} bytes",
                r.pos(),
                data.len()
            )));
        }
        Ok(DlcToc { version, entries })
    }

    pub fn find(&self, hash: u64) -> Option<&DlcTocEntry> {
        self.entries.iter().find(|e| e.hash() == hash)
    }
}

pub fn path_hash(path: &str) -> u64 {
    let mut h = 0x1505u64;
    for byte in path.bytes().take_while(|&b| b != 0) {
        let byte = match byte {
            b'\\' => b'/',
            b'A'..=b'Z' => byte + 32,
            other => other,
        };
        h = h.wrapping_mul(0x21).wrapping_add(i64::from(byte as i8) as u64);
    }
    h
}

pub fn check_member_size(size: u32) -> Result<()> {
    if size as usize > MAX_MEMBER_BYTES {
        return Err(Error::invalid(format!("inner EB member of {size} bytes (at most {MAX_MEMBER_BYTES})")));
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MemberReport {
    pub members: usize,
    pub compressed: usize,
    pub raw: usize,
    pub bytes: u64,
}

#[derive(Debug)]
pub struct DlcPackage<'a> {
    package: XContentPackage<'a>,
    info: DlcInfo,
    toc: DlcToc,
    big: BigArchive,
    big_chain: Vec<u32>,
    big_len: u64,
}

impl<'a> DlcPackage<'a> {
    pub fn parse(data: &'a [u8]) -> Result<Self> {
        let package = XContentPackage::parse(data)?;
        let metadata = package.metadata();
        if metadata.content_type != CONTENT_TYPE_DLC {
            return Err(Error::invalid(format!(
                "content type {:#010x}, not a DLC package ({CONTENT_TYPE_DLC:#010x})",
                metadata.content_type
            )));
        }
        if metadata.execution.title_id != TITLE_ID {
            return Err(Error::invalid(format!(
                "title id {:#010x}, not {TITLE_ID:#010x}",
                metadata.execution.title_id
            )));
        }
        let info_bytes = package.read_root("info.dlc", MAX_INFO_BYTES)?;
        let info = DlcInfo::parse(&info_bytes)?;
        let toc_bytes = package.read_root("dlc.toc", MAX_TOC_BYTES)?;
        let toc = DlcToc::parse(&toc_bytes)?;

        let dir = package
            .find("dlc")
            .filter(|&id| package.entries()[id].is_directory())
            .ok_or_else(|| Error::invalid("the package has no dlc/ directory"))?;
        let files = files_below(&package, dir)?;
        if files.is_empty() {
            return Err(Error::invalid("the package's dlc/ directory holds no file"));
        }
        let mut matched = vec![false; toc.entries.len()];
        for (rel, id) in &files {
            let entry: &DirectoryEntry = package.entry(*id)?;
            let hash = path_hash(rel);
            let found: Vec<usize> = toc
                .entries
                .iter()
                .enumerate()
                .filter(|(_, t)| t.hash() == hash && t.size == entry.length)
                .map(|(i, _)| i)
                .collect();
            let [only] = found[..] else {
                return Err(Error::invalid(format!(
                    "dlc/{rel} ({hash:#018x}, {} bytes) matches {} dlc.toc entries, not 1",
                    entry.length,
                    found.len()
                )));
            };
            matched[only] = true;
        }
        if let Some((index, entry)) = toc.entries.iter().enumerate().find(|(i, _)| !matched[*i]) {
            return Err(Error::invalid(format!(
                "dlc.toc entry {index} ({:#018x}, {} bytes) names no file below dlc/",
                entry.hash(),
                entry.size
            )));
        }

        let big_name = format!("{}.big", info.name);
        let big_path = format!("dlc/{big_name}");
        let big_id = package
            .find(&big_path)
            .filter(|&id| !package.entries()[id].is_directory())
            .ok_or_else(|| Error::invalid(format!("the package has no {big_path} (its info.dlc mount name)")))?;
        let expected = package.entry(big_id)?.length;
        if toc.find(path_hash(&big_name)).is_none_or(|e| e.size != expected) {
            return Err(Error::invalid(format!("dlc.toc does not name {big_path} with its {expected} bytes")));
        }
        let big_chain = package.chain(big_id)?;
        let big_len = u64::from(expected);
        let header: [u8; HEADER_LEN] = package
            .read_chain_range(&big_chain, big_len, 0, HEADER_LEN)?
            .try_into()
            .map_err(|_| Error::invalid(format!("{big_path}: a short EB header")))?;
        let tables = BigArchive::table_size(&header)?;
        if tables as u64 > big_len {
            return Err(Error::invalid(format!(
                "{big_path}: its tables ({tables} bytes) are past its {big_len} bytes"
            )));
        }
        let big = BigArchive::parse(&package.read_chain_range(&big_chain, big_len, 0, tables.max(HEADER_LEN))?)?;
        if big.header.archive_size != big_len {
            return Err(Error::invalid(format!(
                "{big_path} declares {} bytes, the file has {big_len}",
                big.header.archive_size
            )));
        }
        for entry in &big.entries {
            check_member_size(entry.size)?;
            let (offset, stored) = BigArchive::stored_range(entry)?;
            if offset.checked_add(stored as u64).is_none_or(|end| end > big_len) {
                return Err(Error::invalid(format!(
                    "{big_path}: member {:?} ({stored} bytes at {offset:#x}) is past the archive's {big_len} bytes",
                    entry.path()
                )));
            }
        }
        Ok(DlcPackage { package, info, toc, big, big_chain, big_len })
    }

    pub fn package(&self) -> &XContentPackage<'a> {
        &self.package
    }

    pub fn info(&self) -> &DlcInfo {
        &self.info
    }

    pub fn toc(&self) -> &DlcToc {
        &self.toc
    }

    pub fn big(&self) -> &BigArchive {
        &self.big
    }

    pub fn big_len(&self) -> u64 {
        self.big_len
    }

    pub fn members(&self) -> &[BigEntry] {
        &self.big.entries
    }

    pub fn read(&self, path: &str) -> Result<Vec<u8>> {
        let entry = self
            .big
            .find(path)
            .ok_or_else(|| Error::invalid(format!("{path:?} is not in {}'s inner archive", self.info.name)))?;
        self.read_entry(entry).map(std::borrow::Cow::into_owned)
    }

    pub fn read_entry(&self, entry: &BigEntry) -> Result<std::borrow::Cow<'_, [u8]>> {
        check_member_size(entry.size)?;
        let (offset, stored) = BigArchive::stored_range(entry)?;
        let bytes = self.package.read_chain_range(&self.big_chain, self.big_len, offset, stored)?;
        if entry.flags & 0x01 == 0 {
            return Ok(std::borrow::Cow::Owned(bytes));
        }
        Ok(std::borrow::Cow::Owned(BigArchive::decode_stored(entry, &bytes)?.into_owned()))
    }

    pub fn read_optional(&self, path: &str) -> Result<Option<Vec<u8>>> {
        match self.big.find(path) {
            Some(entry) => self.read_entry(entry).map(|b| Some(b.into_owned())),
            None => Ok(None),
        }
    }

    pub fn verify_members(&self) -> Result<MemberReport> {
        let mut report = MemberReport { members: self.big.entries.len(), ..MemberReport::default() };
        for entry in &self.big.entries {
            let bytes = self.read_entry(entry)?;
            if bytes.len() != entry.size as usize {
                return Err(Error::invalid(format!(
                    "inner EB member {:?}: {} bytes decoded, {} declared",
                    entry.path(),
                    bytes.len(),
                    entry.size
                )));
            }
            report.bytes += bytes.len() as u64;
            if entry.flags & 0x01 != 0 {
                report.compressed += 1;
            } else {
                report.raw += 1;
            }
        }
        Ok(report)
    }
}

fn files_below(package: &XContentPackage<'_>, root: usize) -> Result<Vec<(String, usize)>> {
    let prefix = format!("{}/", package.entry(root)?.path);
    let mut out = Vec::new();
    for (id, entry) in package.entries().iter().enumerate() {
        if entry.is_directory() {
            continue;
        }
        if let Some(rel) = entry.path.strip_prefix(&prefix) {
            out.push((rel.to_string(), id));
        }
    }
    out.sort();
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_path_hash_is_the_games() {
        for (path, hash) in [
            ("dimecontentinfo.xml", 0x6066_944b_7931_62cau64),
            ("dlc_presell_japan.big", 0x849e_9652_b213_8e37),
            ("dlc_paid_mnt_eddie.big", 0x0cc3_fa4a_8384_e01d),
            ("dlc_free_presells_fuji.big", 0x14a6_37f8_6525_4b2f),
            ("dlc_presell_eddie.big", 0x849e_9007_a6b6_6748),
            ("dlc_presell_mac.big", 0x4cfe_07a9_fec0_177e),
        ] {
            assert_eq!(path_hash(path), hash, "{path}");
        }
        assert_eq!(path_hash("A\\B"), path_hash("a/b"));
        let first = 0x1505u64.wrapping_mul(0x21).wrapping_sub(0x3D);
        assert_eq!(path_hash("é"), first.wrapping_mul(0x21).wrapping_sub(0x57));
        assert_eq!(path_hash("ab\0cd"), path_hash("ab"));
    }

    fn info(name: &str, category: &str, pairs: &[(&str, &str)], langs: &[(u8, &str, &str)]) -> Vec<u8> {
        let mut v = b"DLC_INFO".to_vec();
        v.push(INFO_VERSION);
        let string = |v: &mut Vec<u8>, s: &str| {
            v.push(s.len() as u8);
            v.extend_from_slice(s.as_bytes());
            v.push(0);
        };
        string(&mut v, name);
        v.extend_from_slice(&[1, 0]);
        string(&mut v, category);
        v.push(pairs.len() as u8);
        for (k, val) in pairs {
            string(&mut v, k);
            string(&mut v, val);
        }
        v.push(langs.len() as u8);
        for (slot, n, d) in langs {
            v.push(*slot);
            string(&mut v, n);
            string(&mut v, d);
        }
        v
    }

    #[test]
    fn parses_info_dlc() {
        let bytes = info(
            "dlc_presell_japan",
            "Track Content",
            &[("vaultname", "dlc_presell_japan"), ("bigname", "dlc_presell_japan"), ("mountversion", "030")],
            &[(0, "SSX™ Japan Range", "SSX™ Japan Range")],
        );
        let parsed = DlcInfo::parse(&bytes).unwrap();
        assert_eq!(parsed.name, "dlc_presell_japan");
        assert_eq!((parsed.unk_01, parsed.unk_00), (1, 0));
        assert_eq!(parsed.category, "Track Content");
        assert_eq!(parsed.mount_version(), Some(30));
        assert_eq!(parsed.display_name(), Some("SSX™ Japan Range"));
        assert_eq!(parsed.languages[0].slot, 0);
        for cut in 0..bytes.len() {
            assert!(DlcInfo::parse(&bytes[..cut]).is_err(), "{cut}");
        }
        let mut longer = bytes.clone();
        longer.push(0);
        assert!(DlcInfo::parse(&longer).is_err());
    }

    #[test]
    fn refuses_hostile_info_dlc() {
        let good = |name: &str| info(name, "Track Content", &[], &[]);
        assert!(DlcInfo::parse(&good("dlc_x")).is_ok());
        let long = "x".repeat(65);
        for bad in ["", "..", "a/b", "a\\b", "con", "a b", "a.", long.as_str()] {
            assert!(DlcInfo::parse(&good(bad)).is_err(), "{bad:?}");
        }
        assert!(DlcInfo::parse(&info("a", "c", &[], &[(11, "a", "b")])).is_ok());
        assert!(DlcInfo::parse(&info("a", "c", &[], &[(12, "a", "b")])).is_err());
        let mut no_nul = good("a");
        let last = no_nul.len() - 1;
        no_nul[last] = b'x';
        assert!(DlcInfo::parse(&no_nul).is_err());
        assert!(DlcInfo::parse(&info("a", "c\0d", &[], &[])).is_err());
        assert!(DlcInfo::parse(&vec![0u8; MAX_INFO_BYTES + 1]).is_err());
    }

    #[test]
    fn info_dlc_reads_as_the_games_parser_does() {
        let mut other = info("dlc_x", "Track Content", &[], &[]);
        other[..8].copy_from_slice(b"XXXXXXXX");
        other[8] = 7;
        assert_eq!(DlcInfo::parse(&other).unwrap().version, 7);
        let bytes = info(
            "a",
            "c",
            &[("Mountversion", "010"), ("k", "x"), ("mountversion", "020")],
            &[(3, "first", "d"), (3, "second", "d")],
        );
        let parsed = DlcInfo::parse(&bytes).unwrap();
        assert_eq!(parsed.pairs, [("mountversion".to_string(), "020".to_string()), ("k".into(), "x".into())]);
        assert_eq!(parsed.mount_version(), Some(20));
        assert_eq!(parsed.pair("MOUNTVERSION"), Some("020"));
        assert_eq!(parsed.languages.len(), 1);
        assert_eq!(parsed.languages[0].name, "second");
        let mut negative = info("a", "c", &[], &[]);
        let at = negative.len() - 2;
        negative[at] = 0x80;
        assert!(DlcInfo::parse(&negative).unwrap().pairs.is_empty());
        let long = "x".repeat(200);
        let mut grouped = info("a", "c", &[], &[]);
        grouped.truncate(grouped.len() - 2);
        grouped.extend_from_slice(&[1, 0xC8, 0x01]);
        grouped.extend_from_slice(long.as_bytes());
        grouped.extend_from_slice(&[0, 1, b'v', 0, 0]);
        assert_eq!(DlcInfo::parse(&grouped).unwrap().pair(&long), Some("v"));
        let mut endless = info("a", "c", &[], &[]);
        endless.truncate(9);
        endless.extend_from_slice(&[0x80; 6]);
        assert!(DlcInfo::parse(&endless).is_err());
    }

    fn toc(entries: &[(u64, u32)]) -> Vec<u8> {
        let mut v = b"DLC_TOC_".to_vec();
        v.extend_from_slice(&TOC_VERSION.to_be_bytes());
        v.extend_from_slice(&(entries.len() as u32).to_be_bytes());
        for (hash, size) in entries {
            v.extend_from_slice(&((hash >> 32) as u32).to_be_bytes());
            v.extend_from_slice(&(*hash as u32).to_be_bytes());
            v.extend_from_slice(&size.to_be_bytes());
        }
        v
    }

    #[test]
    fn parses_toc() {
        let bytes = toc(&[(path_hash("dimecontentinfo.xml"), 318), (path_hash("dlc_presell_japan.big"), 9_071_008)]);
        let parsed = DlcToc::parse(&bytes).unwrap();
        assert_eq!(parsed.version, 1);
        assert_eq!(parsed.entries.len(), 2);
        assert_eq!(parsed.find(path_hash("dlc_presell_japan.big")).unwrap().size, 9_071_008);
        for cut in 0..bytes.len() {
            assert!(DlcToc::parse(&bytes[..cut]).is_err(), "{cut}");
        }
        let mut extra = bytes.clone();
        extra.push(0);
        assert!(DlcToc::parse(&extra).is_err());
        let mut many = b"DLC_TOC_".to_vec();
        many.extend_from_slice(&1u32.to_be_bytes());
        many.extend_from_slice(&(MAX_TOC_ENTRIES as u32 + 1).to_be_bytes());
        assert!(DlcToc::parse(&many).is_err());
        assert!(DlcToc::parse(&vec![0u8; MAX_TOC_BYTES + 1]).is_err());
    }

    #[test]
    fn refuses_garbage_packages() {
        for bytes in [&b""[..], b"LIVE", b"DLC_INFO", &[0xFF; 0x100], &[0u8; 0xB000]] {
            assert!(DlcPackage::parse(bytes).is_err());
        }
    }
}
