use bevy::core_pipeline::FullscreenShader;
use bevy::prelude::*;
use bevy::render::render_resource::binding_types::{
    sampler, storage_buffer_read_only_sized, texture_2d, texture_depth_2d, uniform_buffer,
};
use bevy::render::render_resource::*;
use bevy::render::renderer::RenderDevice;
use bevy::shader::{Shader, ShaderDefVal};

use super::{COLOUR_FORMAT, DEPTH_COPY_FORMAT, DEPTH_FORMAT, DrawUniform, Globals};
use crate::render::post::PostUniform;

#[derive(Resource)]
pub(super) struct ParticlePipelines {
    pub(super) draw_layout: BindGroupLayoutDescriptor,
    pub(super) pass_layout: BindGroupLayoutDescriptor,
    pub(super) techs: Vec<CachedRenderPipelineId>,
    pub(super) depth_filter: CachedRenderPipelineId,
    pub(super) depth_copy: CachedRenderPipelineId,
    pub(super) tonemap: CachedRenderPipelineId,
    pub(super) bloom: CachedRenderPipelineId,
    pub(super) composite: CachedRenderPipelineId,
    pub(super) texture_sampler: Sampler,
    pub(super) smooth: Sampler,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Tech {
    Spray,
    LynxAdd,
    LynxAlpha,
    SnowRamp,
    SnowAlpha,
    InstanceRamp,
    Snowfall,
    Ribbon,
    Avalanche,
    SprayRamp,
    RibbonAlpha,
    Glare,
    ZFeatherAdd,
    ZFeatherAlpha,
}

pub(super) const TECHS: [Tech; 14] = [
    Tech::Spray,
    Tech::LynxAdd,
    Tech::LynxAlpha,
    Tech::SnowRamp,
    Tech::SnowAlpha,
    Tech::InstanceRamp,
    Tech::Snowfall,
    Tech::Ribbon,
    Tech::Avalanche,
    Tech::SprayRamp,
    Tech::RibbonAlpha,
    Tech::Glare,
    Tech::ZFeatherAdd,
    Tech::ZFeatherAlpha,
];

impl Tech {
    pub(super) fn of(shader: &str, technique: &str) -> Option<Tech> {
        Some(match (shader, technique) {
            ("lynx_snowspray_zfeather.fx", "AlphaBlend_ContactShadow_RampColour") => Tech::Spray,
            ("lynx.fx", "AdditiveBlend") => Tech::LynxAdd,
            ("lynx.fx", "AlphaBlend") => Tech::LynxAlpha,
            ("lynx_snow.fx", "AlphaBlend_RampColour") => Tech::SnowRamp,
            ("lynx_snow.fx", "AlphaBlend") => Tech::SnowAlpha,
            ("lynx_instance.fx", "AlphaBlend_RampColour") => Tech::InstanceRamp,
            ("lynx_snowfall.fx", "AlphaBlend") => Tech::Snowfall,
            ("lynx_ribbon.fx", "AdditiveBlend") => Tech::Ribbon,
            ("lynx_avalanche.fx", "AlphaBlend_ContactShadow_RampColour") => Tech::Avalanche,
            ("lynx_snowspray_zfeather.fx", "AlphaBlend_RampColour") => Tech::SprayRamp,
            ("lynx_ribbon.fx", "AlphaBlend") => Tech::RibbonAlpha,
            ("lynx_glare_occluded.fx", "AdditiveBlend") => Tech::Glare,
            ("lynx_zfeather.fx", "AdditiveBlend") => Tech::ZFeatherAdd,
            ("lynx_zfeather.fx", "AlphaBlend") => Tech::ZFeatherAlpha,
            _ => return None,
        })
    }

    pub(super) fn is_ribbon(self) -> bool {
        matches!(self, Tech::Ribbon | Tech::RibbonAlpha)
    }

    fn program(self) -> (&'static str, &'static str, &'static [&'static str], bool) {
        match self {
            Tech::Spray => ("vs_particle", "fs_spray", &["SPRAY"], false),
            Tech::LynxAdd => ("vs_particle", "fs_additive", &["LYNX"], true),
            Tech::LynxAlpha => ("vs_particle", "fs_lynx_alpha", &["LYNX"], false),
            Tech::SnowRamp => ("vs_particle", "fs_snow_ramp", &["SNOW"], false),
            Tech::SnowAlpha => ("vs_particle", "fs_snow_alpha", &["SNOW"], false),
            Tech::InstanceRamp => ("vs_particle", "fs_instance_ramp", &["INSTANCE"], false),
            Tech::Snowfall => ("vs_particle", "fs_snowfall", &["SNOWFALL"], false),
            Tech::Ribbon => ("vs_ribbon", "fs_ribbon", &["RIBBON"], true),
            Tech::Avalanche => ("vs_particle", "fs_avalanche", &["SPRAY"], false),
            Tech::SprayRamp => ("vs_particle", "fs_spray_ramp", &["SPRAY"], false),
            Tech::RibbonAlpha => ("vs_ribbon", "fs_ribbon_alpha", &["RIBBON"], false),
            Tech::Glare => ("vs_glare", "fs_glare", &["GLARE"], true),
            Tech::ZFeatherAdd => ("vs_particle", "fs_zfeather_additive", &["LYNX", "ZFEATHER"], true),
            Tech::ZFeatherAlpha => ("vs_particle", "fs_zfeather_alpha", &["LYNX", "ZFEATHER"], false),
        }
    }
}

fn blend(src: BlendFactor, dst: BlendFactor) -> BlendComponent {
    BlendComponent { src_factor: src, dst_factor: dst, operation: BlendOperation::Add }
}

