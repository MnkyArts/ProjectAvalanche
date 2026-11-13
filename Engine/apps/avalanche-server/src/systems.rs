use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use bevy::prelude::*;
use ssx_net::address::canonical;
use ssx_net::lightyear::netcode::DisconnectCause;
use ssx_net::lightyear::prelude::server::{ClientOf, Stop};
use ssx_net::lightyear::prelude::{Disconnected, LocalAddr};
use ssx_net::server::{
    ChatLog, ChatOutbox, Instances, IssueError, KickReason, Kicked, Leaving, LinkEnd, NotEndedByServer, PlayerSession,
    RiderLink, leave,
};
use ssx_net::{ServerStats, Sessions};
use ssx_net_proto::join::RefusalCode;
use ssx_net_proto::{ClientChat, ServerChat};

use crate::bridge::{JoinQueue, JoinTask, JoinVerdict, Shared};
use crate::console::{self, Command};
use crate::errors;
use crate::logging::SUPPRESSED_LINES;

#[derive(Resource, Clone)]
pub struct ServerFacts {
    pub max_players: u16,
    pub server_entity: Entity,
    pub started: Instant,
}

#[derive(Resource)]
pub struct Bridge {
    pub shared: Arc<Shared>,
    pub joins: JoinQueue,
}

#[derive(Resource)]
pub struct ConsoleLines(pub std::sync::Mutex<std::sync::mpsc::Receiver<String>>);

#[derive(Resource, Default)]
pub struct TickTimes {
    started: Option<Instant>,
    samples: VecDeque<u32>,
}

const TICK_SAMPLES: usize = 900;

impl TickTimes {
    pub fn percentiles(&self) -> Option<(u32, u32, u32)> {
        let mut sorted: Vec<u32> = self.samples.iter().copied().collect();
        if sorted.is_empty() {
            return None;
        }
        sorted.sort_unstable();
        let at = |q: f64| sorted[((sorted.len() - 1) as f64 * q).round() as usize];
        Some((at(0.5), at(0.99), sorted[sorted.len() - 1]))
    }
}

pub fn tick_started(mut times: ResMut<TickTimes>) {
    times.started = Some(Instant::now());
}

pub fn tick_ended(mut times: ResMut<TickTimes>, bridge: Res<Bridge>) {
    let Some(started) = times.started.take() else { return };
    let micros = u32::try_from(started.elapsed().as_micros()).unwrap_or(u32::MAX);
    if times.samples.len() == TICK_SAMPLES {
        times.samples.pop_front();
    }
    times.samples.push_back(micros);
    if let Some((p50, p99, max)) = times.percentiles() {
        bridge.shared.tick_p50_us.store(p50, Ordering::Relaxed);
        bridge.shared.tick_p99_us.store(p99, Ordering::Relaxed);
        bridge.shared.tick_max_us.store(max, Ordering::Relaxed);
    }
}

type LiveSession<'a> = (Entity, &'a PlayerSession, Has<Leaving>);

pub fn decide_joins(
    mut bridge: ResMut<Bridge>,
    facts: Res<ServerFacts>,
    mut sessions: ResMut<Sessions>,
    live: Query<LiveSession, (With<ClientOf>, Without<Kicked>)>,
    servers: Query<&LocalAddr>,
    time: Res<Time<Real>>,
    mut held: Option<ResMut<crate::scripts::HeldJoins>>,
) {
    let now = time.elapsed();
    let bound = servers.get(facts.server_entity).map(|a| a.0).ok();
    while let Some(JoinTask { ticket, reply }) = crate::scripts::next_join(&mut bridge.joins, held.as_deref_mut(), now)
    {
        let verdict = if bridge.shared.stopping.load(Ordering::Acquire) {
            JoinVerdict::Refused(RefusalCode::Busy, "the server is stopping".into())
        } else {
            sessions.expire(now);
            let replaces = live.iter().any(|(_, s, _)| s.player == ticket.player);
            let connected = live.iter().count();
            let reserved = sessions.reserved() - usize::from(sessions.reserved_for(&ticket.player));
            if !replaces && connected + reserved >= usize::from(facts.max_players) {
                JoinVerdict::Refused(RefusalCode::Full, format!("the server is full ({} players)", facts.max_players))
            } else if let Some(bound) = bound {
                match sessions.issue(now, bound, &ticket) {
                    Ok(issued) => JoinVerdict::Accepted(issued.token),
                    Err(IssueError::Address) => {
                        JoinVerdict::Refused(RefusalCode::Flood, "too many unfinished joins from your address".into())
                    }
                    Err(e) => JoinVerdict::Refused(RefusalCode::Busy, format!("no ticket now ({e:?})")),
                }
            } else {
                JoinVerdict::Refused(RefusalCode::Busy, "the server has no game socket".into())
            }
        };
        let _ = reply.send(verdict);
    }
}

