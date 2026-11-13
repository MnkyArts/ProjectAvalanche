mod common;

use core::time::Duration;

use bevy_app::FixedUpdate;
use bevy_ecs::prelude::*;
use bevy_replicon::prelude::EntityCommandsDiffExt;
use common::*;
use ssx_net::bevy_replicon::client::ClientReplicationStats;
use ssx_net::harness::{Io, Peers, WireMeter};
use ssx_net::lightyear::prelude::{LocalTimeline, NetworkTarget, Replicate, RoomAllocator, Rooms};
use ssx_net::protocol::{Standings, StandingsDiff};
use ssx_net_proto::{StandingRow, StandingState};

const ROWS: usize = 16;

fn row(rider: u16, clock_ms: u32) -> StandingRow {
    StandingRow {
        rider,
        state: StandingState::Racing,
        split: Some(4),
        clock_ms,
        points: 0,
        descent_mm: 0,
        tick: clock_ms,
        fraction: 0,
        invalid: None,
    }
}

fn table() -> Standings {
    use bevy_replicon::prelude::Diffable;
    let mut t = Standings::new(100, 0);
    for k in 0..ROWS as u16 {
        t.apply_diff(&StandingsDiff::Set { slot: k as u8, row: row(k, 0) }).expect("16 rows");
    }
    t
}

#[derive(Component, Clone, Copy)]
struct Whole(bool);

fn change(standings: Query<(Entity, &Whole)>, timeline: Res<LocalTimeline>, mut commands: Commands) {
    let tick = timeline.tick().0;
    for (entity, whole) in &standings {
        let slot = (tick as usize % ROWS) as u8;
        if !whole.0 {
            let diff = StandingsDiff::Set { slot, row: row(u16::from(slot), tick.wrapping_mul(33)) };
            commands.entity(entity).apply_diff::<Standings>(diff);
            continue;
        }
        commands.entity(entity).apply_diff::<Standings>(StandingsDiff::Round((tick % 2) as u16));
        for k in 0..ROWS as u16 {
            let diff = StandingsDiff::Set { slot: k as u8, row: row(k, tick.wrapping_mul(33)) };
            commands.entity(entity).apply_diff::<Standings>(diff);
        }
    }
}

fn three_riders() -> (Peers, [Entity; 3], Rooms3) {
    let mut peers = Peers::new(Io::Crossbeam, None, FRAME).expect("server");
    for id in 1..=3 {
        peers.add_client(id).expect("client");
    }
    assert!(peers.steps_for(Duration::from_secs(10), |p| (0..3).all(|i| p.connected(i))));
    let rooms = {
        let mut a = peers.server.world_mut().resource_mut::<RoomAllocator>();
        Rooms3 { a: a.allocate(), b: a.allocate(), own: a.allocate() }
    };
    let links = [0, 1, 2].map(|c| peers.server_link(c).expect("link"));
    let member = [vec![rooms.a], vec![rooms.a, rooms.b], vec![rooms.own]];
    for (link, ids) in links.iter().zip(member) {
        peers.server.world_mut().entity_mut(*link).insert(Rooms::from(ids.into_iter()));
    }
    for client in peers.clients.iter_mut().take(3) {
        client.world_mut().init_resource::<ClientReplicationStats>();
    }
    (peers, links, rooms)
}

#[derive(Clone, Copy)]
struct Rooms3 {
    a: ssx_net::lightyear::prelude::RoomId,
    b: ssx_net::lightyear::prelude::RoomId,
    own: ssx_net::lightyear::prelude::RoomId,
}

fn spawn_in_room(peers: &mut Peers, room: ssx_net::lightyear::prelude::RoomId, component: impl Bundle) {
    peers.server.world_mut().spawn((Replicate::to_clients(NetworkTarget::All), Rooms::single(room), component));
}

fn client_tables<C: Component + Clone>(peers: &mut Peers, client: usize) -> Vec<C> {
    let world = peers.clients[client].world_mut();
    world.query::<&C>().iter(world).cloned().collect()
}

fn stats(peers: &Peers, client: usize) -> ClientReplicationStats {
    *peers.clients[client].world().resource::<ClientReplicationStats>()
}

