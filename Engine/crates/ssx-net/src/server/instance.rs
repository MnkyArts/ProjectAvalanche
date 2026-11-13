use std::collections::HashMap;

use bevy_ecs::prelude::*;
use lightyear::prelude::server::ClientOf;
use lightyear::prelude::{
    LocalTimeline, MessageReceiver, MessageSender, NetworkTarget, Replicate, RoomAllocator, RoomId, Rooms,
};
use ssx_gameplay::instance::{Admission, Instance, Progress, Refusal};
use ssx_gameplay::round::CycleRules;
use ssx_net_proto::{ClientControl, InstanceSpec, ServerControl};

use super::ServerStats;
use super::guard::{KickReason, Kicked, Leaving, kick};
use super::hold::{Ahead, Due, Hold, Offer};
use super::relay::RiderLink;
use super::session::PlayerSession;
use super::time::{LinkTime, Received, ServerTime};
use super::validation::{Input, ValidationInbox};
use crate::protocol::{ControlChannel, Standings, Wire};
use crate::roster::{InInstance, InstanceInfo, PlayerKeyId, PlayerName, PlayerState, RiderNumber};

#[derive(Clone, Debug)]
pub struct InstanceSetup {
    pub spec: InstanceSpec,
    pub capacity: usize,
    pub rounds: Option<(CycleRules, Vec<ssx_net_proto::instance::Level>)>,
}

#[derive(Resource, Clone, Debug, Default)]
pub struct InstanceConfig {
    pub instances: Vec<InstanceSetup>,
}

impl InstanceConfig {
    pub fn freeride(spec: InstanceSpec, capacity: usize) -> Self {
        Self { instances: vec![InstanceSetup { spec, capacity, rounds: None }] }
    }
}

pub const INSTANCE_ID: u16 = 1;

pub struct ServerInstance {
    pub instance: Instance,
    pub room: RoomId,
    pub entity: Entity,
    pub standings: Option<Entity>,
}

#[derive(Resource, Default)]
pub struct Instances {
    pub list: Vec<ServerInstance>,
}

impl Instances {
    pub fn by_id(&self, id: u16) -> Option<&ServerInstance> {
        self.list.iter().find(|i| i.instance.id() == id)
    }

    pub fn by_id_mut(&mut self, id: u16) -> Option<&mut ServerInstance> {
        self.list.iter_mut().find(|i| i.instance.id() == id)
    }

    pub fn of_rider(&self, rider: u16) -> Option<&ServerInstance> {
        self.list.iter().find(|i| i.instance.member(rider).is_some())
    }

    pub fn relay_roles(&self, rider: u16) -> (bool, bool) {
        self.of_rider(rider).map_or((false, false), |i| i.instance.relay_roles(rider))
    }
}

#[derive(Component, Clone, Copy, Debug)]
pub struct PlayerEntity(pub Entity);

#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlacedIn(pub u16);

#[derive(Component, Clone, Debug)]
pub(super) struct PendingWelcome(ServerControl);

