use crate::DecodeError;
use crate::name::drawable;

// PORT(choice): 100 characters, two wrapped lines of the chat layer's 16-px text; leads (recalled, not re-read: this
// round had no network beyond loopback): MTA:SA's chat input and FiveM's chat resource cap a line at a few hundred
// characters; tuning: the chat layer's row measured at the widest glyphs (W, M, Œ) and what players type; the original
// has no chat.
pub const MAX_CHAT_CHARS: usize = 100;

pub const MAX_CHAT_BYTES: usize = MAX_CHAT_CHARS * 3;

pub const QUICK_LINES: u8 = 8;

pub const MAX_CLIENT_CHAT_SIZE: usize = 3 + MAX_CHAT_BYTES;
pub const MAX_SERVER_CHAT_SIZE: usize = 3 + 3 + MUTE_END + MAX_CHAT_BYTES;

const MUTE_END: usize = 8;

pub fn chat_text(text: &str) -> Result<&str, DecodeError> {
    if text.len() > MAX_CHAT_BYTES {
        return Err(DecodeError::Text {
            kind: "chat",
            detail: format!("{} bytes, at most {MAX_CHAT_BYTES}", text.len()),
        });
    }
    if let Some(c) = text.chars().find(|&c| !drawable(c)) {
        return Err(DecodeError::Text { kind: "chat", detail: format!("{c:?} is not a character the fonts draw") });
    }
    let chars = text.chars().count();
    if chars > MAX_CHAT_CHARS {
        return Err(DecodeError::Text {
            kind: "chat",
            detail: format!("{chars} characters, at most {MAX_CHAT_CHARS}"),
        });
    }
    if text.chars().all(|c| c == ' ') {
        return Err(DecodeError::Text { kind: "chat", detail: "an empty line".into() });
    }
    Ok(text)
}

fn text_of(bytes: &[u8]) -> Result<String, DecodeError> {
    let text = core::str::from_utf8(bytes).map_err(|_| DecodeError::Field("chat: the text is not UTF-8"))?;
    Ok(chat_text(text)?.to_string())
}

fn quick(index: u8) -> Result<u8, DecodeError> {
    if index >= QUICK_LINES {
        return Err(DecodeError::Field("chat: no such quick-chat line"));
    }
    Ok(index)
}

fn frame<'a>(bytes: &'a [u8], max: usize, what: &'static str) -> Result<(u8, &'a [u8]), DecodeError> {
    if bytes.len() < 3 || bytes.len() > max {
        return Err(DecodeError::Size { kind: what, expected: max, actual: bytes.len() });
    }
    let len = usize::from(u16::from_be_bytes([bytes[1], bytes[2]]));
    if len != bytes.len() - 3 {
        return Err(DecodeError::Field("chat: the payload length does not match"));
    }
    Ok((bytes[0], &bytes[3..]))
}

