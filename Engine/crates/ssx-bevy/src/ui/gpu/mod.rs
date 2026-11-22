mod pipelines;

use std::collections::HashMap;

use bevy::core_pipeline::tonemapping::tonemapping;
use bevy::core_pipeline::upscaling::upscaling;
use bevy::core_pipeline::{Core2d, Core2dSystems, Core3d, Core3dSystems};
use bevy::prelude::*;
use bevy::render::render_phase::TrackedRenderPass;
use bevy::render::render_resource::*;
use bevy::render::renderer::{RenderContext, RenderDevice, RenderQueue, ViewQuery};
use bevy::render::view::ViewTarget;
use bevy::render::{Render, RenderApp, RenderStartup, RenderSystems};
use bevy::shader::Shader;

use super::{DRAW_FLOATS, GRADIENT_ROWS, UiBlend, UiCamera, UiFrame, UiOp, UiTexture};
use crate::render::bind_cache::BindGroupCache;
use pipelines::{UiPipelines, init_pipelines, result_out_pipeline};

const SAMPLES: u32 = 1;
const UI_FORMAT: TextureFormat = TextureFormat::Rgba8Unorm;
const STENCIL_FORMAT: TextureFormat = TextureFormat::Depth24PlusStencil8;

#[derive(Resource)]
struct UiShaders {
    fill: Handle<Shader>,
    post: Handle<Shader>,
}

pub(super) fn build(app: &mut App) {
    let mut shaders = app.world_mut().resource_mut::<Assets<Shader>>();
    let fill = shaders.add(Shader::from_wgsl(include_str!("ui_fill.wgsl"), "ui/gpu/ui_fill.wgsl"));
    let post = shaders.add(Shader::from_wgsl(include_str!("ui_post.wgsl"), "ui/gpu/ui_post.wgsl"));
    let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
        return;
    };
    render_app
        .insert_resource(UiShaders { fill, post })
        .init_resource::<UiGpu>()
        .add_systems(RenderStartup, init_pipelines)
        .add_systems(Render, prepare.in_set(RenderSystems::PrepareResources))
        .add_systems(Core2d, ui_pass.in_set(Core2dSystems::PostProcess).in_set(super::UiPassSystems).after(tonemapping))
        .add_systems(
            Core3d,
            ui_pass
                .in_set(super::UiPassSystems)
                .after(Core3dSystems::PostProcess)
                .after(bevy::ui_render::render_pass::ui_pass)
                .before(upscaling),
        );
}

struct Targets {
    pub(super) size: UVec2,
    color: TextureView,
    stencil: TextureView,
}

#[derive(Resource, Default)]
struct UiGpu {
    images: Vec<TextureView>,
    image_keys: Vec<usize>,
    image_groups: HashMap<(usize, bool), BindGroup>,
    gradients: Option<(TextureView, BindGroup)>,
    white: Option<(TextureView, BindGroup)>,
    glyphs: Option<(u64, UVec2, TextureView, BindGroup)>,
    gradient_rows: Vec<[u8; 4]>,
    frame: Option<Buffer>,
    draws_group: Option<BindGroup>,
    regions: FrameRegions,
    staged: Vec<u8>,
    written: Vec<u8>,
    targets: Option<Targets>,
}

// PORT(choice): the frame's draw records, vertices, paint coordinates, draw ids and indices go to the GPU in one
// buffer, written with one `write_buffer` (one staging copy of wgpu's instead of five) only when its bytes differ from
// the last frame's (a still screen writes nothing), from two byte vectors kept from frame to frame instead of five new
// ones a frame; the draw records' storage binding is the whole buffer from its start, the vertex and index buffers its
// slices. The gradient rows are written when they differ from the rows the texture holds. The bytes and the draws are
// the same, so the picture is. Bevy's render world and wgpu's queue are the port's engine; the console's UI renderer
// had neither.
#[derive(Clone, Default)]
struct FrameRegions {
    vertices: std::ops::Range<u64>,
    paint_coords: std::ops::Range<u64>,
    draw_ids: std::ops::Range<u64>,
    indices: std::ops::Range<u64>,
}

