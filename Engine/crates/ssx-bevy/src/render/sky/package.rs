use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result, ensure};
use bevy::asset::RenderAssetUsages;
use bevy::image::{ImageAddressMode, ImageFilterMode, ImageSampler, ImageSamplerDescriptor};
use bevy::prelude::{Assets, Handle, Image, Vec3, Vec4};
use bevy::render::render_resource::{
    Extent3d, ShaderType, TextureDimension, TextureFormat, TextureViewDescriptor, TextureViewDimension,
};
use ssx_assets::content::GameContent;
use ssx_assets::content::level::LevelSky;
use ssx_content::material::{MaterialDef, TextureRef};
use ssx_content::sky::{SkyCube, SkyDef};
use ssx_content::texture::Want;

use crate::render::lighting::GameLighting;

#[derive(Clone, Copy, Debug, Default, ShaderType)]
pub struct CubeSkyUniform {
    pub rotation: Vec4,
    pub scale: Vec4,
}

#[derive(Clone, Copy, Debug, Default, ShaderType)]
pub struct DomeUniform {
    pub rotation: Vec4,
    pub scale: Vec4,
    pub tile: Vec4,
    pub sun: Vec4,
}

#[derive(Debug, Default)]
pub struct DomeMesh {
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    pub indices: Vec<u32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Overlay {
    pub clouds: bool,
    pub sun: bool,
}

#[derive(Clone)]
pub enum Background {
    Cube { cube: Handle<Image>, uniform: CubeSkyUniform },
    Dome { mesh: Arc<DomeMesh>, texture: Handle<Image>, uniform: DomeUniform, hook: Option<(u64, String)> },
}

#[derive(Clone)]
pub struct PackageSky {
    pub background: Background,
    pub overlay: Option<Overlay>,
}

pub enum BackgroundLoad {
    Cube {
        cube: crate::render::uploads::LevelImage,
        uniform: CubeSkyUniform,
    },
    Dome {
        mesh: Arc<DomeMesh>,
        texture: crate::render::uploads::LevelImage,
        uniform: DomeUniform,
        hook: Option<(u64, String)>,
    },
}

pub struct PackageLoad {
    pub background: BackgroundLoad,
    pub overlay: Option<Overlay>,
    pub midground: bool,
    pub status: String,
}

impl PackageLoad {
    pub fn queue_images(&mut self, uploads: &mut crate::render::uploads::LevelUploads, images: &mut Assets<Image>) {
        match &mut self.background {
            BackgroundLoad::Cube { cube, .. } => cube.queue(uploads, images),
            BackgroundLoad::Dome { texture, .. } => texture.queue(uploads, images),
        }
    }

