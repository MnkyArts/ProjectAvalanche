use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result};
use bevy::prelude::{Assets, Handle, Image, Mat4, Quat, Vec3, Vec4};
use bevy::render::render_resource::{CompareFunction, Face, ShaderType};

use crate::render::lighting::GameLighting;
use crate::render::lighting::game::GameLightUniform;

pub const VIEW_SCALE: f32 = 25.0;

pub const EFFECT: &str = "terrain_midground";
pub const TECHNIQUE: &str = "forward";

pub const TEXTURES: [&str; 6] = [
    "normalTexture",
    "ambOcclusionTexture",
    "maskTexture",
    "redChannelTexture",
    "greenChannelTexture",
    "baseChannelTexture",
];

#[derive(Debug, Default)]
pub struct MeshData {
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub tangents: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    pub indices: Vec<u32>,
}

#[derive(Clone, Copy, Debug, Default, ShaderType)]
pub struct MidgroundPartUniform {
    pub world: Mat4,
    pub light: GameLightUniform,
    pub beta_r: Vec4,
    pub beta_dash_r: Vec4,
    pub beta_m: Vec4,
    pub beta_dash_m: Vec4,
    pub heights: Vec4,
    pub multipliers: Vec4,
    pub sun_dir: Vec4,
    pub sun_colour: Vec4,
    pub tiles: Vec4,
    pub ao_tile: Vec4,
}

#[derive(Clone, Copy, Debug, Default, ShaderType)]
pub struct MidgroundViewUniform {
    pub view_from_world: Mat4,
    pub clip_from_view: Mat4,
    pub camera: Vec4,
}

pub struct PartLoad {
    pub mesh: Arc<MeshData>,
    pub images: Vec<crate::render::uploads::LevelImage>,
    pub uniform: MidgroundPartUniform,
}

pub struct MidgroundLoad {
    pub parts: Vec<PartLoad>,
    pub cull: Option<Face>,
    pub depth: (CompareFunction, bool),
    pub status: String,
}

#[derive(Clone)]
pub struct MidgroundSettings {
    pub parts: Vec<PartSettings>,
    pub cull: Option<Face>,
    pub depth: (CompareFunction, bool),
}

#[derive(Clone)]
pub struct PartSettings {
    pub mesh: Arc<MeshData>,
    pub textures: Vec<Handle<Image>>,
    pub uniform: MidgroundPartUniform,
}

impl MidgroundLoad {
    pub fn queue_images(&mut self, uploads: &mut crate::render::uploads::LevelUploads, images: &mut Assets<Image>) {
        for i in self.parts.iter_mut().flat_map(|p| &mut p.images) {
            i.queue(uploads, images);
        }
    }

    pub fn into_settings(self, images: &mut Assets<Image>) -> MidgroundSettings {
        let parts = self
            .parts
            .into_iter()
            .map(|p| PartSettings {
                mesh: p.mesh,
                textures: p.images.into_iter().map(|i| i.into_handle(images)).collect(),
                uniform: p.uniform,
            })
            .collect();
        MidgroundSettings { parts, cull: self.cull, depth: self.depth }
    }
}

