use bevy::prelude::*;

use super::collision::{physics_queries, post_update_near};
use super::meta::REVERSE_ANCHOR_Y;
use super::noise::{CarvingNoise, blend_eye};
use super::{
    CAMERA_DISTANCE_FAR, CAMERA_TYPE_15P, CAMERA_TYPE_FAR, CAMERA_TYPE_NORMAL, CAMERA_TYPE_REVERSE, CamNode,
    CamSnapshot, CameraNoise, Chase, HANDBACK_CURVE, HANDBACK_TICKS, MetaFilters, NoiseSignals, Snapshot, StartState,
    Wipeout, camera_node,
};
use crate::camera::GAME_NEAR;
use crate::camera::graph::{self as graph, Channels, Flow, Inputs};
use crate::ride::Ride;

fn fade_weight(elapsed: &mut Option<f32>, ticks: f32, curve: i64, dt: f32) -> Option<f32> {
    let e = elapsed.as_mut()?;
    if ticks - *e <= dt {
        *elapsed = None;
        return None;
    }
    *e = (*e + dt.max(0.0)).min(ticks);
    let w = ssx_assets::anim_eval::curve_weight(*e / ticks, curve);
    if w <= 0.01 {
        *elapsed = None;
        return None;
    }
    Some(w)
}

impl Chase {
    pub(super) fn update(&mut self, ride: &Ride, i: &Inputs, dt: f32, clip: Option<&CameraNoise>) -> Snapshot {
        let exec = ride.last_exec;
        if let Some((cur, _, _)) = ride.session.buffer.playback_slots() {
            if let Some(sn) = self.snapshots.get(cur).copied().flatten() {
                self.restore(&sn);
            }
            self.rewinding = true;
            return self.view();
        }
        if self.rewinding && exec.is_some_and(|e| e.rewind.terminating) {
            if let Some(sn) = exec.and_then(|e| e.recorded).and_then(|k| self.snapshots.get(k).copied().flatten()) {
                self.restore(&sn);
            }
            self.metac = MetaFilters::default();
            self.enter(self.meta.camera_type.max(0));
            (self.handback, self.rewinding) = (Some((0.0, self.rig)), false);
        }
        let from_game = (self.rig.position, self.rig.rotation);
        self.nis_and_start_state();
        self.shared.update_skews(i.lean, dt);
        let safe =
            self.camera_pos.map(|cam| physics_queries(ride, &mut self.queries, cam, i.target_pos, self.shared.up, dt));
        let shake_weight = self.shared.avalanche_shake;
        let sh = &self.shared;
        let layers = NoiseSignals {
            carving: (sh.carving_trigger, sh.carving_weight),
            wipeout: (sh.wipeout_trigger, sh.wipeout_weight),
        };
        let leave = match self.node {
            CamNode::Start => self.start.complete || !self.start.was_in_nis,
            CamNode::Normal => self.meta.camera_type != CAMERA_TYPE_NORMAL,
            CamNode::Reverse => self.meta.camera_type != CAMERA_TYPE_REVERSE,
            CamNode::P15 => self.meta.camera_type != CAMERA_TYPE_15P,
            CamNode::Far => self.meta.camera_type != CAMERA_TYPE_FAR,
        };
        if leave && camera_node(self.meta.camera_type).is_some_and(|n| n != self.node) {
            self.enter(self.meta.camera_type);
        }
        let eye_in_place = self.eye;
        let mut sub = match self.node {
            CamNode::Start => self.start_camera(i, from_game),
            CamNode::Normal => {
                let out = self.flow.update(&mut self.shared, i, &self.rig, dt);
                let m3 = self.meta3p(i, &out, dt, graph::DISTANCE, 0.3);
                self.noise(clip, layers, Wipeout::OnWeight, dt);
                m3
            }
            CamNode::Far => {
                let out = self.flow.update(&mut self.shared, i, &self.rig, dt);
                let m3 = self.meta3p(i, &out, dt, graph::DISTANCE_FAR, 0.3);
                self.noise(clip, layers, Wipeout::Always, dt);
                m3
            }
            CamNode::Reverse => {
                let out = self.flow.update(&mut self.shared, i, &self.rig, dt);
                self.queries.collided_all = false;
                let m3 = self.meta3p(i, &out, dt, graph::REVERSE_DISTANCE, REVERSE_ANCHOR_Y);
                self.shake(clip, shake_weight, dt);
                m3
            }
            CamNode::P15 => {
                let out = self.flow.update(&mut self.shared, i, &self.rig, dt);
                let m15 = self.meta15p(i, &out, dt);
                self.noise(clip, layers, Wipeout::None, dt);
                m15
            }
        };
        if let Some(w) = fade_weight(&mut self.camera_fade, self.camera_fade_ticks, 0, dt) {
            sub = graph::blend(&sub, &self.rig, w);
            self.eye = blend_eye(self.eye, eye_in_place, w);
        }
        let mut fin = self.meta(i, &sub, safe, dt);
        self.start.in_procedural = true;
        self.flow.monitor(&mut self.shared);
        let mut handback = self.handback.map(|h| h.0);
        if let (Some(w), Some((_, old))) =
            (fade_weight(&mut handback, HANDBACK_TICKS, HANDBACK_CURVE, dt), self.handback)
        {
            fin = graph::blend(&fin, &old, w);
        }
        self.handback = handback.zip(self.handback.map(|h| h.1));
        self.near = post_update_near(&ride.world().collision, fin.position, fin.rotation, fin.fov_deg, GAME_NEAR);
        if crate::render::post::env_is("SSX_CAMERA_LOG", "on") {
            let v = Snapshot { position: fin.position, rotation: fin.rotation, fov_deg: fin.fov_deg, near: self.near };
            let eye = v.position + v.rotation * self.eye.1;
            info!(
                "chase camera: step {} {:?}/{:?}, type {} (intro ticks {}), speeds meta {:.3} flow {:.3}, fade {:?}, \
                 eye {:.2} ({:.2} m from the target), view {:.2}, FOV {:.1}, near {:.3}",
                ride.steps,
                self.node,
                self.flow.current.node,
                self.meta.camera_type,
                self.meta.ticks_in_intro_cam,
                self.meta.meta_speed,
                self.meta.filtered_speed,
                self.camera_fade,
                eye,
                eye.distance(i.target_pos),
                v.rotation * Vec3::X,
                fin.fov_deg,
                v.near
            );
        }
        self.rig = fin;
        self.previous = Some(fin.position);
        if let Some(k) = exec.and_then(|e| e.recorded) {
            self.record(k);
        }
        self.view()
    }

