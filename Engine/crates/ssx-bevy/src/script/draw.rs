use std::collections::{BTreeMap, VecDeque};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};

use bevy::prelude::*;
use ssx_content::ContentId;
use ssx_script::host::game::{Align, Draw};
use ssx_ui::image::ImageData;
use ssx_ui::render::overlay::{self, OverlayDraw};
use ssx_ui::text;

use crate::ui::{UiOverlay, UiScene};

const MAX_IMAGES: usize = 32;
const MAX_IMAGE_BYTES: usize = 128 << 20;
const MAX_SIDE: u32 = 2048;
const MAX_LOADING: usize = 4;
pub(super) const FONT: &str = "DIN Mittelschrift Std";

type Loaded = (ContentId, Result<ImageData, String>);

#[derive(Resource)]
pub(crate) struct DrawPool {
    pending: Option<Vec<(String, Draw)>>,
    list: Vec<(String, Draw)>,
    images: BTreeMap<ContentId, Arc<ImageData>>,
    order: VecDeque<ContentId>,
    bytes: usize,
    loading: BTreeMap<ContentId, ()>,
    failed: BTreeMap<ContentId, String>,
    tx: Sender<Loaded>,
    rx: Mutex<Receiver<Loaded>>,
    pub(super) content: Option<Arc<ssx_assets::content::GameContent>>,
}

impl Default for DrawPool {
    fn default() -> Self {
        let (tx, rx) = channel();
        DrawPool {
            pending: None,
            list: Vec::new(),
            images: BTreeMap::new(),
            order: VecDeque::new(),
            bytes: 0,
            loading: BTreeMap::new(),
            failed: BTreeMap::new(),
            tx,
            rx: Mutex::new(rx),
            content: None,
        }
    }
}

impl DrawPool {
    pub(super) fn set(&mut self, draws: Vec<(String, Draw)>) {
        self.pending = Some(draws);
    }

    #[cfg(test)]
    pub(super) fn pending(&self) -> Option<usize> {
        self.pending.as_ref().map(Vec::len)
    }

    pub(super) fn image(&mut self, id: &ContentId) -> Option<(usize, [u32; 2])> {
        if let Some(img) = self.images.get(id) {
            let k = self.order.iter().position(|o| o == id)?;
            return Some((k, [img.width, img.height]));
        }
        if self.failed.contains_key(id) || self.loading.contains_key(id) || self.loading.len() >= MAX_LOADING {
            return None;
        }
        let content = self.content.clone()?;
        let (tx, for_thread) = (self.tx.clone(), id.clone());
        let spawned = std::thread::Builder::new().name("script-image".into()).spawn(move || {
            let r = load_image(&content, &for_thread);
            let _ = tx.send((for_thread, r));
        });
        match spawned {
            Ok(_) => _ = self.loading.insert(id.clone(), ()),
            Err(e) => {
                let why = format!("no image loader thread ({e})");
                warn!("scripts: the image {id} cannot be drawn: {why}");
                self.failed.insert(id.clone(), why);
            }
        }
        None
    }

    fn receive(&mut self) {
        while let Some((id, got)) = self.rx.lock().ok().and_then(|r| r.try_recv().ok()) {
            self.loading.remove(&id);
            match got {
                Ok(image) => {
                    let size = image.rgba.len();
                    while (self.images.len() >= MAX_IMAGES || self.bytes + size > MAX_IMAGE_BYTES)
                        && let Some(old) = self.order.pop_front()
                    {
                        if let Some(h) = self.images.remove(&old) {
                            self.bytes = self.bytes.saturating_sub(h.rgba.len());
                        }
                    }
                    self.bytes += size;
                    self.order.push_back(id.clone());
                    self.images.insert(id, Arc::new(image));
                }
                Err(e) => {
                    warn!("scripts: the image {id} cannot be drawn: {e}");
                    self.failed.insert(id, e);
                }
            }
        }
    }
}

