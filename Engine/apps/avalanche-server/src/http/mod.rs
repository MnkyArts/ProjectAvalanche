mod challenge;
pub mod files;
pub mod held;
mod join;
pub mod limits;

use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use bytes::Bytes;
use http_body_util::{BodyExt, Full, Limited};
use hyper::body::Incoming;
use hyper::header::{CACHE_CONTROL, CONTENT_LENGTH, CONTENT_TYPE, HeaderValue, RETRY_AFTER};
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::{TokioIo, TokioTimer};
use sha2::{Digest, Sha256};
use ssx_net::address::address_key;
use ssx_net_proto::info::{INFO_VERSION, ServerInfo};
use ssx_net_proto::join::HELD_ASK_BODY;
use ssx_net_proto::join::MAX_JOIN_BODY;
use ssx_net_proto::keys::{ServerKey, ServerSecret};
use ssx_net_proto::players::PlayerList;
use ssx_net_proto::{API_LEVEL, BUILD_ID, PROTOCOL_VERSION};
use tokio::net::TcpListener;
use tokio::sync::{Notify, mpsc};

pub use challenge::{CHALLENGE_LIFETIME, ChallengeRefusal, Challenges};
use limits::{AddressLimiter, Connections, Rate, Verdict};

use crate::bridge::{JoinTask, Shared};
use crate::config::Config;

// PORT(choice): 256 overall and 8 per address (a browser's list refresh opens one per server; a join three in turn);
// tuning: phase 4's public servers under a measured connection flood; the original has no dedicated server.
pub const MAX_CONNECTIONS: usize = 256;
pub const MAX_CONNECTIONS_PER_ADDRESS: u32 = 8;
// PORT(choice): 5 s for the head, 10 s for the whole connection, 32 headers (a join's three requests carry a few
// hundred bytes and fewer than 10 headers from the client's ureq; the 8 KiB head is hyper's smallest buffer, a fact,
// not a choice); tuning: the slowest real client a public server must still serve (phase 4's links); the original has
// no dedicated server.
pub const HEAD_TIMEOUT: Duration = Duration::from_secs(5);
pub const CONNECTION_TIMEOUT: Duration = Duration::from_secs(10);
pub const MAX_HEAD_BYTES: usize = 8192;
pub const MAX_HEADERS: usize = 32;

// PORT(choice): /info and /players 4 a second (burst 8), /challenge 2 (burst 4), /join 1 (burst 4), anything else 2
// (burst 4); tuning: the request rates of phase 5's server browser and of the master's reachability check; the original
// has no dedicated server.
pub const INFO_RATE: Rate = Rate { per_second: 4.0, burst: 8.0 };
pub const PLAYERS_RATE: Rate = INFO_RATE;
pub const CHALLENGE_RATE: Rate = Rate { per_second: 2.0, burst: 4.0 };
pub const JOIN_RATE: Rate = Rate { per_second: 1.0, burst: 4.0 };
pub const OTHER_RATE: Rate = Rate { per_second: 2.0, burst: 4.0 };
// PORT(choice): 64 file requests a second with a burst of 512, 1 report a second with a burst of 8; tuning: joins of
// real servers' resource sets (measured: M8a's template map and the alps0101 layout, 5 files each); the original has no
// dedicated server.
pub const RESOURCES_RATE: Rate = INFO_RATE;
pub const FILES_RATE: Rate = Rate { per_second: 64.0, burst: 512.0 };
pub const REPORT_RATE: Rate = Rate { per_second: 1.0, burst: 8.0 };
pub const JOIN_FLOOD: Rate = Rate { per_second: 4.0 / 30.0, burst: 4.0 };
// PORT(choice): 4 a second with a burst of 8 (four held joins' asks, and a slow request's ask arriving with the next);
// tuned with `held::ASK_AFTER_MS` and `held::MAX_HELD_PER_ADDRESS`; the original has no dedicated server.
pub const HELD_RATE: Rate = Rate { per_second: 4.0, burst: 8.0 };
// PORT(choice): 4 096 addresses per limiter; tuning: phase 4's public servers; the original has no dedicated server.
pub const TRACKED_ADDRESSES: usize = 4096;

