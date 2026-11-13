use std::collections::BTreeMap;
use std::io::{Read, Seek, SeekFrom, Write};

use super::{
    CENTRAL_SIG, DESCRIPTOR_SIG, END_SIG, FLAG_DESCRIPTOR, KNOWN_FLAGS, LOCAL_SIG, MAX_COMMENT, MAX_EXTRA,
    MAX_VERSION_NEEDED, Method, ZIP64_LOCATOR_SIG, ZipError, ZipLimits, ZipNames, method_name, refused, shown,
};
use crate::hash::ContentHash;
use crate::path::{MAX_PACKAGE_DEPTH, MAX_PATH_LEN, check_package_path};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ZipEntry {
    pub name: String,
    pub folder: bool,
    pub method: Method,
    pub crc32: u32,
    pub stored: u64,
    pub size: u64,
    data_start: u64,
}

pub struct ZipArchive<R> {
    reader: R,
    entries: Vec<ZipEntry>,
}

impl<R> std::fmt::Debug for ZipArchive<R> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ZipArchive({} entries)", self.entries.len())
    }
}

struct Central {
    name: Vec<u8>,
    made_by: u16,
    flags: u16,
    method: u16,
    crc: u32,
    stored: u32,
    size: u32,
    ext_attr: u32,
    offset: u32,
}

fn u16_at(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}

fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

fn check_extra(extra: &[u8], what: &str) -> Result<(), ZipError> {
    let mut at = 0usize;
    while at < extra.len() {
        let Some(head) = extra.get(at..at + 4) else {
            return Err(refused(format!("{what}: a malformed extra field")));
        };
        match u16_at(head, 0) {
            0x0001 => return Err(refused(format!("{what}: ZIP64 (packages are below 2 GiB and need none)"))),
            0x9901 => return Err(refused(format!("{what}: AES encryption (packages are not encrypted)"))),
            _ => {}
        }
        at += 4 + usize::from(u16_at(head, 2));
        if at > extra.len() {
            return Err(refused(format!("{what}: a malformed extra field")));
        }
    }
    Ok(())
}

fn check_kind(what: &str, folder: bool, made_by: u16, ext_attr: u32) -> Result<(), ZipError> {
    if matches!(made_by >> 8, 3 | 19) {
        let mode = ext_attr >> 16;
        match mode & 0o170_000 {
            0 => {}
            0o100_000 if !folder => {}
            0o040_000 if folder => {}
            0o100_000 => return Err(refused(format!("{what}: a file whose name ends in '/'"))),
            0o040_000 => return Err(refused(format!("{what}: a folder whose name does not end in '/'"))),
            0o120_000 => {
                return Err(refused(format!("{what}: a symbolic link (packages hold files and folders only)")));
            }
            t => return Err(refused(format!("{what}: a special file (mode {t:o}: device, FIFO or socket)"))),
        }
    }
    if ext_attr & 0x400 != 0 {
        return Err(refused(format!("{what}: a reparse point (a link; packages hold files and folders only)")));
    }
    if ext_attr & 0x10 != 0 && !folder {
        return Err(refused(format!("{what}: marked as a folder, but its name does not end in '/'")));
    }
    Ok(())
}

