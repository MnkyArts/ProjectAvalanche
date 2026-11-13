use super::*;

pub fn mirrored_state(gs: &GameState, list: &[SigId], env: &Env) -> GameState {
    let m = gs.clone();
    for &s in list {
        let v = gs.get(s);
        let b = matches!(env.g.signals[s].kind, crate::anim_graph::SignalKind::Bool(_));
        m.set(s, if b { f32::from(u8::from(v == 0.0)) } else { -v });
    }
    m
}

pub struct MirroredInst {
    sub: Box<dyn Controller>,
    list: Vec<SigId>,
}

impl Controller for MirroredInst {
    fn update(&mut self, env: &Env, gs: &mut GameState, dt: f32) {
        let mut m = mirrored_state(gs, &self.list, env);
        self.sub.update(env, &mut m, dt);
        gs.merge_from(&m, &self.list);
        gs.audio_tags.append(&mut m.audio_tags);
    }
    fn pose(&self, env: &Env, gs: &GameState, out: &mut AnimPose) {
        let m = mirrored_state(gs, &self.list, env);
        let mut p = AnimPose::empty(env.bind);
        self.sub.pose(env, &m, &mut p);
        *out = env.rig.mirror(&p);
    }
    fn time(&self) -> f32 {
        self.sub.time()
    }
    fn length(&self) -> f32 {
        self.sub.length()
    }
    fn looping(&self) -> bool {
        self.sub.looping()
    }
    fn set_time(&mut self, env: &Env, gs: &GameState, t: f32) {
        let m = mirrored_state(gs, &self.list, env);
        self.sub.set_time(env, &m, t);
    }
    fn windows(&self, env: &Env) -> Vec<Window> {
        self.sub.windows(env)
    }
    fn label(&self, env: &Env, weight: f32, out: &mut Vec<(String, f32)>) {
        let mut l = Vec::new();
        self.sub.label(env, weight, &mut l);
        out.extend(l.into_iter().map(|(n, w)| (format!("{n} (mirrored)"), w)));
    }
    fn path(&self, env: &Env, out: &mut Vec<(String, f32)>) {
        self.sub.path(env, out);
    }
    fn exit(&mut self, env: &Env, gs: &GameState) {
        let m = mirrored_state(gs, &self.list, env);
        self.sub.exit(env, &m);
        gs.merge_from(&m, &self.list);
    }
    fn delta(&self, env: &Env, gs: &GameState) -> Delta {
        self.sub.delta(env, &mirrored_state(gs, &self.list, env)).mirror()
    }
}

pub struct AutoMirrorInst {
    ctrl: CtrlId,
    chain: Vec<CtrlId>,
    mirrored: bool,
    sub: Option<Box<dyn Controller>>,
    old: Option<(Box<dyn Controller>, f32)>,
    slave: bool,
}

struct MirrorData<'a> {
    signal: Option<SigId>,
    list: &'a [SigId],
    duration: f32,
    freeze: bool,
    continuous: bool,
}

fn mirror_data<'a>(env: &Env<'a>, ctrl: CtrlId) -> Option<MirrorData<'a>> {
    match &env.g.ctrls[ctrl].1 {
        Ctrl::AutoMirror { signal, mirror_list, duration, freeze, continuous, .. } => Some(MirrorData {
            signal: *signal,
            list: mirror_list,
            duration: *duration,
            freeze: *freeze,
            continuous: *continuous,
        }),
        _ => None,
    }
}

impl AutoMirrorInst {
    pub fn new(env: &Env, gs: &GameState, ctrl: CtrlId, chain: Vec<CtrlId>) -> Self {
        let d = mirror_data(env, ctrl).expect("auto mirror");
        let mirrored = d.signal.is_some_and(|s| gs.bool(s));
        let mut a = AutoMirrorInst { ctrl, chain, mirrored, sub: None, old: None, slave: false };
        a.sub = a.make(env, gs, mirrored);
        a
    }

    fn make(&self, env: &Env, gs: &GameState, mirrored: bool) -> Option<Box<dyn Controller>> {
        if self.chain.is_empty() {
            return None;
        }
        let d = mirror_data(env, self.ctrl)?;
        if mirrored {
            let m = mirrored_state(gs, d.list, env);
            let sub = crate::anim_eval::instantiate_chain(env, &m, &self.chain);
            gs.merge_from(&m, d.list);
            Some(Box::new(MirroredInst { sub, list: d.list.to_vec() }))
        } else {
            Some(crate::anim_eval::instantiate_chain(env, gs, &self.chain))
        }
    }
}

