pub mod store;

use std::collections::HashMap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use harfrust::{Direction, Feature, Language, Script, ShapePlan, ShaperData, Tag, UnicodeBuffer};
use ssx_content::font::Limits;
use ssx_content::font::lazy::{Bytes, GlyphFont};
use ssx_formats::gfx::text::{Font, FontLayout};

pub use store::{Budget, Converted, stats as store_stats};

// PORT(choice): 64 cached shape plans per font; past them a plan is made for its call and not kept; the original has no
// fallback fonts.
const PLANS: usize = 64;

static SERIAL: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Shaped {
    pub gid: u32,
    pub cluster: u32,
    pub x_advance: i32,
    pub x_offset: i32,
    pub y_offset: i32,
}

#[derive(Clone, Debug)]
pub struct ShapeRun<'a> {
    pub text: &'a [char],
    pub before: &'a [char],
    pub after: &'a [char],
    pub rtl: bool,
    pub script: Script,
    pub language: Option<Language>,
    pub kerning: bool,
}

type PlanKey = (bool, Script, Option<Language>, bool);

pub struct OtFont {
    pub name: String,
    serial: u64,
    face: GlyphFont,
    shaper: ShaperData,
    plans: Mutex<HashMap<PlanKey, Arc<ShapePlan>>>,
    broken: AtomicBool,
}

impl std::fmt::Debug for OtFont {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OtFont").field("name", &self.name).field("glyphs", &self.face.glyphs).finish()
    }
}

impl OtFont {
    pub fn open(name: &str, bytes: Bytes, limits: &Limits) -> Result<OtFont, String> {
        let face = GlyphFont::open(bytes, limits)?;
        let shaper = catch_unwind(AssertUnwindSafe(|| face.font().map(|f| ShaperData::new(&f))))
            .map_err(|_| "the shaper failed on the font's tables".to_string())??;
        Ok(OtFont {
            name: name.to_string(),
            serial: SERIAL.fetch_add(1, Ordering::Relaxed),
            face,
            shaper,
            plans: Mutex::new(HashMap::new()),
            broken: AtomicBool::new(false),
        })
    }

    pub fn covers(&self, c: char) -> bool {
        !self.broken.load(Ordering::Relaxed) && self.face.covers(c)
    }

    pub fn coverage(&self) -> &[(u32, u32)] {
        &self.face.coverage
    }

    pub fn metrics(&self) -> (u16, u16) {
        (self.face.ascent, self.face.descent)
    }

    pub fn units_per_em(&self) -> u16 {
        self.face.units_per_em
    }

    pub fn shape(&self, run: &ShapeRun<'_>) -> Option<Vec<Shaped>> {
        let ShapeRun { text, before, after, rtl, script, ref language, kerning } = *run;
        if self.broken.load(Ordering::Relaxed) {
            return None;
        }
        let run = catch_unwind(AssertUnwindSafe(|| {
            let font = self.face.font().ok()?;
            let shaper = self.shaper.shaper(&font).build();
            let key: PlanKey = (rtl, script, language.clone(), kerning);
            let plan = {
                let mut plans = store_lock(&self.plans);
                match plans.get(&key) {
                    Some(p) => p.clone(),
                    None => {
                        let features = [Feature::new(Tag::new(b"kern"), u32::from(kerning), ..)];
                        let dir = if rtl { Direction::RightToLeft } else { Direction::LeftToRight };
                        let p = Arc::new(ShapePlan::new(&shaper, dir, Some(script), language.as_ref(), &features));
                        if plans.len() < PLANS {
                            plans.insert(key, p.clone());
                        }
                        p
                    }
                }
            };
            let mut buf = UnicodeBuffer::new();
            let near: Vec<u32> = before.iter().rev().map(|&c| u32::from(c)).collect();
            buf.set_pre_context_codepoints(&near);
            for (i, &c) in text.iter().enumerate() {
                buf.add(c, i as u32);
            }
            buf.set_post_context_codepoints(&after.iter().map(|&c| u32::from(c)).collect::<Vec<u32>>());
            buf.set_direction(if rtl { Direction::RightToLeft } else { Direction::LeftToRight });
            buf.set_script(script);
            if let Some(l) = language {
                buf.set_language(l.clone());
            }
            let features = [Feature::new(Tag::new(b"kern"), u32::from(kerning), ..)];
            let out = shaper.shape(buf, harfrust::ShapeOptions::new().plan(Some(&plan)).features(&features));
            Some(
                out.glyph_infos()
                    .iter()
                    .zip(out.glyph_positions())
                    .map(|(i, p)| Shaped {
                        gid: i.glyph_id,
                        cluster: i.cluster,
                        x_advance: p.x_advance,
                        x_offset: p.x_offset,
                        y_offset: p.y_offset,
                    })
                    .collect(),
            )
        }));
        match run {
            Ok(r) => r,
            Err(_) => {
                self.broken.store(true, Ordering::Relaxed);
                None
            }
        }
    }

    pub fn glyphs(&self, gids: &[u32], budget: &Budget) -> Vec<Option<Converted>> {
        let mut out: Vec<Option<Converted>> = gids.iter().map(|&g| store::get(self.serial, g)).collect();
        if out.iter().all(Option::is_some) || self.broken.load(Ordering::Relaxed) {
            return out;
        }
        let made = catch_unwind(AssertUnwindSafe(|| {
            let Ok(mut session) = self.face.session() else {
                return;
            };
            let mut first: HashMap<u32, usize> = HashMap::new();
            for (k, &gid) in gids.iter().enumerate() {
                if out[k].is_some() {
                    continue;
                }
                if let Some(&p) = first.get(&gid) {
                    out[k] = out[p].clone();
                    continue;
                }
                first.insert(gid, k);
                if !budget.take() {
                    break;
                }
                let glyph = match session.glyph(gid) {
                    Ok(g) => Converted::Glyph(Arc::new(self.one_glyph_font(g))),
                    Err(_) => Converted::Refused,
                };
                store::put(self.serial, gid, glyph.clone());
                out[k] = Some(glyph);
            }
        }));
        if made.is_err() {
            self.broken.store(true, Ordering::Relaxed);
        }
        out
    }

    fn one_glyph_font(&self, g: ssx_content::font::lazy::Glyph) -> Font {
        Font {
            version: 3,
            id: 0,
            flags: Font::HAS_LAYOUT | Font::WIDE_CODES,
            language: 0,
            name: self.name.clone(),
            glyphs: vec![g.shape],
            codes: vec![0],
            layout: Some(FontLayout {
                ascent: self.face.ascent,
                descent: self.face.descent,
                leading: self.face.leading,
                advances: vec![g.advance],
                bounds: vec![g.bounds],
                kerning: Vec::new(),
            }),
            unk_trailing_layout: None,
        }
    }
}

fn store_lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}
