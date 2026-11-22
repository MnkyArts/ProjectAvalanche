use glam::{Quat, Vec3};
use ssx_assets::helicopter::{HeliPose, blend_rotation, blend_translation};
use ssx_assets::nis::{NisActor, ROLE_HELICOPTER1, Rig};

use super::{HeliNode, HeliPhase, Helicopter, MoveController};
use crate::nis::Scene;

const HANDOVER_TICKS: f32 = 20.0;

pub type JointLocal = (Option<[f32; 4]>, Option<[f32; 3]>);

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct NisLast {
    pub cabin: Option<usize>,
    pub scene: Scene,
    pub ticks: f32,
}

impl Helicopter {
    fn nis_actor(&self, cabin: Option<usize>, scene: Scene) -> Option<&NisActor> {
        match cabin {
            Some(c) => self.assets.nis.cabin.get(c)?.scene.actor(ROLE_HELICOPTER1),
            None => self.assets.scene(scene).actor(ROLE_HELICOPTER1),
        }
    }

    pub(super) fn note_nis_pose(&mut self) {
        let HeliPhase::Nis(scene) = self.phase else { return };
        if let Some((_, ticks)) = self.clip() {
            self.nis_last = Some(NisLast { cabin: self.cabin.map(|c| c.index), scene, ticks });
        }
    }

    pub fn nis_pose(&self) -> Option<HeliPose> {
        let (actor, t) = self.clip()?;
        let mut acc: Option<HeliPose> = None;
        for &(scene, time, w, accel) in self.nis_chain.iter().rev() {
            let sc = match scene {
                crate::nis::SceneRef::Cabin(i) => &self.assets.nis.cabin.get(i)?.scene,
                crate::nis::SceneRef::Ready => &self.assets.nis.ready,
                crate::nis::SceneRef::Jump => &self.assets.nis.jump,
            };
            let Some(a) = sc.actor(ROLE_HELICOPTER1) else { continue };
            let mut p = HeliPose::defaults(&self.assets.nis.heli);
            let (clip, at) = a.clip_at(time);
            p.write_nis(clip, at.max(0.0));
            acc = Some(match acc {
                Some(mut old) => {
                    old.blend_from(&p, |j, _| if accel && j == "AITrajectory" { 1.0 } else { w });
                    old
                }
                None => p,
            });
        }
        acc.or_else(|| {
            let mut p = HeliPose::defaults(&self.assets.nis.heli);
            let (clip, at) = actor.clip_at(t);
            p.write_nis(clip, at.max(0.0));
            Some(p)
        })
    }

    pub fn base_pose(&self) -> HeliPose {
        if let Some(p) = self.nis_pose() {
            return p;
        }
        let mut pose = self.gameplay_pose();
        if let (HeliPhase::Gameplay { ticks, .. }, Some(last)) = (self.phase, self.nis_last)
            && let Some(actor) = self.nis_actor(last.cabin, last.scene)
        {
            let w = 1.0 - ssx_assets::anim_eval::curve_weight(ticks / HANDOVER_TICKS, 1);
            let mut blended = HeliPose::defaults(&self.assets.nis.heli);
            let (clip, at) = actor.clip_at(last.ticks);
            blended.write_nis(clip, at);
            blended.blend_from(&pose, |_, _| w);
            pose = blended;
        }
        pose.q.insert("AITrajectory".into(), [0.0, 0.0, 0.0, 1.0]);
        pose.t.insert("AITrajectory".into(), [0.0; 3]);
        pose
    }

    pub fn gameplay_pose(&self) -> HeliPose {
        let side = self.gameplay_side();
        let mut pose = HeliPose::defaults(&self.assets.nis.heli);
        pose.q.extend(side.q);
        pose.t.extend(side.t);
        pose
    }

    pub(super) fn gameplay_side(&self) -> HeliPose {
        let mut side = self.node_side(self.node, self.ctrl.as_ref());
        if let Some((ctrl, node, elapsed)) = &self.blend {
            let w = self.blend_weight(elapsed.max(0.0));
            side = self.node_side(*node, ctrl.as_ref()).blend_valid(&side, w);
        }
        side
    }

    pub fn gameplay_joint(&self, joint: &str) -> JointLocal {
        over_defaults(&self.assets.nis.heli, &self.gameplay_side(), joint)
    }

