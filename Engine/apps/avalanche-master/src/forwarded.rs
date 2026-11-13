use std::fmt;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use crate::address::{canonical, never_a_server};
pub use crate::trust::Holder;

// PORT(choice): 64 networks (a proxy's network or two, IPv4 and IPv6, with room; an operator's choice); the original
// has no master server.
pub const MAX_TRUSTED: usize = 64;
// PORT(choice): 16 entries and 1 024 bytes (one proxy appends one entry; 16 of the longest written form, 45 bytes and
// ", ", are 752 bytes); tuning: the chains a public master behind its proxy sees (its refused-header counter); the
// original has no master server.
pub const MAX_FORWARDED_ENTRIES: usize = 16;
pub const MAX_FORWARDED_BYTES: usize = 1024;
const MAX_ENTRY: usize = 45;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Network {
    addr: IpAddr,
    prefix: u8,
}

pub fn plain_address(s: &[u8]) -> Option<IpAddr> {
    if s.is_empty() || s.len() > MAX_ENTRY || !s.iter().all(|b| b.is_ascii_hexdigit() || *b == b'.' || *b == b':') {
        return None;
    }
    std::str::from_utf8(s).ok()?.parse().ok()
}

impl Network {
    pub fn parse(s: &str) -> Result<Self, String> {
        let (addr, prefix) = match s.split_once('/') {
            Some((a, p)) => (a, Some(p)),
            None => (s, None),
        };
        let addr = plain_address(addr.as_bytes())
            .ok_or_else(|| format!("{:?}: an IPv4 or IPv6 address with an optional /prefix", s))?;
        let width: u8 = if addr.is_ipv4() { 32 } else { 128 };
        let prefix = match prefix {
            None => width,
            Some(p) => {
                let digits = !p.is_empty() && p.len() <= 3 && p.bytes().all(|b| b.is_ascii_digit());
                let n = digits.then(|| p.parse::<u8>().ok()).flatten();
                match n {
                    Some(n) if n <= width && (p == "0" || !p.starts_with('0')) => n,
                    _ => return Err(format!("{s:?}: the prefix is 1 to {width}")),
                }
            }
        };
        if prefix == 0 {
            return Err(format!("{s:?}: /0 trusts every address, and every client could forge its address"));
        }
        if let IpAddr::V6(v6) = addr
            && v6.to_ipv4_mapped().is_some()
        {
            return Err(format!("{s:?}: an IPv4-mapped network; write the IPv4 network"));
        }
        let network = Self { addr: masked(addr, prefix), prefix };
        if network.addr != addr {
            return Err(format!("{s:?}: host bits are set (the network is {network})"));
        }
        Ok(network)
    }

    pub fn contains(&self, ip: IpAddr) -> bool {
        masked(canonical(ip), self.prefix) == self.addr && canonical(ip).is_ipv4() == self.addr.is_ipv4()
    }

    pub fn addr(&self) -> IpAddr {
        self.addr
    }

    pub fn prefix(&self) -> u8 {
        self.prefix
    }
}

fn masked(ip: IpAddr, prefix: u8) -> IpAddr {
    match ip {
        IpAddr::V4(v4) => {
            let mask = u32::MAX.checked_shl(32 - u32::from(prefix.min(32))).unwrap_or(0);
            IpAddr::V4(Ipv4Addr::from(u32::from(v4) & mask))
        }
        IpAddr::V6(v6) => {
            let mask = u128::MAX.checked_shl(128 - u32::from(prefix.min(128))).unwrap_or(0);
            IpAddr::V6(Ipv6Addr::from(u128::from(v6) & mask))
        }
    }
}

impl fmt::Display for Network {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.addr, self.prefix)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Trusted(Vec<Network>);

impl Trusted {
    pub fn new(networks: Vec<Network>) -> Self {
        Self(networks)
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn networks(&self) -> &[Network] {
        &self.0
    }

    pub fn contains(&self, ip: IpAddr) -> bool {
        self.0.iter().any(|n| n.contains(ip))
    }
}

pub trait TrustsPeer {
    fn holder(&self, peer: IpAddr) -> Option<Holder<'_>>;

    fn trusts(&self, peer: IpAddr) -> bool {
        self.holder(peer).is_some()
    }
}

impl TrustsPeer for Trusted {
    fn holder(&self, peer: IpAddr) -> Option<Holder<'_>> {
        self.0.iter().find(|n| n.contains(peer)).map(Holder::Network)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Direct(IpAddr),
    Proxied { proxy: IpAddr, client: IpAddr },
}

impl Source {
    pub fn client(&self) -> IpAddr {
        match *self {
            Source::Direct(ip) => ip,
            Source::Proxied { client, .. } => client,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    Missing,
    TooLong,
    TooMany,
    Empty,
    NotAnAddress,
    NotUnicast,
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Refusal::Missing => f.write_str("a request from a trusted proxy without X-Forwarded-For"),
            Refusal::TooLong => write!(f, "X-Forwarded-For is longer than {MAX_FORWARDED_BYTES} bytes"),
            Refusal::TooMany => write!(f, "X-Forwarded-For has more than {MAX_FORWARDED_ENTRIES} entries"),
            Refusal::Empty => f.write_str("X-Forwarded-For has an empty entry"),
            Refusal::NotAnAddress => f.write_str("an X-Forwarded-For entry is not a plain IP address"),
            Refusal::NotUnicast => f.write_str("the X-Forwarded-For client is not a unicast address"),
        }
    }
}

pub fn source<'a>(
    peer: IpAddr,
    trusted: &impl TrustsPeer,
    lines: impl IntoIterator<Item = &'a [u8]>,
) -> Result<Source, Refusal> {
    let peer = canonical(peer);
    if !trusted.trusts(peer) {
        return Ok(Source::Direct(peer));
    }
    let (mut bytes, mut entries, mut last) = (0usize, 0usize, None);
    for line in lines {
        bytes = bytes.saturating_add(line.len());
        if bytes > MAX_FORWARDED_BYTES {
            return Err(Refusal::TooLong);
        }
        for entry in line.split(|&b| b == b',') {
            entries += 1;
            if entries > MAX_FORWARDED_ENTRIES {
                return Err(Refusal::TooMany);
            }
            let entry = trim_ows(entry);
            if entry.is_empty() {
                return Err(Refusal::Empty);
            }
            last = Some(plain_address(entry).ok_or(Refusal::NotAnAddress)?);
        }
    }
    let client = canonical(last.ok_or(Refusal::Missing)?);
    if never_a_server(client) {
        return Err(Refusal::NotUnicast);
    }
    Ok(Source::Proxied { proxy: peer, client })
}

fn trim_ows(s: &[u8]) -> &[u8] {
    let start = s.iter().position(|&b| b != b' ' && b != b'\t').unwrap_or(s.len());
    let end = s.iter().rposition(|&b| b != b' ' && b != b'\t').map_or(start, |i| i + 1);
    s.get(start..end).unwrap_or(&[])
}

#[cfg(test)]
mod tests;
