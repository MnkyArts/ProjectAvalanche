use bevy::core_pipeline::core_3d::{Transparent3d, main_transparent_pass_3d};
use bevy::core_pipeline::prepass::ViewPrepassTextures;
use bevy::core_pipeline::tonemapping::tonemapping;
use bevy::core_pipeline::{Core3d, Core3dSystems, FullscreenShader};
use bevy::image::ImageSamplerDescriptor;
use bevy::platform::collections::HashMap;
use bevy::prelude::*;
use bevy::render::extract_component::ExtractComponentPlugin;
use bevy::render::extract_resource::ExtractResourcePlugin;
use bevy::render::render_asset::RenderAssets;
use bevy::render::render_phase::{SortedRenderPhase, ViewSortedRenderPhases};
use bevy::render::render_resource::binding_types::{
    sampler, storage_buffer_read_only_sized, storage_buffer_sized, texture_2d, texture_3d, texture_depth_2d,
    uniform_buffer,
};
use bevy::render::render_resource::*;
use bevy::render::renderer::{RenderContext, RenderDevice, RenderQueue, ViewQuery};
use bevy::render::storage::GpuShaderBuffer;
use bevy::render::texture::GpuImage;
use bevy::render::view::{ExtractedView, RetainedViewEntity, ViewDepthStencilTexture, ViewTarget};
use bevy::render::{Render, RenderApp, RenderStartup, RenderSystems};
use bevy::shader::Shader;

use super::adapt::{AdaptMode, HdrAdaptation};
use super::fsr::ScaledScene;
use super::overlay::{OverlayUniform, PostOverlay};
use super::topogrid::{PostTopogrid, TopogridUniform};
use super::{GamePostCamera, PostLevels, PostSettings, PostUniform};
use crate::render::bind_cache::BindGroupCache;
use crate::render::buffers::QueuedUniform;
use crate::render::objects::material::{
    LDR_PIPELINE_LABEL, LDR_POST_PARTICLES_LABEL, LDR_TONEMAP_BUFFER, LDR_TONEMAP_EXPOSURE_OFFSET,
};

const SHADER_DIR: &str = "embedded://ssx_bevy/render/post/";
const QUARTER_FORMAT: TextureFormat = TextureFormat::Rgba16Float;
const BLOOM_FORMAT: TextureFormat = TextureFormat::Rgba8Unorm;
const HDR_FORMAT: TextureFormat = TextureFormat::Rgba16Float;
const FRAME_FORMAT: TextureFormat = TextureFormat::Rgba8Unorm;
const EXPOSURE_BYTES: u64 = 16;

pub(super) fn build(app: &mut App) {
    app.add_plugins((
        ExtractResourcePlugin::<PostSettings>::default(),
        ExtractResourcePlugin::<PostLevels>::default(),
        ExtractComponentPlugin::<GamePostCamera>::default(),
    ));
    let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
        return;
    };
    render_app
        .init_resource::<PostTargets>()
        .init_resource::<HdrAdaptation>()
        .init_resource::<LdrTranslucents>()
        .init_resource::<PostHandoff>()
        .add_systems(RenderStartup, init_pipelines)
        .add_systems(Render, prepare_targets.in_set(RenderSystems::PrepareResources))
        .add_systems(
            Core3d,
            (
                take_ldr_translucents.in_set(Core3dSystems::MainPass).before(main_transparent_pass_3d),
                post_chain.in_set(Core3dSystems::PostProcess).before(tonemapping),
                post_rest.in_set(Core3dSystems::PostProcess).after(post_chain).before(tonemapping),
                restore_ldr_translucents.in_set(Core3dSystems::PostProcess).after(post_rest),
            ),
        );
}

#[derive(Resource)]
struct PostPipelines {
    sampler: Sampler,
    overlay_sampler: Sampler,
    topogrid_sampler: Sampler,
    blank_depth: TextureView,
    blank_normal: TextureView,
    fxaa_sampler: Sampler,
    downsample_layout: BindGroupLayoutDescriptor,
    fxaa_layout: BindGroupLayoutDescriptor,
    adapt_layout: BindGroupLayoutDescriptor,
    bloom_layout: BindGroupLayoutDescriptor,
    final_layout: BindGroupLayoutDescriptor,
    downsample: CachedRenderPipelineId,
    threshold: CachedRenderPipelineId,
    blur_h: CachedRenderPipelineId,
    blur_v: CachedRenderPipelineId,
    tonemap: CachedRenderPipelineId,
    fxaa: CachedRenderPipelineId,
    composite: CachedRenderPipelineId,
    composite_codes: CachedRenderPipelineId,
    adapt: CachedComputePipelineId,
    reduce_levels: [CachedComputePipelineId; 2],
    reduce: CachedComputePipelineId,
}

