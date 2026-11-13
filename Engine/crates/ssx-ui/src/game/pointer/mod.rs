mod lists;

use crate::avm1::Ctx;
use crate::avm1::pointer::PointerEvent;
use crate::display::InstanceId;
use crate::display::hit::Hit;

use super::GameUi;
use super::input::{Button, Input, KEYBOARD_DEVICE};
use lists::Reach;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
}

impl MouseButton {
    fn index(self) -> usize {
        match self {
            MouseButton::Left => 0,
            MouseButton::Right => 1,
            MouseButton::Middle => 2,
        }
    }
}

const MAX_WHEEL_STEPS: i32 = 8;
const MAX_FOCUS_STEPS: u32 = 64;

#[derive(Clone, Debug, Default)]
struct Latch {
    position: Option<(f32, f32)>,
    down: [bool; 3],
    went_down: [bool; 3],
    wheel: i32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Entry {
    pub menu: InstanceId,
    pub item: i32,
}

// PORT(choice): a hover on an entry of a list the pad drives presses the pad's D-pad (one step a read, toward the
// entry's slot, the other axis when a step moved nothing within 8 reads, at most 64 steps) until the list's selection
// is that entry, and a click is the pad's A on it once selected; an entry a carousel shows outside the band its
// selection stays in (`components.RiderMenu`'s `mUnselectableLeft` / `mUnselectableRight` cells, a `GridMenu`'s
// unselectable grids) takes no hover — a step toward it turns the carousel and puts another item under a pointer that
// did not move — and a click on it only selects it; the wheel's notches are capped at 8 a read; the console has no
// pointer: the disc's lists answer only the pad's input.
#[derive(Clone, Copy, Debug)]
struct Focus {
    entry: Entry,
    step: Step<i32>,
}

#[derive(Clone, Copy, Debug, Default)]
struct Step<T> {
    at: Option<T>,
    since: u32,
    steps: u32,
    tried: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Next {
    Step,
    Wait,
    Stalled,
}

const STEP_WAIT: u32 = 8;

impl<T: PartialEq + Copy> Step<T> {
    fn next(&mut self, now: T, max_steps: u32, ways: usize) -> Next {
        if let Some(at) = self.at {
            if at == now {
                self.since += 1;
                if self.since <= STEP_WAIT {
                    return Next::Wait;
                }
                if self.tried + 1 >= ways {
                    return Next::Stalled;
                }
                self.tried += 1;
            } else {
                self.tried = 0;
            }
            self.at = None;
        }
        if self.steps >= max_steps { Next::Stalled } else { Next::Step }
    }

    fn made(&mut self, at: T) {
        self.at = Some(at);
        self.since = 0;
        self.steps += 1;
    }
}

#[derive(Default)]
pub struct PointerRouter {
    latch: Latch,
    read_position: Option<(f32, f32)>,
    read_down: [bool; 3],
    release: Vec<Button>,
    focus: Option<Focus>,
    click: Option<Entry>,
    globe: Option<GlobeFocus>,
    globe_click: Option<u32>,
    #[cfg(feature = "net")]
    tab: Option<(super::fe::ava::browser::Tab, Step<super::fe::ava::browser::Tab>)>,
    pub log: Vec<String>,
}

const MAX_LOG_LINES: usize = 256;
const MAX_LOG_CHARS: usize = 400;

impl PointerRouter {
    fn note(&mut self, line: String) {
        if self.log.len() >= MAX_LOG_LINES {
            self.log.remove(0);
        }
        self.log.push(ssx_core::text::shown_max(&line, MAX_LOG_CHARS).to_string());
    }
}

#[derive(Clone, Copy, Debug)]
struct GlobeFocus {
    key: u32,
    step: Step<u32>,
}

// PORT(choice): the pointer steers the ranges' globe with a light push of the stick (0.26 on its larger axis) toward
// the hovered node's billboard, so the pick follows its direction while the globe turns little; the console has no
// pointer.
const STICK_PUSH: f32 = 0.26;

const MAX_GLOBE_STEPS: u32 = 32;

#[derive(Default)]
struct Under {
    prompts_shown: bool,
    globe: Option<(u32, usize)>,
    entry: Option<(Entry, usize)>,
    preview: bool,
    prompt: Option<(Button, usize)>,
}

impl GameUi {
    pub fn pointer_move(&mut self, x: f32, y: f32) {
        if x.is_finite() && y.is_finite() {
            self.pointer.latch.position = Some((x, y));
        }
    }

