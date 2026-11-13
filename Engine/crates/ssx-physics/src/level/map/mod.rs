mod collision;
pub mod entities;
mod gates;
pub mod heli;
mod player;
#[cfg(test)]
mod tests;

pub use collision::{
    CLUSTER_CELL, CLUSTER_TRIANGLES, MapCollisionStats, TERRAIN_SURFACE, add_map_collision, add_map_collision_with_cap,
};
pub use gates::{FINISH_GROUPS, GateStep, Gates};
pub use heli::MapHeli;
pub use player::MapPlayer;
pub mod server_load;
mod start;
pub use start::{
    BoxedInput, ENVELOPE_INPUTS, ENVELOPE_SEQUENCES, MapStart, START_FIRST, START_IDLE, StartControls, StartDeaths,
    StartEnvelope, StartInput, deaths_at, envelope_gears, envelope_sequence, map_start, map_start_after, neutral,
    sequence_input, start_deaths, start_envelope, start_rider,
};

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context, Result, anyhow, bail};
use glam::{Quat, Vec3};
use ssx_assets::content::GameContent;
use ssx_assets::content::gamemodes::{ModeChoice, ResolvedMode};
use ssx_assets::content::maps::{LayoutBase, MapObject};
use ssx_assets::content::resolve::Found;
use ssx_assets::track_lighting::LightingPreset;
use ssx_content::ContentId;
use ssx_content::map::{
    BOUNDARY_CELL, BoundaryRaster, MAX_COLLISION_TRIANGLES, MAX_DLC_COLLISION_TRIANGLES, Map, Volume, VolumeShape,
};
use ssx_rules::locators::DropStart;
use ssx_rules::placement::Placement;

use crate::character::Tuning;
use crate::collision::CollisionWorld;
use crate::event::{Course, EventSetup};
use crate::rider::World;
use crate::rider::boundary::BoundaryGrid;
use crate::triggers::{TriggerScope, TriggerSet, TriggerVolume, interaction_mask, parse_group_mask};

#[derive(Clone, Debug)]
pub struct ResolvedMap {
    pub map: Found<Map>,
    pub objects: BTreeMap<ContentId, MapObject>,
    pub base: Option<LayoutBase>,
    pub lighting: Option<LightingPreset>,
}

impl ResolvedMap {
    pub fn resolve(content: &GameContent, id: &ContentId) -> Result<Self> {
        let map = content.map(id).map_err(|e| anyhow!("{e}"))?;
        let objects = content.map_objects(&map.value).map_err(|e| anyhow!("{id}: {e}"))?;
        let base = map.value.toml.base.as_ref().map(|b| content.layout_base(b)).transpose()?;
        let lighting = map.value.toml.lighting.as_ref().map(|l| content.map_lighting(l)).transpose()?;
        Ok(ResolvedMap { map, objects, base, lighting })
    }

    pub fn object_list(&self) -> Vec<MapObject> {
        self.objects.values().cloned().collect()
    }
}

pub struct MapLevel {
    pub world: World,
    pub drop: usize,
    pub gates: Gates,
    pub setup: EventSetup,
    pub collision: MapCollisionStats,
    pub intro: ssx_content::map::Intro,
    pub heli: Option<MapHeli>,
    pub start_intro: Option<std::sync::Arc<crate::nis::IntroAssets>>,
    pub entities: entities::MapEntityStats,
}

impl MapLevel {
    pub fn apply_rules(&mut self, big_root: &Path, rules: &ssx_rules::gamemode::Rules, medals: bool) {
        use ssx_rules::gamemode::{Rivals, TimeUp};
        let t = rules.event_type();
        let s = &mut self.setup;
        s.event_type = Some(t);
        if medals && rules.rivals() != Rivals::None {
            s.rivals = rules.board_values();
            s.results = Some(ssx_rules::results::ResultsFlow::Explore);
        }
        let drop = self.world.survival.objectives.as_deref();
        let scripts = heli::event_scripts(t, self.intro, heli::title_update_free_ride_outro());
        let objectives = crate::objectives::EventObjectives::for_map(big_root, drop, t, &scripts);
        s.objectives = Some(std::sync::Arc::new(objectives));
        s.time_up_result = rules.time_up() == TimeUp::Result;
        // PORT(choice): a map's free ride is ridden with the title update's Freeride flag (TU2 flow+0x10774: the HUD's
        // setter 0x82397658 writes only `HUD_RaceStarted` and the meters, objective messages pass unshown at
        // 0x824A77C0, no explore ghosts at 0x8244C19C, the run loops after its finish at 0x82495e3c instead of posting
        // results); the disc has no free ride (`ui_drop` event type 8 is never set, 0 of 265) and retail's HUD setter
        // 0x8238BB00 has no type-8 rule, so without the flag a map's free ride showed the clock, score and trick panels
        // of a timed event; after the finish (the user's decision, 2026-11-13) a map with its helicopter (`intro =
        // "heli"`, the `heli_path` pickup) and a mounted title update plays TU2's `outro_standard_celebration.lua` as a
        // disc drop's Freeride does (`FreerideFinish` → the heli pickup at `nis_outro_locator`, `DO_RestartObjective`),
        // any other map's free ride rides on after its finish with no outro and no results; the disc has no free-ride
        // event.
        s.free_ride = t == crate::event::EventType::FreeRide;
        if s.free_ride {
            crate::guess_hit!();
        }
        self.gates.time_limit_ms = rules.time_limit_ms();
    }

