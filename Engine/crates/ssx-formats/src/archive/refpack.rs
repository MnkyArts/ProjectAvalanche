use crate::reader::Reader;
use crate::{Error, Result};

pub const FLAG_SIGNATURE: u8 = 0x10;
pub const FLAG_LONG_SIZE: u8 = 0x80;
pub const FLAG_SMALL_WINDOW: u8 = 0x40;
pub const FLAG_UNK_0: u8 = 0x01;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Header {
    pub flags: u8,
    pub decompressed_size: u32,
}

pub fn header(data: &[u8]) -> Result<Header> {
    let mut r = Reader::be(data);
    let flags = r.u8()?;
    let magic = r.u8()?;
    if magic != 0xFB {
        return Err(Error::invalid(format!(
            "not a RefPack stream at offset 0x0: second byte is {magic:#04x}, expected 0xfb"
        )));
    }
    if flags & 0x3E != FLAG_SIGNATURE {
        return Err(Error::invalid(format!(
            "not a RefPack stream at offset 0x0: flags {flags:#04x} do not carry the 0x10 signature (the game checks flags & 0x3e == 0x10)"
        )));
    }
    let decompressed_size = read_size(&mut r, flags)?;
    Ok(Header { flags, decompressed_size })
}

fn read_size(r: &mut Reader<'_>, flags: u8) -> Result<u32> {
    if flags & FLAG_LONG_SIZE != 0 {
        r.u32()
    } else {
        let b = r.array::<3>()?;
        Ok(u32::from(b[0]) << 16 | u32::from(b[1]) << 8 | u32::from(b[2]))
    }
}

pub fn decompress(data: &[u8], expected: usize) -> Result<Vec<u8>> {
    let head = header(data)?;
    if head.decompressed_size as usize != expected {
        return Err(Error::Decompress(format!(
            "RefPack declares {} bytes, the member declares {expected}",
            head.decompressed_size
        )));
    }

    let mut r = Reader::be(data);
    r.seek(if head.flags & FLAG_LONG_SIZE != 0 { 6 } else { 5 })?;
    let mut out = Vec::new();

    loop {
        let first_offset = r.pos();
        let first = r.u8()?;
        if first & 0x80 == 0 {
            let second = r.u8()?;
            let literal = (first & 0x03) as usize;
            let distance = (((first & 0x60) as usize) << 3) + second as usize + 1;
            let length = (((first >> 2) & 0x07) as usize) + 3;
            copy_literals(&mut r, &mut out, literal, expected, first_offset)?;
            copy_match(&mut out, distance, length, expected, first_offset)?;
        } else if first & 0x40 == 0 {
            let second = r.u8()?;
            let third = r.u8()?;
            let literal = (second >> 6) as usize;
            let distance = (((second & 0x3F) as usize) << 8) + third as usize + 1;
            let length = (first & 0x3F) as usize + 4;
            copy_literals(&mut r, &mut out, literal, expected, first_offset)?;
            copy_match(&mut out, distance, length, expected, first_offset)?;
        } else if first & 0x20 == 0 {
            let second = r.u8()?;
            let third = r.u8()?;
            let fourth = r.u8()?;
            let literal = (first & 0x03) as usize;
            let distance = (((first & 0x10) as usize) << 12) + ((second as usize) << 8) + third as usize + 1;
            let length = (((first & 0x0C) as usize) << 6) + fourth as usize + 5;
            copy_literals(&mut r, &mut out, literal, expected, first_offset)?;
            copy_match(&mut out, distance, length, expected, first_offset)?;
        } else if first < 0xFC {
            let literal = ((first & 0x1F) as usize) * 4 + 4;
            copy_literals(&mut r, &mut out, literal, expected, first_offset)?;
        } else {
            let literal = (first & 0x03) as usize;
            copy_literals(&mut r, &mut out, literal, expected, first_offset)?;
            if r.remaining() != 0 {
                return Err(Error::Decompress(format!(
                    "{} bytes follow the RefPack stop command at offset {:#x}",
                    r.remaining(),
                    first_offset
                )));
            }
            break;
        }
    }

    if out.len() != expected {
        return Err(Error::Decompress(format!("RefPack produced {} bytes, expected {expected}", out.len())));
    }
    Ok(out)
}

fn copy_literals(
    r: &mut Reader<'_>,
    out: &mut Vec<u8>,
    count: usize,
    expected: usize,
    command_offset: usize,
) -> Result<()> {
    if out.len() + count > expected {
        return Err(Error::Decompress(format!(
            "literal run at offset {command_offset:#x} writes past the declared {expected} bytes"
        )));
    }
    let literals = r.bytes(count)?;
    out.extend_from_slice(literals);
    Ok(())
}

