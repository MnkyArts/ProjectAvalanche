use bevy::prelude::*;
use ssx_physics::DT_TICKS;
use ssx_physics::event::{Course, EventManager, EventSetup, level_event};
use ssx_physics::rewind::{LiveRewindRule, RewindPad};
use ssx_physics::rider::{RiderState, TrickInput, World as PhysicsWorld};
use ssx_physics::session::Session;

use super::clock::pose_of;
use super::controls::PadLock;
use super::scripted::{AutoRewind, env_flag, press_script, wingsuit_script};
use super::{Clock, Ride, RideSetup};

impl Ride {
    pub(super) fn new(mut setup: RideSetup) -> Option<Self> {
        let def = match setup.event_key {
            _ if setup.map.is_some() => None,
            key => {
                let def = level_event(&setup.world.events, key, super::viewer_event_choice);
                if let (Some(key), None) = (key, def) {
                    warn!("ride: the launched event {key:#010x} is not on the drop: the ride has no event");
                }
                def.cloned()
            }
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
        setup.world.survival.goggles_preset = goggles_preset(&setup.big_root, &setup.character, &loadout);
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
                let first = prepare(path).map(|i| read_ahead(i, &setup.big_root));
                let short = match path {
                    IntroPath::Short => None,
                    _ => prepare(IntroPath::Short).map(|i| read_ahead(i, &setup.big_root)),
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
        let workers: std::sync::Arc<dyn ssx_physics::workers::Workers> =
            std::sync::Arc::new(super::workers::PoolWorkers);
        let mut session = Session { event, workers: Some(workers), ..Session::default() };
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
            let m = &mut session.manager;
            match rules.rewinds {
                super::live::LiveRewinds::Event => {}
                super::live::LiveRewinds::Off => m.set_live_rule(LiveRewindRule::Off),
                super::live::LiveRewinds::Limit(n) => m.set_live_rule(LiveRewindRule::Limit(i32::from(n))),
            }
            info!("live: rewinds {:?} ({:?}, rule {:?})", rules.rewinds, m.count, m.live_rule);
        }
        if let Some(d) = def.as_ref().filter(|d| {
            setup.live.is_none() && d.event_type.is_some() && !crate::render::post::env_is("SSX_GHOSTS", "off")
        }) {
            match ssx_physics::session::bots::BotData::load(&setup.big_root) {
                Ok(data) => {
                    let seed: Option<u32> = std::env::var("SSX_BOT_SEED").ok().and_then(|v| v.parse().ok());
                    let mut rng = ssx_assets::career::random::Source::seeded(seed);
                    let course = session.event.setup.course;
                    let free_ride = session.event.setup.free_ride;
                    match ssx_physics::session::bots::BotRace::for_event(
                        &setup.big_root,
                        d,
                        &data,
                        &mut rng,
                        course,
                        free_ride,
                    ) {
                        Some((race, problems)) => {
                            for p in problems {
                                warn!("bots: {p}");
                            }
                            info!("bots: {} for {} (GlobalRandom_Below, seed {seed:?})", race.bots.len(), d.name);
                            session.bots = Some(race);
                        }
                        None if free_ride => info!("bots: none for {} (the title update's Freeride)", d.name),
                        None => {}
                    }
                }
                Err(e) => warn!("bots unavailable: {e:#}"),
            }
        }
        if let Some((slot, replay)) = setup.own_ghost.take()
            && let Some(race) = session.bots.as_mut()
            && !setup.free_ride
            && def.as_ref().and_then(|d| d.event_type).is_some_and(|t| {
                matches!(t, ssx_physics::event::EventType::RaceGhosts | ssx_physics::event::EventType::TrickGhosts)
            })
        {
            info!(
                "own ghost: slot {slot}, {} samples over {:.2} s (gear {:#x}, score {})",
                replay.times.len(),
                replay.duration(),
                replay.header.gear_flags,
                replay.header.score
            );
            race.add_own_ghost(ssx_physics::session::bots::own_ghost_request(slot), replay);
        }
        if let Some(b) = session.bots.as_mut() {
            b.explore_mode = explore_game_mode(setup.game_mode, def.as_ref());
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
                let grid = map_heli.and_then(|h| h.grid.as_ref().ok().cloned());
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
                    Ok(f) => {
                        info!(
                            "helicopter flight: height grid {}",
                            f.grid.as_ref().map_or("none (no elevation, no avoidance)".to_string(), |g| format!(
                                "{} × {} cells over {:.0} … {:.0}",
                                g.width, g.height, g.min, g.max
                            ))
                        );
                        Some(std::sync::Arc::new(f))
                    }
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
            if let Some(s) =
                ride_from_ghost(&setup.world, def.as_ref(), &setup.big_root, &data, &mut session, heli.as_mut(), None)
            {
                state = s;
                intro = None;
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
        let stream_start = stream_start(&setup.world, setup.drop_key, setup.spawn.0);
        Some(Self {
            world: std::sync::Arc::new(setup.world),
            scripts_task: None,
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
            steer_from: setup.steer_from,
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
            r3: false,
            wingsuit_script: wingsuit_script(),
            ws_pressed: None,
            ws_deployed: None,
            oxygen_held: false,
            hold: setup.game,
            paused: false,
            back_held: false,
            back_was: false,
            back_press: false,
            autoplay: setup.autoplay || env_flag("SSX_RIDE_AUTOPLAY"),
            classic: setup.classic,
            viewer_keys: !setup.game,
            a_lock: PadLock::default(),
            latch: super::latch::PadLatch::default(),
            flush_latch: false,
            step_read: false,
            grind_since: None,
            grind_ended: None,
            messages_logged: 0.0,
            loadout,
            game_setup,
            live: setup.live,
            pads_held: false,
            chat_held: false,
            pose_blocks: setup.live.map(|_| ssx_physics::graph::LiveBlocks::default()),
            ghost_rumble: ssx_physics::graph::LandingRumble::default(),
            rumble: super::rumble::RumblePost::default(),
            start_gate: super::live::StartGate::Free,
            go_step: None,
            stream_start,
            stream_from_start: true,
        })
    }
}

pub(super) fn ride_from_ghost(
    world: &PhysicsWorld,
    def: Option<&ssx_physics::event::EventDef>,
    big_root: &std::path::Path,
    data: &ssx_physics::session::heli_jump::HeliJumpData,
    session: &mut Session,
    heli: Option<&mut ssx_physics::helicopter::Helicopter>,
    before: Option<f32>,
) -> Option<RiderState> {
    if let Some((k, before, rivals_of)) = std::env::var("SSX_RIDE_FROM_GHOST").ok().and_then(|v| {
        let mut f = v.split(',').map(str::trim);
        let (k, b) = (f.next()?.parse::<usize>().ok()?, f.next()?.parse::<f32>().ok()?);
        Some((k, before.unwrap_or(b), f.next().map(str::to_string)))
    }) && let Some(row) = match rivals_of {
        Some(n) => world.events.iter().find(|d| d.name == n),
        None => def,
    }
    .and_then(|d| d.rivals.get(k))
        && let Ok(replay) = ssx_assets::ghosts::GhostReplay::load(big_root, row.ghost_id as u32)
    {
        let rail = data.rail_at_start();
        let contact = replay
            .times
            .iter()
            .zip(&replay.positions)
            .find(|(_, p)| Vec3::from_array(**p).distance(rail) < ssx_physics::session::heli_jump::CONTACT_DISTANCE)
            .map_or(replay.duration(), |(t, _)| *t);
        let from = contact - before;
        let i0 = replay.times.iter().position(|&t| t >= from).unwrap_or(replay.times.len().checked_sub(1)?);
        let p0 = Vec3::from_array(*replay.positions.get(i0)?);
        let v0 = Vec3::from_array(replay.pose(from).velocity);
        let heading = Vec3::new(v0.x, 0.0, v0.z).normalize_or(Vec3::Z);
        if let Some(mut s) = RiderState::spawn(world, p0 + Vec3::Y, heading) {
            (s.velocity, s.traj_velocity, s.filtered_velocity) = (v0, v0, v0);
            session.event.on_trigger_race_start(&mut session.timer);
            session.flow.race_proper_started = true;
            if let Some(h) = heli {
                h.traj = (data.heli_start.1, data.heli_start.0);
                h.move_to = Some(data.heli_start);
            }
            session.update_objectives(&mut s, world, false, 0.0);
            if let Some(o) = session.objectives.as_mut() {
                (o.heli.move_to, o.heli.generation) = (Some(data.heli_start), o.heli.generation + 1);
            }
            info!(
                "SSX_RIDE_FROM_GHOST: ghost {} at {from:.2} s ({before} s before its contact at {contact:.2} s): {p0}, \
                 {:.1} km/h",
                row.ghost_id,
                v0.length() * ssx_physics::KMH_PER_M_PER_TICK
            );
            return Some(s);
        }
    }
    None
}

pub(super) fn loop_ghost_start(loops: u32) -> Option<f32> {
    let v = std::env::var("SSX_RIDE_FROM_GHOST_LOOPS").ok()?;
    let list: Vec<&str> = v.split(',').map(str::trim).collect();
    let k = usize::try_from(loops).ok()?.checked_sub(1)?;
    list.get(k).or(list.last())?.parse().ok()
}

// PORT(choice): the level's intro and the short path a restart takes read their NIS bundles while the level loads, off
// the game thread: the ride's first step read them (`AssetDb::load` in `IntroGraph::load`) on the game thread when the
// loading screen went (part of a 73 ms step at alps0101's start, development laptop, `trace_chrome`), the first step
// after a restart read the short path's and built its graph (60 ms of an 83 ms frame on alps0101), and the intro's
// audio read them again. The bundles and what is built from them are the same; the console's loader threads are not
// decoded.
fn read_ahead(
    intro: std::sync::Arc<ssx_physics::nis::IntroAssets>,
    big_root: &std::path::Path,
) -> std::sync::Arc<ssx_physics::nis::IntroAssets> {
    let mut assets = match std::sync::Arc::try_unwrap(intro) {
        Ok(a) => a,
        Err(shared) => return shared,
    };
    if assets.loaded.is_none() {
        let t0 = std::time::Instant::now();
        let bundles: Vec<&str> = assets.nis.bundles.iter().map(String::as_str).collect();
        match ssx_assets::anim_db::AssetDb::load(big_root, &bundles) {
            Ok(db) => {
                info!("intro: its {} NIS bundle(s) read ahead in {:.3} s", bundles.len(), t0.elapsed().as_secs_f32());
                assets.loaded = Some(std::sync::Arc::new(db));
            }
            Err(e) => warn!("intro: its NIS bundles not read ahead ({e:#}): the ride's first step reads them"),
        }
    }
    std::sync::Arc::new(assets)
}

fn explore_game_mode(game_mode: Option<i64>, def: Option<&ssx_physics::event::EventDef>) -> bool {
    match game_mode {
        Some(m) => m == 0,
        // PORT(choice): without a front end (the viewer's ride) the game mode comes from the event's name (`exp_`
        // Explore), as `ssx_ui::game::ride`'s results take it; in the game the front end's flow manager holds it.
        None => def.is_some_and(|d| d.name.starts_with("exp_")),
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

fn goggles_preset(
    big_root: &std::path::Path,
    character: &str,
    loadout: &ssx_assets::character::Loadout,
) -> Option<(u32, ssx_assets::postfx_state::PostFxState)> {
    use ssx_assets::character::{GearItem, gear_postfx_key};
    use ssx_assets::postfx_state::PostFxState;
    let key = match gear_postfx_key(big_root, character, loadout, GearItem::PulseGoggles) {
        Ok(key) => key?,
        Err(e) => {
            warn!("goggles: the item's post-FX preset: {e:#}");
            return None;
        }
    };
    let preset = match ssx_assets::postfx::load_preset_routed(big_root, key) {
        Ok(p) => p,
        Err(e) => {
            warn!("goggles: post-FX preset {key:#010x}: {e:#}");
            return None;
        }
    };
    let refused = PostFxState::refused(&preset);
    if !refused.is_empty() {
        warn!("goggles: preset {} ({:?}): not in the state: {}", preset.name, preset.chain, refused.join(", "));
    }
    info!("goggles: post-FX preset {} ({:?})", preset.name, preset.chain);
    Some((key, PostFxState::of(&preset)))
}

fn stream_start(world: &PhysicsWorld, drop_key: Option<u32>, spawn: Vec3) -> Vec3 {
    match drop_key {
        Some(key) => {
            world.starts.iter().find(|s| s.drop_key == key).and_then(|s| s.slot(0)).map_or(Vec3::ZERO, |s| s.0)
        }
        // PORT(choice): a ride without a drop (a map's track, a test ride) streams from its spawn; the game always
        // rides a drop.
        None => spawn,
    }
}
