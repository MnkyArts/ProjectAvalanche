use super::*;

pub struct ClipInst {
    clip: Option<usize>,
    time: f32,
    prev: f32,
    num_ticks: f32,
    fps_scale: f32,
    name: String,
    delta_trajectory: bool,
}

impl ClipInst {
    pub fn new(env: &Env, ctrl: CtrlId) -> Self {
        match &env.g.ctrls[ctrl].1 {
            Ctrl::Clip { anim, anim_guid, num_ticks, fps_scale, delta_trajectory, .. } => {
                let clip = anim_guid.and_then(|g| env.clip_index.get(&g).copied());
                let drives = |t: usize| clip.is_some_and(|c| env.clips[c].driven.contains(&t));
                ClipInst {
                    clip,
                    time: 0.0,
                    prev: 0.0,
                    num_ticks: *num_ticks,
                    fps_scale: *fps_scale,
                    name: anim.clone(),
                    delta_trajectory: *delta_trajectory != 0 && env.rig.trajectory.is_some_and(drives),
                }
            }
            _ => ClipInst {
                clip: None,
                time: 0.0,
                prev: 0.0,
                num_ticks: 0.0,
                fps_scale: 1.0,
                name: String::new(),
                delta_trajectory: false,
            },
        }
    }

    fn trajectory_at(&self, env: &Env, c: usize, t: usize, tick: f32) -> ([f32; 4], [f32; 3]) {
        let mut nodes = env.bind.nodes.clone();
        env.clips[c].sample_into(env.clips[c].frame(tick * self.fps_scale), &mut nodes);
        (ssx_formats::ant_anim::normalize(nodes[t].rotation), nodes[t].translation)
    }
}

impl Controller for ClipInst {
    fn update(&mut self, _env: &Env, _gs: &mut GameState, dt: f32) {
        self.prev = self.time;
        self.time += dt.max(0.0).min((self.num_ticks - self.time).max(0.0));
    }
    fn pose(&self, env: &Env, _gs: &GameState, out: &mut AnimPose) {
        if let Some(c) = self.clip {
            let keep = env
                .rig
                .trajectory
                .filter(|_| self.delta_trajectory)
                .map(|t| (t, out.pose.nodes[t], out.valid_nodes[t]));
            env.clips[c].sample_anim_into(env.clips[c].frame(self.time * self.fps_scale), out);
            if let Some((t, srt, valid)) = keep {
                (out.pose.nodes[t], out.valid_nodes[t]) = (srt, valid);
            }
        }
    }
    fn delta(&self, env: &Env, _gs: &GameState) -> Delta {
        let (Some(c), Some(t), true) = (self.clip, env.rig.trajectory, self.delta_trajectory) else {
            return Delta::None;
        };
        let ((q0, t0), (q1, t1)) = (self.trajectory_at(env, c, t, self.prev), self.trajectory_at(env, c, t, self.time));
        let c0 = [-q0[0], -q0[1], -q0[2], q0[3]];
        let d = crate::rigamate::rotate(c0, [t1[0] - t0[0], t1[1] - t0[1], t1[2] - t0[2], 0.0]);
        Delta::Value { q: ssx_formats::ant_anim::normalize(crate::rigamate::quat_mul(c0, q1)), t: [d[0], d[1], d[2]] }
    }
    fn time(&self) -> f32 {
        self.time
    }
    fn length(&self) -> f32 {
        self.num_ticks
    }
    fn set_time(&mut self, _env: &Env, _gs: &GameState, t: f32) {
        self.time = t.clamp(0.0, self.num_ticks.max(0.0));
        self.prev = self.time;
    }
    fn label(&self, _env: &Env, weight: f32, out: &mut Vec<(String, f32)>) {
        out.push((format!("{}@{:.0}", self.name, self.time * self.fps_scale), weight));
    }
}

type Sub = Option<Box<dyn Controller>>;

fn sub_time(s: &Sub) -> f32 {
    s.as_ref().map_or(0.0, |c| c.time())
}
fn sub_len(s: &Sub) -> f32 {
    s.as_ref().map_or(0.0, |c| c.length())
}

