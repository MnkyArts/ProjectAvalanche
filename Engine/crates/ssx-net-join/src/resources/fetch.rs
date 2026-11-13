use std::io::Read;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use ssx_content::delivery::range::parse_content_range;
use ssx_content::delivery::{File, FileStore, Partial, StoreError};

use super::source::{Source, agent};

pub const ATTEMPTS: u32 = 5;
pub const FIRST_WAIT: Duration = Duration::from_millis(500);
// PORT(choice): 64 MiB a request (a map's largest file is about 200 MiB of models); tuning: downloads of players on
// public servers; the original has no dedicated server.
pub const MAX_REQUEST_BYTES: u64 = 64 << 20;
// PORT(choice): 32 KiB/s and 10 s (the server's `MIN_FILE_RATE` and connection time); tuning: as the server's; the
// original has no dedicated server.
pub const MIN_RATE: u64 = 32 << 10;
pub const BODY_GRACE: Duration = Duration::from_secs(10);
const READ: usize = 64 << 10;
// PORT(choice): 30 s (a source's five attempts wait 7.5 s in all, its connection 5 s, a request's body 10 s before its
// rate counts); tuning: two game processes joining on one line; the original has no dedicated server.
pub const OTHER_STALL: Duration = Duration::from_secs(30);
const OTHER_POLL: Duration = Duration::from_millis(100);

#[derive(Debug)]
pub enum Miss {
    Again(String),
    NotTheFile(String),
    Elsewhere(String),
    Cancelled,
    Store(StoreError),
}

pub struct Shared<'a> {
    pub server: SocketAddr,
    pub store: &'a FileStore,
    pub cancel: &'a Arc<AtomicBool>,
    pub received: &'a AtomicU64,
    pub dead: &'a mut Vec<bool>,
}

fn cancelled(cancel: &AtomicBool) -> bool {
    cancel.load(Ordering::Acquire)
}

pub(crate) fn wait(d: Duration, cancel: &AtomicBool) -> bool {
    let end = Instant::now() + d;
    while Instant::now() < end {
        if cancelled(cancel) {
            return false;
        }
        std::thread::sleep(Duration::from_millis(50).min(end.saturating_duration_since(Instant::now())));
    }
    !cancelled(cancel)
}

fn take(file: &File, sh: &Shared) -> Result<Option<Partial>, Miss> {
    let (mut seen, mut since) = (None, Instant::now());
    loop {
        match sh.store.partial(&file.hash) {
            Ok(part) if sh.store.has(&file.hash, file.size) => {
                part.discard();
                return Ok(None);
            }
            Ok(part) => return Ok(Some(part)),
            Err(StoreError::Busy(_)) => {}
            Err(e) => return Err(Miss::Store(e)),
        }
        let now = sh.store.partial_len(&file.hash);
        if now != seen {
            (seen, since) = (now, Instant::now());
        } else if since.elapsed() >= OTHER_STALL {
            return Err(Miss::Store(StoreError::Busy(file.hash)));
        }
        if !wait(OTHER_POLL, sh.cancel) {
            return Err(Miss::Cancelled);
        }
    }
}

fn request(source: &Source, file: &File, part: &mut Partial, sh: &Shared) -> Result<(), Miss> {
    let have = part.len().map_err(Miss::Store)?;
    let want = file.size.saturating_sub(have).min(MAX_REQUEST_BYTES);
    if want == 0 {
        return Ok(());
    }
    let budget = BODY_GRACE + Duration::from_secs_f64(want as f64 / MIN_RATE as f64);
    let agent = agent(source, sh.server, Some(Arc::clone(sh.cancel)));
    let mut req = agent.get(&source.url(&file.hash)).config().timeout_recv_body(Some(budget)).build();
    if have > 0 || want < file.size {
        req = req.header("range", &format!("bytes={have}-{}", have + want - 1));
    }
    let mut response = req.call().map_err(|e| Miss::Again(e.to_string()))?;
    let status = response.status().as_u16();
    let from = match status {
        200 => 0,
        206 => {
            let cr = response.headers().get("content-range").map(|v| v.as_bytes().to_vec()).unwrap_or_default();
            match parse_content_range(&cr) {
                Some((r, size)) if r.first == have && size == file.size && r.len <= want => have,
                _ => return Err(Miss::NotTheFile("a range answer for another range or size".into())),
            }
        }
        404 | 403 | 410 | 416 => return Err(Miss::Elsewhere(format!("it answered {status}"))),
        _ => return Err(Miss::Again(format!("it answered {status}"))),
    };
    if from == 0 && have > 0 {
        part.restart().map_err(Miss::Store)?;
    }
    let limit = if from == 0 { file.size } else { want };
    let mut body = response.body_mut().as_reader();
    let mut buf = vec![0u8; READ];
    let mut got = 0u64;
    loop {
        if cancelled(sh.cancel) {
            return Err(Miss::Cancelled);
        }
        let n = body.read(&mut buf).map_err(|e| Miss::Again(e.to_string()))?;
        if n == 0 {
            break;
        }
        got += n as u64;
        if got > limit {
            return Err(Miss::NotTheFile(format!("more than the {limit} bytes asked for")));
        }
        part.append(&buf[..n]).map_err(Miss::Store)?;
        sh.received.fetch_add(n as u64, Ordering::Relaxed);
    }
    if got < limit {
        return Err(Miss::Again(format!("it sent {got} of the {limit} bytes asked for")));
    }
    Ok(())
}

