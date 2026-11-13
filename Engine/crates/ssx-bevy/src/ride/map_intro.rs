use std::path::Path;
use std::sync::Arc;

use bevy::prelude::*;
use ssx_physics::map::MapHeli;
use ssx_physics::nis::{IntroAssets, IntroPath};
use ssx_physics::objectives::EventObjectives;
use ssx_physics::rider::World as PhysicsWorld;
use ssx_physics::session::survive::SurviveLoop;

type Intro = Option<Arc<IntroAssets>>;

pub(super) fn intros(
    world: &mut PhysicsWorld,
    big_root: &Path,
    character: &str,
    heli: &MapHeli,
    id: &str,
    objectives: Option<Arc<EventObjectives>>,
    visits: Option<u32>,
) -> (Intro, Intro) {
    let Some(objectives) = objectives else { return (None, None) };
    let Some(choice) = ssx_physics::nis::intro_of_probe(&objectives.probe) else {
        info!("map {id}: its scripts play no intro");
        return (None, None);
    };
    let wingsuit = world.survival.item_enabled(ssx_physics::character::SLOT_WINGSUIT);
    let requires = match ssx_assets::nis::requires_wingsuit(big_root, &choice.binding) {
        Ok(r) => r,
        Err(e) => {
            warn!("map {id}: the intro {} unreadable ({e:#}): the default level start", choice.binding);
            return (None, None);
        }
    };
    let decide = |v: u32| IntroPath::decide(false, false, requires, wingsuit, v);
    let path = match (std::env::var("SSX_INTRO_PATH").as_deref(), visits) {
        (Ok("long"), _) => IntroPath::Long,
        (Ok("medium"), _) => IntroPath::Medium,
        (Ok("short"), _) => IntroPath::Short,
        (Ok("profile"), _) => decide(1),
        (_, Some(v)) => decide(v),
        _ => IntroPath::Short,
    };
    info!("map {id}: the intro {} on the {path:?} path (rides {visits:?}, wingsuit {wingsuit})", choice.binding);
    let mut prepare = |path| {
        ssx_physics::map::heli::map_intro(world, big_root, &objectives, heli, id, character, path).unwrap_or_else(|e| {
            warn!("map {id}: intro: {e:#}; the default level start");
            None
        })
    };
    let first = prepare(path);
    let short = match path {
        IntroPath::Short => None,
        _ => prepare(IntroPath::Short),
    };
    (first, short)
}

pub(super) fn survive_loop(
    world: &PhysicsWorld,
    big_root: &Path,
    character: &str,
    m: &super::MapRide,
    (at, heading): (Vec3, Vec3),
    intro: &Intro,
    intro_short: &Intro,
) -> Option<SurviveLoop> {
    let free_ride = ssx_physics::map::heli::free_ride_picks_up(&m.setup);
    if m.setup.event_type != Some(ssx_physics::event::EventType::SurviveToDeath) && !free_ride {
        return None;
    }
    let heli = m.heli.as_ref()?;
    match ssx_physics::map::heli::map_heli_pickup(big_root, heli, &m.id.to_string(), character) {
        Ok(outro) => {
            let q = ssx_physics::spawn::level_start_orientation(world, at, heading);
            let what =
                if free_ride { "the free ride's loop (the title update's outro)" } else { "survive-to-death loop" };
            info!("map {}: {what}: {} after the finish, then the intro at {at:.2}", m.id, outro.nis.binding);
            let later = intro_short.clone().or_else(|| intro.clone());
            Some(SurviveLoop::new(later, Some(outro), at, q))
        }
        Err(e) => {
            warn!("map {}: the heli pickup's loop unavailable: {e:#}", m.id);
            None
        }
    }
}
