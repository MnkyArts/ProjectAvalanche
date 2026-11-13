use core::time::Duration;
use std::sync::Arc;
use std::time::Instant;

use glam::{Quat, Vec3};
use ssx_formats::ghost::{POSE_BLOCK_SIZE, PoseBlock, pack_half, pack_vector, unpack_half};
use ssx_gameplay::round::CycleRules;
use ssx_gameplay::validate::{Action, Check, Limits, Policy, Start, TICKS_PER_STEP};
use ssx_net::client::{PoseOutbox, ServerSaid};
use ssx_net::harness::{Conditions, Peers, round};
use ssx_net::server::{ServerStats, ValidationConfig, ValidationLog};
use ssx_net_proto::{
    ClientControl, GoodbyeReason, InstanceState, Mode, NetPose, PoseFlags, ReportKind, StandingState, TICK_HZ,
};
use ssx_rules::course::Course;
use ssx_rules::events::EventType;
use ssx_rules::intro::START_FIRST;
use ssx_rules::subset::TuningLimits;
use ssx_rules::triggers::{TriggerScope, TriggerVolume, VolumeShape};

const RULES: CycleRules =
    CycleRules { min_players: 2, lobby_wait: 0, load_timeout: 300, countdown: 90, round_limit: 1_800, results: 60 };

const JUMP: f64 = 21.0;

fn limits() -> Limits {
    let tuning = TuningLimits {
        fall: 1.17,
        horizontal: 1.27,
        spin: 0.26,
        flip: 0.23,
        grind_spin: 0.11,
        trick_point_bonus: 1.03,
        heli_shut_out: 1200.0,
    };
    let end = TriggerVolume {
        name: "end".into(),
        type_text: String::new(),
        groups: 0,
        interacts: 0,
        shape: VolumeShape::Box { half_extents: Vec3::splat(4.0) },
        centre: Vec3::new(0.0, -100.0, 500.0),
        rotation: Quat::IDENTITY,
        object: 0,
        instance: 0,
        placement: 0,
        scope: TriggerScope::Track,
    };
    Limits {
        drop: "test".into(),
        event_type: Some(EventType::RaceClock),
        splits: Course { start_y: 5.0, end_y: -100.0 }.splits(),
        starts: vec![Vec3::ZERO],
        end: vec![end],
        fall: 1.18,
        horizontal: 1.5,
        split_depth: 4.7,
        tuning,
        start: Start {
            handover: JUMP,
            slot: Vec3::ZERO,
            rotation: Quat::IDENTITY,
            place: Vec3::ZERO,
            margin: 0.18,
            first: [Vec3::ZERO; START_FIRST],
            first_margins: [0.18; START_FIRST],
        },
        rider_starts: Vec::new(),
        gates: Vec::new(),
        time_limit_ms: None,
        time_up: ssx_rules::gamemode::TimeUp::Fail,
        map: false,
    }
}

fn validated(action: Action) -> impl FnOnce(&mut bevy_app::App) {
    move |app| {
        app.insert_resource(ValidationConfig {
            loader: Arc::new(|_: &ssx_net_proto::instance::Level, _: Mode| Ok(limits())),
            policy: Policy { actions: [action; 6] },
            generation: Default::default(),
        });
    }
}

fn block(pos: Vec3) -> PoseBlock {
    let mut raw = [0u8; POSE_BLOCK_SIZE];
    raw[12..16].copy_from_slice(&1.0f32.to_be_bytes());
    raw[0x48..0x4C].copy_from_slice(&1.0f32.to_be_bytes());
    let mut b = PoseBlock::parse(&raw).expect("240 bytes parse");
    b.trajectory_position = pos.to_array();
    b
}

