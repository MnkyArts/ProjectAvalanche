pub mod fetch;
pub mod source;

use std::collections::BTreeSet;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ssx_content::ContentHash;
use ssx_content::delivery::{self, ClientManifest, FileStore, Mount, StoreError};

use fetch::Miss;
use source::Source;

pub const CACHE_FOLDER: &str = "ResourceCache";
// PORT(choice): 8 GiB (four servers' worth of the 2 GiB a package may be); tuning: the cache sizes players keep; the
// original has no dedicated server.
pub const DEFAULT_QUOTA: u64 = 8 << 30;
// PORT(choice): 2 GiB (one package at its cap; M8b's level budget is 512 MiB of models and 1 GiB of images); tuning:
// what players set; the original has no dedicated server.
pub const DEFAULT_LIMIT: u64 = 2 << 30;
// PORT(choice): 15 s (8 MiB at about 0.5 MiB/s); tuning: real servers' manifests; the original has no dedicated server.
pub const MANIFEST_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Clone, Debug)]
pub struct ResourceOptions {
    pub cache: PathBuf,
    pub quota: u64,
    pub limit: u64,
    pub blocklists: ssx_content::blocklist::Follow,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ResourceLine {
    pub name: String,
    pub title: String,
    pub files: usize,
    pub bytes: u64,
    pub cached: u64,
    pub done: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u8)]
pub enum Step {
    #[default]
    Reading = 0,
    Fetching = 1,
    CheckingFiles = 2,
    CheckingContent = 3,
}

impl Step {
    const ALL: [Step; 4] = [Step::Reading, Step::Fetching, Step::CheckingFiles, Step::CheckingContent];
}

#[derive(Debug, Default)]
pub struct Progress {
    pub lines: Mutex<Vec<ResourceLine>>,
    pub total: AtomicU64,
    pub received: AtomicU64,
    step: AtomicU8,
    pub blocklists: Mutex<Vec<String>>,
}

impl Progress {
    pub fn step(&self) -> Step {
        Step::ALL.get(usize::from(self.step.load(Ordering::Relaxed))).copied().unwrap_or_default()
    }

    pub(crate) fn say(&self, step: Step) {
        self.step.store(step as u8, Ordering::Relaxed);
    }
}

pub struct ServerManifest {
    pub manifest: ClientManifest,
    pub hash: ContentHash,
    pub body: Vec<u8>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Need {
    Exactly(u64),
    Between { least: u64, most: u64 },
}

impl Need {
    pub fn over(self, limit: u64) -> bool {
        match self {
            Need::Exactly(n) => n > limit,
            Need::Between { least, .. } => least > limit,
        }
    }

    pub fn shown(self) -> u64 {
        match self {
            Need::Exactly(n) | Need::Between { most: n, .. } => n,
        }
    }
}

pub fn need(store: &FileStore, hash: &ContentHash, size: u64, required: u64, held: u64) -> Need {
    let between = Need::Between { least: required.saturating_sub(held), most: required };
    if size > delivery::MAX_BODY as u64 || !store.has(hash, size) {
        return between;
    }
    match store.read(hash, size).ok().and_then(|body| delivery::from_json(&body).ok()) {
        Some(m) => Need::Exactly(missing_bytes(&m, store)),
        None => between,
    }
}

pub struct CacheView {
    root: PathBuf,
    held: Mutex<Option<(std::time::Instant, u64)>>,
    blocklists: ssx_content::blocklist::Follow,
    lists: Mutex<Option<(std::time::Instant, Arc<ssx_content::blocklist::Blocked>)>>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Look {
    pub need: Option<Need>,
    pub blocked: Option<String>,
}

// PORT(choice): 30 s (a list's rows are asked every 10 s; a download in another game changes the count meanwhile, which
// shifts only a server's lower bound), the browser's; the original has no dedicated server.
pub const HELD_INTERVAL: Duration = Duration::from_secs(30);

impl CacheView {
    pub fn new(root: PathBuf, blocklists: ssx_content::blocklist::Follow) -> Self {
        Self { root, held: Mutex::new(None), blocklists, lists: Mutex::new(None) }
    }

