mod chat;
mod cycle;
mod ends;
mod gate;
mod guard;
mod hold;
mod instance;
mod ping;
mod relay;
mod resources;
mod scripts;
mod session;
mod time;
mod validation;

use std::collections::{BTreeSet, VecDeque};
use std::net::SocketAddr;

use bevy_app::{App, Last, Plugin, PostUpdate, PreUpdate};
use bevy_ecs::prelude::*;
use bevy_time::{Real, Time, Virtual};
use lightyear::connection::ConnectionSystems;
use lightyear::link::{LinkSystems, RecvLinkConditioner};
use lightyear::netcode::TokenUserData;
use lightyear::netcode::prelude::server::{NetcodeConfig, NetcodeServer};
use lightyear::prelude::server::{ClientOf, ServerPlugins, ServerUdpIo};
use lightyear::prelude::{
    Connected, Endpoint, LocalAddr, PeerAddr, PeerId, RemoteId, ReplicationSender, RoomAllocator, RoomId, RoomPlugin,
};
use ssx_net_proto::{RiderId, netcode_protocol_id};

use crate::limits::MAX_CLIENTS;
use crate::protocol::ProtocolPlugin;
use crate::tick::{NetSystems, tick_duration};

pub use crate::traffic::LinkTraffic;
pub use crate::traffic::Traffic;
pub use chat::InstanceOutbox;
pub use chat::{ChatConfig, ChatLink, ChatLog, ChatLogLine, ChatOutbox};
pub use cycle::{RoundEvent, RoundLog};
pub use ends::{LinkEnd, NotEndedByServer};
pub use gate::{Decision, Operator, OperatorGate};
pub use guard::{GuardSystems, KickReason, Kicked, Leaving, PendingSince, PoseRule, leave};
pub use hold::{HELD_ENTRY_BYTES, MAX_HELD, MAX_HELD_BYTES};
pub use instance::{INSTANCE_ID, InstanceConfig, InstanceSetup, Instances, PlacedIn, PlayerEntity, ServerInstance};
pub use relay::{RelayRates, RiderLink};
pub use resources::{ResourcesNow, StandIns, Told};
pub use scripts::{
    BYTE_FLOOD, BYTES, Bucket, EVENTS, FLOOD, HELD_BYTES, HELD_MESSAGES, LATENT_DOWN_BUCKET, LATENT_UP_BUCKET,
    MAX_REPORTS, QUEUE_BYTES, ReportEvent, SEND_BUDGET, ScriptInbox, ScriptLink, ScriptNetStats, ScriptReports,
    ScriptSessions, Served, Shared, Shed, TOTAL_QUEUE_BYTES, WRITES, serve, shed,
};
pub use session::{Claimed, IssueError, Issued, PlayerSession, Sessions, TicketRefusal, TicketRequest};
pub use time::{
    CATCH_UP, CLOCK_RATE_TOLERANCE, JITTER_MARGIN_TICKS, JITTER_MULTIPLE, LONG_FRAME, LinkTime, Received, ServerTime,
    UNALIGNED_GO_TICKS, horizon,
};
pub use validation::{
    CheckOutcomes, LimitsLoader, ValidationConfig, ValidationEvent, ValidationLog, ValidationMeasures,
};

pub struct NetServerPlugin;

