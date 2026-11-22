use std::collections::HashMap;

use bevy::core_pipeline::upscaling::upscaling;
use bevy::core_pipeline::{Core2d, Core2dSystems, Core3d};
use bevy::prelude::*;
use bevy::render::extract_resource::{ExtractResource, ExtractResourcePlugin};
use bevy::render::render_resource::binding_types::{texture_2d, uniform_buffer_sized};
use bevy::render::render_resource::*;
use bevy::render::renderer::{RenderContext, RenderDevice, RenderQueue, ViewQuery};
use bevy::render::view::ViewTarget;
use bevy::render::{Render, RenderApp, RenderStartup, RenderSystems};
use bevy::shader::Shader;

use bevy::render::extract_component::{ExtractComponent, ExtractComponentPlugin};

use super::UiCamera;

#[derive(Component, Clone, Copy, Default, ExtractComponent)]
#[extract_app(RenderApp)]
pub struct FadeCamera;

type FadeViews = Or<(With<UiCamera>, With<FadeCamera>)>;

#[derive(Resource, Clone, Copy, Debug, PartialEq, ExtractResource)]
#[extract_app(RenderApp)]
pub struct ScreenFade {
    pub alpha: f32,
    pub grey: f32,
}

impl Default for ScreenFade {
    fn default() -> Self {
        ScreenFade { alpha: 0.0, grey: 1.0 }
    }
}

impl ScreenFade {
    pub fn set(&mut self, v: [f32; 2]) {
        let new = ScreenFade { alpha: v[0], grey: v[1] };
        if *self != new {
            *self = new;
        }
    }
}

#[derive(Resource)]
struct FadeShader(Handle<Shader>);

#[derive(Resource)]
struct FadeGpu {
    layout: BindGroupLayoutDescriptor,
    shader: Handle<Shader>,
    pipelines: HashMap<TextureFormat, CachedRenderPipelineId>,
    uniform: Option<Buffer>,
    draw: bool,
}

pub(super) fn build(app: &mut App) {
    app.add_plugins((ExtractResourcePlugin::<ScreenFade>::default(), ExtractComponentPlugin::<FadeCamera>::default()))
        .init_resource::<ScreenFade>();
    let shader = app
        .world_mut()
        .resource_mut::<Assets<Shader>>()
        .add(Shader::from_wgsl(include_str!("screen_fade.wgsl"), "ui/screen_fade.wgsl"));
    let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
        return;
    };
    render_app
        .insert_resource(FadeShader(shader))
        .add_systems(RenderStartup, init)
        .add_systems(Render, prepare.in_set(RenderSystems::PrepareResources))
        .add_systems(Core2d, fade_pass.in_set(Core2dSystems::PostProcess).after(super::UiPassSystems))
        .add_systems(Core3d, fade_pass.after(super::UiPassSystems).before(upscaling));
}

fn init(mut commands: Commands, shader: Res<FadeShader>, cache: Res<PipelineCache>) {
    let frag = ShaderStages::FRAGMENT;
    let unfiltered = TextureSampleType::Float { filterable: false };
    let layout = BindGroupLayoutDescriptor::new(
        "ssx_screen_fade",
        &BindGroupLayoutEntries::sequential(frag, (uniform_buffer_sized(false, None), texture_2d(unfiltered))),
    );
    let pipelines = [TextureFormat::Rgba8UnormSrgb, TextureFormat::Rgba16Float]
        .into_iter()
        .map(|f| (f, fade_pipeline(&cache, &layout, &shader.0, f)))
        .collect();
    commands.insert_resource(FadeGpu { layout, shader: shader.0.clone(), pipelines, uniform: None, draw: false });
}

fn prepare(
    fade: Option<Res<ScreenFade>>,
    views: Query<&ViewTarget, FadeViews>,
    device: Res<RenderDevice>,
    queue: Res<RenderQueue>,
    cache: Res<PipelineCache>,
    gpu: Option<ResMut<FadeGpu>>,
) {
    let Some(mut gpu) = gpu else { return };
    for target in &views {
        let format = target.main_texture_format();
        if !gpu.pipelines.contains_key(&format) {
            let id = fade_pipeline(&cache, &gpu.layout, &gpu.shader, format);
            gpu.pipelines.insert(format, id);
        }
    }
    let fade = fade.map_or_else(ScreenFade::default, |f| *f);
    gpu.draw = fade.alpha > 0.0;
    if !gpu.draw {
        return;
    }
    let data: Vec<u8> = [fade.alpha, fade.grey, 0.0, 0.0].iter().flat_map(|f| f.to_le_bytes()).collect();
    match gpu.uniform.as_ref() {
        Some(b) => queue.write_buffer(b, 0, &data),
        None => {
            let b = device.create_buffer(&BufferDescriptor {
                label: Some("ssx_screen_fade_uniform"),
                size: data.len() as u64,
                usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            queue.write_buffer(&b, 0, &data);
            gpu.uniform = Some(b);
        }
    }
}

fn fade_pipeline(
    cache: &PipelineCache,
    layout: &BindGroupLayoutDescriptor,
    shader: &Handle<Shader>,
    format: TextureFormat,
) -> CachedRenderPipelineId {
    cache.queue_render_pipeline(RenderPipelineDescriptor {
        label: Some("ssx_screen_fade".into()),
        layout: vec![layout.clone()],
        vertex: VertexState {
            shader: shader.clone(),
            shader_defs: Vec::new(),
            entry_point: Some("vs_full".into()),
            buffers: Vec::new(),
            constants: Vec::new(),
        },
        fragment: Some(FragmentState {
            shader: shader.clone(),
            entry_point: Some("fs_fade".into()),
            targets: vec![Some(ColorTargetState { format, blend: None, write_mask: ColorWrites::ALL })],
            ..default()
        }),
        ..default()
    })
}

fn fade_pass(
    view: ViewQuery<&ViewTarget, FadeViews>,
    gpu: Option<Res<FadeGpu>>,
    cache: Res<PipelineCache>,
    mut ctx: RenderContext,
) {
    let Some(gpu) = gpu else { return };
    if !gpu.draw {
        return;
    }
    let target = view.into_inner();
    let Some(uniform) = gpu.uniform.as_ref() else { return };
    let Some(pipeline) =
        gpu.pipelines.get(&target.main_texture_format()).and_then(|id| cache.get_render_pipeline(*id).cloned())
    else {
        return;
    };
    let device = ctx.render_device().clone();
    let post = target.post_process_write();
    let group = device.create_bind_group(
        "ssx_screen_fade",
        &cache.get_bind_group_layout(&gpu.layout),
        &BindGroupEntries::sequential((uniform.as_entire_binding(), post.source)),
    );
    let mut pass = ctx.command_encoder().begin_render_pass(&RenderPassDescriptor {
        label: Some("ssx_screen_fade"),
        color_attachments: &[Some(RenderPassColorAttachment {
            view: post.destination,
            depth_slice: None,
            resolve_target: None,
            ops: Operations { load: LoadOp::Load, store: StoreOp::Store },
        })],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    });
    pass.set_pipeline(&pipeline);
    pass.set_bind_group(0, &group, &[]);
    pass.draw(0..3, 0..1);
}