fn parse_central(cd: &[u8], count: usize) -> Result<Vec<Central>, ZipError> {
    let mut out = Vec::with_capacity(count);
    let mut at = 0usize;
    for n in 0..count {
        let Some(h) = cd.get(at..at + 46) else {
            return Err(refused(format!("the central directory ends inside entry {}", n + 1)));
        };
        if u32_at(h, 0) != CENTRAL_SIG {
            return Err(refused(format!("central directory entry {} has no header signature", n + 1)));
        }
        let (name_len, extra_len, comment_len) =
            (usize::from(u16_at(h, 28)), usize::from(u16_at(h, 30)), usize::from(u16_at(h, 32)));
        let Some(rest) = cd.get(at + 46..at + 46 + name_len + extra_len + comment_len) else {
            return Err(refused(format!("the central directory ends inside entry {}", n + 1)));
        };
        let name = rest[..name_len].to_vec();
        let what = shown(&name);
        if extra_len > MAX_EXTRA || comment_len > MAX_COMMENT {
            return Err(refused(format!("{what}: an extra field or comment over {MAX_EXTRA} bytes")));
        }
        check_extra(&rest[name_len..name_len + extra_len], &what)?;
        if u16_at(h, 34) != 0 {
            return Err(refused(format!("{what}: on another disk (a spanned archive)")));
        }
        let method = u16_at(h, 10);
        if method != 0 && method != 8 {
            return Err(refused(format!(
                "{what}: compressed with {} (packages use stored or deflate)",
                method_name(method)
            )));
        }
        let needed = u16_at(h, 6);
        if needed & 0xFF > MAX_VERSION_NEEDED {
            return Err(refused(format!(
                "{what}: needs zip version {}.{} (packages use stored or deflated files: 2.0)",
                (needed & 0xFF) / 10,
                (needed & 0xFF) % 10
            )));
        }
        out.push(Central {
            name,
            made_by: u16_at(h, 4),
            flags: u16_at(h, 8),
            method: u16_at(h, 10),
            crc: u32_at(h, 16),
            stored: u32_at(h, 20),
            size: u32_at(h, 24),
            ext_attr: u32_at(h, 38),
            offset: u32_at(h, 42),
        });
        at += 46 + name_len + extra_len + comment_len;
    }
    if at != cd.len() {
        return Err(refused("bytes after the last central directory entry"));
    }
    Ok(out)
}

fn entry_of(c: &Central, limits: &ZipLimits) -> Result<ZipEntry, ZipError> {
    let what = shown(&c.name);
    let Ok(text) = std::str::from_utf8(&c.name) else {
        return Err(refused(format!("{what}: a name that is not UTF-8")));
    };
    let (path, folder) = match text.strip_suffix('/') {
        Some(p) => (p, true),
        None => (text, false),
    };
    if path.len() > MAX_PATH_LEN {
        return Err(refused(format!("{what}: a name over {MAX_PATH_LEN} bytes")));
    }
    match limits.names {
        ZipNames::Package => check_package_path(path, false),
        ZipNames::PlayerDump => check_dump_path(path),
    }
    .map_err(|e| refused(format!("{what}: {e}")))?;
    if path.split('/').count() > MAX_PACKAGE_DEPTH {
        return Err(refused(format!("{what}: nested deeper than {MAX_PACKAGE_DEPTH} folders")));
    }
    if c.flags & 0x0001 != 0 || c.flags & 0x0040 != 0 || c.flags & 0x2000 != 0 {
        return Err(refused(format!("{what}: encrypted (packages are not encrypted)")));
    }
    if c.flags & !KNOWN_FLAGS != 0 {
        return Err(refused(format!("{what}: unknown general-purpose flags {:#06x}", c.flags & !KNOWN_FLAGS)));
    }
    let method = match c.method {
        0 => Method::Stored,
        8 => Method::Deflated,
        m => {
            return Err(refused(format!(
                "{what}: compressed with {} (packages use stored or deflate)",
                method_name(m)
            )));
        }
    };
    check_kind(&what, folder, c.made_by, c.ext_attr)?;
    let (stored, size) = (u64::from(c.stored), u64::from(c.size));
    if folder && size != 0 {
        return Err(refused(format!("{what}: a folder with {size} bytes of content")));
    }
    if method == Method::Stored && stored != size {
        return Err(refused(format!("{what}: stored, but {stored} bytes in the archive for a {size}-byte file")));
    }
    if size > limits.max_file_bytes {
        return Err(refused(format!("{what}: {size} bytes (a file is at most {})", limits.max_file_bytes)));
    }
    if limits.bomb(size, stored) {
        return Err(refused(format!(
            "{what}: {size} bytes from {stored} compressed, over {}:1 (a decompression bomb?)",
            limits.max_ratio
        )));
    }
    Ok(ZipEntry { name: path.to_string(), folder, method, crc32: c.crc, stored, size, data_start: 0 })
}

fn check_dump_path(path: &str) -> Result<(), String> {
    for seg in path.split('/') {
        if seg.starts_with(' ') || seg.ends_with(' ') || seg.contains(" .") {
            return Err(format!(
                "{} has a segment with a space at its start, end or before a '.'",
                ssx_core::text::quoted(path)
            ));
        }
    }
    check_package_path(&path.replace(' ', "_"), false)
}

