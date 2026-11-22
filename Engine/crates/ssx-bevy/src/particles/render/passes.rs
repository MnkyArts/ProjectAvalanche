use bevy::prelude::*;
use bevy::render::render_resource::*;
use bevy::render::renderer::{RenderContext, RenderDevice};
use bevy::render::view::ViewDepthStencilTexture;

use super::pipelines::{ParticlePipelines, TECHS};
use super::{Bin, ParticleGpu};
use crate::render::bind_cache::BindGroupCache;

#[allow(clippy::too_many_arguments)]
fn pass_bind(
    binds: &mut BindGroupCache,
    device: &RenderDevice,
    layout: &BindGroupLayout,
    post: BindingResource,
    globals: BindingResource,
    src: &TextureView,
    smooth: &Sampler,
    exposure: &Buffer,
    scene_depth: &TextureView,
    half_depth: &TextureView,
) -> BindGroup {
    binds.get(
        device,
        "ssx_particles_pass",
        layout,
        &BindGroupEntries::sequential((
            post,
            globals,
            src,
            smooth,
            exposure.as_entire_binding(),
            scene_depth,
            half_depth,
        )),
    )
}

fn fullscreen(
    ctx: &mut RenderContext,
    label: &'static str,
    target: &TextureView,
    clear: Option<LinearRgba>,
    pipeline: &RenderPipeline,
    bind: &BindGroup,
    depth: Option<&TextureView>,
) {
    let mut pass = ctx.begin_tracked_render_pass(RenderPassDescriptor {
        label: Some(label),
        color_attachments: &[Some(RenderPassColorAttachment {
            view: target,
            depth_slice: None,
            resolve_target: None,
            ops: Operations {
                load: match clear {
                    Some(c) => LoadOp::Clear(c.into()),
                    None => LoadOp::Load,
                },
                store: StoreOp::Store,
            },
        })],
        depth_stencil_attachment: depth.map(|view| RenderPassDepthStencilAttachment {
            view,
            depth_ops: Some(Operations { load: LoadOp::Clear(0.0), store: StoreOp::Store }),
            stencil_ops: None,
        }),
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    });
    pass.set_render_pipeline(pipeline);
    pass.set_bind_group(0, bind, &[]);
    pass.draw(0..3, 0..1);
}

#[allow(clippy::too_many_arguments)]
pub fn draw(
    world: &World,
    ctx: &mut RenderContext,
    depth: &ViewDepthStencilTexture,
    frame: &TextureView,
    bloom: Option<&TextureView>,
    post: BindingResource,
    exposure: &Buffer,
    binds: &mut BindGroupCache,
) {
    let (Some(gpu), Some(p), Some(cache)) = (
        world.get_resource::<ParticleGpu>(),
        world.get_resource::<ParticlePipelines>(),
        world.get_resource::<PipelineCache>(),
    ) else {
        return;
    };
    let (Some(t), Some((_, textures)), Some(records)) = (&gpu.targets, &gpu.textures, &gpu.records) else { return };
    if gpu.cmds.is_empty() || !depth.texture().usage().contains(TextureUsages::TEXTURE_BINDING) {
        return;
    }
    let depth_views = depth.attachment.depth_stencil_views();
    let Some(depth_sampled) = depth_views.depth_only_view() else { return };
    let get = |id| cache.get_render_pipeline(id);
    let (Some(filter), Some(copy), Some(tonemap), Some(bloom_p), Some(composite)) =
        (get(p.depth_filter), get(p.depth_copy), get(p.tonemap), get(p.bloom), get(p.composite))
    else {
        return;
    };
    let Some(techs) = p.techs.iter().map(|&id| get(id)).collect::<Option<Vec<_>>>() else { return };
    let (Some(globals), Some(draws)) = (gpu.globals.binding(), gpu.draws.binding()) else {
        return;
    };
    let device = ctx.render_device().clone();
    let pass_layout = cache.get_bind_group_layout(&p.pass_layout);
    let draw_layout = cache.get_bind_group_layout(&p.draw_layout);
    let mut draw_binds = |sampled_depth: &TextureView| -> Vec<BindGroup> {
        textures
            .iter()
            .map(|tex| {
                binds.get(
                    &device,
                    "ssx_particles_draw",
                    &draw_layout,
                    &BindGroupEntries::sequential((
                        globals.clone(),
                        draws.clone(),
                        records.as_entire_binding(),
                        tex,
                        &p.texture_sampler,
                        sampled_depth,
                    )),
                )
            })
            .collect()
    };
    let (half_binds, full_binds) = (draw_binds(&t.depth_copy), draw_binds(&t.depth_full));
    let mut bind = |src: &TextureView, half: &TextureView| {
        pass_bind(
            binds,
            &device,
            &pass_layout,
            post.clone(),
            globals.clone(),
            src,
            &p.smooth,
            exposure,
            depth_sampled,
            half,
        )
    };
    fullscreen(
        ctx,
        "ssx_particles_depth_filter",
        &t.depth_copy,
        Some(LinearRgba::NONE),
        filter,
        &bind(&t.colour_a, &t.dummy),
        Some(&t.depth),
    );
    let bin = |ctx: &mut RenderContext,
               label: &'static str,
               target: &TextureView,
               depth_view: &TextureView,
               clear: Option<LinearRgba>,
               which: Bin| {
        let mut pass = ctx.begin_tracked_render_pass(RenderPassDescriptor {
            label: Some(label),
            color_attachments: &[Some(RenderPassColorAttachment {
                view: target,
                depth_slice: None,
                resolve_target: None,
                ops: Operations {
                    load: match clear {
                        Some(c) => LoadOp::Clear(c.into()),
                        None => LoadOp::Load,
                    },
                    store: StoreOp::Store,
                },
            })],
            depth_stencil_attachment: Some(RenderPassDepthStencilAttachment {
                view: depth_view,
                depth_ops: Some(Operations { load: LoadOp::Load, store: StoreOp::Store }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        let binds = if which == Bin::FullRes { &full_binds } else { &half_binds };
        for c in gpu.cmds.iter().filter(|c| c.bin == which) {
            pass.set_render_pipeline(techs[TECHS.iter().position(|t| *t == c.tech).unwrap_or(0)]);
            pass.set_bind_group(0, &binds[c.texture], &[c.offset]);
            pass.draw(0..c.vertices, 0..1);
        }
    };
    let clear = Some(LinearRgba::new(0.0, 0.0, 0.0, 1.0));
    bin(ctx, "ssx_particles_tonemapped", &t.colour_a, &t.depth, clear, Bin::Tonemapped);
    if let Some(b) = bloom {
        fullscreen(ctx, "ssx_particles_bloom", b, None, bloom_p, &bind(&t.colour_a, &t.depth_copy), None);
    }
    fullscreen(ctx, "ssx_particles_tonemap", &t.colour_b, None, tonemap, &bind(&t.colour_a, &t.depth_copy), None);
    bin(ctx, "ssx_particles_halfres", &t.colour_b, &t.depth, None, Bin::HalfRes);
    fullscreen(ctx, "ssx_particles_composite", frame, None, composite, &bind(&t.colour_b, &t.depth_copy), None);
    if gpu.cmds.iter().any(|c| c.bin == Bin::FullRes) {
        fullscreen(ctx, "ssx_particles_depth_copy", &t.depth_full, None, copy, &bind(&t.colour_a, &t.dummy), None);
        bin(ctx, "ssx_particles_fullres", frame, depth_views.attachment_view(), None, Bin::FullRes);
    }
}
