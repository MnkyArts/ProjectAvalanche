mod common;

use core::time::Duration;

use bevy_app::{App, FixedFirst, FixedLast, FixedPostUpdate, FixedPreUpdate, FixedUpdate, TaskPoolPlugin};
use bevy_ecs::prelude::*;
use bevy_ecs::schedule::{ScheduleLabel, Schedules};
use bevy_state::app::StatesPlugin;
use bevy_time::{Fixed, Time, TimePlugin, TimeUpdateStrategy, Virtual};
use common::*;
use ssx_net::harness::{Conditions, Io};
use ssx_net::lightyear::core::timeline::LocalTimelineShift;
use ssx_net::lightyear::prelude::{InputTimelineConfig, LocalTimeline, LocalTimelineSync};
use ssx_net::server::CATCH_UP;
use ssx_net::{NetClientPlugin, NetServerPlugin, ServerStats, tick_duration};
use ssx_net_proto::TICK_HZ;

#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct RideStep;

#[derive(Resource, Default)]
struct Steps(u64);

fn count_step(mut steps: ResMut<Steps>) {
    steps.0 += 1;
}

#[test]
fn the_network_tick_is_the_simulation_step() {
    assert_eq!(TICK_HZ as f32, ssx_physics::SIM_HZ, "one network tick per simulation step");
    let shell = Time::<Fixed>::from_hz(f64::from(ssx_physics::SIM_HZ));
    assert_eq!(shell.timestep(), tick_duration(), "the game shell's step is lightyear's tick duration");
}

fn count_over_an_hour(mut app: App) {
    app.insert_resource(Time::<Fixed>::from_hz(f64::from(ssx_physics::SIM_HZ)));
    app.init_resource::<Steps>().add_systems(FixedUpdate, count_step.in_set(RideStep));
    app.finish();
    app.cleanup();
    let frames_ms = [16.667f64, 5.0, 33.333, 50.0, 7.5, 100.0, 16.0, 17.333, 250.0, 1.0];
    let (mut virtual_s, mut frames) = (0.0, 0u64);
    app.update();
    let tick0 = app.world().resource::<LocalTimeline>().tick().0;
    let steps0 = app.world().resource::<Steps>().0;
    while virtual_s < 3600.0 {
        let dt = Duration::from_secs_f64(frames_ms[(frames % frames_ms.len() as u64) as usize] / 1000.0);
        app.insert_resource(TimeUpdateStrategy::ManualDuration(dt));
        app.update();
        virtual_s += dt.as_secs_f64();
        frames += 1;
        let ticks = u64::from(app.world().resource::<LocalTimeline>().tick().0 - tick0);
        let steps = app.world().resource::<Steps>().0 - steps0;
        assert_eq!(ticks, steps, "frame {frames}: lightyear's tick and the fixed steps");
    }
    assert_eq!(app.world().resource::<Time<Fixed>>().timestep(), tick_duration());
    let steps = app.world().resource::<Steps>().0 - steps0;
    let expected = (virtual_s * f64::from(TICK_HZ)).floor() as u64;
    assert!(steps.abs_diff(expected) <= 1, "{steps} steps for {virtual_s:.3} s ({expected} expected)");
    println!("{frames} frames, {virtual_s:.3} s, {steps} fixed steps = lightyear ticks");
}

fn headless() -> App {
    let mut app = App::new();
    app.add_plugins((TaskPoolPlugin::default(), TimePlugin, StatesPlugin));
    app
}

#[test]
fn server_tick_counts_every_fixed_step() {
    let mut app = headless();
    app.add_plugins(NetServerPlugin);
    count_over_an_hour(app);
}

#[test]
fn the_server_catches_a_stall_up_to_the_connections_timeout() {
    let mut app = headless();
    app.add_plugins(NetServerPlugin);
    app.finish();
    app.cleanup();
    assert_eq!(app.world().resource::<Time<Virtual>>().max_delta(), CATCH_UP);
    assert_eq!(CATCH_UP, Duration::from_secs(10), "the connection's timeout");
    let ticks = |app: &App| app.world().resource::<LocalTimeline>().tick().0;
    let clamped = |app: &App| app.world().resource::<ServerStats>().frames_clamped;
    app.insert_resource(TimeUpdateStrategy::ManualDuration(tick_duration()));
    for _ in 0..30 {
        app.update();
    }
    for (stall, steps, clamps) in [(3.0, 90, 0), (10.0, 300, 0), (12.0, 300, 1), (1.0, 30, 1)] {
        let before = ticks(&app);
        app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f64(stall)));
        app.update();
        assert_eq!(ticks(&app) - before, steps, "a frame of {stall} s");
        assert_eq!(clamped(&app), clamps, "clamped frames after a frame of {stall} s");
    }
    let stats = app.world().resource::<ServerStats>();
    assert_eq!(stats.frame_ms_most, 12_000);
    assert_eq!(stats.frames_long, 4, "every stall over 250 ms counted, caught up or clamped");
}

#[test]
fn client_tick_counts_every_fixed_step_offline() {
    let mut app = headless();
    app.add_plugins(NetClientPlugin);
    count_over_an_hour(app);
}

