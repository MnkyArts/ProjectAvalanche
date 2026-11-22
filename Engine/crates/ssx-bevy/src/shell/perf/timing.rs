use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Instant;

use bevy::app::MainScheduleOrder;
use bevy::core_pipeline::upscaling::upscaling;
use bevy::core_pipeline::{Core3d, Core3dSystems};
use bevy::diagnostic::FrameCount;
use bevy::ecs::schedule::{ScheduleLabel, SingleThreadedExecutor};
use bevy::pbr::{
    EARLY_SHADOW_PASS, LATE_SHADOW_PASS, allocate_uniforms, early_gpu_preprocess, main_build_indirect_parameters,
    per_view_shadow_pass, shared_shadow_pass,
};
use bevy::prelude::*;
use bevy::render::render_resource::{Buffer, BufferDescriptor, BufferUsages, CommandEncoderDescriptor, MapMode};
use bevy::render::renderer::{
    CurrentView, PendingCommandBuffers, RenderContext, RenderDevice, RenderGraph, RenderGraphSystems, RenderQueue,
};
use bevy::render::settings::WgpuFeatures;
use bevy::render::{Render, RenderApp, RenderScheduleOrder, RenderStartup, RenderSystems};
use bevy::winit::{UpdateMode, WinitSettings};

const MAX_MARKS: u32 = 64;
const SLOTS: usize = 4;

const FRAME_BEGIN: u8 = 0;
const VIEW_BEGIN: u8 = 1;
const SHADOW_BEGIN: u8 = 2;
const SHADOW_END: u8 = 3;
const MAIN_BEGIN: u8 = 4;
const MAIN_END: u8 = 5;
const VIEW_END: u8 = 6;
const FRAME_END: u8 = 7;

#[derive(ScheduleLabel, Clone, Debug, PartialEq, Eq, Hash)]
struct TimingFrameStart;
#[derive(ScheduleLabel, Clone, Debug, PartialEq, Eq, Hash)]
pub struct TimingFrameEnd;

#[derive(ScheduleLabel, Clone, Debug, PartialEq, Eq, Hash)]
struct TimingRenderEnd;

#[derive(SystemSet, Clone, Debug, PartialEq, Eq, Hash)]
pub struct MainTimesRecorded;

#[derive(Clone, Copy, Debug, Default)]
pub struct GpuFrame {
    pub frame: f32,
    pub views: f32,
    pub cull: f32,
    pub shadow: f32,
    pub main: f32,
    pub post: f32,
}

#[derive(Clone, Copy, Debug)]
pub struct MainTimes {
    pub frame: u32,
    pub frame_ms: f32,
    pub update_ms: f32,
    pub own_ms: f32,
    pub update_mode: Option<UpdateMode>,
}

#[derive(Clone, Copy, Debug)]
pub struct Tagged<T> {
    pub frame: u32,
    pub value: T,
}

#[derive(Default)]
pub struct Samples {
    pub main: Vec<MainTimes>,
    pub extract: Vec<Tagged<f32>>,
    pub render: Vec<Tagged<f32>>,
    pub render_own: Vec<Tagged<f32>>,
    pub gpu: Vec<Tagged<GpuFrame>>,
    pub starts: Vec<Tagged<f64>>,
    pub presents: Vec<Tagged<f64>>,
}

pub fn clock_secs(t: Instant) -> f64 {
    static EPOCH: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    t.saturating_duration_since(*EPOCH.get_or_init(|| t)).as_secs_f64()
}

#[derive(Resource, Clone, Default)]
pub struct FrameTimes(Arc<Mutex<Samples>>);

impl FrameTimes {
    fn lock(&self) -> MutexGuard<'_, Samples> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn take(&self) -> Samples {
        std::mem::take(&mut *self.lock())
    }
}

#[derive(Resource, Clone, Default)]
pub struct GpuTimestamps(pub Arc<std::sync::OnceLock<bool>>);

pub struct FrameTimingPlugin {
    pub gpu: bool,
}

#[derive(Resource, Default)]
struct UpdateBegin(Option<Instant>);