pub(super) fn init_pipelines(
    mut commands: Commands,
    device: Res<RenderDevice>,
    assets: Res<AssetServer>,
    fullscreen: Res<FullscreenShader>,
    cache: Res<PipelineCache>,
) {
    let draw_shader: Handle<Shader> = assets.load("embedded://ssx_bevy/particles/render/particles.wesl");
    let pass_shader: Handle<Shader> = assets.load("embedded://ssx_bevy/particles/render/particles_pass.wesl");
    let float = TextureSampleType::Float { filterable: true };
    let unfiltered = TextureSampleType::Float { filterable: false };
    let both = ShaderStages::VERTEX_FRAGMENT;
    let draw_layout = BindGroupLayoutDescriptor::new(
        "ssx_particles_draw",
        &BindGroupLayoutEntries::sequential(
            both,
            (
                uniform_buffer::<Globals>(false),
                uniform_buffer::<DrawUniform>(true),
                storage_buffer_read_only_sized(false, None),
                texture_2d(float),
                sampler(SamplerBindingType::Filtering),
                texture_2d(unfiltered),
            ),
        ),
    );
    let pass_layout = BindGroupLayoutDescriptor::new(
        "ssx_particles_pass",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::FRAGMENT,
            (
                uniform_buffer::<PostUniform>(false),
                uniform_buffer::<Globals>(false),
                texture_2d(float),
                sampler(SamplerBindingType::Filtering),
                storage_buffer_read_only_sized(false, None),
                texture_depth_2d(),
                texture_2d(unfiltered),
            ),
        ),
    );
    let particle_depth = Some(DepthStencilState {
        format: DEPTH_FORMAT,
        depth_write_enabled: Some(false),
        depth_compare: Some(CompareFunction::GreaterEqual),
        stencil: StencilState::default(),
        bias: DepthBiasState::default(),
    });
    let draw = |vs: &'static str, entry: &'static str, defs: Vec<ShaderDefVal>, state: BlendState, strip: bool| {
        cache.queue_render_pipeline(RenderPipelineDescriptor {
            label: Some("ssx_particles_draw".into()),
            layout: vec![draw_layout.clone()],
            vertex: VertexState {
                shader: draw_shader.clone(),
                shader_defs: defs.clone(),
                entry_point: Some(vs.into()),
                buffers: vec![],
                constants: vec![],
            },
            fragment: Some(FragmentState {
                shader: draw_shader.clone(),
                shader_defs: defs,
                entry_point: Some(entry.into()),
                targets: vec![Some(ColorTargetState {
                    format: COLOUR_FORMAT,
                    blend: Some(state),
                    write_mask: ColorWrites::ALL,
                })],
                constants: vec![],
            }),
            primitive: PrimitiveState {
                cull_mode: None,
                topology: if strip { PrimitiveTopology::TriangleStrip } else { PrimitiveTopology::TriangleList },
                ..default()
            },
            depth_stencil: particle_depth.clone(),
            ..default()
        })
    };
    let pass = |label: &'static str, entry: &'static str, format: TextureFormat, state: Option<BlendState>, depth| {
        cache.queue_render_pipeline(RenderPipelineDescriptor {
            label: Some(label.into()),
            layout: vec![pass_layout.clone()],
            vertex: fullscreen.to_vertex_state(),
            fragment: Some(FragmentState {
                shader: pass_shader.clone(),
                entry_point: Some(entry.into()),
                targets: vec![Some(ColorTargetState { format, blend: state, write_mask: ColorWrites::ALL })],
                ..default()
            }),
            depth_stencil: depth,
            ..default()
        })
    };
    use BlendFactor::{One, OneMinusSrcAlpha, SrcAlpha, Zero};
    let alpha_blend = BlendState { color: blend(SrcAlpha, OneMinusSrcAlpha), alpha: blend(Zero, OneMinusSrcAlpha) };
    let additive = BlendState { color: blend(One, One), alpha: blend(Zero, One) };
    let over = BlendState { color: blend(One, SrcAlpha), alpha: blend(One, SrcAlpha) };
    let write_depth = Some(DepthStencilState {
        format: DEPTH_FORMAT,
        depth_write_enabled: Some(true),
        depth_compare: Some(CompareFunction::Always),
        stencil: StencilState::default(),
        bias: DepthBiasState::default(),
    });
    let linear = |mipmap| SamplerDescriptor {
        address_mode_u: AddressMode::ClampToEdge,
        address_mode_v: AddressMode::ClampToEdge,
        mag_filter: FilterMode::Linear,
        min_filter: FilterMode::Linear,
        mipmap_filter: mipmap,
        ..default()
    };
    let pipelines = ParticlePipelines {
        techs: TECHS
            .iter()
            .map(|t| {
                let (vs, fs, defs, add) = t.program();
                let defs = defs.iter().map(|d| (*d).into()).collect();
                draw(vs, fs, defs, if add { additive } else { alpha_blend }, t.is_ribbon())
            })
            .collect(),
        depth_filter: pass("ssx_particles_depth_filter", "depth_filter", DEPTH_COPY_FORMAT, None, write_depth),
        depth_copy: pass("ssx_particles_depth_copy", "depth_copy", DEPTH_COPY_FORMAT, None, None),
        tonemap: pass("ssx_particles_tonemap", "tonemap", COLOUR_FORMAT, None, None),
        bloom: pass("ssx_particles_bloom", "bloom", COLOUR_FORMAT, Some(over), None),
        composite: pass("ssx_particles_composite", "composite", COLOUR_FORMAT, Some(over), None),
        texture_sampler: device.create_sampler(&linear(MipmapFilterMode::Linear)),
        smooth: device.create_sampler(&linear(MipmapFilterMode::Nearest)),
        draw_layout,
        pass_layout,
    };
    commands.insert_resource(pipelines);
}
