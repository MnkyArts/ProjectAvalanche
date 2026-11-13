pub mod fetch;

use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bevy::prelude::*;
use ssx_content::ContentHash;
use ssx_content::blocklist::{Blocked, Followed, Hit, List, REFRESH_INTERVAL, State, describe, now_secs};
use ssx_core::text::{PATH_CHARS, quoted, shown_max};
use ssx_net_proto::blocklist_file::RESOURCES_HEADER;
use ssx_net_proto::master::MasterUrl;

pub const LOCAL_SOURCE: &str = "the server's own list";
const MAX_PATH: usize = 4096;
// PORT(choice): 10 s: an operator's repeated command does not ask the master again each time; nothing measured; tuning:
// the request rate the master accepts (it answers 1 a second, burst 4, per address); the original has no dedicated
// server.
const MIN_ASK: Duration = Duration::from_secs(10);
// PORT(choice): 120 s, above the queue's longest job seen (a copy's read and mount, seconds); tuning: the queue's times
// on a server with many resources; the original has no dedicated server.
const REQUEUE: Duration = Duration::from_secs(120);

pub fn local_path(s: &str) -> Result<PathBuf, String> {
    if s.is_empty() || s.len() > MAX_PATH || s.chars().any(char::is_control) {
        return Err(format!("blocklist: a path of 1 to {MAX_PATH} bytes, no control characters"));
    }
    Ok(PathBuf::from(s))
}

pub fn cache_dir(data_dir: &Path) -> PathBuf {
    data_dir.join(ssx_content::blocklist::CACHE_FOLDER)
}

#[derive(Clone, Debug)]
pub struct Sources {
    pub dir: PathBuf,
    pub master: Option<MasterUrl>,
    pub local: Option<PathBuf>,
}

impl Sources {
    pub fn of(config: &crate::config::Config, data_dir: &Path) -> Self {
        let local =
            config.discovery.blocklist.as_ref().map(|p| if p.is_absolute() { p.clone() } else { data_dir.join(p) });
        Sources { dir: cache_dir(data_dir), master: config.discovery.master.clone(), local }
    }

