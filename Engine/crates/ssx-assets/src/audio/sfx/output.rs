use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use super::OutputMix;
use crate::sfx_aems::{FRAME_SAMPLES, MIXER_RATE};
pub use crate::sfx_bus::DAC_CHANNELS;

pub mod capture;
pub mod check;
pub mod feed;
pub mod resample;
pub mod sim;
pub mod stats;

pub const MIX_FRAMES: usize = FRAME_SAMPLES;
pub const RATE: u32 = MIXER_RATE;

// PORT(choice): eight frames (42.7 ms) plus the growth (to `MARGIN_MAX`) cover the stalls measured on the development
// machine (Linux, ALSA → PipeWire, 2 048-frame device pulls: no underrun in two five-minute sessions, also beside 8
// busy loops; WASAPI only simulated, Windows not run); tuning: the player logs' `audio:` lines from more machines; no
// counterpart in the game (its Dac keeps three 256-sample frames queued, +0xF4 = 3, 0x82A504E8, on time-critical
// hardware threads).
pub const MARGIN: usize = 8 * MIX_FRAMES;
pub const MARGIN_MAX: usize = 64 * MIX_FRAMES;
pub const GUARD: usize = 2 * MIX_FRAMES;
const SHRINK_WINDOW: Duration = Duration::from_secs(10);
const SHRINK_KEEP: usize = 2 * MIX_FRAMES;
const PULL_WINDOW: usize = 16;
const SETTLE_PULLS: u8 = 4;

pub struct GameTurn(());

impl GameTurn {
    pub fn take() -> Self {
        GAME_TURNS.fetch_add(1, Ordering::SeqCst);
        GameTurn(())
    }
}

impl Drop for GameTurn {
    fn drop(&mut self) {
        GAME_TURNS.fetch_sub(1, Ordering::SeqCst);
        let _g = HANDOFF.0.lock().unwrap_or_else(|e| e.into_inner());
        HANDOFF.1.notify_all();
    }
}

static GAME_TURNS: AtomicUsize = AtomicUsize::new(0);
static HANDOFF: (Mutex<()>, Condvar) = (Mutex::new(()), Condvar::new());

fn wait_for_game(limit: Duration) {
    if GAME_TURNS.load(Ordering::SeqCst) == 0 {
        return;
    }
    let g = HANDOFF.0.lock().unwrap_or_else(|e| e.into_inner());
    let _ = HANDOFF.1.wait_timeout_while(g, limit, |_| GAME_TURNS.load(Ordering::SeqCst) > 0);
}

struct Lead {
    frames: VecDeque<f32>,
    channels: usize,
    want: usize,
    stop: bool,
    primed: bool,
    margin: usize,
    floor: usize,
    low: isize,
    since: Instant,
    pulls: [usize; PULL_WINDOW],
    next_pull: usize,
    settle: u8,
}

impl Lead {
    fn target(&self) -> usize {
        self.pulls.iter().copied().max().unwrap_or(0) + self.margin
    }

    fn level(&self) -> usize {
        self.frames.len() / self.channels
    }
}

struct Inner {
    mix: Arc<Mutex<OutputMix>>,
    lead: Mutex<Lead>,
    room: Condvar,
    ready: Condvar,
}

