use std::net::SocketAddr;
use std::path::Path;
use std::time::{Duration, Instant};

use bevy::prelude::*;
use ssx_net::lightyear::prelude::{Client, Connect, Connected, Disconnect, Disconnected, Link};
use ssx_net::{ClientRoster, RemotePoses, ServerSaid};
use ssx_net_join::browse::{JoinRequest, JoinTask, ServerAddress, Zeroizing};
use ssx_net_proto::{GhostStyle, Rewinds};
use ssx_ui::game::fe::ava::browser::NetProgress;

use crate::ride::live::{LiveRewinds, LiveRules};
use crate::shell::game_mode::{GameUiRes, NetConnect};
use crate::shell::live::SessionLevel;
use crate::shell::live::{LiveEnd, LiveLevel, LiveOrigin, LiveSession};

const MAX_PASSWORD_FILE: u64 = 1024;

// PORT(choice): 60 s (the boot movies run to their end unless skipped, the sign-in waits for its job); tuning: the
// launch times of real sessions from a cold start; the original has no real-time multiplayer.
const LAUNCH_WAIT: Duration = Duration::from_secs(60);

enum State {
    Joining(JoinTask),
    Granted { server: SocketAddr, granted: Box<ssx_net_join::Granted> },
    Connecting { client: Entity },
    Connected { client: Entity, launched: Option<Instant> },
    Ended,
}

#[derive(Resource)]
pub struct NetSession {
    server: String,
    state: State,
    pub hologram: bool,
    joining: Option<Joining>,
}

struct Joining {
    slot: crate::shell::session_content::MountSlot,
    progress: std::sync::Arc<ssx_net_join::resources::Progress>,
    options: ssx_net_join::resources::ResourceOptions,
    shown: Option<(u64, u64, ssx_net_join::resources::Step)>,
}

pub struct JoinSpec {
    pub address: ServerAddress,
    pub name: String,
    pub password: Zeroizing<Vec<u8>>,
    pub origin: LiveOrigin,
    pub key: Option<ssx_net_proto::keys::ServerKey>,
}

fn read_password(path: &Path) -> Result<Vec<u8>, String> {
    use std::io::Read;
    let file = std::fs::File::open(path).map_err(|e| format!("the password file {}: {e}", path.display()))?;
    let mut bytes = Vec::new();
    file.take(MAX_PASSWORD_FILE + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("the password file {}: {e}", path.display()))?;
    if bytes.len() as u64 > MAX_PASSWORD_FILE {
        return Err(format!("the password file {} is longer than {MAX_PASSWORD_FILE} bytes", path.display()));
    }
    if bytes.ends_with(b"\r\n") {
        bytes.truncate(bytes.len() - 2);
    } else if bytes.ends_with(b"\n") {
        bytes.pop();
    }
    if bytes.is_empty() {
        return Err(format!("the password file {} holds no password", path.display()));
    }
    if bytes.len() > ssx_net_proto::join::MAX_PASSWORD_BYTES || bytes.iter().any(|b| b.is_ascii_control()) {
        return Err(format!(
            "the password in {} has more than {} bytes or a control character",
            path.display(),
            ssx_net_proto::join::MAX_PASSWORD_BYTES
        ));
    }
    Ok(bytes)
}

pub fn stored_player_name() -> Option<String> {
    use ssx_ui::game::fe::ava::port_settings::{PortSettings, read_file};
    let path = ssx_assets::setup::app_dir().join("settings.json");
    read_file(&path).ok().and_then(|m| PortSettings::from_json(&serde_json::Value::Object(m)).0.player_name)
}

pub fn spec_of(connect: &NetConnect, name: String) -> Result<JoinSpec, String> {
    let address = ServerAddress::parse(connect.server.trim(), Some(ssx_net_proto::DEFAULT_PORT))
        .map_err(|e| format!("the server {}: {e}", connect.server))?;
    let password = match &connect.password_file {
        Some(p) => read_password(p)?,
        None => Vec::new(),
    };
    let name = connect.name.clone().unwrap_or(name);
    Ok(JoinSpec { address, name, password: Zeroizing::new(password), origin: LiveOrigin::Hub, key: None })
}

