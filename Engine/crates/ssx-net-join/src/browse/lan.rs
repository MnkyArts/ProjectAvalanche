use core::time::Duration;
use std::collections::HashSet;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use ssx_net_proto::PROTOCOL_VERSION;
use ssx_net_proto::query::{LAN_PORT, LanAnswer, LanProbe, MAX_LAN_ANSWER};

pub const EVERY: Duration = Duration::from_secs(2);
// PORT(choice): 256 servers on a LAN, 16 test targets; tuning: a LAN party's servers; the original has no server
// browser.
pub const MAX_SERVERS: usize = 256;
pub const MAX_TARGETS: usize = 16;
const TICK: Duration = Duration::from_millis(50);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Targets {
    Broadcast,
    Given(Vec<SocketAddr>),
}

pub fn test_targets(text: &str) -> Result<Vec<SocketAddr>, String> {
    let mut out = Vec::new();
    for item in text.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        let addr: SocketAddr = match item.parse::<SocketAddr>() {
            Ok(a) => a,
            Err(_) => {
                SocketAddr::new(item.parse::<IpAddr>().map_err(|_| format!("{item:?} is not ip[:port]"))?, LAN_PORT)
            }
        };
        match addr.ip() {
            IpAddr::V4(v4) if !v4.is_unspecified() && !v4.is_multicast() && addr.port() != 0 => out.push(addr),
            _ => return Err(format!("{item:?}: an IPv4 unicast address and a port")),
        }
        if out.len() > MAX_TARGETS {
            return Err(format!("at most {MAX_TARGETS} targets"));
        }
    }
    Ok(out)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Found {
    pub game: SocketAddr,
    pub protocol: u32,
}

pub struct LanSearch {
    found: Mutex<Receiver<Found>>,
    stop: Arc<AtomicBool>,
}

fn run(socket: UdpSocket, targets: Targets, out: std::sync::mpsc::Sender<Found>, stop: Arc<AtomicBool>) {
    let probe = LanProbe { protocol: PROTOCOL_VERSION }.encode();
    let mut seen: HashSet<SocketAddr> = HashSet::new();
    let mut next = Instant::now();
    let mut buf = [0u8; 256];
    while !stop.load(Ordering::Acquire) {
        if Instant::now() >= next {
            next = Instant::now() + EVERY;
            let to: Vec<SocketAddr> = match &targets {
                Targets::Given(list) => list.clone(),
                Targets::Broadcast => ssx_net_lan::interfaces()
                    .map(|l| ssx_net_lan::broadcast_targets(&l))
                    .unwrap_or_default()
                    .into_iter()
                    .map(|b| SocketAddr::new(IpAddr::V4(b), LAN_PORT))
                    .collect(),
            };
            for t in to {
                let _ = socket.send_to(&probe, t);
            }
        }
        let Ok((n, from)) = socket.recv_from(&mut buf) else { continue };
        if n > MAX_LAN_ANSWER || from.port() != LAN_PORT || seen.len() >= MAX_SERVERS {
            continue;
        }
        let Ok(answer) = LanAnswer::decode(buf.get(..n).unwrap_or_default()) else { continue };
        let game = SocketAddr::new(from.ip(), answer.port);
        if seen.insert(game) && out.send(Found { game, protocol: answer.protocol }).is_err() {
            return;
        }
    }
}

impl LanSearch {
    pub fn start(targets: Targets) -> std::io::Result<Self> {
        let socket = UdpSocket::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 0))?;
        socket.set_broadcast(true)?;
        socket.set_read_timeout(Some(TICK))?;
        let (out, found) = std::sync::mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&stop);
        std::thread::Builder::new().name("LAN discovery".into()).spawn(move || run(socket, targets, out, flag))?;
        Ok(Self { found: Mutex::new(found), stop })
    }

    pub fn take(&self) -> Vec<Found> {
        match self.found.lock() {
            Ok(rx) => rx.try_iter().collect(),
            Err(_) => Vec::new(),
        }
    }
}

impl Drop for LanSearch {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_targets_are_read_strictly() {
        assert_eq!(
            test_targets("127.0.3.1, 127.0.3.2:4000"),
            Ok(vec!["127.0.3.1:35450".parse().unwrap(), "127.0.3.2:4000".parse().unwrap()])
        );
        assert_eq!(test_targets(""), Ok(vec![]));
        for bad in ["localhost", "::1", "[::1]:5", "0.0.0.0", "224.0.0.1", "127.0.0.1:0", "1.2.3"] {
            assert!(test_targets(bad).is_err(), "{bad}");
        }
        assert!(test_targets(&vec!["127.0.0.1"; 17].join(",")).is_err());
    }
}