pub struct HttpState {
    pub(crate) identity: ServerSecret,
    pub(crate) identity_key: ServerKey,
    pub(crate) password_hash: Option<[u8; 32]>,
    pub(crate) name: String,
    pub(crate) max_players: u16,
    pub(crate) description: String,
    pub(crate) mode: &'static str,
    pub(crate) level: crate::maps::ShownLevel,
    pub(crate) rewinds: String,
    pub(crate) ghosts: bool,
    pub(crate) ghost_style: &'static str,
    pub(crate) required_api_level: u32,
    pub(crate) challenges: Challenges,
    pub(crate) join_flood: AddressLimiter,
    rates: [AddressLimiter; 9],
    pub(crate) resources: Arc<crate::resources::Published>,
    connections: Connections,
    pub(crate) joins: mpsc::Sender<JoinTask>,
    pub(crate) shared: Arc<Shared>,
    pub(crate) operation: std::sync::OnceLock<Arc<crate::operation::http::OperationHttp>>,
    pub(crate) region: String,
    pub(crate) tags: Vec<String>,
    pub(crate) held: held::HeldTickets,
}

impl HttpState {
    pub fn new(config: &Config, identity: ServerSecret, joins: mpsc::Sender<JoinTask>, shared: Arc<Shared>) -> Self {
        let identity_key = identity.public();
        let password_hash = (!config.password.is_empty()).then(|| Sha256::digest(config.password.as_bytes()).into());
        Self {
            identity,
            identity_key,
            password_hash,
            name: config.name.clone(),
            max_players: config.max_players,
            description: config.description.clone(),
            mode: config.game().mode.name(),
            level: crate::maps::shown(&config.game().level, &Default::default(), &Default::default()),
            rewinds: config.game().rewinds.name(),
            ghosts: config.game().ghosts,
            ghost_style: config.game().ghost_style.name(),
            required_api_level: 0,
            challenges: Challenges::new(identity_key),
            join_flood: AddressLimiter::new(JOIN_FLOOD, TRACKED_ADDRESSES),
            rates: [
                INFO_RATE,
                CHALLENGE_RATE,
                JOIN_RATE,
                PLAYERS_RATE,
                OTHER_RATE,
                RESOURCES_RATE,
                FILES_RATE,
                REPORT_RATE,
                HELD_RATE,
            ]
            .map(|r| AddressLimiter::new(r, TRACKED_ADDRESSES)),
            resources: Arc::new(crate::resources::Published::empty(config.mods)),
            connections: Connections::new(MAX_CONNECTIONS, MAX_CONNECTIONS_PER_ADDRESS),
            joins,
            shared,
            operation: std::sync::OnceLock::new(),
            region: config.discovery.region.clone(),
            tags: config.discovery.tags.clone(),
            held: held::HeldTickets::default(),
        }
    }

    pub(crate) fn info(&self) -> ServerInfo {
        let level = self.level();
        ServerInfo {
            info_version: INFO_VERSION,
            name: self.name.clone(),
            description: self.description.clone(),
            protocol_version: PROTOCOL_VERSION,
            api_level: API_LEVEL,
            build: BUILD_ID.into(),
            players: self.shared.players.load(Ordering::Acquire).min(u32::from(self.max_players)),
            max_players: u32::from(self.max_players),
            password: self.password_hash.is_some(),
            identity_key: self.identity_key.to_hex(),
            mode: self.mode.into(),
            drop: level.drop,
            map: level.map,
            rewinds: self.rewinds.clone(),
            ghosts: self.ghosts,
            ghost_style: self.ghost_style.into(),
            resources: self.resources.get().summary(),
            region: self.region.clone(),
            tags: self.tags.clone(),
            icon: self.operation.get().map_or(String::new(), |o| o.icon_hash()),
        }
    }

    fn level(&self) -> crate::maps::ShownLevel {
        self.shared.level.lock().ok().and_then(|l| l.clone()).unwrap_or_else(|| self.level.clone())
    }

    fn players(&self) -> PlayerList {
        let names = self.shared.player_names.lock().map(|n| n.clone()).unwrap_or_default();
        PlayerList::new(names)
    }
}

pub struct Reply {
    status: u16,
    content_type: &'static str,
    body: Bytes,
    retry_after: Option<u64>,
    stream: Option<files::FileBody>,
    headers: Vec<(hyper::header::HeaderName, Option<HeaderValue>)>,
    etag: Option<String>,
}

type ReplyBody = http_body_util::Either<Full<Bytes>, files::FileBody>;

impl Reply {
    pub(crate) fn octets(body: Vec<u8>) -> Self {
        Self { content_type: "application/octet-stream", body: body.into(), ..Self::status(200, "") }
    }

