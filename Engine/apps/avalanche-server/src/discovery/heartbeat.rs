use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bevy::log::{info, warn};
use bytes::Bytes;
use http_body_util::{BodyExt, Full, Limited};
use hyper::Request;
use hyper::header::{CONTENT_TYPE, HOST, RETRY_AFTER, USER_AGENT};
use hyper_util::rt::TokioIo;
use rustls::pki_types::ServerName;
use ssx_net_proto::BUILD_ID;
use ssx_net_proto::keys::ServerSecret;
use ssx_net_proto::master::{
    HEARTBEAT_VERSION, Heartbeat, HeartbeatAnswer, MASTER_CHALLENGE_SIZE, MAX_ANSWER_BODY, MasterChallenge, MasterUrl,
};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::{TcpSocket, TcpStream};
use tokio::sync::watch;
use tokio::time::timeout;

use super::Stats;
use crate::bridge::Shared;
use crate::http::HttpState;

// PORT(choice): 2 s for the lookup, 3 s to connect, 5 s for the handshake, 5 s for a challenge, 20 s for a heartbeat;
// tuning: heartbeats to real masters over real links; the original has no dedicated server.
pub const RESOLVE_TIMEOUT: Duration = Duration::from_secs(2);
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
pub const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);
pub const CHALLENGE_TIMEOUT: Duration = Duration::from_secs(5);
pub const HEARTBEAT_TIMEOUT: Duration = Duration::from_secs(20);
pub const MAX_RETRY_AFTER: Duration = Duration::from_secs(600);
pub const LEAVE_TIMEOUT: Duration = Duration::from_secs(2);
const MAX_HEAD: usize = 8192;
const MAX_TEXT: usize = 512;

pub struct Beat {
    pub master: MasterUrl,
    pub identity: ServerSecret,
    pub http: Arc<HttpState>,
    pub shared: Arc<Shared>,
    pub game_port: u16,
    pub source: Option<IpAddr>,
    pub region: String,
    pub tags: Vec<String>,
    pub contact: String,
    pub tls: Arc<rustls::ClientConfig>,
    pub interval: Duration,
    pub after_change: Duration,
    pub stats: Arc<Stats>,
    pub last: Arc<Mutex<Option<(String, String)>>>,
}

enum Outcome {
    Answered(HeartbeatAnswer),
    Refused { status: u16, text: String, retry_after: Option<Duration> },
    Failed(String),
}

pub(crate) trait Io: AsyncRead + AsyncWrite + Unpin + Send + 'static {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send + 'static> Io for T {}

