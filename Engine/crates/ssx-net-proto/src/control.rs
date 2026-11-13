use crate::DecodeError;
use crate::round::{ProgressReport, REPORT_SIZE};

// PORT(choice): 200 bytes, a few lines of the game's popup text; tuning: the notice and refusal popups' text fields
// (phase F2), their width and line count at the fonts' widest glyphs; the original has no dedicated server.
pub const MAX_CONTROL_TEXT: usize = 200;

pub const MAX_CONTROL_SIZE: usize = 4 + BAN_END + MAX_CONTROL_TEXT;

const BAN_END: usize = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum GoodbyeReason {
    Kicked = 1,
    Replaced = 2,
    ServerFull = 3,
    ServerStopping = 4,
    Ticket = 5,
    ChatFlood = 6,
    Validation = 7,
    Banned = 8,
    ScriptFlood = 20,
    ScriptBacklog = 21,
}

impl GoodbyeReason {
    fn from_byte(b: u8) -> Option<Self> {
        Some(match b {
            1 => Self::Kicked,
            2 => Self::Replaced,
            3 => Self::ServerFull,
            4 => Self::ServerStopping,
            5 => Self::Ticket,
            6 => Self::ChatFlood,
            7 => Self::Validation,
            8 => Self::Banned,
            20 => Self::ScriptFlood,
            21 => Self::ScriptBacklog,
            _ => return None,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ServerControl {
    Goodbye { reason: GoodbyeReason, text: String, ban: Option<crate::operator::Expiry> },
    Welcome { rider: u16, instance: u16, chat: bool },
    StartAt { round: u16, tick: u32 },
    Invalidated { round: u16, check: crate::round::Check },
    ResourcesChanged { manifest: [u8; 32] },
}

impl ServerControl {
    const GOODBYE: u8 = 2;
    const WELCOME: u8 = 3;
    const START_AT: u8 = 4;
    const INVALIDATED: u8 = 5;
    const RESOURCES_CHANGED: u8 = 6;
    const WELCOME_CHAT: u8 = 1;

    pub fn goodbye(reason: GoodbyeReason, text: &str) -> Result<Self, DecodeError> {
        check_text(text)?;
        if reason == GoodbyeReason::Banned {
            return Err(DecodeError::Field("control message: a goodbye Banned carries the ban's end"));
        }
        Ok(Self::Goodbye { reason, text: text.to_string(), ban: None })
    }

    pub fn banned(text: &str, until: crate::operator::Expiry) -> Result<Self, DecodeError> {
        check_text(text)?;
        Ok(Self::Goodbye { reason: GoodbyeReason::Banned, text: text.to_string(), ban: Some(until) })
    }

    pub fn encode(&self) -> Vec<u8> {
        let (kind, reason, payload) = match self {
            Self::Goodbye { reason, text, ban } => {
                let end = ban.map(|b| b.to_wire().to_be_bytes().to_vec()).unwrap_or_default();
                (Self::GOODBYE, *reason as u8, [end.as_slice(), text.as_bytes()].concat())
            }
            Self::Welcome { rider, instance, chat } => {
                let flags = if *chat { Self::WELCOME_CHAT } else { 0 };
                (Self::WELCOME, 0, [&rider.to_be_bytes()[..], &instance.to_be_bytes(), &[flags]].concat())
            }
            Self::StartAt { round, tick } => {
                (Self::START_AT, 0, [&round.to_be_bytes()[..], &tick.to_be_bytes()].concat())
            }
            Self::Invalidated { round, check } => (Self::INVALIDATED, check.code(), round.to_be_bytes().to_vec()),
            Self::ResourcesChanged { manifest } => (Self::RESOURCES_CHANGED, 0, manifest.to_vec()),
        };
        let mut out = Vec::with_capacity(4 + payload.len());
        out.extend_from_slice(&[kind, reason]);
        out.extend_from_slice(&(payload.len() as u16).to_be_bytes());
        out.extend_from_slice(&payload);
        out
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, DecodeError> {
        if bytes.len() < 4 || bytes.len() > MAX_CONTROL_SIZE {
            return Err(DecodeError::Size { kind: "control message", expected: MAX_CONTROL_SIZE, actual: bytes.len() });
        }
        let len = usize::from(u16::from_be_bytes([bytes[2], bytes[3]]));
        if len != bytes.len() - 4 {
            return Err(DecodeError::Field("control message: the text length does not match"));
        }
        if bytes[0] == Self::WELCOME {
            return match (bytes[1], &bytes[4..]) {
                (0, &[r0, r1, i0, i1, flags]) if flags & !Self::WELCOME_CHAT == 0 => Ok(Self::Welcome {
                    rider: u16::from_be_bytes([r0, r1]),
                    instance: u16::from_be_bytes([i0, i1]),
                    chat: flags & Self::WELCOME_CHAT != 0,
                }),
                _ => Err(DecodeError::Field("control message: a welcome is a rider, an instance and its flags")),
            };
        }
        if bytes[0] == Self::START_AT {
            return match (bytes[1], &bytes[4..]) {
                (0, &[r0, r1, t0, t1, t2, t3]) => Ok(Self::StartAt {
                    round: u16::from_be_bytes([r0, r1]),
                    tick: u32::from_be_bytes([t0, t1, t2, t3]),
                }),
                _ => Err(DecodeError::Field("control message: a start is a round and a tick")),
            };
        }
        if bytes[0] == Self::INVALIDATED {
            return match (crate::round::Check::from_code(bytes[1]), &bytes[4..]) {
                (Some(check), &[r0, r1]) => Ok(Self::Invalidated { round: u16::from_be_bytes([r0, r1]), check }),
                _ => Err(DecodeError::Field("control message: an invalidation is a known check and a round")),
            };
        }
        if bytes[0] == Self::RESOURCES_CHANGED {
            return match (bytes[1], <[u8; 32]>::try_from(&bytes[4..])) {
                (0, Ok(manifest)) => Ok(Self::ResourcesChanged { manifest }),
                _ => Err(DecodeError::Field("control message: a change of resources is the manifest's hash")),
            };
        }
        if bytes[0] != Self::GOODBYE {
            return Err(DecodeError::Field("control message: unknown kind"));
        }
        let reason = GoodbyeReason::from_byte(bytes[1]).ok_or(DecodeError::Field("control message: unknown reason"))?;
        let (ban, text) = match reason {
            GoodbyeReason::Banned => match bytes[4..].split_first_chunk::<BAN_END>() {
                Some((end, text)) => (Some(crate::operator::Expiry::from_wire(u64::from_be_bytes(*end))), text),
                None => return Err(DecodeError::Field("control message: a goodbye Banned without the ban's end")),
            },
            _ => (None, &bytes[4..]),
        };
        let text =
            core::str::from_utf8(text).map_err(|_| DecodeError::Field("control message: the text is not UTF-8"))?;
        check_text(text)?;
        Ok(Self::Goodbye { reason, text: text.to_string(), ban })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum ClientControl {
    Appearance(crate::instance::Appearance),
    Loaded { round: u16 },
    Ready { round: u16, sample: Box<crate::NetPose>, second: Box<crate::NetPose> },
    LevelLeft,
    StartAck { round: u16 },
    Report(Box<ProgressReport>),
}

impl ClientControl {
    const APPEARANCE: u8 = 1;
    const LOADED: u8 = 2;
    const READY: u8 = 3;
    const LEVEL_LEFT: u8 = 4;
    const START_ACK: u8 = 5;
    const REPORT: u8 = 6;
    const READY_SIZE: usize = 2 + 2 * crate::NET_POSE_SIZE;
    pub const MAX_SIZE: usize = 3 + if Self::READY_SIZE > REPORT_SIZE { Self::READY_SIZE } else { REPORT_SIZE };

    pub fn encode(&self) -> Vec<u8> {
        let (kind, payload): (u8, Vec<u8>) = match self {
            Self::Appearance(a) => (Self::APPEARANCE, a.encode().to_vec()),
            Self::Loaded { round } => (Self::LOADED, round.to_be_bytes().to_vec()),
            Self::Ready { round, sample, second } => {
                let mut p = round.to_be_bytes().to_vec();
                p.extend_from_slice(&sample.encode());
                p.extend_from_slice(&second.encode());
                (Self::READY, p)
            }
            Self::LevelLeft => (Self::LEVEL_LEFT, Vec::new()),
            Self::StartAck { round } => (Self::START_ACK, round.to_be_bytes().to_vec()),
            Self::Report(r) => (Self::REPORT, r.encode().to_vec()),
        };
        let mut out = Vec::with_capacity(3 + payload.len());
        out.push(kind);
        out.extend_from_slice(&(payload.len() as u16).to_be_bytes());
        out.extend_from_slice(&payload);
        out
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, DecodeError> {
        if bytes.len() > Self::MAX_SIZE {
            return Err(DecodeError::Size {
                kind: "client control message",
                expected: Self::MAX_SIZE,
                actual: bytes.len(),
            });
        }
        let Some((&[kind, l0, l1], payload)) = bytes.split_first_chunk::<3>() else {
            return Err(DecodeError::Size { kind: "client control message", expected: 3, actual: bytes.len() });
        };
        if usize::from(u16::from_be_bytes([l0, l1])) != payload.len() {
            return Err(DecodeError::Field("client control message: the length does not match"));
        }
        let round = || match payload {
            &[r0, r1] => Ok(u16::from_be_bytes([r0, r1])),
            _ => Err(DecodeError::Field("client control message: a round is 2 bytes")),
        };
        match kind {
            Self::APPEARANCE => Ok(Self::Appearance(crate::instance::Appearance::decode(payload)?)),
            Self::LOADED => Ok(Self::Loaded { round: round()? }),
            Self::READY => match payload.split_first_chunk::<2>() {
                Some((&r, samples)) if payload.len() == Self::READY_SIZE => {
                    let (sample, second) = samples.split_at(crate::NET_POSE_SIZE);
                    Ok(Self::Ready {
                        round: u16::from_be_bytes(r),
                        sample: Box::new(crate::NetPose::decode(sample)?),
                        second: Box::new(crate::NetPose::decode(second)?),
                    })
                }
                _ => Err(DecodeError::Field("client control message: `Ready` is a round and two samples")),
            },
            Self::LEVEL_LEFT if payload.is_empty() => Ok(Self::LevelLeft),
            Self::START_ACK => Ok(Self::StartAck { round: round()? }),
            Self::REPORT => Ok(Self::Report(Box::new(ProgressReport::decode(payload)?))),
            _ => Err(DecodeError::Field("client control message: unknown kind or a payload it does not have")),
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Self::Appearance(_) => "its look",
            Self::Loaded { .. } => "loaded",
            Self::Ready { .. } => "ready",
            Self::LevelLeft => "level left",
            Self::StartAck { .. } => "the start's acknowledgement",
            Self::Report(r) => r.kind.name(),
        }
    }
}

fn check_text(text: &str) -> Result<(), DecodeError> {
    if text.len() > MAX_CONTROL_TEXT {
        return Err(DecodeError::Text { kind: "control message", detail: format!("{} bytes of text", text.len()) });
    }
    if let Some(c) = text.chars().find(|c| c.is_control()) {
        return Err(DecodeError::Text { kind: "control message", detail: format!("control character {c:?}") });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hostile_bytes_never_panic_and_round_trips_hold() {
        let good = [
            ServerControl::goodbye(GoodbyeReason::Kicked, "").unwrap(),
            ServerControl::goodbye(GoodbyeReason::ChatFlood, "too many chat lines").unwrap(),
            ServerControl::Welcome { rider: 0x1234, instance: 7, chat: true },
            ServerControl::Welcome { rider: 0, instance: 1, chat: false },
            ServerControl::StartAt { round: 7, tick: 0xDEAD_BEEF },
            ServerControl::StartAt { round: u16::MAX, tick: 0 },
            ServerControl::goodbye(GoodbyeReason::Validation, "validation: a clock that does not match").unwrap(),
            ServerControl::Invalidated { round: 9, check: crate::Check::Clock },
            ServerControl::Invalidated { round: u16::MAX, check: crate::Check::Tricks },
            ServerControl::ResourcesChanged { manifest: [0xAB; 32] },
            ServerControl::ResourcesChanged { manifest: [0; 32] },
            ServerControl::banned("griefing", crate::operator::Expiry::from_wire(1_794_076_500)).unwrap(),
            ServerControl::banned(&"b".repeat(MAX_CONTROL_TEXT), crate::operator::Expiry::Never).unwrap(),
        ];
        for m in &good {
            assert_eq!(ServerControl::decode(&m.encode()).as_ref(), Ok(m));
        }
        let mut x = 0x2545_F491_4F6C_DD1Du64;
        for len in 0..300usize {
            for _ in 0..200 {
                let bytes: Vec<u8> = (0..len)
                    .map(|_| {
                        x ^= x << 13;
                        x ^= x >> 7;
                        x ^= x << 17;
                        x as u8
                    })
                    .collect();
                if let Ok(m) = ServerControl::decode(&bytes) {
                    assert_eq!(m.encode(), bytes);
                }
            }
        }
        for bad in [
            &[1u8, 0, 0, 0][..],
            &[1, 0, 0, 1, b'a'],
            &[2, 0, 0, 0],
            &[2, 9, 0, 0],
            &[2, 8, 0, 0],
            &[2, 8, 0, 7, 0, 0, 0, 0, 0, 0, 0],
            &[2, 8, 0, 9, 0, 0, 0, 0, 0, 0, 0, 0, 0x0A],
            &[2, 1, 0, 1, 0x0A],
            &[3, 0, 0, 0],
            &[2, 1, 0, 2, b'a'],
            &[3, 1, 0, 5, 0, 1, 0, 1, 0],
            &[3, 0, 0, 4, 0, 1, 0, 1],
            &[3, 0, 0, 5, 0, 1, 0, 1, 2],
            &[3, 0, 0, 5, 0, 1, 0, 1, 0x81],
            &[3, 0, 0, 6, 0, 1, 0, 1, 1, 0],
            &[4, 1, 0, 6, 0, 1, 0, 0, 0, 1],
            &[4, 0, 0, 5, 0, 1, 0, 0, 0],
            &[4, 0, 0, 7, 0, 1, 0, 0, 0, 1, 0],
            &[5, 0, 0, 0],
            &[5, 0, 0, 2, 0, 1],
            &[5, 7, 0, 2, 0, 1],
            &[5, 1, 0, 1, 0],
            &[5, 1, 0, 3, 0, 1, 0],
            &[6, 0, 0, 0],
        ] {
            assert!(ServerControl::decode(bad).is_err(), "{bad:?}");
        }
        assert!(ServerControl::goodbye(GoodbyeReason::Kicked, &"a".repeat(MAX_CONTROL_TEXT + 1)).is_err());
        let changed = |reason: u8, len: usize| {
            let mut b = vec![6u8, reason];
            b.extend_from_slice(&(len as u16).to_be_bytes());
            b.extend(std::iter::repeat_n(0xAB, len));
            ServerControl::decode(&b)
        };
        assert!(changed(0, 32).is_ok());
        for (reason, len) in [(1, 32), (0, 31), (0, 33), (0, 0)] {
            assert!(changed(reason, len).is_err(), "{reason} {len}");
        }
        assert!(ServerControl::goodbye(GoodbyeReason::Banned, "x").is_err());
        assert_eq!(
            ServerControl::decode(&[2, 8, 0, 8, 0, 0, 0, 0, 0, 0, 0, 0]),
            Ok(ServerControl::Goodbye {
                reason: GoodbyeReason::Banned,
                text: String::new(),
                ban: Some(crate::operator::Expiry::Never)
            })
        );
    }

    fn sample() -> crate::NetPose {
        let mut b = [0u8; ssx_formats::ghost::POSE_BLOCK_SIZE];
        b[12..16].copy_from_slice(&1.0f32.to_be_bytes());
        b[0x48..0x4C].copy_from_slice(&1.0f32.to_be_bytes());
        let block = ssx_formats::ghost::PoseBlock::parse(&b).expect("240 bytes parse");
        crate::NetPose::new(900, crate::PoseFlags::default(), block).expect("a sample")
    }

    #[test]
    fn client_control_is_strict() {
        let a = crate::instance::Appearance::new(0x405D_C1D5, 3).unwrap();
        let report = ProgressReport {
            instance: 1,
            round: 3,
            kind: crate::round::ReportKind::Finish,
            split: None,
            clock_ms: 75_000,
            points: 10,
            descent_mm: 1_000_000,
            rewinds: 2,
            loops: 0,
            sample: sample(),
        };
        let good = [
            ClientControl::Appearance(a),
            ClientControl::Loaded { round: 0 },
            ClientControl::Ready {
                round: 65_535,
                sample: Box::new(sample()),
                second: Box::new(sample().with_execution(2).expect("an execution")),
            },
            ClientControl::LevelLeft,
            ClientControl::StartAck { round: 9 },
            ClientControl::Report(Box::new(report.clone())),
        ];
        for m in &good {
            assert_eq!(ClientControl::decode(&m.encode()).as_ref(), Ok(m));
            assert!(m.encode().len() <= ClientControl::MAX_SIZE);
        }
        assert_eq!(good[2].encode().len(), ClientControl::MAX_SIZE);
        assert_eq!(ClientControl::Report(Box::new(report.clone())).encode().len(), 3 + REPORT_SIZE);
        let mut x = 0x9E37_79B9_7F4A_7C15u64;
        for len in 0..16usize {
            for _ in 0..20_000 {
                let bytes: Vec<u8> = (0..len)
                    .map(|_| {
                        x ^= x << 13;
                        x ^= x >> 7;
                        x ^= x << 17;
                        (x % 8) as u8
                    })
                    .collect();
                if let Ok(m) = ClientControl::decode(&bytes) {
                    assert_eq!(m.encode(), bytes);
                }
            }
        }
        for bad in [
            &[][..],
            &[2],
            &[2, 0],
            &[2, 0, 0],
            &[2, 0, 1, 0],
            &[3, 0, 3, 0, 0, 0],
            &[3, 0, 2, 0, 1],
            &[4, 0, 1, 0],
            &[5, 0, 0],
            &[7, 0, 0],
            &[1, 0, 6, 0, 0, 0, 1, 0, 4],
            &[1, 0, 5, 0, 0, 0, 1, 0],
            &[1, 0, 7, 0, 0, 0, 1, 0, 1, 0],
            &[6, 0, 0],
            &[6, 0, 2, 0, 0],
        ] {
            assert!(ClientControl::decode(bad).is_err(), "{bad:?}");
        }
        let mut one = 7u16.to_be_bytes().to_vec();
        one.extend_from_slice(&sample().encode());
        let mut seven = vec![3, 0, 0];
        seven[1..3].copy_from_slice(&(one.len() as u16).to_be_bytes());
        seven.extend_from_slice(&one);
        assert!(ClientControl::decode(&seven).is_err());
        let ready = ClientControl::Ready { round: 7, sample: Box::new(sample()), second: Box::new(sample()) }.encode();
        assert_eq!(ready.len(), 3 + 2 + 2 * crate::NET_POSE_SIZE);
        let mut short = ready.clone();
        short.pop();
        let len = (short.len() - 3) as u16;
        short[1..3].copy_from_slice(&len.to_be_bytes());
        assert!(ClientControl::decode(&short).is_err());
        let mut long = ready;
        long.push(0);
        let len = (long.len() - 3) as u16;
        long[1..3].copy_from_slice(&len.to_be_bytes());
        assert!(ClientControl::decode(&long).is_err());
        let bytes = ClientControl::Report(Box::new(report)).encode();
        let mut short = bytes.clone();
        short.pop();
        assert!(ClientControl::decode(&short).is_err());
        let mut long = bytes.clone();
        long.push(0);
        assert!(ClientControl::decode(&long).is_err());
        let mut lying = bytes;
        lying[1..3].copy_from_slice(&u16::MAX.to_be_bytes());
        assert!(ClientControl::decode(&lying).is_err());
        assert!(ClientControl::decode(&vec![6u8; ClientControl::MAX_SIZE + 1]).is_err());
    }
}
