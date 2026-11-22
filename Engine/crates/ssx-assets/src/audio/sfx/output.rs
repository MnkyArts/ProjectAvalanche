use std::collections::VecDeque;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use super::OutputMix;
use crate::sfx_aems::{FRAME_SAMPLES, MIXER_RATE};
pub use crate::sfx_bus::DAC_CHANNELS;

pub mod capture;
pub mod check;
pub mod feed;
pub mod priority;
pub mod queue;
pub mod resample;
pub mod sim;
pub mod stats;

pub use priority::Scheduling;
pub use queue::{Command, MixInbox, MixQueue, discard, mix_queue};

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
    pulled: Option<Instant>,
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

    // PORT(choice): the port's mixer renders ahead of the device in bursts (a PC device pulls 441…8192 frames at its own
    // times), so a stream read of ~1 ms that the console's real-time frames would have had is waited for here, at most
    // until the frame comes due by the lead's margin (a pull's frames are never waited on); the console's Dac renders a
    // frame per voice buffer that ends (three buffers of 256 frames, +0xF4 = 3, 0x82A504E8) and waits for no read.
    fn step(&self) -> Option<Instant> {
        if self.want > 0 || !self.primed {
            return None;
        }
        let beyond = self.level().checked_sub(self.margin).filter(|&f| f > 0)?;
        Some(self.pulled? + Duration::from_nanos(beyond as u64 * 1_000_000_000 / u64::from(RATE)))
    }
}

