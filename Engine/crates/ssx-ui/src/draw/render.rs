use std::collections::HashMap;
use std::sync::Arc;

use ssx_formats::gfx::text::Font;

use crate::display::{BlendMode, Instance, InstanceId, InstanceKind, Player};
use crate::geom::{Cxform, Mat2d, RectF};
use crate::glyph_filter::{GlyphFilter, ShadowParam, TextFieldParam, shadow_size};
use crate::library::{CharRef, Character, Library};

pub mod cover;
pub mod overlay;
mod stage;
use crate::scale9::Slice9;
use crate::shape::{self, Paint, ShapeMesh, Tolerance};
use crate::tess::{Budget, Mesh, OverBudget, Work};
pub use stage::{ScaleMode, stage_scale, stage_transform};

#[derive(Clone, Debug)]
pub struct DrawItem {
    pub mesh: Arc<Mesh>,
    pub paint: Paint,
    pub transform: Mat2d,
    pub cxform: Cxform,
    pub blend: BlendMode,
}

#[derive(Clone, Debug)]
pub enum Command {
    Draw(DrawItem),
    MaskBegin,
    MaskEnd,
    MaskPopBegin,
    MaskPopEnd,
}

#[derive(Clone, Debug, Default)]
pub struct RenderList {
    pub commands: Vec<Command>,
    pub viewport: (u32, u32),
    pub background: Option<[f32; 4]>,
    pub stage: Option<[f32; 4]>,
}

impl RenderList {
    pub fn draw_count(&self) -> usize {
        self.commands.iter().filter(|c| matches!(c, Command::Draw(_))).count()
    }

    pub fn triangle_count(&self) -> usize {
        self.commands
            .iter()
            .map(|c| match c {
                Command::Draw(d) => d.mesh.triangle_count(),
                _ => 0,
            })
            .sum()
    }
}

// TODO(guess): fixed power-of-two buckets instead of Scaleform's history-dependent reuse band; settle: the mesh cache's reuse test.
fn scale_bucket(scale: f32) -> i32 {
    scale.max(1e-6).log2().round() as i32
}