pub fn load(l: &GameLighting, root: &Path) -> Result<Option<MidgroundLoad>> {
    let Some(track) = l.midground.as_deref() else { return Ok(None) };
    let Some(mg) = ssx_assets::track_objects::load_track_midground(root, track)? else { return Ok(None) };
    let world = Mat4::from_scale_rotation_translation(
        Vec3::from_array(mg.scale),
        Quat::from_array(mg.rotation),
        Vec3::from_array(mg.position),
    );
    let lod0 = mg.object.lods.first().map(Vec::as_slice).unwrap_or_default();
    let mut parts = Vec::new();
    let mut skipped = Vec::new();
    for m in lod0 {
        let effect = m.material.effect.as_deref().unwrap_or("-");
        if effect != "terrain_midground.fx" {
            skipped.push(format!("{} ({effect})", m.material.name));
            continue;
        }
        let o = static_mesh(root, &m.geometry)?;
        let n = o.positions.len();
        let mesh = MeshData {
            normals: o.normals.clone().unwrap_or_else(|| vec![[0.0, 1.0, 0.0]; n]),
            tangents: o.tangents.clone().unwrap_or_else(|| vec![[1.0, 0.0, 0.0]; n]),
            uvs: o.uv0.clone().unwrap_or_else(|| vec![[0.0, 0.0]; n]),
            positions: o.positions,
            indices: o.indices,
        };
        let images = TEXTURES
            .iter()
            .map(|p| {
                let rel = m.material.textures.get(*p).with_context(|| format!("{}: no {p}", m.material.name))?;
                super::load_texture(root, rel, l)
            })
            .collect::<Result<Vec<_>>>()?;
        let mv = |name: &str, i: usize| m.material.vectors.get(name).and_then(|v| v.get(i).copied()).unwrap_or(0.0);
        let mut uniform = part_uniform(l, world);
        uniform.tiles = Vec4::new(
            mv("baseChannelTile", 0),
            mv("redChannelTile", 0),
            mv("greenChannelTile", 0),
            m.material.float("snowBias").unwrap_or(0.0),
        );
        uniform.ao_tile = Vec4::new(mv("aoTile", 0), mv("aoTile", 1), 0.0, 0.0);
        let images = images.into_iter().map(|(i, r)| crate::render::uploads::LevelImage::built(i, r)).collect();
        parts.push(PartLoad { mesh: Arc::new(mesh), images, uniform });
    }
    let cull = crate::render::fx_states::cull(EFFECT, TECHNIQUE)?;
    let depth = crate::render::fx_states::depth_test(EFFECT, TECHNIQUE)?;
    let triangles: usize = parts.iter().map(|p| p.mesh.indices.len() / 3).sum();
    let status = format!(
        "mid-ground: {} ({} part(s), {triangles} triangles) at {:?} rot {:?} scale {:?}, terrain_midground.fxo forward \
         (aerial perspective per vertex, gViewScale {VIEW_SCALE}){}",
        mg.object.name,
        parts.len(),
        mg.position,
        mg.rotation,
        mg.scale,
        if skipped.is_empty() { String::new() } else { format!("; not drawn: {}", skipped.join(", ")) },
    );
    Ok(Some(MidgroundLoad { parts, cull, depth, status }))
}

fn part_uniform(l: &GameLighting, world: Mat4) -> MidgroundPartUniform {
    let [beta_r, beta_dash_r, beta_m, beta_dash_m] = super::scattering(l);
    let glm = l.f("globalLightingMult");
    MidgroundPartUniform {
        world,
        light: GameLightUniform::from_preset(l),
        beta_r: beta_r.extend(super::henyey_greenstein(l)),
        beta_dash_r: beta_dash_r.extend(l.f("atmosPlanetRadius")),
        beta_m: beta_m.extend(glm),
        beta_dash_m: beta_dash_m.extend(VIEW_SCALE),
        heights: Vec4::new(
            super::recip(l.f("atmosRayleighHeightFalloff")),
            l.f("atmosRayleighHeightOffset"),
            super::recip(l.f("atmosMieHeightFalloff")),
            l.f("atmosMieHeightOffset"),
        ),
        multipliers: Vec4::new(
            l.f("atmosExtinctionMultiplier"),
            l.f("atmosInScatteringMultiplier"),
            super::recip(glm),
            0.0,
        ),
        sun_dir: l.v("dirLight_Direction").truncate().extend(0.0),
        sun_colour: l.v("dirLight_Colour"),
        tiles: Vec4::ZERO,
        ao_tile: Vec4::ZERO,
    }
}

pub fn relit(l: &GameLighting, part: &MidgroundPartUniform) -> MidgroundPartUniform {
    MidgroundPartUniform { tiles: part.tiles, ao_tile: part.ao_tile, ..part_uniform(l, part.world) }
}

pub(super) mod gpu {
    use std::collections::HashMap;
    use std::sync::Arc;

    use bevy::mesh::VertexBufferLayout;
    use bevy::prelude::*;
    use bevy::render::render_asset::RenderAssets;
    use bevy::render::render_resource::binding_types::{sampler, texture_2d, texture_depth_2d, uniform_buffer};
    use bevy::render::render_resource::*;
    use bevy::render::renderer::{RenderContext, RenderDevice, RenderQueue};
    use bevy::render::texture::{CachedTexture, GpuImage, TextureCache};
    use bevy::render::view::{ExtractedView, Msaa, ViewDepthStencilTexture, ViewTarget};
    use bevy::shader::Shader;

    use super::super::SkySettings;
    use super::{EFFECT, MeshData, MidgroundPartUniform, MidgroundViewUniform, TECHNIQUE};
    use crate::render::buffers::{QueuedUniform, filled_buffer};

    const SHADER: &str = "embedded://ssx_bevy/render/sky/midground.wgsl";

    #[derive(Resource)]
    pub struct MidgroundPipeline {
        layout: BindGroupLayoutDescriptor,
        shader: Handle<Shader>,
        sampler: Sampler,
    }

