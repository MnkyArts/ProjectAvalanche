use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, UdpSocket};
use std::time::{Duration, Instant};

use crate::Error;

// PORT(choice): four of RFC 6886's nine tries (its 64 s would hold a server's start; the gateway is on the LAN and
// answers in milliseconds); tuning: real gateways' answer times; the original has no dedicated server.
pub const NATPMP_SCHEDULE: [Duration; 4] =
    [Duration::from_millis(250), Duration::from_millis(500), Duration::from_millis(1000), Duration::from_millis(2000)];
// PORT(choice): two of RFC 6887's retransmissions (its MRD is unbounded for a client that keeps a mapping); tuning:
// real gateways' answer times; the original has no dedicated server.
pub const PCP_SCHEDULE: [Duration; 2] = [Duration::from_secs(3), Duration::from_secs(6)];
const BUF: usize = 2048;

pub fn socket(local: Option<Ipv4Addr>, gateway: Ipv4Addr, port: u16) -> Result<UdpSocket, Error> {
    let s = UdpSocket::bind(SocketAddr::new(local.unwrap_or(Ipv4Addr::UNSPECIFIED).into(), 0))
        .map_err(|e| Error::io("a UDP socket", e))?;
    s.connect(SocketAddrV4::new(gateway, port)).map_err(|e| Error::io(&format!("UDP to {gateway}:{port}"), e))?;
    Ok(s)
}

pub fn local_toward(local: Option<Ipv4Addr>, gateway: Ipv4Addr, port: u16) -> Result<Ipv4Addr, Error> {
    match socket(local, gateway, port)?.local_addr().map_err(|e| Error::io("the socket's address", e))? {
        SocketAddr::V4(a) => Ok(*a.ip()),
        SocketAddr::V6(_) => Err(Error::malformed("an IPv6 source towards an IPv4 gateway")),
    }
}

pub fn exchange<T>(
    socket: &UdpSocket,
    request: &[u8],
    schedule: &[Duration],
    what: &str,
    mut accept: impl FnMut(&[u8]) -> Result<Option<T>, Error>,
) -> Result<T, Error> {
    let mut buf = [0u8; BUF];
    let mut ignored: Option<String> = None;
    for wait in schedule {
        socket.send(request).map_err(|e| Error::io(&format!("{what}: sending"), e))?;
        let until = Instant::now() + *wait;
        loop {
            let left = until.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            socket.set_read_timeout(Some(left)).map_err(|e| Error::io(what, e))?;
            match socket.recv(&mut buf) {
                Ok(n) => {
                    let datagram = buf.get(..n).unwrap_or_default();
                    if n == BUF {
                        ignored = Some(format!("a datagram of {BUF} bytes or more"));
                        continue;
                    }
                    match accept(datagram) {
                        Ok(Some(t)) => return Ok(t),
                        Ok(None) => {}
                        Err(e) if e.kind == crate::ErrorKind::Malformed => ignored = Some(e.message),
                        Err(e) => return Err(e),
                    }
                }
                Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => break,
                Err(e) if e.kind() == std::io::ErrorKind::ConnectionRefused => {
                    return Err(Error::io(&format!("{what}: the gateway refuses the port (nothing listens on it)"), e));
                }
                Err(e) => return Err(Error::io(what, e)),
            }
        }
    }
    let total: Duration = schedule.iter().sum();
    Err(Error {
        kind: crate::ErrorKind::Io,
        message: match ignored {
            Some(last) => {
                format!("{what}: no usable answer within {:.2} s (the last one ignored: {last})", total.as_secs_f64())
            }
            None => format!("{what}: no answer within {:.2} s", total.as_secs_f64()),
        },
    })
}
