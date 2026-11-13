use bevy::prelude::*;
use ssx_physics::SIM_HZ;
use ssx_physics::rewind::{RewindPad, RewindState, RewindStep};
use ssx_physics::rider::{Input, RideState, RiderState, TrickInput};

use super::Ride;

pub(super) fn avalanche_override() -> Option<(i32, f32)> {
    let v = std::env::var("SSX_RIDE_AVALANCHE").ok()?;
    let (set, k) = v.split_once(',')?;
    Some((set.trim().parse().ok()?, k.trim().parse().ok()?))
}

fn brake_window() -> Option<(f32, f32)> {
    let v = std::env::var("SSX_RIDE_BRAKE").ok()?;
    let (a, b) = v.split_once(',')?;
    Some((a.trim().parse().ok()?, b.trim().parse().ok()?))
}

fn intro_skip_at() -> Option<f32> {
    std::env::var("SSX_INTRO_SKIP").ok().and_then(|v| v.parse::<f32>().ok())
}

fn slalom_period() -> Option<f32> {
    std::env::var("SSX_RIDE_SLALOM").ok().and_then(|v| v.parse::<f32>().ok()).filter(|p| *p > 0.0)
}

pub(super) fn env_flag(name: &str) -> bool {
    std::env::var(name).is_ok_and(|v| v == "1" || v.eq_ignore_ascii_case("true"))
}

pub(super) fn back_script() -> Option<&'static [f32]> {
    static TIMES: std::sync::OnceLock<Option<Vec<f32>>> = std::sync::OnceLock::new();
    TIMES
        .get_or_init(|| {
            let v = std::env::var("SSX_RIDE_BACK").ok()?;
            Some(v.split(',').filter_map(|x| x.trim().parse().ok()).collect())
        })
        .as_deref()
}

fn grab_buttons_script() -> Option<&'static [char]> {
    static SEQ: std::sync::OnceLock<Option<Vec<char>>> = std::sync::OnceLock::new();
    SEQ.get_or_init(|| {
        let v = std::env::var("SSX_RIDE_GRAB_BUTTONS").ok()?;
        ssx_physics::harness::buttons::parse(&v).map_err(|e| warn!("SSX_RIDE_GRAB_BUTTONS {e}: not used")).ok()
    })
    .as_deref()
}

pub(super) fn press_script() -> Option<(bool, f32, f32)> {
    let v = std::env::var("SSX_RIDE_PRESS").ok()?;
    let mut parts = v.split(',');
    let nose = match parts.next()?.trim() {
        "nose" => true,
        "tail" => false,
        _ => return None,
    };
    let from = parts.next().and_then(|x| x.trim().parse().ok()).unwrap_or(0.0);
    let to = parts.next().and_then(|x| x.trim().parse().ok()).unwrap_or(f32::MAX);
    Some((nose, from, to))
}

pub(super) fn wingsuit_script() -> Option<(f32, f32)> {
    let v = std::env::var("SSX_RIDE_WINGSUIT").ok()?;
    let mut parts = v.split(',');
    let from = parts.next()?.trim().parse().ok()?;
    let hold = parts.next().and_then(|x| x.trim().parse().ok()).unwrap_or(8.0);
    Some((from, hold))
}

impl Ride {
    fn scripted_r1(&mut self) -> bool {
        let Some((from, hold)) = self.wingsuit_script else { return false };
        let (t, s) = (self.seconds(), &self.state);
        if s.state.is_wingsuit() {
            if self.ws_deployed.is_none() {
                info!("wingsuit: deployed at {t:.2} s ({:?})", s.state);
            }
            let d = *self.ws_deployed.get_or_insert(t);
            return t - d < hold;
        }
        if self.ws_deployed.is_some() || t < from || !s.in_air() || s.start.is_some() {
            return false;
        }
        let press = self.ws_pressed.is_none_or(|p| t - p > 1.5 / SIM_HZ);
        if press {
            if self.ws_pressed.is_none() {
                info!("wingsuit: R1 pressed from {t:.2} s in the air (BLN_WingsuitEnabled {})", s.wingsuit.enabled);
            }
            self.ws_pressed = Some(t);
        }
        press
    }

    pub(super) fn scripted(&self) -> bool {
        self.autoplay || self.fixed_input.is_some()
    }

