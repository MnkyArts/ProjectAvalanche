use std::time::Instant;

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use super::{
    GameUiRes, LAUNCH_BOOT_WAIT, PadSlots, Shell, Stage, UiPipelinesReady, WorldCheck, pad_presses, screenshot,
};
use crate::audio::music::frontend::FrontEndAudio;
use crate::shell::game_level;
use crate::ui;

#[derive(SystemParam)]
pub(super) struct TextKeys<'w, 's> {
    typed: MessageReader<'w, 's, bevy::input::keyboard::KeyboardInput>,
    alt_enter: Res<'w, super::display::AltEnter>,
    ime: MessageReader<'w, 's, bevy::window::Ime>,
    windows: Query<'w, 's, &'static mut Window, With<bevy::window::PrimaryWindow>>,
    clipboard: Option<ResMut<'w, bevy::clipboard::Clipboard>>,
}

pub(super) fn shell_input(
    keys: Res<ButtonInput<KeyCode>>,
    pads: Query<(Entity, &Gamepad)>,
    slots: Res<PadSlots>,
    mut shell: ResMut<Shell>,
    mut ui: ResMut<GameUiRes>,
    level: Option<Res<game_level::LevelGame>>,
    mut text: TextKeys,
) {
    let s = &mut *shell;
    let focused = s.started && ui.text_focused();
    if let Ok(mut w) = text.windows.single_mut()
        && w.ime_enabled != focused
    {
        w.ime_enabled = focused;
    }
    if focused {
        let ctrl = [KeyCode::ControlLeft, KeyCode::ControlRight, KeyCode::SuperLeft, KeyCode::SuperRight]
            .iter()
            .any(|k| keys.pressed(*k));
        text_input(&mut text, &mut ui, ctrl);
        if ctrl
            && keys.just_pressed(KeyCode::KeyV)
            && let Some(clipboard) = text.clipboard.as_deref_mut()
        {
            paste(clipboard, &mut ui);
        }
    } else {
        text.typed.clear();
        text.ime.clear();
    }
    if !s.started || level.is_some_and(|l| l.takes_input()) {
        return;
    }
    let keyboard = ssx_ui::game::input::KEYBOARD_DEVICE;
    let search = ui.browser_active();
    for (b, device) in pad_presses(&keys, &pads, &slots, &mut s.stick_dpad, search) {
        if focused && device == keyboard {
            continue;
        }
        s.host.press_from(&mut ui, b, device);
    }
    let mut stick = Vec2::ZERO;
    for (_, pad) in &pads {
        stick +=
            Vec2::new(pad.get(GamepadAxis::LeftStickX).unwrap_or(0.0), pad.get(GamepadAxis::LeftStickY).unwrap_or(0.0));
    }
    let key = |k: KeyCode| if keys.pressed(k) && !focused { 1.0 } else { 0.0 };
    stick += Vec2::new(key(KeyCode::KeyD) - key(KeyCode::KeyA), key(KeyCode::KeyW) - key(KeyCode::KeyS));
    if (stick != Vec2::ZERO || s.last_stick != Vec2::ZERO)
        && let Some(fe) = ui.fe.as_mut()
    {
        ssx_ui::game::fe::globe::stick(fe, stick.x, stick.y);
    }
    s.last_stick = stick;
    if keys.just_pressed(KeyCode::F1) && s.movie.is_some() {
        s.movie = None;
        s.movie_ended = ui.fe.as_ref().map(|x| x.movie_serial);
        s.host.finish_movie(&mut ui);
    }
}

fn paste(clipboard: &mut bevy::clipboard::Clipboard, ui: &mut GameUiRes) {
    use ssx_ui::game::fe::ava::keyboard::TextInput as T;
    match clipboard.fetch_text().poll_result() {
        Some(Ok(text)) => {
            let typed = text.chars().filter(|c| !c.is_control()).take(MAX_PASTE);
            typed.for_each(|c| ui.text_input(T::Char(c)));
        }
        Some(Err(e)) => warn!("shell: the clipboard: {e}"),
        None => {}
    }
}

