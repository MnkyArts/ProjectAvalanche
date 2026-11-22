use std::path::Path;

use anyhow::{Context, Result};
use bevy::mesh::skinning::SkinnedMeshInverseBindposes;
use bevy::prelude::{
    App, Assets, Capsule3d, ChildOf, Color, Commands, Component, Cuboid, Image, IntoScheduleConfigs, Mesh, Mesh3d,
    MeshMaterial3d, Name, Quat, Res, ResMut, Resource, StandardMaterial, Transform, Vec3, Visibility, World, default,
    info, resource_exists, warn,
};
use ssx_assets::vault_db::{fnv1, single};
use ssx_formats::attrib::Value;

use crate::render::lighting::game::GameLightAssets;
use crate::render::objects::foreground::ForegroundMaterial;
use crate::render::objects::material::ObjectMaterial;
use crate::render::objects::{LoadedPart, load_part};
use crate::rider::material::{CharacterMaterial, RiderShading};
use crate::rider::skin::{self as rider_skin, LoadedRider};

pub const BOARD_MESH: &str = "common_board_mesh";
pub const FALLBACK_HALF_WIDTH: f32 = 0.145;
const BODY_RADIUS: f32 = 0.15;
const BODY_LENGTH: f32 = 1.15;
const BOARD_TOP: f32 = 0.06;

#[derive(Component)]
pub struct RiderRoot;

#[derive(Component)]
pub struct RiderBody;

pub fn body_transform(board: Quat) -> Transform {
    let axis = (board * Vec3::Y + Vec3::Y).try_normalize().unwrap_or(Vec3::Y);
    let local = board.inverse() * Quat::from_rotation_arc(Vec3::Y, axis);
    let local_axis = local * Vec3::Y;
    Transform::from_translation(Vec3::Y * BOARD_TOP + local_axis * (BODY_RADIUS + BODY_LENGTH * 0.5))
        .with_rotation(local)
}

pub struct LoadedBoard {
    part: LoadedPart,
    pub source: String,
}

impl LoadedBoard {
    pub fn queue_images(&mut self, uploads: &mut crate::render::uploads::LevelUploads, images: &mut Assets<Image>) {
        self.part.queue_images(uploads, images);
    }

    pub fn into_part(self) -> LoadedPart {
        self.part
    }
}

pub fn load_board(big_root: &Path, track: Option<&str>) -> Result<LoadedBoard> {
    let (geom, material) = ssx_assets::vault_db::with_level_db(big_root, track, |db| {
        let fields = db.fields(fnv1("ge_mesh"), fnv1(BOARD_MESH))?;
        let reference = |name: &str| {
            fields
                .iter()
                .find(|f| f.key == fnv1(name))
                .and_then(|f| single(&f.data))
                .and_then(|v| if let Value::Ref(r) = v { Some(*r) } else { None })
                .filter(|r| r.collection != 0)
                .with_context(|| format!("{BOARD_MESH} has no {name}"))
        };
        let g = reference("Geometry")?;
        let geom = db.text_of(g.class, g.collection)?.context("board geometry without a path")?;
        let m = reference("Material")?;
        Ok((geom, db.material(m.class, m.collection)?))
    })?;
    board_part(big_root, &geom, &material)
}

pub fn board_part(big_root: &Path, geom: &str, material: &ssx_assets::vault_db::Material) -> Result<LoadedBoard> {
    let content = ssx_assets::content::GameContent::of(big_root);
    let id = ssx_assets::content::models::model_id(geom)
        .filter(|id| content.has(id))
        .with_context(|| format!("{geom} not found under {}", big_root.display()))?;
    let def = content.material_for(material, ssx_assets::content::resolve::MaterialUse::Riders);
    let part = load_part(big_root, &id, ssx_assets::content::resolve::ModelUse::Rider, &def)?;
    let diffuse = material.textures.get("diffuseTexture").map_or("no diffuse", String::as_str);
    let source =
        format!("{geom} ({}, {}, {diffuse})", material.name, material.effect.as_deref().unwrap_or("no effect"));
    Ok(LoadedBoard { part, source })
}

#[derive(Resource)]
struct PendingRider {
    board: Option<LoadedBoard>,
    body: Option<LoadedRider>,
}

