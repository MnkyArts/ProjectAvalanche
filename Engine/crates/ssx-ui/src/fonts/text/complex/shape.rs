use std::str::FromStr;
use std::sync::Arc;

use harfrust::Language;
use icu_properties::props::Script;
use ssx_formats::gfx::text::Font;

use super::super::{CharFormat, MISSING_GLYPH_EM, isolate, kerning_pair};
use super::itemize::{Choice, Cluster, script_code};
use crate::ot::OtFont;

// PORT(choice): 256 characters per shaper call (a chat line's limit; the measured Arabic cost 0.4 µs a character, so a
// call stays under 0.2 ms); the original has no fallback fonts.
pub const MAX_SHAPED_CHARS: usize = 256;
const CONTEXT: usize = 5;

#[derive(Clone)]
pub enum Src {
    Df3(Arc<Font>, usize),
    Ot(Arc<OtFont>, u32),
}

#[derive(Clone)]
pub struct UnitGlyph {
    pub src: Src,
    pub dx: f32,
    pub dy: f32,
    pub scale: f32,
    pub advance: f32,
}

#[derive(Clone)]
pub struct Unit {
    pub start: usize,
    pub end: usize,
    pub run: usize,
    pub advance: f32,
    pub glyphs: Vec<UnitGlyph>,
    pub color: [f32; 4],
    pub space: bool,
    pub shaped: bool,
    pub part: u32,
}

pub const NO_PART: u32 = u32::MAX;

pub struct Text<'a> {
    pub chars: &'a [char],
    pub levels: &'a [u8],
    pub run_of: &'a [usize],
    pub formats: &'a [&'a CharFormat],
    pub run_fonts: &'a [Option<Arc<Font>>],
    pub language: Option<Language>,
}

fn space(c: char) -> bool {
    c == ' ' || c == '\u{a0}' || c == '\t'
}

pub fn units(t: &Text<'_>, clusters: &[Cluster]) -> Vec<Unit> {
    let mut out = Vec::with_capacity(clusters.len());
    let mut prev_code: Option<(usize, u16)> = None;
    let mut next_part = 0u32;
    let mut k = 0;
    while k < clusters.len() {
        let c = &clusters[k];
        let run = t.run_of[c.start];
        let f = t.formats[run];
        let unit = |advance, glyphs| Unit {
            start: c.start,
            end: c.end,
            run,
            advance,
            glyphs,
            color: f.color,
            space: t.chars[c.start..c.end].iter().all(|&ch| space(ch)),
            shaped: false,
            part: NO_PART,
        };
        match &c.font {
            Choice::None => {
                prev_code = None;
                // TODO(guess): the cluster draws nothing, as the game's layout draws nothing for a character no font
                // has — GetGlyphShape (font vtable 0x820c9320 +0x38, 0x826b5fe0) and GetTextureGlyph (+0x28,
                // 0x826b71c8) return null for glyph index 0xFFFF, but the line buffer's draw loop that reads the
                // formatter's (0x82715e18) glyph entries was not traced; settle: that loop.
                let missing = if t.run_fonts[run].is_some() {
                    t.chars[c.start..c.drawn].iter().filter(|&&ch| !isolate::is_mark(ch) && ch != '\t').count()
                } else {
                    0
                };
                out.push(unit(missing as f32 * f.size * MISSING_GLYPH_EM, Vec::new()));
                k += 1;
            }
            Choice::Df3 { font, glyph } => {
                let scale = f.size / super::super::em_units(font);
                let adv =
                    font.layout.as_ref().and_then(|l| l.advances.get(*glyph)).map_or(0.0, |&a| f32::from(a) * scale);
                let code = u32::from(t.chars[c.start]).min(0xFFFF) as u16;
                if f.kerning
                    && let (Some((r, p)), Some(run_font)) = (prev_code, t.run_fonts[run].as_ref())
                    && r == run
                    && let (Some(l), Some(last)) = (run_font.layout.as_ref(), out.last_mut())
                    && let Some(kp) = kerning_pair(&l.kerning, p, code)
                {
                    last.advance += f32::from(kp.2) * (f.size / super::super::em_units(run_font));
                }
                prev_code = Some((run, code));
                let g = UnitGlyph { src: Src::Df3(font.clone(), *glyph), dx: 0.0, dy: 0.0, scale, advance: adv };
                out.push(unit(adv + f.letter_spacing, vec![g]));
                k += 1;
            }
            Choice::Ot(font) => {
                prev_code = None;
                let same = |d: &Cluster| {
                    matches!(&d.font, Choice::Ot(g) if Arc::ptr_eq(g, font))
                        && d.script == c.script
                        && t.levels[d.start] == t.levels[c.start]
                        && same_shaping(t.formats[t.run_of[d.start]], f)
                };
                let mut e = k;
                while e < clusters.len() && same(&clusters[e]) {
                    e += 1;
                }
                shape_parts(t, &clusters[k..e], font, c.script, &mut next_part, &mut out);
                k = e;
            }
        }
    }
    out
}

