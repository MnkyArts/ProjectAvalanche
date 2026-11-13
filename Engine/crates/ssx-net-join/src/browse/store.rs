use std::fmt;
use std::io::Read;
use std::path::{Path, PathBuf};

use serde::de::{self, Deserializer, SeqAccess, Visitor};
use serde::{Deserialize, Serialize, Serializer};
use ssx_net_proto::name::server_name;

use super::address::ServerAddress;
use super::files::{move_aside, write_whole};

pub const SERVERS_FILE: &str = "servers.json";
// PORT(choice): 256 KiB, 100 favourites, 30 recent (MTA keeps its recent list short and its favourites unbounded; a
// favourite here is about 90 bytes); tuning: the lists players keep once servers are public (phase 5); the original has
// no server browser.
pub const MAX_FILE: u64 = 256 * 1024;
pub const MAX_FAVOURITES: usize = 100;
pub const MAX_RECENT: usize = 30;
const FORMAT: u32 = 1;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Saved {
    #[serde(serialize_with = "address_out", deserialize_with = "address_in")]
    pub address: ServerAddress,
    #[serde(default, deserialize_with = "name_in")]
    pub name: Option<String>,
    pub when: u64,
}

fn address_out<S: Serializer>(a: &ServerAddress, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_str(&a.to_string())
}

fn address_in<'de, D: Deserializer<'de>>(d: D) -> Result<ServerAddress, D::Error> {
    let text = String::deserialize(d)?;
    ServerAddress::parse(&text, None).map_err(de::Error::custom)
}

fn name_in<'de, D: Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    match Option::<String>::deserialize(d)? {
        Some(n) => match server_name(&n) {
            Ok(checked) if checked == n => Ok(Some(n)),
            _ => Err(de::Error::custom(format!("the name {n:?} breaks the server name rule"))),
        },
        None => Ok(None),
    }
}

struct Capped<const MAX: usize>(Vec<Saved>);

impl<'de, const MAX: usize> Deserialize<'de> for Capped<MAX> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V<const MAX: usize>;
        impl<'de, const MAX: usize> Visitor<'de> for V<MAX> {
            type Value = Capped<MAX>;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                write!(f, "a list of at most {MAX} servers")
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
                let mut out: Vec<Saved> = Vec::new();
                while let Some(s) = seq.next_element::<Saved>()? {
                    if out.len() == MAX {
                        return Err(de::Error::custom(format!("more than {MAX} servers")));
                    }
                    if out.iter().any(|o| o.address == s.address) {
                        return Err(de::Error::custom(format!("{} is listed twice", s.address)));
                    }
                    out.push(s);
                }
                Ok(Capped(out))
            }
        }
        d.deserialize_seq(V::<MAX>)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawFile {
    format: u32,
    favourites: Capped<MAX_FAVOURITES>,
    recent: Capped<MAX_RECENT>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ServersFile {
    format: u32,
    pub favourites: Vec<Saved>,
    pub recent: Vec<Saved>,
}

pub struct Loaded {
    pub file: ServersFile,
    pub problem: Option<String>,
}

impl ServersFile {
    pub fn path(app_dir: &Path) -> PathBuf {
        app_dir.join(SERVERS_FILE)
    }

    pub fn parse(text: &[u8]) -> Result<Self, String> {
        if text.len() as u64 > MAX_FILE {
            return Err(format!("more than {MAX_FILE} bytes"));
        }
        let raw: RawFile = serde_json::from_slice(text).map_err(|e| e.to_string())?;
        if raw.format != FORMAT {
            return Err(format!("format {}, this game reads {FORMAT}", raw.format));
        }
        Ok(Self { format: FORMAT, favourites: raw.favourites.0, recent: raw.recent.0 })
    }

    fn read(path: &Path) -> Result<Option<Vec<u8>>, String> {
        let meta = match std::fs::symlink_metadata(path) {
            Ok(m) => m,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e.to_string()),
        };
        if !meta.file_type().is_file() {
            return Err("not a plain file (a link or a folder)".into());
        }
        let mut bytes = Vec::new();
        let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
        file.take(MAX_FILE + 1).read_to_end(&mut bytes).map_err(|e| e.to_string())?;
        Ok(Some(bytes))
    }

    pub fn load(app_dir: &Path) -> Loaded {
        let path = Self::path(app_dir);
        let failed = match Self::read(&path) {
            Ok(None) => return Loaded { file: Self::default_file(), problem: None },
            Ok(Some(bytes)) => match Self::parse(&bytes) {
                Ok(file) => return Loaded { file, problem: None },
                Err(e) => e,
            },
            Err(e) => e,
        };
        let problem = match move_aside(&path) {
            Ok(aside) => format!(
                "{} could not be read ({failed}); it was kept as {} and the lists start empty",
                path.display(),
                aside.display()
            ),
            Err(e) => format!(
                "{} could not be read ({failed}) and could not be moved aside ({e}); it is left as it is and the lists \
                 are not saved",
                path.display()
            ),
        };
        Loaded { file: Self::default_file(), problem: Some(problem) }
    }

    fn default_file() -> Self {
        Self { format: FORMAT, favourites: Vec::new(), recent: Vec::new() }
    }

    pub fn save(&self, app_dir: &Path) -> Result<Option<String>, String> {
        let path = Self::path(app_dir);
        let mut note = None;
        let current = Self::read(&path).and_then(|b| b.map(|b| Self::parse(&b).map(|_| ())).transpose());
        if let Err(e) = current {
            let aside = move_aside(&path).map_err(|m| {
                format!("{} does not read ({e}) and cannot be moved aside ({m}): not saved", path.display())
            })?;
            note = Some(format!("{} did not read ({e}); it was kept as {}", path.display(), aside.display()));
        }
        let text = serde_json::to_vec_pretty(self).map_err(|e| e.to_string())?;
        write_whole(&path, &text).map_err(|e| format!("writing {}: {e}", path.display()))?;
        Ok(note)
    }

    pub fn toggle_favourite(&mut self, address: &ServerAddress, name: Option<&str>, now: u64) -> Result<bool, String> {
        if let Some(i) = self.favourites.iter().position(|s| s.address == *address) {
            self.favourites.remove(i);
            return Ok(false);
        }
        if self.favourites.len() >= MAX_FAVOURITES {
            return Err(format!("{MAX_FAVOURITES} favourites already: remove one first"));
        }
        self.favourites.insert(0, Saved { address: address.clone(), name: name.map(str::to_string), when: now });
        Ok(true)
    }

    pub fn joined(&mut self, address: &ServerAddress, name: Option<&str>, now: u64) {
        self.recent.retain(|s| s.address != *address);
        self.recent.insert(0, Saved { address: address.clone(), name: name.map(str::to_string), when: now });
        self.recent.truncate(MAX_RECENT);
    }
}

impl Default for ServersFile {
    fn default() -> Self {
        Self::default_file()
    }
}

#[cfg(test)]
mod tests;
