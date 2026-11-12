use std::path::Path;
use std::time::Instant;

use anyhow::{Context, Result};
use bevy::prelude::{info, warn};
use ssx_assets::content::GameContent;
use ssx_assets::content::track_packages::Usage;
use ssx_content::ContentId;
use ssx_physics::character::DEFAULT_GEAR_MIX;
use ssx_physics::map::{ResolvedMap, load_map_level};

use super::{LevelData, LevelSpec, RideOptions, RideWorld, ShadingArg, ride_setup};
use crate::camera;
use crate::render::{lighting, objects, post, sky, terrain};
use crate::ride;

const TRACKS: &str = "data0/data/xenon/gameexplorer";

pub(super) fn load(spec: &LevelSpec, id: &ContentId) -> Result<LevelData> {
    let start = Instant::now();
    let big_root = spec.big_root.clone().context("a map's level needs the extracted archive root (--big-root)")?;
    crate::render::fx_states::init(&big_root);
    let content = GameContent::of(&big_root);
    let resolved = ResolvedMap::resolve(&content, id)?;
    let map = &resolved.map;
    let toml = &map.value.toml;
    let base = resolved.base.as_ref();
    let base_track = base.map(|b| b.track.clone());
    info!(
        "map {id} from {} ({}): \"{}\", {}",
        map.source,
        map.layer,
        toml.title,
        match base {
            Some(b) => format!("a layout on {} ({} drop {})", b.drop, b.track, b.index),
            None => "a new track".into(),
        }
    );
    let map_terrain = terrain::MapTerrain::build(&map.value.definition, &big_root)?;
    let (vertices, triangles) = map_terrain.counts();
    let markers = map_markers(&content, &map.value.definition)?;
    let (mv, mt) = markers.iter().fold((0, 0), |(v, t), m| (v + m.look.vertices(), t + m.look.triangles()));
    let terrain_used = Usage { vertices: vertices as u64 + mv, triangles: triangles as u64 + mt, ..Usage::default() };
    let name = base_track.clone().unwrap_or_else(|| id.path().to_string());
    let (track, materials) = match &base_track {
        Some(t) => {
            let mut track = terrain::load(&big_root.join(TRACKS).join(t), spec.subdivide, None)?;
            let materials = map_terrain.add_to(&mut track);
            (track, materials)
        }
        None => map_terrain.into_track(&name)?,
    };
    let shading = match spec.shading {
        Some(ShadingArg::Height) => terrain::Shading::Vertex(terrain::ColorMode::Height),
        Some(ShadingArg::Material) => terrain::Shading::Vertex(terrain::ColorMode::Material),
        _ => terrain::Shading::Textured(terrain::map_textured(&big_root, base_track.as_deref(), &materials)?),
    };
    let camera = camera::CameraStart {
        pose: camera::initial_pose(&track, spec.camera, spec.look_at, spec.aspect),
        bounds: track.bounds,
    };
    let lighting = map_lighting(&big_root, &content, &resolved, &name)?;
    let objects = match spec.objects {
        false => None,
        true => {
            let source =
                objects::ObjectsSource::Map { base: base_track.clone(), map: map.clone(), terrain: terrain_used };
            Some(objects::load_from(&big_root, &source)?)
        }
    };
    let mut status = Vec::new();
    let sky = sky::load(&lighting, &big_root)
        .inspect(|s| status.push(s.status.clone()))
        .inspect_err(|e| {
            warn!("game sky off: {e:#}");
            status.push(format!("sky: off ({e:#}); flat clear colour"));
        })
        .ok();
    let post = post::load(&lighting, &big_root, spec.screenshot)
        .inspect(|p| status.push(p.status.clone()))
        .inspect_err(|e| {
            warn!("game post chain off: {e:#}");
            status.push(format!("post: off ({e:#}); Bevy tonemapping"));
        })
        .ok();
    let drawn = objects.as_ref().and_then(|o| o.stats.map_drawn).unwrap_or(terrain_used);
    let marked = (markers, (drawn.vertices, drawn.triangles));
    let (ride, riders) = match &spec.ride {
        Some(options) => {
            let (load, decision) = map_ride(options, &spec.character, &big_root, &resolved, &lighting, marked)?;
            (Some(load), Some(decision))
        }
        None => (None, None),
    };
    let live = spec.ride.as_ref().is_some_and(|o| o.live.is_some());
    let bots = live.then(|| ride::bots::load_live_only(&big_root, base_track.as_deref(), riders.as_ref())).flatten();
    let heli = ride::helicopter::load_level(
        Some(&big_root),
        base_track.as_deref(),
        ride.as_ref().and_then(ride::RideLoad::heli_intro),
    );
    let particles = crate::particles::load_level(
        &big_root,
        ride.as_ref().map(ride::RideLoad::ride),
        Some(&lighting),
        ride.as_ref().and_then(ride::RideLoad::avalanche),
    );
    Ok(LevelData {
        name,
        big_root: Some(big_root),
        track,
        shading,
        camera,
        objects,
        lighting: Some(lighting),
        sky,
        post,
        status,
        ride,
        heli,
        ghosts: None,
        geotags: None,
        bots,
        particles,
        load_secs: start.elapsed().as_secs_f32(),
        map_music: toml.music.clone(),
    })
}

