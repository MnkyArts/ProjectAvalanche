use core::time::Duration;
use std::collections::{HashMap, HashSet, VecDeque};
use std::net::{IpAddr, Ipv6Addr, SocketAddr, SocketAddrV6, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use socket2::{Domain, Protocol, Socket, Type};
use ssx_net_proto::keys::random_bytes;
use ssx_net_proto::query::{
    CHALLENGE_SIZE, Challenge, QUERY_ANSWER_SIZE, QUERY_PORT_OFFSET, QueryAnswer, QueryRequest, TOKEN_SIZE,
};

pub const PER_SECOND: u32 = 10;
pub const RETRY_AFTER: Duration = Duration::from_secs(4);
pub const TIMEOUT: Duration = Duration::from_secs(8);
pub const TOKEN_FOR: Duration = Duration::from_secs(25);
// PORT(choice): 1 024 waiting, 128 at once (10 a second for 8 s is 80), 4 096 tokens; tuning: the browser over a full
// master list; the original has no server browser.
pub const MAX_WAITING: usize = 1024;
pub const MAX_ACTIVE: usize = 128;
pub const MAX_TOKENS: usize = 4096;
const TICK: Duration = Duration::from_millis(50);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pinged {
    pub game: SocketAddr,
    pub result: Result<(QueryAnswer, Duration), String>,
}

fn canonical(a: SocketAddr) -> SocketAddr {
    match a {
        SocketAddr::V6(v6) => match v6.ip().to_ipv4_mapped() {
            Some(v4) => SocketAddr::new(IpAddr::V4(v4), v6.port()),
            None => a,
        },
        v4 => v4,
    }
}

struct Wire {
    socket: UdpSocket,
    dual: bool,
}

impl Wire {
    fn open() -> std::io::Result<Self> {
        let dual = (|| -> std::io::Result<UdpSocket> {
            let s = Socket::new(Domain::IPV6, Type::DGRAM, Some(Protocol::UDP))?;
            s.set_only_v6(false)?;
            s.bind(&SocketAddr::new(IpAddr::V6(Ipv6Addr::UNSPECIFIED), 0).into())?;
            Ok(s.into())
        })();
        let (socket, dual) = match dual {
            Ok(s) => (s, true),
            Err(_) => (UdpSocket::bind(("0.0.0.0", 0))?, false),
        };
        Ok(Self { socket, dual })
    }

    fn send(&self, to: SocketAddr, bytes: &[u8]) -> Result<(), String> {
        let to = match (to, self.dual) {
            (SocketAddr::V4(v4), true) => SocketAddr::V6(SocketAddrV6::new(v4.ip().to_ipv6_mapped(), v4.port(), 0, 0)),
            (SocketAddr::V6(_), false) => return Err("this machine has no IPv6 socket".into()),
            (a, _) => a,
        };
        self.socket.send_to(bytes, to).map(|_| ()).map_err(|e| format!("the query could not be sent: {e}"))
    }
}

enum Stage {
    Challenge,
    Query { token: [u8; TOKEN_SIZE] },
}

struct Attempt {
    at: Instant,
    sequence: u32,
}

struct Job {
    game: SocketAddr,
    query: SocketAddr,
    stage: Stage,
    attempts: Vec<Attempt>,
    started: Instant,
}

impl Job {
    fn request(&self, sequence: u32) -> Vec<u8> {
        match self.stage {
            Stage::Challenge => QueryRequest::Challenge.encode(),
            Stage::Query { token } => QueryRequest::Query { token, sequence }.encode(),
        }
    }
}

struct Worker {
    wire: Wire,
    jobs: Mutex<Receiver<SocketAddr>>,
    out: Sender<Pinged>,
    stop: Arc<AtomicBool>,
    waiting: VecDeque<SocketAddr>,
    active: Vec<Job>,
    tokens: HashMap<SocketAddr, ([u8; TOKEN_SIZE], Instant)>,
    next_start: Instant,
}

impl Worker {
    fn finish(&mut self, i: usize, result: Result<(QueryAnswer, Duration), String>) {
        let job = self.active.swap_remove(i);
        let _ = self.out.send(Pinged { game: job.game, result });
    }

    fn send(&mut self, i: usize, now: Instant) {
        let sequence = u32::from_be_bytes(random_bytes());
        let job = &mut self.active[i];
        let bytes = job.request(sequence);
        job.attempts.push(Attempt { at: now, sequence });
        let to = job.query;
        if let Err(e) = self.wire.send(to, &bytes) {
            self.finish(i, Err(e));
        }
    }

    fn step(&mut self, now: Instant) -> Instant {
        if let Ok(rx) = self.jobs.lock() {
            while self.waiting.len() < MAX_WAITING {
                match rx.try_recv() {
                    Ok(game) => self.waiting.push_back(game),
                    Err(_) => break,
                }
            }
        }
        let pace = Duration::from_secs(1) / PER_SECOND;
        while now >= self.next_start && self.active.len() < MAX_ACTIVE {
            let Some(game) = self.waiting.pop_front() else { break };
            self.next_start = self.next_start.max(now.checked_sub(pace).unwrap_or(now)) + pace;
            let Some(port) = game.port().checked_add(QUERY_PORT_OFFSET) else {
                let _ = self.out.send(Pinged { game, result: Err("no query port above 65535".into()) });
                continue;
            };
            let query = canonical(SocketAddr::new(game.ip(), port));
            let stage = match self.tokens.get(&query) {
                Some((token, at)) if now.duration_since(*at) < TOKEN_FOR => Stage::Query { token: *token },
                _ => Stage::Challenge,
            };
            self.active.push(Job { game, query, stage, attempts: Vec::new(), started: now });
            let i = self.active.len() - 1;
            self.send(i, now);
        }
        let mut i = 0;
        while i < self.active.len() {
            let job = &self.active[i];
            if now.duration_since(job.started) >= TIMEOUT {
                let why =
                    format!("no answer to the UDP query on port {} within {} s", job.query.port(), TIMEOUT.as_secs());
                self.finish(i, Err(why));
                continue;
            }
            let last = job.attempts.last().map_or(job.started, |a| a.at);
            if job.attempts.len() < 2 && now.duration_since(last) >= RETRY_AFTER {
                self.send(i, now);
            }
            i += 1;
        }
        let mut next = now + TICK;
        if !self.waiting.is_empty() && self.active.len() < MAX_ACTIVE {
            next = next.min(self.next_start);
        }
        for job in &self.active {
            next = next.min(job.started + TIMEOUT);
            if job.attempts.len() < 2 {
                next = next.min(job.attempts.last().map_or(now, |a| a.at) + RETRY_AFTER);
            }
        }
        next
    }

    fn take(&mut self, bytes: &[u8], from: SocketAddr, at: Instant) {
        let from = canonical(from);
        let Some(i) = self.active.iter().position(|j| j.query == from) else { return };
        match self.active[i].stage {
            Stage::Challenge if bytes.len() == CHALLENGE_SIZE => {
                let Ok(c) = Challenge::decode(bytes) else { return };
                if self.tokens.len() >= MAX_TOKENS {
                    self.tokens.retain(|_, (_, got)| at.duration_since(*got) < TOKEN_FOR);
                    if self.tokens.len() >= MAX_TOKENS {
                        self.tokens.clear();
                    }
                }
                self.tokens.insert(from, (c.token, at));
                let job = &mut self.active[i];
                job.stage = Stage::Query { token: c.token };
                job.attempts.clear();
                self.send(i, at);
            }
            Stage::Query { .. } if bytes.len() == QUERY_ANSWER_SIZE => {
                let Ok(answer) = QueryAnswer::decode(bytes) else { return };
                let job = &self.active[i];
                let Some(sent) = job.attempts.iter().find(|a| a.sequence == answer.sequence).map(|a| a.at) else {
                    return;
                };
                self.finish(i, Ok((answer, at.duration_since(sent))));
            }
            _ => {}
        }
    }

    fn run(mut self) {
        let mut buf = [0u8; 256];
        while !self.stop.load(Ordering::Acquire) {
            let now = Instant::now();
            let next = self.step(now);
            let wait = next.saturating_duration_since(Instant::now()).clamp(Duration::from_millis(1), TICK);
            if self.wire.socket.set_read_timeout(Some(wait)).is_err() {
                return;
            }
            if let Ok((n, from)) = self.wire.socket.recv_from(&mut buf) {
                let at = Instant::now();
                self.take(buf.get(..n).unwrap_or_default(), from, at);
            }
        }
    }
}

pub struct Pinger {
    jobs: SyncSender<SocketAddr>,
    results: Mutex<Receiver<Pinged>>,
    stop: Arc<AtomicBool>,
    waiting: HashSet<SocketAddr>,
    last: HashMap<SocketAddr, Instant>,
}

impl Pinger {
    pub fn start() -> std::io::Result<Self> {
        let wire = Wire::open()?;
        let (jobs, job_rx) = std::sync::mpsc::sync_channel(MAX_WAITING);
        let (out, results) = std::sync::mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let worker = Worker {
            wire,
            jobs: Mutex::new(job_rx),
            out,
            stop: Arc::clone(&stop),
            waiting: VecDeque::new(),
            active: Vec::new(),
            tokens: HashMap::new(),
            next_start: Instant::now(),
        };
        std::thread::Builder::new().name("server browser pings".into()).spawn(move || worker.run())?;
        Ok(Self { jobs, results: Mutex::new(results), stop, waiting: HashSet::new(), last: HashMap::new() })
    }

    pub fn ask(&mut self, game: SocketAddr, interval: Duration, now: Instant) -> bool {
        if self.waiting.contains(&game) || self.last.get(&game).is_some_and(|&t| now.duration_since(t) < interval) {
            return false;
        }
        match self.jobs.try_send(game) {
            Ok(()) => {
                self.waiting.insert(game);
                self.last.insert(game, now);
                true
            }
            Err(TrySendError::Full(_) | TrySendError::Disconnected(_)) => false,
        }
    }

    pub fn take(&mut self) -> Vec<Pinged> {
        let got: Vec<Pinged> = match self.results.lock() {
            Ok(rx) => rx.try_iter().collect(),
            Err(_) => Vec::new(),
        };
        for p in &got {
            self.waiting.remove(&p.game);
        }
        got
    }
}

impl Drop for Pinger {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
    }
}