const MAX_PASTE: usize = ssx_ui::game::fe::ava::keyboard::MAX_LEN;

fn text_input(text: &mut TextKeys, ui: &mut GameUiRes, ctrl: bool) {
    use bevy::input::ButtonState;
    use ssx_ui::game::fe::ava::keyboard::TextInput as T;
    for e in text.alt_enter.typed(text.typed.read_with_id()) {
        if ctrl {
            continue;
        }
        if e.state == ButtonState::Pressed {
            for i in key_text(e.key_code, e.text.as_deref()) {
                ui.text_input(i);
            }
        }
    }
    for e in text.ime.read() {
        if let bevy::window::Ime::Commit { value, .. } = e {
            value.chars().filter(|c| !c.is_control()).for_each(|c| ui.text_input(T::Char(c)));
        }
    }
}

// PORT(choice): Enter confirms and Escape cancels a text field, Backspace / Delete / the arrows / Home / End edit it;
// the game has no text entry and no keyboard text.
pub(super) fn key_text(key: KeyCode, text: Option<&str>) -> Vec<ssx_ui::game::fe::ava::keyboard::TextInput> {
    use ssx_ui::game::fe::ava::keyboard::TextInput as T;
    let edit = match key {
        KeyCode::Backspace => T::Backspace,
        KeyCode::Delete => T::Delete,
        KeyCode::ArrowLeft => T::Left,
        KeyCode::ArrowRight => T::Right,
        KeyCode::Home => T::Home,
        KeyCode::End => T::End,
        KeyCode::Enter | KeyCode::NumpadEnter => T::Enter,
        KeyCode::Escape => T::Escape,
        _ => return text.unwrap_or("").chars().filter(|c| !c.is_control()).map(T::Char).collect(),
    };
    vec![edit]
}

#[derive(SystemParam)]
pub(super) struct Frame<'w, 's> {
    commands: Commands<'w, 's>,
    time: Res<'w, Time>,
    real: Res<'w, Time<Real>>,
    scene: ResMut<'w, ui::UiScene>,
    images: ResMut<'w, Assets<Image>>,
    sprites: Query<'w, 's, (&'static mut Sprite, &'static mut Visibility)>,
    cameras: Query<'w, 's, &'static mut Camera>,
    windows: Query<'w, 's, &'static Window, With<bevy::window::PrimaryWindow>>,
    ready: Res<'w, UiPipelinesReady>,
    ui_frame: Res<'w, ui::UiFrame>,
    globe: ResMut<'w, ui::globe::GlobeView>,
    fade: ResMut<'w, ui::fade::ScreenFade>,
    level: Option<ResMut<'w, game_level::LevelGame>>,
    ride: Option<Res<'w, crate::ride::Ride>>,
    audio: Option<ResMut<'w, FrontEndAudio>>,
    world_check: Res<'w, WorldCheck>,
    exit: MessageWriter<'w, AppExit>,
    live: Option<ResMut<'w, crate::shell::live::LiveSession>>,
    render_scale: Option<Res<'w, super::display::RenderScale>>,
}

