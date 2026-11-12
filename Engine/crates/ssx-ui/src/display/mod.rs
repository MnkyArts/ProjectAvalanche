use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

use ssx_formats::gfx::avm1::Action;
use ssx_formats::gfx::place::{ClipActions, Filter, PlaceObject};
use ssx_formats::gfx::text::EditText;

use crate::geom::{Cxform, Mat2d, RectF, rgba};
use crate::library::{CharRef, Character, FrameTag, Library, MovieId, TimelineDef};
use crate::text::{self, CharFormat, ParaFormat, Paragraph, TextLayout};

pub mod hit;
mod script_ops;
mod text_field;
mod timeline;

use text_field::*;

pub const TIMELINE_DEPTH_OFFSET: i32 = -16384;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct InstanceId {
    index: u32,
    generation: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BlendMode {
    Normal,
    Layer,
    Multiply,
    Screen,
    Lighten,
    Darken,
    Difference,
    Add,
    Subtract,
    Invert,
    Alpha,
    Erase,
    Overlay,
    HardLight,
}

impl BlendMode {
    pub fn from_swf(v: u8) -> BlendMode {
        use BlendMode::*;
        match v {
            2 => Layer,
            3 => Multiply,
            4 => Screen,
            5 => Lighten,
            6 => Darken,
            7 => Difference,
            8 => Add,
            9 => Subtract,
            10 => Invert,
            11 => Alpha,
            12 => Erase,
            13 => Overlay,
            14 => HardLight,
            _ => Normal,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TextShadow {
    pub color: [f32; 4],
    pub blur_x: f32,
    pub blur_y: f32,
    pub strength: f32,
    pub offset: (f32, f32),
    pub knockout: bool,
    pub hide_object: bool,
    pub high_quality: bool,
}

pub fn shadow_offset(distance_px: f32, angle_rad: f32) -> (f32, f32) {
    let tenths = ((f64::from((f64::from(angle_rad) * 572.957763671875) as f32) % 3600.0) as i32) as i16;
    let distance = ((f64::from(distance_px) * 20.0) as i32) as i16;
    let a = f32::from(tenths) * f32::from_bits(0x3AE4_C389);
    let along = |c: f64| f32::from(((f32::from(distance) * c as f32) as i32) as i16);
    (along(f64::from(a).cos()), along(f64::from(a).sin()))
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TextFilter {
    pub blur: (f32, f32),
    pub blur_high_quality: bool,
    pub shadow: Option<TextShadow>,
}

impl TextFilter {
    pub fn from_filters(filters: &[Filter]) -> Option<TextFilter> {
        let mut out = TextFilter::default();
        let mut any = false;
        for f in filters.iter().take(4) {
            match f {
                Filter::Blur { blur_x, blur_y, passes, .. } => {
                    out.blur = (blur_x * 20.0, blur_y * 20.0);
                    out.blur_high_quality = *passes > 1;
                    any = true;
                }
                Filter::DropShadow { color, blur } | Filter::Glow { color, blur } => {
                    let glow = matches!(f, Filter::Glow { .. });
                    let offset = if glow { (0.0, 0.0) } else { shadow_offset(blur.distance, blur.angle) };
                    out.shadow = Some(TextShadow {
                        color: rgba(*color),
                        blur_x: blur.blur_x * 20.0,
                        blur_y: blur.blur_y * 20.0,
                        strength: blur.strength,
                        offset,
                        knockout: blur.knockout,
                        hide_object: !blur.composite_source,
                        high_quality: blur.passes > 1,
                    });
                    any = true;
                }
                _ => {}
            }
        }
        any.then_some(out)
    }
}

#[derive(Clone, Debug)]
pub struct SpriteState {
    pub timeline: Arc<TimelineDef>,
    pub current_frame: u16,
    pub playing: bool,
    pub children: BTreeMap<i32, InstanceId>,
}

#[derive(Clone, Debug)]
pub struct TextState {
    pub def: Arc<EditText>,
    pub html: bool,
    pub source: String,
    pub paragraphs: Vec<Paragraph>,
    pub default_char: CharFormat,
    pub default_para: ParaFormat,
    pub bounds: RectF,
    pub layout: TextLayout,
    pub auto_size: AutoSize,
    pub word_wrap: bool,
    pub text_auto_size: TextAutoSize,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TextAutoSize {
    #[default]
    None,
    Shrink,
    Fit,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AutoSize {
    #[default]
    None,
    Left,
    Center,
    Right,
}

#[derive(Clone, Debug)]
pub enum InstanceKind {
    Sprite(SpriteState),
    Shape,
    Morph,
    EditText(Box<TextState>),
    StaticText,
    Bitmap,
}

#[derive(Clone, Debug)]
pub struct Instance {
    pub parent: Option<InstanceId>,
    pub depth: i32,
    pub character: Option<CharRef>,
    pub movie: MovieId,
    pub name: String,
    pub matrix: Mat2d,
    pub cxform: Cxform,
    pub ratio: f32,
    pub clip_depth: Option<i32>,
    pub blend: BlendMode,
    pub filter: Option<TextFilter>,
    pub visible: bool,
    pub cache_as_bitmap: bool,
    pub placed_frame: Option<u16>,
    pub transformed_by_script: bool,
    pub geom: Option<GeomData>,
    pub clip_actions: Option<Arc<ClipActions>>,
    pub scale9: Option<RectF>,
    pub kind: InstanceKind,
    pub scripted: bool,
    created_tick: u64,
    pub owner: Option<MovieId>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GeomData {
    pub x: i32,
    pub y: i32,
    pub x_scale: f64,
    pub y_scale: f64,
    pub rotation: f64,
    pub orig: Mat2d,
}

impl GeomData {
    pub fn from_matrix(m: &Mat2d) -> GeomData {
        GeomData {
            x: m.tx as i32,
            y: m.ty as i32,
            x_scale: f64::from(m.a).hypot(f64::from(m.b)) * 100.0,
            y_scale: f64::from(m.c).hypot(f64::from(m.d)) * 100.0,
            rotation: f64::from(m.b).atan2(f64::from(m.a)).to_degrees(),
            orig: *m,
        }
    }

    pub fn orig_axes(&self) -> (f64, f64, f64) {
        let o = &self.orig;
        (
            f64::from(o.a).hypot(f64::from(o.b)),
            f64::from(o.c).hypot(f64::from(o.d)),
            f64::from(o.b).atan2(f64::from(o.a)),
        )
    }

    pub fn compose(&self, at: &Mat2d, sx: f32, sy: f32, theta: f32) -> Mat2d {
        let (cos, sin) = (theta.cos(), theta.sin());
        let o = &self.orig;
        Mat2d {
            a: (o.a * cos - o.b * sin) * sx,
            b: (o.a * sin + o.b * cos) * sx,
            c: (o.c * cos - o.d * sin) * sy,
            d: (o.c * sin + o.d * cos) * sy,
            tx: at.tx,
            ty: at.ty,
        }
    }
}

pub fn matrix_in_range(m: &Mat2d) -> bool {
    m.as_array().iter().all(|v| !v.is_infinite())
}

impl Instance {
    pub fn sprite(&self) -> Option<&SpriteState> {
        match &self.kind {
            InstanceKind::Sprite(s) => Some(s),
            _ => None,
        }
    }

    fn sprite_mut(&mut self) -> Option<&mut SpriteState> {
        match &mut self.kind {
            InstanceKind::Sprite(s) => Some(s),
            _ => None,
        }
    }

    pub fn text(&self) -> Option<&TextState> {
        match &self.kind {
            InstanceKind::EditText(t) => Some(t),
            _ => None,
        }
    }
}

#[derive(Clone, Debug)]
pub enum QueuedAction {
    Frame { instance: InstanceId, actions: Arc<[Action]> },
    Init { instance: InstanceId, sprite_id: u16, actions: Arc<[Action]> },
    Construct { instance: InstanceId },
    Load { instance: InstanceId },
    EnterFrame { instance: InstanceId },
    Unload { instance: InstanceId },
}

struct Slot {
    generation: u32,
    instance: Option<Instance>,
}

#[derive(Clone, Debug)]
struct GotoPlace {
    tag: PlaceObject,
    frame: u16,
}

pub struct Player {
    slots: Vec<Slot>,
    free: Vec<u32>,
    pub levels: BTreeMap<i32, InstanceId>,
    actions: Vec<QueuedAction>,
    init_done: HashSet<(MovieId, u16)>,
    tick: u64,
    time: f64,
    pub unresolved: Vec<(MovieId, u16)>,
    instance_names: u32,
    pub script_events: bool,
    /// Scaleform's `_global.noInvisibleAdvance` (the FE sets it, 0x82594A68): a sprite whose own `_visible` is false
    /// is not advanced and gets no enter-frame events (`Sprite::SetVisible` 0x8268CEA0 drops it from the play list,
    /// 0x82644CF8). Its children are listed on their own and advance unless the hide copied the flag onto them
    /// (0x8268C160; [`Player::set_visible`]).
    pub no_invisible_advance: bool,
    live: usize,
    pub peak_instances: usize,
    pub peak_nesting: usize,
    pub refused: u64,
    pub making_for: Option<MovieId>,
    owned: HashMap<MovieId, usize>,
    pub over_cap: Vec<MovieId>,
    scanned: std::sync::atomic::AtomicU64,
}

pub const MAX_INSTANCES: usize = 1 << 16;
pub const MAX_OWNED_INSTANCES: usize = 1 << 14;
pub const MAX_NESTING: usize = 64;

impl Default for Player {
    fn default() -> Self {
        Self::new()
    }
}

impl Player {
    pub fn new() -> Self {
        Player {
            slots: Vec::new(),
            free: Vec::new(),
            levels: BTreeMap::new(),
            actions: Vec::new(),
            init_done: HashSet::new(),
            tick: 0,
            time: 0.0,
            unresolved: Vec::new(),
            instance_names: 0,
            script_events: false,
            no_invisible_advance: false,
            live: 0,
            peak_instances: 0,
            peak_nesting: 0,
            refused: 0,
            making_for: None,
            owned: HashMap::new(),
            over_cap: Vec::new(),
            scanned: std::sync::atomic::AtomicU64::new(0),
        }
    }

    pub fn get(&self, id: InstanceId) -> Option<&Instance> {
        let s = self.slots.get(id.index as usize)?;
        if s.generation != id.generation {
            return None;
        }
        s.instance.as_ref()
    }

    pub fn get_mut(&mut self, id: InstanceId) -> Option<&mut Instance> {
        let s = self.slots.get_mut(id.index as usize)?;
        if s.generation != id.generation {
            return None;
        }
        s.instance.as_mut()
    }

    pub fn instance_count(&self) -> usize {
        self.slots.iter().filter(|s| s.instance.is_some()).count()
    }

    pub(crate) fn owner_for(&self, parent: Option<InstanceId>, package: Option<MovieId>) -> Option<MovieId> {
        self.making_for.or_else(|| parent.and_then(|p| self.get(p)).and_then(|i| i.owner)).or(package)
    }

    pub(crate) fn room_for(&mut self, parent: Option<InstanceId>, owner: Option<MovieId>) -> bool {
        if let Some(o) = owner
            && self.owned.get(&o).copied().unwrap_or(0) >= MAX_OWNED_INSTANCES
        {
            self.refused += 1;
            if !self.over_cap.contains(&o) {
                self.over_cap.push(o);
            }
            return false;
        }
        let mut nesting = 0usize;
        let mut up = parent;
        while let Some(p) = up {
            nesting += 1;
            if nesting >= MAX_NESTING {
                break;
            }
            up = self.get(p).and_then(|i| i.parent);
        }
        if nesting >= MAX_NESTING || self.live >= MAX_INSTANCES {
            self.refused += 1;
            return false;
        }
        self.peak_nesting = self.peak_nesting.max(nesting);
        true
    }

    fn alloc(&mut self, inst: Instance) -> InstanceId {
        if let Some(o) = inst.owner {
            *self.owned.entry(o).or_default() += 1;
        }
        self.live += 1;
        self.peak_instances = self.peak_instances.max(self.live);
        if let Some(index) = self.free.pop() {
            let slot = &mut self.slots[index as usize];
            slot.instance = Some(inst);
            InstanceId { index, generation: slot.generation }
        } else {
            self.slots.push(Slot { generation: 0, instance: Some(inst) });
            InstanceId { index: self.slots.len() as u32 - 1, generation: 0 }
        }
    }

    pub fn take_actions(&mut self) -> Vec<QueuedAction> {
        std::mem::take(&mut self.actions)
    }

    pub fn children(&self, id: InstanceId) -> Vec<(i32, InstanceId)> {
        self.get(id)
            .and_then(Instance::sprite)
            .map_or_else(Vec::new, |s| s.children.iter().map(|(d, c)| (*d, *c)).collect())
    }

    pub fn child_at_depth(&self, id: InstanceId, depth: i32) -> Option<InstanceId> {
        self.get(id)?.sprite()?.children.get(&depth).copied()
    }

    pub fn child_by_name(&self, id: InstanceId, name: &str, case_sensitive: bool) -> Option<InstanceId> {
        let s = self.get(id)?.sprite()?;
        let mut visited = 0u64;
        let found = s.children.values().copied().find(|c| {
            visited += 1;
            self.get(*c)
                .is_some_and(|i| if case_sensitive { i.name == name } else { i.name.eq_ignore_ascii_case(name) })
        });
        self.scanned.fetch_add(visited, std::sync::atomic::Ordering::Relaxed);
        found
    }

    pub fn take_scanned(&self) -> u64 {
        self.scanned.swap(0, std::sync::atomic::Ordering::Relaxed)
    }

    pub fn path(&self, id: InstanceId) -> String {
        let mut parts = Vec::new();
        let mut cur = Some(id);
        while let Some(c) = cur {
            let Some(i) = self.get(c) else { break };
            match i.parent {
                Some(p) => {
                    parts.push(i.name.clone());
                    cur = Some(p);
                }
                None => {
                    let level = self.levels.iter().find(|(_, r)| **r == c).map_or(0, |(l, _)| *l);
                    parts.push(format!("_level{level}"));
                    cur = None;
                }
            }
        }
        parts.reverse();
        parts.join(".")
    }

    fn instantiate(&mut self, lib: &Library, parent: Option<InstanceId>, depth: i32, r: CharRef) -> Option<InstanceId> {
        let (real, ch) = lib.resolve(r)?;
        let owner = self.owner_for(parent, lib.origin(real.movie).map(|_| real.movie));
        if !self.room_for(parent, owner) {
            return None;
        }
        let kind = match ch {
            Character::Sprite(t) => InstanceKind::Sprite(SpriteState {
                timeline: t.clone(),
                current_frame: 0,
                playing: true,
                children: BTreeMap::new(),
            }),
            Character::Shape(_) => InstanceKind::Shape,
            Character::Morph(_) => InstanceKind::Morph,
            Character::EditText(e) => InstanceKind::EditText(Box::new(new_text_state(lib, real.movie, e))),
            Character::Text(_) => InstanceKind::StaticText,
            Character::Bitmap(_) => InstanceKind::Bitmap,
            Character::Font(_) => return None,
        };
        let inst = Instance {
            parent,
            depth,
            character: Some(real),
            movie: real.movie,
            name: String::new(),
            matrix: Mat2d::IDENTITY,
            cxform: Cxform::IDENTITY,
            ratio: 0.0,
            clip_depth: None,
            blend: BlendMode::Normal,
            filter: None,
            visible: true,
            cache_as_bitmap: false,
            placed_frame: None,
            transformed_by_script: false,
            geom: None,
            clip_actions: None,
            scale9: lib.scaling_grid(real),
            kind,
            scripted: false,
            created_tick: self.tick,
            owner,
        };
        let id = self.alloc(inst);
        if let Some(p) = parent.and_then(|p| self.get_mut(p)).and_then(Instance::sprite_mut)
            && let Some(old) = p.children.insert(depth, id)
        {
            self.free_tree(old);
        }
        Some(id)
    }

    fn start(&mut self, lib: &Library, id: InstanceId) {
        if self.get(id).and_then(Instance::sprite).is_some_and(|s| s.current_frame == 0) {
            self.run_frame(lib, id, 1);
        }
    }

    fn free_tree(&mut self, id: InstanceId) {
        let Some(slot) = self.slots.get_mut(id.index as usize) else { return };
        if slot.generation != id.generation {
            return;
        }
        let Some(inst) = slot.instance.take() else { return };
        slot.generation = slot.generation.wrapping_add(1);
        self.free.push(id.index);
        self.live = self.live.saturating_sub(1);
        self.disown(inst.owner);
        if inst.scripted && self.script_events {
            self.actions.push(QueuedAction::Unload { instance: id });
        }
        if let InstanceKind::Sprite(s) = inst.kind {
            for c in s.children.into_values() {
                self.free_tree(c);
            }
        }
    }

    fn disown(&mut self, owner: Option<MovieId>) {
        if let Some(o) = owner
            && let Some(n) = self.owned.get_mut(&o)
        {
            *n = n.saturating_sub(1);
            if *n == 0 {
                self.owned.remove(&o);
            }
        }
    }

    pub fn owned_by(&self, movie: MovieId) -> usize {
        self.owned.get(&movie).copied().unwrap_or(0)
    }

    pub fn unload_owned(&mut self, movie: MovieId) -> usize {
        if self.owned_by(movie) == 0 {
            return 0;
        }
        let before = self.live;
        let tops: Vec<InstanceId> = self
            .slots
            .iter()
            .enumerate()
            .filter_map(|(index, s)| {
                let inst = s.instance.as_ref().filter(|i| i.owner == Some(movie))?;
                let parent_owned = inst.parent.and_then(|p| self.get(p)).is_some_and(|p| p.owner == Some(movie));
                (!parent_owned).then_some(InstanceId { index: index as u32, generation: s.generation })
            })
            .collect();
        for id in tops {
            let loaded_root =
                self.get(id).is_some_and(|i| i.movie == movie && i.character.is_none() && i.placed_frame.is_none());
            if loaded_root {
                for (_, c) in self.children(id) {
                    self.remove(c);
                }
                self.stop(id);
                if let Some(i) = self.get_mut(id) {
                    i.owner = None;
                }
                self.disown(Some(movie));
            } else {
                self.remove(id);
            }
        }
        before - self.live
    }

    pub fn remove(&mut self, id: InstanceId) {
        let Some(inst) = self.get(id) else { return };
        let (parent, depth) = (inst.parent, inst.depth);
        if let Some(p) = parent.and_then(|p| self.get_mut(p)).and_then(Instance::sprite_mut)
            && p.children.get(&depth) == Some(&id)
        {
            p.children.remove(&depth);
        }
        self.levels.retain(|_, r| *r != id);
        self.free_tree(id);
    }
}

#[cfg(test)]
pub(crate) mod tests;
