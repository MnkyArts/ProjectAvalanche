mod io;
mod parse;
mod pool;
mod slot;
mod strm;
mod wait;

use std::cell::Cell;
use std::fs::File;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::time::Instant;

use anyhow::{Context as _, Result};

pub use parse::{BlockKind, BlockType};
#[cfg(test)]
pub(crate) use pool::music_pool_test_lock;
pub use pool::{Lease, Pool, UNSTEALABLE, music_pool, new_owner};
pub use slot::{Block, Next, Slot};
pub use wait::{WAITED_DRY, WAITED_FIRST, WAITED_NS, WaitRecord, take_longest};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ring {
    pub bytes: usize,
}

impl Ring {
    pub const SFX_LARGE: Ring = Ring { bytes: 384 << 10 };
    pub const MUSIC: Ring = Ring { bytes: 150 << 10 };
    pub const SPEECH: Ring = Ring { bytes: 73_728 };

    pub const fn chunk(self) -> usize {
        (self.bytes / 6 + 0x7FF) & !0x7FF
    }

    pub const fn chunks(self) -> usize {
        match self.bytes.checked_div(self.chunk()) {
            Some(n) => n,
            None => 0,
        }
    }
}

pub const SFX_LARGE_SLOTS: usize = 5;

pub const MUSIC_SLOTS: usize = 3;

const MAX_HEADER: usize = 32;

pub struct StreamFile {
    name: String,
    src: Src,
    len: u64,
}

enum Src {
    File(File),
    Memory(Vec<u8>),
}

impl StreamFile {
    pub fn open(path: &Path) -> Result<Arc<Self>> {
        let file = File::open(path).with_context(|| format!("opening {}", path.display()))?;
        let len = file.metadata().with_context(|| format!("reading {}", path.display()))?.len();
        Ok(Arc::new(Self { name: path.display().to_string(), src: Src::File(file), len }))
    }

