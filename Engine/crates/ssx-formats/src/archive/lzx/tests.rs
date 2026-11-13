use super::*;

#[derive(Default)]
struct BitWriter {
    out: Vec<u8>,
    acc: u32,
    bits: u32,
}

impl BitWriter {
    fn put(&mut self, value: u32, count: u32) {
        for i in (0..count).rev() {
            self.acc = (self.acc << 1) | ((value >> i) & 1);
            self.bits += 1;
            if self.bits == 16 {
                self.out.extend_from_slice(&(self.acc as u16).to_le_bytes());
                (self.acc, self.bits) = (0, 0);
            }
        }
    }

    fn raw(&mut self, bytes: &[u8]) {
        assert_eq!(self.bits, 0, "raw bytes must start on a word boundary");
        self.out.extend_from_slice(bytes);
    }

    fn finish(mut self) -> Vec<u8> {
        if self.bits > 0 {
            self.put(0, 16 - self.bits);
        }
        self.out
    }

    fn block_header(&mut self, kind: u32, size: usize) {
        self.put(kind, 3);
        self.put(size as u32 >> 8, 16);
        self.put(size as u32 & 0xFF, 8);
    }

    fn uncompressed(&mut self, size: usize, r: [u32; 3], now: &[u8]) {
        self.block_header(3, size);
        let pad = 16 - self.bits;
        self.put(0, pad);
        for v in r {
            self.raw(&v.to_le_bytes());
        }
        self.raw(now);
    }

    fn lengths(&mut self, lens: &[u8]) {
        for sym in 0..PRETREE_SYMBOLS {
            self.put(if sym < 16 { 4 } else { 0 }, 4);
        }
        for &len in lens {
            let code = (17 - len as u32) % 17;
            assert!(code < 16);
            self.put(code, 4);
        }
    }

    fn verbatim(&mut self, size: usize) {
        self.block_header(1, size);
        let mut main = [0u8; NUM_CHARS + 8 * 34];
        main[..512].fill(9);
        self.lengths(&main[..NUM_CHARS]);
        self.lengths(&main[NUM_CHARS..]);
        self.lengths(&[0; NUM_LENGTH_SYMBOLS]);
    }

    fn literal(&mut self, byte: u8) {
        self.put(byte as u32, 9);
    }

    fn matched(&mut self, slot: u32, len: u32, footer: u32) {
        self.put(256 + slot * 8 + (len - 2), 9);
        self.put(footer, FOOTER_BITS[slot as usize] as u32);
    }
}

fn decode(frames: &[(&[u8], usize)]) -> Result<Vec<u8>> {
    let mut d = LzxDecoder::new(17)?;
    let mut out = Vec::new();
    for &(frame, len) in frames {
        d.decode_frame(frame, len, &mut out)?;
    }
    Ok(out)
}

#[test]
fn a_reference_window_answers_before_the_streams_first_byte() {
    let mut w = BitWriter::default();
    w.put(0, 1);
    w.verbatim(4);
    w.matched(5, 4, 0);
    let frame = w.finish();
    let mut d = LzxDecoder::with_reference(17, b"ABCD").unwrap();
    let mut out = Vec::new();
    d.decode_frame(&frame, 4, &mut out).unwrap();
    assert_eq!(out, b"ABCD");
}

#[test]
fn tables() {
    assert_eq!(&FOOTER_BITS[..8], &[0, 0, 0, 0, 1, 1, 2, 2]);
    assert_eq!((FOOTER_BITS[34], FOOTER_BITS[35], FOOTER_BITS[36], FOOTER_BITS[49]), (16, 16, 17, 17));
    assert_eq!(&POSITION_BASE[..10], &[0, 1, 2, 3, 4, 6, 8, 12, 16, 24]);
    assert_eq!((POSITION_BASE[34], POSITION_BASE[49]), (131072, 1966080));
}

#[test]
fn odd_uncompressed_block_then_next_header_without_padding() {
    let mut w = BitWriter::default();
    w.put(0, 1);
    w.uncompressed(5, [1, 1, 1], b"hello");
    assert_eq!(w.out.len(), 21);
    w.uncompressed(3, [1, 1, 1], b"abc");
    let frame = w.finish();
    assert_eq!(frame.len(), 21 + 4 + 12 + 3);
    assert_eq!(decode(&[(&frame, 8)]).unwrap(), b"helloabc");
}

#[test]
fn uncompressed_block_spanning_frames() {
    let mut w = BitWriter::default();
    w.put(0, 1);
    w.uncompressed(9, [1, 1, 1], b"abcd");
    let f0 = w.finish();
    let mut w = BitWriter::default();
    w.raw(b"efghi");
    w.uncompressed(2, [1, 1, 1], b"xy");
    let f1 = w.finish();
    assert_eq!(decode(&[(&f0, 4), (&f1, 7)]).unwrap(), b"abcdefghixy");
}

