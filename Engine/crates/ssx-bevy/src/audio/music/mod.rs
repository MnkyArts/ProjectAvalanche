use std::path::Path;
use std::sync::{Arc, Mutex};

use bevy::prelude::*;
use ssx_assets::content::audio::AudioScope;
use ssx_assets::music_script::{
    DdMusic, GameMusicState, ObjectiveScript, ScriptRunner, dd_script_name, track_drop_scripts,
};
use ssx_assets::musicbox::{LicensedMusic, MusicBox, Playlist, RaceType, SongSwitches};
use ssx_assets::sfx::output::{MixQueue, Output, Scheduling, mix_queue};
pub use ssx_assets::sfx::output::{discard, stats};
use ssx_assets::sfx::{MusicSource, OutputMix};
use ssx_assets::sfx_queue::{self, Receiver, Sender};

use crate::ride::Ride;

pub mod device;
pub mod frontend;
pub mod map;

const RATE: u32 = ssx_assets::pathfinder::SAMPLE_RATE;

enum Engine {
    Dd(Box<DdMusic>),
    Licensed(Box<LicensedMusic>),
}

struct Shared {
    engine: Engine,
    volume: f32,
    #[cfg(feature = "scripts")]
    level: Option<Engine>,
    script: Option<ObjectiveScript>,
    restart_in: Option<f32>,
    log_from: usize,
}

struct MusicBus {
    shared: Arc<Mutex<Shared>>,
    script_song: bool,
}

impl MusicSource for MusicBus {
    fn render(&mut self, out: &mut [f32]) {
        out.fill(0.0);
        let Ok(mut s) = self.shared.lock() else { return };
        #[cfg(feature = "scripts")]
        {
            self.script_song = s.level.is_some();
        }
        let volume = s.volume;
        match &mut s.engine {
            Engine::Dd(dd) => dd.music.render(out),
            Engine::Licensed(lm) => lm.render(out, ssx_assets::musicbox::MUSIC_SUBMIX_LEVEL),
        }
        for x in out {
            *x *= volume;
        }
    }

    fn finished(&self) -> bool {
        let Ok(s) = self.shared.lock() else { return true };
        #[cfg(feature = "scripts")]
        let mut s = s;
        let done = match &s.engine {
            Engine::Dd(_) => true,
            Engine::Licensed(lm) => lm.finished(),
        };
        #[cfg(feature = "scripts")]
        if done && let Some(level) = s.level.take() {
            s.engine = level;
            return false;
        }
        done
    }

    fn script_song(&self) -> bool {
        self.script_song
    }
}

#[derive(Resource, Clone)]
pub struct GameAudio {
    queue: MixQueue,
    back: Back,
}

impl GameAudio {
    pub fn post<F: FnOnce(&mut OutputMix, &Back) + Send + 'static>(&self, f: F) -> bool {
        self.post_as(std::any::type_name::<F>(), f)
    }

    pub fn post_as(&self, kind: &'static str, f: impl FnOnce(&mut OutputMix, &Back) + Send + 'static) -> bool {
        let back = self.back.clone();
        self.queue.post_as(kind, move |m| f(m, &back))
    }
}

pub type Reply = Box<dyn FnOnce(&mut World) + Send>;

#[derive(Clone)]
pub struct Back(Sender<Reply>);

impl Back {
    pub fn reply(&self, f: impl FnOnce(&mut World) + Send + 'static) {
        let _ = self.0.send(Box::new(f));
    }
}

macro_rules! back_log {
    ($back:expr, $level:ident, $($arg:tt)*) => {{
        let line = format!($($arg)*);
        $back.reply(move |_| bevy::log::$level!("{line}"));
    }};
}
pub(crate) use back_log;

#[derive(Resource)]
struct AudioReplies(Mutex<Receiver<Reply>>);

fn apply_replies(world: &mut World) {
    let replies: Vec<Reply> = world
        .get_resource::<AudioReplies>()
        .and_then(|r| r.0.lock().ok().map(|mut rx| rx.try_iter().collect()))
        .unwrap_or_default();
    for r in replies {
        r(world);
    }
}

