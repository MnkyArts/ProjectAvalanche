use anyhow::Result;

use crate::curve::Curve;
use crate::rider::{RideState, RiderState, World};
use crate::scoring::{ScoringContext, ScoringInputs, number};
use crate::tricks::TrickType;
use crate::tuning::Bundle;

#[cfg(doc)]
use crate::scoring::ScoreState;

pub const REWIND_SNAPSHOTS: usize = 450;
const N: usize = REWIND_SNAPSHOTS;
const LIMIT_AFTER_OLDEST: usize = 1;
const SESSION_TICKS: f32 = 60.0;

fn prev(i: usize) -> usize {
    (i + N - 1) % N
}

fn next(i: usize) -> usize {
    (i + 1) % N
}

#[derive(Clone, Copy, Debug)]
pub struct RecordFlags {
    pub track_started: bool,
    pub track_finished: bool,
    pub nis_active: bool,
    pub trigger_reset: bool,
    pub warped: bool,
    pub delta_ticks: f32,
}

impl Default for RecordFlags {
    fn default() -> Self {
        Self {
            track_started: true,
            track_finished: false,
            nis_active: false,
            trigger_reset: false,
            warped: false,
            delta_ticks: crate::DT_TICKS,
        }
    }
}

#[derive(Clone, Debug)]
struct Snapshot {
    time: f32,
    valid: bool,
    state: RiderState,
    delta_ticks: f32,
}

#[derive(Clone, Debug)]
pub struct RewindBuffer {
    snaps: Vec<Option<Snapshot>>,
    cur: usize,
    oldest: usize,
    newest: usize,
    nb: usize,
    alpha: f32,
    time: f32,
    target_time: f32,
    speed: f32,
    active: bool,
    terminating: bool,
    dilation_on: bool,
    dilation: f32,
    grounded_since_start: bool,
    janked_latch: bool,
}

impl Default for RewindBuffer {
    fn default() -> Self {
        Self::new()
    }
}

impl RewindBuffer {
    pub fn new() -> Self {
        Self {
            snaps: vec![None; N],
            cur: 0,
            oldest: 0,
            newest: N - 1,
            nb: 0,
            alpha: 0.0,
            time: 0.0,
            target_time: 0.0,
            speed: 0.0,
            active: false,
            terminating: false,
            dilation_on: false,
            dilation: 1.0,
            grounded_since_start: false,
            janked_latch: false,
        }
    }

