use crate::render::reload::Recipe;
use crate::render::uploads::{LevelImage, LevelUploads};
use std::path::Path;

use anyhow::{Context, Result};
use bevy::asset::RenderAssetUsages;
use bevy::core_pipeline::prepass::{DepthPrepass, NormalPrepass};
use bevy::image::ImageSampler;
use bevy::prelude::{
    Add, App, Assets, Commands, Entity, Handle, Has, Image, Mat4, On, Query, Res, Resource, Update, Vec3, Vec4, With,
    World,
};
use bevy::render::RenderApp;
use bevy::render::extract_resource::{ExtractResource, ExtractResourcePlugin};
use bevy::render::render_resource::{Extent3d, ShaderType, TextureDimension, TextureFormat};
use ssx_assets::postfx_state::{PostFxScene, PostFxState};
use ssx_assets::postfx_topogrid::{TopogridParams, can_run, runs};

use super::GamePostCamera;

pub const GRID_TEXTURE: &str = "postfx_pulse_grid";

#[derive(Clone, Copy, Debug, Default, ShaderType)]
pub struct TopogridUniform {
    pub world_from_clip: Mat4,
    pub back: Vec4,
    pub colour: Vec4,
    pub gridcolour: Vec4,
    pub invrange: Vec4,
    pub origin: Vec4,
    pub scale: Vec4,
}

impl TopogridUniform {
    pub fn set(&mut self, params: Option<TopogridParams>) {
        match params {
            Some(p) => {
                self.colour = Vec4::new(p.colour[0], p.colour[1], p.colour[2], 1.0);
                self.gridcolour = Vec4::from_array(p.gridcolour);
                self.invrange = Vec4::from_array(p.invrange);
                self.origin = Vec3::from_array(p.origin).extend(p.pulsefade);
                self.scale = Vec4::new(p.scale, 0.0, 0.0, 0.0);
            }
            None => self.colour.w = 0.0,
        }
    }
}

#[derive(Resource, Clone, ExtractResource)]
#[extract_app(RenderApp)]
pub struct PostTopogrid {
    pub uniform: TopogridUniform,
    pub grid: Handle<Image>,
}

#[derive(Resource)]
pub struct TopogridLevel {
    pub gbuffer: bool,
}

fn blank() -> Image {
    let size = Extent3d { width: 1, height: 1, depth_or_array_layers: 1 };
    let mut image =
        Image::new_uninit(size, TextureDimension::D2, TextureFormat::Rgba8Unorm, RenderAssetUsages::RENDER_WORLD);
    image.data = Some(vec![0, 0, 0, 255]);
    image
}

pub(super) fn build(app: &mut App) {
    app.add_plugins(ExtractResourcePlugin::<PostTopogrid>::default())
        .add_systems(Update, follow_gbuffer)
        .add_observer(depth_prepass);
    crate::shell::level::scoped::<TopogridLevel>(app);
    reset(app.world_mut());
}

pub(super) fn reset(world: &mut World) {
    let Some(mut images) = world.get_resource_mut::<Assets<Image>>() else { return };
    let grid = images.add(blank());
    world.insert_resource(PostTopogrid { uniform: TopogridUniform::default(), grid });
}

fn grid_image(big_root: &Path) -> Result<(Image, Recipe)> {
    use crate::render::terrain::textures::{Texels, load_chain, plane_recipe, texture_2d};
    let rel = ssx_assets::postfx_overlay::texture_path(GRID_TEXTURE);
    let id = ssx_assets::content::textures::texture_id(&rel).with_context(|| format!("{rel:?}: not a texture id"))?;
    let want = ssx_content::texture::Want::PLANE;
    let chain = load_chain(big_root, &id, &want)?;
    let recipe = plane_recipe(big_root, &id, want, Texels::Linear, ImageSampler::Default);
    Ok((texture_2d(&chain, Texels::Linear, ImageSampler::Default), recipe))
}

pub struct TopogridLoad {
    switched: Vec<String>,
    gbuffer: bool,
    grid: Option<LevelImage>,
    secs: f32,
}

impl TopogridLoad {
    pub fn queue_images(&mut self, uploads: &mut LevelUploads, images: &mut Assets<Image>) {
        if let Some(g) = self.grid.as_mut() {
            g.queue(uploads, images);
        }
    }
}

