pub(crate) mod midground;
pub mod package;
mod package_gpu;
mod render;

use crate::render::reload::Recipe;
use std::path::Path;

use anyhow::{Context, Result};
use bevy::asset::{RenderAssetUsages, embedded_asset};
use bevy::prelude::{
    App, Assets, Handle, Image, IntoScheduleConfigs, Plugin, Res, ResMut, Resource, Update, Vec3, Vec4, World, default,
    warn,
};
use bevy::render::RenderApp;
use bevy::render::extract_resource::ExtractResource;
use bevy::render::render_resource::{Extent3d, ShaderType, TextureDimension, TextureFormat};

use crate::render::lighting::GameLighting;

const SKY_DISTANCE_KM: f32 = 400.0;
const CLOUD_DISTANCE_SCALE: f32 = 0.5;
const K_RAYLEIGH: f32 = f32::from_bits(0x0DC9_7207);
const K_RAYLEIGH_DASH: f32 = f32::from_bits(0x0BC0_5DBF);
const K_MIE: f32 = f32::from_bits(0x277F_2301);
const K_MIE_DASH: f32 = f32::from_bits(0x2622_6CD2);
const WAVELENGTH_SCALE: f32 = f32::from_bits(0x3586_37BD);

pub struct SkyPlugin;

#[derive(Clone, Copy, Debug, Default, ShaderType)]
pub struct SkyUniform {
    pub beta_r: Vec4,
    pub beta_dash_r: Vec4,
    pub beta_m: Vec4,
    pub beta_dash_m: Vec4,
    pub heights: Vec4,
    pub multipliers: Vec4,
    pub light_dir: Vec4,
    pub light_colour: Vec4,
    pub cloud_ramp0: Vec4,
    pub cloud_ramp1: Vec4,
    pub cloud: Vec4,
    pub sun: Vec4,
}

#[derive(Resource, Clone, ExtractResource)]
#[extract_app(RenderApp)]
pub struct SkySettings {
    pub uniform: SkyUniform,
    pub cloud_density: Handle<Image>,
    pub cloud_normal: Handle<Image>,
    pub light_texture: Handle<Image>,
    pub midground: Option<midground::MidgroundSettings>,
    pub package: Option<package::PackageSky>,
}

impl Plugin for SkyPlugin {
    fn build(&self, app: &mut App) {
        embedded_asset!(app, "sky.wesl");
        embedded_asset!(app, "midground.wgsl");
        embedded_asset!(app, "sky_cube.wesl");
        embedded_asset!(app, "sky_dome.wesl");
        render::build(app);
        crate::shell::level::scoped::<SkySettings>(app);
        use crate::render::lighting::fade::{LightingFadeSet, faded};
        app.add_systems(Update, follow_lighting.after(LightingFadeSet).run_if(faded));
        app.add_systems(
            Update,
            dome_clock.run_if(bevy::ecs::schedule::common_conditions::resource_exists::<SkySettings>),
        );
    }
}

fn follow_lighting(lighting: Res<GameLighting>, settings: Option<ResMut<SkySettings>>) {
    let Some(mut settings) = settings else { return };
    settings.uniform = sky_uniform(&lighting);
    for part in settings.midground.iter_mut().flat_map(|m| &mut m.parts) {
        part.uniform = midground::relit(&lighting, &part.uniform);
    }
    if let (Some(p), Some(def)) = (settings.package.as_mut(), lighting.sky()) {
        match &mut p.background {
            package::Background::Cube { uniform, .. } => *uniform = package::cube_uniform(def, &lighting),
            package::Background::Dome { uniform, .. } => {
                let time = uniform.rotation.w;
                if let ssx_assets::content::level::LevelSky::Dome { material, .. } = &lighting.level.sky {
                    *uniform = package::dome_uniform(def, material, &lighting, time);
                }
            }
        }
    }
}

fn dome_clock(time: Res<bevy::prelude::Time>, settings: Option<ResMut<SkySettings>>) {
    let Some(mut settings) = settings else { return };
    if let Some(package::Background::Dome { uniform, .. }) = settings.package.as_mut().map(|p| &mut p.background) {
        uniform.rotation.w = time.elapsed_secs_wrapped();
    }
}

