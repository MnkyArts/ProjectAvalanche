use ssx_content::ContentId;
use std::collections::HashMap;

use bevy::asset::embedded_asset;
use bevy::mesh::MeshVertexBufferLayoutRef;
use bevy::pbr::{
    ExtendedMaterial, MaterialExtension, MaterialExtensionKey, MaterialExtensionPipeline, MeshPipelineKey,
};
use bevy::prelude::*;
use bevy::render::render_resource::{AsBindGroup, RenderPipelineDescriptor, ShaderType, SpecializedMeshPipelineError};
use bevy::shader::ShaderRef;

use crate::render::shadows::{ShadowPassKey, specialize_shadow_pass};

use crate::render::lighting::game::{GameLightAssets, GameLightUniform, GameLightingPlugin};
use crate::render::objects::material::ObjectMaterialPlugin;
use crate::render::snow_tracks::{SnowTrackParams, SnowTrackTextures};
use crate::render::terrain::look::TerrainLook;
use crate::rider::material::CharacterMaterialPlugin;

const SHADER_PATH: &str = "embedded://ssx_bevy/render/terrain/terrain.wesl";
const PREPASS_SHADER_PATH: &str = "embedded://ssx_bevy/render/terrain/terrain_prepass.wesl";
const PREPASS_FRAGMENT_PATH: &str = "embedded://ssx_bevy/render/terrain/terrain_normal.wesl";

pub const SHADER_STATUS: &str = "\
terrain (microcode port): tri-planar max(|n|*7-1.4,0), UDN base normals, <=4 top-down snow types + scatter, \
coverage ramp + smoothstep(N.-fall),\n  albedo tex^2 / snow 0.5, Phong exp+intensity, fades 75-125/440-480 m, \
material pass dl*lerp(rampColor1,1,AO)*albedo + spec*(gsc*sl*AO + env) + glint,\n  \
env = pow(fresnel*cube(R_view), reflectEnvExponent), sparkle (snowSparkle*), AO = COLOR1.z via maxAOAttenuation; \
COLOR0/1 packed as the exe (0x82A04520)\n\
terrain: technique set per face group (unk_a bits 1/2), effect samplers, fog per vertex; \
game vertex normals (unit face normals averaged per mesh, not renormalised); ride snow tracks (DispTilesMap, CPU port)";

pub type TerrainMaterial = ExtendedMaterial<StandardMaterial, TerrainExtension>;

pub struct TerrainMaterialPlugin;

impl Plugin for TerrainMaterialPlugin {
    fn build(&self, app: &mut App) {
        embedded_asset!(app, "terrain.wesl");
        embedded_asset!(app, "terrain_prepass.wesl");
        embedded_asset!(app, "terrain_normal.wesl");
        embedded_asset!(app, "proxy.wesl");
        embedded_asset!(app, "proxy_vertex.wesl");
        bevy::shader::load_shader_library!(app, "terrain_gbuffer.wesl");
        bevy::shader::load_shader_library!(app, "terrain_ripple.wesl");
        app.add_plugins((
            GameLightingPlugin,
            MaterialPlugin::<TerrainMaterial>::default(),
            MaterialPlugin::<crate::render::terrain::proxy::ProxyMaterial>::default(),
            ObjectMaterialPlugin,
            CharacterMaterialPlugin,
        ))
        .add_systems(Update, toggle_fades)
        .add_systems(PostUpdate, terrain_ripple);
    }

    fn finish(&self, app: &mut App) {
        crate::render::terrain::textures::init_block_textures(app);
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Reflect, ShaderType)]
pub struct TerrainRippleUniform {
    pub a: Vec4,
    pub b: Vec4,
}

fn ripple_override() -> Option<TerrainRippleUniform> {
    static FIXED: std::sync::OnceLock<Option<TerrainRippleUniform>> = std::sync::OnceLock::new();
    *FIXED.get_or_init(|| {
        let v: Vec<f32> =
            std::env::var("SSX_TERRAIN_RIPPLE").ok()?.split(',').filter_map(|x| x.trim().parse().ok()).collect();
        let v: [f32; 8] = v.try_into().ok()?;
        Some(TerrainRippleUniform { a: Vec4::new(v[0], v[1], v[2], v[3]), b: Vec4::new(v[4], v[5], v[6], v[7]) })
    })
}

fn terrain_ripple(
    ripple: Option<Res<crate::ride::TerrainRipple>>,
    mut materials: ResMut<Assets<TerrainMaterial>>,
    mut last: Local<TerrainRippleUniform>,
) {
    let u = ripple_override().unwrap_or_else(|| {
        ripple.map_or_else(Default::default, |r| {
            let p = r.params;
            TerrainRippleUniform {
                a: Vec4::new(p.amplitude, p.falloff, p.frequency, p.phase),
                b: p.position.extend(p.start),
            }
        })
    });
    if *last == u {
        return;
    }
    if last.a.x == 0.0 && u.a.x > 0.0 {
        info!("terrain ripple at {:.1}: amplitude {:.3}, frequency {:.3}", u.b.truncate(), u.a.x, u.a.z);
    }
    *last = u;
    for (_, m) in materials.iter_mut() {
        m.extension.ripple = u;
    }
}