fn bucket_tolerance(bucket: i32) -> Tolerance {
    Tolerance::for_scale(2f32.powf(bucket as f32))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum MeshKey {
    Shape(CharRef, i32),
    Morph(CharRef, u16, i32),
    Glyph(usize, usize, i32),
    Bitmap(CharRef),
}

pub const FRAME_TESSELLATION: Work = Work { edges: 1 << 19, steps: 1 << 22, vertices: 1 << 20 };

pub const CACHE_VERTICES: u64 = 1 << 21;

const REFUSED_KEPT: usize = 256;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TessStats {
    pub shape_peak: Work,
    pub frame_peak: Work,
    pub deferred: u64,
    pub refused: u64,
    pub evicted: u64,
    pub cached_vertices_peak: u64,
}

struct Cached {
    mesh: Arc<ShapeMesh>,
    vertices: u64,
    frame: u64,
    owner: Option<std::sync::Weak<Font>>,
}

#[derive(Default)]
pub struct Renderer {
    meshes: HashMap<MeshKey, Cached>,
    cached_vertices: u64,
    frame: u64,
    frame_work: Work,
    pub tess_stats: TessStats,
    refused: Vec<String>,
    pub tessellated: usize,
    pub glyphs: crate::glyph::GlyphCache,
    pub no_filters: bool,
    overlay: overlay::OverlayCache,
}

#[derive(Default)]
struct TextLayers {
    shadow: Vec<Command>,
    raster: Vec<Command>,
    vector: Vec<Command>,
}

impl TextLayers {
    fn emit(self, out: &mut Vec<Command>) {
        out.extend(self.shadow);
        out.extend(self.raster);
        out.extend(self.vector);
    }
}

#[derive(Clone, Copy)]
struct Inherited {
    xf: Mat2d,
    cx: Cxform,
    blend: BlendMode,
    mask: bool,
    slice: Option<(Slice9, Mat2d, Mat2d)>,
}

impl Renderer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn new_frame(&mut self) {
        self.glyphs.begin_frame();
        self.tess_stats.frame_peak.peak(self.frame_work);
        self.frame += 1;
        self.frame_work = Work::default();
    }

    pub fn frame_work(&self) -> Work {
        self.frame_work
    }

    pub fn take_refused(&mut self) -> Vec<String> {
        std::mem::take(&mut self.refused)
    }

    pub fn build(&mut self, lib: &Library, player: &Player, viewport: (u32, u32), mode: ScaleMode) -> RenderList {
        self.new_frame();
        let mut out = RenderList { commands: Vec::new(), viewport, background: None, stage: None };
        let Some(root) = player.levels.values().next().and_then(|r| player.get(*r)) else { return out };
        let movie = lib.movie(root.movie);
        out.background = movie.background.map(crate::geom::rgba);
        let stage = stage_transform(movie.stage, viewport, mode);
        let ((x0, y0), (x1, y1)) =
            (stage.apply(movie.stage.x0, movie.stage.y0), stage.apply(movie.stage.x1, movie.stage.y1));
        out.stage = Some([x0, y0, x1, y1]);
        for &r in player.levels.values() {
            let inh = Inherited { xf: stage, cx: Cxform::IDENTITY, blend: BlendMode::Normal, mask: false, slice: None };
            self.instance(lib, player, r, inh, &mut out.commands);
        }
        out
    }

    pub fn build_instance(
        &mut self,
        lib: &Library,
        player: &Player,
        id: InstanceId,
        xf: Mat2d,
        out: &mut Vec<Command>,
    ) {
        let inh = Inherited { xf, cx: Cxform::IDENTITY, blend: BlendMode::Normal, mask: false, slice: None };
        self.instance(lib, player, id, inh, out);
    }

    fn instance(&mut self, lib: &Library, player: &Player, id: InstanceId, inh: Inherited, out: &mut Vec<Command>) {
        let Some(inst) = player.get(id) else { return };
        if !inst.visible && !inh.mask {
            return;
        }
        // TODO(guess): an instance with normal (or layer) blending pushes nothing, so the innermost other mode of its ancestors applies; settle: which render-tree nodes push.
        let blend = if inh.mask {
            BlendMode::Normal
        } else if matches!(inst.blend, BlendMode::Normal | BlendMode::Layer) {
            inh.blend
        } else {
            inst.blend
        };
        let matrix = wide_leaderboard(lib, player, inst).unwrap_or(inst.matrix);
        let here =
            Inherited { xf: inh.xf.mul(&matrix), cx: inh.cx.concat(&inst.cxform), blend, mask: inh.mask, slice: None };
        match &inst.kind {
            InstanceKind::Sprite(s) => {
                // TODO(guess): the nine regions' mapping is Flash's model; settle: the consumer of the key data 0x82687D48 fills (kept at the mesh's +0x34, 0x82688BE8 → 0x82687250).
                let slice = match inst.scale9 {
                    Some(grid) => {
                        let (sx, sy, rest) = Slice9::split(&matrix);
                        let scaled = (sx - 1.0).abs() >= 1e-3 || (sy - 1.0).abs() >= 1e-3;
                        let bounds = player.local_bounds(lib, id);
                        (scaled && !bounds.is_empty())
                            .then(|| (Slice9 { grid, bounds, sx, sy }, inh.xf.mul(&rest), Mat2d::IDENTITY))
                    }
                    None => inh.slice.map(|(sl, base, to_grid)| (sl, base, to_grid.mul(&matrix))),
                };
                let here = Inherited { slice, ..here };
                let mut masks: Vec<(i32, InstanceId)> = Vec::new();
                for (&depth, &child) in &s.children {
                    while masks.last().is_some_and(|m| depth > m.0) {
                        let (_, m) = masks.pop().expect("an open mask");
                        self.pop_mask(lib, player, m, here, out);
                    }
                    let Some(c) = player.get(child) else { continue };
                    if let Some(clip) = c.clip_depth.filter(|_| !here.mask) {
                        out.push(Command::MaskBegin);
                        self.instance(lib, player, child, Inherited { mask: true, ..here }, out);
                        out.push(Command::MaskEnd);
                        masks.push((clip, child));
                        continue;
                    }
                    self.instance(lib, player, child, here, out);
                }
                while let Some((_, m)) = masks.pop() {
                    self.pop_mask(lib, player, m, here, out);
                }
                if !here.mask && player.hud_extra >= 0.5 && is_side_dimmer(lib, player, inst) {
                    self.side_dimmer_edges(lib, player, inst, &matrix, &here, out);
                }
            }
            InstanceKind::Shape | InstanceKind::Morph | InstanceKind::Bitmap => {
                let Some(mesh) = self.character_mesh(lib, inst, &here.xf) else { return };
                match &inh.slice {
                    Some((sl, base, to_grid)) => {
                        let sliced = Inherited { xf: *base, ..here };
                        let shape = to_grid.mul(&matrix);
                        for layer in &mesh.layers {
                            let m = Arc::new(sl.slice(&layer.mesh, &shape));
                            let item = DrawItem {
                                mesh: m,
                                paint: layer.paint.clone(),
                                transform: *base,
                                cxform: sliced.cx,
                                blend: sliced.blend,
                            };
                            out.push(Command::Draw(item));
                        }
                    }
                    None => emit(out, &mesh, &here),
                }
            }
            InstanceKind::EditText(t) => {
                let param = inst.filter.filter(|_| !here.mask && !self.no_filters).map(|f| TextFieldParam::new(&f));
                // TODO(guess): the line buffer's flags 0x04 / 0x20 (+0x34, they turn these tests off) are taken as clear; settle: their setters (not among the writers of +0x34 / the view's +0x64).
                let g = crate::text::GUTTER;
                let tr = RectF { x0: t.bounds.x0 + g, y0: t.bounds.y0 + g, x1: t.bounds.x1 - g, y1: t.bounds.y1 - g };
                let limit = tr.height() + 20.0;
                let boxes = &t.layout.line_boxes;
                let visible = boxes
                    .iter()
                    .enumerate()
                    .take_while(|&(i, &(top, h))| if i == 0 { top <= limit } else { top + h <= limit })
                    .count();
                let mut clip = boxes.first().is_some_and(|&(top, h)| h > 0.0 && top <= limit && limit < top + h);
                let mut ended = u32::MAX;
                let mut shown = Vec::with_capacity(t.layout.glyphs.len());
                for gl in &t.layout.glyphs {
                    if gl.line as usize >= visible || gl.line == ended {
                        continue;
                    }
                    let right = gl.x + gl.ink_right;
                    if right <= tr.x0 {
                        continue;
                    }
                    if gl.x >= tr.x1 {
                        ended = gl.line;
                        continue;
                    }
                    clip |= (gl.x < tr.x0 && tr.x0 < right) || (gl.x < tr.x1 && tr.x1 < right);
                    shown.push(gl);
                }
                let clip = clip && !here.mask;
                let b = tr;
                let rect = clip.then(|| {
                    let mesh = Mesh {
                        vertices: vec![[b.x0, b.y0], [b.x1, b.y0], [b.x1, b.y1], [b.x0, b.y1]],
                        indices: vec![0, 1, 2, 0, 2, 3],
                        paint: Vec::new(),
                    };
                    DrawItem {
                        mesh: Arc::new(mesh),
                        paint: Paint::Solid([1.0; 4]),
                        transform: here.xf,
                        cxform: here.cx,
                        blend: BlendMode::Normal,
                    }
                });
                if let Some(r) = &rect {
                    out.extend([Command::MaskBegin, Command::Draw(r.clone()), Command::MaskEnd]);
                }
                let mut layers = TextLayers::default();
                for g in shown {
                    let size = g.scale * crate::text::em_units(&g.font);
                    let glyph = TextGlyph { font: &g.font, index: g.index, size, at: (g.x, g.y), color: g.color };
                    self.text_glyph(&glyph, &here.xf, &here, param.as_ref(), &mut layers);
                }
                layers.emit(out);
                if let Some(r) = rect {
                    out.extend([Command::MaskPopBegin, Command::Draw(r), Command::MaskPopEnd]);
                }
            }
            InstanceKind::StaticText => self.static_text(lib, inst, here, out),
        }
    }

    fn pop_mask(&mut self, lib: &Library, player: &Player, mask: InstanceId, here: Inherited, out: &mut Vec<Command>) {
        out.push(Command::MaskPopBegin);
        self.instance(lib, player, mask, Inherited { mask: true, ..here }, out);
        out.push(Command::MaskPopEnd);
    }
}