fn init_pipelines(
    mut commands: Commands,
    device: Res<RenderDevice>,
    assets: Res<AssetServer>,
    fullscreen: Res<FullscreenShader>,
    cache: Res<PipelineCache>,
) {
    let shader = |name: &str| assets.load::<Shader>(format!("{SHADER_DIR}{name}.wesl"));
    let float = TextureSampleType::Float { filterable: true };
    let frag = ShaderStages::FRAGMENT;
    let downsample_layout = BindGroupLayoutDescriptor::new(
        "ssx_post_downsample",
        &BindGroupLayoutEntries::sequential(frag, (uniform_buffer::<PostUniform>(false), texture_2d(float))),
    );
    let adapt_layout = BindGroupLayoutDescriptor::new(
        "ssx_post_adapt",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::COMPUTE,
            (
                uniform_buffer::<PostUniform>(false),
                texture_2d(float),
                storage_buffer_sized(false, None),
                storage_buffer_sized(false, None),
            ),
        ),
    );
    let bloom_layout = BindGroupLayoutDescriptor::new(
        "ssx_post_bloom",
        &BindGroupLayoutEntries::sequential(
            frag,
            (
                uniform_buffer::<PostUniform>(false),
                texture_2d(float),
                sampler(SamplerBindingType::Filtering),
                storage_buffer_read_only_sized(false, None),
            ),
        ),
    );
    let final_layout = BindGroupLayoutDescriptor::new(
        "ssx_post_final",
        &BindGroupLayoutEntries::sequential(
            frag,
            (
                uniform_buffer::<PostUniform>(false),
                texture_2d(float),
                texture_2d(float),
                sampler(SamplerBindingType::Filtering),
                texture_3d(float),
                storage_buffer_read_only_sized(false, None),
                uniform_buffer::<OverlayUniform>(false),
                texture_2d(float),
                texture_2d(float),
                sampler(SamplerBindingType::Filtering),
                uniform_buffer::<TopogridUniform>(false),
                texture_2d(float),
                sampler(SamplerBindingType::Filtering),
                texture_depth_2d(),
                texture_2d(float),
            ),
        ),
    );
    let fxaa_layout = BindGroupLayoutDescriptor::new(
        "ssx_post_fxaa",
        &BindGroupLayoutEntries::sequential(
            frag,
            (uniform_buffer::<PostUniform>(false), texture_2d(float), sampler(SamplerBindingType::Filtering)),
        ),
    );
    let render = |label: &'static str, layout: &BindGroupLayoutDescriptor, file: &str, entry: &'static str, format| {
        cache.queue_render_pipeline(RenderPipelineDescriptor {
            label: Some(label.into()),
            layout: vec![layout.clone()],
            vertex: fullscreen.to_vertex_state(),
            fragment: Some(FragmentState {
                shader: shader(file),
                entry_point: Some(entry.into()),
                targets: vec![Some(ColorTargetState { format, blend: None, write_mask: ColorWrites::ALL })],
                ..default()
            }),
            ..default()
        })
    };
    let compute = |label: &'static str, entry: &'static str| {
        cache.queue_compute_pipeline(ComputePipelineDescriptor {
            label: Some(label.into()),
            layout: vec![adapt_layout.clone()],
            shader: shader("post_adapt"),
            entry_point: Some(entry.into()),
            zero_initialize_workgroup_memory: true,
            ..default()
        })
    };
    let pipelines = PostPipelines {
        sampler: device.create_sampler(&post_sampler()),
        overlay_sampler: device.create_sampler(&overlay_sampler()),
        topogrid_sampler: device.create_sampler(&topogrid_sampler()),
        blank_depth: blank_view(&device, "ssx_post_blank_depth", TextureFormat::Depth32Float),
        blank_normal: blank_view(&device, "ssx_post_blank_normal", TextureFormat::Rgb10a2Unorm),
        fxaa_sampler: device.create_sampler(&fxaa_sampler()),
        downsample: render("ssx_post_downsample", &downsample_layout, "post_downsample", "downsample", QUARTER_FORMAT),
        threshold: render("ssx_post_threshold", &bloom_layout, "post_bloom", "threshold", BLOOM_FORMAT),
        blur_h: render("ssx_post_blur_h", &bloom_layout, "post_bloom", "blur_h", BLOOM_FORMAT),
        blur_v: render("ssx_post_blur_v", &bloom_layout, "post_bloom", "blur_v", BLOOM_FORMAT),
        tonemap: render("ssx_post_tonemap", &final_layout, "post_final", "tonemap_pass", FRAME_FORMAT),
        fxaa: render("ssx_post_fxaa", &fxaa_layout, "post_fxaa", "fxaa_pass", FRAME_FORMAT),
        composite: render("ssx_post_composite", &final_layout, "post_final", "composite_pass", HDR_FORMAT),
        composite_codes: render(
            "ssx_post_composite_codes",
            &final_layout,
            "post_final",
            "composite_codes_pass",
            HDR_FORMAT,
        ),
        adapt: compute("ssx_post_adapt", "adapt"),
        reduce_levels: [
            compute("ssx_post_reduce_level1", "reduce_level1"),
            compute("ssx_post_reduce_level2", "reduce_level2"),
        ],
        reduce: compute("ssx_post_reduce", "reduce"),
        downsample_layout,
        adapt_layout,
        bloom_layout,
        final_layout,
        fxaa_layout,
    };
    commands.insert_resource(pipelines);
}