pub fn publish(
    bridge: Res<Bridge>,
    live: Query<(Entity, &PlayerSession), Without<Kicked>>,
    (stats, instances, maps): (Res<ServerStats>, Option<Res<Instances>>, Option<Res<crate::maps::ServerMaps>>),
    mut order: Local<Vec<Entity>>,
    mut names: Local<Vec<String>>,
) {
    // PORT(choice): a server of several instances shows its first instance's mode and current level in the browser
    // (`/info` lists no instances); tuning: phase 5's query and the browser's view of several instances; the original
    // has no dedicated server.
    if let Some(first) = instances.as_ref().and_then(|i| i.list.first())
        && let Ok(mut level) = bridge.shared.level.lock()
    {
        let none = Default::default();
        let titles = maps.as_ref().map_or(&none, |m| &m.titles);
        let gamemodes = maps.as_ref().map_or(&none, |m| &m.gamemode_titles);
        let now = crate::maps::shown(&first.instance.spec().level, titles, gamemodes);
        if level.as_ref() != Some(&now) {
            *level = Some(now);
        }
    }
    bridge.shared.players.store(u32::try_from(live.iter().count()).unwrap_or(u32::MAX), Ordering::Release);
    order.retain(|e| live.contains(*e));
    let mut new: Vec<Entity> = live.iter().map(|(e, _)| e).filter(|e| !order.contains(e)).collect();
    new.sort();
    order.extend(new);
    let now = order.iter().filter_map(|e| live.get(*e).ok()).map(|(_, s)| s.name.as_str());
    if now.clone().ne(names.iter().map(String::as_str)) {
        *names = now.map(str::to_string).collect();
        if let Ok(mut shared) = bridge.shared.player_names.lock() {
            shared.clone_from(&names);
        }
    }
    bridge.shared.sessions.store(stats.sessions, Ordering::Relaxed);
    bridge.shared.kicks.store(stats.kicked, Ordering::Relaxed);
    let (shared, t) = (&bridge.shared, stats.traffic);
    for (to, v) in [
        (&shared.sent_bytes, t.sent_bytes),
        (&shared.sent_datagrams, t.sent_datagrams),
        (&shared.recv_bytes, t.recv_bytes),
        (&shared.recv_datagrams, t.recv_datagrams),
        (&shared.poses_received, stats.poses_received),
        (&shared.poses_decimated, stats.poses_decimated),
    ] {
        to.store(v, Ordering::Relaxed);
    }
    for (to, v) in shared.poses_relayed.iter().zip(stats.poses_by_tier) {
        to.store(v, Ordering::Relaxed);
    }
}

fn say_to_operator(text: &str) {
    info!("{text}");
}

fn status(world: &World, facts: &ServerFacts, shared: &Shared, times: &TickTimes) -> Vec<String> {
    let mut out = vec![format!(
        "uptime {:.0} s; tick p50 / p99 / max {}",
        facts.started.elapsed().as_secs_f64(),
        times.percentiles().map_or("-".into(), |(a, b, c)| format!(
            "{:.2} / {:.2} / {:.2} ms",
            f64::from(a) / 1e3,
            f64::from(b) / 1e3,
            f64::from(c) / 1e3
        ))
    )];
    let stats = world.resource::<ServerStats>();
    let sessions = world.resource::<Sessions>();
    out.push(format!(
        "joins accepted {} refused {}; sessions {}; left {}; timed out {}; kicks {}; tickets held {} (unclaimed {}); \
         http requests {} limited {}; system errors {}; log lines suppressed {}",
        shared.joins_accepted.load(Ordering::Relaxed),
        shared.joins_refused.load(Ordering::Relaxed),
        stats.sessions,
        stats.left,
        stats.timed_out,
        stats.kicked,
        sessions.held(),
        sessions.reserved(),
        shared.http_requests.load(Ordering::Relaxed),
        shared.http_limited.load(Ordering::Relaxed),
        errors::count(),
        SUPPRESSED_LINES.load(Ordering::Relaxed),
    ));
    let uptime = facts.started.elapsed().as_secs_f64().max(1.0);
    let t = stats.traffic;
    out.push(format!(
        "traffic: sent {} B in {} datagrams ({:.0} B/s), received {} B in {} datagrams ({:.0} B/s), UDP payload",
        t.sent_bytes,
        t.sent_datagrams,
        t.sent_bytes as f64 / uptime,
        t.recv_bytes,
        t.recv_datagrams,
        t.recv_bytes as f64 / uptime,
    ));
    out.push(format!(
        "chat: received {}, dropped {} (over a rate or the chat off), floods sent away {}, lines sent {}{}",
        stats.chat_received,
        stats.chat_dropped,
        stats.chat_floods,
        stats.chat_sent,
        if world.resource::<ssx_net::server::ChatConfig>().enabled { "" } else { "; the players' chat is off" },
    ));
    crate::rounds::status(world, stats, &mut out);
    out.push(format!(
        "poses: received {}, relayed {} (near {}, middle {}, far {}), not due for a tier {}, stale {}, older epoch {}, \
         over the rate {}, before the look {}",
        stats.poses_received,
        stats.poses_relayed,
        stats.poses_by_tier[0],
        stats.poses_by_tier[1],
        stats.poses_by_tier[2],
        stats.poses_decimated,
        stats.poses_stale,
        stats.poses_older_epoch,
        stats.poses_over_rate,
        stats.poses_before_look,
    ));
    out.extend(world.get_resource::<crate::discovery::Status>().map(|d| d.lines()).unwrap_or_default());
    out
}

