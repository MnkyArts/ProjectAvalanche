use core::time::Duration;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use ssx_net_proto::icon::Icon;
use ssx_net_proto::players::PlayerList;

use super::address::ServerAddress;
use super::query::{InfoAnswer, ask_icon, ask_info, ask_players};

// PORT(choice): 8 threads and 256 waiting questions (a tab of 100 favourites refreshed in about 13 s when every one
// times out after 4 s; MTA queries 10 a second); tuning: the browser over real lists (phase 5); the original has no
// server browser.
pub const WORKERS: usize = 8;
pub const QUEUE: usize = 256;
pub const MIN_INTERVAL: Duration = Duration::from_millis(2500);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Ask {
    Info,
    Players,
    Icon([u8; 32]),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reply {
    Info(Result<(InfoAnswer, Duration), String>),
    Players(Result<PlayerList, String>),
    Icon([u8; 32], Result<Icon, String>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Answer {
    pub address: ServerAddress,
    pub socket: Option<std::net::SocketAddr>,
    pub reply: Reply,
    pub need: Option<crate::resources::Need>,
    pub blocked: Option<String>,
}

struct Job {
    address: ServerAddress,
    ask: Ask,
}

pub struct Refresher {
    jobs: Option<SyncSender<Job>>,
    stopped: Arc<AtomicBool>,
    answers: Mutex<Receiver<Answer>>,
    last: HashMap<(ServerAddress, Ask), Instant>,
    waiting: HashSet<(ServerAddress, Ask)>,
}

fn work(
    jobs: &Mutex<Receiver<Job>>,
    answers: &std::sync::mpsc::Sender<Answer>,
    stopped: &AtomicBool,
    cache: Option<&crate::resources::CacheView>,
) {
    loop {
        let job = match jobs.lock() {
            Ok(rx) => rx.recv(),
            Err(_) => return,
        };
        let Ok(Job { address, ask }) = job else { return };
        if stopped.load(Ordering::Acquire) {
            return;
        }
        let (socket, reply) = match address.resolve() {
            Ok(socket) => (
                Some(socket),
                match ask {
                    Ask::Info => Reply::Info(ask_info(socket)),
                    Ask::Players => Reply::Players(ask_players(socket)),
                    Ask::Icon(hash) => Reply::Icon(hash, ask_icon(socket, &hash)),
                },
            ),
            Err(e) => (
                None,
                match ask {
                    Ask::Info => Reply::Info(Err(e)),
                    Ask::Players => Reply::Players(Err(e)),
                    Ask::Icon(hash) => Reply::Icon(hash, Err(e)),
                },
            ),
        };
        let look = match (&reply, cache) {
            (Reply::Info(Ok((InfoAnswer::Info(info), _))), Some(c)) => c.look(&info.resources),
            _ => crate::resources::Look::default(),
        };
        let (need, blocked) = (look.need, look.blocked);
        if answers.send(Answer { address, socket, reply, need, blocked }).is_err() {
            return;
        }
    }
}

impl Refresher {
    pub fn start(cache: Option<crate::resources::CacheView>) -> std::io::Result<Self> {
        let cache = Arc::new(cache);
        let (jobs, job_rx) = std::sync::mpsc::sync_channel::<Job>(QUEUE);
        let (answer_tx, answers) = std::sync::mpsc::channel();
        let job_rx = Arc::new(Mutex::new(job_rx));
        let stopped = Arc::new(AtomicBool::new(false));
        for n in 0..WORKERS {
            let (rx, tx, stop, cache) =
                (Arc::clone(&job_rx), answer_tx.clone(), Arc::clone(&stopped), Arc::clone(&cache));
            std::thread::Builder::new()
                .name(format!("server browser {n}"))
                .spawn(move || work(&rx, &tx, &stop, cache.as_ref().as_ref()))?;
        }
        Ok(Self {
            jobs: Some(jobs),
            stopped,
            answers: Mutex::new(answers),
            last: HashMap::new(),
            waiting: HashSet::new(),
        })
    }

    pub fn ask(&mut self, address: &ServerAddress, ask: Ask, interval: Duration, now: Instant) -> bool {
        let key = (address.clone(), ask);
        if self.waiting.contains(&key) {
            return false;
        }
        if self.last.get(&key).is_some_and(|&t| now.duration_since(t) < interval.max(MIN_INTERVAL)) {
            return false;
        }
        let Some(jobs) = &self.jobs else { return false };
        match jobs.try_send(Job { address: address.clone(), ask }) {
            Ok(()) => {
                self.last.insert(key.clone(), now);
                self.waiting.insert(key);
                true
            }
            Err(TrySendError::Full(_) | TrySendError::Disconnected(_)) => false,
        }
    }

    pub fn take(&mut self) -> Vec<Answer> {
        let mut out = Vec::new();
        let Ok(rx) = self.answers.lock() else { return out };
        while let Ok(a) = rx.try_recv() {
            let ask = match a.reply {
                Reply::Info(_) => Ask::Info,
                Reply::Players(_) => Ask::Players,
                Reply::Icon(hash, _) => Ask::Icon(hash),
            };
            self.waiting.remove(&(a.address.clone(), ask));
            out.push(a);
        }
        out
    }

    pub fn busy(&self) -> usize {
        self.waiting.len()
    }
}

impl Drop for Refresher {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Release);
        self.jobs = None;
    }
}
