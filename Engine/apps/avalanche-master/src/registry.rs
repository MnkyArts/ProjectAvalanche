use std::collections::HashMap;
use std::net::IpAddr;
use std::time::{Duration, Instant};

use bytes::Bytes;
use ssx_net_proto::keys::{KEY_SIZE, to_hex};
use ssx_net_proto::master::{EXPIRE_AFTER, Heartbeat, LIST_VERSION, ListedServer, ServerList, address_text};

use crate::address::{AddressKey, address_key};
use crate::blocklists::etag;

// PORT(choice): 15 minutes (a server that keeps beating while its ports closed is off the list within a quarter of an
// hour; FiveM's and MTA's masters check once); tuning: the check load of a public master; the original has no master
// server.
pub const RECHECK_AFTER: Duration = Duration::from_secs(15 * 60);
pub const REBUILD_EVERY: Duration = Duration::from_secs(1);

#[derive(Clone, Debug)]
pub struct Listing {
    pub key: [u8; KEY_SIZE],
    pub ip: IpAddr,
    pub beat: Heartbeat,
    pub info_hash: [u8; 32],
    pub last_seen: Instant,
    pub checked_at: Instant,
    pub listed_since: Instant,
}

impl Listing {
    fn listed(&self) -> ListedServer {
        let b = &self.beat;
        ListedServer {
            address: address_text(self.ip),
            port: b.port,
            key: to_hex(&self.key),
            name: b.name.clone(),
            mode: b.mode.clone(),
            drop: b.drop.clone(),
            map: b.map.clone(),
            players: b.players,
            max_players: b.max_players,
            password: b.password,
            region: b.region.clone(),
            tags: b.tags.clone(),
            build: b.build.clone(),
            api_level: b.api_level,
            info_version: b.info_version,
            info_hash: to_hex(&self.info_hash),
            resources: b.resources.clone(),
        }
    }
}

fn fixed(b: &Heartbeat) -> Heartbeat {
    Heartbeat { players: 0, leaving: false, ..b.clone() }
}

struct Cached {
    generation: u64,
    built: Instant,
    body: Bytes,
    etag: String,
}

pub struct Registry {
    max_servers: usize,
    max_per_address: usize,
    entries: HashMap<[u8; KEY_SIZE], Listing>,
    per_address: HashMap<AddressKey, usize>,
    generation: u64,
    lists: HashMap<u32, Cached>,
}

impl Registry {
    pub fn new(max_servers: usize, max_per_address: usize) -> Self {
        Self {
            max_servers,
            max_per_address,
            entries: HashMap::new(),
            per_address: HashMap::new(),
            generation: 0,
            lists: HashMap::new(),
        }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn get(&self, key: &[u8; KEY_SIZE]) -> Option<&Listing> {
        self.entries.get(key)
    }

    pub fn iter(&self) -> impl Iterator<Item = &Listing> {
        self.entries.values()
    }

    pub fn needs_check(&self, key: &[u8; KEY_SIZE], ip: IpAddr, beat: &Heartbeat, now: Instant) -> bool {
        match self.entries.get(key) {
            Some(l) => l.ip != ip || fixed(&l.beat) != fixed(beat) || now.duration_since(l.checked_at) >= RECHECK_AFTER,
            None => true,
        }
    }

    pub fn room_for(&self, key: &[u8; KEY_SIZE], ip: IpAddr, port: u16) -> Result<(), String> {
        let at = address_key(ip);
        if self.entries.get(key).is_some_and(|l| address_key(l.ip) == at) {
            return Ok(());
        }
        let same_place = self.entries.values().filter(|l| l.ip == ip && l.beat.port == port).count();
        let here = self.per_address.get(&at).copied().unwrap_or(0) - same_place.min(1);
        if here >= self.max_per_address {
            return Err(format!(
                "{here} servers from your address are listed already (at most {})",
                self.max_per_address
            ));
        }
        if !self.entries.contains_key(key) && self.entries.len() - same_place.min(1) >= self.max_servers {
            return Err(format!("the master's list is full ({} servers)", self.max_servers));
        }
        Ok(())
    }

    pub fn list(
        &mut self,
        key: [u8; KEY_SIZE],
        ip: IpAddr,
        beat: Heartbeat,
        info_hash: [u8; 32],
        now: Instant,
    ) -> Vec<Listing> {
        let since = self.entries.get(&key).filter(|l| l.ip == ip).map_or(now, |l| l.listed_since);
        self.remove(&key);
        let same_place: Vec<[u8; KEY_SIZE]> =
            self.entries.values().filter(|l| l.ip == ip && l.beat.port == beat.port).map(|l| l.key).collect();
        let replaced: Vec<Listing> = same_place.iter().filter_map(|k| self.remove(k)).collect();
        *self.per_address.entry(address_key(ip)).or_insert(0) += 1;
        self.entries
            .insert(key, Listing { key, ip, beat, info_hash, last_seen: now, checked_at: now, listed_since: since });
        self.generation += 1;
        replaced
    }

    pub fn refresh(&mut self, key: &[u8; KEY_SIZE], beat: &Heartbeat, now: Instant) {
        if let Some(l) = self.entries.get_mut(key) {
            l.last_seen = now;
            if l.beat.players != beat.players {
                l.beat.players = beat.players;
                self.generation += 1;
            }
        }
    }

    pub fn remove(&mut self, key: &[u8; KEY_SIZE]) -> Option<Listing> {
        let l = self.entries.remove(key)?;
        let at = address_key(l.ip);
        if let Some(n) = self.per_address.get_mut(&at) {
            *n -= 1;
            if *n == 0 {
                self.per_address.remove(&at);
            }
        }
        self.generation += 1;
        Some(l)
    }

    pub fn expire(&mut self, now: Instant) -> Vec<Listing> {
        let old: Vec<[u8; KEY_SIZE]> = self
            .entries
            .values()
            .filter(|l| now.saturating_duration_since(l.last_seen) >= EXPIRE_AFTER)
            .map(|l| l.key)
            .collect();
        old.iter().filter_map(|k| self.remove(k)).collect()
    }

    pub fn body(&mut self, protocol: u32, now: Instant) -> (Bytes, String) {
        if let Some(c) = self.lists.get(&protocol)
            && (c.generation == self.generation || now.duration_since(c.built) < REBUILD_EVERY)
        {
            return (c.body.clone(), c.etag.clone());
        }
        let mut servers: Vec<&Listing> =
            self.entries.values().filter(|l| l.beat.protocol_version == protocol).collect();
        servers.sort_by_key(|l| l.key);
        let list = ServerList {
            list_version: LIST_VERSION,
            protocol_version: protocol,
            servers: servers.iter().map(|l| l.listed()).collect(),
        };
        let body = Bytes::from(list.to_json());
        let etag = etag(&body);
        if self.lists.len() >= 64 && !self.lists.contains_key(&protocol) {
            self.lists.retain(|p, _| self.entries.values().any(|l| l.beat.protocol_version == *p));
        }
        self.lists.insert(
            protocol,
            Cached { generation: self.generation, built: now, body: body.clone(), etag: etag.clone() },
        );
        (body, etag)
    }
}

#[cfg(test)]
mod tests {
    use ssx_net_proto::master::{HEARTBEAT_VERSION, MAX_LIST_BYTES, MAX_LIST_ENTRY_BYTES, MAX_LIST_SERVERS};

