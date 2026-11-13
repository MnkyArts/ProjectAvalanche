mod intro;
mod outro;
mod walk;

use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context, Result, bail};
use ssx_formats::ant::{Guid, Stream, Value};
use ssx_formats::ant_anim::{Animation, ChannelToDof, Pose, Skeleton, dof_list};

use crate::anim_db::{Asset, AssetDb, field};
pub use crate::anim_graph::Blend;
use crate::rig_anim::{ANIM_DIR, RIG_BUNDLE};
use crate::rigamate::{Sqt, rig_defaults};

pub use intro::*;
pub use outro::*;
use walk::*;

pub const RIDER_RIG: &str = "genchar_rig";
pub const HELI_RIG: &str = "helicopter_rig";
pub const CAMERA_RIG: &str = "camera_rig";

pub const ROLE_CHARACTER1: i64 = 0;
pub const ROLE_CAMERA1: i64 = 10;
pub const ROLE_HELICOPTER1: i64 = 20;
pub const ROLE_GHOST1: i64 = 30;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Channel {
    Quat(usize),
    Vec3(usize),
    Float(usize),
}

#[derive(Clone, Debug, PartialEq)]
pub struct NisClip {
    pub name: String,
    pub num_ticks: f32,
    pub fps_scale: f32,
    pub delta_trajectory: bool,
    pub anim: crate::anim_clip::ClipData,
    pub channels: HashMap<String, Channel>,
}

impl NisClip {
    pub fn load(db: &AssetDb, all: &Stream, ctrl: &Asset, rig: &str) -> Result<Self> {
        let name = ctrl.name().to_string();
        let anim_g = ctrl.guid("Anim").with_context(|| format!("{name}: no Anim"))?;
        let a = db.get(anim_g).with_context(|| format!("{name}: Anim {anim_g} not loaded"))?;
        let anim = Animation::from_object(a.schema, a.obj)?.with_context(|| format!("{name}: Anim is no animation"))?;
        let t = db.get(anim.info().channel_to_dof).with_context(|| format!("{name}: no ChannelToDof"))?;
        let table = ChannelToDof::parse(t.schema, t.obj)?;
        let (anim, indices) = crate::content::anims::clip_for(db.data_root(), anim_g, anim, table.indices);
        let target = ctrl.guid("Target").and_then(|g| db.get(g)).map_or("AnimRig", |t| t.name());
        let dofs = dof_list(all, rig, target)?;
        let c = anim.counts();
        let mut channels = HashMap::new();
        for (ch, &d) in indices.iter().enumerate() {
            let Some(dof) = dofs.get(usize::from(d)) else {
                bail!("{name}: channel {ch} → DOF {d} outside {rig} {target}")
            };
            let kind = if ch < c.quats {
                Channel::Quat(ch)
            } else if ch < c.quats + c.vec3s {
                Channel::Vec3(ch - c.quats)
            } else {
                Channel::Float(ch - c.quats - c.vec3s)
            };
            channels.insert(dof.name.clone(), kind);
        }
        Ok(Self {
            name,
            num_ticks: ctrl.f32("NumTicks").unwrap_or(0.0),
            fps_scale: ctrl.f32("FPSScale").unwrap_or(1.0),
            delta_trajectory: ctrl.int("DeltaTrajectory") == Some(1),
            anim,
            channels,
        })
    }

    pub fn sample(&self, tick: f32) -> Result<Pose> {
        Ok(self.anim.sample(tick.clamp(0.0, self.num_ticks) * self.fps_scale)?)
    }

    pub fn quat(&self, p: &Pose, dof: &str) -> Option<[f32; 4]> {
        match self.channels.get(dof)? {
            Channel::Quat(i) => p.quats.get(*i).copied(),
            _ => None,
        }
    }

    pub fn vec3(&self, p: &Pose, dof: &str) -> Option<[f32; 3]> {
        match self.channels.get(dof)? {
            Channel::Vec3(i) => p.vec3s.get(*i).copied(),
            _ => None,
        }
    }

    pub fn float(&self, p: &Pose, dof: &str) -> Option<f32> {
        match self.channels.get(dof)? {
            Channel::Float(i) => p.floats.get(*i).copied(),
            _ => None,
        }
    }

