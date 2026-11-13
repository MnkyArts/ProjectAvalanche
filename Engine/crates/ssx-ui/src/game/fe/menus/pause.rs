use ssx_assets::port_options::MenuFocus;

use crate::avm1::Ctx;
use crate::avm1::value::Value;
use crate::game::fe::{FeEnv, FrontEnd, screens};

pub const PAUSE_IS_IN_PAUSE_MENU: u16 = 0x1B9;
pub const PAUSE_IS_EXITING_BE: u16 = 0x1BA;
pub use crate::game::values::id::PAUSE_MENU_DISABLE;
pub const POPUP_IS_ACTIVE: u16 = 0x1C1;
pub const SETTINGS_IS_IN_PAUSE_MENU: u16 = 0x1F3;
pub const RESTART_NEVER_SHOW_AGAIN: u16 = 0x9D;
pub const LOADING_IN_LOADING_SCREEN: u16 = 0x1AE;
pub const LIVEEVENT_REMAINING_MS: u16 = 0x18F;
pub const HUD_NIS_PROMPT_ACTIVE: u16 = 0x112;
pub const FIRST_TUTORIAL: u32 = 0xE389_832F;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum GameRequest {
    Restart,
    Quit,
    SaveSettings,
    Save,
    ExitGame,
}

pub mod reason {
    pub const R2: usize = 2;
    pub const SYSTEM_UI: usize = 3;
    pub const PAD_LOST: usize = 4;
    pub const START: usize = 5;
    pub const R7: usize = 7;
    pub const R8: usize = 8;
    pub const R9: usize = 9;
    pub const TUTORIAL_POPUP: usize = 0xB;
    pub const R14: usize = 0xE;
    pub const MOVIE: usize = 0xF;
    pub const LEAVING: usize = 0x10;
    pub const COUNT: usize = 0x12;
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PauseManager {
    pub requests: [bool; reason::COUNT],
    pub active: [bool; reason::COUNT],
    pub paused: bool,
}

impl PauseManager {
    pub fn set(&mut self, reason: usize, on: bool) {
        self.requests[reason] = on;
    }

    pub fn asked(&self, reason: usize) -> bool {
        self.active[reason] || self.requests[reason]
    }

    pub fn update(&mut self) -> Option<bool> {
        let was = self.paused;
        self.active = self.requests;
        self.paused = self.requests.iter().any(|&r| r);
        (was != self.paused).then_some(self.paused)
    }

    pub fn locks_input(&self) -> bool {
        [3, 4, 5, 6, 8, 9, 10, 0xB, 0xC, 0xD, 0xE, 0xF].iter().any(|&r| self.active[r])
    }

    pub fn audio_event(&self) -> Option<bool> {
        [3, 4, 5, 6, 0xB, 0xC, 0xD, 0xE].iter().any(|&r| self.active[r]).then_some(self.active[reason::TUTORIAL_POPUP])
    }

    pub fn system_ui_message(&mut self, shown: bool) {
        if !(shown && self.active[reason::SYSTEM_UI]) {
            self.set(reason::SYSTEM_UI, shown);
        }
    }

