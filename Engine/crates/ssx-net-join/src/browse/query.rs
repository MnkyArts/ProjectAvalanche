use core::time::Duration;
use std::net::SocketAddr;
use std::time::Instant;

use ssx_net_proto::info::{INFO_VERSION, InfoHead, MAX_INFO_BYTES, ServerInfo};
use ssx_net_proto::players::{MAX_PLAYERS_BYTES, PlayerList};
use ssx_net_proto::{BUILD_ID, PROTOCOL_VERSION};
use ureq::Agent;

// PORT(choice): 4 s in all and 2 s to connect (the join takes 10 s / 5 s; a list of 100 servers is asked 8 at a time,
// so a dead server must not hold a worker long); tuning: the browser over real links (phase 5's masters and queries);
// the original has no server browser.
pub const QUERY_TIMEOUT: Duration = Duration::from_secs(4);
pub const QUERY_CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
const MAX_HEADERS: usize = 8 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InfoAnswer {
    Info(Box<ServerInfo>),
    Incompatible { head: InfoHead, why: String },
}

fn agent() -> Agent {
    Agent::config_builder()
        .timeout_global(Some(QUERY_TIMEOUT))
        .timeout_connect(Some(QUERY_CONNECT_TIMEOUT))
        .max_response_header_size(MAX_HEADERS)
        .http_status_as_error(false)
        .max_redirects(0)
        .proxy(None)
        .user_agent(BUILD_ID)
        .build()
        .into()
}

fn get(server: SocketAddr, path: &str, max: usize) -> Result<Vec<u8>, String> {
    let mut response = agent().get(&format!("http://{server}{path}")).call().map_err(|e| match e {
        ureq::Error::Timeout(_) => format!("no answer within {} s", QUERY_TIMEOUT.as_secs()),
        ureq::Error::Io(io) if io.kind() == std::io::ErrorKind::ConnectionRefused => "nothing answers there".into(),
        other => format!("cannot be reached: {other}"),
    })?;
    let status = response.status().as_u16();
    if status == 429 {
        return Err("asks to be asked less often (HTTP 429)".into());
    }
    if status != 200 {
        return Err(format!("not a Project Avalanche server of this version (HTTP {status} for {path})"));
    }
    let bytes = response.body_mut().with_config().limit(max as u64 + 1).read_to_vec().map_err(|e| match e {
        ureq::Error::BodyExceedsLimit(_) => format!("its answer to {path} is larger than {max} bytes"),
        ureq::Error::Timeout(_) => format!("no answer within {} s", QUERY_TIMEOUT.as_secs()),
        e => format!("its answer to {path} does not read: {e}"),
    })?;
    if bytes.len() > max {
        return Err(format!("its answer to {path} is larger than {max} bytes"));
    }
    Ok(bytes)
}

pub fn ask_info(server: SocketAddr) -> Result<(InfoAnswer, Duration), String> {
    let started = Instant::now();
    let body = get(server, "/info", MAX_INFO_BYTES)?;
    let rtt = started.elapsed();
    let head = InfoHead::from_json(&body)?;
    if head.info_version != INFO_VERSION || head.protocol_version != PROTOCOL_VERSION {
        let why = if head.protocol_version != PROTOCOL_VERSION {
            format!("the server speaks protocol {}, this game {PROTOCOL_VERSION}", head.protocol_version)
        } else {
            format!("the server's info is version {}, this game reads {INFO_VERSION}", head.info_version)
        };
        return Ok((InfoAnswer::Incompatible { head, why }, rtt));
    }
    Ok((InfoAnswer::Info(Box::new(ServerInfo::from_json(&body)?)), rtt))
}

pub fn ask_icon(server: SocketAddr, hash: &[u8; 32]) -> Result<ssx_net_proto::icon::Icon, String> {
    let icon = ssx_net_proto::icon::read_icon(&get(server, "/icon.png", ssx_net_proto::icon::MAX_ICON_BYTES)?)?;
    if icon.hash != *hash {
        return Err("its icon is not the one its info names (another hash)".into());
    }
    Ok(icon)
}

pub fn ask_players(server: SocketAddr) -> Result<PlayerList, String> {
    PlayerList::from_json(&get(server, "/players", MAX_PLAYERS_BYTES)?)
}
