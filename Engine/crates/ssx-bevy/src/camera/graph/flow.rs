use ssx_assets::anim_eval::curve_weight;
use ssx_physics::tags::{BoolTag, FloatTag, TagEvent, TagTracker, TagWindow};

use super::{
    Channels, GraphState, Inputs, Shared, Variant, Variant15, blend, step, step_15p, step_death, step_reverse,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Node {
    Conduit,
    Ground,
    Airborne,
    Grind,
    Death,
    ReverseConduit,
    ReverseGround,
    ReverseDeath,
    P15Conduit,
    P15Ground,
    P15Airborne,
    P15Rails,
    P15Wipeout,
    P15Heli,
    P15Death,
    FarConduit,
    FarGround,
    FarAirborne,
    FarDeath,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Signals {
    pub in_air: bool,
    pub grinding: bool,
    pub wipeout: bool,
    pub dead: bool,
    pub heli_active: bool,
}

impl Signals {
    pub fn of(s: &Shared, i: &Inputs) -> Self {
        Self {
            in_air: s.in_air,
            grinding: i.grinding,
            wipeout: i.wipeout,
            dead: i.dead,
            heli_active: i.heli_interact && i.heli_time > -60.0 && i.heli_time < 15.0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Blend {
    Cut,
    Default,
}

const FADE_TICKS: f32 = 8.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TagVar {
    PlayerOffsetY,
    EnableMetaBlending,
    Handycam,
    TransitionTriggered,
    TransitionSpeedSet,
    BigAir,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum TagValue {
    Bool(BoolTag),
    Float(FloatTag),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CamTag {
    var: TagVar,
    window: TagWindow,
    value: TagValue,
}

const fn bool_tag(var: TagVar, value: bool, always: bool) -> CamTag {
    let duration = if always { 32767.0 } else { 0.0 };
    let tag = BoolTag { value, mode: 1, reset: None };
    CamTag { var, window: TagWindow::new(0.0, duration, always), value: TagValue::Bool(tag) }
}

const fn bool_tag_reset(var: TagVar, value: bool, reset: bool) -> CamTag {
    let tag = BoolTag { value, mode: 1, reset: Some(reset) };
    CamTag { var, window: TagWindow::new(0.0, 32767.0, true), value: TagValue::Bool(tag) }
}

const fn float_tag(var: TagVar, value: f32, duration: f32, reset: Option<f32>) -> CamTag {
    let tag = FloatTag { value, mode: 1, reset, spline: 1.0 };
    CamTag { var, window: TagWindow::new(0.0, duration, false), value: TagValue::Float(tag) }
}

const SPEED_DEFAULT: Option<f32> = Some(0.300_7);
const OFFSET_DEFAULT: Option<f32> = Some(0.8);
const HANDYCAM_DEFAULT: bool = true;

const TRIGGERED: CamTag = bool_tag(TagVar::TransitionTriggered, true, false);
const SPEED_IN: CamTag = float_tag(TagVar::TransitionSpeedSet, 0.1, 0.0, SPEED_DEFAULT);
const SPEED_BACK: CamTag = float_tag(TagVar::TransitionSpeedSet, 0.1, 0.0, None);
const SPEED_DEATH: CamTag = float_tag(TagVar::TransitionSpeedSet, 0.8, 0.0, None);
const SPEED_DEATH_GRIND: CamTag = float_tag(TagVar::TransitionSpeedSet, 0.65, 0.0, None);
const BIG_AIR: CamTag = bool_tag(TagVar::BigAir, false, false);
const SPEED_15P_AIR: CamTag = float_tag(TagVar::TransitionSpeedSet, 0.05, 0.0, SPEED_DEFAULT);
const SPEED_15P_GROUND: CamTag = float_tag(TagVar::TransitionSpeedSet, 0.3, 0.0, SPEED_DEFAULT);
const SPEED_15P_WIPEOUT: CamTag = float_tag(TagVar::TransitionSpeedSet, 0.2, 0.0, SPEED_DEFAULT);
const SPEED_FAR_AIR: CamTag = float_tag(TagVar::TransitionSpeedSet, 0.1, 0.0, None);
const SPEED_FAR_GROUND: CamTag = float_tag(TagVar::TransitionSpeedSet, 0.3, 0.0, SPEED_DEFAULT);

fn node_tags(n: Node) -> &'static [CamTag] {
    const GROUND: [CamTag; 1] = [float_tag(TagVar::PlayerOffsetY, 0.3, 1.0, OFFSET_DEFAULT)];
    const DEATH: [CamTag; 1] = [bool_tag(TagVar::EnableMetaBlending, false, true)];
    const FAR_GROUND: [CamTag; 2] = [
        bool_tag_reset(TagVar::Handycam, false, HANDYCAM_DEFAULT),
        float_tag(TagVar::PlayerOffsetY, 0.3, 1.0, OFFSET_DEFAULT),
    ];
    match n {
        Node::Ground => &GROUND,
        Node::Death | Node::ReverseDeath | Node::P15Death | Node::FarDeath => &DEATH,
        Node::FarGround => &FAR_GROUND,
        _ => &[],
    }
}

fn transition_tags(from: Node, to: Node) -> &'static [CamTag] {
    use Node::*;
    match (from, to) {
        (Conduit, Ground) => &[SPEED_IN, TRIGGERED],
        (Conduit, Airborne) => &[TRIGGERED, SPEED_IN, BIG_AIR],
        (Conduit, Grind) => &[TRIGGERED, SPEED_IN],
        (Conduit, Death) => &[TRIGGERED, SPEED_DEATH_GRIND],
        (Ground | Airborne, Conduit) => &[SPEED_BACK, TRIGGERED],
        (Ground, Death) => &[SPEED_DEATH, TRIGGERED],
        (Airborne, Death) => &[TRIGGERED, SPEED_DEATH],
        (Grind, Death) => &[SPEED_DEATH_GRIND, TRIGGERED],
        (P15Conduit, P15Airborne) => &[TRIGGERED, SPEED_15P_AIR],
        (P15Conduit, P15Heli) => &[TRIGGERED, SPEED_15P_AIR],
        (P15Conduit, P15Ground) => &[TRIGGERED, SPEED_15P_GROUND],
        (P15Conduit, P15Rails) => &[SPEED_15P_GROUND, TRIGGERED],
        (P15Conduit, P15Wipeout) => &[TRIGGERED, SPEED_15P_WIPEOUT],
        (P15Conduit | P15Rails | P15Wipeout, P15Death) => &[TRIGGERED, SPEED_DEATH_GRIND],
        (P15Ground | P15Airborne, P15Death) => &[SPEED_DEATH, TRIGGERED],
        (FarConduit, FarAirborne) => &[TRIGGERED, BIG_AIR, SPEED_FAR_AIR],
        (FarConduit, FarGround) => &[SPEED_FAR_GROUND, TRIGGERED],
        (FarConduit | FarGround, FarDeath) => &[SPEED_DEATH_GRIND, TRIGGERED],
        (FarGround, FarAirborne) => &[SPEED_FAR_AIR, BIG_AIR, TRIGGERED],
        (FarAirborne, FarConduit) => &[TRIGGERED, SPEED_DEATH_GRIND],
        (FarAirborne, FarDeath) => &[SPEED_DEATH_GRIND, TRIGGERED],
        _ => &[],
    }
}

#[derive(Clone, Debug)]
struct TagSet {
    tags: &'static [CamTag],
    tracker: TagTracker,
    saved: Vec<f32>,
    updated: bool,
    registered: bool,
    processed: bool,
}

impl TagSet {
    fn new(tags: &'static [CamTag]) -> Option<Self> {
        let windows: Vec<TagWindow> = tags.iter().map(|t| t.window).collect();
        (!tags.is_empty()).then(|| Self {
            tags,
            tracker: TagTracker::new(&windows),
            saved: vec![0.0; tags.len()],
            updated: false,
            registered: false,
            processed: false,
        })
    }

    fn update(&mut self, t: f32) {
        self.updated = true;
        self.registered |= self.tracker.update(t);
    }

    fn monitor(&mut self, s: &mut Shared) {
        let events = if self.registered {
            self.tracker.process()
        } else if self.processed && !self.updated {
            self.tracker.exit()
        } else {
            Vec::new()
        };
        (self.processed, self.registered, self.updated) = (self.registered, false, false);
        for (i, e) in events {
            apply_tag(&self.tags[i], e, &mut self.saved[i], s);
        }
    }
}

fn apply_tag(t: &CamTag, e: TagEvent, saved: &mut f32, s: &mut Shared) {
    fn on_bool(tag: &BoolTag, e: TagEvent, var: &mut bool, saved: &mut f32) {
        let mut sv = *saved != 0.0;
        tag.on(e, var, &mut sv);
        *saved = f32::from(u8::from(sv));
    }
    match (t.var, t.value) {
        (TagVar::EnableMetaBlending, TagValue::Bool(b)) => on_bool(&b, e, &mut s.enable_meta_blending, saved),
        (TagVar::Handycam, TagValue::Bool(b)) => on_bool(&b, e, &mut s.handycam, saved),
        (TagVar::TransitionTriggered, TagValue::Bool(b)) => on_bool(&b, e, &mut s.transition_triggered, saved),
        (TagVar::BigAir, TagValue::Bool(b)) => on_bool(&b, e, &mut s.big_air, saved),
        (TagVar::PlayerOffsetY, TagValue::Float(f)) => f.on(e, &mut s.player_offset_y, saved),
        (TagVar::TransitionSpeedSet, TagValue::Float(f)) => f.on(e, &mut s.transition_speed_set, saved),
        _ => {}
    }
}

fn entry_ok(n: Node, g: &Signals) -> bool {
    match n {
        Node::Airborne => !g.wipeout && !g.grinding,
        Node::Death | Node::ReverseDeath | Node::P15Death | Node::FarDeath => g.dead,
        Node::FarAirborne => g.in_air && !g.wipeout && !g.dead,
        Node::FarGround => !g.dead,
        _ => true,
    }
}

pub fn choose(node: Node, g: &Signals) -> Option<(Node, Blend)> {
    use Blend::{Cut, Default};
    let pick = |list: &[(Node, bool, Blend)]| list.iter().find(|t| t.1 && entry_ok(t.0, g)).map(|t| (t.0, t.2));
    let (air, grind, wipe) = (g.in_air, g.grinding, g.wipeout);
    match node {
        Node::Conduit => pick(&[
            (Node::Grind, grind && !air && !wipe, Default),
            (Node::Airborne, air && !grind && !wipe, Cut),
            (Node::Death, true, Cut),
            (Node::Ground, true, Cut),
        ]),
        Node::Ground => pick(&[
            (Node::Conduit, grind && !wipe && !air, Default),
            (Node::Conduit, air && !wipe && !grind, Default),
            (Node::Death, true, Cut),
        ]),
        Node::Airborne => pick(&[(Node::Death, true, Cut), (Node::Conduit, !air, Cut)]),
        Node::Grind => pick(&[(Node::Death, true, Default), (Node::Conduit, !grind, Default)]),
        Node::Death => pick(&[(Node::Conduit, !g.dead, Cut)]),
        Node::ReverseConduit => pick(&[(Node::ReverseDeath, true, Default), (Node::ReverseGround, true, Cut)]),
        Node::ReverseGround => pick(&[(Node::ReverseDeath, true, Default)]),
        Node::ReverseDeath => None,
        Node::P15Conduit => pick(&[
            (Node::P15Heli, g.heli_active && !wipe && !grind, Default),
            (Node::P15Airborne, air && !wipe && !grind, Cut),
            (Node::P15Wipeout, wipe && !air && !grind, Cut),
            (Node::P15Rails, grind && !air && !wipe, Cut),
            (Node::P15Ground, !air && !wipe && !grind, Cut),
            (Node::P15Death, true, Cut),
        ]),
        Node::P15Airborne => pick(&[
            (Node::P15Conduit, !air, Cut),
            (Node::P15Conduit, g.heli_active, Default),
            (Node::P15Death, true, Cut),
        ]),
        Node::P15Heli => pick(&[(Node::P15Conduit, !g.heli_active, Default), (Node::P15Death, true, Default)]),
        Node::P15Ground => pick(&[(Node::P15Conduit, grind || wipe || air, Cut), (Node::P15Death, true, Cut)]),
        Node::P15Rails => pick(&[(Node::P15Conduit, !grind, Cut), (Node::P15Death, true, Cut)]),
        Node::P15Wipeout => pick(&[(Node::P15Conduit, !wipe, Cut), (Node::P15Death, true, Cut)]),
        Node::P15Death => pick(&[(Node::P15Conduit, !g.dead, Cut)]),
        Node::FarConduit => {
            pick(&[(Node::FarDeath, true, Cut), (Node::FarAirborne, true, Cut), (Node::FarGround, true, Cut)])
        }
        Node::FarGround => pick(&[(Node::FarDeath, true, Cut), (Node::FarAirborne, true, Cut)]),
        Node::FarAirborne => pick(&[(Node::FarDeath, true, Cut), (Node::FarConduit, !air, Cut)]),
        Node::FarDeath => pick(&[(Node::FarConduit, !g.dead, Cut)]),
    }
}

#[derive(Clone, Debug)]
pub struct NodeInst {
    pub node: Node,
    graph: GraphState,
    fade: Option<Box<Fade>>,
    time: f32,
    trans_time: f32,
    updated: bool,
    tags: Option<TagSet>,
    trans: Vec<TagSet>,
}

#[derive(Clone, Debug)]
struct Fade {
    elapsed: f32,
    old: NodeInst,
}

fn run(node: Node, graph: &mut GraphState, s: &mut Shared, i: &Inputs, rig: &Channels, dt: f32) -> Channels {
    let camera_pos = s.camera_pos;
    match node {
        Node::Conduit | Node::Ground => step(Variant::Ground, graph, s, i, dt),
        Node::Airborne => step(Variant::Air, graph, s, i, dt),
        Node::Grind => step(Variant::Grind, graph, s, i, dt),
        Node::Death | Node::ReverseDeath | Node::P15Death | Node::FarDeath => step_death(s, i, rig),
        Node::FarConduit | Node::FarGround => step(Variant::FarGround, graph, s, i, dt),
        Node::FarAirborne => step(Variant::FarAir, graph, s, i, dt),
        Node::ReverseConduit | Node::ReverseGround => step_reverse(s, i),
        Node::P15Conduit | Node::P15Ground => step_15p(Variant15::Ground, graph, s, i, camera_pos, dt),
        Node::P15Airborne => step_15p(Variant15::Air, graph, s, i, camera_pos, dt),
        Node::P15Rails => step_15p(Variant15::Rails, graph, s, i, camera_pos, dt),
        Node::P15Wipeout => step_15p(Variant15::Wipeout, graph, s, i, camera_pos, dt),
        Node::P15Heli => step_15p(Variant15::Heli, graph, s, i, camera_pos, dt),
    }
}

impl NodeInst {
    fn new(node: Node) -> Self {
        Self {
            node,
            graph: GraphState::default(),
            fade: None,
            time: 0.0,
            trans_time: 0.0,
            updated: false,
            tags: TagSet::new(node_tags(node)),
            trans: Vec::new(),
        }
    }

    fn release_into(self, out: &mut Vec<TagSet>) {
        out.extend(self.tags);
        out.extend(self.trans);
        if let Some(f) = self.fade {
            f.old.release_into(out);
        }
    }

    fn take(&mut self, to: Node) {
        self.trans.extend(TagSet::new(transition_tags(self.node, to)));
        if !self.updated
            && let Some(t) = &mut self.tags
        {
            t.update(self.time);
        }
        for t in &mut self.trans {
            t.update(self.trans_time);
        }
    }

    fn update_tags(&mut self, dt: f32) {
        for t in &mut self.trans {
            t.update(self.trans_time);
        }
        if let Some(t) = &mut self.tags {
            t.update(self.time);
        }
        self.updated = true;
        if !self.trans.is_empty() {
            self.trans_time += dt;
        }
        self.time += dt;
    }

    fn monitor(&mut self, s: &mut Shared) {
        for t in self.tags.iter_mut().chain(self.trans.iter_mut()) {
            t.monitor(s);
        }
        if let Some(f) = &mut self.fade {
            f.old.monitor(s);
        }
    }

    #[cfg(test)]
    pub fn depth(&self) -> usize {
        self.fade.as_ref().map_or(0, |f| 1 + f.old.depth())
    }

    fn update(&mut self, s: &mut Shared, i: &Inputs, rig: &Channels, dt: f32, released: &mut Vec<TagSet>) -> Channels {
        if self.fade.as_ref().is_some_and(|f| FADE_TICKS - f.elapsed <= dt)
            && let Some(f) = self.fade.take()
        {
            f.old.release_into(released);
        }
        let mut w = 0.0;
        if let Some(f) = self.fade.as_mut() {
            f.elapsed = (f.elapsed + dt.max(0.0)).min(FADE_TICKS);
            w = curve_weight(f.elapsed / FADE_TICKS, 0);
        }
        if w <= 0.01
            && let Some(f) = self.fade.take()
        {
            f.old.release_into(released);
        }
        let new = run(self.node, &mut self.graph, s, i, rig, dt);
        let out = match self.fade.as_mut() {
            Some(f) => blend(&new, &f.old.update(s, i, rig, dt, released), w),
            None => new,
        };
        self.update_tags(dt);
        out
    }
}

#[derive(Clone, Debug)]
pub struct Flow {
    pub current: NodeInst,
    released: Vec<TagSet>,
}

impl Default for Flow {
    fn default() -> Self {
        Self { current: NodeInst::new(Node::Conduit), released: Vec::new() }
    }
}

impl Flow {
    pub fn reverse() -> Self {
        Self { current: NodeInst::new(Node::ReverseConduit), released: Vec::new() }
    }

    pub fn far() -> Self {
        Self { current: NodeInst::new(Node::FarConduit), released: Vec::new() }
    }

    pub fn cam15p() -> Self {
        Self { current: NodeInst::new(Node::P15Conduit), released: Vec::new() }
    }

    pub fn update(&mut self, s: &mut Shared, i: &Inputs, rig: &Channels, dt: f32) -> Channels {
        let g = Signals::of(s, i);
        for _ in 0..25 {
            let Some((to, blend)) = choose(self.current.node, &g) else { break };
            let mut old = std::mem::replace(&mut self.current, NodeInst::new(to));
            old.take(to);
            if blend == Blend::Default {
                self.current.fade = Some(Box::new(Fade { elapsed: 0.0, old }));
            } else {
                old.release_into(&mut self.released);
            }
        }
        self.current.update(s, i, rig, dt, &mut self.released)
    }

    pub fn monitor(&mut self, s: &mut Shared) {
        self.current.monitor(s);
        self.released.retain_mut(|t| {
            t.monitor(s);
            t.processed
        });
    }
}
