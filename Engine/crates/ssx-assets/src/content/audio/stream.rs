use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering, fence};
use std::sync::{Arc, OnceLock};
use std::thread::Thread;

use ssx_content::audio::music::LoopRegion;
use ssx_content::audio::{Decoder, Limits};

use super::decide::MusicFile;
use crate::sfx::output::resample::Resampler;
use crate::sfx_queue::{Receiver, Sender, TryRecvError, queue};

pub const MUSIC_RATE: u32 = crate::pathfinder::SAMPLE_RATE;
const BLOCK_FRAMES: usize = 4096;
const AHEAD: usize = 32;

enum Block {
    Frames(Vec<f32>),
    End(Result<(), String>),
}

// PORT(choice): a package song's bound of 32 blocks counted beside an unbounded queue whose receiver never waits for a
// sender (the stream renders on the real-time mixer thread; std's bounded channel has its receiver spin without a
// yield on a slot a sender claimed and has not written); the worker converts a block, then waits for room as the
// bounded send did. Package music is the port's own feature; the console streams only its disc's music.
struct Room {
    queued: AtomicUsize,
    waiting: AtomicBool,
    closed: AtomicBool,
    worker: OnceLock<Thread>,
}

impl Room {
    fn wait(&self) -> bool {
        loop {
            if self.closed.load(Ordering::SeqCst) {
                return false;
            }
            if self.queued.load(Ordering::SeqCst) < AHEAD {
                return true;
            }
            self.waiting.store(true, Ordering::SeqCst);
            if self.queued.load(Ordering::SeqCst) >= AHEAD && !self.closed.load(Ordering::SeqCst) {
                std::thread::park();
            }
            self.waiting.store(false, Ordering::Relaxed);
        }
    }

    fn sent(&self) {
        self.queued.fetch_add(1, Ordering::SeqCst);
    }

    fn taken(&self) {
        self.queued.fetch_sub(1, Ordering::SeqCst);
        if self.waiting.load(Ordering::SeqCst) {
            self.wake();
        }
    }

    fn wake(&self) {
        if let Some(worker) = self.worker.get() {
            worker.unpark();
        }
    }
}

pub struct MusicStream {
    pub name: String,
    rx: Receiver<Block>,
    room: Arc<Room>,
    buf: Vec<f32>,
    pos: usize,
    ended: Option<Result<(), String>>,
    starved: Arc<AtomicU64>,
}

impl MusicStream {
    pub fn open(file: MusicFile, region: Option<LoopRegion>) -> Self {
        let (tx, rx) = queue();
        let room = Arc::new(Room {
            queued: AtomicUsize::new(0),
            waiting: AtomicBool::new(false),
            closed: AtomicBool::new(false),
            worker: OnceLock::new(),
        });
        let name = file.rel.clone();
        let label = name.clone();
        let theirs = room.clone();
        let started = crate::sfx_stream::spawn("ssx_music_stream", move || {
            let _ = theirs.worker.set(std::thread::current());
            fence(Ordering::SeqCst);
            let end = run(&file, region, &tx, &theirs);
            if theirs.wait() {
                theirs.sent();
                let _ = tx.send(Block::End(end));
            }
        });
        let starved = Arc::new(AtomicU64::new(0));
        let mut s = Self { name, rx, room, buf: Vec::new(), pos: 0, ended: None, starved };
        if !started {
            s.ended = Some(Err(format!("{label}: no stream file thread to start the worker")));
        }
        s
    }

    pub fn render(&mut self, out: &mut [f32]) -> usize {
        let frames = out.len() / 2;
        let mut done = 0;
        while done < frames {
            if self.pos * 2 >= self.buf.len() {
                if self.ended.is_some() {
                    break;
                }
                match self.rx.try_recv() {
                    Ok(Block::Frames(b)) => {
                        self.room.taken();
                        (self.buf, self.pos) = (b, 0);
                        continue;
                    }
                    Ok(Block::End(e)) => {
                        self.room.taken();
                        self.ended = Some(e);
                        break;
                    }
                    Err(TryRecvError::Empty) => {
                        self.starved.fetch_add((frames - done) as u64, Ordering::Relaxed);
                        break;
                    }
                    Err(TryRecvError::Disconnected) => {
                        self.ended = Some(Err(format!("{}: the stream's worker ended", self.name)));
                        break;
                    }
                }
            }
            let n = (frames - done).min(self.buf.len() / 2 - self.pos);
            out[done * 2..(done + n) * 2].copy_from_slice(&self.buf[self.pos * 2..(self.pos + n) * 2]);
            self.pos += n;
            done += n;
        }
        out[done * 2..].fill(0.0);
        done
    }

