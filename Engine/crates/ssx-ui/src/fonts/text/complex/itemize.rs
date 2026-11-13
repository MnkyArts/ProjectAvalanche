use std::sync::Arc;

use icu_properties::props::Script;
use icu_properties::{CodePointMapData, PropertyNamesShort};
use icu_segmenter::GraphemeClusterSegmenter;
use ssx_formats::gfx::text::Font;

use super::super::{glyph_index, isolate};
use crate::bidi::{self, Class, class};
use crate::library::Library;
use crate::ot::OtFont;

// PORT(choice): 31 characters per drawn cluster, from UAX #15's stream-safe text format (a starter and at most 30
// non-starters); the original has no fallback fonts.
pub const MAX_CLUSTER_CHARS: usize = 31;

#[derive(Clone)]
pub enum Choice {
    None,
    Df3 { font: Arc<Font>, glyph: usize },
    Ot(Arc<OtFont>),
}

#[derive(Clone)]
pub struct Cluster {
    pub start: usize,
    pub drawn: usize,
    pub end: usize,
    pub script: Script,
    pub font: Choice,
}

pub fn simple_script(s: Script) -> bool {
    [
        Script::Common,
        Script::Inherited,
        Script::Unknown,
        Script::Latin,
        Script::Greek,
        Script::Cyrillic,
        Script::Armenian,
        Script::Georgian,
        Script::Han,
        Script::Hiragana,
        Script::Katakana,
        Script::Bopomofo,
        Script::Hangul,
    ]
    .contains(&s)
}

pub fn script_of(c: char) -> Script {
    CodePointMapData::<Script>::new().get(c)
}

pub fn script_code(s: Script) -> &'static str {
    PropertyNamesShort::<Script>::new().get(s).unwrap_or("Zyyy")
}

pub fn needs_bidi(c: Class) -> bool {
    matches!(c, Class::R | Class::AL | Class::AN | Class::RLE | Class::RLO | Class::RLI)
}

pub fn df3_glyph(lib: &Library, run: &Arc<Font>, c: char, outside: bool) -> Option<(Arc<Font>, usize)> {
    if let Some(g) = glyph_index(run, c).or_else(|| if c == '\u{a0}' { glyph_index(run, ' ') } else { None }) {
        return Some((run.clone(), g));
    }
    outside
        .then(|| lib.disc_glyph(c))
        .flatten()
        .or_else(|| lib.fallback_fonts().iter().find_map(|f| glyph_index(f, c).map(|g| (f, g))))
        .map(|(f, g)| (f.clone(), g))
}

pub fn ot_draws(lib: &Library, c: char, outside: bool) -> bool {
    lib.package_ot().iter().any(|f| f.covers(c)) || (outside && crate::fallback::draws(c))
}

pub fn clusters(chars: &[char]) -> Vec<(usize, usize)> {
    let s: String = chars.iter().collect();
    let mut byte_to_char = vec![0usize; s.len() + 1];
    let mut k = 0;
    for (ci, (b, c)) in s.char_indices().enumerate() {
        byte_to_char[b] = ci;
        k = b + c.len_utf8();
    }
    byte_to_char[k] = chars.len();
    let bounds: Vec<usize> = GraphemeClusterSegmenter::new().segment_str(&s).map(|b| byte_to_char[b]).collect();
    bounds.windows(2).filter(|w| w[1] > w[0]).map(|w| (w[0], w[1])).collect()
}

pub fn levels(chars: &[char], classes: &[Class]) -> (Vec<u8>, Vec<(usize, u8)>) {
    if !classes.iter().any(|&c| needs_bidi(c)) {
        return (vec![0; chars.len()], vec![(chars.len(), 0)]);
    }
    let mut levels = Vec::with_capacity(chars.len());
    let mut paras = Vec::new();
    let mut start = 0;
    while start < chars.len() {
        let end = classes[start..].iter().position(|&c| c == Class::B).map_or(chars.len(), |p| start + p + 1);
        let brackets: Vec<bidi::Bracket> = chars[start..end].iter().map(|&c| class::bracket(c)).collect();
        let r = bidi::resolve(&classes[start..end], &brackets, None);
        levels.extend_from_slice(&r.levels);
        paras.push((end, r.base));
        start = end;
    }
    if paras.is_empty() {
        paras.push((0, 0));
    }
    (levels, paras)
}

