mod common;

use core::time::Duration;
use std::net::Ipv4Addr;

use bevy_ecs::message::Messages;
use common::*;
use ssx_net::client::{ChatBody, ChatFrom, RemotePoseReceived};
use ssx_net::harness::{Conditions, Io, Peers};
use ssx_net::lightyear::netcode::ConnectToken;
use ssx_net::lightyear::prelude::{Authentication, Disconnect, LocalTimeline, RoomAllocator, Rooms};
use ssx_net::limits::LEAVE_GRACE;
use ssx_net::server::{ChatOutbox, KickReason, Leaving, Lobby, PlayerSession, RiderLink, TicketRefusal};
use ssx_net::{PoseOutbox, RemotePoses, ServerSaid, ServerStats, Sessions};
use ssx_net_proto::keys::{PlayerKey, PlayerSecret};
use ssx_net_proto::ticket::SessionTicket;
use ssx_net_proto::{GoodbyeReason, RiderId, ServerChat, netcode_protocol_id};

fn session(peers: &mut Peers, c: usize) -> Option<PlayerSession> {
    let link = peers.server_link(c)?;
    peers.server.world().get::<PlayerSession>(link).cloned()
}

fn rider(peers: &mut Peers, c: usize) -> Option<RiderId> {
    let link = peers.server_link(c)?;
    peers.server.world().get::<RiderLink>(link).map(RiderLink::rider)
}

fn player() -> PlayerKey {
    PlayerSecret::generate().public()
}

fn goodbye(peers: &Peers, c: usize) -> Option<GoodbyeReason> {
    peers.clients[c].world().resource::<ServerSaid>().goodbye.as_ref().map(|(r, _)| *r)
}

fn told_and_gone(peers: &mut Peers, c: usize, reason: GoodbyeReason) -> bool {
    peers.steps_for(LEAVE_GRACE + Duration::from_secs(3), |p| goodbye(p, c) == Some(reason) && !p.connected(c))
}

fn kicked_for(peers: &Peers, reason: KickReason) -> bool {
    peers.server.world().resource::<ServerStats>().kick_reasons.contains(&reason)
}

#[test]
fn issued_tokens_connect_into_the_lobby() {
    let mut peers = Peers::new(Io::Crossbeam, None, FRAME).expect("server");
    let (a, b) = (player(), player());
    let ca = peers.add_player(a, "Elise").expect("a");
    let cb = peers.add_player(b, "Mac").expect("b");
    assert!(peers.steps_for(Duration::from_secs(10), |p| (0..2).all(|c| p.connected(c) && p.synced(c))));
    assert!(ready(&mut peers, &[ca, cb], Duration::from_secs(10)));
    let lobby = peers.server.world().resource::<Lobby>().0;
    for (c, key, name) in [(ca, a, "Elise"), (cb, b, "Mac")] {
        let s = session(&mut peers, c).expect("a session");
        assert_eq!((s.player, s.id, s.name.as_str()), (key, key.id(), name));
        let link = peers.server_link(c).expect("link");
        assert!(peers.server.world().get::<Rooms>(link).is_some_and(|r| r.contains_room(lobby)));
    }
    let p = pose(server_tick(&peers).max(1), 3);
    let a_rider = rider(&mut peers, ca).expect("a rider");
    assert!(peers.send_pose(ca, &p, Duration::from_secs(2)), "the sample is sent");
    assert!(peers.steps_for(Duration::from_secs(2), |peers| {
        peers.clients[cb].world().resource::<RemotePoses>().riders.get(&a_rider).is_some_and(|r| r.pose == p)
    }));
    let stats = peers.server.world().resource::<ServerStats>().clone();
    assert_eq!((stats.sessions, stats.kicked), (2, 0), "{stats:?}");
    let sessions = peers.server.world().resource::<Sessions>();
    assert_eq!((sessions.reserved(), sessions.held()), (0, 2), "both tickets claimed");
}