    pub fn joint(&self, p: &Pose, rig: &Rig, joint: &str) -> ([f32; 4], [f32; 3]) {
        let d = rig.defaults.get(joint).copied().unwrap_or(Sqt::IDENTITY);
        (
            self.quat(p, &format!("{joint}.q")).unwrap_or(d.q),
            self.vec3(p, &format!("{joint}.t")).unwrap_or([d.t[0], d.t[1], d.t[2]]),
        )
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Rig {
    pub name: String,
    pub skeleton: Skeleton,
    pub defaults: HashMap<String, Sqt>,
}

impl Rig {
    pub fn load(all: &Stream, name: &str) -> Result<Self> {
        Ok(Self { name: name.to_string(), skeleton: Skeleton::of_rig(all, name)?, defaults: rig_defaults(all, name)? })
    }

    pub fn chain(&self, joint: &str, root: &str) -> Option<Vec<String>> {
        let mut i = self.skeleton.find(joint)?;
        let mut out = Vec::new();
        loop {
            let j = &self.skeleton.joints[i];
            if j.name == root {
                return Some(out);
            }
            out.push(j.name.clone());
            i = j.parent?;
        }
    }
}

pub fn template_joint(db: &AssetDb, rig: &Rig, id: &Asset) -> Option<String> {
    let template = id.guid("TemplateAsset")?;
    let value = usize::try_from(id.int("EnumValue")?).ok()?;
    let rig_asset = db.all_of_class("RigAsset").find(|r| r.name() == rig.name)?;
    let features = db.get(rig_asset.guid("FeatureCollection")?)?.guids("FeatureAssets");
    for f in features.into_iter().filter_map(|g| db.get(g)).filter(|f| f.is("JointMappingFeature")) {
        for m in f.array("JointMappings") {
            if field(f.schema, m, "JointTemplate").and_then(Value::as_guid) != Some(template) {
                continue;
            }
            let idx = field(f.schema, m, "JointIndices").and_then(Value::as_array)?.get(value)?.as_i64()?;
            return rig.skeleton.joints.get(usize::try_from(idx).ok()?).map(|j| j.name.clone());
        }
    }
    None
}

pub fn intro_bundles(big_root: &Path, binding: &str, character: &str) -> Vec<String> {
    let dir = big_root.join(ANIM_DIR);
    let mut out: Vec<String> =
        [format!("{binding}.cba"), format!("{character}.cba")].into_iter().filter(|b| dir.join(b).exists()).collect();
    out.push(RIG_BUNDLE.to_string());
    out
}

fn same(a: Option<Guid>, b: Guid) -> bool {
    a == Some(b)
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Window {
    pub time: f32,
    pub duration: f32,
    pub always_active: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Attachment {
    pub window: Window,
    pub target: String,
    pub source_joint: String,
    pub target_joint: String,
    pub offset_t: [f32; 3],
    pub offset_q: [f32; 4],
    pub blend_in: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ScriptCall {
    pub window: Window,
    pub script: String,
    pub update: i64,
    pub enter: i64,
    pub exit: i64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct NisActor {
    pub name: String,
    pub role: i64,
    pub length: f32,
    pub clip: NisClip,
    pub offset: Option<(String, String)>,
    pub choices: Vec<(NisClip, f32)>,
    pub seed: Option<u32>,
    pub chooser: Option<NisChooser>,
    pub sequences: Vec<(String, f32)>,
    pub root: Option<(String, Option<f32>)>,
    pub tags: Tags,
}

#[derive(Clone, Debug, PartialEq)]
pub struct NisChooser {
    pub looping: bool,
    pub choose_on_loop: bool,
    pub blend: Option<Blend>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Tags {
    pub drivers: Vec<Attachment>,
    pub attachments: Vec<Attachment>,
    pub stop: Option<Window>,
    pub outro_active: Option<Window>,
    pub scripts: Vec<ScriptCall>,
    pub branch_out: Option<f32>,
    pub early_out: Option<f32>,
    pub branch_in: Option<f32>,
    pub window_blends: Vec<String>,
    pub tuning_offset: Option<[f32; 3]>,
    pub game_state: Vec<GameStateTag>,
}

pub const GAME_STATE_TAGS: [&str; 2] = ["BLN_State_Wingsuit", "FLT_Wingsuit_BlendWeight"];

#[derive(Clone, Debug, PartialEq)]
pub struct GameStateTag {
    pub state: String,
    pub window: Window,
    pub float: bool,
    pub value: f32,
    pub mode: u32,
    pub reset_on_leave: bool,
    pub default: f32,
    pub spline: Vec<f32>,
}

impl Tags {
    pub fn runs_nis_script(&self, ordinal: i64) -> bool {
        self.scripts.iter().any(|s| s.script == "script_nis" && s.update == ordinal)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct NisScene {
    pub node: String,
    pub sequence: String,
    pub loop_end: Option<f32>,
    pub actors: Vec<NisActor>,
    pub node_tags: Tags,
    pub problems: Vec<String>,
    pub active_signals: Vec<String>,
    pub heli_movement: Option<String>,
}

impl NisScene {
    pub fn actor(&self, role: i64) -> Option<&NisActor> {
        self.actors.iter().find(|a| a.role == role)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct IntroNis {
    pub binding: String,
    pub controller: String,
    pub ready: NisScene,
    pub jump: NisScene,
    pub transition_blend: Blend,
    pub branch_out: Option<f32>,
    pub bundles: Vec<String>,
    pub rider: Rig,
    pub heli: Rig,
    pub camera: Rig,
    pub cabin: Vec<CabinScene>,
    pub requires_wingsuit: bool,
    pub character_of_interest: bool,
    pub ready_loop: bool,
    pub jump_start: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IntroPath {
    Short,
    Medium,
    Long,
}

impl IntroPath {
    pub fn decide(
        nis_short: bool,
        restarted: bool,
        requires_wingsuit: bool,
        wingsuit_enabled: bool,
        visits: u32,
    ) -> Self {
        if nis_short || restarted || (requires_wingsuit && !wingsuit_enabled) {
            IntroPath::Short
        } else if visits >= 2 {
            IntroPath::Medium
        } else {
            IntroPath::Long
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct CabinScene {
    pub scene: NisScene,
    pub start: f32,
    pub end: f32,
    pub window: String,
    pub blend: Blend,
    pub tuning_offset: Option<[f32; 3]>,
    pub skip: Option<(SkipTo, Blend)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SkipTo {
    Cabin(usize),
    ReadyLoop,
    Jump,
}

impl IntroNis {
    pub fn rig(&self, role: i64) -> &Rig {
        match role {
            ROLE_HELICOPTER1 => &self.heli,
            ROLE_CAMERA1 => &self.camera,
            _ => &self.rider,
        }
    }
}