    pub fn pointer_button(&mut self, b: MouseButton, down: bool) {
        let i = b.index();
        self.pointer.latch.down[i] = down;
        self.pointer.latch.went_down[i] |= down;
    }

    pub fn pointer_position(&self) -> Option<(f32, f32)> {
        self.pointer.latch.position
    }

    pub fn pointer_wheel(&mut self, notches: i32) {
        let w = &mut self.pointer.latch.wheel;
        *w = w.saturating_add(notches).clamp(-MAX_WHEEL_STEPS, MAX_WHEEL_STEPS);
    }

    pub(super) fn pointer_lost(&mut self) {
        self.pointer.latch.went_down = [false; 3];
        self.pointer.latch.wheel = 0;
    }

    pub(super) fn pointer_read(&mut self, cx: &mut Ctx) {
        for b in std::mem::take(&mut self.pointer.release) {
            self.input.push_from(Input::Button { button: b, down: false }, KEYBOARD_DEVICE);
        }
        let latch = self.pointer.latch.clone();
        self.pointer.latch.went_down = [false; 3];
        self.pointer.latch.wheel = 0;
        let pressed = |i: usize| latch.went_down[i] || (latch.down[i] && !self.pointer.read_down[i]);
        let released = |i: usize| !latch.down[i] && (self.pointer.read_down[i] || latch.went_down[i]);
        let (left_down, left_up, right_down) = (pressed(0), released(0), pressed(1));
        self.pointer.read_down = latch.down;
        let Some(pos) = latch.position else { return };
        let moved = self.pointer.read_position != Some(pos);
        self.pointer.read_position = Some(pos);
        // TODO(guess): a move only when the position changed (the game's handler 0x825757F0 posts one every read; whether GFx's mouse state 0x82667938 reports an unchanged point to the movies was not read); settle: 0x82667938 and the AS2 root's mouse processing.
        if moved {
            cx.vm.pointer_event(PointerEvent::Move { x: pos.0, y: pos.1 });
        }
        let stage = (pos.0 * 20.0, pos.1 * 20.0);
        let need = moved
            || left_down
            || right_down
            || latch.wheel != 0
            || self.pointer.focus.is_some()
            || self.pointer.globe.is_some()
            || self.pointer_tab_pending();
        if !need {
            if left_up {
                cx.vm.pointer_event(PointerEvent::Button { down: false });
            }
            return;
        }
        let subscribers = self.input.subscribers.clone();
        let order = lists::paint_order(cx);
        let reach = lists::reach(cx, &subscribers, &order);
        let globe = self.globe_level().map(|level| (level, self.globe_nodes()));
        let mut under = under(cx, &order, &reach, stage);
        if let Some((_, nodes)) = &globe {
            under.globe = globe_under(cx, &order, nodes, stage);
        }
        if moved
            && let Some((e, _)) = under.entry
            && !under.preview
        {
            let same = self.pointer.focus.is_some_and(|f| f.entry == e);
            if !same {
                self.pointer.focus = Some(Focus { entry: e, step: Step::default() });
                self.pointer.click = None;
            }
        }
        if moved {
            match under.globe {
                Some((k, _)) if self.pointer.globe.is_none_or(|g| g.key != k) => {
                    self.pointer.globe = Some(GlobeFocus { key: k, step: Step::default() });
                    self.pointer.globe_click = None;
                }
                None if self.pointer.globe.is_some() && self.pointer.globe_click.is_none() => self.globe_release(),
                _ => {}
            }
        }
        if left_down {
            cx.vm.pointer_event(PointerEvent::Button { down: true });
            self.click_at(cx, stage, &under, &reach, &order);
        }
        if left_up {
            cx.vm.pointer_event(PointerEvent::Button { down: false });
        }
        // PORT(choice): the right button is the pad's B and the wheel the D-pad (up away from the player) on the screen
        // under it (the game reads no wheel: its mouse handler 0x825757F0 posts buttons 0…3 only, and the movies'
        // button events take the primary one); the console has no mouse.
        if right_down {
            self.pointer_press(Button::DigitalTwo, "the right button: B");
        }
        let w = latch.wheel;
        for _ in 0..w.unsigned_abs() {
            let b = if w > 0 { Button::DpadUp } else { Button::DpadDown };
            self.pointer_press(b, "the wheel");
        }
        self.focus_step(cx, &reach);
        #[cfg(feature = "net")]
        self.tab_step();
        if let Some((level, nodes)) = globe {
            self.globe_step(level, &nodes);
        } else if self.pointer.globe.is_some() {
            self.globe_release();
        }
    }

