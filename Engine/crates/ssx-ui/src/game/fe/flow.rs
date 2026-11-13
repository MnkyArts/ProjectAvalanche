use ssx_assets::career::new_game::{FlowAction, FrontEnd as FlowState, NEW_GAME_DONE};

use super::{FeEnv, FrontEnd, Launch, MovieRequest, screens};

#[derive(Clone, Debug, Default)]
pub struct NewGameHost {
    pub last_state: u32,
}

pub fn signed_in(fe: &mut FrontEnd, env: &mut FeEnv, device: i32) {
    // PORT(choice): the port's sign-in writes the player name (the launcher's `settings.json` `player_name`, else
    // `Player`), which the Options and the name prompt write again when it changes (in the port this sign-in and a
    // confirmed player name are the only writers of `USER_gamertag`, UI 0x22B); no Live in the port: the game's
    // gamertag and picture come from Xbox Live.
    env.values.set_str(0x22B, super::ava::profile_name(fe));
    env.values.set_bool(0x22D, false);
    env.values.set_bool(0x22E, false);
    env.values.set_str(0x22C, "assets/gamertagIcons/player.dds");
    env.values.set_int(0x22F, 0);
    fe.busy.add(env, super::busy::LEVEL);
    fe.busy.add(env, super::busy::LEVEL);
    profile_into_sign_in(fe, env);
    fe.note(format!("Profile_OnSignIn (controller {device}): Explore drops unlocked"));
    super::pads::sign_in(fe, device);
    // TODO(guess): the job's step order against these writes, a failed job's callback (0x82396B58, 0x825081E8) and a profile without a file: the save step 0x825082B0 posts 0x10AD0001 and waits for 0x10AD1001 (0 → state 5: the manager's 0x8244E0A0 with the callback 0x825081E8, not traced; else the job ends with result 1);
    // `FlowLeadProfileLoad` goes from a failed `OperationFind` to `FlowFindError` (→ state 3 or 0x17); settle: the find's failure `FlowFindError`.
    if super::saveload::lead_profile_load(fe, env, device) {
        profile_job_done(fe, env, device);
    }
}

pub fn profile_into_sign_in(fe: &mut FrontEnd, env: &mut FeEnv) {
    env.values.set_int(0x228, fe.profile.credits() as i32);
    fe.profile.unlock_explore(&fe.rules);
}

pub fn profile_job_done(fe: &mut FrontEnd, env: &mut FeEnv, _device: i32) {
    fe.busy.release(env, super::busy::LEVEL);
    fe.requests.push(super::pause::GameRequest::Save);
    fe.note("the save manager's save after the sign-in (0x82457FE8)");
    // TODO(guess): the save completes at once in the port, so the front end's frame state begins with the request (its 0x82469EB8: the second count off, message 0xCC21B341) (the game's after the callback 0x82395F50 switched the frame state: the sign-in adds two busy-icon counts, 0x8246A020 and 0x82396D78; the job's end 0x82396B58 takes one off and asks for the save, 0x82457FE8 → job 0x8250B278;
    // its end requests frame state 2, whose begin 0x82469EB8 takes the other off); settle: the save manager's job queue and callback timing.
    front_end_frame_state_begins(fe, env);
    fe.note("the profile job's end: message 0xDDA6AD47");
    fe.messages.push(super::FeMessage::BootFlowLoaded);
    fe.note("FEBootFlowCompleteMessage (0xB17A59F5)");
    let earnings: i32 =
        [(0u16, 5u16), (1, 6), (3, 4)].iter().map(|&(a, b)| (env.values.int(a) - env.values.int(b)).max(0)).sum();
    if earnings == 0 {
        fe.note("FEGlobeUiTransRequestMessage {0, 0}");
        env.values.set_int(0x11, 1);
        if fe.ava.hub.on {
            fe.go_to_screen_by(env, screens::id::MAIN_MENU, super::ava::hub::OPEN);
        } else {
            fe.go_to_screen(env, screens::id::MAIN_MENU);
        }
    } else {
        let t = env.loc.text("TXT_WYWA_TITLE");
        env.values.set_str(0x32, t);
        env.values.set_int(0x11, 2);
        fe.go_to_screen(env, screens::id::WELCOME_HUB);
    }
}

