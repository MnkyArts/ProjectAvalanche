use ssx_physics::event::{Course, EventState, EventType};
use ssx_physics::rider::{RiderState, World};

const USER_GAMERTAG: u16 = 0x22B;
const USER_GAMERPIC: u16 = 0x22C;
use ssx_physics::scoring::{RecordKind, TrickRecord};
use ssx_physics::session::tombstones::Tombstones;
use ssx_physics::session::{Execution, Session};

use super::GameUi;
use super::hud::HudMessage;
use super::hud::billboards::{Camera, Ghost, ghost_list, tombstone_list, tombstone_points, wingsuit_timer_pos};
use super::hud::trick::ComboEnded;
use super::leaderboard::{ExploreLeaderboard, RaceEntrant, TrickBot, TrickStandings, race_update};
use super::loc::Localization;
use super::tricks::{TrickRecordView, trick_name};
use super::values::{SplitsCallout, UiValue, UiValues, id};

pub const TRICKY_WARN_TICKS: f32 = 120.0;

// TODO(guess): true for every racer of the port (bots and ghosts keep their slot after their playback ended); the race leaderboard 0x824CC8B8 calls vt+0x28 of the racer record's +0x14 interface 0xD6FEE28B (at 0x824CCDD4; false → value −1, last); the player's actor component 0x82ABCFF0 registers 0xD6FEE28B with seven methods (0x82132568),
// so the racers' answer is another component's; the record's +0x10 interface 0x3D4D6AFE (vtable 0x821325CC) has a flag +0x454 (set by +0x24 0x82ABCB90, read by +0x28 0x82ABCB98, 0 from the constructor); settle: the component that answers 0xD6FEE28B for the racers and its +0x28.
pub const RACER_PRESENT: bool = true;

pub fn record_view(r: &TrickRecord) -> TrickRecordView {
    TrickRecordView {
        kind: r.kind as i32,
        trick: u32::from(r.trick_id.value()),
        flips_fwd: r.flips_fwd,
        flips_bwd: r.flips_bwd,
        interaction: r.interaction,
        tier: u32::from(r.tier),
        tweaked: r.tweaked,
        awesome: r.awesome,
        super_awesome: r.super_awesome,
        nose_press: r.nose_press,
        goofy: r.goofy,
        duplicate: r.duplicate,
        spin_fs: r.spin_fs_deg,
        spin_bs: r.spin_bs_deg,
        distance: r.distance,
        time: r.trick_time,
    }
}

#[derive(Clone, Debug, Default)]
pub struct HudFeed {
    combo: Vec<TrickRecord>,
    record: TrickRecord,
    finish_posted: bool,
    ready_loop: bool,
    race_started: bool,
    event_state: EventState,
    pub explore: ExploreLeaderboard,
    pub camera: Option<Camera>,
    pub ghosts: Vec<Ghost>,
    pub rider: String,
    pub ghost_racers: Vec<RaceEntrant>,
    pub rider_titles: std::collections::BTreeMap<u32, String>,
    pub trick: TrickStandings,
    pub session: Option<super::session_board::SessionHud>,
    heights: CourseHeights,
}

impl HudFeed {
    pub fn new(rider: &str) -> Self {
        HudFeed { rider: rider.to_string(), ..Default::default() }
    }

