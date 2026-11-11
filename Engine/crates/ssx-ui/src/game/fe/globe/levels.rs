use ssx_assets::career::globe::Shot;

use crate::game::fe::globe::nodes::stage_point;
use crate::game::fe::globe::scene::{Camera, GlobeEntry, GlobeFlow, GlobeScene, SceneEvents, Segment, V3, mmul};

pub fn enter_message(level: u32) -> u32 {
    match level {
        0 => 7,
        1 => 3,
        2 => 5,
        3 => 9,
        4 => 12,
        5 => 16,
        6 => 20,
        _ => 0,
    }
}

pub fn leave_message(level: u32) -> u32 {
    match level {
        0 | 2 => 11,
        3 => 14,
        4 => 18,
        5 => 21,
        _ => 0,
    }
}

fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn add(a: V3, b: V3) -> V3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

fn scale(a: V3, s: f32) -> V3 {
    [a[0] * s, a[1] * s, a[2] * s]
}

fn normalize(a: V3) -> V3 {
    let l = (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt();
    if l > 0.0 { scale(a, 1.0 / l) } else { a }
}

const APPROACH: V3 = [0.0, 1400.0, 500.0];
const UP: V3 = [0.0, 15.0, 0.0];

impl GlobeScene {
    fn parent(&self, level: i32, node: u32) -> u32 {
        match level {
            4 => self.nodes.range_of(node).unwrap_or(0),
            5 => self.nodes.peak_of(node).unwrap_or(0),
            6 => node,
            _ => 0,
        }
    }

    pub fn request(&mut self, level: u32, node: u32) {
        let mut old_level = self.level.map_or(-1, |l| l as i32);
        let new_level0 = level as i32;
        let mut new_level = new_level0;
        let mut old_parent = self.parent(old_level, self.node);
        let mut new_node = node;
        let mut new_parent = self.parent(new_level, node);
        let mut leaves = Vec::new();
        let mut enters = Vec::new();
        if old_level == -1 && level <= 1 {
            enters.push((enter_message(level), node));
        } else {
            for l in (0..=5i32).rev() {
                if old_level == new_level && old_parent == new_parent {
                    break;
                }
                if old_level == l + 1 {
                    leaves.push((leave_message(l as u32), old_parent));
                    old_parent = self.parent(l, old_parent);
                    old_level = l;
                }
                if new_level == l + 1 {
                    enters.push((enter_message((l + 1) as u32), new_node));
                    new_node = new_parent;
                    new_parent = self.parent(l, new_parent);
                    new_level = l;
                }
            }
        }
        enters.reverse();
        let mut messages: Vec<(u32, u32)> = leaves.into_iter().chain(enters).filter(|(m, _)| *m != 0).collect();
        self.level = Some(level);
        self.node = node;
        if messages.is_empty() && new_level0 == 5 {
            self.drop_focus = node;
            self.drop_focus_scene();
        } else if messages.len() == 1 && messages[0].0 == 16 {
            if let Some(p) = self.nodes.peak_of(node) {
                self.peak_focus = p;
                self.peak_focus_scene(p);
            }
        } else if messages.len() == 2 && messages[0].0 == 18 && messages[1].0 == 16 {
            messages[0].1 = self.nodes.peak_of(node).unwrap_or(messages[0].1);
        }
        self.queue.extend(messages);
    }

    pub fn select_drop(&mut self, drop: u32) {
        if drop != 0 {
            self.node = drop;
        }
        self.drop_focus = drop;
        self.drop_focus_scene();
    }

    fn dispatch(&mut self) {
        if self.state != 0 {
            return;
        }
        let Some((msg, node)) = self.queue.pop_front() else { return };
        self.dispatches = self.dispatches.wrapping_add(1);
        self.state = msg;
        self.state_node = node;
        self.sounds.extend_from_slice(state_sounds(msg));
        self.controller_message(msg, node);
        match msg {
            3 => self.state = 4,
            5 | 7 => {
                self.dd = false;
                self.set_lighting(0, if msg == 5 { 0.5 } else { 1.0 });
                self.state = msg + 1;
            }
            9 => {
                self.midground_visible = false;
                self.midground_range = node;
                self.state = 10;
            }
            12 => {
                self.midground_range = self.nodes.range_of(node).unwrap_or(0);
                self.swap_lighting = true;
            }
            14 => {
                self.swap_lighting = true;
                self.state = 15;
            }
            16 => {
                self.state = 17;
                let peak = self.nodes.peak_of(node).unwrap_or(0);
                self.dd = self.nodes.peaks.get(&peak).is_some_and(|p| p.dd);
                if self.dd {
                    self.set_lighting(1, 1.34);
                }
            }
            18 => {
                self.state = 19;
                if self.dd {
                    self.dd = false;
                    self.set_lighting(1, 0.9);
                }
                self.peak_focus = node;
                self.peak_focus_scene(node);
            }
            _ => {}
        }
    }

    fn controller_message(&mut self, msg: u32, node: u32) {
        self.transition = 4;
        match msg {
            11 => self.follow_focus = false,
            12 => {
                self.peak_focus = node;
                self.peak_focus_scene(node);
                self.follow_focus = true;
                if let Some(r) = self.nodes.range_of(node) {
                    self.focus = r;
                }
            }
            9 | 14 => {
                self.follow_focus = true;
                self.focus = node;
            }
            16 => {
                self.drop_focus = node;
                self.drop_focus_scene();
            }
            _ => {}
        }
        self.scene_message(msg);
    }

    fn peak_focus_scene(&mut self, peak: u32) {
        let Some(p) = self.nodes.peaks.get(&peak) else { return };
        self.peak_shot = p.shot_peak;
        self.shots.queue(&[Segment::new(self.peak_shot, 3, 1.0)]);
        self.drop_level = false;
        self.scene_peak = peak;
    }

    pub fn set_widescreen(&mut self, widescreen: bool) {
        self.nodes = crate::game::fe::globe::nodes::NodeMap::build(&self.data, widescreen);
        if self.scene_peak != 0 && !self.drop_level && self.nodes.peaks.contains_key(&self.scene_peak) {
            self.peak_focus_scene(self.scene_peak);
        }
    }

    fn drop_focus_scene(&mut self) {
        let Some(d) = self.nodes.drops.get(&self.drop_focus) else { return };
        let x = d.screen_x;
        let peak = d.peak;
        let index = self.nodes.drops_of(peak).filter(|o| o.screen_x < x).count();
        if self.drop_level && index != self.drop_index {
            self.shots.queue(&[Segment::new(self.drop_shot, 3, 0.5)]);
        }
        self.drop_index = index;
        self.drop_count = self.nodes.drops_of(peak).count();
        self.drop_level = true;
        self.scene_drop = self.drop_focus;
    }
}

impl GlobeScene {
    fn scene_message(&mut self, msg: u32) {
        let shots = self.data.globe.shots;
        let node_shot = |s: &Shot| (normalize(sub(s.look, s.offset)), s.offset, s.look);
        match msg {
            3 => self.shots.go(shots[1], 1.0, Some(shots[0])),
            5 => self.shots.go(shots[3], 0.5, None),
            7 => self.shots.go(shots[3], 1.0, Some(shots[2])),
            9 => self.shots.go(shots[4], 1.0, None),
            11 => self.shots.go(shots[3], 1.0, None),
            12 => {
                self.drop_level = false;
                if let Some(p) = self.nodes.peaks.get(&self.scene_peak) {
                    self.peak_shot = p.shot_peak;
                }
                let n = self.peak_shot;
                let (dir, offset, look) = node_shot(&n);
                let a = Shot { offset: APPROACH, look, target: [0.0; 3], fov: shots[4].fov };
                let b = Shot {
                    offset: add(sub(offset, scale(dir, 100.0)), UP),
                    look: sub(look, UP),
                    target: [0.0; 3],
                    fov: n.fov,
                };
                self.shots.queue(&[Segment::new(a, 5, 1.0), Segment::new(b, 0, 0.0), Segment::new(n, 6, 0.75)]);
            }
            14 => {
                self.drop_level = false;
                let n = self.peak_shot;
                let (dir, offset, look) = node_shot(&n);
                let a = Shot {
                    offset: add(sub(offset, scale(dir, 80.0)), UP),
                    look: sub(look, UP),
                    target: [0.0; 3],
                    fov: n.fov,
                };
                let b = Shot { offset: APPROACH, look, target: [0.0; 3], fov: shots[4].fov };
                self.shots.queue(&[Segment::new(a, 5, 0.75), Segment::new(b, 0, 0.0), Segment::new(shots[4], 6, 0.75)]);
            }
            16 => {
                if let Some(p) = self.nodes.peaks.get(&self.scene_peak) {
                    self.drop_shot = p.shot_drop;
                }
                self.shots.queue(&[Segment::new(self.drop_shot, 2, 0.5)]);
            }
            18 => {
                self.drop_level = false;
                self.shots.queue(&[Segment::new(self.peak_shot, 2, 0.5)]);
            }
            20 => {
                let (dir, offset, _) = node_shot(&self.drop_shot);
                let o = add(offset, scale(dir, 20.0));
                let s = Shot { offset: o, look: add(o, dir), ..self.drop_shot };
                self.shots.queue(&[Segment::new(s, 2, 1.0)]);
            }
            21 => self.shots.queue(&[Segment::new(self.drop_shot, 2, 0.5)]),
            _ => {}
        }
    }

    fn busy(&self) -> bool {
        self.transition > 0 && self.shots.total > 0.0
    }

    pub fn entity_busy(&self) -> bool {
        !self.queue.is_empty() || self.state != 0 || self.busy()
    }

    fn entity_update(&mut self) {
        let done = |s: &GlobeScene| s.shots.progress() < 0.0;
        match self.state {
            4 | 6 | 8 | 10 if done(self) => self.state = 0x16,
            0xB | 0x11 | 0x13 => self.state = 0x16,
            0xC => {
                self.state = 0xD;
                self.cloud = 0;
            }
            0xD => self.cloud_transition(),
            0xF => {
                let mut p = self.shots.progress();
                if p < 0.0 {
                    self.state = 0x16;
                    p = 1.0;
                }
                let f = (f64::from(p) * 1.5) as f32;
                if !self.fade_posted {
                    if (0.55..0.75).contains(&f) {
                        self.fades.push((crate::game::fe::fade::FADE_OUT, 0.75 - f, 0.0));
                        self.fade_posted = true;
                    }
                } else if f >= 0.75 {
                    self.fades.push((crate::game::fe::fade::FADE_IN, f32::from_bits(0x3E4C_CCCD), 0.0));
                    self.fade_posted = false;
                }
                self.globe_visible = p >= 0.5;
                self.midground_visible = p < 0.5;
                if p >= 0.5 && self.swap_lighting {
                    self.set_lighting(0, 0.0);
                }
                self.cloud_in = 0.0;
                self.cloud_out = if p >= 0.5 { (0.51 - p).max(0.0) * 100.0 } else { (p - 0.2).max(0.0) * 3.333_333_3 };
            }
            0x14 if done(self) => {
                self.midground_visible = false;
                self.state = 0x16;
            }
            0x15 => {
                if done(self) {
                    self.state = 0x16;
                }
                self.midground_visible = true;
            }
            0x16 if !self.busy() => self.state = 0,
            _ => {}
        }
    }

    fn cloud_transition(&mut self) {
        let t = self.shots.time;
        let done = self.shots.total <= 0.0;
        loop {
            match self.cloud {
                0 if t >= 0.9 || done => {
                    self.cloud = 1;
                    if !self.fade_posted {
                        self.fades.push((crate::game::fe::fade::FADE_OUT, 0.1, 0.0));
                        self.fade_posted = true;
                    }
                }
                1 if t >= 1.0 || done => {
                    self.cloud_in = 1.0;
                    self.cloud_out = 0.0;
                    self.cloud = 2;
                }
                1 => {
                    self.cloud_out = 0.0;
                    self.cloud_in = 1.0 - (1.0 - t) * 10.0;
                    return;
                }
                2 => {
                    self.globe_visible = false;
                    self.midground_visible = true;
                    if self.fade_posted {
                        self.fades.push((crate::game::fe::fade::FADE_IN, 0.1, 0.0));
                        self.fade_posted = false;
                    }
                    if self.swap_lighting {
                        self.set_lighting(1, 0.0);
                    }
                    self.cloud = 3;
                }
                3 if t >= 1.4 || done => {
                    self.cloud_in = 0.0;
                    self.cloud_out = 0.0;
                    self.cloud = 4;
                }
                3 => {
                    self.cloud_out = 0.0;
                    self.cloud_in = 1.0 - (t - 1.0) * 2.5;
                    return;
                }
                4 if done => {
                    self.state = 0x16;
                    return;
                }
                _ => return,
            }
        }
    }
}

fn level_input(stick_x: f32, dpad_x: f32, repeat: &mut f32, dt: f32) -> Option<f32> {
    let stick = stick_x.abs() >= 0.25;
    let dpad = dpad_x.abs() >= 0.25;
    if !stick {
        *repeat = -1.0;
    } else if *repeat >= 0.0 {
        *repeat -= dt;
    }
    let go = (stick && *repeat <= 0.0) || dpad;
    go.then_some(if dpad { dpad_x } else { stick_x })
}

#[derive(Default)]
struct Nearest {
    best: Option<u32>,
    best_d: f32,
    far: Option<u32>,
    far_d: f32,
}

impl Nearest {
    fn new() -> Self {
        Nearest { best: None, best_d: -1.0, far: None, far_d: 0.0 }
    }

    fn offer(&mut self, key: u32, d: f32) {
        if d > 0.0 {
            if d < self.best_d || self.best_d < 0.0 {
                self.best_d = d;
                self.best = Some(key);
            }
        } else if d < 0.0 && d < self.far_d {
            self.far_d = d;
            self.far = Some(key);
        }
    }

    fn pick(&self) -> Option<u32> {
        self.best.or(self.far)
    }
}

pub fn state_sounds(state: u32) -> &'static [i32] {
    match state {
        5 => &[0x3E],
        7 => &[0x3F],
        0xC => &[0x45, 0x39],
        0xD => &[0x3A],
        0xE => &[4, 0x3B],
        0x10 => &[0x47, 0x3C],
        0x12 => &[4, 0x38],
        0x14 => &[0x49],
        _ => &[],
    }
}