fn emit(out: &mut Vec<Command>, mesh: &ShapeMesh, here: &Inherited) {
    for layer in &mesh.layers {
        out.push(Command::Draw(DrawItem {
            mesh: layer.mesh.clone(),
            paint: layer.paint.clone(),
            transform: here.xf,
            cxform: here.cx,
            blend: here.blend,
        }));
    }
}

impl Renderer {
    fn tessellated(
        &mut self,
        key: MeshKey,
        what: impl FnOnce() -> String,
        make: impl FnOnce(&mut Budget) -> Result<ShapeMesh, OverBudget>,
    ) -> Option<Arc<ShapeMesh>> {
        if let Some(c) = self.meshes.get_mut(&key) {
            c.frame = self.frame;
            return Some(c.mesh.clone());
        }
        if self.frame_work.reached(&FRAME_TESSELLATION) {
            self.tess_stats.deferred += 1;
            return None;
        }
        let mut budget = Budget::shape();
        let made = make(&mut budget);
        self.frame_work.add(budget.used());
        self.tess_stats.shape_peak.peak(budget.used());
        self.tessellated += 1;
        let mesh = match made {
            Ok(m) => m,
            Err(e) => {
                self.tess_stats.refused += 1;
                if self.refused.len() < REFUSED_KEPT {
                    self.refused.push(format!("{}: {e}", what()));
                }
                ShapeMesh::default()
            }
        };
        let vertices: u64 = mesh.layers.iter().map(|l| l.mesh.vertices.len() as u64).sum();
        if self.cached_vertices + vertices > CACHE_VERTICES {
            let (frame, before) = (self.frame, self.meshes.len());
            self.meshes.retain(|_, c| c.frame == frame);
            self.tess_stats.evicted += (before - self.meshes.len()) as u64;
            self.cached_vertices = self.meshes.values().map(|c| c.vertices).sum();
        }
        let mesh = Arc::new(mesh);
        if self.cached_vertices + vertices <= CACHE_VERTICES {
            self.meshes.insert(key, Cached { mesh: mesh.clone(), vertices, frame: self.frame, owner: None });
            self.cached_vertices += vertices;
            self.tess_stats.cached_vertices_peak = self.tess_stats.cached_vertices_peak.max(self.cached_vertices);
        }
        Some(mesh)
    }

