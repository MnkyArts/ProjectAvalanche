use std::fmt;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use crate::forwarded::{Holder, TrustsPeer};

pub const ROUTE_V4: &str = "/proc/net/route";
pub const ROUTE_V6: &str = "/proc/net/ipv6_route";
// PORT(choice): 1 MiB (8 192 IPv4 routes: the kernel pads each line to 128 bytes; a container has 2 to 4, a host a few
// dozen; a router holding a full Internet table runs no master); the original has no master server.
pub const MAX_TABLE: usize = 1 << 20;

const HEADER_V4: &str = "Iface\tDestination\tGateway \tFlags\tRefCnt\tUse\tMetric\tMask\t\tMTU\tWindow\tIRTT";
const UP_GATEWAY: u32 = 0x0003;
const MAX_IFACE: usize = 15;

#[derive(Debug, PartialEq, Eq)]
pub enum RouteError {
    Unsupported,
    Read { path: &'static str, error: String },
    TooLarge { path: &'static str },
    Header,
    Line { path: &'static str, line: usize, what: &'static str },
}

impl fmt::Display for RouteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RouteError::Unsupported => write!(f, "this system has no {ROUTE_V4} (Linux only)"),
            RouteError::Read { path, error } => write!(f, "{path}: {error}"),
            RouteError::TooLarge { path } => write!(f, "{path} is larger than {MAX_TABLE} bytes"),
            RouteError::Header => write!(f, "{ROUTE_V4} does not start with the kernel's header"),
            RouteError::Line { path, line, what } => write!(f, "{path} line {line}: {what}"),
        }
    }
}

pub fn parse_v4(text: &str) -> Result<Vec<IpAddr>, RouteError> {
    let line_error = |line, what| RouteError::Line { path: ROUTE_V4, line, what };
    let mut lines = lines(text, ROUTE_V4)?.into_iter();
    match lines.next() {
        Some((_, header)) if header.trim_end_matches(' ') == HEADER_V4 => {}
        _ => return Err(RouteError::Header),
    }
    let mut gateways = Vec::new();
    for (n, line) in lines {
        let fields: Vec<&str> = line.trim_end_matches(' ').split('\t').collect();
        let [iface, destination, gateway, flags, refcnt, used, metric, mask, mtu, window, irtt] = fields[..] else {
            return Err(line_error(n, "not 11 tab-separated fields"));
        };
        if !interface(iface) {
            return Err(line_error(n, "not an interface name"));
        }
        let (Some(destination), Some(gateway), Some(flags), Some(mask)) =
            (hex(destination, 8), hex(gateway, 8), hex(flags, 4), hex(mask, 8))
        else {
            return Err(line_error(n, "an address, a mask or the flags are not hexadecimal of the kernel's width"));
        };
        if ![refcnt, used, metric, mtu, window, irtt].into_iter().all(decimal) {
            return Err(line_error(n, "a counter is not a decimal number"));
        }
        if destination == 0 && mask == 0 && flags as u32 & UP_GATEWAY == UP_GATEWAY {
            push(&mut gateways, IpAddr::V4(Ipv4Addr::from((gateway as u32).to_ne_bytes())));
        }
    }
    Ok(gateways)
}

pub fn parse_v6(text: &str) -> Result<Vec<IpAddr>, RouteError> {
    let line_error = |line, what| RouteError::Line { path: ROUTE_V6, line, what };
    let mut gateways = Vec::new();
    for (n, line) in lines(text, ROUTE_V6)? {
        let fields: Vec<&str> = line.split_ascii_whitespace().collect();
        let (numbers, device) = match fields.len() {
            10 => (&fields[..9], fields[9]),
            9 => (&fields[..], ""),
            _ => return Err(line_error(n, "not 9 or 10 fields")),
        };
        if !device.is_empty() && !interface(device) {
            return Err(line_error(n, "not an interface name"));
        }
        let widths = [32, 2, 32, 2, 32, 8, 8, 8, 8];
        let parsed: Option<Vec<u128>> = numbers.iter().zip(widths).map(|(f, w)| hex(f, w)).collect();
        let Some(Ok([destination, prefix, _, _, next_hop, _, _, _, flags])) = parsed.map(<[u128; 9]>::try_from) else {
            return Err(line_error(n, "a field is not hexadecimal of the kernel's width"));
        };
        if destination == 0 && prefix == 0 && flags as u32 & UP_GATEWAY == UP_GATEWAY {
            push(&mut gateways, IpAddr::V6(Ipv6Addr::from(next_hop)));
        }
    }
    Ok(gateways)
}

