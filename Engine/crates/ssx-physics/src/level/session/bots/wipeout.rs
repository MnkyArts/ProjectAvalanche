use anyhow::{Context, Result};
use ssx_assets::anim_db::AssetDb;

use crate::tags::{BoolTag, FloatTag, TagEvent, TagTracker, TagWindow};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Var {
    TriggerWipeout,
    PlayerIsGrinding,
    InAir,
    TriggerGetup,
    OverrideGhostTimeScale,
    GhostTimeScale,
    LayerWeight,
}

impl Var {
    fn of(name: &str) -> Option<Self> {
        Some(match name {
            "BLN_TriggerWipeout" => Var::TriggerWipeout,
            "BLN_PlayerIsGrinding" => Var::PlayerIsGrinding,
            "BLN_InAir" => Var::InAir,
            "BLN_TriggerGetup" => Var::TriggerGetup,
            "BLN_OverrideGhostTimeScale" => Var::OverrideGhostTimeScale,
            "FLT_GhostTimeScale" => Var::GhostTimeScale,
            "FLT_ghost_wipeout_layer_weight" => Var::LayerWeight,
            _ => return None,
        })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum WipeoutTag {
    Bool { var: Var, tag: BoolTag },
    Float { var: Var, value: f32, mode: u32, reset: Option<f32>, spline: Vec<f32> },
}

#[derive(Clone, Debug, PartialEq)]
pub struct WipeoutTransition {
    pub to: usize,
    pub needs: Vec<Var>,
    pub needs_not: Vec<Var>,
    pub end_window: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct WipeoutNode {
    pub name: String,
    pub entry: Vec<Var>,
    pub entry_not: Vec<Var>,
    pub transitions: Vec<WipeoutTransition>,
    pub windows: Vec<TagWindow>,
    pub tags: Vec<WipeoutTag>,
    pub branch_out: Option<f32>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct WipeoutFlow {
    pub nodes: Vec<WipeoutNode>,
    pub start: usize,
}

fn vars(db: &AssetDb, a: &ssx_assets::anim_db::Asset, f: &str) -> Vec<Var> {
    a.guids(f).into_iter().filter_map(|g| db.get(g)).filter_map(|x| Var::of(x.name())).collect()
}

fn num(a: &ssx_assets::anim_db::Asset, f: &str) -> f32 {
    a.f32(f).or(a.int(f).map(|x| x as f32)).unwrap_or(0.0)
}

impl WipeoutFlow {
    pub fn load(db: &AssetDb) -> Result<Self> {
        let sf = db.by_name("sf_ghost_wipeout").context("no sf_ghost_wipeout")?;
        let guids = sf.guids("StateFlowNodeAssets");
        let start = sf.guid("StartNode").and_then(|g| guids.iter().position(|&x| x == g)).unwrap_or(0);
        let mut nodes = Vec::new();
        for n in guids.iter().filter_map(|&g| db.get(g)) {
            let transitions = n
                .guids("Transitions")
                .into_iter()
                .filter_map(|g| db.get(g))
                .filter_map(|t| {
                    let to = guids.iter().position(|&x| Some(x) == t.guid("To"))?;
                    let end_window =
                        t.guid("BranchWindowTypeAsset").and_then(|g| db.get(g)).is_some_and(|w| w.name() == "End");
                    Some(WipeoutTransition {
                        to,
                        needs: vars(db, &t, "ConditionsRequiredTrue"),
                        needs_not: vars(db, &t, "ConditionsRequiredFalse"),
                        end_window,
                    })
                })
                .collect();
            let mut ctrl = n.guid("SubjectController").and_then(|g| db.get(g));
            for _ in 0..8 {
                let Some(c) = ctrl else { break };
                if c.class() == "SequenceContainerAsset" {
                    ctrl = c.guids("ActorAssets").first().and_then(|&g| db.get(g));
                    break;
                }
                ctrl = c.guid("SubjectControllerAsset").and_then(|g| db.get(g));
            }
            let (mut windows, mut tags, mut branch_out) = (Vec::new(), Vec::new(), None);
            if let Some(set) = ctrl.and_then(|a| a.guid("TagCollectionSet")).and_then(|g| db.get(g)) {
                for c in set.guids("TagCollectionKeys").into_iter().filter_map(|g| db.get(g)) {
                    for t in c.guids("TagKeys").into_iter().filter_map(|g| db.get(g)) {
                        let value_asset = t.guid("ValueAsset").and_then(|g| db.get(g));
                        let var = value_asset.and_then(|v| Var::of(v.name()));
                        let window = TagWindow::new(
                            num(&t, "Time"),
                            num(&t, "Duration"),
                            t.bool("AlwaysActive").unwrap_or(false),
                        );
                        let mode = t.int("Mode").unwrap_or(0) as u32;
                        let reset_on_leave = t.bool("ResetOnLeave").unwrap_or(false);
                        match (t.class(), var) {
                            ("BranchOutPointTag", _) => branch_out = Some(num(&t, "Time")),
                            ("BoolGameStateTag", Some(var)) => {
                                let value = t.bool("Value").unwrap_or(false);
                                let default = value_asset.and_then(|v| v.bool("Default")).unwrap_or(false);
                                let reset = reset_on_leave.then_some(default);
                                windows.push(window);
                                tags.push(WipeoutTag::Bool { var, tag: BoolTag { value, mode, reset } });
                            }
                            ("FloatGameStateTag", Some(var)) => {
                                let spline = t
                                    .value("Spline")
                                    .and_then(|v| v.as_array())
                                    .map(|a| a.iter().filter_map(|x| x.as_f32()).collect())
                                    .unwrap_or_else(|| vec![1.0]);
                                windows.push(window);
                                let default = value_asset.and_then(|v| v.f32("Default")).unwrap_or(0.0);
                                tags.push(WipeoutTag::Float {
                                    var,
                                    value: num(&t, "Value"),
                                    mode,
                                    reset: reset_on_leave.then_some(default),
                                    spline,
                                });
                            }
                            _ => {}
                        }
                    }
                }
            }
            nodes.push(WipeoutNode {
                name: n.name().to_string(),
                entry: vars(db, &n, "EntryConditionsRequiredTrue"),
                entry_not: vars(db, &n, "EntryConditionsRequiredFalse"),
                transitions,
                windows,
                tags,
                branch_out,
            });
        }
        anyhow::ensure!(!nodes.is_empty(), "sf_ghost_wipeout has no nodes");
        Ok(Self { nodes, start })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Wipeout {
    pub node: usize,
    pub ticks: f32,
    tracker: TagTracker,
    saved: Vec<f32>,
    pub trigger_getup: bool,
    pub override_time_scale: bool,
    pub time_scale: f32,
    pub layer_weight: f32,
}

impl Wipeout {
    pub fn new(flow: &WipeoutFlow) -> Self {
        let n = &flow.nodes[flow.start];
        Self {
            node: flow.start,
            ticks: 0.0,
            tracker: TagTracker::new(&n.windows),
            saved: vec![0.0; n.tags.len()],
            trigger_getup: false,
            override_time_scale: false,
            time_scale: 1.0,
            layer_weight: 0.0,
        }
    }

    fn get(&self, v: Var, inputs: (bool, bool, bool)) -> bool {
        match v {
            Var::TriggerWipeout => inputs.0,
            Var::PlayerIsGrinding => inputs.1,
            Var::InAir => inputs.2,
            Var::TriggerGetup => self.trigger_getup,
            Var::OverrideGhostTimeScale => self.override_time_scale,
            Var::GhostTimeScale | Var::LayerWeight => false,
        }
    }

    pub fn active(&self, flow: &WipeoutFlow) -> bool {
        self.node != flow.start
    }

    pub fn time_scale_override(&self) -> Option<f32> {
        self.override_time_scale.then_some(self.time_scale)
    }

    pub fn update(&mut self, flow: &WipeoutFlow, trigger: bool, player_grinding: bool, in_air: bool, dt: f32) {
        let inputs = (trigger, player_grinding, in_air);
        let node = &flow.nodes[self.node];
        let reached = node.branch_out.is_none_or(|b| self.ticks >= b);
        let next = node.transitions.iter().find(|t| {
            let target = &flow.nodes[t.to];
            (!t.end_window || reached)
                && t.needs.iter().all(|&v| self.get(v, inputs))
                && t.needs_not.iter().all(|&v| !self.get(v, inputs))
                && target.entry.iter().all(|&v| self.get(v, inputs))
                && target.entry_not.iter().all(|&v| !self.get(v, inputs))
        });
        if let Some(t) = next {
            let events = self.tracker.exit();
            self.apply(flow, self.node, &events);
            self.node = t.to;
            self.ticks = 0.0;
            let n = &flow.nodes[self.node];
            self.tracker = TagTracker::new(&n.windows);
            self.saved = vec![0.0; n.tags.len()];
            // TODO(guess): the new actor's tags are tested once at time 0 on its entry (so the Time-0 windows fire); settle: the actor's first tag update (0x835354B0 → `TagTracker_Update` 0x831A0E80) after a transition's creation.
            crate::guess_hit!();
            if self.tracker.update(0.0) {
                let events = self.tracker.process();
                self.apply(flow, self.node, &events);
            }
        }
        self.ticks += dt;
        if self.tracker.update(self.ticks) {
            let events = self.tracker.process();
            self.apply(flow, self.node, &events);
        }
    }

    fn apply(&mut self, flow: &WipeoutFlow, node: usize, events: &[(usize, TagEvent)]) {
        for &(i, ev) in events {
            let (Some(tag), Some(saved)) = (flow.nodes[node].tags.get(i), self.saved.get_mut(i)) else { continue };
            match tag {
                WipeoutTag::Bool { var, tag } => {
                    let mut s = *saved != 0.0;
                    let v = match var {
                        Var::TriggerGetup => &mut self.trigger_getup,
                        Var::OverrideGhostTimeScale => &mut self.override_time_scale,
                        _ => continue,
                    };
                    tag.on(ev, v, &mut s);
                    *saved = f32::from(u8::from(s));
                }
                WipeoutTag::Float { var, value, mode, reset, spline } => {
                    let w = match ev {
                        TagEvent::Apply(w) | TagEvent::Leave(w) => w,
                        TagEvent::Enter => 0.0,
                    };
                    let tag = FloatTag {
                        value: *value,
                        mode: *mode,
                        reset: *reset,
                        spline: crate::ice_axe::tag_spline(spline, w),
                    };
                    let v = match var {
                        Var::GhostTimeScale => &mut self.time_scale,
                        Var::LayerWeight => &mut self.layer_weight,
                        _ => continue,
                    };
                    tag.on(ev, v, saved);
                }
            }
        }
    }
}
