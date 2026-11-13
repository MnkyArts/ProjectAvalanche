use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

use bevy_ecs::prelude::*;
use bevy_time::{Fixed, Real, Time, Virtual};
use lightyear::prelude::server::ClientOf;
use lightyear::prelude::{LocalTimeline, PingManager};
use ssx_gameplay::validate::Arrival;

use super::ServerStats;
use super::instance::Instances;
use super::validation::{ValidationEvent, ValidationLog};
use crate::tick::tick_duration;

pub const JITTER_MULTIPLE: f64 = 4.0;
pub const JITTER_MARGIN_TICKS: f64 = 1.0;

pub const UNALIGNED_GO_TICKS: f64 = 0.5;

// PORT(choice): 2e-4, twice 100 ppm — a quartz clock's tolerance is commonly given as 20 to 100 ppm, one machine's
// clocks agree (loopback: 0); tuning: the clients' execution counts against the server's real time over long rounds;
// the original has no real-time multiplayer.
pub const CLOCK_RATE_TOLERANCE: f64 = 2.0e-4;

fn tick_s() -> f64 {
    tick_duration().as_secs_f64()
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct RoundStart {
    round: u16,
    tick: u32,
    at: Instant,
    lost: f64,
}

#[derive(Component, Clone, Copy, Debug, Default, PartialEq)]
pub struct LinkTime {
    lost_at_connect: f64,
    one_way_max: f64,
    connected_at: Option<Instant>,
}

impl LinkTime {
    pub fn rates_since_connect(&self, at: Instant) -> f64 {
        self.connected_at
            .map_or(0.0, |c| CLOCK_RATE_TOLERANCE * at.saturating_duration_since(c).as_secs_f64() / tick_s())
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Received {
    pub at: Instant,
    pub after: Instant,
}

#[derive(Resource, Debug, Default)]
pub struct ServerTime {
    lost: f64,
    frame: Option<Instant>,
    frames: VecDeque<Instant>,
    receive_hold: Duration,
    starts: HashMap<u16, RoundStart>,
}

impl ServerTime {
    pub fn set_receive_hold(&mut self, hold: Duration) {
        self.receive_hold = hold;
    }

    fn frame_began(&mut self, f: Instant) {
        self.frames.push_back(f);
        let reach = f.checked_sub(self.receive_hold);
        while self.frames.get(2).is_some_and(|&third| reach.is_some_and(|r| third <= r)) {
            self.frames.pop_front();
        }
    }

    pub fn received(&self) -> Received {
        let at = Instant::now();
        let after = if self.receive_hold.is_zero() {
            self.frame
        } else {
            let read_by = at.checked_sub(self.receive_hold);
            read_by
                .and_then(|r| {
                    let i = self.frames.iter().rposition(|&f| f <= r)?;
                    self.frames.get(i.checked_sub(1)?).copied()
                })
                .or_else(|| self.frames.front().copied())
        };
        Received { at, after: after.unwrap_or(at) }
    }

    pub fn elapsed(&self, instances: &Instances, instance: u16, round: u16, now: u32, at: Instant) -> Option<f64> {
        if let Some(s) = self.starts.get(&instance).filter(|s| s.round == round) {
            return Some(signed_secs(at, s.at) / tick_s());
        }
        let s = instances.by_id(instance).filter(|s| s.instance.round() == round)?;
        s.instance.start_tick().map(|t| f64::from(now) - f64::from(t))
    }

    pub fn lost_since(&self, link: Option<&LinkTime>) -> f64 {
        (self.lost - link.map_or(self.lost, |l| l.lost_at_connect)).max(0.0)
    }

    pub fn started(&self, instance: u16, round: u16) -> bool {
        self.starts.get(&instance).is_some_and(|s| s.round == round)
    }

    pub fn arrival(
        &self,
        instances: &Instances,
        (instance, round): (u16, u16),
        now: u32,
        link: Option<&LinkTime>,
        received: Received,
    ) -> Option<Arrival> {
        let elapsed = self.elapsed(instances, instance, round, now, received.at)?;
        let link = link.copied().unwrap_or_default();
        let lost_by_start = self.starts.get(&instance).filter(|s| s.round == round).map_or(self.lost, |s| s.lost);
        let lost = (lost_by_start - link.lost_at_connect).max(0.0);
        Some(Arrival {
            elapsed,
            window: signed_secs(received.at, received.after).max(0.0) / tick_s(),
            horizon: horizon(link.one_way_max, lost, elapsed),
            pace: link.one_way_max + JITTER_MARGIN_TICKS + 1.0,
        })
    }
}

// PORT(choice): the client's half round trip within lightyear's worst case of a one-way time measured by the server
// (half the server's smoothed round trip, 4 jitters, 1 tick — `SyncConfig::jitter_margin`; both sides smooth the same
// path's round trips with lightyear's EWMA, the client's own samples are not on the wire); tuning: the clients' round
// trip estimates at their GO logged beside the server's over the network runs; the original has no real-time
// multiplayer.
pub fn horizon(one_way_max: f64, lost: f64, elapsed: f64) -> f64 {
    one_way_max + JITTER_MARGIN_TICKS + UNALIGNED_GO_TICKS + lost + CLOCK_RATE_TOLERANCE * elapsed.max(0.0)
}

pub fn ready_early_allowed(horizon: f64, execution: f64) -> f64 {
    horizon - execution
}

fn signed_secs(a: Instant, b: Instant) -> f64 {
    match a.checked_duration_since(b) {
        Some(d) => d.as_secs_f64(),
        None => -b.duration_since(a).as_secs_f64(),
    }
}

type Measured<'a> = (Entity, Option<&'a PingManager>, Option<&'a mut LinkTime>);

type Clocks<'w> = (Res<'w, Time<Real>>, Res<'w, Time<Virtual>>, Res<'w, Time<Fixed>>);

pub(super) fn track(
    (real, virt, fixed): Clocks,
    timeline: Res<LocalTimeline>,
    (instances, log, mut stats): (Option<Res<Instances>>, Option<ResMut<ValidationLog>>, ResMut<ServerStats>),
    mut time: ResMut<ServerTime>,
    mut links: Query<Measured, With<ClientOf>>,
    mut commands: Commands,
) {
    let tick = tick_s();
    let clamped = real.delta().saturating_sub(virt.delta());
    if !clamped.is_zero() {
        let lost = clamped.as_secs_f64() / tick;
        time.lost += lost;
        stats.frames_clamped += 1;
        let ms = u64::try_from(real.delta().as_millis()).unwrap_or(u64::MAX);
        stats.frame_ms_most = stats.frame_ms_most.max(ms);
        if let Some(mut log) = log {
            log.push(ValidationEvent {
                instance: 0,
                round: 0,
                level: String::new(),
                rider: None,
                check: None,
                action: None,
                text: format!(
                    "a frame of {ms} ms: Bevy's fixed steps took 250 ms of it, {lost:.1} ticks are lost for good (the \
                     server's tick lags its real time by {:.1} ticks in all; the validation measures real time)",
                    time.lost
                ),
            });
        }
    }
    time.frame = real.last_update();
    if let Some(f) = time.frame {
        time.frame_began(f);
    }
    let now = timeline.tick().0;
    if let (Some(instances), Some(update)) = (instances, real.last_update()) {
        let lost = time.lost;
        for s in &instances.list {
            let (id, round) = (s.instance.id(), s.instance.round());
            let Some(t) = s.instance.start_tick() else { continue };
            if now < t || time.starts.get(&id).is_some_and(|r| r.round == round && r.tick == t) {
                continue;
            }
            let back = fixed.overstep().as_secs_f64() + f64::from(now - t) * tick;
            let at = update.checked_sub(std::time::Duration::from_secs_f64(back)).unwrap_or(update);
            time.starts.insert(id, RoundStart { round, tick: t, at, lost });
        }
    }
    for (entity, ping, link) in &mut links {
        let one_way = ping
            .filter(|p| p.pongs_recv > 0)
            .map(|p| (p.rtt().as_secs_f64() / 2.0 + JITTER_MULTIPLE * p.jitter().as_secs_f64()) / tick);
        match link {
            Some(mut l) => {
                if let Some(w) = one_way.filter(|w| *w > l.one_way_max) {
                    l.one_way_max = w;
                }
            }
            None => {
                let l = LinkTime {
                    lost_at_connect: time.lost,
                    one_way_max: one_way.unwrap_or(0.0),
                    connected_at: real.last_update(),
                };
                commands.entity(entity).insert(l);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_horizon_and_the_early_ready_follow_the_protocol() {
        let one_way = (0.05 + 4.0 * 0.005) * 30.0;
        assert!((horizon(one_way, 0.0, 0.0) - (one_way + 1.5)).abs() < 1e-12);
        assert!((horizon(one_way, 0.0, 18_000.0) - (one_way + 1.5 + 3.6)).abs() < 1e-9);
        assert!(ready_early_allowed(horizon(one_way, 0.0, 0.0), 21.0) < 0.0);
        let allowed = ready_early_allowed(horizon(one_way, 140.0, 0.0), 41.0);
        assert!((allowed - (one_way + 1.5 + 140.0 - 41.0)).abs() < 1e-9, "{allowed}");
    }

    #[test]
    fn a_links_lost_ticks_are_counted_from_its_connection() {
        let time = ServerTime { lost: 300.0, ..ServerTime::default() };
        let link = LinkTime { lost_at_connect: 120.0, one_way_max: 2.0, connected_at: None };
        assert_eq!(time.lost_since(Some(&link)), 180.0);
        assert_eq!(time.lost_since(None), 0.0);
    }

    #[test]
    fn a_links_clock_rates_grow_with_its_time() {
        let now = Instant::now();
        let hour = now.checked_sub(Duration::from_secs(3600)).expect("an instant an hour ago");
        let link = LinkTime { connected_at: Some(hour), ..LinkTime::default() };
        assert!((link.rates_since_connect(now) - 21.6).abs() < 1e-6, "{}", link.rates_since_connect(now));
        assert_eq!(LinkTime::default().rates_since_connect(now), 0.0);
        assert_eq!(link.rates_since_connect(hour), 0.0);
    }

    #[test]
    fn a_conditioned_links_window_reaches_back_over_its_hold() {
        let now = Instant::now();
        let ago = |ms: u64| now.checked_sub(Duration::from_millis(ms)).expect("an instant before now");
        let mut time = ServerTime { frame: Some(ago(133)), ..ServerTime::default() };
        time.frames.extend([ago(2_000), ago(200), ago(167), ago(133)]);
        assert_eq!(time.received().after, ago(133));
        time.set_receive_hold(Duration::from_millis(70));
        assert_eq!(time.received().after, ago(167));
        time.set_receive_hold(Duration::from_millis(150));
        assert_eq!(time.received().after, ago(200));
        time.set_receive_hold(Duration::from_millis(180));
        assert_eq!(time.received().after, ago(2_000));
    }

    #[test]
    fn a_stall_longer_than_seconds_keeps_its_frame_in_the_window() {
        let now = Instant::now();
        let ago = |ms: u64| now.checked_sub(Duration::from_millis(ms)).expect("an instant before now");
        let mut time = ServerTime::default();
        time.set_receive_hold(Duration::from_millis(70));
        for ms in [2_798, 2_782, 2_766, 2_750, 50, 34, 18] {
            time.frame_began(ago(ms));
        }
        time.frame = Some(ago(18));
        assert_eq!(time.received().after, ago(2_766));
        assert_eq!(time.frames.front().copied(), Some(ago(2_766)));
        let later = |ms: u64| now + Duration::from_millis(ms);
        time.frame_began(later(14));
        time.frame_began(later(30));
        assert_eq!(time.frames.front().copied(), Some(ago(2_750)));
        time.frame_began(later(46));
        assert_eq!(time.frames.front().copied(), Some(ago(50)));
    }
}
