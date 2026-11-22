use std::fmt::Write as _;

use bevy::winit::UpdateMode;
use serde::{Deserialize, Serialize};

pub const FORMAT: u32 = 1;

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Summary {
    pub avg: f32,
    pub p50: f32,
    pub p95: f32,
    pub p99: f32,
    pub max: f32,
}

impl Summary {
    pub fn of(values: &[f32]) -> Summary {
        if values.is_empty() {
            return Summary::default();
        }
        let mut v = values.to_vec();
        v.sort_by(f32::total_cmp);
        let avg = (v.iter().map(|&x| f64::from(x)).sum::<f64>() / v.len() as f64) as f32;
        Summary { avg, p50: rank(&v, 0.50), p95: rank(&v, 0.95), p99: rank(&v, 0.99), max: v[v.len() - 1] }
    }
}

fn rank(v: &[f32], q: f64) -> f32 {
    let i = ((q * v.len() as f64).ceil() as usize).clamp(1, v.len()) - 1;
    v[i]
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct FrameStats {
    pub frames: usize,
    pub seconds: f64,
    pub fps: f32,
    pub frame_ms: Summary,
    pub low_1_fps: f32,
    pub low_01_fps: f32,
    pub std_dev_ms: f32,
    pub variance_ms2: f32,
    pub mean_delta_ms: f32,
}

impl FrameStats {
    pub fn of(frame_ms: &[f32]) -> FrameStats {
        let n = frame_ms.len();
        if n == 0 {
            return FrameStats::default();
        }
        let seconds = frame_ms.iter().map(|&x| f64::from(x)).sum::<f64>() / 1000.0;
        let mean = seconds * 1000.0 / n as f64;
        let variance = frame_ms.iter().map(|&x| (f64::from(x) - mean).powi(2)).sum::<f64>() / n as f64;
        let mut sorted = frame_ms.to_vec();
        sorted.sort_by(|a, b| b.total_cmp(a));
        let low = |share: f64| {
            let k = ((n as f64 * share).ceil() as usize).max(1);
            let worst = sorted[..k].iter().map(|&x| f64::from(x)).sum::<f64>() / k as f64;
            if worst > 0.0 { (1000.0 / worst) as f32 } else { 0.0 }
        };
        let deltas = frame_ms.windows(2).map(|w| f64::from((w[1] - w[0]).abs())).sum::<f64>();
        FrameStats {
            frames: n,
            seconds,
            fps: if seconds > 0.0 { (n as f64 / seconds) as f32 } else { 0.0 },
            frame_ms: Summary::of(frame_ms),
            low_1_fps: low(0.01),
            low_01_fps: low(0.001),
            std_dev_ms: variance.sqrt() as f32,
            variance_ms2: variance as f32,
            mean_delta_ms: if n > 1 { (deltas / (n - 1) as f64) as f32 } else { 0.0 },
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct GpuStats {
    pub frames: usize,
    pub frame_ms: Summary,
    pub cull_ms: f32,
    pub shadow_ms: f32,
    pub main_ms: f32,
    pub post_ms: f32,
    pub rest_ms: f32,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Machine {
    pub adapter: String,
    pub backend: String,
    pub device_type: String,
    pub driver: String,
    pub driver_info: String,
    pub gpu_timestamps: bool,
    pub os: String,
    pub kernel: String,
    pub cpu: String,
    pub physical_cores: String,
    pub threads: usize,
    pub memory: String,
    pub resolution: [u32; 2],
    pub scale_factor: f32,
    pub window_mode: String,
    pub present_mode: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Measured {
    pub frame: FrameStats,
    pub main_ms: Summary,
    pub extract_ms: Summary,
    pub main_thread_ms: Summary,
    pub render_ms: Summary,
    pub gpu: Option<GpuStats>,
    pub own_main_ms: f32,
    pub own_render_ms: f32,
    pub missing_render: usize,
    pub missing_gpu: usize,
    #[serde(default)]
    pub audio: Option<AudioStats>,
    #[serde(default)]
    pub present: Option<Pacing>,
    #[serde(default)]
    pub updates: Option<Pacing>,
    #[serde(default)]
    pub overshoot: Option<WaitOvershoot>,
    #[serde(default)]
    pub cpu: Option<CpuUse>,
    pub check: String,
    pub frames: Vec<[f32; 5]>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Pacing {
    pub intervals: usize,
    pub mean_ms: f32,
    pub std_dev_ms: f32,
    pub p1_ms: f32,
    pub p99_ms: f32,
    pub min_ms: f32,
    pub max_ms: f32,
}

impl Pacing {
    pub fn of(moments: &[f64]) -> Option<Pacing> {
        let ms: Vec<f32> = moments.windows(2).map(|w| ((w[1] - w[0]) * 1000.0) as f32).collect();
        if ms.is_empty() {
            return None;
        }
        let n = ms.len() as f64;
        let mean = ms.iter().map(|&x| f64::from(x)).sum::<f64>() / n;
        let variance = ms.iter().map(|&x| (f64::from(x) - mean).powi(2)).sum::<f64>() / n;
        let mut sorted = ms.clone();
        sorted.sort_by(f32::total_cmp);
        Some(Pacing {
            intervals: ms.len(),
            mean_ms: mean as f32,
            std_dev_ms: variance.sqrt() as f32,
            p1_ms: rank(&sorted, 0.01),
            p99_ms: rank(&sorted, 0.99),
            min_ms: sorted[0],
            max_ms: sorted[sorted.len() - 1],
        })
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct WaitOvershoot {
    pub frames: usize,
    pub regrids: usize,
    pub p50_us: f32,
    pub p99_us: f32,
    pub max_us: f32,
}

impl WaitOvershoot {
    pub fn of(t: &[f64], busy: &[f64], p: f64) -> Option<WaitOvershoot> {
        fn flush(run: &mut Vec<f64>, late: &mut Vec<f32>) {
            if let Some(&min) = run.iter().min_by(|a, b| a.total_cmp(b)) {
                late.extend(run.iter().map(|&d| ((d - min) * 1e6) as f32));
            }
            run.clear();
        }
        let (mut late, mut run, mut regrids) = (Vec::new(), Vec::new(), 0);
        let mut base = None::<(usize, f64)>;
        for k in 0..t.len().min(busy.len()) {
            let on_grid = base.and_then(|(s, ts)| {
                let grid = ts + (k - s) as f64 * p;
                (t[k - 1] + busy[k - 1] < grid).then_some(t[k] - grid)
            });
            match on_grid {
                Some(d) => run.push(d),
                None => {
                    regrids += usize::from(base.is_some());
                    flush(&mut run, &mut late);
                    base = Some((k, t[k]));
                    run.push(0.0);
                }
            }
        }
        flush(&mut run, &mut late);
        if late.is_empty() {
            return None;
        }
        late.sort_by(f32::total_cmp);
        Some(WaitOvershoot {
            frames: late.len(),
            regrids,
            p50_us: rank(&late, 0.50),
            p99_us: rank(&late, 0.99),
            max_us: late[late.len() - 1],
        })
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CpuUse {
    pub user_s: f32,
    pub system_s: f32,
    pub wall_s: f32,
    pub cores: f32,
}

#[derive(Clone, Copy, Debug)]
pub struct CpuCounters {
    user_us: u64,
    system_us: u64,
    at: std::time::Instant,
}

impl CpuCounters {
    #[cfg(target_os = "linux")]
    pub fn now() -> Option<CpuCounters> {
        let usage = unsafe {
            let mut usage: libc::rusage = std::mem::zeroed();
            (libc::getrusage(libc::RUSAGE_SELF, &mut usage) == 0).then_some(usage)
        }?;
        let us = |t: libc::timeval| {
            u64::try_from(t.tv_sec).unwrap_or(0).saturating_mul(1_000_000) + u64::try_from(t.tv_usec).unwrap_or(0)
        };
        Some(CpuCounters { user_us: us(usage.ru_utime), system_us: us(usage.ru_stime), at: std::time::Instant::now() })
    }

    #[cfg(not(target_os = "linux"))]
    pub fn now() -> Option<CpuCounters> {
        None
    }

    pub fn until(self, later: CpuCounters) -> CpuUse {
        let user_s = later.user_us.saturating_sub(self.user_us) as f64 * 1e-6;
        let system_s = later.system_us.saturating_sub(self.system_us) as f64 * 1e-6;
        let wall_s = later.at.saturating_duration_since(self.at).as_secs_f64();
        let cores = if wall_s > 0.0 { (user_s + system_s) / wall_s } else { 0.0 };
        CpuUse { user_s: user_s as f32, system_s: system_s as f32, wall_s: wall_s as f32, cores: cores as f32 }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AudioStats {
    pub rendered_s: f32,
    pub load_pct: f32,
    pub late_pulls: u64,
    pub starved: u64,
    pub underruns: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AudioCounters {
    pub rendered: u64,
    pub rendered_ns: u64,
    pub late: u64,
    pub starved: u64,
    pub underruns: u64,
}

impl AudioCounters {
    pub fn now() -> AudioCounters {
        use std::sync::atomic::Ordering::Relaxed;

        use crate::audio::music::stats;
        AudioCounters {
            rendered: stats::RENDERED.load(Relaxed),
            rendered_ns: stats::RENDERED_NS.load(Relaxed),
            late: stats::LATE.load(Relaxed),
            starved: stats::STARVED.load(Relaxed),
            underruns: stats::UNDERRUNS.load(Relaxed),
        }
    }

    pub fn until(self, later: AudioCounters, rate: u32) -> Option<AudioStats> {
        let frames = later.rendered.checked_sub(self.rendered).filter(|&f| f > 0)?;
        let rendered_s = frames as f64 / f64::from(rate.max(1));
        let ns = later.rendered_ns.saturating_sub(self.rendered_ns) as f64;
        Some(AudioStats {
            rendered_s: rendered_s as f32,
            load_pct: (ns * 1e-9 / rendered_s * 100.0) as f32,
            late_pulls: later.late.saturating_sub(self.late),
            starved: later.starved.saturating_sub(self.starved),
            underruns: later.underruns.saturating_sub(self.underruns),
        })
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Outcome {
    Measured(Box<Measured>),
    Skipped { reason: String },
    Failed { reason: String },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SceneReport {
    pub id: String,
    pub title: String,
    pub outcome: Outcome,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Part {
    pub format: u32,
    pub scene: SceneReport,
    pub machine: Option<Machine>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Report {
    pub format: u32,
    pub program: String,
    pub version: String,
    pub started: String,
    pub machine: Option<Machine>,
    pub options: Vec<String>,
    pub switches: Vec<String>,
    pub scenes: Vec<SceneReport>,
}

impl Measured {
    pub fn of(samples: &crate::shell::perf::timing::Samples, first: u32, last: u32, check: String) -> Measured {
        use std::collections::HashMap;
        let inside = |frame: u32| (first..=last).contains(&frame);
        let by_frame = |list: &[crate::shell::perf::timing::Tagged<f32>]| -> HashMap<u32, f32> {
            list.iter().filter(|t| inside(t.frame)).map(|t| (t.frame, t.value)).collect()
        };
        let extract = by_frame(&samples.extract);
        let render = by_frame(&samples.render);
        let render_own = by_frame(&samples.render_own);
        let gpu: HashMap<u32, crate::shell::perf::timing::GpuFrame> =
            samples.gpu.iter().filter(|t| inside(t.frame)).map(|t| (t.frame, t.value)).collect();
        let main: Vec<_> = samples.main.iter().filter(|m| inside(m.frame)).collect();
        let column = |f: &dyn Fn(&crate::shell::perf::timing::MainTimes) -> Option<f32>| -> Vec<f32> {
            main.iter().filter_map(|m| f(m)).collect()
        };
        let frame_ms = column(&|m| Some(m.frame_ms));
        let update_ms = column(&|m| Some(m.update_ms));
        let extract_ms = column(&|m| extract.get(&m.frame).copied());
        let main_thread_ms = column(&|m| extract.get(&m.frame).map(|e| m.update_ms + e));
        let render_ms = column(&|m| render.get(&m.frame).copied());
        let gpu_frames: Vec<_> = main.iter().filter_map(|m| gpu.get(&m.frame)).collect();
        let gpu_stats = (!gpu_frames.is_empty()).then(|| {
            let n = gpu_frames.len() as f32;
            let mean =
                |f: fn(&crate::shell::perf::timing::GpuFrame) -> f32| gpu_frames.iter().map(|g| f(g)).sum::<f32>() / n;
            GpuStats {
                frames: gpu_frames.len(),
                frame_ms: Summary::of(&gpu_frames.iter().map(|g| g.frame).collect::<Vec<_>>()),
                cull_ms: mean(|g| g.cull),
                shadow_ms: mean(|g| g.shadow),
                main_ms: mean(|g| g.main),
                post_ms: mean(|g| g.post),
                rest_ms: mean(|g| (g.frame - g.views).max(0.0)),
            }
        });
        let n = main.len().max(1) as f32;
        let own_main_ms = main.iter().map(|m| m.own_ms).sum::<f32>() / n;
        let own_render_ms = render_own.values().sum::<f32>() / render_own.len().max(1) as f32;
        let frames = main
            .iter()
            .map(|m| {
                let or_none = |v: Option<f32>| v.unwrap_or(-1.0);
                [
                    m.frame_ms,
                    m.update_ms,
                    or_none(extract.get(&m.frame).copied()),
                    or_none(render.get(&m.frame).copied()),
                    or_none(gpu.get(&m.frame).map(|g| g.frame)),
                ]
            })
            .collect();
        Measured {
            frame: FrameStats::of(&frame_ms),
            main_ms: Summary::of(&update_ms),
            extract_ms: Summary::of(&extract_ms),
            main_thread_ms: Summary::of(&main_thread_ms),
            render_ms: Summary::of(&render_ms),
            missing_render: main.len() - render_ms.len(),
            missing_gpu: if gpu.is_empty() && samples.gpu.is_empty() { 0 } else { main.len() - gpu_frames.len() },
            gpu: gpu_stats,
            own_main_ms,
            own_render_ms,
            audio: None,
            present: None,
            updates: None,
            overshoot: None,
            cpu: None,
            check,
            frames,
        }
    }
}

impl Measured {
    pub fn pace(&mut self, samples: &crate::shell::perf::timing::Samples, first: u32, last: u32, period: Option<f64>) {
        use std::collections::BTreeMap;
        let inside = |frame: u32| (first..=last).contains(&frame);
        let by_frame = |list: &[crate::shell::perf::timing::Tagged<f64>]| -> BTreeMap<u32, f64> {
            list.iter().filter(|t| inside(t.frame)).map(|t| (t.frame, t.value)).collect()
        };
        let presents: Vec<f64> = by_frame(&samples.presents).into_values().collect();
        let starts = by_frame(&samples.starts);
        self.present = Pacing::of(&presents);
        self.updates = Pacing::of(&starts.values().copied().collect::<Vec<_>>());
        let Some(p) = period else { return };
        let extract: BTreeMap<u32, f32> =
            samples.extract.iter().filter(|t| inside(t.frame)).map(|t| (t.frame, t.value)).collect();
        let main: BTreeMap<u32, (f32, Option<UpdateMode>)> =
            samples.main.iter().filter(|m| inside(m.frame)).map(|m| (m.frame, (m.update_ms, m.update_mode))).collect();
        let (mut t, mut busy, mut mode_before) = (Vec::new(), Vec::new(), None);
        for (frame, &start) in &starts {
            t.push(start);
            let mode = main.get(frame).and_then(|&(_, m)| m);
            let changed = matches!((mode_before, mode), (Some(a), Some(b)) if a != b);
            mode_before = mode;
            busy.push(match (main.get(frame), extract.get(frame)) {
                (Some(&(u, _)), Some(e)) if !changed => f64::from(u + e) * 1e-3,
                _ => f64::NAN,
            });
        }
        self.overshoot = WaitOvershoot::of(&t, &busy, p);
    }
}

impl Report {
    pub fn gpu_times(&self, m: &Machine) -> &'static str {
        let switched_off = self
            .switches
            .iter()
            .any(|s| s.strip_prefix("SSX_BENCH_GPU_MARKS=").is_some_and(|v| v.eq_ignore_ascii_case("off")));
        if m.gpu_timestamps {
            "measured"
        } else if switched_off {
            "not measured (SSX_BENCH_GPU_MARKS=off)"
        } else {
            "not available (no timestamp queries on this adapter)"
        }
    }

    pub fn text(&self) -> String {
        let mut t = String::new();
        let _ = writeln!(t, "{} {} — benchmark, {} UTC", self.program, self.version, self.started);
        match &self.machine {
            Some(m) => {
                let _ = writeln!(
                    t,
                    "GPU: {} ({}, {}; driver {} {}); GPU times: {}",
                    m.adapter,
                    m.backend,
                    m.device_type,
                    m.driver,
                    m.driver_info,
                    self.gpu_times(m)
                );
                let _ = writeln!(
                    t,
                    "CPU: {} ({} cores, {} threads for the game); memory {}",
                    m.cpu, m.physical_cores, m.threads, m.memory
                );
                let _ = writeln!(t, "OS: {} (kernel {})", m.os, m.kernel);
                let _ = writeln!(
                    t,
                    "Window: {} x {} pixels, scale {}, {}, present mode {}",
                    m.resolution[0], m.resolution[1], m.scale_factor, m.window_mode, m.present_mode
                );
            }
            None => _ = writeln!(t, "(no scene ran: the machine is not known)"),
        }
        if self.machine.as_ref().is_some_and(|m| m.present_mode.ends_with("(vsync)")) {
            let _ = writeln!(
                t,
                "VSync is on: a frame waits for the display, so the frame times stop at its refresh rate and the \
                 render thread's time includes the wait. Turn VSync off to measure what a frame costs."
            );
        }
        let _ = writeln!(t, "Options: {}", if self.options.is_empty() { "-".into() } else { self.options.join(" ") });
        let _ =
            writeln!(t, "Switches: {}", if self.switches.is_empty() { "-".into() } else { self.switches.join(" ") });
        let _ = writeln!(t);
        let _ = writeln!(
            t,
            "{:<10} {:>6} {:>7} | {:>6} {:>6} {:>6} {:>6} | {:>6} {:>7} {:>5} | {:>6} {:>7} {:>6} | {:>6} {:>6} | {:>23}",
            "scene",
            "frames",
            "avg fps",
            "avg",
            "p50",
            "p95",
            "p99",
            "1%low",
            "0.1%low",
            "sd",
            "main",
            "extract",
            "render",
            "GPU",
            "p95",
            "GPU shadow/main/post/UI"
        );
        let _ = writeln!(
            t,
            "{:<10} {:>6} {:>7} | {:^27} | {:^6} {:^7} {:>5} | {:^21} | {:^13} |",
            "", "", "", "frame time (ms)", "(fps)", "(fps)", "(ms)", "CPU, mean (ms)", "GPU (ms)"
        );
        let mut worst: Option<(&SceneReport, f32)> = None;
        for s in &self.scenes {
            match &s.outcome {
                Outcome::Measured(m) => {
                    let f = &m.frame;
                    let gpu = m.gpu.as_ref();
                    let g = |v: Option<f32>| v.map_or("-".to_string(), |v| format!("{v:.2}"));
                    let split = gpu.map_or("-".to_string(), |g| {
                        format!("{:.1}/{:.1}/{:.1}/{:.1}", g.shadow_ms, g.main_ms, g.post_ms, g.rest_ms)
                    });
                    let _ = writeln!(
                        t,
                        "{:<10} {:>6} {:>7.1} | {:>6.2} {:>6.2} {:>6.2} {:>6.2} | {:>6.1} {:>7.1} {:>5.2} | {:>6.2} {:>7.2} {:>6.2} | {:>6} {:>6} | {:>23}",
                        s.id,
                        f.frames,
                        f.fps,
                        f.frame_ms.avg,
                        f.frame_ms.p50,
                        f.frame_ms.p95,
                        f.frame_ms.p99,
                        f.low_1_fps,
                        f.low_01_fps,
                        f.std_dev_ms,
                        m.main_ms.avg,
                        m.extract_ms.avg,
                        m.render_ms.avg,
                        g(gpu.map(|g| g.frame_ms.avg)),
                        g(gpu.map(|g| g.frame_ms.p95)),
                        split
                    );
                    if worst.is_none_or(|(_, w)| f.frame_ms.avg > w) {
                        worst = Some((s, f.frame_ms.avg));
                    }
                }
                Outcome::Skipped { reason } => _ = writeln!(t, "{:<10} skipped: {reason}", s.id),
                Outcome::Failed { reason } => _ = writeln!(t, "{:<10} FAILED: {reason}", s.id),
            }
        }
        let _ = writeln!(t);
        let _ = writeln!(
            t,
            "{:<10} | {:^31} | {:^15} | {:^20} | {:>5}",
            "pacing", "present to present (ms)", "updates (ms)", "late on grid (µs)", "CPU"
        );
        let _ = writeln!(
            t,
            "{:<10} | {:>7} {:>7} {:>7} {:>7} | {:>7} {:>7} | {:>6} {:>6} {:>6} | {:>5}",
            "", "mean", "sd", "p1", "p99", "mean", "sd", "p50", "p99", "max", "cores"
        );
        for s in &self.scenes {
            if let Outcome::Measured(m) = &s.outcome {
                let p = |v: Option<f32>| v.map_or("-".to_string(), |v| format!("{v:.2}"));
                let us = |v: Option<f32>| v.map_or("-".to_string(), |v| format!("{v:.0}"));
                let (pr, up, o) = (m.present.as_ref(), m.updates.as_ref(), m.overshoot.as_ref());
                let _ = writeln!(
                    t,
                    "{:<10} | {:>7} {:>7} {:>7} {:>7} | {:>7} {:>7} | {:>6} {:>6} {:>6} | {:>5}",
                    s.id,
                    p(pr.map(|x| x.mean_ms)),
                    p(pr.map(|x| x.std_dev_ms)),
                    p(pr.map(|x| x.p1_ms)),
                    p(pr.map(|x| x.p99_ms)),
                    p(up.map(|x| x.mean_ms)),
                    p(up.map(|x| x.std_dev_ms)),
                    us(o.map(|x| x.p50_us)),
                    us(o.map(|x| x.p99_us)),
                    us(o.map(|x| x.max_us)),
                    p(m.cpu.map(|c| c.cores)),
                );
            }
        }
        let _ = writeln!(t);
        for s in &self.scenes {
            let _ = writeln!(t, "{:<10} {}", s.id, s.title);
        }
        let _ = writeln!(t);
        for s in &self.scenes {
            if let Outcome::Measured(m) = &s.outcome {
                let audio = m.audio.map_or_else(
                    || "; the audio mixer rendered nothing".to_string(),
                    |a| {
                        format!(
                            "; the audio mixer {:.0} % of real time, {} late pulls, {} underruns",
                            a.load_pct,
                            a.late_pulls,
                            a.starved + a.underruns
                        )
                    },
                );
                let _ = writeln!(t, "{:<10} {} ({:.1} s measured{audio})", s.id, m.check, m.frame.seconds);
            }
        }
        if let Some((s, _)) = worst
            && let Outcome::Measured(m) = &s.outcome
        {
            let bound = bound_by(m);
            let _ = writeln!(t);
            let _ = writeln!(
                t,
                "Slowest scene: {} ({:.2} ms a frame, {:.1} fps); {bound}",
                s.id, m.frame.frame_ms.avg, m.frame.fps
            );
        }
        let own: Vec<(f32, f32)> = self
            .scenes
            .iter()
            .filter_map(|s| match &s.outcome {
                Outcome::Measured(m) => Some((m.own_main_ms, m.own_render_ms)),
                _ => None,
            })
            .collect();
        if !own.is_empty() {
            let n = own.len() as f32;
            let _ = writeln!(
                t,
                "The measuring's own time: {:.3} ms a frame on the main thread, {:.3} ms on the render thread (mean)",
                own.iter().map(|o| o.0).sum::<f32>() / n,
                own.iter().map(|o| o.1).sum::<f32>() / n
            );
        }
        t
    }
}

pub fn bound_by(m: &Measured) -> String {
    let mut sides = vec![("the main thread", m.main_thread_ms.avg), ("the render thread", m.render_ms.avg)];
    if let Some(g) = &m.gpu {
        sides.push(("the GPU", g.frame_ms.avg));
    }
    sides.sort_by(|a, b| b.1.total_cmp(&a.1));
    let list: Vec<String> = sides.iter().map(|(n, v)| format!("{n} {v:.2} ms")).collect();
    format!("held by {} ({})", sides[0].0, list.join(", "))
}
