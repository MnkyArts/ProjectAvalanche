use core::time::Duration;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};

use ssx_net_proto::info::{MAX_INFO_BYTES, ServerInfo};
use ssx_net_proto::join::{CHALLENGE_SIZE, JoinAnswer, JoinRequest, MAX_ANSWER_BODY, Refusal, TOKEN_SIZE};
use ssx_net_proto::join::{HeldAsk, MAX_ASK_AFTER_MS, MAX_HOLD};
use ssx_net_proto::keys::{ReplySecret, ServerKey};
use ssx_net_proto::name::player_name;
use ssx_net_proto::{API_LEVEL, BUILD_ID, NameError, PROTOCOL_VERSION};
use thiserror::Error;
use ureq::Agent;
use zeroize::Zeroizing;

use crate::identity::PlayerIdentity;
use crate::known::{KnownError, KnownServers, Trust};

// PORT(choice): 10 s per request, 5 s to connect; tuning: the join over real links from the server browser's connecting
// popup (phases F2 and 5); the original has no real-time multiplayer.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_HEADERS: usize = 8 * 1024;
const MAX_ERROR_TEXT: u64 = 512;
const HELD_FOLLOWED: Duration =
    Duration::from_millis(MAX_HOLD.as_millis() as u64 + MAX_ASK_AFTER_MS as u64 + REQUEST_TIMEOUT.as_millis() as u64);
const PAUSE_STEP: Duration = Duration::from_millis(50);

#[derive(Debug, Error)]
pub enum JoinFailure {
    #[error("the server at {server} cannot be reached: {why}")]
    Unreachable { server: SocketAddr, why: String },

    #[error("the server answered {status}: {text}")]
    Http { status: u16, text: String, retry_after_secs: Option<u64> },

    #[error("the server sent what the protocol does not allow: {0}")]
    Malformed(String),

    #[error("the server speaks protocol {server}, this game {PROTOCOL_VERSION}")]
    Version { server: u32 },

    #[error("{0}")]
    Key(#[from] KnownError),

    #[error("your name: {0}")]
    Name(#[from] NameError),

    #[error("the server refused the join ({:?}): {}", .0.code, .0.text)]
    Refused(Refusal),

    #[error("the join was cancelled")]
    Cancelled,

    #[error(
        "the server at {server} presents another identity key than its master listed: listed {listed}, offered \
         {offered}. The list may be older than the server's new key (ask the masters again), or someone is between you \
         and the server"
    )]
    NotListedKey { server: SocketAddr, listed: String, offered: String },
}

pub struct JoinParams<'a> {
    pub name: &'a str,
    pub password: &'a [u8],
    pub cancel: Option<&'a AtomicBool>,
    pub content: ssx_net_proto::join::JoinContent,
}

impl JoinParams<'_> {
    fn go_on(&self) -> Result<(), JoinFailure> {
        match self.cancel {
            Some(flag) if flag.load(Ordering::Acquire) => Err(JoinFailure::Cancelled),
            _ => Ok(()),
        }
    }
}

pub struct Granted {
    pub token: Box<[u8; TOKEN_SIZE]>,
    pub info: ServerInfo,
    pub trust: Trust,
}

fn agent() -> Agent {
    Agent::config_builder()
        .timeout_global(Some(REQUEST_TIMEOUT))
        .timeout_connect(Some(CONNECT_TIMEOUT))
        .max_response_header_size(MAX_HEADERS)
        .http_status_as_error(false)
        .max_redirects(0)
        .proxy(None)
        .user_agent(BUILD_ID)
        .build()
        .into()
}

fn body(
    server: SocketAddr,
    result: Result<ureq::http::Response<ureq::Body>, ureq::Error>,
    max: usize,
) -> Result<Vec<u8>, JoinFailure> {
    let mut response = result.map_err(|e| JoinFailure::Unreachable { server, why: e.to_string() })?;
    let status = response.status().as_u16();
    if status != 200 {
        let retry_after_secs = response
            .headers()
            .get("retry-after")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.trim().parse::<u64>().ok());
        let text = response.body_mut().with_config().limit(MAX_ERROR_TEXT).read_to_vec().unwrap_or_default();
        let text = ssx_core::text::shown_max(&String::from_utf8_lossy(&text), MAX_ERROR_TEXT as usize).to_string();
        return Err(JoinFailure::Http { status, text, retry_after_secs });
    }
    let bytes = response
        .body_mut()
        .with_config()
        .limit(max as u64 + 1)
        .read_to_vec()
        .map_err(|e| JoinFailure::Malformed(format!("the answer's body: {e}")))?;
    if bytes.len() > max {
        return Err(JoinFailure::Malformed(format!("an answer of more than {max} bytes")));
    }
    Ok(bytes)
}

pub fn fetch_info(server: SocketAddr) -> Result<ServerInfo, JoinFailure> {
    let bytes = body(server, agent().get(&format!("http://{server}/info")).call(), MAX_INFO_BYTES)?;
    ServerInfo::from_json(&bytes).map_err(JoinFailure::Malformed)
}

pub fn join(
    server: SocketAddr,
    identity: &PlayerIdentity,
    known: &mut KnownServers,
    params: &JoinParams,
) -> Result<Granted, JoinFailure> {
    run(server, identity, Pin::Known(known), params, &|_| {})
}

