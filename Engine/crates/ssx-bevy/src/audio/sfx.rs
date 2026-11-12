use std::path::Path;

use bevy::prelude::*;
use ssx_assets::sfx::heli::HeliSounds;
use ssx_assets::sfx::{RiderAudioInfo, SfxEngine, TrackAudioInfo, rider_audio_info, riding_banks, track_audio_info};
use ssx_assets::sfx_avalanche::AvalancheSounds;
use ssx_assets::sfx_game::{EnvironmentSounds, RiderSounds};

use crate::audio::music::{GameAudio, output};
use crate::audio::sfx_input::{
    FrameEvents, InputTrack, board_emitter, drive_speech, drop_audio_helicopter, ended_by_death, reverb_zones,
    ride_audio, rider_voice_emitter, speech_enabled, speech_pilot, speech_setup,
};
use crate::ride::Ride;

#[derive(Resource)]
pub struct Sfx {
    rider: RiderSounds,
    env: EnvironmentSounds,
    avalanche: AvalancheSounds,
    track: InputTrack,
    max_boost: f32,
    last_steps: u64,
    log_from: usize,
    verbose: bool,
    pending: Option<SpeechPending>,
    speech: crate::audio::sfx_input::SpeechTrack,
    speech_log: bool,
    speech_time: f32,
    heli: HeliSounds,
    intro: Option<crate::audio::sfx_input::IntroAudio>,
    big_root: std::path::PathBuf,
    geotags_seen: usize,
    event_key: Option<u32>,
    held: Option<ssx_assets::sfx_game::RideAudio>,
    paused_frames: u32,
    objects: ssx_assets::sfx_game::audio_objects::AudioObjects,
    character: String,
}

struct SpeechPending {
    big_root: std::path::PathBuf,
    character: String,
    rider: RiderAudioInfo,
    track: TrackAudioInfo,
    speech: bool,
    deadly_descent: bool,
    seed: u32,
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
    seed: u32,
    event_key: Option<u32>,
    objects: ssx_assets::sfx_game::audio_objects::AudioObjects,
}

pub fn register(app: &mut App) {
    output(app);
    if app.world().contains_resource::<SfxRegistered>() {
        return;
    }
    app.insert_resource(SfxRegistered).add_systems(Update, drive_sfx.run_if(resource_exists::<Sfx>));
}

#[derive(Resource)]
struct SfxRegistered;

pub fn load_level(
    big_root: Option<&Path>,
    track: &str,
    character: &str,
    max_boost: f32,
    screenshot: bool,
) -> Option<SfxLevel> {
    load_level_event(big_root, track, character, max_boost, None, screenshot)
}

pub fn load_level_event(
    big_root: Option<&Path>,
    track: &str,
    character: &str,
    max_boost: f32,
    event_key: Option<u32>,
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
    let seed = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(5489, |d| d.subsec_nanos());
    let mut mt = crate::render::avalanche::Mt19937::new(seed);
    let mut banks = match riding_banks(big_root, &info, &rider, &mut |_, n| mt.below(n)) {
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
    let lang = crate::audio::sfx_input::speech_lang();
    let decision = ssx_assets::content::GameContent::of(big_root).sound_overrides_for(Some(&lang), Some(character));
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
        seed,
        event_key,
    })
}