    #[derive(Clone, Copy, PartialEq, Eq, Hash)]
    pub struct MidgroundKey {
        format: TextureFormat,
        depth_only: bool,
        cull: Option<Face>,
        compare: CompareFunction,
        write: bool,
    }

    impl SpecializedRenderPipeline for MidgroundPipeline {
        type Key = MidgroundKey;

        fn specialize(&self, key: MidgroundKey) -> RenderPipelineDescriptor {
            let attribute = |format: VertexFormat, location: u32| VertexBufferLayout {
                array_stride: format.size(),
                step_mode: VertexStepMode::Vertex,
                attributes: vec![VertexAttribute { format, offset: 0, shader_location: location }],
            };
            RenderPipelineDescriptor {
                label: Some("ssx_midground".into()),
                layout: vec![self.layout.clone()],
                vertex: VertexState {
                    shader: self.shader.clone(),
                    entry_point: Some("vertex".into()),
                    buffers: vec![
                        attribute(VertexFormat::Float32x3, 0),
                        attribute(VertexFormat::Float32x3, 1),
                        attribute(VertexFormat::Float32x3, 2),
                        attribute(VertexFormat::Float32x2, 3),
                    ],
                    ..default()
                },
                primitive: PrimitiveState { cull_mode: key.cull, ..default() },
                depth_stencil: Some(DepthStencilState {
                    format: TextureFormat::Depth32Float,
                    depth_write_enabled: Some(key.write && key.depth_only),
                    depth_compare: Some(if key.write && !key.depth_only {
                        CompareFunction::Equal
                    } else {
                        key.compare
                    }),
                    stencil: StencilState::default(),
                    bias: DepthBiasState::default(),
                }),
                fragment: (!key.depth_only).then(|| FragmentState {
                    shader: self.shader.clone(),
                    entry_point: Some("fragment".into()),
                    targets: vec![Some(ColorTargetState {
                        format: key.format,
                        blend: None,
                        write_mask: ColorWrites::ALL,
                    })],
                    ..default()
                }),
                ..default()
            }
        }
    }

    pub fn init_pipeline(mut commands: Commands, device: Res<RenderDevice>, assets: Res<AssetServer>) {
        let float = TextureSampleType::Float { filterable: true };
        let layout = BindGroupLayoutDescriptor::new(
            "ssx_midground",
            &BindGroupLayoutEntries::sequential(
                ShaderStages::VERTEX_FRAGMENT,
                (
                    uniform_buffer::<MidgroundViewUniform>(false),
                    uniform_buffer::<MidgroundPartUniform>(false),
                    texture_2d(float),
                    texture_2d(float),
                    texture_2d(float),
                    texture_2d(float),
                    texture_2d(float),
                    texture_2d(float),
                    sampler(SamplerBindingType::Filtering),
                    texture_depth_2d(),
                ),
            ),
        );
        let states = crate::render::fx_states::sampler(EFFECT, TECHNIQUE, "baseChannelTextureSampler")
            .and_then(|s| s.descriptor());
        let sampler = match states {
            Ok(d) => device.create_sampler(&SamplerDescriptor { label: Some("ssx_midground"), ..d.as_wgpu() }),
            Err(e) => {
                warn!("mid-ground off: {e:#}");
                return;
            }
        };
        commands.insert_resource(MidgroundPipeline { layout, shader: assets.load(SHADER), sampler });
    }

    struct MeshBuffers {
        streams: [Buffer; 4],
        pub(super) indices: Buffer,
        pub(super) count: u32,
    }

    // PORT(choice): the views' and the parts' uniforms live across frames (one buffer each, written when their bytes
    // change: `QueuedUniform`) instead of two buffers made, filled and freed every frame (Mount Eddie: 2 buffers, 2
    // staging buffers of wgpu's a frame). The pass reads the same values. GPU buffers are the port's renderer; the
    // console's mid-ground set its constants on the device.
    #[derive(Resource, Default)]
    pub struct MidgroundBuffers {
        meshes: HashMap<usize, (Arc<MeshData>, MeshBuffers)>,
        views: HashMap<Entity, QueuedUniform<MidgroundViewUniform>>,
        parts: Vec<QueuedUniform<MidgroundPartUniform>>,
    }

    #[derive(Component)]
    pub struct ViewMidground {
        pipeline: CachedRenderPipelineId,
        prepass: Option<CachedRenderPipelineId>,
        depth: CachedTexture,
        draws: Vec<(BindGroup, usize)>,
    }

