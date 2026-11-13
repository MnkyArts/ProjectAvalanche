use sha2::{Digest, Sha256};

use crate::DecodeError;
use crate::info::{MAX_ANNOUNCED_PLAYERS, ServerInfo};

pub const QUERY_MAGIC: [u8; 4] = *b"SSXQ";
pub const QUERY_VERSION: u8 = 1;
pub const QUERY_PORT_OFFSET: u16 = 1;

pub const TOKEN_SIZE: usize = 16;
pub const INFO_HASH_SIZE: usize = 32;

const HEAD: usize = 6;
pub const CHALLENGE_REQUEST_SIZE: usize = 32;
pub const CHALLENGE_SIZE: usize = HEAD + TOKEN_SIZE;
pub const QUERY_REQUEST_SIZE: usize = 64;
pub const QUERY_ANSWER_SIZE: usize = HEAD + 4 + 4 + 2 + 2 + 1 + 1 + INFO_HASH_SIZE;
const _: () = assert!(CHALLENGE_SIZE <= CHALLENGE_REQUEST_SIZE);
const _: () = assert!(QUERY_ANSWER_SIZE <= QUERY_REQUEST_SIZE);
const _: () = assert!(HEAD + TOKEN_SIZE + 4 <= QUERY_REQUEST_SIZE);

const KIND_CHALLENGE_REQUEST: u8 = 1;
const KIND_CHALLENGE: u8 = 2;
const KIND_QUERY: u8 = 3;
const KIND_ANSWER: u8 = 4;
const FLAG_PASSWORD: u8 = 1;

pub const MODES: [&str; 4] = ["freeride", "race", "trick", "survive"];

pub fn mode_code(name: &str) -> Option<u8> {
    MODES.iter().position(|m| *m == name).map(|i| i as u8 + 1)
}

pub fn mode_name(code: u8) -> Option<&'static str> {
    MODES.get(usize::from(code).checked_sub(1)?).copied()
}

pub fn info_hash(info: &ServerInfo) -> [u8; INFO_HASH_SIZE] {
    let fixed = ServerInfo { players: 0, ..info.clone() };
    Sha256::digest(fixed.to_json().as_bytes()).into()
}

fn size(kind: &'static str, expected: usize, bytes: &[u8]) -> Result<(), DecodeError> {
    if bytes.len() != expected {
        return Err(DecodeError::Size { kind, expected, actual: bytes.len() });
    }
    Ok(())
}

fn head(bytes: &[u8], kind: u8, what: &'static str, used: usize) -> Result<(), DecodeError> {
    if bytes.get(..4) != Some(&QUERY_MAGIC[..]) {
        return Err(DecodeError::Field("query: not the query's magic"));
    }
    if bytes.get(4) != Some(&QUERY_VERSION) {
        return Err(DecodeError::Field("query: another layout version"));
    }
    if bytes.get(5) != Some(&kind) {
        return Err(DecodeError::Text { kind: what, detail: "another kind of message".into() });
    }
    if bytes.get(used..).is_some_and(|pad| pad.iter().any(|&b| b != 0)) {
        return Err(DecodeError::Text { kind: what, detail: "padding that is not zero".into() });
    }
    Ok(())
}

fn put_head(out: &mut Vec<u8>, kind: u8) {
    out.extend_from_slice(&QUERY_MAGIC);
    out.push(QUERY_VERSION);
    out.push(kind);
}