    fn pointer_press(&mut self, b: Button, why: &str) {
        self.pointer.note(format!("pointer: {why} → {b:?}"));
        self.input.push_from(Input::Button { button: b, down: true }, KEYBOARD_DEVICE);
        self.pointer.release.push(b);
        if matches!(b, Button::DpadLeft | Button::DpadRight) {
            let x = if b == Button::DpadRight { 1.0 } else { -1.0 };
            self.fe_call(|fe, _| super::fe::globe::dpad(fe, x));
        }
    }

    fn pointer_tab_pending(&self) -> bool {
        #[cfg(feature = "net")]
        return self.pointer.tab.is_some();
        #[cfg(not(feature = "net"))]
        false
    }

    #[cfg(feature = "net")]
    fn tab_step(&mut self) {
        let Some((target, mut step)) = self.pointer.tab else { return };
        let Some(fe) = self.fe.as_ref() else { return };
        let open = fe.ava.browser.tab;
        let Some(b) = super::fe::ava::browser::pointer::tab_step(fe, target) else {
            self.pointer.tab = None;
            return;
        };
        match step.next(open, MAX_FOCUS_STEPS, 1) {
            Next::Wait => self.pointer.tab = Some((target, step)),
            Next::Stalled => self.pointer.tab = None,
            Next::Step => {
                step.made(open);
                self.pointer.tab = Some((target, step));
                self.pointer_press(b, &format!("the browser's tab {open:?} → {target:?}"));
            }
        }
    }

    fn globe_level(&self) -> Option<u8> {
        let fe = self.fe.as_ref()?;
        fe.globe_scene.as_ref()?;
        match fe.screen {
            0xC | 0x42 | 0xF => Some(1),
            0xD | 0x43 | 0x10 => Some(2),
            0xE | 0x44 | 0x11 => Some(3),
            _ => None,
        }
    }

    fn globe_nodes(&self) -> Vec<super::fe::globe_scene::GlobeEntry> {
        let stage = super::fe::globe::STAGE;
        let scene = self.fe.as_ref().and_then(|f| f.globe_scene.as_ref());
        scene.map(|s| s.entries(stage, stage.0 / stage.1)).unwrap_or_default()
    }

    fn globe_step(&mut self, level: u8, nodes: &[super::fe::globe_scene::GlobeEntry]) {
        let Some(mut g) = self.pointer.globe else { return };
        let selected = nodes.iter().find(|n| n.selected);
        let target = nodes.iter().find(|n| n.key == g.key);
        let (Some(sel), Some(target)) = (selected, target) else {
            self.globe_release();
            return;
        };
        if sel.key == g.key {
            self.globe_release();
            if self.pointer.globe_click.take() == Some(g.key) {
                self.pointer_press(Button::DigitalOne, "a click on a globe node, now selected: A");
            }
            return;
        }
        match g.step.next(sel.key, MAX_GLOBE_STEPS, 1) {
            Next::Wait => {
                self.pointer.globe = Some(g);
                return;
            }
            Next::Stalled => {
                let line = format!("pointer: the globe stays on {:#010x}: no focus on {:#010x}", sel.key, g.key);
                self.pointer.note(line);
                self.globe_release();
                return;
            }
            Next::Step => {}
        }
        g.step.made(sel.key);
        self.pointer.globe = Some(g);
        let (dx, dy) = (target.x - sel.x, target.y - sel.y);
        if level == 1 {
            let len = (dx * dx + dy * dy).sqrt();
            if len < 1.0 {
                return;
            }
            let (ux, uy) = (dx / len, -dy / len);
            let k = STICK_PUSH / ux.abs().max(uy.abs());
            let (x, y) = (ux * k, uy * k);
            self.pointer.note(format!("pointer: the stick toward {:#010x} ({x:.2}, {y:.2})", g.key));
            self.fe_call(|fe, _| super::fe::globe::stick(fe, x, y));
        } else {
            let b = if dx >= 0.0 { Button::DpadRight } else { Button::DpadLeft };
            self.pointer_press(b, &format!("the globe toward {:#010x}", g.key));
        }
    }

    fn globe_release(&mut self) {
        if self.pointer.globe.take().is_some_and(|g| g.step.steps > 0) && self.globe_level() == Some(1) {
            self.fe_call(|fe, _| super::fe::globe::stick(fe, 0.0, 0.0));
        }
        self.pointer.globe_click = None;
    }

