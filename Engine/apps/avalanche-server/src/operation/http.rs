use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use http_body_util::{BodyExt, Limited};
use hyper::body::Incoming;
use hyper::header::{CONTENT_LENGTH, IF_NONE_MATCH};
use hyper::{Method, Request};
use ssx_net::address::{AddressKey, address_key};
use ssx_net_proto::dynamic::Dynamic;
use ssx_net_proto::join::RefusalCode;
use ssx_net_proto::rcon::MAX_REQUEST_BODY;
use tokio::sync::mpsc;

use super::icon::IconFile;
use super::rcon::{RconLimits, RconTask};
use super::store::Operation;
use super::throttle::Throttle;
use crate::http::limits::{Rate, Verdict};
use crate::http::{Challenges, HttpState, Reply};

// PORT(choice): /metrics 2 a second (burst 4), /dynamic 4 (burst 8), the icon 1 (burst 4); tuning: the scrape intervals
// and the browser's refresh measured on public servers; the original has no dedicated server.
pub const METRICS_RATE: Rate = Rate { per_second: 2.0, burst: 4.0 };
pub const DYNAMIC_RATE: Rate = Rate { per_second: 4.0, burst: 8.0 };
pub const ICON_RATE: Rate = Rate { per_second: 1.0, burst: 4.0 };
// PORT(choice): a day; tuning: how often operators change their icons; the original has no dedicated server.
pub const ICON_CACHE: &str = "public, max-age=86400";
// PORT(choice): 16 (an operator's tool sends one at a time; the per-key rate bounds a script); tuning: operators'
// tools; the original has no dedicated server.
pub const RCON_QUEUE: usize = 16;
// PORT(choice): 256 resources (a server runs tens); tuning: phase 6's resource counts; the original has no dedicated
// server.
pub const MAX_SCRIPT_RESOURCES: usize = 256;

pub struct OperationHttp {
    pub operation: Arc<Operation>,
    pub(crate) challenges: Challenges,
    pub(crate) rcon: RconLimits,
    pub(crate) tasks: mpsc::Sender<RconTask>,
    pub(crate) config: super::config::OperationConfig,
    pub(crate) icon: Option<IconFile>,
    rates: [Throttle<AddressKey>; 3],
    pub(crate) dynamic: Mutex<Option<Dynamic>>,
    pub(crate) snapshot: Mutex<super::metrics::Snapshot>,
    pub(crate) refusals: Mutex<BTreeMap<String, u64>>,
    pub(crate) rcon_ran: AtomicU64,
    pub(crate) rcon_refused: AtomicU64,
    pub(crate) script_us: Mutex<BTreeMap<String, u64>>,
}

pub struct RconQueue(pub mpsc::Receiver<RconTask>);

impl OperationHttp {
    pub fn new(
        operation: Arc<Operation>,
        server: ssx_net_proto::keys::ServerKey,
        config: super::config::OperationConfig,
        icon: Option<IconFile>,
    ) -> (Self, RconQueue) {
        let (tx, rx) = mpsc::channel(RCON_QUEUE);
        let addresses = crate::http::TRACKED_ADDRESSES;
        let ops = Self {
            operation,
            challenges: Challenges::new(server),
            rcon: RconLimits::new(),
            tasks: tx,
            config,
            icon,
            rates: [METRICS_RATE, DYNAMIC_RATE, ICON_RATE].map(|r| Throttle::new(r, addresses)),
            dynamic: Mutex::new(None),
            snapshot: Mutex::new(super::metrics::Snapshot::default()),
            refusals: Mutex::new(BTreeMap::new()),
            rcon_ran: AtomicU64::new(0),
            rcon_refused: AtomicU64::new(0),
            script_us: Mutex::new(BTreeMap::new()),
        };
        (ops, RconQueue(rx))
    }

    pub(crate) fn count_rcon(&self, ran: bool) {
        let c = if ran { &self.rcon_ran } else { &self.rcon_refused };
        c.fetch_add(1, Ordering::Relaxed);
    }

    pub fn count_refusal(&self, code: RefusalCode) {
        let mut map = self.refusals.lock().unwrap_or_else(PoisonError::into_inner);
        *map.entry(format!("{code:?}")).or_insert(0) += 1;
    }

    pub fn script_time(&self, resource: &str, spent: Duration) -> Result<(), String> {
        ssx_net_ops::acl::resource(resource)?;
        let us = u64::try_from(spent.as_micros()).unwrap_or(u64::MAX);
        let mut map = self.script_us.lock().unwrap_or_else(PoisonError::into_inner);
        let key = if map.contains_key(resource) || map.len() < MAX_SCRIPT_RESOURCES { resource } else { "(others)" };
        let total = map.entry(key.to_string()).or_insert(0);
        *total = total.saturating_add(us);
        Ok(())
    }

