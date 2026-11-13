mod animator;
mod state_flow;

use std::collections::HashMap;
use std::f32::consts::FRAC_PI_2;

use crate::anim_graph::{Blend, BlendId, ContextField, Ctrl, CtrlId, Graph, SfNode, SigId, StateFlow};
use crate::anim_pose::{AnimPose, Delta, StepPhysics};
use crate::rig_anim::{RigClip, RigPose};

pub use animator::*;
pub use state_flow::*;

#[derive(Debug)]
pub struct GameState {
    values: Vec<std::sync::atomic::AtomicU32>,
    pub random: RandomState,
    pub audio_tags: Vec<AudioTagEvent>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AudioTagEvent {
    pub name: String,
    pub system: i64,
    pub event: i64,
}

#[derive(Debug, Default)]
pub struct RandomState(std::sync::atomic::AtomicU32);

impl Clone for RandomState {
    fn clone(&self) -> Self {
        Self(std::sync::atomic::AtomicU32::new(self.0.load(std::sync::atomic::Ordering::Relaxed)))
    }
}

impl Clone for GameState {
    fn clone(&self) -> Self {
        Self::from_values((0..self.values.len()).map(|i| self.get(i)).collect(), self.random.clone())
    }
}

impl GameState {
    pub fn from_values(values: Vec<f32>, random: RandomState) -> Self {
        Self {
            values: values.into_iter().map(|v| std::sync::atomic::AtomicU32::new(v.to_bits())).collect(),
            random,
            audio_tags: Vec::new(),
        }
    }

    pub fn len(&self) -> usize {
        self.values.len()
    }

    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    pub fn random_u16(&self) -> u16 {
        use std::sync::atomic::Ordering::Relaxed;
        let s = self.random.0.load(Relaxed).wrapping_mul(1_103_515_245).wrapping_add(12_345);
        self.random.0.store(s, Relaxed);
        (s >> 16) as u16
    }

    pub fn new(g: &Graph) -> Self {
        let values = g.signals.iter().map(|s| s.kind.default_value()).collect();
        Self::from_values(values, RandomState(std::sync::atomic::AtomicU32::new(g.random_seed.unwrap_or(0))))
    }

    pub fn get(&self, s: SigId) -> f32 {
        self.values.get(s).map_or(0.0, |x| f32::from_bits(x.load(std::sync::atomic::Ordering::Relaxed)))
    }

    pub fn bool(&self, s: SigId) -> bool {
        self.get(s) != 0.0
    }

    pub fn set(&self, s: SigId, v: f32) {
        if let Some(x) = self.values.get(s) {
            x.store(v.to_bits(), std::sync::atomic::Ordering::Relaxed);
        }
    }

    pub fn merge_from(&self, other: &GameState, keep: &[SigId]) {
        for i in 0..self.len().min(other.len()) {
            if !keep.contains(&i) {
                self.set(i, other.get(i));
            }
        }
        self.random
            .0
            .store(other.random.0.load(std::sync::atomic::Ordering::Relaxed), std::sync::atomic::Ordering::Relaxed);
    }
}

pub struct Env<'a> {
    pub g: &'a Graph,
    pub clips: &'a [RigClip],
    pub clip_index: &'a HashMap<ssx_formats::ant::Guid, usize>,
    pub bind: &'a RigPose,
    pub rig: &'a crate::anim_pose::PoseRig,
    pub physics: Option<&'a StepPhysics>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Window {
    pub kind: String,
    pub filter: u32,
    pub time: f32,
    pub length: f32,
    pub blend: Option<BlendId>,
}

pub fn curve_weight(t: f32, curve: i64) -> f32 {
    let t = t.clamp(0.0, 1.0);
    let f = |x: f32| match curve {
        1 if x <= 0.0 => 1.0,
        1 if x >= 1.0 => 0.0,
        1 => 1.0 / (1.0 + (8.0 * (x - 0.5)).exp()),
        2 => 1.0 - (x * FRAC_PI_2).sin(),
        _ => (x * FRAC_PI_2).cos(),
    };
    if curve == 3 {
        return 1.0 - t;
    }
    let p = t * 20.0;
    let i = p.floor().min(19.0);
    let (a, b) = (f(i / 20.0), f((i + 1.0) / 20.0));
    if t >= 1.0 { 0.0 } else { a + (b - a) * (p - i) }
}