pub fn register(app: &mut App) {
    app.add_systems(crate::shell::level::LevelStartup, spawn_rider.run_if(resource_exists::<PendingRider>));
    app.add_systems(
        bevy::prelude::Update,
        crate::rider::reload::follow.run_if(resource_exists::<crate::render::reload::ContentReload>),
    );
    crate::shell::level::scoped::<PendingRider>(app);
    crate::shell::level::scoped::<RiderShading>(app);
    crate::shell::level::scoped::<crate::rider::skin::RiderRig>(app);
}

pub fn insert(world: &mut World, board: Option<LoadedBoard>, body: Option<LoadedRider>) {
    world.insert_resource(PendingRider { board, body });
}

#[allow(clippy::too_many_arguments)]
fn spawn_rider(
    mut commands: Commands,
    mut pending: ResMut<PendingRider>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut images: ResMut<Assets<Image>>,
    mut objects: ResMut<Assets<ObjectMaterial>>,
    mut foreground: ResMut<Assets<ForegroundMaterial>>,
    mut characters: ResMut<Assets<CharacterMaterial>>,
    mut standard: ResMut<Assets<StandardMaterial>>,
    mut bindposes: ResMut<Assets<SkinnedMeshInverseBindposes>>,
    light: Res<GameLightAssets>,
) {
    let root = commands.spawn((Name::new("rider"), RiderRoot, Transform::default(), Visibility::default())).id();
    let mut board_material = None;
    let separate = pending.board.take().filter(|_| !pending.body.as_ref().is_some_and(|b| b.has_board_part()));
    let board_entity = match separate {
        Some(board) => {
            info!("Ride: board {}", board.source);
            for problem in &board.part.problems {
                warn!("board: {problem}");
            }
            let (lo, hi) = board.part.bounds;
            let offset = -Vec3::new((lo.x + hi.x) * 0.5, lo.y, (lo.z + hi.z) * 0.5);
            let (mesh, material) = board.part.add_character(&mut meshes, &mut images, &light, &mut characters);
            board_material = Some(material.clone());
            let entity = commands.spawn((
                Name::new("snowboard"),
                Mesh3d(mesh),
                MeshMaterial3d(material),
                Transform::from_translation(offset),
                ChildOf(root),
            ));
            Some((entity.id(), true))
        }
        None if pending.body.as_ref().is_some_and(|b| b.has_board_part()) => None,
        None => {
            let id = commands
                .spawn((
                    Name::new("board placeholder"),
                    Mesh3d(meshes.add(Cuboid::new(1.6, BOARD_TOP, FALLBACK_HALF_WIDTH * 2.0))),
                    MeshMaterial3d(
                        standard.add(StandardMaterial { base_color: Color::srgb(0.2, 0.2, 0.25), ..default() }),
                    ),
                    Transform::from_xyz(0.0, BOARD_TOP * 0.5, 0.0),
                    ChildOf(root),
                ))
                .id();
            Some((id, false))
        }
    };
    if let Some(body) = pending.body.take() {
        info!("Ride: character {} (loaded in {:.2}s)", body.summary, body.load_secs);
        for problem in &body.problems {
            warn!("character: {problem}");
        }
        let rig = rider_skin::spawn(
            &mut commands,
            root,
            body,
            board_entity,
            &mut meshes,
            &mut images,
            &light,
            &mut objects,
            &mut foreground,
            &mut characters,
            &mut bindposes,
        );
        for line in rig.anim.clip_lines() {
            info!("Ride: clip {line}");
        }
        for g in &rig.gear_lights {
            let l = &g.light;
            info!("Ride: gear light {} / {}: {} ({}, enable {})", g.slot, g.group, l.name, l.shape.kind(), l.enable);
        }
        let mut materials = rig.character_materials.clone();
        materials.extend(board_material);
        commands.insert_resource(RiderShading::new(materials, rig.wingsuit_materials.clone()));
        commands.insert_resource(rig);
        return;
    }
    if let Some(board) = board_material {
        commands.insert_resource(RiderShading::new(vec![board], Vec::new()));
    }
    commands.spawn((
        Name::new("rider placeholder (capsule)"),
        RiderBody,
        Mesh3d(meshes.add(Capsule3d::new(BODY_RADIUS, BODY_LENGTH))),
        MeshMaterial3d(standard.add(StandardMaterial {
            base_color: Color::srgb(0.95, 0.4, 0.1),
            perceptual_roughness: 0.8,
            ..default()
        })),
        body_transform(Quat::IDENTITY),
        ChildOf(root),
    ));
}
