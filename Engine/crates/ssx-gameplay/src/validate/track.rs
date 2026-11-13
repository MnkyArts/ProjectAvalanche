use std::collections::{HashSet, VecDeque};
use std::sync::Arc;

use glam::Vec3;
use ssx_net_proto::{Mode, NetPose, PoseFlag, ProgressReport};
use ssx_rules::intro::START_FIRST;

use super::limits::*;
use super::{Counts, Finding, Measure};

// PORT(choice): 90 executions (3 s), port bounds — a report waits for the samples of its execution, which come within
// the round trip and its resends (measured under 1 s); tuning: the reports' largest lag behind their samples; the
// original has no validation.
const RECENT_EXECUTIONS: f64 = 90.0;
// PORT(choice): 64 reports (a run reports 9 splits, a trick round a combo every few seconds), a port bound; tuning: the
// most reports waiting at once in the network runs; the original has no validation.
const MAX_PENDING: usize = 64;
// PORT(choice): 4 096 intervals, a port bound (a sample's step each: about 4.5 minutes of samples before merging;
// merging keeps the allowance); the original has no validation.
const MAX_SLOWED: usize = 4096;
// PORT(choice): 300 executions (10 s): a longer gap is a stall, not a ride, and the step's envelope grows with it;
// tuning: the largest honest gap between samples in the network runs; the original has no validation.
const MAX_STEP_GAP: f64 = 300.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Arrival {
    pub elapsed: f64,
    pub window: f64,
    pub horizon: f64,
    pub pace: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Point {
    pub exec: f64,
    pub arrival: f64,
    pub window: f64,
    pub pos: Vec3,
    pub vel: Vec3,
    pub epoch: u8,
    pub rewind: bool,
    pub warped: bool,
}

impl Point {
    pub(super) fn of(pose: &NetPose, at: Arrival) -> Option<Self> {
        let b = pose.block();
        let pos = Vec3::from_array(b.trajectory_position);
        let vel = Vec3::from_array(b.velocity());
        if !pos.is_finite() || !vel.is_finite() {
            return None;
        }
        let f = pose.flags();
        Some(Self {
            exec: f64::from(pose.execution()),
            arrival: at.elapsed,
            window: at.window.max(0.0),
            pos,
            vel,
            epoch: f.warp_epoch(),
            rewind: f.contains(PoseFlag::Rewind),
            warped: [PoseFlag::Teleport, PoseFlag::Rewind, PoseFlag::Respawn].iter().any(|&x| f.contains(x)),
        })
    }
}

#[derive(Clone, Debug)]
pub(super) struct Pending {
    pub report: ProgressReport,
    pub point: Point,
}

#[derive(Clone, Debug)]
pub struct RiderValidation {
    pub(super) limits: Arc<Limits>,
    pub(super) mode: Mode,
    starts: Vec<Start>,
    pub(super) start: Start,
    pub(super) ready: Option<Arrival>,
    start_from: Option<Point>,
    second: Option<(Point, Arrival)>,
    pace: f64,
    active: bool,
    pub(super) over: bool,
    pub(super) last: Option<Point>,
    pub(super) recent: VecDeque<Point>,
    places: HashSet<(i32, i32, i32)>,
    pub(super) touch: Option<(f64, f64)>,
    pub(super) nearest_end: f32,
    pub(super) paused: bool,
    pub(super) slowed: VecDeque<(f64, f64, f64)>,
    pub(super) anchor: Option<(f64, f64, f64)>,
    pub(super) split: Option<u8>,
    pub(super) loops: u16,
    pub(super) gate: usize,
    pub(super) gate_nearest: f32,
    pub(super) points: (u32, f64),
    pub(super) pending: VecDeque<Pending>,
    pub counts: Counts,
}

impl RiderValidation {
    pub fn new(limits: Arc<Limits>, mode: Mode, character: Option<u32>) -> Self {
        let starts = limits.starts_of(character);
        let start = starts.first().copied().unwrap_or(limits.start);
        Self {
            limits,
            mode,
            starts,
            start,
            ready: None,
            start_from: None,
            second: None,
            pace: 0.0,
            active: false,
            over: false,
            last: None,
            recent: VecDeque::new(),
            places: HashSet::new(),
            touch: None,
            nearest_end: f32::INFINITY,
            paused: false,
            slowed: VecDeque::new(),
            anchor: None,
            split: None,
            loops: 0,
            gate: 0,
            gate_nearest: f32::INFINITY,
            points: (0, 0.0),
            pending: VecDeque::new(),
            counts: Counts::default(),
        }
    }

    pub fn limits(&self) -> &Limits {
        &self.limits
    }

    pub fn ready(&mut self, sample: &NetPose, second: &NetPose, at: Arrival) -> Vec<Finding> {
        let mut out = Vec::new();
        if self.ready.is_some() || self.over {
            return out;
        }
        self.ready = Some(at);
        self.active = true;
        let Some(mut p) = Point::of(sample, at) else {
            let e = f64::from(sample.execution());
            self.finding(&mut out, Measure::StartPlace, e, f64::INFINITY, 0.0, "`Ready`'s sample is not finite".into());
            return out;
        };
        let by_distance = |s: &&Start| (s.handover - p.exec).abs();
        if let Some(s) = self.starts.iter().min_by(|a, b| by_distance(a).total_cmp(&by_distance(b))) {
            self.start = *s;
        }
        let s = self.start;
        self.finding(
            &mut out,
            Measure::StartExecution,
            p.exec,
            (p.exec - s.handover).abs(),
            0.0,
            format!(
                "`Ready` at execution {:.0}: the rider's way of {} hands over at {:.0}",
                p.exec,
                self.limits.start_words().0,
                s.handover
            ),
        );
        let d = s.place_distance(p.pos);
        self.finding(
            &mut out,
            Measure::StartPlace,
            p.exec,
            f64::from(d),
            f64::from(s.margin),
            format!(
                "`Ready`'s place {:.2} is {d:.3} m from {} {:.2} (at most {:.3} m)",
                p.pos,
                self.limits.start_words().1,
                s.handover_place(),
                s.margin
            ),
        );
        if p.exec == s.handover {
            self.start_from = Some(p);
        }
        p.arrival = p.exec - at.horizon;
        p.window = 0.0;
        let first = p.exec;
        out.extend(self.take(p, at));
        match Point::of(second, at) {
            Some(mut q) if q.exec == first + 2.0 => {
                q.arrival = q.exec - at.horizon;
                q.window = 0.0;
                self.second = Some((q, at));
            }
            q => {
                let e = q.map_or(f64::from(second.execution()), |q| q.exec);
                self.finding(
                    &mut out,
                    Measure::StartExecution,
                    e,
                    q.map_or(f64::INFINITY, |q| (q.exec - first - 2.0).abs()),
                    0.0,
                    match q {
                        Some(_) => {
                            format!("`Ready`'s second sample at execution {e:.0}, not two after its first ({first:.0})")
                        }
                        None => format!("`Ready`'s second sample (execution {e:.0}) is not finite"),
                    },
                );
            }
        }
        out
    }

    fn take_second(&mut self, exec: Option<f64>, out: &mut Vec<Finding>) {
        if let Some((q, at)) = self.second.take_if(|(q, _)| exec.is_none_or(|e| q.exec <= e)) {
            out.extend(self.take(q, at));
        }
    }

    pub fn is_over(&self) -> bool {
        self.over
    }

    pub fn sample(&mut self, pose: &NetPose, at: Arrival) -> Vec<Finding> {
        let mut out = Vec::new();
        if self.over {
            return out;
        }
        let Some(p) = Point::of(pose, at) else { return out };
        if !self.active {
            return out;
        }
        self.take_second(Some(p.exec), &mut out);
        out.extend(self.take(p, at));
        out
    }

    fn take(&mut self, p: Point, at: Arrival) -> Vec<Finding> {
        let mut out = Vec::new();
        self.pace = at.pace;
        self.check_execution(&p, at, &mut out);
        if let Some(a) = self.last
            && p.exec <= a.exec
        {
            return out;
        }
        self.check_start_first(&p, &mut out);
        if !self.paused {
            self.check_speed(&p, &mut out);
            self.follow(&p, &mut out);
        } else {
            self.last = None;
        }
        self.recent.push_back(p);
        while self.recent.front().is_some_and(|f| f.exec < p.exec - RECENT_EXECUTIONS) {
            self.recent.pop_front();
        }
        if !self.paused {
            self.last = Some(p);
        }
        self.release(p.exec, &mut out);
        out
    }

    fn check_start_first(&mut self, p: &Point, out: &mut Vec<Finding>) {
        let Some(r) = self.start_from else { return };
        let k = p.exec - r.exec;
        if !(1.0..=START_FIRST as f64).contains(&k) || p.epoch != r.epoch || p.warped {
            return;
        }
        let s = self.start;
        let i = k as usize - 1;
        let off = (s.rotation.inverse() * (p.pos - r.pos)).distance(s.first[i]);
        self.finding(
            out,
            Measure::StartFirst,
            p.exec,
            f64::from(off),
            f64::from(s.first_margins[i]),
            format!(
                "execution {:.0}, {k:.0} after the hand-over: {off:.3} m off {}'s way from `Ready`'s place (at most \
                 {:.3} m)",
                p.exec,
                if self.limits.map { "the map's start" } else { "the intro" },
                s.first_margins[i]
            ),
        );
    }

    pub(super) fn check_execution(&mut self, p: &Point, at: Arrival, out: &mut Vec<Finding>) {
        let ahead = p.exec - at.elapsed;
        self.finding(
            out,
            Measure::Execution,
            p.exec,
            ahead,
            at.horizon,
            format!(
                "execution {:.0} came {:.2} ticks after the round's start: {ahead:.2} ahead of the server's time, at most \
                 {:.2}",
                p.exec, at.elapsed, at.horizon
            ),
        );
    }

    pub fn report(&mut self, report: &ProgressReport, at: Arrival) -> Vec<Finding> {
        let mut out = Vec::new();
        if self.over {
            return out;
        }
        let Some(point) = Point::of(&report.sample, at) else {
            let e = f64::from(report.sample.execution());
            self.finding(&mut out, Measure::Step, e, 1.0, 0.0, "a report's sample that is not finite".into());
            return out;
        };
        self.check_execution(&point, at, &mut out);
        self.pending.push_back(Pending { report: report.clone(), point });
        if self.pending.len() > MAX_PENDING
            && let Some(p) = self.pending.pop_front()
        {
            self.check_report(&p, &mut out);
        }
        let passed = self.recent.back().map_or(f64::NEG_INFINITY, |s| s.exec);
        self.release(passed, &mut out);
        out
    }

    pub fn end(&mut self) -> Vec<Finding> {
        let mut out = Vec::new();
        if !self.over {
            self.take_second(None, &mut out);
        }
        while let Some(p) = self.pending.pop_front() {
            self.check_report(&p, &mut out);
        }
        self.over = true;
        out
    }

    fn release(&mut self, exec: f64, out: &mut Vec<Finding>) {
        while self.pending.front().is_some_and(|p| p.point.exec <= exec) {
            if let Some(p) = self.pending.pop_front() {
                self.check_report(&p, out);
            }
        }
    }

    pub(super) fn finding(
        &mut self,
        out: &mut Vec<Finding>,
        m: Measure,
        execution: f64,
        value: f64,
        limit: f64,
        detail: String,
    ) {
        self.counts.made(m, value, limit);
        if value > limit || value.is_nan() {
            out.push(Finding { check: m.check(), execution, detail });
        }
    }

    pub(super) fn check_speed(&mut self, p: &Point, out: &mut Vec<Finding>) {
        let l = &self.limits;
        let (fall, horizontal) = (-p.vel.y, p.vel.with_y(0.0).length());
        let (lf, lh) = (l.fall, l.horizontal);
        self.finding(
            out,
            Measure::Fall,
            p.exec,
            f64::from(fall),
            f64::from(lf),
            format!("a fall of {:.3} m/tick ({:.1} km/h) at {:.1}, the limit {lf:.3}", fall, fall * 216.0, p.pos),
        );
        self.finding(
            out,
            Measure::Horizontal,
            p.exec,
            f64::from(horizontal),
            f64::from(lh),
            format!(
                "a horizontal speed of {horizontal:.3} m/tick ({:.1} km/h) at {:.1}, the limit {lh:.3}",
                horizontal * 216.0,
                p.pos
            ),
        );
    }

    fn follow(&mut self, p: &Point, out: &mut Vec<Finding>) {
        let Some(a) = self.last else {
            self.place(p);
            return;
        };
        let n = self.executions_between(&a, p);
        let warped = p.warped || a.rewind || p.epoch != a.epoch;
        if warped {
            self.check_landing(p, out);
            self.slow(a.exec, p.exec, n);
        } else if n <= MAX_STEP_GAP {
            self.check_step(&a, p, out);
            self.slow(a.exec, p.exec, slowed(&a, p, n));
            self.place_path(a.pos, p.pos);
        } else {
            self.slow(a.exec, p.exec, n);
        }
        if !p.warped {
            self.place(p);
        }
        let (from, since) = if warped { (p.pos, p.exec) } else { (a.pos, a.exec) };
        if !self.pass_gates(from, p.pos) {
            return;
        }
        if let Some(d) = self.limits.end_segment_distance(from, p.pos) {
            self.nearest_end = self.nearest_end.min(d);
            if d <= self.limits.touch_reach() {
                if self.touch.is_none() {
                    self.touch = Some((since, p.exec));
                }
                self.paused = true;
            }
        }
    }

    pub(super) fn pass_gates(&mut self, a: Vec3, b: Vec3) -> bool {
        while let Some(d) = self.limits.gate_segment_distance(self.gate, a, b) {
            self.gate_nearest = self.gate_nearest.min(d);
            if d > self.limits.touch_reach() {
                return false;
            }
            self.gate += 1;
            self.gate_nearest = f32::INFINITY;
        }
        true
    }

    pub(super) fn executions_between(&self, a: &Point, b: &Point) -> f64 {
        let n = (b.exec - a.exec).max(0.0);
        n.min((b.arrival - a.arrival + a.window + self.pace).max(0.0))
    }

    pub(super) fn check_step(&mut self, a: &Point, b: &Point, out: &mut Vec<Finding>) {
        let n = self.executions_between(a, b);
        for (m, slack, part) in [
            (Measure::Step, STEP_SLACK_M, Vec3::ONE),
            (Measure::StepHorizontal, STEP_SLACK_HORIZONTAL_M, Vec3::new(1.0, 0.0, 1.0)),
        ] {
            let (va, vb) = (a.vel * part, b.vel * part);
            let allowed = step_allowed(va, vb, n) + f64::from(slack) * (n / SAMPLE_INTERVAL_TICKS).max(1.0);
            let step = f64::from((a.pos * part).distance(b.pos * part));
            self.finding(
                out,
                m,
                b.exec,
                step,
                allowed,
                format!(
                    "a {} of {step:.2} m over {n:.0} executions from {:.1} to {:.1} (speeds {:.3} and {:.3} m/tick), at \
                     most {allowed:.2} m",
                    m.name(),
                    a.pos,
                    b.pos,
                    va.length(),
                    vb.length()
                ),
            );
        }
    }

    fn check_landing(&mut self, p: &Point, out: &mut Vec<Finding>) {
        let near_start = self.limits.starts.iter().map(|s| s.distance(p.pos)).fold(f32::INFINITY, f32::min);
        let near_ridden = self.ridden_distance(p.pos);
        let d = near_start.min(near_ridden);
        self.finding(
            out,
            Measure::Landing,
            p.exec,
            f64::from(d),
            f64::from(REACH_M),
            format!(
                "a warp to {:.1}, {near_ridden:.1} m from any place the run rode and {near_start:.1} m from the start",
                p.pos
            ),
        );
    }

    fn cell(p: Vec3) -> (i32, i32, i32) {
        let c = |x: f32| (x / GRID_CELL_M).floor().clamp(i32::MIN as f32, i32::MAX as f32) as i32;
        (c(p.x), c(p.y), c(p.z))
    }

    fn place(&mut self, p: &Point) {
        if self.places.len() < MAX_PLACES {
            self.places.insert(Self::cell(p.pos));
        }
    }

    fn place_path(&mut self, a: Vec3, b: Vec3) {
        let n = ((a.distance(b) / (GRID_CELL_M * 0.5)).ceil() as usize).min(64);
        for k in 1..n {
            if self.places.len() >= MAX_PLACES {
                return;
            }
            self.places.insert(Self::cell(a.lerp(b, k as f32 / n as f32)));
        }
    }

    fn ridden_distance(&self, p: Vec3) -> f32 {
        let (cx, cy, cz) = Self::cell(p);
        let r = (REACH_M / GRID_CELL_M).ceil() as i32 + 1;
        let mut best = f32::INFINITY;
        for x in -r..=r {
            for y in -r..=r {
                for z in -r..=r {
                    let k = (cx.saturating_add(x), cy.saturating_add(y), cz.saturating_add(z));
                    if self.places.contains(&k) {
                        let lo = Vec3::new(k.0 as f32, k.1 as f32, k.2 as f32) * GRID_CELL_M;
                        let q = p.clamp(lo, lo + Vec3::splat(GRID_CELL_M));
                        best = best.min(q.distance(p));
                    }
                }
            }
        }
        best
    }

    fn slow(&mut self, from: f64, to: f64, executions: f64) {
        if executions <= 0.0 {
            return;
        }
        if self.slowed.len() == MAX_SLOWED
            && let (Some(a), Some(b)) = (self.slowed.pop_front(), self.slowed.pop_front())
        {
            self.slowed.push_front((a.0, b.1, a.2 + b.2));
        }
        self.slowed.push_back((from, to, executions));
    }

    pub(super) fn slowed_in(&self, lo: f64, hi: f64) -> f64 {
        self.slowed.iter().filter(|s| s.1 > lo && s.0 < hi).map(|s| s.2).sum()
    }

    pub(super) fn sample_before(&self, exec: f64) -> Option<Point> {
        self.recent.iter().rev().find(|s| s.exec <= exec).copied()
    }

    pub(super) fn resume(&mut self) {
        self.paused = false;
        self.last = None;
    }
}

fn step_allowed(va: Vec3, vb: Vec3, n: f64) -> f64 {
    let ticks = f64::from(TICKS_PER_STEP);
    let (sa, sb) = (f64::from(va.length()), f64::from(vb.length()));
    let unseen = (n - 1.0).max(0.0);
    ticks * sb
        + ticks * unseen * sa.max(sb)
        + f64::from(STEP_VELOCITY_CHANGE) * ticks * unseen * f64::from(va.distance(vb))
}

fn slowed(a: &Point, b: &Point, n: f64) -> f64 {
    let travel = f64::from(TICKS_PER_STEP) * n * f64::from(a.vel.length().max(b.vel.length()));
    if travel < f64::from(DILATION_MIN_TRAVEL_M) {
        return n;
    }
    let f = (f64::from(a.pos.distance(b.pos)) / travel).clamp(0.0, 1.0);
    (1.0 - f) * n
}
