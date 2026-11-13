use std::net::SocketAddr;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};
use ssx_net::address::{address_key, canonical};
use ssx_net::server::TicketRequest;
use ssx_net_proto::join::{JoinAnswer, Refusal, RefusalCode, open_join};
use ssx_net_proto::keys::ReplyKey;
use ssx_net_proto::name::player_name;
use ssx_net_proto::{BUILD_ID, JoinError, PROTOCOL_VERSION};
use subtle::ConstantTimeEq;
use tokio::sync::oneshot;

use super::challenge::ChallengeRefusal;
use super::held::{Refused, Waiting};
use super::limits::Verdict;
use super::{HttpState, Reply};
use crate::bridge::{JoinTask, JoinVerdict, Shared};
use ssx_net_proto::join::HeldAsk;

// PORT(choice): 2 s, 60 ticks of the world at 30 Hz; tuning: the world's tick under phase 3's load (instances,
// validation), where a decision must still come within the client's 10 s request time; the original has no dedicated
// server.
const WORLD_ANSWER: Duration = Duration::from_secs(2);

fn sealed(reply: &ReplyKey, answer: &JoinAnswer) -> Reply {
    match answer.seal(reply) {
        Ok(body) => Reply::octets(body),
        Err(_) => Reply::status(500, "the answer could not be sealed"),
    }
}

fn refused(
    state: &HttpState,
    reply: &ReplyKey,
    peer: SocketAddr,
    code: RefusalCode,
    retry: u16,
    text: String,
) -> Reply {
    Shared::count(&state.shared.joins_refused);
    if let Some(ops) = state.operation.get() {
        ops.count_refusal(code);
    }
    bevy::log::info!("join from {}: refused, {code:?}: {text}", canonical(peer.ip()));
    sealed(reply, &JoinAnswer::Refused(Refusal::new(code, retry, text)))
}

pub(super) async fn join(state: &HttpState, peer: SocketAddr, body: &[u8]) -> Reply {
    let opened = match open_join(body, &state.identity) {
        Ok(opened) => opened,
        Err(e) => return Reply::status(400, &e.to_string()),
    };
    let prefix = match opened.prefix() {
        Ok(prefix) => prefix,
        Err(e) => return Reply::status(400, &e.to_string()),
    };
    let reply = prefix.reply;
    let refuse = |code, retry, text: String| refused(state, &reply, peer, code, retry, text);
    if prefix.protocol_version != PROTOCOL_VERSION {
        let text = format!("this server speaks protocol {PROTOCOL_VERSION}, your game {}", prefix.protocol_version);
        return refuse(RefusalCode::Protocol, 0, text);
    }
    if prefix.api_level < state.required_api_level {
        let text =
            format!("this server needs API level {}, your game has {}", state.required_api_level, prefix.api_level);
        return refuse(RefusalCode::ApiLevel, 0, text);
    }
    let request = match opened.request(&state.identity_key) {
        Ok(request) => request,
        Err(JoinError::Signature) => return refuse(RefusalCode::Request, 0, "the signature does not verify".into()),
        Err(e) => return refuse(RefusalCode::Request, 0, format!("the request does not read: {e}")),
    };
    let address = address_key(peer.ip());
    let now = Instant::now();
    match state.challenges.redeem(&request.challenge, address, now) {
        Ok(()) => {}
        Err(ChallengeRefusal::Full) => return refuse(RefusalCode::Busy, 5, "too many joins at once".into()),
        Err(why) => return refuse(RefusalCode::Challenge, 0, format!("the challenge is {why:?}: ask for a new one")),
    }
    let name = match player_name(&request.name) {
        Ok(name) => name,
        Err(e) => return refuse(RefusalCode::Name, 0, e.to_string()),
    };
    if request.build != BUILD_ID {
        bevy::log::info!(
            "join from {}: build {:?}, this server {BUILD_ID:?} (allowed)",
            canonical(peer.ip()),
            request.build
        );
    }
    match state.join_flood.check(address, now) {
        Verdict::Allowed => {}
        Verdict::Limited(wait) => {
            let secs = u16::try_from(wait.as_secs() + 1).unwrap_or(u16::MAX);
            return refuse(RefusalCode::Flood, secs, format!("too many joins from your address; wait {secs} s"));
        }
        Verdict::Full => return refuse(RefusalCode::Busy, 5, "too many joining addresses at once".into()),
    }
    if let Some(ops) = state.operation.get()
        && let Some(answer) = crate::operation::http::banned_join(state, ops, &reply, peer, &request.player)
    {
        return answer;
    }
    if let Some(expected) = &state.password_hash {
        let given: [u8; 32] = Sha256::digest(&request.password[..]).into();
        if !bool::from(given.ct_eq(expected)) {
            let text = if request.password.is_empty() { "this server needs a password" } else { "wrong password" };
            return refuse(RefusalCode::Password, 0, text.into());
        }
    }
    if let Some((code, text)) = content_refusal(&state.resources.get(), &request.content) {
        return refuse(code, 0, text);
    }
    if state.shared.stopping.load(Ordering::Acquire) {
        return refuse(RefusalCode::Busy, 0, "the server is stopping".into());
    }
    let ticket = TicketRequest {
        player: request.player,
        name: name.clone(),
        address: canonical(peer.ip()),
        public_addr: request.server_addr,
        manifest: request.content.manifest,
    };
    let (tx, rx) = oneshot::channel();
    if state.joins.try_send(JoinTask { ticket, reply: tx }).is_err() {
        return refuse(RefusalCode::Busy, 2, "too many joins at once".into());
    }
    let who = format!("{name:?} ({})", request.player.id().short());
    match tokio::time::timeout(WORLD_ANSWER + crate::scripts::FIRST_WORD, rx).await {
        Ok(Ok(JoinVerdict::Held(hold))) => {
            let waiting = Waiting { hold, since: Instant::now(), who };
            held_answer(state, &reply, peer, waiting)
        }
        Ok(Ok(verdict)) => final_answer(state, &reply, peer, verdict, &who),
        Ok(Err(_)) | Err(_) => refuse(RefusalCode::Busy, 2, "the server did not answer in time".into()),
    }
}