    pub fn into_settings(self, images: &mut Assets<Image>) -> PackageSky {
        let background = match self.background {
            BackgroundLoad::Cube { cube, uniform } => Background::Cube { cube: cube.into_handle(images), uniform },
            BackgroundLoad::Dome { mesh, texture, uniform, hook } => {
                Background::Dome { mesh, texture: texture.into_handle(images), uniform, hook }
            }
        };
        PackageSky { background, overlay: self.overlay }
    }
}

fn recip(x: f32) -> f32 {
    if x == 0.0 { 1e30 } else { 1.0 / x }
}

fn turn(def: &SkyDef) -> (f32, f32) {
    let r = def.rotation.to_radians();
    (r.cos(), r.sin())
}

pub fn cube_uniform(def: &SkyDef, l: &GameLighting) -> CubeSkyUniform {
    let (c, s) = turn(def);
    CubeSkyUniform {
        rotation: Vec4::new(c, s, recip(l.f("globalLightingMult")), 0.0),
        scale: (Vec3::from_array(def.tint) * def.exposure).extend(0.0),
    }
}

pub fn dome_uniform(def: &SkyDef, material: &MaterialDef, l: &GameLighting, time: f32) -> DomeUniform {
    let (c, s) = turn(def);
    let v3 = |name: &str, default: Vec3| {
        material.vectors.get(name).filter(|v| v.len() >= 3).map_or(default, |v| Vec3::new(v[0], v[1], v[2]))
    };
    let tile = material.vectors.get("skyTile").filter(|v| v.len() >= 2).map_or([1.0, 1.0], |v| [v[0], v[1]]);
    let exposure = material.float("exposure").unwrap_or(1.0);
    DomeUniform {
        rotation: Vec4::new(c, s, recip(l.f("globalLightingMult")), time),
        scale: (v3("tint", Vec3::ONE) * exposure).extend(0.0),
        tile: Vec4::new(tile[0], tile[1], 0.0, 0.0),
        sun: l.v("dirLight_Direction").truncate().normalize_or_zero().extend(0.0),
    }
}

pub fn cube_image(cube: &SkyCube) -> Image {
    let size = cube.size();
    let extent = Extent3d { width: size, height: size, depth_or_array_layers: 6 };
    let mut image =
        Image::new_uninit(extent, TextureDimension::D2, TextureFormat::Rgba16Float, RenderAssetUsages::RENDER_WORLD);
    let mut data = Vec::with_capacity(cube.bytes() as usize);
    for face in 0..6 {
        for level in &cube.levels {
            data.extend(level.faces[face].iter().flat_map(|v| v.to_le_bytes()));
        }
    }
    image.data = Some(data);
    image.texture_descriptor.mip_level_count = cube.levels.len() as u32;
    image.texture_view_descriptor =
        Some(TextureViewDescriptor { dimension: Some(TextureViewDimension::Cube), ..Default::default() });
    image.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        address_mode_u: ImageAddressMode::ClampToEdge,
        address_mode_v: ImageAddressMode::ClampToEdge,
        address_mode_w: ImageAddressMode::ClampToEdge,
        mag_filter: ImageFilterMode::Linear,
        min_filter: ImageFilterMode::Linear,
        mipmap_filter: ImageFilterMode::Linear,
        ..Default::default()
    });
    image
}

pub fn reflection_faces(cube: &SkyCube, def: &SkyDef) -> (u32, u32, Vec<Vec<u8>>) {
    // TODO(guess): a cubemap sky's linear colours as the game's 8-bit reflection texels: sqrt(min(c · tint · exposure, 1)),
    // as the materials square the texel; the disc's reflection cubes are authored images whose relation to the
    // preset's scattering sky is not known (no exposure of theirs is stored); settle: compare the disc's
    // `reflectCubeTexture`s with their presets' scattering skies rendered into a cube.
    let scale = Vec3::from_array(def.tint) * def.exposure;
    let enc = |v: f32, k: f32| ((v * k).clamp(0.0, 1.0).sqrt() * 255.0 + 0.5) as u8;
    let faces = (0..6)
        .map(|f| {
            cube.levels
                .iter()
                .flat_map(|l| {
                    l.faces[f].as_chunks::<4>().0.iter().flat_map(move |t| {
                        let c = t.map(|b| half::f16::from_bits(b).to_f32());
                        [enc(c[0], scale.x), enc(c[1], scale.y), enc(c[2], scale.z), 255]
                    })
                })
                .collect()
        })
        .collect();
    (cube.size(), cube.levels.len() as u32, faces)
}

pub fn decided_texture(root: &Path, l: &GameLighting, id: &ssx_content::ContentId) -> Result<Image> {
    level_texture(root, l, id)
}

fn level_texture(root: &Path, l: &GameLighting, id: &ssx_content::ContentId) -> Result<Image> {
    use crate::render::terrain::textures::{MipChain, Texels, texture_2d};
    let content = GameContent::of(root);
    let found = l.level.texture(&content, id, &Want::PLANE)?;
    let levels: Vec<ssx_assets::texture::Image> = found
        .texture
        .levels
        .into_iter()
        .filter_map(|lv| {
            lv.images.into_iter().next().map(|rgba| ssx_assets::texture::Image {
                width: lv.width,
                height: lv.height,
                rgba,
            })
        })
        .collect();
    ensure!(!levels.is_empty(), "{id}: no levels");
    let chain = MipChain { levels, blocks: None, origin: Some((id.clone(), found.layer)) };
    let sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        address_mode_u: ImageAddressMode::Repeat,
        address_mode_v: ImageAddressMode::ClampToEdge,
        mag_filter: ImageFilterMode::Linear,
        min_filter: ImageFilterMode::Linear,
        mipmap_filter: ImageFilterMode::Linear,
        ..Default::default()
    });
    Ok(texture_2d(&chain, Texels::Linear, sampler))
}