pub fn console_commands(world: &mut World) {
    let Some(lines) = world.get_resource::<ConsoleLines>() else { return };
    let lines: Vec<String> = match lines.0.lock() {
        Ok(receiver) => receiver.try_iter().take(64).collect(),
        Err(_) => return,
    };
    for line in lines {
        match console::parse(&line) {
            None => {}
            Some(Err(e)) => say_to_operator(&e),
            Some(Ok(command)) => {
                for line in run_command_for(world, command, &crate::operation::commands::Caller::Console).1 {
                    say_to_operator(&line);
                }
            }
        }
    }
}

pub(crate) fn run_command_for(
    world: &mut World,
    command: Command,
    caller: &crate::operation::commands::Caller,
) -> (bool, Vec<String>) {
    let mut lines = Vec::new();
    let ok = run_command(world, command, caller, &mut |text: &str| lines.push(text.to_string()));
    (ok, lines)
}

fn run_command(
    world: &mut World,
    command: Command,
    caller: &crate::operation::commands::Caller,
    say_to_operator: &mut dyn FnMut(&str),
) -> bool {
    let facts = world.resource::<ServerFacts>().clone();
    let shared = Arc::clone(&world.resource::<Bridge>().shared);
    match command {
        Command::Help => {
            say_to_operator(console::HELP);
            say_to_operator(&format!("operation: {}", ssx_net_ops::command::HELP));
        }
        Command::Resource(c) => match world.get_resource_mut::<crate::resources::ServerResources>() {
            Some(mut r) => match r.push(c.clone()) {
                Ok(n) => say_to_operator(&match &c {
                    crate::resources::ResourceCommand::Restart(name) => format!(
                        "restart {name} queued ({n} waiting): until the log's \"restart {name} done\", a command \
                         may still run on the running {name}"
                    ),
                    _ => format!("queued {c:?} ({n} waiting)"),
                }),
                Err(e) => say_to_operator(&e),
            },
            None => say_to_operator("this server runs no resources"),
        },
        Command::Blocklists => match world.get_resource_mut::<crate::blocklist::ServerBlocklists>() {
            Some(mut b) => {
                let mut lines = Vec::new();
                b.status(&mut lines);
                for line in &lines {
                    say_to_operator(line);
                }
                say_to_operator(if b.ask_now() {
                    "blocklists: the master asked now"
                } else {
                    "blocklists: asked within the last 10 s; not asked again"
                });
            }
            None => say_to_operator("blocklists: not followed"),
        },
        Command::Operation(op) => {
            let (ok, lines) = crate::operation::commands::run(world, op, caller);
            for line in &lines {
                say_to_operator(line);
            }
            return ok;
        }
        Command::Scripts => match world.get_resource::<crate::scripts::ServerScripts>() {
            Some(s) => {
                let mut lines = Vec::new();
                s.status(&mut lines);
                if let Some(st) = world.get_resource::<crate::scripts::ScriptStats>() {
                    lines.push(st.line());
                }
                if let Some(st) = world.get_resource::<ssx_net::server::ScriptNetStats>() {
                    lines.push(st.line());
                }
                if let Some(o) = world.get_resource::<crate::scripts::OrderStats>() {
                    lines.push(format!(
                        "scripts: privileged orders applied {}, not applied {}; permission requests recorded {}",
                        o.applied, o.refused, o.requests_recorded
                    ));
                }
                if let Some(h) = world.get_resource::<crate::scripts::HeldJoins>() {
                    lines.push(format!(
                        "scripts: joins with the scripts now {}; answered held {}, let through {}, refused {}, given up \
                         by their clients {}",
                        h.held(),
                        h.held_answered,
                        h.let_through,
                        h.refused,
                        h.given_up
                    ));
                }
                lines.iter().for_each(|l| say_to_operator(l));
            }
            None => say_to_operator("this server runs no scripts"),
        },
        Command::Script(line) => match world.get_resource::<crate::scripts::ServerScripts>() {
            Some(s) => {
                if let Err(e) = crate::scripts::console(s, &line, None) {
                    say_to_operator(&e);
                }
            }
            None => say_to_operator(&format!("unknown command {line:?}; {}", console::HELP)),
        },
        Command::Stop => {
            if shared.stopping.load(Ordering::Acquire) {
                say_to_operator("stopping now: the rounds under way are not waited for");
                shared.stop_now.store(true, Ordering::Release);
                return true;
            }
            say_to_operator("stopping");
            shared.stop_requested.store(true, Ordering::Release);
        }
        Command::Say(text) => match ServerChat::server(&text) {
            Ok(line) => {
                world.resource_mut::<ChatOutbox>().0.push(line);
                info!(target: CHAT_LOG, "chat: the server: {text}");
                say_to_operator(&format!("said: {text}"));
            }
            Err(e) => {
                say_to_operator(&format!("not said: {e}"));
                return false;
            }
        },
        Command::Status => {
            let mut lines = status(world, &facts, &shared, world.resource::<TickTimes>());
            if let Some(r) = world.get_resource::<crate::resources::ServerResources>() {
                r.status(&mut lines);
            }
            if let Some(b) = world.get_resource::<crate::blocklist::ServerBlocklists>() {
                b.status(&mut lines);
            }
            let mut query = world.query::<(&PlayerSession, &RiderLink, Has<Leaving>)>();
            let mut riders: Vec<_> = query
                .iter(world)
                .map(|(s, r, l)| (r.rider().0, s.name.clone(), s.id.short(), canonical(s.address), l))
                .collect();
            riders.sort();
            lines.push(format!("{} riders:", riders.len()));
            for (rider, name, id, address, leaving) in riders {
                lines
                    .push(format!("  {rider:>3} {name:<20} {id} {address}{}", if leaving { " (leaving)" } else { "" }));
            }
            for line in lines {
                say_to_operator(&line);
            }
        }
        Command::Kick { who, reason } => {
            let mut query = world.query_filtered::<(Entity, &PlayerSession, &RiderLink), Without<Leaving>>();
            let matches: Vec<(Entity, String)> = query
                .iter(world)
                .filter(|(_, s, r)| {
                    who.parse::<u16>().ok() == Some(r.rider().0)
                        || (who.len() >= 8
                            && who.chars().all(|c| c.is_ascii_hexdigit())
                            && s.id.to_hex().starts_with(&who.to_ascii_lowercase()))
                        || s.name == who
                })
                .map(|(e, s, _)| (e, s.name.clone()))
                .collect();
            match matches.as_slice() {
                [] => {
                    say_to_operator(&format!("kick: no rider {who:?}"));
                    return false;
                }
                [(link, name)] => {
                    let text = if reason.is_empty() { "kicked by the operator".to_string() } else { reason };
                    let now = world.resource::<Time<Real>>().elapsed();
                    let mut commands = world.commands();
                    leave(&mut commands, *link, KickReason::Operator, &text, now);
                    world.flush();
                    say_to_operator(&format!("kicked {name}: {text}"));
                }
                more => {
                    say_to_operator(&format!("kick: {who:?} names {} riders; use the rider number", more.len()));
                    return false;
                }
            }
        }
    }
    true
}

