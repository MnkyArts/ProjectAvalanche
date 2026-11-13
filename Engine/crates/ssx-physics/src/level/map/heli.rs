use std::path::Path;
use std::sync::Arc;

use anyhow::Result;
use glam::{Quat, Vec3};
use ssx_assets::helicopter::HeliChoice;
use ssx_content::map::{HELI_LOCATORS, HeliPoint, Intro, MapDefinition};

use crate::event::EventType;
use crate::helicopter::HeightGrid;
use crate::nis::{IntroAssets, IntroPath};
use crate::objectives::{EVENT_TYPE_SCRIPT, EventObjectives};
use crate::rider::World;
use crate::triggers::{TriggerScope, TriggerSet, TriggerVolume, VolumeShape, interaction_mask, parse_group_mask};

pub const OUTRO_HELI_PICKUP: &str = "outro_heli_pickup.lua";

pub const OUTRO_STANDARD: &str = "outro_standard_celebration.lua";

pub fn title_update_free_ride_outro() -> bool {
    ssx_assets::data_roots::title_update().is_some()
        && ssx_assets::data_roots::overlay_only(&format!("data/common/scripts/{OUTRO_STANDARD}")).is_some()
}

pub fn intro_script(t: EventType) -> &'static str {
    match t {
        EventType::RaceClock | EventType::RaceGhosts => "eventtype_race.lua",
        // TODO(guess): a map's race against bots plays the race intro (`eventtype_race.lua`, `nis_binding_intro_race`) as its family's helicopter start; the disc's 22 RaceBots `ui_drop`s (of 265) list no `eventtype_race.lua`: 18 are snow starts (`eventtype_headtohead.lua`), 3 deadly descents (`eventtype_dd_lhotse`, `_lhotse_foreshadow`, `_newz`), 1 `eventtype_trick.lua`; settle: none on the disc (no RaceBots event starts with the race's helicopter intro).
        EventType::RaceBots => {
            crate::guess_hit!();
            "eventtype_race.lua"
        }
        EventType::TrickScore | EventType::TrickBots | EventType::TrickGhosts => "eventtype_trick.lua",
        EventType::SurviveToDeath => "eventtype_survive_til_die.lua",
        // TODO(guess): a map's survive-to-bottom run plays `eventtype_survive_til_die.lua`'s intro (`nis_binding_intro_survive`): the disc's 10 SurviveToBottom events are the deadly descents and the skyboarding drop, each with its own `eventtype_dd_*` / `eventtype_skyboarding` script bound to its mountain; the disc's `eventtype_survive.lua` plays the same NIS with the same actors and differs only in its objective's name ("Survive Intro NIS" vs "Survive Til Die Intro NIS", read by no script and no code), but no vault names it (attribdb.bin's strings) and a dedicated server's rules subset (`server-files.tsv`, 342 rows) does not carry it; settle: none on the disc (no generic survive-to-bottom event exists).
        EventType::SurviveToBottom => {
            crate::guess_hit!();
            "eventtype_survive_til_die.lua"
        }
        // TODO(guess): a map's free ride plays the race intro (`eventtype_race.lua`, `nis_binding_intro_race`); the disc has 0 FreeRide `ui_drop`s of 265 (`ui_drop` event type 8 is never set), so no event script names an intro for one; settle: none on the disc (no free-ride event exists).
        EventType::FreeRide => {
            crate::guess_hit!();
            "eventtype_race.lua"
        }
    }
}

pub fn event_scripts(t: EventType, intro: Intro, title_update_outro: bool) -> Vec<String> {
    let mut list = vec![EVENT_TYPE_SCRIPT.to_string()];
    if intro == Intro::Heli {
        list.push(intro_script(t).to_string());
        if t == EventType::SurviveToDeath {
            list.push(OUTRO_HELI_PICKUP.to_string());
        }
        if t == EventType::FreeRide && title_update_outro {
            list.push(OUTRO_STANDARD.to_string());
        }
    }
    list
}

pub fn free_ride_picks_up(setup: &crate::event::EventSetup) -> bool {
    setup.free_ride && setup.objectives.as_ref().is_some_and(|o| o.scripts.list.iter().any(|s| s == OUTRO_STANDARD))
}

pub fn locator_volume(p: &HeliPoint, index: usize) -> TriggerVolume {
    let g = parse_group_mask("locator");
    TriggerVolume {
        name: HELI_LOCATORS[p.order as usize % HELI_LOCATORS.len()].to_string(),
        type_text: "locator".into(),
        groups: g,
        interacts: interaction_mask(g),
        shape: VolumeShape::Sphere { radius: 0.0 },
        centre: Vec3::from_array(p.position),
        rotation: Quat::from_array(p.rotation).normalize(),
        object: 0,
        instance: 0,
        placement: index,
        scope: TriggerScope::Track,
    }
}

pub fn place_locators(set: &mut TriggerSet, def: &MapDefinition) {
    if def.heli_path.is_empty() {
        return;
    }
    set.volumes.retain(|v| !(v.type_text == "locator" && HELI_LOCATORS.contains(&v.name.as_str())));
    for p in &def.heli_path {
        let v = locator_volume(p, set.volumes.len());
        set.volumes.push(v);
    }
}

#[derive(Clone, Debug)]
pub struct MapHeli {
    pub choice: Result<HeliChoice, String>,
    pub grid: Option<HeightGrid>,
    pub vault_track: Option<String>,
}

impl MapHeli {
    pub fn load(big_root: &Path, base: Option<&ssx_assets::content::maps::LayoutBase>, def: &MapDefinition) -> Self {
        let (choice, grid, vault_track) = match base {
            Some(b) => {
                let dir = big_root.join("data0/data/xenon/gameexplorer").join(&b.track);
                (
                    ssx_assets::helicopter::drop_helicopter(big_root, &b.ui_drop),
                    HeightGrid::load(&dir, &b.track).ok(),
                    Some(b.track.clone()),
                )
            }
            None => (ssx_assets::helicopter::default_helicopter(big_root), terrain_grid(def).ok(), None),
        };
        MapHeli { choice: choice.map_err(|e| format!("{e:#}")), grid, vault_track }
    }
}

pub fn terrain_grid(def: &MapDefinition) -> Result<HeightGrid> {
    let g = ssx_content::map::heli::heli_grid(def).map_err(anyhow::Error::msg)?;
    let cell = ssx_content::map::heli::HELI_CELL;
    Ok(HeightGrid {
        min: Vec3::from_array(g.min),
        max: Vec3::from_array(g.max),
        inv_cell: [1.0 / cell, 1.0 / cell],
        width: g.width,
        height: g.height,
        cells: g.cells,
    })
}

pub fn map_intro(
    world: &mut World,
    big_root: &Path,
    objectives: &EventObjectives,
    heli: &MapHeli,
    map: &str,
    character: &str,
    path: IntroPath,
) -> Result<Option<Arc<IntroAssets>>> {
    let Some(choice) = crate::nis::intro_of_probe(&objectives.probe) else { return Ok(None) };
    crate::nis::prepare_choice(world, big_root, choice, heli.choice.clone(), map, character, path).map(Some)
}

pub fn map_heli_pickup(big_root: &Path, heli: &MapHeli, map: &str, character: &str) -> Result<Arc<IntroAssets>> {
    crate::outro::prepare_heli_pickup_with(big_root, heli.choice.clone(), map, character)
}