    pub fn prepare_start_intro(&mut self, big_root: &Path, character: &str) -> Result<()> {
        let (Some(heli), Some(objectives)) = (self.heli.as_ref(), self.setup.objectives.clone()) else {
            self.start_intro = None;
            return Ok(());
        };
        let path = crate::nis::IntroPath::Short;
        let intro = heli::map_intro(&mut self.world, big_root, &objectives, heli, "map", character, path)?;
        self.start_intro = Some(intro.context("the helicopter's scripts play no intro")?);
        Ok(())
    }

    fn check_heli_scripts(&self) -> Result<()> {
        if self.setup.event_type.is_none() {
            return Ok(());
        }
        let o = self.setup.objectives.as_ref().context("the event's objectives were not built")?;
        for name in &o.scripts.list {
            if !o.scripts.sources.contains_key(name) {
                bail!("the event's script {name} is not in the data folder ({})", crate::spawn::SCRIPTS_DIR);
            }
        }
        crate::nis::intro_of_probe(&o.probe).context("the helicopter's event scripts play no intro NIS")?;
        Ok(())
    }

    fn check_heli_pickup(&self) -> Result<()> {
        let heli = self.heli.as_ref().context("a survive-to-death run starts in the helicopter (intro = \"heli\")")?;
        let (p, q) = crate::outro::outro_locator(&self.world).context("the level has no nis_outro_locator")?;
        crate::rider::RiderState::level_start(&self.world, p, q).with_context(|| {
            format!(
                "no ground under the outro locator at {p:.2} (heli_path order 1): the heli pickup starts the rider \
                 there on the surface from {} m above it to {} m below (the disc's outro locators stand on the ground, \
                 −0.13 … +0.27 m on 170 of its 171 drops)",
                crate::rider::SPAWN_RAY_UP,
                crate::rider::SPAWN_RAY_LENGTH - crate::rider::SPAWN_RAY_UP
            )
        })?;
        heli.choice.as_ref().map_err(|e| anyhow!("the helicopter: {e}"))?;
        Ok(())
    }

    pub fn has_finish(&self) -> bool {
        self.gates.finish.is_some() || self.world.triggers.volumes.iter().any(|v| v.name == "end")
    }
    pub fn ride_mode(
        &mut self,
        big_root: &Path,
        content: &GameContent,
        m: &ResolvedMap,
        choice: Option<&str>,
    ) -> Result<ResolvedMode> {
        let choice = match choice {
            Some(c) => ModeChoice::parse(c).map_err(|e| anyhow!("{}: {e}", m.map.id))?,
            None => {
                let first = m.map.value.toml.modes.first().with_context(|| format!("{}: no mode", m.map.id))?;
                ModeChoice::BuiltIn(*first)
            }
        };
        let mode = content.resolve_mode(&m.map, &choice, self.has_finish()).map_err(|e| anyhow!("{e}"))?;
        self.apply_rules(big_root, &mode.rules, mode.gamemode.is_some());
        if self.heli.is_some() {
            self.check_heli_scripts().with_context(|| format!("{}: the helicopter intro", m.map.id))?;
        }
        if mode.rules.loops() {
            self.check_heli_pickup().with_context(|| format!("{}: survive to death", m.map.id))?;
        }
        if heli::free_ride_picks_up(&self.setup) {
            self.check_heli_pickup().with_context(|| format!("{}: the free ride's heli pickup", m.map.id))?;
        }
        Ok(mode)
    }
}

pub fn trigger_volume(v: &Volume, groups: &str, index: usize) -> TriggerVolume {
    let (shape, centre, rotation) = match v.shape {
        VolumeShape::Box { centre, rotation, half_extents } => (
            crate::triggers::VolumeShape::Box { half_extents: Vec3::from_array(half_extents) },
            centre,
            Quat::from_array(rotation).normalize(),
        ),
        VolumeShape::Sphere { centre, radius } => {
            (crate::triggers::VolumeShape::Sphere { radius }, centre, Quat::IDENTITY)
        }
    };
    let g = parse_group_mask(groups);
    TriggerVolume {
        name: v.name.clone(),
        type_text: groups.to_string(),
        groups: g,
        interacts: interaction_mask(g),
        shape,
        centre: Vec3::from_array(centre),
        rotation,
        object: 0,
        instance: 0,
        placement: index,
        scope: TriggerScope::Track,
    }
}