    fn character_mesh(&mut self, lib: &Library, inst: &Instance, xf: &Mat2d) -> Option<Arc<ShapeMesh>> {
        let r = inst.character?;
        let bucket = scale_bucket(xf.max_scale());
        let key = match inst.kind {
            InstanceKind::Morph => MeshKey::Morph(r, (inst.ratio * 65535.0).round() as u16, bucket),
            InstanceKind::Bitmap => MeshKey::Bitmap(r),
            _ => MeshKey::Shape(r, bucket),
        };
        let tol = bucket_tolerance(bucket);
        let what = || format!("{}: character {} at scale 2^{bucket}", lib.movie(r.movie).path, r.id);
        match lib.resolve(r)?.1 {
            Character::Shape(s) => self.tessellated(key, what, |b| {
                shape::build(lib, r.movie, &s.shape.styles, &s.shape.records, s.uses_fill_winding_rule, &tol, b)
            }),
            Character::Morph(ms) => self.tessellated(key, what, |b| {
                let (styles, records) = shape::morph_at(ms, inst.ratio);
                shape::build(lib, r.movie, &styles, &records, false, &tol, b)
            }),
            Character::Bitmap(b) => self.tessellated(key, what, |_| Ok(bitmap_quad(*b))),
            _ => None,
        }
    }

    fn glyph(&mut self, font: &Arc<Font>, index: usize, scale: f32) -> Arc<ShapeMesh> {
        let bucket = scale_bucket(scale);
        let key = MeshKey::Glyph(Arc::as_ptr(font) as usize, index, bucket);
        let tol = bucket_tolerance(bucket);
        let what = || format!("font {:?}: glyph {index} at scale 2^{bucket}", font.name);
        let mesh = self.tessellated(key, what, |b| {
            let mesh = match font.glyphs.get(index) {
                Some(g) => shape::glyph_mesh(&g.records, tol.curve, b)?,
                None => Mesh::default(),
            };
            let layers = if mesh.is_empty() {
                Vec::new()
            } else {
                vec![shape::ShapeLayer { paint: Paint::Solid([1.0; 4]), mesh: Arc::new(mesh) }]
            };
            Ok(ShapeMesh { layers })
        });
        if let Some(c) = self.meshes.get_mut(&key) {
            c.owner.get_or_insert_with(|| Arc::downgrade(font));
        }
        mesh.unwrap_or_default()
    }

    // TODO(guess): for a zero-glyph font the font library's font of that name stands in, index for index (never reached on the disc); settle: Scaleform's static text with a substituted font.
    fn static_text(&mut self, lib: &Library, inst: &Instance, here: Inherited, out: &mut Vec<Command>) {
        let Some(r) = inst.character else { return };
        let Some((_, Character::Text(t))) = lib.resolve(r) else { return };
        let param = inst.filter.filter(|_| !here.mask && !self.no_filters).map(|f| TextFieldParam::new(&f));
        let mut layers = TextLayers::default();
        let base = here.xf.mul(&Mat2d::from_swf(&t.matrix));
        let (mut font, mut color, mut height) = (None, [0.0, 0.0, 0.0, 1.0], 240.0);
        let (mut x, mut y) = (0.0f32, 0.0f32);
        for rec in &t.records {
            if let Some(fid) = rec.font_id {
                font = lib.font_for(r.movie, Some(fid), None);
            }
            if let Some(c) = rec.color {
                color = crate::geom::rgba(c);
            }
            if let Some(h) = rec.height {
                height = f32::from(h);
            }
            if let Some(dx) = rec.x_offset {
                x = f32::from(dx);
            }
            if let Some(dy) = rec.y_offset {
                y = f32::from(dy);
            }
            let Some(f) = font.clone() else { continue };
            for &(g, adv) in &rec.glyphs {
                let glyph = TextGlyph { font: &f, index: g as usize, size: height, at: (x, y), color };
                self.text_glyph(&glyph, &base, &here, param.as_ref(), &mut layers);
                x += adv as f32;
            }
        }
        layers.emit(out);
    }

