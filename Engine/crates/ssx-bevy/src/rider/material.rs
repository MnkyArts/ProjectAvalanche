use bevy::asset::{RenderAssetUsages, embedded_asset, uuid_handle};
use bevy::core_pipeline::prepass::{
    AlphaMask3dPrepass, DeferredPrepass, DepthPrepass, MotionVectorPrepass, NormalPrepass, Opaque3dPrepass,
};
use bevy::mesh::MeshVertexBufferLayoutRef;
use bevy::pbr::{ExtendedMaterial, MaterialExtension, MaterialExtensionKey, MaterialExtensionPipeline};
use bevy::prelude::*;
use bevy::render::render_phase::ViewBinnedRenderPhases;
use bevy::render::render_resource::{AsBindGroup, RenderPipelineDescriptor, ShaderType, SpecializedMeshPipelineError};
use bevy::render::storage::ShaderBuffer;
use bevy::render::sync_world::MainEntity;
use bevy::render::view::ExtractedView;
use bevy::render::{Extract, ExtractSchedule, Render, RenderApp, RenderSystems};
use bevy::shader::ShaderRef;

use crate::render::shadows::{ShadowPassKey, specialize_shadow_pass};

use crate::render::lighting::game::GameLightUniform;

const SHADER_PATH: &str = "embedded://ssx_bevy/rider/character.wesl";
const GBUFFER_PATH: &str = "embedded://ssx_bevy/rider/character_gbuffer.wesl";

pub const CHARACTER_STATUS: &str = "\
rider (listing port): player_body/hair/eye/apparel gbuffer + material passes (normal z:=1, d^2, rim + fill, \
dynamicAOMult ramp tint, hair AO map, eye AO, apparel glow)\n\
board (listing port): player_board passes (rim * rimMult, glow = d^2 * d.a, hologlitch off), diffuse = the board's \
design (TexturePack.Items[ID] by the ge_partbinding: the board item's id, a racer's 0; common_board_diffuse without one)\n\
rider: playerShadowTestResult = shadowtest.fxo query at the AITrajectory joint (8-bit read-back), \
dynamicGlowMult = GlowOff (SSX_RIDER_GLOW=on: GlowOn)\n\
bots (listing port): player_secondary LOD characters = body passes without the shadow test, alpha kill 0.25, \
no hologlitch (racer kind 1)";

pub type CharacterMaterial = ExtendedMaterial<StandardMaterial, CharacterExtension>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CharacterShading {
    Body = 0,
    Hair = 1,
    Eye = 2,
    Apparel = 3,
    Board = 4,
    Wingsuit = 5,
    Secondary = 6,
}

impl CharacterShading {
    pub fn of(effect: &str) -> Option<CharacterShading> {
        match effect {
            "player_body.fx" => Some(CharacterShading::Body),
            "player_hair.fx" => Some(CharacterShading::Hair),
            "player_eye.fx" => Some(CharacterShading::Eye),
            "player_apparel.fx" => Some(CharacterShading::Apparel),
            "player_board.fx" => Some(CharacterShading::Board),
            "player_wingsuit.fx" => Some(CharacterShading::Wingsuit),
            "player_secondary.fx" => Some(CharacterShading::Secondary),
            _ => None,
        }
    }
}

pub struct CharacterMaterialPlugin;

impl Plugin for CharacterMaterialPlugin {
    fn build(&self, app: &mut App) {
        embedded_asset!(app, "character.wesl");
        bevy::shader::load_shader_library!(app, "character_normal.wesl");
        embedded_asset!(app, "character_gbuffer.wesl");
        app.add_plugins(MaterialPlugin::<CharacterMaterial>::default());
        let frame = CharacterFrame::default();
        let _ = app
            .world_mut()
            .resource_mut::<Assets<ShaderBuffer>>()
            .insert(RIDER_FRAME.id(), ShaderBuffer::new(frame.bytes().to_vec(), RenderAssetUsages::default()));
        app.add_systems(PostUpdate, upload_rider_frame);
        if let Some(render_app) = app.get_sub_app_mut(RenderApp) {
            render_app
                .init_resource::<CharacterMeshes>()
                .add_systems(ExtractSchedule, extract_character_meshes)
                .add_systems(
                    Render,
                    characters_out_of_depth_prepass
                        .in_set(RenderSystems::QueueMeshes)
                        .after(bevy::pbr::queue_prepass_material_meshes),
                );
        }
    }
}