    fn master_keys(&self) -> Vec<String> {
        self.master.iter().map(ToString::to_string).collect()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Gauge {
    pub source: String,
    pub entries: usize,
    pub age_secs: Option<u64>,
    pub applied: bool,
}

#[derive(Clone, Debug, Default)]
pub struct Lists {
    pub blocked: Arc<Blocked>,
    pub lines: Vec<String>,
    pub gauges: Vec<Gauge>,
    local: Option<List>,
}

pub fn read_local(path: &Path) -> Result<List, String> {
    let entries = ssx_net_proto::blocklist_file::read(path, RESOURCES_HEADER)?;
    let mut hashes: Vec<ContentHash> = entries.into_iter().map(|(k, _)| ContentHash(k)).collect();
    hashes.sort_unstable();
    List::new(LOCAL_SOURCE.to_string(), hashes)
}

pub fn load(sources: &Sources, now: u64, previous: Option<&List>) -> Result<Lists, String> {
    let Followed { blocked, states, .. } = ssx_content::blocklist::load(&sources.dir, &sources.master_keys(), now);
    let mut lists: Vec<List> = blocked.lists().to_vec();
    let mut lines: Vec<String> = states.iter().map(|(m, s)| format!("blocklist of {}", s.line(m))).collect();
    let mut gauges: Vec<Gauge> = states
        .iter()
        .map(|(m, s)| {
            let (entries, age_secs, applied) = match s {
                State::Applied { entries, age, .. } => (*entries, Some(age.as_secs()), true),
                State::Expired { entries, age } => (*entries, Some(age.as_secs()), false),
                State::NotYet | State::Unreadable(_) => (0, None, false),
            };
            Gauge { source: m.clone(), entries, age_secs, applied }
        })
        .collect();
    let mut local = None;
    if let Some(path) = &sources.local {
        let shown = shown_max(&path.display().to_string(), PATH_CHARS).to_string();
        let list = match (read_local(path), previous) {
            (Ok(l), _) => {
                lines.push(format!("blocklist {LOCAL_SOURCE} ({shown}): {} blocked files", l.len()));
                l
            }
            (Err(e), Some(old)) => {
                lines.push(format!(
                    "blocklist {LOCAL_SOURCE} ({shown}) does not read ({e}): the list read before stays"
                ));
                old.clone()
            }
            (Err(e), None) => return Err(format!("[discovery] blocklist: {e}")),
        };
        gauges.push(Gauge { source: "local".into(), entries: list.len(), age_secs: None, applied: true });
        lists.push(list.clone());
        local = Some(list);
    }
    if sources.master.is_none() && sources.local.is_none() {
        lines.push("blocklists: none followed (a development build without a master; no [discovery] blocklist)".into());
    }
    Ok(Lists { blocked: Arc::new(Blocked::new(lists)), lines, gauges, local })
}

pub fn hits(blocked: &Blocked, files: &[(String, ContentHash)]) -> Vec<Hit> {
    blocked.hits(files.iter().map(|(p, h)| (p.as_str(), h)))
}

pub fn dirs(running: &[crate::resources::Started]) -> Vec<PathBuf> {
    running.iter().map(|s| s.loaded.dir.clone()).collect()
}

pub fn refuse_started(
    blocked: &Blocked,
    before: &[PathBuf],
    running: &[crate::resources::Started],
) -> Result<(), String> {
    if blocked.is_empty() {
        return Ok(());
    }
    for s in running.iter().filter(|s| !before.contains(&s.loaded.dir)) {
        let found = hits(blocked, &s.loaded.hashes);
        if !found.is_empty() {
            return Err(describe(&format!("the resource {}", quoted(&s.loaded.name)), &found));
        }
    }
    Ok(())
}

pub fn remove_new_copies(before: &[crate::resources::Started], running: &[crate::resources::Started]) {
    for s in running.iter().filter(|s| !before.iter().any(|b| b.loaded.dir == s.loaded.dir)) {
        if let Err(e) = std::fs::remove_dir_all(&s.loaded.dir) {
            warn!("blocklists: {}: {e}", s.loaded.dir.display());
        }
    }
}

enum Wake {
    Now,
}

#[derive(Resource)]
pub struct ServerBlocklists {
    pub sources: Sources,
    pub current: Lists,
    updates: Mutex<Receiver<(Option<Lists>, Vec<String>)>>,
    wake: Mutex<Sender<Wake>>,
    last_ask: Option<std::time::Instant>,
    pub fetched: Vec<String>,
    pub stopped: u64,
    stopping: Vec<(String, std::time::Instant)>,
    shared: Arc<crate::bridge::Shared>,
}

#[derive(Resource, Clone, Copy)]
pub struct Interval(pub Duration);

impl ServerBlocklists {
    pub fn status(&self, out: &mut Vec<String>) {
        out.push(format!(
            "blocklists: {} files blocked over {} lists; {} running resources stopped for a blocked file",
            self.current.blocked.entries(),
            self.current.blocked.lists().len(),
            self.stopped
        ));
        out.extend(self.current.lines.iter().map(|l| format!("  {l}")));
        out.extend(self.fetched.iter().map(|l| format!("  last fetch: {l}")));
    }

    pub fn ask_now(&mut self) -> bool {
        let now = std::time::Instant::now();
        if self.last_ask.is_some_and(|t| now.duration_since(t) < MIN_ASK) {
            return false;
        }
        self.last_ask = Some(now);
        self.wake.lock().is_ok_and(|w| w.send(Wake::Now).is_ok())
    }
}

pub fn follow(
    app: &mut App,
    config: &crate::config::Config,
    data_dir: &Path,
    start: Lists,
    shared: Arc<crate::bridge::Shared>,
) -> Result<(), String> {
    let sources = Sources::of(config, data_dir);
    let roots = app.world().get_resource::<crate::discovery::Extras>().map(|e| e.roots.clone()).unwrap_or_default();
    let interval = app.world().get_resource::<Interval>().map_or(REFRESH_INTERVAL, |i| i.0);
    let tls = Arc::new(crate::discovery::heartbeat::tls_config(&roots)?);
    let source: Option<IpAddr> = (!config.bind.is_unspecified()).then_some(config.bind);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_io()
        .enable_time()
        .build()
        .map_err(|e| format!("the blocklists' runtime: {e}"))?;
    let mut follower = Follower { runtime, sources: sources.clone(), local: start.local.clone(), tls, source };
    let (first, fetched) = if sources.master.is_none() {
        (start, Vec::new())
    } else {
        match follower.round() {
            (Some(lists), lines) => (lists, lines),
            (None, lines) => (start, lines),
        }
    };
    for line in &fetched {
        info!("blocklists: {line}");
    }
    if let Some(mut resources) = app.world_mut().get_resource_mut::<crate::resources::ServerResources>() {
        refuse_started(&first.blocked, &[], &resources.running).map_err(|e| format!("[resources] start: {e}"))?;
        resources.set_blocked(Arc::clone(&first.blocked));
    }
    let (send, updates) = std::sync::mpsc::channel();
    let (wake, woken) = std::sync::mpsc::channel();
    std::thread::Builder::new()
        .name("blocklists".into())
        .spawn(move || follower.run(interval, send, woken))
        .map_err(|e| format!("the blocklists' thread: {e}"))?;
    publish_gauges(&shared, &first);
    app.insert_resource(ServerBlocklists {
        sources,
        current: first,
        updates: Mutex::new(updates),
        wake: Mutex::new(wake),
        last_ask: None,
        fetched,
        stopped: 0,
        stopping: Vec::new(),
        shared,
    });
    app.add_systems(Update, apply.before(crate::resources::run_queue));
    Ok(())
}

struct Follower {
    runtime: tokio::runtime::Runtime,
    sources: Sources,
    local: Option<List>,
    tls: Arc<rustls::ClientConfig>,
    source: Option<IpAddr>,
}

impl Follower {
    fn round(&mut self) -> (Option<Lists>, Vec<String>) {
        let now = now_secs();
        let mut outcomes = self.runtime.block_on(fetch::all(&self.sources, &self.tls, self.source, now));
        let loaded = match load(&self.sources, now, self.local.as_ref()) {
            Ok(l) => {
                self.local = l.local.clone().or(self.local.take());
                Some(l)
            }
            Err(e) => {
                outcomes.push(format!("the lists do not load ({e}): the ones applied stay"));
                None
            }
        };
        (loaded, outcomes)
    }

    fn run(mut self, interval: Duration, send: Sender<(Option<Lists>, Vec<String>)>, woken: Receiver<Wake>) {
        loop {
            match woken.recv_timeout(interval) {
                Ok(Wake::Now) | Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }
            if send.send(self.round()).is_err() {
                return;
            }
        }
    }
}

fn publish_gauges(shared: &crate::bridge::Shared, lists: &Lists) {
    *shared.blocklists.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = lists.gauges.clone();
}

pub fn apply(lists: Option<ResMut<ServerBlocklists>>, resources: Option<ResMut<crate::resources::ServerResources>>) {
    let Some(mut lists) = lists else { return };
    let updates: Vec<(Option<Lists>, Vec<String>)> = match lists.updates.lock() {
        Ok(rx) => rx.try_iter().collect(),
        Err(_) => Vec::new(),
    };
    let mut changed = false;
    for (new, fetched) in updates {
        for line in &fetched {
            info!("blocklists: {line}");
        }
        lists.fetched = fetched;
        let Some(new) = new else { continue };
        if new.lines != lists.current.lines {
            for line in &new.lines {
                info!("blocklists: {line}");
            }
        }
        publish_gauges(&lists.shared, &new);
        lists.current = new;
        changed = true;
    }
    let Some(mut resources) = resources else { return };
    if !changed {
        return;
    }
    let blocked = Arc::clone(&lists.current.blocked);
    resources.set_blocked(Arc::clone(&blocked));
    let running: Vec<String> = resources.running.iter().map(|s| s.loaded.name.clone()).collect();
    let now = std::time::Instant::now();
    let mut again = Vec::new();
    lists.stopping.retain(|(n, at)| {
        let late = now.duration_since(*at) >= REQUEUE;
        if late && running.contains(n) {
            again.push(n.clone());
        }
        running.contains(n) && !late
    });
    let mut stops = Vec::new();
    for s in &resources.running {
        let found = hits(&blocked, &s.loaded.hashes);
        if !found.is_empty() && !lists.stopping.iter().any(|(n, _)| *n == s.loaded.name) {
            warn!(
                "blocklists: {}: stopped",
                describe(&format!("the running resource {}", quoted(&s.loaded.name)), &found)
            );
            stops.push(s.loaded.name.clone());
        }
    }
    for name in stops {
        match resources.push(crate::resources::ResourceCommand::Stop(name.clone())) {
            Ok(_) => {
                lists.stopped += u64::from(!again.contains(&name));
                lists.stopping.push((name, now));
            }
            Err(e) => {
                warn!("blocklists: the stop of {} is not queued ({e}); asked again at the next fetch", quoted(&name))
            }
        }
    }
}