pub(super) fn shell_step(mut shell: ResMut<Shell>, mut ui: ResMut<GameUiRes>, mut f: Frame) {
    let s = &mut *shell;
    if let Some(ms) = s.frame_sleep {
        std::thread::sleep(std::time::Duration::from_millis(ms));
    }
    for n in std::mem::take(&mut s.notes) {
        if n.contains("ERROR") {
            error!("{n}");
        } else {
            info!("{n}");
        }
    }
    if f.ui_frame.viewport.y > 0 {
        let aspect = f.ui_frame.viewport.x as f32 / f.ui_frame.viewport.y as f32;
        f.globe.frame = match s.stage {
            Stage::Level => None,
            _ => ui.fe.as_ref().and_then(|fe| fe.globe_frame(aspect)),
        };
    }
    if let Some(window) = f.windows.iter().next() {
        let now = f.real.elapsed_secs_f64();
        super::display::display_class_frame(&mut s.display_pending, s.started, &mut ui, &mut f.scene, window, now);
    }
    let drawn = f.ready.0 && f.ui_frame.viewport.x > 0;
    s.ready_frames = if drawn { s.ready_frames + 1 } else { 0 };
    if !s.started {
        s.started = s.ready_frames >= 5;
        if !s.started {
            return;
        }
    }
    if s.hold > 0 {
        s.hold -= 1;
        if s.hold == 0 && !s.pending_shots.is_empty() {
            for name in std::mem::take(&mut s.pending_shots) {
                screenshot(&mut f.commands, s.shot_dir.join(format!("{name}.png")), false);
            }
            s.hold = 3;
        }
        return;
    }
    if let Some(n) = s.stats_in.as_mut() {
        *n = n.saturating_sub(1);
        if *n == 0 {
            s.stats_in = None;
            s.op_stats = true;
        }
    }
    let dt = f.time.delta_secs_f64().min(0.1);
    if !f.level.as_ref().is_some_and(|l| l.takes_input()) {
        let shots = s.host.frame(&mut ui, dt);
        if !shots.is_empty() {
            s.pending_shots = shots;
            s.hold = 2;
        }
        if !std::mem::take(&mut s.host.displays).is_empty() {
            warn!(
                "shell: --script display=…: the game's display follows its window (resize it or Alt+Enter); not taken"
            );
        }
    }
    if f.level.as_ref().is_some_and(|l| l.results_shown()) {
        s.next_segment();
    }
    if let Some(live) = f.live.as_deref_mut()
        && s.live_frame(&mut ui, f.level.as_deref(), live)
    {
        f.commands.remove_resource::<crate::shell::live::LiveSession>();
        #[cfg(feature = "net")]
        crate::net::finished(&mut f.commands);
    }
    #[cfg(feature = "net")]
    if s.link.is_some()
        && ui.fe.as_ref().is_some_and(|fe| fe.ava.hub.active)
        && !ui.views_loading()
        && !ui.name_prompt_waiting()
        && let Some(address) = s.link.take()
    {
        info!("shell: the link's server {address}: the browser asks the player");
        ui.open_link(address);
        ui.nav.raise(ssx_ui::game::fe::ava::browser::OPEN);
    }
    if s.stage == Stage::Menus
        && let Some((name, at)) = s.test_launch.clone()
        && s.host.time >= at
    {
        let screen = ui.fe.as_ref().map(|fe| fe.screen);
        let menu = !ui.nav.is_active("backgroundcontainer_on") && screen != Some(ssx_ui::game::fe::screens::id::BOOT);
        if menu && !ui.views_loading() {
            s.test_launch = None;
            let key = ui.fe.as_ref().and_then(|fe| fe.career.event_named(&name)).map(|e| e.key);
            let free = s.test_map_mode.as_deref() == Some("freeride");
            match key {
                Some(key) => ui.fe_call(move |fe, env| {
                    ssx_ui::game::fe::progression::go_to_node(fe, key, 2);
                    if free {
                        ssx_ui::game::fe::flow::launch_free_ride(fe, env, key, true);
                    } else if !ssx_ui::game::fe::globe::tutorial_launch(fe, env, key) {
                        ssx_ui::game::fe::flow::launch(fe, env, key, true)
                    }
                }),
                None => error!("shell: --launch {name}: no such ui_drop"),
            }
        } else if s.host.time - at > LAUNCH_BOOT_WAIT {
            s.test_launch = None;
            error!(
                "shell: --launch {name}: the boot flow did not end within {LAUNCH_BOOT_WAIT} s (screen {screen:?}, the \
                 boot's background loaded {}, views still loading {:?}): not launched",
                ui.nav.is_active("backgroundcontainer_on"),
                ui.loading_view_names()
            );
        } else {
            s.boot_press(&mut ui, &format!("--launch {name}"));
        }
    }
    s.maps_frame(&mut ui);
    #[cfg(feature = "net")]
    if s.name_prompt_after_launch && s.test_launch.is_none() && s.test_map.is_none() {
        s.name_prompt_after_launch = false;
        ui.arm_name_prompt();
    }
    let window = f
        .render_scale
        .as_deref()
        .filter(|r| r.enabled)
        .map_or_else(|| f.windows.iter().next().map_or(Vec2::new(1280.0, 720.0), |w| w.size()), |r| r.size.as_vec2());
    s.play_movie(&mut ui, dt, window, &mut f.images, &mut f.sprites, &mut f.cameras, f.audio.as_deref_mut());
    let backdrop_shown = s.movie.is_none() && f.globe.frame.as_ref().is_some_and(|g| g.visible);
    f.globe.backdrop_movie = s.backdrop.frame(backdrop_shown, dt, &mut f.images);
    s.stage_step(&mut ui, f.level.as_deref(), f.ride.as_deref(), f.live.as_deref_mut());
    let sc = &mut *f.scene;
    let t0 = Instant::now();
    if let Some(vm) = sc.vm.as_mut() {
        ui.update(vm, &mut sc.lib, &mut sc.player, dt);
    }
    sc.hidden = ui.fe.as_ref().is_some_and(ssx_ui::game::fe::FrontEnd::movie_queued);
    if let Some(fe) = ui.fe.as_ref() {
        f.fade.set(fe.screen_fade());
    }
    let fe_time = ui.fe.as_ref().map_or(0.0, |fe| fe.time);
    s.fade_shot(&mut f.commands, f.fade.alpha, fe_time);
    if let Some(l) = f.level.as_mut() {
        l.ui_cpu(t0.elapsed() + sc.last_cpu);
    }
    let presses = f.level.as_ref().map_or(0, |l| l.popup_presses());
    if presses >= 3 && !std::mem::replace(&mut s.popup_reported, true) {
        report_popup(sc);
        screenshot(&mut f.commands, s.shot_dir.join("popup_not_closed.png"), false);
    } else if presses == 0 {
        s.popup_reported = false;
    }
    let mut requests = ui.take_requests();
    if std::mem::take(&mut s.host.exit_asked) {
        requests.push(ssx_ui::game::fe::pause::GameRequest::ExitGame);
    }
    for r in requests {
        use ssx_ui::game::fe::pause::GameRequest;
        info!("shell: game request {r:?}");
        match (r, f.level.as_mut()) {
            (GameRequest::ExitGame, _) => {
                info!("shell: Exit: the game closes (the profile {})", s.save.describe());
                f.exit.write(AppExit::Success);
            }
            (GameRequest::SaveSettings, _) => {
                s.save.front_end_from(&ui.values);
                s.write_save(&mut ui, "settings", f.audio.as_deref(), f.ride.as_deref());
                s.notes.push("shell: the audio settings applied".into());
                s.op_audio = true;
            }
            (GameRequest::Save, _) => {
                s.write_save(&mut ui, "the front end's save", f.audio.as_deref(), f.ride.as_deref())
            }
            (r, Some(l)) => l.request(r),
            _ => {}
        }
    }
    #[cfg(feature = "net")]
    for r in ui.take_net_requests() {
        info!("shell: the browser asks {r:?}");
        if f.live.is_some() && matches!(r, ssx_ui::game::fe::ava::browser::NetRequest::Connect(_)) {
            let reason = Some("a network session is already running".to_string());
            ui.net_progress(ssx_ui::game::fe::ava::browser::NetProgress::Ended { reason, refusal: None });
            continue;
        }
        crate::net::request(&mut f.commands, r);
    }
    let asked = ui.fe.as_mut().map(|fe| std::mem::take(&mut fe.saveload.requests)).unwrap_or_default();
    for r in asked {
        use ssx_ui::game::fe::saveload::{self, LoadError, Request};
        info!("shell: save library request {r:?}");
        match r {
            Request::Load => {
                if let Some(profile) = s.save.retry() {
                    s.op_audio |= s.save.profile_into(&mut ui, profile).is_some();
                }
                let error = s.save.damaged.then_some(LoadError::Corrupt);
                ui.fe_call(|fe, env| saveload::loaded(fe, env, error));
            }
            Request::SaveNew => {
                s.save.replace_damaged();
                s.write_save(&mut ui, "a new profile for the damaged one", f.audio.as_deref(), f.ride.as_deref());
            }
            Request::NoSaving => s.save.no_saving = true,
        }
    }
    if f.level.as_mut().is_some_and(|l| l.take_save()) {
        s.write_save(&mut ui, "results", f.audio.as_deref(), f.ride.as_deref());
    }
    let sounds = ui.take_ui_sounds();
    let posts = ui.take_ui_posts();
    let messages = ui.take_fe_messages();
    let music_requests = ui.fe.as_mut().map(|fe| std::mem::take(&mut fe.music.requests)).unwrap_or_default();
    if let Some(audio) = f.audio.as_mut() {
        use ssx_ui::game::fe::music::MusicRequest;
        for r in music_requests {
            match r {
                MusicRequest::Preview { id, key } => audio.preview(&s.big_root, id, key.as_ref()),
                MusicRequest::StopPreview => audio.stop_preview(),
            }
        }
        audio.set_level_loading(s.loading.is_some());
        if let Some(fe) = ui.fe.as_mut() {
            fe.music.controller_playing = audio.preview_playing();
            if fe.music.take_switches_changed() {
                audio.set_song_switches(&fe.music.switches());
            }
        }
    }
    if let Some(audio) = f.audio.as_mut() {
        let boost = ui.values.float(ssx_ui::game::values::id::BOOST_PERCENT);
        for id in sounds {
            audio.ui_sfx(id, boost);
        }
        let now_ms = s.host.time * 1000.0;
        for p in posts {
            audio.ui_post(p.id, p.delay_ms, now_ms);
        }
        use ssx_ui::game::fe::FeMessage;
        for m in messages {
            match m {
                FeMessage::HintShown(kind) => audio.hint_shown(kind),
                FeMessage::HintClosed => audio.hint_closed(),
                FeMessage::PopupState(v) => audio.popup_state(v),
                FeMessage::PauseMenuAudio { opened, leaving, closed } => {
                    audio.pause_menu_event(opened, leaving, closed)
                }
                FeMessage::BootFlowLoaded => audio.boot_flow_loaded(),
                FeMessage::LoadoutRefresh => audio.launch_requested(),
            }
        }
    }
    if let (Some(audio), Some(fe)) = (f.audio.as_mut(), ui.fe.as_ref()) {
        let kind = s.launch.as_ref().map_or(1, |l| ssx_assets::sfx_game::event_kind(l.event_type as u32));
        let nis = f.ride.as_deref().map_or(0, game_level::nis_state);
        audio.set_setting_67(ui.values.bool(0x67));
        audio.frame(fe.screen, kind, ui.values.bool(0x30), nis, ui.fe_audio_wind(), s.host.time * 1000.0);
        audio.watch_ui(|id| ui.values.bool(id));
        audio.results(ui.values.bool(0x1ED), ui.values.bool(0x1D7));
    }
    if let Some(audio) = f.audio.as_ref() {
        let msg = audio.state();
        let beat = audio.beat().map(|b| ssx_ui::game::fe::globe::audio::Beat {
            beat: b.beat,
            to_beat: b.to_beat,
            bar: b.bar,
            to_bar: b.to_bar,
        });
        ui.fe_audio_message(msg.screen, msg.state, beat);
    }
    let presses = s.host.log[s.log_seen.1.min(s.host.log.len())..].iter().filter(|l| l.contains("host: press")).count();
    if presses > 0 && s.input_log {
        s.notes.push(format!(
            "shell: input at {:.2} s: views loading {:?}; nav {:?}; {}",
            s.host.time,
            ui.loading_view_names(),
            ui.nav.active_leaves(),
            input_report(sc)
        ));
    }
    s.print_logs(&ui);
    let t = s.host.time;
    let rides_over = s.max_rides.is_some_and(|n| s.rides_done >= n) && s.stage == Stage::Menus;
    if rides_over || s.seconds.is_some_and(|x| t >= x) {
        let at = *s.done_at.get_or_insert(t);
        let pending = s.host.script.first().is_some_and(|(due, _)| *due <= at + 5.0);
        if (t - at >= 2.0 && !pending) || t - at >= 5.0 || s.seconds.is_some_and(|x| t >= x) {
            info!("shell: the scripted run ends at {t:.2} s (levels played {})", s.rides_done);
            let check = &*f.world_check;
            if check.covered.is_empty() {
                f.exit.write(AppExit::Success);
            } else {
                error!(
                    "world check: the level's world was covered in {} of {} ride screenshots: {}",
                    check.covered.len(),
                    check.shots,
                    check.covered.join(", ")
                );
                f.exit.write(AppExit::error());
            }
        }
    }
}