fn recip(x: f32) -> f32 {
    if x == 0.0 { 1e30 } else { 1.0 / x }
}

fn scattering(l: &GameLighting) -> [Vec3; 4] {
    let colour = l.v("atmosRayleighColour").truncate();
    let lambda = (Vec3::ONE - colour).abs() * WAVELENGTH_SCALE;
    let l2 = lambda * lambda;
    let l4 = l2 * l2;
    let (ray, mie) = (l.f("atmosRayleighMultiplier"), l.f("atmosMieMultiplier"));
    let mie_colour = l.v("atmosMieColour").truncate();
    [
        Vec3::splat(K_RAYLEIGH) / l4 * ray,
        Vec3::splat(K_RAYLEIGH_DASH) / l4 * ray,
        Vec3::splat(K_MIE) / l2 * mie_colour * mie,
        Vec3::splat(K_MIE_DASH) / l2 * mie,
    ]
}

fn henyey_greenstein(l: &GameLighting) -> f32 {
    l.preset.float("atmosHenyeyGreensteinParam").or_else(|| l.preset.float("0xceafcae0")).unwrap_or(0.0)
}

fn sky_uniform(l: &GameLighting) -> SkyUniform {
    let [beta_r, beta_dash_r, beta_m, beta_dash_m] = scattering(l);
    let glm = l.f("globalLightingMult");
    let use_light_texture = f32::from(u8::from(l.preset.textures.contains_key("dirLightTexture")));
    let ramp = |name: &str, w: f32| l.v(name).truncate().extend(w);
    SkyUniform {
        beta_r: beta_r.extend(henyey_greenstein(l)),
        beta_dash_r: beta_dash_r.extend(l.f("atmosPlanetRadius")),
        beta_m: beta_m.extend(SKY_DISTANCE_KM),
        beta_dash_m: beta_dash_m.extend(CLOUD_DISTANCE_SCALE),
        heights: Vec4::new(
            recip(l.f("atmosRayleighHeightFalloff")),
            l.f("atmosRayleighHeightOffset"),
            recip(l.f("atmosMieHeightFalloffSky")),
            l.f("atmosMieHeightOffsetSky"),
        ),
        multipliers: Vec4::new(
            l.f("atmosExtinctionMultiplier"),
            l.f("atmosInScatteringMultiplier"),
            glm,
            use_light_texture,
        ),
        light_dir: l.v("dirLight_Direction").truncate().normalize_or_zero().extend(l.f("dirLightTextureUVScale")),
        light_colour: l.v("dirLight_Colour").truncate().extend(l.f("cloudTextureOffset")),
        cloud_ramp0: ramp("cloudRampColor0", l.f("cloudRampIntensity")),
        cloud_ramp1: ramp("cloudRampColor1", l.f("cloudDensityMultiplier")),
        cloud: Vec4::new(
            l.f("cloudDensityRimLevel"),
            recip(l.f("cloudDensityRimSize")),
            l.f("skySunRimIntensity"),
            l.f("skySunRimFalloff"),
        ),
        sun: Vec4::new(l.f("skySunIntensity"), l.f("skySunFalloff"), recip(glm), 0.0),
    }
}

fn load_texture(big_root: &Path, rel: &str, l: &GameLighting) -> Result<(Image, Option<Recipe>)> {
    use crate::render::terrain::textures::{Texels, load_chain, plane_recipe, texture_2d};
    let id =
        ssx_assets::content::presets::preset_texture_id(rel).with_context(|| format!("{rel:?}: not a texture id"))?;
    if l.level.textures.contains_key(&id) {
        return Ok((package::decided_texture(big_root, l, &id)?, None));
    }
    let want = ssx_content::texture::Want::PLANE;
    let chain = load_chain(big_root, &id, &want)?;
    let recipe = plane_recipe(big_root, &id, want, Texels::Linear, default());
    Ok((texture_2d(&chain, Texels::Linear, default()), Some(recipe)))
}