    pub fn ended(&self) -> Option<&Result<(), String>> {
        self.ended.as_ref()
    }

    pub fn starved(&self) -> u64 {
        self.starved.load(Ordering::Relaxed)
    }
}

impl Drop for MusicStream {
    fn drop(&mut self) {
        self.room.closed.store(true, Ordering::SeqCst);
        fence(Ordering::SeqCst);
        self.room.wake();
    }
}

fn run(file: &MusicFile, region: Option<LoopRegion>, tx: &Sender<Block>, room: &Room) -> Result<(), String> {
    let bytes = file.read()?;
    let limits = Limits::music();
    let mut dec = Decoder::open(&file.rel, bytes, &limits).map_err(|e| e.to_string())?;
    let info = *dec.info();
    if info != file.info {
        return Err(format!(
            "{}: the file changed since it was measured ({} frames at {} Hz × {} then, {} at {} Hz × {} now)",
            file.rel, file.info.frames, file.info.rate, file.info.channels, info.frames, info.rate, info.channels
        ));
    }
    let ch = usize::from(info.channels);
    let at = |s: f64| (s * f64::from(info.rate)).round() as u64;
    let (loop_start, loop_end) = match region {
        Some(r) => (at(r.start).min(info.frames), at(r.end).min(info.frames)),
        None => (0, info.frames),
    };
    if region.is_some() && loop_end <= loop_start {
        return Err(format!("{}: the loop region holds no frames of the file", file.rel));
    }
    let mut conv = (info.rate != MUSIC_RATE).then(|| Resampler::with_channels(info.rate, MUSIC_RATE, 2));
    let mut packet = Vec::new();
    let mut stereo: Vec<f32> = Vec::with_capacity(2 * BLOCK_FRAMES);
    loop {
        packet.clear();
        let start = dec.position();
        let more = dec.next(&mut packet).map_err(|e| e.to_string())?;
        if !more {
            if region.is_none() {
                break;
            }
            dec.rewind().map_err(|e| e.to_string())?;
            continue;
        }
        let n = (packet.len() / ch) as u64;
        let (from, to) = if region.is_some() {
            (loop_start.saturating_sub(start).min(n), loop_end.saturating_sub(start).min(n))
        } else {
            (0, n)
        };
        for f in from..to {
            let k = f as usize * ch;
            let (l, r) = if ch == 1 { (packet[k], packet[k]) } else { (packet[k], packet[k + 1]) };
            stereo.extend_from_slice(&[l, r]);
        }
        if region.is_some() && start + n >= loop_end {
            dec.rewind().map_err(|e| e.to_string())?;
        }
        if stereo.len() >= 2 * BLOCK_FRAMES && !hand_over(&mut stereo, &mut conv, tx, room) {
            return Ok(());
        }
    }
    if conv.is_some() {
        stereo.extend(std::iter::repeat_n(0.0, 2 * 64));
    }
    if !stereo.is_empty() {
        hand_over(&mut stereo, &mut conv, tx, room);
    }
    Ok(())
}

