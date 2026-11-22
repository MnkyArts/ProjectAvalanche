use std::time::Instant;

use bevy::prelude::*;

use super::{
    FrontEndCamera, GameUiRes, LoadedLevel, Shell, WorldOp, audio_values, settings_values, spawn_front_end_camera,
};
use crate::audio::music::frontend::{self as fe_audio, FrontEndAudio};
use crate::shell::{game_level, level};

pub(super) fn shell_world(world: &mut World) {
    let (op, audio, stats) = {
        let mut s = world.resource_mut::<Shell>();
        let op = s.op.get_mut().ok().and_then(Option::take);
        (op, std::mem::take(&mut s.op_audio), std::mem::take(&mut s.op_stats))
    };
    if stats {
        let rides = world.resource::<Shell>().rides_done;
        let stats = level::stats(world);
        info!("shell: the world 120 frames after level {rides}'s teardown: {stats}");
    }
    if audio {
        let values = settings_values(&world.resource::<GameUiRes>().values);
        fe_audio::apply_audio_settings(world, &audio_values(&values));
    }
    match op {
        Some(WorldOp::Spawn(loaded)) => stage_level(world, loaded),
        Some(WorldOp::Teardown) => {
            if let Some(staged) = take_staged(world) {
                spawn_level(world, *staged);
            }
            teardown_level(world);
        }
        None => {}
    }
    if world.get_resource::<crate::render::uploads::LevelUploads>().is_none_or(|u| u.textures_in())
        && let Some(staged) = take_staged(world)
    {
        spawn_level(world, *staged);
    }
}

#[derive(Resource)]
struct StagedLevel(std::sync::Mutex<Box<LoadedLevel>>);

// PORT(choice): under the loading screen the level's spawn waits until its terrain and object textures are in the
// image store, 8 MiB a frame (Mount Eddie's 95 textures, 34.4 MiB, five frames). Sent beside the meshes after the
// spawn instead, its materials were prepared in the order their textures arrived, which changed their bind group
// indices and with them the order of the opaque draws: the GPU's main pass took 5.8 → 6.4–6.5 ms on Mount Eddie and
// 4.9 → 5.6 ms on Whitehorn's benchmark scene (development laptop). The console streams its levels from the disc
// into memory it owns.
fn stage_level(world: &mut World, mut loaded: Box<LoadedLevel>) {
    crate::render::uploads::begin(world);
    loaded.data.queue_textures(world);
    let drop_key = loaded.data.lighting.as_ref().map(|l| l.drop_key);
    let remix = crate::audio::music::MusicLevel::remix_event_type(loaded.music.as_ref(), world);
    let ride = loaded.data.ride.as_ref().map(crate::ride::RideLoad::ride);
    crate::audio::sfx::start_first_loads(world, loaded.sfx.as_mut(), ride, remix, drop_key);
    preload_hud(world);
    world.insert_resource(StagedLevel(std::sync::Mutex::new(loaded)));
}

fn take_staged(world: &mut World) -> Option<Box<LoadedLevel>> {
    let staged = world.remove_resource::<StagedLevel>()?;
    Some(staged.0.into_inner().unwrap_or_else(std::sync::PoisonError::into_inner))
}

pub(super) fn speech_block(
    big_root: &std::path::Path,
    track: &str,
    character: &str,
    words: ssx_assets::sfx::speech::launch::CampaignWords,
    required: [bool; 7],
    descriptions: [Option<u32>; ssx_ui::game::fe::setup::GEAR_TYPES],
) -> ssx_assets::sfx::speech::launch::LaunchBlock {
    use ssx_assets::sfx::speech::launch;
    let infos = launch::gear_audio_infos(big_root)
        .inspect_err(|e| warn!("shell: speech block: no gear audio infos ({e:#})"))
        .unwrap_or_default();
    let gear = descriptions.map(|d| d.and_then(|d| infos.get(&d).copied()).unwrap_or(0));
    let rider = ssx_assets::sfx::rider_audio_info(big_root, character).map_or(0, |r| r.key);
    let peak = ssx_assets::sfx::track_audio_info(big_root, track).ok().map(|t| (t.dd_rider, t.dd_gear));
    let block = launch::LaunchBlock::new(words, required, gear, rider, peak);
    info!(
        "shell: speech block (0x823F7788): chapter {} drop {}/{} dd {} hold {} gear {:?}",
        words.chapter, words.drop, words.drops, words.dd, block.dd_hold, block.gear
    );
    block
}