pub struct LoopingInst {
    sub: Sub,
    first: bool,
    start: f32,
    loop_control: bool,
    loop_start: f32,
    loop_end: f32,
    partial: Option<Delta>,
}

impl Controller for LoopingInst {
    fn update(&mut self, env: &Env, gs: &mut GameState, dt: f32) {
        let Some(s) = &mut self.sub else { return };
        if std::mem::replace(&mut self.first, false) && self.start > 0.0 {
            s.update(env, gs, self.start);
        }
        let rem = if self.loop_control { self.loop_end - s.time() } else { s.length() - s.time() }.max(0.0);
        self.partial = None;
        if rem <= dt && s.length() > 0.0 {
            s.update(env, gs, rem);
            self.partial = Some(s.delta(env, gs));
            set_phase(env, gs, s.as_mut(), 0.0);
            if self.loop_control && self.loop_start > 0.0 {
                s.update(env, gs, self.loop_start);
            }
            s.update(env, gs, dt - rem);
        } else {
            s.update(env, gs, dt);
        }
    }
    fn pose(&self, env: &Env, gs: &GameState, out: &mut AnimPose) {
        if let Some(s) = &self.sub {
            s.pose(env, gs, out);
        }
    }
    fn delta(&self, env: &Env, gs: &GameState) -> Delta {
        let last = self.sub.as_ref().map_or(Delta::None, |s| s.delta(env, gs));
        self.partial.map_or(last, |p| Delta::accumulate(p, last, env.physics))
    }
    fn time(&self) -> f32 {
        sub_time(&self.sub)
    }
    fn length(&self) -> f32 {
        sub_len(&self.sub)
    }
    fn looping(&self) -> bool {
        true
    }
    fn set_time(&mut self, env: &Env, gs: &GameState, t: f32) {
        let l = sub_len(&self.sub);
        if let Some(s) = &mut self.sub {
            s.set_time(env, gs, if l > 0.0 { t.rem_euclid(l).min(l * (1.0 - f32::EPSILON)) } else { t });
        }
    }
    fn windows(&self, env: &Env) -> Vec<Window> {
        self.sub.as_ref().map_or_else(Vec::new, |s| s.windows(env))
    }
    fn label(&self, env: &Env, weight: f32, out: &mut Vec<(String, f32)>) {
        if let Some(s) = &self.sub {
            s.label(env, weight, out);
        }
    }
    fn path(&self, env: &Env, out: &mut Vec<PathNode>) {
        if let Some(s) = &self.sub {
            s.path(env, out);
        }
    }
    fn exit(&mut self, env: &Env, gs: &GameState) {
        if let Some(s) = &mut self.sub {
            s.exit(env, gs);
        }
    }
}

pub struct RateInst {
    sub: Sub,
    signal: Option<SigId>,
    scale: f32,
    range: [f32; 2],
}

pub struct PhaseInst {
    sub: Sub,
    signal: Option<SigId>,
    range: [f32; 2],
}

pub struct RefPoseInst {
    sub: Sub,
    reference: Sub,
    weight: Option<SigId>,
    offset: bool,
}

macro_rules! forward_timeline {
    () => {
        forward_timeline!(no_exit);
        fn exit(&mut self, env: &Env, gs: &GameState) {
            if let Some(s) = &mut self.sub {
                s.exit(env, gs);
            }
        }
        fn delta(&self, env: &Env, gs: &GameState) -> Delta {
            self.sub.as_ref().map_or(Delta::None, |s| s.delta(env, gs))
        }
    };
    (no_exit) => {
        fn time(&self) -> f32 {
            sub_time(&self.sub)
        }
        fn length(&self) -> f32 {
            sub_len(&self.sub)
        }
        fn looping(&self) -> bool {
            self.sub.as_ref().is_some_and(|s| s.looping())
        }
        fn set_time(&mut self, env: &Env, gs: &GameState, t: f32) {
            if let Some(s) = &mut self.sub {
                s.set_time(env, gs, t);
            }
        }
        fn windows(&self, env: &Env) -> Vec<Window> {
            self.sub.as_ref().map_or_else(Vec::new, |s| s.windows(env))
        }
        fn label(&self, env: &Env, weight: f32, out: &mut Vec<(String, f32)>) {
            if let Some(s) = &self.sub {
                s.label(env, weight, out);
            }
        }
        fn path(&self, env: &Env, out: &mut Vec<PathNode>) {
            if let Some(s) = &self.sub {
                s.path(env, out);
            }
        }
    };
}

