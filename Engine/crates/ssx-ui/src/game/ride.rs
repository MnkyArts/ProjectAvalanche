use std::path::Path;

use anyhow::{Context, Result};
use ssx_physics::character::DEFAULT_GEAR_MIX;
use ssx_physics::event::{Course, EventDef, EventManager, EventResult, EventSetup, choose_event};
use ssx_physics::graph::RiderGraph;
use ssx_physics::rewind::RewindPad;
use ssx_physics::rider::{Input, RideState, RiderState, TrickInput, World};
use ssx_physics::session::{Execution, Session};
use ssx_physics::spawn::{level_start_pose, load_drop_starts, track_world_for_drop};
use ssx_physics::{SIM_HZ, Vec2, Vec3};

use super::GameUi;
use super::feed::HudFeed;

mod map;
mod rows;

pub use map::{MapRun, post_map_results};
pub use rows::{DropRivals, RivalGhosts, board_rivals, drop_rivals, target_rows};

#[derive(Clone, Debug)]
pub struct RideOptions {
    pub track: String,
    pub drop: usize,
    pub event: String,
    pub character: String,
    pub jump_every: f32,
    pub spin: f32,
    pub grab: Option<Vec2>,
    pub intro: bool,
    pub setup: Option<ssx_physics::gear::GameSetup>,
}

pub fn game_setup(s: &super::fe::setup::GameSetup) -> ssx_physics::gear::GameSetup {
    ssx_physics::gear::GameSetup {
        character: s.character,
        items: s.gear.map(|g| ssx_physics::gear::GearItem { stats: g.stats, slot: g.slot, description: g.description }),
    }
}

impl Default for RideOptions {
    fn default() -> Self {
        RideOptions {
            track: "alps0101".into(),
            drop: 0,
            event: "race".into(),
            character: "alex".into(),
            jump_every: 4.0,
            spin: 1.0,
            grab: Some(Vec2::new(0.0, 1.0)),
            intro: false,
            setup: None,
        }
    }
}

pub struct RideDriver {
    pub world: World,
    pub rider: RiderState,
    pub session: Session,
    graph: Option<RiderGraph>,
    pub feed: HudFeed,
    pub opts: RideOptions,
    pub step_index: usize,
    pub drop_key: u32,
    pub event_type: i32,
    pub event_name: String,
    pub results: Option<(f32, EventResult)>,
    pub rivals: Vec<(String, u32)>,
    pub ghosts: RivalGhosts,
    pub rules: Option<ssx_assets::career::Rules>,
}

const START_PUSH_S: f32 = 2.0;

#[derive(Clone, Debug, Default)]
pub struct ResultsContext {
    pub drop_key: u32,
    pub event_type: i32,
    pub event_name: String,
    pub character: String,
    pub track_bonuses: Vec<String>,
    pub bot_scores: Vec<u32>,
    pub event_id: Option<ssx_content::ContentId>,
    pub sim_content_id: Option<String>,
}

pub fn bot_scores(_def: &EventDef, bots: Option<&ssx_physics::session::bots::BotRace>) -> Vec<u32> {
    bots.map_or_else(Vec::new, |race| race.scores.clone())
}

pub fn career_results(
    ctx: &ResultsContext,
    r: &EventResult,
    profile: &mut ssx_assets::career::Profile,
    rules: &ssx_assets::career::Rules,
) -> super::hud::results::CareerResults {
    career_results_with_splits(ctx, r, [0; 4], profile, rules)
}