    pub(super) fn scripted_controls(&mut self) {
        self.rewind_pad = RewindPad::default();
        self.l2_hold = false;
        self.autoplay_sat_out = false;
        if self.autoplay
            && let Some(m) = self.map.as_mut()
        {
            let target = m.target(&self.world);
            let (input, pad, turned) = m.player.controls(self.steps, &self.state, target);
            if turned {
                info!("Ride step {}: the test player turns off the slope's side (SSX_MAP_FALL_AT)", self.steps + 1);
            }
            (self.input, self.tricks, self.rewind_pad) = (input, TrickInput::default(), pad);
            (self.r2, self.l2, self.r1) = (0.0, false, false);
            return;
        }
        if self.autoplay {
            let intro = self.state.start.as_ref().and_then(|l| l.intro.as_ref());
            if intro.is_some_and(|i| i.cabin.is_some()) {
                self.autoplay_waited += 1;
                self.autoplay_sat_out = true;
                (self.input, self.tricks) = (Input::default(), TrickInput::default());
                (self.r2, self.l2, self.r1) = (0.0, false, false);
                return;
            }
            let idle = self.last_exec.as_ref().is_none_or(|e| e.rewind.state == RewindState::Idle);
            if let Some(p) = self.course.as_mut() {
                let time = (self.steps - self.autoplay_waited) as f32 / SIM_HZ;
                if self.state.start.is_none() {
                    self.course_start_at = time;
                }
                let go = time - self.course_start_at;
                let c = p.controls(&self.state, &self.session, &self.world, time, go, idle);
                (self.input, self.tricks, self.r1, self.r2, self.l2) = (c.input, c.tricks, c.r1, 0.0, false);
                self.rewind_pad = c.rewind;
                for line in &p.log[self.course_logged..] {
                    info!("Ride step {}: course: {line}", self.steps + 1);
                }
                self.course_logged = p.log.len();
                return;
            }
            (self.input, self.tricks) = autoplay_input(self);
            let time = (self.steps - self.autoplay_waited) as f32 / SIM_HZ;
            let step = self.steps + 1;
            self.rewind_pad = self.auto_rewind.pad(&self.state, idle, time, step);
            self.r2 = 0.0;
            (self.l2, self.r1) = (false, self.scripted_r1());
            return;
        }
        let Some(fixed) = self.fixed_input else { return };
        let in_air = self.state.state != RideState::Riding;
        let release = self.jump_release.iter().any(|&n| self.steps == u64::from(n));
        let mut input = Input { jump: jump_held(&self.jump_release, self.steps), ..fixed };
        if let Some(at) = intro_skip_at()
            && self.state.start.is_some()
            && (self.seconds() - at).abs() < 0.5 / ssx_physics::SIM_HZ
        {
            input.jump = true;
        }
        if let Some(period) = slalom_period()
            && ((self.seconds() / period) as i64) % 2 == 1
        {
            input.steer = -input.steer;
        }
        if let Some((from, to)) = brake_window()
            && (from..to).contains(&self.seconds())
        {
            (input.steer, input.brake, input.tuck) = (0.0, 1.0, 0.0);
        }
        let hold = std::env::var("SSX_INTRO_GO").ok().and_then(|v| v.parse::<f32>().ok()).is_some_and(|go| {
            self.seconds() < go
                && self
                    .state
                    .start
                    .as_ref()
                    .and_then(|l| l.intro.as_ref())
                    .is_some_and(|i| matches!(i.scene, ssx_physics::nis::Scene::Ready { .. }))
        });
        let idle = self.state.start.as_ref().is_some_and(|l| l.idle()) && !hold;
        let finished = self.session.event.track_finished;
        if idle || (self.state.stopped && self.state.state == RideState::Riding && !finished) {
            let push = self.steps.is_multiple_of(2);
            (input.steer, input.brake, input.tuck) = (0.0, 0.0, if push { 1.0 } else { 0.0 });
        }
        if let Some(a) = self.air_input.filter(|_| in_air || release) {
            (input.steer, input.tuck, input.brake) = (a.x, a.y.max(0.0), (-a.y).max(0.0));
            if self.classic {
                input.dpad = a.clamp(Vec2::splat(-1.0), Vec2::splat(1.0));
                input.dpad_held = input.dpad != Vec2::ZERO;
            }
        }
        let grab_ok = self.state.air.air_time > self.world.tuning.player.max_in_air_jump_ticks;
        let mut tricks = if in_air && self.classic {
            let [lb, rb, lt, rt] = self.shoulders.filter(|_| grab_ok).unwrap_or_default();
            TrickInput { lb, rb, lt, rt, x: self.tweak, tweak: self.tweak, ..TrickInput::default() }
        } else if in_air {
            TrickInput {
                right_stick: if grab_ok { self.grab.unwrap_or(Vec2::ZERO) } else { Vec2::ZERO },
                tweak: self.tweak,
                ..TrickInput::default()
            }
        } else {
            TrickInput { tweak: self.boost, ..TrickInput::default() }
        };
        if let Some((nose, from, to)) = self.press_script
            && !in_air
            && (from..to).contains(&self.seconds())
        {
            (tricks.x, tricks.b) = (nose, !nose);
        }
        self.r2 = if tricks.tweak { 1.0 } else { 0.0 };
        let grinding = self.state.grind.is_grinding();
        let now = self.seconds();
        match (grinding, self.grind_since) {
            (true, None) => self.grind_since = Some(now),
            (false, Some(_)) => (self.grind_since, self.grind_ended) = (None, Some(now)),
            _ => {}
        }
        let var = |name: &str| std::env::var(name).ok();
        let held = match var("SSX_RIDE_GRIND_HOLD").and_then(|v| v.parse::<f32>().ok()) {
            Some(hold) => {
                self.grind_since.is_none_or(|t| now - t < hold) && self.grind_ended.is_none_or(|t| now - t >= 1.0)
            }
            None => true,
        };
        if grinding {
            match var("SSX_RIDE_GRIND_TRICK").as_deref() {
                Some("x") => tricks.x = true,
                Some("b") => tricks.b = true,
                Some("y") => tricks.y = true,
                _ => {}
            }
            if let Some(x) = var("SSX_RIDE_GRIND_STEER").and_then(|v| v.parse::<f32>().ok()) {
                input.steer = x;
            }
        }
        self.l2 = self.grind_fixed && held;
        self.l2_hold = self.l2;
        self.r1 = self.scripted_r1();
        (self.input, self.tricks) = (input, tricks);
    }
}

