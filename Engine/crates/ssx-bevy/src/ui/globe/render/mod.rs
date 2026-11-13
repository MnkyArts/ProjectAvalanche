mod pipelines;
mod prepare;
mod upload;

use std::collections::HashMap;

use bevy::core_pipeline::tonemapping::tonemapping;
use bevy::core_pipeline::{Core2d, Core2dSystems};
use bevy::prelude::*;
use bevy::render::render_asset::RenderAssets;
use bevy::render::render_resource::*;
use bevy::render::renderer::{RenderContext, ViewQuery};
use bevy::render::texture::GpuImage;
use bevy::render::view::ViewTarget;
use bevy::render::{Render, RenderApp, RenderStartup, RenderSystems};
use bevy::shader::Shader;

use super::super::UiCamera;
use super::dump::{self, Dumps};
use super::{GlobeSystems, GlobeView};
use crate::render::post::adapt::HdrAdaptation;
use pipelines::{Pipelines, init};
use prepare::prepare;
use upload::{MeshBuffers, MidgroundGpu, Targets, target_texture};

const HDR_FORMAT: TextureFormat = TextureFormat::Rgba16Float;
const LDR_FORMAT: TextureFormat = TextureFormat::Rgba8Unorm;
const DEPTH_FORMAT: TextureFormat = TextureFormat::Depth32Float;

#[derive(Resource)]
struct GlobeShaders {
    globe: Handle<Shader>,
    post: Handle<Shader>,
    midground: Handle<Shader>,
    backdrop: Handle<Shader>,
    fade: Handle<Shader>,
    clouds: Handle<Shader>,
    waves: Handle<Shader>,
}

pub(super) fn build(app: &mut App) {
    let mut shaders = app.world_mut().resource_mut::<Assets<Shader>>();
    let globe = shaders.add(Shader::from_wgsl(include_str!("globe.wgsl"), "ui/globe/render/globe.wgsl"));
    let post = shaders.add(Shader::from_wgsl(include_str!("globe_post.wgsl"), "ui/globe/render/globe_post.wgsl"));
    let midground = shaders.add(Shader::from_wgsl(include_str!("midground.wgsl"), "ui/globe/render/midground.wgsl"));
    let backdrop =
        shaders.add(Shader::from_wgsl(include_str!("globe_backdrop.wgsl"), "ui/globe/render/globe_backdrop.wgsl"));
    let fade = shaders.add(Shader::from_wgsl(include_str!("../../screen_fade.wgsl"), "ui/screen_fade.wgsl"));
    let clouds = shaders.add(Shader::from_wgsl(include_str!("globe_clouds.wgsl"), "ui/globe/render/globe_clouds.wgsl"));
    let waves = shaders.add(Shader::from_wgsl(include_str!("oceanwave.wgsl"), "ui/globe/render/oceanwave.wgsl"));
    let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
        return;
    };
    render_app
        .insert_resource(GlobeShaders { globe, post, midground, backdrop, fade, clouds, waves })
        .init_resource::<GlobeGpu>()
        .init_resource::<HdrAdaptation>()
        .init_resource::<Dumps>()
        .add_systems(RenderStartup, init)
        .add_systems(Render, (prepare, prepare_backdrop_movie).in_set(RenderSystems::PrepareResources))
        .add_systems(Render, dump::write_dumps.in_set(RenderSystems::Cleanup))
        .add_systems(
            Core2d,
            globe_pass
                .in_set(Core2dSystems::PostProcess)
                .in_set(GlobeSystems)
                .after(tonemapping)
                .before(super::super::UiPassSystems),
        );
}

#[derive(Resource, Default)]
struct GlobeGpu {
    uploaded: Option<usize>,
    earth: Option<MeshBuffers>,
    halo: Option<MeshBuffers>,
    diffuse: Option<TextureView>,
    relief: Option<TextureView>,
    frost: Option<TextureView>,
    background: Option<TextureView>,
    backdrop_movie: Option<TextureView>,
    sky_textures: HashMap<String, TextureView>,
    blank: Option<TextureView>,
    earth_uniform: Option<Buffer>,
    halo_uniform: Option<Buffer>,
    backdrop_uniform: Option<Buffer>,
    post_uniform: Option<Buffer>,
    fade_uniform: Option<Buffer>,
    lut: Option<TextureView>,
    lut_version: u64,
    targets: Option<Targets>,
    mg_uploaded: Option<usize>,
    mg_parts: Vec<MidgroundGpu>,
    mg_waves: Vec<(MidgroundGpu, Buffer)>,
    cloud_texture: Option<TextureView>,
    clouds: Vec<(Buffer, Buffer, u32)>,
    active: bool,
    globe: bool,
    midground: bool,
    sky: Option<[String; 3]>,
    bloom: bool,
    dump: Option<std::path::PathBuf>,
}

