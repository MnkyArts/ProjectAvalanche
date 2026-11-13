use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{Context as _, Result};
use bevy::prelude::{UVec2, info, warn};
use ssx_assets::particles::{self, BASE_SNOW_SPRAY, LOD_SNOW_SPRAY};

use super::{ParticleTexture, ParticleWorld, SlotEffect, Spray, SunSlot, TrailSlot, rider, tombstone};
use crate::particles::sim::{EffectInstance, Rng, SetArena};
use crate::particles::spray::SnowSprayDriver;
use crate::particles::track::{DefCache, TrackEffects};
use crate::particles::weather::WeatherManager;
use crate::ride::Ride;
use crate::rider::skin::RiderRig;

pub(super) fn build(
    root: &std::path::Path,
    ride: Option<&Ride>,
    lighting: Option<&crate::render::lighting::GameLighting>,
    avalanche: Option<&crate::render::avalanche::AvalancheScene>,
) -> ParticleWorld {
    let t0 = std::time::Instant::now();
    let mut world = ParticleWorld {
        spray: None,
        bot_sprays: Vec::new(),
        live_sprays: Default::default(),
        sun: None,
        rider: None,
        rider_tried: false,
        heli: None,
        heli_tried: false,
        defs: DefCache::default(),
        texture_names: Vec::new(),
        trails: Vec::new(),
        geotags: Default::default(),
        tombstones: Default::default(),
        track: TrackEffects::default(),
        weather: WeatherManager::default(),
        arena: SetArena::default(),
        rng: Rng::default(),
        clock: 0.0,
        last_render_time: 0.0,
        frame: 0,
        dt_smoothed: 1.0 / 60.0,
        last_step: 0,
        pending: 0.0,
        textures: Default::default(),
        registry: Default::default(),
        registry_snapshot: None,
        light: None,
        level: lighting.map(|l| l.level.clone()),
    };
    let (mut defs, mut names) = (DefCache::default(), Vec::new());
    match build_spray(root, &mut world.arena, &mut defs, &mut names, BASE_SNOW_SPRAY, false) {
        _ if crate::render::post::env_is("SSX_SNOW_SPRAY", "off") => {
            info!("particles: no snow spray (SSX_SNOW_SPRAY=off)")
        }
        Ok(s) => world.spray = Some(s),
        Err(e) => warn!("particles: snow spray unavailable: {e:#}"),
    }
    for _ in ride.as_ref().and_then(|r| r.session.bots.as_ref()).map_or(&[][..], |b| &b.bots[..]) {
        match build_spray(root, &mut world.arena, &mut defs, &mut names, LOD_SNOW_SPRAY, true) {
            Ok(s) => world.bot_sprays.push(s),
            Err(e) => {
                warn!("particles: a bot's lod snow spray unavailable: {e:#}");
                break;
            }
        }
    }
    if !world.bot_sprays.is_empty() {
        let n = world.bot_sprays[0].effects.len();
        info!("particles: {} bots' lod_snowspray ({n} effects each)", world.bot_sprays.len());
    }
    // PORT(choice): a live racer has the bots' `lod_snowspray` (every `BotSnowboarderRef`'s component) because it is
    // drawn as a bot (racer kind 1 or 2); a port feature: the original has no live racers.
    if ride.is_some_and(|r| r.live.is_some()) && !crate::render::post::env_is("SSX_SNOW_SPRAY", "off") {
        let built = particles::load_snow_spray_files(root, LOD_SNOW_SPRAY).and_then(|loaded| {
            let spray = spray_of(&loaded.set, root, &mut world.arena, &mut defs, &mut names, true)?;
            Ok((loaded.set, spray))
        });
        match built {
            Ok((set, spray)) => {
                info!("particles: the live racers' lod_snowspray ({} effects each)", spray.effects.len());
                world.live_sprays.set = Some(set);
                world.live_sprays.add(spray);
            }
            Err(e) => warn!("particles: the live racers' lod snow spray unavailable: {e:#}"),
        }
    }
    if let Some(l) = &lighting {
        let drop = (l.drop_count > 0).then_some(l.drop_key);
        let track = l.disc_track();
        if track.is_none() {
            info!("particles: a map's new track: no track effects, tombstones or track lights");
        }
        match track.map(|t| TrackEffects::load(root, t, drop, &mut world.arena, &mut defs, &mut names)) {
            None => {}
            Some(_) if crate::render::post::env_is("SSX_TRACK_FX", "off") => {
                info!("particles: no track effects (SSX_TRACK_FX=off)")
            }
            Some(Ok(t)) => {
                let entities = t.offline.as_ref().map_or(0, |(c, _)| c.entities.len());
                let controlled = t.controlled();
                info!(
                    "particles: {} track effect slots ({controlled} on the {entities} entities' controllers)",
                    t.slots.len()
                );
                world.track = t;
            }
            Some(Err(e)) => warn!("particles: track effects unavailable: {e:#}"),
        }
        if let Some(m) = ride.and_then(|r| r.map.as_ref()).filter(|m| !m.placed.is_empty())
            && !crate::render::post::env_is("SSX_TRACK_FX", "off")
        {
            match world.track.add_map(root, &m.placed, &mut world.arena, &mut defs, &mut names) {
                Ok(n) => info!("particles: {n} effect slots of the map's {} placed disc objects", m.placed.len()),
                Err(e) => warn!("particles: the map's placed objects' effects unavailable: {e:#}"),
            }
        }
        if let Some(track) = track.filter(|_| ride.as_ref().is_some_and(|r| r.session.tombstones.is_some())) {
            match tombstone::TombstoneSlots::load(root, track, &mut defs, &mut names, &mut world.arena) {
                Ok((t, line)) => {
                    info!("particles: {line}");
                    world.tombstones = t;
                }
                Err(e) => warn!("particles: tombstones unavailable: {e:#}"),
            }
        }
        match track.map(|t| crate::particles::light::NearestLight::load(root, t)) {
            None => {}
            Some(Ok(light)) => world.light = Some(light),
            Some(Err(e)) => warn!("particles: track lights unavailable: {e:#}"),
        }
        let sun = l.sun_drawn() && !crate::render::post::env_is("SSX_SUN", "off");
        if let Some(node) = l.preset.sun_node.as_ref().filter(|_| sun) {
            match defs.node(root, node) {
                Ok((def, _)) => {
                    crate::particles::track::textures_of(&def, &mut names);
                    info!("particles: the sun's glare {} ({})", def.name, l.preset.name);
                    world.sun = Some(SunSlot {
                        node: node.clone(),
                        instance: EffectInstance::new(def, &mut world.arena),
                        dir: l.sun,
                    });
                }
                Err(e) => warn!("particles: the sun's glare: {e:#}"),
            }
        }
    }
    if let Some(scene) = &avalanche {
        for (s, set) in scene.sets.iter().enumerate() {
            for (k, c) in set.chunks.iter().enumerate() {
                for node in &scene.models[c.model].nodes {
                    match defs.node(root, node) {
                        Ok((def, _)) => {
                            crate::particles::track::textures_of(&def, &mut names);
                            world.trails.push(TrailSlot {
                                set: s,
                                chunk: k,
                                node: node.clone(),
                                instance: EffectInstance::new(def, &mut world.arena),
                                start: -1.0,
                                stop: -1.0,
                                blend: Vec::new(),
                            });
                        }
                        Err(e) => warn!("particles: avalanche trail: {e:#}"),
                    }
                }
            }
        }
        info!("particles: {} avalanche trail slots", world.trails.len());
    }
    let mut textures = Vec::new();
    for n in &names {
        match load_texture(root, n, world.level.as_deref()) {
            Ok(t) => textures.push(t),
            Err(e) => warn!("particles: texture {n}: {e:#}"),
        }
    }
    info!(
        "particles: {} particle sets, {} textures (loaded in {:.3} s)",
        world.arena.sets.len(),
        textures.len(),
        t0.elapsed().as_secs_f32()
    );
    world.textures = Arc::new(textures);
    (world.defs, world.texture_names) = (defs, names);
    world
}