pub fn highlight_sound(state: i32) -> Option<i32> {
    match state {
        1 => Some(0x44),
        2 => Some(0x46),
        3 => Some(0x48),
        _ => None,
    }
}

impl GlobeScene {
    pub fn update(&mut self, dt: f32, screen: u32, input: bool, flow: &dyn GlobeFlow) -> SceneEvents {
        self.time += dt;
        self.light.step(dt);
        self.postfx.step(dt);
        self.fades.clear();
        self.sounds.clear();
        self.clouds.update(dt);
        let mut ev = SceneEvents::default();
        self.entity_update();
        self.dispatch();
        self.shots.step(dt);
        if self.transition > 0 && self.shots.total <= 0.0 {
            self.transition -= 1;
        }
        ev.transitioning = self.transition != 0;
        let mut ran = false;
        if self.transition == 0 {
            let range_screen = matches!(screen, 0xC | 0x42 | 0xF);
            if (2..=4).contains(&screen) {
                self.camera_menu(dt);
                ran = true;
            } else if range_screen || matches!(self.state, 10 | 0xF) {
                ev.range = self.camera_range(dt, input, flow);
                if ev.range.is_some() {
                    // TODO(guess): the range spin posts highlight state 1; 0x82511A90 maps 1 to RS_menu_highlight but its caller is unresolved; settle: find the xref that passes 1
                    ev.audio = Some(1);
                }
                ev.list_type = Some(1);
                ran = true;
            } else if matches!(screen, 0xD | 0x43 | 0x10) {
                ran = true;
                if self.state == 0 {
                    self.peak_update(dt, input, flow, &mut ev);
                } else if self.state == 0xD {
                    let held = (self.pitch.target, self.yaw.target);
                    self.camera_targets(dt, held);
                }
            } else if matches!(screen, 0xE | 0x44 | 0x11) {
                ran = true;
                if self.state == 0 {
                    self.drop_update(dt, input, flow, &mut ev);
                }
            }
        }
        if !ran {
            self.camera_idle(dt);
        }
        self.dpad = 0.0;
        ev
    }

