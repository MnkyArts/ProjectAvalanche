use std::collections::BTreeMap;

use crate::reader::Reader;
use crate::{Error, Result};

use super::fnv1;

const VLT_ORDER: [&[u8; 4]; 7] = [b"Vers", b"StrN", b"DepN", b"DatN", b"ExpN", b"PtrN", b"EndC"];
const PTR_END: u16 = 0;
const PTR_SELECT: u16 = 2;
const PTR_FIXUP: u16 = 3;

pub const FILE_VLT: u16 = 0;
pub const FILE_BIN: u16 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Chunk {
    pub tag: [u8; 4],
    pub offset: usize,
    pub size: usize,
}

impl Chunk {
    pub fn payload(&self) -> std::ops::Range<usize> {
        self.offset + 8..self.offset + self.size
    }
}

#[derive(Clone, Debug)]
pub struct Dependency {
    pub hash: u32,
    pub name: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Export {
    pub key: u32,
    pub type_hash: u32,
    pub size: u32,
    pub offset: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PtrTarget {
    pub file: u16,
    pub offset: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PtrRecord {
    pub offset: u32,
    pub kind: u16,
    pub index: u16,
    pub target: u32,
}

#[derive(Clone, Debug)]
pub struct Vault<'a> {
    pub vlt: &'a [u8],
    pub bin: &'a [u8],
    pub chunks: Vec<Chunk>,
    pub unk_version: [u32; 2],
    pub deps: Vec<Dependency>,
    pub exports: Vec<Export>,
    pub ptr_records: Vec<PtrRecord>,
    pub fixups: Vec<BTreeMap<u32, PtrTarget>>,
    pub bin_strings: Vec<(u32, String)>,
    pub bin_data_start: usize,
}

fn read_chunk(data: &[u8], offset: usize) -> Result<Chunk> {
    let mut r = Reader::be(data);
    r.seek(offset)?;
    let tag = r.array::<4>()?;
    let size = r.u32()? as usize;
    if size < 8 || offset + size > data.len() {
        return Err(Error::invalid(format!("chunk {:?} at {offset:#x}: bad size {size:#x}", tag_str(&tag))));
    }
    Ok(Chunk { tag, offset, size })
}

fn tag_str(tag: &[u8; 4]) -> String {
    String::from_utf8_lossy(tag).into_owned()
}

fn expect_zero(data: &[u8], range: std::ops::Range<usize>, what: &str) -> Result<()> {
    match data[range.clone()].iter().position(|&b| b != 0) {
        None => Ok(()),
        Some(i) => Err(Error::invalid(format!("{what}: non-zero byte at {:#x}", range.start + i))),
    }
}

pub fn cstr_in(vlt: &[u8], bin: &[u8], target: PtrTarget) -> Result<String> {
    let mut r = Reader::be(if target.file == FILE_VLT { vlt } else { bin });
    r.seek(target.offset as usize)?;
    r.cstr()
}

impl<'a> Vault<'a> {
    pub fn parse(vlt: &'a [u8], bin: &'a [u8]) -> Result<Self> {
        let mut chunks = Vec::with_capacity(VLT_ORDER.len());
        let mut pos = 0;
        for want in VLT_ORDER {
            let c = read_chunk(vlt, pos)?;
            if &c.tag != want {
                return Err(Error::invalid(format!(
                    "vault chunk at {pos:#x}: expected {:?}, found {:?}",
                    tag_str(want),
                    tag_str(&c.tag)
                )));
            }
            pos += c.size;
            chunks.push(c);
        }
        if pos != vlt.len() {
            return Err(Error::invalid(format!("{} trailing bytes after EndC", vlt.len() - pos)));
        }
        let [vers, strn, depn, datn, expn, ptrn, endc] = chunks[..] else { unreachable!() };
        if vers.size != 16 || strn.size != 16 || endc.size != 16 {
            return Err(Error::invalid("Vers/StrN/EndC chunk is not 16 bytes"));
        }
        expect_zero(vlt, strn.payload(), "StrN")?;
        expect_zero(vlt, endc.payload(), "EndC")?;
        let mut r = Reader::be(vlt);
        r.seek(vers.offset + 8)?;
        let unk_version = [r.u32()?, r.u32()?];

        let deps = parse_deps(vlt, depn)?;
        let exports = parse_exports(vlt, expn, datn)?;
        let (bin_strings, bin_data_start) = parse_bin(bin)?;
        let mut v = Vault {
            vlt,
            bin,
            chunks,
            unk_version,
            deps,
            exports,
            ptr_records: Vec::new(),
            fixups: Vec::new(),
            bin_strings,
            bin_data_start,
        };
        v.parse_pointers(ptrn)?;
        Ok(v)
    }

    pub fn file(&self, file: u16) -> &'a [u8] {
        if file == FILE_VLT { self.vlt } else { self.bin }
    }

    pub fn pointer(&self, file: u16, offset: u32) -> Option<PtrTarget> {
        self.fixups.get(file as usize)?.get(&offset).copied()
    }

    pub fn fixup_count(&self) -> usize {
        self.fixups.iter().map(BTreeMap::len).sum()
    }

    pub fn cstr(&self, target: PtrTarget) -> Result<String> {
        cstr_in(self.vlt, self.bin, target)
    }

    pub fn export_bytes(&self, e: &Export) -> &'a [u8] {
        &self.vlt[e.offset as usize..(e.offset + e.size) as usize]
    }

