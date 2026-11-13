pub mod config;
mod disc;
mod levels;
mod load;
mod publish;
mod reader;
mod set;
#[cfg(test)]
mod tests;

use std::collections::{BTreeMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard, TryLockError};

use bevy::prelude::*;
use ssx_content::delivery::FileStore;
use ssx_content::version::Version;
use ssx_content::{ContentHash, ContentIndex};
use ssx_core::text::shown;

pub use levels::Ids;
pub use publish::{Published, Snapshot};
pub use set::Started;

pub const STATE_FOLDER: &str = "resource-state";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResourceCommand {
    Start(String),
    Stop(String),
    Restart(String),
    Ensure(String),
    Refresh,
}

impl ResourceCommand {
    pub fn parse(word: &str, rest: &str) -> Option<Result<ResourceCommand, String>> {
        let named = |f: fn(String) -> ResourceCommand| {
            if rest.len() <= config::MAX_NAME
                && ssx_content::id::valid_namespace(rest)
                && !["ssx", "ava"].contains(&rest)
            {
                Ok(f(rest.to_string()))
            } else {
                Err(format!("{word} <resource name>: {:?} is not one", rest.chars().take(40).collect::<String>()))
            }
        };
        Some(match word {
            "start" => named(ResourceCommand::Start),
            "stop" if !rest.is_empty() => named(ResourceCommand::Stop),
            "restart" => named(ResourceCommand::Restart),
            "ensure" => named(ResourceCommand::Ensure),
            "refresh" if rest.is_empty() => Ok(ResourceCommand::Refresh),
            _ => return None,
        })
    }
}

// PORT(choice): 64 waiting commands (an operator types them; a script of RCON lines sends a few); tuning: phase 4b's
// RCON in use; the original has no dedicated server.
pub const MAX_QUEUE: usize = 64;

pub struct Context {
    pub folder: PathBuf,
    pub state: PathBuf,
    pub store: FileStore,
    seq: AtomicU64,
    data_index: std::collections::HashMap<(u64, ContentHash), String>,
    ids: Option<levels::IdsInput>,
    pub mods: config::Mods,
    pub mirrors: Vec<ssx_content::delivery::Mirror>,
    settled: Settled,
    pub blocked: std::sync::RwLock<Arc<ssx_content::blocklist::Blocked>>,
    pub scripts: Option<crate::scripts::ScriptsHandle>,
}

#[derive(Clone, Default)]
pub struct Settled {
    lock: Arc<RwLock<()>>,
    mounts: Arc<AtomicU64>,
}

struct Mounting<'a> {
    _alone: RwLockWriteGuard<'a, ()>,
    mounts: &'a AtomicU64,
}

impl Drop for Mounting<'_> {
    fn drop(&mut self) {
        self.mounts.fetch_add(1, Ordering::AcqRel);
    }
}

impl Settled {
    pub fn reading(&self) -> (RwLockReadGuard<'_, ()>, u64) {
        let held = self.lock.read().unwrap_or_else(PoisonError::into_inner);
        (held, self.mounts.load(Ordering::Acquire))
    }

    pub fn try_reading(&self) -> Option<(RwLockReadGuard<'_, ()>, u64)> {
        let held = match self.lock.try_read() {
            Ok(g) => g,
            Err(TryLockError::Poisoned(p)) => p.into_inner(),
            Err(TryLockError::WouldBlock) => return None,
        };
        Some((held, self.mounts.load(Ordering::Acquire)))
    }

    fn mounting(&self) -> Mounting<'_> {
        Mounting { _alone: self.lock.write().unwrap_or_else(PoisonError::into_inner), mounts: &self.mounts }
    }

    pub fn generation(&self) -> Arc<AtomicU64> {
        Arc::clone(&self.mounts)
    }
}

impl Context {
    pub fn blocked(&self) -> Arc<ssx_content::blocklist::Blocked> {
        Arc::clone(&self.blocked.read().unwrap_or_else(std::sync::PoisonError::into_inner))
    }

    fn next_seq(&self) -> u64 {
        self.seq.fetch_add(1, Ordering::Relaxed)
    }

    fn disc_file(&self, hash: &ContentHash, size: u64) -> Result<Option<String>, String> {
        if size < ssx_assets::content::modtool::check::MIN_REFUSED_BYTES {
            return Ok(None);
        }
        if let Some(what) = disc::disc_file(hash, size)? {
            return Ok(Some(what.to_string()));
        }
        Ok(self.data_index.get(&(size, *hash)).map(|p| format!("{p} (the data folder's index)")))
    }
}

