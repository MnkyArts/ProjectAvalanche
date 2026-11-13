use std::net::SocketAddr;
use std::time::{Duration, Instant};

use ssx_net::address::{AddressKey, address_key, canonical};
use ssx_net_proto::JoinError;
use ssx_net_proto::keys::ReplyKey;
use ssx_net_proto::rcon::{RCON_VERSION, RconAnswer, RconRefusalCode, open_rcon};
use tokio::sync::oneshot;

use super::commands::OPERATION_LOG;
use super::http::OperationHttp;
use super::throttle::Throttle;
use crate::http::limits::{Rate, Verdict};
use crate::http::{HttpState, Reply};

// PORT(choice): 2 a second with a burst of 8 per address and per key (an operator types or a script runs a few commands
// in a row); 5 failures, then one a minute, per address and per key (MTA locks an address out after repeated wrong
// passwords); 1 024 keys tracked; tuning: the operators' tools and a measured brute-force attempt; the original has no
// dedicated server.
pub const RCON_RATE: Rate = Rate { per_second: 2.0, burst: 8.0 };
pub const RCON_FAILURES: Rate = Rate { per_second: 1.0 / 60.0, burst: 5.0 };
pub const TRACKED_KEYS: usize = 1024;
// PORT(choice): 5 s (a command runs within a tick; `bans reload` reads a file of at most 16 MiB); tuning: the slowest
// command on a loaded server; the original has no dedicated server.
pub const WORLD_ANSWER: Duration = Duration::from_secs(5);

pub struct RconLimits {
    pub(crate) by_address: Throttle<AddressKey>,
    pub(crate) by_key: Throttle<[u8; 32]>,
    pub(crate) failures_by_address: Throttle<AddressKey>,
    pub(crate) failures_by_key: Throttle<[u8; 32]>,
}

impl RconLimits {
    pub fn new() -> Self {
        let addresses = crate::http::TRACKED_ADDRESSES;
        Self {
            by_address: Throttle::new(RCON_RATE, addresses),
            by_key: Throttle::new(RCON_RATE, TRACKED_KEYS),
            failures_by_address: Throttle::new(RCON_FAILURES, addresses),
            failures_by_key: Throttle::new(RCON_FAILURES, TRACKED_KEYS),
        }
    }
}

impl Default for RconLimits {
    fn default() -> Self {
        Self::new()
    }
}

pub struct RconTask {
    pub operator: ssx_net_proto::rcon::OperatorKey,
    pub line: String,
    pub command: crate::console::Command,
    pub reply: oneshot::Sender<(bool, Vec<String>)>,
}

fn secs(wait: Duration) -> u16 {
    u16::try_from(wait.as_secs() + 1).unwrap_or(u16::MAX)
}

fn sealed(reply: &ReplyKey, answer: &RconAnswer) -> Reply {
    match answer.seal(reply) {
        Ok(body) => Reply::octets(body),
        Err(_) => Reply::status(500, "the answer could not be sealed"),
    }
}

pub(crate) fn challenge(ops: &OperationHttp, peer: SocketAddr) -> Reply {
    if !ops.operation.acl().has_operators() {
        return Reply::status(404, "the remote console is off (no operator key in the ACL)");
    }
    let (address, now) = (address_key(peer.ip()), Instant::now());
    if let Verdict::Limited(wait) = ops.rcon.failures_by_address.peek(address, now).or_full() {
        return Reply::limited(wait);
    }
    match ops.rcon.by_address.spend(address, now) {
        Verdict::Allowed => Reply::octets(ops.challenges.issue(address, now).to_vec()),
        Verdict::Limited(wait) => Reply::limited(wait),
        Verdict::Full => Reply::status(503, "busy"),
    }
}

trait OrFull {
    fn or_full(self) -> Verdict;
}

impl OrFull for Verdict {
    fn or_full(self) -> Verdict {
        match self {
            Verdict::Full => Verdict::Limited(Duration::from_secs(60)),
            v => v,
        }
    }
}

fn failed(ops: &OperationHttp, peer: SocketAddr, key: Option<&[u8; 32]>, why: &str) {
    let now = Instant::now();
    let _ = ops.rcon.failures_by_address.spend(address_key(peer.ip()), now);
    if let Some(k) = key {
        let _ = ops.rcon.failures_by_key.spend(*k, now);
    }
    ops.count_rcon(false);
    bevy::log::warn!(target: OPERATION_LOG, "rcon from {}: refused: {why}", canonical(peer.ip()));
}

