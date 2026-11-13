mod animation;
mod model;

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use ssx_formats::ant::Value as AntValue;
use ssx_formats::attrib::Value;
use ssx_formats::crsf::{CrsfFile, SceneGraph};
use ssx_formats::ghost::{
    self, FLAG_BOARD_PARTICLE, FLAG_GROUND_RAY, GhostFile, PoseBlock, ROTATION_CHANNELS, TRANSLATION_CHANNELS,
    UserHeader,
};

use crate::anim_db::AssetDb;
use crate::anim_eval::Animator;
use crate::anim_rigops::LegRigOps;
use crate::character::{PaletteJoint, Skin, Srt};
use crate::object_mesh::ObjectMesh;
use crate::rig_anim::RigPose;
use crate::rigamate::RigamateRig;
use crate::track_objects::asset_file;
use crate::vault_db::{Db, Material, fnv1, refs, single, single_text, with_track_db};

pub use animation::*;
pub use model::*;

pub const GHOST_DIR: &str = "data/common/ghosts";
pub const MEDAL_LABELS: [&str; 3] =
    ["TXT_EXPLORE_GHOST_MEDAL_GOLD", "TXT_EXPLORE_GHOST_MEDAL_SILVER", "TXT_EXPLORE_GHOST_MEDAL_BRONZE"];
pub const EXPLORE_GHOST_TINT: u32 = 0xFF05_B8F3;
pub const SPAWN_TINT: u32 = 0xFF0C_0A28;

pub fn tint_rgba(word: u32) -> [f32; 4] {
    let b = word.to_be_bytes();
    [b[3], b[2], b[1], b[0]].map(|x| f32::from(x) / 255.0)
}

