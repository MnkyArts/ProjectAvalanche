use core::time::Duration;
use std::sync::Arc;

use bevy_ecs::prelude::*;
use bevy_time::{Real, Time};
use lightyear::prelude::server::ClientOf;
use lightyear::prelude::{MessageReceiver, MessageSender, Transport};
use lightyear::transport::channel::ChannelKind;
use ssx_net_proto::scripts::{Assembler, Direction, LATENT_DOWN, LATENT_UP, Outgoing, QueueFull, ScriptItem};
use ssx_net_proto::{ProgressReport, ReportKind};
use std::collections::VecDeque;

use super::guard::{KickReason, Kicked, Leaving, kick, leave};
use super::session::PlayerSession;
use crate::protocol::{ScriptChannel, ScriptDown, ScriptUp, Wire};

#[derive(Clone, Copy, Debug)]
pub struct Bucket {
    tokens: f64,
    rate: f64,
    burst: f64,
    at: Option<Duration>,
}

impl Bucket {
    pub const fn new(rate: f64, burst: f64) -> Self {
        Self { tokens: burst, rate, burst, at: None }
    }

    pub fn take(&mut self, now: Duration, n: f64) -> bool {
        let dt = self.at.map_or(0.0, |at| now.saturating_sub(at).as_secs_f64());
        self.at = Some(now);
        self.tokens = (self.tokens + dt * self.rate).min(self.burst);
        if self.tokens < n {
            return false;
        }
        self.tokens -= n;
        true
    }
}

pub const EVENTS: (f64, f64) = (50.0, 200.0);
pub const BYTES: (f64, f64) = (128.0 * 1024.0, 384.0 * 1024.0);
pub const WRITES: (f64, f64) = (75.0, 125.0);
pub const FLOOD: (f64, f64) = (75.0, 300.0);
// PORT(choice): the byte flood bound is twice the byte bucket (256 KiB a second, a burst of 768 KiB); FiveM kicks for a
// full reliable buffer instead, MTA has no byte bound on events; tuning: the bytes of real gamemodes' client events (a
// scoreboard's, a map editor's) measured over a session; the original runs no mod scripts.
pub const BYTE_FLOOD: (f64, f64) = (2.0 * BYTES.0, 2.0 * BYTES.1);

pub const HELD_BYTES: usize = 192 * 1024;
pub const HELD_MESSAGES: usize = 192;
// PORT(choice): 16 MiB per client, the host's outbox bytes; the clients together are bounded by TOTAL_QUEUE_BYTES;
// tuning: the snapshots of real gamemodes' trees and a measured slow client; the original runs no mod scripts.
pub const QUEUE_BYTES: usize = 16 << 20;
// PORT(choice): 128 MiB, eight whole queues (the measured honest runs hold kilobytes; 64 slow clients would hold 1 GiB
// without it); tuning: a server's resident memory under many slow clients against what an operator gives it; the
// original runs no mod scripts.
pub const TOTAL_QUEUE_BYTES: usize = 8 * QUEUE_BYTES;
pub const LATENT_DOWN_BUCKET: (f64, f64) = (LATENT_DOWN as f64, LATENT_DOWN as f64);
pub const LATENT_UP_BUCKET: (f64, f64) = (LATENT_UP as f64, 3.0 * LATENT_UP as f64);

pub type Shared = Arc<[u8]>;

// PORT(choice): 64 KiB a frame (1.9 MiB/s at 30 Hz). Sending is the world's work: under a hostile script's flood to 8
// clients on loopback (round net7b's trace) a KiB on the wire cost about 30 us of the frame (lightyear's packets,
// netcode's sealing, the UDP send: 4.6 ms a frame for about 140 wires), so 64 KiB costs about 2 ms of the 33.3 ms tick
// whatever the clients' count, where the clients' windows alone (192 KiB each per round trip) let 64 clients cost more
// than the tick; tuning: the cost per KiB on a real network card, and real gamemodes' traffic beside the rides; the
// original runs no mod scripts.
pub const SEND_BUDGET: usize = 64 << 10;

#[derive(Component, Debug)]
pub struct ScriptLink {
    pub client: u32,
    asm: Assembler,
    events: Bucket,
    bytes: Bucket,
    writes: Bucket,
    flood: Bucket,
    byte_flood: Bucket,
    out: Outgoing<Shared>,
    pub dropped: u64,
    latent_down: Bucket,
    latent_up: Bucket,
    behind_since: Option<Duration>,
    pushed: u64,
    snapshot_end: Option<u64>,
}

