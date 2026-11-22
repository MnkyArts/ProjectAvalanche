use glam::Vec3;

pub mod buttons;
mod line;
pub mod net_run;
mod pilot;
mod unstick;
pub use line::CourseLine;
use pilot::Pilot;
pub use unstick::Unstick;

use crate::rewind::{RewindPad, RewindState, RewindStep};
use crate::rider::{Input, RiderState, TrickInput, World};
use crate::session::Session;

const JUMP_HOLD_S: f32 = 0.3;

#[derive(Clone, Copy, Debug, Default)]
pub struct CourseControls {
    pub input: Input,
    pub tricks: TrickInput,
    pub r1: bool,
    pub rewind: RewindPad,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Style {
    pub offset: f32,
    pub speed: Speed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Speed {
    Follow,
    Tuck,
    Coast,
    Brake,
}

impl Default for Style {
    fn default() -> Self {
        Style { offset: 0.0, speed: Speed::Follow }
    }
}

const OFFSETS: [f32; 5] = [0.0, -4.0, 4.0, -8.0, 8.0];
const SPEEDS: [Speed; 4] = [Speed::Follow, Speed::Coast, Speed::Brake, Speed::Tuck];

fn oxygen_r1(s: &RiderState, on: &mut bool) -> bool {
    let v = &s.survival;
    if !v.low_oxygen || v.oxygen_tank <= 0.0 || v.oxygen >= OXYGEN_OFF {
        *on = false;
    } else if v.oxygen < OXYGEN_ON {
        *on = true;
    }
    *on && !s.in_air()
}

const PILOT_HEIGHT_GAIN: f32 = 0.1;
const PILOT_SPEED_GAIN: f32 = 0.02;

const GRIND_OFF_M: f32 = 6.0;

const OXYGEN_ON: f32 = 0.35;
const OXYGEN_OFF: f32 = 0.95;

#[allow(clippy::too_many_arguments)]
fn policy(
    line: &CourseLine,
    cursor: &mut usize,
    oxygen: &mut bool,
    pilot: &mut Pilot,
    s: &RiderState,
    w: &World,
    style: Style,
    flick: f32,
    time: f32,
) -> (CourseControls, usize, f32, f32) {
    let mut c = CourseControls::default();
    let (k, along, off) = locate(line, cursor, s.traj);
    let speed = s.speed();
    let mps = speed * crate::TICKS_PER_SECOND;
    let lookahead = if s.state.is_wingsuit() { (mps * 0.8).clamp(15.0, 50.0) } else { (mps * 0.45).clamp(6.0, 25.0) };
    let (mut target, tk) = line.at(along + lookahead);
    if style.offset != 0.0 {
        let dir = (target - line.points[k]).with_y(0.0).normalize_or_zero();
        target += dir.cross(Vec3::Y) * style.offset;
    }
    let steer = steer_towards(target - s.traj, s.velocity, s.heading);
    let flying_here = (k..(tk + 1).min(line.points.len())).any(|i| line.wingsuit[i]);
    c.r1 = s.in_air() && (flying_here || (s.state.is_wingsuit() && line.wingsuit[k]));
    c.r1 |= pilot.hold(s, time) && s.in_air();
    if s.state.is_wingsuit() {
        let (here, _) = line.at(along);
        let dy = here.y - s.traj.y;
        let dv = (speed - line.speed[k]) * crate::KMH_PER_M_PER_TICK;
        let bias = match style.speed {
            Speed::Follow => 0.0,
            Speed::Tuck => 0.5,
            Speed::Coast => -0.25,
            Speed::Brake => -0.5,
        };
        let mut pitch = (-PILOT_HEIGHT_GAIN * dy - PILOT_SPEED_GAIN * dv + bias).clamp(-1.0, 1.0);
        if Pilot::flare(s, w) {
            pitch = -1.0;
        }
        (c.input.steer, c.input.tuck, c.input.brake) = (steer, pitch.max(0.0), (-pitch).max(0.0));
        return (c, k, along, off);
    }
    if s.in_air() {
        return (c, k, along, off);
    }
    c.r1 = oxygen_r1(s, oxygen);
    if s.stopped {
        c.input.tuck = flick;
        return (c, k, along, off);
    }
    c.input.steer = steer;
    let rec = line.speed[k];
    match style.speed {
        Speed::Follow if speed < rec * 1.05 || speed < 0.1 => c.input.tuck = 1.0,
        Speed::Follow if speed > rec * 1.4 + 0.1 => c.input.brake = 0.4,
        Speed::Follow | Speed::Coast => {}
        Speed::Tuck => c.input.tuck = 1.0,
        Speed::Brake => c.input.brake = 0.6,
    }
    c.input.jump = line.jumps.iter().any(|&j| j > along && j - along < mps * JUMP_HOLD_S);
    if s.grind.is_grinding() && (line.in_air[k] || off > GRIND_OFF_M) {
        c.input.jump = flick > 0.5;
    }
    (c, k, along, off)
}

fn locate(line: &CourseLine, cursor: &mut usize, p: Vec3) -> (usize, f32, f32) {
    let n = line.points.len();
    let lo = cursor.saturating_sub(10);
    let (mut i, mut t, mut d) = line.project(p, lo..(*cursor + 60).min(n)).unwrap_or((*cursor, 0.0, f32::MAX));
    if d > 30.0
        && let Some(x) = line.project(p, 0..n)
    {
        (i, t, d) = x;
    }
    *cursor = i;
    let k = if t < 0.5 { i } else { (i + 1).min(n - 1) };
    (k, line.along_at(i, t), d)
}

#[derive(Clone, Debug)]
pub struct CoursePlayer {
    pub line: CourseLine,
    cursor: usize,
    style: Style,
    style_in_flight: bool,
    planned_at: f32,
    style_until: f32,
    best: f32,
    anchor: f32,
    best_at: f32,
    reanchor: bool,
    oxygen: bool,
    pilot: Pilot,
    heli_jump_from: Option<f32>,
    off_since: Option<f32>,
    dead_since: Option<f32>,
    hold_left: Option<u32>,
    hold: u32,
    executions: u32,
    pub rewinds: u32,
    last_rewind: Option<(f32, f32)>,
    streak: i32,
    slow_since: Option<f32>,
    pub rollout_steps: u64,
    pub searches: u32,
    pub log: Vec<String>,
    pub trace: f32,
    traced: f32,
}

const OFF_LINE_M: f32 = 45.0;
const OFF_LINE_AIR_M: f32 = 90.0;
const OFF_LINE_S: f32 = 1.5;
const STALL_S: f32 = 8.0;
const STALL_M: f32 = 15.0;
const DEAD_S: f32 = 0.5;
const REWIND_BACK_S: f32 = 3.0;
const STREAK_S: f32 = 15.0;
const STREAK_M: f32 = 60.0;
const SLOW_KMH: f32 = 25.0;
const SLOW_OFF_M: f32 = 15.0;
const SLOW_S: f32 = 4.0;
const HELI_WAIT_KMH: f32 = 30.0;
const HELI_APPROACH_M: f32 = 50.0;
const PLAN_EVERY_S: f32 = 0.5;
const PLAN_HORIZON_S: f32 = 6.0;
const ROLLOUT_LOST_M: f32 = 60.0;
const ROLLOUT_STRAY_M: f32 = 15.0;
const ROLLOUT_STALL_M: f32 = 10.0;
const DAMAGE_PENALTY_M: f32 = 100.0;
const LOW_HEALTH: f32 = 0.25;
const STYLE_HOLD_S: f32 = 1.0;
const FLIGHT_STYLE_HOLD_S: f32 = 2.0;

#[derive(Clone, Copy, Debug, PartialEq)]
struct Rollout {
    survived: bool,
    along: f32,
    off: f32,
    max_off: f32,
    stalled: bool,
    damage: f32,
    health: f32,
}

impl Rollout {
    fn score(&self) -> f32 {
        let stray = (self.max_off - ROLLOUT_STRAY_M).max(0.0);
        let hurt = DAMAGE_PENALTY_M * self.damage;
        if self.survived { self.along - 0.5 * self.off - 2.0 * stray - hurt } else { self.along - 10_000.0 }
    }
}

impl CoursePlayer {
    pub fn new(line: CourseLine) -> Self {
        CoursePlayer {
            line,
            cursor: 0,
            style: Style::default(),
            style_in_flight: false,
            planned_at: f32::NEG_INFINITY,
            style_until: f32::NEG_INFINITY,
            best: 0.0,
            anchor: 0.0,
            best_at: 0.0,
            reanchor: false,
            oxygen: false,
            pilot: Pilot::default(),
            heli_jump_from: None,
            off_since: None,
            dead_since: None,
            hold_left: None,
            hold: 0,
            executions: 0,
            rewinds: 0,
            last_rewind: None,
            streak: 0,
            slow_since: None,
            rollout_steps: 0,
            searches: 0,
            log: Vec::new(),
            trace: 0.0,
            traced: f32::NEG_INFINITY,
        }
    }

    fn trace_line(&mut self, time: f32, k: usize, off: f32, s: &RiderState, c: &CourseControls, what: &str) {
        if self.trace <= 0.0 || time - self.traced < self.trace - 1e-3 {
            return;
        }
        self.traced = time;
        let l = &self.line;
        self.log.push(format!(
            "{time:.2}: {what} at {:.0} m along ({off:.1} m off, line y {:.1} rider {:.1}), {:.0} km/h (recorded {:.0}{}{}), \
             stick ({:+.2}, {:+.2}){}, r1 {}, style {:+.0} m {:?}",
            l.along[k],
            l.points[k].y,
            s.traj,
            s.speed() * crate::KMH_PER_M_PER_TICK,
            l.speed[k] * crate::KMH_PER_M_PER_TICK,
            if l.in_air[k] { ", air" } else { "" },
            if l.wingsuit[k] { ", wingsuit" } else { "" },
            c.input.steer,
            c.input.tuck - c.input.brake,
            if c.input.jump { ", A" } else { "" },
            c.r1,
            self.style.offset,
            self.style.speed
        ));
    }

    fn rollout(&mut self, s: &RiderState, x: &Session, w: &World, style: Style, hold_s: f32, time: f32) -> Rollout {
        let (mut s, mut x) = (s.clone(), x.clone());
        let (mut cursor, mut oxygen, mut pilot) = (self.cursor, self.oxygen, self.pilot);
        let damage0 = s.survival.total_damage_taken;
        let mut out =
            Rollout { survived: true, along: 0.0, off: 0.0, max_off: 0.0, stalled: false, damage: 0.0, health: 1.0 };
        let mut start = None;
        let steps = (PLAN_HORIZON_S * crate::SIM_HZ) as u32;
        let held = (hold_s * crate::SIM_HZ) as u32;
        for i in 0..steps {
            let flick = if i % 2 == 0 { 1.0 } else { 0.0 };
            let st = if i < held { style } else { Style::default() };
            let at = time + i as f32 / crate::SIM_HZ;
            let (c, _, along, off) = policy(&self.line, &mut cursor, &mut oxygen, &mut pilot, &s, w, st, flick, at);
            (out.along, out.off) = (along, off);
            start.get_or_insert(along);
            if !s.in_air() {
                out.max_off = out.max_off.max(off);
            }
            if !s.death.is_alive() || off > ROLLOUT_LOST_M || s.janked.janked {
                out.survived = false;
                break;
            }
            s.hold_standard_r1(c.r1);
            let e = x.execute(&mut s, w, &pad_for_intent(c.input), &c.tricks, 0.0, &RewindPad::default(), None);
            self.rollout_steps += 1;
            if e.rider_dead.is_some() {
                out.survived = false;
                break;
            }
            (out.damage, out.health) = (s.survival.total_damage_taken - damage0, s.survival.health);
            if x.event.track_finished {
                out.along += 1000.0;
                return out;
            }
        }
        let end = self.line.along.last().copied().unwrap_or(0.0);
        out.stalled =
            out.survived && out.along - start.unwrap_or(0.0) < ROLLOUT_STALL_M && out.along + ROLLOUT_STALL_M < end;
        out
    }

    fn plan(&mut self, s: &RiderState, x: &Session, w: &World, time: f32) {
        let flying = s.state.is_wingsuit();
        if time >= self.style_until || (self.style_in_flight && !flying) {
            (self.style, self.style_in_flight) = (Style::default(), false);
        }
        let airborne = s.in_air() && !flying;
        if time - self.planned_at < PLAN_EVERY_S || airborne || (!flying && self.style != Style::default()) {
            return;
        }
        self.planned_at = time;
        let hold = if flying { FLIGHT_STYLE_HOLD_S } else { STYLE_HOLD_S };
        let current = self.rollout(s, x, w, self.style, hold, time);
        let hurt = current.damage > 0.0 && current.health < LOW_HEALTH;
        if current.survived && !current.stalled && current.max_off <= ROLLOUT_STRAY_M && !hurt {
            return;
        }
        self.searches += 1;
        let mut best = (current.score(), self.style);
        for &speed in &SPEEDS {
            for &offset in &OFFSETS {
                let style = Style { offset, speed };
                if style == self.style {
                    continue;
                }
                let r = self.rollout(s, x, w, style, hold, time);
                if r.score() > best.0 {
                    best = (r.score(), style);
                }
            }
        }
        if best.1 != self.style {
            self.log.push(format!(
                "{time:.2}: lookahead{}: the {} {} ({:.0} m along, {:.0} m off at most), {:+.0} m {:?} scores {:.0}",
                if flying { " in flight" } else { "" },
                if self.style == Style::default() { "line's own" } else { "style ridden" },
                if !current.survived {
                    "is lost"
                } else if hurt {
                    "is hurt"
                } else if current.stalled {
                    "stalls"
                } else {
                    "strays"
                },
                current.along,
                current.max_off,
                best.1.offset,
                best.1.speed,
                best.0
            ));
            (self.style, self.style_until, self.style_in_flight) = (best.1, time + hold, flying);
        }
    }

    pub fn controls(
        &mut self,
        s: &RiderState,
        x: &Session,
        w: &World,
        time: f32,
        go_clock: f32,
        rewind_idle: bool,
    ) -> CourseControls {
        let c = self.intent(s, x, w, time, go_clock, rewind_idle);
        CourseControls { input: pad_for_intent(c.input), ..c }
    }

    fn intent(
        &mut self,
        s: &RiderState,
        x: &Session,
        w: &World,
        time: f32,
        go_clock: f32,
        rewind_idle: bool,
    ) -> CourseControls {
        let mut c = CourseControls::default();
        let flick = if ((time * crate::SIM_HZ).round() as i64) % 2 == 0 { 1.0 } else { 0.0 };
        if let Some(n) = self.hold_left {
            c.rewind = RewindPad { rewind_held: n > 0, takeover: n == 0, flip_accel: 0.0 };
            c.r1 = s.in_air() && (s.state.is_wingsuit() || s.wingsuit.signal);
            return c;
        }
        if let Some(l) = s.start.as_ref() {
            if l.idle() && go_clock >= 2.0 {
                (self.cursor, self.best, self.anchor, self.best_at, self.style) = (0, 0.0, 0.0, time, Style::default());
                c.input.tuck = flick;
            }
            return c;
        }
        let finished = x.event.track_finished || x.event_ended;
        let (_, along, off) = locate(&self.line, &mut self.cursor, s.traj);
        if std::mem::take(&mut self.reanchor) {
            (self.best, self.anchor, self.best_at) = (along, along, time);
        }
        self.best = self.best.max(along);
        if self.best >= self.anchor + STALL_M {
            (self.anchor, self.best_at) = (self.best, time);
        }
        self.dead_since = if s.death.is_alive() { None } else { self.dead_since.or(Some(time)) };
        let limit = if s.in_air() { OFF_LINE_AIR_M } else { OFF_LINE_M };
        self.off_since = if off > limit { self.off_since.or(Some(time)) } else { None };
        let end = self.line.along.last().copied().unwrap_or(0.0);
        let slow = !s.in_air() && off > SLOW_OFF_M && s.speed() * crate::KMH_PER_M_PER_TICK < SLOW_KMH;
        self.slow_since = if slow { self.slow_since.or(Some(time)) } else { None };
        let stalled = !s.in_air()
            && (time - self.best_at > STALL_S || self.slow_since.is_some_and(|t| time - t >= SLOW_S))
            && self.best + 5.0 < end;
        let reason = if s.janked.anim_complete {
            Some("janked")
        } else if self.dead_since.is_some_and(|t| time - t >= DEAD_S) {
            Some("dead")
        } else if self.off_since.is_some_and(|t| time - t >= OFF_LINE_S) {
            Some("off the line")
        } else if stalled {
            Some("no progress")
        } else {
            None
        };
        let count = &x.manager.count;
        let can_rewind = s.janked.janked || (count.enabled && (count.infinite || count.rewinds_left > 0));
        if let Some(why) = reason.filter(|_| !finished && rewind_idle && can_rewind) {
            self.streak = match self.last_rewind {
                Some((at, place)) if time - at < STREAK_S || (along - place).abs() < STREAK_M => {
                    (self.streak + 1).min(4)
                }
                _ => 0,
            };
            let back = (REWIND_BACK_S * 2f32.powi(self.streak)).min(15.0);
            self.hold = (30.0 * back + 12.0).round() as u32;
            (self.hold_left, self.executions, self.last_rewind) = (Some(self.hold), 0, Some((time, along)));
            self.slow_since = None;
            self.log.push(format!(
                "{time:.2}: rewind {} ({why}; {along:.0} m along the line of {end:.0}, {off:.0} m off it): {back:.0} s back",
                self.rewinds + 1
            ));
            c.rewind = RewindPad { rewind_held: true, takeover: false, flip_accel: 0.0 };
            return c;
        }
        if !finished {
            self.plan(s, x, w, time);
        }
        let mut cursor = self.cursor;
        let (mut c, k, _, off) =
            policy(&self.line, &mut cursor, &mut self.oxygen, &mut self.pilot, s, w, self.style, flick, time);
        let intercept = x.heli_jump.as_ref().map(|h| h.intercept).filter(|i| i.valid);
        if intercept.is_none() || s.in_air() {
            self.heli_jump_from = None;
        }
        let near_end = |h: &crate::session::heli_jump::HeliJump| {
            x.event.finish_ms.is_some() || (h.data.end_location.0 - s.traj).with_y(0.0).length() < HELI_APPROACH_M
        };
        let delayed = x.event.setup.delayed_finish;
        if let Some(h) = x.heli_jump.as_ref().filter(|h| delayed && near_end(h) && intercept.is_none())
            && !s.in_air()
            && !s.stopped
        {
            let d = (h.data.end_location.0 - s.traj).with_y(0.0);
            c.input.steer = steer_towards(d, s.velocity, s.heading);
            let slow = s.speed() * crate::KMH_PER_M_PER_TICK > HELI_WAIT_KMH;
            (c.input.tuck, c.input.brake, c.input.jump) = (0.0, if slow { 1.0 } else { 0.0 }, false);
        }
        if let Some(i) = intercept
            && !s.in_air()
            && !s.stopped
        {
            let d = (i.position - s.traj).with_y(0.0);
            c.input.steer = steer_towards(d, s.velocity, s.heading);
            (c.input.tuck, c.input.brake) = (1.0, 0.0);
            let from = *self.heli_jump_from.get_or_insert(time);
            c.input.jump = time - from < JUMP_HOLD_S;
        }
        let what = if s.state.is_wingsuit() {
            "flying"
        } else if s.in_air() {
            "in the air"
        } else {
            "riding"
        };
        self.trace_line(time, k, off, s, &c, what);
        c
    }

    pub fn report(&self, p: Vec3) -> CourseReport {
        let mut cursor = self.cursor;
        let (_, along, off) = locate(&self.line, &mut cursor, p);
        CourseReport {
            id: self.line.id,
            line_m: self.line.along.last().copied().unwrap_or(0.0),
            best_m: self.best.max(along),
            along_m: along,
            off_m: off,
            rewinds: self.rewinds,
            searches: self.searches,
        }
    }

    pub fn after(&mut self, r: &RewindStep, time: f32) {
        let Some(n) = self.hold_left.as_mut() else { return };
        *n = n.saturating_sub(1);
        self.executions += 1;
        if r.terminating {
            self.hold_left = None;
            self.rewinds += 1;
            (self.reanchor, self.off_since, self.dead_since, self.planned_at) = (true, None, None, f32::NEG_INFINITY);
            self.slow_since = None;
            (self.style, self.style_in_flight) = (Style::default(), false);
            self.log.push(format!("{time:.2}: hand-back after rewind {}", self.rewinds));
        } else if self.executions > self.hold + 400
            || (r.state == RewindState::Idle && self.executions > self.hold + 30)
        {
            self.hold_left = None;
            self.log.push(format!("{time:.2}: the rewind did not run"));
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CourseReport {
    pub id: u32,
    pub line_m: f32,
    pub best_m: f32,
    pub along_m: f32,
    pub off_m: f32,
    pub rewinds: u32,
    pub searches: u32,
}

// PORT(choice): the test players (the course player and its lookahead, `ride_sim`'s scripted player, the map test
// player, the game's and the UI's scripted rides) steer by intent and push `s/|s|·(0.25 + 0.75·|s|)`, 0 staying 0 —
// the inverse of the matchers' rescale `m = max(min(|s|, 1) − 0.25, 0) / 0.75` (`AnalogStickGestureMatch_Match`
// 0x82D94028, MinMagnitude 0.25, MaxMagnitude 1), exact to one f32 step; the console has no test players, a person
// pushes past the dead zone.
pub fn pad_for_intent(intent: Input) -> Input {
    let v = intent.stick();
    let len = v.length();
    if len <= 0.0 {
        return intent;
    }
    let m = crate::graph::STICK_MIN_MAGNITUDE;
    intent.with_stick(v / len * (m + (1.0 - m) * len.min(1.0)))
}

pub fn steer_towards(d: Vec3, v: Vec3, heading: Vec3) -> f32 {
    let d = d.with_y(0.0).normalize_or_zero();
    let f = if v.with_y(0.0).length() > 0.05 { v } else { heading }.with_y(0.0).normalize_or_zero();
    let side = d.dot(f.cross(Vec3::Y));
    (side.atan2(f.dot(d)) * 2.0).clamp(-1.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(points: &[[f32; 3]]) -> CourseLine {
        let points: Vec<Vec3> = points.iter().map(|p| Vec3::from_array(*p)).collect();
        let mut along = vec![0.0];
        for w in points.windows(2) {
            along.push(along.last().unwrap() + w[0].distance(w[1]));
        }
        let n = points.len();
        CourseLine {
            jumps: Vec::new(),
            id: 1,
            points,
            along,
            speed: vec![1.0; n],
            vy: vec![0.0; n],
            in_air: vec![false; n],
            wingsuit: vec![false; n],
        }
    }

    #[test]
    fn the_line_is_walked_by_distance() {
        let l = line(&[[0.0, 0.0, 0.0], [10.0, 0.0, 0.0], [20.0, 0.0, 0.0], [30.0, 0.0, 0.0]]);
        assert!(l.at(15.0).0.distance(Vec3::new(15.0, 0.0, 0.0)) < 1e-4);
        assert_eq!(l.at(99.0).0, Vec3::new(30.0, 0.0, 0.0));
        assert_eq!(l.at(-5.0).0, Vec3::ZERO);
        let (i, t, d) = l.project(Vec3::new(13.5, 0.0, 4.0), 0..4).unwrap();
        assert_eq!(i, 1);
        assert!((t - 0.35).abs() < 1e-5 && (d - 4.0).abs() < 1e-5);
        assert!((l.along_at(i, t) - 13.5).abs() < 1e-4);
        assert_eq!(l.nearest_in(Vec3::new(29.0, 0.0, 1.0), 0..4).map(|x| x.0), Some(3));
    }

    #[test]
    fn the_intent_comes_through_the_dead_zone() {
        let matched = |i: Input| crate::graph::stick_axes(i.stick());
        for (steer, tuck, brake) in
            [(0.2, 0.0, 0.0), (-0.05, 0.0, 0.0), (0.3, 0.4, 0.0), (0.0, 0.0, 0.6), (0.7, 0.0, 0.1)]
        {
            let intent = Input { steer, tuck, brake, jump: true, ..Input::default() };
            let pad = pad_for_intent(intent);
            assert!(pad.jump, "the buttons are kept");
            assert!(pad.stick().length() >= 0.25, "past the dead zone: {:?}", pad.stick());
            assert!((matched(pad) - intent.stick()).length() < 1e-6, "{:?} → {:?}", intent.stick(), matched(pad));
        }
        let centre = Input { jump: true, ..Input::default() };
        assert_eq!(pad_for_intent(centre).stick(), glam::Vec2::ZERO);
        for full in [glam::Vec2::X, glam::Vec2::NEG_Y, glam::Vec2::new(0.6, 0.8)] {
            let i = Input::default().with_stick(full);
            assert!((pad_for_intent(i).stick() - full).length() < 1e-6, "{full:?}");
        }
    }

    #[test]
    fn steering_turns_towards_the_target() {
        assert!(steer_towards(Vec3::new(10.0, 0.0, 10.0), Vec3::X, Vec3::X) > 0.5);
        assert!(steer_towards(Vec3::new(10.0, 0.0, -10.0), Vec3::X, Vec3::X) < -0.5);
        assert!(steer_towards(Vec3::X, Vec3::X, Vec3::X).abs() < 1e-6);
    }
}