struct ViewTargets {
    size: UVec2,
    quarter: TextureView,
    bloom_a: TextureView,
    bloom_b: TextureView,
    frame: TextureView,
    frame_aa: TextureView,
    adapted: Buffer,
    exposure: Buffer,
    uniform: QueuedUniform<PostUniform>,
    overlay: QueuedUniform<OverlayUniform>,
    topogrid: QueuedUniform<TopogridUniform>,
    fresh: bool,
    complete: bool,
}

#[derive(Resource, Default)]
struct PostTargets(HashMap<Entity, ViewTargets>);

fn quarter_size(size: UVec2) -> UVec2 {
    (size + UVec2::splat(3)) / 4
}

fn create_targets(device: &RenderDevice, size: UVec2) -> ViewTargets {
    let q = quarter_size(size);
    let sized = |label: &'static str, format: TextureFormat, q: UVec2| {
        device
            .create_texture(&TextureDescriptor {
                label: Some(label),
                size: Extent3d { width: q.x, height: q.y, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: TextureDimension::D2,
                format,
                usage: TextureUsages::TEXTURE_BINDING | TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            })
            .create_view(&TextureViewDescriptor::default())
    };
    let texture = |label: &'static str, format: TextureFormat| sized(label, format, q);
    let buffer = |label: &'static str, size: u64| {
        device.create_buffer(&BufferDescriptor {
            label: Some(label),
            size,
            usage: BufferUsages::STORAGE | BufferUsages::COPY_SRC | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    };
    ViewTargets {
        size,
        quarter: texture("ssx_post_quarter", QUARTER_FORMAT),
        bloom_a: texture("ssx_post_bloom_a", BLOOM_FORMAT),
        bloom_b: texture("ssx_post_bloom_b", BLOOM_FORMAT),
        frame: sized("ssx_post_frame", FRAME_FORMAT, size),
        frame_aa: sized("ssx_post_frame_aa", FRAME_FORMAT, size),
        adapted: buffer("ssx_post_adapted", (u64::from(q.x * q.y) * 2 + 64) * 4),
        exposure: buffer("ssx_post_exposure", EXPOSURE_BYTES),
        uniform: QueuedUniform::default(),
        overlay: QueuedUniform::default(),
        topogrid: QueuedUniform::default(),
        fresh: true,
        complete: false,
    }
}

#[allow(clippy::too_many_arguments)]
fn prepare_targets(
    views: Query<(Entity, &ViewTarget, &ExtractedView), With<GamePostCamera>>,
    settings: Option<Res<PostSettings>>,
    overlay: Option<Res<PostOverlay>>,
    topogrid: Option<Res<PostTopogrid>>,
    device: Res<RenderDevice>,
    queue: Res<RenderQueue>,
    cache: Res<PipelineCache>,
    shader_buffers: Res<RenderAssets<GpuShaderBuffer>>,
    mut targets: ResMut<PostTargets>,
    mut adapt: ResMut<HdrAdaptation>,
    levels: Option<Res<PostLevels>>,
) {
    let Some(settings) = settings else {
        return;
    };
    let levels = levels.as_deref().copied().unwrap_or_default();
    if let Some(ldr) = shader_buffers.get(&LDR_TONEMAP_BUFFER) {
        let u = &settings.uniform;
        let words: Vec<f32> =
            [u.hable_a.to_array(), u.hable_b.to_array()].concat().into_iter().chain([u.exposure.x]).collect();
        let bytes: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
        queue.write_buffer(&ldr.buffer, 0, &bytes);
    }
    let loading = cache.waiting_pipelines().next().is_some() || !settings.scene_ready;
    targets.0.retain(|e, _| views.contains(*e));
    for (entity, view, extracted) in &views {
        let s = view.main_texture().size();
        let size = UVec2::new(s.width, s.height);
        if targets.0.get(&entity).is_none_or(|t| t.size != size) {
            targets.0.insert(entity, create_targets(&device, size));
        }
        let Some(t) = targets.0.get_mut(&entity) else { continue };
        let q = quarter_size(size);
        let mut u = settings.uniform;
        u.sizes = UVec4::new(size.x, size.y, q.x, q.y);
        if !levels.bloom_drawn() {
            u.bloom.z = 0.0;
        }
        if !levels.post {
            u.grade.x = 0.0;
        }
        let base = adapt.begin(&device, &queue, q, &t.adapted, t.fresh);
        // TODO(guess): the port's view counts as "sky not ready" until it first draws a complete scene (no pipeline compiling, meshes spawned): the adaptation runs toward black (L = 1) as main.scrape's `ifnot skyrenderobj ready` draws black, later it adapts every frame (until 2026-10-21 the port cleared it on every frame with a pipeline compiling: a bright second mid-ride); settle: the sky render object's ready test and how long a level's loading frames run the scrape.
        let starting = loading && !t.complete;
        t.complete |= !loading;
        let mode = if settings.converge && (t.fresh || starting || u.exposure.w == 0.0) {
            AdaptMode::Current
        } else if starting && base == AdaptMode::Adapt {
            AdaptMode::Black
        } else {
            base
        };
        adapt.ran();
        u.bloom.w = mode.code();
        t.fresh = false;
        t.uniform.write(&device, &queue, "ssx_post_uniform", &u);
        let o = overlay.as_ref().map_or_else(OverlayUniform::default, |o| o.uniform);
        t.overlay.write(&device, &queue, "ssx_post_overlay", &o);
        let mut topo = topogrid.as_ref().map_or_else(TopogridUniform::default, |g| g.uniform);
        topo.world_from_clip = extracted.world_from_view.to_matrix() * extracted.clip_from_view.inverse();
        topo.back = extracted.world_from_view.back().as_vec3().extend(0.0);
        t.topogrid.write(&device, &queue, "ssx_post_topogrid", &topo);
    }
}

fn blank_view(device: &RenderDevice, label: &'static str, format: TextureFormat) -> TextureView {
    device
        .create_texture(&TextureDescriptor {
            label: Some(label),
            size: Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: TextureDimension::D2,
            format,
            usage: TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        })
        .create_view(&TextureViewDescriptor::default())
}

pub(super) fn fullscreen_pass(
    ctx: &mut RenderContext,
    label: &str,
    target: &TextureView,
    pipeline: &RenderPipeline,
    bind: &BindGroup,
) {
    let mut pass = ctx.command_encoder().begin_render_pass(&RenderPassDescriptor {
        label: Some(label),
        color_attachments: &[Some(RenderPassColorAttachment {
            view: target,
            depth_slice: None,
            resolve_target: None,
            ops: Operations { load: LoadOp::Clear(default()), store: StoreOp::Store },
        })],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    });
    pass.set_pipeline(pipeline);
    pass.set_bind_group(0, bind, &[]);
    pass.draw(0..3, 0..1);
}

type PostView =
    (&'static ViewTarget, &'static ViewDepthStencilTexture, Option<&'static ViewPrepassTextures>, Has<ScaledScene>);

#[allow(clippy::too_many_arguments)]
fn post_chain(
    world: &World,
    view: ViewQuery<PostView, With<GamePostCamera>>,
    pipelines: Option<Res<PostPipelines>>,
    targets: Res<PostTargets>,
    settings: Option<Res<PostSettings>>,
    overlay: Option<Res<PostOverlay>>,
    topogrid: Option<Res<PostTopogrid>>,
    images: Res<RenderAssets<GpuImage>>,
    shader_buffers: Res<RenderAssets<GpuShaderBuffer>>,
    ldr: Res<LdrTranslucents>,
    cache: Res<PipelineCache>,
    session: Res<HdrAdaptation>,
    levels: Option<Res<PostLevels>>,
    handoff: Res<PostHandoff>,
    mut binds: Local<BindGroupCache>,
    mut ctx: RenderContext,
) {
    handoff.put(None);
    let (Some(p), Some(settings)) = (pipelines, settings) else {
        return;
    };
    let levels = levels.as_deref().copied().unwrap_or_default();
    let bloom_on = levels.bloom_drawn() && settings.uniform.bloom.z != 0.0;
    let view_entity = view.entity();
    let Some(t) = targets.0.get(&view_entity) else {
        return;
    };
    let (target, depth, prepass, scaled) = view.into_inner();
    if target.main_texture_format() != HDR_FORMAT {
        return;
    }
    let render = |id| cache.get_render_pipeline(id);
    let (Some(downsample), Some(threshold), Some(blur_h), Some(blur_v)) =
        (render(p.downsample), render(p.threshold), render(p.blur_h), render(p.blur_v))
    else {
        return;
    };
    let (Some(tonemap), Some(fxaa), Some(composite)) = (render(p.tonemap), render(p.fxaa), render(p.composite)) else {
        return;
    };
    let composite = if scaled {
        let Some(codes) = render(p.composite_codes) else { return };
        codes
    } else {
        composite
    };
    let compute = |id| cache.get_compute_pipeline(id);
    let (Some(adapt), Some(level1), Some(level2), Some(reduce)) =
        (compute(p.adapt), compute(p.reduce_levels[0]), compute(p.reduce_levels[1]), compute(p.reduce))
    else {
        return;
    };
    let Some(lut) = images.get(&settings.lut) else {
        return;
    };
    let Some(maps) = overlay.and_then(|o| Some([images.get(&o.maps[0])?, images.get(&o.maps[1])?])) else {
        return;
    };
    let Some(grid) = topogrid.and_then(|g| images.get(&g.grid)) else {
        return;
    };
    let gbuffer_depth = prepass.and_then(ViewPrepassTextures::depth_only_view).unwrap_or(&p.blank_depth);
    let gbuffer_normal = prepass.and_then(ViewPrepassTextures::normal_view).unwrap_or(&p.blank_normal);
    let device = ctx.render_device().clone();
    let layout = |l: &BindGroupLayoutDescriptor| cache.get_bind_group_layout(l);
    let post = target.post_process_write();
    // PORT(choice): the chain's bind groups (its own and the particle passes', about fifty a frame on Mount Eddie) are
    // kept across frames and made again only when what they bind changes (`BindGroupCache`: the same layout and the
    // same buffers, views and samplers give the kept group), instead of made and freed every frame: the same resources
    // bound, so the same passes. wgpu's bind groups are the port's renderer; the console's GPU had none.
    let binds = &mut *binds;
    binds.next_frame();
    let mut bind = |label: &'static str, l: &BindGroupLayoutDescriptor, src: &TextureView| {
        binds.get(
            &device,
            label,
            &layout(l),
            &BindGroupEntries::sequential((&t.uniform, src, &p.sampler, t.exposure.as_entire_binding())),
        )
    };
    let bloom_binds = [
        bind("ssx_post_threshold", &p.bloom_layout, &t.quarter),
        bind("ssx_post_blur_h", &p.bloom_layout, &t.bloom_a),
        bind("ssx_post_blur_v", &p.bloom_layout, &t.bloom_b),
    ];

    let down_bg = binds.get(
        &device,
        "ssx_post_downsample",
        &layout(&p.downsample_layout),
        &BindGroupEntries::sequential((&t.uniform, post.source)),
    );
    fullscreen_pass(&mut ctx, "ssx_post_downsample", &t.quarter, downsample, &down_bg);

    let adapt_bg = binds.get(
        &device,
        "ssx_post_adapt",
        &layout(&p.adapt_layout),
        &BindGroupEntries::sequential((
            &t.uniform,
            &t.quarter,
            t.adapted.as_entire_binding(),
            t.exposure.as_entire_binding(),
        )),
    );
    {
        let q = quarter_size(t.size);
        let mut pass = ctx
            .command_encoder()
            .begin_compute_pass(&ComputePassDescriptor { label: Some("ssx_post_exposure"), timestamp_writes: None });
        pass.set_bind_group(0, &adapt_bg, &[]);
        pass.set_pipeline(adapt);
        pass.dispatch_workgroups(q.x.div_ceil(8), q.y.div_ceil(8), 1);
        let mut level = q;
        for pipeline in [level1, level2] {
            level = (level / 2).max(UVec2::ONE);
            pass.set_pipeline(pipeline);
            pass.dispatch_workgroups((level.x * level.y).div_ceil(256), 1, 1);
        }
        pass.set_pipeline(reduce);
        pass.dispatch_workgroups(1, 1, 1);
    }
    session.store(ctx.command_encoder(), quarter_size(t.size), &t.adapted);

    if bloom_on {
        fullscreen_pass(&mut ctx, "ssx_post_threshold", &t.bloom_a, threshold, &bloom_binds[0]);
        fullscreen_pass(&mut ctx, "ssx_post_blur_h", &t.bloom_b, blur_h, &bloom_binds[1]);
        fullscreen_pass(&mut ctx, "ssx_post_blur_v", &t.bloom_a, blur_v, &bloom_binds[2]);
    }

    let mut final_bg = |label: &'static str, src: &TextureView| {
        binds.get(
            &device,
            label,
            &layout(&p.final_layout),
            &BindGroupEntries::sequential((
                &t.uniform,
                src,
                &t.bloom_a,
                &p.sampler,
                &lut.texture_view,
                t.exposure.as_entire_binding(),
                &t.overlay,
                &maps[0].texture_view,
                &maps[1].texture_view,
                &p.overlay_sampler,
                &t.topogrid,
                &grid.texture_view,
                &p.topogrid_sampler,
                gbuffer_depth,
                gbuffer_normal,
            )),
        )
    };
    let tonemap_bg = final_bg("ssx_post_tonemap", post.source);
    let composite_bg = final_bg("ssx_post_composite", if levels.fxaa_drawn() { &t.frame_aa } else { &t.frame });
    fullscreen_pass(&mut ctx, "ssx_post_tonemap", &t.frame, tonemap, &tonemap_bg);
    if let Some(tonemap_buffer) = shader_buffers.get(&LDR_TONEMAP_BUFFER)
        && !ldr.pre.items.is_empty()
    {
        let tonemap = &tonemap_buffer.buffer;
        ctx.command_encoder().copy_buffer_to_buffer(&t.exposure, 8, tonemap, LDR_TONEMAP_EXPOSURE_OFFSET, 4);
        draw_ldr_translucents(world, &mut ctx, &ldr.pre, LDR_PIPELINE_LABEL, view_entity, depth, &t.frame);
    }
    let fxaa = levels.fxaa_drawn().then(|| {
        let bind = binds.get(
            &device,
            "ssx_post_fxaa",
            &layout(&p.fxaa_layout),
            &BindGroupEntries::sequential((&t.uniform, &t.frame, &p.fxaa_sampler)),
        );
        (fxaa.clone(), bind)
    });
    handoff.put(Some(PostRest {
        view: view_entity,
        bloom: bloom_on,
        fxaa,
        composite: (composite.clone(), composite_bg),
        destination: post.destination.clone(),
    }));
}

