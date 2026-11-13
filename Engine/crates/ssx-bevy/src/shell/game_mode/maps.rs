use anyhow::{Context, Result};
use bevy::prelude::{default, error, info, warn};
use ssx_assets::content::GameContent;
use ssx_ui::game::GameUi;
use ssx_ui::game::fe::ava::maps::{self, MapInfo, MapRequest, MapRow, ModeInfo};
use ssx_ui::game::fe::{Launch, MapLaunch};

use super::{LoadedLevel, Shell};
use crate::shell::game_level;
use crate::shell::level::{self, LevelData, LevelSpec};

const LAUNCH_MAP_WAIT: f64 = 20.0;

pub(super) fn list(content: &GameContent) -> (Vec<MapRow>, usize) {
    let (listed, more) = content.listed_maps();
    let rows = listed
        .into_iter()
        .map(|m| MapRow {
            id: m.id.to_string(),
            package: m.package.clone(),
            entry: m.toml.map(|t| MapInfo {
                modes: content.map_modes(&m.id, &t).iter().map(mode_info).collect(),
                title: t.title,
                base: t.base.map(|b| b.path().to_string()),
            }),
        })
        .collect();
    (rows, more)
}

fn mode_info(m: &ssx_assets::content::gamemodes::MapMode) -> ModeInfo {
    ModeInfo {
        choice: m.choice.name(),
        title: m.title.clone(),
        event_type: i64::from(m.event_type),
        targets: m.medal_targets(),
        time_limit_ms: m.rules.as_ref().ok().and_then(|r| r.time_limit_ms()),
        refused: m.rules.as_ref().err().cloned(),
    }
}

impl Shell {
    pub(super) fn maps_frame(&mut self, ui: &mut GameUi) {
        for r in ui.take_map_requests() {
            match r {
                MapRequest::List => {
                    let (rows, more) = list(&GameContent::of(&self.big_root));
                    ui.fe_call(|fe, _| maps::listed(fe, rows, more));
                }
            }
        }
        let Some((id, at)) = self.test_map.clone() else { return };
        let mode = self.test_map_mode.clone();
        if self.stage != super::Stage::Menus || self.host.time < at {
            return;
        }
        let hub = ui.fe.as_ref().is_some_and(|fe| fe.ava.hub.active || fe.ava.maps.active);
        if hub && !ui.views_loading() {
            self.test_map = None;
            let (rows, more) = list(&GameContent::of(&self.big_root));
            let mut launched = Ok(());
            ui.fe_call(|fe, env| {
                maps::listed(fe, rows, more);
                launched = maps::ride(fe, env, &id, mode.as_deref());
            });
            match launched {
                Ok(()) => info!("shell: --launch-map {id}: launched"),
                Err(why) => {
                    error!("shell: --launch-map {id}: refused: {}", ssx_core::text::Text::new(&why));
                    ui.fe_call(|fe, env| {
                        maps::refused(fe, &why);
                        fe.go_to_screen_by(env, ssx_ui::game::fe::screens::id::MAIN_MENU, maps::OPEN);
                    });
                }
            }
        } else if self.host.time - at > LAUNCH_MAP_WAIT {
            self.test_map = None;
            error!("shell: --launch-map {id}: the Project Avalanche menu did not come up within {LAUNCH_MAP_WAIT} s");
        } else {
            self.boot_press(ui, &format!("--launch-map {id}"));
        }
    }

