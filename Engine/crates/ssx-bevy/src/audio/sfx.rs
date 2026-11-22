use std::path::Path;
use std::sync::{Arc, Mutex};

use bevy::prelude::*;
use ssx_assets::sfx::heli::HeliSounds;
use ssx_assets::sfx::speech::context::SpeechRandom;
use ssx_assets::sfx::{RiderAudioInfo, SfxEngine, TrackAudioInfo, rider_audio_info, riding_banks, track_audio_info};
use ssx_assets::sfx_avalanche::AvalancheSounds;
use ssx_assets::sfx_game::{EnvironmentSounds, RiderSounds};

use crate::audio::music::{GameAudio, discard, output};
use crate::audio::sfx_input::{
    FrameEvents, InputTrack, SpeechDrop, SpeechIn, SpeechInputs, SpeechLanguage, SpeechTrack, board_emitter,
    drive_speech, drop_audio_helicopter, ended_by_death, reverb_zones, ride_audio, rider_voice_emitter, speech_enabled,
    speech_pilot,
};
use crate::ride::Ride;

#[derive(Resource)]
pub struct Sfx {
    side: Arc<Mutex<RideSide>>,
    track: InputTrack,
    max_boost: f32,
    last_steps: u64,
    tags_through: u64,
    tag_log: bool,
    pending: Option<FirstLoads>,
    speech_time: f32,
    intro: Option<crate::audio::sfx_input::IntroAudio>,
    big_root: std::path::PathBuf,
    geotags_seen: usize,
    event_key: Option<u32>,
    held: Option<ssx_assets::sfx_game::RideAudio>,
    paused_frames: u32,
    objects: ssx_assets::sfx_game::audio_objects::AudioObjects,
    character: String,
    carried: Carried,
}

// PORT(choice): the controllers (the boarder, environment, avalanche and helicopter audio, the speech's messages) run
// on the mixer thread in the command the game thread queues each frame, because the port's engine is one object that
// the mixer renders; the game's run on its game thread, each call taking the system's lock for itself (System::Lock
// 0x82A1AE90) and queueing what reaches the voices (0x82A48C48), applied at the mixer step's drain (0x82A3EE48) — the
// calls reach the mix at the same point, the next mixer frame.
struct RideSide {
    rider: RiderSounds,
    env: EnvironmentSounds,
    avalanche: AvalancheSounds,
    heli: HeliSounds,
    speech: SpeechTrack,
    log_from: usize,
    verbose: bool,
    speech_log: bool,
    profile_t: f32,
}

struct AudioFrame {
    r: ssx_assets::sfx_game::RideAudio,
    paused: bool,
    events: Vec<String>,
    geotags: usize,
    race_start: bool,
    speech: SpeechIn,
    nis_message: Option<u8>,
    heli: Option<ssx_assets::sfx::heli::HeliTransform>,
    camera: Option<([f32; 3], [f32; 3])>,
    emitters: [ssx_assets::sfx_dsp::sound3d::Emitter; 2],
    kind: i32,
    time: f32,
    presentation: i32,
}

type Lines = Vec<(bool, String)>;

impl RideSide {
    fn new(info: &TrackAudioInfo, verbose: bool) -> Self {
        Self {
            rider: RiderSounds::default(),
            env: EnvironmentSounds::new(info.ambience_index),
            avalanche: AvalancheSounds::default(),
            heli: HeliSounds::default(),
            speech: SpeechTrack::default(),
            log_from: 0,
            verbose,
            speech_log: !std::env::var("SSX_SPEECH_LOG").is_ok_and(|v| v == "0"),
            profile_t: 0.0,
        }
    }

    fn respawn(&mut self, engine: &mut SfxEngine) {
        self.rider.shutdown(engine);
        self.env.shutdown(engine);
        self.avalanche.shutdown(engine);
        self.heli.shutdown(engine);
        engine.kill_level_banks();
        self.speech.respawn(engine);
        self.rider.remix_events_reset();
        engine.remix_restart();
    }