struct PostRest {
    view: Entity,
    bloom: bool,
    fxaa: Option<(RenderPipeline, BindGroup)>,
    composite: (RenderPipeline, BindGroup),
    destination: TextureView,
}

#[derive(Resource, Default)]
struct PostHandoff(std::sync::Mutex<Option<PostRest>>);

impl PostHandoff {
    fn put(&self, rest: Option<PostRest>) {
        *self.0.lock().unwrap_or_else(|e| e.into_inner()) = rest;
    }

    fn take(&self) -> Option<PostRest> {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).take()
    }
}

// PORT(choice): the post chain records into two encoders, its second half (RenderParticleFX's passes,
// TranslucentPass_PostParticles, FXAA, the composite) in a system of its own right after the first, so Bevy finishes
// the two in parallel (`finish_command_buffers`) and submits them in the systems' order. In one encoder the post chain
// was the frame's last to finish (wgpu 30 records the real commands at `finish`) in 79 % of Mount Eddie's frames
// (alps0101: 86 %), 1.05 ms of the 1.23 ms the render thread waited for the finishes (traced). The same passes in the
// same order; wgpu places the transitions between two command buffers of a submission as it does within one.
// Command encoders are the port's renderer; the console's post chain was one command stream.
fn post_rest(
    world: &World,
    view: ViewQuery<PostView, With<GamePostCamera>>,
    targets: Res<PostTargets>,
    ldr: Res<LdrTranslucents>,
    handoff: Res<PostHandoff>,
    mut binds: Local<BindGroupCache>,
    mut ctx: RenderContext,
) {
    let view_entity = view.entity();
    let Some(rest) = handoff.take().filter(|r| r.view == view_entity) else { return };
    let Some(t) = targets.0.get(&view_entity) else { return };
    let (_, depth, _, _) = view.into_inner();
    let binds = &mut *binds;
    binds.next_frame();
    if let Some(post_uniform) = t.uniform.binding() {
        let bloom = rest.bloom.then_some(&t.bloom_a);
        crate::particles::render::draw(world, &mut ctx, depth, &t.frame, bloom, post_uniform, &t.exposure, binds);
    }
    draw_ldr_translucents(world, &mut ctx, &ldr.post, LDR_POST_PARTICLES_LABEL, view_entity, depth, &t.frame);
    if let Some((fxaa, bind)) = &rest.fxaa {
        fullscreen_pass(&mut ctx, "ssx_post_fxaa", &t.frame_aa, fxaa, bind);
    }
    let (composite, bind) = &rest.composite;
    fullscreen_pass(&mut ctx, "ssx_post_composite", &rest.destination, composite, bind);
}

