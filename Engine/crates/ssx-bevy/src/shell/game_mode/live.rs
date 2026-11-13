use bevy::prelude::{error, info, warn};
use ssx_ui::game::GameUi;
use ssx_ui::game::input::Button;

use super::{Shell, Stage};
use crate::shell::game_level::LevelGame;
use crate::shell::live::{LiveLevel, LiveOrigin, LiveSession, SessionLevel};

const MESSAGE_POPUP: &str = "ava_net_message";

const HUB_RETRY_S: f64 = 1.0;

// PORT(choice): 30 s (the quit staging's loading screen and the level's teardown take a few seconds); tuning: the
// longest quit measured on the slowest supported machine; the original has no real-time multiplayer.
const EQUIPMENT_WAIT_S: f64 = 30.0;

#[derive(Clone, Copy, Debug, Default)]
pub struct Away {
    since: Option<f64>,
    equipment: bool,
    relaunched: bool,
}

impl Shell {
    pub(super) fn boot_press(&mut self, ui: &mut GameUi, who: &str) {
        let screen = ui.fe.as_ref().map(|fe| fe.screen);
        let movie = ui.fe.as_ref().is_some_and(|fe| fe.movie.is_some());
        if screen != Some(ssx_ui::game::fe::screens::id::BOOT) || movie || self.host.time < self.launch_start_at {
            return;
        }
        if ui.fe.as_ref().is_some_and(|fe| fe.saveload.dialog == Some(0xC)) {
            info!("shell: {who}: A on the save library's autosave notice");
            self.host.press(ui, Button::DigitalOne);
        } else {
            info!("shell: {who}: START on the boot screen first (the boot flow ends on the main menu)");
            self.host.press(ui, Button::Start);
        }
        self.launch_start_at = self.host.time + 1.0;
    }

    pub(super) fn live_frame(&mut self, ui: &mut GameUi, level: Option<&LevelGame>, live: &mut LiveSession) -> bool {
        if live.end.is_none() {
            if live.launch.is_some() {
                let menu = ui.fe.as_ref().is_some_and(|fe| fe.ava.hub.active) || ui.browser_active();
                if menu && !ui.views_loading() {
                    match live.launch.take() {
                        Some(SessionLevel::Drop(drop)) => {
                            info!("shell: the network session's drop {drop}: launched as --launch launches one");
                            self.test_launch = Some((drop, self.host.time));
                        }
                        Some(SessionLevel::Map { id, event_type, mode }) => {
                            if let Err(why) = self.launch_session_map(ui, &id, &mode, event_type) {
                                warn!("shell: the network session ends: {}", ssx_core::text::Text::new(&why));
                                live.end = Some(crate::shell::live::LiveEnd {
                                    reason: Some(why),
                                    #[cfg(feature = "net")]
                                    refusal: None,
                                });
                            }
                        }
                        None => {}
                    }
                } else if self.stage == Stage::Menus {
                    self.boot_press(ui, "the network session's launch");
                }
            }
            if self.stage == Stage::Level && level.is_some_and(LevelGame::is_up) && live.level == LiveLevel::Loading {
                live.level = LiveLevel::Up;
            }
            if live.level == LiveLevel::Away && self.stage == Stage::Menus {
                self.live_away_frame(ui, live);
            }
            if live.relaunch.is_some() {
                self.live_next_frame(ui, live);
            }
            return false;
        }
        match self.stage {
            Stage::Loading => false,
            Stage::Level => {
                #[cfg(feature = "net")]
                if level.is_some_and(LevelGame::results_shown) || ui.session_results_exit_pending() {
                    if !std::mem::replace(&mut self.live_quit_asked, true) {
                        info!("shell: the network session ended on the round's results: the post-race menu's quit");
                        ui.session_quit_results();
                    }
                    if let Err(why) = ui.session_results_exit_frame() {
                        error!("shell: the network session ended, the level is not left: {why}");
                    }
                    return false;
                }
                if !std::mem::replace(&mut self.live_quit_asked, true) {
                    info!("shell: the network session ended: the level is left (go_to_quit_stage)");
                    ui.nav.raise("go_to_quit_stage");
                }
                false
            }
            Stage::Menus => {
                self.test_launch = None;
                let (there, open, name) = match live.origin {
                    #[cfg(feature = "net")]
                    LiveOrigin::Browser => (ui.browser_active(), ssx_ui::game::fe::ava::browser::OPEN, "the browser"),
                    LiveOrigin::Hub => (
                        ui.fe.as_ref().is_some_and(|fe| fe.ava.hub.active),
                        ssx_ui::game::fe::ava::hub::OPEN,
                        "the Project Avalanche menu",
                    ),
                };
                if !there {
                    self.boot_press(ui, "the network session's end");
                    let booting = ui.fe.as_ref().is_some_and(|fe| fe.screen == ssx_ui::game::fe::screens::id::BOOT);
                    if !booting && self.host.time >= self.live_hub_at && !ui.views_loading() {
                        info!("shell: the network session ended: to {name}");
                        let main = ssx_ui::game::fe::screens::id::MAIN_MENU;
                        ui.fe_call(|fe, env| fe.go_to_screen_by(env, main, open));
                        self.live_hub_at = self.host.time + HUB_RETRY_S;
                    }
                    return false;
                }
                self.live_over(ui, live);
                true
            }
        }
    }

