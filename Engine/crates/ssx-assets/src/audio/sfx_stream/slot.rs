use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

use anyhow::{Result, bail};

use super::io::{Delivery, Msg, Request};
use super::parse::{BlockKind, BlockType, HEAD_BYTES};
use super::{Ring, StreamFile};
use crate::sfx_queue::{Receiver, RecvTimeoutError, Sender, queue};

#[derive(Debug)]
pub struct Block<'a> {
    pub tag: u64,
    pub ty: BlockType,
    pub samples: u32,
    pub payload: &'a [u8],
    pub last: bool,
}

#[derive(Debug)]
pub enum Next<'a> {
    Block(Block<'a>),
    Pending,
    Idle,
    Failed { tag: u64, error: String },
}

struct Chunk {
    tag: u64,
    buf: Vec<u8>,
}

enum Found {
    Ready { tag: u64, size: usize, ty: BlockType, last: bool },
    Pending,
    Idle,
    Failed { tag: u64, error: String },
}

struct Queued {
    tag: u64,
    kind: BlockKind,
    file: Arc<StreamFile>,
    offset: u64,
    frame: u64,
}

#[derive(Clone, Copy)]
struct Waiting {
    since: u64,
    running: bool,
}

pub struct Slot {
    id: u64,
    io: &'static Sender<Msg>,
    rx: Receiver<Delivery>,
    max_block: usize,
    epoch: u32,
    next_tag: u64,
    queued: VecDeque<Queued>,
    chunks: VecDeque<Chunk>,
    at: usize,
    release_front: bool,
    scratch: Vec<u8>,
    failed: Vec<(u64, String)>,
    complete: Vec<u64>,
    running: bool,
    waiting: Option<Waiting>,
    consumed: Arc<AtomicU64>,
    given: Option<(u64, u64)>,
    released: (u64, u64),
}

static NEXT_SLOT: AtomicU64 = AtomicU64::new(1);

impl Slot {
    pub fn new(ring: Ring) -> Result<Self> {
        let Some(io) = super::io::sender() else { bail!("the stream file thread did not start") };
        let (chunk, chunks) = (ring.chunk(), ring.chunks());
        if chunk == 0 || chunks < 2 {
            bail!("a stream ring of {} bytes", ring.bytes);
        }
        let max_block = (chunks - 1) * chunk;
        let id = NEXT_SLOT.fetch_add(1, Relaxed);
        let (to, rx) = queue();
        let consumed = Arc::new(AtomicU64::new(0));
        let open = Msg::Open { slot: id, ring: ring.bytes, chunk, chunks, max_block, consumed: consumed.clone(), to };
        io.send(open).map_err(|_| anyhow::anyhow!("no file thread"))?;
        Ok(Self {
            id,
            io,
            rx,
            max_block,
            epoch: 0,
            next_tag: 1,
            queued: VecDeque::new(),
            chunks: VecDeque::new(),
            at: 0,
            release_front: false,
            scratch: Vec::new(),
            failed: Vec::new(),
            complete: Vec::new(),
            running: false,
            waiting: None,
            consumed,
            given: None,
            released: (0, 0),
        })
    }

    pub fn queue(&mut self, file: Arc<StreamFile>, offset: u64, kind: BlockKind) -> u64 {
        let tag = self.next_tag;
        self.next_tag += 1;
        let frame = super::frame().0;
        self.queued.push_back(Queued { tag, kind, file: file.clone(), offset, frame });
        let req = Request { tag, file, offset, kind };
        let _ = self.io.send(Msg::Queue { slot: self.id, epoch: self.epoch, req });
        tag
    }

    pub fn queued(&self) -> usize {
        self.queued.len()
    }

    pub fn front(&self) -> Option<u64> {
        self.queued.front().map(|q| q.tag)
    }

    pub fn flush(&mut self) {
        (self.given, self.released) = (None, (0, 0));
        self.epoch = self.epoch.wrapping_add(1);
        let _ = self.io.send(Msg::Flush { slot: self.id, epoch: self.epoch });
        self.queued.clear();
        self.failed.clear();
        self.complete.clear();
        self.release_front = false;
        self.running = false;
        self.waiting = None;
        self.at = 0;
        while let Some(c) = self.chunks.pop_front() {
            self.spend(c.buf);
        }
    }

    pub fn skip_front(&mut self) {
        self.give_back();
        let Some(Queued { tag, .. }) = self.queued.pop_front() else { return };
        let released = if self.released.0 == tag { self.released.1 } else { 0 };
        let _ = self.io.send(Msg::Cancel { slot: self.id, epoch: self.epoch, tag, released });
        self.drop_tag(tag);
    }

    fn give_back(&mut self) {
        let Some((tag, n)) = self.given.take() else { return };
        self.consumed.fetch_add(n, std::sync::atomic::Ordering::Release);
        if self.released.0 == tag {
            self.released.1 += n;
        } else {
            self.released = (tag, n);
        }
    }