    fn text_glyph(
        &mut self,
        g: &TextGlyph,
        node: &Mat2d,
        here: &Inherited,
        param: Option<&TextFieldParam>,
        layers: &mut TextLayers,
    ) {
        use crate::glyph::{CachedGlyph, height_ratio, snapped_size};
        let ratio = height_ratio(node);
        let screen = g.size * ratio;
        let shadow = param.and_then(|p| p.shadow);
        if let Some(sh) = shadow.filter(ShadowParam::drawn) {
            let ss = shadow_size(screen);
            if let CachedGlyph::Raster(r) = self.glyphs.filtered(g.font, g.index, ss, sh.filter, ratio, screen) {
                let pen = node.mul(&Mat2d::translate(g.at.0 + sh.offset.0, g.at.1 + sh.offset.1));
                layers.shadow.push(raster_draw(&r, ss, g.size, &pen, sh.color, here));
            }
        }
        if shadow.is_some_and(|s| s.filter.flags & GlyphFilter::KNOCKOUT != 0) {
            return;
        }
        let fs = snapped_size(screen);
        let pen = node.mul(&Mat2d::translate(g.at.0, g.at.1));
        let cached = match param.map(|p| p.text).filter(GlyphFilter::blurred) {
            Some(f) => self.glyphs.filtered(g.font, g.index, fs, f, ratio, screen),
            None => self.glyphs.glyph(g.font, g.index, fs),
        };
        match cached {
            CachedGlyph::Raster(r) => layers.raster.push(raster_draw(&r, fs, g.size, &pen, g.color, here)),
            CachedGlyph::Empty => {}
            CachedGlyph::Vector => {
                let k = g.size / crate::text::em_units(g.font);
                let xf = pen.mul(&Mat2d::scale(k, k));
                let mesh = self.glyph(g.font, g.index, xf.max_scale());
                for layer in &mesh.layers {
                    layers.vector.push(Command::Draw(DrawItem {
                        mesh: layer.mesh.clone(),
                        paint: Paint::Solid(g.color),
                        transform: xf,
                        cxform: here.cx,
                        blend: here.blend,
                    }));
                }
            }
        }
    }

    pub fn cached(&self) -> usize {
        self.meshes.len()
    }

    pub fn cached_vertices(&self) -> u64 {
        self.cached_vertices
    }

    pub fn tessellate_all(&mut self, lib: &Library, movie: crate::library::MovieId, scale: f32) -> (usize, usize) {
        let tol = Tolerance::for_scale(scale);
        let (mut n, mut tris) = (0, 0);
        let m = lib.movie(movie);
        let mut ids: Vec<u16> = m.chars.keys().copied().collect();
        ids.sort();
        for id in ids {
            let mut budget;
            let mesh = match &m.chars[&id] {
                Character::Shape(s) => {
                    budget = Budget::shape();
                    shape::build(
                        lib,
                        movie,
                        &s.shape.styles,
                        &s.shape.records,
                        s.uses_fill_winding_rule,
                        &tol,
                        &mut budget,
                    )
                }
                Character::Morph(ms) => {
                    let (a, ra) = shape::morph_at(ms, 0.0);
                    let (b, rb) = shape::morph_at(ms, 1.0);
                    budget = Budget::shape();
                    shape::build(lib, movie, &a, &ra, false, &tol, &mut budget).and_then(|mut mesh| {
                        budget = Budget::shape();
                        mesh.layers.extend(shape::build(lib, movie, &b, &rb, false, &tol, &mut budget)?.layers);
                        Ok(mesh)
                    })
                }
                _ => continue,
            };
            self.tess_stats.shape_peak.peak(budget.used());
            let Ok(mesh) = mesh else {
                self.tess_stats.refused += 1;
                continue;
            };
            n += 1;
            tris += mesh.triangle_count();
        }
        (n, tris)
    }
}