fn sample(peers: &mut ssx_net::harness::Peers, c: usize, pos: Vec3) {
    let time = round::server_time(peers, c).expect("a time");
    let later = |p: &mut ssx_net::harness::Peers| round::server_time(p, c).is_some_and(|t| t >= time + 2.0);
    let pose = NetPose::at(time, PoseFlags::default(), block(pos))
        .and_then(|p| p.with_execution(round::execution(peers, c)))
        .expect("a sample");
    let before = peers.clients[c].world().resource::<PoseOutbox>().sent;
    peers.clients[c].world_mut().resource_mut::<PoseOutbox>().next = Some(pose);
    let sent = |p: &mut ssx_net::harness::Peers| p.clients[c].world().resource::<PoseOutbox>().sent > before;
    assert!(peers.steps_for(Duration::from_secs(5), sent), "client {c}'s sample goes out");
    assert!(peers.steps_for(Duration::from_secs(5), later), "two ticks pass");
}

fn running(peers: &mut ssx_net::harness::Peers) -> u16 {
    for c in 0..2 {
        round::send(peers, c, [ClientControl::Appearance(round::look()), ClientControl::Loaded { round: 0 }]);
    }
    let told = |p: &mut ssx_net::harness::Peers| (0..2).all(|c| round::start(p, c).is_some());
    assert!(peers.steps_for(Duration::from_secs(20), told), "the start is told");
    let (r, t) = round::start(peers, 0).expect("the start");
    for c in 0..2 {
        round::send(peers, c, [ClientControl::StartAck { round: r }]);
    }
    let reached = |p: &mut ssx_net::harness::Peers| round::server_time(p, 0).is_some_and(|n| n >= f64::from(t) + JUMP);
    assert!(peers.steps_for(Duration::from_secs(30), reached), "the hand-over's tick");
    for c in 0..2 {
        let time = round::server_time(peers, c).expect("a time");
        let at = |e: u32| {
            NetPose::at(time, PoseFlags::default(), block(Vec3::ZERO))
                .and_then(|p| p.with_execution(e))
                .expect("a sample")
        };
        let (sample, second) = (Box::new(at(JUMP as u32)), Box::new(at(JUMP as u32 + 2)));
        round::send(peers, c, [ClientControl::Ready { round: r, sample, second }]);
    }
    r
}

fn ride_and_jump(peers: &mut ssx_net::harness::Peers) {
    for _ in 0..6 {
        for c in 0..2 {
            sample(peers, c, Vec3::ZERO);
        }
    }
    sample(peers, 1, Vec3::new(0.0, 0.0, 20.0));
}

#[test]
fn a_teleport_invalidates_the_run_and_the_rider_rides_on() {
    let mut peers = round::riders_with(2, Mode::Race, RULES, None, validated(Action::Invalidate)).expect("two riders");
    let r = running(&mut peers);
    ride_and_jump(&mut peers);
    let told = |p: &mut ssx_net::harness::Peers| p.clients[1].world().resource::<ServerSaid>().invalidated.is_some();
    assert!(peers.steps_for(Duration::from_secs(10), told), "the cheater is told");
    assert_eq!(peers.clients[1].world().resource::<ServerSaid>().invalidated, Some((r, Check::Step)));
    assert_eq!(peers.clients[0].world().resource::<ServerSaid>().invalidated, None, "the honest rider is not");
    let cheater = round::rider(&peers, 1).expect("rider 1's number");
    let marked = |p: &mut ssx_net::harness::Peers| {
        round::standings(p, 0).is_some_and(|(_, rows)| {
            rows.iter().any(|row| row.rider == cheater && row.state == StandingState::Invalidated)
        })
    };
    assert!(peers.steps_for(Duration::from_secs(10), marked), "every rider sees the row invalidated");
    let (_, rows) = round::standings(&peers, 0).expect("the standings");
    let row = rows.iter().find(|row| row.rider == cheater).expect("the cheater's row");
    assert_eq!((row.invalid, rows.last().map(|x| x.rider)), (Some(Check::Step), Some(cheater)), "ranked last");
    let rep = round::report(&peers, 1, r, ReportKind::Split, Some(0), 100);
    round::send(&mut peers, 1, [ClientControl::Report(Box::new(rep))]);
    let dropped = |p: &mut ssx_net::harness::Peers| p.server.world().resource::<ServerStats>().reports_dropped > 0;
    assert!(peers.steps_for(Duration::from_secs(10), dropped), "the report is dropped");
    assert!(peers.connected(1), "the rider rides on");
    let stats = peers.server.world().resource::<ServerStats>();
    let step = stats.validation[Check::Step.index()];
    assert!(step.found >= 1, "{step:?}");
    assert_eq!((step.invalidated, step.kicked, step.warned), (1, 0, 0));
    assert_eq!(stats.kicked, 0);
}

