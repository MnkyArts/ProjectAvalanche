use std::collections::VecDeque;

use glam::{Quat, Vec3};
use ssx_assets::ghosts::GhostPose;

pub const S02_EXTRAPOLATION_S: f64 = 0.2;

// PORT(choice): two intervals (one lost sample), measured over 8 network bots at 50 ms ± 20 ms per hop and 2 % loss
// (round net3): the drawn root's mean error near / middle / far 0.92 / 1.90 / 3.05 m, against 0.94 / 1.98 / 3.07 m with
// one interval, 1.18 / 4.84 / 5.73 m with phase 2's fixed 200 ms and 0.91 / 1.90 / 3.04 m with a fixed 550 ms; tuning:
// the arrival gaps of real links over the internet (bursts of loss longer than one sample); the original has no live
// racers.
pub const CAP_INTERVALS: f64 = 2.0;

// PORT(choice): 64 (4 s at the near tier's 15 Hz, 22 s at the far tier's 2.86 Hz): the bracket needs the samples of one
// send interval plus the arrival delay; tuning: the arrival delays of phase 2's sessions on real links; the original
// has no live racers.
pub const MAX_SAMPLES: usize = 64;

// PORT(choice): the largest of the last 30 arrivals (2 s at 15 Hz) — a buffer deep enough for the jitter of the newest
// arrivals; tuning: the arrival jitter of real links over the internet; the original has no live racers.
const DELAY_WINDOW: usize = 30;