    fn parse_pointers(&mut self, ptrn: Chunk) -> Result<()> {
        let mut r = Reader::be(self.vlt);
        r.seek(ptrn.offset + 8)?;
        let end = ptrn.offset + ptrn.size;
        self.fixups = vec![BTreeMap::new(); self.deps.len()];
        let mut current: Option<u16> = None;
        loop {
            if r.pos() + 12 > end {
                return Err(Error::invalid("PtrN: no end record"));
            }
            let rec = PtrRecord { offset: r.u32()?, kind: r.u16()?, index: r.u16()?, target: r.u32()? };
            self.ptr_records.push(rec);
            let at = r.pos() - 12;
            if rec.kind != PTR_END && rec.index as usize >= self.deps.len() {
                return Err(Error::invalid(format!("PtrN record at {at:#x}: dependency {} out of range", rec.index)));
            }
            match rec.kind {
                PTR_END if rec.offset == 0 && rec.index == 0 && rec.target == 0 => break,
                PTR_SELECT if rec.offset == 0 && rec.target == 0 => current = Some(rec.index),
                PTR_FIXUP => {
                    let file = current.ok_or_else(|| Error::invalid("PtrN: fixup before any select record"))?;
                    let data = self.file(file);
                    let slot = rec.offset as usize;
                    if !slot.is_multiple_of(4) || slot + 4 > data.len() {
                        return Err(Error::invalid(format!("PtrN: slot {slot:#x} outside file {file}")));
                    }
                    if data[slot..slot + 4] != [0; 4] {
                        return Err(Error::invalid(format!("PtrN: slot {slot:#x} in file {file} is not stored as 0")));
                    }
                    if rec.target as usize >= self.file(rec.index).len() {
                        return Err(Error::invalid(format!(
                            "PtrN: target {:#x} outside file {}",
                            rec.target, rec.index
                        )));
                    }
                    let t = PtrTarget { file: rec.index, offset: rec.target };
                    if self.fixups[file as usize].insert(rec.offset, t).is_some() {
                        return Err(Error::invalid(format!("PtrN: slot {slot:#x} in file {file} fixed up twice")));
                    }
                }
                _ => return Err(Error::invalid(format!("PtrN record at {at:#x}: unexpected {rec:?}"))),
            }
        }
        expect_zero(self.vlt, r.pos()..end, "PtrN padding")
    }
}

