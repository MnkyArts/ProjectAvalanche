use std::f32::consts::{FRAC_PI_2, PI, TAU};
use std::path::Path;

use anyhow::{Context, Result};
use glam::{Quat, Vec2, Vec3};
use ssx_assets::anim_eval::Animator;
pub use ssx_assets::anim_pose::{Delta, StepPhysics};
use ssx_assets::character::{Srt, model_matrix};
use ssx_assets::rig_anim::RigPose;
use ssx_formats::crsf::SceneGraph;

use crate::landing::twist;

pub mod rewind_pack;

mod intro;
mod live_blocks;
mod pose_block;
mod rig_ops;
mod signals;
mod update;

use crate::rider::{Input, RideState, RiderState, TrickInput};
use crate::steering::ground_frame;
use crate::tricks::{SignatureTiming, TrickHand};
pub use intro::*;
pub use live_blocks::{BlockSample, LiveBlocks, TELEPORT_M};
pub use pose_block::{BlockPose, LandingRumble, PoseRecorder, pose_source};
pub use rig_ops::RiderRigOps;

fn signature_of(a: &Animator) -> Option<SignatureTiming> {
    use ssx_assets::anim_graph::Ctrl;
    let g = &a.graph;
    let id = g.ctrl_id("proxy_tricks_signature_enter")?;
    let Ctrl::Proxy { resolved: Some(seq), .. } = &g.ctrls[id].1 else { return None };
    let name = g.ctrls[*seq].0.strip_prefix("1_SEQ_trick_air_sig_")?.strip_suffix("_enter")?;
    SignatureTiming::for_rider(name)
}

pub const RIDER_GRAPH_ROOT: &str = "layer_pose_riding";

const SRT_IDENTITY: Srt = Srt { scale: [1.0; 3], rotation: [0.0, 0.0, 0.0, 1.0], translation: [0.0; 3] };

#[derive(Clone, Copy, Debug)]
pub struct CarveAnimTuning {
    pub uber_traj_max: f32,
    pub depth_lean_distance: f32,
    pub ground_angle_gain: f32,
}

