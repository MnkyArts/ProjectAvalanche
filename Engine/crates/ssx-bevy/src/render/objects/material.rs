use bevy::asset::{RenderAssetUsages, embedded_asset, uuid_handle};
use bevy::mesh::MeshVertexBufferLayoutRef;
use bevy::pbr::{
    ExtendedMaterial, MaterialExtension, MaterialExtensionKey, MaterialExtensionPipeline, MeshPipelineKey,
};
use bevy::prelude::*;
use bevy::render::render_resource::{
    AsBindGroup, BlendComponent, BlendFactor, BlendOperation, BlendState, RenderPipelineDescriptor, ShaderType,
    SpecializedMeshPipelineError, TextureFormat,
};
use bevy::render::storage::ShaderBuffer;
use bevy::shader::{ShaderRef, load_shader_library};

use crate::render::lighting::game::GameLightUniform;
use crate::render::shadows::{ShadowPassKey, specialize_shadow_pass};

const SHADER_PATH: &str = "embedded://ssx_bevy/render/objects/object.wesl";
const PREPASS_PATH: &str = "embedded://ssx_bevy/render/objects/object_prepass.wesl";
const GBUFFER_PATH: &str = "embedded://ssx_bevy/render/objects/object_gbuffer.wesl";

pub const PLACEMENT_BUFFER: Handle<ShaderBuffer> = uuid_handle!("4b5d0a52-8a3e-4f63-9a51-1d9e1f2c6b70");

pub fn placement_bytes(entries: &[[f32; 2]]) -> Vec<u8> {
    entries.iter().flatten().flat_map(|v| v.to_le_bytes()).collect()
}

pub const OBJECT_STATUS: &str = "\
objects (listing port): object_default/alpha/foliage/advanced gbuffer + material passes (normal z:=1, d^2, \
(specInt*spec.g)^2, foliage translucency, advanced snow/scatter/sparkle, glow), alpha test 0.25\n\
objects: c0 UBYTE4N .y=AO .z=snow, instance AO mult (placement word 12), stored g0 tangents, effect samplers/culling, \
LODs by the game's size metric; prelight passes < 400 m, forward_low_lod beyond, translucent_low_lod while fading \
(blended into the 8-bit frame after the tone map, PS7 exposure + Hable + sqrt); approx.: no wind/UV animation";

pub type ObjectMaterial = ExtendedMaterial<StandardMaterial, ObjectExtension>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ObjectShading {
    Default = 0,
    Foliage = 1,
    Advanced = 2,
    ShadowOnly = 3,
}

impl ObjectShading {
    pub fn of(effect: &str) -> ObjectShading {
        if effect.starts_with("object_foliage") {
            ObjectShading::Foliage
        } else if effect == "object_advanced.fx" {
            ObjectShading::Advanced
        } else if effect == "object_shadow.fx" {
            ObjectShading::ShadowOnly
        } else {
            ObjectShading::Default
        }
    }
}

pub struct ObjectMaterialPlugin;

impl Plugin for ObjectMaterialPlugin {
    fn build(&self, app: &mut App) {
        embedded_asset!(app, "object.wesl");
        load_shader_library!(app, "shadow_alpha.wesl");
        load_shader_library!(app, "object_normal.wesl");
        embedded_asset!(app, "object_gbuffer.wesl");
        embedded_asset!(app, "object_prepass.wesl");
        app.add_plugins(MaterialPlugin::<ObjectMaterial>::default()).add_systems(Update, follow_hooks);
        if let Some(mut buffers) = app.world_mut().get_resource_mut::<Assets<ShaderBuffer>>() {
            let neutral = ShaderBuffer::new(placement_bytes(&[[1.0, 1.0]]), RenderAssetUsages::default());
            if let Err(e) = buffers.insert(PLACEMENT_BUFFER.id(), neutral) {
                error!("placement buffer: {e}");
            }
            let ldr = ShaderBuffer::new(ldr_tonemap_neutral(), RenderAssetUsages::default());
            if let Err(e) = buffers.insert(LDR_TONEMAP_BUFFER.id(), ldr) {
                error!("LDR tone-map buffer: {e}");
            }
        }
    }
}