    pub fn feed(&mut self, ui: &mut GameUi, s: &RiderState, x: &Session, world: &World, e: Option<&Execution>) {
        self.write_course_heights(ui, x.event.setup.course);
        ui.gear = world.survival.slots;
        if e.is_some_and(|e| e.event.looped) {
            self.ready_loop = false;
            let (loops, descent) = (x.event.loops, x.event.descent_mm(s.score.descent));
            ui.fe_call(|fe, env| super::fe::flow::survive_loop(fe, env, loops, descent));
        }
        if let Some(o) = x.objectives.as_ref() {
            for (_, effect) in o.effects.get(x.effects_from..).unwrap_or(&[]) {
                if let ssx_physics::objectives::Effect::Fade { fade_out, seconds, delay } = effect {
                    let kind = if *fade_out { super::fe::fade::FADE_OUT } else { super::fe::fade::FADE_IN };
                    if let Some(fe) = ui.fe.as_mut() {
                        fe.fade_message(kind, *seconds, *delay);
                    }
                }
            }
        }
        let ready = s.start.as_ref().and_then(|l| l.intro.as_ref()).is_some_and(|i| i.in_ready_loop());
        if ready && !self.ready_loop {
            ready_loop_objectives(ui, x.event.loops);
            self.ready_loop = true;
        }
        if e.is_some_and(|e| e.race_start) {
            ui.values.set_bool(id::NIS_PROMPT_ACTIVE, false);
            ui.values.set_bool(id::BOOST_NOTIFICATION_ENABLED, false);
            ui.values.set_bool(id::METERS_ACTIVE, true);
            if ui.values.bool(id::HUDMESSAGE_IS_SURVIVAL_LOOP_MSG) {
                ui.values.set_bool(id::HUDMESSAGE_IS_SURVIVAL_LOOP_MSG, false);
                ui.values.set_bool(id::HUDMESSAGE_IS_ENABLED, false);
            }
            self.trick.reset(x.bots.as_ref().map_or(0, |b| b.bots.len()));
            if !self.race_started {
                self.race_started = true;
                ui.values.set_bool(id::RESULT_SCREEN_DATA_READY_FOR_PUBLISH, false);
                ui.values.set_bool(0x188, false);
            }
            let drop = ui.nav.drop_key;
            super::hud::show_drop_elements(&mut ui.values, drop);
            if let Some(fe) = ui.fe.as_mut() {
                super::fe::flow::race_start_camera(fe, &mut ui.values);
            }
        }
        let running = x.event.state == EventState::Running;
        self.messages(ui, s, x, world);
        if let Some((k, _ms)) = e.and_then(|e| e.event.split) {
            match split_callout_kind(x.event.setup.event_type, k) {
                SplitCallout::Race => self.split_callout(ui, s, x, world),
                SplitCallout::Survive => self.survive_callout(ui, s, x),
                SplitCallout::None => {}
            }
        }
        if running {
            self.feed_hud(ui, s, x, world);
        }
        self.project(ui, s, x);
        if e.is_some_and(|e| e.event.results) {
            self.race_started = false;
        }
        let started = matches!(
            x.event.state,
            EventState::Running
                | EventState::DelayedFinish
                | EventState::Finished
                | EventState::LoopOutro
                | EventState::LoopRestart
        );
        if started {
            let et = x.event.setup.event_type;
            if let Some(t @ (EventType::TrickGhosts | EventType::SurviveToDeath)) = et {
                let value = if t == EventType::TrickGhosts {
                    s.score.score
                } else {
                    ui.values.float(id::PLAYER_DESCENT_WITH_LAPS) * 1000.0
                };
                let player = self.player_name(ui);
                self.explore.update(&mut ui.values, &mut ui.loc, t, &player, value);
            }
        }
        let lb = matches!(x.event.state, EventState::Running | EventState::Finished) && !x.event.waiting;
        if let Some(hud) = self.session.as_ref() {
            if lb && let Some(t) = x.event.setup.event_type {
                let entrants = self.session_entrants(ui, s, x, hud);
                let course = x.event.setup.course.map(|c| (c.start_y, c.end_y));
                super::session_board::update(&mut ui.values, &mut ui.loc, t, hud, &entrants, course);
            }
        } else if lb && x.event.setup.event_type == Some(EventType::TrickBots) {
            self.trick_standings(ui, s, x, world);
        }
        if lb
            && self.session.is_none()
            && let Some(t @ (EventType::RaceGhosts | EventType::RaceBots)) = x.event.setup.event_type
            && let Some(c) = x.event.setup.course
        {
            let player = RaceEntrant {
                name: self.player_name(ui),
                picture: ui.values.str(USER_GAMERPIC).to_string(),
                active: true,
                finish_ms: x.event.finish_ms,
                y: Some(s.position.y),
            };
            let racers = self.racers(ui, x, world);
            race_update(&mut ui.values, &mut ui.loc, t, &player, &racers, c.start_y, c.end_y);
        }
        if x.event.finish_message.is_some() && !self.finish_posted {
            let key = if x.event.setup.event_type == Some(EventType::SurviveToDeath) {
                "TXT_HUDMESSAGE_SURVIVED"
            } else {
                "TXT_HUDMESSAGE_FINISHED"
            };
            let text = ui.loc.text(key);
            ui.values.set_str(id::FINISH_MESSAGE, text);
            ui.values.set_bool(id::FINISH_MESSAGE_ACTIVE, true);
            ui.values.set_bool(id::WINGSUIT_ACTIVE, false);
            self.finish_posted = true;
        }
        if x.event.state == EventState::LoopOutro && self.event_state != EventState::LoopOutro {
            super::hud::set_hud_shown(&mut ui.values, false, false, ui.nav.drop_key);
        }
        self.event_state = x.event.state;
        ui.values.set_bool(id::NIS_IS_ACTIVE, x.flow.nis_active || s.start.is_some());
    }