    fn frame(&mut self, engine: &mut SfxEngine, mut f: AudioFrame, log: &mut Lines, parts: &mut [f32; 3]) {
        let profiling = engine.mixer.graph.profiling();
        let mut from = std::time::Instant::now();
        let mut part = |k: usize, from: &mut std::time::Instant| {
            if profiling {
                parts[k] += from.elapsed().as_secs_f32();
                *from = std::time::Instant::now();
            }
        };
        f.r.deadly_descent = engine.remix_event_type == 3;
        engine.set_presentation(f.presentation);
        if f.paused {
            self.rider.paused_frame(engine, &f.r);
        } else {
            self.rider.update(engine, &f.r);
            for tag in &f.events {
                engine.fire_event(tag, &[]);
            }
            self.env.update(engine, &f.r);
            self.avalanche.update(engine, &f.r.avalanche);
        }
        if f.geotags > 0 {
            let (crc, name) = crate::audio::sfx_input::GEOTAG_COLLECTED;
            match engine.aems.resolve(crc, name) {
                Some(key) => {
                    for _ in 0..f.geotags {
                        engine.fire(key, &[]);
                    }
                }
                None => log.push((true, format!("sfx: no CSIS event HUD\\{name} (hudsfx_tuna.csi not loaded)"))),
            }
        }
        part(0, &mut from);
        if f.race_start {
            self.rider.race_start(engine);
        }
        let mut lines = Vec::new();
        drive_speech(engine, &f.speech, &mut self.speech, &mut lines);
        log.extend(lines.into_iter().map(|l| (false, l)));
        part(1, &mut from);
        let now_ms = f.time * 1000.0;
        if let Some(m) = f.nis_message {
            self.rider.nis_state(engine, m);
            self.heli.nis_state(engine, m, now_ms);
        }
        if !f.paused {
            self.heli.frame_flags(f.r.time_dilating, f.r.time_dilation);
            if let (Some(h), Some(cam)) = (f.heli, f.camera) {
                self.heli.transform(engine, &h, cam, f.kind, now_ms);
            }
        }
        let pilot = engine.speech.as_ref().is_some_and(|sp| sp.pilot_speaking());
        self.heli.update(engine, pilot);
        let mut emitters = f.emitters.to_vec();
        emitters.extend(self.avalanche.emitters3d());
        emitters.extend(self.heli.emitters());
        engine.sound3d_frame(f.camera, &emitters, FRAME_DT);
        part(2, &mut from);
    }

    fn report(&mut self, engine: &mut SfxEngine, dt: f32, log: &mut Lines) {
        let lines = &engine.aems.log;
        for l in &lines[self.log_from.min(lines.len())..] {
            if self.verbose || !l.text.starts_with("  ") {
                log.push((false, format!("sfx {:>8.3} s  {}", l.ms / 1000.0, l.text)));
            }
        }
        if engine.aems.log.len() > 4096 {
            engine.aems.log.drain(..2048);
        }
        self.log_from = engine.aems.log.len();
        for l in engine.mixer.events.drain(..) {
            if self.verbose {
                log.push((false, format!("sfx voice {l}")));
            }
        }
        stats::VOICES.store(engine.mixer.graph.voice_count() as u64, std::sync::atomic::Ordering::Relaxed);
        if engine.mixer.graph.profiling() {
            self.profile_t += dt;
        }
        if self.profile_t >= 2.0 {
            let secs = std::mem::take(&mut self.profile_t);
            let top: Vec<String> = engine
                .mixer
                .graph
                .take_profile()
                .iter()
                .take(14)
                .map(|(name, ns, n)| format!("{name} {:.2} ({n})", *ns as f64 * 1e-6 / f64::from(secs)))
                .collect();
            log.push((false, format!("sfx profile (ms/s, blocks): {}", top.join(", "))));
        }
        if let Some(sp) = engine.speech.as_mut() {
            for l in sp.log.drain(..) {
                if self.speech_log {
                    log.push((false, format!("speech {l}")));
                }
            }
        }
    }
}

#[derive(Default)]
struct Carried {
    speech: Option<crate::audio::sfx_input::SpeechStep>,
    trick_update: Option<(i32, i32)>,
    rider_dead: bool,
    geotags: usize,
    intro: std::collections::VecDeque<crate::audio::sfx_input::IntroFrame>,
}

const INTRO_FRAMES_HELD: usize = 30;

// PORT(choice): the port observes the ride once per Bevy frame with new steps and runs the audio frames on their own
// 30 Hz clock, so a Bevy frame can end with no audio frame (at 40+ frames a second about one in four): its intro frame
// waits for the next audio frame, in order, instead of being dropped (before, a scene's tags of such a frame never
// fired — the Rockies Deadly Descent's `trigger_DD_Bed` and `trigger_DD_Seq_1` at tick 1 of sc01). Past 30 waiting
// frames a new one joins the newest (its tags appended, a message only where it had none, the newest helicopter).
// The console runs one audio frame per game frame (0x8236E098 → 0x83084DC0 → 0x83088418) after its simulation step,
// so nothing waits there.
fn hold_intro_frame(
    q: &mut std::collections::VecDeque<crate::audio::sfx_input::IntroFrame>,
    f: crate::audio::sfx_input::IntroFrame,
) {
    let full = q.len() >= INTRO_FRAMES_HELD;
    match q.back_mut() {
        Some(last) if full => {
            last.tags.extend(f.tags);
            last.nis_message = last.nis_message.or(f.nis_message);
            last.heli = f.heli.or(last.heli);
        }
        _ => q.push_back(f),
    }
}

#[derive(Clone)]
struct SpeechPending {
    big_root: std::path::PathBuf,
    rider: RiderAudioInfo,
    track: TrackAudioInfo,
    speech: bool,
    lang: SpeechLanguage,
    deadly_descent: bool,
    rng: SpeechRandom,
}

