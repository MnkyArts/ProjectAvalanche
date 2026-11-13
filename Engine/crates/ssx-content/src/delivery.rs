mod compare;
pub mod config;
mod files;
mod json;
pub mod mirror;
pub mod range;
pub mod store;
#[cfg(test)]
mod tests;

pub use compare::{Difference, FILE_CLASS};
pub use files::{FileRole, ResourceFiles, files_of};
pub use json::{ManifestError, from_json};
pub use mirror::Mirror;
pub use store::{FileStore, Mount, Partial, StoreError};

use crate::data::SimContentId;
use crate::hash::ContentHash;
use crate::id::{ContentId, Kind};
use crate::policy::ClientMods;
use crate::version::Version;

pub const FORMAT: u32 = 1;
// PORT(choice): 8 MiB, 256 resources and 65 536 files a server, 2 048 rows a level, 64 levels: port caps far above the
// measured packages (M8a's template map as a resource 5 files and 16 132 bytes, the alps0101 layout 5 files and 4 503
// bytes; their manifest 2 resources, 3 levels); tuning: the manifests of real servers; the original has no dedicated
// server.
pub const MAX_BODY: usize = 8 << 20;
pub const MAX_RESOURCES: usize = 256;
pub const MAX_FILES: usize = 65_536;
pub const MAX_ROWS: usize = 2048;
pub const MAX_LEVELS: usize = 64;
pub const MAX_NAME: usize = 64;
pub const MAX_TITLE: usize = 256;
pub const MAX_ROW_ID: usize = 512;
pub const MAX_CLASS: usize = 32;
// PORT(choice): 8 GiB a server, a port cap (M8b's level budget holds 512 MiB of models and 1 GiB of decoded images, so
// a map's files are far below one package's 2 GiB); tuning: the sizes real servers ship; the original has no dedicated
// server.
pub const MAX_SERVER_BYTES: u64 = 8 << 30;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClientManifest {
    pub client_mods: ClientMods,
    pub client_scripts: bool,
    pub mirrors: Vec<Mirror>,
    pub content_id: Option<SimContentId>,
    pub resources: Vec<Resource>,
    pub levels: Vec<LevelId>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Resource {
    pub name: String,
    pub version: Version,
    pub title: String,
    pub group: u32,
    pub scripts: bool,
    pub files: Vec<File>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct File {
    pub path: String,
    pub hash: ContentHash,
    pub size: u64,
    pub role: FileRole,
    pub lazy: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LevelId {
    pub level: String,
    pub id: SimContentId,
    pub files: ContentHash,
    pub rows: Vec<Row>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub id: String,
    pub class: String,
    pub hash: ContentHash,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LevelName<'a> {
    Drop(&'a str),
    Map { id: ContentId, mode: &'a str },
}

impl<'a> LevelName<'a> {
    pub fn parse(s: &'a str) -> Result<Self, String> {
        match s.split_once(':') {
            Some(("drop", d)) => {
                let ok = !d.is_empty()
                    && d.len() <= MAX_NAME
                    && d.bytes().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'_');
                if ok { Ok(Self::Drop(d)) } else { Err(format!("not a drop name (1 to {MAX_NAME} of a-z 0-9 _)")) }
            }
            Some(("map", rest)) => {
                let (id, mode) = rest.split_once('@').ok_or("a map's level names its mode (map:<map id>@<mode>)")?;
                let id = ContentId::parse(id).map_err(|e| format!("the map's id: {e}"))?;
                if id.kind() != Kind::Map {
                    return Err(format!("{id}: not a map id"));
                }
                if crate::map::Mode::parse(mode).is_none() {
                    let g = ContentId::parse(mode).map_err(|e| {
                        format!("the mode: not race, trick, survive, death or freeride, nor a gamemode id: {e}")
                    })?;
                    if g.kind() != Kind::Gamemode {
                        return Err(format!("{g}: not a gamemode id"));
                    }
                }
                Ok(Self::Map { id, mode })
            }
            _ => Err("not drop:<ui_drop name> or map:<map id>@<mode>".into()),
        }
    }

    pub fn drop_name(ui_drop: &str) -> String {
        format!("drop:{ui_drop}")
    }

    pub fn map_name(id: &str, mode: &str) -> String {
        format!("map:{id}@{mode}")
    }
}

pub const CONTENT_ID_DOMAIN: &[u8] = b"project-avalanche/server-content-id/1\n";

pub fn content_id(levels: &[(String, SimContentId)]) -> Option<SimContentId> {
    let mut sorted: Vec<&(String, SimContentId)> = levels.iter().collect();
    sorted.sort_by(|a, b| a.0.cmp(&b.0));
    if sorted.windows(2).any(|w| w[0].0 == w[1].0) {
        return None;
    }
    let mut h = blake3::Hasher::new();
    h.update(CONTENT_ID_DOMAIN);
    h.update(&(sorted.len() as u64).to_le_bytes());
    for (level, id) in sorted {
        h.update(&(level.len() as u64).to_le_bytes());
        h.update(level.as_bytes());
        h.update(&id.0);
    }
    Some(SimContentId(*h.finalize().as_bytes()))
}

pub fn files_hash(rows: &[(&str, ContentHash)]) -> ContentHash {
    let mut sorted: Vec<&(&str, ContentHash)> = rows.iter().collect();
    sorted.sort_by(|a, b| a.0.cmp(b.0));
    let mut h = blake3::Hasher::new();
    h.update(b"project-avalanche/level-files/1\n");
    h.update(&(sorted.len() as u64).to_le_bytes());
    for (path, hash) in sorted {
        h.update(&(path.len() as u64).to_le_bytes());
        h.update(path.as_bytes());
        h.update(&hash.0);
    }
    ContentHash(*h.finalize().as_bytes())
}

impl ClientManifest {
    pub fn file_count(&self) -> usize {
        self.resources.iter().map(|r| r.files.len()).sum()
    }

    pub fn required_bytes(&self) -> u64 {
        self.resources.iter().flat_map(|r| &r.files).filter(|f| !f.lazy).map(|f| f.size).sum()
    }

    pub fn level(&self, level: &str) -> Option<&LevelId> {
        self.levels.iter().find(|l| l.level == level)
    }
}