#[derive(Resource, Default)]
struct CharacterMeshes(Vec<MainEntity>);

fn extract_character_meshes(
    mut meshes: ResMut<CharacterMeshes>,
    characters: Extract<Query<Entity, With<MeshMaterial3d<CharacterMaterial>>>>,
) {
    meshes.0.clear();
    meshes.0.extend(characters.iter().map(MainEntity::from));
}

type DepthOnlyPrepass =
    (With<DepthPrepass>, Without<NormalPrepass>, Without<MotionVectorPrepass>, Without<DeferredPrepass>);

// PORT(choice): the characters are taken out of a view's depth-only prepass (Bevy 0.20's `DepthPrepass` without the
// normal, motion-vector or deferred prepass: the level camera's, `render::post::topogrid`), after Bevy queues its
// phases: they write nothing there (no depth, `render::shadows::specialize_shadow_pass`; no colour target), so their
// draws were recorded, skinned and rasterised for no pixel. Bevy 0.20 turns a material's prepass on per material type
// (`Material::enable_prepass`, read when a material is prepared; `specialize_prepass_material_meshes` then leaves the
// type out of every prepass view), and the characters need theirs while the level can run TOPOGRID: the level
// camera's prepass is then the normal prepass, where they write the depth and the gbuffer normal that the TOPOGRID
// pass reads at each pixel (`character_gbuffer.wesl`, `post_final.wesl` `apply_topogrid`), so those views keep them.
// `BinnedRenderPhase::remove` takes an entity out of its retained bin; Bevy queues an entity again only when it is
// specialised again or becomes visible again, both in `queue_prepass_material_meshes`, which this system follows. The
// console drew no depth prepass (the port's, `render::post::topogrid`).
fn characters_out_of_depth_prepass(
    meshes: Res<CharacterMeshes>,
    views: Query<&ExtractedView, DepthOnlyPrepass>,
    mut opaque: ResMut<ViewBinnedRenderPhases<Opaque3dPrepass>>,
    mut alpha_mask: ResMut<ViewBinnedRenderPhases<AlphaMask3dPrepass>>,
) {
    for view in &views {
        if let Some(phase) = opaque.get_mut(&view.retained_view_entity) {
            for &entity in &meshes.0 {
                phase.remove(entity);
            }
        }
        if let Some(phase) = alpha_mask.get_mut(&view.retained_view_entity) {
            for &entity in &meshes.0 {
                phase.remove(entity);
            }
        }
    }
}

pub const RIDER_FRAME: Handle<ShaderBuffer> = uuid_handle!("3f6b2a1c-8d4e-4b7a-9c2e-5a1d7e3f9b04");

#[derive(Clone, Copy, Debug, PartialEq)]
struct CharacterFrame {
    dynamic: Vec4,
    shadow_point: Vec4,
    holo: Vec4,
}

impl Default for CharacterFrame {
    fn default() -> Self {
        CharacterFrame { dynamic: Vec4::new(AO_IN_AIR, 0.0, 0.0, 0.0), shadow_point: Vec4::ZERO, holo: Vec4::ZERO }
    }
}

impl CharacterFrame {
    fn bytes(&self) -> [u8; 48] {
        let mut out = [0; 48];
        let floats =
            self.dynamic.to_array().into_iter().chain(self.shadow_point.to_array()).chain(self.holo.to_array());
        for (chunk, f) in out.as_chunks_mut::<4>().0.iter_mut().zip(floats) {
            *chunk = f.to_le_bytes();
        }
        out
    }

    fn write_to(&self, buffer: &mut ShaderBuffer) {
        buffer.clear();
        buffer.extend_from_slice(&self.bytes());
    }
}