pub fn join_showing(
    server: SocketAddr,
    identity: &PlayerIdentity,
    known: &mut KnownServers,
    params: &JoinParams,
    on_held: &dyn Fn(&str),
) -> Result<Granted, JoinFailure> {
    run(server, identity, Pin::Known(known), params, on_held)
}

pub fn join_listed(
    server: SocketAddr,
    identity: &PlayerIdentity,
    listed: &ServerKey,
    params: &JoinParams,
) -> Result<Granted, JoinFailure> {
    run(server, identity, Pin::Listed(listed), params, &|_| {})
}

pub fn join_listed_showing(
    server: SocketAddr,
    identity: &PlayerIdentity,
    listed: &ServerKey,
    params: &JoinParams,
    on_held: &dyn Fn(&str),
) -> Result<Granted, JoinFailure> {
    run(server, identity, Pin::Listed(listed), params, on_held)
}

enum Pin<'a> {
    Known(&'a mut KnownServers),
    Listed(&'a ServerKey),
}

fn run(
    server: SocketAddr,
    identity: &PlayerIdentity,
    mut pin: Pin,
    params: &JoinParams,
    on_held: &dyn Fn(&str),
) -> Result<Granted, JoinFailure> {
    let name = player_name(params.name)?;
    let agent = agent();
    params.go_on()?;
    let info = fetch_info(server)?;
    if info.protocol_version != PROTOCOL_VERSION {
        return Err(JoinFailure::Version { server: info.protocol_version });
    }
    let key = info.key().map_err(JoinFailure::Malformed)?;
    let address = server.to_string();
    let trust = match &mut pin {
        Pin::Known(known) => known.check(&address, &key)?,
        Pin::Listed(listed) if **listed == key => Trust::Listed,
        Pin::Listed(listed) => {
            return Err(JoinFailure::NotListedKey { server, listed: listed.to_hex(), offered: key.to_hex() });
        }
    };
    params.go_on()?;
    let challenge = body(server, agent.get(&format!("http://{server}/challenge")).call(), CHALLENGE_SIZE)?;
    let challenge: [u8; CHALLENGE_SIZE] = challenge
        .try_into()
        .map_err(|c: Vec<u8>| JoinFailure::Malformed(format!("a challenge of {} bytes", c.len())))?;
    let reply = ReplySecret::generate();
    let request = JoinRequest {
        protocol_version: PROTOCOL_VERSION,
        api_level: API_LEVEL,
        reply: reply.public(),
        challenge,
        player: identity.key(),
        server_addr: server,
        name,
        build: BUILD_ID.into(),
        password: Zeroizing::new(params.password.to_vec()),
        content: params.content,
    };
    let sealed = request.seal(&key, identity.secret()).map_err(|e| JoinFailure::Malformed(e.to_string()))?;
    params.go_on()?;
    let answer = body(
        server,
        agent
            .post(&format!("http://{server}/join"))
            .header("content-type", "application/octet-stream")
            .send(&sealed[..]),
        MAX_ANSWER_BODY,
    )?;
    let answer = JoinAnswer::open(&answer, &reply).map_err(|e| JoinFailure::Malformed(e.to_string()))?;
    let answer = follow_hold(server, &agent, &key, &reply, answer, params, on_held)?;
    params.go_on()?;
    if let Pin::Known(known) = pin {
        known.trust(&address, key)?;
    }
    match answer {
        JoinAnswer::Accepted(token) => Ok(Granted { token, info, trust }),
        JoinAnswer::Refused(refusal) => Err(JoinFailure::Refused(refusal)),
        JoinAnswer::Held(_) => Err(JoinFailure::Malformed("a held join without its final word".into())),
    }
}

fn follow_hold(
    server: SocketAddr,
    agent: &Agent,
    key: &ServerKey,
    reply: &ReplySecret,
    mut answer: JoinAnswer,
    params: &JoinParams,
    on_held: &dyn Fn(&str),
) -> Result<JoinAnswer, JoinFailure> {
    let started = std::time::Instant::now();
    while let JoinAnswer::Held(hold) = answer {
        on_held(&hold.text);
        if started.elapsed() > HELD_FOLLOWED {
            return Err(JoinFailure::Malformed(format!(
                "the server held the join for more than {} s (the protocol's bound is {} s)",
                HELD_FOLLOWED.as_secs(),
                MAX_HOLD.as_secs()
            )));
        }
        let pause_end = std::time::Instant::now() + Duration::from_millis(u64::from(hold.ask_after_ms));
        loop {
            params.go_on()?;
            let left = pause_end.saturating_duration_since(std::time::Instant::now());
            if left.is_zero() {
                break;
            }
            std::thread::sleep(left.min(PAUSE_STEP));
        }
        let ask = HeldAsk { ticket: hold.ticket, reply: reply.public() };
        let sealed = Zeroizing::new(ask.seal(key).map_err(|e| JoinFailure::Malformed(e.to_string()))?);
        let body = body(
            server,
            agent
                .post(&format!("http://{server}/join/held"))
                .header("content-type", "application/octet-stream")
                .send(&sealed[..]),
            MAX_ANSWER_BODY,
        )?;
        answer = JoinAnswer::open(&body, reply).map_err(|e| JoinFailure::Malformed(e.to_string()))?;
    }
    Ok(answer)
}