#[derive(Resource, Clone, Copy, Debug)]
pub struct TerrainFades(pub bool);

fn toggle_fades(
    keys: Res<ButtonInput<KeyCode>>,
    fades: Option<ResMut<TerrainFades>>,
    mut materials: ResMut<Assets<TerrainMaterial>>,
) {
    let Some(mut fades) = fades else { return };
    if !keys.just_pressed(KeyCode::KeyG) {
        return;
    }
    fades.0 = !fades.0;
    for (_, material) in materials.iter_mut() {
        material.extension.params.snow_blend.w = f32::from(u8::from(fades.0));
    }
}

#[derive(Asset, AsBindGroup, Reflect, Debug, Clone)]
#[bind_group_data(TerrainKey)]
pub struct TerrainExtension {
    #[uniform(100)]
    pub params: TerrainParams,
    #[texture(101)]
    pub rock_diffuse_1: Handle<Image>,
    #[texture(102)]
    pub rock_normal_1: Handle<Image>,
    #[texture(111)]
    pub rock_diffuse_2: Handle<Image>,
    #[texture(112)]
    pub rock_normal_2: Handle<Image>,
    #[texture(103, dimension = "2d_array")]
    #[sampler(104)]
    pub snow_normal: Handle<Image>,
    #[texture(105)]
    pub scatter_normal: Handle<Image>,
    #[uniform(106)]
    pub light: GameLightUniform,
    #[storage(140, read_only)]
    pub local_lights: Handle<bevy::render::storage::ShaderBuffer>,
    #[texture(141, dimension = "2d_array")]
    #[sampler(142)]
    pub local_light_textures: Handle<Image>,
    #[texture(107, dimension = "cube")]
    #[sampler(108)]
    pub reflect_cube: Handle<Image>,
    #[texture(109)]
    #[sampler(110)]
    pub sparkle_normal: Handle<Image>,
    #[uniform(113)]
    pub tracks: SnowTrackParams,
    #[texture(114, sample_type = "u_int")]
    pub track_lookup: Handle<Image>,
    #[texture(115, dimension = "2d_array")]
    #[sampler(116)]
    pub track_tiles: Handle<Image>,
    #[uniform(117)]
    pub ripple: TerrainRippleUniform,
    pub shadow_pass: ShadowPassKey,
    pub simple: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TerrainKey {
    shadow_pass: ShadowPassKey,
    simple: bool,
}

impl From<&TerrainExtension> for TerrainKey {
    fn from(e: &TerrainExtension) -> Self {
        TerrainKey { shadow_pass: e.shadow_pass, simple: e.simple }
    }
}

pub const SIMPLE_SET: usize = 3;

impl MaterialExtension for TerrainExtension {
    fn vertex_shader() -> ShaderRef {
        crate::render::lighting::game::GAME_VERTEX_PATH.into()
    }

    fn fragment_shader() -> ShaderRef {
        SHADER_PATH.into()
    }

    fn prepass_vertex_shader() -> ShaderRef {
        PREPASS_SHADER_PATH.into()
    }

    fn prepass_fragment_shader() -> ShaderRef {
        PREPASS_FRAGMENT_PATH.into()
    }