fn system_roots() -> &'static (Vec<rustls::pki_types::CertificateDer<'static>>, usize) {
    static ROOTS: std::sync::OnceLock<(Vec<rustls::pki_types::CertificateDer<'static>>, usize)> =
        std::sync::OnceLock::new();
    ROOTS.get_or_init(|| {
        let found = rustls_native_certs::load_native_certs();
        (found.certs, found.errors.len())
    })
}

pub fn tls_config(extra: &[rustls::pki_types::CertificateDer<'static>]) -> Result<rustls::ClientConfig, String> {
    let mut roots = rustls::RootCertStore::empty();
    let (system, unread) = system_roots();
    let (added, ignored) = roots.add_parsable_certificates(system.iter().cloned());
    if added == 0 {
        warn!(
            "this system has no trusted root certificates ({ignored} ignored, {unread} unread): heartbeats to https \
             masters will fail (install the system's CA certificates)"
        );
    }
    for cert in extra {
        roots.add(cert.clone()).map_err(|e| format!("a test root: {e}"))?;
    }
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut config = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| e.to_string())?
        .with_root_certificates(roots)
        .with_no_client_auth();
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok(config)
}

async fn connect(beat: &Beat, url: &MasterUrl) -> Result<Box<dyn Io>, String> {
    connect_from(beat.source, &beat.tls, url).await
}

pub(crate) async fn connect_from(
    source: Option<IpAddr>,
    tls: &Arc<rustls::ClientConfig>,
    url: &MasterUrl,
) -> Result<Box<dyn Io>, String> {
    let host = url.host_text();
    let addresses: Vec<SocketAddr> =
        match timeout(RESOLVE_TIMEOUT, tokio::net::lookup_host((host.as_str(), url.port))).await {
            Ok(Ok(found)) => found.take(16).collect(),
            Ok(Err(e)) => return Err(format!("the name {host} does not resolve: {e}")),
            Err(_) => return Err(format!("the name {host} did not resolve within {} s", RESOLVE_TIMEOUT.as_secs())),
        };
    let target = addresses
        .iter()
        .find(|a| source.is_none_or(|s| s.is_ipv4() == a.is_ipv4()))
        .copied()
        .ok_or_else(|| format!("{host} has no address of the server's bound family"))?;
    let socket = if target.is_ipv4() { TcpSocket::new_v4() } else { TcpSocket::new_v6() }.map_err(|e| e.to_string())?;
    if let Some(source) = source {
        socket.bind(SocketAddr::new(source, 0)).map_err(|e| format!("binding {source}: {e}"))?;
    }
    let stream: TcpStream = match timeout(CONNECT_TIMEOUT, socket.connect(target)).await {
        Ok(Ok(s)) => s,
        Ok(Err(e)) => return Err(format!("{target} does not answer: {e}")),
        Err(_) => return Err(format!("{target} did not connect within {} s", CONNECT_TIMEOUT.as_secs())),
    };
    if !url.tls {
        return Ok(Box::new(stream));
    }
    let name = ServerName::try_from(host.clone()).map_err(|e| format!("{host}: {e}"))?;
    let connector = tokio_rustls::TlsConnector::from(Arc::clone(tls));
    match timeout(HANDSHAKE_TIMEOUT, connector.connect(name, stream)).await {
        Ok(Ok(s)) => Ok(Box::new(s)),
        Ok(Err(e)) => Err(format!("TLS with {host}: {e}")),
        Err(_) => Err(format!("TLS with {host}: no handshake within {} s", HANDSHAKE_TIMEOUT.as_secs())),
    }
}

async fn request(
    beat: &Beat,
    url: &MasterUrl,
    post: Option<Vec<u8>>,
    path: &str,
    max: usize,
) -> Result<(u16, Option<Duration>, Vec<u8>), String> {
    let io = connect(beat, url).await?;
    let mut b = hyper::client::conn::http1::Builder::new();
    b.max_buf_size(MAX_HEAD).max_headers(32);
    let (mut sender, conn) = b.handshake::<_, Full<Bytes>>(TokioIo::new(io)).await.map_err(|e| format!("HTTP: {e}"))?;
    let driver = tokio::spawn(conn);
    let builder = Request::builder()
        .method(if post.is_some() { "POST" } else { "GET" })
        .uri(path)
        .header(HOST, url.authority())
        .header(USER_AGENT, BUILD_ID);
    let req = match post {
        Some(body) => builder.header(CONTENT_TYPE, "application/octet-stream").body(Full::new(Bytes::from(body))),
        None => builder.body(Full::new(Bytes::new())),
    }
    .map_err(|e| e.to_string())?;
    let result = async {
        let response = sender.send_request(req).await.map_err(|e| format!("HTTP: {e}"))?;
        let status = response.status().as_u16();
        let retry_after = response
            .headers()
            .get(RETRY_AFTER)
            .and_then(|v| v.to_str().ok()?.trim().parse::<u64>().ok())
            .map(|s| Duration::from_secs(s).min(MAX_RETRY_AFTER));
        let limit = if status == 200 { max } else { MAX_TEXT };
        let body = Limited::new(response.into_body(), limit)
            .collect()
            .await
            .map_err(|_| format!("an answer larger than {limit} bytes or cut short (HTTP {status})"))?;
        Ok((status, retry_after, body.to_bytes().to_vec()))
    }
    .await;
    driver.abort();
    result
}

fn plain(bytes: &[u8]) -> String {
    ssx_core::text::shown_max(&String::from_utf8_lossy(bytes), 200).to_string()
}

impl Beat {
    fn heartbeat(&self, leaving: bool) -> Heartbeat {
        let info = self.http.info();
        Heartbeat {
            heartbeat_version: HEARTBEAT_VERSION,
            port: self.game_port,
            protocol_version: info.protocol_version,
            info_version: info.info_version,
            api_level: info.api_level,
            build: info.build,
            name: info.name,
            mode: info.mode,
            drop: info.drop,
            map: info.map,
            players: info.players,
            max_players: info.max_players,
            password: info.password,
            region: self.region.clone(),
            tags: self.tags.clone(),
            contact: self.contact.clone(),
            leaving,
            resources: info.resources,
        }
    }

    async fn send(&self, url: &MasterUrl, leaving: bool) -> Outcome {
        let challenge =
            match timeout(CHALLENGE_TIMEOUT, request(self, url, None, "/v1/challenge", MASTER_CHALLENGE_SIZE)).await {
                Ok(Ok((200, _, body))) => match MasterChallenge::from_bytes(&body) {
                    Ok(c) => c,
                    Err(e) => return Outcome::Failed(format!("its challenge: {e}")),
                },
                Ok(Ok((status, retry_after, body))) => {
                    return Outcome::Refused { status, text: plain(&body), retry_after };
                }
                Ok(Err(e)) => return Outcome::Failed(e),
                Err(_) => return Outcome::Failed(format!("no challenge within {} s", CHALLENGE_TIMEOUT.as_secs())),
            };
        let body = match self.heartbeat(leaving).seal(&self.identity, &challenge) {
            Ok(b) => b,
            Err(e) => return Outcome::Failed(format!("the heartbeat: {e}")),
        };
        Stats::count(&self.stats.heartbeats_sent);
        match timeout(HEARTBEAT_TIMEOUT, request(self, url, Some(body), "/v1/heartbeat", MAX_ANSWER_BODY)).await {
            Ok(Ok((200, _, body))) => match HeartbeatAnswer::open(&body, &self.identity, &challenge) {
                Ok(a) => Outcome::Answered(a),
                Err(e) => Outcome::Failed(format!("its answer is not the master's: {e}")),
            },
            Ok(Ok((status, retry_after, body))) => Outcome::Refused { status, text: plain(&body), retry_after },
            Ok(Err(e)) => Outcome::Failed(e),
            Err(_) => Outcome::Failed(format!("no answer within {} s", HEARTBEAT_TIMEOUT.as_secs())),
        }
    }

    fn note(&self, url: &MasterUrl, text: String) {
        if let Ok(mut last) = self.last.lock() {
            *last = Some((url.to_string(), text));
        }
    }
}

pub async fn run(beat: Arc<Beat>, mut stop: watch::Receiver<bool>) {
    let url = beat.master.clone();
    let mut next = Instant::now();
    let mut change_due: Option<Instant> = None;
    let mut sent_players: Option<u32> = None;
    let mut sent_manifest: Option<String> = None;
    let mut listed: Option<bool> = None;
    let mut tick = tokio::time::interval(Duration::from_secs(1));
    loop {
        tokio::select! {
            _ = tick.tick() => {}
            _ = stop.changed() => {}
        }
        if *stop.borrow() || beat.shared.stopping.load(Ordering::Acquire) {
            break;
        }
        let now = Instant::now();
        let players = beat.shared.players.load(Ordering::Acquire);
        if sent_players.is_some_and(|p| p != players) && change_due.is_none() {
            change_due = Some(now + beat.after_change);
        }
        let manifest = beat.http.resources.get().hash;
        let manifest = ssx_net_proto::resources::hex32(&manifest.0);
        if sent_manifest.as_ref().is_some_and(|m| *m != manifest) && change_due.is_none() {
            change_due = Some(now + beat.after_change);
        }
        if now < next && change_due.is_none_or(|t| now < t) {
            continue;
        }
        change_due = None;
        sent_players = Some(players);
        sent_manifest = Some(manifest);
        next = now + beat.interval;
        match beat.send(&url, false).await {
            Outcome::Answered(a) => {
                if a.listed {
                    Stats::count(&beat.stats.heartbeats_listed);
                    if listed != Some(true) {
                        info!("listed on the master {url}");
                    }
                } else {
                    Stats::count(&beat.stats.heartbeats_refused);
                    warn!("not listed on the master {url}: {}", a.reason);
                }
                beat.note(&url, if a.listed { "listed".into() } else { a.reason.clone() });
                listed = Some(a.listed);
            }
            Outcome::Refused { status, text, retry_after } => {
                Stats::count(&beat.stats.heartbeats_failed);
                if let Some(wait) = retry_after {
                    next = Instant::now() + wait.max(Duration::from_secs(1));
                }
                warn!("the master {url} refused the heartbeat (HTTP {status}): {text}");
                beat.note(&url, format!("refused (HTTP {status}): {text}"));
                listed = Some(false);
            }
            Outcome::Failed(why) => {
                Stats::count(&beat.stats.heartbeats_failed);
                warn!("the heartbeat to the master {url} failed: {why}");
                beat.note(&url, format!("failed: {why}"));
                listed = Some(false);
            }
        }
    }
    if listed == Some(true) {
        match timeout(LEAVE_TIMEOUT, beat.send(&url, true)).await {
            Ok(Outcome::Answered(_)) => info!("left the master {url}'s list"),
            _ => warn!("the leave to the master {url} did not get through (the entry expires by itself)"),
        }
    }
}