impl FrameRegions {
    fn drawn(&self) -> bool {
        [&self.vertices, &self.paint_coords, &self.draw_ids, &self.indices].iter().all(|r| !r.is_empty())
    }
}

fn stage(out: &mut Vec<u8>, frame: &UiFrame) -> FrameRegions {
    fn put(out: &mut [u8], at: &mut usize, words: impl ExactSizeIterator<Item = [u8; 4]>) -> std::ops::Range<u64> {
        let start = *at;
        *at += words.len() * 4;
        for (dst, w) in out[start..*at].as_chunks_mut::<4>().0.iter_mut().zip(words) {
            *dst = w;
        }
        start as u64..*at as u64
    }
    let words = frame.draws.len() * DRAW_FLOATS
        + (frame.vertices.len() + frame.paint_coords.len()) * 2
        + frame.draw_ids.len()
        + frame.indices.len();
    out.clear();
    out.resize(words * 4, 0);
    let at = &mut 0;
    put(out, at, frame.draws.as_flattened().iter().map(|v| v.to_le_bytes()));
    FrameRegions {
        vertices: put(out, at, frame.vertices.as_flattened().iter().map(|v| v.to_le_bytes())),
        paint_coords: put(out, at, frame.paint_coords.as_flattened().iter().map(|v| v.to_le_bytes())),
        draw_ids: put(out, at, frame.draw_ids.iter().map(|v| v.to_le_bytes())),
        indices: put(out, at, frame.indices.iter().map(|v| v.to_le_bytes())),
    }
}

fn texture_2d_desc(
    label: &'static str,
    size: UVec2,
    format: TextureFormat,
    samples: u32,
    usage: TextureUsages,
) -> TextureDescriptor<'static> {
    TextureDescriptor {
        label: Some(label),
        size: Extent3d { width: size.x.max(1), height: size.y.max(1), depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: samples,
        dimension: TextureDimension::D2,
        format,
        usage,
        view_formats: &[],
    }
}

fn upload_rgba(
    device: &RenderDevice,
    queue: &RenderQueue,
    label: &'static str,
    size: UVec2,
    rgba: &[u8],
) -> TextureView {
    let desc = texture_2d_desc(label, size, UI_FORMAT, 1, TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST);
    device
        .create_texture_with_data(queue, &desc, TextureDataOrder::LayerMajor, rgba)
        .create_view(&TextureViewDescriptor::default())
}

fn write_frame(device: &RenderDevice, queue: &RenderQueue, layout: &BindGroupLayout, gpu: &mut UiGpu) {
    let size = (gpu.staged.len().max(16) as u64).next_power_of_two();
    let grown = gpu.frame.as_ref().is_none_or(|b| b.size() < size);
    if grown {
        let usage = BufferUsages::STORAGE | BufferUsages::VERTEX | BufferUsages::INDEX | BufferUsages::COPY_DST;
        let buffer = device.create_buffer(&BufferDescriptor {
            label: Some("ssx_ui_frame"),
            size,
            usage,
            mapped_at_creation: false,
        });
        gpu.draws_group = Some(device.create_bind_group(
            "ssx_ui_draws",
            layout,
            &BindGroupEntries::single(buffer.as_entire_binding()),
        ));
        gpu.frame = Some(buffer);
    } else if gpu.staged == gpu.written {
        return;
    }
    if let Some(b) = &gpu.frame
        && !gpu.staged.is_empty()
    {
        queue.write_buffer(b, 0, &gpu.staged);
    }
    std::mem::swap(&mut gpu.staged, &mut gpu.written);
}