fn fxaa_sampler() -> SamplerDescriptor<'static> {
    match crate::render::fx_states::sampler("fxaa", "Default", "FrameBufferSampler").and_then(|s| s.descriptor()) {
        Ok(d) => SamplerDescriptor { label: Some("ssx_post_fxaa"), ..d.as_wgpu() },
        Err(e) => {
            warn!("fxaa.fxo sampler states unavailable ({e:#}); clamp + linear");
            SamplerDescriptor {
                label: Some("ssx_post_fxaa"),
                mag_filter: FilterMode::Linear,
                min_filter: FilterMode::Linear,
                ..default()
            }
        }
    }
}

fn overlay_sampler() -> SamplerDescriptor<'static> {
    let technique = "BASE_BLOOM_COLOURLOOKUP_OVERLAY";
    match crate::render::fx_states::sampler("postfx", technique, "overlay_map_sampler0").and_then(|s| s.descriptor()) {
        Ok(d) => SamplerDescriptor { label: Some("ssx_post_overlay"), ..d.as_wgpu() },
        Err(e) => {
            warn!("postfx.fxo overlay sampler states unavailable ({e:#}); clamp + linear");
            SamplerDescriptor {
                label: Some("ssx_post_overlay"),
                mag_filter: FilterMode::Linear,
                min_filter: FilterMode::Linear,
                lod_max_clamp: 0.0,
                ..default()
            }
        }
    }
}