#[test]
fn the_kick_policy_tells_the_rider_the_check() {
    let mut peers = round::riders_with(2, Mode::Race, RULES, None, validated(Action::Kick)).expect("two riders");
    running(&mut peers);
    ride_and_jump(&mut peers);
    let gone = |p: &mut ssx_net::harness::Peers| p.clients[1].world().resource::<ServerSaid>().goodbye.is_some();
    assert!(peers.steps_for(Duration::from_secs(10), gone), "the cheater is told why it leaves");
    let goodbye = peers.clients[1].world().resource::<ServerSaid>().goodbye.clone().expect("a goodbye");
    assert_eq!(goodbye.0, GoodbyeReason::Validation);
    assert!(goodbye.1.contains(Check::Step.reason()), "{goodbye:?}");
    assert!(peers.connected(0), "the honest rider stays");
    let ended = |p: &mut ssx_net::harness::Peers| p.server.world().resource::<ServerStats>().rides_validated > 0;
    assert!(peers.steps_for(Duration::from_secs(10), ended), "the cheater's run ends");
    let step = peers.server.world().resource::<ServerStats>().validation[Check::Step.index()];
    assert_eq!((step.kicked, step.invalidated), (1, 1));
    assert!(step.made > 0 && step.found >= 1, "{step:?}");
}

#[test]
fn a_warning_only_counts() {
    let mut peers = round::riders_with(2, Mode::Race, RULES, None, validated(Action::Warn)).expect("two riders");
    running(&mut peers);
    ride_and_jump(&mut peers);
    let found = |p: &mut ssx_net::harness::Peers| {
        p.server.world().resource::<ServerStats>().validation[Check::Step.index()].warned > 0
    };
    assert!(peers.steps_for(Duration::from_secs(10), found), "the finding is counted");
    assert_eq!(peers.clients[1].world().resource::<ServerSaid>().invalidated, None);
    assert!(peers.connected(1));
}

#[test]
fn the_samples_after_a_runs_end_are_not_kept_for_a_ready() {
    let mut peers = round::riders_with(2, Mode::Race, RULES, None, validated(Action::Warn)).expect("two riders");
    let r = running(&mut peers);
    for c in 0..2 {
        sample(&mut peers, c, Vec3::ZERO);
    }
    let (_, t) = round::start(&peers, 0).expect("the start");
    let rep = round::report(&peers, 0, r, ReportKind::Failed, None, round::clock(&peers, 0, t, 0));
    round::send(&mut peers, 0, [ClientControl::Report(Box::new(rep))]);
    let over = |p: &mut ssx_net::harness::Peers| p.server.world().resource::<ServerStats>().rides_validated > 0;
    assert!(peers.steps_for(Duration::from_secs(10), over), "rider 0's run is over");
    let received = peers.server.world().resource::<ServerStats>().poses_received;
    for _ in 0..4 {
        for c in 0..2 {
            sample(&mut peers, c, Vec3::ZERO);
        }
    }
    peers.run_until(Duration::from_millis(300), |_| false);
    assert_eq!(round::instance(&peers, 0).map(|i| (i.0, i.1)), Some((InstanceState::Running, r)), "the round runs");
    let stats = peers.server.world().resource::<ServerStats>();
    assert!(stats.poses_received >= received + 8, "the samples came: {} then {received}", stats.poses_received);
    assert_eq!(
        (stats.samples_before_ready, stats.samples_before_ready_dropped, stats.samples_before_ready_most),
        (0, 0, 0),
        "no sample is kept for a `Ready`"
    );
}