fn spawn_level(world: &mut World, loaded: LoadedLevel) {
    let t0 = Instant::now();
    let LoadedLevel { data, music, sfx, game, speech_block } = loaded;
    let drop_key = data.lighting.as_ref().map(|l| l.drop_key);
    let load_secs = data.load_secs;
    let fe_cams: Vec<Entity> = world.query_filtered::<Entity, With<FrontEndCamera>>().iter(world).collect();
    for e in fe_cams {
        world.despawn(e);
    }
    level::begin(world);
    if let Some(mut a) = world.get_resource_mut::<FrontEndAudio>() {
        a.unplug();
    }
    if let Err(e) = data.insert(world) {
        error!("shell: the level's data did not go in: {e:#}");
    }
    crate::audio::music::start_level(world, music, drop_key);
    crate::audio::sfx::start_level(world, sfx);
    let goals = world.get_resource::<GameUiRes>().map(|ui| [ui.values.int(0x87), ui.values.int(0x6E)]);
    if let Some(goals) = goals {
        match world.get_resource::<FrontEndAudio>() {
            Some(fe) => fe.hand_over_to_level(goals, speech_block),
            None => {
                if let Some(g) = world.get_resource::<crate::audio::music::GameAudio>() {
                    g.post(move |m, _| {
                        if let Some(sfx) = m.sfx.as_mut() {
                            sfx.ui_goals = goals;
                            sfx.launch_block = speech_block;
                        }
                    });
                }
            }
        }
    }
    let insert_secs = t0.elapsed().as_secs_f32();
    let startup_secs = level::run_startup(world);
    game_level::start(world, game);
    let stats = level::stats(world);
    info!(
        "shell: the level is in the world: loaded in {load_secs:.2} s (worker thread), inserted in {insert_secs:.3} s, \
         spawned in {startup_secs:.3} s (main thread); {stats}"
    );
}

pub(super) const HUD_VIEW: &str = "views/hud.gfx";

// PORT(choice): the HUD's view movie `views/hud.gfx` and the movies it imports are parsed into the UI's library in the
// frame the level's loaded data arrives, under the loading screen, before its textures go up; the HUD's first frame
// after `go_to_hud` parsed them on the game thread (35 of the 52 ms of that frame at alps0101's start, development
// laptop, `trace_chrome`), and in the level's spawn frame, the longest of the load, they took 5.9-9.0 ms of it; in the
// data's frame, which makes no entities, 5.5-10.5 ms (alps0101, Mount Eddie; untraced marks). The library keeps every
// movie it parsed for the session, so the view's load in its own frame finds them, as a second ride's does; the load
// itself, its init actions and its events stay in that frame in the UI's order. Then the views of the HUD's containers,
// as the framework's ViewMap names them (`GameUi::hud_view_files`), are asked to be parsed ahead on a thread; the HUD's
// navigation loads three of them 0.06-0.21 s into a race, 0.5-1.8 s after this frame. Asked with the view when the
// level's load started instead, the thread took 28-38 ms of CPU (the view alone 14-24) from 0.30-0.40 s after the
// event's choice, beside the level's loading threads, and on 4 cores the loading screen's frames over 16.7 ms 0.33-0.70
// s after the choice went from 24 to 39 in eight runs (alps0101, Mount Eddie; development laptop); here the level's
// loading threads have ended. The console's resource loads are not decoded.
fn preload_hud(world: &mut World) {
    world.try_resource_scope(|world, mut scene: Mut<crate::ui::UiScene>| {
        let Some(sc) = scene.state() else { return };
        if let Err(e) = sc.lib.load_movie(HUD_VIEW) {
            debug!("shell: {HUD_VIEW} not parsed ahead: {e:#}");
        }
        let (Some(ui), Some(vm)) = (world.get_resource::<GameUiRes>(), sc.vm.as_mut()) else { return };
        let files = ui.hud_view_files(vm, &mut sc.lib, &mut sc.player);
        info!("ui: the HUD's containers parsed ahead from {}", files.join(", "));
        sc.lib.parse_ahead(files);
    });
}

fn teardown_level(world: &mut World) {
    let t0 = Instant::now();
    game_level::end(world);
    crate::render::uploads::reset(world);
    crate::audio::sfx::end_level(world);
    crate::audio::music::end_level(world);
    let (entities, resources) = level::teardown(world);
    if let Some(mut a) = world.get_resource_mut::<FrontEndAudio>() {
        a.plug();
    }
    spawn_front_end_camera(world);
    let stats = level::stats(world);
    info!(
        "shell: the level is torn down in {:.3} s ({entities} entities despawned; resources added since it began that \
         remain: {resources}); {stats}",
        t0.elapsed().as_secs_f32()
    );
}