fn report(sh: &Shared, file: &File, source: &Source) {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(5)))
        .max_redirects(0)
        .proxy(None)
        .http_status_as_error(false)
        .build()
        .into();
    let body = format!("{} {}", file.hash, source.report_name());
    let _ = agent.post(&format!("http://{}/report", sh.server)).send(body.as_bytes());
}

pub fn fetch(file: &File, sources: &[Source], sh: &mut Shared) -> Result<(), Miss> {
    if file.size == 0 {
        let Some(mut part) = take(file, sh)? else { return Ok(()) };
        part.restart().map_err(Miss::Store)?;
        return sh.store.confirm(part, 0).map_err(|e| Miss::Elsewhere(format!("{} ({}): {e}", file.path, file.hash)));
    }
    let mut reasons = Vec::new();
    for (i, source) in sources.iter().enumerate() {
        if sh.dead[i] {
            continue;
        }
        let mut attempt = 0;
        loop {
            if cancelled(sh.cancel) {
                return Err(Miss::Cancelled);
            }
            let Some(mut part) = take(file, sh)? else { return Ok(()) };
            if part.len().map_err(Miss::Store)? >= file.size {
                match sh.store.confirm(part, file.size) {
                    Ok(()) => return Ok(()),
                    Err(StoreError::Mismatch(_) | StoreError::Size { .. }) => continue,
                    Err(e) => return Err(Miss::Store(e)),
                }
            }
            let miss = match request(source, file, &mut part, sh) {
                Ok(()) if part.len().map_err(Miss::Store)? == file.size => match sh.store.confirm(part, file.size) {
                    Ok(()) => return Ok(()),
                    Err(StoreError::Mismatch(_) | StoreError::Size { .. }) => {
                        Miss::NotTheFile("its bytes are not the file's hash".into())
                    }
                    Err(e) => return Err(Miss::Store(e)),
                },
                Ok(()) => {
                    attempt = 0;
                    continue;
                }
                Err(Miss::NotTheFile(why)) => {
                    part.restart().map_err(Miss::Store)?;
                    Miss::NotTheFile(why)
                }
                Err(e) => e,
            };
            match miss {
                Miss::Again(why) => {
                    attempt += 1;
                    if attempt >= ATTEMPTS {
                        reasons.push(format!("{}: {why}", source.shown()));
                        sh.dead[i] = true;
                        break;
                    }
                    if !wait(FIRST_WAIT * 2u32.pow(attempt - 1), sh.cancel) {
                        return Err(Miss::Cancelled);
                    }
                }
                Miss::NotTheFile(why) => {
                    report(sh, file, source);
                    reasons.push(format!("{}: {why} (reported)", source.shown()));
                    break;
                }
                Miss::Elsewhere(why) => {
                    reasons.push(format!("{}: {why}", source.shown()));
                    break;
                }
                other => return Err(other),
            }
        }
    }
    Err(Miss::Elsewhere(format!("{} ({}): {}", file.path, file.hash, reasons.join("; "))))
}

#[cfg(test)]
mod tests {
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;
    use std::sync::atomic::AtomicUsize;

    use ssx_content::ContentHash;
    use ssx_content::delivery::FileRole;

    use super::*;

    #[test]
    fn a_source_that_sends_a_byte_at_a_time_is_dropped() {
        let bytes: Vec<u8> = (0..4096u32).map(|i| (i * 7 % 251) as u8).collect();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let requests = Arc::new(AtomicUsize::new(0));
        let (served, body) = (Arc::clone(&requests), bytes.clone());
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut s) = stream else { return };
                let mut reader = BufReader::new(s.try_clone().unwrap());
                let (mut line, mut first) = (String::new(), 0usize);
                while reader.read_line(&mut line).is_ok_and(|n| n > 2) {
                    if let Some(r) = line.to_ascii_lowercase().strip_prefix("range: bytes=") {
                        first = r.split('-').next().and_then(|a| a.parse().ok()).unwrap_or(0);
                    }
                    line.clear();
                }
                served.fetch_add(1, Ordering::SeqCst);
                let head = format!(
                    "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes {first}-{first}/{}\r\nContent-Length: 1\r\n\
                     Connection: close\r\n\r\n",
                    body.len()
                );
                let _ = s.write_all(head.as_bytes());
                let _ = s.write_all(&body[first..first + 1]);
            }
        });
        let dir = std::env::temp_dir().join(format!("ssx-net-join-drip-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let store = FileStore::open(&dir).unwrap();
        let hash = ContentHash::of(&bytes);
        let file = File { path: "a.png".into(), hash, size: bytes.len() as u64, role: FileRole::Content, lazy: false };
        let (cancel, received, mut dead) = (Arc::new(AtomicBool::new(false)), AtomicU64::new(0), vec![false]);
        let mut sh = Shared { server: addr, store: &store, cancel: &cancel, received: &received, dead: &mut dead };
        let r = fetch(&file, &[Source::Server(addr)], &mut sh);
        assert!(matches!(&r, Err(Miss::Elsewhere(why)) if why.contains("it sent 1 of the")), "{r:?}");
        assert_eq!(requests.load(Ordering::SeqCst), ATTEMPTS as usize);
        assert!(dead[0]);
        assert_eq!(store.partial_len(&hash), Some(u64::from(ATTEMPTS)), "the bytes it sent are kept");
        assert!(!store.has(&hash, file.size));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
