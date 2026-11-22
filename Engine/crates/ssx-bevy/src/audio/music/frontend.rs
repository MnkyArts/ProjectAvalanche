use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::sync::{Arc, Mutex};

use bevy::prelude::*;
use ssx_assets::content::audio::{AudioScope, MusicDecision};
use ssx_assets::musicbox::{LicensedMusic, MUSIC_SUBMIX_LEVEL, MusicBox, SongSwitches};
use ssx_assets::sfx::frontend::{
    BeatStatus, FlowInputs, FlowState, FrontEndMusic, GlobeWind, HudPause, PostSlot, StateMsg, UiContext, UiSounds,
    post_index,
};
use ssx_assets::sfx::movie::MovieAudio;
use ssx_assets::sfx::music_pause::MusicPause;
use ssx_assets::sfx::settings::AudioSettings;
use ssx_assets::sfx::speech::pause::SpeechPause;
use ssx_assets::sfx::{MusicSource, OutputMix, SfxEngine};

use super::{Back, GameAudio, back_log, discard, output};

mod reload;
pub use reload::FrontEndReload;

#[derive(Resource, Clone, Copy)]
pub struct AudioSettingsRes(pub AudioSettings);

struct FeMusicBus(Arc<Mutex<FrontEndMusic>>, Arc<Mutex<Option<Preview>>>, MenuSongs, Arc<AtomicU64>);

type MenuSongs = Arc<Mutex<Option<(LicensedMusic, Vec<f32>)>>>;

impl MusicSource for FeMusicBus {
    fn render(&mut self, out: &mut [f32]) {
        out.fill(0.0);
        if let Ok(mut m) = self.0.lock() {
            m.music.render(out);
        }
        if let Ok(mut p) = self.1.lock()
            && let Some(pv) = p.as_mut()
        {
            pv.render_add(out);
            if pv.stopped {
                self.3.fetch_max(pv.id, Relaxed);
                *p = None;
            }
        }
        if let Ok(mut menu) = self.2.lock()
            && let Some((lm, buf)) = menu.as_mut()
        {
            buf.resize(out.len(), 0.0);
            lm.render(buf, MUSIC_SUBMIX_LEVEL);
            for (o, x) in out.iter_mut().zip(buf.iter()) {
                *o += x;
            }
            if lm.finished() {
                *menu = None;
            }
        }
    }

    fn finished(&self) -> bool {
        self.0.lock().is_ok_and(|m| m.finished())
            && self.1.lock().is_ok_and(|p| p.is_none())
            && self.2.lock().is_ok_and(|m| m.is_none())
    }
}

struct Preview {
    id: u64,
    player: LicensedMusic,
    level: f32,
    fade: Option<(f32, u32)>,
    stopped: bool,
    buf: Vec<f32>,
}

const PREVIEW_FADE_SAMPLES: u32 = 22_050;

impl Preview {
    fn render_add(&mut self, out: &mut [f32]) {
        if self.stopped {
            return;
        }
        self.buf.resize(out.len(), 0.0);
        self.player.render(&mut self.buf, 1.0);
        let (ins, _) = self.buf.as_chunks::<2>();
        for (o, x) in out.as_chunks_mut::<2>().0.iter_mut().zip(ins) {
            if let Some((step, left)) = self.fade.as_mut() {
                if *left == 0 {
                    self.stopped = true;
                    break;
                }
                self.level = (self.level - *step).max(0.0);
                *left -= 1;
            }
            o[0] += x[0] * self.level;
            o[1] += x[1] * self.level;
        }
        if !self.player.playing() {
            self.stopped = true;
        }
    }
}

pub struct FrontEndAudioData {
    engine: SfxEngine,
    music: Option<FrontEndMusic>,
    plan: Arc<MusicDecision>,
    menu_plan: Arc<MusicDecision>,
    big_root: std::path::PathBuf,
}

struct FeSide {
    parked: Option<SfxEngine>,
    ui: UiSounds,
    posts: PostSlot,
    beat: Option<BeatStatus>,
    hud_pause: HudPause,
    globe_wind: GlobeWind,
    music_pause: MusicPause,
}