impl NetSession {
    pub fn start(spec: JoinSpec, big_root: &Path) -> Result<Self, String> {
        let server = spec.address.to_string();
        let app_dir = ssx_assets::setup::app_dir();
        let settings = crate::shell::session_content::stored_resource_settings();
        let (with, slot) = crate::shell::session_content::resource_join(big_root, &app_dir, settings);
        let (progress, options) = (with.progress.clone(), with.options.clone());
        let req = JoinRequest { address: spec.address, name: spec.name, password: spec.password, app_dir };
        let task = JoinTask::spawn(req, spec.key, with).map_err(|e| format!("the join's thread: {e}"))?;
        info!("network: joining {server}");
        let joining = Some(Joining { slot, progress, options, shown: None });
        Ok(Self { server, state: State::Joining(task), hologram: false, joining })
    }

    pub fn ended(server: String) -> Self {
        Self { server, state: State::Ended, hologram: false, joining: None }
    }

    pub fn client(&self) -> Option<Entity> {
        match self.state {
            State::Connecting { client } | State::Connected { client, .. } => Some(client),
            _ => None,
        }
    }

    pub fn connected(&self) -> bool {
        matches!(self.state, State::Connected { .. })
    }

    pub fn is_ended(&self) -> bool {
        matches!(self.state, State::Ended)
    }

    pub fn cancel(&mut self) -> Option<Entity> {
        info!("network: the join to {} cancelled", self.server);
        self.joining = None;
        match std::mem::replace(&mut self.state, State::Ended) {
            State::Joining(task) => {
                task.cancel();
                None
            }
            State::Connecting { client } | State::Connected { client, .. } => Some(client),
            State::Granted { .. } | State::Ended => None,
        }
    }
}

fn end(session: &mut NetSession, live: &mut LiveSession, reason: Option<String>, commands: &mut Commands) {
    end_refused(session, live, reason, None, commands);
}

fn end_refused(
    session: &mut NetSession,
    live: &mut LiveSession,
    reason: Option<String>,
    refusal: Option<ssx_net_proto::join::RefusalCode>,
    commands: &mut Commands,
) {
    if let Some(client) = session.client() {
        commands.trigger(Disconnect { entity: client });
    }
    match &reason {
        Some(r) => warn!("network: the session with {} ends: {r}", session.server),
        None => info!("network: the session with {} ends (the player left)", session.server),
    }
    session.state = State::Ended;
    session.joining = None;
    if live.end.is_none() {
        live.end = Some(LiveEnd { reason, refusal });
    }
}

fn downloading(j: &mut Joining, ui: &mut GameUiRes) {
    use std::sync::atomic::Ordering;
    let (received, total) = (j.progress.received.load(Ordering::Relaxed), j.progress.total.load(Ordering::Relaxed));
    let step = j.progress.step();
    if total == 0 || j.shown == Some((received, total, step)) {
        return;
    }
    j.shown = Some((received, total, step));
    let lines = j.progress.lines.lock().map(|l| l.clone()).unwrap_or_default();
    ui.net_progress(NetProgress::Downloading { lines, received, total, step });
}

pub(super) fn session_level(
    ui: &GameUiRes,
    level: &ssx_net_proto::instance::Level,
    mode: ssx_net_proto::Mode,
) -> Result<SessionLevel, String> {
    use ssx_net_proto::instance::Level;
    match level {
        Level::Drop(d) => check_drop(ui, d, mode).map(|()| SessionLevel::Drop(d.clone())),
        Level::Map { id, event_type, gamemode } => {
            let mode = match gamemode {
                Some(g) => g.clone(),
                None => ssx_net_proto::instance::built_in_mode(*event_type)
                    .ok_or_else(|| format!("the server's map {id} runs the event type {event_type}, no map mode"))?
                    .to_string(),
            };
            Ok(SessionLevel::Map { id: id.clone(), event_type: *event_type, mode })
        }
    }
}