    pub fn look(&self, summary: &ssx_net_proto::resources::ResourceSummary) -> Look {
        let Some((manifest, store)) = self.kept_manifest(summary) else {
            return Look { need: self.need(summary), blocked: None };
        };
        let blocked = crate::blocklist::join::reason(&crate::blocklist::join::hits(&manifest, &self.lists()));
        Look { need: Some(Need::Exactly(missing_bytes(&manifest, &store))), blocked }
    }

    fn kept_manifest(
        &self,
        summary: &ssx_net_proto::resources::ResourceSummary,
    ) -> Option<(ClientManifest, FileStore)> {
        if summary.count == 0 || summary.manifest_bytes > delivery::MAX_BODY as u64 {
            return None;
        }
        let hash = ContentHash(summary.manifest_hash().ok()?);
        let store = FileStore::open(&self.root).ok()?;
        if !store.has(&hash, summary.manifest_bytes) {
            return None;
        }
        let manifest = store.read(&hash, summary.manifest_bytes).ok().and_then(|b| delivery::from_json(&b).ok())?;
        Some((manifest, store))
    }

    fn lists(&self) -> Arc<ssx_content::blocklist::Blocked> {
        let mut l = self.lists.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        match &*l {
            Some((at, b)) if at.elapsed() < HELD_INTERVAL => Arc::clone(b),
            _ => {
                let b = Arc::new(self.blocklists.load(ssx_content::blocklist::now_secs()).blocked);
                *l = Some((std::time::Instant::now(), Arc::clone(&b)));
                b
            }
        }
    }

