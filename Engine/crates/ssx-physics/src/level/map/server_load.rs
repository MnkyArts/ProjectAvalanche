use std::path::Path;
use std::time::{Duration, Instant};

use crate::character::DEFAULT_GEAR_MIX;
use crate::graph::RiderGraph;
use crate::map::{ResolvedMap, load_map_level, start_envelope};
use ssx_assets::content::GameContent;
use ssx_assets::content::gamemodes::{ModeChoice, finish_from_record, map_limit_ms};
use ssx_assets::nis::IntroNis;
use ssx_content::ContentId;
use ssx_content::gamemode::{Rules, TimeUp};
use ssx_content::map::MapToml;
use ssx_gameplay::validate::{Limits, MapLimits};
use ssx_net_proto::instance::built_in_mode;
use ssx_rules::events::EventType;
use ssx_rules::subset::TuningLimits;

#[derive(Clone, Debug)]
pub struct MapLoad {
    pub limits: Option<Limits>,
    pub title: String,
    pub mode: String,
    pub gamemode_title: Option<String>,
    pub rules: Rules,
    pub took: Duration,
    pub clusters: usize,
    pub triangles: usize,
    pub volumes: usize,
    pub gates: usize,
    pub start: Option<LoadedStart>,
}

impl MapLoad {
    pub fn describe(&self, id: &str) -> String {
        let start = match &self.start {
            Some(s) => format!(
                "; its start hands over at execution {} at {:.3?} in spawn 0's frame, the envelope's starts within \
                 {:.4} m of its place and {:.4?} m of its first executions ({} ms)",
                s.handover,
                s.place,
                s.place_spread,
                s.first_spread,
                s.took.as_millis()
            ),
            None => "; a freeride: its start stands on ground (no rounds to validate)".to_string(),
        };
        let r = &self.rules;
        let gamemode = self.gamemode_title.as_deref().map(|t| format!(" \"{}\"", ssx_core::text::shown(t)));
        let limit = r.time_limit_ms().map_or(String::new(), |ms| {
            format!(
                ", a time limit of {ms} ms ending {}",
                if r.time_up() == TimeUp::Result { "with a result" } else { "failed" }
            )
        });
        format!(
            "map {id} ({}) under {}{} ({} {}{limit}, {} targets) loaded in {} ms: {} collision clusters, {} triangles, \
             {} volumes, {} gates{start}",
            ssx_core::text::shown(&self.title),
            ssx_core::text::shown(&self.mode),
            gamemode.unwrap_or_default(),
            r.event_type().name(),
            r.event_type() as u8,
            r.targets().len(),
            self.took.as_millis(),
            self.clusters,
            self.triangles,
            self.volumes,
            self.gates
        )
    }
}

#[derive(Clone, Copy, Debug)]
pub struct LoadedStart {
    pub handover: u32,
    pub place: [f32; 3],
    pub place_spread: f32,
    pub first_spread: [f32; crate::map::START_FIRST],
    pub took: Duration,
}

// PORT(choice): one rider's graph stands for every rider's — the twelve riders' graphs gave the same starts on the
// template slope (4 spots, 1 440 starts each) and three riders' on the alps0101 layout (8 spots, 27 gears × 34 inputs
// each: the largest three distances equal), while a start without a graph lay up to 0.14 / 0.22 / 0.14 / 0.15 m from
// the riders' after 1 to 4 executions on one spot of the layout; tuning: the server running the joining rider's own
// graph (the rider's look names it) or a proof that the graph's output the start reads is the same for every rider; the
// original has no validation.
pub const START_RIDER: &str = "alex";

pub fn mode_choice(id: &str, event_type: u8, gamemode: Option<&str>) -> Result<ModeChoice, String> {
    let name = match gamemode {
        Some(g) => g,
        None => built_in_mode(event_type).ok_or_else(|| {
            format!(
                "{id}: event type {event_type} is no built-in mode's (race 5, trick 0, survive 3, death 4, freeride 8; \
                 the others ride under a gamemode)"
            )
        })?,
    };
    ModeChoice::parse(name).map_err(|e| format!("{id}: {}", ssx_core::text::message(&e)))
}

pub fn record_rules(
    content: &GameContent,
    cid: &ContentId,
    toml: &MapToml,
    choice: &ModeChoice,
) -> Result<(Rules, Option<String>), String> {
    let limit = map_limit_ms(toml);
    match choice {
        ModeChoice::BuiltIn(m) => {
            if !toml.modes.contains(m) {
                let offered: Vec<&str> = toml.modes.iter().map(|m| m.name()).collect();
                return Err(format!("{cid} offers the modes {offered:?}, not {}", m.name()));
            }
            let rules = Rules::plain(m.event_type(), limit).map_err(|e| format!("{cid}: {e}"))?;
            Ok((rules, None))
        }
        ModeChoice::Gamemode(g) => {
            let found = content.gamemode(g).map_err(|e| ssx_core::text::message(&e.to_string()).to_string())?;
            let has_finish = finish_from_record(toml).unwrap_or(true);
            found.value.takes_start(toml.intro).map_err(|e| format!("{g} on {cid}: {e}"))?;
            let rules = found.value.rules(cid, limit, has_finish).map_err(|e| format!("{g} on {cid}: {e}"))?;
            Ok((rules, Some(found.value.title.clone())))
        }
    }
}