impl Plugin for NetServerPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(ServerPlugins { tick_duration: tick_duration() });
        app.add_plugins((RoomPlugin, ProtocolPlugin));
        app.init_resource::<ServerStats>().init_resource::<RiderIds>();
        app.init_resource::<ChatConfig>().init_resource::<ChatOutbox>().init_resource::<ChatLog>();
        app.init_resource::<Sessions>();
        app.init_resource::<InstanceOutbox>();
        app.init_resource::<relay::RelayState>().init_resource::<RoundLog>();
        app.init_resource::<ServerTime>().init_resource::<instance::Holds>();
        app.init_resource::<ResourcesNow>();
        app.init_resource::<StandIns>();
        app.init_resource::<ScriptInbox>().init_resource::<ScriptSessions>().init_resource::<ScriptNetStats>();
        app.init_resource::<ScriptReports>();
        app.add_observer(scripts::on_session).add_observer(scripts::on_link_removed);
        app.add_systems(
            PreUpdate,
            (scripts::receive.after(GuardSystems::Direction), scripts::drop_leaving).in_set(NetSystems::Receive),
        );
        app.add_systems(PostUpdate, scripts::send.in_set(NetSystems::Send).after(instance::send_welcomes));
        app.add_observer(on_connected).add_observer(on_rider_removed).add_observer(on_session_removed);
        app.add_observer(instance::on_player_removed).add_observer(ends::count_ends);
        guard::register(app);
        app.add_systems(
            PreUpdate,
            relay::relay
                .in_set(NetSystems::Receive)
                .after(GuardSystems::Direction)
                .after(instance::receive_client_control),
        );
        app.add_systems(
            PreUpdate,
            instance::receive_client_control.in_set(NetSystems::Receive).after(GuardSystems::Direction),
        );
        app.add_systems(
            PreUpdate,
            chat::receive_chat.in_set(NetSystems::Receive).after(GuardSystems::Direction).after(relay::relay),
        );
        app.add_systems(
            PreUpdate,
            (relay::drop_leaving, instance::drop_leaving_control, chat::drop_leaving_chat).in_set(NetSystems::Receive),
        );
        app.add_systems(PreUpdate, expire_tickets.in_set(NetSystems::Receive));
        app.add_systems(
            PreUpdate,
            validation::run.in_set(NetSystems::Receive).after(instance::receive_client_control).after(relay::relay),
        );
        app.add_systems(
            PreUpdate,
            (cycle::drive, cycle::sync_players)
                .chain()
                .in_set(NetSystems::Receive)
                .after(instance::receive_client_control)
                .after(relay::relay)
                .after(validation::run),
        );
        app.add_systems(PostUpdate, (instance::send_welcomes, ping::publish_pings).in_set(NetSystems::Send));
        app.add_systems(PostUpdate, resources::tell_changes.in_set(NetSystems::Send).after(instance::send_welcomes));
        app.add_systems(Last, time::track);
        app.add_systems(
            PostUpdate,
            crate::traffic::count_sent::<With<ClientOf>, ServerStats>
                .after(ConnectionSystems::Send)
                .before(LinkSystems::Send),
        );
        app.add_systems(
            PreUpdate,
            crate::traffic::count_received::<With<ClientOf>, ServerStats>
                .after(LinkSystems::Receive)
                .before(ConnectionSystems::Receive),
        );
    }

    fn finish(&self, app: &mut App) {
        app.world_mut().get_resource_or_init::<Time<Virtual>>().set_max_delta(time::CATCH_UP);
        let lobby = app.world_mut().resource_mut::<RoomAllocator>().allocate();
        app.insert_resource(Lobby(lobby));
        instance::start(app.world_mut(), lobby);
        validation::start(app.world_mut());
    }
}

#[derive(Resource, Clone, Copy, Debug)]
pub struct Lobby(pub RoomId);

#[derive(Resource, Clone, Debug, Default, PartialEq, Eq)]
pub struct ServerStats {
    pub links: u64,
    pub connected: u64,
    pub sessions: u64,
    pub pending_reaped: u64,
    pub kicked: u64,
    pub left: u64,
    pub timed_out: u64,
    pub kick_reasons: VecDeque<KickReason>,
    pub poses_received: u64,
    pub poses_relayed: u64,
    pub poses_by_tier: [u64; 3],
    pub poses_decimated: u64,
    pub poses_stale: u64,
    pub poses_older_epoch: u64,
    pub poses_over_rate: u64,
    pub poses_before_look: u64,
    pub chat_received: u64,
    pub chat_dropped: u64,
    pub chat_floods: u64,
    pub chat_sent: u64,
    pub chat_muted: u64,
    pub control_stale: u64,
    pub starts_sent: u64,
    pub refusals: VecDeque<(u16, String)>,
    pub validation: [CheckOutcomes; 6],
    pub rides_validated: u64,
    pub samples_before_ready: u64,
    pub samples_before_ready_dropped: u64,
    pub samples_before_ready_most: u64,
    pub samples_before_ready_other_run: u64,
    pub rules_loaded: u64,
    pub rules_failed: u64,
    pub reports_dropped: u64,
    pub report_lead_top: [Option<i64>; 3],
    pub report_clock_excess_top: [Option<i64>; 3],
    pub control_held: u64,
    pub control_held_most: usize,
    pub frames_clamped: u64,
    pub frame_ms_most: u64,
    pub frames_long: u64,
    pub traffic: Traffic,
    pub resources_told: u64,
}

impl ServerStats {
    pub(crate) fn note_refusal(&mut self, rider: u16, detail: &str) {
        if self.refusals.len() == RECORDED_REFUSALS {
            self.refusals.pop_front();
        }
        self.refusals.push_back((rider, detail.chars().take(200).collect()));
    }
}

const RECORDED_REFUSALS: usize = 256;

impl crate::traffic::TrafficCounter for ServerStats {
    fn traffic(&mut self) -> &mut Traffic {
        &mut self.traffic
    }
}

#[derive(Resource, Default)]
struct RiderIds(BTreeSet<u16>);

impl RiderIds {
    fn take(&mut self) -> Option<RiderId> {
        let id = (0..=u16::MAX).find(|i| !self.0.contains(i))?;
        self.0.insert(id);
        Some(RiderId(id))
    }
}

type NewLink<'a> = (Option<&'a TokenUserData>, Option<&'a RemoteId>, Option<&'a PeerAddr>);