#[derive(Resource, Default, Debug)]
pub struct PermissionRequests {
    pub by_resource: BTreeMap<String, (Version, Vec<String>)>,
    pub generation: u64,
}

#[derive(Resource)]
pub struct ServerResources {
    ctx: Arc<Context>,
    pub running: Vec<Started>,
    pub ids: Ids,
    queue: VecDeque<ResourceCommand>,
    reader: reader::Reader,
    pub published: Arc<Published>,
    pub generation: u64,
    said: Vec<(bool, String)>,
}

pub fn folder(config: &crate::config::Config, data_dir: &Path) -> PathBuf {
    let f = &config.resources.folder;
    if f.is_absolute() { f.clone() } else { data_dir.join(f) }
}

pub fn scan(folder: &Path) -> Result<Vec<String>, String> {
    let entries = match std::fs::read_dir(folder) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(format!("{}: {e}", folder.display())),
    };
    let mut out = Vec::new();
    for e in entries.take(4 * ssx_content::delivery::MAX_RESOURCES) {
        let e = e.map_err(|e| format!("{}: {e}", folder.display()))?;
        let name = e.file_name().to_string_lossy().into_owned();
        let name = name.strip_suffix(".zip").unwrap_or(&name).to_string();
        if name.len() <= config::MAX_NAME && ssx_content::id::valid_namespace(&name) && !out.contains(&name) {
            out.push(name);
        }
    }
    out.sort();
    Ok(out)
}

fn ids_index(data: &Path) -> Result<ContentIndex, String> {
    let text = std::fs::read_to_string(data.join("content.tsv")).map_err(|e| format!("{}: {e}", data.display()))?;
    let mut index = ContentIndex::from_tsv(&text).map_err(|e| format!("content.tsv: {e:?}"))?;
    let known = ssx_assets::subset::read_marker(data)
        .is_some_and(|m| m["selection"]["known_pressing"] == serde_json::json!(true));
    if known {
        let members = ssx_assets::content::data::simid::member_index().map_err(|e| format!("{e:#}"))?;
        for (path, row) in members.entries {
            index.entries.entry(path).or_insert(row);
        }
    }
    Ok(index)
}

pub fn prepare(
    config: &crate::config::Config,
    data_dir: &Path,
    rules: Option<&Path>,
    blocked: Arc<ssx_content::blocklist::Blocked>,
    settled: Settled,
    scripts: Option<crate::scripts::ScriptsHandle>,
) -> Result<ServerResources, String> {
    let state = data_dir.join(STATE_FOLDER);
    let installed = state.join("installed");
    if std::fs::symlink_metadata(&installed).is_ok() {
        std::fs::remove_dir_all(&installed).map_err(|e| format!("{}: {e}", installed.display()))?;
    }
    let store = FileStore::open(&state.join("store")).map_err(|e| e.to_string())?;
    disc::count()?;
    let data = rules.and_then(Path::parent);
    let data_index = data
        .and_then(|d| ContentIndex::from_tsv(&std::fs::read_to_string(d.join("content.tsv")).ok()?).ok())
        .map(|i| i.entries.into_iter().map(|(p, (n, h))| ((n, h), p)).collect())
        .unwrap_or_default();
    let mut problem = None;
    let ids = match (rules, data) {
        (Some(r), Some(d)) => {
            let index = ids_index(d);
            let levels = levels::configured(config);
            let content = ssx_assets::content::GameContent::of(r);
            Some(levels::IdsInput { content, index, levels, mount: Default::default() })
        }
        _ => {
            problem = Some("no rules data".into());
            None
        }
    };
    let ctx = Arc::new(Context {
        folder: folder(config, data_dir),
        state,
        store,
        seq: AtomicU64::new(0),
        data_index,
        ids,
        mods: config.mods,
        mirrors: config.resources.mirrors.clone(),
        settled,
        blocked: std::sync::RwLock::new(blocked),
        scripts,
    });
    let mut running = Vec::new();
    let mut said = Vec::new();
    for name in &config.resources.start {
        if running.iter().any(|s: &Started| s.loaded.name == *name) {
            if let Some(s) = running.iter_mut().find(|s| s.loaded.name == *name) {
                s.explicit = true;
            }
            continue;
        }
        let before = crate::blocklist::dirs(&running);
        let outcome = set::run(&ctx, &ResourceCommand::Start(name.clone()), running)
            .map_err(|e| format!("[resources] start: {e}"))?;
        crate::blocklist::refuse_started(&ctx.blocked(), &before, &outcome.running)
            .map_err(|e| format!("[resources] start: {e}"))?;
        said.extend(outcome.said.iter().map(|l| (false, format!("resources: {l}"))));
        running = outcome.running;
    }
    let ids = match &ctx.ids {
        Some(input) => {
            let _mounting = ctx.settled.mounting();
            levels::compute(input, publish::sources(&running)?)
        }
        None => Ids { problem, ..Ids::default() },
    };
    let published = Arc::new(Published::new(publish::snapshot(&ctx, &running, &ids)?));
    said.extend(ids_lines(&ids));
    if let Some(scripts) = &ctx.scripts {
        let content = ctx.ids.as_ref().map(|i| &*i.content);
        let lines =
            crate::scripts::transact(scripts, content, &[], &running).map_err(|e| format!("[resources] start: {e}"))?;
        said.extend(lines.into_iter().map(|l| (false, format!("resources: {l}"))));
    }
    let reader = reader::Reader::start(Arc::clone(&ctx) as Arc<dyn reader::Reading>)?;
    Ok(ServerResources { ctx, running, ids, queue: VecDeque::new(), reader, published, generation: 0, said })
}