    fn player_name(&self, ui: &GameUi) -> String {
        match ui.values.str(USER_GAMERTAG) {
            "" => self.rider.clone(),
            t => t.to_string(),
        }
    }

    fn trick_standings(&mut self, ui: &mut GameUi, s: &RiderState, x: &Session, world: &World) {
        let player = RaceEntrant {
            name: self.player_name(ui),
            picture: ui.values.str(USER_GAMERPIC).to_string(),
            active: true,
            ..Default::default()
        };
        let drop = ui.nav.drop_key;
        let world_tour = ui
            .fe
            .as_ref()
            .and_then(|f| f.career.event(drop))
            .is_none_or(|e| e.drop_type == ssx_assets::career::DropType::Campaign);
        let racers = self.racers(ui, x, world);
        let bots: Vec<TrickBot> = x.bots.as_ref().map_or_else(Vec::new, |b| {
            b.bots
                .iter()
                .zip(racers)
                .enumerate()
                .map(|(k, (bot, r))| TrickBot {
                    name: r.name,
                    picture: r.picture,
                    live: x.event.racers.get(k).map_or(0.0, |p| p.points),
                    recorded: bot.replay.header.score,
                })
                .collect()
        });
        let progress = x.event.progress;
        self.trick.update(
            &mut ui.values,
            &mut ui.loc,
            EventType::TrickBots,
            &player,
            s.score.score,
            progress,
            world_tour,
            &bots,
        );
    }

    fn session_entrants(
        &self,
        ui: &GameUi,
        s: &RiderState,
        x: &Session,
        hud: &super::session_board::SessionHud,
    ) -> Vec<super::session_board::Entrant> {
        let mut out = vec![super::session_board::Entrant {
            name: self.player_name(ui),
            picture: ui.values.str(USER_GAMERPIC).to_string(),
            player: true,
            finish_ms: x.event.finish_ms,
            out: x.event.result.as_ref().is_some_and(|r| r.failed),
            y: s.position.y,
            points: s.score.score.max(0.0) as u32,
            descent_mm: x.event.descent_mm(s.score.descent),
            place: hud.player_place,
        }];
        let Some(b) = x.bots.as_ref() else { return out };
        for (_, id) in b.live_slots.occupants() {
            let (Some(racer), Some(r)) = (b.live.iter().find(|r| r.id == id), hud.riders.get(&id)) else { continue };
            out.push(super::session_board::Entrant {
                name: r.name.clone(),
                picture: r.picture.clone(),
                player: false,
                finish_ms: r.finish_ms,
                out: r.out,
                y: racer.position.y,
                points: r.points,
                descent_mm: r.descent_mm,
                place: r.place,
            });
        }
        out
    }

    fn racers(&self, ui: &mut GameUi, x: &Session, world: &World) -> Vec<RaceEntrant> {
        let Some(b) = x.bots.as_ref() else { return self.ghost_racers.clone() };
        let inputs = b.racers(world);
        b.bots
            .iter()
            .enumerate()
            .map(|(k, bot)| {
                let key = self.rider_titles.get(&bot.request.character).cloned().unwrap_or_default();
                let name = ui.loc.text(&key);
                RaceEntrant {
                    picture: format!("assets\\gamertagIcons\\{}.dds", name.to_ascii_lowercase()),
                    name,
                    active: RACER_PRESENT,
                    finish_ms: x.event.racers.get(k).and_then(|r| r.finish_ms),
                    y: Some(inputs.get(k).copied().flatten().map_or(bot.position.y, |r| r.y)),
                }
            })
            .collect()
    }