impl Controller for AutoMirrorInst {
    fn update(&mut self, env: &Env, gs: &mut GameState, dt: f32) {
        let Some(d) = mirror_data(env, self.ctrl) else { return };
        let want = d.signal.is_some_and(|s| gs.bool(s));
        if d.continuous && !self.slave && want != self.mirrored {
            let old_phase = self.sub.as_ref().map_or(0.0, |s| phase(s.as_ref()));
            if let Some(o) = &mut self.sub {
                o.exit(env, gs);
            }
            let mut new = self.make(env, gs, want);
            if let Some(n) = &mut new {
                set_phase(env, gs, n.as_mut(), old_phase);
            }
            if let Some(old) = std::mem::replace(&mut self.sub, new) {
                self.old = Some((old, 0.0));
            }
            self.mirrored = want;
        }
        if let Some((old, e)) = &mut self.old {
            *e += dt;
            if !d.freeze {
                old.update(env, gs, dt);
            }
            if d.duration - *e <= 0.001 {
                self.old = None;
            }
        }
        if let Some(s) = &mut self.sub {
            s.update(env, gs, dt);
        }
    }
    fn pose(&self, env: &Env, gs: &GameState, out: &mut AnimPose) {
        let mut new = AnimPose::empty(env.bind);
        if let Some(s) = &self.sub {
            s.pose(env, gs, &mut new);
        }
        if let (Some((old, e)), Some(d)) = (&self.old, mirror_data(env, self.ctrl)) {
            let mut o = AnimPose::empty(env.bind);
            old.pose(env, gs, &mut o);
            let t = if d.duration > 0.0 { (e / d.duration).clamp(0.0, 1.0) } else { 1.0 };
            o.blend(&new, t);
            new = o;
        }
        *out = new;
    }
    fn time(&self) -> f32 {
        self.sub.as_ref().map_or(0.0, |s| s.time())
    }
    fn length(&self) -> f32 {
        self.sub.as_ref().map_or(0.0, |s| s.length())
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
    fn path(&self, env: &Env, out: &mut Vec<(String, f32)>) {
        if let Some(s) = &self.sub {
            s.path(env, out);
        }
    }
    fn delta(&self, env: &Env, gs: &GameState) -> Delta {
        self.sub.as_ref().map_or(Delta::None, |s| s.delta(env, gs))
    }
    fn exit(&mut self, env: &Env, gs: &GameState) {
        self.slave = true;
        if let Some(s) = &mut self.sub {
            s.exit(env, gs);
        }
        if let Some((o, _)) = &mut self.old {
            o.exit(env, gs);
        }
    }
}

impl MirroredInst {
    pub fn new(sub: Box<dyn Controller>, list: Vec<SigId>) -> Self {
        MirroredInst { sub, list }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::anim_graph::{AudioTag, Graph};

    #[test]
    fn a_mirrored_sequences_audio_tags_reach_the_outer_state() {
        let mut g = Graph::default();
        g.ctrls.push(("mirror".into(), Ctrl::Mirror { subject: Some(1), phase_offset: 0.0 }));
        let tag =
            AudioTag { name: "trigger_F_Board_Release".into(), time: 3.0, duration: 0.0, system: 4139, event: 17014 };
        let seq = Ctrl::Sequence {
            anims: Vec::new(),
            length: 10.0,
            windows: Vec::new(),
            start_tick: 0.0,
            audio_tags: vec![tag],
            game_tags: Vec::new(),
        };
        g.ctrls.push(("seq".into(), seq));
        let rig = crate::anim_pose::PoseRig::new(&ssx_formats::crsf::SceneGraph { nodes: Vec::new() }, &[], &[]);
        let bind = crate::rig_anim::RigPose { nodes: Vec::new(), floats: Vec::new() };
        let clip_index = std::collections::HashMap::new();
        let env = Env { g: &g, clips: &[], clip_index: &clip_index, bind: &bind, rig: &rig, physics: None };
        let mut gs = GameState::new(&g);
        let mut mirror = instantiate(&env, &gs, 0);
        mirror.update(&env, &mut gs, 2.0);
        assert!(gs.audio_tags.is_empty(), "tick 2: before the tag");
        mirror.update(&env, &mut gs, 2.0);
        let names: Vec<&str> = gs.audio_tags.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, ["trigger_F_Board_Release"], "tick 4: the window (2, 4] holds the tag");
    }
}