fn shape_parts(
    t: &Text<'_>,
    run: &[Cluster],
    font: &Arc<OtFont>,
    script: Script,
    next_part: &mut u32,
    out: &mut Vec<Unit>,
) {
    let mut from = 0;
    while from < run.len() {
        let mut to = from + 1;
        while to < run.len() && run[to].end - run[from].start <= MAX_SHAPED_CHARS {
            to += 1;
        }
        shape_part(t, &run[from..to], font, script, *next_part, out);
        *next_part = next_part.saturating_add(1).min(NO_PART - 1);
        from = to;
    }
}

pub fn reshape(t: &Text<'_>, clusters: &[Cluster], lo: usize, cut: usize, hi: usize, next_part: &mut u32) -> Vec<Unit> {
    let a = clusters.partition_point(|c| c.start < lo);
    let m = clusters.partition_point(|c| c.start < cut).max(a);
    let b = clusters.partition_point(|c| c.start < hi).max(m);
    let mut out = Vec::new();
    for piece in [&clusters[a..m], &clusters[m..b]] {
        if let Some(Cluster { font: Choice::Ot(font), script, .. }) = piece.first() {
            shape_parts(t, piece, font, *script, next_part, &mut out);
        }
    }
    out
}

fn same_shaping(a: &CharFormat, b: &CharFormat) -> bool {
    a.size == b.size && a.letter_spacing == b.letter_spacing && a.kerning == b.kerning
}

fn shape_part(t: &Text<'_>, part: &[Cluster], font: &Arc<OtFont>, script: Script, id: u32, out: &mut Vec<Unit>) {
    let (Some(first), Some(last)) = (part.first(), part.last()) else { return };
    let run = t.run_of[first.start];
    let f = t.formats[run];
    let level = t.levels[first.start];
    let mut text: Vec<char> = Vec::new();
    let mut starts: Vec<(u32, usize)> = Vec::new();
    for c in part {
        starts.push((text.len() as u32, c.start));
        text.extend_from_slice(&t.chars[c.start..c.drawn]);
    }
    let before = &t.chars[first.start.saturating_sub(CONTEXT)..first.start];
    let after = &t.chars[last.end..(last.end + CONTEXT).min(t.chars.len())];
    let hb_script = harfrust::Script::from_str(script_code(script)).unwrap_or(harfrust::script::COMMON);
    let shaped = font.shape(&crate::ot::ShapeRun {
        text: &text,
        before,
        after,
        rtl: level & 1 == 1,
        script: hb_script,
        language: t.language.clone(),
        kerning: f.kerning,
    });
    let k = f.size / f32::from(font.units_per_em());
    let scale = f.size / 20480.0;
    let Some(shaped) = shaped else {
        for c in part {
            out.push(Unit {
                start: c.start,
                end: c.end,
                run,
                advance: 0.0,
                glyphs: Vec::new(),
                color: f.color,
                space: false,
                shaped: true,
                part: id,
            });
        }
        return;
    };
    let ours = |text_at: u32| starts.partition_point(|&(s, _)| s <= text_at).saturating_sub(1);
    let of_glyph: Vec<usize> = shaped.iter().map(|g| ours(g.cluster)).collect();
    let mut heads: Vec<usize> = of_glyph.clone();
    heads.sort_unstable();
    heads.dedup();
    if heads.first() != Some(&0) {
        heads.insert(0, 0);
    }
    for (n, &a) in heads.iter().enumerate() {
        let b = heads.get(n + 1).copied().unwrap_or(part.len());
        let (start, end) = (part[a].start, part[b - 1].end);
        let mut pen = 0.0f32;
        let mut glyphs = Vec::new();
        for (g, _) in shaped.iter().zip(&of_glyph).filter(|(_, o)| **o == a) {
            glyphs.push(UnitGlyph {
                src: Src::Ot(font.clone(), g.gid),
                dx: pen + g.x_offset as f32 * k,
                dy: g.y_offset as f32 * k,
                scale,
                advance: g.x_advance as f32 * k,
            });
            pen += g.x_advance as f32 * k;
        }
        out.push(Unit {
            start,
            end,
            run,
            advance: pen + f.letter_spacing * (b - a) as f32,
            glyphs,
            color: t.formats[t.run_of[start]].color,
            space: t.chars[start..end].iter().all(|&ch| space(ch)),
            shaped: true,
            part: id,
        });
    }
}
