pub mod lazy;
mod outline;
pub mod shipped;
pub mod write;

use std::collections::BTreeMap;

use skrifa::raw::TableProvider;
use skrifa::raw::types::GlyphId;
use skrifa::{FontRef, MetadataProvider};
use ssx_formats::gfx::shape::Shape;
use ssx_formats::gfx::text::{Font, FontLayout};
use ssx_formats::gfx::types::Rect;

pub use outline::{Cost, Outlines};

pub const EM: f64 = 20480.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    pub max_file_bytes: usize,
    pub max_font_glyphs: u32,
    pub max_codes: usize,
    pub max_glyph_points: u32,
    pub max_records: usize,
    pub max_component_visits: u32,
    pub max_subr_calls: u32,
    pub max_charstring_bytes: u64,
    pub max_kerning_pairs: usize,
    pub max_kerning_work: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            max_file_bytes: 32 << 20,
            max_font_glyphs: 65_535,
            max_codes: 16_384,
            max_glyph_points: 8_192,
            max_records: 2_000_000,
            max_component_visits: 4_096,
            max_subr_calls: 4_096,
            max_charstring_bytes: 4 << 20,
            max_kerning_pairs: 1 << 18,
            max_kerning_work: 1 << 24,
        }
    }
}

impl Limits {
    pub fn shipped() -> Limits {
        Limits {
            max_file_bytes: 16_467_736,
            max_font_glyphs: 65_535,
            max_codes: Limits::default().max_codes,
            max_glyph_points: 778,
            max_records: Limits::default().max_records,
            max_component_visits: 9,
            max_subr_calls: 40,
            max_charstring_bytes: 2_017,
            max_kerning_pairs: Limits::default().max_kerning_pairs,
            max_kerning_work: Limits::default().max_kerning_work,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FontInfo {
    pub family: String,
    pub units_per_em: u16,
    pub glyphs: u32,
    pub codes: usize,
    pub mapped_glyphs: usize,
    pub outlines: &'static str,
    pub records: usize,
    pub kerning_pairs: usize,
    pub kerning_source: &'static str,
    pub kerning_dropped: Option<String>,
    pub clamped_advances: usize,
}

#[derive(Clone, Debug)]
pub struct Converted {
    pub font: Font,
    pub info: FontInfo,
    pub bytes: std::sync::Arc<[u8]>,
}

pub fn convert(bytes: &[u8], name: &str, limits: &Limits) -> Result<Converted, String> {
    if bytes.len() > limits.max_file_bytes {
        return Err(format!("{} bytes (a font file is at most {})", bytes.len(), limits.max_file_bytes));
    }
    if bytes.get(..4) == Some(b"ttcf") {
        return Err("a font collection (ttcf): one font per file".into());
    }
    let font = FontRef::new(bytes).map_err(|e| format!("not a TrueType / OpenType font: {e}"))?;
    let head = font.head().map_err(|e| format!("head: {e}"))?;
    let upem = head.units_per_em();
    if !(16..=16384).contains(&upem) {
        return Err(format!("head: {upem} units per EM (16 to 16 384)"));
    }
    let maxp = font.maxp().map_err(|e| format!("maxp: {e}"))?;
    let glyphs = u32::from(maxp.num_glyphs());
    if glyphs == 0 || glyphs > limits.max_font_glyphs {
        return Err(format!("maxp: {glyphs} glyphs (1 to {})", limits.max_font_glyphs));
    }
    let hhea = font.hhea().map_err(|e| format!("hhea: {e}"))?;
    let hmtx = font.hmtx().map_err(|e| format!("hmtx: {e}"))?;
    let charmap = font.charmap();
    if !charmap.has_map() {
        return Err("cmap: no Unicode subtable".into());
    }
    let mut codes: Vec<(u16, GlyphId)> = Vec::new();
    for cp in 0u32..=0xFFFF {
        if (0xD800..=0xDFFF).contains(&cp) {
            continue;
        }
        if let Some(g) = charmap.map(cp).filter(|g| g.to_u32() != 0 && g.to_u32() < glyphs) {
            if codes.len() == limits.max_codes {
                return Err(format!(
                    "cmap: more than {} characters mapped (subset the font to the characters it is for)",
                    limits.max_codes
                ));
            }
            codes.push((cp as u16, g));
        }
    }
    if codes.is_empty() {
        return Err("cmap: no character of the BMP is mapped".into());
    }
    let mut outlines = Outlines::new(&font, glyphs, limits)?;
    let scale = EM / f64::from(upem);
    let mut info = FontInfo {
        family: family_name(&font),
        units_per_em: upem,
        glyphs,
        codes: codes.len(),
        outlines: outlines.kind(),
        ..Default::default()
    };
    let mut shapes: BTreeMap<GlyphId, Shape> = BTreeMap::new();
    let (mut font_glyphs, mut advances, mut bounds) = (Vec::new(), Vec::new(), Vec::new());
    let mut records = 0usize;
    for &(_, g) in &codes {
        if let std::collections::btree_map::Entry::Vacant(slot) = shapes.entry(g) {
            let s = outlines.shape(g, scale).map_err(|e| {
                let at = format!("glyph {}: ", g.to_u32());
                if e.starts_with(&at) { e } else { at + &e }
            })?;
            slot.insert(s);
        }
        let shape = shapes[&g].clone();
        records += shape.records.len();
        if records > limits.max_records {
            return Err(format!("more than {} outline records in all (subset the font)", limits.max_records));
        }
        let adv = (f64::from(hmtx.advance(g).unwrap_or(0)) * scale).round();
        if adv > f64::from(i16::MAX) {
            info.clamped_advances += 1;
        }
        advances.push(adv.clamp(0.0, f64::from(i16::MAX)) as i16);
        bounds.push(outline::bounds(&shape));
        font_glyphs.push(shape);
    }
    info.mapped_glyphs = shapes.len();
    info.records = records;
    let metric = |v: i16| (f64::from(v) * scale).round();
    // PORT(choice): the line's metrics are hhea's ascender, descender and line gap (FreeType's and most engines'
    // default; OS/2's typo metrics would differ for some fonts); the disc's fonts came through Adobe's exporter, whose
    // mapping from a font file is not known; a package font is the author's, not the disc's.
    let ascent = metric(hhea.ascender().to_i16()).clamp(0.0, f64::from(u16::MAX)) as u16;
    let descent = (-metric(hhea.descender().to_i16())).clamp(0.0, f64::from(u16::MAX)) as u16;
    let leading = metric(hhea.line_gap().to_i16()).clamp(f64::from(i16::MIN), f64::from(i16::MAX)) as i16;
    let kerning = match kerning(&font, &codes, scale, limits) {
        Ok((pairs, source)) => {
            info.kerning_source = source;
            pairs
        }
        Err(why) => {
            info.kerning_dropped = Some(why);
            Vec::new()
        }
    };
    info.kerning_pairs = kerning.len();
    let font = Font {
        version: 3,
        id: 0,
        flags: Font::HAS_LAYOUT | Font::WIDE_CODES,
        language: 0,
        name: name.to_string(),
        glyphs: font_glyphs,
        codes: codes.iter().map(|&(c, _)| c).collect(),
        layout: Some(FontLayout { ascent, descent, leading, advances, bounds, kerning }),
        unk_trailing_layout: None,
    };
    Ok(Converted { font, info, bytes: std::sync::Arc::from(bytes) })
}

fn family_name(font: &FontRef) -> String {
    let id = skrifa::string::StringId::FAMILY_NAME;
    let name: String =
        font.localized_strings(id).english_or_first().map(|s| s.chars().take(64).collect()).unwrap_or_default();
    name.chars().filter(|c| !c.is_control()).collect()
}

type CodePairs = Vec<(u16, u16, i16)>;
type GlyphPairs = BTreeMap<(GlyphId, GlyphId), i32>;

fn kerning(
    font: &FontRef,
    codes: &[(u16, GlyphId)],
    scale: f64,
    limits: &Limits,
) -> Result<(CodePairs, &'static str), String> {
    let mut by_glyph: BTreeMap<GlyphId, Vec<u16>> = BTreeMap::new();
    for &(c, g) in codes {
        by_glyph.entry(g).or_default().push(c);
    }
    let (pairs, source) = match gpos_kerning(font, &by_glyph, limits)? {
        Some(p) => (p, "GPOS"),
        None => (kern_table(font, &by_glyph)?, if font.kern().is_ok() { "kern" } else { "none" }),
    };
    let mut out = Vec::new();
    for ((l, r), v) in pairs {
        let v = (f64::from(v) * scale).round().clamp(f64::from(i16::MIN), f64::from(i16::MAX)) as i16;
        if v == 0 {
            continue;
        }
        let (Some(ls), Some(rs)) = (by_glyph.get(&l), by_glyph.get(&r)) else { continue };
        for &a in ls {
            for &b in rs {
                if out.len() == limits.max_kerning_pairs {
                    return Err(format!(
                        "more than {} kerning pairs between mapped characters",
                        limits.max_kerning_pairs
                    ));
                }
                out.push((a, b, v));
            }
        }
    }
    out.sort_unstable();
    Ok((out, source))
}

fn kern_table(
    font: &FontRef,
    by_glyph: &BTreeMap<GlyphId, Vec<u16>>,
) -> Result<BTreeMap<(GlyphId, GlyphId), i32>, String> {
    let mut out = BTreeMap::new();
    let Ok(kern) = font.kern() else { return Ok(out) };
    for sub in kern.subtables() {
        let sub = sub.map_err(|e| format!("kern: {e}"))?;
        if !sub.is_horizontal() || sub.is_cross_stream() {
            continue;
        }
        let Ok(skrifa::raw::tables::kern::SubtableKind::Format0(f0)) = sub.kind() else { continue };
        for p in f0.pairs() {
            let (l, r) = (GlyphId::from(p.left()), GlyphId::from(p.right()));
            if by_glyph.contains_key(&l) && by_glyph.contains_key(&r) {
                out.entry((l, r)).or_insert(i32::from(p.value()));
            }
        }
    }
    Ok(out)
}

fn gpos_kerning(
    font: &FontRef,
    by_glyph: &BTreeMap<GlyphId, Vec<u16>>,
    limits: &Limits,
) -> Result<Option<GlyphPairs>, String> {
    use skrifa::raw::tables::gpos::{PairPos, PositionSubtables};
    use std::collections::HashSet;
    let Ok(gpos) = font.gpos() else { return Ok(None) };
    let e = |what: &str, err: skrifa::raw::ReadError| format!("GPOS {what}: {err}");
    let features = gpos.feature_list().map_err(|err| e("feature list", err))?;
    let kern = skrifa::raw::types::Tag::new(b"kern");
    let mut indices: Vec<u16> = Vec::new();
    for rec in features.feature_records().iter().filter(|r| r.feature_tag() == kern) {
        let f = rec.feature(features.offset_data()).map_err(|err| e("kern feature", err))?;
        indices.extend(f.lookup_list_indices().iter().map(|i| i.get()));
    }
    if indices.is_empty() {
        return Ok(None);
    }
    indices.sort_unstable();
    indices.dedup();
    let lookups = gpos.lookup_list().map_err(|err| e("lookup list", err))?;
    let glyphs: Vec<GlyphId> = by_glyph.keys().copied().collect();
    let mut work = 0u64;
    let mut step = |n: usize| -> Result<(), String> {
        work = work.saturating_add(n as u64);
        if work > limits.max_kerning_work {
            return Err(format!("GPOS kerning: more than {} steps (the tables' size)", limits.max_kerning_work));
        }
        Ok(())
    };
    let mut sums = GlyphPairs::new();
    for i in indices {
        let lookup = lookups.lookups().get(usize::from(i)).map_err(|err| e("lookup", err))?;
        let Ok(PositionSubtables::Pair(subtables)) = lookup.subtables() else { continue };
        let (mut decided, mut decided_all) = (HashSet::new(), HashSet::new());
        for sub in subtables.iter() {
            match sub.map_err(|err| e("pair subtable", err))? {
                PairPos::Format1(f) => {
                    let cov = f.coverage().map_err(|err| e("coverage", err))?;
                    let sets = f.pair_sets();
                    for &g1 in &glyphs {
                        step(1)?;
                        let Some(at) = cov.get(g1) else { continue };
                        if decided_all.contains(&g1) {
                            continue;
                        }
                        let set = sets.get(usize::from(at)).map_err(|err| e("pair set", err))?;
                        let records = set.pair_value_records();
                        step(records.len())?;
                        for rec in records.iter() {
                            let rec = rec.map_err(|err| e("pair record", err))?;
                            let g2 = GlyphId::from(rec.second_glyph());
                            if !by_glyph.contains_key(&g2) || !decided.insert((g1, g2)) {
                                continue;
                            }
                            if let Some(x) = rec.value_record1().x_advance() {
                                *sums.entry((g1, g2)).or_default() += i32::from(x);
                            }
                        }
                    }
                }
                PairPos::Format2(f) => {
                    let cov = f.coverage().map_err(|err| e("coverage", err))?;
                    let (cd1, cd2) = (
                        f.class_def1().map_err(|err| e("class definition", err))?,
                        f.class_def2().map_err(|err| e("class definition", err))?,
                    );
                    step(glyphs.len())?;
                    let mut second: BTreeMap<u16, Vec<GlyphId>> = BTreeMap::new();
                    for &g in &glyphs {
                        second.entry(cd2.get(g)).or_default().push(g);
                    }
                    let class1 = f.class1_records();
                    for &g1 in &glyphs {
                        step(1)?;
                        if cov.get(g1).is_none() || !decided_all.insert(g1) {
                            continue;
                        }
                        let row = class1.get(usize::from(cd1.get(g1))).map_err(|err| e("class record", err))?;
                        step(second.len())?;
                        for (&c2, g2s) in &second {
                            let cell =
                                row.class2_records().get(usize::from(c2)).map_err(|err| e("class record", err))?;
                            let Some(x) = cell.value_record1().x_advance().filter(|&x| x != 0) else { continue };
                            step(g2s.len())?;
                            for &g2 in g2s {
                                if !decided.contains(&(g1, g2)) {
                                    *sums.entry((g1, g2)).or_default() += i32::from(x);
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    Ok(Some(sums))
}

pub(crate) fn rect_of(b: [f64; 4]) -> Rect {
    let c = |v: f64| v.round().clamp(f64::from(i32::MIN / 2), f64::from(i32::MAX / 2)) as i32;
    Rect { x_min: c(b[0]), x_max: c(b[2]), y_min: c(b[1]), y_max: c(b[3]) }
}

#[cfg(test)]
mod tests;
