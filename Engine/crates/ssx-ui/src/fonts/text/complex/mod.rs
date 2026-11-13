pub mod itemize;
pub mod lines;
pub mod shape;

use std::sync::Arc;

use harfrust::Language;
use icu_properties::props::Script;
use ssx_formats::gfx::text::Font;

use super::{Align, CharFormat, GUTTER, Paragraph, PlacedGlyph, TextLayout, glyph_ink_right, isolate, metrics};
use crate::bidi::{Class, class};
use crate::geom::RectF;
use crate::library::{Library, MovieId};
use crate::ot::Converted;
use shape::Src;

// PORT(choice): 16 384 characters per paragraph (a hostile package movie may set any text; about 6 ms of shaping at the
// measured 0.4 µs per Arabic character); the original has no fallback fonts.
pub const MAX_PARAGRAPH_CHARS: usize = 16_384;

pub struct Field<'a> {
    pub lib: &'a Library,
    pub movie: MovieId,
    pub font_id: Option<u16>,
    pub bounds: RectF,
    pub word_wrap: bool,
    pub origin: f32,
    pub field_w: f32,
    pub default_metrics: (f32, f32),
}

pub fn game_layout_draws(lib: &Library, runs: &[(Option<Arc<Font>>, &str)], outside: &[bool]) -> bool {
    let mut low = true;
    let mut at = 0;
    for (font, text) in runs {
        for c in text.chars() {
            let out = outside.get(at).copied().unwrap_or(false);
            at += 1;
            if isolate::is_mark(c) {
                continue;
            }
            low &= u32::from(c) < 0x300;
            if let Some(font) = font
                && itemize::df3_glyph(lib, font, c, out).is_none()
                && itemize::ot_draws(lib, c, out)
            {
                return false;
            }
        }
    }
    if low {
        return true;
    }
    let chars: Vec<char> = runs.iter().flat_map(|(_, t)| t.chars()).filter(|&c| !isolate::is_mark(c)).collect();
    !needs_complex(&chars)
}

pub fn needs_complex(chars: &[char]) -> bool {
    !(chars.iter().all(|&c| !itemize::needs_bidi(class::of(c)) && itemize::simple_script(itemize::script_of(c)))
        && itemize::clusters(chars).iter().all(|&(a, b)| b - a == 1))
}

// PORT(choice): the language from the paragraph's kana / Hangul (a name carries no language tag; a Chinese name gets
// the Japanese forms of the shipped font, `locl` decides between them); the player's language setting as a hint is
// round text2's decision; the original has no fallback fonts.
fn language(chars: &[char]) -> Option<Language> {
    let has = |s: Script| chars.iter().any(|&c| itemize::script_of(c) == s);
    let tag = if has(Script::Hiragana) || has(Script::Katakana) {
        "ja"
    } else if has(Script::Hangul) {
        "ko"
    } else {
        return None;
    };
    tag.parse().ok()
}

