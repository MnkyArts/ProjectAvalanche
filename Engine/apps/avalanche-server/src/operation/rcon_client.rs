use std::collections::BTreeMap;
use std::net::{SocketAddr, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::time::Duration;

use bytes::Bytes;
use http_body_util::{BodyExt, Full, Limited};
use hyper::Request;
use hyper::header::{CONTENT_TYPE, HOST, RETRY_AFTER, USER_AGENT};
use hyper_util::rt::TokioIo;
use ssx_net_proto::info::{MAX_INFO_BYTES, ServerInfo};
use ssx_net_proto::join::CHALLENGE_SIZE;
use ssx_net_proto::keys::{ReplySecret, ServerKey};
use ssx_net_proto::rcon::{MAX_ANSWER_BODY, RCON_VERSION, RconAnswer, RconRequest};
use ssx_net_proto::{BUILD_ID, DEFAULT_PORT};

pub const PIN_FILE: &str = "rcon-servers.toml";
const MAX_PIN_FILE: u64 = 256 * 1024;
const MAX_PINS: usize = 1024;
// PORT(choice): 10 s, the server's own connection time; tuning: operators on slow links; the original has no dedicated
// server.
const REQUEST_TIME: Duration = Duration::from_secs(10);
const MAX_PLAIN: usize = 1024;
// PORT(choice): 20 s in all; tuning: operators' scripts against the server's limits (RCON 2 a second, burst 8; /info 4,
// burst 8), measured; the original has no dedicated server.
const WAIT_BUDGET: Duration = Duration::from_secs(20);

pub struct RconOptions {
    pub address: String,
    pub key: PathBuf,
    pub server_key: Option<String>,
    pub accept_new: bool,
    pub command: String,
}

fn resolve(address: &str) -> Result<SocketAddr, String> {
    let shown = ssx_core::text::quoted_max(address, 100);
    if let Ok(a) = address.parse::<SocketAddr>() {
        return Ok(a);
    }
    if let Ok(ip) = address.trim_start_matches('[').trim_end_matches(']').parse::<std::net::IpAddr>() {
        return Ok(SocketAddr::new(ip, DEFAULT_PORT));
    }
    let with_port = if address.contains(':') { address.to_string() } else { format!("{address}:{DEFAULT_PORT}") };
    with_port
        .to_socket_addrs()
        .map_err(|e| format!("{shown}: {e}"))?
        .next()
        .ok_or_else(|| format!("{shown}: no address"))
}

struct Answer {
    status: u16,
    retry_after: Option<u64>,
    body: Vec<u8>,
}

async fn fetch(addr: SocketAddr, post: Option<Vec<u8>>, path: &str, max: usize) -> Result<Answer, String> {
    let stream = tokio::net::TcpStream::connect(addr).await.map_err(|e| format!("{addr}: {e}"))?;
    let (mut sender, conn) = hyper::client::conn::http1::Builder::new()
        .handshake::<_, Full<Bytes>>(TokioIo::new(stream))
        .await
        .map_err(|e| format!("HTTP: {e}"))?;
    let driver = tokio::spawn(conn);
    let builder = Request::builder()
        .method(if post.is_some() { "POST" } else { "GET" })
        .uri(path)
        .header(HOST, addr.to_string())
        .header(USER_AGENT, BUILD_ID);
    let request = match post {
        Some(body) => builder.header(CONTENT_TYPE, "application/octet-stream").body(Full::new(Bytes::from(body))),
        None => builder.body(Full::new(Bytes::new())),
    }
    .map_err(|e| e.to_string())?;
    let result = async {
        let response = sender.send_request(request).await.map_err(|e| format!("HTTP: {e}"))?;
        let status = response.status().as_u16();
        let retry_after = response.headers().get(RETRY_AFTER).and_then(|v| v.to_str().ok()?.parse::<u64>().ok());
        let limit = if status == 200 { max } else { MAX_PLAIN };
        let body = Limited::new(response.into_body(), limit)
            .collect()
            .await
            .map_err(|_| format!("an answer larger than {limit} bytes or cut short (HTTP {status})"))?;
        Ok(Answer { status, retry_after, body: body.to_bytes().to_vec() })
    }
    .await;
    driver.abort();
    result
}

struct Waits {
    left: Duration,
    notes: Vec<String>,
}

async fn get_ok(
    addr: SocketAddr,
    post: Option<Vec<u8>>,
    path: &str,
    max: usize,
    waits: &mut Waits,
) -> Result<Vec<u8>, String> {
    loop {
        let answer = tokio::time::timeout(REQUEST_TIME, fetch(addr, post.clone(), path, max))
            .await
            .map_err(|_| format!("{path}: no answer within {} s", REQUEST_TIME.as_secs()))??;
        if let Some(secs) = wait_for(&answer, &mut waits.left) {
            waits.notes.push(format!("{path}: the server's rate limit: waited {secs} s"));
            tokio::time::sleep(Duration::from_secs(secs)).await;
            continue;
        }
        return ok_body(path, answer);
    }
}

fn wait_for(answer: &Answer, left: &mut Duration) -> Option<u64> {
    if answer.status != 429 {
        return None;
    }
    let secs = answer.retry_after?.max(1);
    *left = left.checked_sub(Duration::from_secs(secs))?;
    Some(secs)
}

fn ok_body(path: &str, answer: Answer) -> Result<Vec<u8>, String> {
    if answer.status == 200 {
        return Ok(answer.body);
    }
    let text = ssx_core::text::shown_max(&String::from_utf8_lossy(&answer.body), 200).to_string();
    let wait = answer.retry_after.map_or(String::new(), |s| format!(" (the server asks to wait {s} s)"));
    Err(format!("{path}: HTTP {}: {}{wait}", answer.status, text.trim()))
}

fn read_pins(path: &Path) -> Result<BTreeMap<String, String>, String> {
    use std::io::Read;
    let shown = path.display().to_string();
    let mut text = String::new();
    match std::fs::File::open(path) {
        Ok(f) => {
            f.take(MAX_PIN_FILE + 1).read_to_string(&mut text).map_err(|e| format!("{shown}: {e}"))?;
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
        Err(e) => return Err(format!("{shown}: {e}")),
    }
    if text.len() as u64 > MAX_PIN_FILE {
        return Err(format!("{shown}: larger than {MAX_PIN_FILE} bytes"));
    }
    let pins: BTreeMap<String, String> = toml::from_str(&text).map_err(|e| format!("{shown}: {}", e.message()))?;
    if pins.len() > MAX_PINS {
        return Err(format!("{shown}: more than {MAX_PINS} servers"));
    }
    for (address, key) in &pins {
        ServerKey::from_hex(key).map_err(|e| format!("{shown}: {}: {e}", ssx_core::text::quoted_max(address, 100)))?;
    }
    Ok(pins)
}

fn write_pins(path: &Path, pins: &BTreeMap<String, String>) -> Result<(), String> {
    let text = toml::to_string(pins).map_err(|e| e.to_string())?;
    let body = format!("# The servers' identity keys the remote console pinned (address = key).\n{text}");
    ssx_files::replace(path, body.as_bytes(), &ssx_files::Options::new())
        .map_err(|e| format!("{}: {e}", path.display()))
}

fn pinned(options: &RconOptions, offered: &ServerKey) -> Result<Option<String>, String> {
    let hex = offered.to_hex();
    if let Some(given) = &options.server_key {
        let given = ServerKey::from_hex(given).map_err(|e| format!("--server-key: {e}"))?;
        return if given == *offered {
            Ok(None)
        } else {
            Err(format!("the server's key is {hex}, not the one given (--server-key): refused"))
        };
    }
    let pin_path = options.key.parent().unwrap_or(Path::new(".")).join(PIN_FILE);
    let mut pins = read_pins(&pin_path)?;
    let address = options.address.to_ascii_lowercase();
    match pins.get(&address) {
        Some(known) if *known == hex => Ok(None),
        Some(known) => Err(format!(
            "the server at {address} offers the key {hex}, but {} pins {known}: refused (if the server's key really \
             changed, remove its line there)",
            pin_path.display()
        )),
        None if options.accept_new => {
            pins.insert(address.clone(), hex.clone());
            write_pins(&pin_path, &pins)?;
            Ok(Some(format!("pinned {address} = {hex} in {}", pin_path.display())))
        }
        None => Err(format!(
            "{address} is not pinned; its /info offers the key {hex}. Compare it with the server's log line \
             \"identity key …\" and run again with --accept-new (or --server-key <hex>)"
        )),
    }
}

pub fn run(options: &RconOptions) -> Result<(Vec<String>, RconAnswer), String> {
    let secret = super::keys::load(&options.key)?;
    let addr = resolve(&options.address)?;
    let runtime =
        tokio::runtime::Builder::new_current_thread().enable_io().enable_time().build().map_err(|e| e.to_string())?;
    runtime.block_on(async {
        let mut waits = Waits { left: WAIT_BUDGET, notes: Vec::new() };
        let info = ServerInfo::from_json(&get_ok(addr, None, "/info", MAX_INFO_BYTES, &mut waits).await?)?;
        let server = ServerKey::from_hex(&info.identity_key).map_err(|e| format!("/info's key: {e}"))?;
        let mut notes: Vec<String> = pinned(options, &server)?.into_iter().collect();
        let challenge: [u8; CHALLENGE_SIZE] = get_ok(addr, None, "/rcon/challenge", CHALLENGE_SIZE, &mut waits)
            .await?
            .try_into()
            .map_err(|_| format!("/rcon/challenge: not {CHALLENGE_SIZE} bytes"))?;
        let reply = ReplySecret::generate();
        let request = RconRequest {
            rcon_version: RCON_VERSION,
            reply: reply.public(),
            challenge,
            operator: secret.public(),
            command: options.command.clone(),
        };
        let body = request.seal(&server, &secret).map_err(|e| e.to_string())?;
        let answer = get_ok(addr, Some(body), "/rcon", MAX_ANSWER_BODY, &mut waits).await?;
        let answer = RconAnswer::open(&answer, &reply).map_err(|e| format!("the answer: {e}"))?;
        notes.append(&mut waits.notes);
        Ok((notes, answer))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limited(retry_after: Option<u64>) -> Answer {
        Answer { status: 429, retry_after, body: Vec::new() }
    }

    #[test]
    fn waits_are_bounded() {
        let mut left = WAIT_BUDGET;
        assert_eq!(wait_for(&limited(Some(1)), &mut left), Some(1));
        assert_eq!(left, WAIT_BUDGET - Duration::from_secs(1));
        assert_eq!(wait_for(&limited(Some(61)), &mut left), None);
        assert_eq!(wait_for(&limited(None), &mut left), None);
        assert_eq!(wait_for(&Answer { status: 503, retry_after: Some(1), body: Vec::new() }, &mut left), None);
        let mut left = WAIT_BUDGET;
        let tries = std::iter::from_fn(|| wait_for(&limited(Some(0)), &mut left)).count();
        assert_eq!(tries as u64, WAIT_BUDGET.as_secs());
        assert_eq!(left, Duration::ZERO);
    }
}
