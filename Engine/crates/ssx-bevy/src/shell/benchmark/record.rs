use std::path::PathBuf;

use bevy::diagnostic::{FrameCount, SystemInfo};
use bevy::prelude::*;
use bevy::render::renderer::RenderAdapterInfo;
use bevy::window::{PresentMode, PrimaryWindow, WindowMode};

use super::report::{
    AudioCounters, AudioStats, CpuCounters, CpuUse, FORMAT, Machine, Measured, Outcome, Part, SceneReport,
};
use super::{Scene, SceneRun};
use crate::shell::perf::timing::{
    FrameTimes, FrameTimingPlugin, GpuTimestamps, MainTimesRecorded, Samples, TimingFrameEnd,
};

pub const WINDOW_OPENS: &str = "benchmark window opens";
pub const WINDOW_CLOSES: &str = "benchmark window closes";

const LATE_FRAMES: u32 = 8;

pub fn frame_tag(count: &FrameCount) -> u32 {
    count.0.wrapping_add(1)
}

#[derive(Resource, Debug, Default)]
pub struct SceneWindow {
    pub first: Option<u32>,
    pub last: Option<u32>,
    pub check: String,
    pub end_step: Option<(u64, Vec3)>,
    pub abort: Option<Outcome>,
    pub flight_frames: u32,
    audio_from: Option<AudioCounters>,
    pub audio: Option<AudioStats>,
    cpu_from: Option<CpuCounters>,
    pub cpu: Option<CpuUse>,
    period: Option<f64>,
}

impl SceneWindow {
    pub fn open(&mut self, tag: u32, scene: &str) {
        self.first = Some(tag);
        self.audio_from = Some(AudioCounters::now());
        self.cpu_from = CpuCounters::now();
        info!(name: WINDOW_OPENS, "benchmark: scene {scene}: the window opens (frame {tag})");
    }

    pub fn close(&mut self, tag: u32, scene: &str, check: String) {
        self.last = Some(tag);
        let rate = ssx_assets::sfx_aems::MIXER_RATE;
        self.audio = self.audio_from.take().and_then(|from| from.until(AudioCounters::now(), rate));
        self.cpu = self.cpu_from.take().zip(CpuCounters::now()).map(|(from, to)| from.until(to));
        info!(name: WINDOW_CLOSES, "benchmark: scene {scene}: the window closes (frame {tag}): {check}");
        self.check = check;
    }

    pub fn fail(&mut self, reason: impl Into<String>) {
        if self.abort.is_none() {
            self.abort = Some(Outcome::Failed { reason: reason.into() });
        }
    }
}

#[derive(Resource, Debug)]
pub struct SceneEnd {
    pub ok: bool,
}

#[derive(Resource, Debug)]
pub struct BenchmarkScene(pub &'static Scene);

#[derive(Resource)]
struct Recorder {
    part: PathBuf,
    samples: Samples,
    system: Option<std::thread::JoinHandle<SystemInfo>>,
    waited: u32,
    written: bool,
}

pub struct RecordPlugin(pub SceneRun);

impl Plugin for RecordPlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<FrameTimingPlugin>() {
            app.add_plugins(FrameTimingPlugin { gpu: self.0.gpu_marks });
        }
        let system = std::thread::Builder::new().name("benchmark: system info".into()).spawn(SystemInfo::default).ok();
        app.insert_resource(Recorder {
            part: self.0.part.clone(),
            samples: Samples::default(),
            system,
            waited: 0,
            written: false,
        })
        .insert_resource(BenchmarkScene(self.0.scene))
        .init_resource::<SceneWindow>()
        // PORT(choice): the benchmark draws its frames whether its window has the focus or not (Bevy's game settings
        // cap an unfocused window at 60 updates a second, which a measurement must not take for the game's own pace;
        // a frame limit the benchmark was given holds focused or not); the console's game always had the screen.
        .add_systems(Startup, unfocused_continuous)
        .add_systems(TimingFrameEnd, (collect, finish).chain().after(MainTimesRecorded));
    }
}

fn collect(times: Res<FrameTimes>, mut rec: ResMut<Recorder>) {
    let new = times.take();
    let s = &mut rec.samples;
    s.main.extend(new.main);
    s.extract.extend(new.extract);
    s.render.extend(new.render);
    s.render_own.extend(new.render_own);
    s.gpu.extend(new.gpu);
    s.starts.extend(new.starts);
    s.presents.extend(new.presents);
}