impl Plugin for FrameTimingPlugin {
    fn build(&self, app: &mut App) {
        let times = FrameTimes::default();
        let gpu = GpuTimestamps::default();
        app.init_schedule(TimingFrameStart).init_schedule(TimingFrameEnd);
        let mut order = app.world_mut().resource_mut::<MainScheduleOrder>();
        order.insert_before(First, TimingFrameStart);
        order.insert_after(Last, TimingFrameEnd);
        app.insert_resource(times.clone())
            .insert_resource(gpu.clone())
            .init_resource::<UpdateBegin>()
            .add_systems(TimingFrameStart, |mut begin: ResMut<UpdateBegin>| begin.0 = Some(Instant::now()))
            .add_systems(TimingFrameEnd, main_end.in_set(MainTimesRecorded));
        let Some(render) = app.get_sub_app_mut(RenderApp) else { return };
        render
            .insert_resource(times)
            .init_resource::<RenderClock>()
            .add_systems(
                Render,
                present_mark.after(bevy::render::renderer::render_system).in_set(RenderSystems::Render),
            )
            .add_systems(
                Render,
                RENDER_SETS[1..]
                    .iter()
                    .fold(render_begin.in_set(RENDER_SETS[0].clone()), |s, set| s.before(set.clone())),
            );
        if !self.gpu {
            let _ = gpu.0.set(false);
            return;
        }
        render
            .insert_resource(gpu)
            .add_systems(RenderStartup, init_marks)
            .add_systems(
                RenderGraph,
                (
                    frame_begin.in_set(RenderGraphSystems::Begin).before(RenderGraphSystems::Render),
                    frame_end_marks.after(RenderGraphSystems::Render).before(RenderGraphSystems::Submit),
                    map_marks.in_set(RenderGraphSystems::Finish).after(RenderGraphSystems::Submit),
                ),
            )
            .add_systems(
                Core3d,
                (
                    mark::<VIEW_BEGIN>.before(allocate_uniforms),
                    mark::<SHADOW_BEGIN>
                        .after(early_gpu_preprocess)
                        .before(per_view_shadow_pass::<EARLY_SHADOW_PASS>)
                        .before(shared_shadow_pass::<EARLY_SHADOW_PASS>),
                    mark::<SHADOW_END>
                        .after(per_view_shadow_pass::<LATE_SHADOW_PASS>)
                        .after(shared_shadow_pass::<LATE_SHADOW_PASS>)
                        .before(main_build_indirect_parameters),
                    mark::<MAIN_BEGIN>.after(main_build_indirect_parameters).before(Core3dSystems::MainPass),
                    mark::<MAIN_END>.after(Core3dSystems::MainPass).before(Core3dSystems::EarlyPostProcess),
                    mark::<VIEW_END>.after(Core3dSystems::PostProcess).after(upscaling),
                ),
            );
    }

    fn finish(&self, app: &mut App) {
        let times = app.world().resource::<FrameTimes>().clone();
        let Some(render) = app.get_sub_app_mut(RenderApp) else { return };
        match render.world_mut().get_resource_mut::<RenderScheduleOrder>() {
            Some(mut order) => order.labels.push(TimingRenderEnd.intern()),
            None => warn!("frame timing: the render app has no RenderScheduleOrder; no render thread times"),
        }
        let mut end = Schedule::new(TimingRenderEnd);
        end.set_executor(SingleThreadedExecutor::new());
        render.add_schedule(end).add_systems(TimingRenderEnd, render_end);
        let Some(mut extract) = render.take_extract() else {
            warn!("frame timing: the render app has no extract function; no extract times");
            return;
        };
        render.set_extract(move |main, world| {
            let begin = Instant::now();
            extract(main, world);
            let ms = begin.elapsed().as_secs_f32() * 1000.0;
            let frame = main.get_resource::<FrameCount>().map_or(0, |f| f.0);
            times.lock().extract.push(Tagged { frame, value: ms });
        });
    }
}

fn main_end(
    mut begin: ResMut<UpdateBegin>,
    time: Res<Time<Real>>,
    frame: Option<Res<FrameCount>>,
    times: Res<FrameTimes>,
    (settings, windows): (Option<Res<WinitSettings>>, Query<&Window>),
) {
    let own = Instant::now();
    let Some(start) = begin.0.take() else { return };
    let update_ms = (own - start).as_secs_f32() * 1000.0;
    let mut samples = times.lock();
    let own_ms = own.elapsed().as_secs_f32() * 1000.0;
    let frame = frame.map_or(0, |f| f.0);
    let update_mode = settings.map(|s| s.update_mode(windows.iter().any(|w| w.focused)));
    samples.main.push(MainTimes { frame, frame_ms: time.delta_secs() * 1000.0, update_ms, own_ms, update_mode });
    samples.starts.push(Tagged { frame, value: clock_secs(start) });
}

fn present_mark(frame: Option<Res<FrameCount>>, times: Res<FrameTimes>) {
    let now = clock_secs(Instant::now());
    times.lock().presents.push(Tagged { frame: render_frame(frame), value: now });
}