#[test]
fn diff_replication_and_rooms() {
    let (mut peers, _links, rooms) = three_riders();
    peers.steps_for(Duration::from_secs(1), |_| false);
    let tick = |p: &Peers| p.server.world().resource::<LocalTimeline>().tick().0;
    let (before, t0) = ([0, 1, 2].map(|c| stats(&peers, c)), tick(&peers));
    peers.server.add_systems(FixedUpdate, change);
    spawn_in_room(&mut peers, rooms.a, (table(), Whole(false)));
    spawn_in_room(&mut peers, rooms.b, (table(), Whole(true)));
    let have =
        |p: &mut Peers| client_tables::<Standings>(p, 0).len() == 1 && client_tables::<Standings>(p, 1).len() == 2;
    assert!(peers.steps_for(Duration::from_secs(10), have), "riders 0 and 1 replicate their rooms' tables");
    let mid = [0, 1, 2].map(|c| stats(&peers, c));
    peers.steps_for(Duration::from_secs(5), |_| false);
    let (after, t1) = ([0, 1, 2].map(|c| stats(&peers, c)), tick(&peers));
    let ticks = t1.wrapping_sub(t0);
    let d = |c: usize| {
        (
            after[c].messages - mid[c].messages,
            after[c].bytes - mid[c].bytes,
            after[c].components_changed - mid[c].components_changed,
        )
    };
    let ((m0, b0, c0), (m1, b1, c1)) = (d(0), d(1));
    let s2 = |s: &ClientReplicationStats| (s.entities_changed, s.components_changed, s.despawns, s.mappings);
    println!(
        "over {ticks} server ticks from before the tables' spawn: rider 2 (outside both rooms) {} messages, {} bytes, \
         its entities, components, despawns and mappings {:?} → {:?}; in the 5 s window rider 0 (table A, a row by a \
         diff) {m0} messages, {b0} bytes, {c0} component changes; rider 1 (A and B, the whole table) {m1} messages, {b1} \
         bytes, {c1} component changes",
        after[2].messages - before[2].messages,
        after[2].bytes - before[2].bytes,
        s2(&before[2]),
        s2(&after[2]),
    );
    assert_eq!(s2(&after[2]), s2(&before[2]), "the rider outside the rooms receives no replication of them");
    assert!(client_tables::<Standings>(&mut peers, 2).is_empty(), "the rider outside the rooms has no table");
    assert_eq!(client_tables::<Standings>(&mut peers, 0).len(), 1, "rider 0 holds table A only");
    assert_eq!(client_tables::<Standings>(&mut peers, 1).len(), 2, "rider 1 holds both tables");
    for t in client_tables::<Standings>(&mut peers, 1) {
        assert_eq!(t.rows().len(), ROWS);
    }
    assert!(c0 > 0 && c1 > c0, "updates in the window: rider 0 {c0}, rider 1 {c1}");
    assert!(b0 < b1 - b0, "a row's diff ({b0} B) costs less than the whole table ({} B) in the same window", b1 - b0);
}

#[test]
fn idle_traffic_with_and_without_replication() {
    let mut peers = Peers::new(Io::Crossbeam, None, FRAME).expect("server");
    for id in 1..=2 {
        peers.add_client(id).expect("client");
    }
    assert!(peers.steps_for(Duration::from_secs(10), |p| (0..2).all(|i| p.connected(i))));
    let links = [peers.server_link(0).expect("link"), peers.server_link(1).expect("link")];
    peers.server.world_mut().entity_mut(links[1]).remove::<ssx_net::lightyear::prelude::ReplicationSender>();
    peers.steps_for(Duration::from_secs(2), |_| false);
    let meter = |peers: &Peers, link: Entity| peers.server.world().get::<WireMeter>(link).cloned().unwrap_or_default();
    let up = |peers: &Peers, c: usize| peers.client_meter(c);
    let before = (links.map(|l| meter(&peers, l)), [up(&peers, 0), up(&peers, 1)]);
    let mut frames = 0u64;
    peers.steps_for(Duration::from_secs(5), |_| {
        frames += 1;
        false
    });
    let after = (links.map(|l| meter(&peers, l)), [up(&peers, 0), up(&peers, 1)]);
    let per = |a: u64, b: u64| (b - a) as f64 / frames as f64;
    for (c, label) in ["replicated to", "no ReplicationSender"].iter().enumerate() {
        let datagrams = per(before.0[c].sent_datagrams, after.0[c].sent_datagrams);
        println!(
            "client {c} ({label}) over {frames} server frames: down {:.1} B in {datagrams:.3} datagrams a frame, up {:.1} B \
             in {:.3} datagrams a frame",
            per(before.0[c].sent_bytes, after.0[c].sent_bytes),
            per(before.1[c].sent_bytes, after.1[c].sent_bytes),
            per(before.1[c].sent_datagrams, after.1[c].sent_datagrams),
        );
        assert!(datagrams > 0.9 && datagrams < 1.1, "client {c}: {datagrams:.3} datagrams a frame");
    }
}

#[test]
fn a_newcomer_to_a_full_server_gets_every_player_within_the_fragment_limit() {
    use ssx_net::bevy_replicon::client::ClientReplicationStats;
    use ssx_net::limits::{MAX_CLIENTS, MAX_FRAGMENTED_MESSAGE};
    use ssx_net_proto::keys::PlayerSecret;
    let mut peers = Peers::new(Io::Crossbeam, None, FRAME).expect("server");
    let name = "€".repeat(20);
    let n = MAX_CLIENTS - 1;
    for c in 0..n {
        peers.add_player(PlayerSecret::generate().public(), &name).expect("client");
        assert!(peers.steps_for(Duration::from_secs(10), |p| p.connected(c)), "rider {c} connects");
    }
    assert!(ready(&mut peers, &(0..n).collect::<Vec<_>>(), Duration::from_secs(60)), "the riders are ready");
    let newcomer = peers.add_player(PlayerSecret::generate().public(), &name).expect("newcomer");
    peers.clients[newcomer].world_mut().init_resource::<ClientReplicationStats>();
    let complete = |p: &mut Peers| {
        let roster = p.clients[newcomer].world().resource::<ssx_net::ClientRoster>();
        roster.instance.is_some()
            && roster.players.len() == n + 1
            && roster.players.values().filter(|r| r.appearance.is_some()).count() == n
    };
    assert!(peers.steps_for(Duration::from_secs(20), complete), "the newcomer gets every player and look");
    let stats = peers.clients[newcomer].world().resource::<ClientReplicationStats>();
    println!(
        "a newcomer to {n} riders: {} replication messages, {} bytes, {} entities, {} components",
        stats.messages, stats.bytes, stats.entities_changed, stats.components_changed
    );
    assert!(stats.bytes <= MAX_FRAGMENTED_MESSAGE, "{} bytes", stats.bytes);
    assert!(peers.connected(newcomer));
}
