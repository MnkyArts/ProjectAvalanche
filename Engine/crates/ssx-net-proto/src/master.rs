use core::time::Duration;
use std::net::IpAddr;

use crypto_box::aead::Aead;
use crypto_box::{SalsaBox, SecretKey};
use serde::de::{Error as _, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};
use thiserror::Error;
use zeroize::Zeroizing;

use crate::info::{MAX_ANNOUNCED_PLAYERS, ServerInfo};
use crate::join::MAX_BUILD_BYTES;
use crate::keys::{KEY_SIZE, ServerKey, ServerSecret, from_hex, to_hex};
use crate::name::server_name;

pub const HEARTBEAT_VERSION: u32 = 3;
pub const LIST_VERSION: u32 = 3;
pub const BLOCKLIST_VERSION: u32 = 1;

pub const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(60);
pub const HEARTBEAT_AFTER_CHANGE: Duration = Duration::from_secs(15);
pub const MISSED_HEARTBEATS: u32 = 3;
pub const EXPIRE_AFTER: Duration = Duration::from_secs(60 * MISSED_HEARTBEATS as u64);

pub const MASTER_CHALLENGE_SIZE: usize = 8 + 16 + KEY_SIZE + 16;
// PORT(choice): 30 s, the join challenge's lifetime (a heartbeat posts its challenge within a second); tuning: the
// heartbeats of real servers over slow links (the master's log of expired challenges); the original has no master
// server.
pub const CHALLENGE_LIFETIME: Duration = Duration::from_secs(30);