    fn specialize(
        _pipeline: &MaterialExtensionPipeline,
        descriptor: &mut RenderPipelineDescriptor,
        _layout: &MeshVertexBufferLayoutRef,
        key: MaterialExtensionKey<Self>,
    ) -> Result<(), SpecializedMeshPipelineError> {
        let shadow = key.mesh_key.contains(MeshPipelineKey::UNCLIPPED_DEPTH_ORTHO);
        specialize_shadow_pass(descriptor, key.mesh_key, key.bind_group_data.shadow_pass);
        if crate::render::gbuffer::is_depth_prepass(descriptor, key.mesh_key) {
            crate::render::gbuffer::depth_prepass_writes(descriptor, true);
        }
        crate::render::gbuffer::invariant_main_pass(descriptor);
        crate::render::gbuffer::specialize_main_pass(descriptor);
        crate::render::lighting::game::specialize_game_vertex(descriptor, 106);
        descriptor.vertex.shader_defs.push("SSX_RAW_NORMAL".into());
        if !key.bind_group_data.simple || shadow {
            descriptor.vertex.shader_defs.push("SSX_TERRAIN_RIPPLE".into());
        }
        // PORT(choice): the simple techniques' set (3) is compiled into its own fragment shaders (the main pass and
        // the normal prepass) instead of read from `snowFallDirection.w` at run time: with the set a constant the
        // compiler drops what set 3 never runs (the snow detail and its fade, the reflection that set 3 multiplies by
        // a fade of 0, the sparkle), the same values on what it runs. The key already splits the simple materials'
        // pipelines (the ripple), so no pipeline is added. The console drew set 3 with terrain_default_simple.fxo's
        // own techniques (0x829BE6FC / 0x829BE788).
        if key.bind_group_data.simple
            && !shadow
            && let Some(fragment) = descriptor.fragment.as_mut()
        {
            fragment.shader_defs.push("SSX_TERRAIN_SIMPLE".into());
        }
        Ok(())
    }
}

pub fn shadow_pass() -> ShadowPassKey {
    ShadowPassKey::of("terrain_default", "render_shadowbuffer")
}

#[derive(Clone, Copy, Debug, Default, Reflect, ShaderType)]
pub struct TerrainParams {
    pub base: [Vec4; 2],
    pub snow_tile: [Vec4; 2],
    pub snow_eccentricity: [Vec4; 2],
    pub snow_specular: [Vec4; 2],
    pub planar_scatter: Vec4,
    pub snow_blend: Vec4,
    pub snow_fall: Vec4,
    pub reflect: Vec4,
}

impl TerrainParams {
    pub fn from_look(look: &TerrainLook, fades: bool) -> Self {
        let base = |i: usize| {
            let s = &look.base[i];
            Vec4::new(s.tile, s.normal_scale, s.eccentricity, s.specular_intensity)
        };
        let layers = |f: &dyn Fn(usize) -> f32| [Vec4::new(f(0), f(1), f(2), f(3)), Vec4::new(f(4), f(5), 0.0, 0.0)];
        TerrainParams {
            base: [base(0), base(1)],
            snow_tile: layers(&|i| look.snow[i].tile),
            snow_eccentricity: layers(&|i| look.snow[i].eccentricity),
            snow_specular: layers(&|i| look.snow[i].specular_intensity),
            planar_scatter: Vec4::new(
                look.planar_blend_scale,
                look.planar_blend_scale * look.planar_blend_offset,
                look.scatter_tile,
                look.scatter_amount,
            ),
            snow_blend: Vec4::new(
                look.snow_blend_offset,
                look.snow_blend_point,
                look.snow_blend_range,
                f32::from(u8::from(fades)),
            ),
            snow_fall: Vec3::from_array(look.snow_fall_direction).extend(0.0),
            reflect: Vec4::new(look.reflect_env_exponent, look.reflect_env_fresnel, 0.0, 0.0),
        }
    }
}

const GBUFFER: &str = "prelight_gbuffer_4SnowTypes_2BaseSurfaces";
const MATERIAL: &str = "prelight_material_2BaseSurfaces_Snow";

fn terrain_samplers() -> anyhow::Result<[crate::render::fx_states::SamplerStates; 5]> {
    use crate::render::fx_states::sampler;
    let fx = "terrain_default";
    Ok([
        sampler(fx, MATERIAL, "remapBaseSurface1DiffuseMapSampler")?,
        sampler(fx, GBUFFER, "remapBaseSurface1NormalMapSampler")?,
        sampler(fx, GBUFFER, "snowType1NormalMapSampler")?,
        sampler(fx, GBUFFER, "scatterNormalMapSampler")?,
        sampler(fx, MATERIAL, "sparkleNormalMapSampler")?,
    ])
}

pub struct PreparedLooks {
    looks: Vec<(TerrainParams, LookImages)>,
    images: crate::render::uploads::PartImages,
}

struct LookImages {
    diffuse: [usize; 2],
    normal: [usize; 2],
    snow: usize,
    scatter: usize,
    sparkle: usize,
}

impl PreparedLooks {
    pub fn decode(big_root: &std::path::Path, looks: &[TerrainLook]) -> anyhow::Result<PreparedLooks> {
        use crate::render::terrain::textures::{
            Texels, array_recipe, array_texture, decode_all, fit_layers, plane_recipe, texture_2d,
        };
        use ssx_content::texture::Want;
        let want = |texels: Texels| if texels == Texels::NormalXy { Want::NORMAL } else { Want::PLANE };
        let items = looks.iter().flat_map(|look| {
            let base =
                look.base.iter().flat_map(|s| [(s.diffuse.clone(), Want::PLANE), (s.normal.clone(), Want::NORMAL)]);
            base.chain(look.snow.iter().map(|l| (l.normal.clone(), Want::NORMAL)))
                .chain([(look.scatter.clone(), Want::PLANE), (look.sparkle.clone(), Want::PLANE)])
        });
        let chains = decode_all(big_root, items)?;
        let states = terrain_samplers()?;
        let [diffuse_s, normal_s, snow_s, scatter_s, _] = states;
        let shared = snow_s.descriptor()?;
        anyhow::ensure!(
            [diffuse_s, normal_s, scatter_s].iter().all(|s| s.descriptor().ok().as_ref() == Some(&shared)),
            "terrain_default.fxo: the base-surface / snow / scatter samplers differ"
        );
        let sampler_of = |kind: usize| -> anyhow::Result<bevy::image::ImageSampler> { states[kind].image_sampler() };
        let mut images = Vec::new();
        let mut index: HashMap<(Vec<ContentId>, Texels, usize), usize> = HashMap::new();
        let mut image = |layers: Vec<ContentId>, texels: Texels, array: bool, kind: usize| -> anyhow::Result<usize> {
            if let Some(&i) = index.get(&(layers.clone(), texels, kind)) {
                return Ok(i);
            }
            let w = want(texels);
            let layer_chains: Vec<_> = layers.iter().map(|id| &chains[&(id.clone(), w)]).collect();
            let sampler = sampler_of(kind)?;
            images.push(if array {
                let fitted = fit_layers(big_root, &layer_chains, &w);
                let layer_chains: Vec<_> =
                    layer_chains.iter().zip(&fitted).map(|(c, f)| f.as_ref().unwrap_or(c)).collect();
                let recipe = array_recipe(big_root, &layers, w, texels, sampler.clone());
                (array_texture(&layer_chains, texels, sampler)?, recipe)
            } else {
                let recipe = plane_recipe(big_root, &layers[0], w, texels, sampler.clone());
                (texture_2d(layer_chains[0], texels, sampler), recipe)
            });
            index.insert((layers, texels, kind), images.len() - 1);
            Ok(images.len() - 1)
        };
        let mut out = Vec::with_capacity(looks.len());
        for look in looks {
            let ids = LookImages {
                diffuse: [
                    image(vec![look.base[0].diffuse.clone()], Texels::Linear, false, 0)?,
                    image(vec![look.base[1].diffuse.clone()], Texels::Linear, false, 0)?,
                ],
                normal: [
                    image(vec![look.base[0].normal.clone()], Texels::NormalXy, false, 1)?,
                    image(vec![look.base[1].normal.clone()], Texels::NormalXy, false, 1)?,
                ],
                snow: image(look.snow.iter().map(|l| l.normal.clone()).collect(), Texels::NormalXy, true, 2)?,
                scatter: image(vec![look.scatter.clone()], Texels::Linear, false, 3)?,
                sparkle: image(vec![look.sparkle.clone()], Texels::Linear, false, 4)?,
            };
            let mut params = TerrainParams::from_look(look, true);
            params.reflect.z = scatter_s.lod_bias;
            out.push((params, ids));
        }
        Ok(PreparedLooks { looks: out, images: crate::render::uploads::PartImages::Built(images) })
    }

