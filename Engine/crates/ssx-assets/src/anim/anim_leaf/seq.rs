use super::*;
use crate::anim_graph::GameTag;
use crate::tags::{BoolTag, FloatTag, TagEvent, TagTracker, TagWindow};

struct Elem {
    sub: Box<dyn Controller>,
    scale: f32,
    mirror: bool,
    blend: Option<(Blend, f32, Box<Elem>)>,
}

impl Elem {
    pub(super) fn update(&mut self, env: &Env, gs: &mut GameState, dt: f32) {
        if let Some((b, e, old)) = &mut self.blend {
            *e = (*e + dt).min(b.blend_time.max(0.0));
            old.update(env, gs, dt);
            if *e >= b.blend_time || blend_new_weight(b, *e) >= 0.99 {
                self.blend = None;
            }
        }
        self.sub.update(env, gs, dt * self.scale);
    }

    pub(super) fn delta(&self, env: &Env, gs: &GameState) -> Delta {
        let d = self.sub.delta(env, gs);
        let d = if self.mirror { d.mirror() } else { d };
        match &self.blend {
            Some((b, e, old)) => crate::anim_eval::blend_delta(env, b, *e, old.delta(env, gs), d),
            None => d,
        }
    }

    pub(super) fn exit(&mut self, env: &Env, gs: &GameState) {
        self.sub.exit(env, gs);
        if let Some((_, _, old)) = &mut self.blend {
            old.exit(env, gs);
        }
    }

    pub(super) fn pose(&self, env: &Env, gs: &GameState, out: &mut AnimPose) {
        let mut new = AnimPose::empty(env.bind);
        self.sub.pose(env, gs, &mut new);
        if self.mirror {
            new = env.rig.mirror(&new);
        }
        if let Some((b, e, old)) = &self.blend {
            let mut o = AnimPose::empty(env.bind);
            old.pose(env, gs, &mut o);
            crate::anim_eval::apply_blend(env, b, *e, &mut o, &new);
            new = o;
        }
        *out = new;
    }
}

pub struct SeqInst {
    ctrl: CtrlId,
    time: f32,
    length: f32,
    started: usize,
    current: Option<Elem>,
    delta: Option<Delta>,
    tags: crate::tags::TagTracker,
    game: TagTracker,
    game_saved: Vec<(bool, f32)>,
}

fn seq_anims<'a>(env: &Env<'a>, ctrl: CtrlId) -> &'a [crate::anim_graph::SeqAnim] {
    match &env.g.ctrls[ctrl].1 {
        Ctrl::Sequence { anims, .. } => anims,
        _ => &[],
    }
}

impl SeqInst {
    pub fn new(env: &Env, gs: &GameState, ctrl: CtrlId) -> Self {
        let (length, start) = match &env.g.ctrls[ctrl].1 {
            Ctrl::Sequence { length, start_tick, .. } => (*length, *start_tick),
            _ => (0.0, 0.0),
        };
        let windows = Self::audio_windows(env, ctrl);
        let game_windows = Self::game_windows(env, ctrl);
        let game_saved = Self::game_tags(env, ctrl).iter().map(|_| (false, 0.0)).collect();
        let mut s = SeqInst {
            ctrl,
            time: 0.0,
            length,
            started: 0,
            current: None,
            delta: None,
            tags: crate::tags::TagTracker::new(&windows),
            game: TagTracker::new(&game_windows),
            game_saved,
        };
        s.jump(env, gs, start);
        // TODO(guess): a start tick above 0 (`DefaultInitParam.StartTick`) moves the trackers there with the
        // collection's time set (0x8319E068: tags before it do not fire); its callers (the relays 0x831A3DB8 /
        // 0x831A4B00, called through vtables) were not traced to the actor's start — without it the first update
        // would fire every window in (−1, start + dt); settle: the caller of the tag set's time set in the actor's
        // init (ActorController 0x835354B0's family).
        if s.time > 0.0 {
            s.tags.set_time(s.time);
            s.game.set_time(s.time);
        }
        s
    }