fn build_spray(
    root: &std::path::Path,
    arena: &mut SetArena,
    defs: &mut DefCache,
    names: &mut Vec<String>,
    collection: u32,
    lod: bool,
) -> Result<Spray> {
    let loaded = particles::load_snow_spray_files(root, collection)?;
    spray_of(&loaded.set, root, arena, defs, names, lod)
}

pub(super) fn spray_of(
    set: &particles::SnowSpraySet,
    root: &std::path::Path,
    arena: &mut SetArena,
    defs: &mut DefCache,
    names: &mut Vec<String>,
    lod: bool,
) -> Result<Spray> {
    let (mut effects, mut by_key, mut table) = (Vec::new(), HashMap::new(), [[None; 6]; 3]);
    for (s, row) in set.slots.iter().enumerate() {
        for (k, node) in row.iter().enumerate() {
            let Some(node) = node else { continue };
            let index = match by_key.get(&node.key) {
                Some(&i) => i,
                None => {
                    let (def, behaviour) = defs.node(root, node).context("spray node")?;
                    crate::particles::track::textures_of(&def, names);
                    effects.push(SlotEffect {
                        node: node.clone(),
                        instance: EffectInstance::new(def, arena),
                        behaviour,
                        start: -1.0,
                        stop: -1.0,
                        instant: false,
                    });
                    by_key.insert(node.key, effects.len() - 1);
                    effects.len() - 1
                }
            };
            table[s][k] = Some(index);
        }
    }
    Ok(Spray { effects, table, driver: SnowSprayDriver::new(lod), inputs: Default::default() })
}

