use bevy::core_pipeline::{Core2d, Core2dSystems, FullscreenShader};
use bevy::platform::collections::HashMap;
use bevy::prelude::*;
use bevy::render::extract_component::{ExtractComponent, ExtractComponentPlugin};
use bevy::render::render_asset::RenderAssets;
use bevy::render::render_resource::binding_types::{sampler, texture_2d, uniform_buffer};
use bevy::render::render_resource::*;
use bevy::render::renderer::{RenderContext, RenderDevice, RenderQueue, ViewQuery};
use bevy::render::texture::GpuImage;
use bevy::render::view::ViewTarget;
use bevy::render::{Render, RenderApp, RenderStartup, RenderSystems};
use bevy::shader::Shader;

use crate::render::bind_cache::BindGroupCache;
use crate::render::buffers::QueuedUniform;

const SHADER: &str = "embedded://ssx_bevy/render/post/post_fsr.wesl";
const EASU_FORMAT: TextureFormat = TextureFormat::Rgba8Unorm;
const HDR_FORMAT: TextureFormat = TextureFormat::Rgba16Float;
pub const SCENE_FORMAT: TextureFormat = TextureFormat::Rgba8Unorm;

// PORT(choice): 0.2 stops, the default Xenia's FSR presenter keeps beside its reference to AMD's
// FidelityFX-FSR-Overview-Integration.pdf (`xenia/ui/presenter.h`, kFsrSharpnessReductionDefault; ffx_fsr1.h itself
// names only the scale: 0 the maximum, each stop half as sharp); the console's scaler had no sharpening setting.
pub const RCAS_SHARPNESS_STOPS: f32 = 0.2;

#[derive(Component, Clone, Copy, Debug, ExtractComponent)]
#[extract_app(RenderApp)]
pub struct ScaledScene;

#[derive(Component, Clone, Debug, ExtractComponent)]
#[extract_app(RenderApp)]
pub struct FsrUpscale {
    pub input: Handle<Image>,
    pub sharpness: f32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, ShaderType)]
pub struct FsrUniform {
    pub con0: Vec4,
    pub con1: Vec4,
    pub con2: Vec4,
    pub con3: Vec4,
    pub rcas: Vec4,
}

impl FsrUniform {
    pub fn new(input: UVec2, output: UVec2, sharpness: f32) -> FsrUniform {
        let (vp, size, out) = (input.as_vec2(), input.as_vec2(), output.as_vec2());
        let rcp = Vec2::ONE / size;
        FsrUniform {
            con0: Vec4::new(vp.x / out.x, vp.y / out.y, 0.5 * vp.x / out.x - 0.5, 0.5 * vp.y / out.y - 0.5),
            con1: Vec4::new(rcp.x, rcp.y, rcp.x, -rcp.y),
            con2: Vec4::new(-rcp.x, 2.0 * rcp.y, rcp.x, 2.0 * rcp.y),
            con3: Vec4::new(0.0, 4.0 * rcp.y, 0.0, 0.0),
            rcas: Vec4::new((-sharpness).exp2(), 0.0, 0.0, 0.0),
        }
    }
}

pub(super) fn build(app: &mut App) {
    bevy::asset::embedded_asset!(app, "post_fsr.wesl");
    app.add_plugins((
        ExtractComponentPlugin::<ScaledScene>::default(),
        ExtractComponentPlugin::<FsrUpscale>::default(),
    ));
    let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
        return;
    };
    render_app
        .init_resource::<FsrTargets>()
        .add_systems(RenderStartup, init_pipelines)
        .add_systems(Render, prepare.in_set(RenderSystems::PrepareResources))
        .add_systems(Core2d, upscale.in_set(Core2dSystems::Prepass));
}

#[derive(Resource)]
struct FsrPipelines {
    layout: BindGroupLayoutDescriptor,
    sampler: Sampler,
    easu: CachedRenderPipelineId,
    rcas: CachedRenderPipelineId,
}

fn init_pipelines(
    mut commands: Commands,
    device: Res<RenderDevice>,
    assets: Res<AssetServer>,
    fullscreen: Res<FullscreenShader>,
    cache: Res<PipelineCache>,
) {
    let shader = assets.load::<Shader>(SHADER);
    let layout = BindGroupLayoutDescriptor::new(
        "ssx_fsr",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::FRAGMENT,
            (
                uniform_buffer::<FsrUniform>(false),
                texture_2d(TextureSampleType::Float { filterable: true }),
                sampler(SamplerBindingType::Filtering),
            ),
        ),
    );
    let pipeline = |label: &'static str, entry: &'static str, format| {
        cache.queue_render_pipeline(RenderPipelineDescriptor {
            label: Some(label.into()),
            layout: vec![layout.clone()],
            vertex: fullscreen.to_vertex_state(),
            fragment: Some(FragmentState {
                shader: shader.clone(),
                entry_point: Some(entry.into()),
                targets: vec![Some(ColorTargetState { format, blend: None, write_mask: ColorWrites::ALL })],
                ..default()
            }),
            ..default()
        })
    };
    let pipelines = FsrPipelines {
        sampler: device.create_sampler(&SamplerDescriptor {
            label: Some("ssx_fsr_clamp"),
            mag_filter: FilterMode::Linear,
            min_filter: FilterMode::Linear,
            ..default()
        }),
        easu: pipeline("ssx_fsr_easu", "easu_pass", EASU_FORMAT),
        rcas: pipeline("ssx_fsr_rcas", "rcas_pass", HDR_FORMAT),
        layout,
    };
    commands.insert_resource(pipelines);
}