#[derive(Asset, AsBindGroup, Reflect, Debug, Clone)]
#[bind_group_data(ObjectPipelineKey)]
pub struct ObjectExtension {
    #[uniform(100)]
    pub light: GameLightUniform,
    #[uniform(101)]
    pub params: ObjectParams,
    #[texture(102)]
    #[sampler(103)]
    pub normal: Handle<Image>,
    #[texture(104)]
    pub specular: Handle<Image>,
    #[texture(105)]
    pub scatter: Handle<Image>,
    #[texture(106)]
    #[sampler(107)]
    pub sparkle: Handle<Image>,
    #[uniform(120)]
    pub shadow_alpha: Vec4,
    #[storage(121, read_only)]
    pub placements: Handle<ShaderBuffer>,
    #[storage(140, read_only)]
    pub local_lights: Handle<bevy::render::storage::ShaderBuffer>,
    #[texture(141, dimension = "2d_array")]
    #[sampler(142)]
    pub local_light_textures: Handle<Image>,
    #[storage(122, read_only)]
    pub ldr_tonemap: Handle<ShaderBuffer>,
    pub shadow_pass: ShadowPassKey,
    pub ldr: bool,
    pub tangent_sign: bool,
    pub hook: Option<u64>,
    pub hook_wanted: Option<(u64, String)>,
    pub hook_epoch: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Reflect)]
pub struct ObjectPipelineKey {
    pub shadow: ShadowPassKey,
    pub shadow_only: bool,
    pub ldr: bool,
    pub tangent_sign: bool,
    pub hook: Option<u64>,
    pub hook_epoch: u64,
}

impl From<&ObjectExtension> for ObjectPipelineKey {
    fn from(e: &ObjectExtension) -> Self {
        ObjectPipelineKey {
            shadow: e.shadow_pass,
            shadow_only: e.params.shading.x == f32::from(ObjectShading::ShadowOnly as u8),
            ldr: e.ldr,
            tangent_sign: e.tangent_sign,
            hook: e.hook,
            hook_epoch: e.hook_epoch,
        }
    }
}

pub const LDR_FRAME_FORMAT: TextureFormat = TextureFormat::Rgba8Unorm;

pub const LDR_PIPELINE_LABEL: &str = "ssx_translucent_low_lod_ldr";

pub const LDR_POST_PARTICLES_LABEL: &str = "ssx_translucent_post_particles_ldr";

pub const LDR_TONEMAP_BUFFER: Handle<ShaderBuffer> = uuid_handle!("9f3e2a61-5b7c-4d1e-8a20-6c4b3d2e1f90");
pub const LDR_TONEMAP_BYTES: u64 = 48;
pub const LDR_TONEMAP_EXPOSURE_OFFSET: u64 = 36;

impl MaterialExtension for ObjectExtension {
    fn vertex_shader() -> ShaderRef {
        crate::render::lighting::game::GAME_VERTEX_PATH.into()
    }

    fn fragment_shader() -> ShaderRef {
        SHADER_PATH.into()
    }

    fn prepass_vertex_shader() -> ShaderRef {
        PREPASS_PATH.into()
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
        specialize_shadow_pass(descriptor, key.mesh_key, key.bind_group_data.shadow);
        let key_data = key.bind_group_data;
        let blend = key.mesh_key.intersection(MeshPipelineKey::BLEND_RESERVED_BITS) == MeshPipelineKey::BLEND_ALPHA;
        if !key_data.shadow_only
            && key_data.hook.is_none()
            && !key.mesh_key.contains(MeshPipelineKey::MORPH_TARGETS)
            && !blend
        {
            if crate::render::gbuffer::is_depth_prepass(descriptor, key.mesh_key) {
                crate::render::gbuffer::depth_prepass_writes(descriptor, true);
                if let Some(fragment) = descriptor.fragment.as_mut() {
                    fragment.shader_defs.push("SSX_DEPTH_PREPASS".into());
                }
            }
            crate::render::gbuffer::invariant_main_pass(descriptor);
        }
        crate::render::gbuffer::specialize_main_pass(descriptor);
        crate::render::lighting::game::specialize_game_vertex(descriptor, 100);
        if blend && let Some(depth) = descriptor.depth_stencil.as_mut() {
            depth.depth_write_enabled = Some(true);
        }
        if blend && key.bind_group_data.ldr {
            specialize_ldr_translucent(descriptor);
        }
        if key.bind_group_data.tangent_sign
            && let Some(fragment) = descriptor.fragment.as_mut()
        {
            fragment.shader_defs.push("SSX_TANGENT_SIGN".into());
        }
        if let Some(hash) = key.bind_group_data.hook {
            specialize_hook(descriptor, hash);
        }
        Ok(())
    }
}