#[derive(Resource, Default)]
pub enum Lifecycle {
    #[default]
    Running,
    Ending {
        since: Duration,
    },
    Stopping {
        since: Duration,
        stopped_at: Option<Duration>,
    },
}

// PORT(choice): 2 s, twice a polite end's grace (LEAVE_GRACE, 1 s) so that the goodbye can be resent under loss;
// tuning: phase 4's stop, which also posts the round's results first; the original has no dedicated server.
pub const STOP_GRACE: Duration = Duration::from_secs(2);

pub fn lifecycle(
    mut life: ResMut<Lifecycle>,
    (bridge, facts, time): (Res<Bridge>, Res<ServerFacts>, Res<Time<Real>>),
    links: Query<Entity, With<ClientOf>>,
    sessions: Query<Entity, (With<PlayerSession>, Without<Kicked>)>,
    mut exit: MessageWriter<AppExit>,
    mut commands: Commands,
) {
    let now = time.elapsed();
    match *life {
        Lifecycle::Running => {
            if bridge.shared.stop_requested.load(Ordering::Acquire) {
                bridge.shared.stopping.store(true, Ordering::Release);
                info!("stopping: {} riders", sessions.iter().count());
                *life = Lifecycle::Ending { since: now };
            }
        }
        Lifecycle::Ending { .. } => {}
        Lifecycle::Stopping { since, stopped_at: None } => {
            if links.is_empty() || now.saturating_sub(since) >= STOP_GRACE {
                commands.trigger(Stop { entity: facts.server_entity });
                *life = Lifecycle::Stopping { since, stopped_at: Some(now) };
            }
        }
        Lifecycle::Stopping { stopped_at: Some(at), .. } => {
            if now.saturating_sub(at) >= Duration::from_millis(100) {
                exit.write(AppExit::Success);
            }
        }
    }
}

