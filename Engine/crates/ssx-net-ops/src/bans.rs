use std::fmt;
use std::net::IpAddr;
use std::path::Path;

use ssx_net_proto::operator::Expiry;

use crate::range::{Range, canonical};
use crate::rules::FileError;

mod file;

// PORT(choice): 65 536 of each, far above MTA's ban lists of public servers as recalled; tuning: the ban lists of
// long-running public servers; the original has no dedicated server.
pub const MAX_ENTRIES: usize = 65_536;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Target {
    Player([u8; 32]),
    Address(Range),
}

impl Target {
    pub fn holds(&self, id: Option<&[u8; 32]>, ip: Option<IpAddr>) -> bool {
        match self {
            Self::Player(p) => id == Some(p),
            Self::Address(r) => ip.is_some_and(|ip| r.contains(canonical(ip))),
        }
    }

    pub fn parse(text: &str) -> Result<Self, String> {
        if let Some(id) = text.strip_prefix("player:") {
            return crate::rules::player_id(&id.to_ascii_lowercase()).map(Self::Player);
        }
        if let Some(ip) = text.strip_prefix("ip:") {
            let r = Range::parse(ip)?;
            return if r.is_address() { Ok(Self::Address(r)) } else { Err(format!("{r} is a range: write range:{r}")) };
        }
        if let Some(range) = text.strip_prefix("range:") {
            let r = Range::parse(range)?;
            return if r.is_address() {
                Err(format!("{r} is one address: write ip:{r}"))
            } else {
                Ok(Self::Address(r))
            };
        }
        Err(format!("{} is not player:<id>, ip:<address> or range:<range>", ssx_core::text::quoted(text)))
    }
}

impl fmt::Display for Target {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Player(id) => write!(f, "player:{}", ssx_net_proto::keys::to_hex(id)),
            Self::Address(r) if r.is_address() => write!(f, "ip:{r}"),
            Self::Address(r) => write!(f, "range:{r}"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ban {
    pub target: Target,
    pub reason: String,
    pub by: String,
    pub at: u64,
    pub until: Expiry,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mute {
    pub player: [u8; 32],
    pub reason: String,
    pub by: String,
    pub at: u64,
    pub until: Expiry,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BanStore {
    bans: Vec<Ban>,
    mutes: Vec<Mute>,
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum StoreError {
    #[error("the store holds {MAX_ENTRIES} entries of this kind already")]
    Full,
    #[error("{0}")]
    Rule(String),
}

impl BanStore {
    pub fn bans(&self) -> &[Ban] {
        &self.bans
    }

    pub fn mutes(&self) -> &[Mute] {
        &self.mutes
    }

    pub fn ban(&mut self, ban: Ban) -> Result<Option<Ban>, StoreError> {
        crate::rules::reason(&ban.reason).map_err(StoreError::Rule)?;
        crate::rules::by(&ban.by).map_err(StoreError::Rule)?;
        times(ban.at, ban.until)?;
        if let Some(old) = self.bans.iter_mut().find(|b| b.target == ban.target) {
            return Ok(Some(std::mem::replace(old, ban)));
        }
        if self.bans.len() >= MAX_ENTRIES {
            return Err(StoreError::Full);
        }
        self.bans.push(ban);
        Ok(None)
    }

    pub fn unban(&mut self, target: &Target) -> Option<Ban> {
        let k = self.bans.iter().position(|b| b.target == *target)?;
        Some(self.bans.remove(k))
    }

    pub fn unban_at(&mut self, index: usize) -> Option<Ban> {
        (index < self.bans.len()).then(|| self.bans.remove(index))
    }

    pub fn banned(&self, id: Option<&[u8; 32]>, ip: Option<IpAddr>, now: u64) -> Option<&Ban> {
        self.bans.iter().filter(|b| !b.until.passed(now) && b.target.holds(id, ip)).max_by_key(|b| last(b.until))
    }

    pub fn mute(&mut self, mute: Mute) -> Result<Option<Mute>, StoreError> {
        crate::rules::reason(&mute.reason).map_err(StoreError::Rule)?;
        crate::rules::by(&mute.by).map_err(StoreError::Rule)?;
        times(mute.at, mute.until)?;
        if let Some(old) = self.mutes.iter_mut().find(|m| m.player == mute.player) {
            return Ok(Some(std::mem::replace(old, mute)));
        }
        if self.mutes.len() >= MAX_ENTRIES {
            return Err(StoreError::Full);
        }
        self.mutes.push(mute);
        Ok(None)
    }

    pub fn unmute(&mut self, player: &[u8; 32]) -> Option<Mute> {
        let k = self.mutes.iter().position(|m| m.player == *player)?;
        Some(self.mutes.remove(k))
    }

    pub fn muted(&self, player: &[u8; 32], now: u64) -> Option<&Mute> {
        self.mutes.iter().find(|m| m.player == *player && !m.until.passed(now))
    }

    pub fn expire(&mut self, now: u64) -> (Vec<Ban>, Vec<Mute>) {
        let (gone, kept): (Vec<Ban>, Vec<Ban>) = self.bans.drain(..).partition(|b| b.until.passed(now));
        self.bans = kept;
        let (unmuted, kept): (Vec<Mute>, Vec<Mute>) = self.mutes.drain(..).partition(|m| m.until.passed(now));
        self.mutes = kept;
        (gone, unmuted)
    }

    pub fn parse(text: &str) -> Result<Self, FileError> {
        file::parse(text)
    }

    pub fn to_toml(&self) -> String {
        file::write(self)
    }

    pub fn load(path: &Path) -> Result<Self, String> {
        let shown = path.display().to_string();
        match crate::rules::read_file(path) {
            Ok(None) => Ok(Self::default()),
            Ok(Some(text)) => Self::parse(&text).map_err(|e| ssx_core::text::located(&shown, e).to_string()),
            Err(e) => Err(ssx_core::text::located(&shown, e).to_string()),
        }
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        ssx_files::replace(path, self.to_toml().as_bytes(), &ssx_files::Options::new()).map_err(|e| e.to_string())
    }
}

fn times(at: u64, until: Expiry) -> Result<(), StoreError> {
    let max = i64::MAX as u64;
    match until {
        _ if at > max => Err(StoreError::Rule("a start beyond the file's times".into())),
        Expiry::At(t) if t.get() > max => Err(StoreError::Rule("an end beyond the file's times".into())),
        Expiry::At(t) if t.get() <= at => Err(StoreError::Rule("an end before its start".into())),
        _ => Ok(()),
    }
}

fn last(until: Expiry) -> u64 {
    match until {
        Expiry::Never => u64::MAX,
        Expiry::At(t) => t.get(),
    }
}

#[cfg(test)]
mod tests;