fn log_blocklists(j: Option<&Joining>) {
    let Some(j) = j else { return };
    let lines = std::mem::take(&mut *j.progress.blocklists.lock().unwrap_or_else(std::sync::PoisonError::into_inner));
    for line in lines {
        info!("network: {line}");
    }
}

pub(super) fn check_drop(ui: &GameUiRes, drop: &str, mode: ssx_net_proto::Mode) -> Result<(), String> {
    use ssx_net_proto::Mode;
    let Some(fe) = ui.fe.as_ref() else { return Err("the game has no front end to start a drop from".into()) };
    let Some(event) = fe.career.event_named(drop) else {
        return Err(format!("the server's drop {drop} is not on this game's disc"));
    };
    if ssx_ui::game::fe::globe::TUTORIAL_DROPS.contains(&event.key) {
        return Err(format!("the server's drop {drop} is a tutorial drop, which multiplayer does not play"));
    }
    let fits = match mode {
        Mode::Freeride => true,
        Mode::Race => (5..=7).contains(&event.event_type),
        Mode::Trick => (0..=2).contains(&event.event_type),
        Mode::Survive => (3..=4).contains(&event.event_type),
    };
    if !fits {
        let is = match event.event_type {
            0..=2 => "a trick event",
            3..=4 => "a survive event",
            5..=7 => "a race event",
            8 => "a freeride event",
            _ => "an event of no known type",
        };
        return Err(format!("the server's drop {drop} is not a {} event: it is {is}", mode.name()));
    }
    Ok(())
}

type ClientState<'a> = (Has<Connected>, Option<&'a Disconnected>);

