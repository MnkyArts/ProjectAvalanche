use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};

use super::StreamFile;
use super::parse::BlockKind;
use super::strm::{self, HIGH, ON, Order, Strm};
use crate::sfx_queue::{Receiver, Sender, TryRecvError, queue};

pub(super) struct Request {
    pub tag: u64,
    pub file: Arc<StreamFile>,
    pub offset: u64,
    pub kind: BlockKind,
}

pub(super) enum Msg {
    Open {
        slot: u64,
        ring: usize,
        chunk: usize,
        chunks: usize,
        max_block: usize,
        consumed: Arc<AtomicU64>,
        to: Sender<Delivery>,
    },
    Queue {
        slot: u64,
        epoch: u32,
        req: Request,
    },
    Flush {
        slot: u64,
        epoch: u32,
    },
    Cancel {
        slot: u64,
        epoch: u32,
        tag: u64,
        released: u64,
    },
    Spent {
        slot: u64,
        buf: Vec<u8>,
    },
    Close {
        slot: u64,
    },
    Raise,
    Spawn {
        name: &'static str,
        work: Box<dyn FnOnce() + Send>,
    },
}

pub(super) enum Delivery {
    Data { epoch: u32, tag: u64, buf: Vec<u8>, last: bool },
    Failed { epoch: u32, tag: u64, error: String },
}

pub(super) fn sender() -> Option<&'static Sender<Msg>> {
    static IO: OnceLock<Option<Sender<Msg>>> = OnceLock::new();
    IO.get_or_init(|| {
        let (tx, rx) = queue();
        std::thread::Builder::new().name("ssx_stream_io".into()).spawn(move || run(rx)).ok().map(|_| tx)
    })
    .as_ref()
}

struct SlotIo {
    to: Sender<Delivery>,
    chunk: usize,
    chunks: usize,
    max_block: usize,
    made: usize,
    free: Vec<Vec<u8>>,
    epoch: u32,
    queue: VecDeque<Request>,
    cur: Option<Reading>,
    strm: Strm,
    consumed: Arc<AtomicU64>,
    seen: u64,
}

struct Reading {
    req: Request,
    pos: u64,
    head: u64,
    last_head: u64,
    whole: u64,
    end: Option<u64>,
}

impl Reading {
    fn new(req: Request) -> Self {
        let at = req.offset;
        Self { req, pos: at, head: at, last_head: at, whole: at, end: None }
    }
}

fn run(mut rx: Receiver<Msg>) {
    let mut slots: HashMap<u64, SlotIo> = HashMap::new();
    let mut order = Order::default();
    loop {
        let busy = slots.values().any(|s| s.strm.read.is_some());
        let mut next = if busy {
            match rx.try_recv() {
                Ok(m) => Some(m),
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => return,
            }
        } else {
            match rx.recv() {
                Ok(m) => Some(m),
                Err(_) => return,
            }
        };
        while let Some(m) = next {
            handle(&mut slots, m, &mut order);
            next = rx.try_recv().ok();
        }
        for s in slots.values_mut() {
            s.observe(&mut order);
        }
        let Some(id) = strm::pick(slots.iter().map(|(id, s)| (*id, &s.strm))) else { continue };
        if let Some(s) = slots.get_mut(&id)
            && let Step::Gone = s.read_one(&mut order)
        {
            slots.remove(&id);
        }
    }
}

fn handle(slots: &mut HashMap<u64, SlotIo>, m: Msg, order: &mut Order) {
    match m {
        Msg::Open { slot, ring, chunk, chunks, max_block, consumed, to } => {
            let seen = consumed.load(Ordering::Acquire);
            let s = SlotIo {
                to,
                chunk,
                chunks,
                max_block,
                made: 0,
                free: Vec::new(),
                epoch: 0,
                queue: VecDeque::new(),
                cur: None,
                strm: Strm::new(ring, chunks),
                consumed,
                seen,
            };
            slots.insert(slot, s);
        }
        Msg::Queue { slot, epoch, req } => {
            if let Some(s) = slots.get_mut(&slot).filter(|s| s.epoch == epoch) {
                s.observe(order);
                s.queue.push_back(req);
                if s.cur.is_none() && s.strm.read.is_none() && !s.strm.stalled {
                    s.start_next(s.strm.wake(), order);
                }
            }
        }
        Msg::Flush { slot, epoch } => {
            if let Some(s) = slots.get_mut(&slot) {
                s.observe(order);
                s.epoch = epoch;
                s.queue.clear();
                s.cur = None;
                s.strm.flush(order);
            }
        }
        Msg::Cancel { slot, epoch, tag, released } => {
            if let Some(s) = slots.get_mut(&slot).filter(|s| s.epoch == epoch) {
                s.observe(order);
                s.queue.retain(|r| r.tag != tag);
                s.strm.cancel(tag, released, order);
                if s.cur.as_ref().is_some_and(|r| r.req.tag == tag) {
                    s.cur = None;
                    if !s.strm.stalled {
                        s.start_next(HIGH, order);
                    }
                }
            }
        }
        Msg::Spent { slot, mut buf } => {
            if let Some(s) = slots.get_mut(&slot) {
                s.observe(order);
                buf.clear();
                s.free.push(buf);
                if s.strm.stalled {
                    let priority = s.strm.wake();
                    if s.cur.is_some() {
                        s.strm.want(priority, true, order);
                    } else {
                        s.strm.stalled = false;
                        s.start_next(priority, order);
                    }
                }
            }
        }
        Msg::Close { slot } => {
            slots.remove(&slot);
        }
        Msg::Raise => crate::sfx::output::priority::raise_file_thread(),
        Msg::Spawn { name, work } => {
            let _ = std::thread::Builder::new().name(name.into()).spawn(work);
        }
    }
}

