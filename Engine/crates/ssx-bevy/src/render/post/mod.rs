pub mod adapt;
pub mod cube;
pub mod fsr;
pub mod layers;
pub mod overlay;
mod render;
pub mod topogrid;

use std::f32::consts::PI;

use anyhow::{Context, Result};
use bevy::asset::{RenderAssetUsages, embedded_asset};
use bevy::prelude::{
    Added, App, Assets, Component, Handle, Image, IntoScheduleConfigs, Mesh3d, Msaa, Plugin, Query, Res, ResMut,
    Resource, Time, UVec4, Update, Vec4, With, World, info, resource_exists,
};
use bevy::render::RenderApp;
use bevy::render::extract_component::ExtractComponent;
use bevy::render::extract_resource::ExtractResource;
use bevy::render::render_resource::{Extent3d, ShaderType, TextureDimension, TextureFormat};
use bevy::shader::load_shader_library;
use ssx_assets::postfx;

use crate::render::lighting::GameLighting;

pub struct PostPlugin;

#[derive(Component, Clone, Copy, Debug, ExtractComponent)]
#[extract_app(RenderApp)]
pub struct GamePostCamera;

#[derive(Resource, Clone, Debug)]
#[allow(dead_code)]
pub struct PostStatus(pub String);

#[derive(Clone, Copy, Debug, Default, ShaderType)]
pub struct PostUniform {
    pub hable_a: Vec4,
    pub hable_b: Vec4,
    pub exposure: Vec4,
    pub bloom: Vec4,
    pub blur_h_samples: Vec4,
    pub blur_h_offsets: Vec4,
    pub blur_v_samples: Vec4,
    pub blur_v_offsets: Vec4,
    pub sizes: UVec4,
    pub grade: Vec4,
}

#[derive(Resource, Clone, ExtractResource)]
#[extract_app(RenderApp)]
pub struct PostSettings {
    pub uniform: PostUniform,
    pub lut: Handle<Image>,
    pub scene_ready: bool,
    pub converge: bool,
}

// PORT(choice): the bloom and the FXAA can be switched off for slower machines, each for a measured saving (1920 ×
// 1080 on the development laptop's Arc iGPU: bloom off 0.18 ms less GPU and 0.39 ms a frame, FXAA off 0.24–0.27 ms
// less GPU); off, the bloom is the chain's own no-bloom path (a lighting preset's `bloom.z` = 0: no threshold or blur
// passes, no bloom in the tone map, no particle bloom) and the composite reads the tone-mapped frame without FXAA. The
// Post-Processing switch (`post`) turns off the chain's look passes together — the bloom, the FXAA and the colour cube
// (the composite takes the tone-mapped values as an identity cube would) — and keeps everything that shows the player
// something: the exposure and the tone map, the survival overlays (the thin-air, cold and physical-damage vignettes
// of `world_entity_controllers_default.lua`'s post-FX tracks), the pulse goggles' TOPOGRID, the particles and the LDR
// translucents (the ghosts, the helicopter's windows, the geotags), all drawn in the chain. The console always drew
// the whole chain (FXAA forced on by FUN_82FD08B0).
#[derive(Resource, Clone, Copy, Debug, PartialEq, Eq, ExtractResource)]
#[extract_app(RenderApp)]
pub struct PostLevels {
    pub bloom: bool,
    pub fxaa: bool,
    pub post: bool,
}

impl Default for PostLevels {
    fn default() -> PostLevels {
        PostLevels { bloom: true, fxaa: true, post: true }
    }
}

impl PostLevels {
    pub fn of(bloom: bool, fxaa: bool) -> PostLevels {
        PostLevels { bloom, fxaa, post: !env_is("SSX_GAME_POST", "off") }
    }

    pub fn bloom_drawn(self) -> bool {
        self.bloom && self.post
    }

    pub fn fxaa_drawn(self) -> bool {
        self.fxaa && self.post
    }
}