    fn jump(&mut self, env: &Env, gs: &GameState, t: f32) {
        self.time = t.clamp(0.0, self.length.max(0.0));
        self.started = 0;
        self.current = None;
        self.start_due(env, gs, true);
    }

    fn game_tags<'a>(env: &Env<'a>, ctrl: CtrlId) -> &'a [GameTag] {
        match &env.g.ctrls[ctrl].1 {
            Ctrl::Sequence { game_tags, .. } => game_tags,
            _ => &[],
        }
    }

    fn game_windows(env: &Env, ctrl: CtrlId) -> Vec<TagWindow> {
        Self::game_tags(env, ctrl).iter().map(|t| t.window).collect()
    }

    fn audio_windows(env: &Env, ctrl: CtrlId) -> Vec<crate::tags::TagWindow> {
        match &env.g.ctrls[ctrl].1 {
            Ctrl::Sequence { audio_tags, .. } => {
                audio_tags.iter().map(|t| crate::tags::TagWindow::new(t.time, t.duration, false)).collect()
            }
            _ => Vec::new(),
        }
    }

    fn audio_entered(&mut self, env: &Env, gs: &mut GameState) {
        let tags = match &env.g.ctrls[self.ctrl].1 {
            Ctrl::Sequence { audio_tags, .. } => audio_tags,
            _ => return,
        };
        if tags.is_empty() || !self.tags.update(self.time) {
            return;
        }
        for (i, e) in self.tags.process() {
            if e == crate::tags::TagEvent::Enter
                && let Some(t) = tags.get(i)
            {
                gs.audio_tags.push(crate::anim_eval::AudioTagEvent {
                    name: t.name.clone(),
                    system: t.system,
                    event: t.event,
                });
            }
        }
    }

    fn game_event(&mut self, tags: &[GameTag], i: usize, e: TagEvent, gs: &GameState) {
        let Some(t) = tags.get(i) else { return };
        if *TAG_TRACE {
            eprintln!("game tag {} (signal {}) {e:?} at {:.2}", t.name, t.signal, self.time);
        }
        let saved = &mut self.game_saved[i];
        if t.boolean {
            let mut var = gs.bool(t.signal);
            let mut saved_bool = saved.0;
            let reset = t.reset_on_leave.then_some(t.default != 0.0);
            BoolTag { value: t.value != 0.0, mode: t.mode, reset }.on(e, &mut var, &mut saved_bool);
            saved.0 = saved_bool;
            gs.set(t.signal, f32::from(u8::from(var)));
        } else {
            let mut var = gs.get(t.signal);
            let mut saved_float = saved.1;
            let weight = match e {
                TagEvent::Apply(w) | TagEvent::Leave(w) => w,
                TagEvent::Enter => 1.0,
            };
            let spline = crate::tags::tag_spline(&t.spline, weight);
            let reset = t.reset_on_leave.then_some(t.default);
            FloatTag { value: t.value, mode: t.mode, reset, spline }.on(e, &mut var, &mut saved_float);
            saved.1 = saved_float;
            gs.set(t.signal, var);
        }
    }

    fn game_entered(&mut self, env: &Env, gs: &mut GameState) {
        let tags = Self::game_tags(env, self.ctrl);
        if tags.is_empty() || !self.game.update(self.time) {
            return;
        }
        for (i, e) in self.game.process() {
            self.game_event(tags, i, e, gs);
        }
    }

    fn game_exit(&mut self, env: &Env, gs: &GameState) {
        let tags = Self::game_tags(env, self.ctrl);
        for (i, e) in self.game.exit() {
            self.game_event(tags, i, e, gs);
        }
    }

    fn begin(a: &crate::anim_graph::SeqAnim) -> f32 {
        a.start_time + a.start_in_time
    }

    fn start_due(&mut self, env: &Env, gs: &GameState, cut: bool) {
        let anims = seq_anims(env, self.ctrl);
        while let Some(a) = anims.get(self.started) {
            if Self::begin(a) > self.time + 1e-4 {
                break;
            }
            self.started += 1;
            let Some(c) = a.controller else { continue };
            let mut sub = instantiate(env, gs, c);
            let scale = if a.scale != 0.0 { a.scale } else { 1.0 };
            sub.set_time(env, gs, (a.start_in_time + (self.time - Self::begin(a))) * scale);
            let blend = match (self.current.take(), a.blend) {
                (Some(old), Some(b)) if !cut => Some((env.g.blends[b].clone(), 0.0, Box::new(old))),
                _ => None,
            };
            self.current = Some(Elem { sub, scale, mirror: a.flags & 1 != 0, blend });
        }
    }
}