impl Controller for RateInst {
    fn update(&mut self, env: &Env, gs: &mut GameState, dt: f32) {
        let rate = self.signal.map_or(self.scale, |s| gs.get(s)).max(self.range[0]).min(self.range[1]);
        if let Some(s) = &mut self.sub {
            s.update(env, gs, dt * rate);
        }
    }
    fn pose(&self, env: &Env, gs: &GameState, out: &mut AnimPose) {
        if let Some(s) = &self.sub {
            s.pose(env, gs, out);
        }
    }
    forward_timeline!();
}

impl Controller for PhaseInst {
    fn update(&mut self, env: &Env, gs: &mut GameState, _dt: f32) {
        let v = self.signal.map_or(0.0, |s| gs.get(s));
        let span = self.range[1] - self.range[0];
        let p = if span != 0.0 { ((v - self.range[0]) / span).clamp(0.0, 1.0) } else { 0.0 };
        if let Some(s) = &mut self.sub {
            set_phase(env, gs, s.as_mut(), p);
            s.update(env, gs, 0.0);
        }
    }
    fn pose(&self, env: &Env, gs: &GameState, out: &mut AnimPose) {
        if let Some(s) = &self.sub {
            s.pose(env, gs, out);
        }
    }
    forward_timeline!();
}

impl Controller for RefPoseInst {
    fn update(&mut self, env: &Env, gs: &mut GameState, dt: f32) {
        if let Some(s) = &mut self.sub {
            s.update(env, gs, dt);
        }
    }
    fn pose(&self, env: &Env, gs: &GameState, out: &mut AnimPose) {
        let mut subject = AnimPose::empty(env.bind);
        if let Some(s) = &self.sub {
            s.pose(env, gs, &mut subject);
        }
        let mut reference = AnimPose::empty(env.bind);
        if let Some(r) = &self.reference {
            r.pose(env, gs, &mut reference);
        }
        if self.offset {
            *out = crate::anim_pose::difference(&subject, &reference);
        } else {
            let w = self.weight.map_or(1.0, |s| gs.get(s));
            reference.blend(&subject, w);
            *out = reference;
        }
    }
    forward_timeline!(no_exit);
    fn exit(&mut self, env: &Env, gs: &GameState) {
        for s in [&mut self.sub, &mut self.reference].into_iter().flatten() {
            s.exit(env, gs);
        }
    }
    fn delta(&self, _env: &Env, _gs: &GameState) -> Delta {
        Delta::Untraced
    }
}

pub struct PassInst {
    pub sub: Sub,
    pub class: Option<String>,
    pub exited: bool,
    layer: Vec<(SigId, f32)>,
    ground: Option<(usize, [f32; 3])>,
}

impl PassInst {
    pub fn new(sub: Sub, class: Option<String>) -> Self {
        PassInst { sub, class, exited: false, layer: Vec::new(), ground: None }
    }

    fn subject_state(&self, gs: &GameState) -> Option<GameState> {
        if self.layer.is_empty() {
            return None;
        }
        let local = gs.clone();
        for &(id, v) in &self.layer {
            local.set(id, v);
        }
        Some(local)
    }

    fn frozen_carving(&self) -> bool {
        self.exited && self.class.as_deref() == Some("CarvingControllerAsset")
    }
}

