use crate::render::reload::Recipe;
use crate::render::uploads::{LevelImage, LevelUploads};
use std::path::Path;

use anyhow::{Context, Result};
use bevy::asset::RenderAssetUsages;
use bevy::image::ImageSampler;
use bevy::prelude::{App, Assets, Handle, Image, Resource, Vec3, Vec4, World, default};
use bevy::render::RenderApp;
use bevy::render::extract_resource::{ExtractResource, ExtractResourcePlugin};
use bevy::render::render_resource::{Extent3d, ShaderType, TextureDimension, TextureFormat};
use ssx_assets::postfx_overlay::{OverlayDrop, OverlayParams, SurvivalOverlay, UV_SCALE_16_9, texture_path};

#[derive(Clone, Copy, Debug, Default, ShaderType)]
pub struct OverlayUniform {
    pub tint0: Vec4,
    pub tint1: Vec4,
    pub fadeoffbounds: Vec4,
    pub uvscale: Vec4,
}

impl OverlayUniform {
    pub fn set(&mut self, params: Option<OverlayParams>) {
        match params {
            Some(p) => {
                self.tint0 = Vec4::from_array(p.tint_intensity[0]);
                self.tint1 = Vec4::from_array(p.tint_intensity[1]);
                self.fadeoffbounds = Vec3::from_array(p.fadeoffbounds).extend(1.0);
            }
            None => self.fadeoffbounds.w = 0.0,
        }
    }
}

#[derive(Resource, Clone, ExtractResource)]
#[extract_app(RenderApp)]
pub struct PostOverlay {
    pub uniform: OverlayUniform,
    pub maps: [Handle<Image>; 2],
}

#[derive(Resource)]
pub struct ScreenEffects {
    pub overlay: SurvivalOverlay,
}

fn blank() -> Image {
    let size = Extent3d { width: 1, height: 1, depth_or_array_layers: 1 };
    let mut image =
        Image::new_uninit(size, TextureDimension::D2, TextureFormat::Rgba8Unorm, RenderAssetUsages::RENDER_WORLD);
    image.data = Some(vec![255; 4]);
    image
}

pub(super) fn build(app: &mut App) {
    app.add_plugins(ExtractResourcePlugin::<PostOverlay>::default());
    reset(app.world_mut());
}

pub(super) fn reset(world: &mut World) {
    let Some(mut images) = world.get_resource_mut::<Assets<Image>>() else { return };
    let maps = [images.add(blank()), images.add(blank())];
    let uniform = OverlayUniform { uvscale: Vec4::from_array(UV_SCALE_16_9), ..default() };
    world.insert_resource(PostOverlay { uniform, maps });
}

fn overlay_image(big_root: &Path, name: &str) -> Result<(Image, Recipe)> {
    use crate::render::terrain::textures::{Texels, load_chain, plane_recipe, texture_2d};
    let rel = texture_path(name);
    let id = ssx_assets::content::textures::texture_id(&rel).with_context(|| format!("{rel:?}: not a texture id"))?;
    let want = ssx_content::texture::Want::PLANE;
    let chain = load_chain(big_root, &id, &want)?;
    let recipe = plane_recipe(big_root, &id, want, Texels::Linear, ImageSampler::Default);
    Ok((texture_2d(&chain, Texels::Linear, ImageSampler::Default), recipe))
}

pub struct OverlayLoad {
    overlay: SurvivalOverlay,
    maps: [LevelImage; 2],
}

impl OverlayLoad {
    pub fn queue_images(&mut self, uploads: &mut LevelUploads, images: &mut Assets<Image>) {
        for m in &mut self.maps {
            m.queue(uploads, images);
        }
    }
}

pub fn load(big_root: &Path, drop: OverlayDrop) -> Result<OverlayLoad> {
    let overlay = SurvivalOverlay::of(drop);
    let built = |(image, recipe): (Image, Recipe)| LevelImage::built(image, Some(recipe));
    let maps = [built(overlay_image(big_root, overlay.maps[0])?), built(overlay_image(big_root, overlay.maps[1])?)];
    Ok(OverlayLoad { overlay, maps })
}

pub fn insert(world: &mut World, load: OverlayLoad) -> Result<String> {
    let OverlayLoad { overlay, maps: [a, b] } = load;
    let maps = {
        let mut images = world.get_resource_mut::<Assets<Image>>().context("no image assets")?;
        [a, b].map(|m| m.into_handle(&mut images))
    };
    let mut post = world.get_resource_mut::<PostOverlay>().context("the post chain is off")?;
    post.maps = maps;
    let line = format!("screen effects: {:?}, maps {:?}", overlay.drop, overlay.maps);
    world.insert_resource(ScreenEffects { overlay });
    Ok(line)
}
