use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::time::{Duration, Instant};

use bytes::Bytes;
use http_body_util::{BodyExt, Empty, Limited};
use hyper::Request;
use hyper::header::{HOST, USER_AGENT};
use hyper_util::rt::TokioIo;
use ssx_net_proto::BUILD_ID;
use ssx_net_proto::info::{MAX_INFO_BYTES, ServerInfo};
use ssx_net_proto::keys::{ServerKey, random_bytes};
use ssx_net_proto::master::{Heartbeat, heartbeat_against_info};
use ssx_net_proto::query::{Challenge, QUERY_ANSWER_SIZE, QueryAnswer, QueryRequest, info_hash, mode_code};
use tokio::net::{TcpStream, UdpSocket};
use tokio::time::timeout;

// PORT(choice): 2 s to connect, 4 s for `/info`, 1 s a UDP attempt (a check ends within 8 s, inside the heartbeat's
// connection time of 15 s); tuning: checks of servers over real links (the master's log of slow checks); the original
// has no master server.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
pub const INFO_TIMEOUT: Duration = Duration::from_secs(4);
pub const UDP_ATTEMPT: Duration = Duration::from_secs(1);
const MAX_HEAD: usize = 8192;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Checked {
    pub info_hash: [u8; 32],
    pub query_rtt: Duration,
}

async fn fetch_info(addr: SocketAddr) -> Result<Vec<u8>, String> {
    let stream = match timeout(CONNECT_TIMEOUT, TcpStream::connect(addr)).await {
        Ok(Ok(s)) => s,
        Ok(Err(e)) => return Err(format!("TCP {addr} does not answer (GET /info): {e} — is the port forwarded?")),
        Err(_) => {
            return Err(format!(
                "TCP {addr} did not connect within {} s (GET /info): is the port forwarded?",
                CONNECT_TIMEOUT.as_secs()
            ));
        }
    };
    let work = async {
        let mut b = hyper::client::conn::http1::Builder::new();
        b.max_buf_size(MAX_HEAD).max_headers(32);
        let (mut sender, conn) =
            b.handshake::<_, Empty<Bytes>>(TokioIo::new(stream)).await.map_err(|e| format!("HTTP: {e}"))?;
        let driver = tokio::spawn(conn);
        let request = Request::get("/info")
            .header(HOST, addr.to_string())
            .header(USER_AGENT, BUILD_ID)
            .body(Empty::<Bytes>::new())
            .map_err(|e| e.to_string())?;
        let result = async {
            let response = sender.send_request(request).await.map_err(|e| format!("HTTP: {e}"))?;
            if response.status() != hyper::StatusCode::OK {
                return Err(format!("GET /info answered {}", response.status()));
            }
            let body = Limited::new(response.into_body(), MAX_INFO_BYTES)
                .collect()
                .await
                .map_err(|_| format!("its /info is larger than {MAX_INFO_BYTES} bytes or ended early"))?;
            Ok(body.to_bytes().to_vec())
        }
        .await;
        driver.abort();
        result
    };
    match timeout(INFO_TIMEOUT, work).await {
        Ok(r) => r.map_err(|e| format!("TCP {addr}: {e}")),
        Err(_) => Err(format!("TCP {addr}: no whole answer to GET /info within {} s", INFO_TIMEOUT.as_secs())),
    }
}

async fn exchange(socket: &UdpSocket, request: &[u8], expect: usize) -> Result<(Vec<u8>, Duration), ()> {
    let mut buf = [0u8; 128];
    for _ in 0..2 {
        let sent = Instant::now();
        if socket.send(request).await.is_err() {
            return Err(());
        }
        let deadline = sent + UDP_ATTEMPT;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match timeout(left, socket.recv(&mut buf)).await {
                Ok(Ok(n)) if n == expect => return Ok((buf[..n].to_vec(), sent.elapsed())),
                Ok(Ok(_)) => continue,
                Ok(Err(_)) | Err(_) => break,
            }
        }
    }
    Err(())
}

async fn query(addr: SocketAddr) -> Result<(QueryAnswer, Duration), String> {
    let local: SocketAddr = match addr.ip() {
        IpAddr::V4(_) => (Ipv4Addr::UNSPECIFIED, 0).into(),
        IpAddr::V6(_) => (Ipv6Addr::UNSPECIFIED, 0).into(),
    };
    let socket = UdpSocket::bind(local).await.map_err(|e| format!("the master's UDP socket: {e}"))?;
    socket.connect(addr).await.map_err(|e| format!("UDP {addr}: {e}"))?;
    let unreachable = || {
        format!(
            "UDP {addr} (the query) did not answer within {} s: is the port forwarded, and the query on \
             ([discovery] query = true)?",
            2 * UDP_ATTEMPT.as_secs()
        )
    };
    let (bytes, _) = exchange(&socket, &QueryRequest::Challenge.encode(), ssx_net_proto::query::CHALLENGE_SIZE)
        .await
        .map_err(|()| unreachable())?;
    let challenge = Challenge::decode(&bytes).map_err(|e| format!("UDP {addr}: its challenge: {e}"))?;
    let sequence = u32::from_be_bytes(random_bytes());
    let request = QueryRequest::Query { token: challenge.token, sequence }.encode();
    let (bytes, rtt) = exchange(&socket, &request, QUERY_ANSWER_SIZE).await.map_err(|()| unreachable())?;
    let answer = QueryAnswer::decode(&bytes).map_err(|e| format!("UDP {addr}: its query answer: {e}"))?;
    if answer.sequence != sequence {
        return Err(format!("UDP {addr}: the query answered another sequence number"));
    }
    Ok((answer, rtt))
}

pub async fn check(ip: IpAddr, beat: &Heartbeat, key: &ServerKey) -> Result<Checked, String> {
    let tcp = SocketAddr::new(ip, beat.port);
    let body = fetch_info(tcp).await?;
    let info = ServerInfo::from_json(&body).map_err(|e| format!("its /info: {e}"))?;
    if let Some(lie) = heartbeat_against_info(beat, key, &info) {
        return Err(lie);
    }
    let hash = info_hash(&info);
    let udp = SocketAddr::new(ip, beat.port + 1);
    let (answer, query_rtt) = query(udp).await?;
    let differs = |what: &str| Err(format!("its UDP query says another {what} than its /info and heartbeat"));
    if answer.protocol_version != beat.protocol_version {
        return differs("protocol");
    }
    if u32::from(answer.max_players) != beat.max_players {
        return differs("player count");
    }
    if Some(answer.mode) != mode_code(&beat.mode) {
        return differs("mode");
    }
    if answer.password != beat.password {
        return differs("password flag");
    }
    if answer.info_hash != hash {
        return Err(
            "its UDP query's info hash is not its /info's: the query answers for another server, or /info changed"
                .into(),
        );
    }
    Ok(Checked { info_hash: hash, query_rtt })
}