impl Plugin for PostPlugin {
    fn build(&self, app: &mut App) {
        // PORT(choice): `post_common.wesl`, the WESL module the chain's shaders and the particles' (`particles.wesl`,
        // `particles_pass.wesl`) import, is loaded here at the plugin's build and kept for the app's life
        // (`load_shader_library!`, as Bevy 0.20 loads its own imported modules): Bevy's `ShaderLoader` loads no
        // `package::` import, so a module first asked for by the chain's `init_pipelines` in `RenderStartup` raced the
        // particles' `init_pipelines` there, and an importer that reached the `ShaderCache` first logged "module not
        // found" and was re-queued (it compiled the next frame). The console compiles its effects offline (the `.fxo`
        // files).
        load_shader_library!(app, "post_common.wesl");
        embedded_asset!(app, "post_downsample.wesl");
        embedded_asset!(app, "post_adapt.wesl");
        embedded_asset!(app, "post_bloom.wesl");
        embedded_asset!(app, "post_final.wesl");
        embedded_asset!(app, "post_fxaa.wesl");
        app.init_resource::<PostLevels>().add_systems(Update, (tick.run_if(resource_exists::<PostSettings>), msaa_off));
        use crate::render::lighting::fade::{LightingFadeSet, faded};
        app.add_systems(Update, follow_lighting.after(LightingFadeSet).run_if(faded));
        layers::build(app);
        overlay::build(app);
        topogrid::build(app);
        render::build(app);
        fsr::build(app);
        crate::shell::level::scoped::<PostSettings>(app);
        crate::shell::level::scoped::<cube::FrameCube>(app);
        crate::shell::level::scoped::<overlay::ScreenEffects>(app);
        app.add_systems(crate::shell::level::LevelStartup, |status: Option<Res<PostStatus>>| {
            if let Some(s) = status {
                info!("{}", s.0);
            }
        });
    }
}

pub fn reset_level(world: &mut World) {
    overlay::reset(world);
    topogrid::reset(world);
}

pub fn env_is(key: &str, value: &str) -> bool {
    std::env::var(key).is_ok_and(|v| v.eq_ignore_ascii_case(value))
}

fn hable(x: f32, [a, b, c, d, e, f]: [f32; 6]) -> f32 {
    ((x * (a * x + c * b) + d * e) / (x * (a * x + b) + d * f)) - e / f
}

fn gaussian_taps(sigma: f32) -> (Vec4, Vec4) {
    let mut w = [0.0f32; 9];
    for (i, v) in w.iter_mut().enumerate() {
        let x = i as f32 - 4.0;
        *v = (-x * x / (2.0 * sigma * sigma)).exp() / (2.0 * PI * sigma * sigma);
    }
    let sum: f32 = w.iter().sum();
    w.iter_mut().for_each(|v| *v /= sum);
    let outer = 3.0 + w[0] / (w[0] + w[1]);
    let inner = 1.0 + w[2] / (w[2] + w[3]);
    (Vec4::new(w[0] + w[1], w[2] + w[3], w[4], 0.0), Vec4::new(-outer, -inner, inner, outer))
}

fn lut_image(cube: &postfx::ColourCube) -> Image {
    let n = postfx::CUBE_SIZE as u32;
    let size = Extent3d { width: n, height: n, depth_or_array_layers: n };
    Image::new(size, TextureDimension::D3, cube.rgba8(), TextureFormat::Rgba8Unorm, RenderAssetUsages::RENDER_WORLD)
}

pub struct PostLoad {
    uniform: PostUniform,
    lut: Image,
    frame_cube: Option<cube::FrameCube>,
    converge: bool,
    pub status: String,
}

