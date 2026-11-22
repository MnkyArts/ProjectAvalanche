use std::sync::{Arc, Mutex};
use std::thread::ThreadId;
use std::time::{Duration, Instant};

use avalanche_server::ServerOptions;
use bevy::prelude::*;
use ssx_net::ServerStats;
use ssx_net::lightyear::prelude::LocalTimeline;

use super::common::{defaults, temp_dir};

type Seen = Arc<Mutex<Vec<(&'static str, ThreadId)>>>;

#[derive(Default)]
struct Frames {
    count: u64,
    clamped: u64,
    first: Option<(Instant, u32)>,
    last: Option<(Instant, u32)>,
}

fn options(extend: avalanche_server::AppExtension) -> ServerOptions {
    let mut config = defaults();
    config.bind = std::net::Ipv4Addr::LOCALHOST.into();
    config.port = 0;
    ServerOptions {
        config,
        data_dir: temp_dir("frames"),
        console: false,
        logging: false,
        log_to_file: false,
        signals: false,
        conditions: None,
        relay_rates: None,
        extend: Some(extend),
        rules: avalanche_server::rules::RulesSource::Unchecked,
    }
}

fn wait_for(what: &str, limit: Duration, mut done: impl FnMut() -> bool) {
    let until = Instant::now() + limit;
    while !done() {
        assert!(Instant::now() < until, "{what} within {limit:?}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn record(seen: &Seen, name: &'static str) -> impl FnMut() + Send + Sync + 'static {
    let seen = Arc::clone(seen);
    move || {
        if let Ok(mut s) = seen.lock() {
            s.push((name, std::thread::current().id()));
        }
    }
}

#[test]
fn every_schedule_runs_on_the_servers_thread() {
    let seen: Seen = Arc::default();
    let server_thread: Arc<Mutex<Option<ThreadId>>> = Arc::default();
    let (s, t) = (Arc::clone(&seen), Arc::clone(&server_thread));
    let extend: avalanche_server::AppExtension = Box::new(move |app: &mut App| {
        if let Ok(mut t) = t.lock() {
            *t = Some(std::thread::current().id());
        }
        app.add_systems(Main, record(&s, "Main"));
        app.add_systems(First, record(&s, "First"));
        app.add_systems(PreUpdate, record(&s, "PreUpdate"));
        app.add_systems(StateTransition, record(&s, "StateTransition"));
        app.add_systems(RunFixedMainLoop, record(&s, "RunFixedMainLoop"));
        app.add_systems(FixedFirst, record(&s, "FixedFirst"));
        app.add_systems(FixedPreUpdate, record(&s, "FixedPreUpdate"));
        app.add_systems(FixedUpdate, record(&s, "FixedUpdate"));
        app.add_systems(FixedPostUpdate, record(&s, "FixedPostUpdate"));
        app.add_systems(FixedLast, record(&s, "FixedLast"));
        app.add_systems(Update, record(&s, "Update"));
        app.add_systems(PostUpdate, record(&s, "PostUpdate"));
        app.add_systems(Last, record(&s, "Last"));
    });
    let server = avalanche_server::spawn(options(extend)).expect("the server starts");
    let names = || seen.lock().map(|s| s.iter().map(|(n, _)| *n).collect::<Vec<_>>()).unwrap_or_default();
    wait_for("every probed schedule ran", Duration::from_secs(10), || {
        let names = names();
        ["Main", "First", "PreUpdate", "StateTransition", "RunFixedMainLoop", "FixedFirst", "FixedPreUpdate"]
            .into_iter()
            .chain(["FixedUpdate", "FixedPostUpdate", "FixedLast", "Update", "PostUpdate", "Last"])
            .all(|n| names.contains(&n))
    });
    server.stop();
    server.join().expect("the server ends");
    let thread = server_thread.lock().ok().and_then(|t| *t).expect("the extension ran on the server's thread");
    let seen = seen.lock().map(|s| s.clone()).unwrap_or_default();
    let elsewhere: Vec<_> = seen.iter().filter(|(_, id)| *id != thread).map(|(n, _)| *n).collect();
    assert!(elsewhere.is_empty(), "systems that ran on another thread than the server's: {elsewhere:?}");
    println!("{} probe runs, all on the server's thread", seen.len());
}

#[test]
fn a_stall_of_the_process_is_caught_up() {
    let frames: Arc<Mutex<Frames>> = Arc::default();
    let stalled = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let (f, st) = (Arc::clone(&frames), Arc::clone(&stalled));
    let extend: avalanche_server::AppExtension = Box::new(move |app: &mut App| {
        app.add_systems(Update, move |mut n: Local<u32>| {
            *n += 1;
            if *n == 60 && !st.swap(true, std::sync::atomic::Ordering::AcqRel) {
                std::thread::sleep(Duration::from_secs(2));
            }
        });
        app.add_systems(Last, move |timeline: Res<LocalTimeline>, stats: Res<ServerStats>| {
            if let Ok(mut f) = f.lock() {
                let now = (Instant::now(), timeline.tick().0);
                f.count += 1;
                f.clamped = stats.frames_clamped;
                f.first.get_or_insert(now);
                f.last = Some(now);
            }
        });
    });
    let server = avalanche_server::spawn(options(extend)).expect("the server starts");
    let count = || frames.lock().map_or(0, |f| f.count);
    wait_for("the stall and 60 frames after it", Duration::from_secs(20), || count() >= 120);
    let (ticks, real, clamped) = {
        let f = frames.lock().expect("the frames");
        let ((first_at, first_tick), (last_at, last_tick)) = (f.first.expect("a frame"), f.last.expect("a frame"));
        (last_tick - first_tick, last_at.duration_since(first_at), f.clamped)
    };
    server.stop();
    server.join().expect("the server ends");
    assert!(stalled.load(std::sync::atomic::Ordering::Acquire), "the stall ran");
    let expected = real.as_secs_f64() * f64::from(ssx_net_proto::TICK_HZ);
    println!("{ticks} ticks in {:.3} s ({expected:.1} by the real time), {clamped} clamped frames", real.as_secs_f64());
    assert_eq!(clamped, 0, "no frame clamped: the stall was caught up");
    assert!((f64::from(ticks) - expected).abs() <= 2.0, "{ticks} ticks, {expected:.1} by the real time");
}