#[test]
fn every_ticket_refusal_at_the_connect() {
    let mut peers = Peers::new(Io::Crossbeam, None, FRAME).expect("server");
    let missing = peers.add_client_with(77, peers.auth(77)).expect("client");
    let p = player();
    let (first, first_id) = peers.issue(p, "Kaori").expect("first");
    let (second, second_id) = peers.issue(p, "Kaori").expect("second");
    let revoked = peers.add_client_with(first_id, first).expect("client");
    let kept = peers.add_client_with(second_id, second).expect("client");
    let (foreign, foreign_id) = peers.issue_from(player(), "Zoe", Ipv4Addr::new(192, 0, 2, 1).into()).expect("issue");
    let from_elsewhere = peers.add_client_with(foreign_id, foreign).expect("client");
    let (_, victim_id) = peers.issue(player(), "Psymon").expect("issue");
    let (join_id, _, victim, _) =
        peers.server.world().resource::<Sessions>().tickets().into_iter().find(|t| t.1 == victim_id).expect("ticket");
    let forged = ConnectToken::build(peers.server_addr, netcode_protocol_id(), 4242, peers.key())
        .user_data(SessionTicket { join_id, player: victim }.encode())
        .generate()
        .expect("token");
    let mismatched = peers.add_client_with(4242, Authentication::Token(forged)).expect("client");
    for (c, refusal) in [
        (missing, TicketRefusal::Missing),
        (revoked, TicketRefusal::Unknown),
        (from_elsewhere, TicketRefusal::Address),
        (mismatched, TicketRefusal::Mismatch),
    ] {
        assert!(told_and_gone(&mut peers, c, GoodbyeReason::Ticket), "{refusal:?}");
        assert!(kicked_for(&peers, KickReason::Ticket(refusal)), "{refusal:?}");
    }
    assert!(peers.connected(kept), "the newer join's token connects");
    let (auth, id) = peers.issue(player(), "Moby").expect("issue");
    let once = peers.add_client_with(id, auth.clone()).expect("client");
    assert!(peers.steps_for(Duration::from_secs(5), |p| p.connected(once)));
    let entity = peers.client_entities[once];
    peers.clients[once].world_mut().trigger(Disconnect { entity });
    assert!(peers.steps_for(Duration::from_secs(3), |p| p.server_link(once).is_none()), "the server lets it go");
    let twice = peers.add_client_with(id, auth).expect("client");
    assert!(told_and_gone(&mut peers, twice, GoodbyeReason::Ticket));
    assert!(kicked_for(&peers, KickReason::Ticket(TicketRefusal::Used)));
    assert!(peers.connected(kept), "the other rider stays");
}

#[test]
fn disconnect_and_reconnect() {
    let mut peers = Peers::new(Io::Crossbeam, None, FRAME).expect("server");
    let p = player();
    let c0 = peers.add_player(p, "Elise").expect("client");
    assert!(peers.steps_for(Duration::from_secs(5), |peers| peers.connected(c0)));
    let first = rider(&mut peers, c0).expect("rider");
    let entity = peers.client_entities[c0];
    peers.clients[c0].world_mut().trigger(Disconnect { entity });
    assert!(peers.steps_for(Duration::from_secs(3), |peers| peers.server_link(c0).is_none()));
    let (_, ids) = peers.server.world().resource::<Sessions>().state_sizes();
    assert_eq!(ids, 0, "the client id is free");
    let c1 = peers.add_player(p, "Elise").expect("client");
    assert!(peers.steps_for(Duration::from_secs(5), |peers| peers.connected(c1)));
    assert_eq!(rider(&mut peers, c1), Some(first), "the lowest free rider number again");
    assert_eq!(session(&mut peers, c1).map(|s| s.player), Some(p));
    assert_eq!(peers.server.world().resource::<ServerStats>().sessions, 2);
}