pub(super) fn start(world: &mut World, lobby: RoomId) {
    let Some(config) = world.get_resource::<InstanceConfig>().cloned() else { return };
    let now = world.get_resource::<LocalTimeline>().map_or(0, |t| t.tick().0);
    let mut list = Vec::new();
    for (k, setup) in config.instances.into_iter().enumerate() {
        let Ok(id) = u16::try_from(k).map(|k| INSTANCE_ID.saturating_add(k)) else { break };
        let room = world.resource_mut::<RoomAllocator>().allocate();
        let instance = match setup.rounds {
            Some((rules, rotation)) => Instance::rounds(id, setup.spec, setup.capacity, rules, rotation, now),
            None => Instance::freeride(id, setup.spec, setup.capacity),
        };
        let info = InstanceInfo { id, spec: instance.spec().clone(), state: instance.state(), round: instance.round() };
        let entity = world.spawn((Replicate::to_clients(NetworkTarget::All), Rooms::single(lobby), info)).id();
        let standings = instance.spec().mode.has_rounds().then(|| {
            let table = Standings::new(id, instance.round());
            world.spawn((Replicate::to_clients(NetworkTarget::All), Rooms::single(room), table)).id()
        });
        list.push(ServerInstance { instance, room, entity, standings });
    }
    world.insert_resource(Instances { list });
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum PlaceError {
    Instance(Admission),
    Name(String),
}

// PORT(choice): the first instance with room takes a joining player (the join names no instance; MTA's dimensions and
// FiveM's routing buckets are the scripts' choice); tuning: the resources' placement API (phases 6+) and the browser's
// instance choice once servers run several; the original has no real-time multiplayer.
pub(super) fn place(
    commands: &mut Commands,
    instances: Option<&mut Instances>,
    lobby: RoomId,
    link: Entity,
    session: &PlayerSession,
    rider: u16,
    chat: bool,
) -> Result<(), PlaceError> {
    let Some(instances) = instances else {
        commands.entity(link).insert(Rooms::single(lobby));
        return Ok(());
    };
    let name = PlayerName::new(&session.name).map_err(PlaceError::Name)?;
    let mut refusal = Admission::Full;
    let mut placed = None;
    for s in &mut instances.list {
        match s.instance.admit(rider) {
            Ok(()) => {
                placed = Some(s);
                break;
            }
            Err(e) => refusal = e,
        }
    }
    let s = placed.ok_or(PlaceError::Instance(refusal))?;
    let id = s.instance.id();
    let mut key_id = [0u8; 8];
    key_id.copy_from_slice(&session.id.0[..8]);
    let status = s.instance.status(rider).unwrap_or(Progress::Joined.status());
    let player = commands
        .spawn((
            Replicate::to_clients(NetworkTarget::All),
            Rooms::single(lobby),
            RiderNumber(rider),
            name,
            PlayerKeyId(key_id),
            InInstance(id),
            PlayerState(status),
        ))
        .id();
    let welcome = ServerControl::Welcome { rider, instance: id, chat };
    let rooms = Rooms::from([lobby, s.room].into_iter());
    commands.entity(link).insert((rooms, PlayerEntity(player), PlacedIn(id), PendingWelcome(welcome)));
    Ok(())
}

pub(super) fn on_player_removed(
    trigger: On<Remove<PlayerEntity>>,
    links: Query<(&PlayerEntity, Option<&RiderLink>, Option<&PlacedIn>)>,
    (instances, validation): (Option<ResMut<Instances>>, Option<ResMut<ValidationInbox>>),
    mut commands: Commands,
) {
    let Ok((player, link, placed)) = links.get(trigger.entity) else { return };
    if let (Some(mut inbox), Some(link)) = (validation, link) {
        inbox.0.push(Input::Gone { rider: link.rider().0 });
    }
    if let (Some(mut instances), Some(link), Some(placed)) = (instances, link, placed)
        && let Some(s) = instances.by_id_mut(placed.0)
    {
        s.instance.leave(link.rider().0);
    }
    if let Ok(mut e) = commands.get_entity(player.0) {
        e.despawn();
    }
}

type Welcoming<'a> = (Entity, &'a PendingWelcome, &'a mut MessageSender<Wire<ServerControl>>);

pub(super) fn send_welcomes(
    mut links: Query<Welcoming, (With<lightyear::prelude::Connected>, Without<Kicked>)>,
    mut commands: Commands,
) {
    for (link, welcome, mut sender) in &mut links {
        sender.send::<ControlChannel>(Wire::control(&welcome.0));
        commands.entity(link).remove::<PendingWelcome>();
    }
}

type ControlLink<'a> = (
    Entity,
    &'a mut MessageReceiver<Wire<ClientControl>>,
    Option<&'a mut RiderLink>,
    Option<&'a PlacedIn>,
    Option<&'a LinkTime>,
);

