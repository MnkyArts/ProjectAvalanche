use glam::{Quat, Vec3};
use ssx_assets::nis::{NisActor, NisClip, ROLE_CAMERA1, ROLE_CHARACTER1, ROLE_GHOST1, ROLE_HELICOPTER1};
use ssx_formats::ant_anim::Pose;

use super::{IntroState, NisView, Rt, Side, compose, inverse, rt};

fn trajectory_joint(role: i64) -> &'static str {
    if role == ROLE_CAMERA1 { "Trajectory" } else { "AITrajectory" }
}

fn mix(a: Rt, b: Rt, w: f32) -> Rt {
    (a.0.lerp(b.0, w).normalize(), a.1.lerp(b.1, w))
}

impl IntroState {
    fn sides_at(&self, ticks: f32) -> Vec<Side> {
        let mut out = vec![Side { ticks, ..self.side() }];
        out.extend(self.fades.iter().map(|f| f.from));
        out
    }

    fn side_pose(&self, s: &Side, role: i64) -> Option<(&NisActor, &NisClip, Pose)> {
        let a = self.scene_of(s.scene).actor(role)?;
        let clip = self.side_clip(s, a);
        let p = clip.sample(self.side_time(s)).ok()?;
        Some((a, clip, p))
    }

    fn side_joint(&self, s: &Side, role: i64, joint: &str) -> Option<Rt> {
        let (_, clip, p) = self.side_pose(s, role)?;
        let (q, t) = clip.joint(&p, self.assets.nis.rig(role), joint);
        Some(rt(q, t))
    }

    fn blended(&self, ticks: f32, role: i64, joint: &str, f: impl Fn(&Side) -> Option<Rt>) -> Option<Rt> {
        let sides = self.sides_at(ticks);
        let mut acc: Option<Rt> = None;
        for i in (0..sides.len()).rev() {
            let v = f(&sides[i]);
            acc = match (acc, v) {
                (Some(a), Some(b)) if i < self.fades.len() => {
                    let fade = &self.fades[i];
                    let accel = fade.blend.class == "AccelBlendAsset" && joint == trajectory_joint(role);
                    Some(if accel { b } else { mix(a, b, fade.weight()) })
                }
                (a, b) => b.or(a),
            };
        }
        acc
    }

    fn joint_at(&self, ticks: f32, role: i64, joint: &str) -> Option<Rt> {
        self.blended(ticks, role, joint, |s| self.side_joint(s, role, joint))
    }

    fn current_actor(&self, role: i64) -> Option<&NisActor> {
        self.current().actor(role)
    }

    pub fn toc_frame(&self, a: &NisActor) -> Rt {
        let (t, r) = a.offset.as_ref().map_or(("", ""), |(t, r)| (t.as_str(), r.as_str()));
        let p = if t == "VEC_NisPositionAlternate" { self.alternate.1 } else { self.position + self.tuning };
        let q = if r == "QUAT_NisOrientationAlternate" { self.alternate.0 } else { self.orientation };
        (q, p)
    }

    fn toc_trajectory(&self, ticks: f32, role: i64) -> Option<Rt> {
        let joint = trajectory_joint(role);
        self.blended(ticks, role, joint, |s| {
            let a = self.scene_of(s.scene).actor(role)?;
            Some(compose(self.toc_frame(a), self.side_joint(s, role, joint)?))
        })
    }

    pub fn heli(&self, ticks: f32) -> Option<Rt> {
        self.toc_trajectory(ticks, ROLE_HELICOPTER1)
    }

    pub fn heli_joint(&self, ticks: f32, joint: &str) -> Option<Rt> {
        let chain = self.assets.nis.heli.chain(joint, "AITrajectory")?;
        let mut m = self.heli(ticks)?;
        for j in chain.iter().rev() {
            m = compose(m, self.joint_at(ticks, ROLE_HELICOPTER1, j)?);
        }
        Some(m)
    }

    pub fn attached_root(&self, ticks: f32) -> Option<Rt> {
        let d = self.current_actor(ROLE_CHARACTER1)?.tags.drivers.first()?;
        let target = compose(self.heli_joint(ticks, &d.target_joint)?, rt(d.offset_q, d.offset_t));
        let chain = self.assets.nis.rider.chain(&d.source_joint, "AITrajectory")?;
        let mut src = (Quat::IDENTITY, Vec3::ZERO);
        for j in chain.iter().rev() {
            src = compose(src, self.joint_at(ticks, ROLE_CHARACTER1, j)?);
        }
        Some(compose(target, inverse(src)))
    }