const RENDER_SETS: [RenderSystems; 12] = [
    RenderSystems::ExtractCommands,
    RenderSystems::PrepareAssets,
    RenderSystems::PrepareMeshes,
    RenderSystems::CreateViews,
    RenderSystems::Specialize,
    RenderSystems::PrepareViews,
    RenderSystems::Queue,
    RenderSystems::PhaseSort,
    RenderSystems::Prepare,
    RenderSystems::Render,
    RenderSystems::Cleanup,
    RenderSystems::PostCleanup,
];

#[derive(Resource, Default)]
struct RenderClock {
    begin: Option<Instant>,
    own_ns: AtomicU64,
}

impl RenderClock {
    fn add_own(&self, since: Instant) {
        self.own_ns.fetch_add(since.elapsed().as_nanos() as u64, Ordering::Relaxed);
    }
}

fn render_begin(mut clock: ResMut<RenderClock>) {
    clock.begin = Some(Instant::now());
}

fn render_frame(frame: Option<Res<FrameCount>>) -> u32 {
    frame.map_or(0, |f| f.0)
}

fn render_end(mut clock: ResMut<RenderClock>, frame: Option<Res<FrameCount>>, times: Res<FrameTimes>) {
    let own = Instant::now();
    let Some(begin) = clock.begin.take() else { return };
    let ms = (own - begin).as_secs_f32() * 1000.0;
    let frame = render_frame(frame);
    let mut samples = times.lock();
    samples.render.push(Tagged { frame, value: ms });
    clock.add_own(own);
    let own_ns = std::mem::take(clock.own_ns.get_mut());
    samples.render_own.push(Tagged { frame, value: own_ns as f32 * 1e-6 });
}

#[derive(Clone, Copy)]
struct Record {
    mark: u8,
    view: Option<Entity>,
    index: u32,
}

struct Slot {
    read: Buffer,
    mapped: Arc<AtomicBool>,
    pending: Option<(u32, Vec<Record>)>,
}

#[derive(Default)]
struct Current {
    slot: Option<usize>,
    frame: u32,
    records: Vec<Record>,
    map: Option<usize>,
}

#[derive(Resource)]
struct GpuMarks {
    queries: wgpu::QuerySet,
    resolve: Buffer,
    slots: Vec<Slot>,
    period: f32,
    current: Mutex<Current>,
}

fn init_marks(mut commands: Commands, device: Res<RenderDevice>, queue: Res<RenderQueue>, gpu: Res<GpuTimestamps>) {
    let needed = WgpuFeatures::TIMESTAMP_QUERY | WgpuFeatures::TIMESTAMP_QUERY_INSIDE_ENCODERS;
    let supported = device.features().contains(needed);
    let _ = gpu.0.set(supported);
    if !supported {
        warn!("frame timing: the adapter has no timestamp queries inside encoders; no GPU times");
        return;
    }
    let size = u64::from(MAX_MARKS) * 8;
    let buffer = |label, usage| {
        device.create_buffer(&BufferDescriptor { label: Some(label), size, usage, mapped_at_creation: false })
    };
    let queries = device.wgpu_device().create_query_set(&wgpu::QuerySetDescriptor {
        label: Some("ssx_perf_marks"),
        ty: wgpu::QueryType::Timestamp,
        count: MAX_MARKS,
    });
    let slots = (0..SLOTS)
        .map(|_| Slot {
            read: buffer("ssx_perf_read", BufferUsages::COPY_DST | BufferUsages::MAP_READ),
            mapped: Arc::default(),
            pending: None,
        })
        .collect();
    commands.insert_resource(GpuMarks {
        queries,
        resolve: buffer("ssx_perf_resolve", BufferUsages::QUERY_RESOLVE | BufferUsages::COPY_SRC),
        slots,
        period: queue.get_timestamp_period(),
        current: Mutex::default(),
    });
}

fn frame_begin(
    marks: Option<ResMut<GpuMarks>>,
    times: Res<FrameTimes>,
    clock: Res<RenderClock>,
    frame: Option<Res<FrameCount>>,
    device: Res<RenderDevice>,
    mut pending: ResMut<PendingCommandBuffers>,
) {
    let own = Instant::now();
    let Some(mut marks) = marks else { return };
    let period = marks.period;
    for slot in &mut marks.slots {
        if !slot.mapped.load(Ordering::Acquire) {
            continue;
        }
        if let Some((frame, records)) = slot.pending.take() {
            let ticks: Result<Vec<u64>, _> = slot
                .read
                .slice(..)
                .get_mapped_range()
                .map(|view| view.as_chunks::<8>().0.iter().map(|b| u64::from_le_bytes(*b)).collect());
            match ticks {
                Ok(ticks) => {
                    let value = gpu_frame(&records, &ticks, period);
                    times.lock().gpu.push(Tagged { frame, value });
                }
                Err(e) => {
                    error!("frame timing: the GPU marks' read-back failed, the frame's GPU times are dropped: {e}")
                }
            }
        }
        slot.read.unmap();
        slot.mapped.store(false, Ordering::Release);
    }
    let free = marks.slots.iter().position(|s| s.pending.is_none() && !s.mapped.load(Ordering::Acquire));
    let mut encoder = device.create_command_encoder(&CommandEncoderDescriptor { label: Some("ssx_perf_begin") });
    let GpuMarks { queries, current, .. } = &mut *marks;
    let current = current.get_mut().unwrap_or_else(|e| e.into_inner());
    *current = Current { slot: free, frame: render_frame(frame), ..default() };
    if free.is_some() {
        encoder.write_timestamp(queries, 0);
        current.records.push(Record { mark: FRAME_BEGIN, view: None, index: 0 });
        pending.push([encoder.finish()]);
    }
    clock.add_own(own);
}