pub fn drive(
    mut session: ResMut<NetSession>,
    mut live: ResMut<LiveSession>,
    (said, poses, roster): (Res<ServerSaid>, Res<RemotePoses>, Res<ClientRoster>),
    clients: Query<ClientState, With<Client>>,
    mut ui: ResMut<GameUiRes>,
    mut commands: Commands,
) {
    let s = &mut *session;
    if s.is_ended() {
        return;
    }
    if live.end.is_some() {
        end(s, &mut live, None, &mut commands);
        return;
    }
    if let Some(e) = super::errors::take() {
        end(s, &mut live, Some(format!("the network session failed: {e}")), &mut commands);
        return;
    }
    if let (State::Joining(_), Some(j)) = (&s.state, s.joining.as_mut()) {
        downloading(j, &mut ui);
    }
    if let State::Joining(task) = &s.state
        && let Some(text) = task.held()
    {
        ui.net_progress(NetProgress::Held { text });
    }
    match &s.state {
        State::Joining(task) => match task.poll() {
            Some(Ok(joined)) => {
                log_blocklists(s.joining.as_ref());
                #[cfg(feature = "scripts")]
                if joined.resources.is_none() {
                    live.content.client_scripts = ssx_content::policy::ClientScripts::Unstated;
                }
                if let (Some(j), Some(fetched)) = (s.joining.take(), joined.resources) {
                    #[cfg(feature = "scripts")]
                    let server_key = joined.granted.info.identity_key.clone();
                    #[cfg(feature = "scripts")]
                    {
                        let app_dir = ssx_assets::setup::app_dir();
                        let allowed = crate::shell::session_content::stored_resource_settings().server_scripts;
                        match crate::shell::session_content::live_content(&fetched, &server_key, &app_dir, allowed) {
                            Ok(c) => live.content = c,
                            Err(e) => {
                                let why = format!("could not join {}: the server's scripts: {e}", s.server);
                                return end(s, &mut live, Some(why), &mut commands);
                            }
                        }
                    }
                    let server = joined.server;
                    let content = crate::shell::session_content::SessionContent {
                        slot: j.slot,
                        fetched,
                        server,
                        #[cfg(feature = "scripts")]
                        server_key,
                        options: j.options,
                    };
                    commands.insert_resource(content);
                }
                let granted = joined.granted;
                if joined.identity_created {
                    info!("network: made this install's player identity");
                }
                info!(
                    "network: joined {} (drop {}, the server's key {:?}): connecting",
                    granted.info.name, granted.info.drop, granted.trust
                );
                live.server = Some(granted.info.name.clone());
                let max = u16::try_from(granted.info.max_players).ok();
                ui.session_begin(&granted.info.name, max);
                ui.net_progress(NetProgress::Joined { name: granted.info.name.clone() });
                s.state = State::Granted { server: joined.server, granted };
            }
            Some(Err(f)) if f.cancelled => {
                log_blocklists(s.joining.as_ref());
                s.state = State::Ended;
                s.joining = None;
            }
            Some(Err(f)) => {
                log_blocklists(s.joining.as_ref());
                let why = format!("could not join {}: {}", s.server, f.text);
                end_refused(s, &mut live, Some(why), f.refusal, &mut commands)
            }
            None => {}
        },
        State::Connecting { client } | State::Connected { client, .. } => {
            let client = *client;
            let gone = clients.get(client).map_or(Some(None), |(connected, d)| {
                (!connected && d.is_some()).then(|| d.map(|d| d.reason.to_string()))
            });
            if let Some(r) = poses.refused.clone() {
                end(
                    s,
                    &mut live,
                    Some(format!("the server sent what the protocol does not allow: {r}")),
                    &mut commands,
                );
                return;
            }
            if let Some((reason, text)) = said.goodbye.clone()
                && (gone.is_some() || matches!(s.state, State::Connected { .. }))
            {
                let why = if text.is_empty() { format!("{reason:?}") } else { format!("{reason:?}: {text}") };
                let why = match said.ban {
                    Some(until) => {
                        let now = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map_or(0, |d| d.as_secs());
                        format!("{why}; {}", ssx_net_proto::operator::ban_end(until, now))
                    }
                    None => why,
                };
                end(s, &mut live, Some(format!("the server ended the connection ({why})")), &mut commands);
                return;
            }
            if let Some(reason) = gone {
                let why = reason.unwrap_or_else(|| "the connection is gone".into());
                end(s, &mut live, Some(format!("the server stopped answering ({why})")), &mut commands);
                return;
            }
            let connected = clients.get(client).is_ok_and(|(c, _)| c);
            if let State::Connecting { .. } = s.state
                && connected
            {
                info!("network: connected to {}", s.server);
                s.state = State::Connected { client, launched: None };
            }
            if let State::Connected { launched, .. } = &mut s.state {
                if launched.is_none()
                    && let (Some(_), Some(instance)) = (roster.own, roster.instance.as_ref())
                {
                    let spec = &instance.spec;
                    let level = match session_level(&ui, &spec.level, spec.mode) {
                        Ok(l) => l,
                        Err(e) => {
                            end(s, &mut live, Some(e), &mut commands);
                            return;
                        }
                    };
                    let rewinds = match spec.rewinds {
                        Rewinds::On => LiveRewinds::Event,
                        Rewinds::Off => LiveRewinds::Off,
                        Rewinds::Limit(n) => LiveRewinds::Limit(n),
                    };
                    live.rules =
                        Some(LiveRules { explore_ghosts: spec.ghosts, rewinds, rounds: spec.mode.has_rounds() });
                    live.launch = Some(level.clone());
                    live.ridden = Some(level);
                    ui.net_progress(NetProgress::Launching);
                    s.hologram = spec.ghost_style == GhostStyle::Hologram;
                    *launched = Some(Instant::now());
                    info!("network: the instance: {} on {} ({spec:?})", spec.mode.name(), spec.level);
                }
                if let Some(at) = *launched
                    && live.level == LiveLevel::None
                    && at.elapsed() > LAUNCH_WAIT
                {
                    let why = format!("the game did not start the server's level within {} s", LAUNCH_WAIT.as_secs());
                    end(s, &mut live, Some(why), &mut commands);
                }
            }
        }
        State::Granted { .. } | State::Ended => {}
    }
}

