use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::{Duration, Instant};

use socket2::{Domain, Protocol, Socket, Type};
use ssx_net::address::address_key;
use ssx_net_proto::PROTOCOL_VERSION;
use ssx_net_proto::query::{LAN_PORT, LAN_PROBE_SIZE, LanAnswer, LanProbe};
use tokio::net::UdpSocket;

use super::{GlobalLimiter, Stats};
use crate::http::limits::{AddressLimiter, Rate, Verdict};

pub const LAN_RATE: Rate = super::query::QUERY_RATE;
// PORT(choice): 200 answers a second (a LAN of a few dozen browsers probing every 2 s), 1 024 addresses; tuning: a LAN
// party's measured probes; the original has no dedicated server.
pub const GLOBAL_RATE: Rate = Rate { per_second: 200.0, burst: 400.0 };
pub const TRACKED: usize = 1024;
pub const INTERFACES_EVERY: Duration = Duration::from_secs(30);

pub fn listen_address(bind: IpAddr) -> Option<SocketAddr> {
    match bind {
        IpAddr::V4(v4) => Some(SocketAddr::new(IpAddr::V4(v4), LAN_PORT)),
        IpAddr::V6(v6) if v6.is_unspecified() => Some(SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), LAN_PORT)),
        IpAddr::V6(_) => None,
    }
}

pub fn bind(addr: SocketAddr) -> std::io::Result<std::net::UdpSocket> {
    let socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
    socket.set_reuse_address(true)?;
    socket.bind(&addr.into())?;
    socket.set_nonblocking(true)?;
    Ok(socket.into())
}

pub async fn run(socket: UdpSocket, game_port: u16, stats: Arc<Stats>) {
    let per_address = AddressLimiter::new(LAN_RATE, TRACKED);
    let global = GlobalLimiter::new(GLOBAL_RATE);
    let answer = LanAnswer { protocol: PROTOCOL_VERSION, port: game_port }.encode();
    let mut links: Vec<ssx_net_lan::Interface> = Vec::new();
    let mut read_at: Option<Instant> = None;
    let mut buf = [0u8; 1536];
    loop {
        let (n, peer) = match socket.recv_from(&mut buf).await {
            Ok(r) => r,
            Err(_) => continue,
        };
        Stats::count(&stats.lan_received);
        let now = Instant::now();
        if n != LAN_PROBE_SIZE {
            Stats::count(&stats.lan_malformed);
            continue;
        }
        if read_at.is_none_or(|at| now.duration_since(at) >= INTERFACES_EVERY) {
            if let Ok(list) = ssx_net_lan::interfaces() {
                links = list;
            }
            read_at = Some(now);
        }
        if !ssx_net_lan::on_link(peer.ip(), &links) {
            Stats::count(&stats.lan_off_link);
            continue;
        }
        if per_address.check(address_key(peer.ip()), now) != Verdict::Allowed || global.check(now) != Verdict::Allowed {
            Stats::count(&stats.lan_limited);
            continue;
        }
        if LanProbe::decode(buf.get(..n).unwrap_or_default()).is_err() {
            Stats::count(&stats.lan_malformed);
            continue;
        }
        if answer.len() <= n && socket.send_to(&answer, peer).await.is_ok() {
            Stats::count(&stats.lan_answered);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn where_the_responder_listens() {
        assert_eq!(listen_address("0.0.0.0".parse().unwrap()), Some(SocketAddr::from(([0, 0, 0, 0], LAN_PORT))));
        assert_eq!(listen_address("::".parse().unwrap()), Some(SocketAddr::from(([0, 0, 0, 0], LAN_PORT))));
        assert_eq!(
            listen_address("192.168.1.5".parse().unwrap()),
            Some(SocketAddr::from(([192, 168, 1, 5], LAN_PORT)))
        );
        assert_eq!(listen_address("2001:db8::5".parse().unwrap()), None);
    }
}