impl ScriptLink {
    pub fn new(client: u32) -> Self {
        Self {
            client,
            asm: Assembler::default(),
            events: Bucket::new(EVENTS.0, EVENTS.1),
            bytes: Bucket::new(BYTES.0, BYTES.1),
            writes: Bucket::new(WRITES.0, WRITES.1),
            flood: Bucket::new(FLOOD.0, FLOOD.1),
            byte_flood: Bucket::new(BYTE_FLOOD.0, BYTE_FLOOD.1),
            out: Outgoing::new(QUEUE_BYTES),
            dropped: 0,
            latent_down: Bucket::new(LATENT_DOWN_BUCKET.0, LATENT_DOWN_BUCKET.1),
            latent_up: Bucket::new(LATENT_UP_BUCKET.0, LATENT_UP_BUCKET.1),
            behind_since: None,
            pushed: 0,
            snapshot_end: None,
        }
    }

    pub fn queue(&mut self, item: Shared, latent: usize, now: Duration) -> Result<(), QueueFull> {
        let was_empty = self.out.is_empty();
        let n = item.len() as u64;
        self.out.push_paced(item, latent)?;
        self.pushed += n;
        if was_empty {
            self.behind_since = Some(now);
        }
        Ok(())
    }

    pub fn queue_snapshot(&mut self, items: &[Shared], now: Duration) -> Result<(), QueueFull> {
        self.clear();
        for item in items {
            self.queue(Arc::clone(item), 0, now)?;
        }
        self.snapshot_end = Some(self.pushed);
        Ok(())
    }

    pub fn holds_snapshot(&self) -> bool {
        self.snapshot_end.is_some_and(|end| self.held() as u64 > self.pushed - end)
    }

    pub fn take_wires(&mut self, budget: usize, now: Duration) -> (Vec<Vec<u8>>, bool) {
        let mut waited = false;
        let latent = &mut self.latent_down;
        let wires = self.out.take_paced(budget, &mut |bytes| {
            let ok = latent.take(now, bytes as f64);
            waited |= !ok;
            ok
        });
        if self.out.is_empty() {
            self.behind_since = None;
            self.snapshot_end = None;
        }
        (wires, waited)
    }

    pub fn clear(&mut self) {
        self.out.clear();
        if self.out.is_empty() {
            self.behind_since = None;
        }
        self.snapshot_end = None;
    }

    pub fn held(&self) -> usize {
        self.out.held()
    }
}

#[derive(Resource, Default)]
pub struct ScriptInbox(pub Vec<(u32, ScriptItem)>);