pub fn load(lighting: &GameLighting, root: &std::path::Path, converge: bool) -> Result<PostLoad> {
    let name = lighting.postfx.clone().context("the drop names no post-FX preset")?;
    let (preset, build) = postfx::load_presets(root, &[name.as_str()])?.pop().context("preset not built")?;
    let l = lighting;
    let ([a, b, c, d, e, f], white) = curve(l);
    let blur = preset.bloom_blur().context("preset has no bloom_blur_x/y")?;
    let (hs, ho) = gaussian_taps(blur[0]);
    let (vs, vo) = gaussian_taps(blur[1]);
    let glm = l.f("globalLightingMult");
    let mut uniform = PostUniform {
        bloom: Vec4::new(0.0, 0.0, f32::from(u8::from(preset.bloom_active())), 1.0),
        blur_h_samples: hs,
        blur_h_offsets: ho,
        blur_v_samples: vs,
        blur_v_offsets: vo,
        grade: Vec4::X,
        ..Default::default()
    };
    lighting_constants(l, &mut uniform);
    let identity = env_is("SSX_POST_LUT", "identity");
    let cube = if identity { postfx::ColourCube::identity() } else { build.cube.clone() };
    let lut = lut_image(&cube);
    let frame_cube =
        if identity { Ok(None) } else { cube::FrameCube::load(root, &preset.colour_cubes, &build).map(Some) };
    let (frame_cube, frames) = match frame_cube {
        Ok(c) => (c, ""),
        Err(e) => {
            bevy::prelude::warn!("the frames' colour cubes: the stacks did not load ({e:#}); the drop's cube stays");
            (None, " | the frames' colour cubes NOT built")
        }
    };

    let groups: Vec<&str> = preset.colour_cubes.iter().map(|c| c.group.as_str()).collect();
    let second = if build.needs_second_lut { " (depth-blended 2nd LUT NOT built)" } else { "" };
    let status = format!(
        "post: game HDR chain, preset {name} | x gLM {glm} | key {} rate {} | Hable A{a} B{b} C{c} D{d} E{e} F{f} W{white}\n\
         bloom {} thr {} stops w {} sigma {:?} | colour cube {}{second} (max delta {}){}\n\
         as the game: L16 luminance chain (BlitL16 mips), FXAA, no MSAA; samplers from hdr/postfx/fxaa.fxo",
        uniform.exposure.y,
        uniform.exposure.z,
        if preset.bloom_active() { "on" } else { "off" },
        uniform.bloom.x,
        uniform.bloom.y,
        blur,
        groups.join("+"),
        build.cube.max_deviation(),
        if identity { " BYPASSED (SSX_POST_LUT=identity)" } else { frames },
    );
    Ok(PostLoad { uniform, lut, frame_cube, converge, status })
}

fn curve(l: &GameLighting) -> ([f32; 6], f32) {
    let curve = [
        l.f("toneMappingShoulderStrength"),
        l.f("toneMappingLinearStrength"),
        l.f("toneMappingLinearAngle"),
        l.f("toneMappingToeStrength"),
        l.f("toneMappingToeNumerator"),
        l.f("toneMappingToeDenominator"),
    ];
    (curve, l.f("toneMappingLinearWhite"))
}

fn lighting_constants(l: &GameLighting, u: &mut PostUniform) {
    let (curve, white) = curve(l);
    let [a, b, c, d, e, f] = curve;
    let h_white = hable(white, curve);
    u.hable_a = Vec4::new(a, b, b * c, d * e);
    u.hable_b = Vec4::new(d * f, e / f, 1.0 / h_white, 0.0);
    u.exposure = Vec4::new(
        l.f("globalLightingMult"),
        l.f("dynamicExposureKeyValue"),
        l.f("dynamicExposureAdaptionRate"),
        u.exposure.w,
    );
    (u.bloom.x, u.bloom.y) = (l.f("bloom_threshold"), l.f("bloom_weight"));
}

fn follow_lighting(lighting: Res<GameLighting>, settings: Option<ResMut<PostSettings>>) {
    if let Some(mut settings) = settings {
        lighting_constants(&lighting, &mut settings.uniform);
    }
}

pub fn insert(world: &mut World, load: PostLoad) -> Result<()> {
    let lut = world.get_resource_mut::<Assets<Image>>().context("no image assets")?.add(load.lut);
    world.insert_resource(PostSettings { uniform: load.uniform, lut, scene_ready: false, converge: load.converge });
    if let Some(c) = load.frame_cube {
        world.insert_resource(c);
    }
    Ok(())
}

fn tick(time: Res<Time>, meshes: Query<(), With<Mesh3d>>, mut settings: ResMut<PostSettings>) {
    settings.uniform.exposure.w = time.delta_secs();
    settings.scene_ready = !meshes.is_empty();
}

fn msaa_off(mut cameras: Query<&mut Msaa, Added<GamePostCamera>>) {
    for mut msaa in &mut cameras {
        *msaa = Msaa::Off;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gaussian_taps_sum_to_one_and_mirror() {
        let (s, o) = gaussian_taps(3.0);
        assert!((2.0 * s.x + 2.0 * s.y + s.z - 1.0).abs() < 1e-6);
        assert!(o.x < -3.0 && o.x > -4.0 && o.y < -1.0 && o.y > -2.0 && o.z == -o.y && o.w == -o.x);
    }

    #[test]
    fn hable_is_zero_at_zero() {
        let curve = [0.22, 0.3, 0.1, 0.2, 0.01, 0.3];
        assert!(hable(0.0, curve).abs() < 1e-7);
        assert!(hable(3.0, curve) > hable(1.0, curve));
    }
}