fn copy_match(out: &mut Vec<u8>, distance: usize, length: usize, expected: usize, command_offset: usize) -> Result<()> {
    if distance == 0 || distance > out.len() {
        return Err(Error::Decompress(format!(
            "match at offset {command_offset:#x} points {distance} bytes back, only {} decoded",
            out.len()
        )));
    }
    if out.len() + length > expected {
        return Err(Error::Decompress(format!(
            "match at offset {command_offset:#x} writes past the declared {expected} bytes"
        )));
    }
    let start = out.len() - distance;
    for i in 0..length {
        let byte = out[start + i];
        out.push(byte);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stream(size: u32, body: &[u8]) -> Vec<u8> {
        stream_flags(0x10, size, body)
    }

    fn stream_flags(flags: u8, size: u32, body: &[u8]) -> Vec<u8> {
        let mut v = vec![flags, 0xFB, (size >> 16) as u8, (size >> 8) as u8, size as u8];
        v.extend_from_slice(body);
        v
    }

    fn stream_long(flags: u8, size: u32, body: &[u8]) -> Vec<u8> {
        let mut v = vec![flags, 0xFB];
        v.extend_from_slice(&size.to_be_bytes());
        v.extend_from_slice(body);
        v
    }

    #[test]
    fn literal_only_stream() {
        let data = stream(3, &[0xFF, b'A', b'B', b'C']);
        assert_eq!(decompress(&data, 3).unwrap(), b"ABC");
        assert_eq!(header(&data).unwrap(), Header { flags: 0x10, decompressed_size: 3 });
    }

    #[test]
    fn literal_run_command() {
        let data = stream(4, &[0xE0, b'a', b'b', b'c', b'd', 0xFC]);
        assert_eq!(decompress(&data, 4).unwrap(), b"abcd");
    }

    #[test]
    fn two_byte_match_command() {
        let data = stream(8, &[0xE0, b'A', b'B', b'C', b'D', 0x04, 0x03, 0xFC]);
        assert_eq!(decompress(&data, 8).unwrap(), b"ABCDABCD");
    }

    #[test]
    fn three_byte_match_command() {
        let data = stream(9, &[0xE0, b'A', b'B', b'C', b'D', 0x80, 0x40, 0x00, b'E', 0xFC]);
        assert_eq!(decompress(&data, 9).unwrap(), b"ABCDEEEEE");
    }

    #[test]
    fn four_byte_match_command() {
        let data = stream(10, &[0xE0, b'A', b'B', b'A', b'B', 0xC0, 0x00, 0x01, 0x01, 0xFC]);
        assert_eq!(decompress(&data, 10).unwrap(), b"ABABABABAB");
    }

    #[test]
    fn long_size_field() {
        let mut data = stream_long(0x90, 3, &[0xFF, b'x', b'y', b'z']);
        assert_eq!(decompress(&data, 3).unwrap(), b"xyz");
        assert_eq!(header(&data).unwrap(), Header { flags: 0x90, decompressed_size: 3 });
        data[0] = 0x91;
        assert_eq!(decompress(&data, 3).unwrap(), b"xyz");
        assert_eq!(header(&data).unwrap(), Header { flags: 0x91, decompressed_size: 3 });
    }

    #[test]
    fn bit_0_is_not_a_compressed_size_field() {
        let data = stream_flags(0x11, 3, &[0xFF, b'A', b'B', b'C']);
        assert_eq!(decompress(&data, 3).unwrap(), b"ABC");
    }

    #[test]
    fn small_window_flag_is_accepted() {
        let data = stream_flags(0x50, 3, &[0xFF, b'A', b'B', b'C']);
        assert_eq!(decompress(&data, 3).unwrap(), b"ABC");
    }

    #[test]
    fn rejects_missing_signature_bits() {
        for flags in [0x00, 0x20, 0x40] {
            let data = stream_flags(flags, 0, &[0xFC]);
            assert!(header(&data).is_err(), "flags {flags:#04x} must be rejected");
        }
    }

    #[test]
    fn rejects_bad_signature() {
        assert!(decompress(&[0x10, 0xFA, 0, 0, 0], 0).is_err());
        assert!(decompress(&[], 0).is_err());
    }

    #[test]
    fn rejects_size_mismatch() {
        let data = stream(4, &[0xFC]);
        assert!(decompress(&data, 3).is_err());
    }

    #[test]
    fn rejects_missing_stop() {
        let data = stream(4, &[0xE3, b'a', b'b', b'c', b'd']);
        assert!(decompress(&data, 4).is_err());
    }

    #[test]
    fn rejects_trailing_bytes_after_stop() {
        let data = stream(4, &[0xE0, b'a', b'b', b'c', b'd', 0xFC, 0x00]);
        assert!(decompress(&data, 4).is_err());
    }

    #[test]
    fn rejects_truncated_commands() {
        let data = stream(8, &[0xE4, b'a', b'b']);
        assert!(decompress(&data, 8).is_err());
        let data = stream(4, &[0x04]);
        assert!(decompress(&data, 4).is_err());
        assert!(decompress(&[0x10, 0xFB, 0x00], 0).is_err());
    }

    #[test]
    fn rejects_match_before_output() {
        let data = stream(4, &[0x04, 0x00, 0xFC]);
        assert!(decompress(&data, 4).is_err());
    }

    #[test]
    fn rejects_output_overrun() {
        let data = stream(2, &[0xFF, b'A', b'B', b'C']);
        assert!(decompress(&data, 2).is_err());
        let data = stream(5, &[0xE0, b'A', b'B', b'C', b'D', 0x04, 0x03, 0xFC]);
        assert!(decompress(&data, 5).is_err());
    }

    #[test]
    fn rejects_output_underrun() {
        let data = stream(5, &[0xE0, b'A', b'B', b'C', b'D', 0xFC]);
        assert!(decompress(&data, 5).is_err());
    }
}