fn check_names(entries: &[ZipEntry]) -> Result<(), ZipError> {
    let mut exact: BTreeMap<&str, bool> = BTreeMap::new();
    let mut folded: BTreeMap<String, &str> = BTreeMap::new();
    for e in entries {
        if exact.insert(&e.name, e.folder).is_some() {
            return Err(refused(format!("{}: named twice", e.name)));
        }
        let ends = e.name.match_indices('/').map(|(i, _)| i).chain([e.name.len()]);
        for end in ends {
            let prefix = &e.name[..end];
            let seen = folded.entry(prefix.to_ascii_lowercase()).or_insert(prefix);
            if *seen != prefix {
                return Err(refused(format!(
                    "{}: {} and {} differ only in case (one entry on Windows and macOS)",
                    ssx_core::text::shown(&e.name),
                    ssx_core::text::quoted(prefix),
                    ssx_core::text::quoted(seen)
                )));
            }
        }
    }
    for e in entries.iter().filter(|e| !e.folder) {
        let ends = e.name.match_indices('/').map(|(i, _)| i);
        for end in ends {
            if exact.get(&e.name[..end]) == Some(&false) {
                return Err(refused(format!("{}: {} is a file and a folder", e.name, &e.name[..end])));
            }
        }
    }
    Ok(())
}

impl<R: Read + Seek> ZipArchive<R> {
    pub fn open(mut reader: R, limits: &ZipLimits) -> Result<Self, ZipError> {
        let len = reader.seek(SeekFrom::End(0))?;
        if len > limits.max_archive_bytes {
            return Err(refused(format!("{len} bytes (an archive is at most {})", limits.max_archive_bytes)));
        }
        if len < 22 {
            return Err(refused("too short for a zip archive"));
        }
        let tail_len = len.min(22 + 0xFFFF) as usize;
        let mut tail = vec![0u8; tail_len];
        reader.seek(SeekFrom::Start(len - tail_len as u64))?;
        reader.read_exact(&mut tail)?;
        let ends: Vec<usize> = (0..=tail_len - 22)
            .rev()
            .filter(|&i| u32_at(&tail, i) == END_SIG && i + 22 + usize::from(u16_at(&tail, i + 20)) == tail_len)
            .collect();
        let at = match ends[..] {
            [one] => one,
            [] => return Err(refused("no end of central directory record: not a zip archive, or cut short")),
            _ => return Err(refused("several end of central directory records (another archive in the comment?)")),
        };
        let end_pos = len - tail_len as u64 + at as u64;
        if end_pos >= 20 {
            let mut sig = [0u8; 4];
            reader.seek(SeekFrom::Start(end_pos - 20))?;
            reader.read_exact(&mut sig)?;
            if u32::from_le_bytes(sig) == ZIP64_LOCATOR_SIG {
                return Err(refused("a ZIP64 archive (packages are below 2 GiB and need none)"));
            }
        }
        let e = &tail[at..at + 22];
        let (disk, cd_disk, here, total) = (u16_at(e, 4), u16_at(e, 6), u16_at(e, 8), u16_at(e, 10));
        let (cd_size, cd_offset) = (u32_at(e, 12), u32_at(e, 16));
        if disk != 0 || cd_disk != 0 || here != total {
            return Err(refused("a spanned (multi-part) archive"));
        }
        if total == 0xFFFF || cd_size == 0xFFFF_FFFF || cd_offset == 0xFFFF_FFFF {
            return Err(refused("a ZIP64 archive (packages are below 2 GiB and need none)"));
        }
        let count = usize::from(total);
        if count == 0 {
            return Err(refused("an empty archive"));
        }
        if count > limits.max_entries {
            return Err(refused(format!("{count} entries (at most {})", limits.max_entries)));
        }
        if u64::from(cd_offset) + u64::from(cd_size) != end_pos {
            return Err(refused(
                "the central directory does not end where the end record starts (hidden or missing bytes)",
            ));
        }
        let most = count as u64 * (46 + MAX_PATH_LEN as u64 + 1 + MAX_EXTRA as u64 + MAX_COMMENT as u64);
        if u64::from(cd_size) > most {
            return Err(refused(format!("a central directory of {cd_size} bytes for {count} entries")));
        }
        let mut cd = vec![0u8; cd_size as usize];
        reader.seek(SeekFrom::Start(u64::from(cd_offset)))?;
        reader.read_exact(&mut cd)?;
        let central = parse_central(&cd, count)?;
        let mut entries = Vec::with_capacity(count);
        let mut total_bytes = 0u64;
        for c in &central {
            let entry = entry_of(c, limits)?;
            total_bytes = total_bytes.saturating_add(entry.size);
            if total_bytes > limits.max_total_bytes {
                return Err(refused(format!("over {} bytes uncompressed in all", limits.max_total_bytes)));
            }
            entries.push(entry);
        }
        check_names(&entries)?;
        Self::tile(&mut reader, &central, &mut entries, u64::from(cd_offset))?;
        Ok(ZipArchive { reader, entries })
    }