    fn feed_hud(&mut self, ui: &mut GameUi, s: &RiderState, x: &Session, world: &World) {
        let sc = &s.score;
        let rider = self.rider.clone();
        let name = match self.record.kind {
            RecordKind::None | RecordKind::Rewind => String::new(),
            _ => trick_name(&mut ui.loc, &record_view(&self.record), &rider, false),
        };
        let v = &mut ui.values;
        v.set_int(id::RACE_TIME, x.event.clock.ms() as i32);
        v.set_float(id::PLAYER_SPEED, s.player_speed);
        v.set_float(id::PLAYER_DESCENT_WITH_LAPS, x.event.descent_mm(sc.descent) as f32 * 0.001);
        v.set_float(id::PLAYER_DESCENT, ((sc.descent * 1000.0) as i64 as f32) * 0.001);
        v.set_float(id::SCORE, sc.score);
        // TODO(guess): derived from the boost at each feed (the game stores it when the scoring op runs, 0x82CF0230); settle: the frames the op is skipped (pause, NIS, rewind snapshots, 0x82CF1C70).
        v.set_float(id::BOOST_PERCENT, sc.boost / world.tuning.scoring.max_boost);
        v.set_bool(id::IS_BOOSTING, s.boost.is_boosting);
        v.set_bool(id::IS_REWINDING, sc.is_rewinding_backwards);
        let count = x.manager.count;
        v.set_int(id::REWIND_COUNT, count.rewinds_left);
        v.set_int(id::REWIND_COUNT_INITIAL, x.manager.tuning.max_rewind_count);
        v.set_bool(id::REWIND_COUNT_INFINITE, count.infinite);
        v.set_bool(id::REWIND_COUNT_ENABLED, count.enabled);
        v.set_float(id::REWIND_SCORE, if sc.is_rewinding_backwards { sc.rewind_bank } else { 0.0 });
        v.set_int(id::FLOW_LEVEL, sc.flow_level);
        v.set_bool(id::IS_AWESOME, sc.is_awesome);
        let timer = if sc.is_super_awesome { sc.super_timer } else { sc.awesome_timer };
        v.set_int(id::AWESOME_WARN_TIER, i32::from(timer > 0.0 && timer <= TRICKY_WARN_TICKS));
        v.set_bool(id::IS_SUPER_AWESOME, sc.is_super_awesome);
        v.set_float(id::SUPER_AWESOME_ACTIVATION_PERCENTAGE, sc.super_activation);
        v.set_float(id::TRICK_AIR_DIST, sc.flight_distance);
        v.set_int(id::TRICK_TIER, i32::from(self.record.tier));
        v.set_bool(id::TRICK_IS_DUPLICATE, self.record.duplicate);
        v.set_str(id::TRICK_GRAB_NAME, name);
        v.set_float(id::COMBO_CURRENT_TIMER_PERCENTAGE, sc.combo_pct);
        v.set_float(id::WINGSUIT_USAGE, s.wingsuit.use_percentage);
        v.set_bool(id::WINGSUIT_ACTIVE, s.wingsuit.signal && x.event.state == EventState::Running);
        let h = s.survival.hud();
        v.set_float(id::ARMOUR, h.armour);
        v.set_float(id::HEAD_LAMP_LEVEL, h.head_lamp_battery);
        v.set_float(id::PULSE_GOGGLE_LEVEL, h.pulse_goggles_battery);
        v.set_float(id::HEALTH, h.health);
        v.set_float(id::HEALTH_MAX, h.health_max);
        v.set_float(id::TEMPERATURE, h.temperature);
        v.set_float(id::TEMPERATURE_MAX, h.temperature_max);
        v.set_float(id::ICE_AXE_EFFECTIVENESS, h.ice_axe_effectiveness);
        v.set_float(id::WINGSUIT_EFFECTIVENESS, s.wingsuit.effectiveness);
        v.set_float(id::OXYGEN_TANK_LEVEL, h.oxygen_tank);
        v.set_bool(id::IS_OXYGEN_USE_REQUESTED, h.oxygen_use_requested);
    }

    pub fn project(&self, ui: &mut GameUi, s: &RiderState, x: &Session) {
        let in_air = s.in_air().then_some([s.position.x, s.position.y, s.position.z]);
        self.project_into(&mut ui.values, &mut ui.loc, in_air, x.tombstones.as_ref());
    }