pub(crate) async fn request(state: &HttpState, ops: &OperationHttp, peer: SocketAddr, body: &[u8]) -> Reply {
    if !ops.operation.acl().has_operators() {
        return Reply::status(404, "the remote console is off (no operator key in the ACL)");
    }
    let (address, now) = (address_key(peer.ip()), Instant::now());
    if let Verdict::Limited(wait) = ops.rcon.failures_by_address.peek(address, now).or_full() {
        return Reply::limited(wait);
    }
    if let Verdict::Limited(wait) = ops.rcon.by_address.spend(address, now).or_full() {
        return Reply::limited(wait);
    }
    let opened = match open_rcon(body, &state.identity) {
        Ok(opened) => opened,
        Err(e) => {
            failed(ops, peer, None, &format!("the body does not open: {e}"));
            return Reply::status(400, &e.to_string());
        }
    };
    let (version, reply) = match opened.prefix() {
        Ok(prefix) => prefix,
        Err(e) => {
            failed(ops, peer, None, &format!("the request does not read: {e}"));
            return Reply::status(400, &e.to_string());
        }
    };
    let refuse = |code, retry, text: String| sealed(&reply, &RconAnswer::refused(code, retry, &text));
    if version != RCON_VERSION {
        ops.count_rcon(false);
        return refuse(RconRefusalCode::Version, 0, format!("this server speaks RCON {RCON_VERSION}, yours {version}"));
    }
    let request = match opened.request(&state.identity_key) {
        Ok(r) => r,
        Err(JoinError::Signature) => {
            failed(ops, peer, None, "the signature does not verify");
            return refuse(RconRefusalCode::Request, 0, "the signature does not verify".into());
        }
        Err(e) => {
            failed(ops, peer, None, &format!("the request does not read: {e}"));
            return refuse(RconRefusalCode::Request, 0, format!("the request does not read: {e}"));
        }
    };
    let key = *request.operator.as_bytes();
    let id = request.operator.id().short();
    match ops.challenges.redeem(&request.challenge, address, now) {
        Ok(()) => {}
        Err(crate::http::ChallengeRefusal::Full) => {
            ops.count_rcon(false);
            return refuse(RconRefusalCode::Busy, 5, "too many requests at once".into());
        }
        Err(why) => {
            failed(ops, peer, Some(&key), &format!("key {id}: the challenge is {why:?}"));
            return refuse(RconRefusalCode::Challenge, 0, format!("the challenge is {why:?}: ask for a new one"));
        }
    }
    if let Verdict::Limited(wait) = ops.rcon.failures_by_key.peek(key, now).or_full() {
        ops.count_rcon(false);
        return refuse(RconRefusalCode::Limited, secs(wait), "too many failures with this key; wait".into());
    }
    if let Verdict::Limited(wait) = ops.rcon.by_key.spend(key, now).or_full() {
        ops.count_rcon(false);
        return refuse(RconRefusalCode::Limited, secs(wait), "too many commands with this key; wait".into());
    }
    if !ops.operation.acl().is_operator(&request.operator) {
        failed(ops, peer, Some(&key), &format!("key {id} is no operator's"));
        return refuse(RconRefusalCode::UnknownKey, 0, "this key is no operator's on this server".into());
    }
    let command = match crate::console::parse(&request.command) {
        Some(Ok(command)) => command,
        Some(Err(e)) => {
            ops.count_rcon(false);
            return refuse(RconRefusalCode::Command, 0, e);
        }
        None => {
            ops.count_rcon(false);
            return refuse(RconRefusalCode::Command, 0, "an empty command".into());
        }
    };
    let right = right_of(&request.command);
    if !ops.operation.acl().allows(ssx_net_ops::Subject::Operator(&request.operator), &right) {
        ops.count_rcon(false);
        bevy::log::warn!(target: OPERATION_LOG, "rcon {id} from {}: {right} denied", canonical(peer.ip()));
        return refuse(RconRefusalCode::Denied, 0, format!("your groups do not hold {right}"));
    }
    let (tx, rx) = oneshot::channel();
    let task = RconTask { operator: request.operator, line: request.command, command, reply: tx };
    if ops.tasks.try_send(task).is_err() {
        ops.count_rcon(false);
        return refuse(RconRefusalCode::Busy, 2, "too many commands at once".into());
    }
    match tokio::time::timeout(WORLD_ANSWER, rx).await {
        Ok(Ok((ok, lines))) => {
            ops.count_rcon(true);
            sealed(&reply, &RconAnswer::ran(ok, lines))
        }
        Ok(Err(_)) | Err(_) => refuse(RconRefusalCode::Busy, 2, "the server did not answer in time".into()),
    }
}

pub fn right_of(line: &str) -> String {
    let word = line.split_whitespace().next().unwrap_or_default();
    format!("command.{word}")
}