    fn level_camera(&self) -> crate::game::fe::globe::scene::M4 {
        let cam = Camera::from_shot(&self.shots.current);
        let stage = crate::game::fe::globe::STAGE;
        mmul(&cam.view(), &cam.projection(stage.0 / stage.1))
    }

    fn peak_update(&mut self, dt: f32, input: bool, flow: &dyn GlobeFlow, ev: &mut SceneEvents) {
        let Some(focus) = self.nodes.peaks.get(&self.peak_focus).cloned() else { return };
        let dir = if input { level_input(self.stick.0, self.dpad, &mut self.peak_repeat, dt) } else { None };
        let vp = self.level_camera();
        self.highlight = focus.position;
        self.highlight_off = 0.0;
        self.highlight_angle = 0.0;
        let focus_x = stage_point(focus.position, &vp).0;
        let recommended_peak = self.nodes.peak_of(flow.recommended()).unwrap_or(0);
        let mut list = Vec::new();
        let mut near = Nearest::new();
        for p in self.nodes.peaks_of(focus.range) {
            let (x, y) = stage_point(p.position, &vp);
            let selected = p.key == focus.key;
            let entry = GlobeEntry {
                key: p.key,
                kind: 2,
                x,
                y,
                opacity: 1.0,
                scale: 1.0,
                win_condition: 7,
                selectable: p.has_drops,
                selected,
                recommended: p.key == recommended_peak,
                is_new: flow.is_new(p.key),
                ..Default::default()
            };
            if flow.shown(p.key) {
                list.push(entry);
            }
            if let Some(d) = dir.filter(|_| !selected && flow.focusable(p.key)) {
                near.offer(p.key, (x - focus_x) * d);
            }
        }
        self.list = list;
        ev.list_type = Some(2);
        if let Some(k) = near.pick() {
            self.peak_focus = k;
            self.peak_repeat = 0.4;
            self.peak_focus_scene(k);
            ev.peak = Some(k);
            ev.audio = Some(2);
            ev.recommendation = Some(recommended_peak != 0 && recommended_peak == k);
        }
    }