    fn click_at(&mut self, cx: &mut Ctx, stage: (f32, f32), under: &Under, reach: &Reach, order: &[InstanceId]) {
        let flash = cx.pointer_pick_at(stage);
        if let Some((b, _)) = under.prompt {
            self.pointer_press(b, "a click on a button prompt");
            return;
        }
        #[cfg(feature = "net")]
        if let Some(fe) = self.fe.as_ref() {
            use super::fe::ava::browser::pointer::{Click, click};
            match click(fe, cx, &mut self.loc, order, stage) {
                Some(Click::Press(b)) => {
                    self.pointer_press(b, "a click on the browser's line");
                    return;
                }
                Some(Click::Tab(t)) => {
                    self.pointer.note(format!("pointer: a click on the browser's tab {t:?}"));
                    self.pointer.tab = Some((t, Step::default()));
                    return;
                }
                None => {}
            }
        }
        #[cfg(not(feature = "net"))]
        let _ = order;
        if let Some(f) = flash {
            let name = cx.player.path(f);
            self.pointer.note(format!("pointer: a click on the movies' button {name}"));
            return;
        }
        if under.entry.is_none()
            && let Some((k, _)) = under.globe
        {
            let selected = self.globe_nodes().iter().find(|n| n.selected).map(|n| n.key);
            if selected == Some(k) {
                self.pointer_press(Button::DigitalOne, "a click on the globe's selected node: A");
            } else {
                self.pointer.globe = Some(GlobeFocus { key: k, step: Step::default() });
                self.pointer.globe_click = Some(k);
            }
            return;
        }
        let Some((e, _)) = under.entry else {
            // PORT(choice): a click anywhere on a screen that offers the pad no list and no prompt is that screen's A
            // (the results' continue); the console has no pointer.
            if reach.lists.is_empty() && !under.prompts_shown && under.globe.is_none() && self.globe_level().is_none() {
                self.pointer_press(Button::DigitalOne, "a click on a screen of one action: A");
            }
            return;
        };
        match reach.lists.iter().find(|l| l.clip == e.menu).map(|l| l.selected) {
            Some(s) if s == e.item => self.pointer_press(Button::DigitalOne, "a click on the selected entry: A"),
            Some(_) => {
                self.pointer.focus = Some(Focus { entry: e, step: Step::default() });
                self.pointer.click = (!under.preview).then_some(e);
                if under.preview {
                    self.pointer.note(format!("pointer: a click on {} outside the carousel's band: select it", e.item));
                }
            }
            None => {}
        }
    }

