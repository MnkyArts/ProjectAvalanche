use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::time::Duration;

use crypto_box::aead::OsRng;
use zeroize::Zeroizing;

use crate::JoinError;
use crate::keys::{KEY_SIZE, PlayerKey, PlayerSecret, ReplyKey, ReplySecret, ServerKey, ServerSecret};
use crate::operator::Expiry;

pub const CHALLENGE_SIZE: usize = 40;
pub const TOKEN_SIZE: usize = 2048;
pub const MAX_NAME_BYTES: usize = 64;
pub const MAX_BUILD_BYTES: usize = 64;
// PORT(choice): 64 bytes; tuning: the on-screen keyboard's password field of the server browser (phase F2); the
// original has no dedicated server.
pub const MAX_PASSWORD_BYTES: usize = 64;
// PORT(choice): 200 bytes, as a control message's text; tuning: the refusal popup's text field (phase F2); the original
// has no dedicated server.
pub const MAX_REFUSAL_TEXT: usize = 200;
pub const HOLD_TICKET_SIZE: usize = 32;
pub const MAX_HOLD_TEXT: usize = 256;
pub const MIN_ASK_AFTER_MS: u16 = 500;
pub const MAX_ASK_AFTER_MS: u16 = 5_000;
// PORT(choice): 120 s; tuning: the first gamemode with a queue or an outside check on a full server, and the user's
// word on whether a queue for a full server belongs to the scripts (a held join takes no slot while it waits); the
// original has no dedicated server.
pub const MAX_HOLD: Duration = Duration::from_secs(120);

const JOIN_MAGIC: [u8; 4] = *b"SSXJ";
const ANSWER_MAGIC: [u8; 4] = *b"SSXR";
const HELD_MAGIC: [u8; 4] = *b"SSXH";
const ENVELOPE_VERSION: u8 = 1;
pub(crate) const ENVELOPE: usize = 5;
pub(crate) const SEAL: usize = crypto_box::SEALBYTES;
pub(crate) const SIGNATURE: usize = 64;
const SIGNATURE_DOMAIN: &[u8] = b"SSXA join request v1\0";

pub const STABLE_PREFIX: usize = 8 + KEY_SIZE;
const FIXED: usize = STABLE_PREFIX + CHALLENGE_SIZE + KEY_SIZE + 19;
const MIN_INNER: usize = FIXED + 2 + 2 + 1 + 1 + SIGNATURE;
const MAX_INNER: usize =
    FIXED + 1 + MAX_NAME_BYTES + 1 + MAX_BUILD_BYTES + 1 + MAX_PASSWORD_BYTES + JoinContent::MAX_SIZE + SIGNATURE;
pub const MIN_JOIN_BODY: usize = ENVELOPE + SEAL + MIN_INNER;
pub const MAX_JOIN_BODY: usize = ENVELOPE + SEAL + MAX_INNER;
pub const MAX_ANSWER_BODY: usize = ENVELOPE + SEAL + 1 + TOKEN_SIZE;
const MIN_ANSWER_BODY: usize = ENVELOPE + SEAL + 1 + 4;
pub const HELD_ASK_BODY: usize = ENVELOPE + SEAL + HOLD_TICKET_SIZE + KEY_SIZE;
const _: () = assert!(1 + HOLD_TICKET_SIZE + 2 + 2 + MAX_HOLD_TEXT <= 1 + TOKEN_SIZE, "a held answer fits the body");

pub struct JoinRequest {
    pub protocol_version: u32,
    pub api_level: u32,
    pub reply: ReplyKey,
    pub challenge: [u8; CHALLENGE_SIZE],
    pub player: PlayerKey,
    pub server_addr: SocketAddr,
    pub name: String,
    pub build: String,
    pub password: Zeroizing<Vec<u8>>,
    pub content: JoinContent,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct JoinContent {
    pub content_id: Option<[u8; 32]>,
    pub manifest: Option<[u8; 32]>,
}

impl JoinContent {
    const CONTENT_ID: u8 = 1;
    const MANIFEST: u8 = 2;
    pub const MAX_SIZE: usize = 1 + 32 + 32;

    fn put(&self, out: &mut Vec<u8>) {
        let flags = self.content_id.map_or(0, |_| Self::CONTENT_ID) | self.manifest.map_or(0, |_| Self::MANIFEST);
        out.push(flags);
        for field in [&self.content_id, &self.manifest].into_iter().flatten() {
            out.extend_from_slice(field);
        }
    }