const TICK_HZ: f64 = 30.0;
const VELOCITY_TICKS_PER_TICK: f32 = 2.0;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LiveMode {
    #[default]
    Extrapolate,
    Interpolate,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Push {
    Accepted,
    Snapped,
    Stale,
    OlderEpoch,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SampleFlags {
    pub teleport: bool,
    pub respawn: bool,
    pub rewind: bool,
    pub epoch: u8,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Stamp {
    pub tick: f64,
    pub execution: u32,
}

#[derive(Clone, Copy, Debug)]
struct Sample {
    tick: f64,
    stamp: Stamp,
    pose: GhostPose,
    rewind: bool,
}

#[derive(Clone, Debug)]
pub struct LivePose {
    samples: VecDeque<Sample>,
    epoch: Option<u8>,
    delays: VecDeque<f64>,
    correction: Option<Correction>,
    pub mode: LiveMode,
    pub fixed_cap_s: Option<f64>,
    pub cap_intervals: f64,
}

#[derive(Clone, Copy, Debug)]
struct Correction {
    offset: Vec3,
    turn: Quat,
    since: f64,
    over: f64,
}

impl Default for LivePose {
    fn default() -> Self {
        Self {
            samples: VecDeque::new(),
            epoch: None,
            delays: VecDeque::new(),
            correction: None,
            mode: LiveMode::default(),
            fixed_cap_s: None,
            cap_intervals: CAP_INTERVALS,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct LiveView {
    pub pose: GhostPose,
    pub root: Vec3,
    pub rotation: [f32; 4],
    pub age_ticks: f64,
}

fn epoch_newer(a: u8, b: u8) -> bool {
    (1..=127).contains(&a.wrapping_sub(b))
}

impl LivePose {
    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    pub fn newest_tick(&self) -> Option<f64> {
        self.samples.back().map(|s| s.tick)
    }

    pub fn push(&mut self, at: Stamp, flags: SampleFlags, pose: GhostPose, now: f64) -> Push {
        let newer_epoch = match self.epoch {
            Some(e) if e == flags.epoch => false,
            Some(e) if !epoch_newer(flags.epoch, e) => return Push::OlderEpoch,
            _ => true,
        };
        let newest = self.samples.back().map(|s| s.stamp);
        if !newer_epoch
            && newest.is_some_and(|n| at.execution < n.execution || (at.execution == n.execution && at.tick <= n.tick))
        {
            return Push::Stale;
        }
        let tick = at.tick;
        let back = match (newest, self.newest_tick()) {
            (Some(n), Some(last)) if !newer_epoch && at.execution > n.execution => {
                (last + f64::from(at.execution - n.execution) - tick).max(0.0)
            }
            _ => 0.0,
        };
        let rewind_starts = flags.rewind && !self.samples.back().is_some_and(|s| s.rewind);
        let snap = newer_epoch || flags.teleport || flags.respawn || rewind_starts || self.samples.is_empty();
        if snap {
            self.samples.clear();
            self.correction = None;
        } else {
            let (before, before_q) = self.drawn(now);
            for s in &mut self.samples {
                s.tick -= back;
            }
            let interval = self.newest_tick().map_or(1.0, |newest| (tick - newest).clamp(1.0, TICK_HZ));
            self.samples.push_back(Sample { tick, stamp: at, pose, rewind: flags.rewind });
            let (after, after_q) = (self.root_at(now), self.rotation_at(now));
            let turn = (before_q * after_q.conjugate()).normalize();
            self.correction = Some(Correction { offset: before - after, turn, since: now, over: interval });
        }
        if snap {
            self.samples.push_back(Sample { tick, stamp: at, pose, rewind: flags.rewind });
        }
        self.epoch = Some(flags.epoch);
        while self.samples.len() > MAX_SAMPLES {
            self.samples.pop_front();
        }
        if self.delays.len() == DELAY_WINDOW {
            self.delays.pop_front();
        }
        self.delays.push_back((now - tick).max(0.0));
        if snap { Push::Snapped } else { Push::Accepted }
    }

    fn interval(&self) -> f64 {
        let n = self.samples.len();
        if n < 2 { 1.0 } else { (self.samples[n - 1].tick - self.samples[n - 2].tick).max(1.0) }
    }

    pub fn delay_ticks(&self) -> f64 {
        self.delays.iter().copied().fold(0.0, f64::max) + self.interval()
    }

    pub fn cap_ticks(&self) -> f64 {
        match self.fixed_cap_s {
            Some(s) => s * TICK_HZ,
            None => self.delays.iter().copied().fold(0.0, f64::max) + self.cap_intervals * self.interval(),
        }
    }

    fn capped_age(&self, newest: f64, now: f64) -> f64 {
        (now - newest).clamp(0.0, self.cap_ticks().max(0.0))
    }

    fn blend_at(&self, t: f64) -> Option<GhostPose> {
        let i = self.samples.partition_point(|s| s.tick < t);
        let n = self.samples.len();
        if n == 0 {
            return None;
        }
        if i == 0 {
            return Some(self.samples[0].pose);
        }
        if i == n {
            return Some(self.samples[n - 1].pose);
        }
        let (a, b) = (&self.samples[i - 1], &self.samples[i]);
        let u = ((t - a.tick) / (b.tick - a.tick)) as f32;
        Some(GhostPose::blend(&a.pose, &b.pose, u))
    }

    pub fn bracket_span(&self, now: f64) -> Option<f64> {
        let t = now - self.delay_ticks();
        let i = self.samples.partition_point(|s| s.tick < t);
        (i > 0 && i < self.samples.len()).then(|| self.samples[i].tick - self.samples[i - 1].tick)
    }

    fn root_at(&self, now: f64) -> Vec3 {
        let Some(newest) = self.samples.back() else { return Vec3::ZERO };
        let p = Vec3::from_array(newest.pose.trajectory_position);
        if newest.rewind {
            return p;
        }
        let age = self.capped_age(newest.tick, now);
        p + Vec3::from_array(newest.pose.velocity) * VELOCITY_TICKS_PER_TICK * age as f32
    }

    // PORT(choice): the turn rate of the newest two samples, measured over 8 network bots at 50 ms ± 20 ms per hop and
    // 2 % loss (round net3): the drawn rotation's mean error near / middle / far 13.4 / 24.2 / 53.4° against 24.0 /
    // 30.6 / 53.1° for the bracket's (phase 2's, one interval late) and 20.7 / 27.9 / 49.9° for the newest sample's not
    // turned; tuning: the trajectory rotation's own predictability at the samples' ages on recorded rides (a carve's
    // turn reverses within the far tier's interval); the game has no live racers.
    fn rotation_at(&self, now: f64) -> Quat {
        let n = self.samples.len();
        let Some(newest) = self.samples.back() else { return Quat::IDENTITY };
        let q = quat(newest.pose.trajectory_rotation);
        if newest.rewind || n < 2 {
            return q;
        }
        let before = &self.samples[n - 2];
        let span = newest.tick - before.tick;
        if span <= 0.0 {
            return q;
        }
        let mut step = q * quat(before.pose.trajectory_rotation).conjugate();
        if step.w < 0.0 {
            step = -step;
        }
        let (axis, angle) = step.normalize().to_axis_angle();
        let turned = angle * (self.capped_age(newest.tick, now) / span) as f32;
        if !turned.is_finite() || !axis.is_finite() {
            return q;
        }
        (Quat::from_axis_angle(axis, turned) * q).normalize()
    }

    fn drawn(&self, now: f64) -> (Vec3, Quat) {
        let (mut root, mut rotation) = (self.root_at(now), self.rotation_at(now));
        if let Some(c) = self.correction {
            let k = (1.0 - (now - c.since) / c.over).clamp(0.0, 1.0) as f32;
            root += c.offset * k;
            rotation = (Quat::IDENTITY.slerp(c.turn, k) * rotation).normalize();
        }
        (root, rotation)
    }

    pub fn extrapolated_root(&self, now: f64) -> Option<Vec3> {
        (!self.samples.is_empty()).then(|| self.root_at(now))
    }

    pub fn view(&self, now: f64) -> Option<LiveView> {
        let newest = self.samples.back()?;
        let late = now - self.delay_ticks();
        let pose = self.blend_at(late)?;
        let (root, rotation) = match self.mode {
            LiveMode::Interpolate => (Vec3::from_array(pose.trajectory_position), pose.trajectory_rotation),
            LiveMode::Extrapolate => {
                let (root, q) = self.drawn(now);
                (root, q.to_array())
            }
        };
        Some(LiveView { pose, root, rotation, age_ticks: (now - newest.tick).max(0.0) })
    }
}

fn quat(q: [f32; 4]) -> Quat {
    let q = Quat::from_array(q);
    if q.length_squared() > 0.0 && q.is_finite() { q.normalize() } else { Quat::IDENTITY }
}

#[derive(Clone, Debug)]
pub struct LiveRacer {
    pub id: u16,
    pub pose: LivePose,
    pub position: Vec3,
    pub in_air: bool,
}

#[derive(Clone, Debug, Default)]
pub struct LiveSlots {
    slots: [Option<u16>; crate::event::MAX_RACERS],
    chosen_at: Option<f64>,
    stale: bool,
    order: Option<Vec<u16>>,
}

// PORT(choice): the occupants re-chosen at most once a second (the ranking's neighbours in a round, else the nearest
// live racers): the HUD's race leaderboard reorders its rows with an animation (`MovingUp` / `MovingDown`) that a
// faster choice would restart; tuning: the leaderboard's rows watched over a window's round with 16 riders; the game's
// events have at most six rivals and no live riders.
const RECHOOSE_TICKS: f64 = TICK_HZ;

impl LiveSlots {
    pub fn occupants(&self) -> impl Iterator<Item = (usize, u16)> + '_ {
        self.slots.iter().enumerate().filter_map(|(k, s)| s.map(|id| (k, id)))
    }
}

impl super::BotRace {
    pub fn add_live(&mut self, id: u16) {
        if !self.live.iter().any(|r| r.id == id) {
            self.live.push(LiveRacer { id, pose: LivePose::default(), position: Vec3::ZERO, in_air: false });
        }
    }

    pub fn set_live_order(&mut self, order: Option<Vec<u16>>) {
        self.live_slots.order = order;
    }

    pub fn remove_live(&mut self, id: u16) {
        self.live.retain(|r| r.id != id);
        if self.live_slots.slots.contains(&Some(id)) {
            self.live_slots.stale = true;
        }
    }

    pub fn live_mut(&mut self, id: u16) -> Option<&mut LiveRacer> {
        self.live.iter_mut().find(|r| r.id == id)
    }

    pub fn update_live(&mut self, now: f64, player: Vec3) -> Vec<usize> {
        for r in &mut self.live {
            if let Some(v) = r.pose.view(now) {
                r.position = v.root;
                r.in_air = v.pose.flags & ssx_formats::ghost::FLAG_IN_AIR != 0;
            }
        }
        let slots = &mut self.live_slots;
        let due = slots.stale || slots.chosen_at.is_none_or(|at| now - at >= RECHOOSE_TICKS || now < at);
        if !due {
            return Vec::new();
        }
        slots.chosen_at = Some(now);
        slots.stale = false;
        let first = self.bots.len().min(crate::event::MAX_RACERS);
        let free = crate::event::MAX_RACERS - first;
        let shown = |id: u16| self.live.iter().any(|r| r.id == id && !r.pose.is_empty());
        let chosen: Vec<u16> = match &slots.order {
            Some(order) => order.iter().copied().filter(|&id| shown(id)).take(free).collect(),
            None => {
                let mut near: Vec<(f32, u16)> = self
                    .live
                    .iter()
                    .filter(|r| !r.pose.is_empty())
                    .map(|r| (r.position.distance_squared(player), r.id))
                    .collect();
                near.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
                near.iter().take(free).map(|&(_, id)| id).collect()
            }
        };
        let end = first + chosen.len();
        let mut next = [None; crate::event::MAX_RACERS];
        for (slot, kept) in slots.slots[first..end].iter().zip(&mut next[first..end]) {
            if let Some(id) = slot.filter(|id| chosen.contains(id)) {
                *kept = Some(id);
            }
        }
        for &id in &chosen {
            if next.contains(&Some(id)) {
                continue;
            }
            if let Some(k) = (first..end).find(|&k| next[k].is_none()) {
                next[k] = Some(id);
            }
        }
        let changed = (0..crate::event::MAX_RACERS).filter(|&k| next[k] != slots.slots[k]).collect();
        slots.slots = next;
        changed
    }
}

#[cfg(test)]
mod tests;
