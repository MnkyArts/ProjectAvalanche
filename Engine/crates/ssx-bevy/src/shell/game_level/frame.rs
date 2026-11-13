use bevy::prelude::*;
use ssx_ui::game::GameUi;
use ssx_ui::game::fe::host::Step;
use ssx_ui::game::fe::pause::GameRequest;
use ssx_ui::game::input::{Button, Input};

use super::LevelGame;
use super::results::{RideOutcome, post_results, stats_from_ride, sync_named_flags};
use crate::audio::music::frontend::FrontEndAudio;
use crate::ride::Ride;
use crate::shell::game_mode::{self, GameUiRes, UiPipelinesReady};
use crate::ui;

const AUTOPLAY_POPUP_SECONDS: f64 = 2.0;

const SCRIPT_A_HOLD: f64 = 0.1;

// TODO(guess): one UI frame per link (gate update, loading switch + step, simulation switch); settle: the main loop's frame-state request reader (0x828954A8, `[0x838F1E24]`).
const RESTART_FRAMES: u32 = 3;

const DRAWN_FRAMES: u32 = 20;

#[allow(clippy::too_many_arguments)]
pub fn level_frame(
    mut commands: Commands,
    time: Res<Time>,
    mut keys: ResMut<ButtonInput<KeyCode>>,
    (pads, slots): (Query<(Entity, &Gamepad)>, Res<game_mode::PadSlots>),
    level: Option<ResMut<LevelGame>>,
    ride: Option<ResMut<Ride>>,
    mut ui: ResMut<GameUiRes>,
    ready: Res<UiPipelinesReady>,
    meshes: Query<(), With<Mesh3d>>,
    mut audio: Option<ResMut<FrontEndAudio>>,
    (ui_frame, mut world_check): (Res<ui::UiFrame>, ResMut<game_mode::WorldCheck>),
) {
    let (Some(mut level), Some(mut ride)) = (level, ride) else { return };
    let l = &mut *level;
    let ui = &mut ui.0;
    let dt = time.delta_secs_f64();
    if !l.up {
        l.ready_frames = if ready.0 && !meshes.is_empty() { l.ready_frames + 1 } else { 0 };
        if l.ready_frames >= DRAWN_FRAMES && !ui.views_loading() {
            l.up = true;
            ride.hold = false;
            ui.values.set_bool(ssx_ui::game::values::id::READY, true);
            ui.fe_call(ssx_ui::game::fe::pads::hud_ready);
            ui.fe_call(ssx_ui::game::fe::flow::hud_ready);
            // PORT(choice): the world drawn for `DRAWN_FRAMES` frames stands for the camera's streaming query finding
            // its entries resident (0x824D1998 +0x74, 0x82980620; 0x824D0A00 / 0x824D1AC8 only place it in the level's
            // start); the port does not stream the level from a disc.
            ui.fe_call(ssx_ui::game::fe::flow::stream_ready);
            if let Some(a) = audio.as_mut() {
                a.gameplay_start();
            }
            info!("level: drawn after {:.1} s: go_to_hud, the ride starts", l.spawned.elapsed().as_secs_f32());
            if ui.nav.is_active("backgroundcontainer_on") {
                error!("level: the boot's background view is still loaded: it covers the level's world");
            }
        }
        return;
    }
    if ui.fe.as_ref().is_some_and(|f| f.level_start.query) {
        info!("level: the survive loop's streaming report at ride time {:.2} s", ride.seconds());
        ui.fe_call(ssx_ui::game::fe::flow::stream_ready);
    }
    let badges = ride.session.stats.awarded[2].len();
    if badges != l.badges_seen {
        l.badges_seen = badges;
        if l.live {
            info!("level: a network session's ride awarded a badge: not into the profile, no notification");
        } else if l.launch.map.is_some() {
            info!("level: a map's ride awarded a badge: not into the profile, no notification");
        } else {
            stats_from_ride(ui, &ride, &l.stat_names);
        }
    }
    sync_named_flags(l, &mut ride, ui);
    for id in [ssx_ui::game::values::id::READY, ssx_ui::game::values::id::RACE_STARTED] {
        ride.session.set_ui_flag(id, ui.values.bool(id));
    }
    let secs = ride.seconds();
    l.ride_seconds = secs;
    l.frame_ms.push(dt * 1000.0);
    if secs >= l.next_log {
        let n = l.frame_ms.len().min(300);
        let recent = &l.frame_ms[l.frame_ms.len() - n..];
        let avg = recent.iter().sum::<f64>() / n.max(1) as f64;
        let max = recent.iter().copied().fold(0.0, f64::max);
        let m = l.ui_ms.len().min(300);
        let ui_avg = l.ui_ms[l.ui_ms.len() - m..].iter().sum::<f64>() / m.max(1) as f64;
        info!(
            "level: {secs:.0} s: frame time {avg:.2} ms average, {max:.1} ms max over the last {n} frames; the UI's \
             CPU {ui_avg:.2} ms"
        );
        l.next_log += 10.0;
    }
    while l.shots.first().is_some_and(|(t, _)| secs >= *t) {
        let (_, path) = l.shots.remove(0);
        if let Some(op) = game_mode::ui_cover(&ui_frame) {
            world_check.covered_by_ui(&path, op);
        }
        game_mode::screenshot(&mut commands, path, true);
    }
    l.clock += dt;
    match l.prompts.as_mut() {
        Some(p) if l.results_posted => p.release(keys.bypass_change_detection()),
        Some(p) => {
            for name in p.frame(l.clock, ui, keys.bypass_change_detection()) {
                game_mode::screenshot(&mut commands, l.shot_dir.join(format!("{name}.png")), false);
            }
        }
        None => {}
    }
    if l.takes_input() {
        pause_input(&mut commands, l, ui, keys.bypass_change_detection(), &pads, &slots);
    } else if l.live && l.results_posted {
        results_script(&mut commands, l, ui);
    }
    for r in std::mem::take(&mut l.requests) {
        match r {
            GameRequest::Restart => {
                ride.hold = true;
                l.restart_at = Some(l.ui_frames + RESTART_FRAMES);
            }
            GameRequest::Quit => ride.hold = true,
            _ => {}
        }
    }
    l.ui_frames += 1;
    if l.restart_at.is_some_and(|f| l.ui_frames >= f) {
        l.restart_at = None;
        if std::mem::take(&mut l.paused)
            && let Some(a) = audio.as_mut()
        {
            a.game_pause(false);
        }
        ride.respawn();
        l.feed = l.feed0.clone();
        l.sim_hold = None;
        l.ghosts = l.ghosts0.clone();
        l.result = None;
        l.results_posted = false;
        let launch = l.launch.clone();
        let name = ui.fe.as_ref().and_then(|fe| ssx_ui::game::fe::flow::range_visit_name(fe, &launch));
        if let Some(n) = ui.fe.as_mut().and_then(|fe| ssx_ui::game::fe::flow::count_range_visit(fe, &launch)) {
            info!("level: the restart's game simulation counts visit {n} of the range by {}", launch.rider);
        }
        if let Some(name) = name {
            let key = ssx_formats::attrib::fnv1(name.as_bytes());
            ride.session.stats.add_int_key(key, ssx_physics::stats::PERSISTENT, 1, true);
        }
        ui.values.set_bool(ssx_ui::game::values::id::READY, true);
        ui.fe_call(ssx_ui::game::fe::pads::hud_ready);
        ui.fe_call(ssx_ui::game::fe::flow::hud_ready);
        ui.fe_call(ssx_ui::game::fe::flow::stream_ready);
        ride.hold = false;
        info!("level: the event starts over");
    }
    if !l.stopped && l.result.is_none() && l.stop_after.is_some_and(|s| secs >= s) {
        l.stopped = true;
        info!("level: the ride's time limit ({secs:.1} s): go_to_quit_stage");
        ui.nav.raise("go_to_quit_stage");
    }
    if l.live
        && !l.live_rounds
        && l.restart_at.is_none()
        && let Some(r) = l.result.as_ref()
    {
        info!(
            "level: a network session's run ended (time {} ms, points {}, failed {}): no results; the run starts over",
            r.time_ms, r.points, r.failed
        );
        ride.hold = true;
        ui.fe_call(ssx_ui::game::fe::pause::do_restart);
    }
    if !l.live
        && !l.results_posted
        && let Some(r) = l.result.clone()
    {
        l.results_posted = true;
        let posted = if l.launch.map.is_some() {
            let event_type = l.launch.event_type as i32;
            let mode = ride.map.as_ref().and_then(|m| Some((m.id.to_string(), m.mode.as_ref()?)));
            let run = mode.as_ref().map(|(map, mode)| ssx_ui::game::ride::MapRun {
                map,
                mode: &mode.name,
                rules: &mode.rules,
                gamemode: mode.gamemode.is_some(),
                sim_content_id: ride.sim_content_id(),
            });
            let posted = ssx_ui::game::ride::post_map_results(ui, event_type, &l.launch.rider, &r, run.as_ref());
            if r.failed {
                ssx_ui::game::ride::post_death_body(ui, ride.session.event.setup.course, &r);
            }
            posted
        } else {
            stats_from_ride(ui, &ride, &l.stat_names);
            let bonuses = ride.session.stats.track_bonuses();
            if !bonuses.is_empty() {
                info!("level: track bonuses awarded: {bonuses:?}");
            }
            let splits = ride.session.event.explore_splits().unwrap_or_default();
            post_results(
                ui,
                &l.launch,
                &r,
                ride.session.event.setup.course,
                RideOutcome {
                    track_bonuses: bonuses,
                    bot_scores: l.bot_scores.clone(),
                    splits,
                    sim_content_id: ride.sim_content_id().map(str::to_string),
                },
            )
        };
        if let Some(why) = posted.best_not_kept {
            ssx_assets::content::data::simid::best_not_kept(ride.big_root(), &l.launch.event, why);
        }
        let won = posted.won;
        l.save_due = true;
        let summary = ui.fe.as_ref().map_or_else(String::new, |f| game_mode::summary(&f.profile));
        info!(
            "level: results posted ({}, medal {:?} against the rows {:?}): {summary}",
            if won { "won" } else { "not won" },
            posted.medal,
            r.rivals
        );
        let death = r.failed;
        ui.fe_call(|fe, env| ssx_ui::game::fe::flow::results_posted(fe, env, death));
    }
}