impl Inner {
    fn state(&self) -> MutexGuard<'_, Lead> {
        self.lead.lock().unwrap_or_else(|e| e.into_inner())
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Pull {
    pub level: usize,
    pub target: usize,
    pub silence: usize,
    pub wait: Duration,
    pub startup: bool,
}

#[derive(Clone)]
pub struct Output(Arc<Handle>);

struct Handle(Arc<Inner>);

impl Drop for Handle {
    fn drop(&mut self) {
        self.0.state().stop = true;
        self.0.room.notify_all();
        self.0.ready.notify_all();
    }
}

impl Output {
    pub fn start(mix: Arc<Mutex<OutputMix>>, margin: Option<usize>) -> std::io::Result<Self> {
        let env = std::env::var("SSX_AUDIO_LEAD").ok().and_then(|v| v.parse::<usize>().ok());
        let margin = margin.or(env).unwrap_or(MARGIN).next_multiple_of(MIX_FRAMES).clamp(MIX_FRAMES, MARGIN_MAX);
        let lead = Lead {
            frames: VecDeque::new(),
            channels: 2,
            want: 0,
            stop: false,
            primed: false,
            margin,
            floor: margin,
            low: isize::MAX,
            since: Instant::now(),
            pulls: [0; PULL_WINDOW],
            next_pull: 0,
            settle: SETTLE_PULLS,
        };
        stats::LEAD.store(margin as u64, Ordering::Relaxed);
        let inner = Arc::new(Inner { mix, lead: Mutex::new(lead), room: Condvar::new(), ready: Condvar::new() });
        let thread = inner.clone();
        std::thread::Builder::new().name("ssx_audio_mixer".into()).spawn(move || mixer_thread(&thread))?;
        Ok(Self(Arc::new(Handle(inner))))
    }

    pub fn mix(&self) -> &Arc<Mutex<OutputMix>> {
        &self.0.0.mix
    }

    pub fn expect_pull(&self, frames: usize) {
        let inner = &self.0.0;
        let mut lead = inner.state();
        lead.pulls = [0; PULL_WINDOW];
        lead.pulls[0] = frames;
        lead.next_pull = 1;
        lead.primed = false;
        lead.settle = SETTLE_PULLS;
        drop(lead);
        inner.room.notify_one();
    }

    pub fn level(&self) -> usize {
        self.0.0.state().level()
    }

    pub fn set_channels(&self, channels: usize) {
        let inner = &self.0.0;
        let mut lead = inner.state();
        if lead.channels != channels {
            lead.channels = channels;
            lead.frames.clear();
            lead.primed = false;
            lead.settle = SETTLE_PULLS;
        }
        drop(lead);
        inner.room.notify_one();
    }

    pub fn channels(&self) -> usize {
        self.0.0.state().channels
    }

