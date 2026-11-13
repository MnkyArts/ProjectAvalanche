use std::collections::VecDeque;
use std::io::{Read, Seek};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TryRecvError, sync_channel};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::time::Duration;

use ssx_content::movie::{Demuxer, Limits, MovieInfo, Packet, Track};

use crate::av1::{self, Frame};
use crate::check::panic_text;
use crate::decode::{Threads, video_settings};
use crate::isolate::{Isolated, Out};
use crate::vorbis::VorbisStream;
use crate::yuv::packed_to_rgba;

pub trait AudioSink: Send + Sync {
    fn push(&self, frames: &[f32]);
    fn finish(&self);
    fn queued_frames(&self) -> usize;
    fn stopped(&self) -> bool;
}

pub const PICTURE_BYTES_IN_FLIGHT: u64 = 64 << 20;
pub const UNIT_BYTES_IN_FLIGHT: u64 = 64 << 20;
pub const AUDIO_AHEAD_S: f64 = 1.0;
pub const AUDIO_MAX_AHEAD_S: f64 = 30.0;
const POLL: Duration = Duration::from_millis(20);

enum Msg {
    Picture(Frame),
    End(Result<(), String>),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum AtEnd {
    Stop,
    Loop,
}

fn unit_bytes(p: &Packet) -> u64 {
    (p.data.capacity() + std::mem::size_of::<Packet>()) as u64
}

#[derive(Default)]
struct Units {
    queue: VecDeque<Packet>,
    bytes: u64,
    closed: bool,
}

struct Shared {
    stop: AtomicBool,
    mute: AtomicBool,
    units: Mutex<Units>,
    ready: Condvar,
    error: Mutex<Option<String>>,
    pub decoded: AtomicU64,
}

pub struct Playback {
    pub info: MovieInfo,
    rx: Mutex<Receiver<Msg>>,
    shared: Arc<Shared>,
    next: Option<Frame>,
    ended: Option<Result<(), String>>,
    pub shown: u64,
    pub dropped: u64,
    pub received: u64,
    pub frame: Option<u64>,
    pub waits: u64,
}

impl Playback {
    pub fn start<R: Read + Seek + Send + 'static>(
        inner: R,
        len: u64,
        limits: &Limits,
        threads: Threads,
        sink: Arc<dyn AudioSink>,
    ) -> Result<Playback, String> {
        Self::begin(inner, len, limits, threads, sink, AtEnd::Stop)
    }

    pub fn start_looping<R: Read + Seek + Send + 'static>(
        inner: R,
        len: u64,
        limits: &Limits,
        threads: Threads,
    ) -> Result<Playback, String> {
        Self::begin(inner, len, limits, threads, Arc::new(NoSound), AtEnd::Loop)
    }

    fn begin<R: Read + Seek + Send + 'static>(
        inner: R,
        len: u64,
        limits: &Limits,
        threads: Threads,
        sink: Arc<dyn AudioSink>,
        at_end: AtEnd,
    ) -> Result<Playback, String> {
        let demux = Demuxer::open(inner, len, limits).map_err(|e| e.to_string())?;
        let info = demux.info().clone();
        let (settings, expect) = video_settings(&info, threads)?;
        let picture_bytes = u64::from(info.video.width) * u64::from(info.video.height) * 4;
        let depth = (PICTURE_BYTES_IN_FLIGHT / picture_bytes.max(1)).clamp(2, 8) as usize;
        let (tx, rx) = sync_channel(depth);
        let shared = Arc::new(Shared {
            stop: AtomicBool::new(false),
            mute: AtomicBool::new(false),
            units: Mutex::new(Units::default()),
            ready: Condvar::new(),
            error: Mutex::new(None),
            decoded: AtomicU64::new(0),
        });
        let audio = match &info.audio {
            Some(a) if at_end == AtEnd::Stop => Some(VorbisStream::new(a, info.duration_ns, limits.max_overrun_ns)?),
            _ => {
                sink.finish();
                None
            }
        };
        let passes = match at_end {
            AtEnd::Stop => None,
            AtEnd::Loop => Some(Passes::new(&info, limits)?),
        };
        let s = shared.clone();
        std::thread::Builder::new()
            .name("ssx_movie_read".into())
            .spawn(move || {
                let run =
                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| read(demux, audio, &*sink, &s, passes)));
                let result = run.unwrap_or_else(|p| Err(format!("the movie's reader stopped: {}", panic_text(&p))));
                sink.finish();
                let mut u = s.units.lock().unwrap_or_else(PoisonError::into_inner);
                if let Err(e) = result {
                    *s.error.lock().unwrap_or_else(PoisonError::into_inner) = Some(e);
                }
                u.closed = true;
                s.ready.notify_all();
            })
            .map_err(|e| format!("no worker thread for the movie: {e}"))?;
        let s = shared.clone();
        std::thread::Builder::new()
            .name("ssx_movie_pictures".into())
            .spawn(move || {
                let run =
                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| pictures(&settings, expect, &s, &tx)));
                let result =
                    run.unwrap_or_else(|p| Err(format!("the movie's picture decoder stopped: {}", panic_text(&p))));
                if result.is_err() {
                    s.stop.store(true, Ordering::Release);
                    s.ready.notify_all();
                }
                let _ = tx.send(Msg::End(result));
            })
            .map_err(|e| {
                shared.stop.store(true, Ordering::Release);
                format!("no worker thread for the movie: {e}")
            })?;
        Ok(Playback {
            info,
            rx: Mutex::new(rx),
            shared,
            next: None,
            ended: None,
            shown: 0,
            dropped: 0,
            received: 0,
            frame: None,
            waits: 0,
        })
    }
}