type Listening = (With<ClientOf>, Without<Kicked>, Without<Leaving>);

fn refusal_kick(rule: Refusal) -> KickReason {
    match rule {
        Refusal::Order => KickReason::ControlOrder,
        rule => KickReason::Round(rule),
    }
}

#[derive(Resource, Default)]
pub(super) struct Holds(HashMap<Entity, Hold>);

struct Control<'a> {
    instances: Option<&'a mut Instances>,
    validation: Option<&'a mut ValidationInbox>,
    time: &'a ServerTime,
    stats: &'a mut ServerStats,
    now: u32,
}

pub(super) fn receive_client_control(
    mut links: Query<ControlLink, Listening>,
    (mut instances, mut validation): (Option<ResMut<Instances>>, Option<ResMut<ValidationInbox>>),
    (timeline, time): (Res<LocalTimeline>, Res<ServerTime>),
    mut holds: ResMut<Holds>,
    mut stats: ResMut<ServerStats>,
    mut commands: Commands,
) {
    let mut c = Control {
        instances: instances.as_deref_mut(),
        validation: validation.as_deref_mut(),
        time: &time,
        stats: &mut stats,
        now: timeline.tick().0,
    };
    let mut listening = Vec::new();
    for (link, mut receiver, mut rider, placed, link_time) in &mut links {
        listening.push(link);
        let hold = holds.0.entry(link).or_default();
        let released = match c.instances.as_deref() {
            Some(i) => hold.release(|d| {
                c.time
                    .elapsed(i, d.instance, d.round, c.now, std::time::Instant::now())
                    .filter(|_| i.by_id(d.instance).is_some_and(|s| s.instance.round() == d.round))
            }),
            None => hold.release(|_| None),
        };
        let mut ended = false;
        for h in released {
            if !handle(&mut c, link, rider.as_deref_mut(), placed, h.message, h.received, &mut commands) {
                ended = true;
                break;
            }
        }
        if ended {
            receiver.receive().for_each(drop);
            continue;
        }
        for wire in receiver.receive() {
            let message = match ClientControl::decode(wire.bytes()) {
                Ok(m) => m,
                Err(_) => {
                    kick(&mut commands, link, KickReason::MalformedControl);
                    break;
                }
            };
            let received = c.time.received();
            let ahead = ahead_of(&c, placed, link_time, hold, &message, received);
            match hold.offer(message, received, ahead) {
                Offer::Now(message, received) => {
                    if !handle(&mut c, link, rider.as_deref_mut(), placed, *message, received, &mut commands) {
                        break;
                    }
                }
                Offer::Held => c.stats.control_held += 1,
                Offer::Refused(why) => {
                    let id = rider.as_deref().map_or(0, |r| r.rider().0);
                    c.stats.note_refusal(id, &format!("{}: {why}", Refusal::ReportFuture.name()));
                    kick(&mut commands, link, refusal_kick(Refusal::ReportFuture));
                    break;
                }
            }
        }
        if hold.len() > c.stats.control_held_most {
            c.stats.control_held_most = hold.len();
        }
    }
    holds.0.retain(|link, h| !h.is_empty() && listening.contains(link));
}

fn ahead_of(
    c: &Control<'_>,
    placed: Option<&PlacedIn>,
    link: Option<&LinkTime>,
    hold: &Hold,
    message: &ClientControl,
    received: Received,
) -> Option<Ahead> {
    let instances = c.instances.as_deref()?;
    let instance = placed?.0;
    match message {
        ClientControl::Ready { round, sample, .. } => {
            let s = instances.by_id(instance).filter(|s| s.instance.round() == *round)?;
            let t = s.instance.start_tick()?;
            if c.now >= t {
                return None;
            }
            let at = c.time.arrival(instances, (instance, *round), c.now, link, received)?;
            if -at.elapsed > super::time::ready_early_allowed(at.horizon, f64::from(sample.execution()) + 2.0) {
                return None;
            }
            let due = Due { instance, round: *round, execution: 0.0 };
            Some(Ahead { elapsed: at.elapsed, horizon: at.horizon, due })
        }
        ClientControl::Report(r) => {
            let run_started = c.time.started(r.instance, r.round) || hold.holds_ready(r.instance, r.round);
            if instance != r.instance || !run_started {
                return None;
            }
            let at = c.time.arrival(instances, (r.instance, r.round), c.now, link, received)?;
            let due = Due { instance: r.instance, round: r.round, execution: f64::from(r.sample.execution()) };
            Some(Ahead { elapsed: at.elapsed, horizon: at.horizon, due })
        }
        _ => None,
    }
}

