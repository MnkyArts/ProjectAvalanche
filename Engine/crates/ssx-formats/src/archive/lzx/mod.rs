use crate::{Error, Result};

const MIN_WINDOW_BITS: u32 = 15;
const MAX_WINDOW_BITS: u32 = 21;
const NUM_CHARS: usize = 256;
const MAX_POSITION_SLOTS: usize = 50;
const MAX_MAIN_SYMBOLS: usize = NUM_CHARS + MAX_POSITION_SLOTS * 8;
const NUM_LENGTH_SYMBOLS: usize = 249;
const PRETREE_SYMBOLS: usize = 20;
const ALIGNED_SYMBOLS: usize = 8;
const NUM_PRIMARY_LENGTHS: usize = 7;
const MIN_MATCH: usize = 2;
const E8_LIMIT: u64 = 1 << 30;
const E8_MIN_FRAME: usize = 10;

const FOOTER_BITS: [u8; MAX_POSITION_SLOTS] = {
    let mut t = [0u8; MAX_POSITION_SLOTS];
    let mut slot = 4;
    while slot < MAX_POSITION_SLOTS {
        t[slot] = if slot < 36 { (slot / 2 - 1) as u8 } else { 17 };
        slot += 1;
    }
    t
};

const POSITION_BASE: [u32; MAX_POSITION_SLOTS] = {
    let mut t = [0u32; MAX_POSITION_SLOTS];
    let mut slot = 1;
    while slot < MAX_POSITION_SLOTS {
        t[slot] = t[slot - 1] + (1 << FOOTER_BITS[slot - 1]);
        slot += 1;
    }
    t
};

fn position_slots(window_bits: u32) -> usize {
    match window_bits {
        15 => 30,
        16 => 32,
        17 => 34,
        18 => 36,
        19 => 38,
        20 => 42,
        _ => 50,
    }
}

fn err(msg: impl std::fmt::Display) -> Error {
    Error::Decompress(format!("LZX: {msg}"))
}

struct BitReader<'a> {
    data: &'a [u8],
    pos: usize,
    buf: u32,
    bits: u32,
}

impl<'a> BitReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0, buf: 0, bits: 0 }
    }

    #[inline]
    fn word(&mut self) -> u32 {
        let p = self.pos;
        self.pos += 2;
        match self.data.get(p..p + 2) {
            Some(b) => u16::from_le_bytes([b[0], b[1]]) as u32,
            None => self.data.get(p).copied().unwrap_or(0) as u32,
        }
    }

    #[inline]
    fn ensure16(&mut self) {
        if self.bits < 16 {
            let w = self.word();
            self.buf |= w << (16 - self.bits);
            self.bits += 16;
        }
    }

    #[inline]
    fn peek16(&self) -> u32 {
        self.buf >> 16
    }

    #[inline]
    fn consume(&mut self, n: u32) {
        self.buf <<= n;
        self.bits -= n;
    }

    #[inline]
    fn read(&mut self, n: u32) -> u32 {
        self.ensure16();
        let v = ((self.buf as u64) >> (32 - n)) as u32;
        self.consume(n);
        v
    }

    #[inline]
    fn read_long(&mut self, n: u32) -> u32 {
        if n <= 16 { self.read(n) } else { (self.read(16) << (n - 16)) | self.read(n - 16) }
    }
}

impl<'a> BitReader<'a> {
    fn align_for_raw(&mut self) -> Result<()> {
        self.pos -= 2 * (self.bits / 16) as usize;
        if self.bits.is_multiple_of(16) {
            self.pos += 2;
        }
        self.buf = 0;
        self.bits = 0;
        if self.pos > self.data.len() {
            return Err(err("frame ends inside an uncompressed block header"));
        }
        Ok(())
    }