    pub(super) fn start_map_load(
        &mut self,
        ui: &mut GameUi,
        l: &Launch,
        map: &MapLaunch,
        live: Option<crate::ride::live::LiveRules>,
    ) -> Result<()> {
        let fe = ui.fe.as_ref().context("no front end")?;
        let id: ssx_content::ContentId = map.id.parse().map_err(|e| anyhow::anyhow!("{}: {e}", map.id))?;
        info!("shell: game setup (0x82498BA0): {}", l.setup.describe(&fe.career));
        let n = self.rides + 1;
        // PORT(choice): a map's intro path counts the map's rides in this game (the first Long, later Medium) as a
        // drop's counts its range's visits (`NumTimesVisitedRange_<range><Rider>`, FUN_82385F28, persisted in the
        // profile); a map has no range and the profile no record for it, so the count is not saved and starts again
        // with the game; a profile record per map would keep it; maps are not the game's.
        let visits = {
            let c = self.map_rides.entry(map.id.clone()).or_insert(0);
            *c = c.saturating_add(1);
            *c
        };
        let tracks = self.big_root.join("data0/data/xenon/gameexplorer");
        let spec = LevelSpec {
            track_dir: if map.track.is_empty() { tracks } else { tracks.join(&map.track) },
            big_root: Some(self.big_root.clone()),
            subdivide: self.subdivide.unwrap_or(ssx_assets::subdiv::GAME_LEVELS as u32),
            shading: self.shading,
            objects: !self.no_objects,
            drop: crate::render::lighting::DropSelect::Index(0),
            drop_explicit: false,
            camera: None,
            look_at: None,
            aspect: self.aspect,
            ride: Some(level::RideOptions {
                game: true,
                classic: ui.values.int(ssx_ui::game::values::id::SETTINGS_GAMEPLAY_CONTROLLER_SCHEME) != 0,
                autoplay: self.autoplay,
                setup: Some(ssx_ui::game::ride::game_setup(&l.setup)),
                map_mode: Some(map.mode.clone()),
                intro_visits: Some(visits),
                live,
                ..default()
            }),
            character: l.rider.clone(),
            screenshot: false,
            map: Some(id.clone()),
        };
        let shots = self
            .ride_shots
            .iter()
            .map(|&t| (t, self.shot_dir.join(format!("game2_ride{n}_{}.png", t as u32))))
            .collect();
        let opts = game_level::LevelOptions {
            launch: l.clone(),
            stat_names: self.stat_names.clone(),
            big_root: self.big_root.clone(),
            shots,
            stop_after: self.ride_limits.get(n as usize - 1).copied().filter(|&s| s > 0.0),
            script: self.level_scripts.get(n as usize - 1).cloned().unwrap_or_default(),
            shot_dir: self.shot_dir.clone(),
            debug_hud: self.debug_hud,
            autoplay: self.autoplay,
            follow_prompts: self.follow_prompts,
            win_after: None,
        };
        let (tx, rx) = std::sync::mpsc::channel();
        let big_root = self.big_root.clone();
        let speech_lang = self.speech_lang;
        let song_flags = fe.music.switches();
        let event_type = l.event_type;
        std::thread::Builder::new()
            .name(format!("level {n} loader (map)"))
            .spawn(move || {
                let r = (|| -> Result<LoadedLevel> {
                    let data = LevelData::load(&spec)?;
                    let root = Some(big_root.as_path());
                    let music = crate::audio::music::map::load_map_level(
                        root,
                        data.map_music.as_ref(),
                        event_type,
                        &song_flags,
                    );
                    let sfx = crate::audio::sfx::load_level_event(
                        root,
                        &data.name,
                        &spec.character,
                        data.max_boost(),
                        None,
                        speech_lang,
                        false,
                    );
                    let game = game_level::LevelLoad::load(opts, &data)?;
                    Ok(LoadedLevel { data, music, sfx, game, speech_block: None })
                })();
                let _ = tx.send(r);
            })
            .context("starting the level's loader")?;
        let on = if map.track.is_empty() { "a new track" } else { map.track.as_str() };
        info!("shell: level {n}: the map {} ({}) in {} on {on}, rider {}: loading", map.id, l.event, map.mode, l.rider);
        self.loading = Some((std::sync::Mutex::new(rx), std::time::Instant::now()));
        self.rides = n;
        Ok(())
    }

    pub(super) fn map_refused(&mut self, ui: &mut GameUi, launch: Option<&Launch>, why: &str) {
        let Some(map) = launch.and_then(|l| l.map.as_ref()) else { return };
        warn!("shell: the map {} is refused: {}", map.id, ssx_core::text::Text::new(why));
        ui.fe_call(|fe, _| maps::refused(fe, why));
    }
}