#[allow(clippy::too_many_arguments)]
fn prepare(
    frame: Option<Res<UiFrame>>,
    views: Query<&ViewTarget, With<UiCamera>>,
    device: Res<RenderDevice>,
    queue: Res<RenderQueue>,
    cache: Res<PipelineCache>,
    pipelines: Option<ResMut<UiPipelines>>,
    mut gpu: ResMut<UiGpu>,
) {
    let (Some(frame), Some(mut p)) = (frame, pipelines) else { return };
    let gpu = &mut *gpu;
    for view in &views {
        let format = view.main_texture_format();
        if !p.result_out.contains_key(&format) {
            let id = result_out_pipeline(&cache, &p.full_layout, &p.post_shader, format);
            p.result_out.insert(format, id);
        }
    }
    let paint_layout = cache.get_bind_group_layout(&p.paint_layout);
    let paint_group = |view: &TextureView, sampler: &Sampler| {
        device.create_bind_group("ssx_ui_paint", &paint_layout, &BindGroupEntries::sequential((view, sampler)))
    };
    if gpu.white.is_none() {
        let v = upload_rgba(&device, &queue, "ssx_ui_white", UVec2::ONE, &[255; 4]);
        let g = paint_group(&v, &p.linear);
        gpu.white = Some((v, g));
    }
    for (i, img) in frame.images.iter().enumerate() {
        let key = std::sync::Arc::as_ptr(img) as usize;
        if gpu.image_keys.get(i) == Some(&key) {
            continue;
        }
        let view = upload_rgba(&device, &queue, "ssx_ui_image", UVec2::new(img.width, img.height), &img.rgba);
        if i < gpu.images.len() {
            (gpu.images[i], gpu.image_keys[i]) = (view, key);
            gpu.image_groups.retain(|(j, _), _| *j != i);
        } else {
            gpu.images.push(view);
            gpu.image_keys.push(key);
        }
    }
    let rows = UVec2::new(256, GRADIENT_ROWS as u32);
    if gpu.gradients.is_none() {
        let v = upload_rgba(&device, &queue, "ssx_ui_gradients", rows, &vec![0; (rows.x * rows.y * 4) as usize]);
        let g = paint_group(&v, &p.linear);
        gpu.gradients = Some((v, g));
    }
    let used = frame.gradient_rows_used.min(rows.y);
    let texels = &frame.gradients[..(used * 256) as usize];
    if used > 0
        && gpu.gradient_rows.get(..texels.len()) != Some(texels)
        && let Some((v, _)) = &gpu.gradients
    {
        queue.write_texture(
            TexelCopyTextureInfo {
                texture: v.texture(),
                mip_level: 0,
                origin: Origin3d::ZERO,
                aspect: TextureAspect::All,
            },
            texels.as_flattened(),
            TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(256 * 4), rows_per_image: Some(used) },
            Extent3d { width: 256, height: used, depth_or_array_layers: 1 },
        );
        if gpu.gradient_rows.len() < texels.len() {
            gpu.gradient_rows.resize(texels.len(), [0; 4]);
        }
        gpu.gradient_rows[..texels.len()].copy_from_slice(texels);
    }
    gpu.regions = stage(&mut gpu.staged, &frame);
    write_frame(&device, &queue, &cache.get_bind_group_layout(&p.fill_layout), gpu);
    let size = frame.viewport;
    if size.x > 0 && gpu.targets.as_ref().is_none_or(|t| t.size != size) {
        let attach = TextureUsages::RENDER_ATTACHMENT;
        let view = |d: TextureDescriptor| device.create_texture(&d).create_view(&TextureViewDescriptor::default());
        gpu.targets = Some(Targets {
            size,
            color: view(texture_2d_desc(
                "ssx_ui_target",
                size,
                UI_FORMAT,
                SAMPLES,
                attach | TextureUsages::TEXTURE_BINDING,
            )),
            stencil: view(texture_2d_desc("ssx_ui_stencil", size, STENCIL_FORMAT, SAMPLES, attach)),
        });
    }
    if let Some(a) = &frame.glyphs
        && gpu.glyphs.as_ref().is_none_or(|g| g.0 != a.generation)
    {
        let size = UVec2::new(a.width, a.height);
        match gpu.glyphs.as_mut().filter(|g| g.1 == size) {
            Some(g) => {
                queue.write_texture(
                    TexelCopyTextureInfo {
                        texture: g.2.texture(),
                        mip_level: 0,
                        origin: Origin3d::ZERO,
                        aspect: TextureAspect::All,
                    },
                    &a.alpha,
                    TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(a.width), rows_per_image: Some(a.height) },
                    Extent3d { width: a.width, height: a.height, depth_or_array_layers: 1 },
                );
                g.0 = a.generation;
            }
            None => {
                let usage = TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST;
                let desc = texture_2d_desc("ssx_ui_glyphs", size, TextureFormat::R8Unorm, 1, usage);
                let v = device
                    .create_texture_with_data(&queue, &desc, TextureDataOrder::LayerMajor, &a.alpha)
                    .create_view(&TextureViewDescriptor::default());
                let g = paint_group(&v, &p.linear);
                gpu.glyphs = Some((a.generation, size, v, g));
            }
        }
    }
    let used = frame.ops.iter().filter_map(|o| match o {
        UiOp::Draw { texture: UiTexture::Image { image, smooth }, .. } => Some((*image, *smooth)),
        _ => None,
    });
    for key in used {
        if !gpu.image_groups.contains_key(&key)
            && let Some(v) = gpu.images.get(key.0)
        {
            let g = paint_group(v, if key.1 { &p.linear } else { &p.nearest });
            gpu.image_groups.insert(key, g);
        }
    }
}