pub fn itemize(
    lib: &Library,
    chars: &[char],
    levels: &[u8],
    run_of: &[usize],
    run_fonts: &[Option<Arc<Font>>],
    outside: &[bool],
) -> Vec<Cluster> {
    let mut out: Vec<Cluster> = clusters(chars)
        .into_iter()
        .map(|(start, end)| Cluster {
            start,
            drawn: end.min(start + MAX_CLUSTER_CHARS),
            end,
            script: script_of(chars[start]),
            font: Choice::None,
        })
        .collect();
    resolve_scripts(&mut out);
    for c in &mut out {
        let first = chars[c.start];
        let Some(run) = run_fonts[run_of[c.start]].as_ref() else { continue };
        if isolate::is_mark(first) {
            continue;
        }
        let drawn = &chars[c.start..c.drawn];
        let outside = outside.get(c.start).copied().unwrap_or(false);
        let own = script_of(first);
        if drawn.len() == 1 && simple_script(own) && !matches!(class::of(first), Class::R | Class::AL | Class::AN) {
            let mirrored = (levels[c.start] & 1 == 1).then(|| class::mirror(first)).flatten();
            let found =
                mirrored.and_then(|m| df3_glyph(lib, run, m, outside)).or_else(|| df3_glyph(lib, run, first, outside));
            if let Some((font, glyph)) = found {
                c.font = Choice::Df3 { font, glyph };
                continue;
            }
        }
        let covered = |f: &OtFont| drawn.iter().all(|&ch| f.covers(ch) || crate::fallback::ignorable(ch));
        let ot = lib
            .package_ot()
            .iter()
            .find(|f| covered(f))
            .cloned()
            .or_else(|| outside.then(|| crate::fallback::for_cluster(script_code(c.script), drawn)).flatten());
        if let Some(f) = ot {
            c.font = Choice::Ot(f);
        }
    }
    han_runs(lib, chars, run_of, run_fonts, outside, &mut out);
    out
}

fn han_runs(
    lib: &Library,
    chars: &[char],
    run_of: &[usize],
    run_fonts: &[Option<Arc<Font>>],
    outside: &[bool],
    out: &mut [Cluster],
) {
    let member = |c: &Cluster| {
        c.script == Script::Han
            && outside.get(c.start).copied().unwrap_or(false)
            && !isolate::is_mark(chars[c.start])
            && run_fonts[run_of[c.start]].is_some()
    };
    let mut i = 0;
    while i < out.len() {
        if !member(&out[i]) {
            i += 1;
            continue;
        }
        let end = out[i..].iter().position(|c| !member(c)).map_or(out.len(), |p| i + p);
        let run = &mut out[i..end];
        i = end;
        if run.iter().all(|c| matches!(c.font, Choice::Df3 { .. })) {
            continue;
        }
        let text: Vec<char> = run.iter().flat_map(|c| chars[c.start..c.drawn].iter().copied()).collect();
        let covered = |f: &OtFont| text.iter().all(|&ch| f.covers(ch) || crate::fallback::ignorable(ch));
        let font = lib.package_ot().iter().find(|f| covered(f)).cloned().or_else(|| {
            crate::fallback::for_cluster(script_code(Script::Han), &text)
                .or_else(|| crate::fallback::for_cluster(script_code(Script::Han), &[]))
        });
        let Some(font) = font else { continue };
        for c in run.iter_mut() {
            let drawn = &chars[c.start..c.drawn];
            if drawn.iter().all(|&ch| font.covers(ch) || crate::fallback::ignorable(ch)) {
                c.font = Choice::Ot(font.clone());
            }
        }
    }
}

// PORT(choice): UAX #24's Script_Extensions are not read: a Common / Inherited cluster joins the run before it (as
// HarfBuzz's own itemizers do), so a shared punctuation mark is shaped with its neighbours; the original has no
// fallback fonts.
fn resolve_scripts(clusters: &mut [Cluster]) {
    let real = |s: Script| s != Script::Common && s != Script::Inherited && s != Script::Unknown;
    let mut last: Option<Script> = None;
    for c in clusters.iter_mut() {
        if real(c.script) {
            last = Some(c.script);
        } else if let Some(s) = last {
            c.script = s;
        }
    }
    let first_real = clusters.iter().map(|c| c.script).find(|&s| real(s));
    if let Some(s) = first_real {
        for c in clusters.iter_mut().take_while(|c| !real(c.script)) {
            c.script = s;
        }
    }
}
