use ssx_net_proto::{NetPose, PoseFlag};

use crate::tiers::SampleOrder;

// PORT(choice): 20 km; the largest over the disc's 727 ghosts is 3 338 m and over eight recorded rides of every kind 3
// 686 m (examples/pose_bounds.rs in ssx-net, 2026-10-23); tuning: the extents of the maps a server may run; the
// original has no validation.
pub const MAX_COORDINATE_M: f32 = 20_000.0;

// PORT(choice): 0.001; every one of the disc's 329 230 blocks and the rides' 33 906 has length 1.000000 (the packer
// stores the pose's unit quaternion), enough unless another client packs differently; the original has no validation.
pub const ROTATION_LENGTH_TOLERANCE: f32 = 0.001;

// PORT(choice): 8 m per tick (480 m/s); the disc's largest stored speed is 2.71 and root step 2.49 m per tick, the
// rides' 1.34 (pose_bounds, 2026-10-23); tuning: phase 3's speed envelope from the rider tuning's top speeds; the
// original has no validation.
pub const MAX_SPEED_PER_TICK: f32 = 8.0;

const TICKS_PER_STEP: f32 = 2.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TickWindow {
    pub behind: u32,
    pub ahead: u32,
    pub view_behind: u32,
    pub view_ahead: u32,
}

impl TickWindow {
    pub fn both(ticks: u32) -> Self {
        Self { behind: ticks, ahead: ticks, view_behind: ticks, view_ahead: ticks }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Newest {
    order: SampleOrder,
    at: [f32; 3],
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct RiderTrack {
    newest: Option<Newest>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    Relay,
    Stale,
    OlderEpoch,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Violation {
    Tick(i64),
    ViewLead(f64),
    Coordinate(f32),
    RotationLength(f32),
    Speed(f32),
    Step(f32),
}

impl core::fmt::Display for Violation {
    fn fmt(&self, f: &mut core::fmt::Formatter) -> core::fmt::Result {
        match self {
            Self::Tick(d) => write!(f, "a sample {d} ticks from the server's"),
            Self::ViewLead(d) => write!(f, "a sample's view time {d} ticks from its time stamp"),
            Self::Coordinate(c) => write!(f, "a trajectory coordinate of {c} m"),
            Self::RotationLength(l) => write!(f, "a trajectory rotation of length {l}"),
            Self::Speed(s) => write!(f, "a speed of {s} m per tick"),
            Self::Step(s) => write!(f, "a root step of {s} m per tick without a warp"),
        }
    }
}

pub fn epoch_newer(a: u8, b: u8) -> bool {
    (1..=127).contains(&a.wrapping_sub(b))
}

pub fn plausible(pose: &NetPose) -> Result<(), Violation> {
    let b = pose.block();
    if let Some(&c) = b.trajectory_position.iter().find(|c| c.abs() > MAX_COORDINATE_M) {
        return Err(Violation::Coordinate(c));
    }
    let length = b.trajectory_rotation.iter().map(|v| v * v).sum::<f32>().sqrt();
    if (length - 1.0).abs() > ROTATION_LENGTH_TOLERANCE {
        return Err(Violation::RotationLength(length));
    }
    let speed = b.speed();
    if speed > MAX_SPEED_PER_TICK {
        return Err(Violation::Speed(speed));
    }
    Ok(())
}

pub fn check(
    track: &mut RiderTrack,
    server_tick: u32,
    window: TickWindow,
    pose: &NetPose,
) -> Result<Verdict, Violation> {
    let d = i64::from(pose.tick()) - i64::from(server_tick);
    if d < -i64::from(window.behind) || d > i64::from(window.ahead) {
        return Err(Violation::Tick(d));
    }
    let lead = pose.view_lead();
    if lead < -f64::from(window.view_behind) || lead > f64::from(window.view_ahead) {
        return Err(Violation::ViewLead(lead));
    }
    plausible(pose)?;
    let b = pose.block();
    let flags = pose.flags();
    let order = order_of(pose);
    if let Some(n) = track.newest {
        let steps = match follows(order, n.order) {
            Ok(steps) => steps,
            Err(dropped) => return Ok(dropped),
        };
        let warped = order.epoch != n.order.epoch
            || [PoseFlag::Teleport, PoseFlag::Respawn, PoseFlag::Rewind].iter().any(|&f| flags.contains(f));
        if !warped {
            let step = (0..3).map(|i| (b.trajectory_position[i] - n.at[i]).powi(2)).sum::<f32>().sqrt();
            let per_tick = step / (steps as f32 * TICKS_PER_STEP);
            if per_tick > MAX_SPEED_PER_TICK {
                return Err(Violation::Step(per_tick));
            }
        }
    }
    track.newest = Some(Newest { order, at: b.trajectory_position });
    Ok(Verdict::Relay)
}

pub fn order_of(pose: &NetPose) -> SampleOrder {
    SampleOrder { epoch: pose.flags().warp_epoch(), execution: pose.execution(), tick: pose.tick() }
}

pub fn follows(s: SampleOrder, newest: SampleOrder) -> Result<u32, Verdict> {
    let newer_epoch = s.epoch != newest.epoch;
    if newer_epoch && !epoch_newer(s.epoch, newest.epoch) {
        return Err(Verdict::OlderEpoch);
    }
    match s.execution.cmp(&newest.execution) {
        core::cmp::Ordering::Greater => Ok(s.execution - newest.execution),
        core::cmp::Ordering::Equal if s.tick > newest.tick => Ok(s.tick - newest.tick),
        _ if newer_epoch => Ok(0),
        _ => Err(Verdict::Stale),
    }
}

impl RiderTrack {
    pub fn position(&self) -> Option<[f32; 3]> {
        self.newest.map(|n| n.at)
    }

    pub fn newest_tick(&self) -> Option<u32> {
        self.newest.map(|n| n.order.tick)
    }
}

#[cfg(test)]
mod tests;