#[test]
fn an_earlier_runs_sample_before_ready_is_not_judged() {
    let mut peers = round::riders_with(2, Mode::Race, RULES, None, validated(Action::Invalidate)).expect("two riders");
    for c in 0..2 {
        round::send(&mut peers, c, [ClientControl::Appearance(round::look()), ClientControl::Loaded { round: 0 }]);
    }
    let told = |p: &mut ssx_net::harness::Peers| (0..2).all(|c| round::start(p, c).is_some());
    assert!(peers.steps_for(Duration::from_secs(20), told), "the start is told");
    let (r, t) = round::start(&peers, 0).expect("the start");
    for c in 0..2 {
        round::send(&mut peers, c, [ClientControl::StartAck { round: r }]);
    }
    let stamped = |peers: &ssx_net::harness::Peers, c: usize, execution: u32| {
        let time = round::server_time(peers, c).expect("a time");
        NetPose::at(time, PoseFlags::default(), block(Vec3::ZERO))
            .and_then(|p| p.with_execution(execution))
            .expect("a sample")
    };
    let before = peers.clients[0].world().resource::<PoseOutbox>().sent;
    assert!(round::server_time(&peers, 0).is_some_and(|n| n < f64::from(t)), "in the countdown");
    peers.clients[0].world_mut().resource_mut::<PoseOutbox>().next = Some(stamped(&peers, 0, 500));
    let kept = |p: &mut ssx_net::harness::Peers| p.server.world().resource::<ServerStats>().samples_before_ready > 0;
    assert!(peers.steps_for(Duration::from_secs(5), kept), "the earlier run's sample is kept for `Ready`");
    assert_eq!(peers.clients[0].world().resource::<PoseOutbox>().sent, before + 1);
    let reached = |p: &mut ssx_net::harness::Peers| round::server_time(p, 0).is_some_and(|n| n >= f64::from(t) + JUMP);
    assert!(peers.steps_for(Duration::from_secs(30), reached), "the hand-over's tick");
    for c in 0..2 {
        let (sample, second) =
            (Box::new(stamped(&peers, c, JUMP as u32)), Box::new(stamped(&peers, c, JUMP as u32 + 2)));
        round::send(&mut peers, c, [ClientControl::Ready { round: r, sample, second }]);
    }
    let taken = |p: &mut ssx_net::harness::Peers| {
        let s = p.server.world().resource::<ServerStats>();
        s.samples_before_ready_other_run > 0 || Check::ALL.iter().any(|c| s.validation[c.index()].found > 0)
    };
    assert!(peers.steps_for(Duration::from_secs(10), taken), "`Ready` takes the kept sample");
    peers.run_until(Duration::from_millis(300), |_| false);
    let stats = peers.server.world().resource::<ServerStats>();
    assert!(Check::ALL.iter().all(|c| stats.validation[c.index()].found == 0), "{:?}", stats.validation);
    assert_eq!((stats.samples_before_ready, stats.samples_before_ready_other_run), (1, 1));
    assert_eq!(peers.clients[0].world().resource::<ServerSaid>().invalidated, None);
}

#[test]
fn a_server_riders_look_is_validated_as_its_stand_in() {
    for known in [true, false] {
        let clock = stand_in_run(known);
        assert_eq!(clock == 0, known, "the stand-in known: {known}; clock findings {clock}");
    }
}