pub struct SfxLevel {
    engine: SfxEngine,
    info: TrackAudioInfo,
    rider: RiderAudioInfo,
    character: String,
    big_root: std::path::PathBuf,
    max_boost: f32,
    volume: f32,
    verbose: bool,
    event_key: Option<u32>,
    lang: SpeechLanguage,
    objects: ssx_assets::sfx_game::audio_objects::AudioObjects,
    first: Option<FirstLoads>,
}

pub fn register(app: &mut App) {
    output(app);
    if app.world().contains_resource::<SfxRegistered>() {
        return;
    }
    let sfx = resource_exists::<Sfx>;
    app.insert_resource(SfxRegistered)
        .init_resource::<SpeechRng>()
        .init_resource::<StepMessages>()
        .add_systems(FixedUpdate, gather_step_messages.after(crate::ride::RideStepSet).run_if(sfx))
        .add_systems(Update, (gather_step_messages.after(crate::ride::RideStepSet), drive_sfx).chain().run_if(sfx));
}

#[derive(Resource)]
struct SfxRegistered;

#[derive(Resource, Default)]
struct StepMessages {
    through: u64,
    steps: u32,
    speech: Option<crate::audio::sfx_input::SpeechStep>,
    rider_dead: bool,
    trick_update: Option<(i32, i32)>,
}

impl StepMessages {
    fn add(&mut self, steps: u64, step: Option<crate::audio::sfx_input::SpeechStep>, trick_update: Option<(i32, i32)>) {
        if steps == self.through {
            return;
        }
        if steps < self.through {
            *self = Self::default();
        }
        self.through = steps;
        self.steps = self.steps.saturating_add(1);
        let Some(step) = step else { return };
        self.rider_dead |= step.rider_dead.is_some();
        self.speech = crate::audio::sfx_input::SpeechStep::merge(self.speech, Some(step));
        self.trick_update = trick_update.or(self.trick_update);
    }

    fn take(&mut self) -> Self {
        std::mem::replace(self, Self { through: self.through, ..Self::default() })
    }
}

fn gather_step_messages(ride: Option<Res<Ride>>, mut messages: ResMut<StepMessages>) {
    let Some(ride) = ride else { return };
    let step = ride.last_exec.as_ref().map(|e| crate::audio::sfx_input::SpeechStep::new(e, &ride.state));
    let trick_update = ride.state.score.trick_update.map(|r| (r.kind as i32, r.interaction));
    messages.add(ride.steps, step, trick_update);
}

#[derive(Resource)]
struct SpeechRng(SpeechRandom);

impl Default for SpeechRng {
    fn default() -> Self {
        Self(SpeechRandom::boot())
    }
}

pub fn load_level(
    big_root: Option<&Path>,
    track: &str,
    character: &str,
    max_boost: f32,
    lang: SpeechLanguage,
    screenshot: bool,
) -> Option<SfxLevel> {
    load_level_event(big_root, track, character, max_boost, SfxLaunch::default(), lang, screenshot)
}

#[derive(Clone, Copy, Debug, Default)]
pub struct SfxLaunch<'a> {
    pub event_key: Option<u32>,
    pub block: Option<&'a ssx_assets::sfx::speech::launch::LaunchBlock>,
}