pub fn replay_file(big_root: &Path, id: u32) -> Option<PathBuf> {
    crate::data_roots::find(big_root, &format!("{GHOST_DIR}/{id}.bin"))
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GhostPose {
    pub trajectory_rotation: [f32; 4],
    pub trajectory_position: [f32; 3],
    pub rotations: [[f32; 4]; 29],
    pub translations: [[f32; 3]; 12],
    pub flags: u8,
    pub classic_rig: bool,
    pub velocity: [f32; 3],
    pub snow_spray_carving: [f32; 3],
    pub snow_spray_speed: f32,
    pub landing_impact: f32,
    pub snow_coverage: f32,
}

impl GhostPose {
    pub fn of(b: &PoseBlock) -> Self {
        Self {
            trajectory_rotation: normalized(b.trajectory_rotation),
            trajectory_position: b.trajectory_position,
            rotations: std::array::from_fn(|i| b.rotation(i)),
            translations: std::array::from_fn(|i| b.translation(i)),
            flags: b.flags,
            classic_rig: b.classic_rig(),
            velocity: b.velocity(),
            snow_spray_carving: b.snow_spray_carving(),
            snow_spray_speed: ghost::unpack_half(b.snow_spray_speed),
            landing_impact: ghost::unpack_half(b.landing_impact),
            snow_coverage: f32::from(b.snow_coverage) / 255.0,
        }
    }

    pub fn blend(a: &Self, b: &Self, u: f32) -> Self {
        let near = if u < 0.5 { a.flags } else { b.flags };
        let repack = |v: [f32; 3], range: f32| ghost::unpack_vector(ghost::pack_vector(v, [range; 3]), [range; 3]);
        Self {
            trajectory_rotation: ghost::blend_rotation(a.trajectory_rotation, b.trajectory_rotation, u),
            trajectory_position: lerp3(a.trajectory_position, b.trajectory_position, u),
            rotations: std::array::from_fn(|i| {
                ghost::unpack_quaternion(ghost::pack_quaternion(ghost::blend_rotation(
                    a.rotations[i],
                    b.rotations[i],
                    u,
                )))
            }),
            translations: std::array::from_fn(|i| {
                repack(ghost::blend_lerp(a.translations[i], b.translations[i], u), ghost::CHANNEL_RANGE)
            }),
            flags: (near & !FLAG_GROUND_RAY) | (a.flags & b.flags & FLAG_GROUND_RAY),
            classic_rig: a.classic_rig,
            velocity: blend_velocity(a.velocity, b.velocity, u),
            snow_spray_carving: repack(
                ghost::blend_lerp(a.snow_spray_carving, b.snow_spray_carving, u),
                ghost::CARVING_RANGE,
            ),
            snow_spray_speed: requantize_half((b.snow_spray_speed - a.snow_spray_speed).mul_add(u, a.snow_spray_speed)),
            landing_impact: requantize_half((b.landing_impact - a.landing_impact).mul_add(u, a.landing_impact)),
            // TODO(guess): coverage lerps already-normalised endpoint floats, the game lerps raw bytes then × 1/255 (0x82C77040…90); settle: keep the endpoint bytes for last-bit parity.
            snow_coverage: (b.snow_coverage - a.snow_coverage).mul_add(u, a.snow_coverage),
        }
    }

    pub fn board_particle(&self) -> bool {
        self.flags & FLAG_BOARD_PARTICLE != 0
    }
}

fn requantize_half(value: f32) -> f32 {
    ghost::unpack_half(ghost::pack_half(value))
}

fn blend_velocity(a: [f32; 3], b: [f32; 3], u: f32) -> [f32; 3] {
    let v = ghost::blend_lerp(a, b, u);
    let range = ssx_formats::vmx::mul(ghost::vector_length(v), f32::from_bits(0x3F80_20C5));
    ghost::unpack_vector(ghost::pack_vector(v, [range; 3]), [requantize_half(range); 3])
}

fn normalized(q: [f32; 4]) -> [f32; 4] {
    let n = q.iter().map(|c| c * c).sum::<f32>().sqrt();
    if n > 0.0 { q.map(|c| c / n) } else { [0.0, 0.0, 0.0, 1.0] }
}

fn lerp3(a: [f32; 3], b: [f32; 3], u: f32) -> [f32; 3] {
    std::array::from_fn(|i| a[i] + (b[i] - a[i]) * u)
}

fn nlerp(a: [f32; 4], b: [f32; 4], u: f32) -> [f32; 4] {
    let dot: f32 = (0..4).map(|i| a[i] * b[i]).sum();
    let s = if dot > 0.0 { 1.0 } else { -1.0 };
    normalized(std::array::from_fn(|i| a[i] + (s * b[i] - a[i]) * u))
}

#[derive(Clone, Debug)]
pub struct GhostReplay {
    pub id: u32,
    pub header: UserHeader,
    pub times: Vec<f32>,
    pub positions: Vec<[f32; 3]>,
    pub scores: Vec<f32>,
    pub poses: Vec<GhostPose>,
    pub rejected: usize,
}

impl GhostReplay {
    pub fn load(big_root: &Path, id: u32) -> Result<Self> {
        let path = replay_file(big_root, id).with_context(|| format!("{GHOST_DIR}/{id}.bin not found"))?;
        let data = std::fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
        let file = GhostFile::from_container(&data).with_context(|| format!("parsing {}", path.display()))?;
        Self::from_file(id, &file)
    }

    pub fn from_file(id: u32, file: &GhostFile) -> Result<Self> {
        let times = &file.stream(ssx_formats::ghost::POSI).context("no POSI stream")?.times;
        let (positions, scores, poses) = (file.positions()?, file.scores()?, file.poses()?);
        if positions.len() != times.len() || scores.len() != times.len() || poses.len() != times.len() {
            bail!("ghost {id}: the streams' sample counts differ");
        }
        let mut out = Self {
            id,
            header: file.header.clone(),
            times: Vec::new(),
            positions: Vec::new(),
            scores: Vec::new(),
            poses: Vec::new(),
            rejected: 0,
        };
        for (i, &t) in times.iter().enumerate() {
            if out.times.last().is_some_and(|&last| t <= last) {
                out.rejected += 1;
                continue;
            }
            out.times.push(t);
            out.positions.push(positions[i]);
            out.scores.push(scores[i]);
            out.poses.push(GhostPose::of(&poses[i]));
        }
        if out.times.is_empty() {
            bail!("ghost {id}: no samples");
        }
        Ok(out)
    }

    pub fn duration(&self) -> f32 {
        *self.times.last().unwrap_or(&0.0)
    }

    pub fn finished(&self, t: f32) -> bool {
        t > self.duration()
    }

    fn bracket(&self, t: f32) -> (usize, usize, f32) {
        let i = self.times.partition_point(|&x| x < t);
        if i == 0 {
            return (0, 0, 0.0);
        }
        if i == self.times.len() {
            let last = self.times.len() - 1;
            return (last, last, 0.0);
        }
        let (t0, t1) = (self.times[i - 1], self.times[i]);
        (i - 1, i, if t1 > t0 { (t - t0) / (t1 - t0) } else { 0.0 })
    }

    pub fn position(&self, t: f32) -> [f32; 3] {
        let (a, b, u) = self.bracket(t);
        lerp3(self.positions[a], self.positions[b], u)
    }

    pub fn pose(&self, t: f32) -> GhostPose {
        let (a, b, u) = self.bracket(t);
        GhostPose::blend(&self.poses[a], &self.poses[b], u)
    }

    pub fn fed_score(&self, t: f32) -> f32 {
        let i = self.times.partition_point(|&x| x < t);
        self.scores.get((i + 1).min(self.scores.len().saturating_sub(1))).copied().unwrap_or(0.0)
    }

    pub fn time_below(&self, y: f32) -> Option<f32> {
        let i = self.positions.iter().position(|p| p[1] < y)?;
        if i == 0 {
            return Some(self.times[0]);
        }
        let (y0, y1) = (self.positions[i - 1][1], self.positions[i][1]);
        let u = if y0 > y1 { (y0 - y) / (y0 - y1) } else { 1.0 };
        Some(self.times[i - 1] + (self.times[i] - self.times[i - 1]) * u)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn auxiliary_pose() -> GhostPose {
        GhostPose {
            trajectory_rotation: [0.0, 0.0, 0.0, 1.0],
            trajectory_position: [0.0; 3],
            rotations: [[0.0, 0.0, 0.0, 1.0]; 29],
            translations: [[0.0; 3]; 12],
            flags: 0,
            classic_rig: false,
            velocity: [0.0; 3],
            snow_spray_carving: [0.0; 3],
            snow_spray_speed: 0.0,
            landing_impact: 0.0,
            snow_coverage: 0.0,
        }
    }

    #[test]
    fn auxiliary_halves_truncate_and_flush() {
        assert_eq!(requantize_half(1.0009), 1.0);
        assert_eq!(requantize_half(-1.0009), -1.0);
        assert_eq!(requantize_half(2f32.powi(-15)), 0.0);
        assert_eq!(requantize_half(-2f32.powi(-15)).to_bits(), (-0.0f32).to_bits());
        assert_eq!(requantize_half(2f32.powi(-14)), 2f32.powi(-14));
        assert_eq!(requantize_half(65536.0), 65536.0);
        let (mut a, mut b) = (auxiliary_pose(), auxiliary_pose());
        a.snow_spray_speed = 1.0;
        b.snow_spray_speed = 1.001_953_1;
        a.landing_impact = 2f32.powi(-14);
        assert_eq!(GhostPose::blend(&a, &b, 0.375).snow_spray_speed, 1.0);
        assert_eq!(GhostPose::blend(&a, &b, 0.5).landing_impact, 0.0);
    }

    #[test]
    fn auxiliary_carving_requantizes() {
        let (mut a, mut b) = (auxiliary_pose(), auxiliary_pose());
        let step = 1.5 / 512.0;
        b.snow_spray_carving = [step, -step, 2.0 * step];
        assert_eq!(GhostPose::blend(&a, &b, 0.5).snow_spray_carving, [0.0, -step, step]);
        a.snow_spray_carving = [0.75, -0.75, 0.0];
        assert_eq!(GhostPose::blend(&a, &a, 0.0).snow_spray_carving, a.snow_spray_carving);
    }

    #[test]
    fn auxiliary_coverage_stays_continuous() {
        let (a, mut b) = (auxiliary_pose(), auxiliary_pose());
        b.snow_coverage = 1.0 / 255.0;
        assert_eq!(GhostPose::blend(&a, &b, 0.5).snow_coverage, 0.5 / 255.0);
    }

    #[test]
    fn joint_translations_requantize_all_channels_in_biased_code_space() {
        let (a, mut b) = (auxiliary_pose(), auxiliary_pose());
        let step = 2.0 / 512.0;
        b.translations.fill([step, -step, 3.0 * step]);
        let pose = GhostPose::blend(&a, &b, 0.5);
        assert!(pose.translations.iter().all(|p| *p == [0.0, -step, step]));
        b.trajectory_position = [step, -step, 3.0 * step];
        assert_eq!(GhostPose::blend(&a, &b, 0.5).trajectory_position, [step / 2.0, -step / 2.0, 1.5 * step]);
    }

    #[test]
    fn joint_translation_grid_endpoints_and_vector_wrap() {
        let (mut a, mut b) = (auxiliary_pose(), auxiliary_pose());
        a.translations.fill([-2.0, 0.0, 511.0 / 256.0]);
        b.translations.fill([0.75, -0.75, 0.5]);
        assert_eq!(GhostPose::blend(&a, &b, 0.0).translations, a.translations);
        assert_eq!(GhostPose::blend(&a, &b, 1.0).translations, b.translations);
        let wrapped = ghost::unpack_vector(ghost::pack_vector([-2.0, 0.0, 2.0], [2.0; 3]), [2.0; 3]);
        assert_eq!(wrapped, [-2.0, 0.0, -2.0]);
    }

    #[test]
    fn auxiliary_velocity_repacks_blended_direction_and_length() {
        assert_eq!(blend_velocity([1.0, 0.0, 0.0], [-1.0, 0.0, 0.0], 0.5), [0.0; 3]);
        let out = blend_velocity([1.0, 0.0, 0.0], [0.0, 1.0, 0.0], 0.5);
        assert!(out[0] > 0.498 && out[0] < 0.5);
        assert_eq!(out[0], out[1]);
        assert_eq!(out[2], 0.0);
        let range = ghost::unpack_half(0x3c01);
        assert_eq!(blend_velocity([1.0, 0.0, 0.0], [1.0, 0.0, 0.0], 0.0), [511.0 * range / 512.0, 0.0, 0.0]);
        let (mut a, mut b) = (auxiliary_pose(), auxiliary_pose());
        a.classic_rig = true;
        b.classic_rig = false;
        assert!(GhostPose::blend(&a, &b, 1.0).classic_rig);
    }

    #[test]
    fn tints() {
        let gold = tint_rgba(EXPLORE_GHOST_TINT);
        assert!((gold[0] - 243.0 / 255.0).abs() < 1e-6 && (gold[1] - 184.0 / 255.0).abs() < 1e-6);
        assert!((gold[2] - 5.0 / 255.0).abs() < 1e-6 && gold[3] == 1.0);
        assert_eq!(tint_rgba(SPAWN_TINT)[0], 40.0 / 255.0);
    }

    #[test]
    fn blend_takes_the_short_way() {
        let a = [0.0, 0.0, 0.0, 1.0];
        let b = [0.0, -0.382_683_43, 0.0, -0.923_879_5];
        let q = ghost::blend_rotation(a, b, 1.0);
        assert!((q[1] - 0.382_683_43).abs() < 1e-6 && (q[3] - 0.923_879_5).abs() < 1e-6);
    }

    #[test]
    fn blended_flags() {
        let pose = |flags| GhostPose {
            trajectory_rotation: [0.0, 0.0, 0.0, 1.0],
            trajectory_position: [0.0; 3],
            rotations: [[0.0, 0.0, 0.0, 1.0]; 29],
            translations: [[0.0; 3]; 12],
            flags,
            classic_rig: false,
            velocity: [0.0; 3],
            snow_spray_carving: [0.0; 3],
            snow_spray_speed: 0.0,
            landing_impact: 0.0,
            snow_coverage: 0.0,
        };
        let (a, b) = (pose(FLAG_GROUND_RAY), pose(FLAG_GROUND_RAY | FLAG_BOARD_PARTICLE));
        assert_eq!(GhostPose::blend(&a, &b, 0.49).flags, FLAG_GROUND_RAY);
        assert!(GhostPose::blend(&a, &b, 0.5).board_particle());
        assert_eq!(GhostPose::blend(&a, &pose(FLAG_BOARD_PARTICLE), 0.9).flags, FLAG_BOARD_PARTICLE);
    }

    #[test]
    fn brackets() {
        let pose = GhostPose {
            trajectory_rotation: [0.0, 0.0, 0.0, 1.0],
            trajectory_position: [0.0; 3],
            rotations: [[0.0, 0.0, 0.0, 1.0]; 29],
            translations: [[0.0; 3]; 12],
            flags: 0,
            classic_rig: false,
            velocity: [0.0; 3],
            snow_spray_carving: [0.0; 3],
            snow_spray_speed: 0.0,
            landing_impact: 0.0,
            snow_coverage: 0.0,
        };
        let r = GhostReplay {
            id: 1,
            header: UserHeader { gamertag: String::new(), gear_flags: 0, score: 0, build: 0 },
            times: vec![0.0, 0.5, 1.0],
            positions: vec![[0.0, 10.0, 0.0], [1.0, 9.0, 0.0], [3.0, 7.0, 0.0]],
            scores: vec![0.0; 3],
            poses: vec![pose; 3],
            rejected: 0,
        };
        assert_eq!(r.position(-1.0), [0.0, 10.0, 0.0]);
        assert_eq!(r.position(0.75), [2.0, 8.0, 0.0]);
        assert_eq!(r.position(2.0), [3.0, 7.0, 0.0]);
        assert!(r.finished(1.01) && !r.finished(1.0));
        assert_eq!(r.time_below(8.0), Some(0.75));
    }
}
