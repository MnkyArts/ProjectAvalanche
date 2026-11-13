use flate2::{Decompress, FlushDecompress, Status};

// PORT(choice): 64 chunks, far above an image's IHDR, PLTE, tRNS, a few IDAT, metadata and IEND, a bound on hostile
// input; the original has no server browser.
const MAX_CHUNKS: usize = 64;

#[derive(Clone, Copy, Debug)]
struct Header {
    width: u32,
    height: u32,
    depth: u8,
    color: u8,
    interlaced: bool,
}

fn header(data: &[u8]) -> Result<Header, String> {
    let &[w0, w1, w2, w3, h0, h1, h2, h3, depth, color, compression, filter, interlace] = data else {
        return Err("the icon's IHDR is not 13 bytes".into());
    };
    let channels_ok = match color {
        0 => matches!(depth, 1 | 2 | 4 | 8 | 16),
        3 => matches!(depth, 1 | 2 | 4 | 8),
        2 | 4 | 6 => matches!(depth, 8 | 16),
        _ => false,
    };
    if !channels_ok || compression != 0 || filter != 0 || interlace > 1 {
        return Err(format!("the icon's IHDR is not a PNG header (colour {color}, depth {depth})"));
    }
    let (width, height) = (u32::from_be_bytes([w0, w1, w2, w3]), u32::from_be_bytes([h0, h1, h2, h3]));
    Ok(Header { width, height, depth, color, interlaced: interlace == 1 })
}

fn raw_size(h: Header) -> Option<u64> {
    let channels: u64 = match h.color {
        0 | 3 => 1,
        4 => 2,
        2 => 3,
        _ => 4,
    };
    let bits = channels * u64::from(h.depth);
    let rows = |w: u64, rows: u64| rows.checked_mul(w.checked_mul(bits)?.div_ceil(8).checked_add(1)?);
    let (w, hh) = (u64::from(h.width), u64::from(h.height));
    if !h.interlaced {
        return rows(w, hh);
    }
    let passes = [(0, 0, 8, 8), (4, 0, 8, 8), (0, 4, 4, 8), (2, 0, 4, 4), (0, 2, 2, 4), (1, 0, 2, 2), (0, 1, 1, 2)];
    let mut total = 0u64;
    for (x0, y0, dx, dy) in passes {
        let pw = if w > x0 { (w - x0).div_ceil(dx) } else { 0 };
        let ph = if hh > y0 { (hh - y0).div_ceil(dy) } else { 0 };
        if pw > 0 && ph > 0 {
            total = total.checked_add(rows(pw, ph)?)?;
        }
    }
    Some(total)
}

pub(crate) fn structure(bytes: &[u8]) -> Result<(), String> {
    let mut at = 8usize;
    let mut chunks = 0usize;
    let mut head: Option<Header> = None;
    let mut idat: Vec<u8> = Vec::new();
    let mut idat_run = 0u8;
    loop {
        chunks += 1;
        if chunks > MAX_CHUNKS {
            return Err(format!("the icon has more than {MAX_CHUNKS} chunks"));
        }
        let len_bytes = bytes.get(at..at + 4).ok_or("the icon ends inside a chunk's length")?;
        let len = u32::from_be_bytes([len_bytes[0], len_bytes[1], len_bytes[2], len_bytes[3]]);
        let len = usize::try_from(len).map_err(|_| "a chunk's length beyond this machine")?;
        let end = at.checked_add(12).and_then(|e| e.checked_add(len)).ok_or("a chunk's length overflows")?;
        let chunk = bytes.get(at + 4..end).ok_or("the icon ends inside a chunk")?;
        let (kind, rest) = chunk.split_at(4);
        let (data, crc) = rest.split_at(len);
        let mut sum = flate2::Crc::new();
        sum.update(kind);
        sum.update(data);
        if sum.sum().to_be_bytes() != crc {
            return Err(format!("the icon's chunk {} has a wrong CRC", String::from_utf8_lossy(kind).escape_debug()));
        }
        if !kind.iter().all(u8::is_ascii_alphabetic) {
            return Err("the icon has a chunk whose type is not four letters".into());
        }
        at = end;
        match kind {
            b"IHDR" if chunks == 1 => head = Some(header(data)?),
            b"IHDR" => return Err("the icon has a second IHDR".into()),
            _ if chunks == 1 => return Err("the icon's first chunk is not IHDR".into()),
            b"acTL" | b"fcTL" | b"fdAT" => return Err("the icon is animated: it must be one image".into()),
            b"IDAT" if idat_run == 2 => return Err("the icon's image data is split by other chunks".into()),
            b"IDAT" => {
                idat_run = 1;
                idat.extend_from_slice(data);
            }
            b"IEND" => {
                if !data.is_empty() || at != bytes.len() {
                    return Err("the icon has bytes after its end chunk".into());
                }
                break;
            }
            _ if idat_run == 1 => idat_run = 2,
            _ => {}
        }
        if kind[0].is_ascii_uppercase() && !matches!(kind, b"IHDR" | b"PLTE" | b"IDAT" | b"IEND") {
            return Err(format!("the icon has the unknown critical chunk {}", String::from_utf8_lossy(kind)));
        }
    }
    let head = head.ok_or("the icon has no IHDR")?;
    if (head.width, head.height) != (super::ICON_SIDE, super::ICON_SIDE) {
        return Err(format!("the icon is {} x {} pixels", head.width, head.height));
    }
    if idat.is_empty() {
        return Err("the icon has no image data".into());
    }
    let expected = raw_size(head).ok_or("the icon's image size overflows")?;
    let expected = usize::try_from(expected).map_err(|_| "the icon's image size is beyond this machine")?;
    let mut out = vec![0u8; expected + 1];
    let mut z = Decompress::new(true);
    let status = z.decompress(&idat, &mut out, FlushDecompress::Finish).map_err(|e| format!("the icon's data: {e}"))?;
    let (inflated, consumed) = (z.total_out(), z.total_in());
    if inflated > expected as u64 {
        return Err("the icon's image data inflates beyond its image (a decompression bomb)".into());
    }
    if status != Status::StreamEnd || inflated != expected as u64 {
        return Err(format!("the icon's image data inflates to {inflated} bytes, its header says {expected}"));
    }
    if consumed != idat.len() as u64 {
        return Err("the icon's image data has bytes after its compressed stream".into());
    }
    Ok(())
}