    fn raw(&mut self, n: usize) -> Result<&'a [u8]> {
        if self.bits != 0 {
            return Err(err("raw bytes requested in the middle of the bitstream"));
        }
        let out = self.pos.checked_add(n).and_then(|end| self.data.get(self.pos..end));
        let out = out.ok_or_else(|| {
            err(format!(
                "frame ends inside an uncompressed block (wanted {n} bytes at {:#x}, frame is {:#x})",
                self.pos,
                self.data.len()
            ))
        })?;
        self.pos += n;
        Ok(out)
    }

    fn raw_u32(&mut self) -> Result<u32> {
        let b = self.raw(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn overran(&self) -> bool {
        self.pos * 8 - self.bits as usize > self.data.len() * 8
    }
}

const TABLE_BITS: u32 = 12;
const TABLE_SIZE: usize = 1 << TABLE_BITS;
const LONG: u16 = u16::MAX;
const MAX_CODE_LEN: usize = 16;

struct Huffman {
    table: Box<[u16; TABLE_SIZE]>,
    counts: [u16; MAX_CODE_LEN + 1],
    sorted: Vec<u16>,
}

impl Huffman {
    fn new() -> Self {
        Self { table: Box::new([LONG; TABLE_SIZE]), counts: [0; MAX_CODE_LEN + 1], sorted: Vec::new() }
    }

    fn build(&mut self, lens: &[u8], allow_empty: bool, name: &str) -> Result<()> {
        let mut counts = [0u16; MAX_CODE_LEN + 1];
        for &len in lens {
            counts[len as usize] += 1;
        }
        counts[0] = 0;
        let mut left: i32 = 1;
        for &count in &counts[1..] {
            left = (left << 1) - count as i32;
            if left < 0 {
                return Err(err(format!("{name} tree is over-subscribed")));
            }
        }
        self.counts = counts;
        if left != 0 {
            if left == 1 << MAX_CODE_LEN && allow_empty {
                self.table.fill(LONG);
                return Ok(());
            }
            let what = if left == 1 << MAX_CODE_LEN { "empty" } else { "incomplete" };
            return Err(err(format!("{name} tree is {what}")));
        }

        let mut next_code = [0u32; MAX_CODE_LEN + 1];
        let mut offsets = [0usize; MAX_CODE_LEN + 2];
        let mut code = 0u32;
        for len in 1..=MAX_CODE_LEN {
            next_code[len] = code;
            code = (code + counts[len] as u32) << 1;
            offsets[len + 1] = offsets[len] + counts[len] as usize;
        }
        self.sorted.clear();
        self.sorted.resize(offsets[MAX_CODE_LEN + 1], 0);
        self.table.fill(LONG);
        for (sym, &len) in lens.iter().enumerate() {
            let len = len as usize;
            if len == 0 {
                continue;
            }
            self.sorted[offsets[len]] = sym as u16;
            offsets[len] += 1;
            let code = next_code[len] as usize;
            next_code[len] += 1;
            if len as u32 <= TABLE_BITS {
                let shift = TABLE_BITS as usize - len;
                let entry = ((sym as u16) << 4) | (len as u16 - 1);
                self.table[code << shift..(code + 1) << shift].fill(entry);
            }
        }
        Ok(())
    }

    #[inline]
    fn decode(&self, br: &mut BitReader) -> Result<u16> {
        br.ensure16();
        let peek = br.peek16();
        let entry = self.table[(peek >> (16 - TABLE_BITS)) as usize & (TABLE_SIZE - 1)];
        if entry != LONG {
            br.consume((entry & 15) as u32 + 1);
            return Ok(entry >> 4);
        }
        self.decode_long(br, peek)
    }

    #[cold]
    fn decode_long(&self, br: &mut BitReader, peek: u32) -> Result<u16> {
        let (mut code, mut first, mut index) = (0u32, 0u32, 0u32);
        for len in 1..=MAX_CODE_LEN as u32 {
            code |= (peek >> (16 - len)) & 1;
            let count = self.counts[len as usize] as u32;
            if code.wrapping_sub(first) < count {
                br.consume(len);
                return Ok(self.sorted[(index + code - first) as usize]);
            }
            index += count;
            first = (first + count) << 1;
            code <<= 1;
        }
        Err(err("invalid Huffman code (empty tree)"))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BlockType {
    Verbatim,
    Aligned,
    Uncompressed,
}

pub struct LzxDecoder {
    window: Vec<u8>,
    wpos: usize,
    main_symbols: usize,
    total: u64,
    reference: usize,
    r: [u32; 3],
    header_read: bool,
    e8_size: i32,
    block_type: BlockType,
    block_remaining: usize,
    main_lens: [u8; MAX_MAIN_SYMBOLS],
    length_lens: [u8; NUM_LENGTH_SYMBOLS],
    main: Huffman,
    length: Huffman,
    aligned: Huffman,
    pretree: Huffman,
    failed: bool,
}

impl LzxDecoder {
    pub fn new(window_bits: u32) -> Result<Self> {
        if !(MIN_WINDOW_BITS..=MAX_WINDOW_BITS).contains(&window_bits) {
            return Err(err(format!("unsupported window size 2^{window_bits}")));
        }
        Ok(Self {
            window: vec![0; 1 << window_bits],
            wpos: 0,
            main_symbols: NUM_CHARS + 8 * position_slots(window_bits),
            total: 0,
            reference: 0,
            r: [1, 1, 1],
            header_read: false,
            e8_size: 0,
            block_type: BlockType::Verbatim,
            block_remaining: 0,
            main_lens: [0; MAX_MAIN_SYMBOLS],
            length_lens: [0; NUM_LENGTH_SYMBOLS],
            main: Huffman::new(),
            length: Huffman::new(),
            aligned: Huffman::new(),
            pretree: Huffman::new(),
            failed: false,
        })
    }

    pub fn with_reference(window_bits: u32, reference: &[u8]) -> Result<Self> {
        let mut decoder = Self::new(window_bits)?;
        let size = decoder.window.len();
        if reference.len() > size {
            return Err(err(format!("reference window of {} bytes exceeds the {size}-byte window", reference.len())));
        }
        let at = size - reference.len();
        decoder.window[at..].copy_from_slice(reference);
        decoder.reference = reference.len();
        Ok(decoder)
    }

    pub fn reset(&mut self) {
        self.wpos = 0;
        self.total = 0;
        self.r = [1, 1, 1];
        self.header_read = false;
        self.e8_size = 0;
        self.block_remaining = 0;
        self.main_lens.fill(0);
        self.length_lens.fill(0);
        self.failed = false;
    }

    fn read_block_header(&mut self, br: &mut BitReader) -> Result<()> {
        let kind = br.read(3);
        let size = ((br.read(16) << 8) | br.read(8)) as usize;
        self.block_type = match kind {
            1 | 2 => {
                if kind == 2 {
                    let mut lens = [0u8; ALIGNED_SYMBOLS];
                    for len in &mut lens {
                        *len = br.read(3) as u8;
                    }
                    self.aligned.build(&lens, false, "aligned offset")?;
                }
                self.read_lengths(br, Lengths::Main, 0, NUM_CHARS)?;
                self.read_lengths(br, Lengths::Main, NUM_CHARS, self.main_symbols)?;
                self.main.build(&self.main_lens[..self.main_symbols], false, "main")?;
                self.read_lengths(br, Lengths::Length, 0, NUM_LENGTH_SYMBOLS)?;
                self.length.build(&self.length_lens, true, "length")?;
                if kind == 1 { BlockType::Verbatim } else { BlockType::Aligned }
            }
            3 => {
                br.align_for_raw()?;
                self.r = [br.raw_u32()?, br.raw_u32()?, br.raw_u32()?];
                BlockType::Uncompressed
            }
            other => return Err(err(format!("invalid block type {other}"))),
        };
        self.block_remaining = size;
        Ok(())
    }

    fn read_lengths(&mut self, br: &mut BitReader, which: Lengths, first: usize, last: usize) -> Result<()> {
        let mut pre = [0u8; PRETREE_SYMBOLS];
        for len in &mut pre {
            *len = br.read(4) as u8;
        }
        self.pretree.build(&pre, false, "pretree")?;
        let lens = match which {
            Lengths::Main => &mut self.main_lens[first..last],
            Lengths::Length => &mut self.length_lens[first..last],
        };
        let delta = |old: u8, code: u16| (17 + old - code as u8) % 17;
        let mut i = 0;
        while i < lens.len() {
            let code = self.pretree.decode(br)?;
            let (run, value) = match code {
                0..=16 => {
                    lens[i] = delta(lens[i], code);
                    i += 1;
                    continue;
                }
                17 => (4 + br.read(4) as usize, 0),
                18 => (20 + br.read(5) as usize, 0),
                _ => {
                    let run = 4 + br.read(1) as usize;
                    let code = self.pretree.decode(br)?;
                    if code > 16 {
                        return Err(err(format!("pretree run repeats code {code}")));
                    }
                    (run, delta(lens[i], code))
                }
            };
            let span = lens.get_mut(i..i + run).ok_or_else(|| err("pretree run overflows the length table"))?;
            span.fill(value);
            i += run;
        }
        Ok(())
    }
}

#[derive(Clone, Copy)]
enum Lengths {
    Main,
    Length,
}

impl LzxDecoder {
    pub fn decode_frame(&mut self, frame: &[u8], out_len: usize, out: &mut Vec<u8>) -> Result<()> {
        if self.failed {
            return Err(err("decoder is in a failed state"));
        }
        if out_len > self.window.len() {
            return Err(err(format!("frame of {out_len} bytes exceeds the {}-byte window", self.window.len())));
        }
        let result = self.decode_frame_inner(frame, out_len, out);
        self.failed = result.is_err();
        result
    }

    fn decode_frame_inner(&mut self, frame: &[u8], out_len: usize, out: &mut Vec<u8>) -> Result<()> {
        if out_len == 0 {
            return Ok(());
        }
        if self.reference > 0 && self.wpos + out_len > self.window.len() {
            return Err(err(format!(
                "frame of {out_len} bytes at window position {} would overwrite the reference window",
                self.wpos
            )));
        }
        let mut br = BitReader::new(frame);
        if !self.header_read {
            self.header_read = true;
            if br.read(1) == 1 {
                self.e8_size = br.read_long(32) as i32;
            }
        }

        let frame_start = self.wpos;
        let frame_total = self.total;
        let mut todo = out_len;
        while todo > 0 {
            if self.block_remaining == 0 {
                self.read_block_header(&mut br)?;
                continue;
            }
            let run = todo.min(self.block_remaining);
            match self.block_type {
                BlockType::Verbatim => self.decode_run::<false>(&mut br, run)?,
                BlockType::Aligned => self.decode_run::<true>(&mut br, run)?,
                BlockType::Uncompressed => self.copy_raw(&mut br, run)?,
            }
            self.total += run as u64;
            todo -= run;
            self.block_remaining -= run;
        }
        if br.overran() {
            return Err(err(format!("frame of {:#x} bytes is truncated", frame.len())));
        }

        let start = out.len();
        let first = out_len.min(self.window.len() - frame_start);
        out.extend_from_slice(&self.window[frame_start..frame_start + first]);
        out.extend_from_slice(&self.window[..out_len - first]);
        if self.e8_size != 0 {
            e8_translate(&mut out[start..], frame_total, self.e8_size);
        }
        Ok(())
    }

    fn copy_raw(&mut self, br: &mut BitReader, run: usize) -> Result<()> {
        let src = br.raw(run)?;
        let first = run.min(self.window.len() - self.wpos);
        self.window[self.wpos..self.wpos + first].copy_from_slice(&src[..first]);
        self.window[..run - first].copy_from_slice(&src[first..]);
        self.wpos = (self.wpos + run) & (self.window.len() - 1);
        Ok(())
    }

    fn decode_run<const ALIGNED: bool>(&mut self, br: &mut BitReader, run: usize) -> Result<()> {
        let Self { window, main, length, aligned, .. } = self;
        let size = window.len();
        let mask = size - 1;
        let mut wpos = self.wpos;
        let [mut r0, mut r1, mut r2] = self.r;
        let mut left = run;
        while left > 0 {
            let sym = main.decode(br)? as usize;
            if sym < NUM_CHARS {
                window[wpos] = sym as u8;
                wpos = (wpos + 1) & mask;
                left -= 1;
                continue;
            }
            let sym = sym - NUM_CHARS;
            let mut len = sym & 7;
            if len == NUM_PRIMARY_LENGTHS {
                len += length.decode(br)? as usize;
            }
            len += MIN_MATCH;
            let slot = sym >> 3;
            let offset = match slot {
                0 => r0,
                1 => {
                    std::mem::swap(&mut r0, &mut r1);
                    r0
                }
                2 => {
                    std::mem::swap(&mut r0, &mut r2);
                    r0
                }
                _ => {
                    let extra = FOOTER_BITS[slot] as u32;
                    let mut offset = POSITION_BASE[slot] - 2;
                    if ALIGNED && extra >= 3 {
                        offset += br.read_long(extra - 3) << 3;
                        offset += aligned.decode(br)? as u32;
                    } else {
                        offset += br.read_long(extra);
                    }
                    (r2, r1, r0) = (r1, r0, offset);
                    offset
                }
            };
            if len > left {
                return Err(err(format!("match of {len} bytes crosses the end of a block or frame")));
            }
            let produced = self.total + self.reference as u64 + (run - left) as u64;
            let offset = offset as usize;
            if offset == 0 || offset > size || offset as u64 > produced {
                return Err(err(format!("match offset {offset} out of range at stream position {produced}")));
            }
            copy_match(window, wpos, offset, len);
            wpos = (wpos + len) & mask;
            left -= len;
        }
        self.wpos = wpos;
        self.r = [r0, r1, r2];
        Ok(())
    }
}

#[inline]
fn copy_match(window: &mut [u8], dst: usize, offset: usize, len: usize) {
    let size = window.len();
    let mask = size - 1;
    let src = dst.wrapping_sub(offset) & mask;
    if dst + len <= size && src + len <= size {
        if offset >= len || src > dst {
            window.copy_within(src..src + len, dst);
        } else {
            for i in 0..len {
                window[dst + i] = window[src + i];
            }
        }
    } else {
        for i in 0..len {
            window[(dst + i) & mask] = window[(src + i) & mask];
        }
    }
}

fn e8_translate(buf: &mut [u8], pos: u64, size: i32) {
    if buf.len() <= E8_MIN_FRAME || pos >= E8_LIMIT {
        return;
    }
    let end = buf.len() - E8_MIN_FRAME;
    let mut i = 0;
    while i < end {
        if buf[i] != 0xE8 {
            i += 1;
            continue;
        }
        let cur = (pos + i as u64) as i32;
        let field = &mut buf[i + 1..i + 5];
        let abs = i32::from_le_bytes([field[0], field[1], field[2], field[3]]);
        if abs >= -cur && abs < size {
            let rel = if abs >= 0 { abs - cur } else { abs.wrapping_add(size) };
            field.copy_from_slice(&rel.to_le_bytes());
        }
        i += 5;
    }
}

#[cfg(test)]
mod tests;