#[derive(Resource)]
pub struct FrontEndAudio {
    game: GameAudio,
    side: Arc<Mutex<FeSide>>,
    music: Option<Arc<Mutex<FrontEndMusic>>>,
    preview: Arc<Mutex<Option<Preview>>>,
    preview_asked: u64,
    preview_stopped: Arc<AtomicU64>,
    plugged: bool,
    flow: FlowState,
    beat: Option<BeatStatus>,
    settings: AudioSettings,
    last: StateMsg,
    results: (bool, bool),
    movie: Option<Arc<MovieAudio>>,
    screen_state: i32,
    setting_67: bool,
    popup_menu: bool,
    verbose: bool,
    speech_pause: SpeechPause,
    plan: Arc<MusicDecision>,
    menu_plan: Arc<MusicDecision>,
    menu: MenuSongs,
    switches: SongSwitches,
    level_loading: bool,
    big_root: std::path::PathBuf,
}

pub fn load(big_root: Option<&Path>) -> Option<FrontEndAudioData> {
    let mode = std::env::var("SSX_SFX").unwrap_or_default().to_ascii_lowercase();
    if matches!(mode.as_str(), "off" | "0" | "false") {
        return None;
    }
    let big_root = big_root?;
    let mut engine = match SfxEngine::load(big_root, &["fe_sfx"]) {
        Ok(e) => e,
        Err(e) => {
            ssx_core::eprintln!("warning: front-end audio unavailable: {e:#}");
            return None;
        }
    };
    let content = ssx_assets::content::GameContent::of(big_root);
    let sounds = content.sound_overrides(None);
    if !sounds.sounds.is_empty() {
        for id in engine.set_overrides(&sounds.sounds) {
            ssx_core::eprintln!("warning: front-end audio: {id}: no interface of the front end's engine defines it");
        }
    }
    let plan = content.music_plan(&AudioScope::FrontEnd);
    let menu_plan = content.music_plan(&AudioScope::MusicMenu);
    let music_mode = std::env::var("SSX_MUSIC").unwrap_or_default().to_ascii_lowercase();
    let mut music = if matches!(music_mode.as_str(), "off" | "0" | "false") {
        None
    } else {
        FrontEndMusic::load(big_root, ssx_assets::career::random::Source::Global)
            .map_err(|e| ssx_core::eprintln!("warning: front-end music unavailable: {e:#}"))
            .ok()
    };
    if let Some(m) = music.as_mut()
        && !plan.music.is_empty()
    {
        let mut log = Vec::new();
        ssx_assets::content::audio::apply::apply_project(&mut m.music, "ssx_fe", &plan.music, &mut log);
        log.iter().for_each(|l| ssx_core::eprintln!("{l}"));
    }
    Some(FrontEndAudioData { engine, music, plan, menu_plan, big_root: big_root.to_path_buf() })
}

pub fn start(world: &mut World, data: Option<FrontEndAudioData>) {
    let Some(data) = data else { return };
    let Some(g) = world.get_resource::<GameAudio>().cloned() else {
        ssx_core::eprintln!("warning: front-end audio: no output (music::frontend::register)");
        return;
    };
    let mut data = data;
    match world.get_resource::<AudioSettingsRes>() {
        Some(s) => data.engine.apply_settings(&s.0),
        None => world.insert_resource(AudioSettingsRes(data.engine.settings)),
    }
    let settings = data.engine.settings;
    let side = FeSide {
        parked: Some(data.engine),
        ui: UiSounds::default(),
        posts: PostSlot::default(),
        beat: None,
        hud_pause: HudPause::default(),
        globe_wind: GlobeWind::default(),
        music_pause: MusicPause::default(),
    };
    let mut fe = FrontEndAudio {
        game: g,
        side: Arc::new(Mutex::new(side)),
        music: data.music.map(|m| Arc::new(Mutex::new(m))),
        preview: Arc::new(Mutex::new(None)),
        preview_asked: 0,
        preview_stopped: Arc::new(AtomicU64::new(0)),
        plugged: false,
        flow: FlowState::default(),
        beat: None,
        settings,
        last: StateMsg::default(),
        results: (false, false),
        movie: None,
        screen_state: 0,
        setting_67: false,
        popup_menu: false,
        verbose: std::env::var("SSX_FE_AUDIO_LOG").is_ok_and(|v| v == "1"),
        speech_pause: SpeechPause::default(),
        plan: data.plan,
        menu_plan: data.menu_plan,
        menu: Arc::new(Mutex::new(None)),
        switches: SongSwitches::default(),
        level_loading: false,
        big_root: data.big_root,
    };
    fe.plug();
    world.insert_resource(fe);
}