fn prepare_backdrop_movie(
    view: Option<Res<GlobeView>>,
    images: Res<RenderAssets<GpuImage>>,
    mut gpu: ResMut<GlobeGpu>,
) {
    let movie = view.as_ref().and_then(|v| v.backdrop_movie.as_ref()).and_then(|h| images.get(h));
    gpu.backdrop_movie = movie.map(|i| i.texture_view.clone());
}

fn color_desc<'a>(
    label: &'static str,
    attachment: &'a [Option<RenderPassColorAttachment<'a>>],
) -> RenderPassDescriptor<'a> {
    RenderPassDescriptor {
        label: Some(label),
        color_attachments: attachment,
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    }
}

fn attachment(view: &TextureView, clear: bool) -> [Option<RenderPassColorAttachment<'_>>; 1] {
    [Some(RenderPassColorAttachment {
        view,
        depth_slice: None,
        resolve_target: None,
        ops: Operations { load: if clear { LoadOp::Clear(default()) } else { LoadOp::Load }, store: StoreOp::Store },
    })]
}

fn copy(encoder: &mut CommandEncoder, from: &Texture, to: &Texture, size: UVec2) {
    let info =
        |t| TexelCopyTextureInfo { texture: t, mip_level: 0, origin: Origin3d::ZERO, aspect: TextureAspect::All };
    encoder.copy_texture_to_texture(
        info(from),
        info(to),
        Extent3d { width: size.x.max(1), height: size.y.max(1), depth_or_array_layers: 1 },
    );
}

fn full_screen(
    encoder: &mut CommandEncoder,
    label: &'static str,
    view: &TextureView,
    p: &RenderPipeline,
    g: &BindGroup,
) {
    let a = attachment(view, false);
    let mut rp = encoder.begin_render_pass(&color_desc(label, &a));
    rp.set_pipeline(p);
    rp.set_bind_group(0, g, &[]);
    rp.draw(0..3, 0..1);
}