impl Controller for PassInst {
    fn update(&mut self, env: &Env, gs: &mut GameState, dt: f32) {
        let dt = if self.frozen_carving() { 0.0 } else { dt };
        self.ground = match (self.class.as_deref(), env.physics) {
            (Some(c @ "CarvingControllerAsset"), Some(p))
                if !self.exited && dt.abs() > 1.525_878_9e-5 && p.controllers.iter().any(|x| x == c) =>
            {
                p.ground
            }
            _ => None,
        };
        if self.exited
            && self.class.as_deref() == Some("AerialControllerAsset")
            && let Some(p) = env.physics.filter(|p| !p.aerial_layer.is_empty())
        {
            let values = p.aerial_layer.iter().chain(&p.aerial_scripts);
            self.layer = values.filter_map(|(n, v)| env.g.signal_id(n).map(|id| (id, *v))).collect();
        }
        let local = self.subject_state(gs);
        let Some(s) = &mut self.sub else { return };
        let Some(mut local) = local else {
            s.update(env, gs, dt);
            return;
        };
        s.update(env, &mut local, dt);
        let keep: Vec<SigId> = self.layer.iter().map(|&(id, _)| id).collect();
        gs.merge_from(&local, &keep);
        gs.audio_tags.append(&mut local.audio_tags);
        gs.drivers.append(&mut local.drivers);
        gs.queries.append(&mut local.queries);
        gs.pins.append(&mut local.pins);
    }
    fn pose(&self, env: &Env, gs: &GameState, out: &mut AnimPose) {
        if let Some(s) = &self.sub {
            match self.subject_state(gs) {
                Some(local) => s.pose(env, &local, out),
                None => s.pose(env, gs, out),
            }
        }
        if let Some((j, t)) = self.ground
            && let (Some(node), Some(valid)) = (out.pose.nodes.get_mut(j), out.valid_nodes.get_mut(j))
        {
            node.translation = t;
            *valid |= crate::anim_pose::VALID_T;
        }
    }
    forward_timeline!(no_exit);
    fn exit(&mut self, env: &Env, gs: &GameState) {
        self.exited = true;
        if let Some(s) = &mut self.sub {
            s.exit(env, gs);
        }
    }
    fn delta(&self, env: &Env, gs: &GameState) -> Delta {
        match &self.class {
            None => Delta::None,
            Some(c) if c == "TrajectoryOffsetControllerAsset" => {
                self.sub.as_ref().map_or(Delta::None, |s| s.delta(env, gs))
            }
            Some(_) if self.frozen_carving() => Delta::ZERO,
            Some(c) if env.physics.is_some_and(|p| p.controllers.contains(c)) => Delta::Physics,
            Some(_) => Delta::Untraced,
        }
    }
}

impl LoopingInst {
    pub fn new(sub: Sub, start: f32, loop_control: bool, loop_start: f32, loop_end: f32) -> Self {
        LoopingInst { sub, first: true, start, loop_control, loop_start, loop_end, partial: None }
    }
}

impl PhaseInst {
    pub fn new(sub: Sub, signal: Option<SigId>, range: [f32; 2]) -> Self {
        PhaseInst { sub, signal, range }
    }
}

impl RateInst {
    pub fn new(sub: Sub, signal: Option<SigId>, scale: f32, range: [f32; 2]) -> Self {
        RateInst { sub, signal, scale, range }
    }
}