    use super::*;

    fn beat(name: &str) -> Heartbeat {
        Heartbeat {
            heartbeat_version: HEARTBEAT_VERSION,
            port: 27450,
            protocol_version: 4,
            info_version: 3,
            api_level: 0,
            build: "project-avalanche/0.1.0".into(),
            name: name.into(),
            mode: "freeride".into(),
            drop: "exp_alps_vorassay_drop_3".into(),
            map: None,
            players: 0,
            max_players: 16,
            password: false,
            region: String::new(),
            tags: vec![],
            contact: String::new(),
            leaving: false,
            resources: ssx_net_proto::resources::ResourceSummary {
                content_id: "4e".repeat(32),
                client_mods: "presentation".into(),
                client_scripts: false,
                count: 2,
                download: 20_635,
                manifest: "ab".repeat(32),
                manifest_bytes: 2_431,
            },
        }
    }

    #[test]
    fn limits_changes_expiry_and_the_cached_body() {
        let mut r = Registry::new(20, 2);
        let t0 = Instant::now();
        let a: IpAddr = "198.51.100.7".parse().unwrap();
        r.list([1; 32], a, beat("One"), [0; 32], t0);
        r.list([2; 32], a, Heartbeat { port: 27452, ..beat("Two") }, [0; 32], t0);
        assert!(r.room_for(&[3; 32], a, 27454).is_err_and(|e| e.contains("2 servers")), "16 per address (2 here)");
        assert!(r.room_for(&[1; 32], a, 27450).is_ok(), "its own entry");
        assert!(r.room_for(&[3; 32], a, 27450).is_ok(), "a new key at a listed place replaces it");
        assert!(r.room_for(&[3; 32], "198.51.100.8".parse().unwrap(), 27450).is_ok());
        assert!(!r.needs_check(&[1; 32], a, &Heartbeat { players: 5, ..beat("One") }, t0), "players only");
        assert!(!r.needs_check(&[2; 32], a, &Heartbeat { port: 27452, ..beat("Two") }, t0));
        assert!(r.needs_check(&[1; 32], a, &beat("One!"), t0), "a changed name");
        assert!(r.needs_check(&[1; 32], "198.51.100.9".parse().unwrap(), &beat("One"), t0), "moved");
        assert!(r.needs_check(&[1; 32], a, &beat("One"), t0 + RECHECK_AFTER), "the check is old");
        let (body, tag) = r.body(4, t0);
        let list = ServerList::from_json(&body, 4).expect("reads");
        assert_eq!(list.servers.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), ["One", "Two"]);
        assert_eq!(r.body(4, t0 + Duration::from_millis(10)).1, tag, "unchanged: the same body");
        r.refresh(&[1; 32], &Heartbeat { players: 3, ..beat("One") }, t0 + Duration::from_secs(60));
        assert_eq!(r.body(4, t0 + Duration::from_millis(500)).1, tag, "rebuilt at most once a second");
        let (body2, tag2) = r.body(4, t0 + Duration::from_secs(61));
        assert_ne!(tag2, tag);
        assert_eq!(ServerList::from_json(&body2, 4).expect("reads").servers[0].players, 3);
        assert!(ServerList::from_json(&r.body(5, t0).0, 5).expect("reads").servers.is_empty(), "another protocol");
        let t1 = t0 + Duration::from_secs(1);
        let replaced = r.list([9; 32], a, Heartbeat { port: 27452, ..beat("Two") }, [0; 32], t1);
        assert_eq!(replaced.iter().map(|l| l.key).collect::<Vec<_>>(), [[2; 32]]);
        assert_eq!(r.len(), 2);
        assert!(ServerList::from_json(&r.body(4, t0 + Duration::from_secs(70)).0, 4).is_ok(), "no place twice");
        r.remove(&[9; 32]);
        r.list([2; 32], a, Heartbeat { port: 27452, ..beat("Two") }, [0; 32], t0);
        let gone = r.expire(t0 + EXPIRE_AFTER);
        assert_eq!(gone.iter().map(|l| l.key).collect::<Vec<_>>(), [[2; 32]], "3 missed heartbeats");
        assert!(r.room_for(&[3; 32], a, 27454).is_ok(), "room again at the address");
        assert_eq!(r.expire(t0 + Duration::from_secs(60) + EXPIRE_AFTER).len(), 1);
        assert!(r.is_empty());
    }