impl Playback {
    pub fn due(&mut self, t_ns: i64) -> Option<Frame> {
        let mut latest = None;
        loop {
            if self.next.is_none() && self.ended.is_none() {
                let r = self.rx.lock().map_or(Err(TryRecvError::Disconnected), |rx| rx.try_recv());
                match r {
                    Ok(Msg::Picture(f)) => {
                        self.received += 1;
                        self.next = Some(f);
                    }
                    Ok(Msg::End(r)) => self.ended = Some(r),
                    Err(TryRecvError::Empty) => {
                        self.waits += 1;
                        break;
                    }
                    Err(TryRecvError::Disconnected) => {
                        self.ended = Some(Err("the movie's picture worker ended without its end".into()));
                    }
                }
            }
            match self.next.take() {
                Some(f) if f.pts_ns <= t_ns => {
                    if latest.replace(f).is_some() {
                        self.dropped += 1;
                    }
                }
                Some(f) => {
                    self.next = Some(f);
                    break;
                }
                None => break,
            }
        }
        if latest.is_some() {
            self.shown += 1;
            self.frame = Some(self.shown + self.dropped - 1);
        }
        latest
    }

    pub fn ended(&self) -> Option<&Result<(), String>> {
        self.ended.as_ref().filter(|_| self.next.is_none())
    }

    pub fn mute(&self) {
        self.shared.mute.store(true, Ordering::Release);
    }

    pub fn waiting(&self) -> bool {
        self.next.is_none() && self.ended.is_none()
    }

    pub fn decoded(&self) -> u64 {
        self.shared.decoded.load(Ordering::Relaxed)
    }
}

impl Drop for Playback {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Release);
        self.shared.ready.notify_all();
    }
}

fn stopped(s: &Shared) -> bool {
    s.stop.load(Ordering::Acquire)
}

struct NoSound;

impl AudioSink for NoSound {
    fn push(&self, _frames: &[f32]) {}
    fn finish(&self) {}
    fn queued_frames(&self) -> usize {
        0
    }
    fn stopped(&self) -> bool {
        false
    }
}

struct Passes {
    offset: i64,
    last_picture: Option<i64>,
    duration_ns: i64,
    min_frame_ns: i64,
}

impl Passes {
    fn new(info: &MovieInfo, limits: &Limits) -> Result<Self, String> {
        let ns = |v: u64| i64::try_from(v).map_err(|_| format!("a time of {v} ns past 2⁶³"));
        Ok(Self {
            offset: 0,
            last_picture: None,
            duration_ns: ns(info.duration_ns)?,
            min_frame_ns: ns(limits.min_frame_ns)?,
        })
    }

    fn place(&mut self, unit: &mut Packet) -> Result<(), String> {
        self.last_picture = Some(unit.pts_ns);
        unit.pts_ns = unit.pts_ns.checked_add(self.offset).ok_or("a looping movie's time past 2⁶³ ns")?;
        Ok(())
    }

    fn next(&mut self) -> Result<(), String> {
        let last = self.last_picture.take().ok_or("a looping movie whose pass holds no picture")?;
        let length = self.duration_ns.max(last.saturating_add(self.min_frame_ns));
        self.offset = self.offset.checked_add(length).ok_or("a looping movie's time past 2⁶³ ns")?;
        Ok(())
    }
}

