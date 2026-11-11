pub mod ava;
pub mod busy;
pub mod data;
pub mod explore;
pub mod fade;
pub mod flow;
pub mod globe;
pub mod host;
mod loadout;
mod menus;
pub mod pads;
pub mod progression;
mod ridernet;
pub mod screens;
pub mod scripts;

mod dispatch;
mod movies;
mod objects;

pub use globe::{clouds as globe_clouds, levels as globe_levels, look as globe_look, scene as globe_scene};
pub use loadout::{equipment, inventory, loading, setup};
pub use menus::{boot, hint, menu, pause, popup, postrace, saveload, settings, tutorial_pause, unlocks};
pub use ridernet::{badges, credits, music, notify, user};

use std::path::Path;

use anyhow::Result;
use ssx_assets::career::{CareerDb, Profile, Rules};

use super::loc::Localization;
use super::values::UiValues;
use crate::avm1::Ctx;
use crate::avm1::value::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FeMessage {
    HintShown(i32),
    HintClosed,
    PopupState(i32),
    PauseMenuAudio { opened: bool, leaving: bool, closed: bool },
    BootFlowLoaded,
    LoadoutRefresh,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ActionParam {
    pub number: f64,
    pub fields: std::collections::BTreeMap<String, f64>,
}

impl ActionParam {
    pub fn of(cx: &mut Ctx, v: Option<&Value>) -> Self {
        let Some(v) = v else { return ActionParam::default() };
        let mut p = ActionParam { number: cx.to_number(v), ..Default::default() };
        if let Value::Object(_) = v {
            for k in cx.enumerate(v) {
                let k = k.to_string();
                if let Ok(f) = cx.get_member(v, &k) {
                    let n = cx.to_number(&f);
                    p.fields.insert(k, n);
                }
            }
        }
        p
    }

    pub fn number(n: f64) -> Self {
        ActionParam { number: n, ..Default::default() }
    }

    pub fn field(&self, name: &str) -> f64 {
        self.fields.get(name).copied().unwrap_or(f64::NAN)
    }
}

pub struct FeEnv<'a> {
    pub values: &'a mut UiValues,
    pub loc: &'a mut Localization,
    pub nav_events: &'a mut Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MovieRequest {
    pub name: String,
    pub by: &'static str,
    pub can_stop: bool,
}

impl MovieRequest {
    pub fn new(name: impl Into<String>, by: &'static str) -> Self {
        MovieRequest { name: name.into(), by, can_stop: true }
    }
}

