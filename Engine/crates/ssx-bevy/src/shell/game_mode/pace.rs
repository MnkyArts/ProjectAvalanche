use std::time::{Duration, Instant};

use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use bevy::winit::{UpdateMode, WinitSettings};
use ssx_assets::port_options::FrameLimit;

use super::WindowHidden;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Unfocused {
    #[default]
    Game,
    Continuous,
}

#[derive(Resource, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FramePace {
    pub limit: FrameLimit,
    pub unfocused: Unfocused,
}

const UNFOCUSED_WAIT: Duration = Duration::from_nanos(1_000_000_000 / 60);

impl FramePace {
    pub fn settings(self) -> WinitSettings {
        let Some(period) = self.limit.period() else {
            return match self.unfocused {
                Unfocused::Game => WinitSettings::game(),
                Unfocused::Continuous => WinitSettings::continuous(),
            };
        };
        // PORT(choice): the cap waits in winit's own wait between two updates (`UpdateMode::Reactive`: bevy_winit
        // 0.20's `ControlFlow::WaitUntil` at the last update's scheduled start plus the period, so the frames keep a
        // fixed grid that does not drift with each frame's length; a frame that ends past its successor's start starts
        // that one at once and the grid from there). The wait is the OS's high-resolution one: on Linux winit's
        // calloop waits in `polling`'s epoll with a timerfd (nanoseconds), on Windows winit 0.30.13's
        // `wait_for_messages_impl` in `MsgWaitForMultipleObjectsEx` on a waitable timer made with
        // `CREATE_WAITABLE_TIMER_HIGH_RESOLUTION`. Placed there, after the last update's extract, the render thread draws
        // that frame while the main thread waits, and the keys and pads that arrive during the wait are in the next
        // update (winit hands Bevy the events before it); no event ends the wait early (`react_to_*` false), so the cap
        // holds while the mouse moves. Unfocused the cap or Bevy's 60 a second, the lower. The console had no cap.
        let capped = |wait: Duration| UpdateMode::Reactive {
            wait,
            react_to_device_events: false,
            react_to_user_events: false,
            react_to_window_events: false,
        };
        let unfocused = match self.unfocused {
            Unfocused::Game => capped(period.max(UNFOCUSED_WAIT)),
            Unfocused::Continuous => capped(period),
        };
        WinitSettings { focused_mode: capped(period), unfocused_mode: unfocused }
    }
}

pub fn apply_pace(pace: Res<FramePace>, mut hidden: ResMut<WindowHidden>, mut settings: ResMut<WinitSettings>) {
    if !pace.is_changed() {
        return;
    }
    let to = pace.settings();
    info!(
        "shell: frame limit {} (focused {:?}, unfocused {:?})",
        pace.limit.fps().map_or("off".to_string(), |f| format!("{f} a second")),
        to.focused_mode,
        to.unfocused_mode
    );
    restore(&pace, &mut hidden, &mut settings);
}

fn restore(pace: &FramePace, hidden: &mut WindowHidden, settings: &mut WinitSettings) {
    let to = pace.settings();
    if !hidden.restore_to(&to) {
        *settings = to;
    }
}

#[derive(Resource, Default)]
pub struct PhaseLock {
    sin: f64,
    cos: f64,
    frames: u32,
    moved: bool,
    settle: u32,
    last: Option<Instant>,
}

impl PhaseLock {
    fn clear_readings(&mut self) {
        (self.sin, self.cos, self.frames) = (0.0, 0.0, 0);
    }
}

const PHASE_FRAMES: u32 = 16;
const PHASE_SPREAD: f64 = 0.1;
const SAME_RATE_NS: u128 = 1_000;

fn one_step_a_frame(period: Duration, step: Duration) -> bool {
    period.as_nanos().abs_diff(step.as_nanos()) <= SAME_RATE_NS
}