    pub fn need(&self, summary: &ssx_net_proto::resources::ResourceSummary) -> Option<Need> {
        if summary.count == 0 {
            return None;
        }
        let hash = ContentHash(summary.manifest_hash().ok()?);
        let store = FileStore::open(&self.root).ok()?;
        let held = {
            let mut h = self.held.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            match *h {
                Some((at, bytes)) if at.elapsed() < HELD_INTERVAL => bytes,
                _ => {
                    let bytes = store.bytes().ok()?;
                    *h = Some((std::time::Instant::now(), bytes));
                    bytes
                }
            }
        };
        Some(need(&store, &hash, summary.manifest_bytes, summary.download, held))
    }
}

fn keep_manifest(store: &FileStore, hash: &ContentHash, body: &[u8]) -> Result<(), StoreError> {
    let size = body.len() as u64;
    if store.has(hash, size) {
        return Ok(());
    }
    let mut p = match store.partial(hash) {
        Ok(p) => p,
        Err(StoreError::Busy(_)) => return Ok(()),
        Err(e) => return Err(e),
    };
    p.restart()?;
    p.append(body)?;
    store.confirm(p, size)
}

pub struct Fetched {
    pub manifest: ClientManifest,
    pub manifest_hash: ContentHash,
    pub mount: Mount,
    pub packages: Vec<(String, PathBuf)>,
    pub fetched_bytes: u64,
    pub cached_bytes: u64,
    pub content_id: Option<ssx_content::data::SimContentId>,
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(MANIFEST_TIMEOUT))
        .timeout_connect(Some(source::CONNECT_TIMEOUT))
        .max_response_header_size(8 * 1024)
        .http_status_as_error(false)
        .max_redirects(0)
        .proxy(None)
        .user_agent(ssx_net_proto::BUILD_ID)
        .build()
        .into()
}

pub const MAX_RETRY_AFTER: Duration = Duration::from_secs(8);

pub fn fetch_manifest(server: SocketAddr, cancel: &AtomicBool) -> Result<Option<ServerManifest>, String> {
    let (mut last, mut asked) = (String::new(), None);
    for attempt in 0..fetch::ATTEMPTS {
        if attempt > 0 && !fetch::wait(retry_wait(attempt, asked), cancel) {
            return Err("cancelled".into());
        }
        asked = None;
        match agent().get(&format!("http://{server}/resources")).call() {
            Ok(response) => match response.status().as_u16() {
                200 => return read_manifest(response).map(Some),
                404 => return Ok(None),
                s @ (429 | 503) => {
                    let header = response.headers().get("retry-after");
                    asked = header.and_then(|v| v.to_str().ok()?.parse::<u64>().ok()).map(Duration::from_secs);
                    last = format!("the server answered {s} for its resources (busy)");
                }
                s => return Err(format!("the server answered {s} for its resources")),
            },
            Err(e) => last = format!("the server's resources cannot be had: {e}"),
        }
    }
    Err(last)
}

pub fn join_content(
    server: SocketAddr,
    cancel: &AtomicBool,
    levels_id: impl FnOnce(&ClientManifest) -> Result<Option<ssx_content::data::SimContentId>, String>,
) -> Result<ssx_net_proto::join::JoinContent, String> {
    let Some(m) = fetch_manifest(server, cancel)? else { return Ok(ssx_net_proto::join::JoinContent::default()) };
    if !m.manifest.resources.is_empty() {
        return Err(format!(
            "the server runs {} resources, and this client fetches none (the game joins it)",
            m.manifest.resources.len()
        ));
    }
    let content_id = match m.manifest.content_id {
        Some(_) => levels_id(&m.manifest)?.map(|c| c.0),
        None => None,
    };
    Ok(ssx_net_proto::join::JoinContent { content_id, manifest: Some(m.hash.0) })
}

fn retry_wait(attempt: u32, asked: Option<Duration>) -> Duration {
    let schedule = fetch::FIRST_WAIT * (1 << (attempt - 1));
    asked.map_or(schedule, |a| a.min(MAX_RETRY_AFTER).max(schedule))
}

fn read_manifest(mut response: ureq::http::Response<ureq::Body>) -> Result<ServerManifest, String> {
    let max = delivery::MAX_BODY as u64;
    let body = response
        .body_mut()
        .with_config()
        .limit(max + 1)
        .read_to_vec()
        .map_err(|e| format!("the server's resource manifest: {e}"))?;
    if body.len() as u64 > max {
        return Err(format!("the server's resource manifest is larger than {max} bytes"));
    }
    let manifest = delivery::from_json(&body).map_err(|e| e.to_string())?;
    Ok(ServerManifest { manifest, hash: ContentHash::of(&body), body })
}

pub fn missing_bytes(manifest: &ClientManifest, store: &FileStore) -> u64 {
    let mut seen = BTreeSet::new();
    let files = manifest.resources.iter().flat_map(|r| &r.files).filter(|f| !f.lazy);
    files.filter(|f| seen.insert(f.hash) && !store.has(&f.hash, f.size)).map(|f| f.size).sum()
}

pub fn fetch_all(
    server: SocketAddr,
    m: ServerManifest,
    opts: &ResourceOptions,
    progress: &Progress,
    cancel: &Arc<AtomicBool>,
) -> Result<Fetched, String> {
    let store = FileStore::open(&opts.cache).map_err(|e| e.to_string())?;
    keep_manifest(&store, &m.hash, &m.body).map_err(|e| format!("the resource cache: {e}"))?;
    let ServerManifest { manifest, hash: manifest_hash, .. } = m;
    let checked = crate::blocklist::join::check(&manifest, &opts.blocklists, &store, cancel);
    progress.blocklists.lock().unwrap_or_else(std::sync::PoisonError::into_inner).extend(checked.asked);
    checked.verdict?;
    let needed = missing_bytes(&manifest, &store);
    if needed > opts.limit {
        return Err(format!(
            "this server sends {} MB its players do not have; your limit is {} MB (Options)",
            needed.div_ceil(1 << 20),
            opts.limit >> 20
        ));
    }
    progress.total.store(needed, Ordering::Relaxed);
    let lines: Vec<ResourceLine> = manifest
        .resources
        .iter()
        .map(|r| {
            let files = r.files.iter().filter(|f| !f.lazy);
            let (bytes, cached) =
                files.fold((0, 0), |(b, c), f| (b + f.size, c + if store.has(&f.hash, f.size) { f.size } else { 0 }));
            ResourceLine {
                name: r.name.clone(),
                title: r.title.clone(),
                files: r.files.len(),
                bytes,
                cached,
                done: false,
            }
        })
        .collect();
    *progress.lines.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = lines;
    let mut sources: Vec<Source> =
        manifest.mirrors.iter().cloned().enumerate().map(|(i, m)| Source::Mirror(i, m)).collect();
    sources.push(Source::Server(server));
    let mut dead = vec![false; sources.len()];
    let required = manifest.required_bytes();
    let mut groups: Vec<u32> = manifest.resources.iter().map(|r| r.group).collect();
    groups.sort_unstable();
    groups.dedup();
    let received = AtomicU64::new(0);
    for g in groups {
        for (i, r) in manifest.resources.iter().enumerate().filter(|(_, r)| r.group == g) {
            progress.say(Step::Fetching);
            for f in r.files.iter().filter(|f| !f.lazy) {
                if store.has(&f.hash, f.size) {
                    continue;
                }
                let mut sh = fetch::Shared { server, store: &store, cancel, received: &received, dead: &mut dead };
                match fetch::fetch(f, &sources, &mut sh) {
                    Ok(()) => {}
                    Err(Miss::Cancelled) => return Err("cancelled".into()),
                    Err(Miss::Store(e)) => return Err(format!("the resource cache: {e}")),
                    Err(Miss::Again(why) | Miss::NotTheFile(why) | Miss::Elsewhere(why)) => {
                        return Err(format!("{}: {why}", r.name));
                    }
                }
                progress.received.store(received.load(Ordering::Relaxed), Ordering::Relaxed);
            }
            if let Some(line) = progress.lines.lock().unwrap_or_else(std::sync::PoisonError::into_inner).get_mut(i) {
                line.done = true;
            }
        }
    }
    progress.say(Step::CheckingFiles);
    let mount = store.mount().map_err(|e| e.to_string())?;
    let mut packages = Vec::new();
    for r in &manifest.resources {
        if cancel.load(Ordering::Acquire) {
            return Err("cancelled".into());
        }
        let files: Vec<(&str, ContentHash, u64)> =
            r.files.iter().filter(|f| !f.lazy).map(|f| (f.path.as_str(), f.hash, f.size)).collect();
        let dir = match mount.lay_out(&store, &r.name, &files) {
            Ok(dir) => dir,
            Err(StoreError::Mismatch(hash)) => {
                let Some(f) = r.files.iter().find(|f| f.hash == hash) else {
                    return Err(format!("{}: {}", r.name, StoreError::Mismatch(hash)));
                };
                progress.say(Step::Fetching);
                let mut sh = fetch::Shared { server, store: &store, cancel, received: &received, dead: &mut dead };
                match fetch::fetch(f, &sources, &mut sh) {
                    Ok(()) => {}
                    Err(Miss::Cancelled) => return Err("cancelled".into()),
                    Err(Miss::Store(e)) => return Err(format!("the resource cache: {e}")),
                    Err(Miss::Again(why) | Miss::NotTheFile(why) | Miss::Elsewhere(why)) => {
                        return Err(format!("{}: {why}", r.name));
                    }
                }
                progress.say(Step::CheckingFiles);
                mount.lay_out(&store, &r.name, &files).map_err(|e| format!("{}: {e}", r.name))?
            }
            Err(e) => return Err(format!("{}: {e}", r.name)),
        };
        packages.push((r.name.clone(), dir));
    }
    let mut keep: BTreeSet<ContentHash> = manifest.resources.iter().flat_map(|r| &r.files).map(|f| f.hash).collect();
    keep.insert(manifest_hash);
    store.trim(opts.quota, &keep).map_err(|e| format!("the resource cache: {e}"))?;
    let fetched_bytes = received.load(Ordering::Relaxed);
    let cached_bytes = required.saturating_sub(needed);
    Ok(Fetched { manifest, manifest_hash, mount, packages, fetched_bytes, cached_bytes, content_id: None })
}

#[cfg(test)]
mod tests {
    use super::*;
    use ssx_content::delivery::{File, FileRole, Resource};