// PORT(choice): a racer's `hologlitchCollisionBlend` lives in a 48-byte storage buffer of its own, written in place
// (Bevy 0.20's `GpuShaderBuffer::prepare_asset` writes a buffer of the same size into the GPU buffer it has, so the
// materials and their bind groups stay); it was a uniform of every one of the racer's character materials (every LOD's
// parts and its board), and each change made Bevy prepare each of them again (`AsBindGroup`: new uniform buffers and a
// bind group) and specialise their entities again. The value changes up to once a simulation step: while the racer
// fades in and while it is within 4 m of the player (`Racers_UpdateFade` 0x82501BA0, the glitch's `Random_Range`). The
// console hands the value to the racer's render object once per update as an attribute ([r29+0xC] vt+0x8 with the key
// of "hologlitchCollisionBlend" 0x838C9C30, 0x82502214…0x8250222C).
pub struct RacerFrame {
    buffer: Handle<ShaderBuffer>,
    frame: CharacterFrame,
}

impl RacerFrame {
    pub fn new(buffers: &mut Assets<ShaderBuffer>) -> Self {
        let frame = CharacterFrame::default();
        let buffer = buffers.add(ShaderBuffer::new(frame.bytes().to_vec(), RenderAssetUsages::default()));
        RacerFrame { buffer, frame }
    }

    pub fn bind(&self, material: &mut CharacterMaterial) {
        material.extension.frame = self.buffer.clone();
    }

    pub fn set_holo(&mut self, holo: Vec4, buffers: &mut Assets<ShaderBuffer>) {
        if self.frame.holo == holo {
            return;
        }
        if let Some(mut buffer) = buffers.get_mut(&self.buffer) {
            self.frame.holo = holo;
            self.frame.write_to(&mut buffer);
        }
    }
}

fn upload_rider_frame(
    shading: Option<Res<RiderShading>>,
    mut buffers: ResMut<Assets<ShaderBuffer>>,
    mut last: Local<Option<CharacterFrame>>,
) {
    let Some(shading) = shading else { return };
    let Some(frame) = shading.frame else { return };
    if *last == Some(frame) {
        return;
    }
    if let Some(mut buffer) = buffers.get_mut(&RIDER_FRAME) {
        frame.write_to(&mut buffer);
        *last = Some(frame);
    }
}

#[derive(Asset, AsBindGroup, Reflect, Debug, Clone)]
#[bind_group_data(ShadowPassKey)]
pub struct CharacterExtension {
    #[uniform(100)]
    pub light: GameLightUniform,
    #[uniform(101)]
    pub rider: CharacterLightUniform,
    #[uniform(102)]
    pub params: CharacterParams,
    #[texture(103)]
    #[sampler(104)]
    pub normal: Handle<Image>,
    #[texture(105)]
    pub specular: Handle<Image>,
    #[texture(106)]
    pub ambient: Handle<Image>,
    #[storage(107, read_only)]
    pub level_light: Handle<ShaderBuffer>,
    #[uniform(120)]
    pub shadow_alpha: Vec4,
    #[storage(140, read_only)]
    pub local_lights: Handle<bevy::render::storage::ShaderBuffer>,
    #[texture(141, dimension = "2d_array")]
    #[sampler(142)]
    pub local_light_textures: Handle<Image>,
    #[storage(121, read_only)]
    pub frame: Handle<ShaderBuffer>,
    pub shadow_pass: ShadowPassKey,
}

impl From<&CharacterExtension> for ShadowPassKey {
    fn from(e: &CharacterExtension) -> Self {
        e.shadow_pass
    }
}

impl MaterialExtension for CharacterExtension {
    fn vertex_shader() -> ShaderRef {
        crate::render::lighting::game::GAME_VERTEX_PATH.into()
    }

    fn fragment_shader() -> ShaderRef {
        SHADER_PATH.into()
    }

    fn prepass_fragment_shader() -> ShaderRef {
        GBUFFER_PATH.into()
    }

    fn specialize(
        _pipeline: &MaterialExtensionPipeline,
        descriptor: &mut RenderPipelineDescriptor,
        _layout: &MeshVertexBufferLayoutRef,
        key: MaterialExtensionKey<Self>,
    ) -> Result<(), SpecializedMeshPipelineError> {
        specialize_shadow_pass(descriptor, key.mesh_key, key.bind_group_data);
        crate::render::gbuffer::specialize_main_pass(descriptor);
        crate::render::lighting::game::specialize_game_vertex(descriptor, 100);
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Default, Reflect, ShaderType)]
pub struct CharacterParams {
    pub shading: Vec4,
    pub glow: Vec4,
    pub lod_bias: Vec4,
    pub board: Vec4,
    pub holo_bloom: Vec4,
}

