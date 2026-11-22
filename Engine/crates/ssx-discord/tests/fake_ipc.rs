#![cfg(unix)]

mod fake;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use fake::{Behaviour, FakeDiscord, Seen};
use ssx_discord::{Activity, ActivityType, AssetKey, Button, Client, Config, Text, Timing};

const ID: &str = "1571572936433995906";

fn quick() -> Timing {
    Timing {
        retry_first: Duration::from_millis(50),
        retry_max: Duration::from_millis(200),
        poll: Duration::from_millis(10),
        settle: Duration::from_millis(30),
        handshake: Duration::from_millis(300),
        write: Duration::from_millis(500),
        updates: 5,
        window: Duration::from_millis(1500),
    }
}

struct TestDir(PathBuf);

impl std::ops::Deref for TestDir {
    type Target = std::path::Path;
    fn deref(&self) -> &std::path::Path {
        &self.0
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn folder(name: &str) -> TestDir {
    let d = std::env::temp_dir().join(format!("sd-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    TestDir(d)
}

fn config(dir: &std::path::Path, timing: Timing) -> Config {
    let endpoints = (0..3).map(|n| dir.join(format!("discord-ipc-{n}"))).collect();
    Config { client_id: ID.into(), endpoints, timing }
}

fn activity(details: &str) -> Activity {
    let mut a = Activity::new(ActivityType::Playing);
    a.details = Text::new(details);
    a.state = Text::new("Grand Golliat");
    a.large_image = AssetKey::new("logo");
    a.push_button(Button::new("Join the Discord", "https://discord.com/invite/XrFGQEfwdJ").unwrap());
    a
}

const LONG: Duration = Duration::from_secs(5);

#[test]
fn the_presence_reaches_discord_after_the_handshake() {
    let dir = folder("basic");
    let fake = FakeDiscord::start(&dir.join("discord-ipc-0"), Behaviour::Normal);
    let client = Client::start(config(&dir, quick())).unwrap();
    let a = activity("Explore · Race It");
    client.set(a.clone());
    assert!(fake.wait_for(LONG, |f| f.activities().len() == 1));
    let seen = fake.seen();
    assert_eq!(seen[0], Seen::Connected);
    assert_eq!(seen[1], Seen::Handshake(serde_json::json!({ "v": 1, "client_id": ID })));
    let (_, p) = &fake.activities()[0];
    assert_eq!(p["cmd"], "SET_ACTIVITY");
    assert_eq!(p["nonce"], "1");
    assert_eq!(p["args"]["pid"], std::process::id());
    assert_eq!(p["args"]["activity"], a.to_json());
    client.set(a);
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(fake.activities().len(), 1);
    assert!(client.stop(LONG));
    assert!(fake.wait_for(LONG, |f| f.seen().last() == Some(&Seen::Disconnected)));
}

#[test]
fn a_later_endpoint_is_found_when_the_first_is_missing() {
    let dir = folder("second");
    let fake = FakeDiscord::start(&dir.join("discord-ipc-2"), Behaviour::Normal);
    let client = Client::start(config(&dir, quick())).unwrap();
    client.set(activity("In the menus"));
    assert!(fake.wait_for(LONG, |f| f.activities().len() == 1));
    drop(client);
}

#[test]
fn changes_settle_and_the_rate_limit_holds() {
    let dir = folder("rate");
    let fake = FakeDiscord::start(&dir.join("discord-ipc-0"), Behaviour::Normal);
    let t = quick();
    let client = Client::start(config(&dir, t)).unwrap();
    for i in 0..20 {
        client.set(activity(&format!("burst {i}")));
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(fake.wait_for(LONG, |f| f.activities().len() == 1));
    std::thread::sleep(t.settle * 3);
    assert_eq!(fake.activities().len(), 1);
    assert_eq!(fake.activities()[0].1["args"]["activity"]["details"], "burst 19");
    for i in 0..12 {
        client.set(activity(&format!("step {i}")));
        std::thread::sleep(t.settle + Duration::from_millis(15));
    }
    let last =
        |f: &FakeDiscord| f.activities().last().is_some_and(|(_, p)| p["args"]["activity"]["details"] == "step 11");
    assert!(fake.wait_for(LONG, last));
    let times: Vec<Instant> = fake.activities().iter().map(|(at, _)| *at).collect();
    const READ_SLACK: Duration = Duration::from_millis(100);
    let window = t.window - READ_SLACK;
    let gaps: Vec<u128> = times.windows(2).map(|w| w[1].duration_since(w[0]).as_millis()).collect();
    for (i, &t0) in times.iter().enumerate() {
        let within = times[i..].iter().filter(|&&x| x.duration_since(t0) < window).count();
        assert!(within <= t.updates, "{within} updates within {window:?} of update {i} (gaps, ms: {gaps:?})");
    }
    assert!(times.len() < 13, "{} updates", times.len());
    drop(client);
}

#[test]
fn a_discord_started_later_is_found() {
    let dir = folder("late");
    let client = Client::start(config(&dir, quick())).unwrap();
    client.set(activity("In the menus"));
    std::thread::sleep(Duration::from_millis(400));
    let started = Instant::now();
    let fake = FakeDiscord::start(&dir.join("discord-ipc-0"), Behaviour::Normal);
    assert!(fake.wait_for(LONG, |f| f.activities().len() == 1));
    assert!(started.elapsed() < quick().retry_max + Duration::from_millis(300), "{:?}", started.elapsed());
    drop(client);
}

#[test]
fn a_restarted_discord_gets_the_presence_again() {
    let dir = folder("restart");
    let fake = FakeDiscord::start(&dir.join("discord-ipc-0"), Behaviour::Normal);
    let client = Client::start(config(&dir, quick())).unwrap();
    let a = activity("Riding");
    client.set(a.clone());
    assert!(fake.wait_for(LONG, |f| f.activities().len() == 1));
    fake.kick();
    assert!(fake.wait_for(LONG, |f| f.activities().len() == 2));
    assert_eq!(fake.count(|s| *s == Seen::Connected), 2);
    assert_eq!(fake.activities()[1].1["args"]["activity"], a.to_json());
    drop(fake);
    let fake = FakeDiscord::start(&dir.join("discord-ipc-0"), Behaviour::Normal);
    assert!(fake.wait_for(LONG, |f| f.activities().len() == 1));
    drop(client);
}

#[test]
fn hostile_frames_end_the_connection_and_it_is_tried_again() {
    for (name, b) in [("big", Behaviour::Oversized), ("op", Behaviour::BadOpcode)] {
        let dir = folder(name);
        let fake = FakeDiscord::start(&dir.join("discord-ipc-0"), b);
        let client = Client::start(config(&dir, quick())).unwrap();
        client.set(activity("x y"));
        assert!(fake.wait_for(LONG, |f| f.count(|s| *s == Seen::Connected) >= 3), "{name}");
        assert!(fake.count(|s| *s == Seen::Disconnected) >= 2, "{name}");
        drop(client);
    }
}

#[test]
fn ping_is_answered_with_its_payload() {
    let dir = folder("ping");
    let fake = FakeDiscord::start(&dir.join("discord-ipc-0"), Behaviour::PingAfterReady);
    let client = Client::start(config(&dir, quick())).unwrap();
    assert!(fake.wait_for(LONG, |f| f.seen().contains(&Seen::Pong(b"{\"ping\":42}".to_vec()))));
    drop(client);
}

#[test]
fn a_close_with_a_reason_is_retried_with_growing_delays() {
    let dir = folder("close");
    let fake = FakeDiscord::start(&dir.join("discord-ipc-0"), Behaviour::Close(4000, "Invalid Client ID".into()));
    let client = Client::start(config(&dir, quick())).unwrap();
    std::thread::sleep(Duration::from_millis(1200));
    let tries = fake.count(|s| *s == Seen::Connected);
    assert!((4..=9).contains(&tries), "{tries} connections in 1.2 s");
    assert!(fake.activities().is_empty());
    drop(client);
}

#[test]
fn no_ready_ends_the_connection_after_the_handshake_time() {
    let dir = folder("noready");
    let fake = FakeDiscord::start(&dir.join("discord-ipc-0"), Behaviour::NoReady);
    let client = Client::start(config(&dir, quick())).unwrap();
    client.set(activity("x y"));
    assert!(fake.wait_for(LONG, |f| f.count(|s| *s == Seen::Connected) >= 2));
    assert!(fake.activities().is_empty());
    drop(client);
}

#[test]
fn a_refused_activity_is_not_sent_again() {
    let dir = folder("refuse");
    let fake = FakeDiscord::start(&dir.join("discord-ipc-0"), Behaviour::RefuseActivities);
    let client = Client::start(config(&dir, quick())).unwrap();
    client.set(activity("x y"));
    assert!(fake.wait_for(LONG, |f| f.activities().len() == 1));
    std::thread::sleep(Duration::from_millis(400));
    assert_eq!(fake.activities().len(), 1);
    assert_eq!(fake.count(|s| *s == Seen::Connected), 1);
    drop(client);
}

#[test]
fn set_never_waits_for_discord() {
    let dir = folder("stall");
    let path = dir.join("discord-ipc-0");
    let _listener = std::os::unix::net::UnixListener::bind(&path).unwrap();
    let client = Client::start(config(&dir, quick())).unwrap();
    let mut worst = Duration::ZERO;
    for i in 0..5_000 {
        let started = Instant::now();
        client.set(activity(&format!("frame {i}")));
        worst = worst.max(started.elapsed());
    }
    assert!(worst < Duration::from_millis(20), "the slowest set took {worst:?}");
    assert!(client.stop(LONG));
}

#[test]
fn an_invalid_application_id_is_refused() {
    let dir = folder("id");
    let mut c = config(&dir, quick());
    c.client_id = "not-a-snowflake".into();
    assert!(Client::start(c).is_err());
}
