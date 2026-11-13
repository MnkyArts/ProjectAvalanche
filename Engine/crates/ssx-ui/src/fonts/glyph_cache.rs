use std::collections::HashMap;
use std::sync::{Arc, Weak};

use ssx_formats::gfx::text::Font;

use super::glyph::{GlyphRaster, Rasterized, TEXTURE_SIZE, rasterize_glyph};
use super::glyph_filter::{BlurRows, GlyphFilter, rasterize_filtered};
use crate::tess::Mesh;

pub type AtlasRect = [u32; 4];

// TODO(guess): one growing texture, never evicted (the game's is one 1024² texture 0x82726538; a full cache ends the glyph run, 0x82725070 returns 0, and 0x826EF1D0 logs "Increase raster glyph cache capacity"); settle: the packer 0x826EEA88 → 0x82729528, which tries 0x82729078, 0x827290D8 and 0x82729458 in turn and keeps the slots in a most-recently-used list (not decoded further).
#[derive(Clone, Debug)]
pub struct GlyphAtlas {
    pub width: u32,
    pub height: u32,
    pub alpha: Vec<u8>,
    pub generation: u64,
    shelves: Vec<(u32, u32, u32)>,
    used: u32,
}

impl Default for GlyphAtlas {
    fn default() -> Self {
        GlyphAtlas {
            width: TEXTURE_SIZE,
            height: TEXTURE_SIZE,
            alpha: vec![0; (TEXTURE_SIZE * TEXTURE_SIZE) as usize],
            generation: 0,
            shelves: Vec::new(),
            used: 0,
        }
    }
}

impl GlyphAtlas {
    pub fn add(&mut self, r: &GlyphRaster) -> Option<AtlasRect> {
        let (w, h) = (r.width + 1, r.height + 1);
        let shelf = self.shelves.iter_mut().find(|s| s.1 >= h && s.1 <= h + h / 4 + 2 && s.2 + w <= self.width);
        let (x, y) = match shelf {
            Some(s) => {
                s.2 += w;
                (s.2 - w, s.0)
            }
            None => {
                let top = self.used;
                if top + h > MAX_ATLAS_HEIGHT {
                    return None;
                }
                self.used += h;
                while self.used > self.height {
                    self.alpha.resize((self.width * self.height * 2) as usize, 0);
                    self.height *= 2;
                }
                self.shelves.push((top, h, w));
                (0, top)
            }
        };
        for row in 0..r.height {
            let at = ((y + row) * self.width + x) as usize;
            let src = (row * r.width) as usize;
            self.alpha[at..at + r.width as usize].copy_from_slice(&r.alpha[src..src + r.width as usize]);
        }
        self.generation += 1;
        Some([x, y, x + r.width, y + r.height])
    }
}

#[derive(Clone, Debug)]
pub struct RasterGlyph {
    pub raster: Arc<GlyphRaster>,
    pub rect: AtlasRect,
    pub quad: Arc<Mesh>,
    pub scale: f32,
}

// PORT(choice): the cap and the reset, as the port's texture grows and outside text in any script can fill it; the
// game's cache is one 1024² texture (0x82726538), and a full cache ends the glyph run.
pub const MAX_ATLAS_HEIGHT: u32 = 8192;

type GlyphKey = (usize, usize, u16, Option<GlyphFilter>);

#[derive(Clone, Debug, Default)]
pub struct GlyphCache {
    glyphs: HashMap<GlyphKey, (Weak<Font>, Option<RasterGlyph>)>,
    full: bool,
    pub resets: usize,
    pub atlas: GlyphAtlas,
    blur: BlurRows,
    pub rasterized: usize,
    pub too_tall: usize,
}

pub enum CachedGlyph {
    Raster(RasterGlyph),
    Empty,
    Vector,
}

impl GlyphCache {
    pub fn glyph(&mut self, font: &Arc<Font>, index: usize, size: u16) -> CachedGlyph {
        self.entry(font, (Arc::as_ptr(font) as usize, index, size, None), |_| (rasterize_glyph(font, index, size), 1.0))
    }

    pub fn begin_frame(&mut self) {
        if std::mem::take(&mut self.full) {
            let generation = self.atlas.generation + 1;
            self.glyphs.clear();
            self.atlas = GlyphAtlas { generation, ..GlyphAtlas::default() };
            self.resets += 1;
        }
    }

    pub fn filtered(
        &mut self,
        font: &Arc<Font>,
        index: usize,
        size: u16,
        filter: GlyphFilter,
        ratio: f32,
        screen: f32,
    ) -> CachedGlyph {
        let key = (Arc::as_ptr(font) as usize, index, size, Some(filter));
        self.entry(font, key, |c| rasterize_filtered(font, index, size, &filter, ratio, screen, &mut c.blur))
    }

    fn entry(
        &mut self,
        font: &Arc<Font>,
        key: GlyphKey,
        make: impl FnOnce(&mut Self) -> (Rasterized, f32),
    ) -> CachedGlyph {
        if !self.glyphs.contains_key(&key) {
            if self.full {
                return CachedGlyph::Vector;
            }
            let (raster, scale) = make(self);
            let entry = match raster {
                Rasterized::Raster(r) => {
                    self.rasterized += 1;
                    let Some(rect) = self.atlas.add(&r) else {
                        self.full = true;
                        return CachedGlyph::Vector;
                    };
                    let (x0, y0, x1, y1) =
                        (r.x0 as f32, r.y0 as f32, (r.x0 + r.width as i32) as f32, (r.y0 + r.height as i32) as f32);
                    let quad = Mesh {
                        vertices: vec![[x0, y0], [x1, y0], [x1, y1], [x0, y1]],
                        indices: vec![0, 1, 2, 0, 2, 3],
                        paint: Vec::new(),
                    };
                    Some(RasterGlyph { raster: r, rect, quad: Arc::new(quad), scale })
                }
                Rasterized::Empty => {
                    let raster = Arc::new(GlyphRaster { x0: 0, y0: 0, width: 0, height: 0, alpha: Vec::new() });
                    Some(RasterGlyph { raster, rect: [0; 4], quad: Arc::new(Mesh::default()), scale })
                }
                Rasterized::TooTall => {
                    self.too_tall += 1;
                    None
                }
            };
            self.glyphs.insert(key, (Arc::downgrade(font), entry));
        }
        match &self.glyphs[&key].1 {
            Some(r) if r.raster.width > 0 => CachedGlyph::Raster(r.clone()),
            Some(_) => CachedGlyph::Empty,
            None => CachedGlyph::Vector,
        }
    }
}
