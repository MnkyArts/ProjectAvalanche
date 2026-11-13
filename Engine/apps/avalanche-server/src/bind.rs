use std::io;
use std::net::{IpAddr, SocketAddr};

use socket2::{Domain, Protocol, Socket, Type};

pub fn takes_ipv4(addr: SocketAddr) -> bool {
    match addr.ip() {
        IpAddr::V6(v6) => v6.is_unspecified() || v6.to_ipv4_mapped().is_some(),
        IpAddr::V4(_) => true,
    }
}

fn socket(addr: SocketAddr, ty: Type, protocol: Protocol) -> io::Result<Socket> {
    let s = Socket::new(Domain::for_address(addr), ty, Some(protocol))?;
    if addr.is_ipv6() {
        s.set_only_v6(!takes_ipv4(addr))?;
    }
    Ok(s)
}

pub fn tcp_listener(addr: SocketAddr) -> io::Result<std::net::TcpListener> {
    let s = socket(addr, Type::STREAM, Protocol::TCP)?;
    #[cfg(unix)]
    s.set_reuse_address(true)?;
    s.bind(&addr.into())?;
    s.listen(128)?;
    Ok(s.into())
}

pub fn udp_socket(addr: SocketAddr) -> io::Result<std::net::UdpSocket> {
    let s = socket(addr, Type::DGRAM, Protocol::UDP)?;
    s.bind(&addr.into())?;
    Ok(s.into())
}

// PORT(choice): 8 picks, a port bound — a pick fails only when another process holds UDP on the number the system gave
// the TCP listener (two of the test servers' starts in the full test runs of 2026-10-31, each passing on its next run);
// the system's picks of a free port are spread over its ephemeral range, so eight fail together only on a host that
// holds most of that range's UDP ports; tuning: the picks a start needed, logged over the servers' starts; the original
// has no dedicated server.
pub const PORT_PICKS: usize = 8;

#[derive(Debug)]
pub enum PortError {
    Tcp(SocketAddr, io::Error),
    Udp(SocketAddr, io::Error),
}

pub fn game_sockets(addr: SocketAddr) -> Result<(std::net::TcpListener, std::net::UdpSocket, usize), PortError> {
    game_sockets_with(addr, tcp_listener)
}

pub fn game_sockets_with(
    addr: SocketAddr,
    mut listen: impl FnMut(SocketAddr) -> io::Result<std::net::TcpListener>,
) -> Result<(std::net::TcpListener, std::net::UdpSocket, usize), PortError> {
    let mut picks = 0;
    loop {
        picks += 1;
        let listener = listen(addr).map_err(|e| PortError::Tcp(addr, e))?;
        let local = listener.local_addr().map_err(|e| PortError::Tcp(addr, e))?;
        let udp = SocketAddr::new(addr.ip(), local.port());
        match udp_socket(udp) {
            Ok(socket) => return Ok((listener, socket, picks)),
            Err(e) if addr.port() == 0 && e.kind() == io::ErrorKind::AddrInUse && picks < PORT_PICKS => drop(listener),
            Err(e) => return Err(PortError::Udp(udp, e)),
        }
    }
}

