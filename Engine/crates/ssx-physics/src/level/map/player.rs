use glam::Vec3;

use crate::SIM_HZ;
use crate::harness::steer_towards;
use crate::rewind::RewindPad;
use crate::rider::{Input, RideState, RiderState};

pub const PUSH_S: f32 = 2.0;

const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x100_0000_01b3;

#[derive(Clone, Debug)]
pub struct MapPlayer {
    pub loop_t0: f32,
    pub ready_t0: Option<f32>,
    pub rode: bool,
    pub falling: bool,
    pub fall_at: Option<f32>,
    pub restarts: usize,
    pub fingerprint: u64,
    pub hashed: u64,
}

impl MapPlayer {
    pub fn new(fall_at: Option<f32>) -> MapPlayer {
        MapPlayer {
            loop_t0: 0.0,
            ready_t0: None,
            rode: false,
            falling: false,
            fall_at,
            restarts: 0,
            fingerprint: FNV_OFFSET,
            hashed: 0,
        }
    }

    pub fn controls(&mut self, i: u64, s: &RiderState, target: Option<Vec3>) -> (Input, RewindPad, bool) {
        let time = i as f32 / SIM_HZ;
        let riding = s.state == RideState::Riding && s.start.is_none();
        let turned =
            self.fall_at.is_some_and(|t| time >= t) && self.restarts == 0 && s.death.is_alive() && !self.falling;
        self.falling |= turned;
        let steer = match target {
            _ if self.falling => 1.0,
            Some(t) if riding => steer_towards(t - s.traj, s.velocity, s.heading),
            _ => 0.0,
        };
        let waiting = s.stopped || s.start.as_ref().is_some_and(|l| l.idle());
        let intro = s.start.as_ref().and_then(|l| l.intro.as_ref());
        self.rode |= s.start.is_none();
        if !self.rode && self.ready_t0.is_none() && intro.is_some_and(|i| i.in_ready_loop()) {
            self.ready_t0 = Some(time);
        }
        let t0 = if intro.is_some() { self.ready_t0 } else { Some(self.loop_t0) };
        let push = waiting && t0.is_some_and(|t0| (PUSH_S..PUSH_S + 1.0).contains(&(time - t0)));
        let tuck = if push && i % 2 == 1 { 0.0 } else { 1.0 };
        let input = Input { steer: if push { 0.0 } else { steer }, brake: 0.0, tuck, jump: false, ..Input::default() };
        let pad =
            RewindPad { rewind_held: false, takeover: false, flip_accel: input.flip_acceleration(s.tricks.classic) };
        (input, pad, turned)
    }

    pub fn hash(&mut self, s: &RiderState) {
        for v in [s.position, s.traj] {
            for c in v.to_array() {
                for b in c.to_bits().to_le_bytes() {
                    self.fingerprint = (self.fingerprint ^ u64::from(b)).wrapping_mul(FNV_PRIME);
                }
            }
        }
        self.hashed += 1;
    }

    pub fn new_run(&self) -> MapPlayer {
        MapPlayer { restarts: self.restarts + 1, ..MapPlayer::new(None) }
    }

    pub fn restarted(&mut self, i: u64) {
        (self.loop_t0, self.ready_t0, self.rode, self.falling) = (i as f32 / SIM_HZ + 1.0 / SIM_HZ, None, false, false);
        self.restarts += 1;
    }
}
