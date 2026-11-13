use std::fmt;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

pub fn canonical(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or(IpAddr::V6(v6), IpAddr::V4),
        v4 => v4,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Range {
    V4 { net: u32, len: u8 },
    V6 { net: u128, len: u8 },
    Wildcard { net: u32, fixed: u8 },
}

impl Range {
    pub fn address(ip: IpAddr) -> Self {
        match canonical(ip) {
            IpAddr::V4(v4) => Self::V4 { net: u32::from(v4), len: 32 },
            IpAddr::V6(v6) => Self::V6 { net: u128::from(v6), len: 128 },
        }
    }

    pub fn is_address(&self) -> bool {
        matches!(self, Self::V4 { len: 32, .. } | Self::V6 { len: 128, .. })
    }

    pub fn contains(&self, ip: IpAddr) -> bool {
        match (*self, canonical(ip)) {
            (Self::V4 { net, len }, IpAddr::V4(a)) => u32::from(a) & mask32(len) == net,
            (Self::V6 { net, len }, IpAddr::V6(a)) => u128::from(a) & mask128(len) == net,
            (Self::Wildcard { net, fixed }, IpAddr::V4(a)) => u32::from(a) & mask32(fixed * 8) == net,
            _ => false,
        }
    }

    pub fn parse(text: &str) -> Result<Self, String> {
        let shown = ssx_core::text::quoted(text);
        if text.len() > 64 {
            return Err(format!("{shown} is not an address or a range (too long)"));
        }
        if text.contains('*') {
            return wildcard(text).ok_or_else(|| {
                format!("{shown} is not an IPv4 wildcard (`192.0.2.*`, `192.0.*.*`, `10.*.*.*`: trailing octets only)")
            });
        }
        let (addr, len) = match text.split_once('/') {
            Some((a, l)) => (a, Some(l)),
            None => (text, None),
        };
        let ip: IpAddr = addr.parse().map_err(|_| format!("{shown} is not an IPv4 or IPv6 address or prefix"))?;
        let len = match len {
            None => None,
            Some(l)
                if !l.is_empty()
                    && l.len() <= 3
                    && l.bytes().all(|b| b.is_ascii_digit())
                    && (l == "0" || !l.starts_with('0')) =>
            {
                Some(l.parse::<u8>().map_err(|_| format!("{shown}: the prefix length is not a number"))?)
            }
            Some(_) => return Err(format!("{shown}: the prefix length is not a number")),
        };
        match (ip, len) {
            (IpAddr::V4(v4), len) => prefix4(u32::from(v4), len.unwrap_or(32), &shown),
            (IpAddr::V6(v6), len) => {
                let len = len.unwrap_or(128);
                if let Some(v4) = v6.to_ipv4_mapped().filter(|_| len >= 96) {
                    return prefix4(u32::from(v4), len - 96, &shown);
                }
                if len == 0 || len > 128 {
                    return Err(format!("{shown}: an IPv6 prefix is 1 to 128 bits"));
                }
                let net = u128::from(v6);
                if net & !mask128(len) != 0 {
                    return Err(format!("{shown}: bits after the prefix are set (write the network's first address)"));
                }
                Ok(Self::V6 { net, len })
            }
        }
    }
}

fn prefix4(net: u32, len: u8, shown: &impl fmt::Display) -> Result<Range, String> {
    if len == 0 || len > 32 {
        return Err(format!("{shown}: an IPv4 prefix is 1 to 32 bits"));
    }
    if net & !mask32(len) != 0 {
        return Err(format!("{shown}: bits after the prefix are set (write the network's first address)"));
    }
    Ok(Range::V4 { net, len })
}

// PORT(choice): trailing `*` octets only (`192.0.*.*`, not `192.*.2.1`), never all four; MTA's banlist matching was
// recalled, not re-read (this round had no network): it compares the octets one by one, so a middle `*` may work there;
// tuning: MTA's CBanManager source and what operators write; the original has no dedicated server.
fn wildcard(text: &str) -> Option<Range> {
    let parts: Vec<&str> = text.split('.').collect();
    if parts.len() != 4 {
        return None;
    }
    let fixed = parts.iter().take_while(|p| **p != "*").count();
    if fixed == 0 || fixed == 4 || !parts[fixed..].iter().all(|p| *p == "*") {
        return None;
    }
    let mut net = 0u32;
    for (k, p) in parts[..fixed].iter().enumerate() {
        if p.is_empty() || p.len() > 3 || !p.bytes().all(|b| b.is_ascii_digit()) || (p.len() > 1 && p.starts_with('0'))
        {
            return None;
        }
        let octet: u8 = p.parse().ok()?;
        net |= u32::from(octet) << (24 - 8 * k);
    }
    Some(Range::Wildcard { net, fixed: fixed as u8 })
}

fn mask32(len: u8) -> u32 {
    if len == 0 { 0 } else { u32::MAX << (32 - u32::from(len.min(32))) }
}

fn mask128(len: u8) -> u128 {
    if len == 0 { 0 } else { u128::MAX << (128 - u32::from(len.min(128))) }
}

impl fmt::Display for Range {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::V4 { net, len: 32 } => write!(f, "{}", Ipv4Addr::from(net)),
            Self::V4 { net, len } => write!(f, "{}/{len}", Ipv4Addr::from(net)),
            Self::V6 { net, len: 128 } => write!(f, "{}", Ipv6Addr::from(net)),
            Self::V6 { net, len } => write!(f, "{}/{len}", Ipv6Addr::from(net)),
            Self::Wildcard { net, fixed } => {
                let o = net.to_be_bytes();
                for (k, octet) in o.iter().enumerate() {
                    let dot = if k == 0 { "" } else { "." };
                    if k < usize::from(fixed) { write!(f, "{dot}{octet}")? } else { write!(f, "{dot}*")? }
                }
                Ok(())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn addresses_prefixes_and_wildcards() {
        let cases = [
            ("192.0.2.7", "192.0.2.7", &["192.0.2.7", "::ffff:192.0.2.7"][..], &["192.0.2.8"][..]),
            ("::ffff:192.0.2.7", "192.0.2.7", &["192.0.2.7"], &["::1"]),
            ("192.0.2.0/24", "192.0.2.0/24", &["192.0.2.0", "192.0.2.255", "::ffff:192.0.2.9"], &["192.0.3.1"]),
            ("::ffff:192.0.2.0/120", "192.0.2.0/24", &["192.0.2.200"], &["192.0.1.1"]),
            ("192.0.2.*", "192.0.2.*", &["192.0.2.1", "::ffff:192.0.2.250"], &["192.0.3.1", "2001:db8::1"]),
            ("10.*.*.*", "10.*.*.*", &["10.255.0.1"], &["11.0.0.0"]),
            ("2001:db8::/32", "2001:db8::/32", &["2001:db8:ffff::1"], &["2001:db9::1", "192.0.2.1"]),
            ("2001:db8:1:2::/64", "2001:db8:1:2::/64", &["2001:db8:1:2:aaaa::1"], &["2001:db8:1:3::1"]),
            ("2001:db8::1", "2001:db8::1", &["2001:db8::1"], &["2001:db8::2"]),
        ];
        for (text, shown, inside, outside) in cases {
            let r = Range::parse(text).unwrap_or_else(|e| panic!("{text}: {e}"));
            assert_eq!(r.to_string(), shown, "{text}");
            assert_eq!(Range::parse(shown), Ok(r), "{text} reads back");
            for a in inside {
                assert!(r.contains(ip(a)), "{text} holds {a}");
            }
            for a in outside {
                assert!(!r.contains(ip(a)), "{text} does not hold {a}");
            }
        }
        assert!(Range::parse("192.0.2.7").unwrap().is_address());
        assert!(!Range::parse("192.0.2.*").unwrap().is_address());
    }

    #[test]
    fn hostile_text_is_refused() {
        for bad in [
            "",
            "*",
            "*.*.*.*",
            "192.*.2.1",
            "192.0.2.*.*",
            "192.0.2",
            "192.0.2.256",
            "192.0.02.*",
            "+1.0.2.*",
            "192.0.2.1/24",
            "192.0.2.0/0",
            "192.0.2.0/33",
            "192.0.2.0/",
            "192.0.2.0/+8",
            "192.0.2.0/024",
            "2001:db8::1/32",
            "::/0",
            "2001:db8::/129",
            "::ffff:192.0.2.1/120",
            "example.org",
            "192.0.2.7 ",
            "fe80::1%eth0",
            &"1".repeat(65),
        ] {
            assert!(Range::parse(bad).is_err(), "{bad:?}");
        }
    }
}