fn ids_lines(ids: &Ids) -> Vec<(bool, String)> {
    match (&ids.content_id, &ids.problem) {
        (Some(id), _) => {
            let mut lines = vec![(false, format!("resources: content id {id} over {} levels", ids.levels.len()))];
            for l in &ids.levels {
                lines.push((false, format!("resources: {} sim content id {}", l.level, l.id)));
            }
            lines
        }
        (None, Some(p)) => {
            vec![(true, format!("resources: no content id ({p}): joining clients' simulation is not compared"))]
        }
        (None, None) => Vec::new(),
    }
}

fn log(lines: Vec<(bool, String)>) {
    for (warning, line) in lines {
        if warning { warn!("{line}") } else { info!("{line}") }
    }
}

impl ServerResources {
    pub fn set_blocked(&mut self, blocked: Arc<ssx_content::blocklist::Blocked>) {
        *self.ctx.blocked.write().unwrap_or_else(std::sync::PoisonError::into_inner) = blocked;
    }

    pub fn push(&mut self, command: ResourceCommand) -> Result<usize, String> {
        if self.queue.len() >= MAX_QUEUE {
            return Err(format!("{} commands wait; this one is not queued", self.queue.len()));
        }
        self.queue.push_back(command);
        Ok(self.queue.len())
    }

    pub fn content(&self) -> Option<&Arc<ssx_assets::content::GameContent>> {
        self.ctx.ids.as_ref().map(|i| &i.content)
    }

    pub fn settled(&self) -> &Settled {
        &self.ctx.settled
    }

    pub fn status(&self, out: &mut Vec<String>) {
        let s = self.published.get();
        out.push(format!(
            "resources: {} running, {} files, {} bytes; content id {}; client_mods {}; client_scripts {}",
            self.running.len(),
            s.files.len(),
            s.manifest.required_bytes(),
            self.ids.content_id.map_or_else(|| "none".to_string(), |c| c.short()),
            self.ctx.mods.client_mods.name(),
            self.ctx.mods.client_scripts,
        ));
        for r in &self.running {
            out.push(format!(
                "  {} {} ({} files)",
                r.loaded.name,
                r.loaded.manifest.resource.version,
                r.loaded.files.len()
            ));
        }
        out.push(format!(
            "resources: the reading thread: {} started ({} replacing one that ended); {} commands waiting",
            self.reader.started(),
            self.reader.started().saturating_sub(1),
            self.queue.len()
        ));
    }
}

pub struct Read {
    outcome: set::Outcome,
    changed: Option<(Ids, Snapshot)>,
}

pub fn publish_now(
    res: Res<ServerResources>,
    (now, stand_ins): (Option<ResMut<ssx_net::server::ResourcesNow>>, Option<ResMut<ssx_net::server::StandIns>>),
    mut seen: Local<Option<u64>>,
) {
    if let Some(mut now) = now {
        let hash = Some(res.published.get().hash.0);
        if now.0 != hash {
            now.0 = hash;
        }
    }
    let Some(mut stand_ins) = stand_ins else { return };
    if *seen == Some(res.generation) {
        return;
    }
    let Some((_settled, _)) = res.settled().try_reading() else { return };
    *seen = Some(res.generation);
    let riders = res.content().map(|content| content.session_stand_ins()).unwrap_or_default();
    let map: std::collections::HashMap<u32, u32> = riders.iter().map(|&(_, key, stand_in)| (key, stand_in)).collect();
    if map.len() != riders.len() {
        warn!("resources: two riders of the running resources share a look key; the validation reads one stand-in");
    }
    if !riders.is_empty() {
        let named: Vec<String> =
            riders.iter().take(8).map(|(id, _, s)| format!("{} as {s:#010x}", shown(&id.to_string()))).collect();
        let more = riders.len().saturating_sub(named.len());
        info!(
            "resources: {} riders of the running resources validated as their stand-ins: {}{}",
            riders.len(),
            named.join(", "),
            if more > 0 { format!(" and {more} more") } else { String::new() }
        );
    }
    if stand_ins.0 != map {
        stand_ins.0 = map;
    }
}