struct TextGlyph<'a> {
    font: &'a Arc<Font>,
    index: usize,
    size: f32,
    at: (f32, f32),
    color: [f32; 4],
}

fn raster_draw(
    r: &crate::glyph::RasterGlyph,
    class: u16,
    size: f32,
    pen: &Mat2d,
    color: [f32; 4],
    here: &Inherited,
) -> Command {
    let k = size / (f32::from(class) * 0.0625).max(1e-5) * r.scale;
    Command::Draw(DrawItem {
        mesh: r.quad.clone(),
        paint: Paint::Glyph { raster: r.raster.clone(), rect: r.rect, color },
        transform: pen.mul(&Mat2d::scale(k, k)),
        cxform: here.cx,
        blend: here.blend,
    })
}

fn bitmap_quad(b: crate::library::BitmapDef) -> ShapeMesh {
    let (w, h) = (b.size[0] as f32 * 20.0, b.size[1] as f32 * 20.0);
    let mesh = Mesh {
        vertices: vec![[0.0, 0.0], [w, 0.0], [w, h], [0.0, h]],
        indices: vec![0, 1, 2, 0, 2, 3],
        paint: Vec::new(),
    };
    let paint = Paint::Bitmap {
        bitmap: b,
        matrix: Mat2d::scale(20.0, 20.0).mul(&b.texel_to_pixel()),
        repeat: false,
        smooth: true,
    };
    ShapeMesh { layers: vec![shape::ShapeLayer { paint, mesh: Arc::new(mesh) }] }
}

const LEADERBOARD_MOVIES: [&str; 2] = ["components/hud/leaderboard.gfx", "components/hud/dynamicleaderboard.gfx"];

// PORT(choice): for windows wider than 16:9 — the standings components place themselves outside `RepositionIfSD`, so
// their topmost clip is shifted right by the window's extra stage width when drawn, as the HUD's `RepositionIfSD` side
// panels are moved out; the console's displays are 16:9 or 4:3.
fn is_leaderboard_movie(lib: &Library, inst: &Instance) -> bool {
    LEADERBOARD_MOVIES.contains(&lib.movie(inst.movie).path.as_str())
}

fn parent_to_root(player: &Player, inst: &Instance) -> Option<Mat2d> {
    let mut up = Mat2d::IDENTITY;
    let mut cur = inst.parent;
    let mut steps = 0;
    while let Some(c) = cur {
        let i = player.get(c)?;
        up = i.matrix.mul(&up);
        cur = i.parent;
        steps += 1;
        if steps > 64 {
            return None;
        }
    }
    Some(up)
}

fn wide_leaderboard(lib: &Library, player: &Player, inst: &Instance) -> Option<Mat2d> {
    let extra = player.hud_extra as f32;
    if extra < 0.5 {
        return None;
    }
    if inst.parent.and_then(|p| player.get(p)).is_some_and(|p| p.movie == inst.movie) {
        return None;
    }
    if !is_leaderboard_movie(lib, inst) {
        return None;
    }
    let mut cur = inst.parent;
    while let Some(c) = cur {
        let i = player.get(c)?;
        if is_leaderboard_movie(lib, i) {
            return None;
        }
        cur = i.parent;
    }
    let up = parent_to_root(player, inst)?;
    let det = up.determinant();
    if det.abs() < 1e-9 {
        return None;
    }
    let dx = extra * 20.0;
    let m = inst.matrix;
    Some(Mat2d { tx: m.tx + up.d * dx / det, ty: m.ty - up.b * dx / det, ..m })
}

fn is_side_dimmer(lib: &Library, player: &Player, inst: &Instance) -> bool {
    inst.character.as_ref().is_some_and(|c| c.id == 2)
        && inst.parent.and_then(|p| player.get(p)).is_some_and(|p| p.name == "mcDimmer")
        && lib.movie(inst.movie).path.to_ascii_lowercase().rsplit(['/', '\\']).next() == Some("dimmer.gfx")
}