    fn project_into(
        &self,
        v: &mut UiValues,
        loc: &mut Localization,
        in_air: Option<[f32; 3]>,
        tombstones: Option<&Tombstones>,
    ) {
        let latch = self.race_started;
        let tombstones = if latch { tombstone_points(tombstones, loc) } else { Vec::new() };
        if let Some(cam) = &self.camera {
            v.set(id::GHOST_LIST, UiValue::Markers(ghost_list(cam, &self.ghosts)));
            if let Some(p) = in_air.filter(|_| latch) {
                let [x, y] = wingsuit_timer_pos(cam, p);
                v.set_float(id::WINGSUIT_TIMER_POS_X, x);
                v.set_float(id::WINGSUIT_TIMER_POS_Y, y);
            }
            v.set(id::TOMBSTONE_LIST, UiValue::Markers(tombstone_list(cam, &tombstones)));
        } else if latch {
            // TODO(guess): without a camera (the headless tools) the wingsuit timer sits at the stage centre (640, 541); settle: none (the viewer passes its camera, `HudFeed::camera`).
            v.set_float(id::WINGSUIT_TIMER_POS_X, 640.0);
            v.set_float(id::WINGSUIT_TIMER_POS_Y, 541.0);
        }
    }

    pub fn paused_frame(&self, ui: &mut GameUi, s: &RiderState, x: &Session) {
        self.project(ui, s, x);
        ui.values.set_bool(id::NIS_IS_ACTIVE, x.flow.nis_active || s.start.is_some());
    }

    fn messages(&mut self, ui: &mut GameUi, s: &RiderState, x: &Session, world: &World) {
        let sc = &s.score;
        let rider = self.rider.clone();
        let mut list_changed = false;
        // TODO(guess): the messages are delivered after the step in the order sent (posted via 0x831A3AE0 / 0x82AB7AA0); settle: the entity message queue's dispatch point.
        if let Some(r) = sc.trick_update {
            self.record = r;
        }
        for f in sc.flushed.iter().flatten() {
            if f.is_bad_landing {
                self.combo.clear();
            } else if self.record.kind != RecordKind::None {
                self.combo.push(self.record);
            }
            self.record = TrickRecord::default();
            ui.values.set_int(id::COMBO_LEVEL, sc.combo_count as i32);
            ui.values.set_float(id::COMBO_TRICK_POINTS, sc.combo_points);
            ui.values.set_float(id::COMBO_TOTAL_POINTS, sc.combo_points);
            list_changed = true;
            ui.hud.messages.push(HudMessage::BanksFlushed { captured: f.is_points_captured });
        }
        if let Some(m) = sc.combo_ended {
            ui.values.set_float(id::COMBO_BONUS_POINTS, m.bonus_points);
            ui.values.set_float(id::COMBO_TRICK_POINTS, m.trick_points);
            ui.values.set_float(id::COMBO_TOTAL_POINTS, m.total_points);
            ui.values.set_int(id::COMBO_LEVEL, 0);
            ui.values.set_bool(id::COMBO_SUCCESS, m.success);
            self.combo.clear();
            self.record = TrickRecord::default();
            list_changed = true;
            if x.event.setup.event_type == Some(EventType::TrickBots) {
                self.trick_standings(ui, s, x, world);
            }
            ui.hud.messages.push(HudMessage::ScoreComboEnded(ComboEnded {
                success: m.success,
                trick_points: m.trick_points,
                bonus_points: m.bonus_points,
                total_points: m.total_points,
                combo_level: m.combo_level,
            }));
        }
        if list_changed {
            let names: Vec<String> =
                self.combo.iter().map(|r| trick_name(&mut ui.loc, &record_view(r), &rider, true)).collect();
            ui.values.set(id::COMBO_TRICK_LIST, UiValue::List(names));
        }
    }

    fn write_course_heights(&mut self, ui: &mut GameUi, course: Option<Course>) {
        self.heights.write(&mut ui.values, course);
    }