fn framed(kind: u8, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(3 + payload.len());
    out.push(kind);
    out.extend_from_slice(&(payload.len() as u16).to_be_bytes());
    out.extend_from_slice(payload);
    out
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClientChat {
    Text(String),
    Quick(u8),
}

impl ClientChat {
    const TEXT: u8 = 1;
    const QUICK: u8 = 2;

    pub fn text(text: &str) -> Result<Self, DecodeError> {
        Ok(Self::Text(chat_text(text)?.to_string()))
    }

    pub fn quick(index: u8) -> Result<Self, DecodeError> {
        Ok(Self::Quick(quick(index)?))
    }

    pub fn encode(&self) -> Vec<u8> {
        match self {
            Self::Text(t) => framed(Self::TEXT, t.as_bytes()),
            Self::Quick(i) => framed(Self::QUICK, &[*i]),
        }
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, DecodeError> {
        let (kind, payload) = frame(bytes, MAX_CLIENT_CHAT_SIZE, "client chat line")?;
        match (kind, payload) {
            (Self::TEXT, text) => Ok(Self::Text(text_of(text)?)),
            (Self::QUICK, &[i]) => Ok(Self::Quick(quick(i)?)),
            _ => Err(DecodeError::Field("client chat line: unknown kind or a payload it does not have")),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum ChatRefusal {
    Flood { wait_ms: u16 },
    Disabled,
    Muted { until: crate::operator::Expiry, reason: String },
}

impl ChatRefusal {
    const FLOOD: u8 = 1;
    const DISABLED: u8 = 2;
    const MUTED: u8 = 3;

    pub fn muted(until: crate::operator::Expiry, reason: &str) -> Result<Self, DecodeError> {
        if !reason.is_empty() {
            chat_text(reason)?;
        }
        Ok(Self::Muted { until, reason: reason.to_string() })
    }
    pub const MAX_WAIT_MS: u16 = 60_000;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ServerChat {
    Line { rider: u16, text: String },
    Quick { rider: u16, index: u8 },
    Server(String),
    Refused(ChatRefusal),
}

impl ServerChat {
    const LINE: u8 = 1;
    const QUICK: u8 = 2;
    const SERVER: u8 = 3;
    const REFUSED: u8 = 4;

    pub fn server(text: &str) -> Result<Self, DecodeError> {
        Ok(Self::Server(chat_text(text)?.to_string()))
    }

    pub fn encode(&self) -> Vec<u8> {
        match self {
            Self::Line { rider, text } => framed(Self::LINE, &[&rider.to_be_bytes()[..], text.as_bytes()].concat()),
            Self::Quick { rider, index } => {
                let [r0, r1] = rider.to_be_bytes();
                framed(Self::QUICK, &[r0, r1, *index])
            }
            Self::Server(text) => framed(Self::SERVER, text.as_bytes()),
            Self::Refused(r) => {
                let (reason, wait) = match r {
                    ChatRefusal::Flood { wait_ms } => (ChatRefusal::FLOOD, *wait_ms),
                    ChatRefusal::Disabled => (ChatRefusal::DISABLED, 0),
                    ChatRefusal::Muted { .. } => (ChatRefusal::MUTED, 0),
                };
                let [w0, w1] = wait.to_be_bytes();
                let mut payload = vec![reason, w0, w1];
                if let ChatRefusal::Muted { until, reason } = r {
                    payload.extend_from_slice(&until.to_wire().to_be_bytes());
                    payload.extend_from_slice(reason.as_bytes());
                }
                framed(Self::REFUSED, &payload)
            }
        }
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, DecodeError> {
        let (kind, payload) = frame(bytes, MAX_SERVER_CHAT_SIZE, "server chat message")?;
        match (kind, payload) {
            (Self::LINE, [r0, r1, text @ ..]) => {
                Ok(Self::Line { rider: u16::from_be_bytes([*r0, *r1]), text: text_of(text)? })
            }
            (Self::QUICK, &[r0, r1, i]) => Ok(Self::Quick { rider: u16::from_be_bytes([r0, r1]), index: quick(i)? }),
            (Self::SERVER, text) => Ok(Self::Server(text_of(text)?)),
            (Self::REFUSED, &[ChatRefusal::FLOOD, w0, w1]) => {
                let wait_ms = u16::from_be_bytes([w0, w1]);
                if wait_ms == 0 || wait_ms > ChatRefusal::MAX_WAIT_MS {
                    return Err(DecodeError::Field("server chat message: a flood's wait is 1 to 60 000 ms"));
                }
                Ok(Self::Refused(ChatRefusal::Flood { wait_ms }))
            }
            (Self::REFUSED, &[ChatRefusal::DISABLED, 0, 0]) => Ok(Self::Refused(ChatRefusal::Disabled)),
            (Self::REFUSED, [ChatRefusal::MUTED, 0, 0, rest @ ..]) => match rest.split_first_chunk::<MUTE_END>() {
                Some((end, reason)) => {
                    let reason = if reason.is_empty() { String::new() } else { text_of(reason)? };
                    let until = crate::operator::Expiry::from_wire(u64::from_be_bytes(*end));
                    Ok(Self::Refused(ChatRefusal::Muted { until, reason }))
                }
                None => Err(DecodeError::Field("server chat message: a mute's refusal without the mute's end")),
            },
            _ => Err(DecodeError::Field("server chat message: unknown kind or a payload it does not have")),
        }
    }
}

#[cfg(test)]
mod tests;
