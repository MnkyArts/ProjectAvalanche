use std::net::IpAddr;

// PORT(choice): 256 characters and 4 mirrors (MTA takes one `httpdownloadurl`, FiveM several `fileserver_add`); tuning:
// what operators of public servers configure; the original has no dedicated server.
pub const MAX_MIRROR_URL: usize = 256;
pub const MAX_MIRRORS: usize = 4;

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Host {
    Ip(IpAddr),
    Name(String),
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Mirror {
    pub tls: bool,
    pub host: Host,
    pub port: u16,
    pub path: String,
}

fn dns_name(s: &str) -> Result<String, String> {
    let label = |l: &str| {
        !l.is_empty()
            && l.len() <= 63
            && l.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
            && !l.starts_with('-')
            && !l.ends_with('-')
    };
    if s.is_empty() || s.len() > 253 || !s.split('.').all(label) || s.bytes().all(|b| b.is_ascii_digit() || b == b'.') {
        return Err(format!("{s:?} is not a host name"));
    }
    Ok(s.to_ascii_lowercase())
}

fn port(s: &str) -> Result<u16, String> {
    let ok = !s.is_empty() && s.len() <= 5 && s.bytes().all(|b| b.is_ascii_digit()) && !s.starts_with('0');
    ok.then(|| s.parse::<u16>().ok()).flatten().ok_or_else(|| format!("the port {s:?} is not 1 to 65535"))
}

impl Mirror {
    pub fn parse(text: &str) -> Result<Mirror, String> {
        if text.is_empty() || text.len() > MAX_MIRROR_URL || !text.is_ascii() {
            return Err(format!("a mirror's address has 1 to {MAX_MIRROR_URL} ASCII characters"));
        }
        let (scheme, rest) = text.split_once("://").ok_or("a mirror's address starts with https:// or http://")?;
        let tls = match scheme {
            "https" => true,
            "http" => false,
            _ => return Err("a mirror's address starts with https:// or http:// (lower case)".into()),
        };
        let (authority, path) = match rest.find('/') {
            Some(i) => (&rest[..i], &rest[i..]),
            None => (rest, "/"),
        };
        if let Some(c) = authority.chars().find(|c| matches!(c, '?' | '#' | '@' | '%' | '\\') || !c.is_ascii_graphic())
        {
            return Err(format!("{c:?} in a mirror's host (no user info, query or escape)"));
        }
        let default = if tls { 443 } else { 80 };
        let (host, port) = if let Some(v6) = authority.strip_prefix('[') {
            let (ip, after) = v6.split_once(']').ok_or("an IPv6 address in [ ] has no closing ]")?;
            let ip: std::net::Ipv6Addr = ip.parse().map_err(|_| format!("{ip:?} is not an IPv6 address"))?;
            let p = match after.strip_prefix(':') {
                Some(p) => port(p)?,
                None if after.is_empty() => default,
                None => return Err(format!("{after:?} after the IPv6 address")),
            };
            (Host::Ip(IpAddr::V6(ip)), p)
        } else {
            let (h, p) = match authority.split_once(':') {
                Some((h, p)) => (h, port(p)?),
                None => (authority, default),
            };
            match h.parse::<std::net::Ipv4Addr>() {
                Ok(ip) => (Host::Ip(IpAddr::V4(ip)), p),
                Err(_) => (Host::Name(dns_name(h)?), p),
            }
        };
        if let Host::Ip(ip) = host
            && (ip.is_unspecified() || ip.is_multicast())
        {
            return Err(format!("{ip} is not a mirror's address"));
        }
        let path = if path.ends_with('/') { path.to_string() } else { format!("{path}/") };
        if path != "/" {
            for seg in path[1..path.len() - 1].split('/') {
                let plain = seg.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~'));
                if seg.is_empty() || seg == "." || seg == ".." || !plain {
                    return Err(format!("the path {path:?}: segments of letters, digits, - _ . ~ (no . or ..)"));
                }
            }
        }
        let mirror = Mirror { tls, host, port, path };
        if mirror.to_string().len() > MAX_MIRROR_URL {
            return Err(format!("a mirror's address as written has 1 to {MAX_MIRROR_URL} ASCII characters"));
        }
        Ok(mirror)
    }

    pub fn host_text(&self) -> String {
        match &self.host {
            Host::Ip(ip) => ip.to_string(),
            Host::Name(n) => n.clone(),
        }
    }

    pub fn authority(&self) -> String {
        let host = match &self.host {
            Host::Ip(IpAddr::V6(ip)) => format!("[{ip}]"),
            _ => self.host_text(),
        };
        if self.port == if self.tls { 443 } else { 80 } { host } else { format!("{host}:{}", self.port) }
    }

    pub fn file_url(&self, hash: &crate::hash::ContentHash) -> String {
        format!("{}://{}{}{hash}", if self.tls { "https" } else { "http" }, self.authority(), self.path)
    }
}

impl std::fmt::Display for Mirror {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}://{}{}", if self.tls { "https" } else { "http" }, self.authority(), self.path)
    }
}
