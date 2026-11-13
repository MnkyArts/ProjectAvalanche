pub mod fade;
pub mod globe;
pub mod gpu;
pub mod movie;

use std::collections::HashMap;
use std::sync::Arc;

use bevy::prelude::*;
use bevy::render::RenderApp;
use bevy::render::extract_component::{ExtractComponent, ExtractComponentPlugin};
use bevy::render::extract_resource::{ExtractResource, ExtractResourcePlugin};
use ssx_ui::display::{BlendMode, Player};
use ssx_ui::geom::Mat2d;
use ssx_ui::glyph::GlyphAtlas;
use ssx_ui::image::ImageData;
use ssx_ui::library::{ImageId, Library};
use ssx_ui::render::{Command, DrawItem, RenderList, Renderer, ScaleMode};
use ssx_ui::shape::{GradientKind, GradientPaint, Paint};

pub const GRADIENT_ROWS: usize = 256;
pub const DRAW_FLOATS: usize = 32;

#[derive(SystemSet, Clone, Debug, PartialEq, Eq, Hash)]
pub struct UiPassSystems;

#[derive(Component, Clone, Copy, Default, ExtractComponent)]
#[extract_app(RenderApp)]
pub struct UiCamera;

#[derive(Resource)]
pub struct UiScene {
    pub lib: Library,
    pub player: Player,
    pub vm: Option<ssx_ui::avm1::Vm>,
    pub renderer: Renderer,
    pub playing: bool,
    pub scale_mode: ScaleMode,
    pub clear: Option<[f32; 3]>,
    pub hidden: bool,
    pub window_hidden: bool,
    pub last_cpu: std::time::Duration,
    glyph_atlas: Option<Arc<GlyphAtlas>>,
    opaque: ssx_ui::render::cover::OpaqueBitmaps,
}

