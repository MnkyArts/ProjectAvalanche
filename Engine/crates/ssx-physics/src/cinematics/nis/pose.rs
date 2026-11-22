use glam::{Quat, Vec3};
use ssx_assets::nis::{
    LayerClock, LayerSource, NisActor, NisClip, NisLayer, ROLE_CAMERA1, ROLE_CHARACTER1, ROLE_GHOST1, ROLE_HELICOPTER1,
};
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
        let t = self.side_time(s);
        let e = self.element_at(s, a, t);
        let p = e.clip.sample(e.clip_time(t)).ok()?;
        Some((a, e.clip, p))
    }

    fn side_joint(&self, s: &Side, role: i64, joint: &str) -> Option<Rt> {
        let (a, clip, p) = self.side_pose(s, role)?;
        let (qn, tn) = (format!("{joint}.q"), format!("{joint}.t"));
        let (mut q, mut t) = (clip.quat(&p, &qn), clip.vec3(&p, &tn));
        for (l, at, v) in self.side_layers(s, a) {
            q = l.over_quat(at, v, &qn, q);
            t = l.over_vec3(at, v, &tn, t);
        }
        let (dq, dt) = self.assets.nis.rig(role).joint_default(joint);
        Some(rt(q.unwrap_or(dq), t.unwrap_or(dt)))
    }

    fn side_layers<'a>(&'a self, s: &'a Side, a: &'a NisActor) -> impl Iterator<Item = (&'a NisLayer, f32, f32)> {
        let camera = a.role == ROLE_CAMERA1;
        let first = camera && self.side_element(s, a).piece.is_none();
        a.layers.iter().filter(move |l| camera && (first || l.clock == LayerClock::Node)).map(move |l| {
            let t = match l.clock {
                LayerClock::Node => self.node_time(s),
                LayerClock::Element(c) => c.clip_time(self.side_time(s)),
            };
            (l, l.time(t), self.layer_signal(l))
        })
    }

    fn layer_signal(&self, l: &NisLayer) -> f32 {
        match &l.source {
            LayerSource::Clip(_) => 0.0,
            LayerSource::BlendArray { signal, signal_default, .. } => match self.camera_wind {
                Some(v) if signal == "FLT_NisWindValue" => v,
                _ => *signal_default,
            },
        }
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
        self.offset_frame(a.offset.as_ref())
    }

    fn offset_frame(&self, offset: Option<&(String, String)>) -> Rt {
        let (t, r) = offset.map_or(("", ""), |(t, r)| (t.as_str(), r.as_str()));
        let p = if t == "VEC_NisPositionAlternate" { self.alternate.1 } else { self.position + self.tuning };
        let q = if r == "QUAT_NisOrientationAlternate" { self.alternate.0 } else { self.orientation };
        (q, p)
    }

    fn toc_trajectory(&self, ticks: f32, role: i64) -> Option<Rt> {
        let joint = trajectory_joint(role);
        self.blended(ticks, role, joint, |s| {
            let a = self.scene_of(s.scene).actor(role)?;
            Some(compose(self.offset_frame(self.side_element(s, a).offset), self.side_joint(s, role, joint)?))
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

    fn side_trajectory(&self, s: &Side) -> Option<Rt> {
        let a = self.scene_of(s.scene).actor(ROLE_CHARACTER1)?;
        let mut at = self.side_time(s);
        let mut e = self.element_at(s, a, at);
        if e.clip.delta_trajectory {
            let (i, p) = e.piece?;
            let before = self.side_time(&Side { ticks: (s.ticks - self.dt).max(0.0), ..*s });
            if p.clock.begin <= before {
                return None;
            }
            (e, at) = (self.element_before(s, a, i), p.clock.begin);
            if e.clip.delta_trajectory {
                return None;
            }
        }
        let pose = e.clip.sample(e.clip_time(at)).ok()?;
        let (q, t) = e.clip.joint(&pose, self.assets.nis.rig(ROLE_CHARACTER1), "AITrajectory");
        let joint = rt(q, t);
        if e.offset.is_some() {
            return Some(compose(self.offset_frame(e.offset), joint));
        }
        // TODO(guess): the clip's trajectory is taken as the world's (no reader of the params' interface 0x307A58D6 places an actor: of its 13 uses the loader 0x83522130 fills X, Y, Z, `LaunchAngle`, `Scale`, `Mirror`, `StartTick`, the actor inits 0x83535B78 / 0x835002D0 read `StartTick` only, the tracks `Mirror`, 0x8350C840 `Scale`,
        // the camera controller keeps them at +0x98 (its update is the actor's); a read of the embedded `DefaultInitParam` is not ruled out — if X, Y, Z, `LaunchAngle` placed the actor, the scene would sit 3 m off and turned a quarter); settle: the game's picture of this NIS or the sequence container's creation of its actors.
        crate::guess_hit!();
        Some(joint)
    }

    pub fn pose_root(&self) -> Option<Rt> {
        self.current_actor(ROLE_CHARACTER1)?;
        self.blended(self.ticks(), ROLE_CHARACTER1, "AITrajectory", |s| self.side_trajectory(s))
    }

    pub fn placed_root(&self) -> Option<Rt> {
        self.current_actor(ROLE_CHARACTER1)?;
        if self.driver_places() {
            return self.attached_root(self.ticks());
        }
        self.pose_root()
    }

    pub fn driver_places(&self) -> bool {
        self.current_actor(ROLE_CHARACTER1).is_some_and(|a| {
            let e = self.side_element(&self.side(), a);
            !a.tags.drivers.is_empty() && (self.attached || e.offset.is_none() || e.clip.delta_trajectory)
        })
    }

    pub fn rider_step(&self) -> Option<Rt> {
        let sides = self.sides_at(self.ticks());
        let step = |k: usize, s: &Side| -> Option<Rt> {
            let a = self.scene_of(s.scene).actor(ROLE_CHARACTER1)?;
            let frozen = k > 0 && self.fades[k - 1].frozen;
            let before = Side { ticks: if frozen { s.ticks } else { (s.ticks - self.dt).max(0.0) }, ..*s };
            let (t0, t1) = (self.side_time(&before), self.side_time(s));
            if t1 >= t0 {
                return self.element_steps(s, a, t0, t1);
            }
            let end = self.scene_of(s.scene).looping.as_ref()?.length;
            match (self.element_steps(s, a, t0, end), self.element_steps(s, a, 0.0, t1)) {
                (Some(x), Some(y)) => Some(compose(x, y)),
                (x, y) => x.or(y),
            }
        };
        let mut acc: Option<Rt> = None;
        for k in (0..sides.len()).rev() {
            let v = step(k, &sides[k]);
            acc = match (acc, v) {
                (Some(a), Some(b)) if k < self.fades.len() => {
                    let fade = &self.fades[k];
                    Some(if fade.blend.class == "AccelBlendAsset" { b } else { mix(a, b, fade.weight()) })
                }
                (a, b) => b.or(a),
            };
        }
        acc
    }

    fn element_steps(&self, s: &Side, a: &NisActor, from: f32, to: f32) -> Option<Rt> {
        let rig = self.assets.nis.rig(a.role);
        let mut acc: Option<Rt> = None;
        let mut u = from;
        for _ in 0..=a.pieces.len() {
            let e = self.element_at(s, a, u);
            let next = e.next_begin(a).filter(|&b| b < to);
            let v = next.unwrap_or(to);
            if e.clip.delta_trajectory {
                let at = |t: f32| -> Option<Rt> {
                    let p = e.clip.sample(e.clip_time(t)).ok()?;
                    let (q, tr) = e.clip.joint(&p, rig, "AITrajectory");
                    Some(rt(q, tr))
                };
                let part = compose(inverse(at(u)?), at(v)?);
                acc = Some(acc.map_or(part, |x| compose(x, part)));
            }
            match next {
                Some(b) => u = b,
                None => break,
            }
        }
        acc
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
            None if self.side_element(&self.side(), a).offset.is_some() => self.toc_trajectory(t, ROLE_CAMERA1)?,
            None => return None,
        };
        let local = self.joint_at(t, ROLE_CAMERA1, "CameraEye")?;
        let eye = compose(traj, local);
        let hfov = self.camera_float(t, "FieldOfView")?;
        let near = self.camera_float(t, "NearClipDistance");
        Some(NisView { position: eye.1, rotation: eye.0, hfov_deg: hfov, near, trajectory: traj, eye: local })
    }

    pub fn ghost(&self) -> Option<(&str, f32, Rt)> {
        if !self.ghost_bound {
            return None;
        }
        let s = self.side();
        let a = self.scene_of(s.scene).actor(ROLE_GHOST1)?;
        let (root, _, _) = self.ghost_root?;
        let t = self.side_time(&s);
        let e = self.element_at(&s, a, t);
        Some((e.clip.name.as_str(), e.clip_time(t), root))
    }

    pub fn bind_ghost(&mut self, bound: bool) {
        self.ghost_bound = bound;
        self.update_ghost();
    }

    pub(super) fn update_ghost(&mut self) {
        let s = self.side();
        let Some(a) = self.scene_of(s.scene).actor(ROLE_GHOST1).filter(|_| self.ghost_bound) else {
            self.ghost_root = None;
            return;
        };
        let seq = self.side_time(&s);
        let e = self.element_at(&s, a, seq);
        let rig = self.assets.nis.rig(ROLE_GHOST1);
        let root = match self.ghost_root {
            Some((r, scene, prev)) if e.offset.is_none() && e.clip.delta_trajectory => {
                let from = if scene == s.scene { prev } else { self.side_time(&Side { ticks: 0.0, ..s }) };
                self.element_steps(&s, a, from, seq).map(|d| compose(r, d))
            }
            _ => e.clip.sample(e.clip_time(seq)).ok().map(|p| {
                let (q, tr) = e.clip.joint(&p, rig, "AITrajectory");
                compose(self.offset_frame(e.offset), rt(q, tr))
            }),
        };
        self.ghost_root = root.map(|r| (r, s.scene, seq));
    }

    fn camera_float(&self, ticks: f32, dof: &str) -> Option<f32> {
        let value = |s: &Side| {
            let (a, clip, p) = self.side_pose(s, ROLE_CAMERA1)?;
            let mut v = clip.float(&p, dof);
            for (l, at, sig) in self.side_layers(s, a) {
                v = l.over_float(at, sig, dof, v);
            }
            v
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

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Arc;

    use glam::{Quat, Vec3};
    use ssx_assets::anim_graph::LoopFields;
    use ssx_assets::nis::{
        Blend, Channel, ElementClock, IntroNis, NisActor, NisClip, NisLoop, NisPiece, NisScene, ROLE_CHARACTER1, Rig,
        Tags,
    };
    use ssx_formats::ant_anim::{Animation, AnimationInfo, RawAnimation, Skeleton};

    use crate::nis::{IntroAssets, IntroChoice, IntroState, IntroVariant, SceneOrigin};

    fn clip(name: &str, delta_trajectory: bool, keys: &[(i16, Quat, Vec3)]) -> NisClip {
        let data = keys.iter().flat_map(|(_, q, t)| q.to_array().into_iter().chain([t.x, t.y, t.z, 0.0])).collect();
        let info = AnimationInfo {
            name: name.into(),
            codec_type: 0,
            anim_id: 0,
            trim_offset: 0.0,
            end_frame: keys.last().map_or(0, |k| k.0 as u16),
            additive: false,
            channel_to_dof: Default::default(),
        };
        let key_times = keys.iter().map(|k| k.0).collect();
        let raw = RawAnimation { info, key_times, quat_count: 1, vec3_count: 1, float_count: 0, cycle: false, data };
        let channels = HashMap::from([
            ("AITrajectory.q".to_string(), Channel::Quat(0)),
            ("AITrajectory.t".to_string(), Channel::Vec3(0)),
        ]);
        NisClip {
            name: name.into(),
            num_ticks: f32::from(keys.last().map_or(0, |k| k.0)),
            fps_scale: 1.0,
            delta_trajectory,
            anim: ssx_assets::anim_clip::ClipData::Disc(Box::new(Animation::Raw(raw))),
            channels,
        }
    }

    fn actor(clip: NisClip, offset: Option<(&str, &str)>) -> NisActor {
        NisActor {
            name: "Character1".into(),
            role: ROLE_CHARACTER1,
            length: clip.num_ticks,
            clip,
            offset: offset.map(|(t, r)| (t.to_string(), r.to_string())),
            choices: Vec::new(),
            seed: None,
            chooser: None,
            sequences: Vec::new(),
            root: None,
            tags: Tags::default(),
            clock: ElementClock::SEQUENCE,
            pieces: Vec::new(),
            layers: Vec::new(),
        }
    }

    fn scene(node: &str, loop_end: Option<f32>, actor: NisActor, signals: &[&str]) -> NisScene {
        let fields = |loop_end| LoopFields { loop_control: false, start: 0.0, loop_start: 0.0, loop_end };
        let looping =
            loop_end.map(|e| NisLoop::of_scene(fields(e), false, std::slice::from_ref(&actor)).expect("the loop"));
        NisScene {
            node: node.into(),
            sequence: node.into(),
            looping,
            actors: vec![actor],
            node_tags: Tags::default(),
            problems: Vec::new(),
            active_signals: signals.iter().map(|s| s.to_string()).collect(),
            heli_movement: None,
        }
    }

    fn rig(name: &str) -> Rig {
        Rig {
            name: name.into(),
            skeleton: Skeleton { name: name.into(), joints: Vec::new() },
            defaults: HashMap::new(),
        }
    }

    #[test]
    fn the_ready_loops_toc_places_the_go_while_gos_blend_runs() {
        let (q_ready, t_ready) = (Quat::from_rotation_y(0.3), Vec3::new(1.0, 0.25, 2.0));
        let ready = clip("ready", false, &[(0, q_ready, t_ready), (60, q_ready, t_ready)]);
        let go = clip("go", true, &[(0, Quat::IDENTITY, Vec3::ZERO), (60, Quat::IDENTITY, Vec3::new(0.0, -1.0, 6.0))]);
        let nis = IntroNis {
            binding: "nis_binding_test".into(),
            controller: "SF_test".into(),
            ready: scene(
                "ready",
                Some(60.0),
                actor(ready, Some(("VEC_NisPosition", "QUAT_NisOrientation"))),
                &["BLN_IsNisReadyLoop"],
            ),
            jump: scene("go", None, actor(go, None), &[]),
            transition_blend: Blend::default_transition(),
            entry_blend: Blend::default_transition(),
            branch_out: None,
            bundles: Vec::new(),
            rider: rig("genchar_rig"),
            heli: rig("helicopter_rig"),
            camera: rig("camera_rig"),
            cabin: Vec::new(),
            requires_wingsuit: false,
            character_of_interest: false,
            ready_loop: true,
            jump_start: 0.0,
        };
        let choice = IntroChoice {
            script: "eventtype_test.lua".into(),
            binding: nis.binding.clone(),
            origin: SceneOrigin::LevelStart,
            locator_alternate: None,
            variant: IntroVariant::Snow,
            binds_ghost: false,
        };
        let assets = Arc::new(IntroAssets {
            choice,
            nis,
            jump_clip: 0,
            jump_clips: vec![0],
            piece_clips: Vec::new(),
            event: "test".into(),
            heli: Err("none".into()),
            loaded: None,
        });
        let world = crate::testutil::world(crate::testutil::plane(50.0, 0.0));
        let mut s = IntroState::new(assets, &world, Vec3::new(0.0, 0.0, 0.0), Quat::from_rotation_y(-0.8))
            .expect("the intro on the plane");
        let toc = |s: &IntroState| {
            let (q, p) = s.toc_frame(s.assets.nis.ready.actor(ROLE_CHARACTER1).expect("ready actor"));
            ((q * q_ready).normalize(), p + q * t_ready)
        };
        let near = |a: (Quat, Vec3), b: (Quat, Vec3)| a.1.distance(b.1) < 1e-5 && a.0.dot(b.0).abs() > 1.0 - 1e-6;
        s.advance(2.0);
        assert!(s.placed_root().is_some_and(|r| near(r, toc(&s))), "the ready loop: its TOC");
        s.transition();
        for update in 1..=3 {
            s.advance(2.0);
            assert!(!s.fades().is_empty(), "GO's blend runs in the go's update {update}");
            let root = s.placed_root();
            assert!(root.is_some_and(|r| near(r, toc(&s))), "go update {update}: the ready loop's TOC, got {root:?}");
        }
        s.advance(2.0);
        assert!(s.fades().is_empty(), "GO's blend is dropped at the go's 4th update");
        assert_eq!(s.placed_root(), None, "after the blend the go's steps move the trajectory");
    }

    #[test]
    fn a_later_track_animation_takes_over_at_its_begin() {
        let still = |t: Vec3| [(0, Quat::IDENTITY, t), (200, Quat::IDENTITY, t)];
        let ready = clip("ready", false, &still(Vec3::ZERO));
        let glide = |f: f32| Vec3::new(0.05 * f, -0.2 * f, 0.15 * f);
        let p00 = clip("p00", false, &[(0, Quat::IDENTITY, glide(0.0)), (200, Quat::IDENTITY, glide(200.0))]);
        let exit_step = |f: f32| Vec3::new(0.0, -0.1 * f, 0.3 * f);
        let mut exit =
            clip("p00_exit", true, &[(0, Quat::IDENTITY, Vec3::ZERO), (40, Quat::IDENTITY, exit_step(40.0))]);
        exit.num_ticks = 200.0;
        if let ssx_assets::anim_clip::ClipData::Disc(a) = &mut exit.anim
            && let Animation::Raw(r) = a.as_mut()
        {
            r.info.trim_offset = -161.0;
        }
        let toc = Some(("VEC_NisPosition", "QUAT_NisOrientation"));
        let mut rider = actor(p00, toc);
        rider.clock = ElementClock { begin: 0.0, start_in: 0.0, scale: 1.0, end: 161.0 };
        let clock = ElementClock { begin: 161.0, start_in: 161.0, scale: 1.0, end: 200.0 };
        rider.pieces.push(NisPiece { clock, clip: exit, offset: None });
        let mut s = cut_to(actor(ready, toc), rider, vec![usize::MAX], vec![7]);
        for _ in 0..80 {
            s.advance(2.0);
        }
        assert_eq!(s.side_time(&s.side()), 160.0);
        let (q, p) = s.toc_frame(s.assets.nis.jump.actor(ROLE_CHARACTER1).expect("rider"));
        let placed = s.placed_root().expect("before 161 the TOC places the rider");
        assert!(placed.1.distance(p + q * glide(160.0)) < 1e-4, "the TOC'd p00 at 160: {placed:?}");
        assert!(!s.root_motion() && s.rider_step().is_none(), "an absolute clip writes no delta set");
        s.advance(2.0);
        assert!(s.root_motion(), "from 161 the exit clip's root motion takes the rider on");
        let held = s.placed_root().expect("in the update across 161 the TOC'd p00 still holds the trajectory");
        assert!(held.1.distance(p + q * glide(161.0)) < 1e-4, "p00 held at its end, 161: {held:?}");
        assert_eq!((s.go_span(2.0), s.jump_clip()), ((161.0, 162.0), 7), "the go: the exit clip from its begin");
        let (_, step) = s.rider_step().expect("the exit clip's step");
        assert!(step.distance(exit_step(1.0)) < 1e-5, "only the part after 161 (frames 0 → 1): {step}");
        s.advance(2.0);
        assert_eq!(s.placed_root(), None, "after that update no TOC places the rider");
        let (_, step) = s.rider_step().expect("the exit clip's step");
        assert!(step.distance(exit_step(3.0) - exit_step(1.0)) < 1e-5, "frames 1 → 3: {step}");
    }

    fn cut_to(ready: NisActor, jump: NisActor, jump_clips: Vec<usize>, piece_clips: Vec<usize>) -> IntroState {
        let mut s = intro(ready, jump, jump_clips, piece_clips);
        s.transition();
        s
    }

    fn intro(ready: NisActor, jump: NisActor, jump_clips: Vec<usize>, piece_clips: Vec<usize>) -> IntroState {
        let nis = IntroNis {
            binding: "nis_binding_test".into(),
            controller: "SF_test".into(),
            ready: scene("ready", Some(200.0), ready, &["BLN_IsNisReadyLoop"]),
            jump: scene("jump", None, jump, &[]),
            transition_blend: Blend::cut(),
            entry_blend: Blend::default_transition(),
            branch_out: None,
            bundles: Vec::new(),
            rider: rig("genchar_rig"),
            heli: rig("helicopter_rig"),
            camera: rig("camera_rig"),
            cabin: Vec::new(),
            requires_wingsuit: false,
            character_of_interest: false,
            ready_loop: true,
            jump_start: 0.0,
        };
        let choice = IntroChoice {
            script: "eventtype_test.lua".into(),
            binding: nis.binding.clone(),
            origin: SceneOrigin::LevelStart,
            locator_alternate: None,
            variant: IntroVariant::Heli,
            binds_ghost: false,
        };
        let jump_clip = jump_clips.first().copied().unwrap_or(usize::MAX);
        let assets = Arc::new(IntroAssets {
            choice,
            nis,
            jump_clip,
            jump_clips,
            piece_clips,
            event: "test".into(),
            heli: Err("none".into()),
            loaded: None,
        });
        let world = crate::testutil::world(crate::testutil::plane(50.0, 0.0));
        IntroState::new(assets, &world, Vec3::ZERO, Quat::IDENTITY).expect("the intro on the plane")
    }

    #[test]
    fn the_ready_loop_wraps_at_its_sequences_end() {
        let glide = |f: f32| Vec3::new(0.05 * f, -0.2 * f, 0.15 * f);
        let keys = [(0, Quat::IDENTITY, glide(0.0)), (100, Quat::IDENTITY, glide(100.0))];
        let toc = Some(("VEC_NisPosition", "QUAT_NisOrientation"));
        let go = actor(clip("go", true, &keys), None);
        let mut s = intro(actor(clip("ready", false, &keys), toc), go, vec![usize::MAX], Vec::new());
        let (q, p) = s.toc_frame(s.assets.nis.ready.actor(ROLE_CHARACTER1).expect("rider"));
        let at = |s: &IntroState, f: f32| {
            let root = s.placed_root().expect("the TOC places the rider");
            assert!(root.1.distance(p + q * glide(f)) < 1e-4, "at clip frame {f}: {root:?}");
        };
        for update in 1..=60 {
            s.advance(2.0);
            let ticks = 2.0 * update as f32;
            assert_eq!(s.side_time(&s.side()), ticks % 100.0, "update {update}");
            at(&s, ticks % 100.0);
        }
    }

    #[test]
    fn a_first_track_animation_plays_on_its_elements_clock() {
        let ready = clip("ready", false, &[(0, Quat::IDENTITY, Vec3::ZERO), (200, Quat::IDENTITY, Vec3::ZERO)]);
        let glide = |f: f32| Vec3::new(0.05 * f, -0.2 * f, 0.15 * f);
        let keys = [(0, Quat::IDENTITY, glide(0.0)), (400, Quat::IDENTITY, glide(400.0))];
        let toc = Some(("VEC_NisPosition", "QUAT_NisOrientation"));
        let mut placed = actor(clip("placed", false, &keys), toc);
        placed.clock = ElementClock { begin: 0.0, start_in: 80.0, scale: 0.5, end: 300.0 };
        let mut s = cut_to(actor(ready.clone(), toc), placed, vec![usize::MAX], Vec::new());
        for _ in 0..80 {
            s.advance(2.0);
        }
        let (q, p) = s.toc_frame(s.assets.nis.jump.actor(ROLE_CHARACTER1).expect("rider"));
        let root = s.placed_root().expect("the TOC places the rider");
        assert!(root.1.distance(p + q * glide(120.0)) < 1e-3, "sequence 160: clip (80 + 160)·0.5 = 120, {root:?}");
        for _ in 0..80 {
            s.advance(2.0);
        }
        let root = s.placed_root().expect("the TOC places the rider");
        assert!(root.1.distance(p + q * glide(190.0)) < 1e-3, "sequence 320, held at 300: clip 190, {root:?}");
        let mut go = actor(clip("go", true, &keys), None);
        go.clock = ElementClock { begin: 0.0, start_in: 0.0, scale: 2.0, end: f32::INFINITY };
        let mut s = cut_to(actor(ready, toc), go, vec![3], Vec::new());
        for _ in 0..5 {
            s.advance(2.0);
        }
        assert!(s.root_motion());
        assert_eq!((s.go_span(2.0), s.jump_clip()), ((16.0, 20.0), 3), "the go clip over sequence 8 → 10, ·2");
    }
}