fn jump_held(releases: &[u32], step: u64) -> bool {
    releases.iter().enumerate().any(|(i, &n)| {
        let n = u64::from(n);
        step < n && (i == 0 || step + LATER_JUMP_HOLD >= n)
    })
}

const LATER_JUMP_HOLD: u64 = 9;

fn autoplay_input(ride: &Ride) -> (Input, TrickInput) {
    const EVERY: u64 = 4 * SIM_HZ as u64;
    let i = ride.steps - ride.autoplay_waited;
    let (time, s) = (i as f32 / SIM_HZ, &ride.state);
    let jump = i + LATER_JUMP_HOLD >= EVERY && i % EVERY >= EVERY - LATER_JUMP_HOLD;
    let release = i >= EVERY && i.is_multiple_of(EVERY);
    let waiting = s.stopped || s.start.as_ref().is_some_and(|l| l.idle());
    let pushing = waiting && (2.0..3.0).contains(&time);
    let stuck = s.stopped && s.state == RideState::Riding && !ride.session.event.track_finished && time >= 3.0;
    let get_up = s.stopped && s.landing.bail_on_landing;
    let flick = if i.is_multiple_of(2) { 1.0 } else { 0.0 };
    let (steer, tuck) = ride.auto_rewind.react(time).map_or((0.0, 0.0), |side| (side, 1.0));
    let (steer, tuck) = if pushing || stuck || get_up { (0.0, flick) } else { (steer, tuck) };
    let mut input = Input { steer, brake: 0.0, tuck, jump, ..Input::default() };
    let mut tricks = TrickInput::default();
    if s.state != RideState::Riding || release {
        (input.steer, input.tuck, input.brake) = (1.0, 0.0, 0.0);
        if s.tricks.classic {
            input.dpad = Vec2::X;
            input.dpad_held = true;
        }
    }
    if s.state != RideState::Riding && s.air.air_time < 0.5 * 60.0 {
        match grab_buttons_script() {
            Some(seq) if !s.on_ground => ssx_physics::harness::buttons::press(
                &mut tricks,
                ssx_physics::harness::buttons::pressed(seq, s.air.air_time),
            ),
            Some(_) => {}
            None => {
                if s.air.air_time > ride.world.tuning.player.max_in_air_jump_ticks {
                    tricks.right_stick = Vec2::Y;
                }
            }
        }
    }
    (input, tricks)
}

#[derive(Clone, Copy, Debug, Default)]
pub(super) struct AutoRewind {
    hold_left: Option<u32>,
    hold: u32,
    executions: u32,
    last_at: Option<f32>,
    streak: i32,
    count: u32,
    react_until: f32,
    react_side: f32,
}

impl AutoRewind {
    const BACK_S: f32 = 3.0;
    const REACT_S: f32 = 1.5;

    fn pad(&mut self, s: &RiderState, idle: bool, time: f32, step: u64) -> RewindPad {
        if self.hold_left.is_none() && s.janked.anim_complete && idle {
            self.streak = match self.last_at {
                Some(at) if time - at < 10.0 => (self.streak + 1).min(8),
                _ => 0,
            };
            let back = (Self::BACK_S * 2f32.powi(self.streak)).min(15.0);
            let hold = (30.0 * back + 12.0).round() as u32;
            (self.hold_left, self.hold, self.executions, self.last_at) = (Some(hold), hold, 0, Some(time));
            info!(
                "Ride step {step}: autoplay holds rewind for {hold} executions ({back:.0} s back; the rider is janked)"
            );
        }
        match self.hold_left {
            Some(n) => RewindPad { rewind_held: n > 0, takeover: n == 0, flip_accel: 0.0 },
            None => RewindPad::default(),
        }
    }

    pub(super) fn after(&mut self, r: &RewindStep, time: f32) {
        let Some(n) = self.hold_left.as_mut() else { return };
        *n = n.saturating_sub(1);
        self.executions += 1;
        if r.terminating {
            self.hold_left = None;
            self.count += 1;
            self.react_side = if self.count % 2 == 1 { 1.0 } else { -1.0 };
            self.react_until = time + Self::REACT_S;
        } else if self.executions > self.hold + 400 {
            self.hold_left = None;
        }
    }

    fn react(&self, time: f32) -> Option<f32> {
        (time < self.react_until).then_some(self.react_side)
    }
}