pub fn connect_granted(world: &mut World) {
    let granted = world.get_resource_mut::<NetSession>().and_then(|mut s| match &s.state {
        State::Granted { .. } => match std::mem::replace(&mut s.state, State::Ended) {
            State::Granted { server, granted } => Some((server, granted)),
            _ => None,
        },
        _ => None,
    });
    let Some((server, granted)) = granted else { return };
    let local = match server {
        SocketAddr::V4(a) if a.ip().is_loopback() => SocketAddr::from(([127, 0, 0, 1], 0)),
        SocketAddr::V4(_) => SocketAddr::from(([0, 0, 0, 0], 0)),
        SocketAddr::V6(a) if a.ip().is_loopback() => SocketAddr::from((std::net::Ipv6Addr::LOCALHOST, 0)),
        SocketAddr::V6(_) => SocketAddr::from((std::net::Ipv6Addr::UNSPECIFIED, 0)),
    };
    let made = ssx_net::connect_token(&granted.token, server).and_then(|auth| {
        let link = Link::default().with_conditioner(super::test_conditioner());
        ssx_net::client::spawn_udp_client(world, local, server, auth, link).map_err(|e| e.to_string())
    });
    match made {
        Ok(client) => {
            world.trigger(Connect { entity: client });
            if let Some(mut s) = world.get_resource_mut::<NetSession>() {
                s.state = State::Connecting { client };
            }
        }
        Err(e) => {
            warn!("network: the session ends: the server's connect token: {e}");
            if let Some(mut live) = world.get_resource_mut::<LiveSession>() {
                let reason = Some(format!("the server's connect token: {e}"));
                live.end.get_or_insert(LiveEnd { reason, refusal: None });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn file(tag: &str, bytes: &[u8]) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ssx-password-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("the test's directory");
        let path = dir.join("password.txt");
        std::fs::write(&path, bytes).expect("the test's file");
        path
    }

    #[test]
    fn a_password_file_gives_its_first_line_without_its_line_end() {
        assert_eq!(read_password(&file("lf", b"secret pw\n")), Ok(b"secret pw".to_vec()));
        assert_eq!(read_password(&file("crlf", b"secret pw\r\n")), Ok(b"secret pw".to_vec()));
        assert_eq!(read_password(&file("plain", b"secret pw")), Ok(b"secret pw".to_vec()));
        let longest = vec![b'p'; ssx_net_proto::join::MAX_PASSWORD_BYTES];
        assert_eq!(read_password(&file("longest", &longest)), Ok(longest));
    }

    #[test]
    fn a_missing_empty_oversized_or_malformed_password_file_is_refused() {
        let missing = std::env::temp_dir().join(format!("ssx-password-none-{}/password.txt", std::process::id()));
        let refused = |path: &Path, why: &str| {
            let e = read_password(path).expect_err("refused");
            assert!(e.contains(why) && e.contains(&path.display().to_string()), "{e}");
        };
        refused(&missing, "the password file");
        refused(&file("empty", b""), "holds no password");
        refused(&file("newline", b"\n"), "holds no password");
        refused(&file("crlf-only", b"\r\n"), "holds no password");
        refused(&file("mebibyte", &vec![b'p'; 1 << 20]), "longer than 1024 bytes");
        refused(&file("long", &[b'p'; ssx_net_proto::join::MAX_PASSWORD_BYTES + 1]), "more than 64 bytes");
        refused(&file("two-lines", b"secret\npw\n"), "a control character");
        refused(&file("tab", b"secret\tpw"), "a control character");
    }
}