fn topogrid_sampler() -> SamplerDescriptor<'static> {
    let technique = "BASE_BLOOM_COLOURLOOKUP_TOPOGRID";
    match crate::render::fx_states::sampler("postfx", technique, "topogrid_sampler").and_then(|s| s.descriptor()) {
        Ok(d) => SamplerDescriptor { label: Some("ssx_post_topogrid"), ..d.as_wgpu() },
        Err(e) => {
            warn!("postfx.fxo topogrid sampler states unavailable ({e:#}); wrap + linear");
            SamplerDescriptor {
                label: Some("ssx_post_topogrid"),
                address_mode_u: AddressMode::Repeat,
                address_mode_v: AddressMode::Repeat,
                mag_filter: FilterMode::Linear,
                min_filter: FilterMode::Linear,
                mipmap_filter: MipmapFilterMode::Linear,
                ..default()
            }
        }
    }
}

fn post_sampler() -> SamplerDescriptor<'static> {
    let technique = "BASE_BLOOM_COLOURLOOKUP";
    let states = ["bloom_glowmap_sampler", "colourLookupSampler1"]
        .map(|name| crate::render::fx_states::sampler("postfx", technique, name).and_then(|s| s.descriptor()));
    let fallback = SamplerDescriptor {
        label: Some("ssx_post_linear_clamp"),
        mag_filter: FilterMode::Linear,
        min_filter: FilterMode::Linear,
        lod_max_clamp: 0.0,
        ..default()
    };
    match states {
        [Ok(a), Ok(b)] if ImageSamplerDescriptor { address_mode_w: b.address_mode_w, ..a.clone() } == b => {
            SamplerDescriptor { label: Some("ssx_post_linear_clamp"), ..b.as_wgpu() }
        }
        [a, b] => {
            warn!("postfx.fxo sampler states unavailable or different ({:?} / {:?}); clamp + linear", a.err(), b.err());
            fallback
        }
    }
}