#[derive(Resource, Default)]
pub struct ScriptSessions {
    pub joined: Vec<(Entity, u32, String)>,
    pub left: Vec<(u32, String)>,
    next: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReportEvent {
    pub link: Entity,
    pub instance: u16,
    pub round: u16,
    pub kind: ReportKind,
    pub split: Option<u8>,
    pub clock_ms: u32,
    pub points: u32,
}

#[derive(Resource, Default)]
pub struct ScriptReports {
    pub events: VecDeque<ReportEvent>,
    pub dropped: u64,
}

pub const MAX_REPORTS: usize = 4096;

pub(super) fn note_report(commands: &mut Commands, link: Entity, r: &ProgressReport) {
    let e = ReportEvent {
        link,
        instance: r.instance,
        round: r.round,
        kind: r.kind,
        split: r.split,
        clock_ms: r.clock_ms,
        points: r.points,
    };
    commands.queue(move |w: &mut World| {
        if let Some(mut reports) = w.get_resource_mut::<ScriptReports>() {
            if reports.events.len() >= MAX_REPORTS {
                reports.events.pop_front();
                reports.dropped += 1;
            }
            reports.events.push_back(e);
        }
    });
}

#[derive(Resource, Clone, Debug, Default, PartialEq, Eq)]
pub struct ScriptNetStats {
    pub wires_received: u64,
    pub items_received: u64,
    pub dropped: u64,
    pub floods: u64,
    pub malformed: u64,
    pub wires_sent: u64,
    pub bytes_sent: u64,
    pub latent_waits: u64,
    pub latent_over: u64,
    pub budget_frames: u64,
}

impl ScriptNetStats {
    pub fn line(&self) -> String {
        format!(
            "script channel: {} wire messages / {} items received, {} dropped over the buckets, {} floods, {} \
             malformed; {} wire messages / {} bytes sent ({} frames the frame's budget held a client back); latent: \
             {} frames a queue waited for its client's rate, {} chunks past a client's rate up",
            self.wires_received,
            self.items_received,
            self.dropped,
            self.floods,
            self.malformed,
            self.wires_sent,
            self.bytes_sent,
            self.budget_frames,
            self.latent_waits,
            self.latent_over
        )
    }
}

pub(super) fn on_session(
    trigger: On<Add<PlayerSession>>,
    sessions: Query<&PlayerSession>,
    mut book: ResMut<ScriptSessions>,
    mut commands: Commands,
) {
    let Ok(session) = sessions.get(trigger.entity) else { return };
    book.next = book.next.wrapping_add(1).max(1);
    let client = book.next;
    commands.entity(trigger.entity).insert(ScriptLink::new(client));
    book.joined.push((trigger.entity, client, session.name.clone()));
}

pub(super) fn on_link_removed(
    trigger: On<Remove<ScriptLink>>,
    links: Query<(&ScriptLink, Option<&Leaving>, Option<&Kicked>)>,
    mut book: ResMut<ScriptSessions>,
) {
    let Ok((link, leaving, kicked)) = links.get(trigger.entity) else { return };
    let why = match (leaving, kicked) {
        (Some(l), _) => format!("{:?}: {}", l.reason, l.text),
        (None, Some(k)) => format!("{:?}", k.0),
        (None, None) => "left".to_string(),
    };
    book.left.push((link.client, why));
}

type Reading<'a> = (Entity, &'a mut MessageReceiver<Wire<ScriptUp>>, &'a mut ScriptLink);
type Listening = (With<ClientOf>, Without<Kicked>, Without<Leaving>);

pub(super) fn receive(
    mut links: Query<Reading, Listening>,
    time: Res<Time<Real>>,
    mut inbox: ResMut<ScriptInbox>,
    mut stats: ResMut<ScriptNetStats>,
    mut commands: Commands,
) {
    let now = time.elapsed();
    for (entity, mut receiver, mut link) in &mut links {
        let link = &mut *link;
        'wires: for wire in receiver.receive() {
            stats.wires_received += 1;
            let items = match link.asm.take(wire.bytes()) {
                Ok(items) => items,
                Err(_) => {
                    stats.malformed += 1;
                    kick(&mut commands, entity, KickReason::MalformedScript);
                    break;
                }
            };
            for bytes in items {
                let Ok(item) = ScriptItem::decode(&bytes, Direction::ToServer) else {
                    stats.malformed += 1;
                    kick(&mut commands, entity, KickReason::MalformedScript);
                    break 'wires;
                };
                stats.items_received += 1;
                let size = bytes.len() as f64;
                if !link.flood.take(now, 1.0) || !link.byte_flood.take(now, size) {
                    stats.floods += 1;
                    leave(&mut commands, entity, KickReason::ScriptFlood, "too many script messages", now);
                    break 'wires;
                }
                let write = matches!(item.body, ssx_net_proto::scripts::ScriptBody::DataWrite { .. });
                if let ssx_net_proto::scripts::ScriptBody::LatentChunk { bytes, .. } = &item.body
                    && !link.latent_up.take(now, bytes.len() as f64)
                {
                    stats.latent_over += 1;
                    stats.dropped += 1;
                    link.dropped += 1;
                    continue;
                }
                let passed = link.bytes.take(now, size)
                    && (!item.body.is_event() || link.events.take(now, 1.0))
                    && (!write || link.writes.take(now, 1.0));
                if !passed {
                    stats.dropped += 1;
                    link.dropped += 1;
                    continue;
                }
                inbox.0.push((link.client, item));
            }
        }
    }
}

pub(super) fn drop_leaving(mut leaving: Query<&mut MessageReceiver<Wire<ScriptUp>>, With<Leaving>>) {
    for mut receiver in &mut leaving {
        receiver.receive().for_each(drop);
    }
}

type Sending<'a> = (&'a mut ScriptLink, &'a mut MessageSender<Wire<ScriptDown>>, &'a Transport);