    pub fn icon_hash(&self) -> String {
        self.icon.as_ref().map_or(String::new(), |i| i.hash.clone())
    }
}

pub fn serves(path: &str) -> bool {
    matches!(path, "/rcon/challenge" | "/rcon" | "/metrics" | "/dynamic" | "/icon.png")
}

fn rate(ops: &OperationHttp, which: usize, peer: SocketAddr) -> Option<Reply> {
    match ops.rates[which].spend(address_key(peer.ip()), Instant::now()) {
        Verdict::Allowed => None,
        Verdict::Limited(wait) => Some(Reply::limited(wait)),
        Verdict::Full => Some(Reply::status(503, "busy")),
    }
}

pub(crate) async fn handle(
    state: &HttpState,
    ops: &OperationHttp,
    peer: SocketAddr,
    request: Request<Incoming>,
) -> Reply {
    let (method, path) = (request.method().clone(), request.uri().path().to_string());
    match (method, path.as_str()) {
        (Method::GET, "/rcon/challenge") => super::rcon::challenge(ops, peer),
        (Method::POST, "/rcon") => {
            let declared = request.headers().get(CONTENT_LENGTH).and_then(|v| v.to_str().ok()?.parse::<u64>().ok());
            if declared.is_some_and(|n| n > MAX_REQUEST_BODY as u64) {
                return Reply::status(413, "the body is too large");
            }
            match Limited::new(request.into_body(), MAX_REQUEST_BODY).collect().await {
                Ok(body) => super::rcon::request(state, ops, peer, &body.to_bytes()).await,
                Err(_) => Reply::status(413, "the body is too large or ended early"),
            }
        }
        (Method::GET, "/metrics") => {
            if !ops.config.metrics_enabled {
                return Reply::status(404, "not found");
            }
            if !ops.config.metrics_allow.iter().any(|r| r.contains(peer.ip())) {
                return Reply::status(403, "this address may not read the metrics");
            }
            if let Some(limited) = rate(ops, 0, peer) {
                return limited;
            }
            Reply::text("text/plain; version=0.0.4; charset=utf-8", super::metrics::render(state, ops))
        }
        (Method::GET, "/dynamic") => {
            if let Some(limited) = rate(ops, 1, peer) {
                return limited;
            }
            let dynamic = ops.dynamic.lock().unwrap_or_else(PoisonError::into_inner).clone();
            match dynamic {
                Some(d) => Reply::text("application/json", d.to_json()),
                None => Reply::status(503, "the instances are not published yet"),
            }
        }
        (Method::GET, "/icon.png") => {
            let Some(icon) = &ops.icon else { return Reply::status(404, "this server has no icon") };
            if let Some(limited) = rate(ops, 2, peer) {
                return limited;
            }
            let tag = format!("\"{}\"", icon.hash);
            let fresh = request.headers().get(IF_NONE_MATCH).is_some_and(|v| v.as_bytes() == tag.as_bytes());
            Reply::cached(if fresh { None } else { Some(("image/png", icon.bytes.clone())) }, tag)
        }
        (_, "/rcon") => Reply::status(405, "POST only"),
        _ => Reply::status(405, "GET only"),
    }
}

pub(crate) fn banned_join(
    state: &HttpState,
    ops: &OperationHttp,
    reply: &ssx_net_proto::keys::ReplyKey,
    peer: SocketAddr,
    player: &ssx_net_proto::keys::PlayerKey,
) -> Option<Reply> {
    let ban = ops.operation.ban_of(Some(&player.id().0), Some(peer.ip()), super::store::unix_now())?;
    crate::bridge::Shared::count(&state.shared.joins_refused);
    ops.count_refusal(RefusalCode::Banned);
    let text = super::store::ban_text(&ban);
    bevy::log::info!(
        target: super::commands::OPERATION_LOG,
        "join from {} ({}): refused, banned ({}) {}",
        ssx_net::address::canonical(peer.ip()),
        player.id().short(),
        ban.target,
        ban.until.describe()
    );
    let answer = ssx_net_proto::join::JoinAnswer::Refused(ssx_net_proto::join::Refusal::banned(text.into(), ban.until));
    Some(match answer.seal(reply) {
        Ok(body) => Reply::octets(body),
        Err(_) => Reply::status(500, "the answer could not be sealed"),
    })
}