pub fn career_results_with_splits(
    ctx: &ResultsContext,
    r: &EventResult,
    splits: [u32; 4],
    profile: &mut ssx_assets::career::Profile,
    rules: &ssx_assets::career::Rules,
) -> super::hud::results::CareerResults {
    use ssx_assets::career::profile::{EventOutcome, update_rider_level};
    let event = ctx.drop_key;
    let rider = rules.riders.get(&ctx.character).copied().unwrap_or(0);
    let first_run = !profile.explore_scores.contains_key(&event);
    let money_previous = profile.credits();
    let mut stats = profile.riders.get(&rider).cloned().unwrap_or_default();
    let level_before = update_rider_level(&mut stats, &rules.level_thresholds, &rules.level_prices);
    let outcome = EventOutcome {
        event,
        rider,
        failed: r.failed,
        value: r.value,
        leaderboard: r.rivals.clone(),
        rewinds: r.rewinds,
        place: r.place,
        geotag_credits: r.geotag_credits,
        track_bonuses: ctx.track_bonuses.clone(),
        bot_scores: ctx.bot_scores.clone(),
        splits,
        event_id: ctx.event_id.clone(),
        sim_content_id: ctx.sim_content_id.clone(),
        ..Default::default()
    };
    let report = profile.apply_result(rules, &outcome);
    let bonuses = super::hud::results::bonus_lines(
        first_run,
        &rules.paid_track_bonuses(&ctx.track_bonuses),
        rules.track_bonuses.is_empty(),
        r.failed,
        r.geotag_credits,
        r.geotag_count,
    );
    debug_assert_eq!(bonuses.iter().map(|b| b.credits).sum::<i64>(), report.bonus, "the lines sum to the bonus");
    super::hud::results::CareerResults {
        reward: report.reward,
        bonuses,
        money_previous,
        total: report.total,
        level_before,
        level_after: report.level,
        success: report.success,
        medal: report.medal,
        notices: report.notices,
        best_not_kept: report.best_not_kept,
    }
}

pub fn results_portrait(
    career: &ssx_assets::career::CareerDb,
    rules: &ssx_assets::career::Rules,
    character: &str,
) -> String {
    rules
        .riders
        .get(character)
        .and_then(|key| career.riders.iter().find(|r| r.key == *key))
        .map_or_else(String::new, |r| r.icon.clone())
}

