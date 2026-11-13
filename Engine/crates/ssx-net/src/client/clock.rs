use bevy_ecs::prelude::*;
use bevy_time::{Fixed, Real, Time, Virtual};
use lightyear::prelude::client::RemoteTimeline;
use lightyear::prelude::{Client, Connected, LocalTimeline};
use lightyear_sync::timeline::sync::SyncTargetTimeline;

#[derive(Resource, Clone, Copy, Debug, Default, PartialEq)]
pub struct ServerClock {
    pub lead: Option<f64>,
    pub shifts: i64,
    pub lost: f64,
    pub pending: f64,
}

#[derive(Resource, Clone, Copy, Debug, Default, PartialEq)]
pub struct ViewClock {
    origin: Option<(f64, f64)>,
    pub offset: Option<f64>,
}

impl ViewClock {
    pub fn of(&self, t: f64) -> Option<f64> {
        self.offset.map(|o| t + o)
    }

    pub fn update(&mut self, real: f64, estimate: Option<f64>) {
        let Some(e) = estimate else {
            *self = Self::default();
            return;
        };
        let (r0, e0) = *self.origin.get_or_insert((real, e));
        self.offset = Some((real - r0) - (e - e0));
    }

    pub fn origin(&self) -> Option<(f64, f64)> {
        self.origin
    }
}

// PORT(choice): every half second (15 ticks): 64 offsets over the window; tuning: the display error through the stalls
// (net_bot's trace, round net3d's display runs) with a quarter and a whole second; the original has no real-time
// multiplayer.
pub const VIEW_MAP_EVERY_TICKS: f64 = 15.0;
// PORT(choice): 32 s (960 ticks): a server's stall of 2 s moved the clients' estimates for about 6 s (round net3c: the
// senders' stamps jumped back 9 to 10 ticks at a time, the receiver's estimate slewed), 8 s of offsets off in 32 s
// leave the median on the settled ones; the clocks' rates move the true offset by at most 2e-4 × 960 = 0.2 ticks over
// it; tuning: the display error through stalls of 2, 5 and 10 s; the original has no real-time multiplayer.
pub const VIEW_MAP_WINDOW_TICKS: f64 = 960.0;
const VIEW_MAP_KEPT: usize = (VIEW_MAP_WINDOW_TICKS / VIEW_MAP_EVERY_TICKS) as usize + 1;
pub const VIEW_MAP_MAX_AGE_TICKS: f64 = crate::limits::POSE_TICK_WINDOW as f64 / 2.0;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ViewMap {
    origin: Option<(f64, f64)>,
    kept: std::collections::VecDeque<(f64, f64)>,
    offset: Option<f64>,
}

impl ViewMap {
    pub fn place(&mut self, clock: &ViewClock, view: f64, lead: f64, now: f64) -> Option<f64> {
        let own = clock.offset?;
        if !(view.is_finite() && lead.is_finite() && now.is_finite()) {
            return None;
        }
        if self.origin != clock.origin {
            *self = Self { origin: clock.origin, ..Self::default() };
        }
        let offset = own - lead;
        if self.kept.back().is_none_or(|&(at, _)| now - at >= VIEW_MAP_EVERY_TICKS || now < at) {
            self.kept.push_back((now, offset));
            while self.kept.len() > VIEW_MAP_KEPT
                || self.kept.front().is_some_and(|&(at, _)| at < now - VIEW_MAP_WINDOW_TICKS)
            {
                self.kept.pop_front();
            }
            let mut v: Vec<f64> = self.kept.iter().map(|k| k.1).collect();
            v.sort_by(f64::total_cmp);
            let n = v.len();
            self.offset = Some(if n % 2 == 1 { v[n / 2] } else { (v[n / 2 - 1] + v[n / 2]) / 2.0 });
        }
        let c = self.offset.unwrap_or(offset);
        Some((view + c).clamp(now - VIEW_MAP_MAX_AGE_TICKS, now))
    }

