use crate::DecodeError;

// PORT(choice): 64 bytes; the disc's 262 named `ui_drop`s (attribdb.vlt, field 0xAF27A974) are 17…48 characters, all of
// a-z, 0-9 and _ (measured 2026-10-23); tuning: the drop names of the maps a server may run (the maps phase); the
// original has no dedicated server.
pub const MAX_DROP_NAME: usize = 64;

pub fn drop_name(name: &str) -> Result<&str, DecodeError> {
    if name.is_empty() || name.len() > MAX_DROP_NAME {
        return Err(DecodeError::Text { kind: "drop", detail: format!("{} bytes, 1 to {MAX_DROP_NAME}", name.len()) });
    }
    if let Some(c) = name.chars().find(|c| !matches!(c, 'a'..='z' | '0'..='9' | '_')) {
        return Err(DecodeError::Text { kind: "drop", detail: format!("{c:?} is not a-z, 0-9 or _") });
    }
    Ok(name)
}

pub const MAX_MAP_ID: usize = 256;

pub fn map_id(id: &str) -> Result<&str, DecodeError> {
    content_id(id, "map")
}

pub fn gamemode_id(id: &str) -> Result<&str, DecodeError> {
    content_id(id, "gamemode")
}

fn content_id<'a>(id: &'a str, kind: &'static str) -> Result<&'a str, DecodeError> {
    let refused = |detail: String| DecodeError::Text { kind, detail };
    if id.is_empty() || id.len() > MAX_MAP_ID {
        return Err(refused(format!("{} bytes, 1 to {MAX_MAP_ID}", id.len())));
    }
    let segment = |s: &str| {
        !s.is_empty()
            && s != "."
            && s != ".."
            && s.bytes().all(|b| matches!(b, b'a'..=b'z' | b'0'..=b'9' | b'_' | b'.' | b'-'))
    };
    let Some((namespace, rest)) = id.split_once(':') else {
        return Err(refused(format!("no `<namespace>:` before `{kind}/`")));
    };
    let Some(path) = rest.strip_prefix(kind).and_then(|r| r.strip_prefix('/')) else {
        return Err(refused(format!("not a {kind}'s id (`<namespace>:{kind}/<path>`)")));
    };
    if !segment(namespace) {
        return Err(refused("the namespace is not 1 or more of a-z, 0-9, _, . and - (not . or ..)".into()));
    }
    if !path.split('/').all(segment) {
        return Err(refused("a path segment is not 1 or more of a-z, 0-9, _, . and - (not . or ..)".into()));
    }
    Ok(id)
}

pub const MAX_EVENT_TYPE: u8 = 8;

pub const BUILT_IN_MODES: [(&str, u8); 5] = [("freeride", 8), ("race", 5), ("trick", 0), ("survive", 3), ("death", 4)];

pub fn built_in_mode(event_type: u8) -> Option<&'static str> {
    BUILT_IN_MODES.iter().find(|m| m.1 == event_type).map(|m| m.0)
}

pub fn built_in_event_type(name: &str) -> Option<u8> {
    BUILT_IN_MODES.iter().find(|m| m.0 == name).map(|m| m.1)
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Level {
    Drop(String),
    Map { id: String, event_type: u8, gamemode: Option<String> },
}

impl Level {
    pub fn drop(name: &str) -> Result<Self, DecodeError> {
        Ok(Self::Drop(drop_name(name)?.to_string()))
    }

    pub fn map(id: &str, event_type: u8) -> Result<Self, DecodeError> {
        Self::map_under(id, event_type, None)
    }

    pub fn map_under(id: &str, event_type: u8, gamemode: Option<&str>) -> Result<Self, DecodeError> {
        if event_type > MAX_EVENT_TYPE {
            return Err(DecodeError::Field("instance: a map's event type is 0 to 8"));
        }
        let gamemode = gamemode.map(|g| gamemode_id(g).map(str::to_string)).transpose()?;
        Ok(Self::Map { id: map_id(id)?.to_string(), event_type, gamemode })
    }

    pub fn name(&self) -> &str {
        match self {
            Self::Drop(d) => d,
            Self::Map { id, .. } => id,
        }
    }

    pub fn as_drop(&self) -> Option<&str> {
        match self {
            Self::Drop(d) => Some(d),
            Self::Map { .. } => None,
        }
    }

    pub fn is_map(&self) -> bool {
        matches!(self, Self::Map { .. })
    }

    pub fn fits(&self, mode: Mode) -> bool {
        match self {
            Self::Drop(_) => true,
            Self::Map { event_type, .. } => match mode {
                Mode::Freeride => *event_type == 8,
                Mode::Race => (5..=7).contains(event_type),
                Mode::Trick => (0..=2).contains(event_type),
                Mode::Survive => (3..=4).contains(event_type),
            },
        }
    }
}

impl core::fmt::Display for Level {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Drop(d) => f.write_str(d),
            Self::Map { id, gamemode: None, .. } => write!(f, "map {id}"),
            Self::Map { id, gamemode: Some(g), .. } => write!(f, "map {id} ({g})"),
        }
    }
}