fn fixed_systems(app: &mut App) -> Vec<String> {
    let mut names = Vec::new();
    let labels = [
        FixedFirst.intern(),
        FixedPreUpdate.intern(),
        FixedUpdate.intern(),
        FixedPostUpdate.intern(),
        FixedLast.intern(),
    ];
    let schedules = app.world().resource::<Schedules>();
    for label in labels {
        let Some(schedule) = schedules.get(label) else { continue };
        if let Ok(systems) = schedule.systems() {
            names.extend(systems.map(|(_, s)| format!("{label:?}: {}", s.name())));
        }
    }
    names
}

#[test]
fn network_systems_stay_out_of_the_fixed_schedules() {
    for server in [true, false] {
        let mut app = headless();
        if server {
            app.add_plugins(NetServerPlugin);
        } else {
            app.add_plugins(NetClientPlugin);
        }
        app.init_resource::<Steps>().add_systems(FixedUpdate, count_step.in_set(RideStep));
        app.finish();
        app.cleanup();
        app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_millis(40)));
        app.update();
        app.update();
        let names = fixed_systems(&mut app);
        println!("{} fixed systems: {names:#?}", if server { "server" } else { "client" });
        assert!(names.iter().all(|n| !n.contains("ssx_net::")), "no ssx-net system in a fixed schedule");
        let network: Vec<_> = names.iter().filter(|n| !n.contains("count_step") && !n.contains(": bevy_")).collect();
        assert!(
            network.iter().all(|n| n.contains("FixedFirst: lightyear_core::timeline::increment_local_tick")),
            "only lightyear's tick increment (FixedFirst) shares the fixed schedules with the ride: {network:?}"
        );
    }
}

#[derive(Resource, Default)]
struct Shifts(i64, u32);

fn count_shift(shift: On<LocalTimelineShift>, mut shifts: ResMut<Shifts>) {
    shifts.0 += i64::from(shift.delta);
    shifts.1 += 1;
}

#[test]
fn connected_client_runs_at_wall_clock_speed() {
    let conditions = Conditions { latency: Duration::from_millis(50), jitter: Duration::from_millis(10), loss: 0.01 };
    let mut peers = connected(Io::Crossbeam, 1, Some(conditions));
    peers.clients[0].init_resource::<Steps>().add_systems(FixedUpdate, count_step);
    peers.clients[0].init_resource::<Shifts>().add_observer(count_shift);
    assert!(
        peers.steps_for(Duration::from_secs(5), |p| {
            p.clients[0].world().resource::<LocalTimelineSync>().is_synced()
        })
    );
    let mut speeds = Vec::new();
    let tick0 = client_tick(&peers, 0);
    let steps0 = peers.clients[0].world().resource::<Steps>().0;
    let shifts0 = peers.clients[0].world().resource::<Shifts>().0;
    for frame in 0..240 {
        if frame == 60 {
            std::thread::sleep(Duration::from_millis(200));
        }
        peers.step();
        speeds.push(peers.clients[0].world().resource::<Time<Virtual>>().relative_speed());
    }
    assert!(speeds.iter().all(|&s| s == 1.0), "Time<Virtual> stays at 1.0: {speeds:?}");
    let steps = peers.clients[0].world().resource::<Steps>().0 - steps0;
    let ticks = i64::from(client_tick(&peers, 0)) - i64::from(tick0);
    let shifts = peers.clients[0].world().resource::<Shifts>();
    let shift = shifts.0 - shifts0;
    println!(
        "client: {steps} fixed steps, its tick moved {ticks}: {} by the clock sync's whole-tick shifts ({} so far, {} \
         before the measurement); server-client tick offset {}",
        shift,
        shifts.1,
        shifts0,
        i64::from(client_tick(&peers, 0)) - i64::from(server_tick(&peers))
    );
    assert_eq!(ticks, steps as i64 + shift, "every tick is a fixed step or a sync shift");
}

#[test]
fn lightyears_default_sync_paces_the_client() {
    let conditions = Conditions { latency: Duration::from_millis(50), jitter: Duration::from_millis(10), loss: 0.01 };
    let mut peers = connected(Io::Crossbeam, 1, Some(conditions));
    peers.clients[0].insert_resource(InputTimelineConfig::default());
    assert!(
        peers.steps_for(Duration::from_secs(5), |p| {
            p.clients[0].world().resource::<LocalTimelineSync>().is_synced()
        })
    );
    let mut speeds = Vec::new();
    for frame in 0..240 {
        if frame == 60 {
            std::thread::sleep(Duration::from_millis(200));
        }
        peers.step();
        speeds.push(peers.clients[0].world().resource::<Time<Virtual>>().relative_speed());
    }
    let paced = speeds.iter().filter(|&&s| s != 1.0).count();
    let (lo, hi) = speeds.iter().fold((f32::MAX, f32::MIN), |(lo, hi), &s| (lo.min(s), hi.max(s)));
    println!("lightyear's default controller: {paced} of 240 frames off 1.0, relative speed {lo:.4}…{hi:.4}");
    assert!(paced > 0, "lightyear paces Time<Virtual> after a hitch");
}