fn read<R: Read + Seek>(
    mut demux: Demuxer<R>,
    mut audio: Option<VorbisStream>,
    sink: &dyn AudioSink,
    s: &Shared,
    mut passes: Option<Passes>,
) -> Result<(), String> {
    let (rate, channels) = audio.as_ref().map_or((48_000.0, 1), |a| (f64::from(a.rate), a.channels));
    let (ahead, most) = ((AUDIO_AHEAD_S * rate) as usize, (AUDIO_MAX_AHEAD_S * rate) as usize);
    let mut started = false;
    let mut pcm = Vec::new();
    loop {
        loop {
            if stopped(s) || sink.stopped() {
                return Ok(());
            }
            let units = s.units.lock().unwrap_or_else(PoisonError::into_inner);
            let need_units = units.queue.is_empty();
            let muted = s.mute.load(Ordering::Acquire);
            if audio.is_some() && !muted && sink.queued_frames() >= most && need_units {
                return Err(format!(
                    "the movie's pictures lie more than {AUDIO_MAX_AHEAD_S} s behind its sound in the file"
                ));
            }
            if audio.is_none() || muted || sink.queued_frames() < ahead || need_units {
                break;
            }
            let _ = s.ready.wait_timeout(units, POLL);
        }
        let Some(mut p) = demux.next_packet().map_err(|e| e.to_string())? else {
            let Some(passes) = passes.as_mut() else { return Ok(()) };
            passes.next()?;
            demux.restart().map_err(|e| e.to_string())?;
            continue;
        };
        match p.track {
            Track::Video => {
                if let Some(passes) = passes.as_mut() {
                    passes.place(&mut p)?;
                }
                let weight = unit_bytes(&p);
                let mut units = s.units.lock().unwrap_or_else(PoisonError::into_inner);
                while units.bytes + weight > UNIT_BYTES_IN_FLIGHT && !units.queue.is_empty() {
                    if stopped(s) {
                        return Ok(());
                    }
                    units = s.ready.wait_timeout(units, POLL).unwrap_or_else(PoisonError::into_inner).0;
                }
                units.bytes += weight;
                units.queue.push_back(p);
                s.ready.notify_all();
            }
            Track::Audio => {
                let Some(a) = audio.as_mut().filter(|_| !s.mute.load(Ordering::Acquire)) else { continue };
                if !started {
                    started = true;
                    let mut silence = (p.pts_ns.max(0) as f64 * rate / 1e9) as usize;
                    while silence > 0 {
                        while sink.queued_frames() >= ahead {
                            if stopped(s) || sink.stopped() {
                                return Ok(());
                            }
                            std::thread::sleep(POLL);
                        }
                        let n = silence.min(4096);
                        sink.push(&vec![0.0; n * channels]);
                        silence -= n;
                    }
                }
                pcm.clear();
                a.decode(&p, &mut pcm)?;
                sink.push(&pcm);
            }
        }
    }
}

