use std::path::Path;

use anyhow::{Context, Result};
use bevy::mesh::VertexAttributeValues;
use bevy::prelude::{Assets, Handle, Image, Mesh, Vec3};
use ssx_assets::content::GameContent;
use ssx_assets::content::resolve::ModelUse;
use ssx_assets::object_mesh::ObjectMesh;
use ssx_content::ContentId;
use ssx_content::material::MaterialDef;

use super::MaterialHandle;
use super::load::to_bevy_mesh;
use super::specs::{MaterialInput, MaterialSpec, prepare_materials};
use crate::render::lighting::game::GameLightAssets;
use crate::render::objects::foreground::ForegroundMaterial;
use crate::render::objects::material::ObjectMaterial;
use crate::rider::material::{
    CharacterExtension, CharacterLightUniform, CharacterMaterial, CharacterParams, CharacterShading,
};

pub struct LoadedPart {
    mesh: Mesh,
    pub bounds: (Vec3, Vec3),
    images: crate::render::uploads::PartImages,
    material: MaterialSpec,
    pub problems: Vec<String>,
}

pub fn load_part(big_root: &Path, id: &ContentId, use_: ModelUse, material: &MaterialDef) -> Result<LoadedPart> {
    let found = GameContent::of(big_root).model(id, use_)?;
    let (mesh, notes) = found.value.lod0_mesh().map_err(|e| anyhow::anyhow!("{id}: {e}"))?;
    let label = id.path().rsplit('/').next().unwrap_or_default().to_string();
    let mut part = part_from_mesh(big_root, mesh, &label, material)?;
    part.problems.extend(notes.into_iter().map(|n| format!("{id} from {}: {n}", found.source)));
    Ok(part)
}

pub fn part_from_mesh(big_root: &Path, mesh: ObjectMesh, label: &str, material: &MaterialDef) -> Result<LoadedPart> {
    let (lo, hi) = mesh.position_bounds().with_context(|| format!("{label} has no vertices"))?;
    let uv1 = CharacterShading::of(&material.effect).and(mesh.uv1.clone());
    let tangent_sign = mesh.tangent_signs.is_some();
    let mut mesh = to_bevy_mesh(mesh, label);
    if let Some(uv1) = uv1 {
        mesh.insert_attribute(Mesh::ATTRIBUTE_UV_1, uv1);
    }
    let mut problems = Vec::new();
    let (images, mut specs) =
        prepare_materials(big_root, &[MaterialInput { def: material, tangent_sign }], None, &mut problems)?;
    let material = specs.pop().context("no material prepared")?;
    let images = crate::render::uploads::PartImages::Built(images);
    Ok(LoadedPart { mesh, bounds: (Vec3::from_array(lo), Vec3::from_array(hi)), images, material, problems })
}

impl LoadedPart {
    pub fn queue_images(&mut self, uploads: &mut crate::render::uploads::LevelUploads, images: &mut Assets<Image>) {
        self.images.queue(uploads, images);
    }

    pub fn with_skin(mut self, joints: Vec<[u16; 4]>, weights: Vec<[f32; 4]>) -> Result<Self> {
        self.mesh.insert_attribute(Mesh::ATTRIBUTE_JOINT_INDEX, VertexAttributeValues::Uint16x4(joints));
        self.mesh.insert_attribute(Mesh::ATTRIBUTE_JOINT_WEIGHT, weights);
        self.mesh.generate_skinned_mesh_bounds()?;
        Ok(self)
    }

    pub fn add(
        self,
        meshes: &mut Assets<Mesh>,
        images: &mut Assets<Image>,
        light: &GameLightAssets,
        objects: &mut Assets<ObjectMaterial>,
        foreground: &mut Assets<ForegroundMaterial>,
    ) -> (Handle<Mesh>, MaterialHandle) {
        let handles = self.images.into_handles(images);
        (meshes.add(self.mesh), self.material.add(&handles, light, objects, foreground))
    }

    pub fn is_character(&self) -> bool {
        self.material.character.is_some()
    }

    pub fn add_character(
        self,
        meshes: &mut Assets<Mesh>,
        images: &mut Assets<Image>,
        light: &GameLightAssets,
        characters: &mut Assets<CharacterMaterial>,
    ) -> (Handle<Mesh>, Handle<CharacterMaterial>) {
        let handles = self.images.into_handles(images);
        let spec = &self.material;
        let image_or =
            |i: Option<usize>, fallback: &Handle<Image>| i.map_or_else(|| fallback.clone(), |i| handles[i].clone());
        let params = spec.character.unwrap_or_else(|| {
            let p = spec.params.shading;
            CharacterParams::new(CharacterShading::Body, p.y, p.z, Vec3::ZERO)
        });
        let material = CharacterMaterial {
            base: spec.standard_material(&handles),
            extension: CharacterExtension {
                light: light.uniform,
                rider: CharacterLightUniform::default(),
                params,
                normal: image_or(spec.normal, &light.flat_normal),
                specular: image_or(spec.specular, &light.white),
                ambient: image_or(spec.ambient, &light.white),
                level_light: crate::render::lighting::game::LEVEL_LIGHT,
                shadow_alpha: spec.shadow_alpha,
                local_lights: crate::render::lighting::local::LOCAL_LIGHT_BUFFER,
                local_light_textures: crate::render::lighting::local::LOCAL_LIGHT_TEXTURES,
                frame: crate::rider::material::RIDER_FRAME,
                shadow_pass: spec.shadow_pass,
            },
        };
        (meshes.add(self.mesh), characters.add(material))
    }
}