fn specialize_hook(descriptor: &mut RenderPipelineDescriptor, hash: u64) {
    use crate::render::hooks::runtime::{admit, hook_defs, variant_shader};
    const PREPASS: &str = "render/objects/object_prepass.wesl";
    const GBUFFER: &str = "render/objects/object_gbuffer.wesl";
    const FRAGMENT: &str = "render/objects/object.wesl";
    const VERTEX: &str = "render/lighting/game_vertex.wesl";
    let Some(flags) = hook_defs(hash) else { return };
    let with_flags = |defs: &[bevy::shader::ShaderDefVal]| {
        let mut out = defs.to_vec();
        out.extend(flags.iter().cloned());
        out
    };
    let prepass = descriptor
        .vertex
        .shader_defs
        .iter()
        .any(|d| matches!(d, bevy::shader::ShaderDefVal::Bool(n, true) if n == "PREPASS_PIPELINE"));
    if prepass {
        if let Some((shader, _, true)) = variant_shader(hash, PREPASS) {
            let defs = with_flags(&descriptor.vertex.shader_defs);
            if admit(hash, PREPASS, &defs) {
                descriptor.vertex.shader = shader;
                descriptor.vertex.shader_defs = defs;
            }
        }
        if crate::render::gbuffer::is_normal_prepass(descriptor)
            && let Some((shader, true, _)) = variant_shader(hash, GBUFFER)
            && let Some(f) = descriptor.fragment.as_mut()
        {
            let defs = with_flags(&f.shader_defs);
            if admit(hash, GBUFFER, &defs) {
                f.shader = shader;
                f.shader_defs = defs;
            }
        }
        return;
    }
    let (Some((fragment, ..)), Some((vertex, ..)), Some(f)) =
        (variant_shader(hash, FRAGMENT), variant_shader(hash, VERTEX), descriptor.fragment.as_mut())
    else {
        return;
    };
    let fragment_defs = with_flags(&f.shader_defs);
    let vertex_defs = with_flags(&descriptor.vertex.shader_defs);
    let (fragment_ok, vertex_ok) = (admit(hash, FRAGMENT, &fragment_defs), admit(hash, VERTEX, &vertex_defs));
    if !(fragment_ok && vertex_ok) {
        return;
    }
    f.shader = fragment;
    f.shader_defs = fragment_defs;
    descriptor.vertex.shader = vertex;
    descriptor.vertex.shader_defs = vertex_defs;
}

fn follow_hooks(mut seen: Local<u64>, mut materials: ResMut<Assets<ObjectMaterial>>) {
    use crate::render::hooks::runtime::{drawn, epoch, settled};
    let now = settled();
    if now == *seen {
        return;
    }
    *seen = now;
    let changes: Vec<(AssetId<ObjectMaterial>, Option<u64>, u64)> = materials
        .iter()
        .filter_map(|(id, m)| {
            let (hash, key) = m.extension.hook_wanted.as_ref()?;
            let d = drawn(*hash, key);
            let round = d.map_or(0, epoch);
            (d != m.extension.hook || round != m.extension.hook_epoch).then_some((id, d, round))
        })
        .collect();
    for (id, d, round) in changes {
        if let Some(mut m) = materials.get_mut(id) {
            m.extension.hook = d;
            m.extension.hook_epoch = round;
        }
    }
}

fn specialize_ldr_translucent(descriptor: &mut RenderPipelineDescriptor) {
    let Some(fragment) = descriptor.fragment.as_mut() else { return };
    let Some(Some(target)) = fragment.targets.first_mut() else { return };
    target.format = LDR_FRAME_FORMAT;
    target.blend = Some(BlendState {
        color: BlendComponent {
            src_factor: BlendFactor::SrcAlpha,
            dst_factor: BlendFactor::OneMinusSrcAlpha,
            operation: BlendOperation::Add,
        },
        alpha: BlendComponent {
            src_factor: BlendFactor::One,
            dst_factor: BlendFactor::One,
            operation: BlendOperation::Add,
        },
    });
    fragment.shader_defs.push("SSX_LDR_TRANSLUCENT".into());
    descriptor.label = Some(LDR_PIPELINE_LABEL.into());
}

fn ldr_tonemap_neutral() -> Vec<u8> {
    vec![0; LDR_TONEMAP_BYTES as usize]
}

#[derive(Clone, Copy, Debug, Default, Reflect, ShaderType)]
pub struct ObjectParams {
    pub shading: Vec4,
    pub tiles: Vec4,
    pub snow: Vec4,
    pub snow_fall: Vec4,
    pub scatter: Vec4,
    pub lod_bias: Vec4,
    pub alpha_test: Vec4,
}