#[derive(Resource, Default)]
struct LdrTranslucents {
    view: Option<RetainedViewEntity>,
    pre: SortedRenderPhase<Transparent3d>,
    post: SortedRenderPhase<Transparent3d>,
}

fn ldr_pass(cache: &PipelineCache, id: CachedRenderPipelineId) -> Option<LdrPass> {
    if matches!(cache.get_render_pipeline_state(id), CachedPipelineState::Queued) {
        return None;
    }
    match cache.get_render_pipeline_descriptor(id).label.as_deref() {
        Some(LDR_PIPELINE_LABEL) => Some(LdrPass::PreParticles),
        Some(LDR_POST_PARTICLES_LABEL) => Some(LdrPass::PostParticles),
        _ => None,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LdrPass {
    PreParticles,
    PostParticles,
}

fn take_ldr_translucents(
    view: ViewQuery<&ExtractedView, With<GamePostCamera>>,
    mut phases: ResMut<ViewSortedRenderPhases<Transparent3d>>,
    cache: Res<PipelineCache>,
    mut ldr: ResMut<LdrTranslucents>,
) {
    let retained = view.into_inner().retained_view_entity;
    let Some(phase) = phases.get_mut(&retained) else { return };
    let keys: Vec<_> =
        phase.items.iter().filter_map(|(k, item)| ldr_pass(&cache, item.pipeline).map(|pass| (*k, pass))).collect();
    for (key, pass) in keys {
        if let Some(item) = phase.items.shift_remove(&key) {
            let target = match pass {
                LdrPass::PreParticles => &mut ldr.pre,
                LdrPass::PostParticles => &mut ldr.post,
            };
            target.items.insert(key, item);
        }
    }
    ldr.view = Some(retained);
}

fn restore_ldr_translucents(
    mut phases: ResMut<ViewSortedRenderPhases<Transparent3d>>,
    mut ldr: ResMut<LdrTranslucents>,
) {
    let Some(retained) = ldr.view.take() else { return };
    let pre = std::mem::take(&mut ldr.pre.items);
    let post = std::mem::take(&mut ldr.post.items);
    if let Some(phase) = phases.get_mut(&retained) {
        phase.items.extend(pre);
        phase.items.extend(post);
    }
}

fn draw_ldr_translucents(
    world: &World,
    ctx: &mut RenderContext,
    phase: &SortedRenderPhase<Transparent3d>,
    label: &'static str,
    view_entity: Entity,
    depth: &ViewDepthStencilTexture,
    frame: &TextureView,
) {
    if phase.items.is_empty() {
        return;
    }
    let mut pass = ctx.begin_tracked_render_pass(RenderPassDescriptor {
        label: Some(label),
        color_attachments: &[Some(RenderPassColorAttachment {
            view: frame,
            depth_slice: None,
            resolve_target: None,
            ops: Operations { load: LoadOp::Load, store: StoreOp::Store },
        })],
        depth_stencil_attachment: Some(depth.get_attachment(StoreOp::Store)),
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    });
    if let Err(e) = phase.render(&mut pass, world, view_entity) {
        error!("{label} pass: {e:?}");
    }
}
