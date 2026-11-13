use std::path::Path;
use std::sync::{Arc, Mutex};

use bevy::prelude::*;
use ssx_assets::content::audio::{AudioScope, MusicDecision};
use ssx_assets::musicbox::{LicensedMusic, MUSIC_SUBMIX_LEVEL, MusicBox, SongSwitches};
use ssx_assets::sfx::frontend::{
    BeatStatus, FlowInputs, FlowState, FrontEndMusic, GlobeWind, PostSlot, StateMsg, UiContext, UiSounds, post_index,
};
use ssx_assets::sfx::movie::MovieAudio;
use ssx_assets::sfx::music_pause::MusicPause;
use ssx_assets::sfx::settings::AudioSettings;
use ssx_assets::sfx::speech::pause::SpeechPause;
use ssx_assets::sfx::{MusicSource, OutputMix, SfxEngine};

use super::{GameAudio, output};

mod reload;
pub use reload::FrontEndReload;

#[derive(Resource, Clone, Copy)]
pub struct AudioSettingsRes(pub AudioSettings);

struct FeMusicBus(Arc<Mutex<FrontEndMusic>>, Arc<Mutex<Option<Preview>>>, MenuSongs);

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
        self.0.lock().is_ok_and(|m| m.finished()) && self.1.lock().is_ok_and(|p| p.is_none())
    }
}

struct Preview {
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

#[derive(Resource)]
pub struct FrontEndAudio {
    out: Arc<Mutex<OutputMix>>,
    music: Option<Arc<Mutex<FrontEndMusic>>>,
    preview: Arc<Mutex<Option<Preview>>>,
    engine: Mutex<Option<SfxEngine>>,
    plugged: bool,
    flow: FlowState,
    ui: UiSounds,
    posts: PostSlot,
    beat: Option<BeatStatus>,
    globe_wind: GlobeWind,
    last: StateMsg,
    results: (bool, bool),
    movie: Option<Arc<MovieAudio>>,
    screen_state: i32,
    setting_67: bool,
    popup_menu: bool,
    verbose: bool,
    music_pause: MusicPause,
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
    let mut fe = FrontEndAudio {
        out: g.out.clone(),
        music: data.music.map(|m| Arc::new(Mutex::new(m))),
        preview: Arc::new(Mutex::new(None)),
        engine: Mutex::new(Some(data.engine)),
        plugged: false,
        flow: FlowState::default(),
        ui: UiSounds::default(),
        last: StateMsg::default(),
        results: (false, false),
        movie: None,
        screen_state: 0,
        setting_67: false,
        popup_menu: false,
        verbose: std::env::var("SSX_FE_AUDIO_LOG").is_ok_and(|v| v == "1"),
        music_pause: MusicPause::default(),
        speech_pause: SpeechPause::default(),
        plan: data.plan,
        menu_plan: data.menu_plan,
        menu: Arc::new(Mutex::new(None)),
        switches: SongSwitches::default(),
        posts: PostSlot::default(),
        beat: None,
        globe_wind: GlobeWind::default(),
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
    let base = match world.get_resource::<AudioSettingsRes>() {
        Some(s) => Some(s.0),
        None => world.get_resource::<GameAudio>().and_then(|g| g.out.lock().ok()?.sfx.as_ref().map(|e| e.settings)),
    };
    let Some(mut s) = base else { return };
    for &(id, v) in ui_values {
        s.set_ui(id, v);
    }
    world.insert_resource(AudioSettingsRes(s));
    if let Some(g) = world.get_resource::<GameAudio>()
        && let Ok(mut m) = g.out.lock()
        && let Some(e) = m.sfx.as_mut()
    {
        e.apply_settings(&s);
    }
    if let Some(fe) = world.get_resource::<FrontEndAudio>()
        && let Ok(mut e) = fe.engine.lock()
        && let Some(e) = e.as_mut()
    {
        e.apply_settings(&s);
    }
}

pub fn apply_stored_settings(world: &mut World, s: &AudioSettings) {
    world.insert_resource(AudioSettingsRes(*s));
    if let Some(g) = world.get_resource::<GameAudio>()
        && let Ok(mut m) = g.out.lock()
        && let Some(e) = m.sfx.as_mut()
    {
        e.apply_settings(s);
    }
    if let Some(fe) = world.get_resource::<FrontEndAudio>()
        && let Ok(mut e) = fe.engine.lock()
        && let Some(e) = e.as_mut()
    {
        e.apply_settings(s);
    }
}

impl FrontEndAudio {
    pub fn settings(&self) -> Option<AudioSettings> {
        if self.plugged {
            self.out.lock().ok()?.sfx.as_ref().map(|e| e.settings)
        } else {
            self.engine.lock().ok()?.as_ref().map(|e| e.settings)
        }
    }