pub fn register(app: &mut App) {
    output(app);
}

pub fn apply_audio_settings(world: &mut World, ui_values: &[(u32, i32)]) {
    let Some(g) = world.get_resource::<GameAudio>().cloned() else { return };
    let fe = world.get_resource::<FrontEndAudio>().map(|fe| fe.side.clone());
    let Some(base) = world.get_resource::<AudioSettingsRes>().map(|s| s.0) else {
        let values = ui_values.to_vec();
        g.post(move |m, back| {
            let Some(e) = m.sfx.as_mut() else { return };
            let mut s = e.settings;
            for &(id, v) in &values {
                s.set_ui(id, v);
            }
            apply_settings_to(e, fe.as_ref(), &s);
            back.reply(move |w| w.insert_resource(AudioSettingsRes(s)));
        });
        return;
    };
    let mut s = base;
    for &(id, v) in ui_values {
        s.set_ui(id, v);
    }
    world.insert_resource(AudioSettingsRes(s));
    if let Some(mut fe) = world.get_resource_mut::<FrontEndAudio>() {
        fe.settings = s;
    }
    g.post(move |m, _| {
        if let Some(e) = m.sfx.as_mut() {
            apply_settings_to(e, fe.as_ref(), &s);
        }
    });
}

pub fn apply_stored_settings(world: &mut World, s: &AudioSettings) {
    world.insert_resource(AudioSettingsRes(*s));
    let fe = world.get_resource::<FrontEndAudio>().map(|fe| fe.side.clone());
    if let Some(mut f) = world.get_resource_mut::<FrontEndAudio>() {
        f.settings = *s;
    }
    let Some(g) = world.get_resource::<GameAudio>() else { return };
    let s = *s;
    g.post(move |m, _| {
        if let Some(e) = m.sfx.as_mut() {
            apply_settings_to(e, fe.as_ref(), &s);
        }
    });
}

fn apply_settings_to(plugged: &mut SfxEngine, fe: Option<&Arc<Mutex<FeSide>>>, s: &AudioSettings) {
    plugged.apply_settings(s);
    if let Some(side) = fe
        && let Ok(mut side) = side.lock()
        && let Some(e) = side.parked.as_mut()
    {
        e.apply_settings(s);
    }
}

impl FrontEndAudio {
    pub fn settings(&self) -> Option<AudioSettings> {
        Some(self.settings)
    }