    fn node_side(&self, node: HeliNode, ctrl: Option<&MoveController>) -> HeliPose {
        let mut p = HeliPose::default();
        let Some(f) = self.flight.as_deref() else { return p };
        match node {
            HeliNode::ChaseTarget | HeliNode::MoveToPosition | HeliNode::PlayerInteractTarget => {
                let Some(c) = ctrl else { return p };
                if let Some(h) = &f.hover {
                    h.write(&mut p, self.board.speed_ratio, c.ticks.max(0.0));
                }
                if let Some(r) = c.posing.root {
                    p.q.insert("Root".into(), r.to_array());
                }
            }
            HeliNode::IdleEnd { ticks } => {
                if let Some(clip) = &f.end_level {
                    p.write_nis(clip, ticks.max(0.0).rem_euclid(600.0));
                }
            }
            HeliNode::Trick { seq, ticks, mirror } => {
                if let Some(t) = f.tricks.get(seq) {
                    p = HeliPose::trick_dofs(&self.assets.nis.heli, &t.clip, ticks.max(0.0), mirror);
                }
            }
            HeliNode::IdleStart => {}
        }
        p
    }

    pub fn joint_world(&self, joint: &str) -> Option<(Quat, Vec3)> {
        let rig = &self.assets.nis.heli;
        let chain = rig.chain(joint, "AITrajectory")?;
        let locals = if self.clip().is_some() {
            let pose = self.base_pose();
            chain.iter().map(|j| (pose.q.get(j).copied(), pose.t.get(j).copied())).collect()
        } else {
            self.base_joints(&chain)
        };
        let (mut q, mut p) = self.traj;
        for (lq, lt) in locals.iter().rev() {
            let lq = lq.map_or(Quat::IDENTITY, Quat::from_array);
            let lt = lt.map_or(Vec3::ZERO, Vec3::from_array);
            p += q * lt;
            q = (q * lq).normalize();
        }
        Some((q, p))
    }

    // PORT(choice): a joint's world transform (the downwash query's, the objectives' targets) reads one chain of the
    // helicopter's pose, so its joints are computed alone by the same per-joint arithmetic (`blend_rotation` /
    // `blend_translation`, `HeliPose::default_of`) instead of the whole string-keyed pose (0.39 ms of a 3.25 ms step on
    // Mount Eddie, traced), the same values; how the console evaluated these joints was not decoded.
    fn base_joints(&self, joints: &[String]) -> Vec<JointLocal> {
        let rig = &self.assets.nis.heli;
        let side = self.gameplay_side();
        let handover = match (self.phase, self.nis_last) {
            (HeliPhase::Gameplay { ticks, .. }, Some(last)) => self.nis_actor(last.cabin, last.scene).map(|actor| {
                let w = 1.0 - ssx_assets::anim_eval::curve_weight(ticks / HANDOVER_TICKS, 1);
                let (clip, at) = actor.clip_at(last.ticks);
                (w, clip, clip.sample(at).ok())
            }),
            _ => None,
        };
        let joint = |j: &String| {
            if j == "AITrajectory" {
                return (Some([0.0, 0.0, 0.0, 1.0]), Some([0.0; 3]));
            }
            let (gq, gt) = over_defaults(rig, &side, j);
            let Some((w, clip, sampled)) = &handover else { return (gq, gt) };
            let d = HeliPose::default_of(rig, j);
            let cq = sampled.as_ref().and_then(|p| clip.quat(p, &format!("{j}.q"))).or(d.map(|d| d.0));
            let ct = sampled.as_ref().and_then(|p| clip.vec3(p, &format!("{j}.t"))).or(d.map(|d| d.1));
            let blends = *w > 0.0 || w.is_nan();
            let q = match gq {
                Some(b) if blends => Some(blend_rotation(cq.unwrap_or([0.0, 0.0, 0.0, 1.0]), b, *w)),
                _ => cq,
            };
            let t = match gt {
                Some(b) if blends => Some(blend_translation(ct.unwrap_or([0.0; 3]), b, *w)),
                _ => ct,
            };
            (q, t)
        };
        joints.iter().map(joint).collect()
    }
}

pub(super) fn over_defaults(rig: &Rig, side: &HeliPose, j: &str) -> JointLocal {
    let d = HeliPose::default_of(rig, j);
    (side.q.get(j).copied().or(d.map(|d| d.0)), side.t.get(j).copied().or(d.map(|d| d.1)))
}