    pub fn plug(&mut self) {
        if self.plugged {
            return;
        }
        let engine = self.engine.lock().ok().and_then(|mut e| e.take());
        self.arm_menu_songs();
        let music: Option<Box<dyn MusicSource>> =
            self.music.as_ref().map(|m| Box::new(FeMusicBus(m.clone(), self.preview.clone(), self.menu.clone())) as _);
        let _turn = crate::audio::music::GameTurn::take();
        let Ok(mut m) = self.out.lock() else { return };
        let old_sfx = engine.is_some().then(|| std::mem::replace(&mut m.sfx, engine));
        if let (Some(Some(level)), Some(fe)) = (old_sfx.as_ref(), m.sfx.as_mut()) {
            fe.aems.set_rng_state(level.aems.rng_state());
        }
        let level_music = std::mem::replace(&mut m.music, music);
        let tail = std::mem::replace(&mut m.music_tail, level_music.filter(|b| !b.finished()));
        let old = old_sfx;
        (m.remix_event_type, m.swapped) = (1, true);
        drop(m);
        drop((old, tail));
        self.plugged = true;
        self.reset_music_pause();
    }

    fn arm_menu_songs(&mut self) {
        if self.plan.music.added.iter().all(|s| !s.song.menu) {
            return;
        }
        match MusicBox::load(&self.big_root) {
            Ok(mut mb) => {
                mb.add_packages(&self.plan.music);
                mb.apply_switches(&self.switches);
                if let Ok(mut m) = self.menu.lock() {
                    *m = Some((LicensedMusic::menu(mb), Vec::new()));
                }
            }
            Err(e) => ssx_core::eprintln!("warning: the menus' package songs: {e:#}"),
        }
    }

    pub fn set_song_switches(&mut self, switches: &SongSwitches) {
        if self.switches == *switches {
            return;
        }
        self.switches = switches.clone();
        let _turn = crate::audio::music::GameTurn::take();
        if let Ok(mut m) = self.menu.lock()
            && let Some((lm, _)) = m.as_mut()
        {
            lm.mb.apply_switches(switches);
        }
    }

    pub fn set_level_loading(&mut self, loading: bool) {
        self.level_loading = loading;
    }

    fn reset_music_pause(&mut self) {
        self.music_pause = MusicPause::default();
        self.speech_pause = SpeechPause::default();
        ssx_assets::musicbox::set_paused(false);
    }

    pub fn set_unplugged_sounds(&self, sounds: &ssx_assets::sfx_override::SoundOverrides) {
        if let Ok(mut e) = self.engine.lock()
            && let Some(e) = e.as_mut()
        {
            e.set_overrides(sounds);
        }
    }

    pub fn aems_rng(&self) -> Option<[u32; 6]> {
        self.engine.lock().ok()?.as_ref().map(|e| e.aems.rng_state())
    }

    pub fn unplug(&mut self) {
        if !self.plugged {
            return;
        }
        let Ok(mut m) = self.out.lock() else { return };
        if let Ok(mut e) = self.engine.lock() {
            *e = m.sfx.take();
        }
        let music = m.music.take();
        (m.music_tail, m.swapped) = (music.filter(|b| !b.finished()), true);
        drop(m);
        self.plugged = false;
        self.reset_music_pause();
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
        let preview = Preview { player, level: MUSIC_SUBMIX_LEVEL, fade: None, stopped: false, buf: Vec::new() };
        let _turn = crate::audio::music::GameTurn::take();
        if let Ok(mut m) = self.menu.lock()
            && let Some((lm, _)) = m.as_mut()
        {
            lm.stop(0.0);
        }
        if let Ok(mut p) = self.preview.lock() {
            *p = Some(preview);
        }
        if let Ok(mut o) = self.out.lock()
            && o.music_tail.take().is_some()
        {
            info!("music: the ride's song left playing stops for the preview (StopMusic)");
        }
    }

    pub fn stop_preview(&mut self) {
        self.stop_preview_over(PREVIEW_FADE_SAMPLES);
    }

