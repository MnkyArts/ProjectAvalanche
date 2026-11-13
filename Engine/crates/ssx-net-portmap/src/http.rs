use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, TcpStream};
use std::time::{Duration, Instant};

use socket2::{Domain, Protocol as SockProtocol, Socket, Type};

use crate::Error;

// PORT(choice): 8 KiB of head (hyper's smallest read buffer, the server's own limit), 64 KiB of body (a gateway's
// description is 2 to 10 KiB, a SOAP answer under 1 KiB), 32 header lines; tuning: the largest real gateways' answers;
// the original has no dedicated server.
pub const MAX_HEAD: usize = 8 * 1024;
pub const MAX_BODY: usize = 64 * 1024;
const MAX_HEADERS: usize = 32;
// PORT(choice): 2 s to connect, 5 s in all (a gateway on the LAN answers in milliseconds; a busy one in a second or
// two); tuning: the times of real gateways under load; the original has no dedicated server.
pub const CONNECT: Duration = Duration::from_secs(2);
pub const EXCHANGE: Duration = Duration::from_secs(5);

#[derive(Debug)]
pub struct Response {
    pub status: u16,
    pub body: Vec<u8>,
}

pub fn request(
    from: Option<Ipv4Addr>,
    to: SocketAddrV4,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: &[u8],
) -> Result<Response, Error> {
    let started = Instant::now();
    let stream = connect(from, to)?;
    let mut head = format!(
        "{method} {path} HTTP/1.1\r\nHost: {to}\r\nConnection: close\r\nUser-Agent: Project Avalanche server\r\n"
    );
    for (k, v) in headers {
        head.push_str(&format!("{k}: {v}\r\n"));
    }
    if method == "POST" {
        head.push_str(&format!("Content-Length: {}\r\n", body.len()));
    }
    head.push_str("\r\n");
    let mut stream = Deadline { stream, until: started + EXCHANGE };
    stream
        .write_all(head.as_bytes())
        .and_then(|()| stream.write_all(body))
        .map_err(|e| Error::io("sending to the gateway", e))?;
    read_response(&mut stream)
}

fn connect(from: Option<Ipv4Addr>, to: SocketAddrV4) -> Result<TcpStream, Error> {
    let s = Socket::new(Domain::IPV4, Type::STREAM, Some(SockProtocol::TCP)).map_err(|e| Error::io("a socket", e))?;
    if let Some(ip) = from {
        s.bind(&SocketAddr::new(ip.into(), 0).into()).map_err(|e| Error::io("binding the server's address", e))?;
    }
    s.connect_timeout(&SocketAddr::V4(to).into(), CONNECT).map_err(|e| Error::io(&format!("connecting to {to}"), e))?;
    Ok(s.into())
}

struct Deadline {
    stream: TcpStream,
    until: Instant,
}

impl Deadline {
    fn left(&self) -> std::io::Result<Duration> {
        let left = self.until.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(std::io::Error::new(std::io::ErrorKind::TimedOut, "no answer in time"));
        }
        Ok(left)
    }
}

fn timed_out(e: std::io::Error) -> std::io::Error {
    if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) {
        std::io::Error::new(std::io::ErrorKind::TimedOut, "no answer in time")
    } else {
        e
    }
}

impl Read for Deadline {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let left = self.left()?;
        self.stream.set_read_timeout(Some(left))?;
        self.stream.read(buf).map_err(timed_out)
    }
}

impl Write for Deadline {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let left = self.left()?;
        self.stream.set_write_timeout(Some(left))?;
        self.stream.write(buf).map_err(timed_out)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.stream.flush()
    }
}

