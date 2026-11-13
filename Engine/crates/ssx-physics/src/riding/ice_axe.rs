use anyhow::Result;

use ssx_assets::anim_eval::curve_weight;

use crate::curve::Curve;
use crate::tags::{BoolTag, FloatTag, TagEvent, TagTracker, TagWindow};
use crate::tuning::AssetView;

#[derive(Clone, Debug, Default)]
pub struct IceAxeTuning {
    pub max_lean_angle: f32,
    pub equilibrium_lean_angle: f32,
    pub lean_activation_threshold: f32,
    pub blend_activation_threshold: f32,
    pub max_lean_input_curve: Curve,
    pub lean_input_transform: Curve,
    pub chop_lean_threshold: f32,
    pub steering_transform: Curve,
    pub steering_transform_full: Curve,
    pub steering_transform_blend: Curve,
    pub steering_filter_gain: f32,
    pub surface_weight_threshold: f32,
    pub noise_angle_normalization: f32,
}

impl IceAxeTuning {
    pub fn load(a: &AssetView<'_>) -> Result<Self> {
        Ok(Self {
            max_lean_angle: a.f32("MaxLeanAngle")?,
            equilibrium_lean_angle: a.f32("EquilibriumLeanAngle")?,
            lean_activation_threshold: a.f32("IceAxeSurfacePropertyLeanActivationThreshold")?,
            blend_activation_threshold: a.f32("IceAxeSurfacePropertyBlendActivationThreshold")?,
            max_lean_input_curve: a.curve("SpeedVsMaxLeanInputAngleCurveAsset")?,
            lean_input_transform: a.curve("LeanInputAngleTransform")?,
            chop_lean_threshold: a.f32("IceAxeChopLeanThreshold")?,
            steering_transform: a.curve("AxeSteeringInputAngleTransform")?,
            steering_transform_full: a.curve("AxeSteeringInputAngleTransformFull")?,
            steering_transform_blend: a.curve("AxeSteeringInputAngleTransformBlendCurve")?,
            steering_filter_gain: a.f32("IceAxeSteeringInputFilterGain")?,
            surface_weight_threshold: a.f32("IceAxeSurfaceWeightThreshold")?,
            noise_angle_normalization: a.f32("IceAxeNoiseAngleNormalizationValue")?,
        })
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct IceAxeFrame {
    pub blend: f32,
    pub effectiveness: f32,
    pub active: bool,
    pub steering_value: f32,
    pub dig_modifier: f32,
}

pub const DEPLOY_SPLINE: [f32; 11] = [
    0.0,
    0.072_723_05,
    0.172_361_37,
    0.278_821_6,
    0.389_489_53,
    0.5,
    0.610_510_47,
    0.721_178_5,
    0.827_638_6,
    0.927_276_97,
    1.0,
];

pub const LAYER_SPLINE: [f32; 16] = [
    0.0,
    0.042_968_75,
    0.105_001_95,
    0.172_361_37,
    0.243_093_28,
    0.316_406_25,
    0.389_489_53,
    0.462_531_27,
    0.537_468_7,
    0.610_510_47,
    0.683_593_75,
    0.756_906_6,
    0.827_638_6,
    0.894_998,
    0.957_031_25,
    1.0,
];

pub fn tag_spline(s: &[f32], w: f32) -> f32 {
    let Some(&last) = s.last() else { return 0.0 };
    let x = (s.len() - 1) as f32 * w;
    let i = x as usize;
    if i + 1 >= s.len() {
        return last;
    }
    let f = x - i as f32;
    (1.0 - f) * s[i] + s[i + 1] * f
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum DeployNode {
    #[default]
    Holstered,
    Deploy(f32),
    Deployed,
}

pub const DEPLOY_END_TICKS: f32 = 57.0;
const DEPLOY_WINDOWS: [TagWindow; 3] =
    [TagWindow::new(0.0, 15.0, false), TagWindow::new(30.0, 10.0, false), TagWindow::new(40.0, 10.0, false)];

pub const CHOP_END_TICKS: f32 = 29.0;
const CHOP_WINDOWS: [TagWindow; 3] =
    [TagWindow::new(6.0, 0.0, false), TagWindow::new(26.0, 0.0, false), TagWindow::new(26.0, 0.0, false)];
const CHOPPED_TAG: BoolTag = BoolTag { value: true, mode: 1, reset: None };
const CHOP_WEIGHT_TAG: FloatTag = FloatTag { value: 0.0, mode: 1, reset: Some(0.0), spline: 1.0 };
const START_CHOP_WINDOW: [TagWindow; 1] = [TagWindow::new(0.0, 1.0, false)];
const START_CHOP_TAG: BoolTag = BoolTag { value: false, mode: 1, reset: None };
const CHOP_MAX_ACTIVE: usize = 5;

#[derive(Clone, Copy, Debug, PartialEq)]
enum ChopBlend {
    Default,
    Accel(f32),
}

impl ChopBlend {
    fn time(self) -> f32 {
        match self {
            Self::Default => 8.0,
            Self::Accel(t) => t,
        }
    }

    fn new_weight(self, elapsed: f32) -> f32 {
        let time = self.time();
        let t = if time <= 0.0 { 1.0 } else { (elapsed / time).clamp(0.0, 1.0) };
        match self {
            Self::Default => 1.0 - curve_weight(t, 0),
            Self::Accel(_) => t,
        }
    }

    fn done(self, elapsed: f32, dt: f32) -> bool {
        self.time() - elapsed <= dt || self.new_weight(elapsed) >= 0.99
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum ChopNode {
    Neutral,
    Chop(Box<ChopActor>),
    Chopped,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ChopActor {
    pub time: f32,
    tags: TagTracker,
    node_tags: TagTracker,
}

impl ChopActor {
    fn new() -> Self {
        Self { time: 0.0, tags: TagTracker::new(&CHOP_WINDOWS), node_tags: TagTracker::new(&START_CHOP_WINDOW) }
    }
}

#[derive(Clone, Debug, PartialEq)]
struct ChopLink {
    node: ChopNode,
    clock: f32,
    blend: Option<(ChopBlend, f32)>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum ChopCall {
    Actor(usize, TagEvent),
    StartChop(usize, TagEvent),
    Weight(usize, f32),
}

#[derive(Debug, Default)]
struct ChopMonitor {
    exits: Vec<ChopCall>,
    registered: Vec<ChopCall>,
}

impl ChopMonitor {
    fn exit(&mut self, side: usize, link: &mut ChopLink) {
        if let ChopNode::Chop(a) = &mut link.node {
            self.exits.extend(a.tags.exit().into_iter().map(|(i, e)| ChopCall::Actor(i, e)));
            self.exits.extend(a.node_tags.exit().into_iter().map(|(_, e)| ChopCall::StartChop(side, e)));
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ChopLayer {
    chain: Vec<ChopLink>,
    first: bool,
}

impl Default for ChopLayer {
    fn default() -> Self {
        Self { chain: vec![ChopLink { node: ChopNode::Neutral, clock: 0.0, blend: None }], first: true }
    }
}

impl ChopLayer {
    pub fn node(&self) -> Option<&ChopNode> {
        self.chain.first().map(|l| &l.node)
    }

    fn next(&self, side: usize, trigger: bool) -> Option<(f32, ChopNode, ChopBlend, Option<f32>)> {
        match &self.chain.first()?.node {
            ChopNode::Neutral if trigger => {
                let blend = if side == 0 { ChopBlend::Accel(0.0) } else { ChopBlend::Default };
                Some((0.0, ChopNode::Chop(Box::new(ChopActor::new())), blend, Some(1.0)))
            }
            ChopNode::Chop(a) => {
                let to_end = CHOP_END_TICKS - a.time;
                let end = (to_end >= 0.0).then_some((to_end, ChopNode::Chopped, ChopBlend::Accel(15.0), None));
                match end {
                    Some(e) if e.0 <= 0.0 => Some(e),
                    _ if !trigger => Some((0.0, ChopNode::Neutral, ChopBlend::Default, Some(0.0))),
                    e => e,
                }
            }
            ChopNode::Chopped if !trigger => Some((0.0, ChopNode::Neutral, ChopBlend::Default, None)),
            _ => None,
        }
    }

    fn update(&mut self, side: usize, trigger: bool, mut dt: f32, m: &mut ChopMonitor) {
        let first = std::mem::replace(&mut self.first, false);
        let mut advanced = false;
        for _ in 0..25 {
            let Some((t_next, to, blend, weight)) = self.next(side, trigger) else { break };
            if dt <= t_next && !(t_next <= 0.0 && first) {
                break;
            }
            if t_next > 0.0 {
                self.update_link(0, side, t_next, m);
                dt -= t_next;
                advanced = true;
            }
            self.take(side, to, blend, weight, m);
        }
        if !advanced || dt > 0.001 {
            self.update_link(0, side, dt, m);
        }
    }

    fn take(&mut self, side: usize, to: ChopNode, blend: ChopBlend, weight: Option<f32>, m: &mut ChopMonitor) {
        if let Some(w) = weight {
            m.registered.push(ChopCall::Weight(side, w));
        }
        self.chain.insert(0, ChopLink { node: to, clock: 0.0, blend: Some((blend, 0.0)) });
        if self.chain.len() > CHOP_MAX_ACTIVE {
            for mut l in self.chain.drain(CHOP_MAX_ACTIVE..) {
                m.exit(side, &mut l);
            }
        }
    }

    fn update_link(&mut self, i: usize, side: usize, dt: f32, m: &mut ChopMonitor) {
        let Some(link) = self.chain.get_mut(i) else { return };
        let older = match link.blend.as_mut() {
            Some((b, elapsed)) if !b.done(*elapsed, dt) => {
                *elapsed = (*elapsed + dt).min(b.time().max(0.0));
                true
            }
            _ => {
                link.blend = None;
                false
            }
        };
        if older {
            self.update_link(i + 1, side, dt, m);
        } else if self.chain.len() > i + 1 {
            for mut l in self.chain.drain(i + 1..) {
                m.exit(side, &mut l);
            }
        }
        let Some(link) = self.chain.get_mut(i) else { return };
        if let ChopNode::Chop(a) = &mut link.node {
            a.time += dt;
            if a.tags.update(a.time) {
                m.registered.extend(a.tags.process().into_iter().map(|(t, e)| ChopCall::Actor(t, e)));
            }
            if a.node_tags.update(link.clock) {
                m.registered.extend(a.node_tags.process().into_iter().map(|(_, e)| ChopCall::StartChop(side, e)));
            }
        }
        link.clock += dt;
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct IceAxeState {
    pub enabled: bool,
    pub deploy: DeployNode,
    deploy_tags: Option<TagTracker>,
    deploy_saved: [f32; 3],
    pub deployed: bool,
    pub blend_weight: f32,
    pub layer_weight: f32,
    pub noise: [f32; 2],
    pub steering_value: f32,
    pub effects_timer: f32,
    pub chopped: bool,
    pub start_chop: [bool; 2],
    pub chop_weight: [f32; 2],
    pub layers: [ChopLayer; 2],
    pub frame: IceAxeFrame,
}

impl IceAxeState {
    pub fn enter_gameplay(&mut self) {
        (self.deploy, self.deploy_tags, self.deploy_saved) = (DeployNode::Holstered, None, [0.0; 3]);
        self.layers = Default::default();
    }

    pub fn blend(&self, t: &IceAxeTuning, ice_weight: f32) -> f32 {
        let on_ice = if ice_weight > t.surface_weight_threshold { 1.0 } else { 0.0 };
        self.blend_weight * on_ice
    }

    #[allow(clippy::too_many_arguments)]
    pub fn read_inputs(
        &mut self,
        t: &IceAxeTuning,
        effectiveness: f32,
        ice_weight: f32,
        lean_prev: f32,
        stick_angle: f32,
        dig_modifier: f32,
        dt: f32,
    ) -> IceAxeFrame {
        let blend = self.blend(t, ice_weight);
        let active =
            blend >= t.blend_activation_threshold && lean_prev.abs() >= t.lean_activation_threshold && self.chopped;
        let a = stick_angle.abs();
        let lo = t.steering_transform.eval(a);
        let target =
            lo + (t.steering_transform_full.eval(a) - lo) * t.steering_transform_blend.eval(self.effects_timer);
        self.effects_timer = if active { self.effects_timer + dt } else { 0.0 };
        let e = (-(t.steering_filter_gain * dt)).exp();
        self.steering_value = e * (self.steering_value - target) + target;
        let f = self.steering_value.abs() / t.noise_angle_normalization;
        let r = if -f >= 0.0 { 0.0 } else { f };
        let r = if 1.0 - r >= 0.0 { r } else { 1.0 };
        let targets = [if lean_prev >= 0.0 { 0.0 } else { r }, if lean_prev > 0.0 { r } else { 0.0 }];
        for (n, target) in self.noise.iter_mut().zip(targets) {
            *n = e * (*n - target) + target;
        }
        self.frame = IceAxeFrame { blend, effectiveness, active, steering_value: self.steering_value, dig_modifier };
        self.frame
    }

    pub fn chop_update(&mut self, t: &IceAxeTuning, lean: f32) {
        if lean.abs() < t.lean_activation_threshold {
            self.chopped = false;
        }
        let chop = lean.abs() > t.chop_lean_threshold && (self.frame.blend - 1.0).abs() <= 1.525_878_9e-5;
        self.start_chop = [lean < 0.0 && chop, lean > 0.0 && chop];
    }

    pub fn layers_update(&mut self, enabled: bool, dt: f32) {
        self.enabled = enabled;
        match self.deploy {
            DeployNode::Holstered if enabled => {
                self.deploy = DeployNode::Deploy(dt);
                self.deploy_tags = Some(TagTracker::new(&DEPLOY_WINDOWS));
            }
            DeployNode::Deploy(t) if t + dt >= DEPLOY_END_TICKS => {
                self.deploy = DeployNode::Deployed;
                self.deployed = true;
            }
            DeployNode::Deploy(t) => self.deploy = DeployNode::Deploy(t + dt),
            _ => {}
        }
        if let Some(tags) = self.deploy_tags.as_mut() {
            let events = match self.deploy {
                DeployNode::Deploy(t) if tags.update(t) => tags.process(),
                DeployNode::Deploy(_) => Vec::new(),
                _ => tags.exit(),
            };
            for (i, e) in events {
                let w = match e {
                    TagEvent::Apply(w) | TagEvent::Leave(w) => w,
                    TagEvent::Enter => 0.0,
                };
                let (value, spline, var) = match i {
                    0 => (1.0, tag_spline(&LAYER_SPLINE, w), &mut self.layer_weight),
                    1 => (1.0, tag_spline(&DEPLOY_SPLINE, w), &mut self.blend_weight),
                    _ => (0.0, tag_spline(&DEPLOY_SPLINE, w), &mut self.layer_weight),
                };
                FloatTag { value, mode: 3, reset: None, spline }.on(e, var, &mut self.deploy_saved[i]);
            }
            if self.deploy == DeployNode::Deployed {
                self.deploy_tags = None;
            }
        }
        let mut m = ChopMonitor::default();
        for (side, layer) in self.layers.iter_mut().enumerate() {
            layer.update(side, self.start_chop[side], dt, &mut m);
        }
        self.monitor(m);
    }

    fn monitor(&mut self, m: ChopMonitor) {
        let (mut saved_bool, mut saved_float) = (false, 0.0);
        for call in m.exits.into_iter().chain(m.registered) {
            match call {
                ChopCall::Actor(0, e) => CHOPPED_TAG.on(e, &mut self.chopped, &mut saved_bool),
                ChopCall::Actor(1, e) => CHOP_WEIGHT_TAG.on(e, &mut self.chop_weight[1], &mut saved_float),
                ChopCall::Actor(_, e) => CHOP_WEIGHT_TAG.on(e, &mut self.chop_weight[0], &mut saved_float),
                ChopCall::StartChop(side, e) => START_CHOP_TAG.on(e, &mut self.start_chop[side], &mut saved_bool),
                ChopCall::Weight(side, w) => self.chop_weight[side] = w,
            }
        }
    }

    pub fn chop(&self) -> f32 {
        self.chop_weight[0].max(self.chop_weight[1])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tuning() -> IceAxeTuning {
        IceAxeTuning {
            max_lean_angle: std::f32::consts::FRAC_PI_2,
            equilibrium_lean_angle: 1.396_263_4,
            lean_activation_threshold: 0.2,
            blend_activation_threshold: 0.9,
            chop_lean_threshold: 0.2,
            steering_filter_gain: 0.1,
            surface_weight_threshold: 0.6,
            noise_angle_normalization: std::f32::consts::FRAC_PI_6,
            steering_transform: Curve::new(vec![[0.0, 0.0], [1.0, 1.0]]),
            ..IceAxeTuning::default()
        }
    }

    #[test]
    fn spline_interpolates_uniform_samples() {
        assert_eq!(tag_spline(&DEPLOY_SPLINE, 0.0), 0.0);
        assert_eq!(tag_spline(&DEPLOY_SPLINE, 0.5), 0.5);
        assert!((tag_spline(&DEPLOY_SPLINE, 0.05) - 0.036_361_52).abs() < 1e-6);
        assert_eq!(tag_spline(&DEPLOY_SPLINE, 1.0), 1.0);
    }

    #[test]
    fn the_axe_deploys_when_equipped() {
        let mut s = IceAxeState::default();
        let (mut blends, mut layer) = (Vec::new(), Vec::new());
        for _ in 0..32 {
            s.layers_update(true, 2.0);
            blends.push(s.blend_weight);
            layer.push(s.layer_weight);
        }
        assert!((layer[0] - tag_spline(&LAYER_SPLINE, 2.0 / 15.0)).abs() < 1e-6, "tick 2");
        assert!((layer[6] - tag_spline(&LAYER_SPLINE, 14.0 / 15.0)).abs() < 1e-6, "tick 14");
        assert_eq!(layer[7], layer[6], "tick 16: left");
        assert_eq!(layer[19], layer[6], "tick 40: the fade-out starts from it");
        assert_eq!(layer[24], 0.0, "tick 50");
        assert_eq!(blends[13], 0.0, "tick 28");
        assert_eq!(blends[14], 0.0, "tick 30: the window opens at weight 0");
        assert!((blends[15] - 0.172_361_37).abs() < 1e-6, "tick 32: s(0.2)");
        assert_eq!(blends[19], 1.0, "tick 40");
        assert!(s.deployed && s.deploy == DeployNode::Deployed);
        let mut bare = IceAxeState::default();
        bare.layers_update(false, 2.0);
        assert_eq!(bare.deploy, DeployNode::Holstered);
    }

    fn step(s: &mut IceAxeState, trigger: [bool; 2]) {
        s.start_chop = trigger;
        s.layers_update(false, 2.0);
    }

    #[test]
    fn a_chop_ends_at_weight_0() {
        let t = tuning();
        let mut s = IceAxeState { blend_weight: 1.0, ..IceAxeState::default() };
        let f = s.read_inputs(&t, 1.0, 0.9, 0.5, 0.3, 0.25, 2.0);
        assert!(f.blend == 1.0 && !f.active, "not chopped yet");
        s.chop_update(&t, 0.5);
        assert_eq!(s.start_chop, [false, true]);
        let mut chopped_at = None;
        let mut weights = Vec::new();
        for i in 0..15 {
            step(&mut s, [false, true]);
            weights.push(s.chop_weight[1]);
            if s.chopped && chopped_at.is_none() {
                chopped_at = Some(i);
            }
        }
        assert_eq!(chopped_at, Some(2), "actor ticks 2, 4, 6");
        assert_eq!((weights[0], weights[11]), (1.0, 1.0), "ticks 2…24");
        assert_eq!(weights[12], 0.0, "tick 26");
        assert_eq!(weights[13], 0.0, "tick 28: the leave writes the default 0");
        assert!(matches!(s.layers[1].node(), Some(ChopNode::Chopped)) && s.chop() == 0.0, "End at 29");
        step(&mut s, [false, false]);
        assert!(matches!(s.layers[1].node(), Some(ChopNode::Neutral)) && s.chop() == 0.0, "chopped → neutral");
        s.start_chop = [false, true];
        assert!(s.read_inputs(&t, 1.0, 0.9, 0.5, 0.3, 0.25, 2.0).active, "chopped and leaning: the bite");
        assert!(!s.read_inputs(&t, 1.0, 0.1, 0.5, 0.3, 0.25, 2.0).active, "off ice");
        s.chop_update(&t, 0.1);
        assert!(!s.chopped && s.start_chop == [false, false]);
    }

    #[test]
    fn the_chop_node_clears_its_trigger() {
        let mut s = IceAxeState::default();
        step(&mut s, [true, false]);
        assert_eq!((s.start_chop, s.chop_weight), ([false, false], [1.0, 0.0]));
        s.layers_update(false, 2.0);
        assert!(matches!(s.layers[0].node(), Some(ChopNode::Neutral)) && s.chop_weight == [0.0, 0.0]);
    }

    #[test]
    fn an_interrupted_chop_runs_on_in_its_blend() {
        let mut s = IceAxeState::default();
        step(&mut s, [false, true]);
        step(&mut s, [false, false]);
        assert!(matches!(s.layers[1].node(), Some(ChopNode::Neutral)) && s.chop_weight[1] == 0.0 && !s.chopped);
        step(&mut s, [false, false]);
        assert!(s.chopped, "the old side's actor at tick 6");
        step(&mut s, [false, false]);
        step(&mut s, [false, false]);
        assert_eq!(s.layers[1].chain.len(), 1, "the blend dropped");
    }

    #[test]
    fn a_new_chop_and_the_old_side() {
        for side in 0..2 {
            let on = |b: bool| if side == 0 { [b, false] } else { [false, b] };
            let mut s = IceAxeState::default();
            for _ in 0..11 {
                step(&mut s, on(true));
            }
            step(&mut s, on(false));
            assert_eq!(s.chop_weight[side], 0.0, "chop → neutral");
            step(&mut s, on(true));
            let expect = if side == 0 { 1.0 } else { 0.0 };
            assert_eq!(s.chop_weight[side], expect, "side {side}");
        }
    }
}
