use std::net::IpAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use hmac::digest::KeyInit;
use hmac::{Hmac, Mac};
use sha2::Sha256;
use ssx_net::address::{address_key, canonical};
use ssx_net_proto::keys::random_bytes;
use ssx_net_proto::query::{
    CHALLENGE_REQUEST_SIZE, Challenge, QUERY_REQUEST_SIZE, QueryAnswer, QueryRequest, TOKEN_SIZE, info_hash, mode_code,
};
use subtle::ConstantTimeEq;
use tokio::net::UdpSocket;
use zeroize::Zeroizing;

use super::{GlobalLimiter, Stats};
use crate::http::HttpState;
use crate::http::limits::{AddressLimiter, Rate, Verdict};

pub const QUERY_RATE: Rate = Rate { per_second: 5.0 / 6.0, burst: 5.0 };
pub const CACHE_FOR: Duration = Duration::from_secs(10);
// PORT(choice): 30 s windows (the browser asks a server again every 10 s and keeps its token 25 s); tuning: the
// browser's measured query pattern over real links; the original has no dedicated server.
pub const TOKEN_WINDOW: Duration = Duration::from_secs(30);
// PORT(choice): 1 000 challenges a second (a browser asks at 10 a second; a hundred browsers at once), 4 096 addresses;
// tuning: a public server's measured query load; the original has no dedicated server.
pub const GLOBAL_RATE: Rate = Rate { per_second: 1000.0, burst: 2000.0 };
pub const TRACKED: usize = 4096;

const TOKEN_DOMAIN: &[u8] = b"SSXA query token v1\0";

struct Tokens {
    key: Zeroizing<[u8; 64]>,
    started: Instant,
}

impl Tokens {
    fn token(&self, ip: IpAddr, window: u64) -> [u8; TOKEN_SIZE] {
        let mut mac = <Hmac<Sha256> as KeyInit>::new(&(*self.key).into());
        mac.update(TOKEN_DOMAIN);
        let bytes = match canonical(ip) {
            IpAddr::V4(v4) => v4.to_ipv6_mapped().octets(),
            IpAddr::V6(v6) => v6.octets(),
        };
        mac.update(&bytes);
        mac.update(&window.to_be_bytes());
        let full = mac.finalize().into_bytes();
        let mut out = [0u8; TOKEN_SIZE];
        out.copy_from_slice(&full[..TOKEN_SIZE]);
        out
    }

    fn window(&self, now: Instant) -> u64 {
        now.saturating_duration_since(self.started).as_secs() / TOKEN_WINDOW.as_secs()
    }

    fn valid(&self, ip: IpAddr, token: &[u8; TOKEN_SIZE], now: Instant) -> bool {
        let w = self.window(now);
        let this = bool::from(self.token(ip, w).ct_eq(token));
        let last = w > 0 && bool::from(self.token(ip, w - 1).ct_eq(token));
        this | last
    }
}

fn state_now(http: &HttpState) -> Option<QueryAnswer> {
    let info = http.info();
    Some(QueryAnswer {
        sequence: 0,
        protocol_version: info.protocol_version,
        players: u16::try_from(info.players).ok()?,
        max_players: u16::try_from(info.max_players).ok()?,
        mode: mode_code(&info.mode)?,
        password: info.password,
        info_hash: info_hash(&info),
    })
}

pub async fn run(socket: UdpSocket, http: Arc<HttpState>, stats: Arc<Stats>) {
    let tokens = Tokens { key: Zeroizing::new(random_bytes()), started: Instant::now() };
    let per_address = AddressLimiter::new(QUERY_RATE, TRACKED);
    let global = GlobalLimiter::new(GLOBAL_RATE);
    let mut cache: Option<(Instant, QueryAnswer)> = None;
    let mut buf = [0u8; 1536];
    loop {
        let (n, peer) = match socket.recv_from(&mut buf).await {
            Ok(r) => r,
            Err(_) => continue,
        };
        Stats::count(&stats.query_received);
        if n != CHALLENGE_REQUEST_SIZE && n != QUERY_REQUEST_SIZE {
            Stats::count(&stats.query_malformed);
            continue;
        }
        let now = Instant::now();
        let Ok(request) = QueryRequest::decode(buf.get(..n).unwrap_or_default()) else {
            Stats::count(&stats.query_malformed);
            continue;
        };
        let allowed = match &request {
            QueryRequest::Challenge => {
                per_address.check(address_key(peer.ip()), now) == Verdict::Allowed
                    && global.check(now) == Verdict::Allowed
            }
            QueryRequest::Query { token, .. } => {
                if !tokens.valid(peer.ip(), token, now) {
                    Stats::count(&stats.query_bad_token);
                    continue;
                }
                per_address.check(address_key(peer.ip()), now) == Verdict::Allowed
            }
        };
        if !allowed {
            Stats::count(&stats.query_limited);
            continue;
        }
        let answer = match request {
            QueryRequest::Challenge => Challenge { token: tokens.token(peer.ip(), tokens.window(now)) }.encode(),
            QueryRequest::Query { sequence, .. } => {
                if cache.is_none_or(|(at, _)| now.duration_since(at) >= CACHE_FOR) {
                    cache = state_now(&http).map(|a| (now, a));
                }
                let Some((_, state)) = cache else { continue };
                match (QueryAnswer { sequence, ..state }).encode() {
                    Ok(bytes) => bytes,
                    Err(_) => continue,
                }
            }
        };
        if answer.len() <= n && socket.send_to(&answer, peer).await.is_ok() {
            Stats::count(&stats.query_answered);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_hold_two_windows_for_their_address() {
        let t = Tokens { key: Zeroizing::new(random_bytes()), started: Instant::now() - Duration::from_secs(95) };
        let now = Instant::now();
        let a: IpAddr = "192.0.2.7".parse().unwrap();
        let w = t.window(now);
        assert_eq!(w, 3);
        assert!(t.valid(a, &t.token(a, w), now));
        assert!(t.valid(a, &t.token(a, w - 1), now), "the window before");
        assert!(!t.valid(a, &t.token(a, w - 2), now), "older");
        assert!(!t.valid("192.0.2.8".parse().unwrap(), &t.token(a, w), now), "another address");
        assert!(t.valid("::ffff:192.0.2.7".parse().unwrap(), &t.token(a, w), now), "the same address mapped");
        let other = Tokens { key: Zeroizing::new(random_bytes()), started: t.started };
        assert!(!other.valid(a, &t.token(a, w), now), "another start");
    }
}