    fn live_away_frame(&mut self, ui: &mut GameUi, live: &mut LiveSession) {
        let now = self.host.time;
        let away = self.live_away.get_or_insert_with(Away::default);
        let since = *away.since.get_or_insert(now);
        let Some(fe) = ui.fe.as_ref() else { return };
        let (screen, launching) = (fe.screen, fe.launch.is_some() || fe.in_drop);
        if screen == ssx_ui::game::fe::screens::id::EQUIPMENT {
            if !std::mem::replace(&mut away.equipment, true) {
                info!("shell: the equipment screen between two levels of the network session ({:.1} s)", now - since);
            }
            return;
        }
        if launching || away.relaunched || ui.views_loading() {
            return;
        }
        if !away.equipment {
            if now - since > EQUIPMENT_WAIT_S {
                let why = format!("the equipment screen did not come within {EQUIPMENT_WAIT_S:.0} s");
                warn!("shell: the network session ends: {why}");
                live.end = Some(crate::shell::live::LiveEnd {
                    reason: Some(why),
                    #[cfg(feature = "net")]
                    refusal: None,
                });
            }
            return;
        }
        if screen == ssx_ui::game::fe::screens::id::LOADING {
            return;
        }
        let Some(level) = live.ridden.clone() else { return };
        away.relaunched = true;
        let drop = match level {
            SessionLevel::Drop(drop) => drop,
            SessionLevel::Map { id, event_type, mode } => {
                info!("shell: the player backed out of the equipment (screen {screen:#x}): to the server's map {id}");
                if let Err(why) = self.launch_session_map(ui, &id, &mode, event_type) {
                    warn!("shell: the network session ends: {}", ssx_core::text::Text::new(&why));
                    live.end = Some(crate::shell::live::LiveEnd {
                        reason: Some(why),
                        #[cfg(feature = "net")]
                        refusal: None,
                    });
                }
                return;
            }
        };
        let Some(fe) = ui.fe.as_ref() else { return };
        let key = fe.career.event_named(&drop).map(|e| e.key);
        match key {
            Some(key) => {
                info!(
                    "shell: the player backed out of the equipment (screen {screen:#x}): to the server's drop {drop}"
                );
                ui.fe_call(|fe, env| ssx_ui::game::fe::flow::launch(fe, env, key, true));
            }
            None => error!("shell: the server's drop {drop} is gone from the career data"),
        }
    }

    fn live_next_frame(&mut self, ui: &mut GameUi, live: &mut LiveSession) {
        match self.stage {
            Stage::Level => {
                #[cfg(feature = "net")]
                if ui.session_results_exit_pending() {
                    return;
                }
                if !std::mem::replace(&mut self.live_quit_asked, true) {
                    info!("shell: the next round's drop: the level is left (go_to_quit_stage)");
                    ui.nav.raise("go_to_quit_stage");
                }
            }
            Stage::Loading => {}
            Stage::Menus => {
                let since = *self.live_next_since.get_or_insert(self.host.time);
                if !ui.menus_settled() {
                    if self.host.time - since > EQUIPMENT_WAIT_S {
                        let why = format!("the menu did not come within {EQUIPMENT_WAIT_S:.0} s for the next drop");
                        warn!("shell: the network session ends: {why}");
                        live.end = Some(crate::shell::live::LiveEnd {
                            reason: Some(why),
                            #[cfg(feature = "net")]
                            refusal: None,
                        });
                    }
                    return;
                }
                if ui.fe.is_none() {
                    return;
                }
                let Some(level) = live.relaunch.take() else { return };
                (self.live_quit_asked, self.live_next_since) = (false, None);
                let drop = match level {
                    SessionLevel::Drop(drop) => drop,
                    SessionLevel::Map { ref id, event_type, ref mode } => {
                        match self.launch_session_map(ui, id, mode, event_type) {
                            Ok(()) => {
                                info!("shell: the next round's level {level}: launched");
                                live.ridden = Some(level);
                            }
                            Err(why) => {
                                warn!("shell: the network session ends: {}", ssx_core::text::Text::new(&why));
                                live.end = Some(crate::shell::live::LiveEnd {
                                    reason: Some(why),
                                    #[cfg(feature = "net")]
                                    refusal: None,
                                });
                            }
                        }
                        return;
                    }
                };
                let Some(fe) = ui.fe.as_ref() else { return };
                match fe.career.event_named(&drop).map(|e| e.key) {
                    Some(key) => {
                        info!("shell: the next round's drop {drop}: launched");
                        live.ridden = Some(SessionLevel::Drop(drop));
                        ui.fe_call(|fe, env| ssx_ui::game::fe::flow::launch(fe, env, key, true));
                    }
                    None => {
                        let why = format!("the server's next drop {drop} is not on this game's disc");
                        warn!("shell: the network session ends: {why}");
                        live.end = Some(crate::shell::live::LiveEnd {
                            reason: Some(why),
                            #[cfg(feature = "net")]
                            refusal: None,
                        });
                    }
                }
            }
        }
    }