    fn stop_preview_over(&mut self, samples: u32) {
        let _turn = crate::audio::music::GameTurn::take();
        if let Ok(mut p) = self.preview.lock()
            && let Some(pv) = p.as_mut()
            && pv.fade.is_none()
        {
            let samples = samples.max(1);
            pv.fade = Some((pv.level / samples as f32, samples));
            info!("music: preview stop ({:.1} s fade)", samples as f32 / ssx_assets::pathfinder::SAMPLE_RATE as f32);
        }
    }

    pub fn preview_playing(&self) -> bool {
        self.preview.lock().is_ok_and(|p| p.as_ref().is_some_and(|pv| !pv.stopped))
    }

    pub fn gameplay_start(&mut self) {
        let secs = ssx_assets::sfx_game::tuning::MUSIC_LEVEL_START_FADE;
        self.stop_preview_over((secs * ssx_assets::pathfinder::SAMPLE_RATE as f32).round() as u32);
        if let Ok(mut m) = self.menu.lock()
            && let Some((lm, _)) = m.as_mut()
        {
            lm.end_posts(secs);
        }
        if let Some(music) = &self.music
            && let Ok(mut m) = music.lock()
        {
            m.gameplay_start();
        }
    }

    pub fn movie_started(&mut self) {
        self.flow.movie_started();
    }

    pub fn movie_audio(&mut self, feed: Option<Arc<MovieAudio>>) -> bool {
        let Some(feed) = feed else { return false };
        self.flow.movie_started();
        self.movie = Some(feed.clone());
        let Ok(mut m) = self.out.lock() else { return false };
        let Some(e) = m.sfx.as_mut() else { return false };
        e.set_loop_rate(f64::from_bits(0x403D_F851_E000_0000) as f32);
        let ok = e.mixer.start_movie(feed);
        for l in e.mixer.events.drain(..) {
            if self.verbose {
                info!("fe audio {l}");
            }
        }
        ok
    }