pub fn launch(fe: &mut FrontEnd, env: &mut FeEnv, drop: u32, direct: bool) {
    env.values.set_int(0xB4, drop as i32);
    let mut l: Launch = super::globe::launch_info(fe, drop);
    l.direct = direct;
    if fe.free_ride_drop == Some(drop) {
        fe.free_ride_drop = None;
        l.free_ride = true;
    }
    launch_with(fe, env, l);
}

pub fn launch_free_ride(fe: &mut FrontEnd, env: &mut FeEnv, drop: u32, direct: bool) {
    fe.free_ride_drop = Some(drop);
    launch(fe, env, drop, direct);
}

pub const MAP_GAME_MODE: i64 = 2;

pub fn launch_map(fe: &mut FrontEnd, env: &mut FeEnv, map: super::MapLaunch, event_type: i64) {
    env.values.set_int(0xB4, 0);
    let l = Launch {
        event: map.title.clone(),
        track: map.track.clone(),
        event_type,
        game_mode: MAP_GAME_MODE,
        map: Some(map),
        ..Launch::default()
    };
    fe.ava.maps.riding = l.map.as_ref().map(|m| m.id.clone());
    launch_with(fe, env, l);
}

fn launch_with(fe: &mut FrontEnd, env: &mut FeEnv, mut l: Launch) {
    let (drop, direct) = (l.drop, l.direct);
    let rider = commit_setup(fe, env, &mut l);
    fe.game_mode = l.game_mode;
    fe.current_drop = drop;
    level_set_up(fe);
    fe.equipment.drop = drop;
    fe.note(format!(
        "launch {} ({:#010x}) on {} — event type {}, game mode {}, rider {}{}",
        l.event,
        drop,
        l.track,
        l.event_type,
        l.game_mode,
        rider,
        if direct { ", direct" } else { "" }
    ));
    if l.free_ride {
        fe.note("free ride: the drop's event with the Freeride flag (no explore ghosts or objective messages, a loop)");
    }
    fe.in_drop = true;
    fe.game_pause.reset();
    fe.go_to_screen(env, screens::id::LOADING);
    fe.note("FENotificationPendingLoadBE (0x54A3F73C)");
    pending_load(fe, l);
}

fn commit_setup(fe: &mut FrontEnd, env: &mut FeEnv, l: &mut Launch) -> String {
    let mut inv = fe.equipment.inv.take().unwrap_or_else(|| Box::new(super::inventory::Inventory::new(&fe.career)));
    inv.store.clear_shown_special();
    l.setup = super::setup::commit(&inv, &fe.career, &fe.profile, env.loc);
    fe.equipment.inv = Some(inv);
    l.rider = match fe.career.riders.iter().find(|r| r.key == l.setup.character && l.setup.character != 0) {
        Some(r) => r.name.clone(),
        None => fe.equipment.rider_name(fe),
    };
    l.rider.clone()
}

#[derive(Clone, Debug, Default)]
pub struct LoadLatches {
    pub pending: bool,
    pub screen_loaded: bool,
    pub prepared: bool,
    pub job: Option<u8>,
    pub setup_countdown: u8,
    pub commit_due: bool,
    pub quit_pending: bool,
    launch: Option<Launch>,
}

fn pending_load(fe: &mut FrontEnd, l: Launch) {
    let ll = &mut fe.load_latches;
    (ll.pending, ll.screen_loaded, ll.prepared) = (true, false, false);
    ll.job = Some(0);
    ll.launch = Some(l);
}

