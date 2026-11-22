pub mod timing;

use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use bevy::diagnostic::DiagnosticsStore;
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use bevy::render::diagnostic::RenderDiagnosticsPlugin;
use bevy::render::render_resource::{CachedPipelineState, PipelineCache, PipelineDescriptor};
use bevy::render::{Render, RenderApp};
use bevy::window::PrimaryWindow;

use self::timing::{FrameTimes, FrameTimingPlugin, GpuFrame};
use crate::render::terrain::TrackStats;

#[derive(Default)]
struct Shared {
    render_entities: u32,
    pipelines: (u32, u32, u32),
}

#[derive(Resource, Clone)]
struct PerfShared(Arc<Mutex<Shared>>);

impl PerfShared {
    fn lock(&self) -> std::sync::MutexGuard<'_, Shared> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }
}

#[derive(bevy::ecs::system::SystemParam)]
struct Counts<'w> {
    entities: &'w bevy::ecs::entity::Entities,
    archetypes: &'w bevy::ecs::archetype::Archetypes,
    meshes: Res<'w, Assets<Mesh>>,
    images: Res<'w, Assets<Image>>,
    particles: Option<Res<'w, crate::particles::ParticleFrame>>,
    lights: Option<Res<'w, crate::render::lighting::local::DynamicLocalLights>>,
    snow: Option<Res<'w, crate::render::snow_tracks::SnowTracks>>,
}

impl Counts<'_> {
    const HEADER: &'static str = "ents\tarchetypes\trender_ents\tmeshes\timages\tp_draws\tp_verts\tdyn_lights\tsnow_ms\t\
                                  sfx_ms\tsfx_cmd\tsfx_max\taud_load\taud_cpu\taud_drain\taud_max\taud_demand\taud_silence\tvoices\tgpu_mem_mb\ttex_bc_mb\ttex_rgba_mb\t\
                                  aud_drain_max\taud_drain_longest";

    fn line(&self, render_entities: u32) -> String {
        use std::sync::atomic::Ordering::Relaxed;
        let (pd, pv) = self.particles.as_ref().map_or((0, 0), |p| (p.draws.drawn, p.draws.vertices));
        let ms = |ns: u64| ns as f64 * 1e-6;
        let mb = |bytes: u64| bytes as f64 / (1024.0 * 1024.0);
        let calls = crate::audio::sfx::stats::CALLS.swap(0, Relaxed).max(1) as f64;
        let sfx = ms(crate::audio::sfx::stats::NS.swap(0, Relaxed)) / calls;
        let sfx_cmd = ms(crate::audio::sfx::stats::COMMAND_NS.swap(0, Relaxed)) / calls;
        let sfx_max = ms(crate::audio::sfx::stats::MAX_NS.swap(0, Relaxed));
        let audio_s =
            crate::audio::music::stats::FRAMES.swap(0, Relaxed) as f64 / f64::from(ssx_assets::sfx_aems::MIXER_RATE);
        let load = ms(crate::audio::music::stats::RENDER_NS.swap(0, Relaxed)) / (audio_s * 1000.0).max(1e-9) * 100.0;
        let cpu = ms(crate::audio::music::stats::RENDER_CPU_NS.swap(0, Relaxed)) / (audio_s * 1000.0).max(1e-9) * 100.0;
        let drain = ms(crate::audio::music::stats::DRAIN_NS.swap(0, Relaxed));
        let aud_max = ms(crate::audio::music::stats::RENDER_MAX_NS.swap(0, Relaxed));
        let longest = crate::audio::music::stats::take_longest_drain(crate::audio::music::stats::DrainReader::Perf);
        let drain_max = longest.map_or(0.0, |d| ms(d.ns));
        let longest = longest.map_or_else(|| "-".to_owned(), |d| d.to_string());
        format!(
            "{}\t{}\t{render_entities}\t{}\t{}\t{pd}\t{pv}\t{}\t{:.2}\t{sfx:.2}\t{sfx_cmd:.2}\t{sfx_max:.1}\t\
             {load:.1}\t{cpu:.1}\t{drain:.1}\t{aud_max:.1}\t{}\t{}\t{}\t{:.0}\t{:.1}\t{:.1}\t{drain_max:.1}\t{longest}",
            self.entities.count_spawned(),
            self.archetypes.len(),
            self.meshes.len(),
            self.images.len(),
            self.lights.as_ref().map_or(0, |l| l.lights.len()),
            self.snow.as_ref().map_or(0.0, |t| t.update_ms),
            crate::audio::music::stats::ON_DEMAND.swap(0, Relaxed),
            crate::audio::music::stats::SILENCE.swap(0, Relaxed),
            crate::audio::sfx::stats::VOICES.load(Relaxed),
            gpu_memory_mb(),
            mb(crate::render::terrain::textures::BLOCK_BYTES.load(Relaxed)),
            mb(crate::render::terrain::textures::DECODED_BYTES.load(Relaxed)),
        )
    }
}