pub fn output(app: &mut App) -> GameAudio {
    if let Some(g) = app.world().get_resource::<GameAudio>() {
        return g.clone();
    }
    let (queue, inbox) = mix_queue();
    let (tx, rx) = sfx_queue::queue();
    let g = GameAudio { queue, back: Back(tx) };
    app.insert_resource(g.clone()).insert_resource(AudioReplies(Mutex::new(rx))).add_systems(First, apply_replies);
    match Output::start(OutputMix::new(None, RATE, None), inbox, None, Scheduling::Audio) {
        Ok(output) => {
            app.add_systems(Startup, move |mut commands: Commands| {
                commands.insert_resource(device::AudioDevice::start(output.clone()));
            })
            .add_systems(Update, device::watch.run_if(resource_exists::<device::AudioDevice>));
        }
        Err(e) => error!("audio: no mixer thread ({e}): the game runs silent"),
    }
    g
}

#[derive(Resource)]
pub struct Music {
    shared: Arc<Mutex<Shared>>,
    game: GameAudio,
    dd: bool,
    last_steps: u64,
    verbose: bool,
    scope: AudioScope,
    race: Option<RaceType>,
}

#[cfg(feature = "scripts")]
impl Music {
    pub(crate) fn play_script_song(&self, song: LicensedMusic) -> Result<(), &'static str> {
        if self.dd {
            return Err("a Deadly Descent's music is its Pathfinder score");
        }
        let shared = self.shared.clone();
        let posted = self.game.post(move |_, _| {
            let Ok(mut shared) = shared.lock() else { return };
            let old = std::mem::replace(&mut shared.engine, Engine::Licensed(Box::new(song)));
            match shared.level {
                None => shared.level = Some(old),
                Some(_) => discard(old),
            }
        });
        if posted { Ok(()) } else { Err("the game's audio does not run") }
    }

    pub(crate) fn end_script_song(&self) {
        let shared = self.shared.clone();
        self.game.post(move |_, _| {
            if let Ok(mut shared) = shared.lock()
                && let Some(level) = shared.level.take()
            {
                discard(std::mem::replace(&mut shared.engine, level));
            }
        });
    }

    pub(crate) fn scope(&self) -> &AudioScope {
        &self.scope
    }
}

#[derive(Resource, Clone, Copy)]
pub struct AudioDrop(pub Option<u32>);

#[derive(Resource, Clone, Copy)]
pub struct RemixEventType(pub i32);

pub struct MusicLevel {
    engine: Engine,
    script: Option<ObjectiveScript>,
    label: String,
    remix_type: i32,
    volume: f32,
    verbose: bool,
    scope: AudioScope,
    race: Option<RaceType>,
}

type Loaded = (Engine, Option<ObjectiveScript>, String, i32, AudioScope, Option<RaceType>);

pub fn register(app: &mut App) {
    output(app);
    if app.world().contains_resource::<MusicRegistered>() {
        return;
    }
    app.insert_resource(MusicRegistered).add_systems(Update, drive_music.run_if(resource_exists::<Music>));
}

#[derive(Resource)]
struct MusicRegistered;

pub fn load_level(big_root: Option<&Path>, track: &str, drop_key: Option<u32>, screenshot: bool) -> Option<MusicLevel> {
    load_level_event(big_root, track, drop_key, None, &SongSwitches::default(), screenshot)
}

pub fn load_level_event(
    big_root: Option<&Path>,
    track: &str,
    drop_key: Option<u32>,
    event_key: Option<u32>,
    song_flags: &SongSwitches,
    screenshot: bool,
) -> Option<MusicLevel> {
    let mode = std::env::var("SSX_MUSIC").unwrap_or_default().to_ascii_lowercase();
    if matches!(mode.as_str(), "off" | "0" | "false") || (screenshot && mode.is_empty()) {
        return None;
    }
    let Some(big_root) = big_root else {
        ssx_core::eprintln!("warning: music unavailable: no extracted archive root");
        return None;
    };
    let volume = std::env::var("SSX_MUSIC_VOLUME").ok().and_then(|v| v.parse().ok()).unwrap_or(1.0f32).clamp(0.0, 1.0);
    match load(big_root, track, drop_key, event_key, song_flags, &mode) {
        Ok((engine, script, label, remix_type, scope, race)) => {
            let verbose = std::env::var("SSX_MUSIC_LOG").is_ok_and(|v| v == "1");
            Some(MusicLevel { engine, script, label, remix_type, volume, verbose, scope, race })
        }
        Err(e) => {
            ssx_core::eprintln!("warning: music unavailable: {e:#}");
            None
        }
    }
}