    fn post<F: FnOnce(&mut OutputMix, &mut FeSide, &Back) + Send + 'static>(&self, f: F) {
        let side = self.side.clone();
        self.game.post_as(std::any::type_name::<F>(), move |m, back| {
            if let Ok(mut side) = side.lock() {
                f(m, &mut side, back);
            }
        });
    }

    pub fn plug(&mut self) {
        if self.plugged {
            return;
        }
        self.arm_menu_songs();
        let music: Option<Box<dyn MusicSource>> = self.music.as_ref().map(|m| {
            Box::new(FeMusicBus(m.clone(), self.preview.clone(), self.menu.clone(), self.preview_stopped.clone())) as _
        });
        self.post(move |m, side, _| {
            let engine = side.parked.take();
            let old_sfx = engine.is_some().then(|| std::mem::replace(&mut m.sfx, engine));
            if let (Some(Some(level)), Some(fe)) = (old_sfx.as_ref(), m.sfx.as_mut()) {
                fe.aems.set_rng_state(level.aems.rng_state());
            }
            let level_music = std::mem::replace(&mut m.music, music);
            let tail = std::mem::replace(&mut m.music_tail, level_music.filter(|b| !b.finished()));
            (m.remix_event_type, m.swapped) = (1, true);
            side.music_pause = MusicPause::default();
            discard((old_sfx, tail));
        });
        self.plugged = true;
        self.reset_pauses();
    }

    fn arm_menu_songs(&mut self) {
        if self.plan.music.added.iter().all(|s| !s.song.menu) {
            return;
        }
        match MusicBox::load(&self.big_root) {
            Ok(mut mb) => {
                mb.add_packages(&self.plan.music);
                mb.apply_switches(&self.switches);
                let (menu, songs) = (self.menu.clone(), LicensedMusic::menu(mb));
                self.game.post(move |_, _| {
                    if let Ok(mut m) = menu.lock() {
                        discard(m.replace((songs, Vec::new())));
                    }
                });
            }
            Err(e) => ssx_core::eprintln!("warning: the menus' package songs: {e:#}"),
        }
    }

    pub fn set_song_switches(&mut self, switches: &SongSwitches) {
        if self.switches == *switches {
            return;
        }
        self.switches = switches.clone();
        let (menu, switches) = (self.menu.clone(), switches.clone());
        self.game.post(move |_, _| {
            if let Ok(mut m) = menu.lock()
                && let Some((lm, _)) = m.as_mut()
            {
                lm.mb.apply_switches(&switches);
            }
        });
    }

    pub fn set_level_loading(&mut self, loading: bool) {
        self.level_loading = loading;
    }

    fn reset_pauses(&mut self) {
        self.speech_pause = SpeechPause::default();
        ssx_assets::musicbox::set_paused(false);
    }

    pub fn set_unplugged_sounds(&self, sounds: &ssx_assets::sfx_override::SoundOverrides) {
        let sounds = sounds.clone();
        self.post(move |_, side, _| {
            if let Some(e) = side.parked.as_mut() {
                e.set_overrides(&sounds);
            }
        });
    }

    pub fn hand_over_to_level(&self, goals: [i32; 2], block: Option<ssx_assets::sfx::speech::launch::LaunchBlock>) {
        self.post(move |m, side, _| {
            let rng = side.parked.as_ref().map(|e| e.aems.rng_state());
            if let Some(sfx) = m.sfx.as_mut() {
                sfx.ui_goals = goals;
                sfx.launch_block = block;
                if let Some(rng) = rng {
                    sfx.aems.set_rng_state(rng);
                }
            }
        });
    }

    pub fn unplug(&mut self) {
        if !self.plugged {
            return;
        }
        self.post(|m, side, _| {
            side.parked = m.sfx.take();
            let music = m.music.take();
            discard(std::mem::replace(&mut m.music_tail, music.filter(|b| !b.finished())));
            m.swapped = true;
            side.music_pause = MusicPause::default();
        });
        self.plugged = false;
        self.reset_pauses();
    }

    pub fn preview(&mut self, big_root: &Path, id: u32, key: Option<&ssx_content::ContentId>) {
        if self.music.is_none() {
            return;
        }
        let mut mb = match MusicBox::load(big_root) {
            Ok(mb) => mb,
            Err(e) => {
                ssx_core::eprintln!("warning: the music menu's preview: {e:#}");
                return;
            }
        };
        if !self.menu_plan.music.is_empty() {
            mb.add_packages(&self.menu_plan.music);
        }
        let id = match key {
            None => id,
            Some(k) => match mb.package.keys.iter().find(|(_, key)| *key == k) {
                Some((&id, _)) => id,
                None => {
                    warn!("music: no preview: {k} is no longer a song of the packages' music");
                    return;
                }
            },
        };
        let player = LicensedMusic::song(mb, id);
        let label = player.mb.song(id).map_or_else(|| format!("{id:#x}"), |s| s.label());
        info!("music: preview {label} (PlaySong {id:#x})");
        self.preview_asked += 1;
        let preview = Preview {
            id: self.preview_asked,
            player,
            level: MUSIC_SUBMIX_LEVEL,
            fade: None,
            stopped: false,
            buf: Vec::new(),
        };
        let (menu, slot) = (self.menu.clone(), self.preview.clone());
        self.game.post(move |m, back| {
            if let Ok(mut m) = menu.lock()
                && let Some((lm, _)) = m.as_mut()
            {
                lm.stop(0.0);
            }
            if let Ok(mut p) = slot.lock() {
                discard(p.replace(preview));
            }
            if let Some(tail) = m.music_tail.take() {
                discard(tail);
                back_log!(back, info, "music: the ride's song left playing stops for the preview (StopMusic)");
            }
        });
    }

    pub fn stop_preview(&mut self) {
        self.stop_preview_over(PREVIEW_FADE_SAMPLES);
    }

    fn stop_preview_over(&mut self, samples: u32) {
        let slot = self.preview.clone();
        self.game.post(move |_, back| {
            if let Ok(mut p) = slot.lock()
                && let Some(pv) = p.as_mut()
                && pv.fade.is_none()
            {
                let samples = samples.max(1);
                pv.fade = Some((pv.level / samples as f32, samples));
                let secs = samples as f32 / ssx_assets::pathfinder::SAMPLE_RATE as f32;
                back_log!(back, info, "music: preview stop ({secs:.1} s fade)");
            }
        });
    }

    pub fn preview_playing(&self) -> bool {
        self.preview_asked > self.preview_stopped.load(Relaxed)
    }

    pub fn gameplay_start(&mut self) {
        let secs = ssx_assets::sfx_game::tuning::MUSIC_LEVEL_START_FADE;
        self.stop_preview_over((secs * ssx_assets::pathfinder::SAMPLE_RATE as f32).round() as u32);
        let (menu, music) = (self.menu.clone(), self.music.clone());
        self.game.post(move |_, _| {
            if let Ok(mut m) = menu.lock()
                && let Some((lm, _)) = m.as_mut()
            {
                lm.end_posts(secs);
            }
            if let Some(music) = &music
                && let Ok(mut m) = music.lock()
            {
                m.gameplay_start();
            }
        });
    }

    pub fn movie_started(&mut self) {
        self.flow.movie_started();
    }

    pub fn movie_audio(&mut self, feed: Option<Arc<MovieAudio>>) {
        let Some(feed) = feed else { return };
        self.flow.movie_started();
        self.movie = Some(feed.clone());
        let verbose = self.verbose;
        self.game.post(move |m, back| {
            let Some(e) = m.sfx.as_mut() else { return };
            e.set_loop_rate(f64::from_bits(0x403D_F851_E000_0000) as f32);
            if !e.mixer.start_movie(feed) {
                back_log!(back, warn, "fe audio: the movie's voice did not start");
            }
            for l in e.mixer.events.drain(..) {
                if verbose {
                    back_log!(back, info, "fe audio {l}");
                }
            }
        });
    }

    pub fn frame(&mut self, screen: u32, kind: i32, boot_done: bool, nis: u8, wind: i32, now_ms: f64) {
        let mut movie_ended = false;
        match &self.movie {
            Some(f) if !f.stopped() => self.flow.movie_started(),
            Some(_) => {
                self.movie = None;
                movie_ended = true;
            }
            None => {}
        }
        let (results_ready, goal_complete) = self.results;
        let setting_67 = self.setting_67;
        let msg = self.flow.frame(&FlowInputs {
            screen,
            kind,
            boot_done,
            nis,
            results_ready,
            goal_complete,
            setting_67,
            wind,
        });
        self.screen_state = ssx_assets::sfx::frontend::screen_state(screen, kind, boot_done);
        self.last = msg;
        let (music, preview, menu) = (self.music.clone(), self.preview.clone(), self.menu.clone());
        let (plugged, level_loading, verbose, settings) =
            (self.plugged, self.level_loading, self.verbose, self.settings);
        // PORT(choice): the audio service's beat message (0x8F554D9D, read in its update 0x83099190) reaches the FE
        // audio object a game frame later than in the game: the port's front-end music runs on the mixer thread, in
        // this command, and the game thread takes its result with the next frame's replies; the game reads the music
        // under the system's lock (0x82A1AE90) on its game thread in the same frame.
        self.post(move |m, side, back| {
            if movie_ended && let Some(e) = m.sfx.as_mut() {
                e.set_loop_rate(1.0 / ssx_assets::sfx::UPDATE_PERIOD);
            }
            side.ui.frame();
            let level_song = m.music_tail.as_ref().is_some_and(|t| !t.finished());
            let preview_on = preview.lock().is_ok_and(|p| p.as_ref().is_some_and(|pv| !pv.stopped));
            if let Ok(mut menu) = menu.lock()
                && let Some((lm, _)) = menu.as_mut()
            {
                lm.hold_posts(level_song || preview_on || level_loading);
            }
            let Some(e) = m.sfx.as_mut() else { return };
            if plugged {
                for l in e.mixer.events.drain(..) {
                    if verbose {
                        back_log!(back, info, "fe audio {l}");
                    }
                }
                if e.aems.log.len() > 4096 {
                    e.aems.log.drain(..2048);
                }
            }
            e.set_game_state(msg.state);
            e.game_result = msg.result;
            if let Some(l) = side.hud_pause.state_message(e, msg.state) {
                back_log!(back, info, "fe audio {l}");
            }
            if let Some(l) = side.globe_wind.state_message(e, msg.wind)
                && verbose
            {
                back_log!(back, info, "fe audio {l}");
            }
            side.music_pause.state_msg(&mut e.mixer.graph, msg.state, msg.restart);
            side.music_pause.update(&mut e.mixer.graph);
            for l in side.music_pause.log.drain(..) {
                back_log!(back, info, "music: {l}");
            }
            if let Some(music) = &music
                && plugged
                && let Ok(mut fm) = music.lock()
            {
                let fm = &mut *fm;
                let menu_song = menu.lock().is_ok_and(|m| m.as_ref().is_some_and(|(lm, _)| lm.song_open()));
                let playing = level_song || menu_song || preview_on;
                fm.module.state_msg(&mut fm.music.rt, &msg, &mut e.settings, playing);
                fm.module.update(&mut fm.music.rt);
                let last = side.beat.take();
                if fm.awake() {
                    let status = fm.beat_status();
                    if verbose && status.map(|b| (b.beat, b.bar)) != last.map(|b| (b.beat, b.bar)) {
                        back_log!(back, info, "fe audio beat message {status:?} at {now_ms:.0} ms");
                    }
                    if let Some(index) = side.posts.update(now_ms, status.map(|b| b.to_beat), msg.state) {
                        side.ui.fire_posted(e, index, false);
                        for l in side.ui.log.drain(..) {
                            if verbose {
                                back_log!(back, info, "fe audio {l} at {now_ms:.0} ms");
                            }
                        }
                    }
                    side.beat = status;
                }
                for l in fm.module.log.drain(..).chain(fm.music.rt.log.drain(..).map(|l| l.text)) {
                    if verbose {
                        back_log!(back, info, "fe music {l}");
                    }
                }
            }
            let (beat, now) = (side.beat, plugged.then_some(e.settings).filter(|s| *s != settings));
            back.reply(move |w| {
                if let Some(mut fe) = w.get_resource_mut::<FrontEndAudio>() {
                    fe.beat = beat;
                    if let Some(s) = now {
                        fe.settings = s;
                    }
                }
            });
        });
    }

    pub fn results(&mut self, ready: bool, goal_complete: bool) {
        self.results = (ready, goal_complete);
    }

    pub fn ui_sfx(&mut self, id: i32, boost: f32) {
        let (last_state, audio_mode, screen_state, popup_menu) =
            (self.last.state, self.flow.audio_mode, self.screen_state, self.popup_menu);
        let verbose = self.verbose;
        self.post(move |m, side, back| {
            let Some(e) = m.sfx.as_mut() else { return };
            let ctx = UiContext { last_state, boost, audio_mode, screen_state, popup_menu };
            side.ui.play(e, id, &ctx);
            for l in side.ui.log.drain(..) {
                if verbose {
                    back_log!(back, info, "fe audio {l}");
                }
            }
        });
    }

    pub fn ui_post(&mut self, id: i32, delay_ms: f32, now_ms: f64) {
        let Some(index) = post_index(id) else { return };
        let verbose = self.verbose;
        if verbose {
            info!("fe audio post {id:#x} (CSIS {index}) delay {delay_ms:.1} ms at {now_ms:.0} ms");
        }
        self.post(move |m, side, back| {
            let Some(index) = side.posts.receive(index, delay_ms, now_ms, now_ms) else { return };
            let Some(e) = m.sfx.as_mut() else { return };
            side.ui.fire_posted(e, index, true);
            for l in side.ui.log.drain(..) {
                if verbose {
                    back_log!(back, info, "fe audio {l} at {now_ms:.0} ms");
                }
            }
        });
    }

    pub fn beat(&self) -> Option<BeatStatus> {
        self.beat
    }

    pub fn watch_ui(&mut self, value: impl Fn(u16) -> bool) {
        self.popup_menu = value(0x219);
        for n in self.flow.watch(&value) {
            self.tutorial_sound(n);
        }
    }

    pub fn tutorial_sound(&mut self, n: u32) {
        let verbose = self.verbose;
        self.post(move |m, side, back| {
            let Some(e) = m.sfx.as_mut() else { return };
            side.ui.tutorial_sound(e, n);
            for l in side.ui.log.drain(..) {
                if verbose {
                    back_log!(back, info, "fe audio {l}");
                }
            }
        });
    }

    pub fn hint_shown(&mut self, kind: i32) {
        self.flow.hint_shown(kind);
    }

    pub fn hint_closed(&mut self) {
        self.flow.hint_closed();
    }

    pub fn popup_state(&mut self, v: i32) {
        if self.flow.popup_state(v) {
            self.tutorial_sound(1);
        }
    }

    pub fn game_pause(&mut self, paused: bool) {
        let msg = if paused { None } else { self.speech_pause.unpause_event() };
        self.post(move |m, side, back| {
            if let Some(e) = m.sfx.as_mut() {
                e.game_pause(paused);
                if !paused {
                    side.music_pause.unpause_event(&mut e.mixer.graph);
                    speech_message(e, msg, "the pause's end (0xEA473DCC)", back);
                }
            }
        });
    }

    pub fn game_pause_event(&mut self, paused: bool, tutorial: bool) {
        if !paused {
            self.game_pause(false);
            return;
        }
        let msg = self.speech_pause.pause_event(tutorial);
        self.post(move |m, side, back| {
            if let Some(e) = m.sfx.as_mut() {
                e.game_pause(true);
                side.music_pause.pause_event(&mut e.mixer.graph, tutorial);
                speech_message(e, msg, "the pause's begin (0xC225AE47)", back);
            }
        });
    }

    pub fn pause_menu_event(&mut self, opened: bool, leaving: bool, closed: bool) {
        let msg = self.speech_pause.pause_menu(opened, closed);
        self.post(move |m, side, back| {
            if let Some(e) = m.sfx.as_mut() {
                side.music_pause.pause_menu(&mut e.mixer.graph, opened, closed);
                let what =
                    format!("the pause menu's 0x8BE0E597 {{{}, {}, {}}}", opened as u8, leaving as u8, closed as u8);
                speech_message(e, msg, &what, back);
            }
        });
    }

    pub fn speech_paused(&self) -> bool {
        self.speech_pause.paused
    }

    pub fn boot_flow_loaded(&mut self) {
        self.flow.boot_flow_loaded();
    }

    pub fn launch_requested(&mut self) {
        self.flow.launch_requested();
    }

    pub fn set_setting_67(&mut self, on: bool) {
        self.setting_67 = on;
    }

    pub fn state(&self) -> StateMsg {
        self.last
    }
}

fn speech_message(e: &mut SfxEngine, msg: Option<bool>, why: &str, back: &Back) {
    let Some(pause) = msg else { return };
    let voices = e.speech.is_some();
    e.speech_message(pause);
    back_log!(
        back,
        info,
        "speech: {} on {why}{}",
        if pause { "PauseSpeech" } else { "UnpauseSpeech" },
        if voices { "" } else { " (no speech loaded)" }
    );
}