pub fn stack_note(ip: IpAddr) -> &'static str {
    match ip {
        IpAddr::V6(v6) if v6.is_unspecified() => " (IPv6 and IPv4: dual stack)",
        IpAddr::V4(v4) if v4.is_unspecified() => " (IPv4 only; \"::\" listens on IPv6 too)",
        IpAddr::V6(_) => " (this IPv6 address only)",
        IpAddr::V4(_) => " (this IPv4 address only)",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, Ipv6Addr, TcpStream};

    fn colliding_pick() -> (std::net::UdpSocket, std::net::TcpListener) {
        loop {
            let held = std::net::UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).expect("a UDP socket");
            let port = held.local_addr().unwrap().port();
            if let Ok(l) = tcp_listener(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port)) {
                return (held, l);
            }
        }
    }

    #[test]
    fn a_picked_port_whose_udp_is_taken_is_picked_again() {
        let any = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0);
        let (_held, first) = colliding_pick();
        let taken = first.local_addr().unwrap().port();
        let mut first = Some(first);
        let (tcp, udp, picks) =
            game_sockets_with(any, |a| first.take().map_or_else(|| tcp_listener(a), Ok)).expect("the second pick");
        assert_eq!(picks, 2, "one collision, one pick more");
        let port = tcp.local_addr().unwrap().port();
        assert_ne!(port, taken);
        assert_eq!(udp.local_addr().unwrap().port(), port, "UDP and TCP on one number");
        drop((tcp, udp));
        let configured = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), taken);
        let mut calls = 0;
        let err = game_sockets_with(configured, |a| {
            calls += 1;
            tcp_listener(a)
        })
        .expect_err("the configured port's UDP is held");
        assert!(matches!(err, PortError::Udp(a, ref e) if a.port() == taken && e.kind() == io::ErrorKind::AddrInUse));
        assert_eq!(calls, 1);
        let mut pairs: Vec<_> = (0..PORT_PICKS).map(|_| colliding_pick()).collect();
        let last = pairs.last().unwrap().1.local_addr().unwrap().port();
        let mut listeners: Vec<_> = pairs.iter_mut().map(|p| p.1.try_clone().unwrap()).collect();
        listeners.reverse();
        let err = game_sockets_with(any, |_| listeners.pop().ok_or_else(|| io::Error::other("no more picks")))
            .expect_err("every pick collides");
        assert!(matches!(err, PortError::Udp(a, _) if a.port() == last), "{err:?}");
        assert!(listeners.is_empty(), "{PORT_PICKS} picks");
    }

    #[test]
    fn ipv6_sockets_and_ipv4_peers() {
        let mapped = SocketAddr::new(IpAddr::V6(Ipv4Addr::LOCALHOST.to_ipv6_mapped()), 0);
        let l = tcp_listener(mapped).expect("an IPv6 socket on the mapped loopback");
        let port = l.local_addr().unwrap().port();
        let _v4 = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).expect("IPv4 to it");
        let (_, peer) = l.accept().unwrap();
        assert_eq!(peer.ip(), IpAddr::V6(Ipv4Addr::LOCALHOST.to_ipv6_mapped()), "a mapped peer");
        assert_eq!(ssx_net::address::canonical(peer.ip()), IpAddr::V4(Ipv4Addr::LOCALHOST));
        let u = udp_socket(mapped).unwrap();
        let to = (Ipv4Addr::LOCALHOST, u.local_addr().unwrap().port());
        std::net::UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap().send_to(b"x", to).expect("IPv4 to it");
        u.set_read_timeout(Some(std::time::Duration::from_secs(2))).unwrap();
        let mut b = [0u8; 4];
        let (_, from) = u.recv_from(&mut b).expect("received");
        assert_eq!(from.ip(), IpAddr::V6(Ipv4Addr::LOCALHOST.to_ipv6_mapped()));
        let Ok(only) = tcp_listener(SocketAddr::new(IpAddr::V6(Ipv6Addr::LOCALHOST), 0)) else { return };
        let p = only.local_addr().unwrap().port();
        let _v6 = TcpStream::connect((Ipv6Addr::LOCALHOST, p)).expect("IPv6 to ::1");
        assert!(only.accept().is_ok());
        let dual = socket(SocketAddr::new(IpAddr::V6(Ipv6Addr::UNSPECIFIED), 0), Type::STREAM, Protocol::TCP).unwrap();
        assert!(!dual.only_v6().unwrap(), "`::` is set to the dual stack before its bind");
        assert!(
            socket(SocketAddr::new(IpAddr::V6(Ipv6Addr::LOCALHOST), 0), Type::DGRAM, Protocol::UDP)
                .unwrap()
                .only_v6()
                .unwrap()
        );
    }
}