impl CharacterParams {
    pub fn new(shading: CharacterShading, spec_intensity: f32, eccentricity: f32, glow: Vec3) -> Self {
        CharacterParams {
            shading: Vec4::new(shading as u8 as f32, spec_intensity, eccentricity, 0.0),
            glow: glow.extend(0.0),
            lod_bias: Vec4::ZERO,
            board: Vec4::ZERO,
            holo_bloom: Vec4::ZERO,
        }
    }
}

#[derive(Clone, Copy, Debug, Reflect, ShaderType)]
pub struct CharacterLightUniform {
    pub dynamic: Vec4,
    pub shadow_point: Vec4,
    pub frame: Vec4,
}

impl Default for CharacterLightUniform {
    fn default() -> Self {
        CharacterLightUniform {
            dynamic: Vec4::new(AO_IN_AIR, 0.0, 0.0, 0.0),
            shadow_point: Vec4::ZERO,
            frame: Vec4::ZERO,
        }
    }
}

const AO_IN_AIR: f32 = 0.6;
const AO_CONTACT_SCALE: f32 = 0.8;
const AO_CONTACT_WEIGHT: f32 = 2.0;
const AO_SAMPLES: usize = 45;

#[derive(Resource, Default)]
pub struct RiderShading {
    pub materials: Vec<Handle<CharacterMaterial>>,
    samples: std::collections::VecDeque<(f32, f32)>,
    last_step: Option<u64>,
    pub glow: f32,
    pub ao: f32,
    pub wingsuit_deployed: bool,
    pub wingsuit: Vec<Handle<CharacterMaterial>>,
    pub ao_override: Option<f32>,
    frame: Option<CharacterFrame>,
    bound: bool,
    wingsuit_alpha: Option<f32>,
}

impl RiderShading {
    pub fn new(materials: Vec<Handle<CharacterMaterial>>, wingsuit: Vec<Handle<CharacterMaterial>>) -> Self {
        let glow = std::env::var("SSX_RIDER_GLOW").is_ok_and(|v| v == "on" || v == "1");
        RiderShading { materials, wingsuit, glow: if glow { 1.0 } else { 0.0 }, ao: AO_IN_AIR, ..default() }
    }

    pub fn sample(&mut self, step: u64, contact_ao: Option<f32>) {
        if self.last_step == Some(step) {
            return;
        }
        self.last_step = Some(step);
        let sample = match contact_ao {
            Some(a) => (1.0 - AO_CONTACT_SCALE * (1.0 - a), AO_CONTACT_WEIGHT),
            None => (AO_IN_AIR, 1.0),
        };
        let sample = match self.ao_override {
            Some(v) if (v - 1.0).abs() > 1.525_878_9e-5 => (v, 1.0),
            _ => sample,
        };
        if self.samples.len() == AO_SAMPLES {
            self.samples.pop_front();
        }
        self.samples.push_back(sample);
        let (sum, weight) = self.samples.iter().fold((0.0, 0.0), |(s, w), (v, k)| (s + v * k, w + k));
        self.ao = if weight > 0.0 { (sum / weight).clamp(0.0, 1.0) } else { AO_IN_AIR };
    }

    pub fn replace(
        &mut self,
        old: &[Handle<CharacterMaterial>],
        new: Vec<Handle<CharacterMaterial>>,
        wingsuit: Vec<Handle<CharacterMaterial>>,
    ) {
        self.materials.retain(|m| !old.contains(m));
        self.materials.extend(new);
        self.wingsuit = wingsuit;
        (self.bound, self.wingsuit_alpha) = (false, None);
    }

