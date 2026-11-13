use std::path::PathBuf;

use eframe::egui;
use ssx_assets::setup::DiscCheck;

use crate::art::{self, Gallery};
use crate::catalog::{self, DropCache, Rider};
use crate::command::{self, Invocation};
use crate::dlc_list::DlcPage;
use crate::jobs::{self, Background, SetupJob, Viewer};
use crate::mod_list::ModsPage;
use crate::settings::Settings;
use crate::tu_list::TuPage;
use crate::tuner_page::TunerPage;
use crate::ui::{ExtrasTab, MenuItem, ModsTab, Page, SettingsTab};
use crate::update::Update;

pub struct ScreenshotPlan {
    pub path: PathBuf,
    pub requested: bool,
}

pub struct Launcher {
    pub settings: Settings,
    pub saved: Settings,
    pub data_state: ssx_assets::folder::State,
    pub data_ready: bool,
    pub data_disc: Option<String>,
    pub languages: Vec<(&'static str, &'static str)>,
    pub page: Page,
    pub cursor: MenuItem,
    pub settings_tab: SettingsTab,
    pub extras_tab: ExtrasTab,
    pub mods_tab: ModsTab,
    pub mods: ModsPage,
    pub dlc: DlcPage,
    pub tuner: TunerPage,
    pub tu: TuPage,
    pub app_dir: PathBuf,
    pub save_path: PathBuf,
    pub save_exists: bool,
    pub files_frame: Option<u64>,
    pub save_synced: bool,
    pub dlc_root: PathBuf,
    pub tuner_root: PathBuf,
    pub tu_root: PathBuf,
    pub host: crate::host::HostPage,
    pub last_page: Page,
    pub update: Update,
    pub program_missing: Vec<&'static str>,
    pub gallery: Gallery,
    pub check: Option<Result<DiscCheck, String>>,
    pub checking: Option<Background<Result<DiscCheck, String>>>,
    pub job: Option<SetupJob>,
    pub setup_message: Option<(bool, String)>,
    pub tracks: Vec<String>,
    pub riders: Result<Vec<Rider>, String>,
    pub drops: DropCache,
    pub viewer: Option<Viewer>,
    pub started_game: bool,
    pub displays: crate::display::Displays,
    pub settings_save: crate::settings_flow::SettingsSave,
    pub start_after_save: Option<(Invocation, bool)>,
    pub launch_error: Option<String>,
    pub show_command: bool,
    pub screenshot: Option<ScreenshotPlan>,
    pub auto_extract: bool,
    pub shot_after_setup: bool,
    pub auto_launch: bool,
    pub auto_launch_pending: bool,
    pub auto_play_pending: bool,
    pub link: Option<String>,
    pub link_note: Option<String>,
    pub link_status: Option<crate::link_handler::Status>,
    pub forced_hover: Option<String>,
    pub scripted_keys: std::collections::VecDeque<(egui::Key, bool)>,
    pub scroll: f32,
    pub window_pixels: Option<[f32; 2]>,
    pub frame: u64,
    pub timing: Option<std::time::Instant>,
}

fn program_missing() -> Vec<&'static str> {
    if !crate::update::install::release_build() {
        return Vec::new();
    }
    crate::update::install::install_dir().map(|dir| crate::update::install::missing(&dir)).unwrap_or_default()
}

pub fn art_request(settings: &Settings, ready: bool) -> art::Request {
    art::Request { app_dir: crate::settings::app_dir(), game_data: ready.then(|| settings.game_data()) }
}