pub fn read_response(r: &mut impl Read) -> Result<Response, Error> {
    let mut buf = Vec::with_capacity(4096);
    let mut chunk = [0u8; 4096];
    let head_end = loop {
        if let Some(i) = find(&buf, b"\r\n\r\n") {
            break i;
        }
        if buf.len() > MAX_HEAD {
            return Err(Error::malformed(format!("HTTP: a head larger than {MAX_HEAD} bytes")));
        }
        let n = r.read(&mut chunk).map_err(|e| Error::io("reading the gateway's answer", e))?;
        if n == 0 {
            return Err(Error::malformed("HTTP: the connection closed before the head ended"));
        }
        buf.extend_from_slice(chunk.get(..n).unwrap_or_default());
    };
    if head_end > MAX_HEAD {
        return Err(Error::malformed(format!("HTTP: a head larger than {MAX_HEAD} bytes")));
    }
    let head = std::str::from_utf8(buf.get(..head_end).unwrap_or_default())
        .map_err(|_| Error::malformed("HTTP: a head that is not UTF-8"))?;
    let mut lines = head.split("\r\n");
    let status_line = lines.next().unwrap_or_default();
    let mut parts = status_line.splitn(3, ' ');
    let version = parts.next().unwrap_or_default();
    if !version.eq_ignore_ascii_case("HTTP/1.1") && !version.eq_ignore_ascii_case("HTTP/1.0") {
        return Err(Error::malformed("HTTP: not an HTTP/1.x answer"));
    }
    let status = parts
        .next()
        .filter(|s| s.len() == 3 && s.bytes().all(|b| b.is_ascii_digit()))
        .and_then(|s| s.parse::<u16>().ok())
        .ok_or_else(|| Error::malformed("HTTP: a status that is not three digits"))?;
    let (mut length, mut chunked) = (None, false);
    for (n, line) in lines.enumerate() {
        if n >= MAX_HEADERS {
            return Err(Error::malformed(format!("HTTP: more than {MAX_HEADERS} header lines")));
        }
        let (name, value) = line.split_once(':').ok_or_else(|| Error::malformed("HTTP: a header line without ':'"))?;
        let value = value.trim();
        if name.eq_ignore_ascii_case("Content-Length") {
            let ok = !value.is_empty() && value.len() <= 10 && value.bytes().all(|b| b.is_ascii_digit());
            let v = ok
                .then(|| value.parse::<usize>().ok())
                .flatten()
                .ok_or_else(|| Error::malformed("HTTP: a Content-Length that is no number"))?;
            if length.replace(v).is_some_and(|old| old != v) {
                return Err(Error::malformed("HTTP: two different Content-Length lines"));
            }
        } else if name.eq_ignore_ascii_case("Transfer-Encoding") {
            if !value.eq_ignore_ascii_case("chunked") {
                return Err(Error::malformed("HTTP: a Transfer-Encoding other than chunked"));
            }
            chunked = true;
        }
    }
    if (300..400).contains(&status) {
        return Err(Error::malformed(format!("HTTP: the gateway answers {status} (a redirect is never followed)")));
    }
    let rest = buf.get(head_end + 4..).unwrap_or_default().to_vec();
    let body = match (chunked, length) {
        (true, Some(_)) => return Err(Error::malformed("HTTP: both chunked and a Content-Length")),
        (true, None) => read_chunked(r, rest)?,
        (false, Some(n)) => {
            if n > MAX_BODY {
                return Err(Error::malformed(format!("HTTP: a body of {n} bytes (at most {MAX_BODY})")));
            }
            read_to(r, rest, Some(n))?
        }
        (false, None) => read_to(r, rest, None)?,
    };
    Ok(Response { status, body })
}

fn read_to(r: &mut impl Read, mut have: Vec<u8>, len: Option<usize>) -> Result<Vec<u8>, Error> {
    let want = len.unwrap_or(MAX_BODY + 1);
    let mut chunk = [0u8; 4096];
    while have.len() < want {
        let n = r.read(&mut chunk).map_err(|e| Error::io("reading the gateway's answer", e))?;
        if n == 0 {
            if len.is_some() {
                return Err(Error::malformed("HTTP: the body ended before its Content-Length"));
            }
            break;
        }
        have.extend_from_slice(chunk.get(..n).unwrap_or_default());
        if have.len() > MAX_BODY {
            return Err(Error::malformed(format!("HTTP: a body larger than {MAX_BODY} bytes")));
        }
    }
    if let Some(n) = len
        && have.len() != n
    {
        return Err(Error::malformed("HTTP: more bytes than the Content-Length"));
    }
    Ok(have)
}

