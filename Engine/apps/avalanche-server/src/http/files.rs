use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::task::{Context, Poll};

use bytes::Bytes;
use hyper::body::{Body, Frame, SizeHint};
use hyper::header::{ACCEPT_RANGES, CONTENT_RANGE, HeaderValue};
use ssx_content::ContentHash;
use ssx_content::delivery::range::{ByteRange, RangeError, parse_range};

use super::{HttpState, Reply};
use crate::bridge::Shared;

// PORT(choice): 32 KiB/s (a 256 kbit/s line; a slower reader resumes with a range on its next connection); tuning: the
// downloads of players on public servers; the original has no dedicated server.
pub const MIN_FILE_RATE: u64 = 32 << 10;
const CHUNK: usize = 256 << 10;
pub const MAX_REPORT_BODY: usize = 256;

#[derive(Clone, Default)]
pub struct Progress(pub Arc<AtomicU64>);

pub struct FileBody {
    file: Option<std::fs::File>,
    left: u64,
    reading: Option<tokio::task::JoinHandle<(std::fs::File, std::io::Result<Vec<u8>>)>>,
    progress: Progress,
}

impl Body for FileBody {
    type Data = Bytes;
    type Error = std::io::Error;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, std::io::Error>>> {
        if self.left == 0 {
            return Poll::Ready(None);
        }
        if self.reading.is_none() {
            let Some(mut file) = self.file.take() else { return Poll::Ready(None) };
            let n = self.left.min(CHUNK as u64) as usize;
            self.reading = Some(tokio::task::spawn_blocking(move || {
                use std::io::Read;
                let mut buf = vec![0u8; n];
                let r = file.read_exact(&mut buf).map(|()| buf);
                (file, r)
            }));
        }
        let Some(reading) = self.reading.as_mut() else { return Poll::Ready(None) };
        match Pin::new(reading).poll(cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(Err(e)) => {
                self.reading = None;
                Poll::Ready(Some(Err(std::io::Error::other(e))))
            }
            Poll::Ready(Ok((file, read))) => {
                self.reading = None;
                match read {
                    Ok(buf) => {
                        self.left -= buf.len() as u64;
                        self.progress.0.fetch_add(buf.len() as u64, Ordering::Relaxed);
                        self.file = Some(file);
                        Poll::Ready(Some(Ok(Frame::data(Bytes::from(buf)))))
                    }
                    Err(e) => Poll::Ready(Some(Err(e))),
                }
            }
        }
    }

    fn is_end_stream(&self) -> bool {
        self.left == 0
    }

    fn size_hint(&self) -> SizeHint {
        SizeHint::with_exact(self.left)
    }
}

use std::future::Future;

pub(super) fn manifest(state: &HttpState) -> Reply {
    let s = state.resources.get();
    Reply { content_type: "application/json", body: s.json.clone(), ..Reply::status(200, "") }
}

pub(super) async fn file(state: &HttpState, hash: &str, range: Option<&HeaderValue>, progress: Progress) -> Reply {
    let Ok(hash) = hash.parse::<ContentHash>() else { return Reply::status(404, "not a file of this server") };
    let s = state.resources.get();
    let Some(&size) = s.files.get(&hash) else { return Reply::status(404, "not a file of this server") };
    let range = match range.map(|r| parse_range(r.as_bytes(), size)) {
        None if size == 0 => None,
        None => Some(ByteRange { first: 0, len: size }),
        Some(Ok(r)) => Some(r),
        Some(Err(RangeError::Unreadable)) => return Reply::status(416, "one byte range a request (bytes=a-b)"),
        Some(Err(RangeError::Unsatisfiable)) => {
            let mut r = Reply::status(416, "the range is past the file's end");
            r.headers.push((CONTENT_RANGE, HeaderValue::from_str(&format!("bytes */{size}")).ok()));
            return r;
        }
    };
    let path = s.store.join(hash.to_string());
    let opened = tokio::task::spawn_blocking(move || -> std::io::Result<std::fs::File> {
        use std::io::{Seek, SeekFrom};
        let mut f = ssx_files::open_plain(&path)
            .map_err(ssx_files::Error::into_io)?
            .ok_or_else(|| std::io::Error::other("not a plain file"))?;
        if f.metadata()?.len() != size {
            return Err(std::io::Error::other("the store's copy is not the manifest's size"));
        }
        if let Some(r) = range {
            f.seek(SeekFrom::Start(r.first))?;
        }
        Ok(f)
    })
    .await;
    let file = match opened {
        Ok(Ok(f)) => f,
        Ok(Err(e)) => {
            bevy::log::warn!("resources: {hash}: {e}");
            return Reply::status(500, "the file cannot be read");
        }
        Err(_) => return Reply::status(500, "the file cannot be read"),
    };
    let partial = range.is_some_and(|r| r.len != size);
    let len = range.map_or(0, |r| r.len);
    let mut reply = Reply {
        content_type: "application/octet-stream",
        stream: Some(FileBody { file: Some(file), left: len, reading: None, progress }),
        ..Reply::status(if partial { 206 } else { 200 }, "")
    };
    reply.body = Bytes::new();
    reply.headers.push((ACCEPT_RANGES, Some(HeaderValue::from_static("bytes"))));
    if let (true, Some(r)) = (partial, range) {
        reply.headers.push((CONTENT_RANGE, HeaderValue::from_str(&r.content_range(size)).ok()));
    }
    reply
}

pub(super) fn report(state: &HttpState, peer: std::net::SocketAddr, body: &[u8]) -> Reply {
    let text = std::str::from_utf8(body).unwrap_or("");
    let mut words = text.trim_end_matches('\n').split(' ');
    let hash = words.next().and_then(|h| h.parse::<ContentHash>().ok());
    let s = state.resources.get();
    let source = match (words.next(), words.next(), words.next()) {
        (Some("server"), None, None) => Some("the server".to_string()),
        (Some("mirror"), Some(n), None) => n
            .parse::<usize>()
            .ok()
            .filter(|&i| i < s.manifest.mirrors.len() && n == i.to_string())
            .map(|i| format!("mirror {}", s.manifest.mirrors[i])),
        _ => None,
    };
    let (Some(hash), Some(source)) = (hash, source) else {
        return Reply::status(400, "a report is `<hash> server` or `<hash> mirror <n>`");
    };
    if !s.files.contains_key(&hash) {
        return Reply::status(404, "not a file of this server");
    }
    Shared::count(&state.shared.file_reports);
    bevy::log::warn!("resources: {} reports {source} sent a file that is not {hash}", peer.ip());
    Reply::status(200, "reported")
}