fn mark<const M: u8>(
    mut ctx: RenderContext,
    marks: Option<Res<GpuMarks>>,
    view: Res<CurrentView>,
    clock: Res<RenderClock>,
) {
    let own = Instant::now();
    let Some(marks) = marks else { return };
    let mut current = marks.current.lock().unwrap_or_else(|e| e.into_inner());
    let index = current.records.len() as u32;
    if current.slot.is_none() || index + 1 >= MAX_MARKS {
        return;
    }
    ctx.command_encoder().write_timestamp(&marks.queries, index);
    current.records.push(Record { mark: M, view: Some(view.0), index });
    clock.add_own(own);
}

fn frame_end_marks(
    marks: Option<ResMut<GpuMarks>>,
    clock: Res<RenderClock>,
    device: Res<RenderDevice>,
    mut pending: ResMut<PendingCommandBuffers>,
) {
    let own = Instant::now();
    let Some(mut marks) = marks else { return };
    let GpuMarks { queries, resolve, slots, current, .. } = &mut *marks;
    let current = current.get_mut().unwrap_or_else(|e| e.into_inner());
    let Some(slot) = current.slot.take() else { return };
    let index = current.records.len() as u32;
    let mut encoder = device.create_command_encoder(&CommandEncoderDescriptor { label: Some("ssx_perf_end") });
    encoder.write_timestamp(queries, index);
    current.records.push(Record { mark: FRAME_END, view: None, index });
    let n = index + 1;
    encoder.resolve_query_set(queries, 0..n, resolve, 0);
    encoder.copy_buffer_to_buffer(resolve, 0, &slots[slot].read, 0, u64::from(n) * 8);
    pending.push([encoder.finish()]);
    slots[slot].pending = Some((current.frame, std::mem::take(&mut current.records)));
    current.map = Some(slot);
    clock.add_own(own);
}

fn map_marks(marks: Option<ResMut<GpuMarks>>, clock: Res<RenderClock>) {
    let own = Instant::now();
    let Some(mut marks) = marks else { return };
    let GpuMarks { slots, current, .. } = &mut *marks;
    let Some(slot) = current.get_mut().unwrap_or_else(|e| e.into_inner()).map.take() else { return };
    let mapped = slots[slot].mapped.clone();
    slots[slot].read.slice(..).map_async(MapMode::Read, move |r| mapped.store(r.is_ok(), Ordering::Release));
    clock.add_own(own);
}

fn gpu_frame(records: &[Record], ticks: &[u64], period: f32) -> GpuFrame {
    let at = |r: &Record| ticks.get(r.index as usize).copied().unwrap_or(0);
    let ms = |a: u64, b: u64| b.saturating_sub(a) as f32 * period * 1e-6;
    let find = |mark: u8, view: Option<Entity>| records.iter().find(|r| r.mark == mark && r.view == view).map(at);
    let mut out = GpuFrame::default();
    if let (Some(a), Some(b)) = (find(FRAME_BEGIN, None), find(FRAME_END, None)) {
        out.frame = ms(a, b);
    }
    let mut views: Vec<Entity> = records.iter().filter_map(|r| r.view).collect();
    views.dedup();
    for v in views {
        let t = |m| find(m, Some(v));
        let seg = |a: u8, b: u8| t(a).zip(t(b)).map_or(0.0, |(a, b)| ms(a, b));
        out.views += seg(VIEW_BEGIN, VIEW_END);
        out.cull += seg(VIEW_BEGIN, SHADOW_BEGIN) + seg(SHADOW_END, MAIN_BEGIN);
        out.shadow += seg(SHADOW_BEGIN, SHADOW_END);
        out.main += seg(MAIN_BEGIN, MAIN_END);
        out.post += seg(MAIN_END, VIEW_END);
    }
    out
}