fn final_answer(state: &HttpState, reply: &ReplyKey, peer: SocketAddr, verdict: JoinVerdict, who: &str) -> Reply {
    match verdict {
        JoinVerdict::Accepted(token) => {
            Shared::count(&state.shared.joins_accepted);
            bevy::log::info!("join from {}: {who} accepted", canonical(peer.ip()));
            sealed(reply, &JoinAnswer::Accepted(token))
        }
        JoinVerdict::Refused(code, text) => refused(state, reply, peer, code, 0, text),
        JoinVerdict::Held(_) => {
            refused(state, reply, peer, RefusalCode::Busy, 2, "the server held the join twice".into())
        }
    }
}

fn held_answer(state: &HttpState, reply: &ReplyKey, peer: SocketAddr, waiting: Waiting) -> Reply {
    let who = waiting.who.clone();
    match state.held.issue(address_key(peer.ip()), *reply, waiting, Instant::now()) {
        Ok(hold) => {
            bevy::log::debug!("join from {}: {who} held: {}", canonical(peer.ip()), ssx_core::text::shown(&hold.text));
            sealed(reply, &JoinAnswer::Held(hold))
        }
        Err((Refused::Address, _)) => {
            refused(state, reply, peer, RefusalCode::Flood, 5, "too many held joins from your address".into())
        }
        Err((Refused::Full, _)) => {
            refused(state, reply, peer, RefusalCode::Busy, 5, "too many joins are held by the server".into())
        }
    }
}

pub(super) async fn held(state: &HttpState, peer: SocketAddr, body: &[u8]) -> Reply {
    let ask = match HeldAsk::open(body, &state.identity) {
        Ok(ask) => ask,
        Err(e) => return Reply::status(400, &e.to_string()),
    };
    let reply = ask.reply;
    let Some(mut waiting) = state.held.take(&ask, address_key(peer.ip()), Instant::now()) else {
        let text = "the held join's ticket is unknown, used or late: join again".to_string();
        return refused(state, &reply, peer, RefusalCode::Request, 0, text);
    };
    match waiting.hold.verdict.try_recv() {
        Ok(verdict) => final_answer(state, &reply, peer, verdict, &waiting.who),
        Err(oneshot::error::TryRecvError::Empty) => held_answer(state, &reply, peer, waiting),
        Err(oneshot::error::TryRecvError::Closed) => {
            refused(state, &reply, peer, RefusalCode::Busy, 2, "the server dropped the held join".into())
        }
    }
}

fn content_refusal(
    now: &crate::resources::Snapshot,
    content: &ssx_net_proto::join::JoinContent,
) -> Option<(RefusalCode, String)> {
    match content.manifest {
        None => {
            return Some((
                RefusalCode::Content,
                "your game did not read this server's resources (GET /resources): it cannot join".into(),
            ));
        }
        Some(m) if m != now.hash.0 => {
            return Some((
                RefusalCode::ResourcesChanged,
                "the server's resources changed since your game read them: it reads them again".into(),
            ));
        }
        Some(_) => {}
    }
    let ours = now.manifest.content_id.map(|c| c.0);
    if content.content_id == ours {
        return None;
    }
    let short = |id: Option<[u8; 32]>| match id {
        Some(id) => ssx_net_proto::resources::hex32(&id)[..16].to_string(),
        None => "none".to_string(),
    };
    Some((
        RefusalCode::Content,
        format!("content differs from the server's: yours {}, the server's {}", short(content.content_id), short(ours)),
    ))
}