    fn split_callout(&mut self, ui: &mut GameUi, s: &RiderState, x: &Session, world: &World) {
        let Some(course) = x.event.setup.course else { return };
        let racers = self.racers(ui, x, world);
        let Some((low, y)) =
            racers.iter().filter_map(|r| r.y.map(|y| (r, y))).fold(None::<(&RaceEntrant, f32)>, |a, (r, y)| match a {
                Some((b, by)) if by <= y => Some((b, by)),
                _ => Some((r, y)),
            })
        else {
            return;
        };
        let (ahead, gap) = split_gap(x.event.clock.ms() as f32, course.start_y, s.position.y, y);
        let kind = x.event.setup.event_type.map_or(2, super::leaderboard::value_kind);
        let gap = super::leaderboard::value_text(&mut ui.loc, kind, gap as u32 as i32, true);
        let key = if ahead { "TXT_SPLITS_YOU_ARE_AHEAD" } else { "TXT_SPLITS_YOU_ARE_BEHIND" };
        let body = ui.loc.format(key, &[&gap, &low.name]);
        let callout = SplitsCallout { ahead, show_time: false, kind: 2, body, ..Default::default() };
        ui.values.set(id::SPLITS_CALLOUT, UiValue::Splits(callout));
        ui.values.set_bool(id::SPLITS_CALLOUT_ENABLED, true);
    }