fn array<const N: usize>(bytes: &[u8], at: usize) -> [u8; N] {
    let mut out = [0u8; N];
    if let Some(src) = bytes.get(at..at + N) {
        out.copy_from_slice(src);
    }
    out
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QueryRequest {
    Challenge,
    Query { token: [u8; TOKEN_SIZE], sequence: u32 },
}

impl QueryRequest {
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(QUERY_REQUEST_SIZE);
        match self {
            Self::Challenge => {
                put_head(&mut out, KIND_CHALLENGE_REQUEST);
                out.resize(CHALLENGE_REQUEST_SIZE, 0);
            }
            Self::Query { token, sequence } => {
                put_head(&mut out, KIND_QUERY);
                out.extend_from_slice(token);
                out.extend_from_slice(&sequence.to_be_bytes());
                out.resize(QUERY_REQUEST_SIZE, 0);
            }
        }
        out
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, DecodeError> {
        match bytes.len() {
            CHALLENGE_REQUEST_SIZE => {
                head(bytes, KIND_CHALLENGE_REQUEST, "challenge request", HEAD)?;
                Ok(Self::Challenge)
            }
            QUERY_REQUEST_SIZE => {
                head(bytes, KIND_QUERY, "query", HEAD + TOKEN_SIZE + 4)?;
                Ok(Self::Query {
                    token: array(bytes, HEAD),
                    sequence: u32::from_be_bytes(array(bytes, HEAD + TOKEN_SIZE)),
                })
            }
            n => Err(DecodeError::Text {
                kind: "query request",
                detail: format!("{n} bytes, {CHALLENGE_REQUEST_SIZE} or {QUERY_REQUEST_SIZE}"),
            }),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Challenge {
    pub token: [u8; TOKEN_SIZE],
}

impl Challenge {
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(CHALLENGE_SIZE);
        put_head(&mut out, KIND_CHALLENGE);
        out.extend_from_slice(&self.token);
        out
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, DecodeError> {
        size("challenge", CHALLENGE_SIZE, bytes)?;
        head(bytes, KIND_CHALLENGE, "challenge", CHALLENGE_SIZE)?;
        Ok(Self { token: array(bytes, HEAD) })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct QueryAnswer {
    pub sequence: u32,
    pub protocol_version: u32,
    pub players: u16,
    pub max_players: u16,
    pub mode: u8,
    pub password: bool,
    pub info_hash: [u8; INFO_HASH_SIZE],
}

impl QueryAnswer {
    pub fn encode(&self) -> Result<Vec<u8>, DecodeError> {
        self.check()?;
        let mut out = Vec::with_capacity(QUERY_ANSWER_SIZE);
        put_head(&mut out, KIND_ANSWER);
        out.extend_from_slice(&self.sequence.to_be_bytes());
        out.extend_from_slice(&self.protocol_version.to_be_bytes());
        out.extend_from_slice(&self.players.to_be_bytes());
        out.extend_from_slice(&self.max_players.to_be_bytes());
        out.push(self.mode);
        out.push(if self.password { FLAG_PASSWORD } else { 0 });
        out.extend_from_slice(&self.info_hash);
        Ok(out)
    }

    fn check(&self) -> Result<(), DecodeError> {
        if self.max_players == 0
            || u32::from(self.max_players) > MAX_ANNOUNCED_PLAYERS
            || self.players > self.max_players
        {
            return Err(DecodeError::Text {
                kind: "query answer",
                detail: format!("{} of {} players", self.players, self.max_players),
            });
        }
        if mode_name(self.mode).is_none() {
            return Err(DecodeError::Text { kind: "query answer", detail: format!("mode byte {}", self.mode) });
        }
        Ok(())
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, DecodeError> {
        size("query answer", QUERY_ANSWER_SIZE, bytes)?;
        head(bytes, KIND_ANSWER, "query answer", QUERY_ANSWER_SIZE)?;
        let flags = bytes.get(HEAD + 13).copied().unwrap_or(0);
        if flags & !FLAG_PASSWORD != 0 {
            return Err(DecodeError::Text { kind: "query answer", detail: format!("flags {flags:#04x}") });
        }
        let answer = Self {
            sequence: u32::from_be_bytes(array(bytes, HEAD)),
            protocol_version: u32::from_be_bytes(array(bytes, HEAD + 4)),
            players: u16::from_be_bytes(array(bytes, HEAD + 8)),
            max_players: u16::from_be_bytes(array(bytes, HEAD + 10)),
            mode: bytes.get(HEAD + 12).copied().unwrap_or(0),
            password: flags & FLAG_PASSWORD != 0,
            info_hash: array(bytes, HEAD + 14),
        };
        answer.check()?;
        Ok(answer)
    }
}

// PORT(choice): 35450, far from the game ports (27450 and up, each with its query port + 1; MTA's LAN port is 34219);
// tuning: a collision reported by operators (the port's protocol); the original has no dedicated server.
pub const LAN_PORT: u16 = 35450;
pub const LAN_PROBE_SIZE: usize = 64;
const PROBE_TEXT: &str = "AVALANCHE-CLIENT ";
const ANSWER_TEXT: &str = "AVALANCHE-SERVER ";
pub const MAX_LAN_ANSWER: usize = ANSWER_TEXT.len() + 10 + 1 + 5 + 1;
const _: () = assert!(MAX_LAN_ANSWER <= LAN_PROBE_SIZE);
const _: () = assert!(PROBE_TEXT.len() + 10 < LAN_PROBE_SIZE);

fn decimal(text: &str) -> Option<u64> {
    let ok = !text.is_empty()
        && text.len() <= 10
        && text.bytes().all(|b| b.is_ascii_digit())
        && (text == "0" || !text.starts_with('0'));
    ok.then(|| text.parse().ok()).flatten()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LanProbe {
    pub protocol: u32,
}

impl LanProbe {
    pub fn encode(&self) -> Vec<u8> {
        let mut out = format!("{PROBE_TEXT}{}\n", self.protocol).into_bytes();
        out.resize(LAN_PROBE_SIZE, 0);
        out
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, DecodeError> {
        size("LAN probe", LAN_PROBE_SIZE, bytes)?;
        let end = bytes.iter().position(|&b| b == b'\n').ok_or(DecodeError::Field("LAN probe: no line end"))?;
        if bytes.get(end + 1..).unwrap_or_default().iter().any(|&b| b != 0) {
            return Err(DecodeError::Field("LAN probe: padding that is not zero"));
        }
        let text = std::str::from_utf8(bytes.get(..end).unwrap_or_default())
            .map_err(|_| DecodeError::Field("LAN probe: not text"))?;
        let protocol = text
            .strip_prefix(PROBE_TEXT)
            .and_then(decimal)
            .and_then(|p| u32::try_from(p).ok())
            .ok_or(DecodeError::Field("LAN probe: not `AVALANCHE-CLIENT <protocol>`"))?;
        Ok(Self { protocol })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LanAnswer {
    pub protocol: u32,
    pub port: u16,
}

impl LanAnswer {
    pub fn encode(&self) -> Vec<u8> {
        format!("{ANSWER_TEXT}{} {}\n", self.protocol, self.port).into_bytes()
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, DecodeError> {
        if bytes.len() > MAX_LAN_ANSWER {
            return Err(DecodeError::Text {
                kind: "LAN answer",
                detail: format!("{} bytes, at most {MAX_LAN_ANSWER}", bytes.len()),
            });
        }
        let text = std::str::from_utf8(bytes).map_err(|_| DecodeError::Field("LAN answer: not text"))?;
        let bad = DecodeError::Field("LAN answer: not `AVALANCHE-SERVER <protocol> <port>`");
        let body = text.strip_prefix(ANSWER_TEXT).and_then(|t| t.strip_suffix('\n')).ok_or(bad.clone())?;
        let (protocol, port) = body.split_once(' ').ok_or(bad.clone())?;
        let protocol = decimal(protocol).and_then(|p| u32::try_from(p).ok()).ok_or(bad.clone())?;
        let port = decimal(port).and_then(|p| u16::try_from(p).ok()).filter(|&p| p != 0).ok_or(bad)?;
        Ok(Self { protocol, port })
    }
}

#[cfg(test)]
mod tests;