struct Full<'a> {
    pipeline: &'a RenderPipeline,
    groups: Vec<BindGroup>,
}

fn fullscreen(encoder: &mut CommandEncoder, label: &str, target: &TextureView, clear: bool, pass: Full) {
    let mut rp = encoder.begin_render_pass(&RenderPassDescriptor {
        label: Some(label),
        color_attachments: &[Some(RenderPassColorAttachment {
            view: target,
            depth_slice: None,
            resolve_target: None,
            ops: Operations {
                load: if clear { LoadOp::Clear(default()) } else { LoadOp::Load },
                store: StoreOp::Store,
            },
        })],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    });
    rp.set_pipeline(pass.pipeline);
    for (i, g) in pass.groups.iter().enumerate() {
        rp.set_bind_group(i as u32, g, &[]);
    }
    rp.draw(0..3, 0..1);
}

fn texture_group(gpu: &UiGpu, t: UiTexture) -> Option<&BindGroup> {
    match t {
        UiTexture::None => gpu.white.as_ref().map(|w| &w.1),
        UiTexture::Gradients => gpu.gradients.as_ref().map(|g| &g.1),
        UiTexture::Image { image, smooth } => gpu.image_groups.get(&(image, smooth)),
        UiTexture::Glyphs => gpu.glyphs.as_ref().map(|g| &g.3),
    }
}

