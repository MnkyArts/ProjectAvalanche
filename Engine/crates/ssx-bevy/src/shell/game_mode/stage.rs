use std::sync::mpsc::TryRecvError;
use std::time::Instant;

use anyhow::{Context, Result};
use bevy::prelude::{Assets, Camera, Commands, Image, Query, Sprite, Vec2, Visibility, default, error, info};
use ssx_ui::game::GameUi;

use super::world::speech_block;
use super::{LoadedLevel, Shell, Stage, WorldOp, screenshot};
use crate::audio::music::frontend::FrontEndAudio;
use crate::ride::Ride;
use crate::shell::game_level;
use crate::shell::level::{self, LevelData, LevelSpec};
use crate::ui;

impl Shell {
    pub(super) fn fade_shot(&mut self, commands: &mut Commands, alpha: f32, fe_time: f64) {
        if self.fade_shots == 0 {
            return;
        }
        let ended = alpha <= 0.0 && self.fade_last > 0.0;
        self.fade_last = alpha;
        if alpha <= 0.0 && !ended {
            self.fade_frames = 0;
            return;
        }
        if ended || self.fade_frames.is_multiple_of(3) {
            self.fade_shots -= 1;
            let name = format!("fade_{fe_time:07.2}_{alpha:.3}.png");
            screenshot(commands, self.shot_dir.join(name), false);
        }
        self.fade_frames += 1;
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn play_movie(
        &mut self,
        ui: &mut GameUi,
        dt: f64,
        window: Vec2,
        images: &mut Assets<Image>,
        sprites: &mut Query<(&mut Sprite, &mut Visibility)>,
        cameras: &mut Query<&mut Camera>,
        audio: Option<&mut FrontEndAudio>,
    ) {
        let wanted = ui.fe.as_ref().and_then(|x| x.movie.as_ref().map(|m| m.name.clone()));
        let serial = ui.fe.as_ref().map_or(0, |x| x.movie_serial);
        match (&mut self.movie, wanted) {
            (None, Some(_)) if self.movie_ended == Some(serial) => {}
            (None, Some(name)) => {
                let content = ssx_assets::content::GameContent::of(&self.big_root);
                let lang = ui::movie::movie_language(&self.lang);
                let fmv = ssx_assets::content::fmv::movie_id(&name)
                    .ok_or_else(|| anyhow::anyhow!("movie {name}: not a movie id"))
                    .and_then(|id| content.fmv(&id).ok_or_else(|| anyhow::anyhow!("movie {id} not found")));
                match fmv.and_then(|f| ui::movie::MoviePlayback::open_fmv(&name, &f, &content, images, lang)) {
                    Ok(m) => {
                        info!("shell: movie {name} ({})", m.describe());
                        if let Ok((mut sprite, mut v)) = sprites.get_mut(self.movie_sprite) {
                            sprite.image = m.image.clone();
                            *v = Visibility::Visible;
                        }
                        if let Some(a) = audio {
                            a.movie_started();
                            a.movie_audio(m.audio());
                        }
                        self.movie = Some(m);
                    }
                    Err(e) => {
                        error!("{e:#}");
                        self.movie_ended = Some(serial);
                        self.host.finish_movie(ui);
                    }
                }
            }
            (Some(m), Some(name)) if m.name == name => {
                let stopped = ui.fe.as_ref().is_some_and(|fe| fe.movie_stop);
                if m.advance(dt, images) || m.time >= self.movie_cap || stopped {
                    if let Some((id, source, why)) = m.failure() {
                        error!("shell: movie {name} ({id} of {source}) stopped at {:.2} s: {why}", m.time);
                        ssx_assets::content::GameContent::of(&self.big_root).refuse_fmv(id, source, why);
                    }
                    if let Some(counts) = m.picture_counts() {
                        info!("shell: movie {name}: {counts}");
                    }
                    self.movie = None;
                    self.movie_ended = Some(serial);
                    self.host.finish_movie(ui);
                }
            }
            (Some(_), _) => self.movie = None,
            (None, None) => {}
        }
        if self.movie.is_none()
            && let Ok((_, mut v)) = sprites.get_mut(self.movie_sprite)
        {
            *v = Visibility::Hidden;
        }
        if let Some(m) = &self.movie
            && let Some(size) = images.get(&m.image).map(Image::size_f32).filter(|s| s.x > 0.0 && s.y > 0.0)
            && let Ok((mut sprite, _)) = sprites.get_mut(self.movie_sprite)
        {
            let whole = size * (window.x / size.x).min(window.y / size.y);
            if sprite.custom_size != Some(whole) {
                sprite.custom_size = Some(whole);
            }
        }
        if let Ok(mut c) = cameras.get_mut(self.movie_camera) {
            c.is_active = self.movie.is_some();
            c.clear_color = bevy::prelude::ClearColorConfig::Custom(bevy::prelude::Color::BLACK);
        }
        self.host.viewer_movie_frame = self.movie.as_ref().map(ui::movie::MoviePlayback::frame);
    }

    pub(super) fn stage_step(
        &mut self,
        ui: &mut GameUi,
        level: Option<&game_level::LevelGame>,
        ride: Option<&Ride>,
        mut live: Option<&mut crate::shell::live::LiveSession>,
    ) {
        match self.stage {
            Stage::Menus => {
                let Some(launch) = ui.fe.as_ref().and_then(|f| f.launch.clone()) else { return };
                if self.max_rides.is_some_and(|n| self.rides >= n) {
                    return;
                }
                let _ = self.host.frame(ui, 0.0);
                let rules = live.as_deref().filter(|l| !l.ending()).and_then(|l| l.rules);
                if let (Some(l), Some(fe)) = (live.as_deref_mut(), ui.fe.as_mut()) {
                    if l.ending() {
                        return;
                    }
                    l.level = crate::shell::live::LiveLevel::Loading;
                    self.live_away = None;
                    self.live_profile.get_or_insert_with(|| fe.profile.clone());
                    fe.network_session = true;
                }
                let started = match launch.map.clone() {
                    Some(map) => self.start_map_load(ui, &launch, &map, rules),
                    None => self.start_load(ui, &launch, rules),
                };
                match started {
                    Ok(()) => {
                        self.launch = Some(launch);
                        self.stage = Stage::Loading;
                        self.segment_started = false;
                    }
                    Err(e) => {
                        error!("shell: the level did not start: {e:#}");
                        self.map_refused(ui, Some(&launch), &format!("{e:#}"));
                        self.level_lost(ui);
                    }
                }
            }
            Stage::Loading => {
                let Some((rx, since)) = self.loading.as_mut() else { return };
                let received = match rx.get_mut() {
                    Ok(rx) => rx.try_recv(),
                    Err(_) => Err(TryRecvError::Disconnected),
                };
                match received {
                    Ok(Ok(loaded)) => {
                        info!("shell: the level's data arrived after {:.2} s", since.elapsed().as_secs_f32());
                        self.loading = None;
                        self.set_op(WorldOp::Spawn(Box::new(loaded)));
                        self.stage = Stage::Level;
                    }
                    Ok(Err(e)) => {
                        error!("shell: the level did not load: {e:#}");
                        self.loading = None;
                        let launch = self.launch.clone();
                        self.map_refused(ui, launch.as_ref(), &format!("{e:#}"));
                        self.level_lost(ui);
                    }
                    Err(TryRecvError::Empty) => {}
                    Err(TryRecvError::Disconnected) => {
                        error!("shell: the level's loader ended without a level");
                        self.loading = None;
                        let launch = self.launch.clone();
                        self.map_refused(ui, launch.as_ref(), "the level's loader ended without a level");
                        self.level_lost(ui);
                    }
                }
            }
            Stage::Level => {
                if ui.fe.as_ref().is_some_and(|f| !f.in_drop) {
                    if let Some(l) = level {
                        info!("shell: level {} ends: {}", self.rides, l.summary());
                    }
                    let change_gear = ui.fe.as_ref().is_some_and(|f| f.change_gear);
                    let live_level = live.is_some();
                    if let Some(l) = live
                        && !l.ending()
                    {
                        if l.relaunch.is_some() {
                            info!("shell: the network session's level was left for the next round's drop");
                            l.level = crate::shell::live::LiveLevel::Next;
                        } else if change_gear {
                            info!("shell: the network session's level was left for the equipment: the session stays");
                            l.level = crate::shell::live::LiveLevel::Away;
                            self.live_away = Some(super::live::Away::default());
                        } else {
                            info!("shell: the network session's level was left: the server is left");
                            l.level = crate::shell::live::LiveLevel::Left;
                            l.end = Some(crate::shell::live::LiveEnd {
                                reason: None,
                                #[cfg(feature = "net")]
                                refusal: None,
                            });
                        }
                    }
                    let map_level = self.launch.as_ref().is_some_and(|l| l.map.is_some());
                    match ride {
                        Some(_) if map_level => {
                            info!("shell: a map's ride: no stats and no badge progress to the career")
                        }
                        Some(r) if !live_level => {
                            game_level::stats_from_ride(ui, r, &self.stat_names);
                            let n = ui.fe.as_ref().map_or(0, |f| f.badge_progress.len());
                            info!("shell: the badge manager has the progress of {n} badges");
                        }
                        Some(_) => {
                            info!("shell: a network session's ride: no stats and no badge progress to the career")
                        }
                        None => {}
                    }
                    self.set_op(WorldOp::Teardown);
                    self.stats_in = Some(120);
                    self.back_to_menus(ui);
                }
            }
        }
    }

    fn set_op(&mut self, op: WorldOp) {
        if let Ok(o) = self.op.get_mut() {
            *o = Some(op);
        }
    }

    // PORT(choice): the quit path for a level that cannot load; the game has no reachable error screen (neither its nav
    // event `go_to_error_screen` nor its message 0x4211FC81 has a live sender — the one sender, 0x82481B28, has no
    // caller), and a console disc has no missing level.
    fn level_lost(&mut self, ui: &mut GameUi) {
        error!("shell: the level did not come up: back to the front end (no result is posted)");
        ui.fe_call(|fe, env| {
            fe.busy.release(env, ssx_ui::game::fe::busy::LEVEL);
            if fe.profile.new_game_active() && fe.profile.new_game_state != 0 {
                ssx_ui::game::fe::flow::game_quit(fe, env);
            } else {
                ssx_ui::game::fe::flow::do_continue(fe, env);
                ssx_ui::game::fe::flow::leave_level(fe, env);
            }
        });
        let _ = ui.take_requests();
        self.back_to_menus(ui);
    }

    fn back_to_menus(&mut self, ui: &mut GameUi) {
        ui.fe_call(ssx_ui::game::fe::flow::front_end_entered);
        self.stage = Stage::Menus;
        self.launch = None;
        self.rides_done += 1;
        self.next_segment();
    }

    pub(super) fn next_segment(&mut self) {
        if std::mem::replace(&mut self.segment_started, true) {
            return;
        }
        if let Some(seg) = self.segments.pop_front() {
            let t0 = self.host.time;
            self.host.script.extend(seg.into_iter().map(|(t, s)| (t0 + t, s)));
        }
    }

    pub(super) fn write_save(
        &mut self,
        ui: &mut GameUi,
        why: &str,
        audio: Option<&FrontEndAudio>,
        ride: Option<&Ride>,
    ) {
        if self.live_profile.is_some() {
            info!("shell: {why}: not saved (a network session's level)");
            return;
        }
        if let Some(mut a) = audio.and_then(FrontEndAudio::settings) {
            for (id, v) in super::audio_values(&super::settings_values(&ui.values)) {
                a.set_ui(id, v);
            }
            self.save.extras.store_audio(&self.save.schema, &a);
        }
        if let Some(fe) = ui.fe.as_mut() {
            self.save.extras.set_song_flags(&self.save.schema, &fe.music.flags());
            self.save.music_switches_into(fe);
        }
        if let Some(r) = ride {
            game_level::stats_from_ride(ui, r, &self.stat_names);
        }
        if let Some(fe) = ui.fe.as_ref() {
            self.save.write(&fe.profile, why);
        }
    }

    fn start_load(
        &mut self,
        ui: &mut GameUi,
        l: &ssx_ui::game::fe::Launch,
        live: Option<crate::ride::live::LiveRules>,
    ) -> Result<()> {
        let visits = ui.fe.as_mut().and_then(|fe| ssx_ui::game::fe::flow::count_range_visit(fe, l));
        info!("shell: {} counts visit {visits:?} of its range by {} (the intro's path)", l.event, l.rider);
        let fe = ui.fe.as_ref().context("no front end")?;
        let ge_drop = fe.career.event(l.drop).map(|e| e.drop).context("the launched drop has no event")?;
        info!("shell: game setup (0x82498BA0): {}", l.setup.describe(&fe.career));
        let n = self.rides + 1;
        let track_dir = ssx_assets::data_roots::track_dir(&self.big_root, &l.track)
            .with_context(|| format!("{}: no track folder (the disc's data0 or a mounted package)", l.track))?;
        let drop_index = game_level::drop_index(&self.big_root, &l.track, ge_drop)?;
        let spec = LevelSpec {
            track_dir,
            big_root: Some(self.big_root.clone()),
            subdivide: self.subdivide.unwrap_or(ssx_assets::subdiv::GAME_LEVELS as u32),
            shading: self.shading,
            objects: !self.no_objects,
            drop: crate::render::lighting::DropSelect::Index(drop_index),
            drop_explicit: true,
            camera: None,
            look_at: None,
            aspect: self.aspect,
            ride: Some(level::RideOptions {
                event_key: Some(l.drop),
                free_ride: l.free_ride,
                collected_geotags: fe.profile.collected_geotags.iter().copied().collect(),
                game: true,
                classic: ui.values.int(ssx_ui::game::values::id::SETTINGS_GAMEPLAY_CONTROLLER_SCHEME) != 0,
                autoplay: self.autoplay,
                setup: Some(ssx_ui::game::ride::game_setup(&l.setup)),
                setup_presentation: l.setup.presentation.as_deref().map(ssx_ui::game::ride::game_setup),
                intro_visits: visits,
                own_tombstone: ssx_ui::game::ride::own_tombstone_for(ui, l.drop),
                live,
                event_name: Some(l.event.clone()),
                ..default()
            }),
            character: l.rider.clone(),
            screenshot: false,
            map: None,
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
            win_after: self.ride_win,
        };
        let (tx, rx) = std::sync::mpsc::channel();
        let big_root = self.big_root.clone();
        let speech_lang = self.speech_lang;
        let song_flags = fe.music.switches();
        let block_in = fe.career.event(l.drop).map(|e| {
            use ssx_assets::sfx::speech::launch;
            let words = launch::campaign_words(&fe.profile, l.drop, e.deadly_descent);
            let required = launch::required_gear(&e.required, e.forced.len());
            let descriptions = l.setup.gear.map(|g| (!g.is_cleared()).then_some(g.description));
            (words, required, descriptions)
        });
        std::thread::Builder::new()
            .name(format!("level {n} loader"))
            .spawn(move || {
                let r = (|| -> Result<LoadedLevel> {
                    let data = LevelData::load(&spec)?;
                    let root = Some(big_root.as_path());
                    let drop_key = data.lighting.as_ref().map(|l| l.drop_key);
                    let event = spec.ride.as_ref().and_then(|r| r.event_key);
                    let music =
                        crate::audio::music::load_level_event(root, &data.name, drop_key, event, &song_flags, false);
                    let sfx = crate::audio::sfx::load_level_event(
                        root,
                        &data.name,
                        &spec.character,
                        data.max_boost(),
                        event,
                        speech_lang,
                        false,
                    );
                    let game = game_level::LevelLoad::load(opts, &data)?;
                    let speech_block = block_in.map(|(words, required, descriptions)| {
                        speech_block(&big_root, &data.name, &spec.character, words, required, descriptions)
                    });
                    Ok(LoadedLevel { data, music, sfx, game, speech_block })
                })();
                let _ = tx.send(r);
            })
            .context("starting the level's loader")?;
        info!("shell: level {n}: {} on {} (ge_drop {ge_drop:#010x}), rider {}: loading", l.event, l.track, l.rider);
        self.loading = Some((std::sync::Mutex::new(rx), Instant::now()));
        self.rides = n;
        Ok(())
    }

    pub(super) fn print_logs(&mut self, ui: &GameUi) {
        if let Some(fe) = ui.fe.as_ref() {
            for l in &fe.log[self.log_seen.0.min(fe.log.len())..] {
                info!("fe {l}");
            }
            self.log_seen.0 = fe.log.len();
        }
        for l in &self.host.log[self.log_seen.1.min(self.host.log.len())..] {
            info!("{l}");
        }
        self.log_seen.1 = self.host.log.len();
    }
}
