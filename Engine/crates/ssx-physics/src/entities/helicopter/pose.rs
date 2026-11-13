use glam::{Quat, Vec3};
use ssx_assets::helicopter::HeliPose;
use ssx_assets::nis::{NisActor, ROLE_HELICOPTER1};

use super::{HeliNode, HeliPhase, Helicopter, MoveController};
use crate::nis::Scene;

const HANDOVER_TICKS: f32 = 20.0;

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
            p.write_nis(&a.clip, time.max(0.0));
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
            p.write_nis(&actor.clip, t.max(0.0));
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
            blended.write_nis(&actor.clip, last.ticks);
            blended.blend_from(&pose, |_, _| w);
            pose = blended;
        }
        pose.q.insert("AITrajectory".into(), [0.0, 0.0, 0.0, 1.0]);
        pose.t.insert("AITrajectory".into(), [0.0; 3]);
        pose
    }

    pub fn gameplay_pose(&self) -> HeliPose {
        let mut side = self.node_side(self.node, self.ctrl.as_ref());
        if let Some((ctrl, node, elapsed)) = &self.blend {
            let w = self.blend_weight(elapsed.max(0.0));
            side = self.node_side(*node, ctrl.as_ref()).blend_valid(&side, w);
        }
        let mut pose = HeliPose::defaults(&self.assets.nis.heli);
        pose.q.extend(side.q);
        pose.t.extend(side.t);
        pose
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
        let pose = self.base_pose();
        let (mut q, mut p) = self.traj;
        for j in chain.iter().rev() {
            let lq = pose.q.get(j).map_or(Quat::IDENTITY, |v| Quat::from_array(*v));
            let lt = pose.t.get(j).map_or(Vec3::ZERO, |v| Vec3::from_array(*v));
            p += q * lt;
            q = (q * lq).normalize();
        }
        Some((q, p))
    }
}