impl MusicLevel {
    pub fn remix_event_type(level: Option<&MusicLevel>, world: &World) -> Option<i32> {
        level.filter(|_| world.contains_resource::<GameAudio>()).map(|l| l.remix_type)
    }
}

pub fn start_level(world: &mut World, level: Option<MusicLevel>, drop_key: Option<u32>) {
    world.insert_resource(AudioDrop(drop_key));
    let Some(l) = level else { return };
    let Some(game) = world.get_resource::<GameAudio>().cloned() else { return };
    ssx_core::eprintln!("music: {}", l.label);
    let dd = matches!(l.engine, Engine::Dd(_));
    let shared = Arc::new(Mutex::new(Shared {
        engine: l.engine,
        volume: l.volume,
        #[cfg(feature = "scripts")]
        level: None,
        script: l.script,
        restart_in: None,
        log_from: 0,
    }));
    let (bus, remix_type) = (MusicBus { shared: shared.clone(), script_song: false }, l.remix_type);
    game.post(move |m, _| {
        discard(m.music.replace(Box::new(bus)));
        (m.remix_event_type, m.swapped) = (remix_type, true);
    });
    world.insert_resource(RemixEventType(remix_type));
    world.insert_resource(Music { dd, shared, game, last_steps: 0, verbose: l.verbose, scope: l.scope, race: l.race });
}

pub fn end_level(world: &mut World) {
    if let Some(music) = world.get_resource::<Music>() {
        let shared = music.shared.clone();
        music.game.post(move |_, _| {
            if let Ok(mut shared) = shared.lock()
                && let Engine::Licensed(lm) = &mut shared.engine
            {
                lm.level_end();
            }
        });
    }
    if !world.contains_resource::<frontend::FrontEndAudio>()
        && let Some(g) = world.get_resource::<GameAudio>()
    {
        g.post(|m, _| {
            discard(m.music.take());
            m.remix_event_type = 1;
        });
    }
    world.remove_resource::<Music>();
    world.remove_resource::<AudioDrop>();
    world.remove_resource::<RemixEventType>();
}

pub fn insert(app: &mut App, big_root: Option<&Path>, track: &str, drop_key: Option<u32>, screenshot: bool) {
    let level = load_level(big_root, track, drop_key, screenshot);
    if level.is_some() {
        register(app);
    }
    start_level(app.world_mut(), level, drop_key);
}