    fn enter(&mut self, ty: i32) {
        let Some(node) = camera_node(ty) else { return };
        self.node = node;
        self.flow = match node {
            CamNode::Reverse => Flow::reverse(),
            CamNode::P15 => Flow::cam15p(),
            CamNode::Far => Flow::far(),
            CamNode::Start | CamNode::Normal => Flow::default(),
        };
        (self.meta3p, self.meta15) = (MetaFilters::default(), MetaFilters::default());
        self.camera_fade = (node != CamNode::Reverse).then_some(0.0);
        self.camera_fade_ticks = if node == CamNode::Far { 6.0 } else { 8.0 };
        self.shake_time = None;
        (self.carving, self.wipeout_time) = (CarvingNoise::new(self.shared.carving_weight), None);
    }

    fn nis_and_start_state(&mut self) {
        let st = &mut self.start;
        if !st.in_procedural {
            (st.iterations, st.state, st.complete) = StartState::defaults();
            self.camera_pos = Some(self.rig.position);
            self.shared.camera_pos = self.rig.position;
            self.camera_orientation = self.rig.rotation;
        }
        st.was_in_nis = st.in_nis;
        st.in_nis = st.nis_active;
        st.nis_active = false;
    }

    fn start_camera(&mut self, i: &Inputs, from_game: (Vec3, Quat)) -> Channels {
        let (st, s, m) = (&mut self.start, &mut self.shared, &mut self.meta);
        let was = st.was_in_nis;
        if was {
            s.fov = self.rig.fov_deg;
        }
        m.type_speed_set = if was { 0.02 } else { 1.0 };
        let old = st.iterations;
        let second = old > -1.0 && old < 1.0;
        st.iterations = if st.state { old + 1.0 } else { -1.0 };
        st.complete = st.iterations > 1.0;
        let off = !was && second;
        let first = st.state && st.iterations < 1.0 && was;
        s.enable_meta_blending = !off;
        m.ideal_distance = match m.camera_type {
            CAMERA_TYPE_REVERSE => graph::REVERSE_DISTANCE,
            CAMERA_TYPE_FAR => CAMERA_DISTANCE_FAR,
            CAMERA_TYPE_15P => graph::CAMERA_15P_DISTANCE,
            _ => m.ideal_distance,
        };
        s.force_hinge_reset |= first;
        m.trigger_blend_reset = first || off;
        if first {
            (m.meta_position_offset, m.meta_position_desired) = (Vec3::ZERO, Vec3::ZERO);
            (m.meta_target_offset, m.meta_target_desired) = (Vec3::ZERO, Vec3::ZERO);
            (m.position_offset, m.position_desired) = (Vec3::ZERO, Vec3::ZERO);
            (m.target_offset, m.target_desired) = (Vec3::ZERO, Vec3::ZERO);
        }
        let held = (self.camera_pos.unwrap_or(from_game.0), self.camera_orientation);
        let (position, rotation) = if m.trigger_blend_reset { (from_game.0, from_game.1) } else { held };
        (self.camera_pos, self.camera_orientation) = (Some(position), rotation);
        s.camera_pos = position;
        Channels {
            position,
            rotation,
            interest: i.target_pos + Vec3::new(0.0, s.player_offset_y, 0.0),
            fov_deg: self.rig.fov_deg,
        }
    }