pub(super) fn send(
    mut links: Query<Sending, Listening>,
    time: Res<Time<Real>>,
    mut stats: ResMut<ScriptNetStats>,
    mut turn: Local<u32>,
) {
    let now = time.elapsed();
    let kind = ChannelKind::of::<ScriptChannel>();
    let mut ready: Vec<_> = links
        .iter_mut()
        .filter_map(|(link, sender, transport)| {
            if link.out.is_empty() {
                return None;
            }
            let (n, b) = transport.channel_send(kind).and_then(|c| c.reliable_held()).unwrap_or((0, 0));
            (n < HELD_MESSAGES).then(|| (link, sender, HELD_BYTES.saturating_sub(b)))
        })
        .collect();
    if ready.is_empty() {
        return;
    }
    ready.sort_unstable_by_key(|(l, ..)| l.client);
    let clients: Vec<u32> = ready.iter().map(|(l, ..)| l.client).collect();
    let (first, next) = first_served(&clients, *turn);
    *turn = next;
    ready.rotate_left(first);
    let mut views: Vec<(&mut ScriptLink, usize)> = ready.iter_mut().map(|(l, _, room)| (&mut **l, *room)).collect();
    let (served, limited) = serve(&mut views, SEND_BUDGET, now);
    drop(views);
    stats.budget_frames += u64::from(limited);
    for ((_, sender, _), Served { wires, waited }) in ready.iter_mut().zip(served) {
        if waited {
            stats.latent_waits += 1;
        }
        for w in wires {
            stats.wires_sent += 1;
            stats.bytes_sent += w.len() as u64;
            sender.send::<ScriptChannel>(Wire::from_raw(w));
        }
    }
}

fn first_served(clients: &[u32], turn: u32) -> (usize, u32) {
    let first = clients.iter().position(|&c| c >= turn).unwrap_or(0);
    (first, clients.get(first).map_or(0, |c| c.wrapping_add(1)))
}

#[derive(Debug)]
pub struct Served {
    pub wires: Vec<Vec<u8>>,
    pub waited: bool,
}