fn stand_in_run(known: bool) -> u64 {
    use ssx_gameplay::validate::RiderStart;
    use ssx_net::server::StandIns;
    const SERVER_RIDER: u32 = 0x5A3E_0001;
    const OWN: f64 = 40.0;
    let stand_in = ssx_formats::attrib::fnv1(b"kaori");
    let setup = move |app: &mut bevy_app::App| {
        let mut rules = limits();
        rules.rider_starts =
            vec![RiderStart { character: stand_in, start: Start { handover: OWN, ..rules.start }, every_path: true }];
        app.insert_resource(ValidationConfig {
            loader: Arc::new(move |_: &ssx_net_proto::instance::Level, _: Mode| Ok(rules.clone())),
            policy: Policy { actions: [Action::Warn; 6] },
            generation: Default::default(),
        });
        if known {
            app.insert_resource(StandIns([(SERVER_RIDER, stand_in)].into()));
        }
    };
    let mut peers = round::riders_with(2, Mode::Race, RULES, None, setup).expect("two riders");
    let looks = [round::look(), ssx_net_proto::Appearance { character: SERVER_RIDER, gear: 0 }];
    for (c, look) in looks.into_iter().enumerate() {
        round::send(&mut peers, c, [ClientControl::Appearance(look), ClientControl::Loaded { round: 0 }]);
    }
    let told = |p: &mut ssx_net::harness::Peers| (0..2).all(|c| round::start(p, c).is_some());
    assert!(peers.steps_for(Duration::from_secs(20), told), "the start is told");
    let (r, t) = round::start(&peers, 0).expect("the start");
    for c in 0..2 {
        round::send(&mut peers, c, [ClientControl::StartAck { round: r }]);
    }
    for (c, handover) in [(0, JUMP), (1, OWN)] {
        let reached =
            |p: &mut ssx_net::harness::Peers| round::server_time(p, c).is_some_and(|n| n >= f64::from(t) + handover);
        assert!(peers.steps_for(Duration::from_secs(30), reached), "rider {c}'s hand-over");
        let time = round::server_time(&peers, c).expect("a time");
        let at = |e: u32| {
            NetPose::at(time, PoseFlags::default(), block(Vec3::ZERO))
                .and_then(|p| p.with_execution(e))
                .expect("a sample")
        };
        let (sample, second) = (Box::new(at(handover as u32)), Box::new(at(handover as u32 + 2)));
        round::send(&mut peers, c, [ClientControl::Ready { round: r, sample, second }]);
    }
    ride_and_jump(&mut peers);
    let jumped = |p: &mut ssx_net::harness::Peers| {
        p.server.world().resource::<ServerStats>().validation[Check::Step.index()].found > 0
    };
    assert!(peers.steps_for(Duration::from_secs(10), jumped), "rider 1's run is validated (known: {known})");
    peers.server.world().resource::<ServerStats>().validation[Check::Clock.index()].found
}

const PROOF_LINKS: Conditions =
    Conditions { latency: Duration::from_millis(50), jitter: Duration::from_millis(20), loss: 0.0 };

const STALL: Duration = Duration::from_millis(2_700);

const SPEED: f32 = 0.4;

struct Run {
    handover_at: Instant,
    offered: Option<u32>,
}

impl Run {
    fn execution(&self) -> u32 {
        JUMP as u32 + (self.handover_at.elapsed().as_secs_f64() * f64::from(TICK_HZ)) as u32
    }
}

fn ridden(e: u32) -> (Vec3, Vec3) {
    let off = JUMP as u32 + START_FIRST as u32 + 1;
    if e < off {
        return (Vec3::ZERO, Vec3::ZERO);
    }
    let vel = Vec3::new(0.0, 0.0, SPEED);
    (vel * TICKS_PER_STEP * (e - off) as f32, vel)
}

fn moving_block(pos: Vec3, vel: Vec3) -> PoseBlock {
    let mut b = block(pos);
    if vel != Vec3::ZERO {
        b.speed = pack_half(vel.length() * 1.001) | 0x8000;
        let s = unpack_half(b.speed & 0x7FFF);
        b.velocity = pack_vector(vel.to_array(), [s; 3]) & 0x3FFF_FFFF;
    }
    b
}