pub const MAX_MAP_TITLE_CHARS: usize = 64;

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MapSummary {
    pub id: String,
    pub title: String,
    pub event_type: u8,
    #[serde(deserialize_with = "crate::info::present")]
    pub gamemode: Option<GamemodeSummary>,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GamemodeSummary {
    pub id: String,
    pub title: String,
}

fn title_rule(what: &str, title: &str) -> Result<(), String> {
    let chars = title.chars().count();
    if !(1..=MAX_MAP_TITLE_CHARS).contains(&chars) {
        return Err(format!("the {what}'s title has {chars} characters, 1 to {MAX_MAP_TITLE_CHARS}"));
    }
    if title.chars().any(char::is_control) {
        return Err(format!("the {what}'s title has a control character"));
    }
    Ok(())
}

impl MapSummary {
    pub fn validate(&self) -> Result<(), String> {
        map_id(&self.id).map_err(|e| e.to_string())?;
        title_rule("map", &self.title)?;
        if self.event_type > MAX_EVENT_TYPE {
            return Err(format!("the map's event type {} is not 0 to {MAX_EVENT_TYPE}", self.event_type));
        }
        if let Some(g) = &self.gamemode {
            gamemode_id(&g.id).map_err(|e| e.to_string())?;
            title_rule("gamemode", &g.title)?;
        }
        Ok(())
    }

    pub fn check_level(drop: &str, map: Option<&MapSummary>) -> Result<(), String> {
        match map {
            Some(m) if drop.is_empty() => m.validate(),
            Some(_) => Err("a drop beside a map".into()),
            None => drop_name(drop).map(|_| ()).map_err(|e| e.to_string()),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Mode {
    Freeride = 1,
    Race = 2,
    Trick = 3,
    Survive = 4,
}

impl Mode {
    pub const ALL: [Mode; 4] = [Mode::Freeride, Mode::Race, Mode::Trick, Mode::Survive];

    pub fn from_byte(b: u8) -> Option<Self> {
        Self::ALL.into_iter().find(|m| *m as u8 == b)
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Freeride => "freeride",
            Self::Race => "race",
            Self::Trick => "trick",
            Self::Survive => "survive",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|m| m.name() == s)
    }

    pub fn has_rounds(self) -> bool {
        self != Self::Freeride
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Rewinds {
    On,
    Off,
    Limit(u8),
}

// PORT(choice): 99 (the game's own `MaxRewindCount` is 3, `CharacterPropertiesAsset`); tuning: the server browser's
// rules column and what operators ask for (phase F2); the original has no dedicated server.
pub const MAX_REWIND_LIMIT: u8 = 99;

impl Rewinds {
    const ON: u8 = 0xFF;
    const OFF: u8 = 0;

    pub fn to_byte(self) -> u8 {
        match self {
            Self::On => Self::ON,
            Self::Off => Self::OFF,
            Self::Limit(n) => n,
        }
    }

    pub fn from_byte(b: u8) -> Option<Self> {
        match b {
            Self::ON => Some(Self::On),
            Self::OFF => Some(Self::Off),
            n if n <= MAX_REWIND_LIMIT => Some(Self::Limit(n)),
            _ => None,
        }
    }

    pub fn name(self) -> String {
        match self {
            Self::On => "on".into(),
            Self::Off => "off".into(),
            Self::Limit(n) => n.to_string(),
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "on" => Some(Self::On),
            "off" => Some(Self::Off),
            n if !n.is_empty() && n.len() <= 2 && n.bytes().all(|b| b.is_ascii_digit()) && !n.starts_with('0') => {
                n.parse::<u8>().ok().filter(|&n| (1..=MAX_REWIND_LIMIT).contains(&n)).map(Self::Limit)
            }
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum GhostStyle {
    Character = 0,
    Hologram = 1,
}

impl GhostStyle {
    pub fn from_byte(b: u8) -> Option<Self> {
        match b {
            0 => Some(Self::Character),
            1 => Some(Self::Hologram),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Character => "character",
            Self::Hologram => "hologram",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "character" => Some(Self::Character),
            "hologram" => Some(Self::Hologram),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct InstanceSpec {
    pub level: Level,
    pub mode: Mode,
    pub rewinds: Rewinds,
    pub ghosts: bool,
    pub ghost_style: GhostStyle,
}

const LEVEL_DROP: u8 = 1;
const LEVEL_MAP: u8 = 2;

pub const MAX_SPEC_SIZE: usize = 4 + 1 + 1 + 2 + MAX_MAP_ID + 2 + MAX_MAP_ID;
const _: () = assert!(4 + 1 + 1 + MAX_DROP_NAME <= MAX_SPEC_SIZE);

impl InstanceSpec {
    pub fn new(
        drop: &str,
        mode: Mode,
        rewinds: Rewinds,
        ghosts: bool,
        ghost_style: GhostStyle,
    ) -> Result<Self, DecodeError> {
        Self::on_level(Level::drop(drop)?, mode, rewinds, ghosts, ghost_style)
    }

    pub fn on_level(
        level: Level,
        mode: Mode,
        rewinds: Rewinds,
        ghosts: bool,
        ghost_style: GhostStyle,
    ) -> Result<Self, DecodeError> {
        if let Rewinds::Limit(n) = rewinds
            && !(1..=MAX_REWIND_LIMIT).contains(&n)
        {
            return Err(DecodeError::Field("instance: a rewind limit is 1 to 99 (0 is \"off\")"));
        }
        if !level.fits(mode) {
            return Err(DecodeError::Field("instance: the map's event type is not of the mode's kind"));
        }
        Ok(Self { level, mode, rewinds, ghosts, ghost_style })
    }

    pub fn drop(&self) -> Option<&str> {
        self.level.as_drop()
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(MAX_SPEC_SIZE);
        out.extend_from_slice(&[
            self.mode as u8,
            self.rewinds.to_byte(),
            u8::from(self.ghosts),
            self.ghost_style as u8,
        ]);
        match &self.level {
            Level::Drop(d) => {
                out.extend_from_slice(&[LEVEL_DROP, d.len() as u8]);
                out.extend_from_slice(d.as_bytes());
            }
            Level::Map { id, event_type, gamemode } => {
                out.extend_from_slice(&[LEVEL_MAP, *event_type]);
                out.extend_from_slice(&(id.len() as u16).to_be_bytes());
                out.extend_from_slice(id.as_bytes());
                let g = gamemode.as_deref().unwrap_or("");
                out.extend_from_slice(&(g.len() as u16).to_be_bytes());
                out.extend_from_slice(g.as_bytes());
            }
        }
        out
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, DecodeError> {
        let (spec, rest) = Self::decode_prefix(bytes)?;
        if !rest.is_empty() {
            return Err(DecodeError::Field("instance: bytes after the spec"));
        }
        Ok(spec)
    }

    pub fn decode_prefix(bytes: &[u8]) -> Result<(Self, &[u8]), DecodeError> {
        let Some((head, rest)) = bytes.split_first_chunk::<5>() else {
            return Err(DecodeError::Size { kind: "instance", expected: 5, actual: bytes.len() });
        };
        let mode = Mode::from_byte(head[0]).ok_or(DecodeError::Field("instance: unknown mode"))?;
        let rewinds = Rewinds::from_byte(head[1]).ok_or(DecodeError::Field("instance: rewinds out of range"))?;
        let ghosts = match head[2] {
            0 => false,
            1 => true,
            _ => return Err(DecodeError::Field("instance: ghosts is not 0 or 1")),
        };
        let ghost_style = GhostStyle::from_byte(head[3]).ok_or(DecodeError::Field("instance: unknown ghost style"))?;
        let (level, rest) = match head[4] {
            LEVEL_DROP => {
                let (&len, rest) =
                    rest.split_first().ok_or(DecodeError::Size { kind: "instance drop", expected: 1, actual: 0 })?;
                let (name, rest) = text_prefix(rest, usize::from(len), "instance drop")?;
                (Level::drop(name)?, rest)
            }
            LEVEL_MAP => {
                let Some(([event_type, l0, l1], rest)) = rest.split_first_chunk::<3>().map(|(h, r)| (*h, r)) else {
                    return Err(DecodeError::Size { kind: "instance map", expected: 3, actual: rest.len() });
                };
                let (id, rest) = text_prefix(rest, usize::from(u16::from_be_bytes([l0, l1])), "instance map")?;
                let Some(([g0, g1], rest)) = rest.split_first_chunk::<2>().map(|(h, r)| (*h, r)) else {
                    return Err(DecodeError::Size { kind: "instance gamemode", expected: 2, actual: rest.len() });
                };
                let (g, rest) = text_prefix(rest, usize::from(u16::from_be_bytes([g0, g1])), "instance gamemode")?;
                (Level::map_under(id, event_type, (!g.is_empty()).then_some(g))?, rest)
            }
            _ => return Err(DecodeError::Field("instance: the level is not a drop (1) or a map (2)")),
        };
        Ok((Self::on_level(level, mode, rewinds, ghosts, ghost_style)?, rest))
    }
}

fn text_prefix<'a>(bytes: &'a [u8], len: usize, kind: &'static str) -> Result<(&'a str, &'a [u8]), DecodeError> {
    if bytes.len() < len {
        return Err(DecodeError::Size { kind, expected: len, actual: bytes.len() });
    }
    let (text, rest) = bytes.split_at(len);
    let text = core::str::from_utf8(text).map_err(|_| DecodeError::Text { kind, detail: "not UTF-8".into() })?;
    Ok((text, rest))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum InstanceState {
    Lobby = 1,
    Loading = 2,
    Countdown = 3,
    Running = 4,
    Results = 5,
}

impl InstanceState {
    pub const ALL: [InstanceState; 5] = [
        InstanceState::Lobby,
        InstanceState::Loading,
        InstanceState::Countdown,
        InstanceState::Running,
        InstanceState::Results,
    ];

    pub fn from_byte(b: u8) -> Option<Self> {
        Self::ALL.into_iter().find(|s| *s as u8 == b)
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Lobby => "lobby",
            Self::Loading => "loading",
            Self::Countdown => "countdown",
            Self::Running => "running",
            Self::Results => "results",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PlayerStatus {
    Joining = 1,
    Loading = 2,
    Intro = 3,
    Riding = 4,
    Away = 5,
    Spectating = 6,
    Finished = 7,
}

impl PlayerStatus {
    pub fn from_byte(b: u8) -> Option<Self> {
        Some(match b {
            1 => Self::Joining,
            2 => Self::Loading,
            3 => Self::Intro,
            4 => Self::Riding,
            5 => Self::Away,
            6 => Self::Spectating,
            7 => Self::Finished,
            _ => return None,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Appearance {
    pub character: u32,
    pub gear: u16,
}

impl Appearance {
    pub const HEAD_LAMP: u16 = 1;
    pub const ICE_AXE: u16 = 2;
    pub const GEAR: u16 = Self::HEAD_LAMP | Self::ICE_AXE;
    pub const SIZE: usize = 6;

    pub fn new(character: u32, gear: u16) -> Result<Self, DecodeError> {
        if gear & !Self::GEAR != 0 {
            return Err(DecodeError::Field("appearance: undefined gear bits"));
        }
        Ok(Self { character, gear })
    }

    pub fn encode(&self) -> [u8; Self::SIZE] {
        let mut out = [0; Self::SIZE];
        out[..4].copy_from_slice(&self.character.to_be_bytes());
        out[4..].copy_from_slice(&self.gear.to_be_bytes());
        out
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, DecodeError> {
        let b: &[u8; Self::SIZE] = bytes.try_into().map_err(|_| DecodeError::Size {
            kind: "appearance",
            expected: Self::SIZE,
            actual: bytes.len(),
        })?;
        Self::new(u32::from_be_bytes([b[0], b[1], b[2], b[3]]), u16::from_be_bytes([b[4], b[5]]))
    }
}

#[cfg(test)]
mod tests;