pub const MOVIE_SKIP_FROM_FRAME: i32 = 0x3D;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Launch {
    pub drop: u32,
    pub event: String,
    pub track: String,
    pub track_drop: usize,
    pub event_type: i64,
    pub game_mode: i64,
    pub rider: String,
    pub setup: setup::GameSetup,
    pub direct: bool,
    pub map: Option<MapLaunch>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct MapLaunch {
    pub id: String,
    pub title: String,
    pub track: String,
    pub mode: String,
}

pub struct FrontEnd {
    pub career: CareerDb,
    pub rules: Rules,
    pub profile: Profile,
    pub screen: u32,
    pub fe_mode: i32,
    pub game_mode: i64,
    pub menu_selection: i32,
    pub boot: boot::BootFlow,
    pub menu: menu::MainMenu,
    pub globe: globe::Globe,
    pub globe_scene: Option<globe::scene::GlobeScene>,
    pub equipment: equipment::Equipment,
    pub loading: loading::Loading,
    pub user: user::UserData,
    pub dimmer: user::Dimmer,
    pub new_game: flow::NewGameHost,
    pub movie: Option<MovieRequest>,
    pub movie_queue: std::collections::VecDeque<MovieRequest>,
    pub movie_skip: i32,
    pub movie_stop: bool,
    movie_seen: bool,
    movie_end: bool,
    pub movie_serial: u32,
    pub movie_finished_latch: bool,
    pub progression: progression::Progression,
    pub campaign_start: ssx_assets::career::globe::focus::CampaignGlobe,
    pub launch: Option<Launch>,
    pub in_drop: bool,
    pub last_success: Option<bool>,
    pub current_drop: u32,
    pub last_result_drop: u32,
    pub hints: hint::Hints,
    pub popups: popup::Popups,
    pub saveload: saveload::SaveLoad,
    pub pause: pause::Pause,
    pub game_pause: pause::PauseManager,
    pub simulation: bool,
    pub pads: pads::Pads,
    pub load_latches: flow::LoadLatches,
    pub change_gear: bool,
    pub hud_ready_seen: bool,
    pub camera_type: Option<i32>,
    pub camera_restore: Option<i32>,
    pub level_start: flow::LevelStart,
    pub fader: fade::Fader,
    pub fader_by_ride: bool,
    pub network_session: bool,
    pub withheld_popups: Vec<String>,
    pub pending_strings: Vec<(String, String)>,
    pub loop_fade_in: bool,
    pub busy: busy::BusyIcon,
    pub post_race: postrace::PostRaceMenu,
    pub settings: settings::Settings,
    pub unlocks: Vec<unlocks::UnlockItem>,
    pub results_hint_wait: bool,
    pub unlocks_hint_wait: bool,
    pub requests: Vec<pause::GameRequest>,
    pub ui_sounds: Vec<i32>,
    pub messages: Vec<FeMessage>,
    pub audio_defaults: [(u16, i32); 9],
    pub audio_store: [(u16, i32); 9],
    pub publish: Vec<String>,
    pub game_actions: Vec<(String, ActionParam)>,
    pub notifications: notify::Notifications,
    pub tutorial_pause: tutorial_pause::TutorialPause,
    pub tutorial_ui: bool,
    pub badges: badges::Badges,
    pub badge_progress: std::collections::BTreeMap<u32, f32>,
    pub scripts: Option<scripts::FeScripts>,
    pub credits_text: Option<Vec<u8>>,
    pub music: music::Music,
    pub ava: ava::Ava,
    pub input_lock: i32,
    pub time: f64,
    pub log: Vec<String>,
}

pub fn screen_sound(from: u32, to: u32) -> Option<i32> {
    match (from, to) {
        (_, screens::id::EQUIPMENT) => Some(2),
        (
            screens::id::EXPLORE_RANGE | screens::id::CAMPAIGN_RANGE | screens::id::GLOBAL_RANGE,
            screens::id::MAIN_MENU,
        ) => Some(4),
        _ => None,
    }
}

impl FrontEnd {
    pub fn reload_riders(&mut self, big_root: &Path) -> Result<()> {
        let career = CareerDb::load(big_root)?;
        let mut rules = career.rules();
        rules.load_badge_texts(big_root)?;
        self.pending_strings.extend(ssx_assets::content::characters::career::port_strings(&career));
        (self.career, self.rules) = (career, rules);
        Ok(())
    }

    pub fn new(big_root: &Path, profile: Profile) -> Result<Self> {
        Self::with_files(&crate::fs::Disc::new(big_root)?, big_root, profile)
    }

    pub fn with_files(files: &dyn crate::fs::FileSource, big_root: &Path, mut profile: Profile) -> Result<Self> {
        let career = CareerDb::load(big_root)?;
        let pending_strings = ssx_assets::content::characters::career::port_strings(&career).into_iter().collect();
        let mut rules = career.rules();
        rules.load_badge_texts(big_root)?;
        let attribdb = loading::AttribDb::from_files(files)?;
        let tips = loading::Tips::from_db(&attribdb)?;
        let hint_records = hint::Records::from_db(&attribdb)?;
        let globe_data = ssx_assets::career::globe::GlobeData::load(big_root).ok();
        let campaign_start = globe_data.as_ref().map_or_else(Default::default, |g| {
            let peaks = career.peaks.iter().map(|p| p.key).collect();
            ssx_assets::career::globe::focus::CampaignGlobe::start(&g.ui_drop_order, &peaks, &rules)
        });
        profile.campaign_globe.merge_start(&campaign_start);
        profile.own_campaign_riders(&rules);
        let globe_scene = globe_data.map(globe::scene::GlobeScene::new);
        let audio_defaults = settings::audio_values(&ssx_assets::sfx::settings::AudioSettings::defaults(
            &ssx_assets::sfx::settings::FeRanges::load(big_root)?,
        ));
        Ok(FrontEnd {
            career,
            rules,
            profile,
            screen: 0,
            fe_mode: 0,
            game_mode: -1,
            menu_selection: 0,
            boot: boot::BootFlow::new(),
            menu: menu::MainMenu::default(),
            globe: globe::Globe::default(),
            globe_scene,
            equipment: equipment::Equipment::default(),
            loading: loading::Loading { tips, ..Default::default() },
            user: user::UserData::default(),
            dimmer: user::Dimmer::default(),
            new_game: flow::NewGameHost::default(),
            movie: None,
            movie_queue: Default::default(),
            movie_skip: -1,
            movie_stop: false,
            movie_seen: false,
            movie_end: false,
            movie_serial: 0,
            movie_finished_latch: false,
            progression: Default::default(),
            campaign_start,
            launch: None,
            in_drop: false,
            last_success: None,
            current_drop: 0,
            last_result_drop: 0,
            hints: hint::Hints { records: hint_records },
            popups: popup::Popups::load(files),
            saveload: saveload::SaveLoad::default(),
            pause: pause::Pause::default(),
            game_pause: pause::PauseManager::default(),
            simulation: false,
            pads: pads::Pads::default(),
            load_latches: flow::LoadLatches::default(),
            change_gear: false,
            hud_ready_seen: false,
            camera_type: None,
            camera_restore: None,
            level_start: flow::LevelStart::default(),
            fader: fade::Fader::default(),
            fader_by_ride: false,
            network_session: false,
            pending_strings,
            withheld_popups: Vec::new(),
            loop_fade_in: false,
            busy: busy::BusyIcon::default(),
            post_race: postrace::PostRaceMenu::default(),
            settings: settings::Settings::default(),
            unlocks: Vec::new(),
            results_hint_wait: false,
            unlocks_hint_wait: false,
            requests: Vec::new(),
            ui_sounds: Vec::new(),
            messages: Vec::new(),
            audio_defaults,
            audio_store: audio_defaults,
            publish: Vec::new(),
            game_actions: Vec::new(),
            notifications: Default::default(),
            tutorial_pause: Default::default(),
            tutorial_ui: false,
            badges: Default::default(),
            badge_progress: Default::default(),
            scripts: Some(scripts::FeScripts::load(big_root, &[])),
            credits_text: credits::read(files),
            music: music::Music::with_songs(music::menu_songs(big_root)),
            ava: ava::Ava::default(),
            input_lock: 0,
            time: 0.0,
            log: Vec::new(),
        })
    }

    pub fn note(&mut self, what: impl AsRef<str>) {
        self.log.push(format!("[{:7.2}] {}", self.time, ssx_core::text::Text::new(what.as_ref())));
    }

    pub fn pause_step(&mut self) -> Option<bool> {
        let edge = self.game_pause.update();
        if let Some(paused) = edge {
            let reasons: Vec<usize> = (0..pause::reason::COUNT).filter(|&r| self.game_pause.active[r]).collect();
            self.note(format!("pause manager: {} (reasons {reasons:x?})", if paused { "paused" } else { "running" }));
        }
        edge
    }

    pub fn popup_message(&mut self, kind: i32) {
        self.messages.push(FeMessage::HintShown(kind));
        let number = |n: i32| ActionParam { number: f64::from(n), ..Default::default() };
        let action = match kind {
            1 => ("HintTutorial", number(1)),
            0 | 2 => ("PopupTutorial", number(kind)),
            5 => ("PopupObjective", ActionParam::default()),
            7 => ("PopupNone", ActionParam::default()),
            _ => return,
        };
        self.game_actions.push((action.0.to_string(), action.1));
    }

    pub fn play_sound(&mut self, id: i32) {
        self.note(format!("FE manager play-sound {id} (message 0xA80A89B7)"));
        self.ui_sounds.push(id);
    }

    pub fn go_to_screen(&mut self, env: &mut FeEnv, screen: u32) {
        let Some(event) = screens::nav_event(screen) else {
            self.note(format!("GoToScreen({screen:#x}): no nav event"));
            return;
        };
        self.go_to_screen_by(env, screen, event);
    }

    pub fn go_to_screen_by(&mut self, env: &mut FeEnv, screen: u32, event: &str) {
        if screens::writes_breadcrumb(screen) {
            let key = screens::SCREENS[screen as usize].2;
            let text = env.loc.text(key);
            env.values.set_str(0x33, text);
        }
        if let Some(id) = screen_sound(self.screen, screen) {
            self.play_sound(id);
        }
        self.screen = screen;
        globe::scene_screen(self, env);
        env.nav_events.push(event.to_string());
        self.note(format!("GoToScreen({screen:#x}) → {event}"));
    }

    pub fn construct(&mut self, provider: &str) {
        if provider == "globe" {
            globe::construct(self);
        }
    }

    pub fn update(&mut self, env: &mut FeEnv, dt: f64) {
        self.time += dt;
        scripts::update(self, env.loc, dt as f32);
        self.globe.advance_free = !self.globe_scene.as_ref().is_some_and(|s| s.entity_busy());
        let fe = !self.movie_queued();
        let held = self.movie.is_some();
        self.movie_update();
        if fe {
            boot::update(self, env, dt);
            menu::update(self, env);
            user::update(self, env);
        }
        flow::update(self, env, dt);
        if fe {
            flow::results_update(self, env);
            unlocks::update(self, env);
            loading::update(self, env, dt);
            equipment::update(self, env);
            pause::update(self, env);
        }
        tutorial_pause::update(self, env, fe);
        if fe {
            notify::update(self, env);
            music::update(self, env);
        }
        progression::update(self, env, dt as f32);
        #[cfg(feature = "net")]
        if self.ava.hub.on {
            ava::browser::update(self, env);
            ava::options::update(self, env);
            ava::session::chat::update(self, env);
            ava::session::players::update(self);
        }
        ava::maps::update(self, env);
        flow::level_start_update(self, env);
        if !self.fader_by_ride {
            self.fader.step(dt as f32);
        }
        if std::mem::take(&mut self.movie_finished_latch) {
            self.note("MovieFinishedPlaybackMessage (0xF7067BBE): A_GameMovie");
            self.game_actions.push(("GameMovie".into(), ActionParam::default()));
        }
        flow::load_latches(self, env);
        progression::pending_navigation(self, env);
        if held && self.movie.is_some() {
            self.movie_seen = true;
            if std::mem::take(&mut self.movie_end) {
                self.movie_finished(env);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn screen_changes_play_their_sounds() {
        assert_eq!(screen_sound(screens::id::MAIN_MENU, screens::id::EQUIPMENT), Some(2));
        for from in [screens::id::EXPLORE_RANGE, screens::id::CAMPAIGN_RANGE, screens::id::GLOBAL_RANGE] {
            assert_eq!(screen_sound(from, screens::id::MAIN_MENU), Some(4));
        }
        assert_eq!(screen_sound(0x10, screens::id::MAIN_MENU), None);
        assert_eq!(screen_sound(screens::id::MAIN_MENU, screens::id::EXPLORE_RANGE), None);
    }
}