pub fn load(big_root: &Path, scene: &PostFxScene, goggles: Option<&PostFxState>) -> Result<TopogridLoad> {
    let start = std::time::Instant::now();
    let gbuffer = goggles.is_some_and(runs) || can_run(scene);
    let grid = if gbuffer {
        let (image, recipe) = grid_image(big_root)?;
        Some(LevelImage::built(image, Some(recipe)))
    } else {
        None
    };
    let switched = scene
        .presets
        .iter()
        .filter(|(_, (_, st))| st.float(ssx_assets::postfx_topogrid::FIELD_ON).is_some())
        .map(|(n, (p, _))| format!("{n} (layer priority {p:?})"))
        .collect();
    Ok(TopogridLoad { switched, gbuffer, grid, secs: start.elapsed().as_secs_f32() })
}

pub fn insert(world: &mut World, load: TopogridLoad) -> Result<String> {
    let TopogridLoad { switched, gbuffer, grid, secs } = load;
    if let Some(grid) = grid {
        let handle = {
            let mut images = world.get_resource_mut::<Assets<Image>>().context("no image assets")?;
            grid.into_handle(&mut images)
        };
        world.get_resource_mut::<PostTopogrid>().context("the post chain is off")?.grid = handle;
    }
    let line = format!(
        "TOPOGRID: gbuffer pass {}, presets with the pass's switch: {} ({secs:.3} s)",
        if gbuffer { "on" } else { "off" },
        switched.join(", ")
    );
    world.insert_resource(TopogridLevel { gbuffer });
    Ok(line)
}

// PORT(choice): the level camera always draws Bevy 0.20's depth prepass (`DepthPrepass`) before its main pass, so the
// main pass shades only each pixel's nearest opaque surface (Mount Eddie at 1600 × 900: 18 % fewer fragments shaded,
// the opaque pass 2.8 ms of GPU instead of 3.5, the prepass 0.13). The prepass writes only depth the main pass writes,
// at the same value (`render::gbuffer::depth_prepass_writes`: the terrain and the disc's objects, their main pass's
// kills in it; every other game family writes none), and Bevy's main pass keeps `GreaterEqual` with depth writes, so
// each pixel keeps the surface it had. "The same value" by construction: both passes' vertex shaders compute the clip
// position by the same functions and declare it `@invariant` (render/lighting/game_position.wesl; naga emits the
// SPIR-V decoration `Invariant`, HLSL `precise`), which WGSL makes equal across entry points; the image gate found 0 px
// on Mesa (Intel), NVIDIA and AMD are not run yet.
// The trade: it costs the render thread its recording (Bevy's `early_prepass` node, 0.25 ms a frame on Mount Eddie at
// 1920 × 1080, traced, and one command buffer more) for 0.4–2.0 ms of GPU in the benchmark's rides; the characters,
// whose prepass does not match their main pass, are taken out of it (`rider::material`
// `characters_out_of_depth_prepass`). The console's deferred renderer drew a gbuffer pass of its own every frame.
fn depth_prepass(add: On<Add<GamePostCamera>>, mut commands: Commands) {
    commands.entity(add.entity).insert(DepthPrepass);
}

// PORT(choice): the level camera draws Bevy 0.20's normal prepass (`NormalPrepass`: each game material's prepass
// fragment shader writes its gbuffer normal, world space ×0.5 + 0.5, into an Rgb10a2Unorm target; the depth prepass
// goes to a Depth32Float copy) only while the level can run TOPOGRID (the equipped goggles' preset or a startable
// preset with `on` > 0.001), the pass's only reader; a frame with it costs the normal's writes. The port's own
// Bevy-material meshes (script markers, placeholders) take Bevy's prepass shaders. The console's deferred renderer
// drew its gbuffer pass (8-bit RT, view-space normal) every frame.
fn follow_gbuffer(
    level: Option<Res<TopogridLevel>>,
    cameras: Query<(Entity, Has<NormalPrepass>), With<GamePostCamera>>,
    mut commands: Commands,
) {
    let wanted = level.is_some_and(|l| l.gbuffer);
    for (camera, has) in &cameras {
        if wanted && !has {
            commands.entity(camera).insert(NormalPrepass);
        } else if !wanted && has {
            commands.entity(camera).remove::<NormalPrepass>();
        }
    }
}