    fn read(r: &mut Cursor) -> Result<Self, JoinError> {
        let flags = r.u8()?;
        if flags & !(Self::CONTENT_ID | Self::MANIFEST) != 0 {
            return Err(JoinError::Field("the content fields' flags have undefined bits"));
        }
        let content_id = if flags & Self::CONTENT_ID != 0 { Some(r.array()?) } else { None };
        let manifest = if flags & Self::MANIFEST != 0 { Some(r.array()?) } else { None };
        Ok(Self { content_id, manifest })
    }
}

impl JoinRequest {
    pub fn seal(&self, server: &ServerKey, signer: &PlayerSecret) -> Result<Vec<u8>, JoinError> {
        seal_inner(&self.signed_bytes(server, signer)?, server)
    }

    pub fn signed_bytes(&self, server: &ServerKey, signer: &PlayerSecret) -> Result<Zeroizing<Vec<u8>>, JoinError> {
        if signer.public() != self.player {
            return Err(JoinError::Field("the signing key is not the request's player key"));
        }
        let mut inner = Zeroizing::new(Vec::with_capacity(MAX_INNER));
        inner.extend_from_slice(&self.protocol_version.to_be_bytes());
        inner.extend_from_slice(&self.api_level.to_be_bytes());
        inner.extend_from_slice(self.reply.as_bytes());
        inner.extend_from_slice(&self.challenge);
        inner.extend_from_slice(self.player.as_bytes());
        put_addr(&mut inner, self.server_addr);
        put_field(&mut inner, self.name.as_bytes(), 1, MAX_NAME_BYTES, "the name")?;
        put_field(&mut inner, self.build.as_bytes(), 1, MAX_BUILD_BYTES, "the build id")?;
        put_field(&mut inner, &self.password, 0, MAX_PASSWORD_BYTES, "the password")?;
        self.content.put(&mut inner);
        let signature = signer.sign(&signed_message(server, &inner));
        inner.extend_from_slice(&signature);
        Ok(inner)
    }
}

pub fn seal_inner(inner: &[u8], server: &ServerKey) -> Result<Vec<u8>, JoinError> {
    seal_envelope(JOIN_MAGIC, &server.box_key(), inner)
}

pub fn sign_inner(inner_unsigned: &[u8], server: &ServerKey, signer: &PlayerSecret) -> Vec<u8> {
    let mut out = inner_unsigned.to_vec();
    out.extend_from_slice(&signer.sign(&signed_message(server, inner_unsigned)));
    out
}

fn put_addr(out: &mut Vec<u8>, addr: SocketAddr) {
    let mut ip = [0u8; 16];
    let family = match addr.ip() {
        IpAddr::V4(v4) => {
            ip[..4].copy_from_slice(&v4.octets());
            4
        }
        IpAddr::V6(v6) => {
            ip.copy_from_slice(&v6.octets());
            6
        }
    };
    out.push(family);
    out.extend_from_slice(&ip);
    out.extend_from_slice(&addr.port().to_be_bytes());
}

fn put_field(out: &mut Vec<u8>, bytes: &[u8], min: usize, max: usize, what: &'static str) -> Result<(), JoinError> {
    if bytes.len() < min || bytes.len() > max {
        return Err(JoinError::Size { what, min, max, actual: bytes.len() });
    }
    out.push(bytes.len() as u8);
    out.extend_from_slice(bytes);
    Ok(())
}

fn signed_message(server: &ServerKey, inner_unsigned: &[u8]) -> Zeroizing<Vec<u8>> {
    let mut m = Zeroizing::new(Vec::with_capacity(SIGNATURE_DOMAIN.len() + KEY_SIZE + inner_unsigned.len()));
    m.extend_from_slice(SIGNATURE_DOMAIN);
    m.extend_from_slice(server.as_bytes());
    m.extend_from_slice(inner_unsigned);
    m
}

pub(crate) fn seal_envelope(magic: [u8; 4], to: &crypto_box::PublicKey, inner: &[u8]) -> Result<Vec<u8>, JoinError> {
    let sealed = to.seal(&mut OsRng, inner).map_err(|_| JoinError::Seal("sealing"))?;
    let mut out = Vec::with_capacity(ENVELOPE + sealed.len());
    out.extend_from_slice(&magic);
    out.push(ENVELOPE_VERSION);
    out.extend_from_slice(&sealed);
    Ok(out)
}

pub(crate) fn open_envelope(
    what: &'static str,
    magic: [u8; 4],
    body: &[u8],
    (min, max): (usize, usize),
    secret: &crypto_box::SecretKey,
) -> Result<Zeroizing<Vec<u8>>, JoinError> {
    if body.len() < min || body.len() > max {
        return Err(JoinError::Size { what, min, max, actual: body.len() });
    }
    if body[..4] != magic || body[4] != ENVELOPE_VERSION {
        return Err(JoinError::Envelope(what));
    }
    secret.unseal(&body[ENVELOPE..]).map(Zeroizing::new).map_err(|_| JoinError::Seal(what))
}

pub struct OpenedJoin(Zeroizing<Vec<u8>>);

pub fn open_join(body: &[u8], secret: &ServerSecret) -> Result<OpenedJoin, JoinError> {
    open_envelope("join request", JOIN_MAGIC, body, (MIN_JOIN_BODY, MAX_JOIN_BODY), secret.box_secret()).map(OpenedJoin)
}

#[derive(Clone, Copy, Debug)]
pub struct JoinPrefix {
    pub protocol_version: u32,
    pub api_level: u32,
    pub reply: ReplyKey,
}

impl OpenedJoin {
    pub fn prefix(&self) -> Result<JoinPrefix, JoinError> {
        let mut r = Cursor(&self.0);
        let protocol_version = r.u32()?;
        let api_level = r.u32()?;
        let reply = ReplyKey::from_bytes(r.array()?)?;
        Ok(JoinPrefix { protocol_version, api_level, reply })
    }