pub trait Controller: Send + Sync {
    fn update(&mut self, env: &Env, gs: &mut GameState, dt: f32);
    fn pose(&self, env: &Env, gs: &GameState, out: &mut AnimPose);
    fn time(&self) -> f32;
    fn length(&self) -> f32 {
        0.0
    }
    fn looping(&self) -> bool {
        false
    }
    fn set_time(&mut self, _env: &Env, _gs: &GameState, _t: f32) {}
    fn windows(&self, _env: &Env) -> Vec<Window> {
        Vec::new()
    }
    fn label(&self, env: &Env, weight: f32, out: &mut Vec<(String, f32)>);
    fn path(&self, _env: &Env, _out: &mut Vec<(String, f32)>) {}
    fn as_flow(&self) -> Option<&SfInst> {
        None
    }
    fn exit(&mut self, _env: &Env, _gs: &GameState) {}
    fn delta(&self, _env: &Env, _gs: &GameState) -> Delta {
        Delta::Untraced
    }
}

pub fn blend_delta(env: &Env, blend: &Blend, elapsed: f32, old: Delta, new: Delta) -> Delta {
    match blend.class.as_str() {
        "AccelBlendAsset" => new,
        "BlendCurveWithJointPivotAsset" | "DiffBlendAsset" => Delta::Untraced,
        "TimeScaleBlendAsset" if blend.dof_curves.is_some() => {
            let t = blend_new_weight(blend, elapsed);
            let set = blend.dof_curves.as_ref().and_then(|c| c.sets.iter().find(|s| s.layout == "DeltaTrajectory"));
            Delta::blend(old, new, set.map_or(t, |s| s.weight(0, t)), env.physics)
        }
        _ => Delta::blend(old, new, blend_new_weight(blend, elapsed), env.physics),
    }
}

pub fn apply_blend(env: &Env, blend: &Blend, elapsed: f32, old: &mut AnimPose, new: &AnimPose) {
    let w = blend_new_weight(blend, elapsed);
    match &blend.dof_curves {
        Some(c) => crate::anim_pose::blend_masked(old, new, 1.0, Some(&env.rig.dof_weights(c, w))),
        None => old.blend(new, w),
    }
}

pub struct BlendInst {
    pub blend: Blend,
    pub elapsed: f32,
    pub prev: Box<NodeInst>,
}

pub fn blend_new_weight(blend: &Blend, elapsed: f32) -> f32 {
    let t = if blend.blend_time <= 0.0 { 1.0 } else { (elapsed / blend.blend_time).clamp(0.0, 1.0) };
    match blend.class.as_str() {
        "BlendCurveAsset" | "BlendCurveWithJointPivotAsset" => 1.0 - curve_weight(t, blend.curve),
        "BezierBlendAsset" => {
            let s = &blend.samples;
            let x = t * s.len() as f32;
            let i = x as usize;
            if i >= s.len() {
                1.0
            } else {
                let next = s.get(i + 1).copied().unwrap_or(1.0);
                let f = (x - i as f32).clamp(0.0, 1.0);
                (1.0 - f) * s[i] + f * next
            }
        }
        _ => t,
    }
}

impl BlendInst {
    pub fn new_weight(&self) -> f32 {
        blend_new_weight(&self.blend, self.elapsed)
    }

    fn done(&self, dt: f32) -> bool {
        self.blend.blend_time - self.elapsed <= dt || self.new_weight() >= 0.99
    }
}

pub struct ModularInst {
    items: Vec<Box<dyn Controller>>,
}