    pub fn queue_textures(&mut self, uploads: &mut crate::render::uploads::LevelUploads, images: &mut Assets<Image>) {
        self.images.queue(uploads, images);
    }

    pub fn into_materials(
        self,
        images: &mut Assets<Image>,
        light: &GameLightAssets,
        tracks: &SnowTrackTextures,
    ) -> Vec<[TerrainMaterial; SIMPLE_SET + 1]> {
        let handles = self.images.into_handles(images);
        let shadow_pass = shadow_pass();
        self.looks
            .into_iter()
            .map(|(params, LookImages { diffuse, normal, snow, scatter, sparkle })| {
                std::array::from_fn(|group| TerrainMaterial {
                    base: StandardMaterial { base_color: Color::WHITE, ..default() },
                    extension: TerrainExtension {
                        params: TerrainParams { snow_fall: params.snow_fall.with_w(group as f32), ..params },
                        rock_diffuse_1: handles[diffuse[0]].clone(),
                        rock_normal_1: handles[normal[0]].clone(),
                        rock_diffuse_2: handles[diffuse[1]].clone(),
                        rock_normal_2: handles[normal[1]].clone(),
                        snow_normal: handles[snow].clone(),
                        scatter_normal: handles[scatter].clone(),
                        light: light.uniform,
                        local_lights: crate::render::lighting::local::LOCAL_LIGHT_BUFFER,
                        local_light_textures: crate::render::lighting::local::LOCAL_LIGHT_TEXTURES,
                        reflect_cube: light.reflect_cube.clone(),
                        sparkle_normal: handles[sparkle].clone(),
                        tracks: tracks.params,
                        track_lookup: tracks.lookup.clone(),
                        track_tiles: tracks.tiles.clone(),
                        ripple: TerrainRippleUniform::default(),
                        shadow_pass,
                        simple: group == SIMPLE_SET,
                    },
                })
            })
            .collect()
    }
}