fn read_chunked(r: &mut impl Read, have: Vec<u8>) -> Result<Vec<u8>, Error> {
    let mut input = have;
    let mut chunk = [0u8; 4096];
    let mut more = |input: &mut Vec<u8>| -> Result<(), Error> {
        let n = r.read(&mut chunk).map_err(|e| Error::io("reading the gateway's answer", e))?;
        if n == 0 {
            return Err(Error::malformed("HTTP: the chunked body ended early"));
        }
        if input.len() + n > MAX_BODY + MAX_HEAD {
            return Err(Error::malformed(format!("HTTP: a chunked body larger than {MAX_BODY} bytes")));
        }
        input.extend_from_slice(chunk.get(..n).unwrap_or_default());
        Ok(())
    };
    let mut body = Vec::new();
    let mut at = 0usize;
    loop {
        let line_end = loop {
            if let Some(i) = find(input.get(at..).unwrap_or_default(), b"\r\n") {
                break at + i;
            }
            if input.len() - at > 80 {
                return Err(Error::malformed("HTTP: a chunk size line longer than 80 bytes"));
            }
            more(&mut input)?;
        };
        let line = std::str::from_utf8(input.get(at..line_end).unwrap_or_default())
            .map_err(|_| Error::malformed("HTTP: a chunk size that is not text"))?;
        let size_text = line.split(';').next().unwrap_or_default().trim();
        if size_text.is_empty() || size_text.len() > 8 || !size_text.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(Error::malformed("HTTP: a chunk size that is not hex"));
        }
        let size = usize::from_str_radix(size_text, 16).map_err(|_| Error::malformed("HTTP: a chunk size"))?;
        at = line_end + 2;
        if size == 0 {
            let mut lines = 0;
            loop {
                let end = loop {
                    if let Some(i) = find(input.get(at..).unwrap_or_default(), b"\r\n") {
                        break at + i;
                    }
                    if input.len() - at > MAX_HEAD {
                        return Err(Error::malformed("HTTP: a trailer larger than 8 KiB"));
                    }
                    more(&mut input)?;
                };
                let empty = end == at;
                at = end + 2;
                lines += 1;
                if empty {
                    return Ok(body);
                }
                if lines > MAX_HEADERS {
                    return Err(Error::malformed("HTTP: more than 32 trailer lines"));
                }
            }
        }
        if body.len() + size > MAX_BODY {
            return Err(Error::malformed(format!("HTTP: a chunked body larger than {MAX_BODY} bytes")));
        }
        while input.len() < at + size + 2 {
            more(&mut input)?;
        }
        body.extend_from_slice(input.get(at..at + size).unwrap_or_default());
        if input.get(at + size..at + size + 2) != Some(b"\r\n") {
            return Err(Error::malformed("HTTP: a chunk not ended by CRLF"));
        }
        at += size + 2;
    }
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(s: &[u8]) -> Result<Response, Error> {
        read_response(&mut std::io::Cursor::new(s.to_vec()))
    }

    #[test]
    fn framings_and_caps() {
        let r = read(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nhello").expect("length");
        assert_eq!((r.status, r.body.as_slice()), (200, &b"hello"[..]));
        let r =
            read(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nabc\r\n2;x=y\r\nde\r\n0\r\nX: 1\r\n\r\n")
                .expect("chunked");
        assert_eq!(r.body, b"abcde");
        assert_eq!(read(b"HTTP/1.0 500 Error\r\n\r\n<fault/>").expect("to the close").body, b"<fault/>");
        let big = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", MAX_BODY + 1);
        assert!(read(big.as_bytes()).unwrap_err().to_string().contains("at most"));
        let mut close = b"HTTP/1.1 200 OK\r\n\r\n".to_vec();
        close.extend(std::iter::repeat_n(b'x', MAX_BODY + 1));
        assert!(read(&close).unwrap_err().to_string().contains("larger"));
        let mut huge_chunk = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n".to_vec();
        huge_chunk.extend_from_slice(b"ffffffff\r\n");
        assert!(read(&huge_chunk).unwrap_err().to_string().contains("larger"));
        assert!(
            read(b"HTTP/1.1 302 Found\r\nLocation: http://10.0.0.9/\r\n\r\n")
                .unwrap_err()
                .to_string()
                .contains("redirect")
        );
        assert!(read(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: gzip\r\n\r\n").is_err());
        assert!(read(b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\nContent-Length: 4\r\n\r\nabcd").is_err());
        assert!(read(b"HTTP/1.1 200 OK\r\nContent-Length: 9\r\n\r\nabc").unwrap_err().to_string().contains("ended"));
        assert!(read(format!("HTTP/1.1 200 OK\r\n{}\r\n", "X: y\r\n".repeat(33)).as_bytes()).is_err());
        assert!(read(&[b'a'; MAX_HEAD + 10]).unwrap_err().to_string().contains("head"));
        assert!(read(b"SSH-2.0-OpenSSH\r\n\r\n").is_err());
    }
}