impl Controller for ModularInst {
    fn update(&mut self, env: &Env, gs: &mut GameState, dt: f32) {
        self.items.iter_mut().for_each(|i| i.update(env, gs, dt));
    }
    fn pose(&self, env: &Env, gs: &GameState, out: &mut AnimPose) {
        self.items.iter().for_each(|i| i.pose(env, gs, out));
    }
    fn time(&self) -> f32 {
        self.items.last().map_or(0.0, |i| i.time())
    }
    fn windows(&self, env: &Env) -> Vec<Window> {
        self.items.iter().flat_map(|i| i.windows(env)).collect()
    }
    fn label(&self, env: &Env, weight: f32, out: &mut Vec<(String, f32)>) {
        self.items.iter().for_each(|i| i.label(env, weight, out));
    }
    fn exit(&mut self, env: &Env, gs: &GameState) {
        self.items.iter_mut().for_each(|i| i.exit(env, gs));
    }
}

pub struct EmptyInst;

impl Controller for EmptyInst {
    fn update(&mut self, _env: &Env, _gs: &mut GameState, _dt: f32) {}
    fn delta(&self, _env: &Env, _gs: &GameState) -> Delta {
        Delta::None
    }
    fn pose(&self, _env: &Env, _gs: &GameState, _out: &mut AnimPose) {}
    fn time(&self) -> f32 {
        0.0
    }
    fn label(&self, env: &Env, weight: f32, out: &mut Vec<(String, f32)>) {
        let _ = env;
        out.push(("(no pose)".into(), weight));
    }
}

pub fn instantiate(env: &Env, gs: &GameState, ctrl: CtrlId) -> Box<dyn Controller> {
    instantiate_chain(env, gs, &[ctrl])
}

pub fn instantiate_chain(env: &Env, gs: &GameState, chain: &[CtrlId]) -> Box<dyn Controller> {
    use crate::anim_leaf::*;
    let Some(&ctrl) = chain.first() else { return Box::new(EmptyInst) };
    let rest = &chain[1..];
    let subject = |data: Option<CtrlId>| -> Option<Box<dyn Controller>> {
        if rest.is_empty() { data.map(|c| instantiate(env, gs, c)) } else { Some(instantiate_chain(env, gs, rest)) }
    };
    match &env.g.ctrls[ctrl].1 {
        Ctrl::StateFlow(_) => Box::new(SfInst::new(env, gs, ctrl)),
        Ctrl::Modular(items) => {
            let mut c: Vec<CtrlId> = items.iter().filter(|i| i.1).filter_map(|i| i.0).collect();
            c.extend_from_slice(rest);
            instantiate_chain(env, gs, &c)
        }
        Ctrl::Physics { subject: s, class } => Box::new(PassInst::new(subject(*s), Some(class.clone()))),
        Ctrl::TrajMask { subject: s } => Box::new(PassInst::new(subject(*s), None)),
        Ctrl::Chooser { .. } if chooser_is_dynamic(env, ctrl) => Box::new(ChooserInst::new(env, gs, ctrl)),
        Ctrl::Chooser { .. } => match chooser_static(env, gs, ctrl) {
            Some(c) => instantiate(env, gs, c),
            None => Box::new(EmptyInst),
        },
        Ctrl::Clip { .. } => Box::new(ClipInst::new(env, ctrl)),
        Ctrl::Sequence { .. } => Box::new(SeqInst::new(env, gs, ctrl)),
        Ctrl::Looping { subject: s, loop_control, start, loop_start, loop_end } => {
            Box::new(LoopingInst::new(subject(*s), *start, *loop_control, *loop_start, *loop_end))
        }
        Ctrl::BlendArray { .. } => Box::new(BlendArrayInst::new(env, gs, ctrl)),
        Ctrl::SignalPhase { subject: s, phase, range, .. } => Box::new(PhaseInst::new(subject(*s), *phase, *range)),
        Ctrl::ReferencePoseBlend { subject: s, reference, frame, weight } => {
            Box::new(RefPoseInst::new(env, gs, subject(*s), *reference, *frame, *weight, false))
        }
        Ctrl::OffsetPose { subject: s, reference, frame } => {
            Box::new(RefPoseInst::new(env, gs, subject(*s), *reference, *frame, None, true))
        }
        Ctrl::PlaybackRate { subject: s, signal, scale, range } => {
            Box::new(RateInst::new(subject(*s), *signal, *scale, *range))
        }
        Ctrl::Layers(_) => Box::new(LayersInst::new(env, gs, ctrl)),
        Ctrl::AutoMirror { subject: s, .. } => {
            let chain = if rest.is_empty() { s.map(|c| vec![c]).unwrap_or_default() } else { rest.to_vec() };
            Box::new(AutoMirrorInst::new(env, gs, ctrl, chain))
        }
        Ctrl::Mirror { subject: s, .. } => match subject(*s) {
            Some(sub) => Box::new(MirroredInst::new(sub, Vec::new())),
            None => Box::new(EmptyInst),
        },
        Ctrl::Proxy { default, resolved, .. } => match resolved.or(*default) {
            Some(c) => instantiate(env, gs, c),
            None => Box::new(EmptyInst),
        },
        Ctrl::BlendSpace { .. } => Box::new(crate::anim_space::BlendSpaceInst::new(env, gs, ctrl)),
        Ctrl::ContextDb { fields, entries } => match context_db_choice(gs, fields, entries) {
            Some(c) => instantiate(env, gs, c),
            None => Box::new(EmptyInst),
        },
        Ctrl::Other(_) => Box::new(EmptyInst),
    }
}