pub fn loading_screen_loaded(fe: &mut FrontEnd) {
    fe.load_latches.screen_loaded = true;
    fe.note("FELoadingScreenLoaded (0x142C97B6): +0xF57");
}

// TODO(guess): one job step per FE update, before the latch test, the stats groups resolved at state 1's first test; settle: the job queue's update (service 0xC666D625) and 0x82BF0170 offline.
fn prepare_job_step(fe: &mut FrontEnd) {
    let Some(state) = fe.load_latches.job else { return };
    fe.load_latches.job = match state {
        0 => Some(1),
        1 => Some(4),
        _ => {
            fe.load_latches.prepared = true;
            fe.note("the drop's StatsGroupFetcher job ends: FEPrepareLoadingResponse (0xFAC82297): +0xF58");
            None
        }
    };
}

pub fn load_latches(fe: &mut FrontEnd, env: &mut FeEnv) {
    prepare_job_step(fe);
    if fe.load_latches.quit_pending && fe.load_latches.screen_loaded {
        (fe.load_latches.quit_pending, fe.load_latches.screen_loaded) = (false, false);
        fe.note("the quit latch: FENotificationEndGameplay, FE manager +0x40 (0xB0E2239A {5})");
        leave_level(fe, env);
    }
    if std::mem::take(&mut fe.load_latches.commit_due)
        && let Some(mut l) = fe.load_latches.launch.take()
    {
        commit_setup(fe, env, &mut l);
        fe.note(format!(
            "0x82393940: the commit — game setup (0x82498BA0): {}; the load request 6 (0x82383610); \
             LoadGameResponseMessage {{0}} → view event Loaded",
            l.setup.describe(&fe.career)
        ));
        fe.launch = Some(l);
        fe.busy.add(env, super::busy::LEVEL);
        fe.game_actions.push(("Loaded".into(), super::ActionParam::default()));
    }
    let ll = &mut fe.load_latches;
    if ll.setup_countdown > 0 {
        ll.setup_countdown -= 1;
        ll.commit_due = ll.setup_countdown == 0;
    }
    if !(ll.pending && ll.screen_loaded && ll.prepared) {
        return;
    }
    (ll.pending, ll.screen_loaded, ll.prepared) = (false, false, false);
    ll.setup_countdown = 1;
    let quick = env.values.bool(0x1AD);
    env.values.set_bool(0x1AD, false);
    fe.note(format!("the load latches: the level's setup (0x29FBCF22{})", if quick { ", quick select" } else { "" }));
}