fn hand_over(stereo: &mut Vec<f32>, conv: &mut Option<Resampler>, tx: &Sender<Block>, room: &Room) -> bool {
    let block = match conv {
        None => std::mem::take(stereo),
        Some(r) => {
            let frames_in = stereo.len() / 2;
            let mut out_frames = 0usize;
            while r.need(out_frames + 1) <= frames_in {
                out_frames += 1;
            }
            let need = r.need(out_frames);
            let mut out = vec![0.0f32; out_frames * 2];
            r.process(&stereo[..need * 2], &mut out);
            stereo.drain(..need * 2);
            out
        }
    };
    if !room.wait() {
        return false;
    }
    room.sent();
    tx.send(Block::Frames(block)).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(samples: &[f32], rate: u32, channels: u16, served: Option<Vec<u8>>) -> MusicFile {
        let bytes: Arc<[u8]> = crate::content::export_audio::wav_f32(samples, rate, channels).into();
        let info = ssx_content::audio::walk("a.wav", &bytes).unwrap();
        let served: Arc<[u8]> = served.map_or(bytes, Into::into);
        MusicFile { rel: "a.wav".into(), info, read: Arc::new(move || Ok(served.clone())) }
    }

    fn drain(s: &mut MusicStream, frames: usize) -> Vec<f32> {
        let mut got = Vec::new();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while got.len() / 2 < frames && s.ended().is_none() && std::time::Instant::now() < deadline {
            let mut out = vec![0.0; 2 * 1024.min(frames - got.len() / 2)];
            let n = s.render(&mut out);
            got.extend_from_slice(&out[..n * 2]);
            if n == 0 {
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
        }
        got
    }

    #[test]
    fn a_mono_file_streams_converted_to_stereo_at_the_engines_rate() {
        let mut s = MusicStream::open(file(&vec![0.25; 22_050], 22_050, 1, None), None);
        let got = drain(&mut s, usize::MAX);
        assert_eq!(s.ended(), Some(&Ok(())));
        let frames = got.len() / 2;
        assert!((44_100..44_100 + 256).contains(&frames), "{frames} frames for 1 s");
        assert!(got[2000..80_000].iter().all(|v| (v - 0.25).abs() < 1e-3), "the level kept on both sides");
    }

    #[test]
    fn a_loop_region_repeats_and_a_damaged_stream_ends_with_its_reason() {
        let ramp: Vec<f32> = (0..44_100).flat_map(|i| [i as f32 / 44_100.0; 2]).collect();
        let region = LoopRegion { start: 0.25, end: 0.5 };
        let mut s = MusicStream::open(file(&ramp, 44_100, 2, None), Some(region));
        let got = drain(&mut s, 3 * 44_100);
        assert!(s.ended().is_none(), "a loop does not end");
        assert_eq!(got.len() / 2, 3 * 44_100);
        let lo = 11_025.0 / 44_100.0;
        assert!(got.iter().all(|v| (lo - 1e-6..0.5).contains(v)), "every frame inside the region");
        assert!((got[2 * 11_025] - lo).abs() < 1e-6, "sample-exact: the region's first frame again after its end");
        let mut cut = crate::content::export_audio::wav_f32(&ramp, 44_100, 2);
        cut.truncate(cut.len() - 1000);
        let mut s = MusicStream::open(file(&ramp, 44_100, 2, Some(cut)), None);
        drain(&mut s, usize::MAX);
        let e = s.ended().cloned().unwrap().unwrap_err();
        assert!(e.contains("a.wav") && e.contains("cut short"), "{e}");
        let other = crate::content::export_audio::wav_f32(&ramp[..ramp.len() / 2], 44_100, 2);
        let mut s = MusicStream::open(file(&ramp, 44_100, 2, Some(other)), None);
        drain(&mut s, usize::MAX);
        let e = s.ended().cloned().unwrap().unwrap_err();
        assert!(e.contains("changed since it was measured"), "{e}");
    }

    #[test]
    fn the_worker_waits_for_room_and_ends_with_the_stream() {
        let ramp: Vec<f32> = (0..44_100).flat_map(|i| [i as f32 / 44_100.0; 2]).collect();
        let s = MusicStream::open(file(&ramp, 44_100, 2, None), Some(LoopRegion { start: 0.0, end: 1.0 }));
        let room = s.room.clone();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while room.queued.load(Ordering::SeqCst) < AHEAD && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
        assert_eq!(room.queued.load(Ordering::SeqCst), AHEAD);
        drop(s);
        while Arc::strong_count(&room) > 1 && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(Arc::strong_count(&room), 1, "the worker ended");
    }
}