    pub fn hud_ready(&mut self, system_ui: bool) -> bool {
        let set = system_ui && !self.active[reason::SYSTEM_UI];
        if set {
            self.set(reason::SYSTEM_UI, true);
        }
        set
    }

    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

#[derive(Clone, Debug, Default)]
pub struct Pause {
    pub active: bool,
    pub resume_requested: bool,
    pub leaving: bool,
    pub restart: bool,
    pub settings: bool,
    pub restart_enabled: bool,
    pub players: bool,
    pub last: Option<&'static str>,
}

pub fn restart_allowed(fe: &FrontEnd, values: &crate::game::values::UiValues) -> bool {
    // TODO(guess): a port choice — the game disables the restart entry only in a live event with under a minute left
    // (game mode 3, LIVEEVENT_REMAINING_MS < 60 000, 0x8240D330); a network session's menu has no restart at all (its
    // items are Resume, Players, Settings, Leave server), as the original's live-event pause (the online racers'
    // sessions) is not decoded; settle: the live-event pause, if it is ever decoded (the live-event manager
    // 0x82426C18).
    !fe.network_session && (fe.game_mode != 3 || values.int(LIVEEVENT_REMAINING_MS) >= 60_000)
}

pub fn game_input(fe: &mut FrontEnd, env: &mut FeEnv, start: bool) {
    if fe.screen == screens::id::HUD && start {
        pause_menu(fe, env);
    } else if fe.screen == screens::id::PAUSE && fe.pause.resume_requested && !env.values.bool(POPUP_IS_ACTIVE) {
        fe.pause.resume_requested = false;
        fe.note("pause: 0xC3CF3A7C {0} → view event Unpause");
        unpause(fe, env);
    }
}

pub fn pause_menu(fe: &mut FrontEnd, env: &mut FeEnv) {
    if env.values.bool(LOADING_IN_LOADING_SCREEN) || env.values.bool(PAUSE_MENU_DISABLE) {
        return;
    }
    fe.game_pause.set(reason::START, true);
    fe.pause.last = None;
    fe.note("pause: 0xC3F77F89 {1} (pause reason 5), 0xC3CF3A7C {1} → view event Pause → hud provider");
    fe.go_to_screen(env, screens::id::PAUSE);
}

fn unpause(fe: &mut FrontEnd, env: &mut FeEnv) {
    if fe.pause.active {
        fe.go_to_screen(env, screens::id::HUD);
    }
}

pub fn activate(fe: &mut FrontEnd, env: &mut FeEnv) {
    fe.pause.active = true;
    fe.pause.leaving = false;
    fe.pause.restart = false;
    fe.pause.settings = false;
    fe.pause.players = false;
    fe.pause.restart_enabled = restart_allowed(fe, env.values);
    env.values.set_bool(PAUSE_IS_IN_PAUSE_MENU, true);
    env.values.set_bool(PAUSE_IS_EXITING_BE, false);
    fe.note("pause menu: FE manager event 0xB (0x329E66FA {2}, audio event 0x8BE0E597 {1, 0, 0})");
    fe.messages.push(crate::game::fe::FeMessage::PopupState(2));
    fe.messages.push(crate::game::fe::FeMessage::PauseMenuAudio { opened: true, leaving: false, closed: false });
    fe.publish.push("dp_pause_menu_data".into());
    fe.publish.push("dp_pause_menu_index".into());
}

pub fn deactivate(fe: &mut FrontEnd, env: &mut FeEnv) {
    if !fe.pause.active {
        return;
    }
    fe.pause.active = false;
    if !fe.pause.settings && !fe.pause.players {
        if fe.pause.leaving {
            fe.note("pause menu closed: FE manager event 0xD (0x329E66FA {3}, audio event 0x8BE0E597 {0, 1, 0})");
        } else {
            fe.game_pause.set(reason::START, false);
            fe.note(
                "pause menu closed: FE manager events 0xE (0xC3F77F89 {0}: reason 5 off), 0xC (0x329E66FA {3}, audio \
                 event 0x8BE0E597 {0, 0, 1})",
            );
        }
        fe.messages.push(crate::game::fe::FeMessage::PopupState(3));
        let leaving = fe.pause.leaving;
        fe.messages.push(crate::game::fe::FeMessage::PauseMenuAudio { opened: false, leaving, closed: !leaving });
        env.values.set_bool(SETTINGS_IS_IN_PAUSE_MENU, false);
    }
    if fe.pause.leaving {
        env.values.set_bool(PAUSE_IS_EXITING_BE, true);
    }
}

pub fn update(fe: &mut FrontEnd, env: &mut FeEnv) {
    if !fe.pause.active {
        return;
    }
    let allowed = restart_allowed(fe, env.values);
    if allowed != fe.pause.restart_enabled {
        fe.pause.restart_enabled = allowed;
        fe.publish.push("dp_pause_menu_data".into());
    }
}

pub fn action(fe: &mut FrontEnd, env: &mut FeEnv, action: &str) {
    match action {
        "Unpause" | "A_GameUnpause" => unpause(fe, env),
        "A_GameResume" => {
            fe.pause.leaving = false;
            fe.pause.restart = false;
            fe.pause.resume_requested = true;
            env.values.set_bool(POPUP_IS_ACTIVE, false);
            env.values.set_bool(PAUSE_IS_IN_PAUSE_MENU, false);
        }
        "A_GameRestartStage" => {
            fe.pause.last = Some("RestartStage");
            if env.values.bool(RESTART_NEVER_SHOW_AGAIN) {
                fe.pause.restart = true;
                save_and_leave(fe, env);
            } else {
                env.values.set_bool(POPUP_IS_ACTIVE, true);
                env.nav_events.push("go_to_restart_popup".into());
            }
        }
        "A_GameQuitStage" => {
            fe.pause.last = Some("QuitStage");
            env.values.set_bool(POPUP_IS_ACTIVE, true);
            env.nav_events.push("go_to_quit_popup".into());
        }
        "A_GameSettings" => {
            fe.pause.last = Some("A_GameSettings");
            env.values.set_bool(SETTINGS_IS_IN_PAUSE_MENU, true);
            fe.pause.settings = true;
            env.nav_events.push("go_to_settings_menu".into());
        }
        "A_GameShowObjective" => {
            fe.pause.last = Some("ShowObjective");
            env.values.set_bool(POPUP_IS_ACTIVE, true);
            env.nav_events.push("go_to_show_objective".into());
        }
        _ => {}
    }
}

pub fn nav_action(fe: &mut FrontEnd, env: &mut FeEnv, name: &str) -> bool {
    match name {
        "do_event_quit" | "do_event_restart" | "do_restart_never_show_again" => {
            env.values.set_bool(POPUP_IS_ACTIVE, false);
            if name == "do_restart_never_show_again" {
                env.values.set_bool(RESTART_NEVER_SHOW_AGAIN, true);
                fe.pause.leaving = true;
            }
            fe.pause.restart = name != "do_event_quit";
            fe.note(format!("pause menu: {name} (statistics counter 1)"));
            save_and_leave(fe, env);
            fe.busy.add(env, crate::game::fe::busy::PAUSE_LEAVE);
        }
        "do_close_popup" => env.values.set_bool(POPUP_IS_ACTIVE, false),
        _ => return false,
    }
    true
}

// TODO(guess): the save (0x82405FB0 → 0x82457FE8, end 0x82405F50) completes at once in the port; settle: the save manager's job queue and its callback timing.
fn save_and_leave(fe: &mut FrontEnd, env: &mut FeEnv) {
    fe.note("pause menu: save (FE manager +0xC0, +0xC4)");
    crate::game::fe::flow::store_camera_type(fe);
    fe.requests.push(GameRequest::Save);
    fe.pause.leaving = true;
    fe.game_pause.set(reason::LEAVING, true);
    fe.note("pause menu: FE manager +0x80(1) (0x34A67757 {1}: reason 0x10), SplashScreenDisplayMessage (0x576B3006)");
    env.values.set_bool(PAUSE_IS_IN_PAUSE_MENU, false);
    env.values.set_bool(HUD_NIS_PROMPT_ACTIVE, false);
    let screen = if fe.pause.restart { screens::id::RESTART_STAGE } else { screens::id::QUIT_STAGE };
    fe.go_to_screen(env, screen);
}

pub fn do_restart(fe: &mut FrontEnd, env: &mut FeEnv) {
    fe.note("FENotificationRestartGameplay (0x5A5A5C39), FE manager +0x3C (0xB0E2239A)");
    fe.requests.push(GameRequest::Restart);
    fe.level_start.due = true;
    fe.level_start.streamed = false;
    crate::game::hud::level_start(env.values, fe.level_start.nis);
    // TODO(guess): the reset comes at once here, the game's at the switch back into `FSGameSimulationBE` (the ride is held meanwhile); settle: the main loop's order of the frame-state request against the updates.
    fe.game_pause.reset();
}

fn items(fe: &FrontEnd) -> Vec<(&'static str, bool, &'static str)> {
    let drop = fe.current_drop;
    let full = drop != FIRST_TUTORIAL;
    let objective = fe.career.event(drop).is_some_and(|e| !e.hud_title.is_empty());
    let mut items = vec![("TXT_PAUSE_MENU_HEADING", true, "Resume")];
    if full {
        let key = if fe.pause.restart_enabled { "TXT_PAUSE_MENU_RESTART" } else { "TXT_PAUSE_MENU_RESTART_DISABLED" };
        items.push((key, fe.pause.restart_enabled, "RestartStage"));
    }
    items.push(("TXT_PAUSE_MENU_SETTINGS", true, "A_GameSettings"));
    if objective {
        items.push(("TXT_CAM_OBJECTIVE_TITLE", true, "ShowObjective"));
    }
    if full {
        items.push(("TXT_PAUSE_MENU_QUIT", true, "QuitStage"));
    }
    items
}

fn menu_index(fe: &FrontEnd) -> usize {
    match fe.menu_focus {
        MenuFocus::FirstItem => 0,
        // PORT(choice): the option "Menus: the last item picked" focuses the item last picked since the pause began
        // (Settings, the objective, a restart or quit that was not confirmed; a network session's Players and Leave)
        // while it is enabled; the game's pause menu always opens on Resume (+0x84 = 0 from 0x825D3070).
        MenuFocus::LastItem => fe.pause.last.and_then(|last| last_picked(fe, last)).unwrap_or(0),
    }
}

fn last_picked(fe: &FrontEnd, last: &str) -> Option<usize> {
    #[cfg(feature = "net")]
    if fe.network_session {
        return crate::game::fe::ava::session::pause::ITEMS.iter().position(|(_, advance)| *advance == last);
    }
    items(fe).iter().position(|&(_, enabled, advance)| advance == last && enabled)
}

pub fn build(fe: &FrontEnd, cx: &mut Ctx, loc: &mut crate::game::loc::Localization, dp: &str) -> Option<Value> {
    match dp {
        "dp_pause_menu_index" => Some(Value::Number(menu_index(fe) as f64)),
        #[cfg(feature = "net")]
        "dp_pause_menu_data" if fe.network_session => Some(crate::game::fe::ava::session::pause::build(cx, loc)),
        "dp_pause_menu_data" => {
            let mut out = Vec::new();
            for (key, enabled, advance) in items(fe) {
                let o = Value::Object(cx.new_object());
                let _ = cx.set_member(&o, "strItem", Value::str(&loc.text(key)));
                let _ = cx.set_member(&o, "bEnabled", Value::Bool(enabled));
                let _ = cx.set_member(&o, "bFocusable", Value::Bool(false));
                let _ = cx.set_member(&o, "eventAdvance", Value::str(advance));
                out.push(o);
            }
            Some(Value::Object(cx.new_array(out)))
        }
        _ => None,
    }
}