fn on_connected(
    trigger: On<Add<Connected>>,
    links: Query<NewLink, With<ClientOf>>,
    others: Query<(Entity, &PlayerSession), Without<Kicked>>,
    (mut sessions, mut ids, mut stats): (ResMut<Sessions>, ResMut<RiderIds>, ResMut<ServerStats>),
    (lobby, time, mut instances): (Res<Lobby>, Res<Time<Real>>, Option<ResMut<Instances>>),
    (chat, gate): (Res<ChatConfig>, Option<Res<OperatorGate>>),
    mut commands: Commands,
) {
    let entity = trigger.entity;
    let Ok((user_data, remote, peer)) = links.get(entity) else { return };
    stats.connected += 1;
    let now = time.elapsed();
    commands.entity(entity).remove::<PendingSince>();
    let (Some(user_data), Some(RemoteId(PeerId::Netcode(client_id))), Some(peer)) = (user_data, remote, peer) else {
        leave(&mut commands, entity, KickReason::Ticket(TicketRefusal::Missing), "no session ticket", now);
        return;
    };
    let claimed = match sessions.claim(now, &user_data.0, *client_id, peer.0.ip()) {
        Ok(claimed) => claimed,
        Err(refusal) => {
            leave(&mut commands, entity, KickReason::Ticket(refusal), &format!("session ticket: {refusal:?}"), now);
            return;
        }
    };
    if let Some(ban) = gate.as_deref().and_then(|g| g.0.banned(&claimed.player, peer.0.ip())) {
        sessions.release(*client_id);
        leave(&mut commands, entity, KickReason::Banned(ban.until), &ban.reason, now);
        return;
    }
    let Some(rider) = ids.take() else {
        sessions.release(*client_id);
        leave(&mut commands, entity, KickReason::ServerFull, "no rider number left", now);
        return;
    };
    stats.sessions += 1;
    for (other, session) in &others {
        if session.player == claimed.player {
            leave(&mut commands, other, KickReason::Replaced, "your key connected again", now);
        }
    }
    let claimed_manifest = claimed.manifest;
    let session = PlayerSession {
        player: claimed.player,
        id: claimed.player.id(),
        name: claimed.name,
        join_id: claimed.join_id,
        client_id: *client_id,
        address: peer.0.ip(),
    };
    match instance::place(&mut commands, instances.as_deref_mut(), lobby.0, entity, &session, rider.0, chat.enabled) {
        Ok(()) => {
            commands.entity(entity).insert((session, ReplicationSender, RiderLink::new(rider), ChatLink::new()));
            commands.entity(entity).insert(Told(claimed_manifest));
        }
        Err(e) => {
            ids.0.remove(&rider.0);
            sessions.release(*client_id);
            let (reason, text) = match e {
                instance::PlaceError::Instance(_) => (KickReason::ServerFull, "every instance is full".to_string()),
                instance::PlaceError::Name(why) => {
                    (KickReason::Ticket(TicketRefusal::Mismatch), format!("name: {why}"))
                }
            };
            leave(&mut commands, entity, reason, &text, now);
        }
    }
}

fn on_rider_removed(
    trigger: On<Remove<RiderLink>>,
    riders: Query<&RiderLink>,
    mut ids: ResMut<RiderIds>,
    mut relay: ResMut<relay::RelayState>,
) {
    if let Ok(link) = riders.get(trigger.entity) {
        ids.0.remove(&link.rider().0);
        relay.forget(link.rider().0);
    }
}

fn on_session_removed(
    trigger: On<Remove<PlayerSession>>,
    sessions_on_links: Query<&PlayerSession>,
    mut sessions: ResMut<Sessions>,
) {
    if let Ok(session) = sessions_on_links.get(trigger.entity) {
        sessions.release(session.client_id);
    }
}

fn expire_tickets(mut sessions: ResMut<Sessions>, time: Res<Time<Real>>) {
    sessions.expire(time.elapsed());
}

pub fn spawn_udp_server(
    world: &mut World,
    addr: SocketAddr,
    private_key: [u8; 32],
    conditioner: Option<RecvLinkConditioner>,
) -> Entity {
    world.get_resource_or_init::<Sessions>().set_netcode_key(private_key);
    world.spawn((netcode_server(private_key, true), LocalAddr(addr), ServerUdpIo, Endpoint::new(conditioner))).id()
}

pub fn netcode_server(private_key: [u8; 32], check_address: bool) -> NetcodeServer {
    NetcodeServer::new(NetcodeConfig {
        max_clients: MAX_CLIENTS,
        protocol_id: netcode_protocol_id(),
        private_key,
        server_addr_check: check_address,
        ..NetcodeConfig::default()
    })
}

pub fn key_from_hex(hex: &str) -> Result<[u8; 32], String> {
    ssx_net_proto::keys::from_hex(hex).map_err(|e| e.to_string())
}