fn pictures(settings: &av1::Settings, expect: av1::Expect, s: &Shared, tx: &SyncSender<Msg>) -> Result<(), String> {
    let mut child = Isolated::spawn(settings, &expect)?;
    let mut sent: VecDeque<i64> = VecDeque::new();
    let mut pending: Option<Packet> = None;
    let mut finished = false;
    loop {
        if stopped(s) {
            return Ok(());
        }
        while !finished {
            if pending.is_none() {
                let mut units = s.units.lock().unwrap_or_else(PoisonError::into_inner);
                pending = units.queue.pop_front();
                if let Some(u) = &pending {
                    units.bytes -= unit_bytes(u);
                    s.ready.notify_all();
                } else if units.closed {
                    drop(units);
                    if let Some(e) = s.error.lock().unwrap_or_else(PoisonError::into_inner).take() {
                        return Err(e);
                    }
                    child.finish();
                    finished = true;
                }
            }
            let Some(mut u) = pending.take() else { break };
            let pts = u.pts_ns;
            match child.push(pts, std::mem::take(&mut u.data))? {
                None => sent.push_back(pts),
                Some(back) => {
                    u.data = back;
                    pending = Some(u);
                    break;
                }
            }
        }
        match child.next(POLL)? {
            Some(Out::Picture(pts, planes)) => {
                if sent.pop_front() != Some(pts) {
                    return Err(format!(
                        "AV1: a picture at {:.3} s that no temporal unit stands for",
                        pts as f64 / 1e9
                    ));
                }
                let (w, h) = (expect.width as usize, expect.height as usize);
                let rgba = packed_to_rgba(&planes, w, h, expect.mono, expect.matrix, expect.full_range, expect.siting);
                let f = Frame { pts_ns: pts, width: expect.width, height: expect.height, rgba, planes: None };
                s.decoded.fetch_add(1, Ordering::Relaxed);
                if tx.send(Msg::Picture(f)).is_err() {
                    return Ok(());
                }
            }
            Some(Out::Done) => {
                if let Some(&t) = sent.front() {
                    return Err(format!("AV1: the temporal unit at {:.3} s gave no picture", t as f64 / 1e9));
                }
                return Ok(());
            }
            None => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;
    use std::time::Instant;

    use ssx_content::movie::write::*;

    use super::*;

    fn movie(ms: f64, blocks: &[Vec<u8>]) -> Vec<u8> {
        let setup = vorbis_setup(&[(16, 2, 1)]);
        let tracks = element(
            0x1654_AE6B,
            &[
                video_track(1, 320, 180, &av1_private(BASE_SEQ), Some(33_333_333)),
                audio_track(2, 2, 48_000.0, &vorbis_private(2, 48_000, &setup)),
            ]
            .concat(),
        );
        file(&[info(ms), tracks, cluster(0, blocks)])
    }

    fn shared() -> Arc<Shared> {
        Arc::new(Shared {
            stop: AtomicBool::new(false),
            mute: AtomicBool::new(false),
            units: Mutex::new(Units::default()),
            ready: Condvar::new(),
            error: Mutex::new(None),
            decoded: AtomicU64::new(0),
        })
    }

    fn looped(m: Vec<u8>, n: usize) -> (Vec<i64>, Result<(), String>) {
        let limits = Limits::default();
        let demux = Demuxer::open(Cursor::new(m.clone()), m.len() as u64, &limits).unwrap();
        let passes = Passes::new(demux.info(), &limits).unwrap();
        let s = shared();
        let reader = {
            let s = s.clone();
            std::thread::spawn(move || {
                let r = read(demux, None, &NoSound, &s, Some(passes));
                s.units.lock().unwrap().closed = true;
                s.ready.notify_all();
                r
            })
        };
        let mut times = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(20);
        while times.len() < n && Instant::now() < deadline {
            let mut units = s.units.lock().unwrap();
            match units.queue.pop_front() {
                Some(u) => {
                    units.bytes -= unit_bytes(&u);
                    times.push(u.pts_ns);
                    s.ready.notify_all();
                }
                None if units.closed => break,
                None => drop(s.ready.wait_timeout(units, POLL).unwrap()),
            }
        }
        s.stop.store(true, Ordering::Release);
        s.ready.notify_all();
        (times, reader.join().unwrap())
    }

    #[test]
    fn a_looping_movie_reads_its_one_file_again_with_rising_times() {
        let blocks = [
            simple_block(1, 0, 0x80, &unit()),
            simple_block(2, 0, 0x80, &[0, 1, 2]),
            simple_block(1, 34, 0, &unit()),
            simple_block(1, 500, 0, &unit()),
        ];
        let (times, end) = looped(movie(1000.0, &blocks), 9);
        assert_eq!(end, Ok(()), "stopped, not failed");
        let ms = |t: i64| t * 1_000_000;
        let want: Vec<i64> = (0..3).flat_map(|k| [ms(1000 * k), ms(1000 * k + 34), ms(1000 * k + 500)]).collect();
        assert_eq!(times, want);
    }

    #[test]
    fn a_pass_shorter_than_its_pictures_takes_one_picture_time_more() {
        let blocks = [simple_block(1, 0, 0x80, &unit()), simple_block(1, 40, 0, &unit())];
        let (times, end) = looped(movie(10.0, &blocks), 6);
        assert_eq!(end, Ok(()));
        let pass = 40_000_000 + 1_000_000_000 / 60;
        assert_eq!(times, [0, 40_000_000, pass, pass + 40_000_000, 2 * pass, 2 * pass + 40_000_000]);
    }

    #[test]
    fn a_looping_movie_without_pictures_is_refused() {
        let (times, end) = looped(movie(1000.0, &[simple_block(2, 0, 0x80, &[0, 1, 2])]), 1);
        assert!(times.is_empty());
        assert_eq!(end, Err("a looping movie whose pass holds no picture".to_string()));
    }
}