impl Controller for SeqInst {
    fn update(&mut self, env: &Env, gs: &mut GameState, dt: f32) {
        let mut dt = dt.clamp(0.0, (self.length - self.time).max(0.0));
        let anims = seq_anims(env, self.ctrl);
        let mut parts: Vec<Delta> = Vec::new();
        for _ in 0..64 {
            let next = anims.get(self.started).map(Self::begin);
            let step = next.map_or(dt, |b| (b - self.time).clamp(0.0, dt));
            if let Some(e) = &mut self.current {
                e.update(env, gs, step);
                parts.push(e.delta(env, gs));
            }
            self.time += step;
            dt -= step;
            let before = self.started;
            self.start_due(env, gs, false);
            if dt <= 1e-6 || (step <= 0.0 && self.started == before) {
                break;
            }
        }
        self.delta = (parts.len() > 1)
            .then(|| parts.into_iter().reduce(|a, b| Delta::accumulate(a, b, env.physics)).unwrap_or(Delta::None));
        self.audio_entered(env, gs);
        self.game_entered(env, gs);
    }
    fn delta(&self, env: &Env, gs: &GameState) -> Delta {
        self.delta.unwrap_or_else(|| self.current.as_ref().map_or(Delta::None, |e| e.delta(env, gs)))
    }
    fn pose(&self, env: &Env, gs: &GameState, out: &mut AnimPose) {
        if let Some(e) = &self.current {
            e.pose(env, gs, out);
        }
    }
    fn time(&self) -> f32 {
        self.time.min(self.length)
    }
    fn length(&self) -> f32 {
        self.length
    }
    // TODO(guess): that a sequence's phase jump sets its tag set's time (0x8319E068) rather than updating it at the new
    // time (which would fire the windows passed forwards, 0x8319E1D0); the time set's callers (0x831A3DB8 /
    // 0x831A4B00, vtable-called) were not traced to the jump; settle: the caller of the tag set's time set in the
    // sequence jump (state-flow transitions' `DestinationPhase`, 0x82B78B50's family).
    fn set_time(&mut self, env: &Env, gs: &GameState, t: f32) {
        self.jump(env, gs, t);
        self.tags.set_time(self.time);
        self.game.set_time(self.time);
    }
    fn windows(&self, env: &Env) -> Vec<Window> {
        match &env.g.ctrls[self.ctrl].1 {
            Ctrl::Sequence { windows, .. } => windows
                .iter()
                .map(|w| Window {
                    kind: w.kind.clone(),
                    filter: w.filter,
                    time: w.time,
                    length: w.length,
                    blend: w.blend,
                })
                .collect(),
            _ => Vec::new(),
        }
    }
    fn label(&self, env: &Env, weight: f32, out: &mut Vec<(String, f32)>) {
        if let Some(e) = &self.current {
            let w = e.blend.as_ref().map_or(1.0, |(b, el, _)| blend_new_weight(b, *el));
            e.sub.label(env, weight * w, out);
            if let Some((_, _, old)) = &e.blend {
                old.sub.label(env, weight * (1.0 - w), out);
            }
        }
    }
    fn path(&self, env: &Env, out: &mut Vec<(String, f32)>) {
        if let Some(e) = &self.current {
            e.sub.path(env, out);
        }
    }
    fn exit(&mut self, env: &Env, gs: &GameState) {
        if let Some(e) = &mut self.current {
            e.exit(env, gs);
        }
        self.game_exit(env, gs);
    }
}

static TAG_TRACE: std::sync::LazyLock<bool> = std::sync::LazyLock::new(|| std::env::var("SSX_TAG_TRACE").is_ok());