fn gpu_memory_mb() -> f64 {
    let Ok(dir) = std::fs::read_dir("/proc/self/fdinfo") else { return 0.0 };
    let mut clients = std::collections::HashSet::new();
    let mut kib = 0.0;
    for entry in dir.flatten() {
        let Ok(text) = std::fs::read_to_string(entry.path()) else { continue };
        let Some(client) = text.lines().find_map(|l| l.strip_prefix("drm-client-id:")) else { continue };
        if !clients.insert(client.trim().to_string()) {
            continue;
        }
        for line in text.lines().filter(|l| l.starts_with("drm-total-")) {
            let Some((_, v)) = line.split_once(':') else { continue };
            let mut it = v.split_whitespace();
            let n: f64 = it.next().and_then(|n| n.parse().ok()).unwrap_or(0.0);
            kib += match it.next() {
                Some("MiB") => n * 1024.0,
                Some("GiB") => n * 1024.0 * 1024.0,
                Some("KiB") => n,
                _ => n / 1024.0,
            };
        }
    }
    kib / 1024.0
}

pub struct PerfPlugin {
    out: PathBuf,
    window: f32,
}

impl PerfPlugin {
    pub fn from_env() -> Option<PerfPlugin> {
        let out = std::env::var_os("SSX_PERF").filter(|v| !v.is_empty())?;
        let window = std::env::var("SSX_PERF_WINDOW").ok().and_then(|v| v.parse().ok()).unwrap_or(2.0);
        Some(PerfPlugin { out: out.into(), window })
    }
}

impl Plugin for PerfPlugin {
    fn build(&self, app: &mut App) {
        let file = std::fs::File::create(&self.out).unwrap_or_else(|e| panic!("SSX_PERF {}: {e}", self.out.display()));
        let shared = PerfShared(Arc::default());
        if !app.is_plugin_added::<RenderDiagnosticsPlugin>() {
            app.add_plugins(RenderDiagnosticsPlugin);
        }
        if !app.is_plugin_added::<FrameTimingPlugin>() {
            app.add_plugins(FrameTimingPlugin { gpu: true });
        }
        let now = Instant::now();
        app.insert_resource(PerfState {
            out: std::io::BufWriter::new(file),
            window: self.window,
            start: now,
            window_start: now,
            frame_ms: Vec::new(),
            main_ms: Vec::new(),
            render_ms: Vec::new(),
            gpu: Vec::new(),
            header: false,
            exit: std::env::var("SSX_PERF_EXIT").ok().and_then(|v| v.parse().ok()),
            hitch_ms: std::env::var("SSX_PERF_HITCH").ok().and_then(|v| v.parse().ok()).unwrap_or(50.0),
            hitches: std::fs::File::create(self.out.with_extension("hitches.tsv")).ok().map(|f| {
                let mut w = std::io::BufWriter::new(f);
                let _ = writeln!(
                    w,
                    "t\tscene\tframe_ms\tmain_ms\tpipes_asked\tpipes_created\tpipes_waiting\tcreated_since"
                );
                w
            }),
            hitch_pipes: 0,
        })
        .insert_resource(shared.clone())
        .add_systems(timing::TimingFrameEnd, frame_end.after(timing::MainTimesRecorded))
        .add_systems(Startup, (window_size, continuous));
        let Some(render) = app.get_sub_app_mut(RenderApp) else { return };
        let pipeline_log = std::fs::File::create(self.out.with_extension("pipelines.tsv")).ok().map(|f| {
            let mut w = std::io::BufWriter::new(f);
            let _ = writeln!(w, "id\tasked_s\tcreated_s\twait_ms\tlabel\tdetail");
            w
        });
        render
            .insert_resource(PipelineLog { out: pipeline_log, start: now, seen: Vec::new() })
            .insert_resource(shared)
            .add_systems(Render, pipelines.in_set(bevy::render::RenderSystems::PostCleanup));
    }
}

#[derive(Resource)]
struct PerfState {
    out: std::io::BufWriter<std::fs::File>,
    window: f32,
    start: Instant,
    window_start: Instant,
    frame_ms: Vec<f32>,
    main_ms: Vec<f32>,
    render_ms: Vec<f32>,
    gpu: Vec<GpuFrame>,
    header: bool,
    exit: Option<f32>,
    hitch_ms: f32,
    hitches: Option<std::io::BufWriter<std::fs::File>>,
    hitch_pipes: u32,
}

