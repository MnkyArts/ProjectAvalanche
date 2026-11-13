use bevy::prelude::*;
use ssx_physics::DT_TICKS;
use ssx_physics::event::{Course, EventManager, EventSetup, choose_event};
use ssx_physics::rewind::RewindPad;
use ssx_physics::rider::{RiderState, TrickInput, World as PhysicsWorld};
use ssx_physics::session::Session;

use super::clock::pose_of;
use super::controls::PadLock;
use super::scripted::{AutoRewind, env_flag, press_script, wingsuit_script};
use super::{Clock, Ride, RideSetup};

impl Ride {
    pub(super) fn new(mut setup: RideSetup) -> Option<Self> {
        let want = std::env::var("SSX_RIDE_EVENT").unwrap_or_else(|_| "race".into());
        let def = match setup.event_key {
            _ if setup.map.is_some() => None,
            Some(key) => setup.world.events.iter().find(|e| e.key == key).cloned(),
            None if want == "none" => None,
            None => choose_event(setup.world.events.iter(), &want).cloned(),
        };
        setup.world.survival.apply_event(def.as_ref());
        let test_gear = std::env::var("SSX_RIDE_LOADOUT").is_ok() || std::env::var("SSX_RIDE_GEAR").is_ok();
        let game_setup = setup.setup.clone().filter(|s| s.character != 0 && !test_gear);
        if let Some(s) = &game_setup {
            let gear = s.gear_stats();
            let stats: Vec<String> = (0..37)
                .filter(|&i| gear.stats[i] != 0.0)
                .map(|i| format!("{} {}", ssx_physics::character::GEAR_STAT_NAMES[i], gear.stats[i]))
                .collect();
            info!("gear: the launch's game setup (0x82AB2C48): {}", stats.join(", "));
            if let Err(e) = setup.world.equip(&gear) {
                warn!("gear: the setup failed ({e:#}); the event's loadout");
                setup.world.equip(&ssx_physics::survival::event_loadout(&setup.world.survival)).ok();
            }
        } else if std::env::var("SSX_RIDE_LOADOUT").is_ok_and(|v| v == "none") {
            info!("survival: default gear (SSX_RIDE_LOADOUT=none)");
        } else {
            let gear = match std::env::var("SSX_RIDE_GEAR") {
                Ok(list) => {
                    for name in list.split(',').map(str::trim).filter(|n| !n.is_empty()) {
                        match required_value(name) {
                            Some(v) if !setup.world.survival.required.contains(&v) => {
                                setup.world.survival.required.push(v)
                            }
                            Some(_) => {}
                            None => warn!("SSX_RIDE_GEAR: unknown item {name:?}"),
                        }
                    }
                    let value = ssx_physics::survival::TEMPLATE_VALUE;
                    ssx_physics::survival::loadout(&setup.world.survival.required, value)
                }
                Err(_) => ssx_physics::survival::event_loadout(&setup.world.survival),
            };
            if let Err(e) = setup.world.equip(&gear) {
                warn!("survival: the loadout failed ({e:#}); default gear");
            }
        }
        // PORT(choice): without a game setup each equipped item is drawn as its first variant
        // (`equipment_description.id` 0) and the head lamp unscaled (factor 1); the look is the player's choice, which
        // only a front end's setup carries; the game always has that setup.
        let loadout = match &game_setup {
            Some(s) => {
                let entries: Vec<(usize, u32)> = s
                    .items
                    .iter()
                    .enumerate()
                    .filter(|(_, g)| !g.is_cleared())
                    .map(|(t, g)| (t, g.description))
                    .collect();
                let lamp = s.item(6).map_or([None; 2], |g| [g.stat(20), g.stat(21)]);
                ssx_assets::character::Loadout::from_setup(&entries, lamp)
            }
            None => ssx_assets::character::Loadout::from_slots(&setup.world.survival.slots, 0),
        };
        let d = &setup.world.survival;
        info!(
            "survival: cold {}, low oxygen {}, rock damage {}, permanent death {}, required gear {:?}",
            d.cold, d.low_oxygen, d.rock_damage, d.death_permanent, d.required
        );
        let (intro, intro_short) = match (&def, setup.drop_start && !crate::render::post::env_is("SSX_INTRO", "off")) {
            (Some(d), true) => {
                use ssx_physics::nis::IntroPath;
                let wingsuit = setup.world.survival.item_enabled(ssx_physics::character::SLOT_WINGSUIT);
                let decide = |visits: u32| {
                    ssx_physics::nis::profile_path(&setup.big_root, d, visits, wingsuit).unwrap_or(IntroPath::Short)
                };
                let path = match (std::env::var("SSX_INTRO_PATH").as_deref(), setup.intro_visits) {
                    (Ok("long"), _) => IntroPath::Long,
                    (Ok("medium"), _) => IntroPath::Medium,
                    (Ok("short"), _) => IntroPath::Short,
                    (Ok("profile"), _) => decide(1),
                    (_, Some(visits)) => decide(visits),
                    _ => IntroPath::Short,
                };
                info!("intro: the {path:?} path (range visits {:?}, wingsuit {wingsuit})", setup.intro_visits);
                let mut prepare = |path| {
                    ssx_physics::nis::prepare_path(&mut setup.world, &setup.big_root, d, &setup.character, path)
                        .unwrap_or_else(|e| {
                            warn!("intro: {e:#}; the default level start");
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
            (None, true) => match setup.map.as_ref().and_then(|m| Some((m.heli.clone()?, m))) {
                Some((heli, m)) => {
                    let (id, objectives) = (m.id.to_string(), m.setup.objectives.clone());
                    super::map_intro::intros(
                        &mut setup.world,
                        &setup.big_root,
                        &setup.character,
                        &heli,
                        &id,
                        objectives,
                        setup.intro_visits,
                    )
                }
                None => (None, None),
            },
            _ => (None, None),
        };
        match &intro {
            Some(i) => info!(
                "intro: {} ({}): ready loop {}, go {}; helicopter {}",
                i.choice.binding,
                i.nis.controller,
                i.nis.ready.sequence,
                i.nis.jump.sequence,
                match &i.heli {
                    Ok(h) => format!("{} ({:?}, game object {:08x})", h.name, h.source, h.gameobject),
                    Err(e) => format!("not found: {e}"),
                }
            ),
            None => {
                info!("intro: none (no event, a test spawn or SSX_INTRO=off): the default level start")
            }
        }
        let mut state = level_start(&setup.world, setup.spawn, intro.as_ref())?;
        state.tricks.classic = setup.classic;
        let course = setup.world.end_locator.map(|e| Course { start_y: setup.spawn.0.y, end_y: e.y });
        let event = match &setup.map {
            Some(m) => EventManager::new(m.setup.clone()),
            None => {
                EventManager::new(EventSetup { free_ride: setup.free_ride, ..EventSetup::new(def.as_ref(), course) })
            }
        };
        let mut session = Session { event, ..Session::default() };
        if let Some(m) = setup.map.as_mut() {
            m.gates.reset(&mut session);
        }
        match ssx_physics::event::rider_display_names(&setup.big_root) {
            Ok(names) => session.stats.rider = names.get(&setup.character).cloned(),
            Err(e) => warn!("the riders' names: {e:#}"),
        }
        let heli_vaults = match setup.map.as_ref().and_then(|m| m.heli.as_ref()) {
            Some(h) => h.vault_track.clone(),
            None => Some(setup.track.clone()),
        };
        if let Some(Ok(h)) = intro.as_ref().map(|i| &i.heli)
            && let Some(e) = ssx_physics::controllers::NamedEntity::load_in(
                &setup.big_root,
                heli_vaults.as_deref(),
                "DropHeli",
                h.gameobject,
            )
        {
            info!("controllers: DropHeli runs {}", e.script);
            session.named_entities.push(e);
        }
        if let Some(rules) = setup.live {
            session.bots = Some(live_rivals(&setup.big_root, def.as_ref(), rules, session.event.setup.course));
            let count = &mut session.manager.count;
            match rules.rewinds {
                super::live::LiveRewinds::Event => {}
                super::live::LiveRewinds::Off => count.enabled = false,
                super::live::LiveRewinds::Limit(n) => (count.infinite, count.rewinds_left) = (false, i32::from(n)),
            }
            info!("live: rewinds {:?} ({count:?})", rules.rewinds);
        }
        if let Some(d) = def.as_ref().filter(|d| {
            setup.live.is_none() && d.event_type.is_some() && !crate::render::post::env_is("SSX_GHOSTS", "off")
        }) {
            match ssx_physics::session::bots::BotData::load(&setup.big_root) {
                Ok(data) => {
                    let seed: Option<u32> = std::env::var("SSX_BOT_SEED").ok().and_then(|v| v.parse().ok());
                    let mut rng = ssx_assets::career::random::Source::seeded(seed);
                    let course = session.event.setup.course;
                    if let Some((race, problems)) =
                        ssx_physics::session::bots::BotRace::for_event(&setup.big_root, d, &data, &mut rng, course)
                    {
                        for p in problems {
                            warn!("bots: {p}");
                        }
                        info!("bots: {} for {} (GlobalRandom_Below, seed {seed:?})", race.bots.len(), d.name);
                        session.bots = Some(race);
                    }
                }
                Err(e) => warn!("bots unavailable: {e:#}"),
            }
        }
        let own = setup.own_tombstone.as_ref();
        if let Some(t) = def.as_ref().and_then(|d| ssx_physics::session::tombstones::Tombstones::for_event_with(d, own))
        {
            for r in &t.pool {
                info!("tombstone: {} {} m at {:?}", r.name, r.distance_m, r.position);
            }
            session.tombstones = Some(t);
        }
        if let Some(d) = def.as_ref().filter(|d| {
            d.event_type == Some(ssx_physics::event::EventType::SurviveToDeath)
                && ssx_physics::outro::outro_of_scripts(&d.scripts) == Some(ssx_physics::outro::OutroScript::HeliJump)
        }) {
            let (at, heading) = setup.spawn;
            let q = ssx_physics::spawn::level_start_orientation(&setup.world, at, heading);
            info!("survive-to-death loop: the heli jump's outro after the finish ({})", d.name);
            let later = intro_short.clone().or_else(|| intro.clone());
            session.survive = Some(ssx_physics::session::survive::SurviveLoop::new(later, None, at, q));
        }
        if let Some(d) = def.as_ref().filter(|d| {
            d.event_type == Some(ssx_physics::event::EventType::SurviveToDeath)
                && ssx_physics::outro::outro_of_scripts(&d.scripts) == Some(ssx_physics::outro::OutroScript::HeliPickup)
                && !crate::render::post::env_is("SSX_OUTRO", "off")
        }) {
            match ssx_physics::outro::prepare_heli_pickup(&setup.big_root, &d.name, &setup.character) {
                Ok(outro) => {
                    let (at, heading) = setup.spawn;
                    let q = ssx_physics::spawn::level_start_orientation(&setup.world, at, heading);
                    info!(
                        "survive-to-death loop: {} after the finish, then {:?}",
                        outro.nis.binding,
                        intro.as_ref().map(|i| &i.nis.binding)
                    );
                    let later = intro_short.clone().or_else(|| intro.clone());
                    session.survive = Some(ssx_physics::session::survive::SurviveLoop::new(later, Some(outro), at, q));
                }
                Err(e) => warn!("survive-to-death loop unavailable: {e:#}"),
            }
        }
        if let Some(d) = def.as_ref().filter(|d| {
            setup.free_ride
                && ssx_physics::outro::outro_of_scripts(&d.scripts) == Some(ssx_physics::outro::OutroScript::Standard)
                && !crate::render::post::env_is("SSX_OUTRO", "off")
        }) {
            match ssx_physics::outro::prepare_heli_pickup(&setup.big_root, &d.name, &setup.character) {
                Ok(outro) => {
                    let (at, heading) = setup.spawn;
                    let q = ssx_physics::spawn::level_start_orientation(&setup.world, at, heading);
                    info!("free ride: {} after the finish, then the level start again", outro.nis.binding);
                    let later = intro_short.clone().or_else(|| intro.clone());
                    session.survive = Some(ssx_physics::session::survive::SurviveLoop::new(later, Some(outro), at, q));
                }
                Err(e) => warn!("free ride: the heli pickup is unavailable: {e:#}"),
            }
        }
        if let Some(m) = setup.map.as_ref().filter(|_| !crate::render::post::env_is("SSX_OUTRO", "off"))
            && let Some(loop_) = super::map_intro::survive_loop(
                &setup.world,
                &setup.big_root,
                &setup.character,
                m,
                setup.spawn,
                &intro,
                &intro_short,
            )
        {
            session.survive = Some(loop_);
        }
        state.score.boost = setup.boost_meter.clamp(0.0, setup.world.tuning.scoring.max_boost);
        let pose = pose_of(&state);
        let clock = match setup.screenshot_steps {
            Some(n) => {
                let sub = std::env::var("SSX_RIDE_SUBFRAMES").ok().and_then(|v| v.parse().ok()).unwrap_or(1).max(1);
                Clock::PerFrame { started: false, remaining: n, sub, drawn: 0, phase: 0 }
            }
            None => Clock::Fixed,
        };
        let map_heli = setup.map.as_ref().and_then(|m| m.heli.as_ref());
        let heli_flight = match (&intro, setup.drop_key) {
            (Some(_), None) if map_heli.is_some() => {
                let grid = map_heli.and_then(|h| h.grid.clone());
                match ssx_physics::helicopter::HeliFlightData::with_grid(&setup.big_root, grid) {
                    Ok(f) => Some(std::sync::Arc::new(f)),
                    Err(e) => {
                        warn!("helicopter flight unavailable: {e:#}");
                        None
                    }
                }
            }
            (Some(_), Some(key)) => {
                let scripts =
                    ssx_physics::spawn::drop_objective_scripts(&setup.big_root, &setup.track, key).unwrap_or_default();
                let locator = ssx_physics::outro::outro_locator(&setup.world);
                match ssx_physics::helicopter::HeliFlightData::load(&setup.big_root, &setup.track, &scripts, locator) {
                    Ok(f) => Some(std::sync::Arc::new(f)),
                    Err(e) => {
                        warn!("helicopter flight unavailable: {e:#}");
                        None
                    }
                }
            }
            _ => None,
        };
        let mut heli = state
            .start
            .as_ref()
            .and_then(|l| l.intro.as_ref())
            .map(|i| ssx_physics::helicopter::Helicopter::with_flight(i, heli_flight.clone()));
        if !crate::render::post::env_is("SSX_GEOTAGS", "off") && setup.map.is_none() {
            match ssx_physics::session::geotags::GeotagData::load(&setup.big_root, &setup.track) {
                Ok(Some(data)) => {
                    let seed = crate::render::geotags::seed();
                    let mut rng = ssx_assets::career::random::Source::seeded(seed);
                    let collected = &setup.collected_geotags;
                    let run = ssx_physics::session::geotags::GeotagRun::new(
                        std::sync::Arc::new(data),
                        |key| collected.contains(&key),
                        &mut rng,
                    );
                    let ids: Vec<u32> = run.entities.iter().map(|e| e.record.row.id).collect();
                    info!("geotags: {} of {} rows placed (seed {seed:?}): {ids:?}", ids.len(), run.data.rows.len());
                    session.geotags = Some(run);
                }
                Ok(None) => info!("geotags: {} has none", setup.track),
                Err(e) => warn!("geotags unavailable: {e:#}"),
            }
        }
        if def.is_some()
            && let Some(key) = setup.drop_key
        {
            let scripts =
                ssx_physics::spawn::drop_objective_scripts(&setup.big_root, &setup.track, key).unwrap_or_default();
            session.tutorial = ssx_physics::session::tutorial::Tutorial::for_drop(&setup.big_root, &scripts);
            if session.tutorial.is_some() {
                info!("tutorial drop: {scripts:?}");
            }
        }
        if setup.game {
            session.set_ui_flag(ssx_physics::objectives::UI_HUD_READY, false);
        }
        let mut intro = intro;
        let heli_jump = match &setup.map {
            Some(_) => Err(anyhow::anyhow!("a map's ride")),
            None => ssx_physics::session::heli_jump::HeliJumpData::load(&setup.big_root, &setup.world.triggers),
        };
        if let Ok(data) = heli_jump {
            let data = std::sync::Arc::new(data);
            let outro_script = def.as_ref().and_then(|d| ssx_physics::outro::outro_of_scripts(&d.scripts))
                == Some(ssx_physics::outro::OutroScript::HeliJump);
            let name = def.as_ref().map_or("", |d| d.name.as_str());
            info!("heli jump: the track's objectives{}", if outro_script { ", outro_heli_jump.lua" } else { "" });
            session.heli_jump = Some(ssx_physics::session::heli_jump::HeliJump::new(
                data.clone(),
                outro_script,
                &setup.big_root,
                name,
                &setup.character,
            ));
            if let Some((k, before)) = std::env::var("SSX_RIDE_FROM_GHOST").ok().and_then(|v| {
                let (k, b) = v.split_once(',')?;
                Some((k.trim().parse::<usize>().ok()?, b.trim().parse::<f32>().ok()?))
            }) && let Some(row) = def.as_ref().and_then(|d| d.rivals.get(k))
                && let Ok(replay) = ssx_assets::ghosts::GhostReplay::load(&setup.big_root, row.ghost_id as u32)
            {
                let rail = data.rail_at_start();
                let contact = replay
                    .times
                    .iter()
                    .zip(&replay.positions)
                    .find(|(_, p)| {
                        Vec3::from_array(**p).distance(rail) < ssx_physics::session::heli_jump::CONTACT_DISTANCE
                    })
                    .map_or(replay.duration(), |(t, _)| *t);
                let from = contact - before;
                let i0 = replay.times.iter().position(|&t| t >= from).unwrap_or(replay.times.len() - 1);
                let p0 = Vec3::from_array(replay.positions[i0]);
                let v0 = Vec3::from_array(replay.pose(from).velocity);
                let heading = Vec3::new(v0.x, 0.0, v0.z).normalize_or(Vec3::Z);
                if let Some(mut s) = RiderState::spawn(&setup.world, p0 + Vec3::Y, heading) {
                    (s.velocity, s.traj_velocity, s.filtered_velocity) = (v0, v0, v0);
                    state = s;
                    intro = None;
                    session.event.on_trigger_race_start(&mut session.timer);
                    session.flow.race_proper_started = true;
                    if let Some(h) = heli.as_mut() {
                        h.traj = (data.heli_start.1, data.heli_start.0);
                        h.move_to = Some(data.heli_start);
                    }
                    session.update_objectives(&mut state, &setup.world, false, 0.0);
                    if let Some(o) = session.objectives.as_mut() {
                        (o.heli.move_to, o.heli.generation) = (Some(data.heli_start), o.heli.generation + 1);
                    }
                    info!(
                        "SSX_RIDE_FROM_GHOST: ghost {} at {from:.2} s ({before} s before its contact at {contact:.2} s): {p0}, \
                         {:.1} km/h",
                        row.ghost_id,
                        v0.length() * ssx_physics::KMH_PER_M_PER_TICK
                    );
                }
            }
        }
        let course = match std::env::var("SSX_COURSE").ok().filter(|_| setup.autoplay || env_flag("SSX_RIDE_AUTOPLAY"))
        {
            Some(v) => {
                let line = match (v.parse::<u32>(), setup.world.end_locator) {
                    (Ok(id), _) => ssx_assets::ghosts::GhostReplay::load(&setup.big_root, id)
                        .map(|r| (ssx_physics::harness::CourseLine::from_replay(&r), f32::NAN, f32::NAN)),
                    (Err(_), Some(end)) => {
                        ssx_physics::harness::CourseLine::nearest_disc(&setup.big_root, setup.spawn.0, end)
                    }
                    (Err(_), None) => Err(anyhow::anyhow!("no end locator")),
                };
                match line {
                    Ok((line, ds, de)) => {
                        info!(
                            "course: replay {} ({} samples, {:.0} m; {ds:.0} m from the level start, {de:.0} m from the \
                             end locator, wingsuit on {} samples)",
                            line.id,
                            line.points.len(),
                            line.along.last().copied().unwrap_or(0.0),
                            line.wingsuit.iter().filter(|&&w| w).count()
                        );
                        Some(ssx_physics::harness::CoursePlayer::new(line))
                    }
                    Err(e) => {
                        warn!("course: unavailable ({e:#}); the scripted player rides");
                        None
                    }
                }
            }
            None => None,
        };
        Some(Self {
            world: std::sync::Arc::new(setup.world),
            spawn: setup.spawn,
            intro,
            intro_short,
            big_root: setup.big_root,
            sim_content_id: setup.sim_content_id,
            map: setup.map,
            intro_graph: false,
            cabin_seen: None,
            world_hidden: false,
            heli,
            heli_flight,
            vista: None,
            event_def: def,
            character: setup.character,
            wire_key: setup.wire_key,
            outro: None,
            state,
            prev: pose,
            curr: pose,
            step_ticks: DT_TICKS,
            render_lag: 0.0,
            render_steps: 0,
            render: pose,
            render_time: 0.0,
            steps: 0,
            audio_tag_log: Default::default(),
            distance: 0.0,
            input: setup.fixed_input.unwrap_or_default(),
            tricks: TrickInput::default(),
            fixed_input: setup.fixed_input,
            jump_release: setup.jump_release,
            air_input: setup.air_input,
            grab: setup.grab,
            tweak: setup.tweak,
            shoulders: setup.shoulders,
            boost: setup.boost,
            boost_meter: setup.boost_meter,
            r2: 0.0,
            clock,
            report: false,
            step_ms: 0.0,
            session,
            rewind_pad: RewindPad::default(),
            rewinding: false,
            auto_rewind: AutoRewind::default(),
            autoplay_waited: 0,
            autoplay_sat_out: false,
            numpad_held: Vec::new(),
            key_rewind: Default::default(),
            key_rewind_state: ssx_physics::rewind::RewindState::Idle,
            key_holds: Default::default(),
            course,
            course_logged: 0,
            course_start_at: 0.0,
            flare_pushes: 0,
            l2: false,
            l2_hold: false,
            grind_fixed: env_flag("SSX_RIDE_GRIND"),
            rail_debug: env_flag("SSX_RIDE_RAIL_DEBUG"),
            last_grind_capture: 0.0,
            rewind_backwards: false,
            last_exec: None,
            press_script: press_script(),
            last_press_capture: 0.0,
            motion_blur_cue: 0,
            r1: false,
            wingsuit_script: wingsuit_script(),
            ws_pressed: None,
            ws_deployed: None,
            hold: setup.game,
            paused: false,
            back_held: false,
            back_was: false,
            back_press: false,
            autoplay: setup.autoplay || env_flag("SSX_RIDE_AUTOPLAY"),
            classic: setup.classic,
            viewer_keys: !setup.game,
            a_lock: PadLock::default(),
            grind_since: None,
            grind_ended: None,
            messages_logged: 0.0,
            loadout,
            game_setup,
            live: setup.live,
            pads_held: false,
            chat_held: false,
            pose_blocks: setup.live.map(|_| ssx_physics::graph::LiveBlocks::default()),
            start_gate: super::live::StartGate::Free,
            go_step: None,
        })
    }
}

fn live_rivals(
    big_root: &std::path::Path,
    def: Option<&ssx_physics::event::EventDef>,
    rules: super::live::LiveRules,
    course: Option<Course>,
) -> ssx_physics::session::bots::BotRace {
    use ssx_physics::event::EventType;
    use ssx_physics::session::bots::{BotData, BotRace, explore_requests};
    let empty = || BotRace { course, ..BotRace::default() };
    let Some(d) = def.filter(|d| matches!(d.event_type, Some(EventType::RaceGhosts | EventType::TrickGhosts))) else {
        info!("live: no rivals (the event has no explore ghosts)");
        return empty();
    };
    if !rules.explore_ghosts {
        info!("live: the event's explore ghosts are off (the server's `ghosts = off`)");
        return empty();
    }
    match BotData::load(big_root) {
        Ok(data) => {
            let (race, problems) = BotRace::load_requests(big_root, explore_requests(d), Vec::new(), &data, course);
            for p in problems {
                warn!("live: explore ghosts: {p}");
            }
            info!("live: {} explore ghosts of {} kept (the server's `ghosts = on`)", race.bots.len(), d.name);
            race
        }
        Err(e) => {
            warn!("live: the explore ghosts unavailable: {e:#}");
            empty()
        }
    }
}

fn required_value(name: &str) -> Option<u32> {
    let names = ["wingsuit", "iceaxe", "oxygen", "thermal", "armour", "headlamp", "goggles"];
    names.iter().position(|n| n.eq_ignore_ascii_case(name)).map(|i| i as u32)
}

pub(super) fn level_start(
    world: &PhysicsWorld,
    (at, heading): (Vec3, Vec3),
    intro: Option<&std::sync::Arc<ssx_physics::nis::IntroAssets>>,
) -> Option<RiderState> {
    let q = ssx_physics::spawn::level_start_orientation(world, at, heading);
    match intro {
        Some(i) => ssx_physics::nis::level_start_or_below(world, at, q, i.clone()),
        None => RiderState::level_start(world, at, q),
    }
}