    fn drop_update(&mut self, dt: f32, input: bool, flow: &dyn GlobeFlow, ev: &mut SceneEvents) {
        let Some(focus) = self.nodes.drops.get(&self.drop_focus).cloned() else { return };
        let dir = if input { level_input(self.stick.0, self.dpad, &mut self.drop_repeat, dt) } else { None };
        self.highlight_off = 1.0;
        let focus_x = flow.drop_point(focus.key).0;
        let recommended = flow.recommended();
        let mode = flow.mode();
        let mut list = Vec::new();
        let mut near = Nearest::new();
        for d in self.nodes.drops_of(focus.peak) {
            let (x, y) = flow.drop_point(d.key);
            let selected = d.key == focus.key;
            let entry = GlobeEntry {
                key: d.key,
                kind: 3,
                name: d.title.clone(),
                x,
                y,
                opacity: 1.0,
                scale: 1.0,
                win_condition: d.event_type as i32,
                selectable: d.selectable,
                selected,
                recommended: d.key == recommended,
                is_new: flow.is_new(d.key),
            };
            let of_mode = d.drop_type == mode;
            if of_mode && flow.shown(d.key) {
                list.push(entry);
            }
            if let Some(dd) = dir.filter(|_| !selected && of_mode && flow.focusable(d.key)) {
                near.offer(d.key, (x - focus_x) * dd);
            }
        }
        self.list = list;
        ev.list_type = Some(3);
        if let Some(k) = near.pick() {
            self.drop_focus = k;
            self.drop_repeat = 0.4;
            self.drop_focus_scene();
            ev.drop = Some(k);
            ev.audio = Some(3);
            ev.recommendation = Some(k == recommended);
        }
    }
}