struct FsrView {
    size: UVec2,
    easu: TextureView,
    uniform: QueuedUniform<FsrUniform>,
}

#[derive(Resource, Default)]
struct FsrTargets(HashMap<Entity, FsrView>);

fn prepare(
    views: Query<(Entity, &ViewTarget, &FsrUpscale)>,
    images: Res<RenderAssets<GpuImage>>,
    device: Res<RenderDevice>,
    queue: Res<RenderQueue>,
    mut targets: ResMut<FsrTargets>,
) {
    targets.0.retain(|e, _| views.contains(*e));
    for (entity, view, fsr) in &views {
        let Some(input) = images.get(&fsr.input) else { continue };
        let s = view.main_texture().size();
        let size = UVec2::new(s.width, s.height);
        if targets.0.get(&entity).is_none_or(|t| t.size != size) {
            let easu = device
                .create_texture(&TextureDescriptor {
                    label: Some("ssx_fsr_easu"),
                    size: Extent3d { width: size.x, height: size.y, depth_or_array_layers: 1 },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: TextureDimension::D2,
                    format: EASU_FORMAT,
                    usage: TextureUsages::TEXTURE_BINDING | TextureUsages::RENDER_ATTACHMENT,
                    view_formats: &[],
                })
                .create_view(&TextureViewDescriptor::default());
            targets.0.insert(entity, FsrView { size, easu, uniform: QueuedUniform::default() });
        }
        let Some(t) = targets.0.get_mut(&entity) else { continue };
        let value = FsrUniform::new(input.size_2d(), size, fsr.sharpness);
        t.uniform.write(&device, &queue, "ssx_fsr_uniform", &value);
    }
}

fn upscale(
    view: ViewQuery<(&ViewTarget, &FsrUpscale)>,
    pipelines: Option<Res<FsrPipelines>>,
    targets: Res<FsrTargets>,
    images: Res<RenderAssets<GpuImage>>,
    cache: Res<PipelineCache>,
    mut binds: Local<BindGroupCache>,
    mut ctx: RenderContext,
) {
    let Some(p) = pipelines else { return };
    let Some(t) = targets.0.get(&view.entity()) else { return };
    let (target, fsr) = view.into_inner();
    if target.main_texture_format() != HDR_FORMAT {
        return;
    }
    let Some(input) = images.get(&fsr.input) else { return };
    let (Some(easu), Some(rcas)) = (cache.get_render_pipeline(p.easu), cache.get_render_pipeline(p.rcas)) else {
        return;
    };
    let device = ctx.render_device().clone();
    let layout = cache.get_bind_group_layout(&p.layout);
    binds.next_frame();
    let easu_bg = binds.get(
        &device,
        "ssx_fsr_easu",
        &layout,
        &BindGroupEntries::sequential((&t.uniform, &input.texture_view, &p.sampler)),
    );
    let rcas_bg =
        binds.get(&device, "ssx_fsr_rcas", &layout, &BindGroupEntries::sequential((&t.uniform, &t.easu, &p.sampler)));
    super::render::fullscreen_pass(&mut ctx, "ssx_fsr_easu", &t.easu, easu, &easu_bg);
    super::render::fullscreen_pass(&mut ctx, "ssx_fsr_rcas", target.main_texture_view(), rcas, &rcas_bg);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_constants_are_amds() {
        let u = FsrUniform::new(UVec2::new(1477, 831), UVec2::new(2560, 1440), 0.2);
        let (sx, sy) = (1477.0 / 2560.0, 831.0 / 1440.0);
        assert!(u.con0.abs_diff_eq(Vec4::new(sx, sy, 0.5 * sx - 0.5, 0.5 * sy - 0.5), 1e-7));
        let (rx, ry) = (1.0 / 1477.0, 1.0 / 831.0);
        assert!(u.con1.abs_diff_eq(Vec4::new(rx, ry, rx, -ry), 1e-9));
        assert!(u.con2.abs_diff_eq(Vec4::new(-rx, 2.0 * ry, rx, 2.0 * ry), 1e-9));
        assert!(u.con3.abs_diff_eq(Vec4::new(0.0, 4.0 * ry, 0.0, 0.0), 1e-9));
        assert!((u.rcas.x - 0.870_550_6).abs() < 1e-6);
        let p0 = 0.0 * u.con0.x + u.con0.z;
        assert!((p0 - (0.5 * sx - 0.5)).abs() < 1e-7);
    }
}