fn ride(peers: &mut Peers, runs: &mut [Run]) {
    for (c, run) in runs.iter_mut().enumerate() {
        let e = run.execution();
        if run.offered == Some(e) {
            continue;
        }
        let Some(time) = round::server_time(peers, c) else { continue };
        let (pos, vel) = ridden(e);
        let pose = NetPose::at(time, PoseFlags::default(), moving_block(pos, vel))
            .and_then(|p| p.with_execution(e))
            .expect("a sample");
        peers.clients[c].world_mut().resource_mut::<PoseOutbox>().next = Some(pose);
        run.offered = Some(e);
    }
}

#[test]
fn an_honest_run_through_a_long_server_stall_is_not_flagged() {
    let mut peers =
        round::riders_with(2, Mode::Race, RULES, Some(PROOF_LINKS), validated(Action::Invalidate)).expect("two riders");
    running(&mut peers);
    let mut runs: Vec<Run> = (0..2).map(|_| Run { handover_at: Instant::now(), offered: None }).collect();
    peers.steps_for(Duration::from_secs(2), |p| {
        ride(p, &mut runs);
        false
    });
    let sent = |p: &Peers| (0..2).map(|c| p.clients[c].world().resource::<PoseOutbox>().sent).sum::<u64>();
    let before = sent(&peers);
    peers.stall_server(STALL, |p| ride(p, &mut runs));
    let during = sent(&peers) - before;
    peers.steps_for(Duration::from_secs(2), |p| {
        ride(p, &mut runs);
        false
    });
    let stats = peers.server.world().resource::<ServerStats>();
    assert!(
        stats.frame_ms_most >= 2_700,
        "the stall was one frame of the server: its longest {} ms",
        stats.frame_ms_most
    );
    assert!(during >= 2 * 60, "the riders sent through the stall: {during} samples");
    let findings: Vec<&str> = peers
        .server
        .world()
        .resource::<ValidationLog>()
        .events
        .iter()
        .filter(|e| e.check.is_some())
        .map(|e| e.text.as_str())
        .collect();
    assert!(findings.is_empty(), "honest findings: {findings:#?}");
    for (check, outcome) in Check::ALL.iter().zip(stats.validation) {
        assert_eq!(outcome.found, 0, "{check:?}: {outcome:?}");
    }
    for (c, client) in peers.clients.iter().enumerate().take(2) {
        assert_eq!(client.world().resource::<ServerSaid>().invalidated, None, "rider {c}");
    }
}

#[test]
fn moved_records_are_loaded_for_the_next_round_not_the_running_one() {
    use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
    let loads = Arc::new(AtomicUsize::new(0));
    let generation = Arc::new(AtomicU64::new(0));
    let (counted, moved) = (Arc::clone(&loads), Arc::clone(&generation));
    let setup = move |app: &mut bevy_app::App| {
        app.insert_resource(ValidationConfig {
            loader: Arc::new(move |_: &ssx_net_proto::instance::Level, _: Mode| {
                counted.fetch_add(1, Ordering::SeqCst);
                Ok(limits())
            }),
            policy: Policy { actions: [Action::Warn; 6] },
            generation: moved,
        });
    };
    let rules = CycleRules { round_limit: 360, results: 30, ..RULES };
    let mut peers = round::riders_with(2, Mode::Race, rules, None, setup).expect("two riders");
    let r = running(&mut peers);
    assert_eq!(loads.load(Ordering::SeqCst), 1, "round {r}'s limits");
    generation.fetch_add(1, Ordering::SeqCst);
    peers.run_until(Duration::from_secs(2), |_| false);
    assert_eq!(round::start(&peers, 0).map(|s| s.0), Some(r), "the round runs on");
    assert_eq!(loads.load(Ordering::SeqCst), 1, "the running round keeps the limits it started with");
    let next = |_: &mut ssx_net::harness::Peers| loads.load(Ordering::SeqCst) >= 2;
    assert!(peers.steps_for(Duration::from_secs(30), next), "the next round loads the moved records' limits");
    assert_eq!(loads.load(Ordering::SeqCst), 2);
}