fn window_size(mut windows: Query<&mut Window, With<PrimaryWindow>>) {
    let Some((w, h)) = std::env::var("SSX_PERF_SIZE").ok().and_then(|s| {
        let (w, h) = s.split_once('x')?;
        Some((w.parse().ok()?, h.parse().ok()?))
    }) else {
        return;
    };
    for mut window in &mut windows {
        window.resolution.set_scale_factor_override(Some(1.0));
        window.resolution.set_physical_resolution(w, h);
    }
}

fn continuous(pace: Option<ResMut<crate::game_mode::FramePace>>, mut commands: Commands) {
    match pace {
        Some(mut pace) => pace.unfocused = crate::game_mode::Unfocused::Continuous,
        None => commands.insert_resource(bevy::winit::WinitSettings::continuous()),
    }
}

fn stats(v: &mut [f32], q: f32) -> (f32, f32) {
    if v.is_empty() {
        return (0.0, 0.0);
    }
    v.sort_by(f32::total_cmp);
    let mean = v.iter().sum::<f32>() / v.len() as f32;
    (mean, v[((v.len() - 1) as f32 * q).round() as usize])
}

#[derive(SystemParam)]
struct WindowInputs<'w, 's> {
    times: Res<'w, FrameTimes>,
    shared: Res<'w, PerfShared>,
    diagnostics: Res<'w, DiagnosticsStore>,
    track: Option<Res<'w, TrackStats>>,
    counts: Counts<'w>,
    exit: MessageWriter<'w, AppExit>,
    _marker: std::marker::PhantomData<&'s ()>,
}

fn frame_end(mut s: ResMut<PerfState>, mut w: WindowInputs) {
    let now = Instant::now();
    let samples = w.times.take();
    s.render_ms.extend(samples.render.iter().map(|r| r.value));
    s.gpu.extend(samples.gpu.iter().map(|g| g.value));
    for m in &samples.main {
        s.frame_ms.push(m.frame_ms);
        s.main_ms.push(m.update_ms);
        if m.frame_ms > s.hitch_ms {
            let (asked, created, waiting) = w.shared.lock().pipelines;
            let since = created.saturating_sub(std::mem::replace(&mut s.hitch_pipes, created));
            let t = (now - s.start).as_secs_f32();
            let scene = w.track.as_ref().map_or("fe", |t| t.name.as_str()).to_string();
            let (frame_ms, main) = (m.frame_ms, m.update_ms);
            if let Some(out) = s.hitches.as_mut() {
                let _ =
                    writeln!(out, "{t:.2}\t{scene}\t{frame_ms:.1}\t{main:.1}\t{asked}\t{created}\t{waiting}\t{since}");
                let _ = out.flush();
            }
        }
    }
    if s.exit.is_some_and(|e| (now - s.start).as_secs_f32() >= e) {
        w.exit.write(AppExit::Success);
    }
    if (now - s.window_start).as_secs_f32() < s.window || s.frame_ms.is_empty() {
        return;
    }
    let scene = w.track.as_ref().map_or_else(|| "fe".to_string(), |t| t.name.clone());
    let n = s.frame_ms.len();
    let mut frames = std::mem::take(&mut s.frame_ms);
    let mut mains = std::mem::take(&mut s.main_ms);
    let mut render = std::mem::take(&mut s.render_ms);
    let gpu = std::mem::take(&mut s.gpu);
    let (ft, ft50) = stats(&mut frames, 0.5);
    let ft95 = frames[((n - 1) as f32 * 0.95).round() as usize];
    let ft99 = frames[((n - 1) as f32 * 0.99).round() as usize];
    let ftmax = frames.last().copied().unwrap_or(0.0);
    let (main, main95) = stats(&mut mains, 0.95);
    let (rend, rend95) = stats(&mut render, 0.95);
    let g = gpu.len().max(1) as f32;
    let sum = |f: fn(&GpuFrame) -> f32| gpu.iter().map(f).sum::<f32>() / g;
    let mut gframes: Vec<f32> = gpu.iter().map(|f| f.frame).collect();
    let (gf, gf95) = stats(&mut gframes, 0.95);
    let pass = |name: &str| {
        w.diagnostics
            .iter()
            .find(|d| d.path().as_str() == format!("render/{name}/elapsed_gpu"))
            .and_then(|d| d.average())
            .unwrap_or(0.0)
    };
    if !s.header {
        s.header = true;
        let _ = writeln!(
            s.out,
            "t\tscene\tframes\tft_avg\tft_p50\tft_p95\tft_p99\tft_max\tmain_avg\tmain_p95\trender_avg\trender_p95\t\
             gpu_n\tgpu_frame\tgpu_p95\tgpu_views\tgpu_cull\tgpu_shadow\tgpu_main\tgpu_post\tgpu_rest\topaque\ttransparent\t\
             {}\tpipes_created\tpipes_waiting",
            Counts::HEADER
        );
    }
    let t = (now - s.start).as_secs_f32();
    let (render_entities, (_, created, waiting)) = {
        let sh = w.shared.lock();
        (sh.render_entities, sh.pipelines)
    };
    let line = format!(
        "{t:.1}\t{scene}\t{n}\t{ft:.2}\t{ft50:.2}\t{ft95:.2}\t{ft99:.2}\t{ftmax:.2}\t{main:.2}\t{main95:.2}\t{rend:.2}\t\
         {rend95:.2}\t{}\t{gf:.2}\t{gf95:.2}\t{:.2}\t{:.2}\t{:.2}\t{:.2}\t{:.2}\t{:.2}\t{:.2}\t{:.2}\t{}\t{created}\t\
         {waiting}",
        gpu.len(),
        sum(|f| f.views),
        sum(|f| f.cull),
        sum(|f| f.shadow),
        sum(|f| f.main),
        sum(|f| f.post),
        sum(|f| f.frame - f.views),
        pass("main_opaque_pass_3d"),
        pass("main_transparent_pass_3d"),
        w.counts.line(render_entities),
    );
    let _ = writeln!(s.out, "{line}");
    let _ = s.out.flush();
    s.window_start = now;
}