pub fn load_level_event(
    big_root: Option<&Path>,
    track: &str,
    character: &str,
    max_boost: f32,
    launch: SfxLaunch,
    lang: SpeechLanguage,
    screenshot: bool,
) -> Option<SfxLevel> {
    let mode = std::env::var("SSX_SFX").unwrap_or_default().to_ascii_lowercase();
    if matches!(mode.as_str(), "off" | "0" | "false") || (screenshot && mode.is_empty()) {
        return None;
    }
    let Some(big_root) = big_root else {
        ssx_core::eprintln!("warning: sfx unavailable: no extracted archive root");
        return None;
    };
    let info = track_audio_info(big_root, track).unwrap_or_else(|e| {
        ssx_core::eprintln!("warning: sfx: no audio game info for {track} ({e:#}); the generic one");
        TrackAudioInfo::generic()
    });
    let rider = rider_audio_info(big_root, character).unwrap_or_else(|e| {
        ssx_core::eprintln!("warning: sfx: no audio game info for {character} ({e:#}); the generic one");
        RiderAudioInfo::generic()
    });
    let nis = launch.block.and_then(|b| ssx_assets::sfx::dd_nis_descr(b, &info, lang.name()));
    let below = &mut |_: &str, n| ssx_assets::career::random::below(n);
    let mut banks = match riding_banks(big_root, &info, &rider, nis.as_deref(), below) {
        Ok(b) => b,
        Err(e) => {
            ssx_core::eprintln!("warning: sfx unavailable: {e:#}");
            return None;
        }
    };
    banks.push("fe_sfx".into());
    let names: Vec<&str> = banks.iter().map(String::as_str).collect();
    let mut engine = match SfxEngine::load(big_root, &names) {
        Ok(e) => e,
        Err(e) => {
            ssx_core::eprintln!("warning: sfx unavailable: {e:#}");
            return None;
        }
    };
    let start = std::time::Instant::now();
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get().min(8));
    match engine.decode_ram_sounds(threads) {
        Ok((n, bytes)) => ssx_core::eprintln!(
            "sfx: {n} RAM sounds decoded ahead ({:.1} MiB, {:.2} s)",
            bytes as f64 / (1024.0 * 1024.0),
            start.elapsed().as_secs_f32()
        ),
        Err(e) => ssx_core::eprintln!("warning: sfx: RAM sounds not decoded ahead ({e:#}); at their voices' start"),
    }
    if engine.mixer.graph.profiling() {
        let sizes = engine.mixer.decoded_ram_sizes();
        let total: usize = sizes.iter().map(|s| s.2 * s.3).sum::<usize>().max(1);
        let mut run = 0;
        for (k, (bank, idx, frames, ch)) in sizes.iter().enumerate() {
            run += frames * ch;
            if k < 30 || k % 50 == 0 {
                ssx_core::eprintln!(
                    "sfx ram {k:>4}: {bank} {idx} {frames} frames × {ch} ({:.1}% cumulative)",
                    run as f64 * 100.0 / total as f64
                );
            }
        }
    }
    let verbose = std::env::var("SSX_SFX_LOG").is_ok_and(|v| v == "1");
    engine.set_verbose(verbose);
    let decision =
        ssx_assets::content::GameContent::of(big_root).sound_overrides_for(Some(lang.code()), Some(character));
    if !decision.sounds.is_empty() {
        for id in engine.set_overrides(&decision.sounds) {
            ssx_core::eprintln!("warning: sfx: {id}: no interface of the level's engine defines it");
        }
    }
    if let Err(e) = engine.start_reverb(big_root, &info) {
        ssx_core::eprintln!("warning: sfx reverb unavailable: {e:#}");
    }
    let volume = std::env::var("SSX_SFX_VOLUME").ok().and_then(|v| v.parse().ok()).unwrap_or(1.0f32).clamp(0.0, 1.0);
    ssx_core::eprintln!("sfx: {} banks ({})", banks.len(), banks.join(", "));
    let objects = ssx_assets::sfx_game::audio_objects::AudioObjects::load(big_root).unwrap_or_else(|e| {
        ssx_core::eprintln!("warning: audio_objects unavailable: {e:#}");
        Default::default()
    });
    Some(SfxLevel {
        objects,
        engine,
        info,
        rider,
        character: character.to_string(),
        big_root: big_root.to_path_buf(),
        max_boost,
        volume,
        verbose,
        event_key: launch.event_key,
        lang,
        first: None,
    })
}

fn speech_pending(world: &mut World, l: &SfxLevel, deadly_descent: bool) -> SpeechPending {
    SpeechPending {
        big_root: l.big_root.clone(),
        rider: l.rider.clone(),
        track: l.info.clone(),
        speech: speech_enabled(),
        lang: l.lang,
        deadly_descent,
        rng: world.get_resource_or_insert_with(SpeechRng::default).0.clone(),
    }
}

// PORT(choice): the first ride frame's loads start when the level's data arrives, before its textures go up, instead of
// at its spawn: the spawn frame waited 16-38 ms for them in `drive_sfx` (alps0101, the heli drop; development laptop,
// `trace_chrome`) once the rest of its work had left it. Their inputs are the ones the spawn would hand them (the same
// ride, remix type and drop; the run's speech RNG handle, cloned, not drawn from) and the first ride frame applies their
// result as before; the console's loader threads are not decoded.
pub fn start_first_loads(
    world: &mut World,
    level: Option<&mut SfxLevel>,
    ride: Option<&Ride>,
    remix_event_type: Option<i32>,
    drop: Option<u32>,
) {
    let (Some(l), Some(ride)) = (level, ride) else { return };
    let pending = speech_pending(world, l, remix_event_type == Some(3));
    l.first = Some(FirstLoads::start(pending, RideInputs::of(ride, l.event_key), drop));
}

