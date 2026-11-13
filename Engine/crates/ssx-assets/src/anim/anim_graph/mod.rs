pub mod added;
mod build;

use std::collections::HashMap;

use anyhow::Result;
use ssx_formats::ant::{Guid, Value};

use crate::anim_db::{Asset, AssetDb, field};

use build::*;

pub type CtrlId = usize;
pub type SigId = usize;
pub type BlendId = usize;

#[derive(Clone, Debug, PartialEq)]
pub enum SignalKind {
    Bool(bool),
    Float(f32),
    Int(i64),
    Enum(i64),
    Vector,
    Script { script: String, ordinal: i64, float: bool, function: String },
    Other(String),
}

impl SignalKind {
    pub fn default_value(&self) -> f32 {
        match *self {
            SignalKind::Bool(b) => f32::from(u8::from(b)),
            SignalKind::Float(x) => x,
            SignalKind::Int(i) | SignalKind::Enum(i) => i as f32,
            _ => 0.0,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Signal {
    pub name: String,
    pub kind: SignalKind,
    pub trigger: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct EnumCond {
    pub signal: SigId,
    pub value: i64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Blend {
    pub class: String,
    pub blend_time: f32,
    pub launch_angle: f32,
    pub curve: i64,
    pub sync_phase: bool,
    pub weight: Option<SigId>,
    pub samples: Vec<f32>,
    pub dof_curves: Option<DofCurves>,
}

impl Blend {
    pub fn default_transition() -> Self {
        Blend {
            class: "BlendCurveAsset".into(),
            blend_time: 8.0,
            launch_angle: 0.0,
            curve: 0,
            sync_phase: false,
            weight: None,
            samples: Vec::new(),
            dof_curves: None,
        }
    }

    pub fn of_asset(a: &Asset) -> Option<Self> {
        if !a.is("IBlendAsset") {
            return None;
        }
        Some(Blend {
            class: a.class().to_string(),
            blend_time: a.f32("BlendTime").unwrap_or(0.0),
            launch_angle: a.f32("LaunchAngle").unwrap_or(0.0),
            curve: a.int("CurveType").or_else(|| a.int("Curve")).unwrap_or(-1),
            sync_phase: a.bool("ShouldSyncPhase").unwrap_or(false),
            weight: None,
            samples: a.array("Samples").iter().filter_map(Value::as_f32).collect(),
            dof_curves: None,
        })
    }

    pub fn cut() -> Self {
        Blend { class: "AccelBlendAsset".into(), blend_time: 0.0, curve: -1, ..Self::default_transition() }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct DofCurves {
    pub sets: Vec<DofSetCurve>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DofSetCurve {
    pub layout: String,
    pub for_each: bool,
    pub curves: Vec<[u8; 4]>,
    pub inherited: Vec<u8>,
}

impl DofSetCurve {
    pub fn weight(&self, i: usize, t: f32) -> f32 {
        let k = if self.for_each { i } else { 0 };
        let Some(&[b0, b1, b2, b3]) = self.curves.get(k) else { return t };
        let big_t = 255.0 * t;
        if b1 == 0 {
            return if big_t > f32::from(b0) { 1.0 } else { 0.0 };
        }
        let u = ((big_t - f32::from(b0)) / f32::from(b1)).clamp(0.0, 1.0);
        let (a, b) = if self.inherited.get(k).is_some_and(|&m| m != 0) {
            (1.0, 2.0)
        } else {
            (f32::from(b2) * 6.0 / 255.0 - 1.5, f32::from(b3) * 6.0 / 255.0 - 1.5)
        };
        (1.0 + a - b) * u * u * u + (b - 2.0 * a) * u * u + a * u
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct SfNode {
    pub name: String,
    pub conduit: bool,
    pub transparent: bool,
    pub subject: Option<CtrlId>,
    pub transitions: Vec<usize>,
    pub entry_true: Vec<SigId>,
    pub entry_false: Vec<SigId>,
    pub entry_enum_true: Vec<EnumCond>,
    pub entry_enum_false: Vec<EnumCond>,
    pub active_signals: Vec<SigId>,
    pub tags: Vec<String>,
    pub start_tick: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Transition {
    pub name: String,
    pub from: usize,
    pub to: usize,
    pub override_blend: Option<BlendId>,
    pub window: Option<String>,
    pub branch_in_window: Option<String>,
    pub cond_true: Vec<SigId>,
    pub cond_false: Vec<SigId>,
    pub enum_true: Vec<EnumCond>,
    pub enum_false: Vec<EnumCond>,
    pub branch_in_phase: u8,
    pub destination_phase: f32,
    pub branch_in_any: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct StateFlow {
    pub name: String,
    pub start: usize,
    pub mirrored: Option<SigId>,
    pub nodes: Vec<SfNode>,
    pub transitions: Vec<Transition>,
    pub max_active: u32,
    pub transition_as_blend_slave: bool,
    pub identifier: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Selector {
    Enum(i64),
    Linear { signal: Option<SigId>, threshold: f32 },
    Signals { all_true: Vec<SigId>, all_false: Vec<SigId> },
    Random { weight: f32 },
    Other(String),
}

#[derive(Clone, Debug, PartialEq)]
pub struct ChooserEntry {
    pub controller: Option<CtrlId>,
    pub selector: Selector,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SeqAnim {
    pub controller: Option<CtrlId>,
    pub blend: Option<BlendId>,
    pub flags: i64,
    pub start_time: f32,
    pub start_in_time: f32,
    pub end_in_time: f32,
    pub start_hold_time: f32,
    pub end_hold_time: f32,
    pub scale: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AudioTag {
    pub name: String,
    pub time: f32,
    pub duration: f32,
    pub system: i64,
    pub event: i64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct GameTag {
    pub name: String,
    pub signal: SigId,
    pub window: crate::tags::TagWindow,
    pub value: f32,
    pub boolean: bool,
    pub mode: u32,
    pub reset_on_leave: bool,
    pub default: f32,
    pub spline: Vec<f32>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct BranchWindow {
    pub kind: String,
    pub filter: u32,
    pub time: f32,
    pub length: f32,
    pub blend: Option<BlendId>,
    pub destination_phase: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct BlendItem {
    pub controller: Option<CtrlId>,
    pub x: f32,
    pub end_time: f32,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct Mask {
    pub name: String,
    pub slots: Vec<(String, Vec<f32>)>,
    pub defaults: Vec<(String, f32)>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Layer {
    pub controller: Option<CtrlId>,
    pub signal: Option<SigId>,
    pub mask: Option<Mask>,
    pub mask_array: Option<MaskArray>,
    pub update_mode: u32,
    pub update_rate: u32,
    pub blend_mode: u32,
    pub branch_in_mode: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MaskArray {
    pub value: Option<SigId>,
    pub range: [f32; 2],
    pub max_dec: f32,
    pub max_inc: f32,
    pub items: Vec<(Mask, f32)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContextField {
    Bool,
    Enum,
    Bitmap { shift: bool },
}

#[derive(Clone, Debug, PartialEq)]
pub enum Ctrl {
    StateFlow(StateFlow),
    Modular(Vec<(Option<CtrlId>, bool)>),
    Physics {
        class: String,
        subject: Option<CtrlId>,
    },
    Chooser {
        policy: String,
        policy_signal: Option<SigId>,
        cache_last_choice: bool,
        entries: Vec<ChooserEntry>,
        choose_on_reset: bool,
        choose_every_update: bool,
        choose_as_blend_slave: bool,
        looping: bool,
        choose_on_loop: bool,
        phase_match_previous: bool,
        blend: Option<BlendId>,
    },
    Clip {
        anim: String,
        anim_guid: Option<Guid>,
        target: String,
        num_ticks: f32,
        fps: f32,
        fps_scale: f32,
        delta_trajectory: u8,
        distance: f32,
    },
    Sequence {
        anims: Vec<SeqAnim>,
        length: f32,
        windows: Vec<BranchWindow>,
        start_tick: f32,
        audio_tags: Vec<AudioTag>,
        game_tags: Vec<GameTag>,
    },
    Looping {
        subject: Option<CtrlId>,
        loop_control: bool,
        start: f32,
        loop_start: f32,
        loop_end: f32,
    },
    BlendArray {
        signal: Option<SigId>,
        clamp: [f32; 2],
        damping: [f32; 2],
        mirrored: bool,
        master: Option<CtrlId>,
        master_length: f32,
        items: Vec<BlendItem>,
        range: [f32; 2],
        snap: i64,
    },
    SignalPhase {
        subject: Option<CtrlId>,
        phase: Option<SigId>,
        range: [f32; 2],
        curve: Option<String>,
    },
    ReferencePoseBlend {
        subject: Option<CtrlId>,
        reference: Option<CtrlId>,
        frame: f32,
        weight: Option<SigId>,
    },
    OffsetPose {
        subject: Option<CtrlId>,
        reference: Option<CtrlId>,
        frame: f32,
    },
    PlaybackRate {
        subject: Option<CtrlId>,
        signal: Option<SigId>,
        scale: f32,
        range: [f32; 2],
    },
    Layers(Vec<Layer>),
    AutoMirror {
        subject: Option<CtrlId>,
        signal: Option<SigId>,
        mirror_list: Vec<SigId>,
        duration: f32,
        freeze: bool,
        continuous: bool,
    },
    Mirror {
        subject: Option<CtrlId>,
        phase_offset: f32,
    },
    Proxy {
        default: Option<CtrlId>,
        bank: Option<String>,
        resolved: Option<CtrlId>,
    },
    TrajMask {
        subject: Option<CtrlId>,
    },
    ContextDb {
        fields: Vec<(Option<SigId>, ContextField)>,
        entries: Vec<(Option<CtrlId>, Vec<i64>)>,
    },
    BlendSpace {
        x: Option<SigId>,
        y: Option<SigId>,
        axes: [[f32; 5]; 2],
        items: Vec<([f32; 2], Option<CtrlId>)>,
        triangles: Vec<[usize; 3]>,
        inverse_area: Vec<f32>,
        hull: Vec<([f32; 2], [f32; 2], usize)>,
        snap: Vec<bool>,
        snap_tolerance: f32,
    },
    Other(String),
}

#[derive(Clone, Debug, Default)]
pub struct Graph {
    pub ctrls: Vec<(String, Ctrl)>,
    pub signals: Vec<Signal>,
    pub blends: Vec<Blend>,
    ctrl_ids: HashMap<Guid, CtrlId>,
    sig_ids: HashMap<Guid, SigId>,
    blend_ids: HashMap<Guid, BlendId>,
    pub random_seed: Option<u32>,
    sig_names: std::sync::OnceLock<(usize, HashMap<String, SigId>)>,
}

impl Graph {
    pub fn signal_id(&self, name: &str) -> Option<SigId> {
        let (n, names) = self.sig_names.get_or_init(|| {
            let mut m = HashMap::with_capacity(self.signals.len());
            for (i, s) in self.signals.iter().enumerate() {
                m.entry(s.name.clone()).or_insert(i);
            }
            (self.signals.len(), m)
        });
        if *n == self.signals.len() {
            names.get(name).copied()
        } else {
            self.signals.iter().position(|s| s.name == name)
        }
    }

    pub fn ctrl_id(&self, name: &str) -> Option<CtrlId> {
        self.ctrls.iter().position(|c| c.0 == name)
    }

    pub fn clip_names(&self) -> Vec<String> {
        let mut v: Vec<String> = self.clip_guids().into_iter().map(|c| c.1).collect();
        v.sort();
        v.dedup();
        v
    }

    pub fn clip_guids(&self) -> Vec<(Guid, String)> {
        let mut seen = std::collections::HashSet::new();
        self.ctrls
            .iter()
            .filter_map(|c| match &c.1 {
                Ctrl::Clip { anim, anim_guid: Some(g), target, .. } if target == "AnimRig" && seen.insert(*g) => {
                    Some((*g, anim.clone()))
                }
                _ => None,
            })
            .collect()
    }
}

impl Graph {
    pub fn load(db: &AssetDb, root: &str) -> Result<(Graph, CtrlId)> {
        let a = db.by_name(root).ok_or_else(|| anyhow::anyhow!("no asset {root}"))?;
        let overrides = db.character_config().map(|c| db.config_overrides(c)).unwrap_or_default();
        let mut b = Builder { db, g: Graph::default(), overrides };
        let id = b.ctrl(Some(a.guid)).ok_or_else(|| anyhow::anyhow!("{root} is not a controller"))?;
        let mut g = b.g;
        if let Some(tricks) = crate::content::tricks::tricks_for(db.data_root()) {
            for p in added::add_tricks(&mut g, &tricks, db) {
                crate::content::tricks::notice(db.data_root(), p);
            }
        }
        Ok((g, id))
    }
}