fn input_report(sc: &mut ui::UiScene) -> String {
    const VIEW: &str = "_level0.instance1.mc10.mView";
    let Some(vm) = sc.vm.as_mut() else { return String::new() };
    let mut cx = vm.ctx(&mut sc.lib, &mut sc.player);
    let text = |cx: &mut ssx_ui::avm1::Ctx, path: &str| {
        let v = ssx_ui::game::resolve_path(cx, path);
        v.and_then(|v| cx.to_string(&v).ok()).map_or_else(|| "?".to_string(), |s| s.to_string())
    };
    let input = format!("{VIEW}.mInputManager");
    let mut out = Vec::new();
    for m in ["mMinPriority", "mInputLockManager.mLockCount", "mInputLockManager.mAllInputLocked"] {
        out.push(format!("{m} = {}", text(&mut cx, &format!("{input}.{m}"))));
    }
    if let Some(locks) = ssx_ui::game::resolve_path(&mut cx, &format!("{input}.mInputLockManager.mLocks")) {
        let labels: Vec<String> = cx.enumerate(&locks).iter().map(|k| k.to_string()).collect();
        out.push(format!("locks {labels:?}"));
    }
    let n = text(&mut cx, &format!("{input}.mListenerLists.length")).parse::<usize>().unwrap_or(0);
    for i in 0..n.min(32) {
        let at = format!("{input}.mListenerLists.{i}");
        let (priority, who) = (text(&mut cx, &format!("{at}.priority")), text(&mut cx, &format!("{at}.listener")));
        out.push(format!("listener {i}: priority {priority} {who}"));
    }
    if let Some(root) = ssx_ui::game::resolve_path(&mut cx, VIEW) {
        let layers: Vec<String> =
            cx.enumerate(&root).iter().map(|k| k.to_string()).filter(|k| k.starts_with("ssx_")).collect();
        for layer in layers {
            let Some(clip) = ssx_ui::game::resolve_path(&mut cx, &format!("{VIEW}.{layer}")) else { continue };
            let views: Vec<String> = cx.enumerate(&clip).iter().map(|k| k.to_string()).collect();
            if !views.is_empty() {
                out.push(format!("{layer}: {views:?}"));
            }
        }
    }
    out.join("; ")
}

