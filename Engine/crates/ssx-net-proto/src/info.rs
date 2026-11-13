use serde::{Deserialize, Serialize};

use crate::instance::MapSummary;
use crate::join::MAX_BUILD_BYTES;
use crate::keys::ServerKey;
use crate::name::server_name;
use crate::resources::ResourceSummary;

pub const INFO_VERSION: u32 = 6;

pub const MAX_INFO_BYTES: usize = 4096;

pub const MAX_ANNOUNCED_PLAYERS: u32 = 64;

// PORT(choice): 250 characters, the plan's cap (FiveM's for a server's description); tuning: the server browser's
// details pane measured with the longest descriptions operators write; the original has no server browser.
pub const MAX_DESCRIPTION_CHARS: usize = 250;

pub fn description(text: &str) -> Result<&str, String> {
    let chars = text.chars().count();
    if chars > MAX_DESCRIPTION_CHARS {
        return Err(format!("the description has {chars} characters, at most {MAX_DESCRIPTION_CHARS}"));
    }
    if let Some(c) = text.chars().find(|c| c.is_control()) {
        return Err(format!("the description has the control character {c:?}"));
    }
    Ok(text)
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServerInfo {
    pub info_version: u32,
    pub name: String,
    pub description: String,
    pub protocol_version: u32,
    pub api_level: u32,
    pub build: String,
    pub players: u32,
    pub max_players: u32,
    pub password: bool,
    pub mode: String,
    pub drop: String,
    #[serde(deserialize_with = "present")]
    pub map: Option<MapSummary>,
    pub rewinds: String,
    pub ghosts: bool,
    pub ghost_style: String,
    pub identity_key: String,
    pub resources: ResourceSummary,
    pub region: String,
    pub tags: Vec<String>,
    pub icon: String,
}

pub fn icon_hash(text: &str) -> Result<Option<[u8; 32]>, String> {
    if text.is_empty() {
        return Ok(None);
    }
    if text.len() != 64 || !text.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
        return Err("the icon's hash is not empty or 64 lowercase hex digits".into());
    }
    let mut out = [0u8; 32];
    let digit = |b: u8| if b <= b'9' { b - b'0' } else { b - b'a' + 10 };
    for (o, [hi, lo]) in out.iter_mut().zip(text.as_bytes().as_chunks::<2>().0) {
        *o = digit(*hi) << 4 | digit(*lo);
    }
    Ok(Some(out))
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct InfoHead {
    pub info_version: u32,
    pub protocol_version: u32,
    #[serde(default, deserialize_with = "lenient_name")]
    pub name: Option<String>,
}

pub(crate) fn present<'de, D: serde::Deserializer<'de>, T: Deserialize<'de>>(d: D) -> Result<Option<T>, D::Error> {
    Option::<T>::deserialize(d)
}

fn lenient_name<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    let raw = Option::<String>::deserialize(d)?;
    Ok(raw.filter(|n| server_name(n).is_ok_and(|checked| checked == *n)))
}

impl InfoHead {
    pub fn from_json(body: &[u8]) -> Result<Self, String> {
        if body.len() > MAX_INFO_BYTES {
            return Err(format!("{} bytes of server info, at most {MAX_INFO_BYTES}", body.len()));
        }
        serde_json::from_slice(body).map_err(|e| format!("the server info does not read: {e}"))
    }
}

impl ServerInfo {
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }

    pub fn from_json(body: &[u8]) -> Result<Self, String> {
        let head = InfoHead::from_json(body)?;
        if head.info_version != INFO_VERSION {
            return Err(format!("the server info is version {}, this client reads {INFO_VERSION}", head.info_version));
        }
        let info: Self = serde_json::from_slice(body).map_err(|e| format!("the server info does not read: {e}"))?;
        info.validate()?;
        Ok(info)
    }

    pub fn validate(&self) -> Result<(), String> {
        match server_name(&self.name) {
            Ok(name) if name == self.name => {}
            Ok(_) => return Err("the server name has spaces the name rule removes".into()),
            Err(e) => return Err(format!("the server name: {e}")),
        }
        description(&self.description)?;
        if self.build.is_empty()
            || self.build.len() > MAX_BUILD_BYTES
            || !self.build.bytes().all(|b| (0x20..=0x7E).contains(&b))
        {
            return Err("the build id is not 1 to 64 printable ASCII characters".into());
        }
        if self.max_players == 0 || self.max_players > MAX_ANNOUNCED_PLAYERS || self.players > self.max_players {
            return Err(format!("{} of {} players", self.players, self.max_players));
        }
        ServerKey::from_hex(&self.identity_key).map_err(|e| format!("the identity key: {e}"))?;
        if crate::instance::Mode::parse(&self.mode).is_none() {
            return Err("the mode is not one this client knows".into());
        }
        MapSummary::check_level(&self.drop, self.map.as_ref())?;
        if crate::instance::Rewinds::parse(&self.rewinds).is_none() {
            return Err(format!("the rewinds {:?} are not on, off or 1 to 99", self.rewinds));
        }
        if crate::instance::GhostStyle::parse(&self.ghost_style).is_none() {
            return Err(format!("the riders' look {:?} is not character or hologram", self.ghost_style));
        }
        self.resources.validate().map_err(|e| format!("the resources: {e}"))?;
        crate::master::region(&self.region)?;
        crate::master::tags(&self.tags)?;
        icon_hash(&self.icon)?;
        Ok(())
    }

    pub fn key(&self) -> Result<ServerKey, String> {
        ServerKey::from_hex(&self.identity_key).map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests;