    #[test]
    fn the_largest_list_fits_the_clients_cap() {
        let mut r = Registry::new(MAX_LIST_SERVERS, 256);
        let t0 = Instant::now();
        let json_len = |s: &str| serde_json::to_string(s).map_or(0, |j| j.len());
        let widest = (0..=0x10_FFFF_u32)
            .filter_map(char::from_u32)
            .filter(|c| ssx_net_proto::name::server_name(&c.to_string()).is_ok_and(|n| n == c.to_string()))
            .max_by_key(|c| json_len(&c.to_string()))
            .expect("a drawable character");
        let mut wide = beat(&widest.to_string().repeat(ssx_net_proto::name::MAX_SERVER_NAME_CHARS));
        wide.drop = String::new();
        wide.map = Some(ssx_net_proto::instance::MapSummary {
            id: format!("a:map/{}", "c".repeat(ssx_net_proto::instance::MAX_MAP_ID - 6)),
            title: "\u{1D11E}".repeat(ssx_net_proto::instance::MAX_MAP_TITLE_CHARS),
            event_type: 8,
            gamemode: Some(ssx_net_proto::instance::GamemodeSummary {
                id: format!("a:gamemode/{}", "c".repeat(ssx_net_proto::instance::MAX_MAP_ID - 11)),
                title: "\u{1D11E}".repeat(ssx_net_proto::instance::MAX_MAP_TITLE_CHARS),
            }),
        });
        wide.region = "r".repeat(32);
        wide.tags = (0..8).map(|i| format!("{i}{}", "t".repeat(23))).collect();
        wide.build = "\\".repeat(64);
        wide.max_players = 64;
        wide.players = 64;
        wide.port = 65534;
        wide.protocol_version = u32::MAX;
        wide.api_level = u32::MAX;
        wide.info_version = u32::MAX;
        wide.resources.client_mods = "presentation".into();
        wide.resources.client_scripts = false;
        wide.resources.count = ssx_net_proto::resources::MAX_RESOURCES;
        wide.resources.download = ssx_net_proto::resources::MAX_DOWNLOAD;
        wide.resources.manifest_bytes = ssx_net_proto::resources::MAX_MANIFEST_BYTES;
        for i in 0..MAX_LIST_SERVERS as u32 {
            let mut key = [0u8; 32];
            key[..4].copy_from_slice(&i.to_be_bytes());
            let (high, low) = (0x1000 | (i >> 12) as u16, 0x1000 | (i & 0xFFF) as u16);
            let ip = IpAddr::from([0xfd00, 0xffff, 0xffff, 0xffff, high, 0xffff, 0xffff, low]);
            r.list(key, ip, wide.clone(), [0xFF; 32], t0);
        }
        let (body, _) = r.body(u32::MAX, t0);
        let entry = r.iter().map(|l| serde_json::to_vec(&l.listed()).unwrap_or_default().len()).max().unwrap_or(0);
        assert!(entry <= MAX_LIST_ENTRY_BYTES, "{entry} > {MAX_LIST_ENTRY_BYTES}");
        assert_eq!(entry, 2_261, "the widest entry");
        assert!(body.len() <= MAX_LIST_BYTES, "{} > {MAX_LIST_BYTES}", body.len());
        assert_eq!(ServerList::from_json(&body, u32::MAX).expect("reads").servers.len(), MAX_LIST_SERVERS);
    }
}