fn unfocused_continuous(
    pace: Option<ResMut<crate::game_mode::FramePace>>,
    mut window: ResMut<SceneWindow>,
    mut commands: Commands,
) {
    match pace {
        Some(mut pace) => {
            pace.unfocused = crate::game_mode::Unfocused::Continuous;
            window.period = pace.limit.period().map(|d| d.as_secs_f64());
        }
        None => commands.insert_resource(bevy::winit::WinitSettings::continuous()),
    }
}

fn finish(
    mut commands: Commands,
    mut rec: ResMut<Recorder>,
    mut window: ResMut<SceneWindow>,
    scene: Res<BenchmarkScene>,
    adapter: Option<Res<RenderAdapterInfo>>,
    gpu: Res<GpuTimestamps>,
    windows: Query<&Window, With<PrimaryWindow>>,
) {
    if rec.written {
        return;
    }
    let outcome = match (window.abort.take(), window.first, window.last) {
        (Some(abort), ..) => abort,
        (None, Some(first), Some(last)) => {
            let late = |list: &[crate::shell::perf::timing::Tagged<f32>]| list.iter().any(|t| t.frame >= last);
            let gpu_due = gpu.0.get().copied().unwrap_or(false);
            let arrived = late(&rec.samples.render) && (!gpu_due || rec.samples.gpu.iter().any(|t| t.frame >= last));
            if !arrived && rec.waited < LATE_FRAMES {
                rec.waited += 1;
                return;
            }
            let mut measured = Measured::of(&rec.samples, first, last, std::mem::take(&mut window.check));
            measured.audio = window.audio.take();
            measured.cpu = window.cpu.take();
            measured.pace(&rec.samples, first, last, window.period);
            Outcome::Measured(Box::new(measured))
        }
        _ => return,
    };
    rec.written = true;
    let ok = matches!(outcome, Outcome::Measured(_) | Outcome::Skipped { .. });
    let machine = machine(&mut rec, adapter.as_deref(), &gpu, windows.iter().next());
    let part = Part {
        format: FORMAT,
        scene: SceneReport { id: scene.0.id.to_string(), title: scene.0.title.to_string(), outcome },
        machine: Some(machine),
    };
    let written = serde_json::to_vec(&part).map_err(anyhow::Error::from).and_then(|bytes| {
        ssx_files::replace(&rec.part, &bytes, &ssx_files::Options::new()).map_err(anyhow::Error::from)
    });
    match written {
        Ok(()) => info!("benchmark: scene {}: result written to {}", scene.0.id, rec.part.display()),
        Err(e) => {
            error!("benchmark: scene {}: the result could not be written to {}: {e:#}", scene.0.id, rec.part.display())
        }
    }
    commands.insert_resource(SceneEnd { ok });
}

fn machine(
    rec: &mut Recorder,
    adapter: Option<&RenderAdapterInfo>,
    gpu: &GpuTimestamps,
    window: Option<&Window>,
) -> Machine {
    let system = rec.system.take().and_then(|h| h.join().ok());
    let mut m = Machine {
        gpu_timestamps: gpu.0.get().copied().unwrap_or(false),
        threads: std::thread::available_parallelism().map_or(0, |n| n.get()),
        ..Machine::default()
    };
    if let Some(a) = adapter {
        m.adapter = a.name.clone();
        m.backend = format!("{:?}", a.backend);
        m.device_type = format!("{:?}", a.device_type);
        m.driver = a.driver.clone();
        m.driver_info = a.driver_info.clone();
    }
    if let Some(s) = system {
        m.os = s.os;
        m.kernel = s.kernel;
        m.cpu = s.cpu;
        m.physical_cores = s.core_count;
        m.memory = s.memory;
    }
    if let Some(w) = window {
        m.resolution = [w.physical_width(), w.physical_height()];
        m.scale_factor = w.scale_factor();
        m.window_mode = match w.mode {
            WindowMode::Windowed => "windowed".to_string(),
            WindowMode::BorderlessFullscreen(_) => "borderless fullscreen".to_string(),
            WindowMode::Fullscreen(..) => "exclusive fullscreen".to_string(),
        };
        m.present_mode = match w.present_mode {
            PresentMode::AutoVsync | PresentMode::Fifo | PresentMode::FifoRelaxed => {
                format!("{:?} (vsync)", w.present_mode)
            }
            other => format!("{other:?} (no vsync)"),
        };
    }
    m
}