    pub fn request(&self, server: &ServerKey) -> Result<JoinRequest, JoinError> {
        let bytes: &[u8] = &self.0;
        if bytes.len() < MIN_INNER || bytes.len() > MAX_INNER {
            return Err(JoinError::Size { what: "join request", min: MIN_INNER, max: MAX_INNER, actual: bytes.len() });
        }
        let JoinPrefix { protocol_version, api_level, reply } = self.prefix()?;
        let mut r = Cursor(&bytes[STABLE_PREFIX..]);
        let challenge = r.array()?;
        let player = PlayerKey::from_bytes(r.array()?)?;
        let server_addr = r.addr()?;
        let name = r.field(1, MAX_NAME_BYTES, "the name")?;
        let name = core::str::from_utf8(name).map_err(|_| JoinError::Field("the name is not UTF-8"))?.to_string();
        let build = r.field(1, MAX_BUILD_BYTES, "the build id")?;
        if !build.iter().all(|b| (0x20..=0x7E).contains(b)) {
            return Err(JoinError::Field("the build id is not printable ASCII"));
        }
        let build = String::from_utf8_lossy(build).into_owned();
        let password = Zeroizing::new(r.field(0, MAX_PASSWORD_BYTES, "the password")?.to_vec());
        let content = JoinContent::read(&mut r)?;
        let signature: [u8; SIGNATURE] = r.array()?;
        if !r.0.is_empty() {
            return Err(JoinError::Field("bytes after the signature"));
        }
        let unsigned = &bytes[..bytes.len() - SIGNATURE];
        if !player.verify(&signed_message(server, unsigned), &signature) {
            return Err(JoinError::Signature);
        }
        Ok(JoinRequest {
            protocol_version,
            api_level,
            reply,
            challenge,
            player,
            server_addr,
            name,
            build,
            password,
            content,
        })
    }
}

pub(crate) struct Cursor<'a>(pub(crate) &'a [u8]);