fn load(
    big_root: &Path,
    track: &str,
    drop_key: Option<u32>,
    event_key: Option<u32>,
    song_flags: &SongSwitches,
    mode: &str,
) -> anyhow::Result<Loaded> {
    let drops = track_drop_scripts(big_root, track).unwrap_or_default();
    let events = drop_key.and_then(|k| ssx_physics::spawn::load_drop_events(big_root, track, k).ok()).map(|e| e.0);
    let launched = event_key.and_then(|k| events.as_ref()?.iter().find(|e| e.key == k).cloned());
    let dd = match &launched {
        Some(d) => d.scripts.iter().find(|s| s.starts_with("eventtype_dd_")).map(|script| {
            let chapter =
                d.name.strip_prefix("chapter_").and_then(|r| r.split('_').next()).and_then(|n| n.parse().ok());
            (script.strip_suffix(".lua").unwrap_or(script).to_string(), chapter)
        }),
        None => {
            dd_script_name(&drops, drop_key).or_else(|| dd_script_name(&drops, None).filter(|_| drop_key.is_none()))
        }
    };
    let dd = match mode {
        "licensed" => None,
        "dd" => dd.or_else(|| dd_script_name(&drops, None)),
        _ => dd,
    };
    let scope = AudioScope::Level { drop: launched.as_ref().map(|d| d.name.to_ascii_lowercase()) };
    let plan = ssx_assets::content::GameContent::of(big_root).music_plan(&scope);
    if let Some((name, chapter)) = dd {
        let script = ObjectiveScript::load(big_root, &name)?;
        let label =
            format!("Deadly Descent ({name}, song {:?}, chapter {chapter:?}) — Pathfinder ssxdd", script.song());
        let mut dd = DdMusic::load(big_root, script.clone(), chapter)?;
        if !plan.music.is_empty() {
            let mut log = Vec::new();
            ssx_assets::content::audio::apply::apply_project(&mut dd.music, "ssxdd", &plan.music, &mut log);
            log.iter().for_each(|l| ssx_core::eprintln!("{l}"));
        }
        return Ok((Engine::Dd(Box::new(dd)), Some(script), label, 3, scope, None));
    }
    let def = ssx_physics::event::level_event(
        events.as_deref().unwrap_or_default(),
        event_key,
        crate::ride::viewer_event_choice,
    )
    .cloned();
    let event = def.as_ref().map_or_else(|| "no event".to_string(), |d| d.name.clone());
    let kind = def.as_ref().map_or(6, |d| d.event_type.map_or(6, |t| ssx_assets::sfx_game::event_kind(t as u32)));
    let race = def.as_ref().and_then(|d| RaceType::from_event(d.key, kind));
    let remix_type = ssx_assets::sfx_game::race_event_type(kind);
    let mut mb = MusicBox::load(big_root)?;
    mb.apply_switches(song_flags);
    if !plan.music.is_empty() {
        mb.add_packages(&plan.music);
    }
    let off = song_flags.off();
    let (engine, label) = match race {
        Some(r) => {
            let lm = LicensedMusic::game(mb, r);
            let n = LicensedMusic::candidates(&lm.mb, r).iter().filter(|(_, w)| *w > 1).count();
            let seed = ssx_assets::musicbox::session().lock().map_or(0, |s| s.seed());
            (
                lm,
                format!("licensed soundtrack ({event} → {r:?}, {n} songs weighed; {off} switched off; seed {seed:#x})"),
            )
        }
        None => (
            LicensedMusic::new(mb, Playlist { ids: Vec::new(), next: 0 }),
            format!("no music ({event}: songs mask 8, no song; no custom playlist)"),
        ),
    };
    Ok((Engine::Licensed(Box::new(engine)), None, label, remix_type, scope, race))
}

pub struct MusicReload {
    plan: Arc<ssx_assets::content::audio::MusicDecision>,
    score: Option<anyhow::Result<ssx_assets::score::ScorePlayer>>,
    tracks: Vec<(usize, anyhow::Result<ssx_assets::track_override::TrackOverride>)>,
}

impl Music {
    pub fn restart(&self) {
        if self.dd {
            return;
        }
        let shared = self.shared.clone();
        self.game.post(move |_, _| {
            if let Ok(mut shared) = shared.lock()
                && let Engine::Licensed(lm) = &mut shared.engine
            {
                lm.restart();
            }
        });
    }

    pub fn reload_inputs(&self) -> (AudioScope, bool) {
        (self.scope.clone(), self.dd)
    }

    pub fn prepare_reload(big_root: &Path, scope: &AudioScope, dd: bool) -> MusicReload {
        let plan = ssx_assets::content::GameContent::of(big_root).music_plan(scope);
        let (mut score, mut tracks) = (None, Vec::new());
        if dd {
            use ssx_assets::content::audio::apply;
            score = plan.music.scores.get("ssxdd").map(|t| apply::score(t).map_err(anyhow::Error::msg));
            for ((p, track), t) in &plan.music.tracks {
                if p == "ssxdd" {
                    tracks.push((*track, apply::track_override(t).map_err(anyhow::Error::msg)));
                }
            }
        }
        MusicReload { plan, score, tracks }
    }