pub fn boundary_grid(r: &BoundaryRaster) -> BoundaryGrid {
    let min = Vec3::new(r.min[0], 0.0, r.min[1]);
    let span = Vec3::new(r.width as f32 * BOUNDARY_CELL, 0.0, r.height as f32 * BOUNDARY_CELL);
    BoundaryGrid {
        min,
        max: min + span - Vec3::new(0.01, 0.0, 0.01),
        cells_per_metre: Vec3::new(1.0 / BOUNDARY_CELL, 0.0, 1.0 / BOUNDARY_CELL),
        width: r.width,
        height: r.height,
        cells: r.cells.clone(),
    }
}

pub fn load_map_level(big_root: &Path, m: &ResolvedMap, gear_mix: f32) -> Result<MapLevel> {
    let def = &m.map.value.definition;
    let base_dir = match &m.base {
        Some(b) => Some(
            ssx_assets::data_roots::track_dir(big_root, &b.track)
                .with_context(|| format!("{}: no track folder for {}", m.map.id, b.track))?,
        ),
        None => None,
    };
    let cap = match &base_dir {
        Some(dir) if ssx_assets::data_roots::in_packages(dir) => MAX_DLC_COLLISION_TRIANGLES,
        _ => MAX_COLLISION_TRIANGLES,
    };
    let mut stats = MapCollisionStats::default();
    let mut add = |c: &mut CollisionWorld| -> Result<()> {
        stats = add_map_collision_with_cap(c, def, &m.objects, cap)?;
        Ok(())
    };
    let (mut world, drop) = match &m.base {
        Some(b) => {
            let dir = base_dir.as_deref().context("a layout's track folder")?;
            let w = crate::spawn::track_world_for_drop_with(big_root, dir, &b.track, gear_mix, b.index, &mut add)
                .with_context(|| format!("{}: the world of {}", m.map.id, b.drop))?;
            (w, b.index)
        }
        None => {
            let mut collision = CollisionWorld::default();
            add(&mut collision)?;
            collision.boundary = def.boundary.as_ref().map(|b| boundary_grid(&b.raster));
            collision.finish();
            let tuning = Tuning::load(&crate::tuning::load(big_root)?)?;
            let props = tuning.resolve(gear_mix)?;
            let scripts = crate::objectives::DropScripts::load(big_root, Vec::new());
            let survival = crate::spawn::survival_of_scripts(scripts);
            let starts = vec![DropStart { drop_key: 0, slots: Vec::new(), terrain_files: Vec::new() }];
            let triggers = TriggerSet::default();
            let w =
                World { collision, tuning, props, starts, triggers, events: Vec::new(), end_locator: None, survival };
            (w, 0)
        }
    };
    if !def.spawns.is_empty() {
        let slots = def
            .spawns
            .iter()
            .map(|s| Placement { scale: [1.0; 3], rotation: s.rotation, position: s.position, unk: [0.0; 6] })
            .collect();
        world.starts.get_mut(drop).context("the base drop's start")?.slots = slots;
    }
    if let Some(l) = &m.lighting {
        crate::spawn::apply_lighting(&mut world.survival, Some(l));
    }
    let set = &mut world.triggers.volumes;
    if def.finish.is_some() {
        set.retain(|v| v.name != "end");
    }
    for v in &def.triggers {
        set.push(trigger_volume(v, "trigger", set.len()));
    }
    for v in &def.deaths {
        set.push(trigger_volume(v, "death", set.len()));
    }
    let finish = def.finish.as_ref().map(|f| {
        set.push(trigger_volume(f, FINISH_GROUPS, set.len()));
        world.end_locator = Some(Vec3::from_array(match f.shape {
            VolumeShape::Box { centre, .. } | VolumeShape::Sphere { centre, .. } => centre,
        }));
        set.len() - 1
    });
    let checkpoints = def.checkpoints.iter().enumerate().map(|(k, v)| trigger_volume(v, "", k)).collect();
    let limit = m.map.value.toml.time_limit_s.map(|s| (f64::from(s) * 1000.0).round() as u32);
    let gates = Gates::new(checkpoints, finish, limit);
    let start_y = world.starts.get(drop).and_then(|d| d.slot(0)).map(|(p, _)| p.y);
    let course = start_y.zip(world.end_locator).map(|(start_y, e)| Course { start_y, end_y: e.y });
    let setup = EventSetup::new(None, course);
    let map_volumes = def.checkpoints.len() + def.triggers.len() + def.deaths.len() + usize::from(def.finish.is_some());
    let entities = entities::place_entities(&mut world.triggers, big_root, def, &m.objects, map_volumes)
        .with_context(|| format!("{}: its entities", m.map.id))?;
    crate::map::heli::place_locators(&mut world.triggers, def);
    let intro = m.map.value.toml.intro;
    let heli = (intro == ssx_content::map::Intro::Heli).then(|| MapHeli::load(big_root, m.base.as_ref(), def));
    Ok(MapLevel { world, drop, gates, setup, collision: stats, intro, heli, start_intro: None, entities })
}
