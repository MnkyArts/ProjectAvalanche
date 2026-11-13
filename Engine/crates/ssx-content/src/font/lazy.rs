use std::sync::Arc;

use skrifa::raw::TableProvider;
use skrifa::raw::types::GlyphId;
use skrifa::{FontRef, MetadataProvider};
use ssx_formats::gfx::shape::Shape;
use ssx_formats::gfx::types::Rect;

use super::{EM, Limits, Outlines, family_name};

#[derive(Clone, Debug)]
pub struct Bytes(pub Arc<[u8]>);

impl Bytes {
    pub fn get(&self) -> &[u8] {
        &self.0
    }
}

pub const MAX_COVERAGE_STEPS: usize = 2 * 0x11_0000;

#[derive(Debug)]
pub struct GlyphFont {
    bytes: Bytes,
    pub family: String,
    pub units_per_em: u16,
    pub glyphs: u32,
    pub outlines: &'static str,
    pub ascent: u16,
    pub descent: u16,
    pub leading: i16,
    pub coverage: Vec<(u32, u32)>,
    limits: Limits,
}

#[derive(Clone, Debug)]
pub struct Glyph {
    pub shape: Shape,
    pub advance: i16,
    pub bounds: Rect,
}

impl GlyphFont {
    pub fn open(bytes: Bytes, limits: &Limits) -> Result<GlyphFont, String> {
        let data = bytes.get();
        if data.len() > limits.max_file_bytes {
            return Err(format!("{} bytes (a font file is at most {})", data.len(), limits.max_file_bytes));
        }
        if data.get(..4) == Some(b"ttcf") {
            return Err("a font collection (ttcf): one font per file".into());
        }
        let font = FontRef::new(data).map_err(|e| format!("not a TrueType / OpenType font: {e}"))?;
        let upem = font.head().map_err(|e| format!("head: {e}"))?.units_per_em();
        if !(16..=16384).contains(&upem) {
            return Err(format!("head: {upem} units per EM (16 to 16 384)"));
        }
        let glyphs = u32::from(font.maxp().map_err(|e| format!("maxp: {e}"))?.num_glyphs());
        if glyphs == 0 || glyphs > limits.max_font_glyphs {
            return Err(format!("maxp: {glyphs} glyphs (1 to {})", limits.max_font_glyphs));
        }
        let hhea = font.hhea().map_err(|e| format!("hhea: {e}"))?;
        font.hmtx().map_err(|e| format!("hmtx: {e}"))?;
        let charmap = font.charmap();
        if !charmap.has_map() {
            return Err("cmap: no Unicode subtable".into());
        }
        let outlines = Outlines::new(&font, glyphs, limits)?.kind();
        let mut mapped: Vec<u32> = Vec::new();
        for (steps, (cp, g)) in charmap.mappings().enumerate() {
            if steps >= MAX_COVERAGE_STEPS {
                return Err(format!("cmap: more than {MAX_COVERAGE_STEPS} mappings (overlapping ranges)"));
            }
            if g.to_u32() != 0 && g.to_u32() < glyphs && char::from_u32(cp).is_some() {
                mapped.push(cp);
            }
        }
        mapped.sort_unstable();
        mapped.dedup();
        if mapped.is_empty() {
            return Err("cmap: no character is mapped".into());
        }
        let mut coverage: Vec<(u32, u32)> = Vec::new();
        for cp in mapped {
            match coverage.last_mut() {
                Some(r) if r.1 + 1 == cp => r.1 = cp,
                _ => coverage.push((cp, cp)),
            }
        }
        let scale = EM / f64::from(upem);
        let metric = |v: i16| (f64::from(v) * scale).round();
        // PORT(choice): as `convert`'s — the line metrics are hhea's ascender, descender and line gap (FreeType's and
        // most engines' default; OS/2's typo metrics would differ for some fonts); the layout keeps the run font's line
        // metrics anyway; the shipped and the packages' fonts are not the disc's.
        let ascent = metric(hhea.ascender().to_i16()).clamp(0.0, f64::from(u16::MAX)) as u16;
        let descent = (-metric(hhea.descender().to_i16())).clamp(0.0, f64::from(u16::MAX)) as u16;
        let leading = metric(hhea.line_gap().to_i16()).clamp(f64::from(i16::MIN), f64::from(i16::MAX)) as i16;
        let family = family_name(&font);
        Ok(GlyphFont {
            bytes,
            family,
            units_per_em: upem,
            glyphs,
            outlines,
            ascent,
            descent,
            leading,
            coverage,
            limits: *limits,
        })
    }

    pub fn bytes(&self) -> &[u8] {
        self.bytes.get()
    }

    pub fn font(&self) -> Result<FontRef<'_>, String> {
        FontRef::new(self.bytes.get()).map_err(|e| format!("not a TrueType / OpenType font: {e}"))
    }

    pub fn covers(&self, c: char) -> bool {
        let cp = u32::from(c);
        let at = self.coverage.partition_point(|r| r.1 < cp);
        self.coverage.get(at).is_some_and(|r| r.0 <= cp)
    }

    pub fn characters(&self) -> usize {
        self.coverage.iter().map(|r| (r.1 - r.0) as usize + 1).sum()
    }

    pub fn session(&self) -> Result<Session<'_>, String> {
        let font = self.font()?;
        let outlines = Outlines::new(&font, self.glyphs, &self.limits)?;
        Ok(Session { font, outlines, scale: EM / f64::from(self.units_per_em) })
    }
}

pub struct Session<'a> {
    font: FontRef<'a>,
    outlines: Outlines<'a>,
    scale: f64,
}

impl Session<'_> {
    pub fn glyph(&mut self, gid: u32) -> Result<Glyph, String> {
        let g = GlyphId::new(gid);
        let shape = self.outlines.shape(g, self.scale).map_err(|e| {
            let at = format!("glyph {gid}: ");
            if e.starts_with(&at) { e } else { at + &e }
        })?;
        let advance = self.font.hmtx().ok().and_then(|h| h.advance(g)).unwrap_or(0);
        let advance = (f64::from(advance) * self.scale).round().clamp(0.0, f64::from(i16::MAX)) as i16;
        let bounds = super::outline::bounds(&shape);
        Ok(Glyph { shape, advance, bounds })
    }

    pub fn last(&self) -> super::Cost {
        self.outlines.last
    }
}