#[test]
fn the_server_tells_a_riders_goodbye_from_a_timeout() {
    let mut peers = Peers::new(Io::Crossbeam, None, FRAME).expect("server");
    let left = peers.add_player(player(), "Alex").expect("client");
    let silent = peers.add_player(player(), "Elise").expect("client");
    let kept = peers.add_player(player(), "Zoe").expect("client");
    assert!(peers.steps_for(Duration::from_secs(5), |p| p.connected(left) && p.connected(silent) && p.connected(kept)));
    let entity = peers.client_entities[left];
    peers.clients[left].world_mut().trigger(Disconnect { entity });
    assert!(peers.steps_for(Duration::from_secs(3), |p| p.server_link(left).is_none()));
    let stats = peers.server.world().resource::<ServerStats>();
    assert_eq!((stats.left, stats.timed_out, stats.kicked), (1, 0, 0), "the goodbye");
    let timeout = Duration::from_secs(ssx_net::limits::CONNECTION_TIMEOUT_SECS as u64);
    let end = std::time::Instant::now() + timeout + Duration::from_secs(5);
    while std::time::Instant::now() < end && peers.server.world().resource::<ServerStats>().timed_out == 0 {
        std::thread::sleep(peers.frame);
        for (i, client) in peers.clients.iter_mut().enumerate() {
            if i != silent {
                client.update();
            }
        }
        peers.server.update();
    }
    let stats = peers.server.world().resource::<ServerStats>();
    assert_eq!((stats.left, stats.timed_out, stats.kicked), (1, 1, 0), "the timeout");
    assert!(peers.server_link(silent).is_none() && peers.connected(kept), "the other rider stays");
}

#[test]
fn the_same_key_replaces_the_older_connection() {
    let mut peers = Peers::new(Io::Crossbeam, None, FRAME).expect("server");
    let p = player();
    let old = peers.add_player(p, "Elise").expect("client");
    assert!(peers.steps_for(Duration::from_secs(5), |peers| peers.connected(old)));
    let new = peers.add_player(p, "Elise").expect("client");
    assert!(told_and_gone(&mut peers, old, GoodbyeReason::Replaced));
    assert!(peers.connected(new));
    assert!(kicked_for(&peers, KickReason::Replaced));
    let world = peers.server.world_mut();
    let sessions: Vec<PlayerKey> = world.query::<&PlayerSession>().iter(world).map(|s| s.player).collect();
    assert_eq!(sessions, vec![p], "one session of the key");
}

#[test]
fn rooms_bound_the_relay() {
    let mut peers = Peers::new(Io::Crossbeam, None, FRAME).expect("server");
    for name in ["A", "B", "C"] {
        peers.add_player(player(), name).expect("client");
    }
    assert!(peers.steps_for(Duration::from_secs(10), |p| (0..3).all(|c| p.connected(c) && p.synced(c))));
    assert!(ready(&mut peers, &[0, 1, 2], Duration::from_secs(10)));
    let elsewhere = peers.server.world_mut().resource_mut::<RoomAllocator>().allocate();
    put_in_room(&mut peers, vec![2], elsewhere);
    for (from, k) in [(0usize, 1u64), (2, 2)] {
        let p = pose(server_tick(&peers).max(1), k);
        assert!(peers.send_pose(from, &p, Duration::from_secs(1)), "client {from}'s sample is sent");
        peers.steps_for(Duration::from_secs(1), |_| false);
    }
    let heard = |peers: &Peers, c: usize| peers.clients[c].world().resource::<RemotePoses>().riders.len();
    assert_eq!(
        (heard(&peers, 0), heard(&peers, 1), heard(&peers, 2)),
        (0, 1, 0),
        "instance: 0 → 1; room of its own: 2"
    );
}