fn globe_pass(
    view: ViewQuery<&ViewTarget, With<UiCamera>>,
    p: Option<Res<Pipelines>>,
    gpu: Res<GlobeGpu>,
    cache: Res<PipelineCache>,
    dumps: Res<Dumps>,
    adapt: Res<HdrAdaptation>,
    mut ctx: RenderContext,
) {
    let Some(p) = p else { return };
    if !gpu.active {
        return;
    }
    let target = view.into_inner();
    let (Some(t), Some(earth), Some(halo)) = (gpu.targets.as_ref(), gpu.earth.as_ref(), gpu.halo.as_ref()) else {
        return;
    };
    let (Some(diffuse), Some(relief), Some(frost)) = (gpu.diffuse.as_ref(), gpu.relief.as_ref(), gpu.frost.as_ref())
    else {
        return;
    };
    let (Some(background), Some(blank), Some(lut)) = (gpu.background.as_ref(), gpu.blank.as_ref(), gpu.lut.as_ref())
    else {
        return;
    };
    let (Some(eu), Some(hu), Some(bu), Some(pu)) = (
        gpu.earth_uniform.as_ref(),
        gpu.halo_uniform.as_ref(),
        gpu.backdrop_uniform.as_ref(),
        gpu.post_uniform.as_ref(),
    ) else {
        return;
    };
    let get = |id: CachedRenderPipelineId| cache.get_render_pipeline(id).cloned();
    let (Some(earth_p), Some(halo_p), Some(quarter_p)) = (get(p.earth), get(p.halo), get(p.quarter)) else { return };
    let (Some(background_p), Some(sky_p), Some(threshold_p)) = (get(p.background), get(p.sky), get(p.threshold)) else {
        return;
    };
    let (Some(tonemap_p), Some(fxaa_p), Some(blur_h_p), Some(blur_v_p)) =
        (get(p.tonemap), get(p.fxaa), get(p.blur_h), get(p.blur_v))
    else {
        return;
    };
    let Some(composite_p) = p.composite.get(&target.main_texture_format()).and_then(|id| get(*id)) else { return };
    let (Some(lum_p), Some(reduce_p)) =
        (cache.get_compute_pipeline(p.luminance).cloned(), cache.get_compute_pipeline(p.reduce).cloned())
    else {
        return;
    };
    let device = ctx.render_device().clone();
    let globe_layout = cache.get_bind_group_layout(&p.globe_layout);
    let globe_group = |u: &Buffer, tex: &TextureView| {
        device.create_bind_group(
            "ssx_globe",
            &globe_layout,
            &BindGroupEntries::sequential((
                u.as_entire_binding(),
                tex,
                relief,
                &t.frame_view,
                &p.wrap_linear,
                &p.clamp_point,
            )),
        )
    };
    let earth_group = globe_group(eu, diffuse);
    let halo_group = globe_group(hu, frost);
    let sky_tex =
        |i: usize| -> &TextureView { gpu.sky.as_ref().and_then(|s| gpu.sky_textures.get(&s[i])).unwrap_or(blank) };
    let movie = gpu.backdrop_movie.as_ref().filter(|_| gpu.sky.is_none()).zip(get(p.background_movie));
    let (backdrop_p, picture) = match &movie {
        Some((view, movie_p)) => (movie_p, *view),
        None if gpu.sky.is_some() => (&sky_p, background),
        None => (&background_p, background),
    };
    let backdrop_group = device.create_bind_group(
        "ssx_globe_backdrop",
        &cache.get_bind_group_layout(&p.backdrop_layout),
        &BindGroupEntries::sequential((
            bu.as_entire_binding(),
            picture,
            sky_tex(0),
            sky_tex(1),
            sky_tex(2),
            &p.clamp_linear,
            &p.sky_cloud,
        )),
    );
    let single = |label: &'static str, layout: &BindGroupLayoutDescriptor, tex: &TextureView| {
        device.create_bind_group(
            label,
            &cache.get_bind_group_layout(layout),
            &BindGroupEntries::sequential((pu.as_entire_binding(), tex)),
        )
    };
    let quarter_group = single("ssx_globe_quarter", &p.quarter_layout, &t.hdr_view);
    let compute_group = device.create_bind_group(
        "ssx_globe_exposure",
        &cache.get_bind_group_layout(&p.compute_layout),
        &BindGroupEntries::sequential((
            pu.as_entire_binding(),
            &t.quarter_view,
            t.adapted.as_entire_binding(),
            t.exposure.as_entire_binding(),
        )),
    );
    let tone_group = |label: &'static str, tex: &TextureView| {
        device.create_bind_group(
            label,
            &cache.get_bind_group_layout(&p.tone_layout),
            &BindGroupEntries::with_indices((
                (0, pu.as_entire_binding()),
                (1, tex),
                (4, t.exposure.as_entire_binding()),
            )),
        )
    };
    let threshold_group = tone_group("ssx_globe_threshold", &t.quarter_view);
    let tonemap_group = tone_group("ssx_globe_tonemap", &t.hdr_view);
    let filter_group = |label: &'static str, tex: &TextureView| {
        device.create_bind_group(
            label,
            &cache.get_bind_group_layout(&p.filter_layout),
            &BindGroupEntries::with_indices(((0, pu.as_entire_binding()), (1, tex), (5, &p.clamp_linear))),
        )
    };
    let fxaa_group = filter_group("ssx_globe_fxaa", &t.ldr_view);
    let blur_h_group = filter_group("ssx_globe_blur_h", &t.bloom_a_view);
    let blur_v_group = filter_group("ssx_globe_blur_v", &t.bloom_b_view);
    let composite_group = device.create_bind_group(
        "ssx_globe_composite",
        &cache.get_bind_group_layout(&p.composite_layout),
        &BindGroupEntries::with_indices((
            (0, pu.as_entire_binding()),
            (1, &t.ldr_aa_view),
            (5, &p.clamp_linear),
            (6, &t.bloom_a_view),
            (7, lut),
        )),
    );
    let cloud_layout = cache.get_bind_group_layout(&p.cloud_layout);
    let cloud_groups: Vec<(BindGroup, u32)> = match gpu.cloud_texture.as_ref() {
        Some(ct) => gpu
            .clouds
            .iter()
            .map(|(u, r, n)| {
                let g = device.create_bind_group(
                    "ssx_globe_clouds",
                    &cloud_layout,
                    &BindGroupEntries::with_indices((
                        (0, u.as_entire_binding()),
                        (1, r.as_entire_binding()),
                        (2, ct),
                        (3, &p.particle),
                        (4, &t.half_depth_view),
                    )),
                );
                (g, *n)
            })
            .collect(),
        None => Vec::new(),
    };
    let first_cloud = gpu.clouds.first().map(|(u, _, _)| u);
    let filter_group = first_cloud.map(|u| {
        device.create_bind_group(
            "ssx_globe_depth_filter",
            &cache.get_bind_group_layout(&p.depth_filter_layout),
            &BindGroupEntries::with_indices(((0, u.as_entire_binding()), (5, &t.depth_view))),
        )
    });
    let comp_group = first_cloud.map(|u| {
        device.create_bind_group(
            "ssx_globe_cloud_composite",
            &cache.get_bind_group_layout(&p.cloud_composite_layout),
            &BindGroupEntries::with_indices((
                (0, u.as_entire_binding()),
                (4, &t.half_depth_view),
                (5, &t.depth_view),
                (6, &t.half_colour_view),
                (7, &p.clamp_linear),
            )),
        )
    });
    let (cloud_p, filter_p, comp_p) = (get(p.cloud), get(p.depth_filter), get(p.cloud_composite));
    let wave_p = get(p.wave);
    let wave_layout = cache.get_bind_group_layout(&p.wave_layout);
    let wave_groups: Vec<_> = gpu
        .mg_waves
        .iter()
        .filter(|(g, _)| gpu.midground && g.uniform.is_some() && g.textures.len() == 3)
        .map(|(g, colours)| {
            let tx = &g.textures;
            let group = device.create_bind_group(
                "ssx_globe_waves",
                &wave_layout,
                &BindGroupEntries::sequential((
                    g.uniform.as_ref().map(|u| u.as_entire_binding()).expect("filtered"),
                    &tx[0],
                    &tx[1],
                    &tx[2],
                    &p.wrap_trilinear,
                    t.exposure.as_entire_binding(),
                )),
            );
            (g, colours, group)
        })
        .collect();
    let post = target.post_process_write();
    let mg_layout = cache.get_bind_group_layout(&p.mg_layout);
    let mg_groups: Vec<_> = gpu
        .mg_parts
        .iter()
        .filter(|g| gpu.midground && g.uniform.is_some() && g.textures.len() == 7)
        .map(|g| {
            let tx = &g.textures;
            let group = device.create_bind_group(
                "ssx_globe_midground",
                &mg_layout,
                &BindGroupEntries::sequential((
                    g.uniform.as_ref().map(|u| u.as_entire_binding()).expect("filtered"),
                    &tx[0],
                    &tx[1],
                    &tx[2],
                    &tx[3],
                    &tx[4],
                    &tx[5],
                    &tx[6],
                    &p.wrap_trilinear,
                    &p.border_trilinear,
                )),
            );
            (g, group)
        })
        .collect();
    let mg_p = cache.get_render_pipeline(p.midground).cloned();
    let fade_p = p.fade.get(&target.main_texture_format()).and_then(|id| get(*id));
    let encoder = ctx.command_encoder();
    let dd = gpu.dump.as_deref();
    let q_size = UVec2::new((t.size.x / 4).max(1), (t.size.y / 4).max(1));
    let h_size = UVec2::new((t.size.x / 2).max(1), (t.size.y / 2).max(1));
    let grab = |encoder: &mut CommandEncoder, name: &str, tex: &Texture, size: UVec2, format: TextureFormat| {
        if let Some(d) = dd {
            dumps.texture(&device, encoder, d, name, tex, size, format);
        }
    };
    full_screen(encoder, "ssx_globe_backdrop", &t.hdr_view, backdrop_p, &backdrop_group);
    grab(encoder, "backdrop", &t.hdr, t.size, HDR_FORMAT);
    {
        let a = attachment(&t.hdr_view, false);
        let mut desc = color_desc("ssx_globe_midground", &a);
        let depth = RenderPassDepthStencilAttachment {
            view: &t.depth_view,
            depth_ops: Some(Operations { load: LoadOp::Clear(1.0), store: StoreOp::Store }),
            stencil_ops: None,
        };
        desc.depth_stencil_attachment = Some(depth);
        let mut rp = encoder.begin_render_pass(&desc);
        if let Some(mg_p) = mg_p.as_ref().filter(|_| gpu.midground) {
            rp.set_pipeline(mg_p);
            for (g, group) in &mg_groups {
                rp.set_bind_group(0, group, &[]);
                rp.set_vertex_buffer(0, *g.positions.slice(..));
                rp.set_vertex_buffer(1, *g.normals.slice(..));
                rp.set_vertex_buffer(2, *g.tangents.slice(..));
                rp.set_vertex_buffer(3, *g.uvs.slice(..));
                rp.set_index_buffer(*g.indices.slice(..), IndexFormat::Uint32);
                rp.draw_indexed(0..g.count, 0, 0..1);
            }
        }
    }
    if gpu.midground {
        grab(encoder, "midground", &t.hdr, t.size, HDR_FORMAT);
    }
    if gpu.globe {
        copy(encoder, &t.hdr, &t.frame, t.size);
        {
            let a = attachment(&t.hdr_view, false);
            let mut rp = encoder.begin_render_pass(&color_desc("ssx_globe_earth", &a));
            rp.set_pipeline(&earth_p);
            rp.set_bind_group(0, &earth_group, &[]);
            rp.set_vertex_buffer(0, *earth.positions.slice(..));
            rp.set_index_buffer(*earth.indices.slice(..), IndexFormat::Uint32);
            rp.draw_indexed(0..earth.count, 0, 0..1);
        }
        copy(encoder, &t.hdr, &t.frame, t.size);
        {
            let a = attachment(&t.hdr_view, false);
            let mut rp = encoder.begin_render_pass(&color_desc("ssx_globe_halo", &a));
            rp.set_pipeline(&halo_p);
            rp.set_bind_group(0, &halo_group, &[]);
            rp.set_vertex_buffer(0, *halo.positions.slice(..));
            rp.set_vertex_buffer(1, *halo.uvs.slice(..));
            rp.set_index_buffer(*halo.indices.slice(..), IndexFormat::Uint32);
            rp.draw_indexed(0..halo.count, 0, 0..1);
        }
    }
    grab(encoder, "hdr", &t.hdr, t.size, HDR_FORMAT);
    full_screen(encoder, "ssx_globe_quarter", &t.quarter_view, &quarter_p, &quarter_group);
    grab(encoder, "quarter", &t.quarter, q_size, HDR_FORMAT);
    if let Some(d) = dd {
        dumps.buffer(&device, encoder, d, "adapted_prev", &t.adapted, t.adapted.size());
    }
    {
        let q = upload::quarter_of(t.size);
        let mut pass = encoder
            .begin_compute_pass(&ComputePassDescriptor { label: Some("ssx_globe_exposure"), timestamp_writes: None });
        pass.set_bind_group(0, &compute_group, &[]);
        pass.set_pipeline(&lum_p);
        pass.dispatch_workgroups(q.x.div_ceil(8), q.y.div_ceil(8), 1);
        pass.set_pipeline(&reduce_p);
        pass.dispatch_workgroups(1, 1, 1);
    }
    adapt.store(encoder, upload::quarter_of(t.size), &t.adapted);
    if let Some(d) = dd {
        dumps.buffer(&device, encoder, d, "adapted", &t.adapted, t.adapted.size());
        dumps.buffer(&device, encoder, d, "exposure", &t.exposure, 16);
    }
    if gpu.bloom {
        full_screen(encoder, "ssx_globe_threshold", &t.bloom_a_view, &threshold_p, &threshold_group);
        grab(encoder, "threshold", &t.bloom_a, q_size, LDR_FORMAT);
    }
    full_screen(encoder, "ssx_globe_tonemap", &t.ldr_view, &tonemap_p, &tonemap_group);
    grab(encoder, "tonemap", &t.ldr, t.size, LDR_FORMAT);
    if !wave_groups.is_empty()
        && let Some(wave_p) = wave_p.as_ref()
    {
        grab(encoder, "depth_mg", &t.depth, t.size, DEPTH_FORMAT);
        let a = attachment(&t.ldr_view, false);
        let mut desc = color_desc("ssx_globe_waves", &a);
        desc.depth_stencil_attachment = Some(RenderPassDepthStencilAttachment {
            view: &t.depth_view,
            depth_ops: Some(Operations { load: LoadOp::Load, store: StoreOp::Store }),
            stencil_ops: None,
        });
        let mut rp = encoder.begin_render_pass(&desc);
        rp.set_pipeline(wave_p);
        for (g, colours, group) in &wave_groups {
            rp.set_bind_group(0, group, &[]);
            rp.set_vertex_buffer(0, *g.positions.slice(..));
            rp.set_vertex_buffer(1, *g.normals.slice(..));
            rp.set_vertex_buffer(2, *g.tangents.slice(..));
            rp.set_vertex_buffer(3, *g.uvs.slice(..));
            rp.set_vertex_buffer(4, *colours.slice(..));
            rp.set_index_buffer(*g.indices.slice(..), IndexFormat::Uint32);
            rp.draw_indexed(0..g.count, 0, 0..1);
        }
        drop(rp);
        grab(encoder, "waves", &t.ldr, t.size, LDR_FORMAT);
    }
    if !cloud_groups.is_empty()
        && let (Some(cloud_p), Some(filter_p), Some(comp_p)) = (cloud_p, filter_p, comp_p)
        && let (Some(filter_group), Some(comp_group)) = (filter_group.as_ref(), comp_group.as_ref())
    {
        grab(encoder, "depth", &t.depth, t.size, DEPTH_FORMAT);
        full_screen(encoder, "ssx_globe_depth_filter", &t.half_depth_view, &filter_p, filter_group);
        grab(encoder, "half_depth", &t.half_depth, h_size, TextureFormat::R32Float);
        {
            let a = [Some(RenderPassColorAttachment {
                view: &t.half_colour_view,
                depth_slice: None,
                resolve_target: None,
                ops: Operations {
                    load: LoadOp::Clear(LinearRgba::new(0.0, 0.0, 0.0, 1.0).into()),
                    store: StoreOp::Store,
                },
            })];
            let mut rp = encoder.begin_render_pass(&color_desc("ssx_globe_clouds", &a));
            rp.set_pipeline(&cloud_p);
            for (group, n) in &cloud_groups {
                rp.set_bind_group(0, group, &[]);
                rp.draw(0..n * 6, 0..1);
            }
        }
        grab(encoder, "clouds_half", &t.half_colour, h_size, LDR_FORMAT);
        full_screen(encoder, "ssx_globe_cloud_composite", &t.ldr_view, &comp_p, comp_group);
        grab(encoder, "clouds", &t.ldr, t.size, LDR_FORMAT);
    }
    full_screen(encoder, "ssx_globe_fxaa", &t.ldr_aa_view, &fxaa_p, &fxaa_group);
    grab(encoder, "fxaa", &t.ldr_aa, t.size, LDR_FORMAT);
    if gpu.bloom {
        full_screen(encoder, "ssx_globe_blur_h", &t.bloom_b_view, &blur_h_p, &blur_h_group);
        grab(encoder, "blur_h", &t.bloom_b, q_size, LDR_FORMAT);
        full_screen(encoder, "ssx_globe_blur_v", &t.bloom_a_view, &blur_v_p, &blur_v_group);
        grab(encoder, "blur_v", &t.bloom_a, q_size, LDR_FORMAT);
    }
    full_screen(encoder, "ssx_globe_composite", post.destination, &composite_p, &composite_group);
    if let Some(d) = dd {
        let format = target.main_texture_format();
        let usage = TextureUsages::RENDER_ATTACHMENT | TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_SRC;
        let composite = target_texture(&device, "ssx_globe_dump_composite", t.size, format, usage);
        let composite_view = composite.create_view(&TextureViewDescriptor::default());
        full_screen(encoder, "ssx_globe_dump_composite", &composite_view, &composite_p, &composite_group);
        dumps.texture(&device, encoder, d, "composite", &composite, t.size, format);
        if let (Some(fade_p), Some(fu)) = (fade_p.as_ref(), gpu.fade_uniform.as_ref()) {
            let faded = target_texture(&device, "ssx_globe_dump_fade", t.size, format, usage);
            let faded_view = faded.create_view(&TextureViewDescriptor::default());
            let group = device.create_bind_group(
                "ssx_screen_fade",
                &cache.get_bind_group_layout(&p.fade_layout),
                &BindGroupEntries::sequential((fu.as_entire_binding(), &composite_view)),
            );
            full_screen(encoder, "ssx_globe_dump_fade", &faded_view, fade_p, &group);
            dumps.texture(&device, encoder, d, "fade", &faded, t.size, format);
        }
    }
}