impl UiScene {
    pub fn new(lib: Library, player: Player) -> Self {
        UiScene {
            lib,
            player,
            vm: None,
            renderer: Renderer::new(),
            playing: false,
            scale_mode: ScaleMode::NoBorder,
            clear: None,
            hidden: false,
            window_hidden: false,
            last_cpu: std::time::Duration::ZERO,
            glyph_atlas: None,
            opaque: Default::default(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum UiBlend {
    Normal,
    Add,
    Multiply,
    Subtract,
    Lighten,
    Darken,
}

impl UiBlend {
    pub const ALL: [UiBlend; 6] =
        [UiBlend::Normal, UiBlend::Add, UiBlend::Multiply, UiBlend::Subtract, UiBlend::Lighten, UiBlend::Darken];

    fn from_mode(b: BlendMode) -> Option<UiBlend> {
        Some(match b {
            BlendMode::Add => UiBlend::Add,
            BlendMode::Multiply => UiBlend::Multiply,
            BlendMode::Subtract => UiBlend::Subtract,
            BlendMode::Lighten => UiBlend::Lighten,
            BlendMode::Darken => UiBlend::Darken,
            BlendMode::Alpha | BlendMode::Erase => return None,
            _ => UiBlend::Normal,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum UiTexture {
    None,
    Image { image: ImageId, smooth: bool },
    Gradients,
    Glyphs,
}

#[derive(Clone, Copy, Debug)]
pub enum UiOp {
    Draw { draw: u32, first_index: u32, index_count: u32, base_vertex: i32, texture: UiTexture, blend: UiBlend },
    MaskBegin,
    MaskEnd,
    MaskPopBegin,
    MaskPopEnd,
}

#[derive(Resource, Clone, Default, ExtractResource)]
#[extract_app(RenderApp)]
pub struct UiFrame {
    pub viewport: UVec2,
    pub vertices: Arc<Vec<[f32; 2]>>,
    pub paint_coords: Arc<Vec<[f32; 2]>>,
    pub draw_ids: Arc<Vec<u32>>,
    pub indices: Arc<Vec<u32>>,
    pub draws: Arc<Vec<[f32; DRAW_FLOATS]>>,
    pub ops: Arc<Vec<UiOp>>,
    pub gradients: Arc<Vec<[u8; 4]>>,
    pub gradient_rows_used: u32,
    pub images: Arc<Vec<Arc<ImageData>>>,
    pub glyphs: Option<Arc<GlyphAtlas>>,
    pub clear: Option<[f32; 3]>,
}

impl UiFrame {
    pub fn triangle_count(&self) -> usize {
        let count = |ops: &[UiOp]| -> usize {
            ops.iter()
                .map(|o| match o {
                    UiOp::Draw { index_count, .. } => *index_count as usize / 3,
                    _ => 0,
                })
                .sum()
        };
        count(&self.ops)
    }
}

#[derive(Resource, Default)]
pub struct UiOverlay {
    pub draws: Vec<ssx_ui::render::overlay::OverlayDraw>,
    pub images: Vec<Arc<ImageData>>,
    pub font: String,
    pub cpu: std::time::Duration,
}

struct FrameBuilder<'a> {
    lib: &'a Library,
    extra: &'a [Arc<ImageData>],
    vertices: Vec<[f32; 2]>,
    paint_coords: Vec<[f32; 2]>,
    draw_ids: Vec<u32>,
    indices: Vec<u32>,
    draws: Vec<[f32; DRAW_FLOATS]>,
    gradients: Vec<[u8; 4]>,
    gradient_rows: HashMap<*const GradientPaint, u32>,
    glyph_size: (u32, u32),
}

impl FrameBuilder<'_> {
    fn mesh(&mut self, vertices: &[[f32; 2]], paint: &[[f32; 2]], indices: &[u32], draw: u32) -> (u32, u32) {
        let base = self.vertices.len() as u32;
        let r = (self.indices.len() as u32, indices.len() as u32);
        self.vertices.extend_from_slice(vertices);
        self.paint_coords.extend_from_slice(if paint.is_empty() { vertices } else { paint });
        self.draw_ids.resize(self.vertices.len(), draw);
        self.indices.extend(indices.iter().map(|i| i + base));
        r
    }

    fn gradient_row(&mut self, g: &Arc<GradientPaint>) -> u32 {
        let key = Arc::as_ptr(g);
        if let Some(&r) = self.gradient_rows.get(&key) {
            return r;
        }
        let row = (self.gradient_rows.len()).min(GRADIENT_ROWS - 1) as u32;
        let ramp = g.ramp();
        let at = row as usize * 256;
        self.gradients[at..at + 256].copy_from_slice(&ramp);
        self.gradient_rows.insert(key, row);
        row
    }

    fn draw(&mut self, d: &DrawItem, target: Vec2) -> (u32, UiTexture) {
        let xf = d.transform;
        let (inv, kind, rect, tex, texture) = match &d.paint {
            Paint::Solid(c) => (Mat2d::IDENTITY, 0.0, *c, [1.0, 1.0], UiTexture::None),
            Paint::Gradient(g) => {
                let row = self.gradient_row(g);
                let (kind, focal) = match g.kind {
                    GradientKind::Linear => (1.0, 0.0),
                    GradientKind::Radial => (2.0, 0.0),
                    GradientKind::Focal(f) => (3.0, f),
                };
                let v = (row as f32 + 0.5) / GRADIENT_ROWS as f32;
                let inv = g.matrix.inverse().unwrap_or(Mat2d::scale(0.0, 0.0));
                (inv, kind, [v, focal, f32::from(g.spread), 0.0], [256.0, GRADIENT_ROWS as f32], UiTexture::Gradients)
            }
            Paint::Bitmap { bitmap, matrix, repeat, smooth } => {
                let n = self.lib.images().len();
                let img = match bitmap.image.checked_sub(n) {
                    Some(k) => self.extra.get(k),
                    None => Some(self.lib.image(bitmap.image)),
                };
                match img {
                    Some(img) => {
                        let r = bitmap.rect.map(|v| v as f32);
                        let inv = matrix.inverse().unwrap_or(Mat2d::scale(0.0, 0.0));
                        let kind = if *repeat { 5.0 } else { 4.0 };
                        let tex = [img.width as f32, img.height as f32];
                        (inv, kind, r, tex, UiTexture::Image { image: bitmap.image, smooth: *smooth })
                    }
                    None => (Mat2d::IDENTITY, 0.0, [0.0; 4], [1.0, 1.0], UiTexture::None),
                }
            }
            Paint::Glyph { raster, rect, .. } => {
                let inv = Mat2d::translate(-(raster.x0 as f32), -(raster.y0 as f32));
                let (w, h) = self.glyph_size;
                (inv, 7.0, rect.map(|v| v as f32), [w as f32, h as f32], UiTexture::Glyphs)
            }
        };
        let colour = match &d.paint {
            Paint::Glyph { color, .. } => *color,
            _ => [1.0; 4],
        };
        let lerp_white = matches!(UiBlend::from_mode(d.blend), Some(UiBlend::Multiply | UiBlend::Darken));
        let multiply = if lerp_white { 1.0 } else { 0.0 };
        let c = &d.cxform;
        let rec: [f32; DRAW_FLOATS] = [
            xf.a,
            xf.b,
            xf.c,
            xf.d,
            xf.tx,
            xf.ty,
            target.x,
            target.y,
            c.mult[0] * colour[0],
            c.mult[1] * colour[1],
            c.mult[2] * colour[2],
            c.mult[3] * colour[3],
            c.add[0],
            c.add[1],
            c.add[2],
            c.add[3],
            inv.a,
            inv.b,
            inv.c,
            inv.d,
            inv.tx,
            inv.ty,
            kind,
            multiply,
            rect[0],
            rect[1],
            rect[2],
            rect[3],
            tex[0],
            tex[1],
            0.0,
            0.0,
        ];
        self.draws.push(rec);
        (self.draws.len() as u32 - 1, texture)
    }

    fn op(&mut self, d: &DrawItem, target: Vec2) -> Option<UiOp> {
        let blend = UiBlend::from_mode(d.blend)?;
        let (draw, texture) = self.draw(d, target);
        let (first_index, index_count) = self.mesh(&d.mesh.vertices, &d.mesh.paint, &d.mesh.indices, draw);
        Some(UiOp::Draw { draw, first_index, index_count, base_vertex: 0, texture, blend })
    }
}

pub fn flatten(lib: &Library, list: &RenderList, glyphs: Option<Arc<GlyphAtlas>>, extra: &[Arc<ImageData>]) -> UiFrame {
    let mut b = FrameBuilder {
        lib,
        extra,
        vertices: Vec::new(),
        paint_coords: Vec::new(),
        draw_ids: Vec::new(),
        indices: Vec::new(),
        draws: Vec::new(),
        gradients: vec![[0; 4]; GRADIENT_ROWS * 256],
        gradient_rows: HashMap::new(),
        glyph_size: glyphs.as_ref().map_or((1, 1), |a| (a.width, a.height)),
    };
    let vp = Vec2::new(list.viewport.0 as f32, list.viewport.1 as f32);
    let mut ops = Vec::new();
    for cmd in &list.commands {
        match cmd {
            Command::Draw(d) => ops.extend(b.op(d, vp)),
            Command::MaskBegin => ops.push(UiOp::MaskBegin),
            Command::MaskEnd => ops.push(UiOp::MaskEnd),
            Command::MaskPopBegin => ops.push(UiOp::MaskPopBegin),
            Command::MaskPopEnd => ops.push(UiOp::MaskPopEnd),
        }
    }
    UiFrame {
        viewport: UVec2::new(list.viewport.0, list.viewport.1),
        gradient_rows_used: b.gradient_rows.len() as u32,
        vertices: Arc::new(b.vertices),
        paint_coords: Arc::new(b.paint_coords),
        draw_ids: Arc::new(b.draw_ids),
        indices: Arc::new(b.indices),
        draws: Arc::new(b.draws),
        ops: Arc::new(ops),
        gradients: Arc::new(b.gradients),
        images: Arc::new(lib.images().iter().chain(extra).cloned().collect()),
        glyphs,
        clear: None,
    }
}

pub struct UiPlugin;

impl Plugin for UiPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((ExtractResourcePlugin::<UiFrame>::default(), ExtractComponentPlugin::<UiCamera>::default()))
            .init_resource::<UiFrame>()
            .add_systems(PostUpdate, build_frame);
        gpu::build(app);
        fade::build(app);
    }
}

fn build_frame(
    time: Res<Time>,
    scene: Option<ResMut<UiScene>>,
    cameras: Query<&Camera, With<UiCamera>>,
    mut frame: ResMut<UiFrame>,
    overlay: Option<ResMut<UiOverlay>>,
) {
    let Some(mut scene) = scene else { return };
    let Some(size) = cameras.iter().find_map(|c| c.physical_viewport_size()) else { return };
    let t0 = std::time::Instant::now();
    let scene = &mut *scene;
    if scene.playing {
        match scene.vm.as_mut() {
            Some(vm) => drop(vm.advance(&mut scene.lib, &mut scene.player, time.delta_secs_f64())),
            None => drop(scene.player.tick(&scene.lib, time.delta_secs_f64())),
        }
    }
    if scene.window_hidden {
        scene.last_cpu = t0.elapsed();
        return;
    }
    let mut list = if scene.hidden {
        RenderList { viewport: (size.x, size.y), ..Default::default() }
    } else {
        let list = scene.renderer.build(&scene.lib, &scene.player, (size.x, size.y), scene.scale_mode);
        for note in scene.renderer.take_refused() {
            warn!("ui: a shape not drawn: {note}");
        }
        list
    };
    // PORT(choice): beside a stage that a screen covers opaquely (the title's backdrop) a wider window shows black, not
    // the 3-D layer under the screen (the globe), which the game's 16:9 picture never showed under such a screen; black
    // is what the title's backdrop ends in at both edges (at 1920 x 1080: (0, 0, 0) to (0, 1, 2)); the game drew
    // nothing beside its 16:9 stage.
    // PORT(choice): a colour or gradient backdrop over the whole stage (the pause's and the results' dimmers) reaches
    // the window's edges, so the side is dimmed as the stage is; the game drew nothing beside its stage.
    let beside = list.shows_beside_stage();
    let hidden_beside = beside && list.covers_stage(&scene.lib, &mut scene.opaque);
    if beside {
        list.extend_backdrops();
    }
    let mut overlay = overlay;
    let extra = match overlay.as_deref_mut() {
        Some(o) if !scene.hidden && !o.draws.is_empty() => {
            let t = std::time::Instant::now();
            let viewport = (size.x, size.y);
            let stage = ssx_ui::render::overlay::stage_transform(viewport, scene.scale_mode);
            let base = scene.lib.images().len();
            let more = scene.renderer.overlay(&scene.lib, &o.font, &o.draws, stage, viewport, base);
            list.commands.extend(more);
            o.cpu = t.elapsed();
            o.images.clone()
        }
        _ => Vec::new(),
    };
    let atlas = &scene.renderer.glyphs.atlas;
    if scene.glyph_atlas.as_ref().is_none_or(|a| a.generation != atlas.generation) {
        scene.glyph_atlas = Some(Arc::new(atlas.clone()));
    }
    *frame = flatten(&scene.lib, &list, scene.glyph_atlas.clone(), &extra);
    frame.clear = scene.clear.or(hidden_beside.then_some([0.0; 3]));
    scene.last_cpu = t0.elapsed();
}

#[cfg(test)]
mod tests {
    use super::*;
    use ssx_ui::geom::Cxform;
    use ssx_ui::tess::Mesh;

    struct NoFiles;

    impl ssx_ui::fs::FileSource for NoFiles {
        fn read(&self, _: &str) -> Option<Vec<u8>> {
            None
        }

        fn exists(&self, _: &str) -> bool {
            false
        }

        fn list(&self, _: &str, _: &str) -> Vec<String> {
            Vec::new()
        }

        fn image(&self, path: &str) -> anyhow::Result<ssx_ui::image::ImageData> {
            anyhow::bail!("{path}: no files")
        }
    }

    fn draw(mesh: &Arc<Mesh>, blend: BlendMode) -> Command {
        Command::Draw(DrawItem {
            mesh: mesh.clone(),
            paint: Paint::Solid([1.0, 0.0, 0.0, 1.0]),
            transform: Mat2d::IDENTITY,
            cxform: Cxform::default(),
            blend,
        })
    }

    #[test]
    fn every_draw_has_its_own_vertices_its_record_and_absolute_indices() {
        let lib = Library::new(Box::new(NoFiles));
        let quad = Arc::new(Mesh {
            vertices: vec![[0.0, 0.0], [20.0, 0.0], [20.0, 20.0], [0.0, 20.0]],
            indices: vec![0, 1, 2, 0, 2, 3],
            paint: Vec::new(),
        });
        let list = RenderList {
            commands: vec![
                draw(&quad, BlendMode::Normal),
                draw(&quad, BlendMode::Normal),
                Command::MaskBegin,
                draw(&quad, BlendMode::Normal),
                Command::MaskEnd,
                draw(&quad, BlendMode::Add),
            ],
            viewport: (64, 64),
            background: None,
            stage: None,
        };
        let f = flatten(&lib, &list, None, &[]);
        assert_eq!(f.vertices.len(), 16);
        assert_eq!(f.paint_coords.len(), 16);
        assert_eq!(*f.draw_ids, [0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3]);
        let expected: Vec<u32> = (0..4u32).flat_map(|k| [0, 1, 2, 0, 2, 3].map(|i| i + 4 * k)).collect();
        assert_eq!(*f.indices, expected);
        let draws: Vec<(u32, u32, u32, i32)> = f
            .ops
            .iter()
            .filter_map(|o| match *o {
                UiOp::Draw { draw, first_index, index_count, base_vertex, .. } => {
                    Some((draw, first_index, index_count, base_vertex))
                }
                _ => None,
            })
            .collect();
        assert_eq!(draws, [(0, 0, 6, 0), (1, 6, 6, 0), (2, 12, 6, 0), (3, 18, 6, 0)]);
        assert!(matches!(f.ops[2], UiOp::MaskBegin) && matches!(f.ops[4], UiOp::MaskEnd));
    }
}