    pub(crate) fn status(status: u16, text: &str) -> Self {
        Self {
            status,
            content_type: "text/plain; charset=utf-8",
            body: Bytes::from(format!("{text}\n")),
            retry_after: None,
            stream: None,
            headers: Vec::new(),
            etag: None,
        }
    }

    pub(crate) fn text(content_type: &'static str, body: String) -> Self {
        Self { content_type, ..Self::status(200, "") }.with_body(Bytes::from(body))
    }

    fn with_body(self, body: Bytes) -> Self {
        Self { body, ..self }
    }

    pub(crate) fn cached(body: Option<(&'static str, Bytes)>, etag: String) -> Self {
        match body {
            Some((content_type, body)) => {
                Self { content_type, etag: Some(etag), ..Self::status(200, "") }.with_body(body)
            }
            None => Self { etag: Some(etag), ..Self::status(304, "") }.with_body(Bytes::new()),
        }
    }

    pub(crate) fn limited(wait: Duration) -> Self {
        Self { retry_after: Some(wait.as_secs() + 1), ..Self::status(429, "too many requests") }
    }

    fn into_response(self) -> Response<ReplyBody> {
        let body = match self.stream {
            Some(file) => http_body_util::Either::Right(file),
            None => http_body_util::Either::Left(Full::new(self.body)),
        };
        let mut response = Response::new(body);
        *response.status_mut() = StatusCode::from_u16(self.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        let headers = response.headers_mut();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static(self.content_type));
        match self.etag.as_deref().and_then(|t| HeaderValue::from_str(t).ok()) {
            Some(tag) => {
                headers.insert(CACHE_CONTROL, HeaderValue::from_static(crate::operation::http::ICON_CACHE));
                headers.insert(hyper::header::ETAG, tag);
            }
            None => {
                headers.insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
            }
        }
        if let Some(secs) = self.retry_after {
            headers.insert(RETRY_AFTER, HeaderValue::from(secs));
        }
        for (name, value) in self.headers {
            if let Some(v) = value {
                headers.insert(name, v);
            }
        }
        response
    }
}

fn endpoint(method: &Method, path: &str) -> usize {
    match (method, path) {
        (&Method::GET, "/info") => 0,
        (&Method::GET, "/challenge") => 1,
        (&Method::POST, "/join") => 2,
        (&Method::GET, "/players") => 3,
        (&Method::GET, "/resources") => 5,
        (&Method::GET, p) if p.starts_with("/files/") => 6,
        (&Method::POST, "/report") => 7,
        (&Method::POST, "/join/held") => 8,
        _ => 4,
    }
}

async fn handle(state: &HttpState, peer: SocketAddr, request: Request<Incoming>) -> Reply {
    Shared::count(&state.shared.http_requests);
    if let Some(ops) = state.operation.get()
        && crate::operation::http::serves(request.uri().path())
    {
        return crate::operation::http::handle(state, ops, peer, request).await;
    }
    let (method, path) = (request.method().clone(), request.uri().path().to_string());
    let limiter = endpoint(&method, &path);
    match state.rates[limiter].check(address_key(peer.ip()), Instant::now()) {
        Verdict::Allowed => {}
        Verdict::Limited(wait) => {
            Shared::count(&state.shared.http_limited);
            return Reply::limited(wait);
        }
        Verdict::Full => {
            Shared::count(&state.shared.http_limited);
            return Reply::status(503, "busy");
        }
    }
    match (method, path.as_str()) {
        (Method::GET, "/info") => {
            Reply { content_type: "application/json", ..Reply::status(200, &state.info().to_json()) }
        }
        (Method::GET, "/players") => {
            Reply { content_type: "application/json", ..Reply::status(200, &state.players().to_json()) }
        }
        (Method::GET, "/challenge") => {
            Reply::octets(state.challenges.issue(address_key(peer.ip()), Instant::now()).to_vec())
        }
        (Method::POST, "/join") => {
            let declared = request.headers().get(CONTENT_LENGTH).and_then(|v| v.to_str().ok()?.parse::<u64>().ok());
            if declared.is_some_and(|n| n > MAX_JOIN_BODY as u64) {
                Shared::count(&state.shared.http_limited);
                return Reply::status(413, "the body is too large");
            }
            match Limited::new(request.into_body(), MAX_JOIN_BODY).collect().await {
                Ok(body) => join::join(state, peer, &body.to_bytes()).await,
                Err(_) => {
                    Shared::count(&state.shared.http_limited);
                    Reply::status(413, "the body is too large or ended early")
                }
            }
        }
        (Method::GET, "/resources") => files::manifest(state),
        (Method::GET, p) if p.starts_with("/files/") => {
            let progress = request.extensions().get::<files::Progress>().cloned().unwrap_or_default();
            files::file(state, &p["/files/".len()..], request.headers().get(hyper::header::RANGE), progress).await
        }
        (Method::POST, "/report") => {
            let declared = request.headers().get(CONTENT_LENGTH).and_then(|v| v.to_str().ok()?.parse::<u64>().ok());
            if declared.is_some_and(|n| n > files::MAX_REPORT_BODY as u64) {
                return Reply::status(413, "the body is too large");
            }
            match Limited::new(request.into_body(), files::MAX_REPORT_BODY).collect().await {
                Ok(body) => files::report(state, peer, &body.to_bytes()),
                Err(_) => Reply::status(413, "the body is too large or ended early"),
            }
        }
        (Method::POST, "/join/held") => {
            let declared = request.headers().get(CONTENT_LENGTH).and_then(|v| v.to_str().ok()?.parse::<u64>().ok());
            if declared.is_some_and(|n| n > HELD_ASK_BODY as u64) {
                Shared::count(&state.shared.http_limited);
                return Reply::status(413, "the body is too large");
            }
            match Limited::new(request.into_body(), HELD_ASK_BODY).collect().await {
                Ok(body) => join::held(state, peer, &body.to_bytes()).await,
                Err(_) => {
                    Shared::count(&state.shared.http_limited);
                    Reply::status(413, "the body is too large or ended early")
                }
            }
        }
        (_, "/join/held") => Reply::status(405, "POST only"),
        (_, "/resources") => Reply::status(405, "GET only"),
        (_, p) if p.starts_with("/files/") => Reply::status(405, "GET only"),
        (_, "/report") => Reply::status(405, "POST only"),
        (_, "/info" | "/players" | "/challenge") => Reply { retry_after: None, ..Reply::status(405, "GET only") },
        (_, "/join") => Reply::status(405, "POST only"),
        _ => Reply::status(404, "not found"),
    }
}

async fn serve(state: Arc<HttpState>, stream: tokio::net::TcpStream, peer: SocketAddr) {
    let Some(_slot) = state.connections.open(address_key(peer.ip())) else {
        Shared::count(&state.shared.http_limited);
        return;
    };
    let progress = files::Progress::default();
    let service = service_fn(|mut request: Request<Incoming>| {
        let state = Arc::clone(&state);
        request.extensions_mut().insert(progress.clone());
        async move { Ok::<_, Infallible>(handle(&state, peer, request).await.into_response()) }
    });
    let mut builder = http1::Builder::new();
    builder
        .timer(TokioTimer::new())
        .header_read_timeout(HEAD_TIMEOUT)
        .max_buf_size(MAX_HEAD_BYTES)
        .max_headers(MAX_HEADERS)
        .keep_alive(false);
    let connection = builder.serve_connection(TokioIo::new(stream), service);
    let start = tokio::time::Instant::now();
    let allowed = |sent: u64| CONNECTION_TIMEOUT + Duration::from_secs_f64(sent as f64 / files::MIN_FILE_RATE as f64);
    tokio::pin!(connection);
    loop {
        let deadline = start + allowed(progress.0.load(Ordering::Relaxed));
        tokio::select! {
            _ = &mut connection => break,
            () = tokio::time::sleep_until(deadline) => {
                if start.elapsed() >= allowed(progress.0.load(Ordering::Relaxed)) {
                    Shared::count(&state.shared.http_limited);
                    break;
                }
            }
        }
    }
}

pub async fn run(listener: TcpListener, state: Arc<HttpState>, shutdown: Arc<Notify>) {
    let mut sweep = tokio::time::interval(Duration::from_secs(1));
    sweep.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            () = shutdown.notified() => break,
            _ = sweep.tick() => state.held.sweep(Instant::now()),
            accepted = listener.accept() => match accepted {
                Ok((stream, peer)) => {
                    tokio::spawn(serve(Arc::clone(&state), stream, peer));
                }
                Err(e) => {
                    bevy::log::warn!("HTTP accept: {e}");
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            },
        }
    }
}

pub fn state_sizes(state: &HttpState) -> (usize, usize, usize) {
    let tracked = state.rates.iter().map(AddressLimiter::tracked).sum::<usize>() + state.join_flood.tracked();
    (tracked, state.challenges.remembered(), state.connections.open_now())
}