fn replay<'a>(
    rp: &mut TrackedRenderPass<'a>,
    ops: &[UiOp],
    gpu: &'a UiGpu,
    pick: &dyn Fn(UiBlend, u8) -> Option<&'a RenderPipeline>,
) {
    struct Batch<'a> {
        pipeline: &'a RenderPipeline,
        group: &'a BindGroup,
        base_vertex: i32,
        indices: std::ops::Range<u32>,
    }
    fn flush<'a>(rp: &mut TrackedRenderPass<'a>, batch: &mut Option<Batch<'a>>) {
        if let Some(b) = batch.take() {
            rp.set_render_pipeline(b.pipeline);
            rp.set_bind_group(1, b.group, &[]);
            rp.draw_indexed(b.indices, b.base_vertex, 0..1);
        }
    }
    let (mut mode, mut level) = (0u8, 0u32);
    let mut batch: Option<Batch<'a>> = None;
    for op in ops {
        match *op {
            UiOp::MaskBegin => {
                flush(rp, &mut batch);
                mode = 1;
            }
            UiOp::MaskEnd => {
                flush(rp, &mut batch);
                mode = 0;
                level += 1;
                rp.set_stencil_reference(level);
            }
            UiOp::MaskPopBegin => {
                flush(rp, &mut batch);
                mode = 2;
            }
            UiOp::MaskPopEnd => {
                flush(rp, &mut batch);
                mode = 0;
                level = level.saturating_sub(1);
                rp.set_stencil_reference(level);
            }
            UiOp::Draw { first_index, index_count, base_vertex, texture, blend, .. } => {
                let Some(p) = pick(if mode == 0 { blend } else { UiBlend::Normal }, mode) else { continue };
                let Some(g) = texture_group(gpu, texture) else { continue };
                let range = first_index..first_index + index_count;
                if let Some(b) = batch.as_mut()
                    && std::ptr::eq(b.pipeline, p)
                    && std::ptr::eq(b.group, g)
                    && b.base_vertex == base_vertex
                    && b.indices.end == range.start
                {
                    b.indices.end = range.end;
                    continue;
                }
                flush(rp, &mut batch);
                batch = Some(Batch { pipeline: p, group: g, base_vertex, indices: range });
            }
        }
    }
    flush(rp, &mut batch);
}

