use anyhow::{Context, Result};
use bevy::asset::RenderAssetUsages;
use bevy::image::{ImageAddressMode, ImageFilterMode, ImageSampler, ImageSamplerDescriptor};
use bevy::prelude::{Image, warn};
use bevy::render::render_resource::{
    Extent3d, TextureDimension, TextureFormat, TextureUsages, TextureViewDescriptor, TextureViewDimension,
};
use ssx_assets::track_lights::text as text_texture;
use ssx_content::texture::Want;

use super::Projected;

pub(super) fn texture_array(big_root: &std::path::Path, sources: &[Projected]) -> Result<Image> {
    let content = ssx_assets::content::GameContent::of(big_root);
    let files: Vec<(ssx_content::ContentId, Want)> = sources
        .iter()
        .filter_map(|s| match s {
            Projected::File(rel) => Some(
                ssx_assets::content::textures::texture_id(rel)
                    .filter(|id| content.has(id))
                    .map(|id| (id, Want::PLANE))
                    .with_context(|| format!("projected texture {rel} not found")),
            ),
            Projected::Text { .. } => None,
        })
        .collect::<Result<_>>()?;
    let chains = crate::render::terrain::textures::decode_all(big_root, files.iter().cloned())?;
    let font = if sources.iter().any(|s| matches!(s, Projected::Text { .. })) {
        Some(text_texture::load_font(big_root)?)
    } else {
        None
    };
    let mut text_chains = Vec::new();
    for s in sources {
        if let (Projected::Text { string, colour }, Some(font)) = (s, &font) {
            let levels = text_texture::render_text_texture(font, string, colour.map(f32::from_bits))
                .with_context(|| format!("text projection {string:?}"))?;
            text_chains.push(text_layer(levels));
        }
    }
    let mut next_file = files.iter();
    let mut next_text = text_chains.iter();
    let layers: Vec<&crate::render::terrain::textures::MipChain> = sources
        .iter()
        .map(|s| match s {
            Projected::File(_) => next_file.next().and_then(|f| chains.get(f)),
            Projected::Text { .. } => next_text.next(),
        })
        .collect::<Option<_>>()
        .context("projected texture layers out of step")?;
    let fitted = crate::render::terrain::textures::fit_layers(big_root, &layers, &Want::PLANE);
    let layers: Vec<_> = layers.iter().zip(&fitted).map(|(c, f)| f.as_ref().unwrap_or(c)).collect();
    let sampler = crate::render::fx_states::sampler(
        "prelighting",
        "render_textureprojectionlight_spec_farz",
        "DynamicLight_ProjectedTextureSampler",
    )
    .and_then(|s| s.image_sampler())
    .unwrap_or_else(|e| {
        warn!("projected texture sampler: {e:#}; clamp + trilinear");
        clamp_trilinear()
    });
    let image = crate::render::terrain::textures::array_texture(
        &layers,
        crate::render::terrain::textures::Texels::Linear,
        sampler,
    )?;
    Ok(own_texture(image))
}

// PORT(choice): the array's descriptor leaves out COPY_DST (wgpu 30's `create_texture_with_data` adds it for its own
// upload), so each level's array is a new texture: bevy_render 0.20's `GpuImage::prepare_asset` writes a replacement
// with an equal descriptor and COPY_DST into the old texture, level 0 only and at level 0's layer stride — on Mount
// Eddie's second drop (the same 11 layers of 256² with 9 levels) each layer's level 0 took other layers' texels and the
// smaller levels kept the first load's, so near projections showed other markings (#113, #114). The console has no
// array: each box light binds its own texture (`DynamicLight_ProjectedTexture`, handle 0x838F74F8 registered by
// 0x829C6268, set from the light's +0x50 at 0x829C6F30-0x829C6F78), every level of it.
fn own_texture(mut image: Image) -> Image {
    image.texture_descriptor.usage = TextureUsages::TEXTURE_BINDING;
    image
}

fn text_layer(levels: Vec<ssx_assets::texture::Image>) -> crate::render::terrain::textures::MipChain {
    let mut out: Vec<ssx_assets::texture::Image> = levels
        .into_iter()
        .map(|l| {
            let row = (l.width * 4) as usize;
            let mut rgba = l.rgba.clone();
            let last = l.rgba[l.rgba.len() - row..].to_vec();
            for _ in 0..l.width - l.height {
                rgba.extend_from_slice(&last);
            }
            ssx_assets::texture::Image { width: l.width, height: l.width, rgba }
        })
        .collect();
    while out.last().is_some_and(|l| l.width > 1) {
        let next = text_texture::half(&out[out.len() - 1]);
        out.push(next);
    }
    crate::render::terrain::textures::MipChain::from_levels(out)
}

fn clamp_trilinear() -> ImageSampler {
    ImageSampler::Descriptor(ImageSamplerDescriptor {
        address_mode_u: ImageAddressMode::ClampToEdge,
        address_mode_v: ImageAddressMode::ClampToEdge,
        mag_filter: ImageFilterMode::Linear,
        min_filter: ImageFilterMode::Linear,
        mipmap_filter: ImageFilterMode::Linear,
        ..Default::default()
    })
}

pub(super) fn blank_array() -> Image {
    let size = Extent3d { width: 1, height: 1, depth_or_array_layers: 1 };
    let mut image =
        Image::new_uninit(size, TextureDimension::D2, TextureFormat::Rgba8Unorm, RenderAssetUsages::RENDER_WORLD);
    image.data = Some(vec![255; 4]);
    image.texture_view_descriptor =
        Some(TextureViewDescriptor { dimension: Some(TextureViewDimension::D2Array), ..Default::default() });
    image.sampler = clamp_trilinear();
    own_texture(image)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::terrain::textures::{MipChain, Texels, array_texture};

    #[test]
    fn the_handles_arrays_are_new_textures_with_every_level() {
        let level =
            |w: u32, v: u8| ssx_assets::texture::Image { width: w, height: w, rgba: vec![v; (w * w * 4) as usize] };
        let chain = |v: u8| MipChain::from_levels(vec![level(4, v), level(2, v), level(1, v)]);
        let (a, b) = (chain(1), chain(2));
        let image = own_texture(array_texture(&[&a, &b], Texels::Linear, clamp_trilinear()).unwrap());
        assert!(!image.texture_descriptor.usage.contains(TextureUsages::COPY_DST));
        assert!(image.texture_descriptor.usage.contains(TextureUsages::TEXTURE_BINDING));
        assert_eq!(image.data.as_ref().map(Vec::len), Some(2 * (16 + 4 + 1) * 4));
        assert_eq!(image.texture_descriptor.mip_level_count, 3);
        assert!(!blank_array().texture_descriptor.usage.contains(TextureUsages::COPY_DST));
    }
}