fn load_image(content: &ssx_assets::content::GameContent, id: &ContentId) -> Result<ImageData, String> {
    let found = content.texture(id, &ssx_content::texture::Want::PLANE).map_err(|e| e.to_string())?;
    let t = &found.texture;
    let (w, h) = (t.width(), t.height());
    if w == 0 || h == 0 || w > MAX_SIDE || h > MAX_SIDE {
        return Err(format!("{id}: {w} × {h} (an image is at most {MAX_SIDE} a side)"));
    }
    let texels = t.levels.first().and_then(|l| l.images.first()).ok_or("no texels")?;
    if texels.len() as u64 != u64::from(w) * u64::from(h) * 4 {
        return Err(format!("{id}: not decoded to 8-bit RGBA texels"));
    }
    Ok(ImageData { width: w, height: h, rgba: texels.clone(), source: id.to_string() })
}

pub(super) fn colour(c: u32) -> [f32; 4] {
    let [a, r, g, b] = c.to_be_bytes();
    [r, g, b, a].map(|v| f32::from(v) / 255.0)
}

fn overlay_draw(pool: &mut DrawPool, d: &Draw) -> Option<OverlayDraw> {
    Some(match d {
        Draw::Rect { x, y, w, h, colour: c } => OverlayDraw::Rect { x: *x, y: *y, w: *w, h: *h, color: colour(*c) },
        Draw::Image { x, y, w, h, texture, colour: c } => {
            let (image, texels) = pool.image(texture)?;
            OverlayDraw::Image { x: *x, y: *y, w: *w, h: *h, image, texels, color: colour(*c) }
        }
        Draw::Text { text: t, x, y, colour: c, size, align, shadow } => OverlayDraw::Text {
            text: t.clone(),
            x: *x,
            y: *y,
            size: *size,
            color: colour(*c),
            align: match align {
                Align::Left => text::Align::Left,
                Align::Center => text::Align::Center,
                Align::Right => text::Align::Right,
            },
            shadow: *shadow,
        },
    })
}

pub(super) fn overlay(
    mut pool: ResMut<DrawPool>,
    mut out: ResMut<UiOverlay>,
    state: Res<super::orders::OrderState>,
    scene: Res<super::markers::MarkerScene>,
    mut ui: Option<ResMut<UiScene>>,
    cameras: Query<(&Camera, &GlobalTransform), With<Camera3d>>,
) {
    let ui = ui.as_deref_mut().and_then(UiScene::state);
    let p = &mut *pool;
    p.receive();
    if let Some(list) = p.pending.take() {
        p.list = list;
    }
    let mut draws = Vec::with_capacity(p.list.len() + 2);
    if let Some((camera, eye)) = cameras.iter().next()
        && let Some(size) = camera.physical_viewport_size()
    {
        let mode = ui.as_ref().map_or(ssx_ui::render::ScaleMode::NoBorder, |u| u.scale_mode);
        let to_stage = overlay::stage_transform((size.x, size.y), mode).inverse();
        let scale = camera.target_scaling_factor().unwrap_or(1.0);
        for (icon, at) in scene.icon_points() {
            let z = eye.translation().distance(at);
            if z <= 0.0 || z >= 200.0 {
                continue;
            }
            let Ok(v) = camera.world_to_viewport(eye, at) else { continue };
            let Some(inv) = to_stage else { continue };
            let Some((k, texels)) = p.image(&icon) else { continue };
            let near = (1.0 - z / 120.0).clamp(0.0, 1.0);
            let side = 64.0 * (40.0 + 20.0 * near) / 100.0;
            let (x, y) = inv.apply(v.x * scale, v.y * scale);
            let color = [1.0, 1.0, 1.0, near];
            draws.push(OverlayDraw::Image {
                x: x - side / 2.0,
                y: y - side / 2.0,
                w: side,
                h: side,
                image: k,
                texels,
                color,
            });
        }
    }
    if let Some(color) = state.fade_colour() {
        draws.push(OverlayDraw::Fill { color });
    }
    let list = std::mem::take(&mut p.list);
    draws.extend(list.iter().filter_map(|(_, d)| overlay_draw(p, d)));
    p.list = list;
    out.images = p.order.iter().filter_map(|id| p.images.get(id).cloned()).collect();
    out.draws = draws;
    if out.font.is_empty() {
        out.font = FONT.to_string();
        if ui.as_ref().is_some_and(|u| !u.lib.font_library().contains_key(FONT)) {
            warn!("scripts: the font library has no {FONT}: the scripts' texts are not drawn");
        }
    }
}