pub fn start_level(world: &mut World, level: Option<SfxLevel>) {
    let Some(mut l) = level else { return };
    if let Some(s) = world.get_resource::<crate::audio::music::frontend::AudioSettingsRes>() {
        l.engine.apply_settings(&s.0);
    }
    let deadly_descent = world.get_resource::<crate::audio::music::RemixEventType>().is_some_and(|r| r.0 == 3);
    let first = match l.first.take() {
        Some(first) => Ok(first),
        None => Err(speech_pending(world, &l, deadly_descent)),
    };
    let side = Arc::new(Mutex::new(RideSide::new(&l.info, l.verbose)));
    if let Some(g) = world.get_resource::<GameAudio>() {
        let (mut engine, volume) = (l.engine, l.volume);
        g.post(move |m, _| {
            engine.remix_event_type = m.remix_event_type;
            discard(m.sfx.replace(engine));
            (m.sfx_volume, m.swapped) = (volume, true);
        });
    }
    let character = l.character;
    let pending = Some(match first {
        Ok(first) => first,
        Err(pending) => {
            let drop = world.get_resource::<crate::audio::music::AudioDrop>().and_then(|d| d.0);
            match world.get_resource::<Ride>() {
                Some(ride) => FirstLoads::start(pending, RideInputs::of(ride, l.event_key), drop),
                None => FirstLoads::Waiting(Box::new(pending)),
            }
        }
    });
    world.insert_resource(Sfx {
        side,
        track: InputTrack::default(),
        max_boost: l.max_boost,
        last_steps: 0,
        tags_through: 0,
        tag_log: std::env::var("SSX_ANIM_TAG_LOG").is_ok(),
        pending,
        speech_time: 0.0,
        intro: None,
        big_root: l.big_root,
        geotags_seen: 0,
        event_key: l.event_key,
        held: None,
        paused_frames: 0,
        objects: l.objects,
        character,
        carried: Carried::default(),
    });
    world.insert_resource(StepMessages::default());
}

impl Sfx {
    pub fn character(&self) -> &str {
        &self.character
    }
}

pub fn end_level(world: &mut World) {
    if !world.contains_resource::<crate::audio::music::frontend::FrontEndAudio>()
        && let Some(g) = world.get_resource::<GameAudio>()
    {
        g.post(|m, _| discard(m.sfx.take()));
    }
    world.remove_resource::<Sfx>();
}

pub fn insert(app: &mut App, big_root: Option<&Path>, track: &str, character: &str, max_boost: f32, screenshot: bool) {
    let lang = app.world().get_resource::<SpeechLanguage>().copied().unwrap_or_else(|| SpeechLanguage::of(None));
    let Some(level) = load_level(big_root, track, character, max_boost, lang, screenshot) else { return };
    register(app);
    start_level(app.world_mut(), Some(level));
}

pub mod stats {
    use std::sync::atomic::{AtomicU64, Ordering};

    pub static CALLS: AtomicU64 = AtomicU64::new(0);
    pub static NS: AtomicU64 = AtomicU64::new(0);
    pub static MAX_NS: AtomicU64 = AtomicU64::new(0);
    pub static COMMAND_NS: AtomicU64 = AtomicU64::new(0);
    pub static VOICES: AtomicU64 = AtomicU64::new(0);

    pub fn add(ns: u64) {
        CALLS.fetch_add(1, Ordering::Relaxed);
        NS.fetch_add(ns, Ordering::Relaxed);
        MAX_NS.fetch_max(ns, Ordering::Relaxed);
    }

    pub fn command(ns: u64) {
        COMMAND_NS.fetch_add(ns, Ordering::Relaxed);
    }
}

const FRAME_DT: f32 = 1.0 / 30.0;

#[derive(Default)]
struct Accum(f32);

struct DriveTimer {
    start: std::time::Instant,
}

impl Drop for DriveTimer {
    fn drop(&mut self) {
        stats::add(self.start.elapsed().as_nanos() as u64);
    }
}

// PORT(choice): a map's ride launches no `ui_drop` (`shell/game_mode/maps.rs` loads its sounds without an event key),
// so its sounds take the asset viewer's choice among its world's events — on a layout its drop's `race` event by
// default (the helicopter bank, the intro scripts, `UI_DropWarning`, the tutorial flag), on a new track none; the
// drawn helicopter of a layout is its base `ui_drop`'s (`ssx_physics::map::MapHeli`). The console has no maps.
fn ride_event(ride: &Ride, launched: Option<u32>) -> Option<ssx_physics::event::EventDef> {
    ssx_physics::event::level_event(&ride.world().events, launched, crate::ride::viewer_event_choice).cloned()
}

struct RideInputs {
    event: Option<ssx_physics::event::EventDef>,
    kind: ssx_assets::sfx::speech::game::EventKind,
    intro: Option<Arc<ssx_physics::nis::IntroAssets>>,
}

impl RideInputs {
    fn of(ride: &Ride, launched: Option<u32>) -> Self {
        Self {
            event: ride_event(ride, launched),
            kind: crate::audio::sfx_input::speech_kind(&ride.session),
            intro: ride.intro.clone(),
        }
    }
}

enum FirstLoads {
    Waiting(Box<SpeechPending>),
    Loading(std::thread::JoinHandle<(FirstFrameLoads, IntroLoad)>),
}