    fn tile(reader: &mut R, central: &[Central], entries: &mut [ZipEntry], cd_offset: u64) -> Result<(), ZipError> {
        let mut order: Vec<usize> = (0..central.len()).collect();
        order.sort_by_key(|&i| central[i].offset);
        let mut expected = 0u64;
        for (k, &i) in order.iter().enumerate() {
            let c = &central[i];
            let what = &entries[i].name;
            let offset = u64::from(c.offset);
            if offset != expected {
                return Err(refused(format!(
                    "{what}: starts at byte {offset}, where {expected} was expected (overlapping entries or hidden bytes)"
                )));
            }
            let mut h = [0u8; 30];
            reader.seek(SeekFrom::Start(offset))?;
            reader.read_exact(&mut h).map_err(|_| refused(format!("{what}: its local header is cut short")))?;
            if u32_at(&h, 0) != LOCAL_SIG {
                return Err(refused(format!("{what}: no local header at byte {offset}")));
            }
            let (flags, method) = (u16_at(&h, 6), u16_at(&h, 8));
            if u16_at(&h, 4) & 0xFF > MAX_VERSION_NEEDED || flags != c.flags || method != c.method {
                return Err(refused(format!("{what}: the local header's version, flags or method differ")));
            }
            let (crc, stored, size) = (u32_at(&h, 14), u32_at(&h, 18), u32_at(&h, 22));
            let same = (crc, stored, size) == (c.crc, c.stored, c.size);
            if !(same || flags & FLAG_DESCRIPTOR != 0 && (crc, stored, size) == (0, 0, 0)) {
                return Err(refused(format!("{what}: the local header's CRC or sizes differ from the directory's")));
            }
            let (name_len, extra_len) = (usize::from(u16_at(&h, 26)), usize::from(u16_at(&h, 28)));
            if extra_len > MAX_EXTRA {
                return Err(refused(format!("{what}: an extra field over {MAX_EXTRA} bytes")));
            }
            let mut rest = vec![0u8; name_len + extra_len];
            reader.read_exact(&mut rest).map_err(|_| refused(format!("{what}: its local header is cut short")))?;
            if rest[..name_len] != c.name[..] {
                return Err(refused(format!("{what}: the local header names {}", shown(&rest[..name_len]))));
            }
            check_extra(&rest[name_len..], what)?;
            let data_start = offset + 30 + (name_len + extra_len) as u64;
            let data_end = data_start + u64::from(c.stored);
            let next = order.get(k + 1).map_or(cd_offset, |&j| u64::from(central[j].offset));
            if data_end > next {
                return Err(refused(format!("{what}: its data runs into the next entry (overlapping entries)")));
            }
            let gap = next - data_end;
            if flags & FLAG_DESCRIPTOR != 0 {
                let mut d = [0u8; 16];
                if gap != 12 && gap != 16 {
                    return Err(refused(format!("{what}: {gap} bytes after its data where a data descriptor belongs")));
                }
                reader.seek(SeekFrom::Start(data_end))?;
                reader.read_exact(&mut d[..gap as usize])?;
                let body = if gap == 16 {
                    if u32_at(&d, 0) != DESCRIPTOR_SIG {
                        return Err(refused(format!("{what}: a malformed data descriptor")));
                    }
                    &d[4..16]
                } else {
                    &d[..12]
                };
                if (u32_at(body, 0), u32_at(body, 4), u32_at(body, 8)) != (c.crc, c.stored, c.size) {
                    return Err(refused(format!("{what}: its data descriptor differs from the directory's values")));
                }
            } else if gap != 0 {
                return Err(refused(format!("{what}: {gap} hidden bytes after its data")));
            }
            entries[i].data_start = data_start;
            expected = next;
        }
        if expected != cd_offset {
            return Err(refused("hidden bytes before the central directory"));
        }
        Ok(())
    }

