use std::sync::Mutex;
use std::sync::mpsc::{Receiver, channel};

use anyhow::Result;
use bevy::ecs::system::SystemParam;
use bevy::log::{info, warn};
use bevy::mesh::skinning::SkinnedMeshInverseBindposes;
use bevy::prelude::{Assets, Commands, Image, Local, Mesh, Res, ResMut};
use ssx_assets::content::GameContent;
use ssx_assets::content::characters::dress::dress;
use ssx_assets::content::characters::ride::package_items;
use ssx_content::Kind;

use super::material::{CharacterMaterial, RiderShading};
use super::skin::{GearLight, PartAssets, Redress, RiderRig, SkinnedPart, load_parts, spawn_parts};
use crate::render::lighting::game::GameLightAssets;
use crate::render::objects::foreground::ForegroundMaterial;
use crate::render::objects::material::ObjectMaterial;
use crate::render::reload::ContentReload;

const KINDS: [Kind; 4] = [Kind::Character, Kind::Gear, Kind::Board, Kind::Material];

pub(super) struct Redressed {
    parts: Vec<SkinnedPart>,
    lights: Vec<GearLight>,
    summary: String,
    problems: Vec<String>,
    redress: Redress,
}

fn redress(mut r: Redress, rig: &ssx_formats::crsf::SceneGraph) -> Result<Redressed> {
    let content = GameContent::of(&r.big_root);
    if let Some(before) = &r.riders {
        let keys: Vec<u32> = r.loadout.descriptions.iter().map(|&(_, k)| k).collect();
        let items = package_items(&content, &keys);
        let d = content.rider_looks_again(before, std::slice::from_ref(&r.request.id), &items);
        r.request.look = d.look(&r.request.id);
        r.request.allowed_items = Some(d.items.clone());
    }
    let character = dress(&r.big_root, r.track.as_deref(), &r.request, &r.loadout)?;
    if character.parts.is_empty() {
        anyhow::bail!("{}: no part loaded ({})", r.request.id, character.problems.join("; "));
    }
    let summary = character.summary();
    let mut problems = character.problems.clone();
    let (parts, lights) = load_parts(&r.big_root, character, rig, &mut problems);
    Ok(Redressed { parts, lights, summary, problems, redress: r })
}

#[derive(SystemParam)]
pub(super) struct Stores<'w> {
    meshes: ResMut<'w, Assets<Mesh>>,
    images: ResMut<'w, Assets<Image>>,
    objects: ResMut<'w, Assets<ObjectMaterial>>,
    foreground: ResMut<'w, Assets<ForegroundMaterial>>,
    characters: ResMut<'w, Assets<CharacterMaterial>>,
    bindposes: ResMut<'w, Assets<SkinnedMeshInverseBindposes>>,
    light: Option<Res<'w, GameLightAssets>>,
}

pub(super) fn follow(
    reload: Option<Res<ContentReload>>,
    mut seen: Local<Option<u64>>,
    mut pending: Local<Option<Mutex<Receiver<Result<Redressed>>>>>,
    mut commands: Commands,
    rig: Option<ResMut<RiderRig>>,
    shading: Option<ResMut<RiderShading>>,
    mut stores: Stores,
) {
    let (Some(reload), Some(mut rig)) = (reload, rig) else { return };
    if stores.light.is_none() {
        return;
    }
    if let Some(rx) = pending.as_ref() {
        let Some(got) = rx.lock().ok().and_then(|r| r.try_recv().ok()) else { return };
        *pending = None;
        match got {
            Ok(r) => put_on(&mut commands, &mut rig, shading, &mut stores, r),
            Err(e) => warn!("mod-dev: the rider's look not dressed again (the drawn one stays): {e:#}"),
        }
        return;
    }
    let Some(prev) = *seen else {
        *seen = Some(reload.generation);
        return;
    };
    if reload.generation == prev {
        return;
    }
    *seen = Some(reload.generation);
    if !reload.ids.as_ref().is_none_or(|ids| ids.iter().any(|id| KINDS.contains(&id.kind()))) {
        return;
    }
    let (r, drawn) = (rig.redress.clone(), rig.rig().clone());
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        let _ = tx.send(redress(r, &drawn));
    });
    *pending = Some(Mutex::new(rx));
}

fn put_on(
    commands: &mut Commands,
    rig: &mut RiderRig,
    shading: Option<ResMut<RiderShading>>,
    s: &mut Stores,
    r: Redressed,
) {
    let Some(light) = s.light.as_deref() else { return };
    for e in rig.part_entities.drain(..) {
        commands.entity(e).despawn();
    }
    let mut assets = PartAssets {
        meshes: &mut s.meshes,
        images: &mut s.images,
        light,
        objects: &mut s.objects,
        foreground: &mut s.foreground,
        characters: &mut s.characters,
        bindposes: &mut s.bindposes,
    };
    let n = r.parts.len();
    let spawned = spawn_parts(commands, rig.root, rig.nodes(), r.parts, &mut assets);
    if let Some(mut shading) = shading {
        shading.replace(
            &rig.character_materials,
            spawned.character_materials.clone(),
            spawned.wingsuit_materials.clone(),
        );
    }
    rig.part_entities = spawned.entities;
    rig.character_materials = spawned.character_materials;
    rig.wingsuit_materials = spawned.wingsuit_materials;
    rig.gear_lights = r.lights;
    rig.summary = r.summary;
    rig.redress = r.redress;
    for p in &r.problems {
        warn!("mod-dev: character: {p}");
    }
    info!("mod-dev: the rider dressed again: {n} parts ({})", rig.summary);
}