    fn survive_callout(&mut self, ui: &mut GameUi, s: &RiderState, x: &Session) {
        let Some(course) = x.event.setup.course else { return };
        let Some(left) = survive_left(s.position.y, course.end_y) else { return };
        let text = ui.loc.distance_long(f64::from(left));
        let body = ui.loc.format("TXT_SPLITS_DISTANCE_REMAINING", &[&text]);
        let callout = SplitsCallout { ahead: true, show_time: false, kind: 2, body, ..Default::default() };
        ui.values.set(id::SPLITS_CALLOUT, UiValue::Splits(callout));
        ui.values.set_bool(id::SPLITS_CALLOUT_ENABLED, true);
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct CourseHeights {
    written: bool,
}

impl CourseHeights {
    fn write(&mut self, v: &mut UiValues, course: Option<Course>) -> bool {
        if self.written {
            return false;
        }
        self.written = true;
        let (start, end) = course.map_or((0.0, 0.0), |c| (c.start_y, c.end_y));
        v.set_float(id::START_ELEVATION, start);
        v.set_float(id::END_ELEVATION, end);
        true
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SplitCallout {
    None,
    Race,
    Survive,
}

fn split_callout_kind(event_type: Option<EventType>, split: i32) -> SplitCallout {
    match event_type {
        Some(EventType::RaceGhosts | EventType::RaceBots) if split >= 1 => SplitCallout::Race,
        Some(EventType::SurviveToBottom | EventType::SurviveToDeath) => SplitCallout::Survive,
        _ => SplitCallout::None,
    }
}

fn split_gap(clock_ms: f32, start: f32, me: f32, low: f32) -> (bool, f32) {
    if low > me { (true, clock_ms / (start - me) * (low - me)) } else { (false, clock_ms / (start - low) * (me - low)) }
}

fn survive_left(y: f32, end: f32) -> Option<f32> {
    let left = y - end;
    (left > 0.0).then_some(left)
}

const NO_PROMPT_DROPS: [u32; 3] = [0x28F6_34F8, 0xC145_699A, 0xB0F3_51A2];

pub fn nis_prompt_shown(drop: u32, no_prompt: bool, tutorial: bool, loops: u32, session: bool) -> bool {
    // TODO(guess): a port choice (the user's, 2026-11-08: a change of equipment makes no sense in a multiplayer session) — a network session's level counts as HUD_NONISPROMPT (UI 0x11A) raised: the game has no network sessions, and its own rule (0x11A set when the front end's frame state begins, 0x82381C88) leaves the prompt to a level restarted in place, which a session's next round on the same drop is (A_GameRestartStage); settle: none (no network sessions on the disc).
    !no_prompt && !session && !tutorial && loops == 0 && !NO_PROMPT_DROPS.contains(&drop)
}

fn ready_loop_objectives(ui: &mut GameUi, loops: u32) {
    let drop = ui.nav.drop_key;
    let (tutorial, session) = match ui.fe.as_ref() {
        Some(fe) => (fe.profile.is_tutorial_drop(drop), fe.network_session),
        None => (ssx_assets::career::Profile::new().is_tutorial_drop(drop), false),
    };
    ready_loop_values(&mut ui.values, &mut ui.loc, drop, tutorial, loops, session);
}

fn ready_loop_values(v: &mut UiValues, loc: &mut Localization, drop: u32, tutorial: bool, loops: u32, session: bool) {
    if nis_prompt_shown(drop, v.bool(id::NO_NIS_PROMPT), tutorial, loops, session) {
        let text = loc.text("TXT_NISPROMPT_MESSAGE");
        v.set_str(id::NIS_PROMPT_MESSAGE, text);
        v.set_int(id::NIS_PROMPT_CALLOUT, 4);
        v.set_bool(id::NIS_PROMPT_ACTIVE, true);
    }
    v.set_bool(id::NO_NIS_PROMPT, false);
    if !tutorial {
        v.set_bool(id::BOOST_NOTIFICATION_ENABLED, true);
    }
    v.set_str(id::NIS_TITLE_MESSAGE, "");
    for i in [id::NIS_TITLE_ACTIVE, id::NIS_TITLE_ENABLED, id::NIS_TITLE_INITIALIZED] {
        v.set_bool(i, false);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ssx_assets::career::Profile;
    use ssx_assets::career::profile::DropState;
    use ssx_formats::attrib::fnv1;

    #[test]
    fn the_ready_loops_prompt_follows_the_profile_and_the_loops() {
        let (prologue, first, second) =
            (fnv1(b"chapter_1_drop_h2h"), fnv1(b"chapter_2_drop_h2h"), fnv1(b"chapter_2_drop_gen_1"));
        let (final_drop, other) = (fnv1(b"chapter_11_drop_h2h"), fnv1(b"exp_alps_vorassay_drop_3"));
        let mut p = Profile::new();
        assert!(p.is_tutorial_drop(prologue) && p.is_tutorial_drop(first) && p.is_tutorial_drop(second));
        assert!(!nis_prompt_shown(prologue, false, p.is_tutorial_drop(prologue), 0, false));
        p.drop_states.insert(second, DropState::Completed);
        assert!(nis_prompt_shown(prologue, false, p.is_tutorial_drop(prologue), 0, false));
        assert!(!p.is_tutorial_drop(first), "the boost notification shows on a replayed tutorial");
        assert!(!nis_prompt_shown(first, false, p.is_tutorial_drop(first), 0, false));
        assert!(!nis_prompt_shown(second, false, p.is_tutorial_drop(second), 0, false));
        assert!(!nis_prompt_shown(final_drop, false, false, 0, false));
        assert!(nis_prompt_shown(other, false, false, 0, false));
        assert!(!nis_prompt_shown(other, true, false, 0, false), "HUD_NONISPROMPT");
        assert!(!nis_prompt_shown(other, false, false, 1, false), "a survive loop's next intro");
    }

    #[test]
    fn the_prompt_comes_on_a_restart_and_never_in_a_session() {
        let drop = fnv1(b"exp_alps_vorassay_drop_3");
        let mut loc = Localization::empty(crate::game::loc::Language::English);
        for session in [false, true] {
            let mut v = UiValues::new();
            let mut hud_ready_seen = true;
            let due = crate::game::fe::flow::front_end_meta_values(&mut v, &mut hud_ready_seen);
            assert!(due && !hud_ready_seen, "0x329E66FA {{1}} after a level came up, once");
            assert!(v.bool(id::NO_NIS_PROMPT) && v.bool(id::NIS_TITLE_ENABLED));
            ready_loop_values(&mut v, &mut loc, drop, false, 0, session);
            assert!(!v.bool(id::NIS_PROMPT_ACTIVE), "the first ready loop after the front end (session {session})");
            assert!(!v.bool(id::NO_NIS_PROMPT), "DO_NisPrompt clears HUD_NONISPROMPT");
            ready_loop_values(&mut v, &mut loc, drop, false, 0, session);
            assert_eq!(v.bool(id::NIS_PROMPT_ACTIVE), !session, "the restarted level's ready loop (session {session})");
            if !session {
                assert_eq!(v.str(id::NIS_PROMPT_MESSAGE), "TXT_NISPROMPT_MESSAGE");
                assert_eq!(v.int(id::NIS_PROMPT_CALLOUT), 4);
            }
        }
        let mut v = UiValues::new();
        let mut hud_ready_seen = false;
        assert!(!crate::game::fe::flow::front_end_meta_values(&mut v, &mut hud_ready_seen), "the boot's: no level");
    }

    #[test]
    fn the_split_callout_follows_the_event_type() {
        use SplitCallout::*;
        for (t, split, want) in [
            (Some(EventType::RaceGhosts), 0, None),
            (Some(EventType::RaceGhosts), 1, Race),
            (Some(EventType::RaceGhosts), 8, Race),
            (Some(EventType::RaceBots), 1, Race),
            (Some(EventType::RaceClock), 1, None),
            (Some(EventType::SurviveToBottom), 0, Survive),
            (Some(EventType::SurviveToDeath), 8, Survive),
            (Some(EventType::TrickScore), 1, None),
            (Some(EventType::TrickBots), 1, None),
            (Some(EventType::TrickGhosts), 1, None),
            (Some(EventType::FreeRide), 1, None),
            (Option::<EventType>::None, 1, None),
        ] {
            assert_eq!(split_callout_kind(t, split), want, "{t:?} split {split}");
        }
    }

    #[test]
    fn the_split_gap_is_the_clock_scaled_by_the_heights() {
        let (ahead, gap) = split_gap(60_000.0, 1_000.0, 400.0, 300.0);
        assert!(!ahead && (gap - 8_571.428).abs() < 0.01);
        let (ahead, gap) = split_gap(60_000.0, 1_000.0, 300.0, 400.0);
        assert!(ahead && (gap - 8_571.428).abs() < 0.01);
        assert_eq!(split_gap(60_000.0, 1_000.0, 400.0, 400.0), (false, 0.0));
    }

    #[test]
    fn the_survive_callout_is_the_height_above_the_end() {
        assert_eq!(survive_left(-519.6, -1_280.0), Some(760.4), "760.4 m above the end");
        assert_eq!(survive_left(-1_280.0, -1_280.0), None, "at the end");
        assert_eq!(survive_left(-1_300.0, -1_280.0), None, "below the end");
    }

    #[test]
    fn a_levels_course_heights_replace_the_previous_levels() {
        let mut v = UiValues::new();
        let mut h = CourseHeights::default();
        assert!(h.write(&mut v, Some(Course { start_y: 1_794.0, end_y: -1_070.0 })));
        assert_eq!((v.float(id::START_ELEVATION), v.float(id::END_ELEVATION)), (1_794.0, -1_070.0));
        assert!(!h.write(&mut v, Some(Course { start_y: 0.0, end_y: -1.0 })), "once per level");
        let mut h = CourseHeights::default();
        assert!(h.write(&mut v, Some(Course { start_y: 308.7, end_y: -1_280.0 })));
        assert_eq!((v.float(id::START_ELEVATION), v.float(id::END_ELEVATION)), (308.7, -1_280.0));
        let mut h = CourseHeights::default();
        assert!(h.write(&mut v, None));
        assert_eq!((v.float(id::START_ELEVATION), v.float(id::END_ELEVATION)), (0.0, 0.0));
    }

    #[test]
    fn the_projections_are_written_in_every_frame() {
        let mut f = HudFeed::new("elise");
        let identity = [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0], [0.0, 0.0, 0.0, 1.0]];
        f.camera = Some(Camera { view_proj: identity, screen: [1280.0, 720.0] });
        f.ghosts = vec![Ghost { id: 7, name: "Gold".into(), colour: 0xFF00_0000, position: [0.0, -0.8, 10.0] }];
        let mut v = UiValues::new();
        let mut loc = Localization::empty(crate::game::loc::Language::English);
        for _ in 0..2 {
            v.clear_dirty();
            f.project_into(&mut v, &mut loc, Some([0.0, 0.0, 10.0]), None);
            assert!(v.is_dirty(id::GHOST_LIST) && v.is_dirty(id::TOMBSTONE_LIST));
            let g = v.markers(id::GHOST_LIST);
            assert_eq!((g.len(), g[0].x, g[0].y), (1, 640.0, 360.0));
            assert!(v.markers(id::TOMBSTONE_LIST).is_empty());
            assert!(!v.written(id::WINGSUIT_TIMER_POS_X), "no wingsuit timer before the race latch");
        }
        f.race_started = true;
        f.project_into(&mut v, &mut loc, Some([0.0, 0.0, 10.0]), None);
        assert_eq!((v.float(id::WINGSUIT_TIMER_POS_X), v.float(id::WINGSUIT_TIMER_POS_Y)), (715.0, 360.0));
    }
}