    pub fn apply(&mut self, shadow_point: Vec3, materials: &mut Assets<CharacterMaterial>) {
        let deployed = if self.wingsuit_deployed { 1.0 } else { 0.0 };
        if !self.bound {
            for handle in &self.materials {
                if let Some(mut m) = materials.get_mut(handle) {
                    m.extension.rider.frame.x = 1.0;
                }
            }
            self.bound = true;
        }
        self.frame = Some(CharacterFrame {
            dynamic: Vec4::new(self.ao, self.glow, deployed, 0.0),
            shadow_point: shadow_point.extend(1.0),
            holo: Vec4::ZERO,
        });
        if self.wingsuit_alpha != Some(deployed) {
            for handle in &self.wingsuit {
                if let Some(mut m) = materials.get_mut(handle) {
                    m.extension.shadow_alpha.y = deployed;
                }
            }
            self.wingsuit_alpha = Some(deployed);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_racers_glitch_is_written_into_its_frame_buffer_in_place() {
        let mut buffers = Assets::<ShaderBuffer>::default();
        let mut frame = RacerFrame::new(&mut buffers);
        frame.set_holo(Vec4::new(0.5, 0.25, 0.0, 0.0), &mut buffers);
        let buffer = buffers.get(&frame.buffer).expect("the racer's buffer");
        assert_eq!(buffer.buffer_size(), 48);
        let bytes: &[u8] = buffer.cast_slice().expect("initialised");
        let floats: Vec<f32> = bytes.as_chunks::<4>().0.iter().map(|b| f32::from_le_bytes(*b)).collect();
        assert_eq!(floats, [AO_IN_AIR, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.5, 0.25, 0.0, 0.0]);
    }

    #[test]
    fn characters_leave_a_depth_only_prepass_and_stay_in_the_normal_prepass() {
        use bevy::core_pipeline::prepass::{OpaqueNoLightmap3dBatchSetKey, OpaqueNoLightmap3dBinKey};
        use bevy::ecs::system::RunSystemOnce;
        use bevy::render::batching::gpu_preprocessing::GpuPreprocessingMode;
        use bevy::render::mesh::allocator::MeshSlabs;
        use bevy::render::render_phase::{BinnedRenderPhaseType, DrawFunctionId, InputUniformIndex};
        use bevy::render::render_resource::{CachedRenderPipelineId, TextureFormat};
        use bevy::render::view::RetainedViewEntity;

        let mut world = World::new();
        let character = world.spawn_empty().id();
        let other = world.spawn_empty().id();
        let mut opaque = ViewBinnedRenderPhases::<Opaque3dPrepass>::default();
        let mut views = Vec::new();
        for normal in [false, true] {
            let view = world.spawn(DepthPrepass).id();
            if normal {
                world.entity_mut(view).insert(NormalPrepass);
            }
            let retained = RetainedViewEntity::new(MainEntity::from(view), None, 0);
            world.entity_mut(view).insert(ExtractedView {
                retained_view_entity: retained,
                clip_from_view: Mat4::IDENTITY,
                world_from_view: GlobalTransform::IDENTITY,
                clip_from_world: None,
                target_format: TextureFormat::Rgba8Unorm,
                viewport: UVec4::ZERO,
                color_grading: default(),
                invert_culling: false,
            });
            opaque.prepare_for_new_frame(retained, GpuPreprocessingMode::None);
            let phase = opaque.get_mut(&retained).expect("the view's phase");
            for mesh in [character, other] {
                let key = OpaqueNoLightmap3dBatchSetKey {
                    pipeline: CachedRenderPipelineId::INVALID,
                    draw_function: DrawFunctionId(0),
                    material_bind_group_index: None,
                    slabs: MeshSlabs::default(),
                };
                let bin = OpaqueNoLightmap3dBinKey { asset_id: AssetId::<Mesh>::default().untyped() };
                let item = (mesh, MainEntity::from(mesh));
                phase.add(key, bin, item, InputUniformIndex::default(), BinnedRenderPhaseType::UnbatchableMesh);
            }
            views.push(retained);
        }
        world.insert_resource(opaque);
        world.insert_resource(ViewBinnedRenderPhases::<AlphaMask3dPrepass>::default());
        world.insert_resource(CharacterMeshes(vec![MainEntity::from(character)]));
        world.run_system_once(characters_out_of_depth_prepass).expect("the system runs");

        let opaque = world.resource::<ViewBinnedRenderPhases<Opaque3dPrepass>>();
        let meshes = |view| {
            let phase = &opaque[view];
            let mut out: Vec<MainEntity> =
                phase.unbatchable_meshes.values().flat_map(|b| b.entities.keys().copied()).collect();
            out.sort();
            out
        };
        assert_eq!(meshes(&views[0]), [MainEntity::from(other)]);
        let mut both = vec![MainEntity::from(character), MainEntity::from(other)];
        both.sort();
        assert_eq!(meshes(&views[1]), both);
    }
}
