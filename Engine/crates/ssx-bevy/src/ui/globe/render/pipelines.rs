use std::collections::HashMap;

use bevy::image::{ImageAddressMode, ImageFilterMode, ImageSamplerBorderColor, ImageSamplerDescriptor};
use bevy::mesh::VertexBufferLayout;
use bevy::prelude::*;
use bevy::render::render_resource::binding_types::{
    sampler, storage_buffer_read_only_sized, storage_buffer_sized, texture_2d, texture_3d, uniform_buffer_sized,
};
use bevy::render::render_resource::*;
use bevy::render::renderer::RenderDevice;
use bevy::shader::Shader;

use super::{DEPTH_FORMAT, GlobeShaders, HDR_FORMAT, LDR_FORMAT};

#[derive(Resource)]
pub(super) struct Pipelines {
    pub(super) globe_layout: BindGroupLayoutDescriptor,
    pub(super) mg_layout: BindGroupLayoutDescriptor,
    pub(super) backdrop_layout: BindGroupLayoutDescriptor,
    pub(super) quarter_layout: BindGroupLayoutDescriptor,
    pub(super) compute_layout: BindGroupLayoutDescriptor,
    pub(super) tone_layout: BindGroupLayoutDescriptor,
    pub(super) filter_layout: BindGroupLayoutDescriptor,
    pub(super) composite_layout: BindGroupLayoutDescriptor,
    pub(super) fade_layout: BindGroupLayoutDescriptor,
    pub(super) cloud_layout: BindGroupLayoutDescriptor,
    pub(super) depth_filter_layout: BindGroupLayoutDescriptor,
    pub(super) cloud_composite_layout: BindGroupLayoutDescriptor,
    pub(super) wave_layout: BindGroupLayoutDescriptor,
    pub(super) wave: CachedRenderPipelineId,
    pub(super) cloud: CachedRenderPipelineId,
    pub(super) depth_filter: CachedRenderPipelineId,
    pub(super) cloud_composite: CachedRenderPipelineId,
    pub(super) midground: CachedRenderPipelineId,
    pub(super) earth: CachedRenderPipelineId,
    pub(super) halo: CachedRenderPipelineId,
    pub(super) background: CachedRenderPipelineId,
    pub(super) background_movie: CachedRenderPipelineId,
    pub(super) sky: CachedRenderPipelineId,
    pub(super) quarter: CachedRenderPipelineId,
    pub(super) luminance: CachedComputePipelineId,
    pub(super) reduce: CachedComputePipelineId,
    pub(super) threshold: CachedRenderPipelineId,
    pub(super) tonemap: CachedRenderPipelineId,
    pub(super) fxaa: CachedRenderPipelineId,
    pub(super) blur_h: CachedRenderPipelineId,
    pub(super) blur_v: CachedRenderPipelineId,
    pub(super) composite: HashMap<TextureFormat, CachedRenderPipelineId>,
    pub(super) fade: HashMap<TextureFormat, CachedRenderPipelineId>,
    pub(super) wrap_linear: Sampler,
    pub(super) clamp_point: Sampler,
    pub(super) clamp_linear: Sampler,
    pub(super) wrap_trilinear: Sampler,
    pub(super) border_trilinear: Sampler,
    pub(super) sky_cloud: Sampler,
    pub(super) particle: Sampler,
    pub(super) post: Handle<Shader>,
    pub(super) fade_shader: Handle<Shader>,
}

fn image_sampler(d: ImageSamplerDescriptor, device: &RenderDevice) -> Sampler {
    device.create_sampler(&d.as_wgpu())
}