fn map_lighting(
    big_root: &Path,
    content: &std::sync::Arc<GameContent>,
    m: &ResolvedMap,
    name: &str,
) -> Result<lighting::GameLighting> {
    let toml = &m.map.value.toml;
    let named_postfx = toml.postfx.as_ref().map(|p| p.path().to_string());
    let with_sky = |mut p: ssx_assets::track_lighting::LightingPreset| {
        if toml.sky.is_some() {
            p.sky = toml.sky.clone();
        }
        p
    };
    match &m.base {
        Some(b) => {
            let drops = ssx_assets::track_lighting::load_track_drops(big_root, &b.track)?;
            let index = drops
                .iter()
                .position(|d| d.key == b.ge_drop)
                .with_context(|| format!("{}: drop {:#010x} of {} has no lighting", m.map.id, b.ge_drop, b.track))?;
            let mut l = lighting::load(big_root, &b.track, &lighting::DropSelect::Index(index))?;
            let preset = m.lighting.clone().unwrap_or_else(|| l.preset.clone());
            if m.lighting.is_some() || toml.sky.is_some() {
                l = l.with_map_preset(big_root, with_sky(preset.clone()))?;
            }
            match (named_postfx, &toml.lighting) {
                (Some(p), _) => l.postfx = Some(p),
                (None, Some(id)) => l.postfx = content.paired_postfx(id)?,
                (None, None) => {}
            }
            Ok(l)
        }
        None => {
            let lit_by: ContentId = match &toml.lighting {
                Some(id) => id.clone(),
                None => lighting::NEW_TRACK_LIGHTING.parse()?,
            };
            let preset = match &m.lighting {
                Some(p) => p.clone(),
                None => content.map_lighting(&lit_by)?,
            };
            let postfx = match named_postfx {
                Some(p) => Some(p),
                None => content.paired_postfx(&lit_by)?,
            };
            let midground = match &toml.midground {
                Some(t) => Some(content.map_midground(t).map(|_| t.path().to_string())?),
                None => None,
            };
            lighting::GameLighting::for_new_track(big_root, name, with_sky(preset), postfx, midground)
        }
    }
}