pub fn load_resolved(
    folder: &Path,
    content: &GameContent,
    id: &str,
    level_of: (u8, Option<&str>),
    resolved: &ResolvedMap,
    tuning: &TuningLimits,
) -> Result<MapLoad, String> {
    let t0 = Instant::now();
    let (event_type, gamemode) = level_of;
    let choice = mode_choice(id, event_type, gamemode)?;
    let toml = &resolved.map.value.toml;
    check_toml(folder, content, id, toml)?;
    record_rules(content, &resolved.map.id, toml, &choice)?;
    let mut level = load_map_level(folder, resolved, DEFAULT_GEAR_MIX)
        .map_err(|e| format!("{id}: its level: {}", ssx_core::text::message(&format!("{e:#}"))))?;
    let ridden = level
        .ride_mode(folder, content, resolved, Some(&choice.name()))
        .map_err(|e| ssx_core::text::message(&format!("{e:#}")).to_string())?;
    let rules = ridden.rules;
    let t = rules.event_type();
    if t as u8 != event_type {
        return Err(format!(
            "{id} under {}: its rules ride {} ({}), the server's level names event type {event_type}",
            choice.name(),
            t.name(),
            t as u8
        ));
    }
    let end: Vec<_> = level.world.triggers.volumes.iter().filter(|v| v.name == "end").cloned().collect();
    if end.is_empty() && t != EventType::FreeRide {
        return Err(format!("{id} has no volume named `end`, which a {} needs", t.name()));
    }
    let (limits, loaded_start) = match t {
        EventType::FreeRide => {
            level
                .prepare_start_intro(folder, START_RIDER)
                .map_err(|e| format!("{id}: its helicopter intro: {}", ssx_core::text::message(&format!("{e:#}"))))?;
            crate::map::start_rider(&level).map_err(|e| format!("{id}: its start: {e:#}"))?;
            (None, None)
        }
        _ => {
            let ts = Instant::now();
            level
                .prepare_start_intro(folder, START_RIDER)
                .map_err(|e| format!("{id}: its helicopter intro: {}", ssx_core::text::message(&format!("{e:#}"))))?;
            let intro = level.start_intro.as_ref().map(|i| &i.nis);
            let mut graph = start_graph(folder, resolved.base.as_ref().map(|b| b.track.as_str()), intro)
                .map_err(|e| format!("{id}: {e}"))?;
            let env = start_envelope(&mut level, Some(&mut graph)).map_err(|e| format!("{id}: its start: {e:#}"))?;
            let start = env.start;
            let w = &level.world;
            let course = level.setup.course.ok_or_else(|| format!("{id}: no course (no spawn slot 0 or no finish)"))?;
            let spawns = w
                .starts
                .get(level.drop)
                .map(|d| d.slots.iter().map(|s| crate::Vec3::from_array(s.position)).collect())
                .unwrap_or_default();
            let limits = Limits::of_map(
                MapLimits {
                    id: id.to_string(),
                    event_type: Some(t),
                    spawns,
                    end,
                    splits: course.splits(),
                    gates: level.gates.checkpoints.clone(),
                    time_limit_ms: rules.time_limit_ms(),
                    time_up: rules.time_up(),
                    handover: start.handover,
                    slot: start.slot,
                    rotation: start.rotation,
                    place: start.place,
                    first: start.first,
                    place_spread: env.place,
                    first_spread: env.first,
                },
                tuning,
            )?;
            let loaded = LoadedStart {
                handover: start.handover,
                place: start.place.to_array(),
                place_spread: env.place,
                first_spread: env.first,
                took: ts.elapsed(),
            };
            (Some(limits), Some(loaded))
        }
    };
    let w = &level.world;
    let c = &level.collision;
    Ok(MapLoad {
        limits,
        title: toml.title.clone(),
        mode: choice.name(),
        gamemode_title: ridden.gamemode.as_ref().map(|g| g.value.title.clone()),
        rules,
        took: t0.elapsed(),
        clusters: c.terrain_clusters + c.rail_clusters + c.collision_clusters,
        triangles: c.triangles(),
        volumes: w.triggers.volumes.len(),
        gates: level.gates.checkpoints.len(),
        start: loaded_start,
    })
}

fn start_graph(folder: &Path, track: Option<&str>, intro: Option<&IntroNis>) -> Result<RiderGraph, String> {
    let graph = match track {
        Some(t) => RiderGraph::load(folder, t, START_RIDER),
        None => RiderGraph::load_untracked(folder, START_RIDER),
    };
    let mut graph = graph.map_err(|e| {
        format!(
            "its start is checked with a rider's animation ({START_RIDER}'s), whose data is not in the rules folder {} \
             (`avalanche-server setup --with verify` takes it from the disc): {}",
            shown_folder(folder),
            ssx_core::text::message(&format!("{e:#}"))
        )
    })?;
    graph
        .set_intro(folder, intro)
        .map_err(|e| format!("{START_RIDER}'s level start: {}", ssx_core::text::message(&format!("{e:#}"))))?;
    Ok(graph)
}

fn shown_folder(folder: &Path) -> String {
    ssx_core::text::shown_max(&folder.display().to_string(), ssx_core::text::PATH_CHARS).to_string()
}

pub fn check_toml(folder: &Path, content: &GameContent, id: &str, toml: &MapToml) -> Result<(), String> {
    if let Some(base) = &toml.base {
        let b = content
            .layout_base(base)
            .map_err(|e| format!("{id}: its base {base}: {}", ssx_core::text::message(&format!("{e:#}"))))?;
        let dir = folder.join("data0/data/xenon/gameexplorer").join(&b.track);
        let terrain = std::fs::read_dir(&dir)
            .map(|d| d.flatten().any(|e| e.path().extension().is_some_and(|x| x == "subd")))
            .unwrap_or(false);
        if !terrain {
            return Err(format!(
                "{id} is a layout on {} (track {}), whose terrain is not in the rules folder {}: `avalanche-server \
                 setup --with collision` takes it from the disc",
                b.ui_drop,
                b.track,
                shown_folder(folder)
            ));
        }
    }
    Ok(())
}