const HEARTBEAT_MAGIC: [u8; 4] = *b"SSXH";
const ANSWER_MAGIC: [u8; 4] = *b"SSXM";
const ENVELOPE_VERSION: u8 = 1;
const ENVELOPE: usize = 5;
const NONCE: usize = 24;
const MAC: usize = 16;
const HEARTBEAT_DOMAIN: &[u8] = b"SSXA heartbeat v1\0";
const ANSWER_DOMAIN: &[u8] = b"SSXA heartbeat answer v1\0";
pub const MAX_HEARTBEAT_JSON: usize = 3072;
pub const MAX_ANSWER_JSON: usize = 512;
const HEARTBEAT_FIXED: usize = ENVELOPE + KEY_SIZE + MASTER_CHALLENGE_SIZE + NONCE + MAC + HEARTBEAT_DOMAIN.len();
pub const MIN_HEARTBEAT_BODY: usize = HEARTBEAT_FIXED + 2;
pub const MAX_HEARTBEAT_BODY: usize = HEARTBEAT_FIXED + MAX_HEARTBEAT_JSON;
const ANSWER_FIXED: usize = ENVELOPE + NONCE + MAC + ANSWER_DOMAIN.len();
pub const MAX_ANSWER_BODY: usize = ANSWER_FIXED + MAX_ANSWER_JSON;
pub const MAX_REASON_BYTES: usize = 300;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum MasterError {
    #[error("{what}: {actual} bytes, expected {min}…{max}")]
    Size { what: &'static str, min: usize, max: usize, actual: usize },
    #[error("{0}: not this protocol's envelope (magic or version)")]
    Envelope(&'static str),
    #[error("{0}: the box does not open with these keys")]
    Seal(&'static str),
    #[error("{0}")]
    Field(String),
}

fn field(text: impl Into<String>) -> MasterError {
    MasterError::Field(text.into())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MasterChallenge {
    pub issued_ms: u64,
    pub nonce: [u8; 16],
    pub ephemeral: [u8; KEY_SIZE],
    pub tag: [u8; 16],
}

impl MasterChallenge {
    pub fn to_bytes(&self) -> [u8; MASTER_CHALLENGE_SIZE] {
        let mut out = [0u8; MASTER_CHALLENGE_SIZE];
        out[..8].copy_from_slice(&self.issued_ms.to_be_bytes());
        out[8..24].copy_from_slice(&self.nonce);
        out[24..56].copy_from_slice(&self.ephemeral);
        out[56..].copy_from_slice(&self.tag);
        out
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, MasterError> {
        let b: &[u8; MASTER_CHALLENGE_SIZE] = bytes.try_into().map_err(|_| MasterError::Size {
            what: "master challenge",
            min: MASTER_CHALLENGE_SIZE,
            max: MASTER_CHALLENGE_SIZE,
            actual: bytes.len(),
        })?;
        let mut c = Self { issued_ms: 0, nonce: [0; 16], ephemeral: [0; KEY_SIZE], tag: [0; 16] };
        c.issued_ms = u64::from_be_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]);
        c.nonce.copy_from_slice(&b[8..24]);
        c.ephemeral.copy_from_slice(&b[24..56]);
        c.tag.copy_from_slice(&b[56..]);
        Ok(c)
    }
}

pub struct OneTimeSecret(SecretKey);

impl OneTimeSecret {
    pub fn from_bytes(bytes: [u8; KEY_SIZE]) -> Self {
        Self(SecretKey::from_bytes(bytes))
    }

    pub fn public(&self) -> [u8; KEY_SIZE] {
        self.0.public_key().to_bytes()
    }
}

fn open_box(
    key: &SalsaBox,
    nonce: &[u8; NONCE],
    sealed: &[u8],
    domain: &[u8],
    what: &'static str,
) -> Result<Zeroizing<Vec<u8>>, MasterError> {
    let plain = Zeroizing::new(key.decrypt(nonce.into(), sealed).map_err(|_| MasterError::Seal(what))?);
    match plain.strip_prefix(domain) {
        Some(json) => Ok(Zeroizing::new(json.to_vec())),
        None => Err(MasterError::Seal(what)),
    }
}

fn seal_box(key: &SalsaBox, magic: [u8; 4], head: &[u8], domain: &[u8], json: &[u8]) -> Result<Vec<u8>, MasterError> {
    let nonce: [u8; NONCE] = crate::keys::random_bytes();
    let mut plain = Zeroizing::new(Vec::with_capacity(domain.len() + json.len()));
    plain.extend_from_slice(domain);
    plain.extend_from_slice(json);
    let sealed = key.encrypt((&nonce).into(), plain.as_slice()).map_err(|_| MasterError::Seal("sealing"))?;
    let mut out = Vec::with_capacity(ENVELOPE + head.len() + NONCE + sealed.len());
    out.extend_from_slice(&magic);
    out.push(ENVELOPE_VERSION);
    out.extend_from_slice(head);
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&sealed);
    Ok(out)
}

fn envelope<'a>(body: &'a [u8], magic: [u8; 4], what: &'static str) -> Result<&'a [u8], MasterError> {
    match body.split_at_checked(ENVELOPE) {
        Some((head, rest)) if head[..4] == magic && head[4] == ENVELOPE_VERSION => Ok(rest),
        _ => Err(MasterError::Envelope(what)),
    }
}

fn take<const N: usize>(rest: &mut &[u8]) -> Result<[u8; N], MasterError> {
    let (head, tail) = rest.split_at_checked(N).ok_or(MasterError::Field("a message cut short".into()))?;
    *rest = tail;
    let mut out = [0u8; N];
    out.copy_from_slice(head);
    Ok(out)
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Heartbeat {
    pub heartbeat_version: u32,
    pub port: u16,
    pub protocol_version: u32,
    pub info_version: u32,
    pub api_level: u32,
    pub build: String,
    pub name: String,
    pub mode: String,
    pub drop: String,
    #[serde(deserialize_with = "crate::info::present")]
    pub map: Option<crate::instance::MapSummary>,
    pub players: u32,
    pub max_players: u32,
    pub password: bool,
    pub region: String,
    pub tags: Vec<String>,
    pub contact: String,
    pub leaving: bool,
    pub resources: crate::resources::ResourceSummary,
}

// PORT(choice): a region of 1 to 32 and up to 8 tags of 1 to 24 characters of a-z, 0-9 and `-` (FiveM's tags are free
// text; MTA has none), a contact of 128 printable ASCII characters; tuning: the regions and tags operators use, and the
// browser's region and tag filters for them; the original has no master server.
pub const MAX_REGION_CHARS: usize = 32;
pub const MAX_TAGS: usize = 8;
pub const MAX_TAG_CHARS: usize = 24;
pub const MAX_CONTACT_CHARS: usize = 128;

fn slug(text: &str, max: usize, what: &str) -> Result<(), String> {
    if text.is_empty() || text.len() > max {
        return Err(format!("{what} {text:?}: 1 to {max} characters"));
    }
    if !text.bytes().all(|b| matches!(b, b'a'..=b'z' | b'0'..=b'9' | b'-')) {
        return Err(format!("{what} {text:?}: only a-z, 0-9 and -"));
    }
    Ok(())
}

pub fn region(text: &str) -> Result<(), String> {
    if text.is_empty() { Ok(()) } else { slug(text, MAX_REGION_CHARS, "the region") }
}

pub fn tags(list: &[String]) -> Result<(), String> {
    if list.len() > MAX_TAGS {
        return Err(format!("{} tags, at most {MAX_TAGS}", list.len()));
    }
    for (i, t) in list.iter().enumerate() {
        slug(t, MAX_TAG_CHARS, "a tag")?;
        if list[..i].contains(t) {
            return Err(format!("the tag {t:?} twice"));
        }
    }
    Ok(())
}

pub fn contact(text: &str) -> Result<(), String> {
    if text.len() > MAX_CONTACT_CHARS || !text.bytes().all(|b| (0x20..=0x7E).contains(&b)) {
        return Err(format!("the contact: at most {MAX_CONTACT_CHARS} printable ASCII characters"));
    }
    Ok(())
}

pub fn build(text: &str) -> Result<(), String> {
    if text.is_empty() || text.len() > MAX_BUILD_BYTES || !text.bytes().all(|b| (0x20..=0x7E).contains(&b)) {
        return Err("the build id is not 1 to 64 printable ASCII characters".into());
    }
    Ok(())
}

fn exact_name(name: &str) -> Result<(), String> {
    match server_name(name) {
        Ok(n) if n == name => Ok(()),
        Ok(_) => Err("the server name has spaces the name rule removes".into()),
        Err(e) => Err(format!("the server name: {e}")),
    }
}

fn counts(players: u32, max_players: u32) -> Result<(), String> {
    if max_players == 0 || max_players > MAX_ANNOUNCED_PLAYERS || players > max_players {
        return Err(format!("{players} of {max_players} players"));
    }
    Ok(())
}

fn port(port: u16) -> Result<(), String> {
    if port == 0 || port == u16::MAX {
        return Err(format!("the port {port}: 1 to 65534 (the query is on the port + 1)"));
    }
    Ok(())
}

impl Heartbeat {
    pub fn validate(&self) -> Result<(), String> {
        if self.heartbeat_version != HEARTBEAT_VERSION {
            return Err(format!("heartbeat version {}, this master reads {HEARTBEAT_VERSION}", self.heartbeat_version));
        }
        port(self.port)?;
        build(&self.build)?;
        exact_name(&self.name)?;
        if crate::query::mode_code(&self.mode).is_none() {
            return Err(format!("the mode {:?} is not one of {:?}", self.mode, crate::query::MODES));
        }
        crate::instance::MapSummary::check_level(&self.drop, self.map.as_ref())?;
        counts(self.players, self.max_players)?;
        region(&self.region)?;
        tags(&self.tags)?;
        contact(&self.contact)?;
        self.resources.validate().map_err(|e| format!("the resources: {e}"))
    }

    pub fn seal(&self, server: &ServerSecret, challenge: &MasterChallenge) -> Result<Vec<u8>, MasterError> {
        self.validate().map_err(MasterError::Field)?;
        let json = serde_json::to_vec(self).map_err(|e| field(e.to_string()))?;
        if json.len() > MAX_HEARTBEAT_JSON {
            return Err(field(format!("{} bytes of heartbeat, at most {MAX_HEARTBEAT_JSON}", json.len())));
        }
        seal_heartbeat_json(&json, server, challenge)
    }
}

pub fn seal_heartbeat_json(
    json: &[u8],
    server: &ServerSecret,
    challenge: &MasterChallenge,
) -> Result<Vec<u8>, MasterError> {
    let ephemeral = ServerKey::from_bytes(challenge.ephemeral)
        .map_err(|_| field("the master's one-time key is a low-order point"))?;
    let key = SalsaBox::new(&ephemeral.box_key(), server.box_secret());
    let mut head = Vec::with_capacity(KEY_SIZE + MASTER_CHALLENGE_SIZE);
    head.extend_from_slice(server.public().as_bytes());
    head.extend_from_slice(&challenge.to_bytes());
    seal_box(&key, HEARTBEAT_MAGIC, &head, HEARTBEAT_DOMAIN, json)
}

pub fn seal_answer_json(json: &[u8], server: &ServerKey, secret: &OneTimeSecret) -> Result<Vec<u8>, MasterError> {
    let key = SalsaBox::new(&server.box_key(), &secret.0);
    seal_box(&key, ANSWER_MAGIC, &[], ANSWER_DOMAIN, json)
}

pub struct HeartbeatEnvelope<'a> {
    pub server: [u8; KEY_SIZE],
    pub challenge: MasterChallenge,
    nonce: [u8; NONCE],
    sealed: &'a [u8],
}

impl<'a> HeartbeatEnvelope<'a> {
    pub fn read(body: &'a [u8]) -> Result<Self, MasterError> {
        if !(MIN_HEARTBEAT_BODY..=MAX_HEARTBEAT_BODY).contains(&body.len()) {
            return Err(MasterError::Size {
                what: "heartbeat",
                min: MIN_HEARTBEAT_BODY,
                max: MAX_HEARTBEAT_BODY,
                actual: body.len(),
            });
        }
        let mut rest = envelope(body, HEARTBEAT_MAGIC, "heartbeat")?;
        let server = take::<KEY_SIZE>(&mut rest)?;
        let challenge = MasterChallenge::from_bytes(&take::<MASTER_CHALLENGE_SIZE>(&mut rest)?)?;
        let nonce = take::<NONCE>(&mut rest)?;
        Ok(Self { server, challenge, nonce, sealed: rest })
    }

    pub fn open(&self, server: &ServerKey, secret: &OneTimeSecret) -> Result<Heartbeat, MasterError> {
        let key = SalsaBox::new(&server.box_key(), &secret.0);
        let json = open_box(&key, &self.nonce, self.sealed, HEARTBEAT_DOMAIN, "heartbeat")?;
        let beat: Heartbeat =
            serde_json::from_slice(&json).map_err(|e| field(format!("the heartbeat does not read: {e}")))?;
        beat.validate().map_err(MasterError::Field)?;
        Ok(beat)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HeartbeatAnswer {
    pub listed: bool,
    pub reason: String,
}

fn reason_text(text: &str) -> Result<(), String> {
    if text.len() > MAX_REASON_BYTES || text.chars().any(char::is_control) {
        return Err(format!("a reason of at most {MAX_REASON_BYTES} bytes without control characters"));
    }
    Ok(())
}

impl HeartbeatAnswer {
    pub fn seal(&self, server: &ServerKey, secret: &OneTimeSecret) -> Result<Vec<u8>, MasterError> {
        let mut reason: String = self.reason.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
        while reason.len() > MAX_REASON_BYTES {
            reason.pop();
        }
        let json = serde_json::to_vec(&Self { listed: self.listed, reason }).map_err(|e| field(e.to_string()))?;
        if json.len() > MAX_ANSWER_JSON {
            return Err(field("the answer is too long"));
        }
        let key = SalsaBox::new(&server.box_key(), &secret.0);
        seal_box(&key, ANSWER_MAGIC, &[], ANSWER_DOMAIN, &json)
    }

    pub fn open(body: &[u8], server: &ServerSecret, challenge: &MasterChallenge) -> Result<Self, MasterError> {
        let min = ANSWER_FIXED + 2;
        if !(min..=MAX_ANSWER_BODY).contains(&body.len()) {
            return Err(MasterError::Size { what: "heartbeat answer", min, max: MAX_ANSWER_BODY, actual: body.len() });
        }
        let mut rest = envelope(body, ANSWER_MAGIC, "heartbeat answer")?;
        let nonce = take::<NONCE>(&mut rest)?;
        let ephemeral = ServerKey::from_bytes(challenge.ephemeral)
            .map_err(|_| field("the master's one-time key is a low-order point"))?;
        let key = SalsaBox::new(&ephemeral.box_key(), server.box_secret());
        let json = open_box(&key, &nonce, rest, ANSWER_DOMAIN, "heartbeat answer")?;
        let answer: Self =
            serde_json::from_slice(&json).map_err(|e| field(format!("the master's answer does not read: {e}")))?;
        reason_text(&answer.reason).map_err(MasterError::Field)?;
        Ok(answer)
    }
}

pub fn address_text(ip: IpAddr) -> String {
    match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or_else(|| v6.to_string(), |v4| v4.to_string()),
        v4 => v4.to_string(),
    }
}

// PORT(choice): 10 000 servers (MTA's master lists a few thousand servers, FiveM's up to about 30 000); tuning: the
// size of real masters' lists; the original has no master server.
pub const MAX_LIST_SERVERS: usize = 10_000;
pub const MAX_LIST_ENTRY_BYTES: usize = 2304;
pub const MAX_LIST_BYTES: usize = 64 + MAX_LIST_SERVERS * (MAX_LIST_ENTRY_BYTES + 1);
pub const MAX_BLOCKLIST_ENTRIES: usize = 100_000;
pub const MAX_BLOCKLIST_BYTES: usize = 8 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ListedServer {
    pub address: String,
    pub port: u16,
    pub key: String,
    pub name: String,
    pub mode: String,
    pub drop: String,
    #[serde(deserialize_with = "crate::info::present")]
    pub map: Option<crate::instance::MapSummary>,
    pub players: u32,
    pub max_players: u32,
    pub password: bool,
    pub region: String,
    pub tags: Vec<String>,
    pub build: String,
    pub api_level: u32,
    pub info_version: u32,
    pub info_hash: String,
    pub resources: crate::resources::ResourceSummary,
}

fn lower_hex(text: &str, what: &str) -> Result<[u8; KEY_SIZE], String> {
    let bytes = from_hex(text).map_err(|e| format!("{what}: {e}"))?;
    if to_hex(&bytes) != text {
        return Err(format!("{what}: not lower-case hex"));
    }
    Ok(bytes)
}

impl ListedServer {
    pub fn ip(&self) -> Result<IpAddr, String> {
        let ip: IpAddr =
            self.address.parse().map_err(|_| format!("the address {:?} is not an IP address", self.address))?;
        if address_text(ip) != self.address {
            return Err(format!("the address {:?} is not written as a master writes it", self.address));
        }
        if ip.is_unspecified() || ip.is_multicast() || ip == IpAddr::from([255, 255, 255, 255]) {
            return Err(format!("the address {} is not a server's", self.address));
        }
        Ok(ip)
    }

    pub fn validate(&self) -> Result<(), String> {
        self.ip()?;
        port(self.port)?;
        lower_hex(&self.key, "the key")?;
        exact_name(&self.name)?;
        if crate::query::mode_code(&self.mode).is_none() {
            return Err(format!("the mode {:?}", self.mode));
        }
        crate::instance::MapSummary::check_level(&self.drop, self.map.as_ref())?;
        counts(self.players, self.max_players)?;
        region(&self.region)?;
        tags(&self.tags)?;
        build(&self.build)?;
        lower_hex(&self.info_hash, "the info hash")?;
        self.resources.validate().map_err(|e| format!("the resources: {e}"))?;
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServerList {
    pub list_version: u32,
    pub protocol_version: u32,
    #[serde(deserialize_with = "bounded_servers")]
    pub servers: Vec<ListedServer>,
}

struct Bounded<T>(usize, core::marker::PhantomData<T>);

impl<'de, T: Deserialize<'de>> Visitor<'de> for Bounded<T> {
    type Value = Vec<T>;

    fn expecting(&self, f: &mut core::fmt::Formatter) -> core::fmt::Result {
        write!(f, "an array of at most {} entries", self.0)
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Vec<T>, A::Error> {
        let mut out = Vec::new();
        while let Some(x) = seq.next_element()? {
            if out.len() == self.0 {
                return Err(A::Error::custom(format!("more than {} entries", self.0)));
            }
            out.push(x);
        }
        Ok(out)
    }
}

fn bounded_servers<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<ListedServer>, D::Error> {
    d.deserialize_seq(Bounded(MAX_LIST_SERVERS, core::marker::PhantomData))
}

fn bounded_hashes<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<String>, D::Error> {
    d.deserialize_seq(Bounded(MAX_BLOCKLIST_ENTRIES, core::marker::PhantomData))
}

impl ServerList {
    pub fn to_json(&self) -> Vec<u8> {
        serde_json::to_vec(self).unwrap_or_default()
    }

    pub fn from_json(body: &[u8], protocol: u32) -> Result<Self, String> {
        if body.len() > MAX_LIST_BYTES {
            return Err(format!("a list of {} bytes, at most {MAX_LIST_BYTES}", body.len()));
        }
        let list: Self = serde_json::from_slice(body).map_err(|e| format!("the list does not read: {e}"))?;
        if list.list_version != LIST_VERSION {
            return Err(format!("the list is version {}, this game reads {LIST_VERSION}", list.list_version));
        }
        if list.protocol_version != protocol {
            return Err(format!("a list of protocol {} for protocol {protocol}", list.protocol_version));
        }
        let mut keys = std::collections::HashSet::with_capacity(list.servers.len());
        let mut places = std::collections::HashSet::with_capacity(list.servers.len());
        for (i, s) in list.servers.iter().enumerate() {
            s.validate().map_err(|e| format!("server {i}: {e}"))?;
            if !keys.insert(s.key.as_str()) {
                return Err(format!("server {i}: the key {} twice", s.key));
            }
            if !places.insert((s.address.as_str(), s.port)) {
                return Err(format!("server {i}: {}:{} twice", s.address, s.port));
            }
        }
        Ok(list)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Blocklist {
    pub blocklist_version: u32,
    #[serde(deserialize_with = "bounded_hashes")]
    pub resources: Vec<String>,
}

impl Blocklist {
    pub fn to_json(&self) -> Vec<u8> {
        serde_json::to_vec(self).unwrap_or_default()
    }

    pub fn from_json(body: &[u8]) -> Result<Self, String> {
        if body.len() > MAX_BLOCKLIST_BYTES {
            return Err(format!("a blocklist of {} bytes, at most {MAX_BLOCKLIST_BYTES}", body.len()));
        }
        let list: Self = serde_json::from_slice(body).map_err(|e| format!("the blocklist does not read: {e}"))?;
        if list.blocklist_version != BLOCKLIST_VERSION {
            return Err(format!("blocklist version {}, this game reads {BLOCKLIST_VERSION}", list.blocklist_version));
        }
        for h in &list.resources {
            lower_hex(h, "a resource hash")?;
        }
        Ok(list)
    }
}

pub const MAX_MASTER_URL: usize = 256;

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum MasterHost {
    Ip(IpAddr),
    Name(String),
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct MasterUrl {
    pub tls: bool,
    pub host: MasterHost,
    pub port: u16,
}

fn dns_name(s: &str) -> Result<String, String> {
    if s.is_empty() || s.len() > 253 {
        return Err("a host name has 1 to 253 characters".into());
    }
    for label in s.split('.') {
        let ok = !label.is_empty()
            && label.len() <= 63
            && label.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
            && !label.starts_with('-')
            && !label.ends_with('-');
        if !ok {
            return Err(format!("{s:?} is not a host name (letters, digits and - between dots)"));
        }
    }
    if s.bytes().all(|b| b.is_ascii_digit() || b == b'.') {
        return Err(format!("{s:?} is not an IPv4 address"));
    }
    Ok(s.to_ascii_lowercase())
}

fn url_port(s: &str) -> Result<u16, String> {
    let ok = !s.is_empty() && s.len() <= 5 && s.bytes().all(|b| b.is_ascii_digit()) && !s.starts_with('0');
    ok.then(|| s.parse::<u16>().ok()).flatten().ok_or_else(|| format!("the port {s:?} is not 1 to 65535"))
}

impl MasterUrl {
    pub fn parse(text: &str) -> Result<Self, String> {
        if text.is_empty() || text.len() > MAX_MASTER_URL || !text.is_ascii() {
            return Err(format!("a master's address has 1 to {MAX_MASTER_URL} ASCII characters"));
        }
        let (scheme, rest) = text.split_once("://").ok_or("a master's address starts with https:// or http://")?;
        let tls = match scheme.to_ascii_lowercase().as_str() {
            "https" => true,
            "http" => false,
            _ => return Err(format!("the scheme {scheme:?} is not https or http")),
        };
        let rest = rest.strip_suffix('/').unwrap_or(rest);
        if let Some(c) = rest.chars().find(|c| matches!(c, '/' | '?' | '#' | '@' | '%' | '\\') || !c.is_ascii_graphic())
        {
            return Err(format!("{c:?} in a master's address (only https://host[:port])"));
        }
        let default = if tls { 443 } else { 80 };
        let (host, port) = if let Some(v6) = rest.strip_prefix('[') {
            let (ip, after) = v6.split_once(']').ok_or("an IPv6 address in [ ] has no closing ]")?;
            let ip: std::net::Ipv6Addr = ip.parse().map_err(|_| format!("{ip:?} is not an IPv6 address"))?;
            let port = match after.strip_prefix(':') {
                Some(p) => url_port(p)?,
                None if after.is_empty() => default,
                None => return Err(format!("{after:?} after the IPv6 address")),
            };
            (MasterHost::Ip(IpAddr::V6(ip)), port)
        } else {
            if rest.matches(':').count() > 1 {
                return Err("an IPv6 address is written in [ ]".into());
            }
            let (h, port) = match rest.split_once(':') {
                Some((h, p)) => (h, url_port(p)?),
                None => (rest, default),
            };
            match h.parse::<std::net::Ipv4Addr>() {
                Ok(ip) => (MasterHost::Ip(IpAddr::V4(ip)), port),
                Err(_) => (MasterHost::Name(dns_name(h)?), port),
            }
        };
        if let MasterHost::Ip(ip) = host
            && (ip.is_unspecified() || ip.is_multicast())
        {
            return Err(format!("{ip} is not a master's address"));
        }
        Ok(Self { tls, host, port })
    }

    pub fn host_text(&self) -> String {
        match &self.host {
            MasterHost::Ip(ip) => ip.to_string(),
            MasterHost::Name(n) => n.clone(),
        }
    }

    pub fn authority(&self) -> String {
        let host = match &self.host {
            MasterHost::Ip(IpAddr::V6(ip)) => format!("[{ip}]"),
            _ => self.host_text(),
        };
        if self.port == if self.tls { 443 } else { 80 } { host } else { format!("{host}:{}", self.port) }
    }
}

pub fn the_master() -> Option<MasterUrl> {
    ssx_core::master::master().url().and_then(|u| MasterUrl::parse(u).ok())
}

impl core::fmt::Display for MasterUrl {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{}://{}", if self.tls { "https" } else { "http" }, self.authority())
    }
}

pub fn heartbeat_against_info(beat: &Heartbeat, key: &ServerKey, info: &ServerInfo) -> Option<String> {
    let differs = |what: &str, info: &dyn core::fmt::Debug, beat: &dyn core::fmt::Debug| {
        Some(format!("its /info says {what} {info:?}, its heartbeat {beat:?}"))
    };
    if info.identity_key != key.to_hex() {
        return Some("its /info names another identity key than the one its heartbeat was boxed with".into());
    }
    if info.info_version != beat.info_version {
        return differs("info version", &info.info_version, &beat.info_version);
    }
    if info.protocol_version != beat.protocol_version {
        return differs("protocol", &info.protocol_version, &beat.protocol_version);
    }
    if info.api_level != beat.api_level {
        return differs("API level", &info.api_level, &beat.api_level);
    }
    if info.build != beat.build {
        return differs("build", &info.build, &beat.build);
    }
    if info.name != beat.name {
        return differs("name", &info.name, &beat.name);
    }
    if info.mode != beat.mode {
        return differs("mode", &info.mode, &beat.mode);
    }
    if info.drop != beat.drop {
        return differs("drop", &info.drop, &beat.drop);
    }
    if info.map != beat.map {
        return differs("map", &info.map, &beat.map);
    }
    if info.max_players != beat.max_players {
        return differs("max players", &info.max_players, &beat.max_players);
    }
    if info.password != beat.password {
        return differs("password", &info.password, &beat.password);
    }
    if info.resources != beat.resources {
        return Some("its /info states other resources than its heartbeat".into());
    }
    if info.region != beat.region {
        return differs("region", &info.region, &beat.region);
    }
    if info.tags != beat.tags {
        return differs("tags", &info.tags, &beat.tags);
    }
    None
}

#[cfg(test)]
mod tests;
