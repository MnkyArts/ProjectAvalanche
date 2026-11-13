use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};

use ssx_net_proto::keys::{ServerKey, to_hex};
use thiserror::Error;

pub const KNOWN_SERVERS_FILE: &str = "known_servers.txt";

const HEADER: &str = "Project Avalanche known servers v1";
// PORT(choice): 10 000 servers (1 MiB holds them at about 90 bytes a line); tuning: the direct connects players make
// once servers are public (phase 5); a full file is refused with a message, never trimmed; the original has no
// real-time multiplayer.
const MAX_FILE: u64 = 1 << 20;
pub const MAX_KNOWN_SERVERS: usize = 10_000;
const MAX_ADDRESS: usize = 262;

#[derive(Debug, Error)]
pub enum KnownError {
    #[error("{path}: {error}")]
    Io { path: PathBuf, error: std::io::Error },

    #[error("{path}: line {line}: {why} (the file was not changed; fix or remove that line)")]
    Malformed { path: PathBuf, line: usize, why: &'static str },

    #[error(
        "the server at {address} presents another identity key than the one trusted before: trusted {pinned}, \
         offered {offered}. If its operator replaced the key, remove the line for {address} from {path} and join \
         again; otherwise someone may be between you and the server"
    )]
    KeyChanged { address: String, pinned: String, offered: String, path: PathBuf },

    #[error("{0}")]
    Address(&'static str),

    #[error("{path} holds {MAX_KNOWN_SERVERS} servers; remove some lines")]
    Full { path: PathBuf },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Trust {
    Known,
    New,
    Listed,
}

pub struct KnownServers {
    path: PathBuf,
    servers: BTreeMap<String, ServerKey>,
}

fn check_address(address: &str) -> Result<(), KnownError> {
    if address.is_empty() || address.len() > MAX_ADDRESS || !address.chars().all(|c| c.is_ascii_graphic()) {
        return Err(KnownError::Address("a server address is 1 to 262 printable ASCII characters"));
    }
    Ok(())
}

impl KnownServers {
    pub fn load(app_dir: &Path) -> Result<Self, KnownError> {
        let path = app_dir.join(KNOWN_SERVERS_FILE);
        let io = |error| KnownError::Io { path: path.clone(), error };
        let mut text = String::new();
        match std::fs::File::open(&path) {
            Ok(file) => {
                file.take(MAX_FILE + 1).read_to_string(&mut text).map_err(io)?;
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self { path, servers: BTreeMap::new() });
            }
            Err(e) => return Err(io(e)),
        }
        let malformed = |line, why| KnownError::Malformed { path: path.clone(), line, why };
        if text.len() as u64 > MAX_FILE {
            return Err(malformed(0, "the file is larger than 1 MiB"));
        }
        let mut lines = text.lines().enumerate();
        if lines.next().map(|(_, l)| l) != Some(HEADER) {
            return Err(malformed(1, "not a known-servers file of this version"));
        }
        let mut servers = BTreeMap::new();
        for (i, line) in lines {
            let mut parts = line.split(' ');
            let (Some(address), Some(key), None) = (parts.next(), parts.next(), parts.next()) else {
                return Err(malformed(i + 1, "not `<address> <key>`"));
            };
            check_address(address).map_err(|_| malformed(i + 1, "the address"))?;
            let key = ServerKey::from_hex(key).map_err(|_| malformed(i + 1, "the key"))?;
            if servers.insert(address.to_string(), key).is_some() {
                return Err(malformed(i + 1, "the address is listed twice"));
            }
            if servers.len() > MAX_KNOWN_SERVERS {
                return Err(malformed(i + 1, "more than 10 000 servers"));
            }
        }
        Ok(Self { path, servers })
    }

    pub fn check(&self, address: &str, key: &ServerKey) -> Result<Trust, KnownError> {
        check_address(address)?;
        match self.servers.get(address) {
            None => Ok(Trust::New),
            Some(pinned) if pinned == key => Ok(Trust::Known),
            Some(pinned) => Err(KnownError::KeyChanged {
                address: address.to_string(),
                pinned: pinned.to_hex(),
                offered: to_hex(key.as_bytes()),
                path: self.path.clone(),
            }),
        }
    }

    pub fn trust(&mut self, address: &str, key: ServerKey) -> Result<(), KnownError> {
        if self.check(address, &key)? == Trust::Known {
            return Ok(());
        }
        if self.servers.len() >= MAX_KNOWN_SERVERS {
            return Err(KnownError::Full { path: self.path.clone() });
        }
        self.servers.insert(address.to_string(), key);
        self.save()
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn save(&self) -> Result<(), KnownError> {
        let io = |error| KnownError::Io { path: self.path.clone(), error };
        if let Some(dir) = self.path.parent() {
            ssx_files::create_dir_all(dir).map_err(|e| io(e.into_io()))?;
        }
        let mut text = format!("{HEADER}\n");
        for (address, key) in &self.servers {
            text.push_str(&format!("{address} {}\n", key.to_hex()));
        }
        ssx_files::replace(&self.path, text.as_bytes(), &ssx_files::Options::new()).map_err(|e| io(e.into_io()))
    }
}

#[cfg(test)]
mod tests {
    use ssx_net_proto::keys::ServerSecret;

    use super::*;

    #[test]
    fn first_key_pinned_changed_key_refused() {
        let dir = std::env::temp_dir().join(format!("ssx-known-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (a, b) = (ServerSecret::generate().public(), ServerSecret::generate().public());
        let mut known = KnownServers::load(&dir).expect("empty");
        assert_eq!(known.check("127.0.0.1:27450", &a).expect("check"), Trust::New);
        known.trust("127.0.0.1:27450", a).expect("trust");
        let mut known = KnownServers::load(&dir).expect("load");
        assert_eq!(known.check("127.0.0.1:27450", &a).expect("check"), Trust::Known);
        assert!(matches!(known.check("127.0.0.1:27450", &b), Err(KnownError::KeyChanged { .. })));
        assert!(known.trust("127.0.0.1:27450", b).is_err(), "never replaced by trust");
        known.trust("[::1]:27450", b).expect("another address");
        assert_eq!(KnownServers::load(&dir).expect("load").servers.len(), 2);
        let path = dir.join(KNOWN_SERVERS_FILE);
        for bad in [
            "Project Avalanche known servers v2\n".to_string(),
            format!("{HEADER}\n127.0.0.1:1 {}\n127.0.0.1:1 {}\n", a.to_hex(), b.to_hex()),
            format!("{HEADER}\n127.0.0.1:1  {}\n", a.to_hex()),
            format!("{HEADER}\n127.0.0.1:1 {}\n", "00".repeat(32)),
            format!("{HEADER}\nhost\u{7}:1 {}\n", a.to_hex()),
        ] {
            std::fs::write(&path, &bad).expect("write");
            assert!(matches!(KnownServers::load(&dir), Err(KnownError::Malformed { .. })), "{bad:?}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
