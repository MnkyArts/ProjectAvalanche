use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, RwLock};
use std::time::{Duration, Instant};

use crate::blocklists::Blocklists;
use crate::challenge::Challenges;
use crate::config::Config;
use crate::limits::{AddressLimiter, Connections, GlobalLimiter, Limiter, Rate};
use crate::registry::Registry;
use crate::trust::{Trust, TrustSet};

// PORT(choice): challenge and heartbeat 2 a second with a burst of 32, the list 1 a second (burst 8), the blocklist 1 a
// second (burst 4), anything else 2 a second (burst 4); tuning: a public master's measured request pattern; the
// original has no master server.
pub const CHALLENGE_RATE: Rate = Rate { per_second: 2.0, burst: 32.0 };
pub const HEARTBEAT_RATE: Rate = Rate { per_second: 2.0, burst: 32.0 };
pub const LIST_RATE: Rate = Rate { per_second: 1.0, burst: 8.0 };
pub const BLOCKLIST_RATE: Rate = Rate { per_second: 1.0, burst: 4.0 };
pub const OTHER_RATE: Rate = Rate { per_second: 2.0, burst: 4.0 };
// PORT(choice): one per 5 s with a burst of 4; tuning: a public master's measured request pattern; the original has no
// master server.
pub const PER_KEY_RATE: Rate = Rate { per_second: 0.2, burst: 4.0 };
// PORT(choice): one per 2 s with a burst of 16 per address, 128 at once (with the 512 connections, under the 1 024 file
// descriptors a Unix process gets by default), a 4 s wait; tuning: a public master's measured load (the proof's 100
// servers starting at once needed more than the first 64); the original has no master server.
pub const CHECK_RATE: Rate = Rate { per_second: 0.5, burst: 16.0 };
pub const CHECKS_AT_ONCE: usize = 128;
pub const CHECK_WAIT: Duration = Duration::from_secs(4);
// PORT(choice): 500 a second with a burst of 1 000 (10 000 servers beat 167 times a second); tuning: a public master's
// measured request pattern; the original has no master server.
pub const GLOBAL_HEARTBEAT_RATE: Rate = Rate { per_second: 500.0, burst: 1000.0 };
// PORT(choice): 16 384 (a full list's 10 000 servers and the browsers asking); tuning: a public master's measured
// request pattern; the original has no master server.
pub const TRACKED: usize = 16_384;
// PORT(choice): 512 overall (with the checks' sockets under the 1 024 file descriptors a Unix process gets by default),
// 16 per address (a host of 16 servers heartbeating at once); tuning: a public master's measured request pattern; the
// original has no master server.
pub const MAX_CONNECTIONS: usize = 512;
pub const MAX_CONNECTIONS_PER_ADDRESS: u32 = 16;
// PORT(choice): 448 of the 512 (64 kept for direct peers: the health check asks once every 30 s); tuning: a public
// master's measured load behind its proxy (its proxies' pool full in `status`); the original has no master server.
pub const MAX_PROXIED_CONNECTIONS: usize = 448;
// PORT(choice): 5 s for the head and the handshake, 15 s for the connection, 32 headers (the server's HTTP limits, with
// room for the check); tuning: the slowest real servers and browsers; the original has no master server.
pub const HEAD_TIMEOUT: Duration = Duration::from_secs(5);
pub const CONNECTION_TIMEOUT: Duration = Duration::from_secs(15);
pub const MAX_HEAD_BYTES: usize = 8192;
pub const MAX_HEADERS: usize = 32;

pub const ENDPOINTS: usize = 5;
pub const CHALLENGE: usize = 0;
pub const HEARTBEAT: usize = 1;
pub const SERVERS: usize = 2;
pub const BLOCKLIST: usize = 3;
pub const OTHER: usize = 4;

pub struct Limits {
    pub endpoint: [AddressLimiter; ENDPOINTS],
    pub per_key: Limiter<[u8; 32]>,
    pub checks: AddressLimiter,
    pub heartbeats: GlobalLimiter,
    pub connections: Connections,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            endpoint: [CHALLENGE_RATE, HEARTBEAT_RATE, LIST_RATE, BLOCKLIST_RATE, OTHER_RATE]
                .map(|r| AddressLimiter::new(r, TRACKED)),
            per_key: Limiter::new(PER_KEY_RATE, TRACKED),
            checks: AddressLimiter::new(CHECK_RATE, TRACKED),
            heartbeats: GlobalLimiter::new(GLOBAL_HEARTBEAT_RATE),
            connections: Connections::new(MAX_CONNECTIONS, MAX_CONNECTIONS_PER_ADDRESS, MAX_PROXIED_CONNECTIONS),
        }
    }
}

#[derive(Default)]
pub struct Stats {
    pub requests: AtomicU64,
    pub limited: AtomicU64,
    pub heartbeats: AtomicU64,
    pub refused: AtomicU64,
    pub checks_passed: AtomicU64,
    pub checks_failed: AtomicU64,
    pub expired: AtomicU64,
    pub lists_served: AtomicU64,
    pub not_modified: AtomicU64,
    pub proxied: AtomicU64,
    pub forwarded_refused: AtomicU64,
    pub proxy_pool_full: AtomicU64,
}

impl Stats {
    pub fn count(counter: &AtomicU64) {
        counter.fetch_add(1, Ordering::Relaxed);
    }
}

pub struct Master {
    pub config: Config,
    pub started: Instant,
    pub challenges: Challenges,
    pub registry: Mutex<Registry>,
    pub blocklists: RwLock<Blocklists>,
    pub limits: Limits,
    pub checks: tokio::sync::Semaphore,
    pub stats: Stats,
    pub stopping: AtomicBool,
    pub trust: Trust,
}

impl Master {
    pub fn new(config: Config, blocklists: Blocklists) -> Self {
        let trust = Trust::new(TrustSet::new(config.trusted.networks().to_vec(), config.trusted_hosts.clone()));
        Self {
            registry: Mutex::new(Registry::new(config.max_servers, config.max_per_address)),
            trust,
            config,
            started: Instant::now(),
            challenges: Challenges::default(),
            blocklists: RwLock::new(blocklists),
            limits: Limits::default(),
            checks: tokio::sync::Semaphore::new(CHECKS_AT_ONCE),
            stats: Stats::default(),
            stopping: AtomicBool::new(false),
        }
    }
}