    pub fn entries(&self) -> &[ZipEntry] {
        &self.entries
    }

    pub fn extract(&mut self, i: usize, out: &mut dyn Write) -> Result<ContentHash, ZipError> {
        let e = self.entries.get(i).cloned().ok_or_else(|| refused(format!("no entry {i}")))?;
        self.reader.seek(SeekFrom::Start(e.data_start))?;
        let mut raw = (&mut self.reader).take(e.stored);
        let mut sink =
            Sink { name: &e.name, size: e.size, n: 0, crc: flate2::Crc::new(), hash: blake3::Hasher::new(), out };
        match e.method {
            Method::Stored => {
                let mut buf = vec![0u8; 64 << 10];
                loop {
                    let k = raw.read(&mut buf).map_err(|err| refused(format!("{}: {err}", e.name)))?;
                    if k == 0 {
                        break;
                    }
                    sink.put(&buf[..k])?;
                }
            }
            Method::Deflated => inflate(&mut raw, e.stored, &mut sink)?,
        }
        if sink.n != e.size {
            return Err(refused(format!("{}: {} bytes where {} were stated", e.name, sink.n, e.size)));
        }
        if sink.crc.sum() != e.crc32 {
            return Err(refused(format!("{}: its CRC-32 does not match (a damaged or altered archive)", e.name)));
        }
        Ok(ContentHash(*sink.hash.finalize().as_bytes()))
    }

    pub fn read(&mut self, i: usize, max: u64) -> Result<Vec<u8>, ZipError> {
        let size = self.entries.get(i).map_or(0, |e| e.size);
        if size > max {
            let name = self.entries.get(i).map_or("", |e| e.name.as_str());
            return Err(refused(format!("{name}: {size} bytes (at most {max} here)")));
        }
        let mut out = Vec::with_capacity(size as usize);
        self.extract(i, &mut out)?;
        Ok(out)
    }
}

struct Sink<'a> {
    name: &'a str,
    size: u64,
    n: u64,
    crc: flate2::Crc,
    hash: blake3::Hasher,
    out: &'a mut dyn Write,
}

impl Sink<'_> {
    fn put(&mut self, bytes: &[u8]) -> Result<(), ZipError> {
        self.n += bytes.len() as u64;
        if self.n > self.size {
            return Err(refused(format!("{}: expands past its stated {} bytes", self.name, self.size)));
        }
        self.crc.update(bytes);
        self.hash.update(bytes);
        self.out.write_all(bytes)?;
        Ok(())
    }
}

fn inflate(raw: &mut dyn Read, stored: u64, sink: &mut Sink<'_>) -> Result<(), ZipError> {
    use flate2::{Decompress, FlushDecompress, Status};
    let name = sink.name.to_string();
    let corrupt = |why: &str| refused(format!("{name}: {why}"));
    let mut dec = Decompress::new(false);
    let (mut input, mut output) = (vec![0u8; 64 << 10], vec![0u8; 64 << 10]);
    let (mut pos, mut len, mut left) = (0usize, 0usize, stored);
    loop {
        if pos == len && left > 0 {
            let want = left.min(input.len() as u64) as usize;
            raw.read_exact(&mut input[..want]).map_err(|e| corrupt(&e.to_string()))?;
            (pos, len, left) = (0, want, left - want as u64);
        }
        let (in0, out0) = (dec.total_in(), dec.total_out());
        let status = dec
            .decompress(&input[pos..len], &mut output, FlushDecompress::None)
            .map_err(|_| corrupt("a corrupt deflate stream"))?;
        let (consumed, produced) = ((dec.total_in() - in0) as usize, (dec.total_out() - out0) as usize);
        pos += consumed;
        sink.put(&output[..produced])?;
        match status {
            Status::StreamEnd => break,
            _ if consumed == 0 && produced == 0 => {
                let why = if pos == len && left == 0 {
                    "the deflate stream is cut short"
                } else {
                    "a corrupt deflate stream"
                };
                return Err(corrupt(why));
            }
            _ => {}
        }
    }
    if dec.total_in() != stored {
        return Err(corrupt("bytes after the end of its deflate stream"));
    }
    Ok(())
}