    fn spend(&self, buf: Vec<u8>) {
        let _ = self.io.send(Msg::Spent { slot: self.id, buf });
    }

    fn drop_tag(&mut self, tag: u64) {
        self.release();
        while self.chunks.front().is_some_and(|c| c.tag <= tag) {
            if let Some(c) = self.chunks.pop_front() {
                self.spend(c.buf);
            }
            self.at = 0;
        }
        self.failed.retain(|f| f.0 > tag);
        self.complete.retain(|&t| t > tag);
    }

    fn release(&mut self) {
        if std::mem::take(&mut self.release_front)
            && let Some(c) = self.chunks.pop_front()
        {
            self.at = 0;
            self.spend(c.buf);
        }
    }

    fn accept(&mut self, d: Delivery) {
        match d {
            Delivery::Data { epoch, tag, buf, last } => {
                if epoch != self.epoch || !self.queued.iter().any(|q| q.tag == tag) {
                    self.spend(buf);
                    return;
                }
                if last {
                    self.complete.push(tag);
                }
                self.chunks.push_back(Chunk { tag, buf });
            }
            Delivery::Failed { epoch, tag, error } => {
                if epoch == self.epoch {
                    self.failed.push((tag, error));
                }
            }
        }
    }
}

impl Slot {
    pub fn next_block(&mut self) -> Next<'_> {
        let wait = !super::live_thread();
        self.give_back();
        if self.queued_this_frame() {
            return Next::Pending;
        }
        let found = loop {
            self.release();
            while let Ok(d) = self.rx.try_recv() {
                self.accept(d);
            }
            let got = match self.find() {
                Found::Pending if wait => self.rx.recv().map_err(|_| ()),
                Found::Pending => match self.frame_due() {
                    Some(left) => {
                        let since = std::time::Instant::now();
                        let got = self.rx.recv_timeout(left);
                        super::wait::WAITED_NS.fetch_add(since.elapsed().as_nanos() as u64, Relaxed);
                        match got {
                            Ok(d) => Ok(d),
                            Err(RecvTimeoutError::Timeout) => break Found::Pending,
                            Err(RecvTimeoutError::Disconnected) => Err(()),
                        }
                    }
                    None => break Found::Pending,
                },
                f => break f,
            };
            match got {
                Ok(d) => self.accept(d),
                Err(()) => {
                    let tag = self.front().unwrap_or(0);
                    break Found::Failed { tag, error: "the stream file thread is gone".into() };
                }
            }
        };
        match found {
            Found::Ready { tag, size, ty, last } => self.take(tag, size, ty, last),
            Found::Pending => {
                super::count_late(self.running);
                if self.waiting.is_none() {
                    let since = crate::sfx::output::stats::RENDERED.load(std::sync::atomic::Ordering::Relaxed);
                    self.waiting = Some(Waiting { since, running: self.running });
                }
                Next::Pending
            }
            Found::Idle => {
                self.running = false;
                self.waiting = None;
                Next::Idle
            }
            Found::Failed { tag, error } => {
                self.waiting = None;
                if self.front() == Some(tag) {
                    self.queued.pop_front();
                }
                self.drop_tag(tag);
                Next::Failed { tag, error }
            }
        }
    }

    fn available(&self, tag: u64) -> usize {
        let mut n = 0;
        for (i, c) in self.chunks.iter().enumerate() {
            if c.tag != tag {
                break;
            }
            n += c.buf.len() - if i == 0 { self.at } else { 0 };
        }
        n
    }

    fn peek(&self, out: &mut [u8]) {
        let mut done = 0;
        for (i, c) in self.chunks.iter().enumerate() {
            let from = if i == 0 { self.at } else { 0 };
            let n = (c.buf.len() - from).min(out.len() - done);
            out[done..done + n].copy_from_slice(&c.buf[from..from + n]);
            done += n;
            if done == out.len() {
                break;
            }
        }
    }

    fn find(&mut self) -> Found {
        let Some(&Queued { tag, kind, .. }) = self.queued.front() else { return Found::Idle };
        while self.chunks.front().is_some_and(|c| c.tag < tag) {
            if let Some(c) = self.chunks.pop_front() {
                self.spend(c.buf);
            }
            self.at = 0;
        }
        let not_there = |s: &Self| match s.failed.iter().find(|f| f.0 == tag) {
            Some((_, e)) => Found::Failed { tag, error: e.clone() },
            None if s.complete.contains(&tag) => {
                Found::Failed { tag, error: "the stream's bytes end inside a block".into() }
            }
            None => Found::Pending,
        };
        let avail = self.available(tag);
        let need = kind.head_bytes();
        if avail < need {
            return not_there(self);
        }
        let mut word = [0u8; HEAD_BYTES];
        self.peek(&mut word[..need]);
        let head = match kind.head(&word[..need], self.max_block) {
            Ok(h) => h,
            Err(e) => return Found::Failed { tag, error: format!("{e:#}") },
        };
        if head.ty == BlockType::Other {
            return Found::Failed { tag, error: "an SPS U block (none is on the disc)".into() };
        }
        if avail < head.size {
            return not_there(self);
        }
        Found::Ready { tag, size: head.size, ty: head.ty, last: head.last }
    }

    fn queued_this_frame(&self) -> bool {
        let n = super::frame().0;
        n != 0 && self.queued.front().is_some_and(|q| q.frame == n)
    }

    fn frame_due(&self) -> Option<std::time::Duration> {
        let step = super::frame().1?;
        step.checked_duration_since(std::time::Instant::now()).filter(|d| !d.is_zero())
    }

    fn take(&mut self, tag: u64, size: usize, ty: BlockType, last: bool) -> Next<'_> {
        if let Some(w) = self.waiting.take()
            && let Some(q) = self.queued.front()
        {
            let now = crate::sfx::output::stats::RENDERED.load(std::sync::atomic::Ordering::Relaxed);
            super::wait::record(super::wait::WaitRecord::new(
                now.saturating_sub(w.since),
                w.running,
                q.file.name(),
                q.offset,
            ));
        }
        self.running = true;
        self.given = Some((tag, size as u64));
        if last {
            self.queued.pop_front();
            self.complete.retain(|&t| t != tag);
        }
        let front_len = self.chunks.front().map_or(0, |c| c.buf.len());
        let whole = self.at + size <= front_len;
        let start = self.at;
        if whole {
            self.at += size;
            self.release_front = self.at == front_len;
        } else {
            self.scratch.clear();
            let mut left = size;
            while left > 0 {
                let Some(c) = self.chunks.front() else { break };
                let n = (c.buf.len() - self.at).min(left);
                self.scratch.extend_from_slice(&c.buf[self.at..self.at + n]);
                left -= n;
                self.at += n;
                if self.at < c.buf.len() {
                    continue;
                }
                if left == 0 {
                    self.release_front = true;
                } else if let Some(c) = self.chunks.pop_front() {
                    self.at = 0;
                    self.spend(c.buf);
                }
            }
        }
        let block: &[u8] = match (whole, self.chunks.front()) {
            (true, Some(c)) => &c.buf[start..start + size],
            (true, None) => &[],
            (false, _) => &self.scratch,
        };
        let word = |k: usize| block.get(k..k + 4).map_or(0, |w| u32::from_be_bytes([w[0], w[1], w[2], w[3]]));
        let (samples, payload) = match ty {
            BlockType::Data => (word(4), block.get(8..).unwrap_or(&[])),
            BlockType::Header => (0, block.get(4..).unwrap_or(&[])),
            BlockType::End | BlockType::Other => (0, &[][..]),
        };
        Next::Block(Block { tag, ty, samples, payload, last })
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        let _ = self.io.send(Msg::Close { slot: self.id });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sfx_stream::Ring;

    #[test]
    fn a_slot_runs_from_its_first_block_until_idle_or_flushed() {
        let block = |fill: u8, last: bool| {
            let mut b = ((8 + 64) as u32 | if last { 0x8000_0000 } else { 0 }).to_be_bytes().to_vec();
            b.extend(1u32.to_be_bytes());
            b.extend([fill; 64]);
            b
        };
        let file = StreamFile::memory("run", [block(1, false), block(2, true)].concat());
        let mut slot = Slot::new(Ring::SPEECH).unwrap();
        slot.queue(file.clone(), 0, BlockKind::V0);
        assert!(!slot.running);
        assert!(matches!(slot.next_block(), Next::Block(b) if !b.last));
        assert!(slot.running);
        assert!(matches!(slot.next_block(), Next::Block(b) if b.last));
        assert!(slot.running);
        assert!(matches!(slot.next_block(), Next::Idle));
        assert!(!slot.running);
        slot.queue(file, 0, BlockKind::V0);
        assert!(matches!(slot.next_block(), Next::Block(_)));
        slot.flush();
        assert!(!slot.running);
    }

    #[test]
    fn the_live_mixer_has_a_new_stream_from_the_next_frame_on() {
        let mut b = ((8 + 64) as u32 | 0x8000_0000).to_be_bytes().to_vec();
        b.extend(1u32.to_be_bytes());
        b.extend([7u8; 64]);
        let file = StreamFile::memory("live", b);
        crate::sfx_stream::mark_live_thread();
        let step = || Some(std::time::Instant::now() + std::time::Duration::from_secs(5));
        let mut slot = Slot::new(Ring::SPEECH).unwrap();
        crate::sfx_stream::live_frame(4, None);
        slot.queue(file, 0, BlockKind::V0);
        crate::sfx_stream::live_frame(4, step());
        std::thread::sleep(std::time::Duration::from_millis(50));
        assert!(matches!(slot.next_block(), Next::Pending), "the queueing frame");
        crate::sfx_stream::live_frame(5, step());
        assert!(matches!(slot.next_block(), Next::Block(b) if b.last && b.payload == [7u8; 64]));
        assert!(matches!(slot.next_block(), Next::Idle));
    }
}