#[derive(Resource)]
struct PipelineLog {
    out: Option<std::io::BufWriter<std::fs::File>>,
    start: Instant,
    seen: Vec<(f32, bool)>,
}

fn pipelines(
    shared: Res<PerfShared>,
    entities: &bevy::ecs::entity::Entities,
    cache: Res<PipelineCache>,
    mut log: ResMut<PipelineLog>,
    mut frames: Local<u32>,
) {
    let mut sh = shared.lock();
    let (mut asked, mut created) = (0, 0);
    let now = log.start.elapsed().as_secs_f32();
    let PipelineLog { out, seen, .. } = &mut *log;
    for (id, p) in cache.pipelines().enumerate() {
        asked += 1;
        let ok = matches!(p.state, CachedPipelineState::Ok(_));
        created += u32::from(ok);
        if seen.len() <= id {
            seen.resize(id + 1, (now, false));
        }
        if ok && !seen[id].1 {
            seen[id].1 = true;
            let (label, detail) = match &p.descriptor {
                PipelineDescriptor::RenderPipelineDescriptor(d) => {
                    let attributes: usize = d.vertex.buffers.iter().map(|b| b.attributes.len()).sum();
                    let mut defs: Vec<String> = d.vertex.shader_defs.iter().map(|s| format!("{s:?}")).collect();
                    defs.extend(d.fragment.iter().flat_map(|f| f.shader_defs.iter().map(|s| format!("{s:?}"))));
                    defs.sort();
                    defs.dedup();
                    (
                        d.label.as_deref().unwrap_or("-").to_string(),
                        format!("attributes {attributes}; {}", defs.join(" ")),
                    )
                }
                PipelineDescriptor::ComputePipelineDescriptor(d) => {
                    (d.label.as_deref().unwrap_or("-").to_string(), String::new())
                }
                PipelineDescriptor::MeshPipelineDescriptor(d) => {
                    let mut defs: Vec<String> = d.mesh.shader_defs.iter().map(|s| format!("{s:?}")).collect();
                    defs.extend(d.task.iter().flat_map(|t| t.shader_defs.iter().map(|s| format!("{s:?}"))));
                    defs.extend(d.fragment.iter().flat_map(|f| f.shader_defs.iter().map(|s| format!("{s:?}"))));
                    defs.sort();
                    defs.dedup();
                    (d.label.as_deref().unwrap_or("-").to_string(), format!("mesh shader; {}", defs.join(" ")))
                }
            };
            if let Some(w) = out.as_mut() {
                let wait = (now - seen[id].0) * 1000.0;
                let _ = writeln!(w, "{id}\t{:.2}\t{now:.2}\t{wait:.0}\t{label}\t{detail}", seen[id].0);
                let _ = w.flush();
            }
        }
    }
    sh.pipelines = (asked, created, cache.waiting_pipelines().count() as u32);
    *frames = frames.wrapping_add(1);
    if *frames % 30 == 1 {
        sh.render_entities = entities.count_spawned();
    }
}