impl RefPoseInst {
    pub fn new(
        env: &Env,
        gs: &GameState,
        sub: Sub,
        reference: Option<CtrlId>,
        frame: f32,
        weight: Option<SigId>,
        offset: bool,
    ) -> Self {
        let reference = reference.map(|c| {
            let mut r = instantiate(env, gs, c);
            r.set_time(env, gs, frame);
            r
        });
        RefPoseInst { sub, reference, weight, offset }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::anim_graph::{Graph, Signal, SignalKind};

    #[test]
    fn an_exited_aerial_writes_its_outputs_into_its_own_layer() {
        let mut g = Graph::default();
        let float = |name: &str| Signal { name: name.into(), kind: SignalKind::Float(0.0), trigger: false };
        g.signals = vec![float("FLT_FlipSpeed"), float("FLT_SpinWiggleAngle")];
        g.ctrls.push(("aerial".into(), Ctrl::Physics { class: "AerialControllerAsset".into(), subject: Some(1) }));
        g.ctrls.push((
            "rate".into(),
            Ctrl::PlaybackRate { subject: Some(2), signal: Some(0), scale: 1.0, range: [0.0, 10.0] },
        ));
        let seq = Ctrl::Sequence {
            anims: Vec::new(),
            length: 100.0,
            windows: Vec::new(),
            start_tick: 0.0,
            audio_tags: Vec::new(),
            game_tags: Vec::new(),
            drivers: Vec::new(),
            queries: Vec::new(),
            pins: Vec::new(),
        };
        g.ctrls.push(("seq".into(), seq));
        let rig = crate::anim_pose::PoseRig::new(&ssx_formats::crsf::SceneGraph { nodes: Vec::new() }, &[], &[]);
        let bind = crate::rig_anim::RigPose { nodes: Vec::new(), floats: Vec::new() };
        let clip_index = std::collections::HashMap::new();
        let physics = crate::anim_pose::StepPhysics {
            q: [0.0, 0.0, 0.0, 1.0],
            t: [0.0; 3],
            controllers: vec!["AerialControllerAsset".into()],
            aerial_layer: vec![("FLT_FlipSpeed".into(), 3.0), ("FLT_SpinWiggleAngle".into(), 0.5)],
            aerial_scripts: Vec::new(),
            ground: None,
        };
        let env = Env { g: &g, clips: &[], clip_index: &clip_index, bind: &bind, rig: &rig, physics: Some(&physics) };
        let mut gs = GameState::new(&g);
        gs.set(0, 1.0);
        let mut aerial = instantiate(&env, &gs, 0);
        aerial.update(&env, &mut gs, 2.0);
        assert_eq!(aerial.time(), 2.0, "before the exit the subject reads the entity's 1");
        aerial.exit(&env, &gs);
        aerial.update(&env, &mut gs, 2.0);
        assert_eq!(aerial.time(), 8.0, "after it the layer's 3");
        assert_eq!((gs.get(0), gs.get(1)), (1.0, 0.0), "the entity keeps its values");
        let env = Env { physics: None, ..env };
        aerial.update(&env, &mut gs, 1.0);
        assert_eq!(aerial.time(), 11.0);
    }

    #[test]
    fn an_exited_aerials_subject_reads_the_scripts_on_its_layer() {
        let mut g = Graph::default();
        let script = Signal {
            name: "FLTX_Rate".into(),
            kind: SignalKind::Script { script: "s".into(), ordinal: 0, float: true, function: "Rate".into() },
            trigger: false,
        };
        let float = Signal { name: "FLT_FlipMagnitude".into(), kind: SignalKind::Float(0.0), trigger: false };
        g.signals = vec![script, float];
        g.ctrls.push(("aerial".into(), Ctrl::Physics { class: "AerialControllerAsset".into(), subject: Some(1) }));
        g.ctrls.push((
            "rate".into(),
            Ctrl::PlaybackRate { subject: Some(2), signal: Some(0), scale: 1.0, range: [0.0, 10.0] },
        ));
        let seq = Ctrl::Sequence {
            anims: Vec::new(),
            length: 100.0,
            windows: Vec::new(),
            start_tick: 0.0,
            audio_tags: Vec::new(),
            game_tags: Vec::new(),
            drivers: Vec::new(),
            queries: Vec::new(),
            pins: Vec::new(),
        };
        g.ctrls.push(("seq".into(), seq));
        let rig = crate::anim_pose::PoseRig::new(&ssx_formats::crsf::SceneGraph { nodes: Vec::new() }, &[], &[]);
        let bind = crate::rig_anim::RigPose { nodes: Vec::new(), floats: Vec::new() };
        let clip_index = std::collections::HashMap::new();
        let physics = crate::anim_pose::StepPhysics {
            q: [0.0, 0.0, 0.0, 1.0],
            t: [0.0; 3],
            controllers: vec!["AerialControllerAsset".into()],
            aerial_layer: vec![("FLT_FlipMagnitude".into(), 0.25)],
            aerial_scripts: vec![("FLTX_Rate".into(), 4.0)],
            ground: None,
        };
        let env = Env { g: &g, clips: &[], clip_index: &clip_index, bind: &bind, rig: &rig, physics: Some(&physics) };
        let mut gs = GameState::new(&g);
        gs.set(0, 1.0);
        gs.set(1, 0.5);
        let mut aerial = instantiate(&env, &gs, 0);
        aerial.exit(&env, &gs);
        aerial.update(&env, &mut gs, 2.0);
        assert_eq!(aerial.time(), 8.0, "the subject reads the script value on the layer");
        assert_eq!((gs.get(0), gs.get(1)), (1.0, 0.5), "the entity keeps its own");
    }

    #[test]
    fn a_running_carving_writes_the_ground_joint_over_its_subject() {
        let mut g = Graph::default();
        g.ctrls.push(("carve".into(), Ctrl::Physics { class: "CarvingControllerAsset".into(), subject: Some(1) }));
        let seq = Ctrl::Sequence {
            anims: Vec::new(),
            length: 100.0,
            windows: Vec::new(),
            start_tick: 0.0,
            audio_tags: Vec::new(),
            game_tags: Vec::new(),
            drivers: Vec::new(),
            queries: Vec::new(),
            pins: Vec::new(),
        };
        g.ctrls.push(("seq".into(), seq));
        let node = |n: &str| ssx_formats::crsf::SceneNode {
            parent: None,
            child_count: 0,
            name: n.into(),
            unk_a: 0,
            flags: 0,
            attributes: None,
            unk_group_flag: 0,
            kind: ssx_formats::crsf::NodeKind::Group,
        };
        let sg = ssx_formats::crsf::SceneGraph { nodes: vec![node("AITrajectory"), node("Ground")] };
        let rig = crate::anim_pose::PoseRig::new(&sg, &[], &[]);
        let srt = crate::character::Srt { scale: [1.0; 3], rotation: [0.0, 0.0, 0.0, 1.0], translation: [0.0; 3] };
        let bind = crate::rig_anim::RigPose { nodes: vec![srt; 2], floats: Vec::new() };
        let clip_index = std::collections::HashMap::new();
        let step = |class: &str| crate::anim_pose::StepPhysics {
            q: [0.0, 0.0, 0.0, 1.0],
            t: [0.0; 3],
            controllers: vec![class.into()],
            aerial_layer: Vec::new(),
            aerial_scripts: Vec::new(),
            ground: Some((1, [0.1, -0.8, 0.3])),
        };
        let (carving, aerial) = (step("CarvingControllerAsset"), step("AerialControllerAsset"));
        let env = Env { g: &g, clips: &[], clip_index: &clip_index, bind: &bind, rig: &rig, physics: Some(&carving) };
        let mut gs = GameState::new(&g);
        let mut carve = instantiate(&env, &gs, 0);
        let ground = |c: &dyn Controller, env: &Env, gs: &GameState| {
            let mut out = AnimPose::empty(&bind);
            c.pose(env, gs, &mut out);
            (out.valid_nodes[1] & crate::anim_pose::VALID_T != 0).then_some(out.pose.nodes[1].translation)
        };
        carve.update(&env, &mut gs, 2.0);
        assert_eq!(ground(carve.as_ref(), &env, &gs), Some([0.1, -0.8, 0.3]));
        carve.update(&env, &mut gs, 0.0);
        assert_eq!(ground(carve.as_ref(), &env, &gs), None, "a zero update writes nothing");
        let env_air = Env { physics: Some(&aerial), ..env };
        carve.update(&env_air, &mut gs, 2.0);
        assert_eq!(ground(carve.as_ref(), &env_air, &gs), None, "the step simulated no carving");
        carve.update(&env, &mut gs, 2.0);
        carve.exit(&env, &gs);
        carve.update(&env, &mut gs, 2.0);
        assert_eq!(ground(carve.as_ref(), &env, &gs), None, "an exited carving updates with dt 0");
    }
}