pub fn start_level(world: &mut World, level: Option<SfxLevel>) {
    let Some(mut l) = level else { return };
    if let Some(s) = world.get_resource::<crate::audio::music::frontend::AudioSettingsRes>() {
        l.engine.apply_settings(&s.0);
    }
    let mut deadly_descent = false;
    if let Some(g) = world.get_resource::<GameAudio>()
        && let Ok(mut m) = g.out.lock()
    {
        l.engine.remix_event_type = m.remix_event_type;
        deadly_descent = m.remix_event_type == 3;
        m.sfx = Some(l.engine);
        (m.sfx_volume, m.swapped) = (l.volume, true);
    }
    let character = l.character.clone();
    let pending = Some(SpeechPending {
        big_root: l.big_root.clone(),
        character: l.character,
        rider: l.rider.clone(),
        track: l.info.clone(),
        speech: speech_enabled(),
        deadly_descent,
        seed: l.seed,
    });
    world.insert_resource(Sfx {
        rider: RiderSounds::default(),
        env: EnvironmentSounds::new(l.info.ambience_index),
        avalanche: AvalancheSounds::default(),
        track: InputTrack::default(),
        max_boost: l.max_boost,
        last_steps: 0,
        log_from: 0,
        verbose: l.verbose,
        pending,
        speech: Default::default(),
        speech_log: !std::env::var("SSX_SPEECH_LOG").is_ok_and(|v| v == "0"),
        speech_time: 0.0,
        heli: HeliSounds::default(),
        intro: None,
        big_root: l.big_root,
        geotags_seen: 0,
        event_key: l.event_key,
        held: None,
        paused_frames: 0,
        objects: l.objects,
        character,
    });
}

impl Sfx {
    pub fn character(&self) -> &str {
        &self.character
    }
}

#[allow(dead_code)]
pub fn end_level(world: &mut World) {
    if !world.contains_resource::<crate::audio::music::frontend::FrontEndAudio>()
        && let Some(g) = world.get_resource::<GameAudio>()
    {
        let _turn = crate::audio::music::GameTurn::take();
        let old = g.out.lock().ok().and_then(|mut m| m.sfx.take());
        drop(old);
    }
    world.remove_resource::<Sfx>();
}

pub fn insert(app: &mut App, big_root: Option<&Path>, track: &str, character: &str, max_boost: f32, screenshot: bool) {
    let Some(level) = load_level(big_root, track, character, max_boost, screenshot) else { return };
    register(app);
    start_level(app.world_mut(), Some(level));
}

pub mod stats {
    use std::sync::atomic::{AtomicU64, Ordering};

    pub static CALLS: AtomicU64 = AtomicU64::new(0);
    pub static NS: AtomicU64 = AtomicU64::new(0);
    pub static MAX_NS: AtomicU64 = AtomicU64::new(0);
    pub static LOCK_NS: AtomicU64 = AtomicU64::new(0);
    pub static VOICES: AtomicU64 = AtomicU64::new(0);

    pub fn add(ns: u64, lock_ns: u64) {
        CALLS.fetch_add(1, Ordering::Relaxed);
        NS.fetch_add(ns, Ordering::Relaxed);
        MAX_NS.fetch_max(ns, Ordering::Relaxed);
        LOCK_NS.fetch_add(lock_ns, Ordering::Relaxed);
    }
}

const FRAME_DT: f32 = 1.0 / 30.0;

#[derive(Default)]
struct Accum(f32);

struct DriveTimer {
    start: std::time::Instant,
    lock_ns: u64,
}

impl Drop for DriveTimer {
    fn drop(&mut self) {
        stats::add(self.start.elapsed().as_nanos() as u64, self.lock_ns);
    }
}

fn ride_event(ride: &Ride, launched: Option<u32>) -> Option<ssx_physics::event::EventDef> {
    if let Some(def) = launched.and_then(|k| ride.world().events.iter().find(|e| e.key == k)) {
        return Some(def.clone());
    }
    let want = std::env::var("SSX_RIDE_EVENT").unwrap_or_else(|_| "race".into());
    if want == "none" { None } else { ssx_physics::event::choose_event(ride.world().events.iter(), &want).cloned() }
}

struct FirstFrameLoads {
    heli: u32,
    bank: anyhow::Result<(ssx_assets::sfx_aems::Bank, ssx_assets::sfx_mixer::DecodedRam)>,
    speech: Option<anyhow::Result<(ssx_assets::sfx::speech::Speech, ssx_assets::sfx::speech::SpeechSetup)>>,
}