    pub fn pull(&self, out: &mut [f32], wait: Duration) -> Pull {
        let inner = &self.0.0;
        let mut lead = inner.state();
        let ch = lead.channels;
        let need = out.len() / ch;
        let mut pull = Pull { level: lead.level(), target: lead.target(), ..Pull::default() };
        let slot = lead.next_pull % PULL_WINDOW;
        lead.pulls[slot] = need;
        lead.next_pull = slot + 1;
        if !lead.primed {
            drop(lead);
            inner.room.notify_one();
            out.fill(0.0);
            pull.startup = true;
            stats::pull(&pull, need);
            return pull;
        }
        let mut got = 0;
        let mut waited: Option<Instant> = None;
        loop {
            let n = lead.level().min(need - got);
            for (o, x) in out[ch * got..ch * (got + n)].iter_mut().zip(lead.frames.drain(..ch * n)) {
                *o = x;
            }
            got += n;
            if got == need || lead.stop {
                break;
            }
            let start = *waited.get_or_insert_with(Instant::now);
            let Some(left) = wait.checked_sub(start.elapsed()).filter(|d| !d.is_zero()) else { break };
            lead.want = need - got;
            inner.room.notify_one();
            lead = inner.ready.wait_timeout(lead, left).unwrap_or_else(|e| e.into_inner()).0;
        }
        lead.want = 0;
        let left = pull.level as isize - need as isize;
        let short = GUARD as isize - left;
        let settled = lead.settle == 0;
        lead.settle = lead.settle.saturating_sub(1);
        if settled && short > 0 {
            lead.margin = (lead.margin + (short as usize).next_multiple_of(MIX_FRAMES)).min(MARGIN_MAX);
            (lead.low, lead.since) = (isize::MAX, Instant::now());
        } else if settled {
            lead.low = lead.low.min(left);
            if lead.since.elapsed() >= SHRINK_WINDOW {
                let spare = (lead.low - (GUARD + SHRINK_KEEP) as isize).max(0) as usize / 2 / MIX_FRAMES * MIX_FRAMES;
                lead.margin = lead.margin.saturating_sub(spare).max(lead.floor);
                (lead.low, lead.since) = (isize::MAX, Instant::now());
            }
        }
        stats::LEAD.store(lead.margin as u64, Ordering::Relaxed);
        stats::QUEUED.store(lead.level() as u64, Ordering::Relaxed);
        drop(lead);
        inner.room.notify_one();
        out[ch * got..].fill(0.0);
        pull.silence = need - got;
        pull.wait = waited.map_or(Duration::ZERO, |t| t.elapsed());
        stats::pull(&pull, need);
        pull
    }
}

pub fn delay() -> Duration {
    let frames = stats::QUEUED.load(Ordering::Relaxed) + stats::DEVICE.load(Ordering::Relaxed) * 3 / 4;
    Duration::from_secs_f64(frames as f64 / f64::from(RATE))
}

fn mixer_thread(inner: &Inner) {
    raise_priority();
    let mut frame = Vec::new();
    loop {
        let (ahead, demand, ch) = {
            let mut lead = inner.state();
            loop {
                let level = lead.level();
                let full = level + MIX_FRAMES > lead.target();
                lead.primed |= full;
                if lead.stop || !full || level < lead.want {
                    break;
                }
                lead = inner.room.wait(lead).unwrap_or_else(|e| e.into_inner());
            }
            if lead.stop {
                return;
            }
            (lead.level(), lead.want > 0, lead.channels)
        };
        let urgent = demand || ahead < GUARD;
        wait_for_game(Duration::from_micros(if urgent { 500 } else { 2000 }));
        let t0 = Instant::now();
        let mut m = inner.mix.lock().unwrap_or_else(|e| e.into_inner());
        let t1 = Instant::now();
        frame.resize(ch * MIX_FRAMES, 0.0);
        m.render_channels(&mut frame, ch);
        let swapped = std::mem::take(&mut m.swapped);
        drop(m);
        stats::render((t1 - t0).as_nanos() as u64, t1.elapsed().as_nanos() as u64, MIX_FRAMES as u64);
        for x in &mut frame {
            *x = x.clamp(-1.0, 1.0);
        }
        let mut lead = inner.state();
        if lead.channels != ch {
            continue;
        }
        if swapped {
            declick(&mut lead.frames, &mut frame, ch);
        }
        lead.frames.extend(&frame);
        stats::QUEUED.store(lead.level() as u64, Ordering::Relaxed);
        if lead.want > 0 {
            stats::ON_DEMAND.fetch_add(MIX_FRAMES as u64, Ordering::Relaxed);
            inner.ready.notify_one();
        }
    }
}

// TODO(guess): a port choice — the game has one mixer whose front-end and level sounds end by their own events (0x830D0F28), where the shell swaps two engines between frames (a 0.58 step measured; this ramp makes the lead's last frame end at 0 and the next start at 0, 5.3 ms each way); settle: one engine for both.
fn declick(queued: &mut VecDeque<f32>, next: &mut [f32], ch: usize) {
    let tail = (queued.len() / ch).min(MIX_FRAMES);
    let from = queued.len() - ch * tail;
    for (k, x) in queued.iter_mut().skip(from).enumerate() {
        *x *= 1.0 - (k / ch + 1) as f32 / tail as f32;
    }
    let frames = next.len() / ch;
    for (k, x) in next.iter_mut().enumerate() {
        *x *= (k / ch) as f32 / frames as f32;
    }
}

// PORT(choice): the port leaves the thread's placement to the system (not run on Windows here, cross-checked only); the
// game also pins the thread to hardware thread 4 (System +0x14E, `XSetThreadProcessor` 0x82351550; XAudio2 on 4 / 5,
// mask 0x30), which a PC has no counterpart of.
fn raise_priority() {
    #[cfg(windows)]
    {
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn GetCurrentThread() -> *mut core::ffi::c_void;
            fn SetThreadPriority(thread: *mut core::ffi::c_void, priority: i32) -> i32;
        }
        const THREAD_PRIORITY_TIME_CRITICAL: i32 = 15;
        unsafe {
            SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_TIME_CRITICAL);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_swap_of_the_engines_is_ramped() {
        let mut queued: VecDeque<f32> = vec![1.0; 2 * 3 * MIX_FRAMES].into();
        let mut next = vec![-1.0f32; 2 * MIX_FRAMES];
        declick(&mut queued, &mut next, 2);
        assert!(queued.iter().take(2 * 2 * MIX_FRAMES).all(|x| *x == 1.0));
        let tail: Vec<f32> = queued.iter().skip(2 * 2 * MIX_FRAMES).copied().collect();
        assert!(tail.windows(2).all(|w| w[1] <= w[0]) && tail[0] < 1.0 && tail[tail.len() - 1] == 0.0);
        assert_eq!(tail[0], tail[1]);
        assert!(next[0] == 0.0 && next[1] == 0.0 && next.windows(2).all(|w| w[1] <= w[0]));
        assert!(next[next.len() - 1] < -0.99);
        let mut short: VecDeque<f32> = vec![1.0; 2 * 10].into();
        declick(&mut short, &mut next, 2);
        assert_eq!(short[short.len() - 1], 0.0);
        declick(&mut VecDeque::new(), &mut next, 2);
    }

    #[test]
    fn a_surround_lead_carries_six_channels() {
        struct Tone;
        impl super::super::MusicSource for Tone {
            fn render(&mut self, out: &mut [f32]) {
                out.fill(0.25);
            }
        }
        let mix = Arc::new(Mutex::new(OutputMix::new(Some(Box::new(Tone)), RATE, None)));
        let out = Output::start(mix, Some(4 * MIX_FRAMES)).unwrap();
        out.set_channels(DAC_CHANNELS);
        out.expect_pull(512);
        let mut buf = vec![0.0f32; DAC_CHANNELS * 512];
        let limit = Instant::now() + Duration::from_secs(10);
        while out.pull(&mut buf, Duration::from_millis(50)).startup {
            assert!(Instant::now() < limit, "the lead never filled");
            std::thread::sleep(Duration::from_millis(1));
        }
        let p = out.pull(&mut buf, Duration::from_secs(5));
        assert_eq!(p.silence, 0);
        let f = &buf[DAC_CHANNELS * 100..DAC_CHANNELS * 101];
        assert!(f[0] > 0.0 && f[1] > 0.0 && f[2..].iter().all(|x| *x == 0.0), "{f:?}");
    }

    #[test]
    fn a_pull_takes_the_lead_and_a_larger_one_waits_for_the_mixer() {
        let mix = Arc::new(Mutex::new(OutputMix::new(None, 44_100, None)));
        let out = Output::start(mix, Some(4 * MIX_FRAMES)).unwrap();
        out.expect_pull(512);
        let mut buf = vec![1.0f32; 2 * 512];
        let limit = Instant::now() + Duration::from_secs(10);
        loop {
            let p = out.pull(&mut buf, Duration::from_millis(50));
            assert!(buf.iter().all(|x| *x == 0.0));
            if !p.startup {
                assert_eq!((p.silence, p.wait), (0, Duration::ZERO));
                break;
            }
            assert!(Instant::now() < limit, "the lead never filled");
            std::thread::sleep(Duration::from_millis(1));
        }
        let mut big = vec![1.0f32; 2 * 8192];
        let p = out.pull(&mut big, Duration::from_secs(10));
        assert_eq!(p.silence, 0);
        assert!(!p.wait.is_zero() && big.iter().all(|x| *x == 0.0));
        let limit = Instant::now() + Duration::from_secs(10);
        while out.level() < 8192 {
            assert!(Instant::now() < limit, "the lead does not follow the pulls");
            std::thread::sleep(Duration::from_millis(1));
        }
        std::thread::sleep(Duration::from_millis(20));
        assert!(out.level() <= 8192 + MARGIN_MAX + MIX_FRAMES, "the lead holds {} frames", out.level());
        let held = out.mix().clone();
        let guard = held.lock().unwrap();
        let mut huge = vec![1.0f32; 2 * 65_536];
        let p = out.pull(&mut huge, Duration::from_millis(20));
        assert!(p.silence > 0 && huge[huge.len() - 1] == 0.0);
        drop(guard);
    }
}
