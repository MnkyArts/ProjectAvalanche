use std::cell::Cell;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::sync::{Mutex, TryLockError};

pub static WAITED_FIRST: AtomicU64 = AtomicU64::new(0);
pub static WAITED_DRY: AtomicU64 = AtomicU64::new(0);
pub static WAITED_NS: AtomicU64 = AtomicU64::new(0);

const LABEL: usize = 40;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WaitRecord {
    pub frames: u64,
    pub running: bool,
    pub offset: u64,
    label: [u8; LABEL],
    len: u8,
    cut: bool,
}

impl WaitRecord {
    pub fn new(frames: u64, running: bool, name: &str, offset: u64) -> Self {
        let mut from = name.len().saturating_sub(LABEL);
        while !name.is_char_boundary(from) {
            from += 1;
        }
        let tail = &name.as_bytes()[from..];
        let mut label = [0u8; LABEL];
        label[..tail.len()].copy_from_slice(tail);
        Self { frames, running, offset, label, len: tail.len() as u8, cut: from > 0 }
    }

    pub fn name(&self) -> &str {
        std::str::from_utf8(&self.label[..usize::from(self.len)]).unwrap_or("")
    }
}

impl std::fmt::Display for WaitRecord {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let ms = self.frames as f64 * 1000.0 / f64::from(crate::sfx_aems::MIXER_RATE);
        let what = if self.running { "ran dry" } else { "waited for its first block" };
        let cut = if self.cut { "…" } else { "" };
        write!(f, "{ms:.1} ms, {cut}{} at {:#x} {what}", self.name(), self.offset)
    }
}

static LONGEST: Mutex<Option<WaitRecord>> = Mutex::new(None);

thread_local! {
    static STAGED: Cell<Option<WaitRecord>> = const { Cell::new(None) };
}

fn worse(a: &WaitRecord, b: &WaitRecord) -> bool {
    (a.running, a.frames) > (b.running, b.frames)
}

pub(crate) fn record(r: WaitRecord) {
    if r.running { &WAITED_DRY } else { &WAITED_FIRST }.fetch_add(r.frames, Relaxed);
    STAGED.with(|staged| {
        let best = match staged.get() {
            Some(s) if !worse(&r, &s) => s,
            _ => r,
        };
        let mut longest = match LONGEST.try_lock() {
            Ok(longest) => longest,
            Err(TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
            Err(TryLockError::WouldBlock) => {
                staged.set(Some(best));
                return;
            }
        };
        if longest.is_none_or(|s| worse(&best, &s)) {
            *longest = Some(best);
        }
        staged.set(None);
    });
}

pub fn take_longest() -> Option<WaitRecord> {
    LONGEST.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).take()
}