    fn view(&self) -> Snapshot {
        let (q, t) = self.eye;
        Snapshot {
            position: self.rig.position + self.rig.rotation * t,
            rotation: (self.rig.rotation * q).normalize(),
            fov_deg: self.rig.fov_deg,
            near: self.near,
        }
    }

    fn record(&mut self, k: usize) {
        if self.snapshots.len() <= k {
            self.snapshots.resize(ssx_physics::rewind::REWIND_SNAPSHOTS.max(k + 1), None);
        }
        self.snapshots[k] = Some(CamSnapshot {
            shared: self.shared,
            meta: self.meta,
            queries: self.queries,
            ticks_since_collision: self.ticks_since_collision,
            previous: self.previous,
            camera_pos: self.camera_pos,
            pose: self.rig,
            eye: self.eye,
            start: self.start,
            camera_orientation: self.camera_orientation,
            near: self.near,
        });
    }

    fn restore(&mut self, sn: &CamSnapshot) {
        (self.shared, self.meta, self.queries) = (sn.shared, sn.meta, sn.queries);
        (self.ticks_since_collision, self.previous, self.camera_pos) =
            (sn.ticks_since_collision, sn.previous, sn.camera_pos);
        self.rig = sn.pose;
        self.near = sn.near;
        self.eye = sn.eye;
        (self.start, self.camera_orientation) = (sn.start, sn.camera_orientation);
    }
}

#[cfg(test)]
mod tests {
    use super::super::MetaState;
    use super::*;
    use crate::camera::graph::Shared;
    #[test]
    fn fade_weights_follow_the_node_update() {
        let mut e = Some(0.0);
        let w: Vec<Option<f32>> = (0..4).map(|_| fade_weight(&mut e, 8.0, 0, 2.0)).collect();
        assert!(w[..3].iter().all(Option::is_some) && w[3].is_none() && e.is_none(), "{w:?}");
        assert!((w[0].unwrap_or(0.0) - (std::f32::consts::FRAC_PI_8).cos()).abs() < 1e-5);
    }

    #[test]
    fn start_camera_holds_the_nis_pose_for_two_executions() {
        let rig = Channels {
            position: Vec3::new(1.0, 2.0, 3.0),
            rotation: Quat::from_rotation_y(0.4),
            interest: Vec3::ZERO,
            fov_deg: 57.0,
        };
        let meta = MetaState { position_offset: Vec3::X, ..MetaState::default() };
        let mut c = Chase::base(0, Shared::new(Vec3::ZERO), meta, CamNode::Start, rig, (Quat::IDENTITY, Vec3::ZERO));
        (c.start.in_nis, c.start.nis_active) = (true, true);
        let i = Inputs {
            target_pos: Vec3::new(0.0, -1.0, 0.0),
            target_up: Vec3::Y,
            velocity: Vec3::ZERO,
            speed: 0.0,
            ground_normal: Vec3::Y,
            lean: 0.0,
            airborne: true,
            wipeout: false,
            boosting: false,
            grinding: false,
            dead: false,
            track_finished: false,
            aggregate_mass: 0.0,
            bailed_by_avalanche: false,
            alive: true,
            heli_position: Vec3::ZERO,
            heli_interact: false,
            heli_time: 0.0,
            back_press: false,
        };
        c.nis_and_start_state();
        assert!(c.start.was_in_nis && c.camera_pos == Some(rig.position));
        let first = c.start_camera(&i, (rig.position, rig.rotation));
        assert!(c.meta.trigger_blend_reset && c.shared.force_hinge_reset && c.meta.position_offset == Vec3::ZERO);
        assert_eq!(
            (first.position, first.fov_deg, c.shared.fov, c.meta.type_speed_set),
            (rig.position, 57.0, 57.0, 0.02)
        );
        (c.camera_pos, c.camera_orientation, c.start.in_procedural) = (Some(Vec3::ONE), Quat::IDENTITY, true);
        c.nis_and_start_state();
        assert!(c.start.was_in_nis);
        let second = c.start_camera(&i, (rig.position, rig.rotation));
        assert!(!c.meta.trigger_blend_reset && !c.start.complete && c.shared.enable_meta_blending);
        assert_eq!((second.position, second.rotation), (Vec3::ONE, Quat::IDENTITY));
        c.nis_and_start_state();
        assert!(!c.start.was_in_nis, "the third execution leaves the node (¬BLN_CameraWasInNIS)");
    }
}