fn load_texture(
    root: &std::path::Path,
    name: &str,
    level: Option<&ssx_assets::content::level::LevelPackages>,
) -> Result<ParticleTexture> {
    let id = particles::texture_id(name).with_context(|| format!("{name:?}: not a texture id"))?;
    let content = ssx_assets::content::GameContent::of(root);
    let want = ssx_content::texture::Want::PLANE;
    let found = match level.filter(|l| l.textures.contains_key(&id)) {
        Some(l) => l.texture(&content, &id, &want)?,
        None => content.texture(&id, &want)?,
    };
    let levels = found.texture.levels;
    let first = levels.first().context("no levels")?;
    let (w, h) = (first.width, first.height);
    let chain = levels
        .into_iter()
        .enumerate()
        .take_while(|(i, l)| l.width == (w >> i).max(1) && l.height == (h >> i).max(1))
        .filter_map(|(_, l)| l.images.into_iter().next())
        .collect();
    Ok(ParticleTexture { name: name.to_owned(), size: UVec2::new(w, h), levels: chain })
}

pub(super) fn load_rider_effects(
    w: &mut ParticleWorld,
    root: &std::path::Path,
    track: Option<&str>,
    rig: &RiderRig,
    ride: &Ride,
) {
    w.rider_tried = true;
    if crate::render::post::env_is("SSX_RIDER_FX", "off") {
        return;
    }
    let name = rig.summary.split_whitespace().next().unwrap_or(ssx_assets::character::DEFAULT_CHARACTER);
    let loadout = ride.loadout.clone();
    let (_, graph, _) = rig.anim.posed();
    let mut names = std::mem::take(&mut w.texture_names);
    let before = names.len();
    match rider::RiderEffects::load(root, track, name, &loadout, graph, &mut w.arena, &mut w.defs, &mut names) {
        Ok(r) => {
            let unbound: Vec<&str> =
                r.slots.iter().filter(|s| s.joint.is_none()).map(|s| s.node.joint_name.as_str()).collect();
            info!(
                "particles: {name}'s own effects with {:?} {:?}: {} slots ({})",
                loadout.items,
                loadout.descriptions,
                r.slots.len(),
                if unbound.is_empty() { "all on rig joints".to_owned() } else { format!("no joint: {unbound:?}") }
            );
            w.rider = Some(r);
        }
        Err(e) => warn!("particles: the rider's effects unavailable: {e:#}"),
    }
    w.texture_names = names;
    load_new_textures(w, root, before);
}

pub(super) fn load_heli_effects(
    w: &mut ParticleWorld,
    root: &std::path::Path,
    heli: &crate::ride::helicopter::HeliVisual,
) {
    w.heli_tried = true;
    let mut names = std::mem::take(&mut w.texture_names);
    let before = names.len();
    let nodes = heli.particle_nodes.iter().map(|n| ("heli".to_owned(), n.clone()));
    match rider::RiderEffects::from_nodes(root, nodes, heli.rig(), &mut w.arena, &mut w.defs, &mut names) {
        Ok(r) => {
            let unbound: Vec<&str> =
                r.slots.iter().filter(|s| s.joint.is_none()).map(|s| s.node.joint_name.as_str()).collect();
            info!(
                "particles: the helicopter's effects: {} slots ({})",
                r.slots.len(),
                if unbound.is_empty() { "all on rig joints".to_owned() } else { format!("no joint: {unbound:?}") }
            );
            w.heli = Some(r);
        }
        Err(e) => warn!("particles: the helicopter's effects unavailable: {e:#}"),
    }
    w.texture_names = names;
    load_new_textures(w, root, before);
}

pub(crate) fn reload_textures(
    w: &mut ParticleWorld,
    root: &std::path::Path,
    touched: impl Fn(&ssx_content::ContentId) -> bool,
) -> usize {
    let mut n = 0;
    let all: Vec<ParticleTexture> = w
        .textures
        .iter()
        .map(|t| {
            let changed = particles::texture_id(&t.name).is_some_and(|id| touched(&id));
            match changed.then(|| load_texture(root, &t.name, w.level.as_deref())) {
                Some(Ok(new)) => {
                    n += 1;
                    new
                }
                Some(Err(e)) => {
                    warn!("particles: texture {} not reloaded: {e:#}", t.name);
                    ParticleTexture { name: t.name.clone(), size: t.size, levels: t.levels.clone() }
                }
                None => ParticleTexture { name: t.name.clone(), size: t.size, levels: t.levels.clone() },
            }
        })
        .collect();
    if n > 0 {
        w.textures = Arc::new(all);
    }
    n
}

pub(super) fn load_new_textures(w: &mut ParticleWorld, root: &std::path::Path, before: usize) {
    if w.texture_names.len() <= before {
        return;
    }
    let mut textures: Vec<ParticleTexture> = Vec::new();
    for n in &w.texture_names[before..] {
        match load_texture(root, n, w.level.as_deref()) {
            Ok(t) => textures.push(t),
            Err(e) => warn!("particles: texture {n}: {e:#}"),
        }
    }
    let mut all: Vec<ParticleTexture> = w
        .textures
        .iter()
        .map(|t| ParticleTexture { name: t.name.clone(), size: t.size, levels: t.levels.clone() })
        .collect();
    all.extend(textures);
    w.textures = Arc::new(all);
}