fn verbatim_frames() -> (Vec<u8>, Vec<u8>) {
    let mut w = BitWriter::default();
    w.put(0, 1);
    w.uncompressed(5, [5, 2, 1], b"hello");
    w.verbatim(14);
    w.matched(0, 5, 0);
    w.literal(b'!');
    w.matched(3, 2, 0);
    let f0 = w.finish();
    let mut w = BitWriter::default();
    w.matched(6, 4, 3);
    w.matched(2, 2, 0);
    (f0, w.finish())
}

#[test]
fn verbatim_block_with_repeated_offsets() {
    let (f0, f1) = verbatim_frames();
    assert_eq!(decode(&[(&f0, 13), (&f1, 6)]).unwrap(), b"hellohello!!!ohel!o");
}

#[test]
fn e8_translation() {
    let mut data = [0u8; 30];
    data[3] = 0xE8;
    data[4..8].copy_from_slice(&103i32.to_le_bytes());
    data[8] = 0xE8;
    data[9..13].copy_from_slice(&(-2i32).to_le_bytes());
    data[13] = 0xE8;
    data[14..18].copy_from_slice(&5000i32.to_le_bytes());
    data[25] = 0xE8;
    data[26..30].copy_from_slice(&7i32.to_le_bytes());
    let mut w = BitWriter::default();
    w.put(1, 1);
    w.put(0, 16);
    w.put(1000, 16);
    w.uncompressed(data.len(), [1, 1, 1], &data);
    let out = decode(&[(&w.finish(), data.len())]).unwrap();
    let mut expected = data;
    expected[4..8].copy_from_slice(&100i32.to_le_bytes());
    expected[9..13].copy_from_slice(&998i32.to_le_bytes());
    assert_eq!(out, expected);
}

fn error_of(frames: &[(&[u8], usize)]) -> String {
    decode(frames).unwrap_err().to_string()
}

#[test]
fn rejects_bad_block_types() {
    for kind in [0, 4, 5, 6, 7] {
        let mut w = BitWriter::default();
        w.put(0, 1);
        w.block_header(kind, 100);
        let frame = w.finish();
        assert!(error_of(&[(&frame, 100)]).contains(&format!("invalid block type {kind}")));
    }
}

#[test]
fn rejects_truncated_input() {
    let mut w = BitWriter::default();
    w.put(0, 1);
    w.uncompressed(8, [1, 1, 1], b"abcd");
    let frame = w.finish();
    assert!(error_of(&[(&frame, 8)]).contains("frame ends inside an uncompressed block"));
    assert!(error_of(&[(&frame[..10], 4)]).contains("frame ends inside an uncompressed block"));
    assert!(error_of(&[(&[], 4)]).contains("invalid block type 0"));

    let (f0, _) = verbatim_frames();
    for cut in 0..f0.len() {
        assert!(decode(&[(&f0[..cut], 13)]).is_err(), "prefix of {cut} bytes decoded");
    }
}

#[test]
fn rejects_bad_matches() {
    let mut w = BitWriter::default();
    w.put(0, 1);
    w.verbatim(4);
    w.matched(3, 2, 0);
    assert!(error_of(&[(&w.finish(), 4)]).contains("match offset 1 out of range"));

    let (f0, _) = verbatim_frames();
    assert!(error_of(&[(&f0, 12)]).contains("crosses the end"));
}

#[test]
fn rejects_bad_parameters_and_stays_failed() {
    assert!(LzxDecoder::new(14).is_err());
    assert!(LzxDecoder::new(22).is_err());
    let mut d = LzxDecoder::new(15).unwrap();
    let mut out = Vec::new();
    assert!(d.decode_frame(&[0; 4], (1 << 15) + 1, &mut out).is_err());

    let mut d = LzxDecoder::new(17).unwrap();
    let (f0, f1) = verbatim_frames();
    assert!(d.decode_frame(&f0, 12, &mut out).is_err());
    assert!(d.decode_frame(&f1, 6, &mut out).unwrap_err().to_string().contains("failed state"));
    d.reset();
    out.clear();
    d.decode_frame(&f0, 13, &mut out).unwrap();
    d.decode_frame(&f1, 6, &mut out).unwrap();
    assert_eq!(out, b"hellohello!!!ohel!o");
}

#[test]
fn garbage_never_panics() {
    let mut seed = 0x1234_5678_u32;
    let mut next = || {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        seed
    };
    for _ in 0..2000 {
        let len = (next() % 600) as usize;
        let mut frame: Vec<u8> = (0..len).map(|_| next() as u8).collect();
        if let Some(b) = frame.get_mut(1) {
            *b = (*b & 0x1F) | (((next() % 3 + 1) as u8) << 5);
        }
        let mut d = LzxDecoder::new(15 + next() % 7).unwrap();
        let mut out = Vec::new();
        for _ in 0..3 {
            let _ = d.decode_frame(&frame, 1 + (next() % 32768) as usize, &mut out);
        }
    }
}