    fn manifest(files: &[(&str, &[u8], bool)]) -> ClientManifest {
        let files = files
            .iter()
            .map(|&(path, bytes, lazy)| File {
                path: path.into(),
                hash: ContentHash::of(bytes),
                size: bytes.len() as u64,
                role: if path == "resource.toml" { FileRole::Manifest } else { FileRole::Content },
                lazy,
            })
            .collect();
        ClientManifest {
            client_mods: ssx_content::policy::ClientMods::Presentation,
            client_scripts: false,
            mirrors: Vec::new(),
            content_id: None,
            resources: vec![Resource {
                name: "skins".into(),
                version: "1.0.0".parse().unwrap(),
                title: "Skins".into(),
                group: 0,
                scripts: false,
                files,
            }],
            levels: Vec::new(),
        }
    }

    fn put(store: &FileStore, bytes: &[u8]) {
        store.put(&ContentHash::of(bytes), bytes.len() as u64, &mut &bytes[..]).unwrap();
    }

    #[test]
    fn the_browser_knows_what_a_server_would_fetch() {
        let dir = std::env::temp_dir().join(format!("ssx-net-join-need-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let store = FileStore::open(&dir).unwrap();
        let m = manifest(&[("resource.toml", b"[resource]", false), ("a.png", b"aaaa", false), ("b.ogg", b"bb", true)]);
        let body = m.to_json().into_bytes();
        let hash = ContentHash::of(&body);
        let size = body.len() as u64;
        assert_eq!(need(&store, &hash, size, 14, 0), Need::Between { least: 14, most: 14 }, "never seen");
        assert_eq!(need(&store, &hash, size, 14, 10), Need::Between { least: 4, most: 14 });
        keep_manifest(&store, &hash, &body).unwrap();
        keep_manifest(&store, &hash, &body).unwrap();
        assert_eq!(need(&store, &hash, size, 14, 0), Need::Exactly(14));
        put(&store, b"aaaa");
        assert_eq!(need(&store, &hash, size, 14, 0), Need::Exactly(10), "a.png held; b.ogg lazy");
        put(&store, b"[resource]");
        let n = need(&store, &hash, size, 14, 0);
        assert_eq!((n, n.over(0), n.shown()), (Need::Exactly(0), false, 0), "a reconnect fetches nothing");
        assert!(matches!(need(&store, &hash, size + 1, 14, 0), Need::Between { .. }));
        assert!(matches!(need(&store, &hash, delivery::MAX_BODY as u64 + 1, 14, 0), Need::Between { .. }));
        assert!(matches!(need(&store, &ContentHash::of(b"aaaa"), 4, 14, 0), Need::Between { .. }));
        let n = Need::Between { least: 3 << 20, most: 9 << 20 };
        assert!(n.over(2 << 20) && !n.over(3 << 20) && n.shown() == 9 << 20);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_busy_servers_wait() {
        let ms = Duration::from_millis;
        let waits: Vec<Duration> = (1..fetch::ATTEMPTS).map(|a| retry_wait(a, None)).collect();
        assert_eq!(waits, [ms(500), ms(1000), ms(2000), ms(4000)]);
        assert_eq!(retry_wait(1, Some(Duration::from_secs(1))), ms(1000));
        assert_eq!(retry_wait(4, Some(Duration::from_secs(1))), ms(4000));
        assert_eq!(retry_wait(1, Some(Duration::from_secs(3600))), MAX_RETRY_AFTER);
    }
}