pub fn paragraph(
    f: &Field<'_>,
    para: &Paragraph,
    char_base: u32,
    top: &mut f32,
    last_leading: &mut f32,
    out: &mut TextLayout,
) {
    let mut chars: Vec<char> = Vec::new();
    let mut run_of: Vec<usize> = Vec::new();
    let formats: Vec<&CharFormat> = para.runs.iter().map(|r| &r.format).collect();
    let run_fonts: Vec<Option<Arc<Font>>> =
        para.runs.iter().map(|r| f.lib.font_for(f.movie, f.font_id, r.format.face.as_deref())).collect();
    for (ri, r) in para.runs.iter().enumerate() {
        for c in r.text.chars().take(MAX_PARAGRAPH_CHARS.saturating_sub(chars.len())) {
            chars.push(c);
            run_of.push(ri);
        }
    }
    let run_metrics: Vec<Option<(f32, f32)>> =
        run_fonts.iter().zip(&formats).map(|(fo, fm)| fo.as_ref().map(|fo| metrics(fo, fm.size))).collect();
    let para_metrics = run_metrics.iter().flatten().next().copied();
    let classes: Vec<Class> = chars.iter().map(|&c| class::of(c)).collect();
    let (levels, bidi_paras) = itemize::levels(&chars, &classes);
    let bidi_on = classes.iter().any(|&c| itemize::needs_bidi(c));
    let outside = isolate::outside(chars.iter().copied());
    let clusters = itemize::itemize(f.lib, &chars, &levels, &run_of, &run_fonts, &outside);
    let text = shape::Text {
        chars: &chars,
        levels: &levels,
        run_of: &run_of,
        formats: &formats,
        run_fonts: &run_fonts,
        language: language(&chars),
    };
    let mut units = shape::units(&text, &clusters);
    let mut brk = lines::breaks(&chars, &classes, &units);
    let pf = &para.format;
    let avail = |first: bool| f.field_w - pf.left_margin - pf.right_margin - if first { pf.indent } else { 0.0 };
    let mut first = true;
    for line in lines::fill(&text, &clusters, &classes, &mut units, &mut brk, f.word_wrap, avail) {
        let indent = if first { pf.indent } else { 0.0 };
        let c0 = units.get(line.0).map_or(0, |u| u.start);
        let base = bidi_paras.iter().find(|p| c0 < p.0).or(bidi_paras.last()).map_or(0, |p| p.1);
        let (order, width) = lines::visual(&units, line, &classes, &levels, base, bidi_on);
        // PORT(choice): a line's ascent and descent are its runs' fonts' (a fallback glyph's taller ink, e.g. a
        // Devanagari or Thai mark stack, overhangs the line as the game path's fallback glyphs do); the shipped fonts'
        // own metrics would change line heights between scripts; the original has no fallback fonts.
        let (ascent, descent) = if line.0 == line.1 {
            para_metrics.unwrap_or(f.default_metrics)
        } else {
            units[line.0..line.1]
                .iter()
                .filter_map(|u| run_metrics[u.run])
                .fold((0.0f32, 0.0f32), |m, i| (m.0.max(i.0), m.1.max(i.1)))
        };
        let left = f.bounds.x0 + GUTTER + pf.left_margin + indent;
        let right = f.bounds.x1 - GUTTER - pf.right_margin;
        let x0 = match pf.align {
            Align::Right => right - width,
            Align::Center => left + (right - left - width) / 2.0,
            Align::Left | Align::Justify => left,
        };
        let baseline = *top + ascent;
        let converted = lines::convert(&units, &order, f.lib.text_budget());
        let mut pen = x0;
        for &u in &order {
            let unit = &units[u];
            for (gi, g) in unit.glyphs.iter().enumerate().filter(|_| !unit.space) {
                let (font, index) = match &g.src {
                    Src::Df3(font, index) => (font.clone(), *index),
                    Src::Ot(..) => match converted.get(&(u, gi)) {
                        Some(Some(Converted::Glyph(font))) => (font.clone(), 0),
                        Some(Some(Converted::Refused)) => continue,
                        _ => {
                            out.pending = true;
                            continue;
                        }
                    },
                };
                let scale = g.scale;
                out.glyphs.push(PlacedGlyph {
                    ink_right: glyph_ink_right(&font, index) * scale,
                    font,
                    index,
                    x: pen + g.dx,
                    y: baseline - g.dy,
                    scale,
                    color: unit.color,
                    line: out.lines as u32,
                    advance: g.advance,
                    char_index: char_base.saturating_add(u32::try_from(unit.start).unwrap_or(u32::MAX)),
                });
            }
            pen += unit.advance;
        }
        out.text_width = out.text_width.max(width);
        out.line_boxes.push((*top - f.origin, ascent + descent));
        out.lines += 1;
        *top = baseline + descent + pf.leading;
        *last_leading = pf.leading;
        first = false;
    }
}

#[cfg(test)]
mod tests;