struct Inner {
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
    pub fn start(
        mix: OutputMix,
        inbox: MixInbox,
        margin: Option<usize>,
        scheduling: Scheduling,
    ) -> std::io::Result<Self> {
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
            pulled: None,
            next_pull: 0,
            settle: SETTLE_PULLS,
        };
        stats::LEAD.store(margin as u64, Ordering::Relaxed);
        let inner = Arc::new(Inner { lead: Mutex::new(lead), room: Condvar::new(), ready: Condvar::new() });
        let thread = inner.clone();
        let scheduling = scheduling.or_env();
        if scheduling == Scheduling::Audio {
            crate::sfx_stream::raise_file_thread();
        }
        std::thread::Builder::new()
            .name("ssx_audio_mixer".into())
            .spawn(move || mixer_thread(&thread, mix, inbox, scheduling))?;
        Ok(Self(Arc::new(Handle(inner))))
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
            lead.pulled = None;
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
        lead.pulled = Some(Instant::now());
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

const DRAIN_PERIOD: Duration = Duration::from_nanos(FRAME_SAMPLES as u64 * 1_000_000_000 / MIXER_RATE as u64);

fn mixer_thread(inner: &Inner, mut mix: OutputMix, mut inbox: MixInbox, scheduling: Scheduling) {
    let mut promotion = priority::Promotion::start(scheduling);
    crate::sfx_stream::mark_live_thread();
    let mut frames_rendered = 0u64;
    let mut frame = Vec::new();
    let mut drains = stats::DrainLog::default();
    loop {
        promotion.check();
        crate::sfx_stream::live_frame(frames_rendered + 1, None);
        let (t0, before) = (Instant::now(), stats::ThreadUsage::now());
        let mut longest = ("", Duration::ZERO);
        let commands = inbox.drain_each(&mut mix, |kind, took| {
            if took > longest.1 {
                longest = (kind, took);
            }
            promotion.check();
        });
        if commands > 0 {
            drains.record(stats::DrainRecord {
                ns: t0.elapsed().as_nanos() as u64,
                commands: commands as u64,
                kind: longest.0,
                kind_ns: longest.1.as_nanos() as u64,
                thread: before.zip(stats::ThreadUsage::now()).map(|(b, a)| a.since(b)),
            });
        } else {
            drains.publish();
        }
        let (ch, step) = {
            let mut lead = inner.state();
            let level = lead.level();
            let full = level + MIX_FRAMES > lead.target();
            lead.primed |= full;
            if lead.stop {
                return;
            }
            if full && level >= lead.want {
                drop(inner.room.wait_timeout(lead, DRAIN_PERIOD));
                promotion.blocked();
                continue;
            }
            (lead.channels, lead.step())
        };
        let (t1, cpu) = (Instant::now(), stats::thread_cpu());
        frames_rendered += 1;
        crate::sfx_stream::live_frame(frames_rendered, step);
        frame.resize(ch * MIX_FRAMES, 0.0);
        mix.render_channels(&mut frame, ch);
        let swapped = std::mem::take(&mut mix.swapped);
        let on_cpu = cpu.zip(stats::thread_cpu()).map_or(0, |(a, b)| b.saturating_sub(a).as_nanos() as u64);
        stats::render(t1.elapsed().as_nanos() as u64, on_cpu, MIX_FRAMES as u64);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_frame_beyond_the_margin_comes_due_after_the_last_pull() {
        let t = Instant::now();
        let mut lead = Lead {
            frames: vec![0.0; 2 * (MARGIN + 3 * MIX_FRAMES)].into(),
            channels: 2,
            want: 0,
            stop: false,
            primed: true,
            margin: MARGIN,
            floor: MARGIN,
            low: isize::MAX,
            since: t,
            pulls: [0; PULL_WINDOW],
            pulled: Some(t),
            next_pull: 0,
            settle: 0,
        };
        assert_eq!(lead.step(), Some(t + Duration::from_nanos(3 * 256 * 1_000_000_000 / 48_000)));
        lead.want = 1;
        assert_eq!(lead.step(), None, "a pull waits");
        lead.want = 0;
        lead.pulled = None;
        assert_eq!(lead.step(), None, "no pull yet");
        lead.pulled = Some(t);
        lead.primed = false;
        assert_eq!(lead.step(), None, "not primed");
        lead.primed = true;
        lead.frames.truncate(2 * MARGIN);
        assert_eq!(lead.step(), None, "within the margin");
    }

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
        let (_queue, inbox) = mix_queue();
        let out = Output::start(
            OutputMix::new(Some(Box::new(Tone)), RATE, None),
            inbox,
            Some(4 * MIX_FRAMES),
            Scheduling::Normal,
        )
        .unwrap();
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
        let (queue, inbox) = mix_queue();
        let out =
            Output::start(OutputMix::new(None, 44_100, None), inbox, Some(4 * MIX_FRAMES), Scheduling::Normal).unwrap();
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
        let (release, held) = std::sync::mpsc::channel::<()>();
        let (started, running) = std::sync::mpsc::channel::<()>();
        assert!(queue.post(move |_| {
            let _ = started.send(());
            let _ = held.recv();
        }));
        running.recv_timeout(Duration::from_secs(10)).expect("the mixer applies the command");
        let mut huge = vec![1.0f32; 2 * 65_536];
        let p = out.pull(&mut huge, Duration::from_millis(20));
        assert!(p.silence > 0 && huge[huge.len() - 1] == 0.0);
        drop(release);
    }

    #[test]
    fn commands_are_applied_while_the_lead_is_full() {
        let (queue, inbox) = mix_queue();
        let out =
            Output::start(OutputMix::new(None, 44_100, None), inbox, Some(4 * MIX_FRAMES), Scheduling::Normal).unwrap();
        out.expect_pull(512);
        let limit = Instant::now() + Duration::from_secs(10);
        while out.level() < 512 + 4 * MIX_FRAMES - MIX_FRAMES {
            assert!(Instant::now() < limit, "the lead never filled");
            std::thread::sleep(Duration::from_millis(1));
        }
        let (tx, rx) = std::sync::mpsc::channel();
        assert!(queue.post(move |m| {
            let _ = tx.send(m.remix_event_type);
        }));
        assert_eq!(rx.recv_timeout(Duration::from_secs(10)), Ok(1));
        drop(out);
    }
}