#[test]
fn channels_under_200ms_30ms_jitter_5_percent_loss() {
    let conditions = Conditions { latency: Duration::from_millis(200), jitter: Duration::from_millis(30), loss: 0.05 };
    let mut peers = Peers::new(Io::Crossbeam, Some(conditions), FRAME).expect("server");
    for name in ["A", "B"] {
        peers.add_player(player(), name).expect("client");
    }
    assert!(peers.steps_for(Duration::from_secs(20), |p| (0..2).all(|c| p.connected(c) && p.synced(c))));
    assert!(ready(&mut peers, &[0, 1], Duration::from_secs(20)));
    let a = rider(&mut peers, 0).expect("rider");
    let (mut sent, mut received, mut notices) = (Vec::new(), Vec::new(), Vec::new());
    for frame in 0..1380u64 {
        if frame % 4 == 0 && frame < 1200 {
            let p = pose(client_tick(&peers, 0), frame);
            sent.push(p.clone());
            peers.clients[0].world_mut().resource_mut::<PoseOutbox>().next = Some(p);
        }
        if frame % 30 == 0 && frame < 1200 {
            let text = format!("notice {}", frame / 30);
            notices.push(text.clone());
            peers.server.world_mut().resource_mut::<ChatOutbox>().0.push(ServerChat::server(&text).expect("text"));
        }
        peers.step();
        let mut messages = peers.clients[1].world_mut().resource_mut::<Messages<RemotePoseReceived>>();
        received.extend(messages.drain().filter(|m| m.rider == a).map(|m| m.pose));
    }
    assert!(peers.connected(0) && peers.connected(1), "no disconnect");
    let delivered = received.len() as f64 / sent.len() as f64;
    let stats = peers.server.world().resource::<ServerStats>().clone();
    let remote = peers.clients[1].world().resource::<RemotePoses>().riders.get(&a).map(|r| (r.samples, r.stale));
    println!(
        "poses: {} sent, {} received ({:.1} %); server {stats:?}; client (samples, stale) {remote:?}; sent by the outbox {}",
        sent.len(),
        received.len(),
        delivered * 100.0,
        peers.clients[0].world().resource::<PoseOutbox>().sent
    );
    assert!(delivered > 0.8 && delivered <= 1.0, "{delivered}");
    assert_eq!(remote.map(|r| r.0), Some(received.len() as u64), "every accepted sample was announced");
    for r in &received {
        assert!(sent.iter().any(|s| s.encode() == r.encode()), "byte-equal to a sent sample");
    }
    let mut ticks: Vec<u32> = received.iter().map(|r| r.tick()).collect();
    ticks.dedup();
    assert_eq!(ticks.len(), received.len(), "no sample twice");
    assert!(notices.len() < ssx_net::client::KEPT_LINES);
    for (c, client) in peers.clients.iter().enumerate().take(2) {
        let said: Vec<String> = client
            .world()
            .resource::<ssx_net::ChatInbox>()
            .lines
            .iter()
            .filter(|l| l.from == ChatFrom::Server)
            .filter_map(|l| match &l.body {
                ChatBody::Text(t) => Some(t.clone()),
                ChatBody::Quick(_) => None,
            })
            .collect();
        assert_eq!(said, notices, "client {c}");
    }
    let link = peers.server_link(0).expect("link");
    let now = peers.server.world().resource::<bevy_time::Time<bevy_time::Real>>().elapsed();
    peers.server.world_mut().entity_mut(link).insert(Leaving {
        reason: KickReason::Operator,
        text: "test".into(),
        since: now,
    });
    assert!(peers.steps_for(LEAVE_GRACE + Duration::from_secs(3), |p| !p.connected(0)));
    println!("the reason arrived before the disconnect: {:?}", goodbye(&peers, 0));
    assert!(peers.connected(1));
}

fn sample_every_frame(
    timeline: bevy_ecs::prelude::Res<LocalTimeline>,
    mut outbox: bevy_ecs::prelude::ResMut<PoseOutbox>,
) {
    let tick = timeline.tick().0;
    outbox.next = Some(pose(tick, u64::from(tick)));
}

#[test]
fn a_client_joining_a_running_server_is_not_kicked_for_its_first_samples() {
    let mut peers = Peers::new(Io::Crossbeam, None, FRAME).expect("server");
    peers.server.world_mut().resource_mut::<LocalTimeline>().apply_delta(5400);
    let late = peers.add_player(player(), "Late").expect("client");
    let watcher = peers.add_player(player(), "Watcher").expect("client");
    peers.clients[late].add_systems(bevy_app::Update, sample_every_frame);
    assert!(ready(&mut peers, &[late, watcher], Duration::from_secs(10)));
    let heard = peers.steps_for(Duration::from_secs(10), |p| {
        p.clients[watcher].world().resource::<RemotePoses>().riders.values().any(|r| r.samples >= 30)
    });
    let stats = peers.server.world().resource::<ServerStats>().clone();
    assert!(heard, "the watcher hears the late rider: {stats:?}");
    assert_eq!(stats.kicked, 0, "{stats:?}");
    assert!(peers.connected(late));
}