    pub fn placed_root(&self) -> Option<Rt> {
        let ticks = self.ticks();
        let a = self.current_actor(ROLE_CHARACTER1)?;
        let driven = !a.tags.drivers.is_empty() && (self.attached || a.offset.is_none() || a.clip.delta_trajectory);
        if !driven && a.offset.is_some() && !a.clip.delta_trajectory {
            return self.toc_trajectory(ticks, ROLE_CHARACTER1);
        }
        if !driven && !a.clip.delta_trajectory {
            // TODO(guess): the clip's trajectory is taken as the world's (no reader of the params' interface 0x307A58D6 places an actor: of its 13 uses the loader 0x83522130 fills X, Y, Z, `LaunchAngle`, `Scale`, `Mirror`, `StartTick`, the actor inits 0x83535B78 / 0x835002D0 read `StartTick` only, the tracks `Mirror`, 0x8350C840 `Scale`,
            // the camera controller keeps them at +0x98 (its update is the actor's); a read of the embedded `DefaultInitParam` is not ruled out — if X, Y, Z, `LaunchAngle` placed the actor, the scene would sit 3 m off and turned a quarter); settle: the game's picture of this NIS or the sequence container's creation of its actors.
            crate::guess_hit!();
            return self.joint_at(ticks, ROLE_CHARACTER1, "AITrajectory");
        }
        self.attached_root(ticks)
    }

    pub fn camera(&self, rider: Rt) -> Option<NisView> {
        let t = self.ticks();
        let a = self.current_actor(ROLE_CAMERA1)?;
        crate::guess_hit!();
        // TODO(guess): a random camera chooser (the survive intro's go, the heli-pickup outro's, cabin scenes 1 / 2a / 2) shows its first item; the camera's `Feature:RandomValue` (`Seed` 1876900708, the one random value of all 151 random choosers on the disc) is also drawn by `QueryTask_Cam_GetTarget`'s relay of `CameraRandomVector3` into `VEC_CameraTutorialRandomOffset` whenever that query runs; settle: when the query runs and how many draws a `RandomVector3Expression` takes, then the draws before each scene.
        let arm_of = |source: &str| self.joint_at(t, ROLE_CAMERA1, source);
        let traj = match a.tags.attachments.first() {
            Some(att) if att.target == "ID_NisTargetHelicopter" => {
                let target = self.heli_joint(t, &att.target_joint).or_else(|| self.heli(t))?;
                compose(compose(target, rt(att.offset_q, att.offset_t)), inverse(arm_of(&att.source_joint)?))
            }
            Some(att) => {
                let mut target = rider;
                if let Some(chain) = self.assets.nis.rider.chain(&att.target_joint, "AITrajectory") {
                    for j in chain.iter().rev() {
                        let Some(l) = self.joint_at(t, ROLE_CHARACTER1, j) else { break };
                        target = compose(target, l);
                    }
                }
                compose(compose(target, rt(att.offset_q, att.offset_t)), inverse(arm_of(&att.source_joint)?))
            }
            None if a.offset.is_some() => self.toc_trajectory(t, ROLE_CAMERA1)?,
            None => return None,
        };
        let local = self.joint_at(t, ROLE_CAMERA1, "CameraEye")?;
        let eye = compose(traj, local);
        let hfov = self.camera_float(t, "FieldOfView")?;
        let near = self.camera_float(t, "NearClipDistance");
        Some(NisView { position: eye.1, rotation: eye.0, hfov_deg: hfov, near, trajectory: traj, eye: local })
    }

    pub fn ghost(&self) -> Option<(&str, f32, Rt)> {
        let s = self.side();
        let a = self.scene_of(s.scene).actor(ROLE_GHOST1)?;
        let (root, _, _) = self.ghost_root?;
        Some((self.side_clip(&s, a).name.as_str(), self.side_time(&s), root))
    }

    pub(super) fn update_ghost(&mut self) {
        let s = self.side();
        let Some(a) = self.scene_of(s.scene).actor(ROLE_GHOST1) else {
            self.ghost_root = None;
            return;
        };
        let clip = self.side_clip(&s, a);
        let rig = self.assets.nis.rig(ROLE_GHOST1);
        let traj = |t: f32| {
            let p = clip.sample(t).ok()?;
            let (q, tr) = clip.joint(&p, rig, "AITrajectory");
            Some(rt(q, tr))
        };
        let t = self.side_time(&s);
        let root = match self.ghost_root {
            Some((r, scene, t0)) if a.offset.is_none() && clip.delta_trajectory => {
                let from = if scene == s.scene { t0 } else { self.side_time(&Side { ticks: 0.0, ..s }) };
                traj(from).zip(traj(t)).map(|(a0, a1)| compose(r, compose(inverse(a0), a1)))
            }
            _ => traj(t).map(|j| compose(self.toc_frame(a), j)),
        };
        self.ghost_root = root.map(|r| (r, s.scene, t));
    }

    fn camera_float(&self, ticks: f32, dof: &str) -> Option<f32> {
        let value = |s: &Side| {
            let (_, clip, p) = self.side_pose(s, ROLE_CAMERA1)?;
            clip.float(&p, dof)
        };
        let sides = self.sides_at(ticks);
        let mut acc: Option<f32> = None;
        for i in (0..sides.len()).rev() {
            acc = match (acc, value(&sides[i])) {
                (Some(a), Some(b)) if i < self.fades.len() => Some(a + (b - a) * self.fades[i].weight()),
                (a, b) => b.or(a),
            };
        }
        acc
    }
}