fn texel(rgba: [u8; 4]) -> Image {
    let size = Extent3d { width: 1, height: 1, depth_or_array_layers: 1 };
    Image::new(size, TextureDimension::D2, rgba.to_vec(), TextureFormat::Rgba8Unorm, RenderAssetUsages::RENDER_WORLD)
}

pub struct SkyLoad {
    uniform: SkyUniform,
    images: [crate::render::uploads::LevelImage; 3],
    midground: Option<midground::MidgroundLoad>,
    package: Option<package::PackageLoad>,
    pub status: String,
}

pub fn load(lighting: &GameLighting, root: &Path) -> Result<SkyLoad> {
    if crate::render::post::env_is("SSX_GAME_SKY", "off") {
        anyhow::bail!("disabled by SSX_GAME_SKY=off");
    }
    let tex = |name: &str| lighting.preset.textures.get(name).cloned();
    let density = tex("cloudDensityTexture").context("preset has no cloudDensityTexture")?;
    let normal = tex("cloudDensityNormalTexture").context("preset has no cloudDensityNormalTexture")?;
    let moon = tex("dirLightTexture");
    let density_img = load_texture(root, &density, lighting)?;
    let normal_img = load_texture(root, &normal, lighting)?;
    let light_img = match &moon {
        Some(rel) => load_texture(root, rel, lighting)?,
        None => (texel([0, 0, 0, 0]), None),
    };
    let uniform = sky_uniform(lighting);
    let name = |rel: &str| rel.rsplit('\\').next().unwrap_or(rel).to_string();
    let package = package::load(lighting, root).inspect_err(|e| warn!("the level's package sky: {e:#}")).ok().flatten();
    let midground = if package.as_ref().is_some_and(|p| !p.midground) {
        Ok(None)
    } else {
        midground::load(lighting, root)
            .inspect_err(|e| warn!("mid-ground mountains off: {e:#}"))
            .map_err(|e| format!("mid-ground: off ({e:#})"))
    };
    let midground_status = match &midground {
        Ok(Some(m)) => m.status.clone(),
        Ok(None) => "mid-ground: none placed by the track".to_string(),
        Err(e) => e.clone(),
    };
    let status = format!(
        "sky: sky.fxo port (scattering to {SKY_DISTANCE_KM} km, HG g {}, clouds {} / {}{}, sun disc) | written / gLM; \
         shader atan2/acos polynomials; samplers + depth/stencil from sky.fxo",
        uniform.beta_r.w,
        name(&density),
        name(&normal),
        moon.as_deref().map_or_else(String::new, |m| format!(", moon {}", name(m))),
    ) + "\n"
        + &midground_status;
    let midground = midground.ok().flatten();
    let status = match &package {
        Some(p) => format!("{}\n{status}", p.status),
        None => status,
    };
    let images = [density_img, normal_img, light_img].map(|(i, r)| crate::render::uploads::LevelImage::built(i, r));
    Ok(SkyLoad { uniform, images, midground, package, status })
}

impl SkyLoad {
    pub fn queue_images(&mut self, uploads: &mut crate::render::uploads::LevelUploads, images: &mut Assets<Image>) {
        for i in &mut self.images {
            i.queue(uploads, images);
        }
        if let Some(m) = self.midground.as_mut() {
            m.queue_images(uploads, images);
        }
        if let Some(p) = self.package.as_mut() {
            p.queue_images(uploads, images);
        }
    }
}

pub fn insert(world: &mut World, load: SkyLoad) -> Result<()> {
    let [density, normal, light] = load.images;
    let mut images = world.get_resource_mut::<Assets<Image>>().context("no image assets")?;
    let (cloud_density, cloud_normal, light_texture) =
        (density.into_handle(&mut images), normal.into_handle(&mut images), light.into_handle(&mut images));
    let settings = SkySettings {
        uniform: load.uniform,
        cloud_density,
        cloud_normal,
        light_texture,
        midground: load.midground.map(|m| m.into_settings(&mut images)),
        package: load.package.map(|p| p.into_settings(&mut images)),
    };
    world.insert_resource(settings);
    Ok(())
}