#[cfg(target_os = "linux")]
pub fn default_gateways() -> Result<Vec<IpAddr>, RouteError> {
    let mut gateways = parse_v4(
        &read(ROUTE_V4)?.ok_or(RouteError::Read { path: ROUTE_V4, error: "missing (is /proc mounted?)".into() })?,
    )?;
    if let Some(v6) = read(ROUTE_V6)? {
        parse_v6(&v6)?.into_iter().for_each(|g| push(&mut gateways, g));
    }
    Ok(gateways)
}

#[cfg(not(target_os = "linux"))]
pub fn default_gateways() -> Result<Vec<IpAddr>, RouteError> {
    Err(RouteError::Unsupported)
}

const INSIDE_HAZARD: &str = "Docker hands a published port's loopback and IPv6 connections, and the host's own, to \
                             the master from it, and they can name any client: publish no port, and trust only the \
                             proxy (its name, or a range that leaves the gateway out)";

pub fn gateway_lines(found: &Result<Vec<IpAddr>, RouteError>, trusted: &impl TrustsPeer) -> Vec<String> {
    let gateways = match found {
        Ok(g) => g,
        Err(e) => return vec![format!("note: {}", not_read(e))],
    };
    if gateways.is_empty() {
        return vec!["default gateway: none (no default route)".into()];
    }
    gateways
        .iter()
        .map(|g| match trusted.holder(*g) {
            Some(Holder::Network(net)) => {
                format!("warning: the default gateway {g} lies inside the trusted network {net}: {INSIDE_HAZARD}")
            }
            Some(Holder::Name(name)) => {
                format!("warning: the default gateway {g} is an address of the trusted name {name}: {INSIDE_HAZARD}")
            }
            None => format!("default gateway {g}: not a trusted proxy's address"),
        })
        .collect()
}

#[derive(Debug, PartialEq, Eq)]
pub enum StartLine {
    Warning(String),
    NotRead { text: String, unsupported: bool },
}

pub fn start_line(found: &Result<Vec<IpAddr>, RouteError>, trusted: &impl TrustsPeer) -> Option<StartLine> {
    let gateways = match found {
        Ok(g) => g,
        Err(e) => {
            return Some(StartLine::NotRead { text: not_read(e), unsupported: matches!(e, RouteError::Unsupported) });
        }
    };
    let inside: Vec<String> = gateways
        .iter()
        .filter_map(|g| trusted.holder(*g).map(|holder| (g, holder)))
        .map(|(g, holder)| match holder {
            Holder::Network(net) => format!("{g} (in {net})"),
            Holder::Name(name) => format!("{g} (an address of {name})"),
        })
        .collect();
    match inside.as_slice() {
        [] => None,
        [one] => {
            Some(StartLine::Warning(format!("the default gateway {one} is a trusted proxy's address: {INSIDE_HAZARD}")))
        }
        many => Some(StartLine::Warning(format!(
            "the default gateways {} are trusted proxies' addresses: {INSIDE_HAZARD}",
            many.join(", ")
        ))),
    }
}

fn not_read(e: &RouteError) -> String {
    format!("the default gateway was not read ({e}): whether it lies in a trusted network is not known")
}

#[cfg(target_os = "linux")]
fn read(path: &'static str) -> Result<Option<String>, RouteError> {
    use std::io::Read;
    let file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(RouteError::Read { path, error: e.to_string() }),
    };
    let mut bytes = Vec::new();
    file.take(MAX_TABLE as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| RouteError::Read { path, error: e.to_string() })?;
    if bytes.len() > MAX_TABLE {
        return Err(RouteError::TooLarge { path });
    }
    String::from_utf8(bytes).map(Some).map_err(|_| RouteError::Read { path, error: "not UTF-8".into() })
}

fn lines<'t>(text: &'t str, path: &'static str) -> Result<Vec<(usize, &'t str)>, RouteError> {
    if text.is_empty() {
        return Ok(Vec::new());
    }
    let Some(body) = text.strip_suffix('\n') else {
        return Err(RouteError::Line { path, line: text.split('\n').count(), what: "the last line does not end" });
    };
    Ok(body.split('\n').enumerate().map(|(i, l)| (i + 1, l)).collect())
}

fn hex(s: &str, width: usize) -> Option<u128> {
    if s.len() == width && s.bytes().all(|b| b.is_ascii_hexdigit()) { u128::from_str_radix(s, 16).ok() } else { None }
}

fn decimal(s: &str) -> bool {
    let digits = s.strip_prefix('-').unwrap_or(s);
    !digits.is_empty() && digits.len() <= 20 && digits.bytes().all(|b| b.is_ascii_digit())
}

fn interface(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= MAX_IFACE
        && !s.chars().any(|c| c.is_whitespace() || c.is_control() || c == '/' || c == ':')
}

fn push(gateways: &mut Vec<IpAddr>, g: IpAddr) {
    if !gateways.contains(&g) {
        gateways.push(g);
    }
}

#[cfg(test)]
mod tests;
