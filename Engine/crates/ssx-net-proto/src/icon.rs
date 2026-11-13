use std::io::Cursor;

use sha2::{Digest, Sha256};

pub const ICON_SIDE: u32 = 96;
// PORT(choice): 64 KiB, the 36 864 bytes of 96 x 96 RGBA stored without compression plus the PNG's chunks and room for
// palettes and metadata; FiveM's own cap of its icon was not read here (no network); tuning: the icons operators make;
// the original has no server browser.
pub const MAX_ICON_BYTES: usize = 64 * 1024;
// PORT(choice): 1 MiB, far above what a 96-pixel image of 16-bit RGBA needs (a row of 769 bytes and the 32 KiB window)
// and far below what hurts a client, a bound on hostile input, measured by the tests' largest icon; the original has no
// server browser.
const DECODER_BYTES: usize = 1024 * 1024;
const SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Icon {
    pub rgba: Vec<u8>,
    pub hash: [u8; 32],
}

pub fn hash_hex(bytes: &[u8]) -> String {
    crate::keys::to_hex(&Sha256::digest(bytes).into())
}

pub fn dimensions(bytes: &[u8]) -> Result<(u32, u32), String> {
    let head = bytes.get(..24).ok_or("not a PNG: shorter than its header")?;
    if head[..8] != SIGNATURE {
        return Err("not a PNG: no PNG signature".into());
    }
    if head[8..12] != [0, 0, 0, 13] || &head[12..16] != b"IHDR" {
        return Err("not a PNG: the first chunk is not a 13-byte IHDR".into());
    }
    let word = |at: usize| u32::from_be_bytes([head[at], head[at + 1], head[at + 2], head[at + 3]]);
    Ok((word(16), word(20)))
}

pub fn read_icon(bytes: &[u8]) -> Result<Icon, String> {
    if bytes.len() > MAX_ICON_BYTES {
        return Err(format!("the icon is {} bytes, at most {MAX_ICON_BYTES}", bytes.len()));
    }
    let (w, h) = dimensions(bytes)?;
    if (w, h) != (ICON_SIDE, ICON_SIDE) {
        return Err(format!("the icon is {w} x {h} pixels, it must be {ICON_SIDE} x {ICON_SIDE}"));
    }
    structure(bytes)?;
    let mut decoder = png::Decoder::new_with_limits(Cursor::new(bytes), png::Limits { bytes: DECODER_BYTES });
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    decoder.set_ignore_text_chunk(true);
    decoder.set_ignore_iccp_chunk(true);
    let mut reader = decoder.read_info().map_err(|e| format!("the icon does not decode: {e}"))?;
    let info = reader.info();
    if (info.width, info.height) != (ICON_SIDE, ICON_SIDE) {
        return Err("the icon's header changed between two reads".into());
    }
    if info.animation_control.is_some() {
        return Err("the icon is animated: it must be one image".into());
    }
    let (color, depth) = reader.output_color_type();
    if depth != png::BitDepth::Eight {
        return Err(format!("the icon decodes to {depth:?}-bit samples, not 8"));
    }
    let channels = match color {
        png::ColorType::Grayscale => 1,
        png::ColorType::GrayscaleAlpha => 2,
        png::ColorType::Rgb => 3,
        png::ColorType::Rgba => 4,
        png::ColorType::Indexed => return Err("the icon's palette was not expanded".into()),
    };
    let pixels = (ICON_SIDE * ICON_SIDE) as usize;
    let size = reader.output_buffer_size().ok_or("the icon's buffer size overflows")?;
    if size != pixels * channels {
        return Err(format!("the icon decodes to {size} bytes, not {}", pixels * channels));
    }
    let mut buf = vec![0u8; size];
    reader.next_frame(&mut buf).map_err(|e| format!("the icon does not decode: {e}"))?;
    reader.finish().map_err(|e| format!("the icon's end does not read: {e}"))?;
    let mut rgba = Vec::with_capacity(pixels * 4);
    for px in buf.chunks_exact(channels) {
        match *px {
            [g] => rgba.extend_from_slice(&[g, g, g, 255]),
            [g, a] => rgba.extend_from_slice(&[g, g, g, a]),
            [r, g, b] => rgba.extend_from_slice(&[r, g, b, 255]),
            [r, g, b, a] => rgba.extend_from_slice(&[r, g, b, a]),
            _ => return Err("the icon's pixels do not split into samples".into()),
        }
    }
    Ok(Icon { rgba, hash: Sha256::digest(bytes).into() })
}

mod structure;
use structure::structure;

#[cfg(test)]
mod tests;