pub fn simulation_begins(fe: &mut FrontEnd) {
    fe.game_pause.reset();
    fe.simulation = true;
    fe.note("FSGameSimulationBE begins: a new pause manager");
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LevelStart {
    pub set_up: bool,
    pub due: bool,
    pub streamed: bool,
    pub countdown: i32,
    pub query: bool,
    pub nis: bool,
}

impl Default for LevelStart {
    fn default() -> Self {
        LevelStart { set_up: false, due: false, streamed: false, countdown: -1, query: false, nis: false }
    }
}

pub fn level_set_up(fe: &mut FrontEnd) {
    fe.level_start = LevelStart { set_up: true, due: true, streamed: false, ..fe.level_start };
}

pub fn stream_ready(fe: &mut FrontEnd, env: &mut FeEnv) {
    fe.note("the camera's streaming is resident: 0xDAE83AFD {0} → 0x823837A8 (+0xF52)");
    fe.game_pause.set(super::pause::reason::R7, false);
    fe.level_start.query = false;
    fe.level_start.streamed = true;
    if fe.level_start.due && !fe.game_pause.paused {
        arm_level_start(fe, env);
    }
}

fn arm_level_start(fe: &mut FrontEnd, env: &mut FeEnv) {
    fe.level_start.due = false;
    let (seconds, delay) = if std::mem::take(&mut fe.loop_fade_in) {
        (super::fade::LOOP_IN_S, super::fade::LOOP_IN_DELAY_S)
    } else {
        (super::fade::LEVEL_START_IN_S, 0.0)
    };
    fe.fade_message(super::fade::FADE_IN, seconds, delay);
    fe.busy.release(env, super::busy::LEVEL);
    fe.level_start.countdown = 1;
    fe.note("the level's start armed (0x824691E8): the countdown 1");
}

pub fn survive_loop(fe: &mut FrontEnd, env: &mut FeEnv, loops: u32, descent_mm: u32) {
    fe.note(format!("the survive loop's message 0x78669743 {{{loops}, {descent_mm} mm}} → 0x823864E0"));
    crate::game::hud::elements_off(env.values);
    let label = env.loc.text("TXT_DROPS_SURVIVED_COLON");
    env.values.set_str(0xCF, label);
    env.values.set_str(0xD0, loops.to_string());
    let label = env.loc.text("TXT_TOTAL_DESCENT_COLON");
    env.values.set_str(0xD5, label);
    let descent = env.loc.distance_with_unit(f64::from(descent_mm as f32 / 1000.0));
    env.values.set_str(0xD6, descent);
    let message = env.loc.text("TXT_RESTART_DROP_HUD_MESSAGE");
    env.values.set_str(0xD3, message);
    env.values.set_bool(0xD2, true);
    env.values.set_bool(0xD1, true);
    fe.level_start.due = true;
    fe.level_start.streamed = false;
    fe.level_start.query = true;
    fe.game_pause.set(super::pause::reason::R7, true);
    fe.fade_message(super::fade::FADE_OUT, 0.0, 0.0);
    fe.loop_fade_in = true;
}

pub fn level_start_update(fe: &mut FrontEnd, env: &mut FeEnv) {
    let s = fe.level_start;
    if s.set_up && s.due && s.streamed && !fe.game_pause.paused {
        arm_level_start(fe, env);
    }
    if fe.level_start.countdown > 0 {
        fe.level_start.countdown -= 1;
        if fe.level_start.countdown == 0 {
            fe.level_start.countdown = -1;
            fe.note("0x12B9B0F8 → 0x824077F8: A_GameGameplayStarted");
            fe.game_actions.push(("A_GameGameplayStarted".into(), super::ActionParam::default()));
        }
    }
}

pub fn count_range_visit(fe: &mut FrontEnd, l: &Launch) -> Option<u32> {
    let (range, rider) = visit_range_and_rider(fe, l)?;
    Some(fe.profile.visit_range_by(range, &rider))
}

pub fn range_visit_name(fe: &FrontEnd, l: &Launch) -> Option<String> {
    let (range, rider) = visit_range_and_rider(fe, l)?;
    Some(ssx_assets::career::new_game::range_visit_stat(range, &rider))
}

fn visit_range_and_rider(fe: &FrontEnd, l: &Launch) -> Option<(u32, String)> {
    let e = fe.career.event(l.drop)?;
    let (peak, range) = fe.career.peak_and_range(e);
    fe.career.peak(peak)?;
    let rider = fe.career.riders.iter().find(|r| r.name == l.rider)?.display_name.clone();
    Some((range, rider))
}

pub fn do_continue(fe: &mut FrontEnd, _env: &mut FeEnv) {
    fe.note("FENotificationPendingQuitBE (0xCFFD4E8B): +0xF56 (0x82383A08)");
    (fe.load_latches.quit_pending, fe.load_latches.screen_loaded) = (true, false);
}

fn consume_mods(fe: &mut FrontEnd, env: &mut FeEnv) {
    use crate::game::values::{UiValue, id};
    let mut inv = fe.equipment.inv.take().unwrap_or_else(|| Box::new(super::inventory::Inventory::new(&fe.career)));
    let consumed = inv.consume_mods(&mut fe.profile);
    if !inv.equipped[3].is_empty() && !inv.equipped[0].is_empty() {
        let drop = super::inventory::DropInfo::of(&fe.career, &fe.profile, fe.equipment.drop);
        inv.survival = inv.survival_score(&fe.career, &fe.profile, drop.as_ref());
        env.values.set_int(0x6C, inv.survival);
    }
    env.values.set(id::BOOST_NOTIFICATION, UiValue::Boosts(Vec::new()));
    if !consumed.is_empty() {
        fe.note(format!("FUN_82493450: the mods {consumed:x?} consumed"));
    }
    fe.equipment.inv = Some(inv);
    fe.equipment.rebuild("dp_equipment_items");
    fe.publish.push("dp_equipment_items".into());
}

pub fn leave_level(fe: &mut FrontEnd, env: &mut FeEnv) {
    fe.load_latches.quit_pending = false;
    consume_mods(fe, env);
    fe.level_start = LevelStart::default();
    fe.busy.add(env, super::busy::LEVEL);
    fe.requests.push(super::pause::GameRequest::Quit);
    fe.in_drop = false;
    fe.launch = None;
    fe.game_pause.reset();
    fe.simulation = false;
    env.values.set_bool(0x1AE, false);
}

pub const SAVED_CAMERA_TYPES: [i32; 3] = [4, 0, 5];
pub const CAMERA_TYPE_REVERSE: i32 = 2;
pub const SPLITS_CALLOUT_FORCE_OFF: u16 = 0x12F;

pub fn store_camera_type(fe: &mut FrontEnd) {
    let Some(t) = stored_camera_type(fe.camera_type) else { return };
    if fe.profile.camera_type() != t {
        fe.note(format!("the camera type {t} into the profile (CTYP, 0x82399190)"));
    }
    fe.profile.set_camera_type(t);
}

pub fn race_start_camera(fe: &mut FrontEnd, values: &mut crate::game::values::UiValues) {
    let (restore, reverse) = camera_restore(fe.profile.camera_type(), fe.camera_type);
    if let Some(saved) = restore {
        fe.note(format!("race start (0x82399DA8): the camera's INT_CameraFallbackType = CTYP {saved}"));
        fe.camera_restore = Some(saved);
    }
    values.set_bool(SPLITS_CALLOUT_FORCE_OFF, reverse);
}

fn stored_camera_type(camera_type: Option<i32>) -> Option<i32> {
    camera_type.filter(|t| SAVED_CAMERA_TYPES.contains(t))
}

fn camera_restore(saved: i32, camera_type: Option<i32>) -> (Option<i32>, bool) {
    let reverse = camera_type == Some(CAMERA_TYPE_REVERSE);
    let restore = (saved >= 0 && SAVED_CAMERA_TYPES.contains(&saved) && !reverse).then_some(saved);
    (restore, reverse)
}

pub fn front_end_entered(fe: &mut FrontEnd, env: &mut FeEnv) {
    front_end_frame_state_begins(fe, env);
    fe.note("the front end's frame state begins: 0xCE72F936 {2} → 0x824079A8: A_GameQuitComplete");
    fe.game_actions.push(("A_GameQuitComplete".into(), super::ActionParam::default()));
}

pub fn front_end_frame_state_begins(fe: &mut FrontEnd, env: &mut FeEnv) {
    if front_end_meta_values(env.values, &mut fe.hud_ready_seen) {
        fe.messages.push(super::FeMessage::PopupState(1));
    }
    fe.note("0xCC21B341 → 0x82381C88: HUD_NONISPROMPT (0x11A) and HUD_NISTitleEnabled (0x117) on");
    fe.busy.release(env, super::busy::LEVEL);
}

pub fn front_end_meta_values(values: &mut crate::game::values::UiValues, hud_ready_seen: &mut bool) -> bool {
    use crate::game::values::id;
    let due = std::mem::take(hud_ready_seen);
    values.set_bool(id::NO_NIS_PROMPT, true);
    values.set_bool(id::NIS_TITLE_ENABLED, true);
    due
}

pub fn hud_ready(fe: &mut FrontEnd, _env: &mut FeEnv) {
    fe.hud_ready_seen = true;
}

pub fn game_quit_complete(fe: &mut FrontEnd, env: &mut FeEnv) {
    fe.note("A_GameQuitComplete: FENotificationCompleteUnloadingGameplay (0x0541811E)");
    if fe.game_mode == 3 {
        let text = env.loc.text("TXT_FAILED_SIGN_IN_TO_FIRST_PARTY_NETWORK");
        env.values.set_str(super::popup::POPUP_BODY, text);
        env.nav_events.push("globalevent_quitstage_disconnect".into());
        return;
    }
    quit_or_quick_select(fe, env);
}

fn quit_or_quick_select(fe: &mut FrontEnd, env: &mut FeEnv) {
    if fe.menu_selection != 4 {
        super::progression::quit_return(fe, env);
    } else {
        fe.go_to_screen(env, screens::id::QUICKTRACK);
    }
}

pub fn game_quit(fe: &mut FrontEnd, env: &mut FeEnv) {
    fe.note("FE manager +0x40 (message 0xB0E2239A)");
    consume_mods(fe, env);
    fe.level_start = LevelStart::default();
    fe.busy.add(env, super::busy::LEVEL);
    fe.requests.push(super::pause::GameRequest::Quit);
    fe.in_drop = false;
    fe.launch = None;
    fe.game_pause.reset();
    fe.simulation = false;
    quit_or_quick_select(fe, env);
}

pub fn campaign_drop_completed(values: &mut crate::game::values::UiValues, drop: u32) {
    if drop == 0x12A3_5AB4 {
        values.set_bool(0xA2, true);
        values.set_bool(0xA3, true);
    }
    if drop == super::globe::FINAL_DROP {
        values.set_bool(0x57, true);
    }
}

pub fn results_advance(fe: &mut FrontEnd, env: &mut FeEnv, success: bool) {
    env.values.set_int(0xA5, 0);
    if fe.game_mode != 0 && fe.game_mode != 3 {
        if !fe.unlocks.is_empty() {
            fe.go_to_screen(env, screens::id::UNLOCKS);
            return;
        }
        if fe.game_mode == 1 {
            if success {
                if env.values.bool(0x57) {
                    super::unlocks::end_of_campaign_hint(fe, env);
                    fe.results_hint_wait = true;
                } else {
                    fe.note("FENotificationPendingQuitBE (0xCFFD4E8B)");
                    fe.go_to_screen(env, screens::id::CONTINUE_STAGE);
                }
                return;
            }
            let drop = fe.current_drop;
            let c = fe.profile.drop_counts.entry(drop).or_default();
            if c.completions == 0 && c.failures > 2 && c.unk_flag == 0 && drop != super::globe::FINAL_DROP {
                c.unk_flag = 1;
                env.nav_events.push("go_to_skip_race_popup".into());
                return;
            }
            env.values.set_int(0xA5, 1);
            fe.go_to_screen(env, screens::id::EXPLORE_POST_MENU);
            return;
        }
    }
    env.values.set_int(0xA5, 0x19);
    fe.go_to_screen(env, screens::id::EXPLORE_POST_MENU);
}

pub fn do_skip_race(fe: &mut FrontEnd, env: &mut FeEnv) {
    let drop = fe.current_drop;
    let before = fe.profile.drop_state(drop);
    let reward = super::globe::drop_reward(fe, drop);
    let notices = fe.profile.skip_campaign_drop(&fe.rules, drop);
    fe.profile.add_credits(reward, true);
    let adj = env.values.int(0x93);
    env.values.set_int(0x93, adj + reward as i32);
    if before == ssx_assets::career::profile::DropState::Unlocked
        && fe.profile.drop_state(drop) == ssx_assets::career::profile::DropState::Completed
    {
        campaign_drop_completed(env.values, drop);
    }
    fe.last_result_drop = drop;
    fe.note(format!(
        "skip race {drop:#010x}: {} notices, the FE manager's +0xA0({reward}, earned, animated)",
        notices.len()
    ));
    for n in &notices {
        let items = super::unlocks::items_of(n, &fe.career, env.loc);
        fe.unlocks.extend(items);
    }
    fe.last_success = Some(true);
    let screen = if fe.unlocks.is_empty() { screens::id::CONTINUE_STAGE } else { screens::id::UNLOCKS };
    fe.go_to_screen(env, screen);
}

pub fn results_posted(fe: &mut FrontEnd, env: &mut FeEnv, death: bool) {
    env.values.set_bool(0x1B2, false);
    fe.note(format!("FEShowResultsScreenMessage {{{}}}", if death { 4 } else { 2 }));
    fe.go_to_screen(env, if death { 0x27 } else { screens::id::EXPLORE_RESULTS });
    // TODO(guess): the upload request's offline end (its caller of 0x8247C180) is taken to come at once; settle: that caller's timing offline.
    env.values.set_bool(0x1B2, true);
    env.values.set_bool(0x1B3, true);
    fe.note("upload done offline (0x8247C180): MODE_ispostcompleted, MODE_ispostsuccessful");
}

pub fn results_update(fe: &mut FrontEnd, env: &mut FeEnv) {
    if env.values.is_dirty(0x1B2) {
        fe.publish.push("dp_race_results_upload".into());
    }
    if fe.results_hint_wait && env.values.bool(super::hint::HINT_DONE) {
        fe.results_hint_wait = false;
        fe.note("the end-of-campaign hint is done: FENotificationPendingQuitBE (0xCFFD4E8B), screen 0x41");
        fe.go_to_screen(env, screens::id::CONTINUE_STAGE);
    }
}

pub fn evaluate_previous(fe: &mut FrontEnd, env: &mut FeEnv, args: &[String]) {
    let mode = fe.game_mode;
    let index = if args.len() < 2 {
        0
    } else {
        match fe.screen {
            0x30 => match mode {
                3 => 1,
                1 => 2,
                _ => 0,
            },
            0x1E | 0x22..=0x26 | 0x28 | 0x37 | 0x40 => match mode {
                3 => 3,
                1 => 5,
                _ => 2,
            },
            0x45 if mode == 3 => 1,
            _ => 0,
        }
    };
    let Some(event) = args.get(index) else { return };
    match screens::SCREENS.iter().position(|s| s.0 == event.as_str()) {
        Some(screen) => {
            fe.note(format!("evaluatePrevious → {event}"));
            fe.go_to_screen(env, screen as u32);
        }
        None => env.nav_events.push(event.clone()),
    }
}

pub fn misc_nav(fe: &mut FrontEnd, env: &mut FeEnv, name: &str, arg: &str) {
    match name {
        "enterGameplay" => {
            fe.note("FENotificationEnterGameplay (0x886EE698)");
            env.values.set_bool(0x1AE, false);
        }
        "enterFE" => {
            if fe.current_drop != 0 {
                for movie in std::mem::take(&mut fe.profile.movie_queue) {
                    fe.note(format!("FLOW_PlayMovie {movie} (story, enterFE)"));
                    fe.queue_movie(super::MovieRequest::new(movie, "story"));
                }
            }
        }
        "setGlobeInputEnabled" => fe.globe.input = arg == "true",
        "activityfeedwelcomestart" => fe.note("FEActivityWelcomeStart (0xE3455D04)"),
        _ => fe.note(format!("nav action {name}({arg})")),
    }
}

pub fn update(fe: &mut FrontEnd, env: &mut FeEnv, _dt: f64) {
    if !fe.profile.new_game_active() || fe.profile.new_game_state == 0 {
        return;
    }
    let state = FlowState {
        hint_done: env.values.bool(super::hint::HINT_DONE),
        front_end_idle: true,
        movie_playing: fe.movie.is_some(),
        in_drop: fe.in_drop,
        screen: fe.screen,
        screen_busy: false,
    };
    let before = fe.profile.new_game_state;
    let actions = fe.profile.new_game_update(&fe.rules, &state);
    if fe.profile.new_game_state != before {
        fe.note(format!("new game: state {before:#x} → {:#x}", fe.profile.new_game_state));
    }
    for a in actions {
        match a {
            FlowAction::ShowHint(h) => {
                env.values.set_bool(super::hint::HINT_DONE, false);
                fe.note(format!("new game: message 0x716B0EC4 (tutorial hint {h:#010x})"));
                super::hint::campaign_hint(fe, env, h);
            }
            FlowAction::GlobeFocus { drop, kind } => {
                env.values.set_int(0x2E, 1);
                fe.note(format!("FEGlobeGoToNode {{{drop:#010x}, {kind}}}"));
                let (peak, range) = fe.career.event(drop).map(|e| fe.career.peak_and_range(e)).unwrap_or((0, 0));
                fe.globe.drop = drop;
                fe.globe.peak = peak;
                fe.globe.range = range;
            }
            FlowAction::PlayMovie(m) => {
                fe.note(format!("FLOW_PlayMovie {m} (new game)"));
                fe.queue_movie(MovieRequest::new(m, "new_game"));
            }
            FlowAction::GlobeProgression { from, to, flag } => {
                fe.note(format!("Campaign_GlobeProgression from {from:#010x} to {to:x?} ({flag})"));
                fe.profile.campaign_progression(&fe.rules, from, &to, flag);
            }
            FlowAction::LaunchDrop(d) => {
                fe.messages.push(super::FeMessage::LoadoutRefresh);
                let rider = ssx_formats::attrib::fnv1(ssx_assets::career::world_tour::NEW_GAME_RIDER.as_bytes());
                super::equipment::flow_equip(fe, rider, 0);
                for key in ssx_assets::career::world_tour::NEW_GAME_ITEMS {
                    super::equipment::flow_equip(fe, key, 0);
                }
                launch(fe, env, d, true);
            }
            FlowAction::GotoScreen(s) => {
                fe.note(format!("FEGenericNavMsg {{{s:#x}}}"));
                fe.go_to_screen(env, s);
            }
            FlowAction::LockInput(b) => {
                if b {
                    fe.note("new game: save (FE manager +0xC4)");
                    fe.requests.push(super::pause::GameRequest::Save);
                }
                fe.note(format!("Input_{} (slot 0)", if b { "Lock" } else { "Unlock" }))
            }
        }
    }
    if fe.profile.new_game_state == NEW_GAME_DONE {
        fe.note("new game done");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_camera_type_is_kept_and_restored() {
        assert_eq!(stored_camera_type(Some(5)), Some(5));
        assert_eq!(stored_camera_type(Some(4)), Some(4));
        assert_eq!(stored_camera_type(Some(0)), Some(0));
        assert_eq!(stored_camera_type(Some(CAMERA_TYPE_REVERSE)), None, "a drop's reverse camera is not kept");
        assert_eq!(stored_camera_type(Some(6)), None, "nor the tutorial's");
        assert_eq!(stored_camera_type(None), None);
        assert_eq!(camera_restore(5, Some(4)), (Some(5), false), "after the hand-over's 15P intro type");
        assert_eq!(camera_restore(0, Some(4)), (Some(0), false));
        assert_eq!(camera_restore(-1, Some(4)), (None, false), "no saved type");
        assert_eq!(camera_restore(6, Some(4)), (None, false));
        assert_eq!(camera_restore(5, Some(CAMERA_TYPE_REVERSE)), (None, true), "the reverse camera keeps its type");
    }
}