fn parse_deps(vlt: &[u8], depn: Chunk) -> Result<Vec<Dependency>> {
    let mut r = Reader::be(vlt);
    r.seek(depn.offset + 8)?;
    let count = r.u32()? as usize;
    if count != 2 {
        return Err(Error::invalid(format!("DepN: {count} dependencies, expected 2 (.vlt, .bin)")));
    }
    let hashes: Vec<u32> = (0..count).map(|_| r.u32()).collect::<Result<_>>()?;
    let offsets: Vec<u32> = (0..count).map(|_| r.u32()).collect::<Result<_>>()?;
    let strings = r.pos();
    let end = depn.offset + depn.size;
    let mut deps = Vec::with_capacity(count);
    let mut last = strings;
    for (hash, off) in hashes.into_iter().zip(offsets) {
        let mut s = Reader::be(&vlt[..end]);
        s.seek(strings + off as usize)?;
        let name = s.cstr()?;
        last = last.max(s.pos());
        if fnv1(name.as_bytes()) != hash {
            return Err(Error::invalid(format!("DepN: hash {hash:#010x} is not FNV-1 of {name:?}")));
        }
        deps.push(Dependency { hash, name });
    }
    expect_zero(vlt, last..end, "DepN padding")?;
    let stem = |n: &str, ext: &str| n.strip_suffix(ext).map(str::to_owned);
    match (stem(&deps[0].name, ".vlt"), stem(&deps[1].name, ".bin")) {
        (Some(a), Some(b)) if a == b => Ok(deps),
        _ => Err(Error::invalid(format!("DepN: unexpected names {:?}, {:?}", deps[0].name, deps[1].name))),
    }
}

fn parse_exports(vlt: &[u8], expn: Chunk, datn: Chunk) -> Result<Vec<Export>> {
    let mut r = Reader::be(vlt);
    r.seek(expn.offset + 8)?;
    let count = r.u32()? as usize;
    let mut exports = Vec::with_capacity(count);
    let mut prev_end = datn.offset + 8;
    for _ in 0..count {
        let e = Export { key: r.u32()?, type_hash: r.u32()?, size: r.u32()?, offset: r.u32()? };
        let (start, end) = (e.offset as usize, e.offset as usize + e.size as usize);
        if !start.is_multiple_of(8) || start < prev_end || end > datn.offset + datn.size {
            return Err(Error::invalid(format!("ExpN: export {e:?} not 8-aligned/ordered inside DatN")));
        }
        expect_zero(vlt, prev_end..start, "DatN padding")?;
        prev_end = end;
        exports.push(e);
    }
    expect_zero(vlt, prev_end..datn.offset + datn.size, "DatN tail")?;
    expect_zero(vlt, r.pos()..expn.offset + expn.size, "ExpN padding")?;
    Ok(exports)
}

fn parse_bin(bin: &[u8]) -> Result<(Vec<(u32, String)>, usize)> {
    let stre = read_chunk(bin, 0)?;
    if &stre.tag != b"StrE" {
        return Err(Error::invalid("bin: first chunk is not StrE"));
    }
    let endc = read_chunk(bin, stre.size)?;
    if &endc.tag != b"EndC" || endc.size != 16 {
        return Err(Error::invalid("bin: StrE is not followed by a 16-byte EndC"));
    }
    expect_zero(bin, endc.payload(), "bin EndC")?;
    let payload = stre.payload();
    let used = bin[payload.clone()].iter().rposition(|&b| b != 0).map_or(0, |i| i + 2);
    let end = payload.start + used;
    if end > payload.end || payload.end - end >= 16 {
        return Err(Error::invalid(format!("bin StrE: {} padding bytes", payload.end.saturating_sub(end))));
    }
    let mut r = Reader::be(&bin[..end]);
    r.seek(payload.start)?;
    let mut strings = Vec::new();
    while r.remaining() > 0 {
        let at = r.pos() as u32;
        strings.push((at, r.cstr()?));
    }
    Ok((strings, endc.offset + endc.size))
}