    pub fn len(&self) -> usize {
        (self.newest + 1 + N - self.oldest) % N
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn span_ticks(&self) -> f32 {
        match (self.is_empty(), self.snap(self.oldest), self.snap(self.newest)) {
            (false, Some(a), Some(b)) => b.time - a.time,
            _ => 0.0,
        }
    }

    pub fn time(&self) -> f32 {
        self.time
    }

    pub fn is_active(&self) -> bool {
        self.active
    }

    pub fn is_terminating(&self) -> bool {
        self.terminating
    }

    pub fn time_dilation(&self) -> Option<f32> {
        self.dilation_on.then_some(self.dilation)
    }

    pub fn playback(&self) -> Option<(&RiderState, &RiderState, f32)> {
        let (a, b) = (self.snap(self.cur)?, self.snap(self.nb)?);
        self.active.then_some((&a.state, &b.state, self.alpha))
    }

    pub fn playback_slots(&self) -> Option<(usize, usize, f32)> {
        (self.active && self.snap(self.cur).is_some()).then_some((self.cur, self.nb, self.alpha))
    }

    fn snap(&self, i: usize) -> Option<&Snapshot> {
        self.snaps[i].as_ref()
    }

    fn valid(&self, i: usize) -> bool {
        self.snap(i).is_some_and(|x| x.valid)
    }

    fn time_of(&self, i: usize) -> f32 {
        self.snap(i).map_or(f32::NAN, |x| x.time)
    }

    fn blend(&self, cur: usize, nb: usize) -> f32 {
        let den = self.time_of(nb) - self.time_of(cur);
        let a = (self.time - self.time_of(cur)) / den;
        if den == 0.0 || a.is_nan() { 0.0 } else { a.clamp(0.0, 1.0) }
    }

    pub fn race_proper_started(&self) -> bool {
        self.grounded_since_start
    }

    pub fn record_with(&mut self, s: &RiderState, f: &RecordFlags) -> Option<usize> {
        let in_air = s.bln_in_air();
        self.grounded_since_start = f.track_started && (self.grounded_since_start || !in_air);
        if self.active {
            self.janked_latch = false;
            return None;
        }
        if self.janked_latch {
            return None;
        }
        self.janked_latch |= s.janked.anim_complete;
        let valid = f.track_started && !f.track_finished && !f.nis_active && !f.trigger_reset && !f.warped;
        let w = self.cur;
        match &mut self.snaps[w] {
            Some(x) => {
                (x.time, x.valid, x.delta_ticks) = (self.time, valid, f.delta_ticks);
                x.state.clone_from(s);
            }
            slot => *slot = Some(Snapshot { time: self.time, valid, state: s.clone(), delta_ticks: f.delta_ticks }),
        }
        if let Some(x) = &mut self.snaps[w] {
            x.state.grind.sa = None;
        }
        self.newest = w;
        let n = next(w);
        if n == self.oldest {
            self.oldest = next(self.oldest);
        }
        if !self.grounded_since_start {
            self.oldest = n;
        }
        self.cur = n;
        Some(w)
    }

    #[cfg(test)]
    pub(crate) fn record(&mut self, s: &RiderState, dt: f32) {
        if !self.active && !self.janked_latch {
            self.time += dt;
        }
        self.grounded_since_start = true;
        self.record_with(s, &RecordFlags { delta_ticks: dt, ..RecordFlags::default() });
    }

    #[cfg(test)]
    pub(crate) fn rewind(&mut self, s: &mut RiderState, ticks: f32) -> Option<f32> {
        if self.is_empty() {
            return None;
        }
        let newest = self.snap(self.newest)?.time;
        let mut k = self.newest;
        while k != self.oldest && newest - self.time_of(k) < ticks {
            k = prev(k);
        }
        let snap = self.snap(k)?.clone();
        restore(s, &snap.state);
        terminate_playback(s);
        (self.newest, self.cur, self.time, self.janked_latch) = (k, next(k), snap.time, false);
        (self.active, self.terminating, self.speed) = (false, false, 0.0);
        Some(newest - snap.time)
    }
}

fn restore(s: &mut RiderState, snap: &RiderState) {
    let (score, jump_trigger, graph) = (s.score, s.jump.trigger, s.graph);
    let (sa, feature) = (s.grind.sa.take(), s.grind.feature);
    let (pad_r1, oxygen_pad) = (s.wingsuit.pad_r1, s.survival.oxygen_pad);
    let temperature_enabled = s.survival.temperature_enabled;
    s.clone_from(snap);
    (s.wingsuit.pad_r1, s.survival.oxygen_pad) = (pad_r1, oxygen_pad);
    s.survival.temperature_enabled = temperature_enabled;
    s.score = score;
    s.score.restore_from_snapshot(&snap.score);
    s.jump.trigger = jump_trigger;
    s.graph = graph;
    s.gameplay = None;
    s.grind.sa = sa;
    s.grind.feature = feature;
    let live = std::mem::take(&mut s.board_particle);
    s.board_particle.keep_feature(live);
    s.board_particle.rig_op_view();
}

fn terminate_playback(s: &mut RiderState) {
    s.tricks.trick_type = TrickType::None;
    s.tricks.gesture_len = -1;
    s.tricks.grind.group = -1;
    s.tricks.grind.abort();
    if !s.board_particle.enabled {
        s.landing.uber_crash = false;
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct RewindTuning {
    pub pre_time: f32,
    pub pre_curve: Curve,
    pub enter_time: f32,
    pub enter_curve: Curve,
    pub abort_time: f32,
    pub abort_curve: Curve,
    pub transition_a_time: f32,
    pub transition_a_curve: Curve,
    pub transition_b_time: f32,
    pub transition_b_curve: Curve,
    pub time_dilation: crate::session::dilation::TimeDilationTuning,
}

impl Default for RewindTuning {
    fn default() -> Self {
        let curve = |name: &str, points: &[[f32; 2]]| Curve { name: name.into(), points: points.to_vec() };
        let (one, ramp_end) = ([1.0, 1.0], [0.999_984_74, 1.0]);
        Self {
            pre_time: 0.25,
            pre_curve: curve("curve_rewind_pre_time_dilation", &[[0.0, 1.0], [1.0, 0.302_259_74]]),
            enter_time: 0.4,
            enter_curve: curve("curve_rewind_enter_time_dilation", &[[0.0, 0.203_389_7], ramp_end, one]),
            abort_time: 1.0,
            abort_curve: curve(
                "curve_rewind_abort_time_dilation",
                &[[0.0, 0.307_909_5], [0.849_655_1, 0.293_785_2], ramp_end, one],
            ),
            transition_a_time: 0.35,
            transition_a_curve: curve("curve_rewind_transitionA_time_dilation", &[[0.0, 1.0], [1.0, 0.0]]),
            transition_b_time: 0.3,
            transition_b_curve: curve("curve_rewind_transitionB_time_dilation", &[[0.0, 0.0], ramp_end, one]),
            time_dilation: Default::default(),
        }
    }
}

impl RewindTuning {
    pub fn load(bundle: &Bundle) -> Result<Self> {
        let a = bundle.view("Rewind Management SceneOp", "RewindManagementSceneOpAsset")?;
        Ok(Self {
            pre_time: number(&a, "RewindTimePreRewind")?,
            pre_curve: a.curve("RewindCurvePreRewindAsset")?,
            enter_time: number(&a, "RewindTimeEnterRewind")?,
            enter_curve: a.curve("RewindCurveEnterRewindAsset")?,
            abort_time: number(&a, "RewindTimeAbortRewind")?,
            abort_curve: a.curve("RewindCurveAbortRewindAsset")?,
            transition_a_time: number(&a, "RewindTimeTransitionARewind")?,
            transition_a_curve: a.curve("RewindCurveTransitionARewindAsset")?,
            transition_b_time: number(&a, "RewindTimeTransitionBRewind")?,
            transition_b_curve: a.curve("RewindCurveTransitionBRewindAsset")?,
            time_dilation: crate::session::dilation::TimeDilationTuning::load(bundle)?,
        })
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RewindState {
    PreRewind = 0,
    Enter = 1,
    Rewinding = 2,
    Decelerate = 3,
    Accelerate = 4,
    Replay = 5,
    Abort = 6,
    #[default]
    Idle = 7,
    WaitRelease = 8,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct RewindPad {
    pub rewind_held: bool,
    pub takeover: bool,
    pub flip_accel: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RewindStep {
    pub physics: bool,
    pub next_dilation: f32,
    pub active: bool,
    pub terminating: bool,
    pub speed: f32,
    pub stopped_at_start: bool,
    pub state: RewindState,
    pub delta_ticks: f32,
}

impl RewindStep {
    pub fn apply_to(&self, ctx: &mut ScoringContext) {
        (ctx.rewind_active, ctx.rewind_speed, ctx.rewind_terminating) = (self.active, self.speed, self.terminating);
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RewindCount {
    pub rewinds_left: i32,
    pub initial: i32,
    pub session_start: f32,
    pub enabled: bool,
    pub infinite: bool,
}

impl RewindCount {
    pub fn can_rewind(&self, t: f32, janked: bool) -> bool {
        (self.enabled || janked) && (self.infinite || self.rewinds_left > 0 || t - self.session_start < SESSION_TICKS)
    }

    pub fn consume(&mut self, target_time: f32) {
        if !self.infinite && (self.session_start == 0.0 || target_time - self.session_start > SESSION_TICKS) {
            self.rewinds_left -= 1;
            self.session_start = target_time;
        }
    }
}

fn playback_inputs(s: &RiderState, flip_accel: f32) -> ScoringInputs {
    let in_air = s.state != RideState::Riding;
    let o = s.air.outputs;
    ScoringInputs {
        in_air,
        wipeout: crate::rider::is_scoring_wipeout(s, in_air),
        spin_speed: if in_air { o.spin_speed } else { 0.0 },
        flip_speed: if in_air { o.flip_speed } else { 0.0 },
        flip_accel,
        goofy: s.goofy,
        trick_type: s.tricks.trick_type,
        tricking: s.tricks.is_tricking,
        trick_ending: s.tricks.ending(),
        trick_locked: s.tricks.trick_locked,
        tweaking: s.tricks.is_tweaking,
        is_grinding: s.grind.is_grinding(),
        is_grind_trick: s.tricks.is_grind_trick(),
        super_grind: s.grind.carving.super_grind,
        grind_spin_vel: s.grind.carving.spin_vel,
        grind_group: s.tricks.grind.group,
        is_board_press: s.press.is_board_press,
        press_value: s.grind.carving.press.value,
        lua_wipeout: s.landing.uber_crash || s.landing.tumbling,
        helicopter_interaction: s.heli.helicopter_interaction,
    }
}

fn curve_factor(c: &Curve, x: f32, factor: &mut f32, dilate: &mut bool) {
    *factor = c.eval(x).max(0.01);
    *dilate = true;
}

fn go(state: &mut RewindState, timer: &mut f32, to: RewindState) {
    (*state, *timer) = (to, 0.0);
}

#[derive(Clone, Debug, PartialEq)]
pub struct RewindManager {
    pub tuning: RewindTuning,
    pub state: RewindState,
    pub timer: f32,
    pub start_index: usize,
    pub count: RewindCount,
    pub count_initialised: bool,
    pub live_rule: Option<LiveRewindRule>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LiveRewindRule {
    Off,
    Limit(i32),
}

impl RewindManager {
    pub fn new(tuning: RewindTuning) -> Self {
        let count = RewindCount { rewinds_left: 0, initial: 0, session_start: 0.0, enabled: true, infinite: true };
        let (state, timer) = (RewindState::Idle, 0.0);
        Self { tuning, state, timer, start_index: 0, count, count_initialised: false, live_rule: None }
    }

    pub fn set_live_rule(&mut self, rule: LiveRewindRule) {
        self.live_rule = Some(rule);
        self.apply_live_rule();
    }

    // PORT(choice): a live session's rule (the server's `rewinds = off | N`) is written into the count of every node the
    // level builds, the level start's and each restart's; the console has no live session, its count's flags come from
    // the node's constructor and the objectives' writes alone.
    fn apply_live_rule(&mut self) {
        match self.live_rule {
            Some(LiveRewindRule::Off) => self.count.enabled = false,
            Some(LiveRewindRule::Limit(_)) => self.count.infinite = false,
            None => {}
        }
    }

    pub fn init_count(&mut self, w: &World, resolved: bool) {
        if self.count_initialised || !resolved {
            return;
        }
        // PORT(choice): a live session's `rewinds = N` (the server's rule) is the count every init gives; the console
        // has no live session, its count is always the resolved `MaxRewindCount`.
        let n = match self.live_rule {
            Some(LiveRewindRule::Limit(n)) => n,
            _ => w.props.max_rewind_count as i32,
        };
        (self.count.rewinds_left, self.count.initial, self.count_initialised) = (n, n, true);
    }

    pub fn restart(&mut self, buf: &mut RewindBuffer) {
        *buf = RewindBuffer::new();
        *self = Self { live_rule: self.live_rule, ..Self::new(self.tuning.clone()) };
        self.apply_live_rule();
    }

    pub fn step(
        &mut self,
        buf: &mut RewindBuffer,
        s: &mut RiderState,
        w: &World,
        pad: &RewindPad,
        ctx: &ScoringContext,
        dt: f32,
    ) -> RewindStep {
        use RewindState::*;
        self.init_count(w, s.props_resolved);
        let (btn, janked) = (pad.rewind_held, s.janked.anim_complete);
        self.timer += 2.0;
        let secs = self.timer * 0.016_666_668;
        let k = &self.tuning;
        let (mut step, mut factor, mut dilate) = (dt, 1.0f32, false);
        let mut stepping = matches!(self.state, Enter | Rewinding | Decelerate | Accelerate | Replay);
        let forward = matches!(self.state, Accelerate | Replay);
        let (mut terminating, mut stopped_at_start) = (false, false);
        let mut cur = buf.cur;
        match self.state {
            Idle => {
                if btn && buf.grounded_since_start {
                    self.start_index = buf.cur;
                    go(&mut self.state, &mut self.timer, PreRewind);
                }
            }
            PreRewind => {
                step = 2.0;
                if secs <= k.pre_time {
                    curve_factor(&k.pre_curve, secs / k.pre_time, &mut factor, &mut dilate);
                } else {
                    let n = prev(buf.cur);
                    if janked && (self.count.rewinds_left == 0 || !self.count.enabled) {
                        self.count.rewinds_left = 1;
                    }
                    if btn && self.count.can_rewind(buf.time_of(n), janked) {
                        if buf.valid(n) {
                            buf.target_time = buf.time_of(n);
                            go(&mut self.state, &mut self.timer, Enter);
                            self.count.consume(buf.target_time);
                            (cur, stepping) = (n, true);
                        }
                    } else {
                        curve_factor(&k.pre_curve, 1.0, &mut factor, &mut dilate);
                        go(&mut self.state, &mut self.timer, Abort);
                    }
                }
            }
            Enter => {
                if secs > k.enter_time {
                    go(&mut self.state, &mut self.timer, Rewinding);
                } else {
                    curve_factor(&k.enter_curve, secs / k.enter_time, &mut factor, &mut dilate);
                }
            }
            Rewinding => {
                if !btn || cur == (buf.oldest + LIMIT_AFTER_OLDEST) % N {
                    go(&mut self.state, &mut self.timer, Decelerate);
                }
            }
            Decelerate => {
                if secs > k.transition_a_time {
                    if !btn {
                        curve_factor(&k.transition_a_curve, 1.0, &mut factor, &mut dilate);
                        go(&mut self.state, &mut self.timer, Accelerate);
                    }
                } else {
                    curve_factor(&k.transition_a_curve, secs / k.transition_a_time, &mut factor, &mut dilate);
                }
            }
            Accelerate => {
                if secs > k.transition_b_time {
                    go(&mut self.state, &mut self.timer, Replay);
                } else {
                    step = k.transition_b_curve.eval(secs / k.transition_b_time) * dt;
                }
            }
            Replay => {
                if btn && self.count.can_rewind(buf.time_of(cur), janked) {
                    buf.time = buf.time_of(cur);
                    go(&mut self.state, &mut self.timer, Enter);
                    self.count.consume(buf.target_time);
                }
            }
            Abort => {
                step = 2.0;
                if secs > k.abort_time {
                    go(&mut self.state, &mut self.timer, WaitRelease);
                } else {
                    curve_factor(&k.abort_curve, secs / k.abort_time, &mut factor, &mut dilate);
                }
            }
            WaitRelease => {
                if !btn {
                    go(&mut self.state, &mut self.timer, Idle);
                }
            }
        }
        if stepping && forward {
            if cur == self.start_index || cur == buf.newest {
                (stepping, terminating, buf.alpha) = (false, true, 0.0);
                go(&mut self.state, &mut self.timer, Idle);
            } else {
                buf.time += step;
                if buf.time >= buf.time_of(next(cur)) {
                    cur = next(cur);
                }
                (buf.nb, buf.alpha) = (next(cur), buf.blend(cur, next(cur)));
            }
            if self.state == Replay && pad.takeover {
                (stepping, terminating) = (false, true);
                go(&mut self.state, &mut self.timer, Idle);
            }
        } else if stepping {
            (dilate, step, factor) = (false, -2.0 * factor, 1.0);
            if cur != buf.oldest {
                buf.time += step;
                while cur != buf.oldest && buf.valid(prev(cur)) && buf.time <= buf.time_of(prev(cur)) {
                    cur = prev(cur);
                }
                (buf.nb, buf.alpha) = (prev(cur), buf.blend(cur, prev(cur)));
            } else {
                buf.alpha = 0.0;
            }
            if cur == buf.oldest {
                (stopped_at_start, step) = (true, 0.0);
            }
        } else if !buf.janked_latch {
            buf.time += step;
        }
        (buf.dilation_on, buf.dilation, buf.cur) = (dilate, factor, cur);
        (buf.active, buf.terminating, buf.speed) = (stepping, terminating, step / dt);
        let delta_ticks = playback(buf, s, w, pad, ctx, dt);
        RewindStep {
            physics: !buf.active,
            next_dilation: if dilate { factor } else { 1.0 },
            active: buf.active,
            terminating,
            speed: buf.speed,
            stopped_at_start,
            state: self.state,
            delta_ticks,
        }
    }
}

fn playback(buf: &RewindBuffer, s: &mut RiderState, w: &World, pad: &RewindPad, ctx: &ScoringContext, dt: f32) -> f32 {
    let Some(snap) = buf.snap(buf.cur) else { return dt };
    if buf.active {
        restore(s, &snap.state);
        let mut c = *ctx;
        (c.rewind_active, c.rewind_speed, c.rewind_terminating) = (true, buf.speed, false);
        let inputs = playback_inputs(s, pad.flip_accel);
        s.score.update_with(&w.tuning.scoring, &w.props, &inputs, &c, snap.delta_ticks);
        snap.delta_ticks
    } else if buf.terminating {
        restore(s, &snap.state);
        terminate_playback(s);
        snap.delta_ticks
    } else {
        dt
    }
}

#[cfg(test)]
mod tests;
