use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};

use ssx_formats::gfx::text::Font;

// PORT(choice): the store's cap: 64 MiB of converted glyphs (the CJK font's glyphs are 88-byte records, 105 per glyph
// at the median and 683 at most — about 9 KiB and 59 KiB; every one of its 65 535 glyphs would be 589 MiB; a chat line
// in each of eleven scripts converted 195 glyphs, 515 KiB); the number bounds a session's memory; the original has no
// fallback fonts.
pub const STORE_BYTES: usize = 64 << 20;
// PORT(choice): conversions per frame: 256 glyphs (the six files' slowest glyph 49 µs, p99 19 µs in release, example
// `font_shipped`: at most about 12 ms of a frame, 5 ms at p99); the original has no fallback fonts.
pub const FRAME_CONVERSIONS: u32 = 256;

#[derive(Debug)]
pub struct Budget {
    left: Mutex<u32>,
}

impl Default for Budget {
    fn default() -> Self {
        Budget { left: Mutex::new(FRAME_CONVERSIONS) }
    }
}

impl Budget {
    pub fn refill(&self) {
        *lock(&self.left) = FRAME_CONVERSIONS;
    }

    pub fn take(&self) -> bool {
        let mut left = lock(&self.left);
        if *left == 0 {
            return false;
        }
        *left -= 1;
        true
    }
}

#[derive(Clone, Debug)]
pub enum Converted {
    Glyph(Arc<Font>),
    Refused,
}

struct Entry {
    glyph: Converted,
    bytes: usize,
    used: u64,
}

#[derive(Default)]
struct Store {
    glyphs: HashMap<(u64, u32), Entry>,
    bytes: usize,
    clock: u64,
    converted: u64,
    refused: u64,
    evicted: u64,
}

static STORE: Mutex<Option<Store>> = Mutex::new(None);

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

pub fn glyph_bytes(f: &Font) -> usize {
    let records: usize = f.glyphs.iter().map(|s| s.records.len()).sum();
    records * std::mem::size_of::<ssx_formats::gfx::shape::ShapeRecord>() + std::mem::size_of::<Font>() + 64
}

pub fn get(serial: u64, gid: u32) -> Option<Converted> {
    let mut s = lock(&STORE);
    let s = s.get_or_insert_with(Store::default);
    s.clock += 1;
    let clock = s.clock;
    s.glyphs.get_mut(&(serial, gid)).map(|e| {
        e.used = clock;
        e.glyph.clone()
    })
}

pub fn put(serial: u64, gid: u32, glyph: Converted) {
    let bytes = match &glyph {
        Converted::Glyph(f) => glyph_bytes(f),
        Converted::Refused => 64,
    };
    let mut s = lock(&STORE);
    let s = s.get_or_insert_with(Store::default);
    s.clock += 1;
    match glyph {
        Converted::Glyph(_) => s.converted += 1,
        Converted::Refused => s.refused += 1,
    }
    let entry = Entry { glyph, bytes, used: s.clock };
    if let Some(old) = s.glyphs.insert((serial, gid), entry) {
        s.bytes -= old.bytes;
    }
    s.bytes += bytes;
    if s.bytes > STORE_BYTES {
        let mut by_use: Vec<(u64, (u64, u32))> = s.glyphs.iter().map(|(k, e)| (e.used, *k)).collect();
        by_use.sort_unstable();
        for (_, k) in by_use {
            if s.bytes <= STORE_BYTES / 4 * 3 {
                break;
            }
            if let Some(e) = s.glyphs.remove(&k) {
                s.bytes -= e.bytes;
                s.evicted += 1;
            }
        }
    }
}

pub fn stats() -> (usize, usize, u64, u64, u64) {
    let s = lock(&STORE);
    s.as_ref().map_or((0, 0, 0, 0, 0), |s| (s.glyphs.len(), s.bytes, s.converted, s.refused, s.evicted))
}