    pub fn apply_reload(&self, r: MusicReload) {
        let (shared, race, summary) = (self.shared.clone(), self.race, r.plan.summary(&self.scope));
        self.game.post(move |_, back| {
            let Ok(mut shared) = shared.lock() else { return };
            match &mut shared.engine {
                Engine::Dd(dd) => {
                    dd.music.clear_packages();
                    match r.score {
                        Some(Ok(s)) => dd.music.set_score(s, "the package's score (reloaded)"),
                        Some(Err(e)) => {
                            back_log!(back, warn, "mod-dev: the score not used: {e:#}; the project's own plays")
                        }
                        None => {}
                    }
                    for (track, o) in r.tracks {
                        match o {
                            Ok(o) if dd.music.rt.score.is_none() => dd.music.set_track_override(track, o),
                            Ok(_) => {}
                            Err(e) => back_log!(
                                back,
                                warn,
                                "mod-dev: track {track} not replaced: {e:#}; the project's own plays"
                            ),
                        }
                    }
                }
                Engine::Licensed(lm) => lm.replace_packages(&r.plan.music, race),
            }
            back_log!(back, info, "mod-dev: the level's music re-applied ({summary})");
        });
    }
}

fn game_state(ride: Option<&Ride>) -> GameMusicState {
    ride.map_or_else(GameMusicState::default, |r| {
        crate::audio::sfx_input::music_state(&r.state, &r.session, r.rewind_backwards)
    })
}

const LOGIC_DT: f32 = 1.0 / 30.0;

#[derive(Default)]
struct Accum(f32);

fn drive_music(
    time: Res<Time>,
    mut music: ResMut<Music>,
    ride: Option<Res<Ride>>,
    fe: Option<Res<frontend::FrontEndAudio>>,
    mut acc: Local<Accum>,
) {
    let mut gs = game_state(ride.as_deref());
    if let Some(fe) = fe {
        let m = fe.state();
        (gs.race_started, gs.result) = (m.restart, m.result);
    }
    let steps = ride.as_ref().map_or(0, |r| r.steps);
    let respawned = steps < music.last_steps;
    let setup = steps > 0 && music.last_steps == 0;
    music.last_steps = steps;
    acc.0 += time.delta_secs();
    let shared = music.shared.clone();
    if !music.dd {
        if setup {
            music.game.post(move |_, _| {
                if let Ok(mut shared) = shared.lock()
                    && let Engine::Licensed(lm) = &mut shared.engine
                {
                    lm.event_setup();
                }
            });
        }
        return;
    }
    let mut frames = 0u32;
    while acc.0 >= LOGIC_DT {
        acc.0 -= LOGIC_DT;
        frames += 1;
    }
    if frames == 0 && !respawned {
        return;
    }
    let verbose = music.verbose;
    music.game.post(move |_, back| {
        let Ok(mut shared) = shared.lock() else { return };
        let Shared { engine, script, restart_in, log_from, .. } = &mut *shared;
        let Engine::Dd(dd) = engine else { return };
        if respawned {
            // TODO(guess): a respawn = an event restart: the module's stop (vtable +0x20) and the objectives rerun; settle: the restart menu's code (the callers of +0x1C / +0x20).
            dd.module.stop(&mut dd.music.rt);
            *restart_in = Some(dd.module.stop_frames.max(1) as f32 * ssx_assets::sfx::UPDATE_PERIOD);
        }
        for _ in 0..frames {
            match restart_in.as_mut() {
                Some(t) => {
                    for _ in 0..ssx_assets::music_script::MODULE_FRAMES_PER_STEP {
                        dd.module.frame(&mut dd.music.rt);
                    }
                    *t -= LOGIC_DT;
                    if *t <= 0.0 {
                        *restart_in = None;
                        if let Some(script) = script {
                            dd.runner = ScriptRunner::new(script.clone());
                        }
                    }
                }
                None => dd.update(LOGIC_DT, &gs),
            }
        }
        let log = &dd.music.rt.log;
        for l in &log[(*log_from).min(log.len())..] {
            let segment = l.text.contains(" queue n") || l.text.contains(" play n");
            if verbose || !segment {
                back_log!(back, info, "music {:>8.3} s  {}", l.ms as f32 / 1000.0, l.text);
            }
        }
        if log.len() > 4096 {
            dd.music.rt.log.drain(..2048);
        }
        *log_from = dd.music.rt.log.len();
    });
}