    pub fn offset(&self) -> Option<f64> {
        self.offset
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StepGrid {
    pub lead: f64,
    pub shifts: i64,
    pub lost: f64,
}

impl ServerClock {
    pub fn server_time(&self, local: f64) -> Option<f64> {
        self.lead.map(|lead| local - lead)
    }

    pub fn local_now(&self, timeline: &LocalTimeline, fixed: &Time<Fixed>) -> f64 {
        local_time(timeline, fixed) + self.pending
    }

    pub fn server_now(&self, timeline: &LocalTimeline, fixed: &Time<Fixed>) -> Option<f64> {
        self.server_time(self.local_now(timeline, fixed))
    }

    pub fn grid_time(&self, tick: u32, grid: StepGrid) -> f64 {
        f64::from(tick) - grid.lead - (self.shifts - grid.shifts) as f64 + (self.lost - grid.lost)
    }
}

pub fn local_time(timeline: &LocalTimeline, fixed: &Time<Fixed>) -> f64 {
    f64::from(timeline.tick().0) + fixed.overstep_fraction_f64()
}

pub(super) fn count_shifts(shift: On<lightyear::prelude::LocalTimelineShift>, mut clock: ResMut<ServerClock>) {
    clock.shifts = clock.shifts.saturating_add(i64::from(shift.delta));
    clock.lead = clock.lead.map(|lead| lead + f64::from(shift.delta));
}

pub(super) fn update(
    remote: Query<&RemoteTimeline, (With<Client>, With<Connected>)>,
    timeline: Res<LocalTimeline>,
    fixed: Res<Time<Fixed>>,
    virt: Res<Time<Virtual>>,
    real: Res<Time<Real>>,
    mut clock: ResMut<ServerClock>,
    mut view: ResMut<ViewClock>,
) {
    let tick = fixed.timestep().as_secs_f64();
    clock.pending = virt.delta().as_secs_f64() / tick;
    clock.lost += real.delta().saturating_sub(virt.delta()).as_secs_f64() / tick;
    let estimate = remote.iter().find(|r| r.is_initialized()).map(|r| r.current_estimate());
    let local = clock.local_now(&timeline, &fixed);
    clock.lead = estimate.map(|e| {
        let server = f64::from(e.tick().0) + f64::from(e.overstep().to_f32());
        local - server
    });
    view.update(real.elapsed().as_secs_f64() / tick, clock.server_time(local));
}

pub(super) fn pending_taken(mut clock: ResMut<ServerClock>) {
    clock.pending = 0.0;
}

const GRID_EPSILON: f64 = 1e-6;

// PORT(choice): one second (the 50-race runs with the alignment at the start's arrival measured a first round's skew of
// 0.62–1.50 ticks against 0.1–0.6 in the later rounds, the estimate a second or two old); tuning: the start's skew of
// the first rounds after the joins with this alignment over the 50-race runs; the original has no real-time
// multiplayer.
pub const ALIGN_BEFORE_START_TICKS: f64 = 30.0;

pub fn align_due(clock: &ServerClock, timeline: &LocalTimeline, fixed: &Time<Fixed>, start: u32) -> bool {
    clock.server_now(timeline, fixed).is_some_and(|now| now + ALIGN_BEFORE_START_TICKS >= f64::from(start))
}

pub fn align_steps(
    clock: &mut ServerClock,
    timeline: &LocalTimeline,
    fixed: &mut Time<Fixed>,
) -> Option<(f64, StepGrid)> {
    let next = f64::from(timeline.tick().0) + 1.0;
    let server = clock.server_time(next)?;
    let fraction = server - server.floor();
    let shift = if fraction.is_finite() && (GRID_EPSILON..1.0 - GRID_EPSILON).contains(&fraction) {
        fixed.accumulate_overstep(fixed.timestep().mul_f64(fraction));
        fraction
    } else {
        0.0
    };
    let lead = clock.lead? + shift;
    clock.lead = Some(lead);
    Some((shift, StepGrid { lead: lead.round(), shifts: clock.shifts, lost: clock.lost }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_steps_go_onto_the_servers_tick_grid() {
        let mut timeline = LocalTimeline::default();
        timeline.apply_delta(500);
        let mut fixed = Time::<Fixed>::from_hz(30.0);
        let mut clock = ServerClock { lead: Some(99.7), ..ServerClock::default() };
        let (shift, grid) = align_steps(&mut clock, &timeline, &mut fixed).expect("an estimate");
        assert!((shift - 0.3).abs() < 1e-9, "{shift}");
        assert_eq!(grid, StepGrid { lead: 100.0, shifts: 0, lost: 0.0 });
        let next = clock.server_time(501.0).expect("an estimate");
        assert!((next - 401.0).abs() < 1e-9, "{next}");
        let moved = fixed.overstep().as_secs_f64() * 30.0;
        assert!((moved - 0.3).abs() < 1e-6, "{moved}");
        assert_eq!(align_steps(&mut clock, &timeline, &mut fixed).map(|a| a.0), Some(0.0), "on the grid already");
        assert_eq!(align_steps(&mut ServerClock::default(), &timeline, &mut fixed), None, "no estimate");
        clock.lead = Some(100.45);
        assert_eq!(clock.grid_time(505, grid), 405.0);
        clock.shifts += 1;
        assert_eq!(clock.grid_time(506, grid), 405.0);
        clock.lost += 28.5;
        clock.shifts += 28;
        assert_eq!(clock.grid_time(506 + 7 + 28, grid), 405.0 + 7.0 + 28.5);
    }

    #[test]
    fn a_shift_of_the_local_tick_moves_the_lead_with_it() {
        let mut world = World::new();
        world.insert_resource(ServerClock { lead: Some(99.7), ..ServerClock::default() });
        world.add_observer(count_shifts);
        let mut timeline = LocalTimeline::default();
        timeline.apply_delta(500);
        let server = |w: &World, t: &LocalTimeline| w.resource::<ServerClock>().server_time(f64::from(t.tick().0));
        let before = server(&world, &timeline).expect("an estimate");
        for delta in [10, -9, 1] {
            world.trigger(lightyear::prelude::LocalTimelineShift { delta });
            timeline.apply_delta(delta);
            let after = server(&world, &timeline).expect("an estimate");
            assert!((after - before).abs() < 1e-9, "the server's time after a shift of {delta}: {after}, {before}");
        }
        assert_eq!(world.resource::<ServerClock>().shifts, 2);
        world.insert_resource(ServerClock::default());
        world.trigger(lightyear::prelude::LocalTimelineShift { delta: 4 });
        assert_eq!(*world.resource::<ServerClock>(), ServerClock { shifts: 4, ..ServerClock::default() });
    }

    #[test]
    fn the_frames_instant_holds_its_pending_time() {
        let mut timeline = LocalTimeline::default();
        timeline.apply_delta(500);
        let fixed = Time::<Fixed>::from_hz(30.0);
        let clock = ServerClock { lead: Some(100.0), pending: 0.5, ..ServerClock::default() };
        assert!((clock.local_now(&timeline, &fixed) - 500.5).abs() < 1e-9);
        assert_eq!(clock.server_now(&timeline, &fixed), Some(400.5));
        assert_eq!(local_time(&timeline, &fixed), 500.0);
    }

    #[test]
    fn the_view_time_runs_with_real_time_through_the_estimates_slew() {
        let mut view = ViewClock::default();
        assert_eq!(view.of(5.0), None);
        let estimate = |real: f64| 1000.0 + real - 52.0 * (1.0 - (-real.max(0.0) / 33.0).exp());
        let mut last = None;
        for k in 0..=90 {
            let real = f64::from(k);
            let e = estimate(real);
            view.update(real, Some(e));
            let now = view.of(e).unwrap();
            assert!((now - (1000.0 + real)).abs() < 1e-9, "the view time at real {real}: {now}");
            if let Some(l) = last {
                assert!(now > l);
            }
            last = Some(now);
        }
        assert!(estimate(5.0) < estimate(0.0) + 5.0 - 6.0, "the estimate fell behind real time");
        view.update(91.0, None);
        assert_eq!(view, ViewClock::default());
        view.update(92.0, Some(10.0));
        assert_eq!(view.of(10.0), Some(10.0));
    }

    #[test]
    fn a_senders_view_time_goes_on_the_receivers_through_a_servers_stall() {
        let lost = |r: f64| (r - 1200.0).clamp(0.0, 60.0);
        let server = |r: f64| 5000.0 + r - lost(r);
        let sender_est = |r: f64| {
            server(r) + if r < 1260.0 { lost(r) } else { (60.0 - 10.0 * ((r - 1260.0) / 15.0).floor()).max(0.0) }
        };
        let receiver_est = |r: f64| server(r) + if r < 1290.0 { lost(r) } else { 60.0 * (-(r - 1290.0) / 30.0).exp() };
        let (mut sv, mut rv) = (ViewClock::default(), ViewClock::default());
        let mut map = ViewMap::default();
        let mut constant_liar = ViewMap::default();
        let transit = 3.0;
        let (mut worst_view, mut worst_stamp, mut worst_lie) = (0.0f64, 0.0f64, 0.0f64);
        let mut r = 0.0;
        while r < 2400.0 {
            sv.update(r, Some(sender_est(r)));
            rv.update(r + 0.0, Some(receiver_est(r)));
            if r >= 10.0 && (r as u64).is_multiple_of(2) && !(1200.0..1260.0).contains(&(r - transit)) {
                let sent = r - transit;
                let stamp = sender_est(sent);
                let mut s2 = ViewClock { origin: sv.origin, ..ViewClock::default() };
                s2.update(sent, Some(stamp));
                let view = s2.of(stamp).unwrap();
                let lead = view - stamp;
                let now = rv.of(receiver_est(r)).unwrap();
                let placed = map.place(&rv, view, lead, now).unwrap();
                let lied = constant_liar.place(&rv, view + 200.0, lead + 200.0, now).unwrap();
                worst_lie = worst_lie.max((lied - placed).abs());
                let truth = now - transit;
                if r > 60.0 {
                    worst_view = worst_view.max((placed - truth).abs());
                    worst_stamp = worst_stamp.max((rv.of(stamp).unwrap() - truth).abs());
                }
            }
            r += 1.0;
        }
        assert!(worst_view < 0.5, "the view time placed the samples {worst_view} ticks off");
        assert!(worst_stamp > 10.0, "the stamps alone: {worst_stamp} ticks off");
        assert!(worst_lie < 1e-6, "a constant lie moved the samples {worst_lie} ticks");
        let now = rv.of(receiver_est(r)).unwrap();
        let mut liar = ViewMap::default();
        for (view, lead) in [(1e9, 0.0), (-1e9, 0.0), (now, 1e7), (now, -1e7), (f64::NAN, 0.0), (now, f64::INFINITY)] {
            if let Some(t) = liar.place(&rv, view, lead, now) {
                assert!((now - VIEW_MAP_MAX_AGE_TICKS..=now).contains(&t), "{view} {lead}: {t}");
            } else {
                assert!(!view.is_finite() || !lead.is_finite());
            }
        }
        let mut fresh = ViewClock::default();
        fresh.update(0.0, Some(100.0));
        let t = map.place(&fresh, 90.0, 0.0, 100.0).unwrap();
        assert_eq!((t, map.offset()), (90.0, Some(0.0)));
        assert_eq!(ViewMap::default().place(&ViewClock::default(), 1.0, 0.0, 1.0), None, "no view time yet");
    }
}