impl FirstLoads {
    // PORT(choice): the first ride frame's loads (the helicopter's bank with its RAM sounds decoded, the speech's
    // tables and banks, the intro's scripts and NIS scenes) run on their own thread from the level's spawn (the game's
    // shell starts them when the level's data arrives, before its textures go up: `start_first_loads`), and the
    // first ride frame takes their result (waiting for the rest if any): on the game thread they were 0.23 s of the
    // 0.52 s spawn frame (alps0101, development laptop, `trace_chrome`), and the level's startup schedules (0.18 s) now
    // run beside them. The loads read only the level's data and the run's speech RNG handle (cloned, not drawn from),
    // so their results are the same; the frame that applies them is the same. The console's loader threads are not
    // decoded.
    fn start(p: SpeechPending, ride: RideInputs, drop: Option<u32>) -> Self {
        let fallback = p.clone();
        let spawned = std::thread::Builder::new().name("sfx-first-loads".into()).spawn(move || {
            let big_root = p.big_root.clone();
            (FirstFrameLoads::load(p, &ride, drop), IntroLoad::load(&big_root, &ride))
        });
        match spawned {
            Ok(thread) => FirstLoads::Loading(thread),
            Err(e) => {
                warn!("sfx: no thread for the first ride frame's loads ({e}): that frame loads them");
                FirstLoads::Waiting(Box::new(fallback))
            }
        }
    }
}

struct FirstFrameLoads {
    heli: u32,
    bank: anyhow::Result<(ssx_assets::sfx_aems::Bank, ssx_assets::sfx_mixer::DecodedRam)>,
    speech: Option<anyhow::Result<(ssx_assets::sfx::speech::Speech, ssx_assets::sfx::speech::SpeechSetup)>>,
}

impl FirstFrameLoads {
    fn load(p: SpeechPending, ride: &RideInputs, drop: Option<u32>) -> Self {
        let heli = drop_audio_helicopter(&p.big_root, ride.event.as_ref());
        let bank = ssx_assets::sfx::heli::helicopter_bank(&p.big_root, heli).and_then(|name| {
            let bank = ssx_assets::sfx_aems::Bank::load(&p.big_root, &name)?;
            let ram = ssx_assets::sfx_mixer::decode_ram_sounds(&bank)?;
            Ok((bank, ram))
        });
        let speech = p.speech.then(|| {
            let pilot = speech_pilot(&p.big_root, heli)?;
            let setup = SpeechDrop {
                big_root: &p.big_root,
                rider: &p.rider,
                track: &p.track,
                pilot,
                kind: ride.kind,
                drop,
                deadly_descent: p.deadly_descent,
                rng: p.rng,
                random: ssx_assets::career::random::Source::Global,
                lang: p.lang,
            }
            .setup();
            Ok((ssx_assets::sfx::speech::Speech::load(&p.big_root, &setup)?, setup))
        });
        Self { heli, bank, speech }
    }

    fn apply(self, engine: &mut SfxEngine, log: &mut Lines) {
        let heli = self.heli;
        match self.bank {
            Ok((bank, ram)) => {
                engine.add_loaded_bank(bank, ram);
                log.push((false, format!("sfx: helicopter {heli:08x}'s bank loaded")));
            }
            Err(e) => log.push((true, format!("sfx: no helicopter bank ({e:#})"))),
        }
        match self.speech {
            Some(Ok((speech, setup))) => {
                engine.set_speech(speech);
                log.push((
                    false,
                    format!(
                        "speech: {} {} with pilot {}, region {}",
                        setup.lang, setup.rider, setup.pilot, setup.region_id
                    ),
                ));
            }
            Some(Err(e)) => log.push((true, format!("speech unavailable: {e:#}"))),
            None => {}
        }
    }
}

struct IntroLoad {
    drop_warning: i32,
    tutorial: bool,
    intro: crate::audio::sfx_input::IntroAudio,
}

impl IntroLoad {
    fn load(big_root: &Path, ride: &RideInputs) -> Self {
        let def = ride.event.as_ref();
        let drop_warning =
            def.and_then(|d| ssx_assets::sfx_avalanche::ui_drop_warning(big_root, d.key).ok()).unwrap_or(0);
        let tutorial = def.is_some_and(|d| ssx_assets::musicbox::TUTORIAL_DROPS.contains(&d.key));
        let scripts: &[String] = def.map_or(&[], |d| &d.scripts);
        let intro =
            crate::audio::sfx_input::IntroAudio::load(big_root, ride.intro.as_ref(), scripts).unwrap_or_else(|e| {
                warn!("intro audio: {e:#}");
                Default::default()
            });
        Self { drop_warning, tutorial, intro }
    }
}

#[derive(bevy::ecs::system::SystemParam)]
struct SfxWorld<'w, 's> {
    time: Res<'w, Time>,
    ride: Option<Res<'w, Ride>>,
    cameras: Query<'w, 's, &'static GlobalTransform, With<Camera3d>>,
    drop: Option<Res<'w, crate::audio::music::AudioDrop>>,
}