pub fn serve(ready: &mut [(&mut ScriptLink, usize)], budget: usize, now: Duration) -> (Vec<Served>, bool) {
    let mut left = budget;
    let mut limited = false;
    let n = ready.len();
    let mut out = Vec::with_capacity(n);
    for (i, (link, room)) in ready.iter_mut().enumerate() {
        let share = left / (n - i);
        let take = share.min(*room);
        let (wires, waited) = link.take_wires(take, now);
        left -= wires.iter().map(Vec::len).sum::<usize>().min(left);
        limited |= take < *room && !waited && !link.out.is_empty();
        out.push(Served { wires, waited });
    }
    (out, limited)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shed {
    Resync,
    SendAway,
}

pub fn shed(links: &mut [(u32, &mut ScriptLink)], budget: usize) -> Vec<(u32, Shed)> {
    let mut total: usize = links.iter().map(|(_, l)| l.held()).sum();
    let mut did: Vec<(u32, Shed)> = Vec::new();
    while total > budget {
        let next = links
            .iter()
            .enumerate()
            .filter(|(_, (c, l))| l.held() > 0 && !did.iter().any(|(d, _)| d == c))
            .min_by_key(|(_, (c, l))| (l.behind_since, *c))
            .map(|(i, _)| i);
        let Some(i) = next else { break };
        let (client, link) = &mut links[i];
        let what = if link.holds_snapshot() { Shed::SendAway } else { Shed::Resync };
        let before = link.held();
        link.clear();
        total -= before - link.held();
        did.push((*client, what));
    }
    did
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bucket_gives_its_burst_then_its_rate() {
        let mut b = Bucket::new(50.0, 200.0);
        let t = Duration::from_secs(1);
        assert_eq!((0..250).filter(|_| b.take(t, 1.0)).count(), 200);
        assert_eq!((0..100).filter(|_| b.take(t + Duration::from_secs(1), 1.0)).count(), 50);
    }

    #[test]
    fn the_queues_past_the_budget_lose_the_longest_waiting_first() {
        let s = Duration::from_secs;
        let item = |n: usize| -> Shared { vec![7u8; n].into() };
        let (mut a, mut b, mut c, mut d) =
            (ScriptLink::new(1), ScriptLink::new(2), ScriptLink::new(3), ScriptLink::new(4));
        a.queue(item(40_000), 0, s(3)).unwrap();
        b.queue_snapshot(&[item(40_000)], s(1)).unwrap();
        c.queue(item(40_000), 0, s(2)).unwrap();
        let budget = 100_000;
        let mut links = [(1, &mut a), (2, &mut b), (3, &mut c), (4, &mut d)];
        assert!(shed(&mut links, 120_000).is_empty(), "within the budget");
        assert_eq!(shed(&mut links, budget), vec![(2, Shed::SendAway)], "the oldest holds its snapshot");
        assert_eq!(b.held(), 0);
        let mut links = [(1, &mut a), (2, &mut b), (3, &mut c), (4, &mut d)];
        assert_eq!(shed(&mut links, 50_000), vec![(3, Shed::Resync)], "then the next oldest, resynced");
        let mut links = [(1, &mut a), (2, &mut b), (3, &mut c), (4, &mut d)];
        assert_eq!(shed(&mut links, 0), vec![(1, Shed::Resync)], "an empty queue is never chosen");
        assert_eq!((a.held(), c.held(), d.held()), (0, 0, 0));
        d.queue_snapshot(&[item(500)], s(4)).unwrap();
        d.queue(item(40_000), 0, s(5)).unwrap();
        assert!(d.holds_snapshot());
        let (wires, _) = d.take_wires(600, s(6));
        assert!(!wires.is_empty() && !d.holds_snapshot() && d.held() > 39_000, "{}", d.held());
        let mut links = [(4, &mut d)];
        assert_eq!(shed(&mut links, 0), vec![(4, Shed::Resync)]);
    }

    #[test]
    fn latent_chunks_wait_for_the_clients_rate() {
        let mut l = ScriptLink::new(1);
        let chunk = 64 << 10;
        let n = LATENT_DOWN as usize / chunk + 2;
        for _ in 0..n {
            l.queue(vec![1u8; chunk + 20].into(), chunk, Duration::ZERO).unwrap();
        }
        let now = Duration::from_secs(10);
        let (wires, waited) = l.take_wires(usize::MAX, now);
        let sent: usize = wires.iter().map(Vec::len).sum();
        let whole = LATENT_DOWN as usize / chunk;
        assert!(waited && sent > whole * chunk && sent < (whole + 1) * (chunk + 1024), "{sent}");
        let (wires, waited) = l.take_wires(usize::MAX, now + Duration::from_millis(50));
        assert!(!wires.is_empty() && waited && l.held() > 0, "the last chunk waits its time");
    }

    #[test]
    fn a_frame_sends_its_budget_shared_out_evenly() {
        let now = Duration::from_secs(1);
        let big: Shared = vec![1u8; 60_000].into();
        let mut links: Vec<ScriptLink> = (1..=4).map(ScriptLink::new).collect();
        for l in &mut links[..3] {
            (0..4).for_each(|_| l.queue(Arc::clone(&big), 0, now).unwrap());
        }
        links[3].queue(vec![2u8; 300].into(), 0, now).unwrap();
        let rooms = [HELD_BYTES, 2_000, HELD_BYTES, HELD_BYTES];
        let mut ready: Vec<(&mut ScriptLink, usize)> = links.iter_mut().zip(rooms).collect();
        let (served, limited) = serve(&mut ready, SEND_BUDGET, now);
        let sent: Vec<usize> = served.iter().map(|s| s.wires.iter().map(Vec::len).sum()).collect();
        assert_eq!(sent, [16_384, 2_000, 23_576, 303]);
        assert!(limited, "the first link took its share and holds more");
        let mut links: Vec<ScriptLink> = (1..=64).map(ScriptLink::new).collect();
        links.iter_mut().for_each(|l| l.queue(Arc::clone(&big), 0, now).unwrap());
        let mut ready: Vec<(&mut ScriptLink, usize)> = links.iter_mut().map(|l| (l, HELD_BYTES)).collect();
        let (served, _) = serve(&mut ready, SEND_BUDGET, now);
        assert!(served.iter().all(|s| s.wires.len() == 1 && s.wires[0].len() == 1024));
        assert_eq!(Arc::strong_count(&big), 1 + 3 * 4 + 64);
    }

    #[test]
    fn each_frame_the_next_client_is_served_first() {
        let mut turn = 0;
        let mut firsts = Vec::new();
        for _ in 0..4 {
            let (first, next) = first_served(&[3, 5, 9], turn);
            firsts.push([3, 5, 9][first]);
            turn = next;
        }
        assert_eq!(firsts, [3, 5, 9, 3]);
        assert_eq!(first_served(&[3, 7, 9], 4), (1, 8));
        assert_eq!(first_served(&[u32::MAX], 0), (0, 0));
    }
}