pub(super) fn init(
    mut commands: Commands,
    device: Res<RenderDevice>,
    shaders: Res<GlobeShaders>,
    cache: Res<PipelineCache>,
) {
    let frag = ShaderStages::FRAGMENT;
    let both = ShaderStages::VERTEX_FRAGMENT;
    let float = TextureSampleType::Float { filterable: true };
    let unfiltered = TextureSampleType::Float { filterable: false };
    let globe_layout = BindGroupLayoutDescriptor::new(
        "ssx_globe",
        &BindGroupLayoutEntries::sequential(
            both,
            (
                uniform_buffer_sized(false, None),
                texture_2d(float),
                texture_2d(float),
                texture_2d(unfiltered),
                sampler(SamplerBindingType::Filtering),
                sampler(SamplerBindingType::NonFiltering),
            ),
        ),
    );
    let backdrop_layout = BindGroupLayoutDescriptor::new(
        "ssx_globe_backdrop",
        &BindGroupLayoutEntries::sequential(
            frag,
            (
                uniform_buffer_sized(false, None),
                texture_2d(float),
                texture_2d(float),
                texture_2d(float),
                texture_2d(float),
                sampler(SamplerBindingType::Filtering),
                sampler(SamplerBindingType::Filtering),
            ),
        ),
    );
    let quarter_layout = BindGroupLayoutDescriptor::new(
        "ssx_globe_quarter",
        &BindGroupLayoutEntries::sequential(frag, (uniform_buffer_sized(false, None), texture_2d(unfiltered))),
    );
    let compute_layout = BindGroupLayoutDescriptor::new(
        "ssx_globe_exposure",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::COMPUTE,
            (
                uniform_buffer_sized(false, None),
                texture_2d(unfiltered),
                storage_buffer_sized(false, None),
                storage_buffer_sized(false, None),
            ),
        ),
    );
    let tone_layout = BindGroupLayoutDescriptor::new(
        "ssx_globe_tone",
        &BindGroupLayoutEntries::with_indices(
            frag,
            (
                (0, uniform_buffer_sized(false, None)),
                (1, texture_2d(unfiltered)),
                (4, storage_buffer_read_only_sized(false, None)),
            ),
        ),
    );
    let filter_layout = BindGroupLayoutDescriptor::new(
        "ssx_globe_filter",
        &BindGroupLayoutEntries::with_indices(
            frag,
            (
                (0, uniform_buffer_sized(false, None)),
                (1, texture_2d(float)),
                (5, sampler(SamplerBindingType::Filtering)),
            ),
        ),
    );
    let composite_layout = BindGroupLayoutDescriptor::new(
        "ssx_globe_composite",
        &BindGroupLayoutEntries::with_indices(
            frag,
            (
                (0, uniform_buffer_sized(false, None)),
                (1, texture_2d(unfiltered)),
                (5, sampler(SamplerBindingType::Filtering)),
                (6, texture_2d(float)),
                (7, texture_3d(float)),
            ),
        ),
    );
    let cloud_layout = BindGroupLayoutDescriptor::new(
        "ssx_globe_clouds",
        &BindGroupLayoutEntries::with_indices(
            both,
            (
                (0, uniform_buffer_sized(false, None)),
                (1, storage_buffer_read_only_sized(false, None)),
                (2, texture_2d(float)),
                (3, sampler(SamplerBindingType::Filtering)),
                (4, texture_2d(unfiltered)),
            ),
        ),
    );
    let depth_filter_layout = BindGroupLayoutDescriptor::new(
        "ssx_globe_depth_filter",
        &BindGroupLayoutEntries::with_indices(
            frag,
            (
                (0, uniform_buffer_sized(false, None)),
                (5, bevy::render::render_resource::binding_types::texture_depth_2d()),
            ),
        ),
    );
    let cloud_composite_layout = BindGroupLayoutDescriptor::new(
        "ssx_globe_cloud_composite",
        &BindGroupLayoutEntries::with_indices(
            frag,
            (
                (0, uniform_buffer_sized(false, None)),
                (4, texture_2d(unfiltered)),
                (5, bevy::render::render_resource::binding_types::texture_depth_2d()),
                (6, texture_2d(float)),
                (7, sampler(SamplerBindingType::Filtering)),
            ),
        ),
    );
    let wave_layout = BindGroupLayoutDescriptor::new(
        "ssx_globe_waves",
        &BindGroupLayoutEntries::sequential(
            both,
            (
                uniform_buffer_sized(false, None),
                texture_2d(float),
                texture_2d(float),
                texture_2d(float),
                sampler(SamplerBindingType::Filtering),
                storage_buffer_read_only_sized(false, None),
            ),
        ),
    );
    let fade_layout = BindGroupLayoutDescriptor::new(
        "ssx_screen_fade",
        &BindGroupLayoutEntries::sequential(frag, (uniform_buffer_sized(false, None), texture_2d(unfiltered))),
    );
    let attr = |location: u32, format: VertexFormat, stride: u64| VertexBufferLayout {
        array_stride: stride,
        step_mode: VertexStepMode::Vertex,
        attributes: vec![VertexAttribute { format, offset: 0, shader_location: location }],
    };
    let draw = |label: &'static str, vs: &'static str, fs: &'static str, buffers: Vec<VertexBufferLayout>| {
        cache.queue_render_pipeline(RenderPipelineDescriptor {
            label: Some(label.into()),
            layout: vec![globe_layout.clone()],
            vertex: VertexState {
                shader: shaders.globe.clone(),
                shader_defs: Vec::new(),
                entry_point: Some(vs.into()),
                buffers,
                constants: Vec::new(),
            },
            fragment: Some(FragmentState {
                shader: shaders.globe.clone(),
                entry_point: Some(fs.into()),
                targets: vec![Some(ColorTargetState { format: HDR_FORMAT, blend: None, write_mask: ColorWrites::ALL })],
                ..default()
            }),
            primitive: PrimitiveState { cull_mode: None, ..default() },
            ..default()
        })
    };
    let earth = draw("ssx_globe_earth", "earth_vs", "earth_fs", vec![attr(0, VertexFormat::Float32x3, 12)]);
    let mg_layout = BindGroupLayoutDescriptor::new(
        "ssx_globe_midground",
        &BindGroupLayoutEntries::sequential(
            both,
            (
                uniform_buffer_sized(false, None),
                texture_2d(float),
                texture_2d(float),
                texture_2d(float),
                texture_2d(float),
                texture_2d(float),
                texture_2d(float),
                texture_2d(float),
                sampler(SamplerBindingType::Filtering),
                sampler(SamplerBindingType::Filtering),
            ),
        ),
    );
    let midground = cache.queue_render_pipeline(RenderPipelineDescriptor {
        label: Some("ssx_globe_midground".into()),
        layout: vec![mg_layout.clone()],
        vertex: VertexState {
            shader: shaders.midground.clone(),
            shader_defs: Vec::new(),
            entry_point: Some("mg_vs".into()),
            buffers: vec![
                attr(0, VertexFormat::Float32x3, 12),
                attr(1, VertexFormat::Float32x3, 12),
                attr(2, VertexFormat::Float32x3, 12),
                attr(3, VertexFormat::Float32x2, 8),
            ],
            constants: Vec::new(),
        },
        fragment: Some(FragmentState {
            shader: shaders.midground.clone(),
            entry_point: Some("mg_fs".into()),
            targets: vec![Some(ColorTargetState { format: HDR_FORMAT, blend: None, write_mask: ColorWrites::ALL })],
            ..default()
        }),
        primitive: PrimitiveState { cull_mode: None, ..default() },
        depth_stencil: Some(DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: Some(true),
            depth_compare: Some(CompareFunction::Less),
            stencil: StencilState::default(),
            bias: DepthBiasState::default(),
        }),
        ..default()
    });
    let halo = draw(
        "ssx_globe_halo",
        "halo_vs",
        "halo_fs",
        vec![attr(0, VertexFormat::Float32x3, 12), attr(1, VertexFormat::Float32x2, 8)],
    );
    let full = |label: &'static str,
                shader: &Handle<Shader>,
                layout: &BindGroupLayoutDescriptor,
                fs: &'static str,
                format: TextureFormat| {
        cache.queue_render_pipeline(RenderPipelineDescriptor {
            label: Some(label.into()),
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
                entry_point: Some(fs.into()),
                targets: vec![Some(ColorTargetState { format, blend: None, write_mask: ColorWrites::ALL })],
                ..default()
            }),
            ..default()
        })
    };
    let background = full("ssx_globe_background", &shaders.backdrop, &backdrop_layout, "fs_background", HDR_FORMAT);
    let background_movie =
        full("ssx_globe_background_movie", &shaders.backdrop, &backdrop_layout, "fs_background_movie", HDR_FORMAT);
    let sky = full("ssx_globe_sky", &shaders.backdrop, &backdrop_layout, "fs_sky", HDR_FORMAT);
    let quarter = full("ssx_globe_quarter", &shaders.post, &quarter_layout, "fs_quarter", HDR_FORMAT);
    let threshold = full("ssx_globe_threshold", &shaders.post, &tone_layout, "fs_threshold", LDR_FORMAT);
    let tonemap = full("ssx_globe_tonemap", &shaders.post, &tone_layout, "fs_tonemap", LDR_FORMAT);
    let fxaa = full("ssx_globe_fxaa", &shaders.post, &filter_layout, "fs_fxaa", LDR_FORMAT);
    let blur_h = full("ssx_globe_blur_h", &shaders.post, &filter_layout, "fs_blur_h", LDR_FORMAT);
    let blur_v = full("ssx_globe_blur_v", &shaders.post, &filter_layout, "fs_blur_v", LDR_FORMAT);
    let cloud = cache.queue_render_pipeline(RenderPipelineDescriptor {
        label: Some("ssx_globe_cloud".into()),
        layout: vec![cloud_layout.clone()],
        vertex: VertexState {
            shader: shaders.clouds.clone(),
            shader_defs: Vec::new(),
            entry_point: Some("vs_cloud".into()),
            buffers: Vec::new(),
            constants: Vec::new(),
        },
        fragment: Some(FragmentState {
            shader: shaders.clouds.clone(),
            entry_point: Some("fs_cloud".into()),
            targets: vec![Some(ColorTargetState {
                format: LDR_FORMAT,
                blend: Some(BlendState {
                    color: BlendComponent {
                        src_factor: BlendFactor::SrcAlpha,
                        dst_factor: BlendFactor::OneMinusSrcAlpha,
                        operation: BlendOperation::Add,
                    },
                    alpha: BlendComponent {
                        src_factor: BlendFactor::Zero,
                        dst_factor: BlendFactor::OneMinusSrcAlpha,
                        operation: BlendOperation::Add,
                    },
                }),
                write_mask: ColorWrites::ALL,
            })],
            ..default()
        }),
        primitive: PrimitiveState { cull_mode: None, ..default() },
        ..default()
    });
    let wave = cache.queue_render_pipeline(RenderPipelineDescriptor {
        label: Some("ssx_globe_waves".into()),
        layout: vec![wave_layout.clone()],
        vertex: VertexState {
            shader: shaders.waves.clone(),
            shader_defs: Vec::new(),
            entry_point: Some("wave_vs".into()),
            buffers: vec![
                attr(0, VertexFormat::Float32x3, 12),
                attr(1, VertexFormat::Float32x3, 12),
                attr(2, VertexFormat::Float32x3, 12),
                attr(3, VertexFormat::Float32x2, 8),
                attr(4, VertexFormat::Unorm8x4, 4),
            ],
            constants: Vec::new(),
        },
        fragment: Some(FragmentState {
            shader: shaders.waves.clone(),
            entry_point: Some("wave_fs".into()),
            targets: vec![Some(ColorTargetState {
                format: LDR_FORMAT,
                blend: Some(BlendState {
                    color: BlendComponent {
                        src_factor: BlendFactor::SrcAlpha,
                        dst_factor: BlendFactor::OneMinusSrcAlpha,
                        operation: BlendOperation::Add,
                    },
                    alpha: BlendComponent::OVER,
                }),
                write_mask: ColorWrites::ALL,
            })],
            ..default()
        }),
        primitive: PrimitiveState { cull_mode: None, ..default() },
        depth_stencil: Some(DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: Some(true),
            depth_compare: Some(CompareFunction::LessEqual),
            stencil: StencilState::default(),
            bias: DepthBiasState::default(),
        }),
        ..default()
    });
    let depth_filter = full(
        "ssx_globe_depth_filter",
        &shaders.clouds,
        &depth_filter_layout,
        "fs_depth_filter",
        TextureFormat::R32Float,
    );
    let cloud_composite = cache.queue_render_pipeline(RenderPipelineDescriptor {
        label: Some("ssx_globe_cloud_composite".into()),
        layout: vec![cloud_composite_layout.clone()],
        vertex: VertexState {
            shader: shaders.clouds.clone(),
            shader_defs: Vec::new(),
            entry_point: Some("vs_full".into()),
            buffers: Vec::new(),
            constants: Vec::new(),
        },
        fragment: Some(FragmentState {
            shader: shaders.clouds.clone(),
            entry_point: Some("fs_composite".into()),
            targets: vec![Some(ColorTargetState {
                format: LDR_FORMAT,
                blend: Some(BlendState {
                    color: BlendComponent {
                        src_factor: BlendFactor::One,
                        dst_factor: BlendFactor::SrcAlpha,
                        operation: BlendOperation::Add,
                    },
                    alpha: BlendComponent {
                        src_factor: BlendFactor::Zero,
                        dst_factor: BlendFactor::One,
                        operation: BlendOperation::Add,
                    },
                }),
                write_mask: ColorWrites::ALL,
            })],
            ..default()
        }),
        ..default()
    });
    let compute = |label: &'static str, entry: &'static str| {
        cache.queue_compute_pipeline(ComputePipelineDescriptor {
            label: Some(label.into()),
            layout: vec![compute_layout.clone()],
            shader: shaders.post.clone(),
            entry_point: Some(entry.into()),
            zero_initialize_workgroup_memory: true,
            ..default()
        })
    };
    let luminance = compute("ssx_globe_luminance", "luminance");
    let reduce = compute("ssx_globe_reduce", "reduce");
    let sampler_desc = |address: AddressMode, filter: FilterMode| SamplerDescriptor {
        label: Some("ssx_globe_sampler"),
        address_mode_u: address,
        address_mode_v: address,
        mag_filter: filter,
        min_filter: filter,
        mipmap_filter: MipmapFilterMode::Nearest,
        ..default()
    };
    let linear = ImageFilterMode::Linear;
    commands.insert_resource(Pipelines {
        globe_layout,
        mg_layout,
        backdrop_layout,
        quarter_layout,
        compute_layout,
        tone_layout,
        filter_layout,
        composite_layout,
        fade_layout,
        cloud_layout,
        depth_filter_layout,
        cloud_composite_layout,
        wave_layout,
        wave,
        cloud,
        depth_filter,
        cloud_composite,
        midground,
        earth,
        halo,
        background,
        background_movie,
        sky,
        quarter,
        luminance,
        reduce,
        threshold,
        tonemap,
        fxaa,
        blur_h,
        blur_v,
        composite: HashMap::new(),
        fade: HashMap::new(),
        wrap_linear: device.create_sampler(&sampler_desc(AddressMode::Repeat, FilterMode::Linear)),
        clamp_point: device.create_sampler(&sampler_desc(AddressMode::ClampToEdge, FilterMode::Nearest)),
        clamp_linear: device.create_sampler(&sampler_desc(AddressMode::ClampToEdge, FilterMode::Linear)),
        wrap_trilinear: device.create_sampler(&SamplerDescriptor {
            mipmap_filter: MipmapFilterMode::Linear,
            ..sampler_desc(AddressMode::Repeat, FilterMode::Linear)
        }),
        border_trilinear: image_sampler(
            ImageSamplerDescriptor {
                label: Some("ssx_globe_border".into()),
                address_mode_u: ImageAddressMode::ClampToBorder,
                address_mode_v: ImageAddressMode::ClampToBorder,
                mag_filter: linear,
                min_filter: linear,
                mipmap_filter: linear,
                border_color: Some(ImageSamplerBorderColor::TransparentBlack),
                ..default()
            },
            &device,
        ),
        sky_cloud: image_sampler(
            ImageSamplerDescriptor {
                label: Some("ssx_globe_sky_cloud".into()),
                address_mode_u: ImageAddressMode::Repeat,
                address_mode_v: ImageAddressMode::ClampToBorder,
                mag_filter: linear,
                min_filter: linear,
                mipmap_filter: ImageFilterMode::Nearest,
                border_color: Some(ImageSamplerBorderColor::TransparentBlack),
                ..default()
            },
            &device,
        ),
        particle: device.create_sampler(&SamplerDescriptor {
            mipmap_filter: MipmapFilterMode::Linear,
            ..sampler_desc(AddressMode::ClampToEdge, FilterMode::Linear)
        }),
        post: shaders.post.clone(),
        fade_shader: shaders.fade.clone(),
    });
}
