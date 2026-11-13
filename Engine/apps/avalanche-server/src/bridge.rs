use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

use ssx_net::server::TicketRequest;
use ssx_net_proto::join::{RefusalCode, TOKEN_SIZE};
use tokio::sync::{mpsc, oneshot};

// PORT(choice): 64 waiting joins (the world takes them every tick, 30 a second); tuning: a measured join burst of phase
// 4's 32-player servers; the original has no dedicated server.
pub const JOIN_QUEUE: usize = 64;

#[derive(Default)]
pub struct Shared {
    pub players: AtomicU32,
    pub player_names: Mutex<Vec<String>>,
    pub level: Mutex<Option<crate::maps::ShownLevel>>,
    pub stopping: AtomicBool,
    pub stop_requested: AtomicBool,
    pub joins_accepted: AtomicU64,
    pub joins_refused: AtomicU64,
    pub http_requests: AtomicU64,
    pub http_limited: AtomicU64,
    pub sessions: AtomicU64,
    pub kicks: AtomicU64,
    pub tick_p50_us: AtomicU32,
    pub tick_p99_us: AtomicU32,
    pub tick_max_us: AtomicU32,
    pub sent_bytes: AtomicU64,
    pub sent_datagrams: AtomicU64,
    pub recv_bytes: AtomicU64,
    pub recv_datagrams: AtomicU64,
    pub poses_received: AtomicU64,
    pub poses_relayed: [AtomicU64; 3],
    pub poses_decimated: AtomicU64,
    pub file_reports: AtomicU64,
    pub stop_now: AtomicBool,
    pub blocklists: Mutex<Vec<crate::blocklist::Gauge>>,
}

impl Shared {
    pub fn count(counter: &AtomicU64) {
        counter.fetch_add(1, Ordering::Relaxed);
    }
}

pub enum JoinVerdict {
    Accepted(Box<[u8; TOKEN_SIZE]>),
    Refused(RefusalCode, String),
    Held(HeldJoin),
}

pub struct HeldJoin {
    pub text: tokio::sync::watch::Receiver<String>,
    pub verdict: oneshot::Receiver<JoinVerdict>,
}

pub struct JoinTask {
    pub ticket: TicketRequest,
    pub reply: oneshot::Sender<JoinVerdict>,
}

pub struct JoinQueue(pub mpsc::Receiver<JoinTask>);

pub fn join_queue() -> (mpsc::Sender<JoinTask>, JoinQueue) {
    let (tx, rx) = mpsc::channel(JOIN_QUEUE);
    (tx, JoinQueue(rx))
}