impl<'a> Cursor<'a> {
    pub(crate) fn take(&mut self, n: usize) -> Result<&'a [u8], JoinError> {
        if self.0.len() < n {
            return Err(JoinError::Field("the request ends early"));
        }
        let (head, rest) = self.0.split_at(n);
        self.0 = rest;
        Ok(head)
    }

    pub(crate) fn array<const N: usize>(&mut self) -> Result<[u8; N], JoinError> {
        let mut out = [0u8; N];
        out.copy_from_slice(self.take(N)?);
        Ok(out)
    }

    pub(crate) fn u8(&mut self) -> Result<u8, JoinError> {
        Ok(self.take(1)?[0])
    }

    pub(crate) fn u16(&mut self) -> Result<u16, JoinError> {
        Ok(u16::from_be_bytes(self.array()?))
    }

    pub(crate) fn u32(&mut self) -> Result<u32, JoinError> {
        Ok(u32::from_be_bytes(self.array()?))
    }

    pub(crate) fn field(&mut self, min: usize, max: usize, what: &'static str) -> Result<&'a [u8], JoinError> {
        let len = usize::from(self.u8()?);
        if len < min || len > max {
            return Err(JoinError::Size { what, min, max, actual: len });
        }
        self.take(len)
    }

    fn addr(&mut self) -> Result<SocketAddr, JoinError> {
        let family = self.u8()?;
        let ip: [u8; 16] = self.array()?;
        let port = self.u16()?;
        let ip = match family {
            4 if ip[4..] == [0; 12] => IpAddr::V4(Ipv4Addr::new(ip[0], ip[1], ip[2], ip[3])),
            6 => IpAddr::V6(Ipv6Addr::from(ip)),
            _ => return Err(JoinError::Field("the server address is not IPv4 or IPv6")),
        };
        if port == 0 || ip.is_unspecified() || ip.is_multicast() {
            return Err(JoinError::Field("the server address is not a unicast address with a port"));
        }
        Ok(SocketAddr::new(ip, port))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RefusalCode {
    Protocol = 1,
    ApiLevel = 2,
    Challenge = 3,
    Request = 4,
    Name = 5,
    Flood = 6,
    Password = 7,
    Full = 8,
    Busy = 9,
    ResourcesChanged = 10,
    Content = 11,
    Banned = 12,
    Scripts = 20,
}

impl RefusalCode {
    fn from_byte(b: u8) -> Option<Self> {
        Some(match b {
            1 => Self::Protocol,
            2 => Self::ApiLevel,
            3 => Self::Challenge,
            4 => Self::Request,
            5 => Self::Name,
            6 => Self::Flood,
            7 => Self::Password,
            8 => Self::Full,
            9 => Self::Busy,
            10 => Self::ResourcesChanged,
            11 => Self::Content,
            12 => Self::Banned,
            20 => Self::Scripts,
            _ => return None,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Refusal {
    pub code: RefusalCode,
    pub retry_after_secs: u16,
    pub text: String,
    pub ban: Option<Expiry>,
}

impl Refusal {
    pub fn new(code: RefusalCode, retry_after_secs: u16, text: String) -> Self {
        Self { code, retry_after_secs, text, ban: None }
    }

    pub fn banned(text: String, until: Expiry) -> Self {
        Self { code: RefusalCode::Banned, retry_after_secs: 0, text, ban: Some(until) }
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct Hold {
    pub ticket: [u8; HOLD_TICKET_SIZE],
    pub ask_after_ms: u16,
    pub text: String,
}

impl core::fmt::Debug for Hold {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "Hold {{ ask after {} ms, {:?} }}", self.ask_after_ms, self.text)
    }
}

pub fn hold_text(text: &str) -> String {
    let mut out = String::new();
    for c in text.chars().filter(|c| !ssx_core::text::is_hidden(*c)) {
        if out.len() + c.len_utf8() > MAX_HOLD_TEXT {
            break;
        }
        out.push(c);
    }
    out
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JoinAnswer {
    Accepted(Box<[u8; TOKEN_SIZE]>),
    Refused(Refusal),
    Held(Hold),
}

impl JoinAnswer {
    const ACCEPTED: u8 = 1;
    const REFUSED: u8 = 2;
    const HELD: u8 = 3;

    pub fn seal(&self, reply: &ReplyKey) -> Result<Vec<u8>, JoinError> {
        let mut inner = Zeroizing::new(Vec::with_capacity(1 + TOKEN_SIZE));
        match self {
            Self::Accepted(token) => {
                inner.push(Self::ACCEPTED);
                inner.extend_from_slice(&token[..]);
            }
            Self::Refused(r) => {
                let mut text: String = r.text.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
                while text.len() > MAX_REFUSAL_TEXT {
                    text.pop();
                }
                inner.push(Self::REFUSED);
                inner.push(r.code as u8);
                inner.extend_from_slice(&r.retry_after_secs.to_be_bytes());
                match (r.code, r.ban) {
                    (RefusalCode::Banned, Some(until)) => inner.extend_from_slice(&until.to_wire().to_be_bytes()),
                    (RefusalCode::Banned, None) | (_, Some(_)) => {
                        return Err(JoinError::Field("a ban's end belongs to the refusal Banned, and only to it"));
                    }
                    _ => {}
                }
                inner.push(text.len() as u8);
                inner.extend_from_slice(text.as_bytes());
            }
            Self::Held(hold) => {
                if !(MIN_ASK_AFTER_MS..=MAX_ASK_AFTER_MS).contains(&hold.ask_after_ms) {
                    return Err(JoinError::Field("a held answer's pause is outside the protocol's range"));
                }
                let text = hold_text(&hold.text);
                inner.push(Self::HELD);
                inner.extend_from_slice(&hold.ticket);
                inner.extend_from_slice(&hold.ask_after_ms.to_be_bytes());
                inner.extend_from_slice(&(text.len() as u16).to_be_bytes());
                inner.extend_from_slice(text.as_bytes());
            }
        }
        seal_envelope(ANSWER_MAGIC, &reply.box_key(), &inner)
    }

    pub fn open(body: &[u8], reply: &ReplySecret) -> Result<Self, JoinError> {
        let inner =
            open_envelope("join answer", ANSWER_MAGIC, body, (MIN_ANSWER_BODY, MAX_ANSWER_BODY), reply.box_secret())?;
        let mut r = Cursor(&inner);
        let answer = match r.u8()? {
            Self::ACCEPTED => Self::Accepted(Box::new(r.array()?)),
            Self::REFUSED => {
                let code = RefusalCode::from_byte(r.u8()?).ok_or(JoinError::Field("an unknown refusal code"))?;
                let retry_after_secs = r.u16()?;
                let ban = match code {
                    RefusalCode::Banned => Some(Expiry::from_wire(u64::from_be_bytes(r.array()?))),
                    _ => None,
                };
                let text = r.field(0, MAX_REFUSAL_TEXT, "the refusal's text")?;
                let text =
                    core::str::from_utf8(text).map_err(|_| JoinError::Field("the refusal's text is not UTF-8"))?;
                if text.chars().any(char::is_control) {
                    return Err(JoinError::Field("the refusal's text has control characters"));
                }
                Self::Refused(Refusal { code, retry_after_secs, text: text.to_string(), ban })
            }
            Self::HELD => {
                let ticket = r.array()?;
                let ask_after_ms = r.u16()?;
                if !(MIN_ASK_AFTER_MS..=MAX_ASK_AFTER_MS).contains(&ask_after_ms) {
                    return Err(JoinError::Field("a held answer's pause is outside the protocol's range"));
                }
                let len = usize::from(r.u16()?);
                if len > MAX_HOLD_TEXT {
                    return Err(JoinError::Size {
                        what: "a held answer's text",
                        min: 0,
                        max: MAX_HOLD_TEXT,
                        actual: len,
                    });
                }
                let text = core::str::from_utf8(r.take(len)?)
                    .map_err(|_| JoinError::Field("a held answer's text is not UTF-8"))?;
                if text.chars().any(ssx_core::text::is_hidden) {
                    return Err(JoinError::Field("a held answer's text has hidden characters"));
                }
                Self::Held(Hold { ticket, ask_after_ms, text: text.to_string() })
            }
            _ => return Err(JoinError::Field("an unknown answer kind")),
        };
        if !r.0.is_empty() {
            return Err(JoinError::Field("bytes after the answer"));
        }
        Ok(answer)
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct HeldAsk {
    pub ticket: [u8; HOLD_TICKET_SIZE],
    pub reply: ReplyKey,
}

pub fn seal_answer_inner(inner: &[u8], reply: &ReplyKey) -> Result<Vec<u8>, JoinError> {
    seal_envelope(ANSWER_MAGIC, &reply.box_key(), inner)
}

pub fn seal_held_ask_inner(inner: &[u8], server: &ServerKey) -> Result<Vec<u8>, JoinError> {
    seal_envelope(HELD_MAGIC, &server.box_key(), inner)
}

impl core::fmt::Debug for HeldAsk {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "HeldAsk {{ {:?} }}", self.reply)
    }
}

impl HeldAsk {
    pub fn seal(&self, server: &ServerKey) -> Result<Vec<u8>, JoinError> {
        let mut inner = Zeroizing::new(Vec::with_capacity(HOLD_TICKET_SIZE + KEY_SIZE));
        inner.extend_from_slice(&self.ticket);
        inner.extend_from_slice(self.reply.as_bytes());
        seal_envelope(HELD_MAGIC, &server.box_key(), &inner)
    }

    pub fn open(body: &[u8], secret: &ServerSecret) -> Result<Self, JoinError> {
        let inner =
            open_envelope("held join's ask", HELD_MAGIC, body, (HELD_ASK_BODY, HELD_ASK_BODY), secret.box_secret())?;
        let mut r = Cursor(&inner);
        let ticket = r.array()?;
        let reply = ReplyKey::from_bytes(r.array()?)?;
        if !r.0.is_empty() {
            return Err(JoinError::Field("bytes after the ask"));
        }
        Ok(Self { ticket, reply })
    }
}

#[cfg(test)]
mod tests;