impl Renderer {
    // PORT(choice): for windows wider than 16:9 — where the side dimmer's gradient reaches the stage's left or right
    // edge, its padded end colour is drawn on out to the window's edge (the same gradient paint over a quad beyond the
    // shape's bounds), so the dimming does not stop in a hard edge beside the 16:9 stage; the stage's own pixels are
    // unchanged; the console's displays are 16:9 or 4:3.
    fn side_dimmer_edges(
        &mut self,
        lib: &Library,
        player: &Player,
        inst: &Instance,
        matrix: &Mat2d,
        here: &Inherited,
        out: &mut Vec<Command>,
    ) {
        const STAGE_WIDTH_TWIPS: f32 = 1280.0 * 20.0;
        let Some(up) = parent_to_root(player, inst) else { return };
        let to_root = up.mul(matrix);
        let window = (-player.hud_extra as f32 * 20.0, STAGE_WIDTH_TWIPS + player.hud_extra as f32 * 20.0);
        let InstanceKind::Sprite(s) = &inst.kind else { return };
        for &child in s.children.values() {
            let Some(c) = player.get(child) else { continue };
            if !c.visible || !matches!(c.kind, InstanceKind::Shape) {
                continue;
            }
            let full = to_root.mul(&c.matrix);
            if full.b.abs() > 1e-6 || full.c.abs() > 1e-6 || full.a.abs() <= 1e-6 {
                continue;
            }
            let b = player.local_bounds(lib, child);
            if b.is_empty() {
                continue;
            }
            let xf = here.xf.mul(&c.matrix);
            let Some(mesh) = self.character_mesh(lib, c, &xf) else { continue };
            let (sx0, sx1) = (full.a * b.x0 + full.tx, full.a * b.x1 + full.tx);
            let (left, right) = (sx0.min(sx1), sx0.max(sx1));
            let to_local = |x: f32| (x - full.tx) / full.a;
            let span = |a: f32, b: f32| (to_local(a).min(to_local(b)), to_local(a).max(to_local(b)));
            let mut spans = Vec::new();
            if left <= 0.5 && right > 0.5 && window.0 < left {
                spans.push(span(window.0, left));
            }
            if right >= STAGE_WIDTH_TWIPS - 0.5 && left < STAGE_WIDTH_TWIPS - 0.5 && window.1 > right {
                spans.push(span(right, window.1));
            }
            let blend = if matches!(c.blend, BlendMode::Normal | BlendMode::Layer) { here.blend } else { c.blend };
            for layer in &mesh.layers {
                let shape::Paint::Gradient(g) = &layer.paint else { continue };
                if g.spread != 0 {
                    continue;
                }
                for &(x0, x1) in &spans {
                    let quad = Mesh {
                        vertices: vec![[x0, b.y0], [x1, b.y0], [x1, b.y1], [x0, b.y1]],
                        indices: vec![0, 1, 2, 0, 2, 3],
                        paint: Vec::new(),
                    };
                    out.push(Command::Draw(DrawItem {
                        mesh: Arc::new(quad),
                        paint: layer.paint.clone(),
                        transform: xf,
                        cxform: here.cx.concat(&c.cxform),
                        blend,
                    }));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::display::tests::{empty_lib, movie, po};
    use crate::library::FrameTag;

    #[test]
    fn a_padded_image_maps_the_whole_file_onto_its_target() {
        let b = crate::library::BitmapDef { image: 0, rect: [0, 0, 248, 464], size: [248, 461] };
        let mesh = bitmap_quad(b);
        let l = &mesh.layers[0];
        assert_eq!(l.mesh.vertices[2], [248.0 * 20.0, 461.0 * 20.0]);
        let Paint::Bitmap { matrix, .. } = &l.paint else { panic!("a bitmap paint") };
        let (u, v) = matrix.inverse().expect("invertible").apply(248.0 * 20.0, 461.0 * 20.0);
        assert!((u - 248.0).abs() < 1e-3 && (v - 464.0).abs() < 1e-3, "{u} {v}");
        assert_eq!(crate::library::BitmapDef::of_rect(0, [2, 3, 12, 23]).size, [10, 20]);
    }

    #[test]
    fn clip_layers_bracket_their_depth_range() {
        let mut lib = empty_lib();
        let mut mask = po(1, Some(1), false, 0);
        mask.clip_depth = Some(3);
        mask.flags |= 0x40;
        let m = movie(
            &mut lib,
            vec![vec![
                FrameTag::Place(Arc::new(mask)),
                FrameTag::Place(Arc::new(po(2, Some(2), false, 0))),
                FrameTag::Place(Arc::new(po(5, Some(2), false, 0))),
            ]],
        );
        let mut player = Player::new();
        player.load_level(&lib, m, 0);
        let list = Renderer::new().build(&lib, &player, (1280, 720), ScaleMode::NoBorder);
        let kinds: Vec<&str> = list
            .commands
            .iter()
            .map(|c| match c {
                Command::Draw(_) => "draw",
                Command::MaskBegin => "begin",
                Command::MaskEnd => "end",
                Command::MaskPopBegin => "pop",
                Command::MaskPopEnd => "popend",
            })
            .collect();
        assert_eq!(kinds, ["begin", "draw", "end", "draw", "pop", "draw", "popend", "draw"]);
    }

    #[test]
    fn the_side_dimmer_reaches_the_window_edge_only_in_a_wider_window() {
        use crate::library::{FrameDef, MovieDef, TimelineDef};
        use ssx_formats::gfx::shape::{DefineShape, FillStyle, Gradient, Shape, ShapeRecord, Styles};
        use ssx_formats::gfx::types::{Matrix, Rect, Rgba};
        let (w, h) = (436 * 20, 728 * 20);
        let gradient = FillStyle::Gradient {
            kind: 0x10,
            matrix: Matrix { a: -0.2661, d: -0.4443, tx: 218 * 20, ..Matrix::IDENTITY },
            gradient: Gradient {
                spread: 0,
                interpolation: 0,
                records: vec![(29, Rgba { r: 0x4E, g: 0x9F, b: 0xB1, a: 0 }), (214, Rgba { r: 0, g: 0, b: 0, a: 255 })],
                focal: None,
            },
        };
        let records = vec![
            ShapeRecord::StyleChange {
                move_to: Some((0, 0)),
                fill0: None,
                fill1: Some(1),
                line: None,
                new_styles: None,
            },
            ShapeRecord::Straight { dx: w, dy: 0 },
            ShapeRecord::Straight { dx: 0, dy: h },
            ShapeRecord::Straight { dx: -w, dy: 0 },
            ShapeRecord::Straight { dx: 0, dy: -h },
        ];
        let shape = DefineShape {
            version: 3,
            id: 1,
            bounds: Rect { x_min: 0, x_max: w, y_min: 0, y_max: h },
            edge_bounds: None,
            unk_reserved: 0,
            uses_fill_winding_rule: false,
            uses_non_scaling_strokes: false,
            uses_scaling_strokes: false,
            shape: Shape { styles: Styles { fills: vec![gradient], lines: Vec::new() }, records },
        };
        let build = |tx: i32, extra: f64| {
            let mut lib = empty_lib();
            let id = lib.movies().len();
            let frame = |tags: Vec<FrameTag>| vec![FrameDef { tags, ..Default::default() }];
            let sprite =
                |sid: u16, tags| Character::Sprite(Arc::new(TimelineDef { movie: id, id: sid, frames: frame(tags) }));
            let mut chars = HashMap::new();
            chars.insert(1, Character::Shape(Arc::new(shape.clone())));
            chars.insert(2, sprite(2, vec![FrameTag::Place(Arc::new(po(1, Some(1), false, 0)))]));
            chars.insert(7, sprite(7, vec![FrameTag::Place(Arc::new(po(1, Some(2), false, tx)))]));
            let named =
                ssx_formats::gfx::place::PlaceObject { name: Some("mcDimmer".into()), ..po(1, Some(7), false, 0) };
            let m = lib.insert_movie(MovieDef {
                path: "views/dimmer.gfx".into(),
                dir: String::new(),
                version: 8,
                frame_rate: 30.0,
                stage: RectF::new(0.0, 0.0, 25600.0, 14400.0),
                background: None,
                swf_name: "Dimmer".into(),
                root: Arc::new(TimelineDef { movie: id, id: 0, frames: frame(vec![FrameTag::Place(Arc::new(named))]) }),
                chars,
                exports: HashMap::new(),
                imports: HashMap::new(),
                scaling_grids: HashMap::new(),
            });
            let mut player = Player::new();
            player.load_level(&lib, m, 0);
            player.hud_extra = extra;
            let list = Renderer::new().build(&lib, &player, (1280, 720), ScaleMode::NoBorder);
            list.commands
                .iter()
                .filter_map(|c| match c {
                    Command::Draw(d) => Some(
                        d.mesh
                            .vertices
                            .iter()
                            .map(|v| v[0])
                            .fold((f32::MAX, f32::MIN), |(lo, hi), x| (lo.min(x), hi.max(x))),
                    ),
                    _ => None,
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(build(0, 0.0).len(), 1, "16:9: the shape alone");
        let wide = build(0, 100.0);
        assert_eq!(wide.len(), 2, "wider: the shape and its left end colour");
        assert_eq!(wide[1], (-2000.0, 0.0), "from the window's edge (100 px beside the stage) to the shape");
        assert_eq!(
            build(-50 * 20, 100.0)[1],
            (-1000.0, 0.0),
            "slid 50 px out: from the window's edge to its left side"
        );
        assert_eq!(build(-300 * 20, 100.0).len(), 1, "slid past the window's edge: it covers the extra width itself");
        assert_eq!(build(-500 * 20, 100.0).len(), 1, "wholly beside the stage: unchanged");
    }
}