pub fn run_queue(mut res: ResMut<ServerResources>, mut requests: ResMut<PermissionRequests>) {
    let res = &mut *res;
    if !res.said.is_empty() {
        log(std::mem::take(&mut res.said));
    }
    let answer = match res.reader.poll() {
        Some(reader::Answer::Read(command, result)) => Some((command, result.map(|r| *r))),
        Some(reader::Answer::Ended(Some(command), why)) => Some((command, Err(why))),
        Some(reader::Answer::Ended(None, why)) => {
            warn!("resources: the running set was being mounted again: {why}");
            None
        }
        Some(reader::Answer::Remounted(Ok(()))) => {
            info!("resources: the running set is mounted again after the reading thread ended");
            None
        }
        Some(reader::Answer::Remounted(Err(why))) => {
            warn!(
                "resources: the running set does not mount again ({}); the next command's mount replaces it",
                shown(&why)
            );
            None
        }
        None => None,
    };
    if let Some((command, result)) = answer {
        let was = res.ids.content_id;
        let was_package = match &command {
            ResourceCommand::Restart(name) => {
                res.running.iter().find(|s| s.loaded.name == *name).map(|s| s.loaded.package)
            }
            _ => None,
        };
        match (result, &command) {
            (Err(e), ResourceCommand::Restart(name)) => warn!("{}", restart_failed(name, &res.running, &e)),
            (Err(e), _) => warn!("resources: {command:?}: {e}"),
            (Ok(read), _) => {
                apply(res, &mut requests, read);
                if let ResourceCommand::Restart(name) = &command {
                    info!("{}", restart_done(name, &res.running, (res.ids.content_id, was), was_package));
                }
            }
        }
    }
    if res.reader.busy() {
        return;
    }
    let now = std::time::Instant::now();
    if res.reader.needs_remount() {
        if let Err(why) = res.reader.remount(res.running.clone(), now)
            && !res.queue.is_empty()
            && let Some(command) = res.queue.pop_front()
        {
            refused(&command, &res.running, &why);
        }
        return;
    }
    let Some(command) = res.queue.pop_front() else { return };
    let (running, had_ids) = (res.running.clone(), res.ids.content_id.is_some());
    if let Err((command, why)) = res.reader.read(command, running, had_ids, now) {
        refused(&command, &res.running, &why);
    }
}

fn refused(command: &ResourceCommand, running: &[Started], why: &str) {
    match command {
        ResourceCommand::Restart(name) => warn!("{}", restart_failed(name, running, why)),
        _ => warn!("resources: {command:?}: {}", shown(why)),
    }
}

impl reader::Reading for Context {
    fn read(&self, command: &ResourceCommand, running: Vec<Started>, had_ids: bool) -> Result<Read, String> {
        read(self, command, running, had_ids)
    }

    fn remount(&self, running: &[Started]) -> Result<(), String> {
        let keep: Vec<&std::path::Path> = running.iter().map(|s| s.loaded.dir.as_path()).collect();
        let installed = self.state.join("installed");
        let mut problems = Vec::new();
        if let Ok(entries) = std::fs::read_dir(&installed) {
            for path in entries.flatten().map(|e| e.path()).filter(|p| !keep.contains(&p.as_path())) {
                if let Err(e) = std::fs::remove_dir_all(&path) {
                    problems.push(format!("{}: {e}", path.display()));
                }
            }
        }
        if let Some(input) = &self.ids {
            let sources = publish::sources(running)?;
            let _mounting = self.settled.mounting();
            drop(levels::compute(input, sources));
        }
        if problems.is_empty() { Ok(()) } else { Err(format!("a copy stays: {}", problems.join("; "))) }
    }
}