fn phase_turns(period: Duration, overstep: Duration) -> f64 {
    let p = period.as_nanos().max(1);
    (overstep.as_nanos() % p) as f64 / p as f64
}

fn moved_wait(wait: Duration, turns: f64) -> Option<Duration> {
    let off = turns - 0.5;
    (off.abs() > 0.25).then(|| wait.mul_f64(1.0 - off))
}

pub fn lock_phase(
    pace: Res<FramePace>,
    mut hidden: ResMut<WindowHidden>,
    (fixed, virt, strategy): (Res<Time<Fixed>>, Res<Time<Virtual>>, Res<TimeUpdateStrategy>),
    windows: Query<&Window>,
    mut lock: ResMut<PhaseLock>,
    mut settings: ResMut<WinitSettings>,
) {
    if lock.moved {
        lock.moved = false;
        restore(&pace, &mut hidden, &mut settings);
    }
    let settling = lock.settle > 0;
    if settling {
        lock.settle -= 1;
    }
    let now = Instant::now();
    let gap = lock.last.replace(now).map(|last| now - last);
    let focused = windows.iter().any(|w| w.focused);
    let mode = if focused { settings.focused_mode } else { settings.unfocused_mode };
    let wait = match mode {
        UpdateMode::Reactive { wait, .. } => Some(wait),
        UpdateMode::Continuous => None,
    };
    // PORT(choice): under a cap, the grid is kept half a frame away from the fixed steps' boundaries. Bevy 0.20 times
    // `Time<Real>` by the instant the render world sends after each frame (bevy_render 0.20 `send_time`), so the
    // fixed steps a frame runs depend on when the frame before it finished rendering, which depends on the steps that
    // frame ran: with the boundary inside that spread, the frames at 30 a second fell into running 0 and 2 ride steps
    // by turns for the whole run (alps, 1920 x 1080, Mailbox: 48 % of the updates 8 ms longer or shorter than the one
    // before; the presents' sd 11.2 ms and the frame p99 53.6 ms against 3.6 and 42.5 in a run with one step each).
    // Where a frame is one step long (a cap of 30), a circular mean of the boundary's place in the frame over 16
    // updates that lies within a quarter frame of the updates makes one wait up to half a frame longer or shorter
    // (bevy_winit 0.20 starts its grid anew when the update mode changes), restored at the next update. Measured with
    // it: no cycle in nine 30 s windows (FIFO, Immediate, Mailbox; alps, Mount Eddie, heli: 0–1.1 % of the updates
    // 8 ms longer or shorter than the one before), the presents' sd 3.4–4.1 ms; 14–15 moves a run, 1 inside the
    // windows (FIFO, alps: one frame 41.7 ms long).
    // Not while hidden, paused, at another speed or on a manual clock; after a stall (updates more than 1.25 waits
    // apart) the readings start anew. The steps and the ride's clock are not touched. The console had no cap.
    let reading = wait.filter(|&w| {
        !settling
            && !hidden.hidden
            && pace.limit.period().is_some()
            && matches!(*strategy, TimeUpdateStrategy::Automatic)
            && !virt.is_paused()
            && (virt.relative_speed_f64() - 1.0).abs() < 1e-9
            && gap.is_some_and(|g| g <= w.mul_f64(1.25))
            && one_step_a_frame(w, fixed.timestep())
    });
    let Some(wait) = reading else {
        lock.clear_readings();
        return;
    };
    let angle = std::f64::consts::TAU * phase_turns(wait, fixed.overstep());
    lock.sin += angle.sin();
    lock.cos += angle.cos();
    lock.frames += 1;
    if lock.frames < PHASE_FRAMES {
        return;
    }
    let (sin, cos, n) = (lock.sin, lock.cos, f64::from(lock.frames));
    lock.clear_readings();
    if sin.hypot(cos) / n < PHASE_SPREAD {
        return;
    }
    let turns = sin.atan2(cos).rem_euclid(std::f64::consts::TAU) / std::f64::consts::TAU;
    let Some(moved) = moved_wait(wait, turns) else { return };
    let UpdateMode::Reactive { react_to_device_events, react_to_user_events, react_to_window_events, .. } = mode else {
        return;
    };
    let moved =
        UpdateMode::Reactive { wait: moved, react_to_device_events, react_to_user_events, react_to_window_events };
    if focused {
        settings.focused_mode = moved;
    } else {
        settings.unfocused_mode = moved;
    }
    (lock.moved, lock.settle) = (true, 2);
    debug!("shell: the frame grid moved: the fixed steps' boundary at {turns:.2} of a frame, {wait:?} → {moved:?}");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cap_waits_without_waking_on_events_and_off_is_bevys_own() {
        let off = FramePace::default().settings();
        let game = WinitSettings::game();
        assert_eq!((off.focused_mode, off.unfocused_mode), (game.focused_mode, game.unfocused_mode));
        let bench = FramePace { unfocused: Unfocused::Continuous, ..default() }.settings();
        assert_eq!((bench.focused_mode, bench.unfocused_mode), (UpdateMode::Continuous, UpdateMode::Continuous));
        let at = |limit, unfocused| FramePace { limit, unfocused }.settings();
        let reactive = |ns: u64| UpdateMode::Reactive {
            wait: Duration::from_nanos(ns),
            react_to_device_events: false,
            react_to_user_events: false,
            react_to_window_events: false,
        };
        let s = at(FrameLimit::Fps30, Unfocused::Game);
        assert_eq!((s.focused_mode, s.unfocused_mode), (reactive(33_333_333), reactive(33_333_333)));
        let s = at(FrameLimit::Fps144, Unfocused::Game);
        assert_eq!((s.focused_mode, s.unfocused_mode), (reactive(6_944_444), reactive(16_666_666)));
        let s = at(FrameLimit::Fps144, Unfocused::Continuous);
        assert_eq!(s.unfocused_mode, reactive(6_944_444));
    }

    fn step() -> Duration {
        Time::<Fixed>::from_hz(f64::from(ssx_physics::SIM_HZ)).timestep()
    }

    #[test]
    fn a_frame_is_one_step_long_at_30_only() {
        let period = |l: FrameLimit| l.period().expect("a cap");
        assert!(one_step_a_frame(period(FrameLimit::Fps30), step()));
        for l in [FrameLimit::Fps60, FrameLimit::Fps120, FrameLimit::Fps144] {
            assert!(!one_step_a_frame(period(l), step()), "{l:?}");
        }
        assert!(!one_step_a_frame(UNFOCUSED_WAIT, step()));
        assert!(!one_step_a_frame(Duration::ZERO, step()));
    }

    #[test]
    fn a_moved_wait_puts_the_boundary_half_a_frame_from_the_updates() {
        for l in [FrameLimit::Fps30, FrameLimit::Fps60, FrameLimit::Fps120] {
            let wait = l.period().expect("a cap");
            for i in 0..100 {
                let turns = f64::from(i) / 100.0;
                match moved_wait(wait, turns) {
                    Some(m) => {
                        let after =
                            (turns + (m.as_secs_f64() - wait.as_secs_f64()) / wait.as_secs_f64()).rem_euclid(1.0);
                        assert!((after - 0.5).abs() < 1e-6, "{l:?} {turns}: {after}");
                        assert!(m >= wait.mul_f64(0.5) && m <= wait.mul_f64(1.5), "{l:?} {turns}: {m:?}");
                    }
                    None => assert!((turns - 0.5).abs() <= 0.25, "{l:?} {turns}"),
                }
            }
        }
    }

    fn world(limit: FrameLimit, overstep: Duration) -> World {
        let mut world = World::new();
        let pace = FramePace { limit, unfocused: Unfocused::Game };
        let mut fixed = Time::<Fixed>::from_hz(f64::from(ssx_physics::SIM_HZ));
        fixed.accumulate_overstep(overstep);
        world.insert_resource(pace.settings());
        world.insert_resource(pace);
        world.insert_resource(fixed);
        world.insert_resource(Time::<Virtual>::default());
        world.insert_resource(TimeUpdateStrategy::default());
        world.insert_resource(WindowHidden::default());
        world.init_resource::<PhaseLock>();
        world.spawn(Window { focused: true, ..default() });
        world
    }

    fn set_overstep(world: &mut World, overstep: Duration) {
        let mut fixed = world.resource_mut::<Time<Fixed>>();
        let now = fixed.overstep();
        fixed.discard_overstep(now);
        fixed.accumulate_overstep(overstep);
    }

    fn modes(world: &World) -> (UpdateMode, UpdateMode) {
        let s = world.resource::<WinitSettings>();
        (s.focused_mode, s.unfocused_mode)
    }

    fn focused_wait(world: &World) -> Duration {
        match world.resource::<WinitSettings>().focused_mode {
            UpdateMode::Reactive { wait, .. } => wait,
            UpdateMode::Continuous => Duration::ZERO,
        }
    }

    fn run(world: &mut World, updates: u32) {
        use bevy::ecs::system::RunSystemOnce;
        for _ in 0..updates {
            world.run_system_once(lock_phase).expect("the lock runs");
        }
    }

    #[test]
    fn a_boundary_at_the_updates_moves_the_grid_once_and_the_wait_is_restored() {
        let wait = FrameLimit::Fps30.period().expect("a cap");
        let mut w = world(FrameLimit::Fps30, Duration::from_millis(1));
        run(&mut w, PHASE_FRAMES);
        assert_eq!(focused_wait(&w), wait);
        run(&mut w, 1);
        let moved = focused_wait(&w);
        let expected = wait.mul_f64(1.5) - Duration::from_millis(1);
        assert!(moved.abs_diff(expected) < Duration::from_micros(1), "{moved:?} against {expected:?}");
        run(&mut w, 1);
        assert_eq!(focused_wait(&w), wait);
        let pace = w.resource::<FramePace>().settings();
        assert_eq!(modes(&w), (pace.focused_mode, pace.unfocused_mode));
        set_overstep(&mut w, wait / 2);
        run(&mut w, 3 * PHASE_FRAMES);
        assert_eq!(focused_wait(&w), wait);
    }

    #[test]
    fn frames_on_both_sides_of_the_boundary_read_as_at_it() {
        let wait = FrameLimit::Fps30.period().expect("a cap");
        let mut w = world(FrameLimit::Fps30, Duration::ZERO);
        run(&mut w, 1);
        for i in 0..PHASE_FRAMES {
            let off = if i % 2 == 0 { Duration::from_millis(2) } else { wait - Duration::from_millis(2) };
            set_overstep(&mut w, off);
            run(&mut w, 1);
        }
        let moved = focused_wait(&w);
        assert!(moved > wait.mul_f64(1.4) || moved < wait.mul_f64(0.6), "{moved:?}");
    }

    #[test]
    fn no_move_while_hidden_paused_uncapped_or_above_30() {
        let check = |mut w: World| {
            let before = modes(&w);
            run(&mut w, 4 * PHASE_FRAMES);
            assert_eq!(modes(&w), before);
        };
        let at = Duration::from_millis(1);
        let mut hidden = world(FrameLimit::Fps30, at);
        hidden.resource_mut::<WindowHidden>().hidden = true;
        check(hidden);
        let mut paused = world(FrameLimit::Fps30, at);
        paused.resource_mut::<Time<Virtual>>().pause();
        check(paused);
        let mut manual = world(FrameLimit::Fps30, at);
        manual.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_millis(33)));
        check(manual);
        check(world(FrameLimit::Off, at));
        for l in [FrameLimit::Fps60, FrameLimit::Fps120, FrameLimit::Fps144] {
            check(world(l, at));
        }
    }
}