    fn focus_step(&mut self, cx: &mut Ctx, reach: &Reach) {
        let Some(mut f) = self.pointer.focus else { return };
        let Some(menu) = reach.lists.iter().find(|m| m.clip == f.entry.menu) else {
            self.pointer.focus = None;
            self.pointer.click = None;
            return;
        };
        if menu.selected == f.entry.item {
            self.pointer.focus = None;
            if self.pointer.click.take() == Some(f.entry) {
                self.pointer_press(Button::DigitalOne, "a click on an entry, now selected: A");
            }
            return;
        }
        let ways = lists::step_ways(cx, menu, f.entry.item);
        match f.step.next(menu.selected, MAX_FOCUS_STEPS, ways.len()) {
            Next::Wait => {
                self.pointer.focus = Some(f);
                return;
            }
            Next::Stalled => {
                let line = format!("pointer: the selection stays at {}: no focus on {}", menu.selected, f.entry.item);
                self.pointer.note(line);
                self.pointer.focus = None;
                self.pointer.click = None;
                return;
            }
            Next::Step => {}
        }
        let Some(&dir) = ways.get(f.step.tried) else {
            self.pointer.focus = None;
            self.pointer.click = None;
            return;
        };
        f.step.made(menu.selected);
        self.pointer.focus = Some(f);
        self.pointer_press(dir, &format!("focus {} → {}", menu.selected, f.entry.item));
    }
}

impl Ctx<'_> {
    pub fn pointer_pick_at(&mut self, stage: (f32, f32)) -> Option<InstanceId> {
        let saved = std::mem::replace(&mut self.vm.pointer.position, stage);
        let picked = self.pointer_pick();
        self.vm.pointer.position = saved;
        picked
    }
}

// PORT(choice): an entry is hit anywhere in the slot its menu lays it out in (`mCellWidth` × the row's height from the
// cell's `_x` / `_y`; the rows a view stacks itself — a popup's buttons, a settings page's rows, RiderNet's music rows
// — bands from each row's top to the next's, as wide as the rows are drawn together), a prompt in its own button area
// (`mcHitArea`), a globe's billboard where its body is drawn (the union of the bounds of its visible, not faded, not
// additive / screen / lighten parts; the menus' cells, prompts and billboards are not the movies' buttons, and their
// invisible, faded or glowing decorations reach over their neighbours); the console has no pointer.
fn under(cx: &mut Ctx, order: &[InstanceId], reach: &Reach, stage: (f32, f32)) -> Under {
    let rank: std::collections::HashMap<InstanceId, usize> = order.iter().enumerate().map(|(i, id)| (*id, i)).collect();
    let prompts = reach.prompts(cx, lists::instances_of(cx, order, lists::Kind::Label));
    let mut meshes = std::mem::take(&mut cx.vm.pointer.meshes);
    let mut out = Under { prompts_shown: prompts.iter().any(|&id| lists::drawn(cx, id)), ..Under::default() };
    let mut shown_prompts = Vec::new();
    {
        let mut hit = Hit { player: cx.player, lib: cx.lib, meshes: &mut meshes };
        for l in &reach.lists {
            for c in &l.cells {
                let Some(&r) = rank.get(&c.clip) else { continue };
                if lists::cell_hit(cx, &mut hit, c, stage) && out.entry.is_none_or(|(_, o)| r > o) {
                    out.entry = Some((Entry { menu: l.clip, item: c.item }, r));
                    out.preview = c.preview;
                }
            }
        }
        for id in prompts {
            let area = cx.player.child_by_name(id, "mcHitArea", true);
            let inside = match area {
                Some(a) => hit.bounds(a, stage),
                None => hit.drawn(id, stage),
            };
            if inside && hit.shown_at(id, stage) && lists::drawn(cx, id) {
                shown_prompts.push(id);
            }
        }
    }
    cx.vm.pointer.meshes = meshes;
    for id in shown_prompts {
        let Some(&r) = rank.get(&id) else { continue };
        if out.prompt.is_none_or(|(_, o)| r > o)
            && let Some(b) = lists::prompt_button(cx, id)
        {
            out.prompt = Some((b, r));
        }
    }
    out
}

pub fn targets(cx: &mut Ctx, subscribers: &[String]) -> Vec<String> {
    let px = |b: crate::geom::RectF| {
        format!("({:.0}, {:.0})–({:.0}, {:.0})", b.x0 / 20.0, b.y0 / 20.0, b.x1 / 20.0, b.y1 / 20.0)
    };
    let order = lists::paint_order(cx);
    let reach = lists::reach(cx, subscribers, &order);
    let mut out = Vec::new();
    for m in lists::instances_of(cx, &order, lists::Kind::Menu) {
        let obj = crate::avm1::value::Value::Object(cx.object_for(m));
        let (a, ps) = (lists::flag(cx, &obj, "mIsActive"), lists::flag(cx, &obj, "mIsPassive"));
        let n = |cx: &mut Ctx, k: &str| lists::number(cx, &obj, k).map_or("-".to_string(), |v| v.to_string());
        let fields = ["mFirstVisibleItemIndex", "mSelectedCellIndex", "mNumTotalItems", "mNumVisibleItems"]
            .map(|k| format!("{k} {}", n(cx, k)))
            .join(", ");
        out.push(format!("menu {} active {a} passive {ps}: {fields}", cx.player.path(m)));
    }
    for m in lists::instances_of(cx, &order, lists::Kind::Popup) {
        let obj = crate::avm1::value::Value::Object(cx.object_for(m));
        let r = lists::flag(cx, &obj, "mReceivedPublish");
        let n = |cx: &mut Ctx, k: &str| lists::number(cx, &obj, k).map_or("-".to_string(), |v| v.to_string());
        let (b, sel) = (n(cx, "mNumButtons"), n(cx, "mSelectedIndex"));
        out.push(format!("popup {} received {r} buttons {b} selected {sel}", cx.player.path(m)));
    }
    for (l, p) in lists::listeners(cx, subscribers) {
        out.push(format!("listener {} priority {p}", cx.player.path(l)));
    }
    for l in &reach.lists {
        out.push(format!("list {} selected {}", cx.player.path(l.clip), l.selected));
        for c in &l.cells {
            let parent = cx.player.get(c.clip).and_then(|i| i.parent);
            let w = parent.map_or(crate::geom::Mat2d::IDENTITY, |p| cx.player.world_matrix(p));
            let outside = if c.preview { " outside the band" } else { "" };
            out.push(format!("  entry {} {}{outside}", c.item, px(c.slot.transform(&w))));
        }
    }
    {
        for id in reach.prompts(cx, lists::instances_of(cx, &order, lists::Kind::Label)) {
            {
                let b = lists::prompt_button(cx, id);
                let r = match cx.player.child_by_name(id, "mcHitArea", true) {
                    Some(a) => crate::display::hit::stage_bounds(cx.player, cx.lib, a),
                    None => {
                        crate::display::hit::drawn_bounds(cx.player, cx.lib, id).transform(&cx.player.world_matrix(id))
                    }
                };
                out.push(format!("prompt {} {b:?} {}", cx.player.path(id), px(r)));
            }
        }
    }
    for (k, id) in lists::billboards(cx, &order) {
        let r = crate::display::hit::drawn_bounds(cx.player, cx.lib, id).transform(&cx.player.world_matrix(id));
        out.push(format!("billboard {k:#010x} {} {}", cx.player.path(id), px(r)));
    }
    for id in order {
        let Some(&obj) = cx.vm.display_objs.get(&id) else { continue };
        if crate::avm1::pointer::BUTTON_EVENTS.iter().any(|n| cx.has_property(obj, n)) {
            let r = crate::display::hit::stage_bounds(cx.player, cx.lib, id);
            out.push(format!("button {} {}", cx.player.path(id), px(r)));
        }
    }
    out
}

fn globe_under(
    cx: &mut Ctx,
    order: &[InstanceId],
    nodes: &[super::fe::globe_scene::GlobeEntry],
    stage: (f32, f32),
) -> Option<(u32, usize)> {
    let rank: std::collections::HashMap<InstanceId, usize> = order.iter().enumerate().map(|(i, id)| (*id, i)).collect();
    let boards: Vec<(u32, InstanceId)> =
        lists::billboards(cx, order).into_iter().filter(|(k, _)| nodes.iter().any(|n| n.key == *k)).collect();
    let mut meshes = std::mem::take(&mut cx.vm.pointer.meshes);
    let mut best: Option<(u32, usize)> = None;
    {
        let mut hit = Hit { player: cx.player, lib: cx.lib, meshes: &mut meshes };
        for (k, id) in boards {
            let Some(&r) = rank.get(&id) else { continue };
            if hit.drawn(id, stage)
                && hit.shown_at(id, stage)
                && lists::drawn(cx, id)
                && best.is_none_or(|(_, o)| r > o)
            {
                best = Some((k, r));
            }
        }
    }
    cx.vm.pointer.meshes = meshes;
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_focus_wait_is_bounded() {
        let mut s = Step::<i32>::default();
        assert_eq!(s.next(0, MAX_FOCUS_STEPS, 2), Next::Step);
        s.made(0);
        for _ in 0..STEP_WAIT {
            assert_eq!(s.next(0, MAX_FOCUS_STEPS, 2), Next::Wait);
        }
        assert_eq!(s.next(0, MAX_FOCUS_STEPS, 2), Next::Step, "the other way");
        assert_eq!(s.tried, 1);
        s.made(0);
        for _ in 0..STEP_WAIT {
            assert_eq!(s.next(0, MAX_FOCUS_STEPS, 2), Next::Wait);
        }
        assert_eq!(s.next(0, MAX_FOCUS_STEPS, 2), Next::Stalled, "no way moved it");
        let mut s = Step::<i32>::default();
        for at in 0..MAX_FOCUS_STEPS as i32 {
            assert_eq!(s.next(at, MAX_FOCUS_STEPS, 1), Next::Step);
            s.made(at);
        }
        assert_eq!(s.next(MAX_FOCUS_STEPS as i32, MAX_FOCUS_STEPS, 1), Next::Stalled, "the steps' cap");
    }

    #[test]
    fn the_log_is_bounded() {
        let mut r = PointerRouter::default();
        for i in 0..MAX_LOG_LINES + 44 {
            r.note(format!("line {i}"));
        }
        assert_eq!(r.log.len(), MAX_LOG_LINES);
        assert_eq!(r.log[0], "line 44");
        r.note("x".repeat(100_000));
        let last = r.log.last().expect("a line");
        assert!(last.ends_with("… (100000 bytes)"), "{last}");
        assert_eq!(last.chars().count(), MAX_LOG_CHARS + "… (100000 bytes)".chars().count());
        r.note("a\nb\u{1b}[2J".into());
        assert_eq!(r.log.last().map(String::as_str), Some("a\\nb\\u{1b}[2J"));
    }
}