impl CarveAnimTuning {
    pub fn load(db: &ssx_assets::anim_db::AssetDb) -> Result<Self> {
        let f = |name: &str| {
            db.by_name("carve_riding").and_then(|a| a.f32(name)).with_context(|| format!("carve_riding has no {name}"))
        };
        Ok(Self {
            uber_traj_max: f("UberCrashTrajAngleMaxDelta")?,
            depth_lean_distance: f("SurfaceDepthLeanAngleModifierDistance")?,
            ground_angle_gain: f("NormalizedGroundAngleGain")?,
        })
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct GraphFeedback {
    pub driven: bool,
    pub traj_height_dof: f32,
    pub ground_offset: Vec3,
    pub board_x: Vec3,
    pub board_z: Vec3,
    pub board_offset: Vec3,
    pub carving_stopped: bool,
    pub allow_airborne: bool,
    pub uber_bail_sliding: bool,
    pub uber_recover: bool,
    pub recover_ticks: f32,
    pub jump_start: bool,
    pub in_rock_bail: bool,
    pub traj_rotation: Quat,
}

pub fn uber_tilt(q: Quat, n: Vec3, max: f32) -> Quat {
    let nl = q.conjugate() * n;
    if (nl.x.abs() - 1.0).abs() <= 1.525_879e-5 {
        return q;
    }
    let Some(axis) = Vec3::X.cross(nl).try_normalize() else { return q };
    let angle = nl.x.clamp(-1.0, 1.0).acos().clamp(-max, max);
    (q * Quat::from_axis_angle(axis, angle)).normalize()
}

pub fn stick_axes(s: Vec2) -> Vec2 {
    let len = s.length();
    if len <= 0.0 {
        return Vec2::ZERO;
    }
    s / len * ((len.min(1.0) - 0.25).max(0.0) / 0.75)
}

fn wrap_pi(x: f32) -> f32 {
    let y = (x + PI).rem_euclid(TAU) - PI;
    if y <= -PI { y + TAU } else { y }
}

fn wrap_4pi(x: f32) -> f32 {
    let t = 2.0 * TAU;
    let y = (x + TAU).rem_euclid(t) - TAU;
    if y <= -TAU { y + t } else { y }
}

fn hand_value(h: TrickHand) -> f32 {
    match h {
        TrickHand::None => -1.0,
        TrickHand::Backhand => 0.0,
        TrickHand::Forehand => 1.0,
        TrickHand::Both => 2.0,
    }
}

fn row(q: Quat, i: usize) -> Vec3 {
    q * [Vec3::X, Vec3::Y, Vec3::Z][i]
}

#[derive(Clone, Debug, Default)]
struct Producers {
    carving_init: bool,
    angle_to_ground: f32,
    board_move: f32,
    rstick_filtered: f32,
    airborne_timer: f32,
    was_in_air: bool,
    in_loop: bool,
    last_state: Option<RideState>,
    in_air_tag: bool,
    uber_traj: Option<Quat>,
    uber_frames: u32,
}

impl Producers {
    fn carving(&mut self, s: &RiderState, dt: f32, t: &CarveAnimTuning) {
        let a = s.alignment;
        let g = ground_frame(a, s.ground_normal);
        let lean = twist(g.conjugate() * a, Vec3::X);
        let mut x = lean * s.steering.stance_override_sign;
        if wrap_pi(s.landing.spin_target).abs() > FRAC_PI_2 {
            x = -x;
        }
        let h = -s.contact.map_or(0.0, |g| g.distance);
        let depth = s.surface.snow_depth + h - s.carving.riding_height;
        let range = if depth > 0.0 { FRAC_PI_2 - depth.atan2(t.depth_lean_distance) } else { FRAC_PI_2 };
        let n = (x / range).clamp(-1.0, 1.0);
        let k = (-t.ground_angle_gain * dt).exp();
        let m = s.grind.carving.board_angle / PI;
        if self.carving_init {
            self.angle_to_ground = n + (self.angle_to_ground - n) * k;
            self.board_move = m + (self.board_move - m) * k;
        } else {
            self.angle_to_ground = n;
            self.board_move = m;
            self.carving_init = true;
        }
    }
}

#[derive(Default)]
struct RewindGraph {
    assets: Option<RewindAssets>,
    channels: Vec<(usize, bool)>,
    records: Vec<Option<Vec<u32>>>,
    running: bool,
    fade: Option<(f32, RigPose)>,
}

pub const REWIND_ROTATIONS: [&str; 29] = [
    "Hips",
    "RightUpLeg",
    "RightLeg",
    "RightAnkleEffectorAux",
    "LeftUpLeg",
    "LeftLeg",
    "LeftAnkleEffectorAux",
    "Spine",
    "Spine1",
    "Spine2",
    "Spine3",
    "LeftShoulder",
    "LeftArm",
    "LeftForeArm",
    "RightShoulder",
    "RightArm",
    "RightForeArm",
    "LeftWing",
    "LeftShoulder_Wing",
    "LeftArm_Wing",
    "LeftForeArm_Wing",
    "RightWing",
    "RightShoulder_Wing",
    "RightArm_Wing",
    "RightForeArm_Wing",
    "Neck",
    "Neck1",
    "Head",
    "Board",
];
pub const REWIND_TRANSLATIONS: [&str; 12] = [
    "Hips",
    "RightAnkleEffectorAux",
    "LeftAnkleEffectorAux",
    "LeftWing",
    "LeftShoulder_Wing",
    "LeftArm_Wing",
    "LeftForeArm_Wing",
    "RightWing",
    "RightShoulder_Wing",
    "RightArm_Wing",
    "RightForeArm_Wing",
    "Board",
];

pub struct RewindAssets {
    animator: Animator,
    blend: ssx_assets::anim_graph::Blend,
}

impl RewindAssets {
    pub fn load(
        db: &ssx_assets::anim_db::AssetDb,
        rig: &SceneGraph,
        bind: Vec<Srt>,
        floats: &[String],
    ) -> Result<Self> {
        let animator = Animator::new(db, "layers_ghost_rewind", rig, bind, floats)?;
        let t = db
            .by_name("Transition state_rewind to state_gameloop_gameplay")
            .context("no state_rewind → state_gameloop_gameplay transition")?;
        let b = t.guid("OverrideBlend").and_then(|g| db.get(g)).context("the hand-back has no OverrideBlend")?;
        anyhow::ensure!(b.class() == "BlendCurveAsset", "the hand-back blend is a {}", b.class());
        let blend = ssx_assets::anim_graph::Blend {
            class: b.class().to_string(),
            blend_time: b.f32("BlendTime").context("the hand-back blend has no BlendTime")?,
            launch_angle: 0.0,
            curve: b.int("CurveType").context("the hand-back blend has no CurveType")?,
            sync_phase: false,
            weight: None,
            samples: Vec::new(),
            dof_curves: None,
        };
        Ok(Self { animator, blend })
    }
}

pub struct RiderGraph {
    animator: Animator,
    forward: bool,
    scripts: std::sync::Mutex<crate::anim_lua::AnimScripts>,
    script_signals: Vec<(usize, String, bool)>,
    producers: Producers,
    rig: SceneGraph,
    carve: CarveAnimTuning,
    trajectory: Option<usize>,
    roots: Vec<usize>,
    height_dof: Option<usize>,
    ground: Option<usize>,
    board: Option<usize>,
    hips: Option<usize>,
    head: Option<usize>,
    pose: RigPose,
    traj_rotation: Quat,
    updates: u64,
    signature: Option<SignatureTiming>,
    start: Option<LevelStartGraph>,
    rewind: RewindGraph,
    block_joints: ([Option<usize>; 29], [Option<usize>; 12]),
    floats: Vec<String>,
    script_updates: u64,
}

impl RiderGraph {
    pub fn new(
        animator: Animator,
        rig: SceneGraph,
        floats: &[String],
        carve: CarveAnimTuning,
        scripts: crate::anim_lua::AnimScripts,
    ) -> Self {
        let trajectory = rig.find("AITrajectory");
        let mut roots = Vec::new();
        let mut p = trajectory.and_then(|t| rig.nodes[t].parent);
        while let Some(i) = p {
            roots.push(i);
            p = rig.nodes[i].parent;
        }
        let pose = animator.bind.clone();
        let signature = signature_of(&animator);
        let channels: Vec<(usize, bool)> = REWIND_ROTATIONS
            .iter()
            .filter_map(|n| rig.find(n).map(|i| (i, true)))
            .chain(REWIND_TRANSLATIONS.iter().filter_map(|n| rig.find(n).map(|i| (i, false))))
            .collect();
        let block_joints = (REWIND_ROTATIONS.map(|n| rig.find(n)), REWIND_TRANSLATIONS.map(|n| rig.find(n)));
        let script_signals = (animator.graph.signals.iter().enumerate())
            .filter_map(|(i, s)| match &s.kind {
                ssx_assets::anim_graph::SignalKind::Script { function, float, .. } => {
                    Some((i, function.clone(), *float))
                }
                _ => None,
            })
            .collect();
        Self {
            animator,
            forward: false,
            scripts: std::sync::Mutex::new(scripts),
            script_signals,
            producers: Producers::default(),
            height_dof: floats.iter().position(|n| n == "TrajectoryVelocityRig.AITrajectory.height"),
            ground: rig.find("Ground"),
            board: rig.find("Board"),
            hips: rig.find("Hips"),
            head: rig.find("Head"),
            rig,
            carve,
            trajectory,
            roots,
            pose,
            traj_rotation: Quat::from_rotation_y(-FRAC_PI_2),
            updates: 0,
            signature,
            start: None,
            rewind: RewindGraph { channels, ..RewindGraph::default() },
            block_joints,
            floats: floats.to_vec(),
            script_updates: 0,
        }
    }

    pub fn set_intro(&mut self, big_root: &Path, nis: Option<&ssx_assets::nis::IntroNis>) -> Result<()> {
        let intro = match nis {
            Some(n) => Some(IntroGraph::load(big_root, n, &self.rig, self.animator.bind.nodes.clone(), &self.floats)?),
            None => None,
        };
        if let Some(st) = &mut self.start {
            st.intro = intro;
            (st.running, st.elapsed) = (false, None);
        }
        Ok(())
    }

    pub fn set_rewind(&mut self, assets: RewindAssets) {
        self.rewind.assets = Some(assets);
    }

    pub fn with_level_start(mut self, start: LevelStartGraph) -> Self {
        self.set_level_start(start);
        self
    }

    pub fn set_level_start(&mut self, start: LevelStartGraph) {
        self.start = Some(start);
    }

    pub fn signature(&self) -> Option<SignatureTiming> {
        self.signature
    }

    pub fn load(big_root: &Path, track: &str, name: &str) -> Result<Self> {
        let character = ssx_assets::character::load_character(big_root, track, name)?;
        Self::of_character(big_root, name, &character)
    }

    pub fn load_untracked(big_root: &Path, name: &str) -> Result<Self> {
        let loadout = ssx_assets::character::Loadout::default();
        let character = ssx_assets::character::load_character_untracked(big_root, name, &loadout)?;
        Self::of_character(big_root, name, &character)
    }

    fn of_character(big_root: &Path, name: &str, character: &ssx_assets::character::Character) -> Result<Self> {
        use ssx_assets::rig_anim::{ANIM_DIR, RIG_BUNDLE, float_dofs};
        let (rig, bind, _) =
            ssx_assets::anim_rigops::extend_character_rig(big_root, &character.rig, &character.bind_pose())?;
        let floats = float_dofs(big_root)?;
        let own = format!("{name}.cba");
        let dir = big_root.join(ANIM_DIR);
        let bundles: Vec<&str> =
            if dir.join(&own).exists() { vec![RIG_BUNDLE, own.as_str()] } else { vec![RIG_BUNDLE] };
        let db = ssx_assets::anim_db::AssetDb::load(&dir, &bundles)?;
        let carve = CarveAnimTuning::load(&db)?;
        let start = LevelStartGraph::load(&db, &rig, bind.clone(), &floats)?;
        let rewind = RewindAssets::load(&db, &rig, bind.clone(), &floats)?;
        let animator = Animator::new(&db, RIDER_GRAPH_ROOT, &rig, bind, &floats)?;
        let scripts = crate::anim_lua::AnimScripts::from_db(&db)?;
        let mut g = Self::new(animator, rig, &floats, carve, scripts).with_level_start(start);
        g.set_rewind(rewind);
        Ok(g)
    }

    pub fn reset(&mut self) {
        self.animator.reset();
        if let Some(st) = &mut self.start {
            st.animator.reset();
            if let Some(i) = &mut st.intro {
                i.reset(&st.animator.gs.random);
            }
            (st.running, st.elapsed) = (false, None);
        }
        self.producers = Producers::default();
        self.forward = false;
        self.scripts_mut().reset();
        let (assets, channels) = (self.rewind.assets.take(), std::mem::take(&mut self.rewind.channels));
        self.rewind = RewindGraph { assets, channels, ..RewindGraph::default() };
        self.pose = self.animator.bind.clone();
        self.traj_rotation = Quat::from_rotation_y(-FRAC_PI_2);
        self.updates = 0;
    }

    pub fn animator(&self) -> &Animator {
        match &self.start {
            Some(st) if st.running => st.current(),
            _ => &self.animator,
        }
    }

    pub fn rig(&self) -> &SceneGraph {
        &self.rig
    }

    pub fn pose(&self) -> &RigPose {
        &self.pose
    }

    pub fn trajectory_rotation(&self) -> Quat {
        self.traj_rotation
    }
    pub fn trajectory_joint(&self) -> Option<usize> {
        self.trajectory
    }

    pub fn updates(&self) -> u64 {
        self.updates
    }

    pub fn script_errors(&self) -> Vec<(String, String)> {
        let s = self.scripts.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        s.errors.iter().map(|(a, b)| (a.clone(), b.clone())).collect()
    }

    pub fn script_cost(&self) -> (f64, u64, u64) {
        let s = self.scripts.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        (s.seconds, s.calls, self.script_updates)
    }

    fn scripts_mut(&mut self) -> &mut crate::anim_lua::AnimScripts {
        self.scripts.get_mut().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub fn signal(&self, name: &str) -> f32 {
        let a = self.animator();
        a.graph.signal_id(name).map_or(0.0, |i| a.gs.get(i))
    }

    fn put(&mut self, name: &str, v: f32) {
        self.scripts_mut().set(name, v);
        self.animator.set(name, v);
    }

    fn put_bool(&mut self, name: &str, v: bool) {
        self.put(name, f32::from(u8::from(v)));
    }

    pub fn carving_outputs(&self) -> (f32, f32) {
        (self.producers.angle_to_ground, self.producers.board_move)
    }

    pub fn forward(&self) -> bool {
        self.forward
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uber_tilt_steps_towards_normal() {
        let n = Vec3::Y;
        let q = Quat::IDENTITY;
        let t = uber_tilt(q, n, 0.1);
        assert!(((t * Vec3::X).angle_between(n) - (FRAC_PI_2 - 0.1)).abs() < 1e-4);
        let aligned = Quat::from_rotation_z(FRAC_PI_2);
        assert_eq!(uber_tilt(aligned, n, 0.1), aligned);
        let mut q = q;
        for _ in 0..20 {
            q = uber_tilt(q, n, 0.1);
        }
        assert!((q * Vec3::X).angle_between(n) < 1e-3);
    }

    #[test]
    fn stick_axes_window() {
        assert_eq!(stick_axes(Vec2::new(0.2, 0.0)), Vec2::ZERO);
        assert!((stick_axes(Vec2::new(0.0, 0.625)) - Vec2::new(0.0, 0.5)).length() < 1e-6);
        assert!((stick_axes(Vec2::new(-2.0, 0.0)) - Vec2::new(-1.0, 0.0)).length() < 1e-6);
    }
}