pub fn post_event_results(
    ui: &mut GameUi,
    ctx: &ResultsContext,
    r: &EventResult,
    profile: &mut ssx_assets::career::Profile,
    rules: &ssx_assets::career::Rules,
) -> Posted {
    post_event_results_with_splits(ui, ctx, r, [0; 4], profile, rules)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Posted {
    pub won: bool,
    pub best_not_kept: Option<ssx_assets::career::profile::BestNotKept>,
    pub medal: Option<ssx_assets::career::profile::Medal>,
}

pub fn own_tombstone(
    profile: &ssx_assets::career::Profile,
    drop: u32,
    gamertag: &str,
) -> Option<ssx_physics::session::tombstones::OwnRecord> {
    let row = profile.explore_scores.get(&drop)?;
    ssx_physics::session::tombstones::OwnRecord::from_row(row.score, row.splits, gamertag)
}

pub fn own_tombstone_for(ui: &GameUi, drop: u32) -> Option<ssx_physics::session::tombstones::OwnRecord> {
    own_tombstone(&ui.fe.as_ref()?.profile, drop, ui.values.str(0x22B))
}

pub fn post_event_results_with_splits(
    ui: &mut GameUi,
    ctx: &ResultsContext,
    r: &EventResult,
    splits: [u32; 4],
    profile: &mut ssx_assets::career::Profile,
    rules: &ssx_assets::career::Rules,
) -> Posted {
    let career = career_results_with_splits(ctx, r, splits, profile, rules);
    let medal = career.medal.map_or(-1, |m| m as i32);
    // PORT(choice): without a front end the game mode (0x825775C0) comes from the event's name (`exp_` 0, `chapter_` 1,
    // else 3), with one from the launch; in the game the front end's is the game mode.
    let game_mode = if let Some(f) = ui.fe.as_ref() {
        f.game_mode as i32
    } else if ctx.event_name.starts_with("exp_") {
        0
    } else if ctx.event_name.starts_with("chapter_") {
        1
    } else {
        3
    };
    let input = super::hud::results::ResultsInput {
        event_type: ctx.event_type,
        game_mode,
        points: r.points,
        time_ms: r.time_ms,
        descent_mm: r.descent_mm,
        rewinds: r.rewinds,
        failed: r.failed,
        rank: r.rank,
        medal,
        career,
        tutorial: super::fe::globe::TUTORIAL_DROPS.contains(&ctx.drop_key),
        character_pic: ui.fe.as_ref().map_or_else(String::new, |f| results_portrait(&f.career, rules, &ctx.character)),
    };
    let posted =
        Posted { won: input.career.success, best_not_kept: input.career.best_not_kept, medal: input.career.medal };
    if !r.failed && r.geotag_credits > 0 {
        ui.values.set_int(0x228, (input.career.money_previous - r.geotag_credits) as i32);
    }
    let bank_adjust = ui.values.int(0x93);
    ui.values.set_int(0x93, bank_adjust + input.career.total as i32);
    if let Some(f) = ui.fe.as_mut() {
        for n in &input.career.notices {
            let items = super::fe::unlocks::items_of(n, &f.career, &mut ui.loc);
            f.unlocks.extend(items);
        }
        if let Some(why) = posted.best_not_kept {
            super::fe::notify::post_best_not_kept(f, &mut ui.loc, why, &input.character_pic);
        }
    }
    super::hud::results::post_results(&mut ui.values, &mut ui.loc, &input);
    posted
}

pub fn post_death_body(ui: &mut GameUi, course: Option<ssx_physics::event::Course>, r: &EventResult) {
    let course_mm = course.map_or(0, |c| ((c.start_y - c.end_y).abs() * 1000.0) as i64);
    super::hud::results::death_body(&mut ui.values, &mut ui.loc, course_mm, r.descent_mm, ui.free_ride);
}

impl RideDriver {
    fn results_context(&self) -> ResultsContext {
        ResultsContext {
            drop_key: self.drop_key,
            event_type: self.event_type,
            event_name: self.event_name.clone(),
            character: self.opts.character.clone(),
            track_bonuses: self.session.stats.track_bonuses(),
            bot_scores: self.session.bots.as_ref().map_or_else(Vec::new, |r| r.scores.clone()),
            event_id: ssx_assets::content::data::events::event_id(&self.event_name),
            sim_content_id: None,
        }
    }

    pub fn post_results(&self, ui: &mut GameUi, r: &EventResult) {
        let mut profile = ssx_assets::career::Profile::new();
        let _ = self.post_results_with(ui, r, &mut profile);
    }

    pub fn post_results_with(
        &self,
        ui: &mut GameUi,
        r: &EventResult,
        profile: &mut ssx_assets::career::Profile,
    ) -> bool {
        let rules = self.rules.clone().unwrap_or_default();
        let splits = self.session.event.explore_splits().unwrap_or_default();
        let won = post_event_results_with_splits(ui, &self.results_context(), r, splits, profile, &rules).won;
        if r.failed {
            post_death_body(ui, self.session.event.setup.course, r);
        }
        won
    }

    pub fn load(big_root: &Path, opts: RideOptions) -> Result<Self> {
        let track_dir = big_root.join("data0/data/xenon/gameexplorer").join(&opts.track);
        let mut world = track_world_for_drop(big_root, &track_dir, &opts.track, DEFAULT_GEAR_MIX, true, opts.drop)?;
        let (start_pos, start_q) = level_start_pose(&world, opts.drop, None, None)?;
        let start_y = world.starts[opts.drop].slot(0).map_or(start_pos.y, |(p, _)| p.y);
        let course = world.end_locator.map(|e| Course { start_y, end_y: e.y });
        let def = if opts.event == "none" { None } else { choose_event(world.events.iter(), &opts.event).cloned() };
        world.survival.apply_event(def.as_ref());
        let gear = match opts.setup.as_ref().filter(|s| s.character != 0) {
            Some(s) => s.gear_stats(),
            None => ssx_physics::survival::event_loadout(&world.survival),
        };
        world.equip(&gear)?;
        let intro = match (&def, opts.intro) {
            (Some(d), true) => ssx_physics::nis::prepare(&mut world, big_root, d, &opts.character)?,
            _ => None,
        };
        let rider = match intro {
            Some(i) => match ssx_physics::nis::level_start(&world, start_pos, start_q, i.clone()) {
                Some(s) => Some(s),
                None => world.collision.ray_cast(start_pos, -Vec3::Y, 20_000.0).and_then(|hit| {
                    ssx_physics::nis::level_start_on(&world, hit.point + Vec3::Y, start_pos, start_q, i)
                }),
            },
            None => RiderState::level_start(&world, start_pos, start_q),
        }
        .context("no ground below the level start")?;
        let graph = RiderGraph::load(big_root, &opts.track, &opts.character).ok();
        let mut session =
            Session { event: EventManager::new(EventSetup::new(def.as_ref(), course)), ..Session::default() };
        if def.is_some()
            && let Some(start) = world.starts.get(opts.drop)
            && let Ok(scripts) = ssx_physics::spawn::drop_objective_scripts(big_root, &opts.track, start.drop_key)
        {
            session.tutorial = ssx_physics::session::tutorial::Tutorial::for_drop(big_root, &scripts);
        }
        if let Some(d) = def.as_ref()
            && let Ok(data) = ssx_physics::session::bots::BotData::load(big_root)
        {
            let mut rng = ssx_assets::career::random::Mt19937::seeded(ssx_physics::session::bots::DEFAULT_BOT_SEED);
            let course = session.event.setup.course;
            session.bots =
                ssx_physics::session::bots::BotRace::for_event(big_root, d, &data, &mut rng, course).map(|(r, _)| r);
        }
        session.tombstones = def.as_ref().and_then(ssx_physics::session::tombstones::Tombstones::for_event);
        let drop_key = def.as_ref().map(|d| d.key).unwrap_or_else(|| {
            load_drop_starts(big_root, &opts.track).ok().and_then(|s| s.get(opts.drop).map(|d| d.drop_key)).unwrap_or(0)
        });
        let event_type = def.as_ref().and_then(|d| d.event_type).map_or(-1, |t| t as i32);
        let event_name = def.as_ref().map(|d| d.name.clone()).unwrap_or_default();
        let DropRivals { rivals, replays, rider_titles, problems } = drop_rivals(big_root, def.as_ref());
        for p in problems {
            ssx_core::eprintln!("ride: {p}");
        }
        let career = ssx_assets::career::CareerDb::load(big_root).ok();
        let mut feed = HudFeed::new(&opts.character);
        feed.explore.rivals = rivals.clone();
        feed.rider_titles = rider_titles;
        Ok(RideDriver {
            world,
            rider,
            session,
            graph,
            feed,
            opts,
            step_index: 0,
            drop_key,
            event_type,
            event_name,
            results: None,
            rivals,
            ghosts: RivalGhosts::new(replays),
            rules: career.map(|c| {
                let mut r = c.rules();
                if let Err(e) = r.load_badge_texts(big_root) {
                    ssx_core::eprintln!("ride: the badges' texts: {e:#}");
                }
                r
            }),
        })
    }

    pub fn time(&self) -> f32 {
        self.step_index as f32 / SIM_HZ
    }

    pub fn step(&mut self, ui: &mut GameUi) -> Execution {
        let i = self.step_index;
        let time = self.time();
        let s = &self.rider;
        let (mut jump, mut release) = (false, false);
        if self.opts.jump_every > 0.0 {
            let every = ((self.opts.jump_every * SIM_HZ).round() as usize).max(2);
            let hold = ((0.3 * SIM_HZ).round() as usize).clamp(1, every - 1);
            jump = i + hold >= every && i % every >= every - hold;
            release = i >= every && i.is_multiple_of(every);
        }
        let waiting = s.stopped || s.start.as_ref().is_some_and(|l| l.idle());
        let pushing = waiting && (START_PUSH_S..START_PUSH_S + 1.0).contains(&time);
        let get_up = s.stopped && s.landing.bail_on_landing;
        let flick = if i.is_multiple_of(2) { 1.0 } else { 0.0 };
        let (steer, tuck) = if pushing || get_up { (0.0, flick) } else { (0.0, 0.0) };
        let mut input = Input { steer, brake: 0.0, tuck, jump, ..Input::default() };
        let mut tricks = TrickInput::default();
        if s.state != RideState::Riding || release {
            (input.steer, input.tuck, input.brake) = (self.opts.spin, 0.0, 0.0);
            if s.tricks.classic {
                input.dpad = Vec2::new(self.opts.spin, 0.0);
                input.dpad_held = self.opts.spin != 0.0;
            }
        }
        if s.state != RideState::Riding && s.air.air_time < 0.5 * 60.0 {
            tricks.right_stick = self.opts.grab.unwrap_or(Vec2::ZERO);
        }
        let e = self.session.execute(
            &mut self.rider,
            &self.world,
            &input,
            &tricks,
            0.0,
            &RewindPad::default(),
            self.graph.as_mut(),
        );
        self.step_index += 1;
        if e.event.results
            && let Some(r) = self.session.event.result.clone()
        {
            match ui.fe.as_mut().map(|f| std::mem::replace(&mut f.profile, ssx_assets::career::Profile::new())) {
                Some(mut profile) => {
                    use ssx_assets::career::profile::DropState;
                    let drop = ui.fe.as_ref().map_or(0, |f| f.current_drop);
                    let before = profile.drop_state(drop);
                    let success = self.post_results_with(ui, &r, &mut profile);
                    if before == DropState::Unlocked && profile.drop_state(drop) == DropState::Completed {
                        super::fe::flow::campaign_drop_completed(&mut ui.values, drop);
                    }
                    if let Some(f) = ui.fe.as_mut() {
                        f.profile = profile;
                        f.last_success = Some(success);
                        f.last_result_drop = drop;
                    }
                }
                None => self.post_results(ui, &r),
            }
            self.results = Some((self.time(), r));
        }
        let (ghosts, racers) = self.ghosts.step(
            self.session.event.clock.seconds,
            self.session.event.clock.ms(),
            &self.world.triggers,
            &mut ui.loc,
        );
        let explore = ssx_physics::event::shows_explore_ghosts(self.session.event.setup.event_type);
        self.feed.ghosts = if explore { ghosts } else { Vec::new() };
        self.feed.ghost_racers = if explore { racers } else { Vec::new() };
        self.feed.feed(ui, &self.rider, &self.session, &self.world, Some(&e));
        let events = self.session.tutorial.as_mut().map_or_else(Vec::new, |t| t.take_events());
        let rider = self.feed.rider.clone();
        ui.tutorial_step(&self.rider, &self.session, &events, &rider);
        if std::env::var_os("SSX_RIDE_TRACE").is_some() && i.is_multiple_of(30) {
            let s = &self.rider;
            ssx_core::eprintln!(
                "{time:6.1} {:?} stopped {} speed {:.1} m/s traj {:.1} bail {} start {}",
                s.state,
                s.stopped,
                s.player_speed,
                s.traj,
                s.landing.bail_on_landing,
                s.start.is_some()
            );
        }
        e
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_front_ends_setup_is_the_rides_gear() {
        let mut s = super::super::fe::setup::GameSetup { character: 0x405D_C1D5, ..Default::default() };
        s.gear[0].set_slot(0);
        for (stat, v) in [(0, 19), (1, 27), (2, 22)] {
            s.gear[0].set_stat(stat, v);
        }
        s.gear[0].description = 0xA3FB_594D;
        s.gear[12].set_slot(12);
        s.gear[13].set_slot(13);
        let g = game_setup(&s);
        assert_eq!((g.character, g.items[0].description), (0x405D_C1D5, 0xA3FB_594D));
        assert_eq!(g.items[0].stats[..3], [(0, 19), (1, 27), (2, 22)]);
        let stats = g.gear_stats();
        assert_eq!((stats.stats[0], stats.stats[1], stats.stats[2], stats.stats[3]), (19.0, 27.0, 22.0, 0.0));
        assert!(stats.slots[0] && stats.slots[12] && stats.slots[13] && !stats.slots[1]);
        assert!(RideOptions::default().setup.is_none());
    }

    #[test]
    fn the_results_portrait_is_the_launched_riders_own() {
        use ssx_assets::career::{CareerDb, Rider, RiderContent};
        let rider = |name: &str, icon: &str, content: Option<RiderContent>| Rider {
            key: ssx_formats::attrib::fnv1(name.as_bytes()),
            name: name.into(),
            title: String::new(),
            display_name: name.into(),
            cost: 0,
            locked: false,
            default: false,
            unk_940b9b22: 0,
            stat_bonuses: Vec::new(),
            portrait: String::new(),
            description: String::new(),
            round_icon: String::new(),
            icon: icon.into(),
            content,
        };
        let id: ssx_content::ContentId = "skins:character/sam".parse().expect("an id");
        let added = RiderContent {
            id: id.clone(),
            stand_in: ssx_formats::attrib::fnv1(b"sam"),
            portrait: None,
            round_icon: None,
            icon: None,
            added: true,
            default_board: None,
            disc_stat_bonuses: None,
        };
        let career = CareerDb {
            riders: vec![rider("sam", "disc_sam.png", None), rider(id.as_str(), "package_sam.png", Some(added))],
            ..CareerDb::default()
        };
        assert_eq!(career.riders[1].key, ssx_assets::content::characters::career::content_key(&id));
        let rules = career.rules();
        assert_eq!(results_portrait(&career, &rules, id.as_str()), "package_sam.png");
        assert_eq!(results_portrait(&career, &rules, "sam"), "disc_sam.png");
        assert_eq!(results_portrait(&career, &rules, "nobody"), "");
    }
}