fn handle(
    c: &mut Control<'_>,
    link: Entity,
    rider: Option<&mut RiderLink>,
    placed: Option<&PlacedIn>,
    message: ClientControl,
    received: Received,
    commands: &mut Commands,
) -> bool {
    let instance = placed.and_then(|p| c.instances.as_deref_mut().and_then(|i| i.by_id_mut(p.0)));
    let (Some(rider), Some(s)) = (rider, instance) else {
        kick(commands, link, KickReason::ControlOrder);
        return false;
    };
    let (start, id, round) = (s.instance.start_tick(), s.instance.id(), s.instance.round());
    match s.instance.control(rider.rider().0, &message, c.now) {
        Ok(a) if a.stale => c.stats.control_stale += 1,
        Ok(a) if a.dropped => c.stats.reports_dropped += 1,
        Ok(a) => {
            let rider_id = rider.rider().0;
            if message == ClientControl::LevelLeft {
                rider.restart_track();
            }
            if let ClientControl::Report(r) = &message
                && start.is_some()
                && let Some(instances) = c.instances.as_deref()
            {
                let elapsed = c.time.elapsed(instances, id, round, c.now, received.at);
                note_report(c.stats, r, elapsed);
            }
            if let ClientControl::Report(r) = &message {
                super::scripts::note_report(commands, link, r);
            }
            if let Some(inbox) = c.validation.as_deref_mut() {
                match message {
                    ClientControl::Ready { sample, second, .. } => {
                        inbox.0.push(Input::Ready { rider: rider_id, link, received, sample, second })
                    }
                    ClientControl::Report(r) => {
                        inbox.0.push(Input::Report { rider: rider_id, report: r, over: a.progress, received })
                    }
                    ClientControl::LevelLeft => inbox.0.push(Input::Gone { rider: rider_id }),
                    _ => {}
                }
            }
        }
        Err(refused) => {
            c.stats.note_refusal(rider.rider().0, &refused.to_string());
            kick(commands, link, refusal_kick(refused.rule));
            return false;
        }
    }
    true
}

fn note_report(stats: &mut ServerStats, r: &ssx_net_proto::ProgressReport, elapsed: Option<f64>) {
    let q = |x: f64| (x * 256.0).round().clamp(i64::MIN as f64, i64::MAX as f64) as i64;
    let execution = f64::from(r.sample.execution());
    if let Some(e) = elapsed {
        top3(&mut stats.report_lead_top, q(execution - e));
    }
    let clock = f64::from(r.clock_ms) * f64::from(ssx_net_proto::TICK_HZ) / 1000.0;
    top3(&mut stats.report_clock_excess_top, q(clock - execution));
}

fn top3(top: &mut [Option<i64>; 3], mut v: i64) {
    for slot in top.iter_mut() {
        match *slot {
            None => {
                *slot = Some(v);
                return;
            }
            Some(s) if v > s => {
                *slot = Some(v);
                v = s;
            }
            Some(_) => {}
        }
    }
}

pub(super) fn drop_leaving_control(mut leaving: Query<&mut MessageReceiver<Wire<ClientControl>>, With<Leaving>>) {
    for mut receiver in &mut leaving {
        receiver.receive().for_each(drop);
    }
}