    pub fn frame(&mut self, screen: u32, kind: i32, boot_done: bool, nis: u8, wind: i32, now_ms: f64) {
        match &self.movie {
            Some(f) if !f.stopped() => self.flow.movie_started(),
            Some(_) => {
                self.movie = None;
                if let Ok(mut m) = self.out.lock()
                    && let Some(e) = m.sfx.as_mut()
                {
                    e.set_loop_rate(1.0 / ssx_assets::sfx::UPDATE_PERIOD);
                }
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
        self.ui.frame();
        let _turn = crate::audio::music::GameTurn::take();
        let Ok(mut m) = self.out.lock() else { return };
        let level_song = m.music_tail.as_ref().is_some_and(|t| !t.finished());
        let preview = self.preview.lock().is_ok_and(|p| p.as_ref().is_some_and(|pv| !pv.stopped));
        if let Ok(mut menu) = self.menu.lock()
            && let Some((lm, _)) = menu.as_mut()
        {
            lm.hold_posts(level_song || preview || self.level_loading);
        }
        if let Some(e) = m.sfx.as_mut() {
            if self.plugged {
                for l in e.mixer.events.drain(..) {
                    if self.verbose {
                        info!("fe audio {l}");
                    }
                }
                if e.aems.log.len() > 4096 {
                    e.aems.log.drain(..2048);
                }
            }
            e.set_game_state(msg.state);
            e.game_result = msg.result;
            if let Some(l) = self.globe_wind.state_message(e, msg.wind)
                && self.verbose
            {
                info!("fe audio {l}");
            }
            self.music_pause.state_msg(&mut e.mixer.graph, msg.state, msg.restart);
            self.music_pause.update(&mut e.mixer.graph);
            for l in self.music_pause.log.drain(..) {
                info!("music: {l}");
            }
            if let Some(music) = &self.music
                && self.plugged
                && let Ok(mut fm) = music.lock()
            {
                let fm = &mut *fm;
                let menu_song = self.menu.lock().is_ok_and(|m| m.as_ref().is_some_and(|(lm, _)| lm.song_open()));
                let playing = level_song
                    || menu_song
                    || self.preview.lock().is_ok_and(|p| p.as_ref().is_some_and(|pv| !pv.stopped));
                fm.module.state_msg(&mut fm.music.rt, &msg, &mut e.settings, playing);
                fm.module.update(&mut fm.music.rt);
                let last = self.beat.take();
                if fm.awake() {
                    let status = fm.beat_status();
                    if self.verbose && status.map(|b| (b.beat, b.bar)) != last.map(|b| (b.beat, b.bar)) {
                        info!("fe audio beat message {status:?} at {now_ms:.0} ms");
                    }
                    if let Some(index) = self.posts.update(now_ms, status.map(|b| b.to_beat), msg.state) {
                        self.ui.fire_posted(e, index, false);
                        for l in self.ui.log.drain(..) {
                            if self.verbose {
                                info!("fe audio {l} at {now_ms:.0} ms");
                            }
                        }
                    }
                    self.beat = status;
                }
                for l in fm.module.log.drain(..).chain(fm.music.rt.log.drain(..).map(|l| l.text)) {
                    if self.verbose {
                        info!("fe music {l}");
                    }
                }
            }
        }
        self.last = msg;
    }

    pub fn results(&mut self, ready: bool, goal_complete: bool) {
        self.results = (ready, goal_complete);
    }

    pub fn ui_sfx(&mut self, id: i32, boost: f32) -> Vec<&'static str> {
        let _turn = crate::audio::music::GameTurn::take();
        let Ok(mut m) = self.out.lock() else { return Vec::new() };
        let Some(e) = m.sfx.as_mut() else { return Vec::new() };
        let ctx = UiContext {
            last_state: self.last.state,
            boost,
            audio_mode: self.flow.audio_mode,
            screen_state: self.screen_state,
            popup_menu: self.popup_menu,
        };
        let fired = self.ui.play(e, id, &ctx);
        for l in self.ui.log.drain(..) {
            if self.verbose {
                info!("fe audio {l}");
            }
        }
        fired
    }

    pub fn ui_post(&mut self, id: i32, delay_ms: f32, now_ms: f64) {
        let Some(index) = post_index(id) else { return };
        let fire = self.posts.receive(index, delay_ms, now_ms, now_ms);
        if self.verbose {
            info!("fe audio post {id:#x} (CSIS {index}) delay {delay_ms:.1} ms at {now_ms:.0} ms");
        }
        let Some(index) = fire else { return };
        let _turn = crate::audio::music::GameTurn::take();
        let Ok(mut m) = self.out.lock() else { return };
        let Some(e) = m.sfx.as_mut() else { return };
        self.ui.fire_posted(e, index, true);
        for l in self.ui.log.drain(..) {
            if self.verbose {
                info!("fe audio {l} at {now_ms:.0} ms");
            }
        }
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
        let _turn = crate::audio::music::GameTurn::take();
        let Ok(mut m) = self.out.lock() else { return };
        let Some(e) = m.sfx.as_mut() else { return };
        self.ui.tutorial_sound(e, n);
        for l in self.ui.log.drain(..) {
            if self.verbose {
                info!("fe audio {l}");
            }
        }
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
        let _turn = crate::audio::music::GameTurn::take();
        let Ok(mut m) = self.out.lock() else { return };
        if let Some(e) = m.sfx.as_mut() {
            e.game_pause(paused);
            if !paused {
                self.music_pause.unpause_event(&mut e.mixer.graph);
                let msg = self.speech_pause.unpause_event();
                speech_message(e, msg, "the pause's end (0xEA473DCC)");
            }
        }
    }

    pub fn game_pause_event(&mut self, paused: bool, tutorial: bool) {
        if !paused {
            self.game_pause(false);
            return;
        }
        let _turn = crate::audio::music::GameTurn::take();
        let Ok(mut m) = self.out.lock() else { return };
        if let Some(e) = m.sfx.as_mut() {
            e.game_pause(true);
            self.music_pause.pause_event(&mut e.mixer.graph, tutorial);
            let msg = self.speech_pause.pause_event(tutorial);
            speech_message(e, msg, "the pause's begin (0xC225AE47)");
        }
    }

    pub fn pause_menu_event(&mut self, opened: bool, leaving: bool, closed: bool) {
        let _turn = crate::audio::music::GameTurn::take();
        let Ok(mut m) = self.out.lock() else { return };
        if let Some(e) = m.sfx.as_mut() {
            self.music_pause.pause_menu(&mut e.mixer.graph, opened, closed);
            let msg = self.speech_pause.pause_menu(opened, closed);
            let what = format!("the pause menu's 0x8BE0E597 {{{}, {}, {}}}", opened as u8, leaving as u8, closed as u8);
            speech_message(e, msg, &what);
        }
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

fn speech_message(e: &mut SfxEngine, msg: Option<bool>, why: &str) {
    let Some(pause) = msg else { return };
    let voices = e.speech.is_some();
    e.speech_message(pause);
    info!(
        "speech: {} on {why}{}",
        if pause { "PauseSpeech" } else { "UnpauseSpeech" },
        if voices { "" } else { " (no speech loaded)" }
    );
}
