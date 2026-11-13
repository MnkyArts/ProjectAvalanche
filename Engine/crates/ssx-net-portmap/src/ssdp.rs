use std::net::{Ipv4Addr, SocketAddrV4};

use crate::Error;

pub const MULTICAST: SocketAddrV4 = SocketAddrV4::new(Ipv4Addr::new(239, 255, 255, 250), 1900);
pub const SEARCH_TARGET: &str = "urn:schemas-upnp-org:device:InternetGatewayDevice:1";
// PORT(choice): MX 2 (UDA 1.1 allows 1 to 5), 1 500 bytes (one Ethernet frame), 32 lines of at most 512 bytes, a path
// of at most 256 bytes; tuning: real gateways' SSDP answers; the original has no dedicated server.
pub const MX: u32 = 2;
pub const MAX_DATAGRAM: usize = 1500;
const MAX_LINES: usize = 32;
const MAX_LINE: usize = 512;
const MAX_PATH: usize = 256;

pub fn search(host: SocketAddrV4) -> String {
    format!("M-SEARCH * HTTP/1.1\r\nHOST: {host}\r\nMAN: \"ssdp:discover\"\r\nMX: {MX}\r\nST: {SEARCH_TARGET}\r\n\r\n")
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Location {
    pub addr: SocketAddrV4,
    pub path: String,
}

pub fn parse_url(url: &str) -> Result<Location, Error> {
    let rest = url
        .strip_prefix("http://")
        .or_else(|| url.get(..7).filter(|s| s.eq_ignore_ascii_case("http://")).and(url.get(7..)))
        .ok_or_else(|| Error::malformed("a URL that is not http://"))?;
    let (host, path) = match rest.find('/') {
        Some(i) => (rest.get(..i).unwrap_or_default(), rest.get(i..).unwrap_or_default()),
        None => (rest, "/"),
    };
    let (ip, port) = match host.rsplit_once(':') {
        Some((ip, port)) => {
            let ok = !port.is_empty() && port.len() <= 5 && port.bytes().all(|b| b.is_ascii_digit());
            let port = ok.then(|| port.parse::<u16>().ok()).flatten().filter(|p| *p != 0);
            (ip, port.ok_or_else(|| Error::malformed("a URL with a port that is not 1 to 65535"))?)
        }
        None => (host, 80),
    };
    let ip: Ipv4Addr =
        ip.parse().map_err(|_| Error::malformed("a URL whose host is no IPv4 address (no name is looked up)"))?;
    if path.len() > MAX_PATH || !path.bytes().all(|b| (0x21..0x7F).contains(&b)) {
        return Err(Error::malformed("a URL whose path is longer than 256 bytes or not printable"));
    }
    Ok(Location { addr: SocketAddrV4::new(ip, port), path: path.to_string() })
}

pub fn answer(datagram: &[u8]) -> Result<Location, Error> {
    if datagram.len() > MAX_DATAGRAM {
        return Err(Error::malformed(format!("SSDP: an answer of {} bytes (at most {MAX_DATAGRAM})", datagram.len())));
    }
    let text = std::str::from_utf8(datagram).map_err(|_| Error::malformed("SSDP: an answer that is not UTF-8"))?;
    let mut lines = text.split("\r\n");
    let status = lines.next().unwrap_or_default();
    let mut parts = status.splitn(3, ' ');
    if !parts.next().is_some_and(|v| v.eq_ignore_ascii_case("HTTP/1.1")) || parts.next() != Some("200") {
        return Err(Error::malformed("SSDP: not an HTTP/1.1 200 answer"));
    }
    let (mut st, mut location) = (None, None);
    let mut n = 0;
    for line in lines {
        if line.is_empty() {
            break;
        }
        n += 1;
        if n > MAX_LINES || line.len() > MAX_LINE {
            return Err(Error::malformed("SSDP: more than 32 header lines, or a line longer than 512 bytes"));
        }
        let (name, value) = line.split_once(':').ok_or_else(|| Error::malformed("SSDP: a header line without ':'"))?;
        let value = value.trim();
        if name.eq_ignore_ascii_case("ST") {
            if st.replace(value).is_some() {
                return Err(Error::malformed("SSDP: two ST lines"));
            }
        } else if name.eq_ignore_ascii_case("LOCATION") && location.replace(value).is_some() {
            return Err(Error::malformed("SSDP: two LOCATION lines"));
        }
    }
    let st = st.ok_or_else(|| Error::malformed("SSDP: no ST line"))?;
    let gateway_type = [
        "urn:schemas-upnp-org:device:InternetGatewayDevice:1",
        "urn:schemas-upnp-org:device:InternetGatewayDevice:2",
        "urn:schemas-upnp-org:service:WANIPConnection:1",
        "urn:schemas-upnp-org:service:WANIPConnection:2",
        "urn:schemas-upnp-org:service:WANPPPConnection:1",
    ];
    if !gateway_type.contains(&st) {
        return Err(Error::malformed("SSDP: the answer is not a gateway's (its ST)"));
    }
    parse_url(location.ok_or_else(|| Error::malformed("SSDP: no LOCATION line"))?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn answers_and_urls() {
        let ok = "HTTP/1.1 200 OK\r\nCACHE-CONTROL: max-age=120\r\nST: urn:schemas-upnp-org:device:InternetGatewayDevice:1\r\nUSN: uuid:x\r\nEXT:\r\nLOCATION: http://192.168.1.1:5000/rootDesc.xml\r\n\r\n";
        assert_eq!(
            answer(ok.as_bytes()),
            Ok(Location { addr: "192.168.1.1:5000".parse().unwrap(), path: "/rootDesc.xml".into() })
        );
        for bad in [
            ok.replace("200 OK", "301 Moved"),
            ok.replace("LOCATION: http://192.168.1.1:5000", "LOCATION: http://router.lan:5000"),
            ok.replace("http://192.168.1.1:5000", "https://192.168.1.1:5000"),
            ok.replace("http://192.168.1.1:5000", "http://192.168.1.1:0"),
            ok.replace("/rootDesc.xml", "/root desc.xml"),
            ok.replace("InternetGatewayDevice:1", "MediaServer:1"),
            ok.replace("\r\nEXT:", "\r\nEXT:\r\nLOCATION: http://10.0.0.1/x"),
            ok.replace("EXT:\r\n", &"X: y\r\n".repeat(33)),
            ok.replace("LOCATION: http://192.168.1.1:5000/rootDesc.xml\r\n", ""),
            format!("{ok}{}", " ".repeat(1500)),
        ] {
            assert!(answer(bad.as_bytes()).is_err(), "{bad:?}");
        }
        assert_eq!(parse_url("HTTP://10.0.0.1").unwrap().path, "/");
        assert_eq!(parse_url("http://10.0.0.1/a").unwrap().addr.port(), 80);
        assert!(search(MULTICAST).starts_with("M-SEARCH * HTTP/1.1\r\nHOST: 239.255.255.250:1900\r\n"));
    }
}