    fn launch_session_map(&self, ui: &mut GameUi, id: &str, mode: &str, event_type: u8) -> Result<(), String> {
        use ssx_assets::content::gamemodes::ModeChoice;
        let content = ssx_assets::content::GameContent::of(&self.big_root);
        let shown = ssx_core::text::shown(id);
        let cid: ssx_content::ContentId = id.parse().map_err(|e| format!("the server's map {shown}: {e}"))?;
        if content.session_policy().is_none() {
            return Err(format!(
                "the server's map {shown}: the server sent no resources, and a session rides the server's maps only"
            ));
        }
        content.map(&cid).map_err(|e| format!("the server's map: {e}"))?;
        if let ModeChoice::Gamemode(g) = ModeChoice::parse(mode).map_err(|e| format!("the server's mode: {e}"))? {
            content.gamemode(&g).map_err(|e| format!("the server's gamemode: {e}"))?;
        }
        let (rows, more) = super::maps::list(&content);
        let mut launched = Err("the game has no front end to start the map from".to_string());
        ui.fe_call(|fe, env| {
            ssx_ui::game::fe::ava::maps::listed(fe, rows, more);
            launched = ssx_ui::game::fe::ava::maps::session::ride(fe, env, id, mode, event_type);
        });
        if launched.is_ok() {
            info!("shell: the network session's map {id} ({mode}, event type {event_type}): launched");
        }
        launched
    }

    fn live_over(&mut self, ui: &mut GameUi, live: &LiveSession) {
        if let Some(profile) = self.live_profile.take()
            && let Some(fe) = ui.fe.as_mut()
        {
            fe.profile = profile;
            info!("shell: the career profile is as the network session found it (no progress from multiplayer)");
        }
        if let Some(fe) = ui.fe.as_mut() {
            fe.network_session = false;
        }
        (self.live_quit_asked, self.live_next_since) = (false, None);
        let text = match live.end.as_ref().and_then(|e| e.reason.as_deref()) {
            Some(reason) => {
                warn!("shell: the network session ended: {reason}");
                reason.to_string()
            }
            None => {
                info!("shell: the network session ended (the player left the server)");
                left_text(live.server.as_deref())
            }
        };
        #[cfg(feature = "net")]
        if live.origin == LiveOrigin::Browser {
            let refusal = live.end.as_ref().and_then(|e| e.refusal);
            ui.net_progress(ssx_ui::game::fe::ava::browser::NetProgress::Ended { reason: Some(text), refusal });
            return;
        }
        let glyphs = ui.fe.as_ref().and_then(|fe| fe.ava.glyphs.clone());
        ui.values.set_str(ssx_ui::game::fe::popup::POPUP_BODY, shown_text(&text, glyphs.as_ref()));
        ui.nav.raise(MESSAGE_POPUP);
    }
}

fn left_text(server: Option<&str>) -> String {
    match server {
        Some(name) => format!("you left the server {name}"),
        None => "you left the server".into(),
    }
}

pub fn shown_text(text: &str, glyphs: Option<&std::collections::BTreeSet<char>>) -> String {
    ssx_ui::game::fe::ava::shown(text, glyphs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_players_own_leave_names_the_server_as_the_popup_shows_it() {
        assert_eq!(left_text(Some("Alps Freeride")), "you left the server Alps Freeride");
        assert_eq!(left_text(None), "you left the server");
        let glyphs: std::collections::BTreeSet<char> = "you left the server <b>Alps&</b>\u{7}é".chars().collect();
        let wrap = ssx_ui::text::isolate::wrap;
        assert_eq!(
            shown_text(&left_text(Some("<b>Alps&</b>\u{7}é")), Some(&glyphs)),
            wrap("you left the server ?b?Alps??/b??é")
        );
        assert_eq!(shown_text("Alps\u{e9}\u{1F600}", None), wrap("Alps??"));
    }
}