fn drive_sfx(
    world: SfxWorld,
    mut sfx: ResMut<Sfx>,
    mut steps: ResMut<StepMessages>,
    game: Res<GameAudio>,
    mut acc: Local<Accum>,
) {
    let SfxWorld { time, ride, cameras, drop } = world;
    let Some(ride) = ride else { return };
    let _timer = DriveTimer { start: std::time::Instant::now() };
    let launched = sfx.event_key;
    let (loads, first_intro) = match sfx.pending.take() {
        Some(FirstLoads::Waiting(p)) => {
            let inputs = RideInputs::of(&ride, launched);
            let big_root = p.big_root.clone();
            let loads = FirstFrameLoads::load(*p, &inputs, drop.as_ref().and_then(|d| d.0));
            (Some(loads), Some(IntroLoad::load(&big_root, &inputs)))
        }
        Some(FirstLoads::Loading(thread)) => match thread.join() {
            Ok((loads, intro)) => (Some(loads), Some(intro)),
            Err(_) => {
                error!(
                    "sfx: the first ride frame's loads failed (their thread panicked): no helicopter bank or speech"
                );
                (None, None)
            }
        },
        None => (None, None),
    };
    let respawn = ride.steps < sfx.last_steps;
    let intro_load = first_intro
        .or_else(|| {
            (sfx.intro.is_none() || respawn).then(|| IntroLoad::load(&sfx.big_root, &RideInputs::of(&ride, launched)))
        })
        .map(|i| {
            sfx.intro = Some(i.intro);
            (i.drop_warning, i.tutorial)
        });
    let fresh = ride.steps != sfx.last_steps;
    let fresh_intro = if fresh { sfx.intro.as_mut().map(|i| i.frame(&ride.state, ride.heli.as_ref())) } else { None };
    acc.0 += time.delta_secs();
    let sfx = &mut *sfx;
    if respawn {
        sfx.track = InputTrack::default();
        sfx.held = None;
    }
    let gathered = steps.take();
    if gathered.speech.is_some_and(|s| s.race_start) {
        info!(
            "sfx: the race start (ride step {}, {} step(s) since the last audio frame): the remix and Tricky posts on",
            gathered.through, gathered.steps
        );
    }
    let mut rider_dead = gathered.rider_dead;
    let mut speech_events = gathered.speech;
    sfx.last_steps = ride.steps;
    if respawn {
        sfx.carried = Carried::default();
        sfx.tags_through = 0;
    }
    let carried = std::mem::take(&mut sfx.carried);
    let mut intro_frames = carried.intro;
    if let Some(f) = fresh_intro {
        hold_intro_frame(&mut intro_frames, f);
    }
    rider_dead |= carried.rider_dead;
    speech_events = crate::audio::sfx_input::SpeechStep::merge(carried.speech, speech_events);
    let mut trick_update = gathered.trick_update.or(carried.trick_update);
    let mut geotag_pickups =
        crate::audio::sfx_input::geotag_pickups(&ride.session, &mut sfx.geotags_seen) + carried.geotags;
    let kind = crate::audio::sfx_input::event_kind(&ride.session);
    let paused = ride.hold;
    if !paused && sfx.paused_frames > 0 {
        info!(
            "sfx: the pause ended after {} audio frames: the boarder component's messages held, the audio frame (speech \
             clock {:.3} s) ran on",
            std::mem::take(&mut sfx.paused_frames),
            sfx.speech_time
        );
    }
    let camera = cameras.iter().next().map(|t| (t.translation().to_array(), (*t.forward()).to_array()));
    let mut frames = Vec::new();
    while acc.0 >= FRAME_DT {
        acc.0 -= FRAME_DT;
        let ev = FrameEvents {
            rider_dead,
            ended_by_death: ended_by_death(&ride.session),
            reverb_zones: reverb_zones(&ride.session, ride.world()),
            advise_zones: crate::audio::sfx_input::advise_zones(&ride.session, ride.world()),
            sim_dt: ride.last_exec.map_or(ssx_physics::DT_TICKS, |e| e.dt) / ssx_physics::TICKS_PER_SECOND,
            ground_material: crate::audio::sfx_input::ground_material(&ride.state, ride.world(), &sfx.objects),
        };
        rider_dead = false;
        let nis = if paused { Default::default() } else { intro_frames.pop_front().unwrap_or_default() };
        let (r, events) = match sfx.held.clone().filter(|_| paused) {
            Some(mut r) => {
                r.paused = true;
                sfx.paused_frames += 1;
                (r, Vec::new())
            }
            None => {
                let mut r = ride_audio(&ride.state, sfx.max_boost, ride.rewind_backwards, ev, &mut sfx.track);
                r.trick_update = trick_update.take();
                let (state, inside) = sfx.intro.as_ref().map_or((0, false), |i| i.reported());
                let audio_type = sfx.intro.as_ref().map_or(0, |i| i.audio_type);
                (r.nis, r.inside_heli, r.nis_audio_type, r.kind) = (state, inside, audio_type, kind);
                r.eog = crate::audio::sfx_input::end_of_game(&ride.session);
                (r.time_dilating, r.time_dilation_state_one, r.time_dilation) = ride.session.executed_time_dilation();
                let mut events = Vec::new();
                for tag in ride.audio_tags_after(sfx.tags_through) {
                    if sfx.tag_log {
                        info!("anim tag {} (system {}, event {})", tag.name, tag.system, tag.event);
                    }
                    events.push(tag.name.clone());
                }
                sfx.tags_through = ride.steps;
                if sfx.tag_log {
                    for tag in &nis.tags {
                        info!("nis tag {tag} (the rider's AudioEventTag of the playing NIS scene)");
                    }
                }
                events.extend(nis.tags.iter().cloned());
                sfx.held = Some(r.clone());
                (r, events)
            }
        };
        sfx.speech_time += FRAME_DT;
        let step = speech_events.take().unwrap_or_default();
        let speech = SpeechIn::read(SpeechInputs {
            s: &ride.state,
            world: ride.world(),
            session: &ride.session,
            step: &step,
            input: &sfx.track,
            r: &r,
            t: sfx.speech_time,
        });
        frames.push(AudioFrame {
            r,
            paused,
            events,
            geotags: std::mem::take(&mut geotag_pickups),
            race_start: step.race_start,
            speech,
            nis_message: nis.nis_message,
            heli: sfx.intro.as_ref().and_then(|i| i.heli()),
            camera,
            emitters: [board_emitter(&ride.state), rider_voice_emitter(&ride.state)],
            kind,
            time: sfx.speech_time,
            presentation: ride.session.presentation,
        });
    }
    sfx.carried =
        Carried { speech: speech_events, trick_update, rider_dead, geotags: geotag_pickups, intro: intro_frames };
    if loads.is_none() && !respawn && intro_load.is_none() && frames.is_empty() {
        return;
    }
    let (side, dt) = (sfx.side.clone(), time.delta_secs());
    game.post(move |m, back| {
        let start = std::time::Instant::now();
        let Some(engine) = m.sfx.as_mut() else { return };
        let Ok(mut side) = side.lock() else { return };
        let mut log = Lines::new();
        if let Some(l) = loads {
            l.apply(engine, &mut log);
        }
        if respawn {
            side.respawn(engine);
        }
        if let Some((drop_warning, tutorial)) = intro_load {
            side.avalanche.drop_warning = drop_warning;
            engine.tutorial = tutorial;
        }
        let set_up = start.elapsed();
        let mut parts = [0.0f32; 3];
        let n = frames.len();
        for f in frames {
            side.frame(engine, f, &mut log, &mut parts);
        }
        let took = start.elapsed();
        if engine.mixer.graph.profiling() && took.as_secs_f32() > 0.02 {
            log.push((
                false,
                format!(
                    "sfx slow command: set-up {:.1} ms, {n} frames {:.1} ms (controllers {:.1}, speech {:.1}, \
                     helicopter and 3-D {:.1})",
                    set_up.as_secs_f32() * 1e3,
                    (took - set_up).as_secs_f32() * 1e3,
                    parts[0] * 1e3,
                    parts[1] * 1e3,
                    parts[2] * 1e3
                ),
            ));
        }
        side.report(engine, dt, &mut log);
        stats::command(start.elapsed().as_nanos() as u64);
        back.reply(move |_| {
            for (warning, line) in log {
                if warning {
                    warn!("{line}");
                } else {
                    info!("{line}");
                }
            }
        });
    });
}

#[cfg(test)]
mod tests {
    use super::StepMessages;
    use crate::audio::sfx_input::SpeechStep;

    fn race_start() -> Option<SpeechStep> {
        Some(SpeechStep { race_start: true, ..Default::default() })
    }

    #[test]
    fn every_step_of_a_frame_reaches_the_audio() {
        let mut m = StepMessages::default();
        m.add(1, race_start(), Some((1, 2)));
        m.add(2, Some(SpeechStep::default()), None);
        let taken = m.take();
        assert!(taken.speech.is_some_and(|s| s.race_start));
        assert_eq!(taken.trick_update, Some((1, 2)));
        assert_eq!((taken.steps, m.through, m.steps), (2, 2, 0));
        assert!(m.speech.is_none() && m.trick_update.is_none());
        m.add(2, race_start(), None);
        assert!(m.take().speech.is_none());
    }

    #[test]
    fn a_respawn_starts_afresh() {
        let mut m = StepMessages::default();
        m.add(10, race_start(), Some((3, 4)));
        m.add(1, Some(SpeechStep::default()), None);
        let taken = m.take();
        assert!(taken.speech.is_some_and(|s| !s.race_start));
        assert_eq!((taken.through, taken.trick_update), (1, None));
    }
}
