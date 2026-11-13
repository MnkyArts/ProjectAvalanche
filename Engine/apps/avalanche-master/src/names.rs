use std::fmt;
use std::net::{IpAddr, ToSocketAddrs};

use crate::address::{canonical, never_a_server, not_public};

// PORT(choice): 8 names (Coolify runs one proxy container, `coolify-proxy`; room for a second proxy; an operator's
// choice: more names are more lookups every refresh); the original has no master server.
pub const MAX_TRUSTED_HOSTS: usize = 8;
// PORT(choice): 8 (a proxy container on a few Docker networks with an IPv4 and an IPv6 address each; Docker's DNS
// answers the addresses on one shared network; more is an alias several containers share); the original has no master
// server.
pub const MAX_ADDRESSES: usize = 8;
const MAX_NAME: usize = 253;
const MAX_LABEL: usize = 63;

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct HostName(String);

impl HostName {
    pub fn parse(s: &str) -> Result<Self, String> {
        let shown = ssx_core::text::quoted_max(s, 80);
        if s.is_empty() || s.len() > MAX_NAME {
            return Err(format!("{shown}: a host name of 1 to {MAX_NAME} characters"));
        }
        if s.ends_with('.') {
            return Err(format!("{shown}: a name without the trailing dot (it is looked up as an absolute name)"));
        }
        for label in s.split('.') {
            let bytes = label.as_bytes();
            let (Some(first), Some(last)) = (bytes.first(), bytes.last()) else {
                return Err(format!("{shown}: an empty label"));
            };
            if bytes.len() > MAX_LABEL {
                return Err(format!("{shown}: a label is longer than {MAX_LABEL} characters"));
            }
            if !bytes.iter().all(|b| b.is_ascii_alphanumeric() || *b == b'-' || *b == b'_') {
                return Err(format!("{shown}: letters, digits, '-' and '_' only (an IDN in its xn-- form)"));
            }
            if !first.is_ascii_alphanumeric() || !last.is_ascii_alphanumeric() {
                return Err(format!("{shown}: each label starts and ends with a letter or digit"));
            }
        }
        if !s.rsplit('.').next().and_then(|l| l.bytes().next()).is_some_and(|b| b.is_ascii_alphabetic()) {
            return Err(format!("{shown}: the last label starts with a letter (an address belongs in `trusted`)"));
        }
        Ok(Self(s.to_ascii_lowercase()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn absolute(&self) -> String {
        format!("{}.", self.0)
    }
}

impl fmt::Display for HostName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

pub trait Resolve: Send + Sync {
    fn resolve(&self, name: &HostName) -> Result<Vec<IpAddr>, String>;
}

pub struct SystemResolver;

impl Resolve for SystemResolver {
    fn resolve(&self, name: &HostName) -> Result<Vec<IpAddr>, String> {
        let absolute = name.absolute();
        let found = (absolute.as_str(), 0u16).to_socket_addrs().map_err(|e| e.to_string())?;
        let mut addrs: Vec<IpAddr> = Vec::new();
        for ip in found.map(|a| canonical(a.ip())) {
            if !addrs.contains(&ip) {
                addrs.push(ip);
                if addrs.len() > MAX_ADDRESSES {
                    break;
                }
            }
        }
        Ok(addrs)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Answer {
    Trusted(Vec<IpAddr>),
    Refused(String),
}

pub fn judge(addrs: &[IpAddr]) -> Answer {
    let mut seen: Vec<IpAddr> = Vec::new();
    for ip in addrs.iter().map(|&ip| canonical(ip)) {
        if !seen.contains(&ip) {
            seen.push(ip);
            if seen.len() > MAX_ADDRESSES {
                return Answer::Refused(format!("more than {MAX_ADDRESSES} addresses"));
            }
        }
    }
    if seen.is_empty() {
        return Answer::Refused("no address".into());
    }
    if let Some(ip) = seen.iter().find(|ip| never_a_server(**ip)) {
        return Answer::Refused(format!("{ip} is not a unicast address"));
    }
    if let Some(ip) = seen.iter().find(|ip| not_public(**ip).is_none()) {
        return Answer::Refused(format!("{ip} is an address the Internet routes to"));
    }
    Answer::Trusted(seen)
}

pub fn addresses(addrs: &[IpAddr]) -> String {
    addrs.iter().map(ToString::to_string).collect::<Vec<_>>().join(", ")
}

#[cfg(test)]
mod tests;