fn who(sessions: &Query<&PlayerSession>, link: Entity) -> String {
    match sessions.get(link) {
        Ok(s) => who_text(&s.name, &s.id.short(), s.address),
        Err(_) => format!("link {link}"),
    }
}

fn who_text(name: &str, id: &str, address: std::net::IpAddr) -> String {
    format!("{name:?} ({id}) from {}", canonical(address))
}

pub fn log_kick(
    kicked: On<Add<Kicked>>,
    links: Query<(&Kicked, Option<&RiderLink>)>,
    sessions: Query<&PlayerSession>,
    stats: Res<ServerStats>,
) {
    if let Ok((k, rider)) = links.get(kicked.entity) {
        let detail = match (k.0, rider) {
            (KickReason::ControlOrder | KickReason::Round(_), Some(r)) => stats
                .refusals
                .iter()
                .rev()
                .find(|(n, _)| *n == r.rider().0)
                .map_or(String::new(), |(_, d)| format!(" ({d})")),
            _ => String::new(),
        };
        info!("ended the connection of {}: {:?}{detail}", who(&sessions, kicked.entity), k.0);
    }
}

pub fn log_leave(leaving: On<Add<Leaving>>, links: Query<&Leaving>, sessions: Query<&PlayerSession>) {
    if let Ok(l) = links.get(leaving.entity) {
        info!("ending the connection of {}: {:?} ({})", who(&sessions, leaving.entity), l.reason, l.text);
    }
}

pub fn log_disconnect(
    ended: On<Add<Disconnected>>,
    links: Query<(&Disconnected, Option<&DisconnectCause>), NotEndedByServer>,
    sessions: Query<&PlayerSession>,
) {
    if let Ok((d, cause)) = links.get(ended.entity) {
        let end = LinkEnd::of(cause);
        info!("the connection of {} ended: {} ({:?})", who(&sessions, ended.entity), end.describe(), d.reason);
    }
}

pub fn report_suppressed(mut last: Local<(u64, Option<Instant>)>) {
    let now = Instant::now();
    if last.1.is_some_and(|at| now.duration_since(at) < Duration::from_secs(60)) {
        return;
    }
    last.1 = Some(now);
    let total = SUPPRESSED_LINES.load(Ordering::Relaxed);
    if total > last.0 {
        warn!("{} log lines suppressed by the rate limit in the last minute", total - last.0);
        last.0 = total;
    }
}

pub const CHAT_LOG: &str = "avalanche_server::chat";

pub fn log_chat(mut log: ResMut<ChatLog>) {
    for l in log.lines.drain(..) {
        let id: String = l.key_id.iter().map(|b| format!("{b:02x}")).collect();
        match &l.line {
            ClientChat::Text(text) => info!(target: CHAT_LOG, "chat: {} [{id}] {}: {text}", l.rider, l.name),
            ClientChat::Quick(i) => info!(target: CHAT_LOG, "chat: {} [{id}] {}: quick line {i}", l.rider, l.name),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_ipv4_mapped_address_is_logged_as_ipv4() {
        let mapped: std::net::IpAddr = "::ffff:192.0.2.7".parse().expect("an address");
        assert_eq!(who_text("Elise", "ab12", mapped), "\"Elise\" (ab12) from 192.0.2.7");
        let v6: std::net::IpAddr = "2001:db8::7".parse().expect("an address");
        assert_eq!(who_text("Elise", "ab12", v6), "\"Elise\" (ab12) from 2001:db8::7");
    }
}