enum Step {
    Read,
    Idle,
    Gone,
}

impl SlotIo {
    fn room(&self) -> bool {
        !self.free.is_empty() || self.made < self.chunks
    }

    fn observe(&mut self, order: &mut Order) {
        let now = self.consumed.load(Ordering::Acquire);
        if now != self.seen {
            self.strm.consume(now.wrapping_sub(self.seen), order);
            self.seen = now;
        }
    }

    fn start_next(&mut self, priority: i32, order: &mut Order) {
        match self.queue.pop_front() {
            Some(req) => {
                self.strm.started(req.tag);
                self.cur = Some(Reading::new(req));
                let room = self.room();
                self.strm.want(priority, room, order);
            }
            None => {
                self.cur = None;
                self.strm.read = None;
                self.strm.stalled = false;
            }
        }
    }

    fn read_one(&mut self, order: &mut Order) -> Step {
        self.strm.read = None;
        let mut buf = match self.free.pop() {
            Some(b) => b,
            None if self.made < self.chunks => {
                self.made += 1;
                Vec::with_capacity(self.chunk)
            }
            None => {
                self.strm.stalled = true;
                return Step::Idle;
            }
        };
        let Some(r) = self.cur.as_mut() else {
            self.free.push(buf);
            return Step::Idle;
        };
        let (epoch, tag) = (self.epoch, r.req.tag);
        let delivery = match r.read(&mut buf, self.chunk, self.max_block) {
            Ok((last, whole)) => {
                self.strm.fill(tag, whole);
                if last {
                    self.start_next(HIGH, order);
                } else {
                    let room = self.room();
                    self.strm.want(ON, room, order);
                }
                Delivery::Data { epoch, tag, buf, last }
            }
            Err(error) => {
                self.free.push(buf);
                // PORT(choice): a failed read of a PC file (an I/O error, a file shorter than its blocks) ends its
                // request as its last block would and the next request reads at the high priority (0x82FB1770's end
                // of a request), so one bad file does not stop the slot; the console reads its own disc.
                self.start_next(HIGH, order);
                Delivery::Failed { epoch, tag, error }
            }
        };
        if self.to.send(delivery).is_err() {
            return Step::Gone;
        }
        Step::Read
    }
}

impl Reading {
    fn read(&mut self, buf: &mut Vec<u8>, chunk: usize, max_block: usize) -> Result<(bool, u64), String> {
        let file = &self.req.file;
        let stop = self.end.unwrap_or(u64::MAX).min(file.len());
        let left = stop.saturating_sub(self.pos);
        if left == 0 {
            return Err(format!("{}: the file ends at {:#x} before the stream's last block", file.name(), self.pos));
        }
        let n = usize::try_from(left).unwrap_or(usize::MAX).min(chunk);
        buf.resize(n, 0);
        let got =
            file.read_at(buf, self.pos).map_err(|e| format!("{}: reading at {:#x}: {e}", file.name(), self.pos))?;
        if got < n {
            return Err(format!("{}: a short read at {:#x}", file.name(), self.pos + got as u64));
        }
        let end = self.pos + n as u64;
        let kind = self.req.kind;
        let need = kind.head_bytes();
        while self.end.is_none() && self.head + need as u64 <= end {
            let mut word = [0u8; super::parse::HEAD_BYTES];
            let at = self.head;
            if at >= self.pos {
                let k = (at - self.pos) as usize;
                word[..need].copy_from_slice(&buf[k..k + need]);
            } else {
                let got = file.read_at(&mut word[..need], at).map_err(|e| format!("{}: {e}", file.name()))?;
                if got < need {
                    return Err(format!("{}: a block head at {at:#x} past the file's end", file.name()));
                }
            }
            let head = kind.head(&word[..need], max_block).map_err(|e| format!("{} at {at:#x}: {e:#}", file.name()))?;
            self.last_head = self.head;
            self.head += head.size as u64;
            if head.last {
                self.end = Some(self.head);
            }
        }
        let whole = match self.end {
            Some(e) if e <= end => e,
            _ if self.head <= end => self.head,
            _ => self.last_head,
        };
        let completed = whole.saturating_sub(self.whole);
        self.whole = self.whole.max(whole);
        match self.end {
            Some(e) if e <= end => {
                buf.truncate((e - self.pos) as usize);
                self.pos = e;
                Ok((true, completed))
            }
            _ => {
                self.pos = end;
                Ok((false, completed))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v0(n: usize, last: bool) -> Vec<u8> {
        let mut b = ((8 + n) as u32 | if last { 0x8000_0000 } else { 0 }).to_be_bytes().to_vec();
        b.extend(1u32.to_be_bytes());
        b.extend(std::iter::repeat_n(0x55, n));
        b
    }

    #[test]
    fn a_read_counts_the_whole_blocks_it_completes() {
        let mut file = vec![0xEE; 4];
        let sizes = [3000usize, 9000, 500, 20_000, 100];
        for (k, &n) in sizes.iter().enumerate() {
            file.extend(v0(n, k == sizes.len() - 1));
        }
        let req = Request { tag: 1, file: StreamFile::memory("t", file), offset: 4, kind: BlockKind::V0 };
        let mut r = Reading::new(req);
        let mut buf = Vec::new();
        let mut got = Vec::new();
        loop {
            let (last, whole) = r.read(&mut buf, 8192, 64 << 10).unwrap();
            got.push(whole);
            if last {
                break;
            }
        }
        assert_eq!(got, [3008, 9516, 0, 20116]);
        assert_eq!(got.iter().sum::<u64>(), sizes.iter().map(|n| 8 + *n as u64).sum::<u64>());
    }
}
