use std::collections::VecDeque;
use std::io::{self, Write};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, Once, OnceLock, PoisonError, Weak};
use std::thread::{JoinHandle, ThreadId};
use std::time::SystemTime;

#[cfg(test)]
mod tests;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stream {
    Out,
    Err,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Keep {
    Droppable,
    Always,
}

// PORT(choice): 1 MiB of lines that may be left out (about 5 000 log lines, half a minute of a level's load at its
// densest), 8 MiB in all before a line that is always kept waits for room; the console writes no log.
pub const QUEUE_BYTES: usize = 1 << 20;
pub const QUEUE_BYTES_KEPT: usize = 8 << 20;
const GAP_BYTES: usize = 64;

enum Entry {
    Line(Stream, Vec<u8>),
    Gap { lines: u64, bytes: u64, since: SystemTime },
}

#[derive(Default)]
struct State {
    queue: VecDeque<Entry>,
    bytes: usize,
    queued: u64,
    written: u64,
    running: bool,
    closing: bool,
    writer: Option<ThreadId>,
}

pub struct Output {
    state: Mutex<State>,
    wake: Condvar,
    room: Condvar,
}

pub struct Running {
    output: Arc<Output>,
    thread: Option<JoinHandle<()>>,
}

impl Drop for Running {
    fn drop(&mut self) {
        self.output.lock().closing = true;
        self.output.wake.notify_all();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

struct EndGuard<'a>(&'a Output);

impl Drop for EndGuard<'_> {
    fn drop(&mut self) {
        let mut s = self.0.lock();
        s.running = false;
        s.writer = None;
        s.bytes = 0;
        let left: Vec<Entry> = s.queue.drain(..).collect();
        s.written = s.queued;
        drop(s);
        self.0.wake.notify_all();
        self.0.room.notify_all();
        let mut direct = std_sink;
        let _ = write_batch(&mut direct, left);
    }
}

impl Output {
    pub fn new() -> Arc<Self> {
        Arc::new(Output { state: Mutex::new(State::default()), wake: Condvar::new(), room: Condvar::new() })
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn start(
        self: &Arc<Self>,
        name: &str,
        sink: impl FnMut(Stream, &[u8]) -> io::Result<()> + Send + 'static,
    ) -> io::Result<Running> {
        {
            let mut s = self.lock();
            if s.running {
                return Err(io::Error::new(io::ErrorKind::AlreadyExists, "the output's writer runs already"));
            }
            (s.running, s.closing) = (true, false);
        }
        let output = Arc::clone(self);
        match std::thread::Builder::new().name(name.into()).spawn(move || output.work(sink)) {
            Ok(t) => {
                started(self);
                Ok(Running { output: Arc::clone(self), thread: Some(t) })
            }
            Err(e) => {
                self.lock().running = false;
                Err(e)
            }
        }
    }

    pub fn line(&self, stream: Stream, keep: Keep, bytes: Vec<u8>) -> Result<(), Vec<u8>> {
        let mut s = self.lock();
        if !s.running {
            return Err(bytes);
        }
        let on_writer = s.writer == Some(std::thread::current().id());
        match keep {
            Keep::Droppable if s.bytes + bytes.len() > QUEUE_BYTES => {
                let n = bytes.len() as u64;
                if let Some(Entry::Gap { lines, bytes: b, .. }) = s.queue.back_mut() {
                    *lines += 1;
                    *b += n;
                } else {
                    s.queue.push_back(Entry::Gap { lines: 1, bytes: n, since: SystemTime::now() });
                    s.bytes += GAP_BYTES;
                    s.queued += 1;
                }
                return Ok(());
            }
            Keep::Droppable => {}
            Keep::Always => {
                while s.running && !on_writer && !s.queue.is_empty() && s.bytes + bytes.len() > QUEUE_BYTES_KEPT {
                    s = self.room.wait(s).unwrap_or_else(PoisonError::into_inner);
                }
                if !s.running {
                    return Err(bytes);
                }
            }
        }
        s.bytes += bytes.len();
        s.queued += 1;
        s.queue.push_back(Entry::Line(stream, bytes));
        drop(s);
        self.wake.notify_one();
        Ok(())
    }

    pub fn flush(&self) {
        let me = std::thread::current().id();
        let mut s = self.lock();
        let target = s.queued;
        while s.running && s.writer != Some(me) && s.written < target {
            s = self.room.wait(s).unwrap_or_else(PoisonError::into_inner);
        }
    }

    fn work(&self, mut sink: impl FnMut(Stream, &[u8]) -> io::Result<()>) {
        self.lock().writer = Some(std::thread::current().id());
        let _end = EndGuard(self);
        let mut refused = 0u64;
        loop {
            let batch: Vec<Entry> = {
                let mut s = self.lock();
                while s.queue.is_empty() && !s.closing {
                    s = self.wake.wait(s).unwrap_or_else(PoisonError::into_inner);
                }
                if s.queue.is_empty() {
                    s.running = false;
                    return;
                }
                s.bytes = 0;
                s.queue.drain(..).collect()
            };
            self.room.notify_all();
            let n = batch.len() as u64;
            if refused > 0 {
                let notice = notice(&format!("{refused} writes of the program's output were refused by its stream"));
                if sink(Stream::Err, notice.as_bytes()).is_ok() {
                    refused = 0;
                }
            }
            refused += write_batch(&mut sink, batch);
            self.lock().written += n;
            self.room.notify_all();
        }
    }
}

fn write_batch(sink: &mut impl FnMut(Stream, &[u8]) -> io::Result<()>, batch: Vec<Entry>) -> u64 {
    let mut refused = 0;
    let mut put = |stream: Stream, bytes: &[u8]| {
        if sink(stream, bytes).is_err() {
            refused += 1;
        }
    };
    let mut run: Option<(Stream, Vec<u8>)> = None;
    for e in batch {
        match e {
            Entry::Line(stream, bytes) => match &mut run {
                Some((s, buf)) if *s == stream => buf.extend_from_slice(&bytes),
                _ => {
                    if let Some((s, buf)) = run.replace((stream, bytes)) {
                        put(s, &buf);
                    }
                }
            },
            Entry::Gap { lines, bytes, since } => {
                if let Some((s, buf)) = run.take() {
                    put(s, &buf);
                }
                let text = format!(
                    "{lines} lines of the log ({bytes} bytes) left out here, the first at {}: the log's queue held \
                     {QUEUE_BYTES} bytes while its stream took its time",
                    utc(since)
                );
                put(Stream::Err, notice(&text).as_bytes());
            }
        }
    }
    if let Some((s, buf)) = run {
        put(s, &buf);
    }
    refused
}

fn notice(text: &str) -> String {
    format!("{}  WARN ssx_core::output: log: {text}\n", utc(SystemTime::now()))
}

fn utc(t: SystemTime) -> String {
    let d = t.duration_since(SystemTime::UNIX_EPOCH).unwrap_or_default();
    let secs = d.as_secs();
    let (days, rem) = (secs / 86_400, secs % 86_400);
    let z = days + 719_468;
    let era = z / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + u64::from(month <= 2);
    let (h, m, s) = (rem / 3_600, rem % 3_600 / 60, rem % 60);
    format!("{year:04}-{month:02}-{day:02}T{h:02}:{m:02}:{s:02}.{:06}Z", d.subsec_micros())
}

fn std_sink(stream: Stream, bytes: &[u8]) -> io::Result<()> {
    match stream {
        Stream::Out => {
            let mut out = io::stdout().lock();
            out.write_all(bytes)?;
            out.flush()
        }
        Stream::Err => io::stderr().lock().write_all(bytes),
    }
}

static PROCESS: OnceLock<Arc<Output>> = OnceLock::new();

static STARTED: Mutex<Vec<Weak<Output>>> = Mutex::new(Vec::new());

fn started(output: &Arc<Output>) {
    {
        let mut list = STARTED.lock().unwrap_or_else(PoisonError::into_inner);
        list.retain(|w| w.strong_count() > 0);
        if !list.iter().any(|w| std::ptr::eq(w.as_ptr(), Arc::as_ptr(output))) {
            list.push(Arc::downgrade(output));
        }
    }
    static HOOK: Once = Once::new();
    HOOK.call_once(|| {
        let before = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |p| {
            flush();
            before(p);
            flush();
        }));
    });
}

// PORT(choice): one process-wide queue, as the streams it writes are the process's; the console writes no log.
pub fn start() -> io::Result<Running> {
    PROCESS.get_or_init(Output::new).start("log-writer", std_sink)
}

pub fn flush() {
    let outputs: Vec<Arc<Output>> =
        STARTED.lock().unwrap_or_else(PoisonError::into_inner).iter().filter_map(Weak::upgrade).collect();
    for o in outputs {
        o.flush();
    }
}

pub fn line(stream: Stream, keep: Keep, bytes: Vec<u8>) -> io::Result<()> {
    let bytes = match PROCESS.get() {
        Some(o) => match o.line(stream, keep, bytes) {
            Ok(()) => return Ok(()),
            Err(b) => b,
        },
        None => bytes,
    };
    std_sink(stream, &bytes)
}