fn results_script(commands: &mut Commands, l: &mut LevelGame, ui: &mut GameUi) {
    for (b, device) in std::mem::take(&mut l.release) {
        ui.input.push_from(Input::Button { button: b, down: false }, device);
    }
    while l.script.first().is_some_and(|(t, _)| *t <= l.clock) {
        match l.script.remove(0).1 {
            Step::Press(b) => {
                info!("level: press {b:?} (script, under the round's results), UI frame {}", l.ui_frames);
                ui.input.push_from(Input::Button { button: b, down: true }, ssx_ui::game::input::SCRIPT_DEVICE);
                l.release.push((b, ssx_ui::game::input::SCRIPT_DEVICE));
            }
            Step::Shot(name) => game_mode::screenshot(commands, l.shot_dir.join(format!("{name}.png")), false),
            step => info!("level: {step:?} (script) is not taken under the round's results"),
        }
    }
}

fn pause_input(
    commands: &mut Commands,
    l: &mut LevelGame,
    ui: &mut GameUi,
    keys: &mut ButtonInput<KeyCode>,
    pads: &Query<(Entity, &Gamepad)>,
    slots: &game_mode::PadSlots,
) {
    for (b, device) in std::mem::take(&mut l.release) {
        ui.input.push_from(Input::Button { button: b, down: false }, device);
    }
    if l.script_a_until.is_some_and(|t| l.clock >= t) {
        keys.release(KeyCode::Space);
        l.script_a_until = None;
    }
    if l.script_back_until.is_some_and(|t| l.clock >= t) {
        keys.release(KeyCode::KeyC);
        l.script_back_until = None;
    }
    if let Some((key, _)) = l.script_bumper_until.filter(|(_, t)| l.clock >= *t) {
        keys.release(key);
        l.script_bumper_until = None;
    }
    let typing = ui.text_focused();
    let pad_start = pads
        .iter()
        .find(|(_, p)| p.just_pressed(GamepadButton::Start))
        .and_then(|(e, _)| slots.device(e))
        .or((!typing && keys.just_pressed(KeyCode::Escape)).then_some(ssx_ui::game::input::KEYBOARD_DEVICE));
    let mut start = pad_start.is_some();
    let mut script_presses = Vec::new();
    while l.script.first().is_some_and(|(t, _)| *t <= l.clock) {
        match l.script.remove(0).1 {
            Step::Press(b @ (Button::DigitalOne | Button::Select)) if typing => {
                info!("level: press {b:?} (script, the on-screen keyboard's), UI frame {}", l.ui_frames);
                script_presses.push(b);
            }
            Step::Press(Button::DigitalOne) => {
                info!("level: press DigitalOne (script: Space, the UI's and the ride's A), UI frame {}", l.ui_frames);
                keys.press(KeyCode::Space);
                l.script_a_until = Some(l.clock + SCRIPT_A_HOLD);
            }
            Step::Press(Button::Select) => {
                info!("level: press Back (script: C, the ride's Back), UI frame {}", l.ui_frames);
                keys.press(KeyCode::KeyC);
                l.script_back_until = Some(l.clock + SCRIPT_A_HOLD);
            }
            Step::Press(b @ (Button::LeftBumper | Button::RightBumper)) => {
                let key = if b == Button::LeftBumper { KeyCode::KeyQ } else { KeyCode::KeyE };
                info!("level: press {b:?} (script: the UI's and the keyboard's {key:?}), UI frame {}", l.ui_frames);
                keys.press(key);
                l.script_bumper_until = Some((key, l.clock + SCRIPT_A_HOLD));
                script_presses.push(b);
            }
            Step::Press(b) => {
                info!("level: press {b:?} (script), UI frame {}", l.ui_frames);
                start |= b == Button::Start;
                script_presses.push(b);
            }
            Step::Shot(name) => game_mode::screenshot(commands, l.shot_dir.join(format!("{name}.png")), false),
            Step::Nav(e) => {
                info!("level: nav event {e} (script)");
                ui.nav.raise(&e);
            }
            Step::TutorialPopup(text) => {
                info!("level: DO_TutorialPopup({text}) (script)");
                ui.tutorial_popup(&text);
            }
            Step::Chat(quick) => {
                let opened = ui.chat_open(quick);
                info!("level: the chat key (quick {quick}, script): {}", if opened { "opened" } else { "not opened" });
            }
            Step::Text(t) => {
                info!("level: type {t:?} (script)");
                for c in t.chars() {
                    ui.text_input(ssx_ui::game::fe::ava::keyboard::TextInput::Char(c));
                }
            }
            Step::TextKey(k) => {
                info!("level: key {k:?} (script)");
                ui.text_input(k);
            }
            step @ (Step::Pad(_) | Step::Focus(_)) => {
                info!("level: {step:?} (script) at ride UI frame {}", l.ui_frames);
                ssx_ui::game::fe::host::script_interrupt(ui, &step);
            }
            _ => {}
        }
    }
    let mut presses: Vec<(Button, u8)> = game_mode::pad_presses(keys, pads, slots, &mut l.stick_dpad, false)
        .into_iter()
        .filter(|&(b, device)| b != Button::Start && !(typing && device == ssx_ui::game::input::KEYBOARD_DEVICE))
        .collect();
    if let Some(device) = pad_start {
        presses.push((Button::Start, device));
    }
    presses.extend(script_presses.into_iter().map(|b| (b, ssx_ui::game::input::SCRIPT_DEVICE)));
    if l.autoplay {
        let popup = ui.fe.as_ref().filter(|f| f.tutorial_pause.shown && !f.tutorial_pause.hidden);
        let text = popup.and_then(|f| f.tutorial_pause.current.as_ref()).map(|p| p.message.clone());
        if text != l.popup_text {
            (l.popup_text, l.popup_since, l.popup_presses) = (text, None, 0);
        }
        match (l.popup_text.is_some(), l.popup_since) {
            (true, None) => l.popup_since = Some(l.clock),
            (true, Some(since)) if l.clock - since >= AUTOPLAY_POPUP_SECONDS => {
                info!("level: press DigitalOne (autoplay: the tutorial popup)");
                presses.push((Button::DigitalOne, ssx_ui::game::input::SCRIPT_DEVICE));
                l.popup_since = None;
                l.popup_presses += 1;
            }
            _ => {}
        }
    }
    for (b, device) in presses {
        ui.input.push_from(Input::Button { button: b, down: true }, device);
        l.release.push((b, device));
    }
    ui.game_input(start && !ui.chat_held());
}