fn context_db_choice(
    gs: &GameState,
    fields: &[(Option<SigId>, ContextField)],
    entries: &[(Option<CtrlId>, Vec<i64>)],
) -> Option<CtrlId> {
    let pass = |(sig, kind): &(Option<SigId>, ContextField), cell: i64| {
        let v = sig.map_or(0.0, |s| gs.get(s));
        match kind {
            ContextField::Bool => cell == 2 || (cell == 1) == (v != 0.0),
            ContextField::Enum => cell < 0 || v.round() as i64 == cell,
            ContextField::Bitmap { shift } => {
                let v = v.round() as i64;
                let q = if *shift { 1i64.checked_shl(v as u32).unwrap_or(0) } else { v };
                q & cell != 0
            }
        }
    };
    let scores: Vec<u8> =
        entries.iter().map(|(_, cells)| u8::from(fields.iter().zip(cells).all(|(f, &c)| pass(f, c))) * 0xFF).collect();
    let mut best: (i32, usize) = (-1, 0);
    for (i, &sc) in scores.iter().enumerate() {
        if i32::from(sc) > best.0 {
            best = (i32::from(sc), i);
        }
    }
    entries.get(best.1).and_then(|e| e.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_db_filter() {
        let fields = [(Some(0), ContextField::Enum), (Some(1), ContextField::Bool), (Some(2), ContextField::Bool)];
        let entries = vec![
            (Some(10), vec![1, 1, 0]),
            (Some(11), vec![0, 0, 2]),
            (Some(12), vec![-1, 1, 1]),
            (Some(13), vec![-5, 1, 1]),
        ];
        let gs = |v: [f32; 3]| GameState::from_values(v.to_vec(), RandomState::default());
        assert_eq!(context_db_choice(&gs([1.0, 1.0, 0.0]), &fields, &entries), Some(10));
        assert_eq!(context_db_choice(&gs([0.0, 0.0, 1.0]), &fields, &entries), Some(11));
        assert_eq!(context_db_choice(&gs([4.0, 1.0, 1.0]), &fields, &entries), Some(12));
        assert_eq!(context_db_choice(&gs([3.0, 0.0, 0.0]), &fields, &entries), Some(10));
        let bitmap = [(Some(0), ContextField::Bitmap { shift: true })];
        let e = vec![(Some(1), vec![0b0010]), (Some(2), vec![0b1000])];
        assert_eq!(context_db_choice(&gs([3.0, 0.0, 0.0]), &bitmap, &e), Some(2));
    }
}