pub(crate) fn restart_done(
    name: &str,
    running: &[set::Started],
    ids: (Option<ssx_content::data::SimContentId>, Option<ssx_content::data::SimContentId>),
    was_package: Option<ssx_content::ContentHash>,
) -> String {
    let id = |i: Option<ssx_content::data::SimContentId>| i.map_or("none".to_string(), |i| i.to_string());
    let short = |h: Option<ssx_content::ContentHash>| h.map_or("none".to_string(), |h| h.to_string()[..16].to_string());
    let runs = match running.iter().find(|s| s.loaded.name == name) {
        Some(s) => format!(
            "{name} {} (package {}, was {}) runs",
            s.loaded.manifest.resource.version,
            short(Some(s.loaded.package)),
            short(was_package)
        ),
        None => format!("{name} does not run"),
    };
    format!(
        "resources: restart {name} done: {runs}; the server's content id {} (was {}); commands typed from here on run \
         on it",
        id(ids.0),
        id(ids.1)
    )
}

pub(crate) fn restart_failed(name: &str, running: &[set::Started], why: &str) -> String {
    match running.iter().find(|s| s.loaded.name == name) {
        Some(s) => format!(
            "resources: restart {name} failed, the running {name} {} stays (commands keep running on it): {}",
            s.loaded.manifest.resource.version,
            shown(why)
        ),
        None => format!("resources: restart {name} failed, nothing changed: {}", shown(why)),
    }
}

fn read(ctx: &Context, command: &ResourceCommand, running: Vec<Started>, had_ids: bool) -> Result<Read, String> {
    let before = running.clone();
    let o = set::run(ctx, command, running)?;
    if !o.changed {
        return Ok(Read { outcome: o, changed: None });
    }
    if let Err(why) = crate::blocklist::refuse_started(&ctx.blocked(), &crate::blocklist::dirs(&before), &o.running) {
        crate::blocklist::remove_new_copies(&before, &o.running);
        return Err(format!("refused: {why}"));
    }
    let _mounting = ctx.settled.mounting();
    let undo = |o: &set::Outcome, why: String| {
        if let Some(input) = &ctx.ids {
            match publish::sources(&before) {
                Ok(s) => drop(levels::compute(input, s)),
                Err(e) => warn!("resources: the running set does not mount again for the ids: {e}"),
            }
        }
        let old: Vec<&PathBuf> = before.iter().map(|s| &s.loaded.dir).collect();
        for s in o.running.iter().filter(|s| !old.contains(&&s.loaded.dir)) {
            let _ = std::fs::remove_dir_all(&s.loaded.dir);
        }
        format!("refused: {why}")
    };
    let ids = match &ctx.ids {
        Some(input) => match publish::sources(&o.running) {
            Ok(s) => levels::compute(input, s),
            Err(e) => return Err(undo(&o, e)),
        },
        None => Ids::default(),
    };
    if let (true, Some(problem)) = (had_ids, &ids.problem) {
        return Err(undo(&o, format!("with it the levels' ids do not compute ({problem})")));
    }
    match publish::snapshot(ctx, &o.running, &ids) {
        Ok(snapshot) => {
            let mut o = o;
            if let Some(scripts) = &ctx.scripts {
                let content = ctx.ids.as_ref().map(|i| &*i.content);
                match crate::scripts::transact(scripts, content, &before, &o.running) {
                    Ok(lines) => o.said.extend(lines),
                    Err(e) => return Err(undo(&o, format!("its scripts: {e}"))),
                }
            }
            Ok(Read { outcome: o, changed: Some((ids, snapshot)) })
        }
        Err(e) => Err(undo(&o, e)),
    }
}

fn apply(res: &mut ServerResources, requests: &mut PermissionRequests, read: Read) {
    let Read { outcome, changed } = read;
    for line in &outcome.said {
        info!("resources: {line}");
    }
    let Some((ids, snapshot)) = changed else { return };
    res.published.set(snapshot);
    for dir in &outcome.removed {
        let _ = std::fs::remove_dir_all(dir);
    }
    let keep = res.published.get().files.keys().copied().collect();
    if let Err(e) = res.ctx.store.trim(0, &keep) {
        warn!("resources: the file store: {e}");
    }
    requests.by_resource.clear();
    for s in &outcome.running {
        let p = &s.loaded.manifest.permissions.request;
        if !p.is_empty() {
            requests.by_resource.insert(s.loaded.name.clone(), (s.loaded.manifest.resource.version, p.clone()));
        }
    }
    requests.generation += 1;
    res.running = outcome.running;
    res.ids = ids;
    res.generation += 1;
    log(ids_lines(&res.ids));
}