impl FirstFrameLoads {
    fn load(p: SpeechPending, ride: &Ride, drop: Option<u32>, launched: Option<u32>) -> Self {
        let heli = drop_audio_helicopter(&p.big_root, ride_event(ride, launched).as_ref());
        let bank = ssx_assets::sfx::heli::helicopter_bank(&p.big_root, heli).and_then(|name| {
            let bank = ssx_assets::sfx_aems::Bank::load(&p.big_root, &name)?;
            let ram = ssx_assets::sfx_mixer::decode_ram_sounds(&bank)?;
            Ok((bank, ram))
        });
        let speech = p.speech.then(|| {
            let pilot = speech_pilot(&p.big_root, heli)?;
            let setup = speech_setup(
                &p.big_root,
                &p.character,
                &p.rider,
                &p.track,
                pilot,
                &ride.session,
                drop,
                p.deadly_descent,
                p.seed,
            );
            Ok((ssx_assets::sfx::speech::Speech::load(&p.big_root, &setup)?, setup))
        });
        Self { heli, bank, speech }
    }

    fn apply(self, engine: &mut SfxEngine) {
        let heli = self.heli;
        match self.bank {
            Ok((bank, ram)) => {
                engine.add_loaded_bank(bank, ram);
                info!("sfx: helicopter {heli:08x}'s bank loaded");
            }
            Err(e) => warn!("sfx: no helicopter bank ({e:#})"),
        }
        match self.speech {
            Some(Ok((speech, setup))) => {
                engine.set_speech(speech);
                info!("speech: {} {} with pilot {}, region {}", setup.lang, setup.rider, setup.pilot, setup.region_id);
            }
            Some(Err(e)) => warn!("speech unavailable: {e:#}"),
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
    fn load(big_root: &Path, ride: &Ride, launched: Option<u32>) -> Self {
        let def = ride_event(ride, launched);
        let drop_warning =
            def.as_ref().and_then(|d| ssx_assets::sfx_avalanche::ui_drop_warning(big_root, d.key).ok()).unwrap_or(0);
        let tutorial = def.as_ref().is_some_and(|d| ssx_assets::musicbox::TUTORIAL_DROPS.contains(&d.key));
        let scripts: Vec<String> = def.map(|d| d.scripts).unwrap_or_default();
        let intro =
            crate::audio::sfx_input::IntroAudio::load(big_root, ride.intro.as_ref(), &scripts).unwrap_or_else(|e| {
                warn!("intro audio: {e:#}");
                Default::default()
            });
        Self { drop_warning, tutorial, intro }
    }
}

#[allow(clippy::too_many_arguments)]
fn drive_sfx(
    time: Res<Time>,
    mut sfx: ResMut<Sfx>,
    game: Res<GameAudio>,
    ride: Option<Res<Ride>>,
    cameras: Query<&GlobalTransform, With<Camera3d>>,
    drop: Option<Res<crate::audio::music::AudioDrop>>,
    mut acc: Local<Accum>,
    mut profile_t: Local<f32>,
) {
    let Some(ride) = ride else { return };
    let mut marks = [std::time::Instant::now(); 4];
    let launched = sfx.event_key;
    let loads = sfx.pending.take().map(|p| FirstFrameLoads::load(p, &ride, drop.as_ref().and_then(|d| d.0), launched));
    let respawn = ride.steps < sfx.last_steps;
    let intro_load =
        (sfx.intro.is_none() || respawn).then(|| IntroLoad::load(&sfx.big_root, &ride, launched)).map(|i| {
            sfx.intro = Some(i.intro);
            (i.drop_warning, i.tutorial)
        });
    let fresh = ride.steps != sfx.last_steps;
    let mut intro = if fresh { sfx.intro.as_mut().map(|i| i.frame(&ride.state, ride.heli.as_ref())) } else { None };
    marks[1] = std::time::Instant::now();
    let mut timer = DriveTimer { start: marks[0], lock_ns: 0 };
    acc.0 += time.delta_secs();
    let sfx = &mut *sfx;
    let Some(mut out) = game.lock() else { return };
    timer.lock_ns = marks[1].elapsed().as_nanos() as u64;
    let Some(engine) = out.sfx.as_mut() else { return };
    if let Some(l) = loads {
        l.apply(engine);
    }
    if respawn {
        sfx.rider.shutdown(engine);
        sfx.env.shutdown(engine);
        sfx.avalanche.shutdown(engine);
        sfx.heli.shutdown(engine);
        engine.kill_level_banks();
        sfx.track = InputTrack::default();
        sfx.speech.respawn(engine);
        sfx.held = None;
    }
    if let Some((drop_warning, tutorial)) = intro_load {
        sfx.avalanche.drop_warning = drop_warning;
        engine.tutorial = tutorial;
    }
    marks[2] = std::time::Instant::now();
    let mut rider_dead = fresh && ride.last_exec.is_some_and(|e| e.rider_dead.is_some());
    let mut speech_events = if fresh {
        ride.last_exec.as_ref().map(|e| crate::audio::sfx_input::SpeechStep::new(e, &ride.state))
    } else {
        None
    };
    sfx.last_steps = ride.steps;
    let mut trick_update =
        if fresh { ride.state.score.trick_update.map(|r| (r.kind as i32, r.interaction)) } else { None };
    let mut geotag_pickups = crate::audio::sfx_input::geotag_pickups(&ride.session, &mut sfx.geotags_seen);
    let kind = crate::audio::sfx_input::event_kind(&ride.session);
    let dd = engine.remix_event_type == 3;
    let mut parts = [0.0f32; 4];
    let profiling = engine.mixer.graph.profiling();
    let mut part = |k: usize, from: &mut std::time::Instant| {
        if profiling {
            parts[k] += from.elapsed().as_secs_f32();
            *from = std::time::Instant::now();
        }
    };
    part(3, &mut { marks[2] });
    let paused = ride.hold;
    if !paused && sfx.paused_frames > 0 {
        info!(
            "sfx: the pause ended after {} audio frames: the boarder component's messages held, the audio frame (speech \
             clock {:.3} s) ran on",
            std::mem::take(&mut sfx.paused_frames),
            sfx.speech_time
        );
    }
    while acc.0 >= FRAME_DT {
        acc.0 -= FRAME_DT;
        let mut from = std::time::Instant::now();
        let ev = FrameEvents {
            rider_dead,
            ended_by_death: ended_by_death(&ride.session),
            reverb_zones: reverb_zones(&ride.session, ride.world()),
            advise_zones: crate::audio::sfx_input::advise_zones(&ride.session, ride.world()),
            sim_dt: ride.last_exec.map_or(ssx_physics::DT_TICKS, |e| e.dt) / ssx_physics::TICKS_PER_SECOND,
            ground_material: crate::audio::sfx_input::ground_material(&ride.state, ride.world(), &sfx.objects),
        };
        rider_dead = false;
        let nis = intro.take().unwrap_or_default();
        let r = match sfx.held.clone().filter(|_| paused) {
            Some(mut r) => {
                r.paused = true;
                sfx.paused_frames += 1;
                sfx.rider.paused_frame(engine, &r);
                r
            }
            None => {
                let mut r = ride_audio(&ride.state, sfx.max_boost, ride.rewind_backwards, ev, &mut sfx.track);
                r.trick_update = trick_update.take();
                let (state, inside) = sfx.intro.as_ref().map_or((0, false), |i| i.reported());
                let audio_type = sfx.intro.as_ref().map_or(0, |i| i.audio_type);
                (r.nis, r.inside_heli, r.nis_audio_type, r.kind, r.deadly_descent) =
                    (state, inside, audio_type, kind, dd);
                r.eog = crate::audio::sfx_input::end_of_game(&ride.session);
                (r.time_dilating, r.time_dilation_state_one, r.time_dilation) = ride.session.executed_time_dilation();
                sfx.rider.update(engine, &r);
                for tag in &nis.tags {
                    engine.fire_event(tag, &[]);
                }
                sfx.env.update(engine, &r);
                sfx.avalanche.update(engine, &r.avalanche);
                sfx.held = Some(r.clone());
                r
            }
        };
        if geotag_pickups > 0 {
            let (crc, name) = crate::audio::sfx_input::GEOTAG_COLLECTED;
            match engine.aems.resolve(crc, name) {
                Some(key) => {
                    for _ in 0..geotag_pickups {
                        engine.fire(key, &[]);
                    }
                }
                None => warn!("sfx: no CSIS event HUD\\{name} (hudsfx_tuna.csi not loaded)"),
            }
            geotag_pickups = 0;
        }
        part(0, &mut from);
        sfx.speech_time += FRAME_DT;
        let step = speech_events.take().unwrap_or_default();
        if step.race_start {
            sfx.rider.race_start(engine);
        }
        drive_speech(
            engine,
            &ride.state,
            ride.world(),
            &ride.session,
            &step,
            &sfx.track,
            &r,
            sfx.speech_time,
            &mut sfx.speech,
        );
        part(1, &mut from);
        let camera = cameras.iter().next().map(|t| (t.translation().to_array(), (*t.forward()).to_array()));
        let now_ms = sfx.speech_time * 1000.0;
        if let Some(m) = nis.nis_message {
            sfx.heli.nis_state(engine, m, now_ms);
        }
        if !paused {
            sfx.heli.frame_flags(r.time_dilating, r.time_dilation);
            if let (Some(h), Some(cam)) = (sfx.intro.as_ref().and_then(|i| i.heli()), camera) {
                sfx.heli.transform(engine, &h, cam, kind, now_ms);
            }
        }
        let pilot = engine.speech.as_ref().is_some_and(|sp| sp.pilot_speaking());
        sfx.heli.update(engine, pilot);
        let mut emitters = vec![board_emitter(&ride.state), rider_voice_emitter(&ride.state)];
        emitters.extend(sfx.avalanche.emitters3d());
        emitters.extend(sfx.heli.emitters());
        engine.sound3d_frame(camera, &emitters, FRAME_DT);
        part(2, &mut from);
    }
    marks[3] = std::time::Instant::now();
    if engine.mixer.graph.profiling() && (marks[3] - marks[0]).as_secs_f32() > 0.02 {
        let ms = |a: std::time::Instant, b: std::time::Instant| (b - a).as_secs_f32() * 1000.0;
        info!(
            "sfx slow frame: loads {:.1} ms, lock {:.1}, set-up {:.1}, frames {:.1} (the step's messages {:.1}, \
             controllers {:.1}, speech {:.1}, helicopter and 3-D {:.1})",
            ms(marks[0], marks[1]),
            timer.lock_ns as f32 * 1e-6,
            ms(marks[1], marks[2]) - timer.lock_ns as f32 * 1e-6,
            ms(marks[2], marks[3]),
            parts[3] * 1e3,
            parts[0] * 1e3,
            parts[1] * 1e3,
            parts[2] * 1e3
        );
    }
    let log = &engine.aems.log;
    for l in &log[sfx.log_from.min(log.len())..] {
        if sfx.verbose || !l.text.starts_with("  ") {
            info!("sfx {:>8.3} s  {}", l.ms / 1000.0, l.text);
        }
    }
    if engine.aems.log.len() > 4096 {
        engine.aems.log.drain(..2048);
    }
    sfx.log_from = engine.aems.log.len();
    engine.mixer.events.clear();
    stats::VOICES.store(engine.mixer.graph.voice_count() as u64, std::sync::atomic::Ordering::Relaxed);
    if engine.mixer.graph.profiling() {
        *profile_t += time.delta_secs();
    }
    if *profile_t >= 2.0 {
        let secs = std::mem::take(&mut *profile_t);
        let top: Vec<String> = engine
            .mixer
            .graph
            .take_profile()
            .iter()
            .take(14)
            .map(|(name, ns, n)| format!("{name} {:.2} ({n})", *ns as f64 * 1e-6 / f64::from(secs)))
            .collect();
        info!("sfx profile (ms/s, blocks): {}", top.join(", "));
    }
    if let Some(sp) = engine.speech.as_mut() {
        for l in sp.log.drain(..) {
            if sfx.speech_log {
                info!("speech {l}");
            }
        }
    }
}