pub struct SceneOnly {
    pub mode: i64,
    pub points: std::collections::BTreeMap<u32, (f32, f32)>,
}

impl SceneOnly {
    pub fn new(scene: &GlobeScene, mode: i64) -> Self {
        let stage = crate::game::fe::globe::STAGE;
        let mut points = std::collections::BTreeMap::new();
        for d in scene.nodes.drops.values() {
            if let Some(p) = scene.nodes.peaks.get(&d.peak) {
                let c = Camera::from_shot(&p.shot_drop);
                points.insert(d.key, stage_point(d.position, &mmul(&c.view(), &c.projection(stage.0 / stage.1))));
            }
        }
        SceneOnly { mode, points }
    }
}

impl GlobeFlow for SceneOnly {
    fn shown(&self, _key: u32) -> bool {
        true
    }
    fn focusable(&self, _key: u32) -> bool {
        true
    }
    fn mode(&self) -> i64 {
        self.mode
    }
    fn recommended(&self) -> u32 {
        0
    }
    fn is_new(&self, _key: u32) -> bool {
        false
    }
    fn drop_point(&self, drop: u32) -> (f32, f32) {
        self.points.get(&drop).copied().unwrap_or((0.0, 0.0))
    }
}

pub fn screen_of_level(level: u32) -> u32 {
    match level {
        2 => 2,
        3 => 0x42,
        4 => 0x43,
        5 => 0x44,
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn globe_states_play_their_sounds() {
        let mut scene = GlobeScene::new(Default::default());
        for (state, ids) in [
            (5, &[0x3E][..]),
            (7, &[0x3F]),
            (0xC, &[0x45, 0x39]),
            (0xD, &[0x3A]),
            (0xE, &[4, 0x3B]),
            (0x10, &[0x47, 0x3C]),
            (0x12, &[4, 0x38]),
            (0x14, &[0x49]),
            (9, &[]),
        ] {
            scene.state = 0;
            scene.sounds.clear();
            scene.queue.push_back((state, 0));
            scene.dispatch();
            assert_eq!(scene.sounds, ids, "state {state:#x}");
        }
    }

    #[test]
    fn highlight_states_map_to_sounds() {
        assert_eq!([1, 2, 3, 0, 4].map(highlight_sound), [Some(0x44), Some(0x46), Some(0x48), None, None]);
    }
}