    pub fn memory(name: impl Into<String>, bytes: Vec<u8>) -> Arc<Self> {
        let len = bytes.len() as u64;
        Arc::new(Self { name: name.into(), src: Src::Memory(bytes), len })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn len(&self) -> u64 {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    fn blocks_end(&self, kind: BlockKind, offset: u64, max: u64) -> Result<u64> {
        let mut at = offset;
        loop {
            let need = kind.head_bytes();
            let mut word = [0u8; parse::HEAD_BYTES];
            self.read_exact_at(&mut word[..need], at)?;
            let head = kind.head(&word[..need], usize::try_from(max)?)?;
            at += head.size as u64;
            if at - offset > max {
                anyhow::bail!("{}: a sound at {offset:#x} over {max} bytes", self.name);
            }
            if head.last {
                return Ok(at);
            }
        }
    }

    pub fn read_blocks(&self, kind: BlockKind, offset: u64, max: u64) -> Result<Vec<u8>> {
        let end = self.blocks_end(kind, offset, max)?;
        let mut out = vec![0u8; usize::try_from(end - offset)?];
        self.read_exact_at(&mut out, offset)?;
        Ok(out)
    }

    pub fn read_v0_sound(&self, header: &ssx_formats::eaac::SoundHeader, offset: u64, max: u64) -> Result<Vec<u8>> {
        let mut end = self.blocks_end(BlockKind::V0, offset, max)?;
        if header.has_intro_section() {
            let rel = header.loop_offset.context("a looped stream without a loop offset")?;
            end = end.max(self.blocks_end(BlockKind::V0, offset + u64::from(rel), max)?);
        }
        if end - offset > max {
            anyhow::bail!("{}: a sound at {offset:#x} over {max} bytes", self.name);
        }
        let mut out = vec![0u8; usize::try_from(end - offset)?];
        self.read_exact_at(&mut out, offset)?;
        Ok(out)
    }

    pub fn read_sps_header(&self, offset: u64) -> Result<ssx_formats::eaac::SoundHeader> {
        let mut word = [0u8; 4];
        self.read_exact_at(&mut word, offset)?;
        let word = u32::from_be_bytes(word);
        let size = (word & 0x00FF_FFFF) as usize;
        if (word >> 24) as u8 != b'H' || !(4 + 8..=4 + MAX_HEADER).contains(&size) {
            anyhow::bail!("{}: SPS at {offset:#x} without an H block ({word:#010x})", self.name);
        }
        let mut head = [0u8; MAX_HEADER];
        self.read_exact_at(&mut head[..size - 4], offset + 4)?;
        let header = ssx_formats::eaac::SoundHeader::parse(&head[..size - 4])?;
        if header.version != 1 || header.size + 4 != size {
            anyhow::bail!("{}: SPS H block at {offset:#x}: size {size}, header {header:?}", self.name);
        }
        Ok(header)
    }

    pub fn read_whole(&self, max: u64) -> Result<Vec<u8>> {
        if self.len > max {
            anyhow::bail!("{}: {} bytes, over {max}", self.name, self.len);
        }
        let mut out = vec![0u8; usize::try_from(self.len)?];
        self.read_exact_at(&mut out, 0)?;
        Ok(out)
    }

    pub fn read_exact_at(&self, buf: &mut [u8], at: u64) -> Result<()> {
        let n = self.read_at(buf, at).with_context(|| format!("reading {} at {at:#x}", self.name))?;
        if n < buf.len() {
            anyhow::bail!("{}: {} bytes at {at:#x} past the file's end", self.name, buf.len());
        }
        Ok(())
    }

    fn read_at(&self, buf: &mut [u8], at: u64) -> std::io::Result<usize> {
        match &self.src {
            Src::Memory(b) => {
                let from = usize::try_from(at).unwrap_or(usize::MAX).min(b.len());
                let n = buf.len().min(b.len() - from);
                buf[..n].copy_from_slice(&b[from..from + n]);
                Ok(n)
            }
            Src::File(f) => {
                let mut done = 0;
                while done < buf.len() {
                    match positioned_read(f, &mut buf[done..], at + done as u64) {
                        Ok(0) => break,
                        Ok(n) => done += n,
                        Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                        Err(e) => return Err(e),
                    }
                }
                Ok(done)
            }
        }
    }
}

#[cfg(unix)]
fn positioned_read(f: &File, buf: &mut [u8], at: u64) -> std::io::Result<usize> {
    std::os::unix::fs::FileExt::read_at(f, buf, at)
}

#[cfg(windows)]
fn positioned_read(f: &File, buf: &mut [u8], at: u64) -> std::io::Result<usize> {
    std::os::windows::fs::FileExt::seek_read(f, buf, at)
}

// PORT(choice): a package's song (a mod's file the port decodes on a worker of its own) gets its worker from the file
// thread, so a song opened on the mixer's thread spawns no thread there; the console's MusicBox only streams its own
// `SPS` songs through SndPlayer1.
pub fn spawn(name: &'static str, work: impl FnOnce() + Send + 'static) -> bool {
    io::sender().is_some_and(|io| io.send(io::Msg::Spawn { name, work: Box::new(work) }).is_ok())
}

thread_local! {
    static LIVE: Cell<bool> = const { Cell::new(false) };
    static FRAME: Cell<(u64, Option<Instant>)> = const { Cell::new((0, None)) };
}

pub(crate) fn raise_file_thread() {
    if let Some(io) = io::sender() {
        let _ = io.send(io::Msg::Raise);
    }
}

// PORT(choice): offline renders (`sfx_ride`, `music_mix`, the probes) wait for a stream's reads so their output is
// byte-exact from run to run; only the device's mixer thread waits no longer than its frame comes due. The console has
// only the live mixer.
pub fn mark_live_thread() {
    LIVE.with(|l| l.set(true));
}

pub(crate) fn live_thread() -> bool {
    LIVE.with(Cell::get)
}

pub fn live_frame(n: u64, step: Option<Instant>) {
    FRAME.with(|f| f.set((n, step)));
}

pub(crate) fn frame() -> (u64, Option<Instant>) {
    FRAME.with(Cell::get)
}

pub static LATE_FIRST: AtomicU64 = AtomicU64::new(0);
pub static LATE_DRY: AtomicU64 = AtomicU64::new(0);

pub(crate) fn count_late(running: bool) {
    if running { &LATE_DRY } else { &LATE_FIRST }.fetch_add(1, Relaxed);
}

#[cfg(test)]
mod tests;