    #[allow(clippy::too_many_arguments)]
    pub fn prepare(
        mut commands: Commands,
        settings: Option<Res<SkySettings>>,
        pipeline: Option<Res<MidgroundPipeline>>,
        mut pipelines: ResMut<SpecializedRenderPipelines<MidgroundPipeline>>,
        cache: Res<PipelineCache>,
        mut textures: ResMut<TextureCache>,
        mut buffers: ResMut<MidgroundBuffers>,
        images: Res<RenderAssets<GpuImage>>,
        device: Res<RenderDevice>,
        queue: Res<RenderQueue>,
        views: Query<(Entity, &ExtractedView, &ViewTarget, &ViewDepthStencilTexture, &Msaa)>,
    ) {
        let mg = settings.as_ref().and_then(|s| s.midground.as_ref());
        let (Some(mg), Some(pipeline)) = (mg, pipeline) else {
            buffers.meshes.clear();
            buffers.views.clear();
            buffers.parts.clear();
            for (entity, ..) in &views {
                commands.entity(entity).remove::<ViewMidground>();
            }
            return;
        };
        buffers.meshes.retain(|k, _| mg.parts.iter().any(|p| Arc::as_ptr(&p.mesh) as usize == *k));
        for p in &mg.parts {
            let key = Arc::as_ptr(&p.mesh) as usize;
            buffers.meshes.entry(key).or_insert_with(|| {
                let m = &p.mesh;
                let vertex = |label: &'static str, floats: &[f32]| {
                    filled_buffer(&device, &queue, label, &float_bytes(floats), BufferUsages::VERTEX)
                };
                let streams = [
                    vertex("ssx_midground_positions", m.positions.as_flattened()),
                    vertex("ssx_midground_normals", m.normals.as_flattened()),
                    vertex("ssx_midground_tangents", m.tangents.as_flattened()),
                    vertex("ssx_midground_uvs", m.uvs.as_flattened()),
                ];
                let index_bytes: Vec<u8> = m.indices.iter().flat_map(|i| i.to_le_bytes()).collect();
                let indices =
                    filled_buffer(&device, &queue, "ssx_midground_indices", &index_bytes, BufferUsages::INDEX);
                (p.mesh.clone(), MeshBuffers { streams, indices, count: m.indices.len() as u32 })
            });
        }
        buffers.views.retain(|e, _| views.contains(*e));
        buffers.parts.resize_with(mg.parts.len(), QueuedUniform::default);
        for (uniform, p) in buffers.parts.iter_mut().zip(&mg.parts) {
            uniform.write(&device, &queue, "ssx_midground_part", &p.uniform);
        }
        let MidgroundBuffers { views: view_uniforms, parts: part_uniforms, .. } = &mut *buffers;
        let layout = cache.get_bind_group_layout(&pipeline.layout);
        for (entity, view, target, scene_depth, msaa) in &views {
            let sampled = scene_depth.attachment.depth_stencil_views().depth_only_view();
            let Some(scene_depth_view) = sampled.filter(|_| {
                msaa.samples() == 1 && scene_depth.texture().usage().contains(TextureUsages::TEXTURE_BINDING)
            }) else {
                commands.entity(entity).remove::<ViewMidground>();
                continue;
            };
            let key = MidgroundKey {
                format: view.target_format,
                depth_only: false,
                cull: mg.cull,
                compare: mg.depth.0,
                write: mg.depth.1,
            };
            let id = pipelines.specialize(&cache, &pipeline, key);
            let prepass =
                key.write.then(|| pipelines.specialize(&cache, &pipeline, MidgroundKey { depth_only: true, ..key }));
            let size = target.main_texture().size();
            let depth = textures.get(
                &device,
                TextureDescriptor {
                    label: Some("ssx_midground_depth"),
                    size: Extent3d { width: size.width, height: size.height, depth_or_array_layers: 1 },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: TextureDimension::D2,
                    format: TextureFormat::Depth32Float,
                    usage: TextureUsages::RENDER_ATTACHMENT,
                    view_formats: &[],
                },
            );
            let view_uniform = view_uniforms.entry(entity).or_default();
            view_uniform.write(
                &device,
                &queue,
                "ssx_midground_view",
                &MidgroundViewUniform {
                    view_from_world: view.world_from_view.to_matrix().inverse(),
                    clip_from_view: view.clip_from_view,
                    camera: view.world_from_view.translation().extend(1.0),
                },
            );
            let mut draws = Vec::new();
            for (p, part_uniform) in mg.parts.iter().zip(part_uniforms.iter()) {
                let tex: Option<Vec<&GpuImage>> = p.textures.iter().map(|h| images.get(h)).collect();
                let Some(tex) = tex else { continue };
                let bind_group = device.create_bind_group(
                    "ssx_midground",
                    &layout,
                    &BindGroupEntries::sequential((
                        &*view_uniform,
                        part_uniform,
                        &tex[0].texture_view,
                        &tex[1].texture_view,
                        &tex[2].texture_view,
                        &tex[3].texture_view,
                        &tex[4].texture_view,
                        &tex[5].texture_view,
                        &pipeline.sampler,
                        scene_depth_view,
                    )),
                );
                draws.push((bind_group, Arc::as_ptr(&p.mesh) as usize));
            }
            commands.entity(entity).insert(ViewMidground { pipeline: id, prepass, depth, draws });
        }
    }

    fn float_bytes(v: &[f32]) -> Vec<u8> {
        v.iter().flat_map(|f| f.to_le_bytes()).collect()
    }

    pub fn record(
        ctx: &mut RenderContext,
        target: &ViewTarget,
        mg: &ViewMidground,
        buffers: &MidgroundBuffers,
        cache: &PipelineCache,
    ) {
        let Some(pipeline) = cache.get_render_pipeline(mg.pipeline) else { return };
        let prepass = match mg.prepass {
            Some(id) => match cache.get_render_pipeline(id) {
                Some(p) => Some(p),
                None => return,
            },
            None => None,
        };
        if mg.draws.is_empty() {
            return;
        }
        // TODO(guess): the mountains' depth (Pv / gViewScale) stays in this pass's own buffer, the game writes it into the scene's (later translucents behind it hidden); settle: Bevy's depth with a stencil aspect (the game's EQUAL ref 4 mask 3 test as is).
        // the depth-only pass (its own render pass: no colour target)
        if let Some(prepass) = prepass {
            let mut pass = ctx.begin_tracked_render_pass(RenderPassDescriptor {
                label: Some("ssx_midground_depth"),
                color_attachments: &[],
                depth_stencil_attachment: Some(RenderPassDepthStencilAttachment {
                    view: &mg.depth.default_view,
                    depth_ops: Some(Operations { load: LoadOp::Clear(0.0), store: StoreOp::Store }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            draw_all(&mut pass, prepass, &mg.draws, buffers);
        }
        let color = [Some(target.get_color_attachment())];
        let load = if prepass.is_some() { LoadOp::Load } else { LoadOp::Clear(0.0) };
        let mut pass = ctx.begin_tracked_render_pass(RenderPassDescriptor {
            label: Some("ssx_midground"),
            color_attachments: &color,
            depth_stencil_attachment: Some(RenderPassDepthStencilAttachment {
                view: &mg.depth.default_view,
                depth_ops: Some(Operations { load, store: StoreOp::Discard }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        draw_all(&mut pass, pipeline, &mg.draws, buffers);
    }

    fn draw_all<'a>(
        pass: &mut bevy::render::render_phase::TrackedRenderPass<'a>,
        pipeline: &'a RenderPipeline,
        draws: &'a [(BindGroup, usize)],
        buffers: &'a MidgroundBuffers,
    ) {
        pass.set_render_pipeline(pipeline);
        for (bind_group, mesh) in draws {
            let Some((_, b)) = buffers.meshes.get(mesh) else { continue };
            pass.set_bind_group(0, bind_group, &[]);
            for (slot, stream) in b.streams.iter().enumerate() {
                pass.set_vertex_buffer(slot, stream.slice(..));
            }
            pass.set_index_buffer(b.indices.slice(..), IndexFormat::Uint32);
            pass.draw_indexed(0..b.count, 0, 0..1);
        }
    }
}

pub(crate) fn static_mesh(root: &Path, geometry: &str) -> Result<ssx_assets::object_mesh::ObjectMesh> {
    let content = ssx_assets::content::GameContent::of(root);
    let rel = geometry.replace('\\', "/");
    let id = ssx_assets::content::models::model_id(&rel)
        .filter(|id| content.has(id))
        .with_context(|| format!("{rel} not found"))?;
    let found = content.model(&id, ssx_assets::content::resolve::ModelUse::Static)?;
    let (mesh, notes) = found.value.lod0_mesh().map_err(|e| anyhow::anyhow!("{id}: {e}"))?;
    for n in notes {
        bevy::log::warn!("{id} from {}: {n}", found.source);
    }
    Ok(mesh)
}