impl Launcher {
    pub fn new(
        ctx: &egui::Context,
        settings: Settings,
        screenshot: Option<PathBuf>,
        gallery: Gallery,
        auto_update: bool,
        data_state: ssx_assets::folder::State,
    ) -> Self {
        gallery.attach(ctx);
        let app_dir = crate::settings::app_dir();
        let save_path = crate::command::save_path_in(&app_dir);
        let mut app = Self {
            saved: settings.clone(),
            data_state: ssx_assets::folder::State::Missing,
            data_ready: false,
            data_disc: None,
            languages: Vec::new(),
            settings,
            page: Page::Play,
            cursor: MenuItem::Page(Page::Play),
            settings_tab: SettingsTab::default(),
            extras_tab: ExtrasTab::default(),
            mods_tab: ModsTab::default(),
            mods: ModsPage::default(),
            dlc: DlcPage::default(),
            tuner: TunerPage::default(),
            tu: TuPage::default(),
            save_exists: save_path.is_file(),
            save_path,
            files_frame: None,
            save_synced: true,
            dlc_root: DlcPage::root(&app_dir),
            tuner_root: TunerPage::root(&app_dir),
            tu_root: TuPage::root(&app_dir),
            app_dir,
            host: crate::host::HostPage::default(),
            last_page: Page::Play,
            update: Update::Idle,
            program_missing: program_missing(),
            gallery,
            check: None,
            checking: None,
            job: None,
            setup_message: None,
            tracks: Vec::new(),
            riders: Ok(Vec::new()),
            drops: DropCache::default(),
            viewer: None,
            started_game: false,
            displays: Default::default(),
            settings_save: Default::default(),
            start_after_save: None,
            launch_error: None,
            show_command: false,
            screenshot: screenshot.map(|path| ScreenshotPlan { path, requested: false }),
            auto_extract: false,
            shot_after_setup: false,
            auto_launch: false,
            auto_launch_pending: false,
            auto_play_pending: false,
            link: None,
            link_note: None,
            link_status: None,
            forced_hover: None,
            scripted_keys: Default::default(),
            scroll: 0.0,
            window_pixels: None,
            frame: 0,
            timing: None,
        };
        app.set_data_state(data_state);
        app.refresh_mods(ctx);
        app.refresh_dlc(ctx);
        app.refresh_tuner(ctx);
        app.refresh_tu(ctx);
        if app.data_ready {
            app.refresh_catalog();
        } else if let Some(iso) = app.settings.iso.clone().filter(|p| p.is_file()) {
            app.checking = Some(jobs::inspect(ctx, iso));
        }
        if auto_update {
            app.update = Update::start_check(ctx);
        }
        app
    }

    pub fn go(&mut self, page: Page) {
        (self.page, self.cursor) = (page, MenuItem::Page(page));
    }

    pub fn go_to_setup(&mut self) {
        self.go(Page::Settings);
        self.settings_tab = SettingsTab::GameData;
    }

    pub fn refresh_catalog(&mut self) {
        let big = self.settings.big_root();
        self.tracks = catalog::tracks(&big);
        self.riders = catalog::riders(&big).map_err(|e| format!("{e:#}"));
        self.drops.clear();
        if !self.tracks.contains(&self.settings.track)
            && let Some(first) = self.tracks.first()
        {
            self.settings.track = first.clone();
        }
    }

    pub fn data_changed(&mut self) {
        self.read_data_state();
        if self.data_ready {
            self.refresh_catalog();
            self.gallery.restart(art_request(&self.settings, self.data_ready));
        }
    }

    pub fn read_data_state(&mut self) {
        self.set_data_state(self.settings.data_state());
    }

    fn set_data_state(&mut self, state: ssx_assets::folder::State) {
        let data = self.settings.game_data();
        self.data_ready = state.playable();
        self.data_state = state;
        self.data_disc =
            ssx_assets::setup::read_marker(&data).and_then(|m| m["fingerprint"].as_str().map(str::to_owned));
        self.languages = crate::language::available(&self.settings.big_root());
    }

    pub fn running(&self) -> bool {
        self.viewer.as_ref().is_some_and(|v| v.status().is_none())
    }

    pub fn start(&mut self, ctx: &egui::Context, inv: &Invocation, game: bool) {
        self.launch_error = None;
        if !inv.program.is_file() {
            let path = inv.program.display();
            self.launch_error = Some(if game {
                format!("the game is missing: {path} (it ships next to the launcher)")
            } else {
                format!("the asset viewer is missing: {path} (a development tool, built from the source)")
            });
            return;
        }
        if self.start_after_saving(ctx, inv, game) {
            return;
        }
        match Viewer::spawn(ctx, inv, if game { jobs::GAME_LOG } else { jobs::VIEWER_LOG }) {
            Ok(v) => {
                (self.viewer, self.started_game) = (Some(v), game);
                self.displays.synced = !game;
                self.save_synced = false;
            }
            Err(e) => self.launch_error = Some(format!("could not start {}: {e}", inv.program.display())),
        }
    }

    pub fn play(&mut self, ctx: &egui::Context) {
        let game = command::game(&self.settings, &self.settings.big_root());
        self.start(ctx, &game, true);
    }
}