pub fn load(l: &GameLighting, root: &Path) -> Result<Option<PackageLoad>> {
    let def = match &l.level.sky {
        LevelSky::Scattering => return Ok(None),
        LevelSky::Cubemap { sky, .. } | LevelSky::Dome { sky, .. } => sky.def.clone(),
    };
    let overlay = (def.clouds || def.sun).then_some(Overlay { clouds: def.clouds, sun: def.sun });
    let (background, status) = match &l.level.sky {
        LevelSky::Cubemap { id, source, sky } => {
            let cube = sky.cube.as_ref().with_context(|| format!("{id}: a cubemap sky without its cube"))?;
            let status = format!(
                "sky: {id} from {source}, a cubemap ({}², {} levels), rotation {} deg, exposure {}{}",
                cube.size(),
                cube.levels.len(),
                def.rotation,
                def.exposure,
                if def.reflection { ", the reflection cube" } else { "" }
            );
            let cube = crate::render::uploads::LevelImage::built(cube_image(cube), None);
            (BackgroundLoad::Cube { cube, uniform: cube_uniform(&def, l) }, status)
        }
        LevelSky::Dome { id, source, model, material, .. } => {
            let (mesh, notes) = model.value.lod0_mesh().map_err(|e| anyhow::anyhow!("{}: {e}", model.id))?;
            for n in notes {
                bevy::log::warn!("{}: {n}", model.id);
            }
            let n = mesh.positions.len();
            let dome = DomeMesh {
                normals: mesh.normals.clone().unwrap_or_else(|| vec![[0.0, 1.0, 0.0]; n]),
                uvs: mesh.uv0.clone().unwrap_or_else(|| vec![[0.0, 0.0]; n]),
                positions: mesh.positions,
                indices: mesh.indices,
            };
            let texture = match material.textures.get("skyTexture") {
                Some(TextureRef::Id(t)) => (level_texture(root, l, t)?, None),
                _ => (white(), None),
            };
            let (hook, hook_note) = match material.shader.as_ref() {
                None => (None, String::new()),
                Some(s) => {
                    use crate::render::hooks::runtime::{HookRequest, refusal, request};
                    let unused = |why: String| {
                        (None, format!("; its shader hook is not used ({why}): the dome's own shader draws"))
                    };
                    match (&s.problem, s.source.clone()) {
                        (Some(p), _) => unused(p.clone()),
                        (None, Some(text)) => {
                            let hash = request(HookRequest::sky(&s.file, text));
                            match refusal(hash) {
                                None => (Some((hash, material.name.clone())), " with a shader hook".to_string()),
                                Some(why) => unused(format!("refused: {why}")),
                            }
                        }
                        (None, None) => unused(format!("{}: its text was not read", s.file)),
                    }
                }
            };
            let status = format!("sky: {id} from {source}, a dome ({} triangles){hook_note}", dome.indices.len() / 3);
            let uniform = dome_uniform(&def, material, l, 0.0);
            let texture = crate::render::uploads::LevelImage::built(texture.0, texture.1);
            (BackgroundLoad::Dome { mesh: Arc::new(dome), texture, uniform, hook }, status)
        }
        LevelSky::Scattering => return Ok(None),
    };
    Ok(Some(PackageLoad { background, overlay, midground: def.midground, status }))
}

fn white() -> Image {
    let size = Extent3d { width: 1, height: 1, depth_or_array_layers: 1 };
    Image::new(size, TextureDimension::D2, vec![255; 4], TextureFormat::Rgba8Unorm, RenderAssetUsages::RENDER_WORLD)
}