fn map_ride(
    cli: &RideOptions,
    character: &str,
    big_root: &Path,
    m: &ResolvedMap,
    lighting: &lighting::GameLighting,
    (markers, drawn): (Vec<ride::MapMarker>, (u64, u64)),
) -> Result<(ride::RideLoad, ssx_assets::content::characters::budget::RiderLooks)> {
    let start = Instant::now();
    let content = GameContent::of(big_root);
    let policy = match cli.live.is_some() {
        true => content.session_policy().unwrap_or(ssx_content::policy::ClientMods::SESSION_DEFAULT),
        false => ssx_content::policy::ClientMods::Any,
    };
    content.set_sim_policy(policy);
    let mut level = load_map_level(big_root, m, DEFAULT_GEAR_MIX)?;
    let mode = level.ride_mode(big_root, &content, m, cli.map_mode.as_deref())?;
    let r = &mode.rules;
    info!(
        "map {}: mode {} ({:?}), targets {:?}, time limit {:?} ms ({}), loops {}",
        m.map.id,
        mode.choice.name(),
        r.event_type(),
        r.targets(),
        r.time_limit_ms(),
        r.time_up().name(),
        r.loops()
    );
    let spawn = ssx_physics::spawn::spawn_pose_for_drop(&level.world, level.drop, cli.spawn, cli.heading)?;
    let data = big_root.parent().unwrap_or(big_root);
    let sim_content_id = match content.map_sim_content_id(data, &m.map, m.base.as_ref(), &m.object_list(), Some(&mode))
    {
        Ok(sid) => {
            info!("map {}: sim content id {} ({} rows)", m.map.id, sid.id.short(), sid.rows.len());
            Some(sid.id.to_string())
        }
        Err(e) => {
            warn!("map {}: no sim content id: {e:#}", m.map.id);
            None
        }
    };
    let gates = &level.gates;
    info!(
        "map {}: {} checkpoints, finish {}, time limit {:?} ms, {} volumes ({} death)",
        m.map.id,
        gates.checkpoints.len(),
        gates.finish.map_or("none".to_string(), |i| format!("volume {i}")),
        gates.time_limit_ms,
        level.world.triggers.volumes.len(),
        level.world.triggers.volumes.iter().filter(|v| v.type_text == "death").count()
    );
    let mut map_ride = ride::MapRide::new(m.map.id.clone(), m.map.value.toml.title.clone(), level.gates, level.setup);
    (map_ride.markers, map_ride.drawn) = (markers, drawn);
    map_ride.mode = Some(ride::MapRideMode::of(&mode));
    if let Some(h) = level.heli {
        match &h.choice {
            Ok(c) => info!(
                "map {}: helicopter {} ({:?}, game object {:08x}), height grid {}",
                m.map.id,
                c.name,
                c.source,
                c.gameobject,
                h.grid.as_ref().map_or("none".to_string(), |g| format!("{} × {}", g.width, g.height))
            ),
            Err(e) => warn!("map {}: no helicopter ({e})", m.map.id),
        }
        map_ride.heli = Some(std::sync::Arc::new(h));
    }
    let e = level.entities;
    if e.entities > 0 {
        info!(
            "map {}: {} entities ({} with a controller, {} with a rigid body, {} trigger volumes)",
            m.map.id, e.entities, e.scripted, e.bodies, e.volumes
        );
    }
    map_ride.placed = m
        .map
        .value
        .definition
        .props
        .iter()
        .enumerate()
        .filter_map(|(i, p)| match &p.look {
            ssx_content::map::PropLook::Object(id) => {
                let key = m.objects.get(id)?.key?;
                Some((u32::try_from(i).ok()?, key, ssx_physics::map::entities::prop_placement(p)))
            }
            _ => None,
        })
        .collect();
    let track_name = m.base.as_ref().map_or_else(|| m.map.id.path().to_string(), |b| b.track.clone());
    let w = RideWorld {
        world: level.world,
        spawn,
        drop_start: cli.spawn.is_none(),
        drop_key: None,
        load_secs: start.elapsed().as_secs_f32(),
        map: Some(map_ride),
        vault_track: m.base.as_ref().map(|b| b.track.as_str()),
        track_name: &track_name,
        sim_content_id,
    };
    let (setup, decision) = ride_setup(cli, character, big_root, Some(&lighting.level), w)?;
    let load = ride::prepare(setup, big_root, Some(lighting))?;
    Ok((load, decision))
}

fn map_markers(content: &GameContent, def: &ssx_content::map::MapDefinition) -> Result<Vec<ride::MapMarker>> {
    let mut looks = std::collections::BTreeMap::new();
    let mut out = Vec::with_capacity(def.markers.len());
    for p in &def.markers {
        let look = match looks.get(&p.marker) {
            Some(l) => std::sync::Arc::clone(l),
            None => {
                let l = std::sync::Arc::new(
                    content.marker(&p.marker).with_context(|| format!("node {:?}: its marker {}", p.node, p.marker))?,
                );
                looks.insert(p.marker.clone(), std::sync::Arc::clone(&l));
                l
            }
        };
        let q = bevy::math::Quat::from_array(p.placement.rotation);
        let heading = q.to_euler(bevy::math::EulerRot::YXZ).0.to_degrees();
        out.push(ride::MapMarker { node: p.node.clone(), look, position: p.placement.position, heading });
    }
    Ok(out)
}
