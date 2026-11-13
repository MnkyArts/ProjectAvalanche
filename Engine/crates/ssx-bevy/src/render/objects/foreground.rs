use bevy::asset::embedded_asset;
use bevy::mesh::MeshVertexBufferLayoutRef;
use bevy::pbr::{ExtendedMaterial, MaterialExtension, MaterialExtensionKey, MaterialExtensionPipeline};
use bevy::prelude::*;
use bevy::render::render_resource::{AsBindGroup, RenderPipelineDescriptor, ShaderType, SpecializedMeshPipelineError};
use bevy::shader::ShaderRef;

use crate::render::lighting::game::GameLightUniform;
use crate::render::shadows::{ShadowPassKey, specialize_shadow_pass};

const SHADER_PATH: &str = "embedded://ssx_bevy/render/objects/foreground.wesl";

pub type ForegroundMaterial = ExtendedMaterial<StandardMaterial, ForegroundExtension>;

pub struct ForegroundMaterialPlugin;

impl Plugin for ForegroundMaterialPlugin {
    fn build(&self, app: &mut App) {
        embedded_asset!(app, "foreground.wesl");
        app.add_plugins(MaterialPlugin::<ForegroundMaterial>::default());
    }
}

#[derive(Asset, AsBindGroup, Reflect, Debug, Clone)]
#[bind_group_data(ShadowPassKey)]
pub struct ForegroundExtension {
    #[uniform(100)]
    pub params: ForegroundParams,
    #[texture(101)]
    pub mask: Handle<Image>,
    #[texture(102)]
    pub base: Handle<Image>,
    #[texture(103)]
    pub red: Handle<Image>,
    #[texture(104)]
    #[sampler(105)]
    pub green: Handle<Image>,
    #[uniform(106)]
    pub light: GameLightUniform,
    #[texture(107)]
    pub occlusion: Handle<Image>,
    #[texture(108)]
    pub normal: Handle<Image>,
    pub shadow_pass: ShadowPassKey,
}

impl From<&ForegroundExtension> for ShadowPassKey {
    fn from(e: &ForegroundExtension) -> Self {
        e.shadow_pass
    }
}

impl MaterialExtension for ForegroundExtension {
    fn vertex_shader() -> ShaderRef {
        crate::render::lighting::game::GAME_VERTEX_PATH.into()
    }

    fn fragment_shader() -> ShaderRef {
        SHADER_PATH.into()
    }

    fn enable_prepass() -> bool {
        false
    }

    fn specialize(
        _pipeline: &MaterialExtensionPipeline,
        descriptor: &mut RenderPipelineDescriptor,
        _layout: &MeshVertexBufferLayoutRef,
        key: MaterialExtensionKey<Self>,
    ) -> Result<(), SpecializedMeshPipelineError> {
        specialize_shadow_pass(descriptor, key.mesh_key, key.bind_group_data);
        crate::render::lighting::game::specialize_game_vertex(descriptor, 106);
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Default, Reflect, ShaderType)]
pub struct ForegroundParams {
    pub tiles: Vec4,
    pub ao_tile: Vec4,
}
