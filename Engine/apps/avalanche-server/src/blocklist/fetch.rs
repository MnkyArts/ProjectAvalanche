use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use http_body_util::{BodyExt, Empty, Limited};
use hyper::Request;
use hyper::header::{ETAG, HOST, IF_NONE_MATCH, USER_AGENT};
use hyper_util::rt::TokioIo;
use ssx_content::blocklist::{Answer, cache::MAX_ETAG, from_hex_list};
use ssx_core::text::{PATH_CHARS, shown_max};
use ssx_net_proto::BUILD_ID;
use ssx_net_proto::master::{Blocklist, MAX_BLOCKLIST_BYTES, MasterUrl};
use tokio::time::timeout;

use super::Sources;

// PORT(choice): 15 s, the browser's bound for a master's list (an 8 MiB list at about 4.5 Mbit/s); tuning: the master's
// list over real links; the original has no dedicated server.
pub const TIMEOUT: Duration = Duration::from_secs(15);
const MAX_HEAD: usize = 8192;
const MAX_TEXT: usize = 512;

pub async fn fetch(
    url: &MasterUrl,
    etag: Option<&str>,
    tls: &Arc<rustls::ClientConfig>,
    source: Option<IpAddr>,
) -> Result<Answer, String> {
    match timeout(TIMEOUT, ask(url, etag, tls, source)).await {
        Ok(r) => r,
        Err(_) => Err(format!("no whole answer within {} s", TIMEOUT.as_secs())),
    }
}

async fn ask(
    url: &MasterUrl,
    etag: Option<&str>,
    tls: &Arc<rustls::ClientConfig>,
    source: Option<IpAddr>,
) -> Result<Answer, String> {
    let io = crate::discovery::heartbeat::connect_from(source, tls, url).await?;
    let mut b = hyper::client::conn::http1::Builder::new();
    b.max_buf_size(MAX_HEAD).max_headers(32);
    let (mut sender, conn) =
        b.handshake::<_, Empty<Bytes>>(TokioIo::new(io)).await.map_err(|e| format!("HTTP: {e}"))?;
    let driver = tokio::spawn(conn);
    let mut req = Request::builder().method("GET").uri("/v1/blocklist").header(HOST, url.authority());
    req = req.header(USER_AGENT, BUILD_ID);
    if let Some(tag) = etag {
        req = req.header(IF_NONE_MATCH, tag);
    }
    let req = req.body(Empty::new()).map_err(|e| e.to_string())?;
    let result = async {
        let response = sender.send_request(req).await.map_err(|e| format!("HTTP: {e}"))?;
        let status = response.status().as_u16();
        let tag = response
            .headers()
            .get(ETAG)
            .and_then(|v| v.to_str().ok())
            .filter(|t| !t.is_empty() && t.len() <= MAX_ETAG && t.bytes().all(|b| (0x21..=0x7E).contains(&b)))
            .map(str::to_string);
        match status {
            304 if etag.is_some() => return Ok(Answer::NotModified),
            304 => return Err("answered 304 Not Modified to a request that named no list".to_string()),
            200 => {}
            _ => {
                let _ = Limited::new(response.into_body(), MAX_TEXT).collect().await;
                return Err(format!("not a Project Avalanche master of this version (HTTP {status})"));
            }
        }
        let body = Limited::new(response.into_body(), MAX_BLOCKLIST_BYTES)
            .collect()
            .await
            .map_err(|_| format!("its blocklist is larger than {MAX_BLOCKLIST_BYTES} bytes or cut short"))?
            .to_bytes();
        let list = Blocklist::from_json(&body).map_err(|e| ssx_core::text::message(&e).to_string())?;
        let hashes = from_hex_list(&list.resources).map_err(|e| ssx_core::text::message(&e).to_string())?;
        Ok(Answer::List { hashes, etag: tag })
    }
    .await;
    driver.abort();
    result
}

pub async fn all(sources: &Sources, tls: &Arc<rustls::ClientConfig>, source: Option<IpAddr>, now: u64) -> Vec<String> {
    let Some(url) = &sources.master else { return Vec::new() };
    let key = url.to_string();
    let etag = ssx_content::blocklist::etag_of(&sources.dir, &key);
    let shown = shown_max(&key, PATH_CHARS).to_string();
    let line = match fetch(url, etag.as_deref(), tls, source).await {
        Ok(answer) => {
            let current = answer == Answer::NotModified;
            match ssx_content::blocklist::record(&sources.dir, &key, answer, now) {
                Ok(n) if current => format!("{shown}: {n} blocked files (unchanged)"),
                Ok(n) => format!("{shown}: {n} blocked files (a new list)"),
                Err(e) => format!("{shown}: its answer is not kept ({e})"),
            }
        }
        Err(e) => format!("{shown}: not refreshed ({e}); the list kept still applies while young enough"),
    };
    vec![line]
}