fn report_popup(sc: &mut ui::UiScene) {
    const VIEW: &str = "_level0.instance1.mc10.mView.ssx_tutorial.tutorialpausemenu.mView.mcTutorialPopup";
    let Some(vm) = sc.vm.as_mut() else { return };
    let mut traces: Vec<String> =
        vm.trace_log.iter().rev().filter(|l| l.contains("Tutorial_Popup")).take(6).cloned().collect();
    traces.reverse();
    let mut cx = vm.ctx(&mut sc.lib, &mut sc.player);
    let mut state = Vec::new();
    let members = [
        "mIsPopupEnabled",
        "mIsPopupVisible",
        "mHasAnimatedIn",
        "_visible",
        "mcUnblurred._currentframe",
        "mcUnblurred._totalframes",
        "mcUnblurred._visible",
    ];
    for m in members {
        let v = ssx_ui::game::resolve_path(&mut cx, &format!("{VIEW}.{m}"));
        let text = v.and_then(|v| cx.to_string(&v).ok()).map_or_else(|| "?".to_string(), |s| s.to_string());
        state.push(format!("{m} = {text}"));
    }
    const INPUT: &str = "_level0.instance1.mc10.mView.mInputManager";
    let text = |cx: &mut ssx_ui::avm1::Ctx, path: &str| {
        let v = ssx_ui::game::resolve_path(cx, path);
        v.and_then(|v| cx.to_string(&v).ok()).map_or_else(|| "?".to_string(), |s| s.to_string())
    };
    let mut input = Vec::new();
    for m in ["mMinPriority", "mInputLockManager.mLockCount", "mInputLockManager.mAllInputLocked"] {
        input.push(format!("{m} = {}", text(&mut cx, &format!("{INPUT}.{m}"))));
    }
    if let Some(locks) = ssx_ui::game::resolve_path(&mut cx, &format!("{INPUT}.mInputLockManager.mLocks")) {
        let labels: Vec<String> = cx.enumerate(&locks).iter().map(|k| k.to_string()).collect();
        input.push(format!("locks {labels:?}"));
    }
    let n = text(&mut cx, &format!("{INPUT}.mListenerLists.length")).parse::<usize>().unwrap_or(0);
    for i in 0..n.min(32) {
        let at = format!("{INPUT}.mListenerLists.{i}");
        let (priority, who) = (text(&mut cx, &format!("{at}.priority")), text(&mut cx, &format!("{at}.listener")));
        input.push(format!("listener {i}: priority {priority} {who}"));
    }
    let list_path = format!("{VIEW}.mcUnblurred.mcCallout.mcListAnim.mcList");
    let mut list = Vec::new();
    if let Some(l) = ssx_ui::game::resolve_path(&mut cx, &list_path) {
        let names: Vec<String> = cx.enumerate(&l).iter().map(|k| k.to_string()).collect();
        for n in names.iter().filter(|n| n.starts_with('m') && !n.starts_with("mc")).take(40) {
            list.push(format!("{n} = {}", text(&mut cx, &format!("{list_path}.{n}"))));
        }
    }
    error!(
        "shell: the tutorial popup did not close on A (autoplay, 3 presses): {}; its last traces: {traces:?}; the \
         input manager: {}; the list: {}",
        state.join(", "),
        input.join("; "),
        list.join(", ")
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use ssx_ui::game::fe::ava::keyboard::TextInput as T;

    #[test]
    fn keys_become_text() {
        assert_eq!(key_text(KeyCode::KeyA, Some("a")), [T::Char('a')]);
        assert_eq!(key_text(KeyCode::KeyA, Some("A")), [T::Char('A')]);
        assert_eq!(key_text(KeyCode::Backspace, Some("\u{8}")), [T::Backspace]);
        assert_eq!(key_text(KeyCode::Enter, Some("\r")), [T::Enter]);
        assert_eq!(key_text(KeyCode::Tab, Some("\t")), []);
        assert_eq!(key_text(KeyCode::ShiftLeft, None), []);
        assert_eq!(key_text(KeyCode::Digit2, Some("\u{e9}")), [T::Char('\u{e9}')]);
    }
}