#[allow(clippy::too_many_arguments)]
fn ui_pass(
    view: ViewQuery<&ViewTarget, With<UiCamera>>,
    frame: Option<Res<UiFrame>>,
    pipelines: Option<Res<UiPipelines>>,
    gpu: Res<UiGpu>,
    cache: Res<PipelineCache>,
    mut binds: Local<BindGroupCache>,
    mut ctx: RenderContext,
) {
    let (Some(frame), Some(p)) = (frame, pipelines) else { return };
    let target = view.into_inner();
    let gpu = &*gpu;
    let Some(t) = gpu.targets.as_ref().filter(|t| t.size == frame.viewport) else { return };
    let (Some(buffer), Some(draws_group)) = (&gpu.frame, &gpu.draws_group) else {
        return;
    };
    let get = |id: CachedRenderPipelineId| cache.get_render_pipeline(id).cloned();
    let Some(out) = p.result_out.get(&target.main_texture_format()).and_then(|id| get(*id)) else { return };
    let (Some(scene_in), Some(mask_incr), Some(mask_decr)) = (get(p.scene_in), get(p.mask_incr), get(p.mask_decr))
    else {
        return;
    };
    let content: HashMap<UiBlend, RenderPipeline> =
        p.content.iter().filter_map(|(b, id)| Some((*b, get(*id)?))).collect();
    if content.len() != p.content.len() {
        return;
    }
    let device = ctx.render_device().clone();
    let full_layout = cache.get_bind_group_layout(&p.full_layout);
    let post = target.post_process_write();
    // PORT(choice): the scene's and the result's bind groups are kept across frames and made again only when what
    // they bind changes (`BindGroupCache`: the view's two main textures in turn, the UI target, its sampler), instead
    // of two made and freed every frame. The same resources bound, so the same passes. wgpu's bind groups are the
    // port's renderer; the console's GPU had none.
    binds.next_frame();
    match frame.clear {
        Some([r, g, b]) => {
            let _ = ctx.command_encoder().begin_render_pass(&RenderPassDescriptor {
                label: Some("ssx_ui_clear"),
                color_attachments: &[Some(RenderPassColorAttachment {
                    view: &t.color,
                    depth_slice: None,
                    resolve_target: None,
                    ops: Operations {
                        load: LoadOp::Clear(LinearRgba::new(r, g, b, 1.0).into()),
                        store: StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
        }
        None => {
            let g = binds.get(
                &device,
                "ssx_ui_scene",
                &full_layout,
                &BindGroupEntries::sequential((post.source, &p.linear)),
            );
            fullscreen(
                ctx.command_encoder(),
                "ssx_ui_scene_in",
                &t.color,
                true,
                Full { pipeline: &scene_in, groups: vec![g] },
            );
        }
    }
    {
        let mut rp = ctx.begin_tracked_render_pass(RenderPassDescriptor {
            label: Some("ssx_ui"),
            color_attachments: &[Some(RenderPassColorAttachment {
                view: &t.color,
                depth_slice: None,
                resolve_target: None,
                ops: Operations { load: LoadOp::Load, store: StoreOp::Store },
            })],
            depth_stencil_attachment: Some(RenderPassDepthStencilAttachment {
                view: &t.stencil,
                depth_ops: Some(Operations { load: LoadOp::Clear(1.0), store: StoreOp::Discard }),
                stencil_ops: Some(Operations { load: LoadOp::Clear(0), store: StoreOp::Discard }),
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        let r = &gpu.regions;
        if r.drawn() {
            rp.set_vertex_buffer(0, buffer.slice(r.vertices.clone()));
            rp.set_vertex_buffer(1, buffer.slice(r.paint_coords.clone()));
            rp.set_vertex_buffer(2, buffer.slice(r.draw_ids.clone()));
            rp.set_index_buffer(buffer.slice(r.indices.clone()), IndexFormat::Uint32);
            rp.set_bind_group(0, draws_group, &[]);
            rp.set_stencil_reference(0);
            replay(&mut rp, &frame.ops, gpu, &|b, mode| match mode {
                0 => content.get(&b),
                1 => Some(&mask_incr),
                _ => Some(&mask_decr),
            });
        }
    }
    let g = binds.get(&device, "ssx_ui_result", &full_layout, &BindGroupEntries::sequential((&t.color, &p.linear)));
    fullscreen(
        ctx.command_encoder(),
        "ssx_ui_result_out",
        post.destination,
        true,
        Full { pipeline: &out, groups: vec![g] },
    );
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::{DRAW_FLOATS, UiFrame, stage};

    #[test]
    fn the_frames_arrays_are_staged_in_one_run_of_little_endian_bytes() {
        let mut draw = [0.0; DRAW_FLOATS];
        draw[0] = 1.5;
        draw[DRAW_FLOATS - 1] = -2.0;
        let frame = UiFrame {
            draws: Arc::new(vec![draw, [0.25; DRAW_FLOATS]]),
            vertices: Arc::new(vec![[1.0, 2.0], [3.0, 4.0], [5.0, -0.0]]),
            paint_coords: Arc::new(vec![[0.5, 0.5], [0.0, 1.0], [1.0, 0.0]]),
            draw_ids: Arc::new(vec![0, 0, 1]),
            indices: Arc::new(vec![0, 1, 2]),
            ..UiFrame::default()
        };
        let mut out = vec![7; 3];
        let r = stage(&mut out, &frame);
        let draws = 2 * DRAW_FLOATS as u64 * 4;
        assert_eq!(r.vertices, draws..draws + 24);
        assert_eq!(r.paint_coords, draws + 24..draws + 48);
        assert_eq!(r.draw_ids, draws + 48..draws + 60);
        assert_eq!(r.indices, draws + 60..draws + 72);
        assert_eq!(out.len() as u64, draws + 72);
        assert!(r.drawn());
        assert_eq!(out[..4], 1.5f32.to_le_bytes());
        assert_eq!(out[(draws - 4) as usize..draws as usize], 0.25f32.to_le_bytes());
        assert_eq!(out[(draws + 20) as usize..(draws + 24) as usize], (-0.0f32).to_le_bytes());
        assert_eq!(out[(draws + 56) as usize..(draws + 60) as usize], 1u32.to_le_bytes());

        let r = stage(&mut out, &UiFrame::default());
        assert!(out.is_empty());
        assert!(!r.drawn());
    }
}
