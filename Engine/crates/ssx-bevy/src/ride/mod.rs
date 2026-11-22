use bevy::prelude::{
    App, Assets, FixedUpdate, Image, IntoScheduleConfigs, KeyCode, PostUpdate, PreUpdate, Quat, Resource,
    RunFixedMainLoop, RunFixedMainLoopSystems, SystemSet, Update, Vec2, Vec3, World, debug, info, not, resource_exists,
    warn,
};
use ssx_assets::postfx_overlay::OverlayDrop;
use ssx_physics::SIM_HZ;
use ssx_physics::rewind::RewindPad;
use ssx_physics::rider::{Input, RiderState, TrickInput, World as PhysicsWorld};
use ssx_physics::session::{Execution, Session};

use crate::camera::CameraMode;
use crate::rider::skin::LoadedRider;
use crate::rider::visual::{self as rider_visual, LoadedBoard};
pub use clock::PausedFrames;
use clock::{fixed_step, frame_step, place_rider, report};
use controls::{PadLock, latch_frame, latch_frame_end, viewer_key_presses};
use draw::{
    animate_rider, draw_rail_debug, hide_viewer_hud, light_gear, outro_fade, screen_effects, shade_rider,
    spawn_outro_fade, terrain_ripple, toggle_viewer_hud, world_render,
};
use hud::{GearHud, RiderLabel, spawn_hud, update_hud, update_trick_hud};
use scripted::AutoRewind;

pub mod bots;
mod clock;
mod controls;
mod draw;
pub mod helicopter;
mod hud;
mod key_player;
mod latch;
pub mod live;
mod map_intro;
pub mod rumble;
mod scripted;
mod start;
mod step;
mod strip;
mod tap_script;
mod workers;

pub use draw::TerrainRipple;

const TRIGGER_THRESHOLD: f32 = 0.5;
const L2_HOLD_MIN: f32 = 0.2;

pub struct RideSetup {
    pub world: PhysicsWorld,
    pub spawn: (Vec3, Vec3),
    pub drop_start: bool,
    pub big_root: std::path::PathBuf,
    pub character: String,
    pub wire_key: u32,
    pub track: String,
    pub drop_key: Option<u32>,
    pub fixed_input: Option<Input>,
    pub jump_release: Vec<u32>,
    pub air_input: Option<Vec2>,
    pub steer_from: Option<f32>,
    pub grab: Option<Vec2>,
    pub tweak: bool,
    pub shoulders: Option<[bool; 4]>,
    pub boost: bool,
    pub boost_meter: f32,
    pub screenshot_steps: Option<u32>,
    pub board: Option<LoadedBoard>,
    pub rider: Option<LoadedRider>,
    pub fly_camera: bool,
    pub load_secs: f32,
    pub event_key: Option<u32>,
    pub free_ride: bool,
    pub collected_geotags: Vec<u64>,
    pub game: bool,
    pub classic: bool,
    pub autoplay: bool,
    pub setup: Option<ssx_physics::gear::GameSetup>,
    pub intro_visits: Option<u32>,
    pub own_tombstone: Option<ssx_physics::session::tombstones::OwnRecord>,
    pub own_ghost: Option<(usize, ssx_assets::ghosts::GhostReplay)>,
    pub live: Option<live::LiveRules>,
    pub game_mode: Option<i64>,
    pub sim_content_id: Option<String>,
    pub map: Option<MapRide>,
}

#[derive(Clone, Debug)]
pub struct MapRide {
    pub id: ssx_content::ContentId,
    pub title: String,
    pub gates: ssx_physics::map::Gates,
    pub setup: ssx_physics::event::EventSetup,
    pub player: ssx_physics::map::MapPlayer,
    pub finished: bool,
    pub markers: Vec<MapMarker>,
    pub drawn: (u64, u64),
    pub mode: Option<MapRideMode>,
    pub heli: Option<std::sync::Arc<ssx_physics::map::MapHeli>>,
    pub placed: Vec<(u32, u32, ssx_assets::track_objects::Placement)>,
}

#[derive(Clone, Debug)]
pub struct MapRideMode {
    pub name: String,
    pub title: Option<String>,
    pub rules: ssx_content::gamemode::Rules,
    pub gamemode: Option<ssx_content::ContentId>,
}

impl MapRideMode {
    pub fn of(m: &ssx_assets::content::gamemodes::ResolvedMode) -> Self {
        MapRideMode {
            name: m.choice.name(),
            title: m.gamemode.as_ref().map(|g| g.value.title.clone()),
            rules: m.rules.clone(),
            gamemode: m.gamemode.as_ref().map(|g| g.id.clone()),
        }
    }
}

#[derive(Clone, Debug)]
pub struct MapMarker {
    pub node: String,
    pub look: std::sync::Arc<ssx_assets::content::markers::MarkerLook>,
    pub position: [f32; 3],
    pub heading: f32,
}

impl MapRide {
    pub fn new(
        id: ssx_content::ContentId,
        title: String,
        gates: ssx_physics::map::Gates,
        setup: ssx_physics::event::EventSetup,
    ) -> MapRide {
        let fall_at =
            std::env::var("SSX_MAP_FALL_AT").ok().and_then(|v| v.parse::<f32>().ok()).filter(|t| t.is_finite());
        let player = ssx_physics::map::MapPlayer::new(fall_at);
        MapRide {
            id,
            title,
            gates,
            setup,
            player,
            finished: false,
            markers: Vec::new(),
            drawn: (0, 0),
            mode: None,
            heli: None,
            placed: Vec::new(),
        }
    }

    pub fn target(&self, world: &PhysicsWorld) -> Option<Vec3> {
        let gate = self.gates.checkpoints.get(self.gates.next).map(|v| v.centre);
        gate.or_else(|| self.gates.finish.and_then(|i| world.triggers.volumes.get(i)).map(|v| v.centre))
    }
}

#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct RideStepSet;

#[derive(Clone, Copy, Debug)]
pub struct Pose {
    pub traj: Vec3,
    pub traj_rot: Quat,
    pub board: Vec3,
    pub board_rot: Quat,
}

enum Clock {
    Fixed,
    PerFrame { started: bool, remaining: u32, sub: u32, drawn: u32, phase: u32 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DrawnFrame {
    pub index: u32,
    pub phase: u32,
    pub sub: u32,
}

pub struct OutroRun {
    pub playing: bool,
    pub assets: Option<std::sync::Arc<ssx_physics::nis::IntroAssets>>,
    pub reaction: Option<i64>,
}

#[derive(Resource)]
pub struct Ride {
    world: std::sync::Arc<PhysicsWorld>,
    scripts_task: Option<bevy::tasks::Task<std::thread::Result<ssx_physics::session::ScriptsBuild>>>,
    spawn: (Vec3, Vec3),
    pub intro: Option<std::sync::Arc<ssx_physics::nis::IntroAssets>>,
    intro_short: Option<std::sync::Arc<ssx_physics::nis::IntroAssets>>,
    big_root: std::path::PathBuf,
    sim_content_id: Option<String>,
    pub map: Option<MapRide>,
    intro_graph: bool,
    pub cabin_seen: Option<usize>,
    pub world_hidden: bool,
    pub heli: Option<ssx_physics::helicopter::Helicopter>,
    heli_flight: Option<std::sync::Arc<ssx_physics::helicopter::HeliFlightData>>,
    pub vista: Option<ssx_physics::outro::VistaCamera>,
    event_def: Option<ssx_physics::event::EventDef>,
    character: String,
    wire_key: u32,
    pub outro: Option<OutroRun>,
    pub state: RiderState,
    prev: Pose,
    curr: Pose,
    step_ticks: f32,
    render_lag: f32,
    render_steps: u64,
    pub render: Pose,
    pub render_time: f32,
    pub steps: u64,
    pub(crate) audio_tag_log: AudioTagLog,
    pub distance: f64,
    pub input: Input,
    pub tricks: TrickInput,
    fixed_input: Option<Input>,
    jump_release: Vec<u32>,
    air_input: Option<Vec2>,
    steer_from: Option<f32>,
    grab: Option<Vec2>,
    tweak: bool,
    shoulders: Option<[bool; 4]>,
    boost: bool,
    boost_meter: f32,
    r2: f32,
    clock: Clock,
    report: bool,
    pub step_ms: f32,
    pub session: Session,
    rewind_pad: RewindPad,
    pub rewinding: bool,
    l2: bool,
    l2_hold: bool,
    grind_fixed: bool,
    pub rail_debug: bool,
    last_grind_capture: f32,
    pub rewind_backwards: bool,
    pub last_exec: Option<Execution>,
    press_script: Option<(bool, f32, f32)>,
    r1: bool,
    r3: bool,
    wingsuit_script: Option<(f32, f32)>,
    ws_pressed: Option<f32>,
    ws_deployed: Option<f32>,
    oxygen_held: bool,
    last_press_capture: f32,
    pub motion_blur_cue: i32,
    pub hold: bool,
    pub paused: bool,
    back_held: bool,
    back_was: bool,
    pub back_press: bool,
    autoplay: bool,
    classic: bool,
    auto_rewind: AutoRewind,
    autoplay_waited: u64,
    autoplay_sat_out: bool,
    numpad_held: Vec<KeyCode>,
    key_rewind: key_player::KeyRewindRun,
    key_rewind_state: ssx_physics::rewind::RewindState,
    key_holds: key_player::KeyHoldsRun,
    course: Option<ssx_physics::harness::CoursePlayer>,
    course_logged: usize,
    course_start_at: f32,
    flare_pushes: usize,
    viewer_keys: bool,
    a_lock: PadLock,
    latch: latch::PadLatch,
    flush_latch: bool,
    step_read: bool,
    grind_since: Option<f32>,
    grind_ended: Option<f32>,
    messages_logged: f64,
    pub loadout: ssx_assets::character::Loadout,
    pub game_setup: Option<ssx_physics::gear::GameSetup>,
    pub live: Option<live::LiveRules>,
    pub pads_held: bool,
    pub chat_held: bool,
    pub pose_blocks: Option<ssx_physics::graph::LiveBlocks>,
    pub ghost_rumble: ssx_physics::graph::LandingRumble,
    rumble: rumble::RumblePost,
    pub start_gate: live::StartGate,
    pub go_step: Option<u64>,
    stream_start: Vec3,
    stream_from_start: bool,
}

impl Ride {
    pub fn stream_from_start(&self) -> Option<Vec3> {
        self.stream_from_start.then_some(self.stream_start)
    }

    pub fn sim_content_id(&self) -> Option<&str> {
        self.sim_content_id.as_deref()
    }

    pub fn big_root(&self) -> &std::path::Path {
        &self.big_root
    }

    pub fn world(&self) -> &PhysicsWorld {
        &self.world
    }

    // PORT(choice): the objectives' sets A and B, the game start's other batches with the scene batch and the entities'
    // controllers are built on a thread from the level's start (the spawn frame: the flag store seeded, the loading
    // threads ended) instead of in the ride's first execution on the game thread, where they took 10.9–19.3 ms and made
    // the ride's first frame 25–37 ms (alps0101, Mount Eddie, the heli scene, an online race; untraced, development
    // laptop). The first execution takes the build only when it was made from its world, the session's inputs and the
    // content state as they are then (`ScriptsBuild::made_from`), else builds them as before. The console built them in
    // the first update of `FMGameScripting` (0x82379D40); its threads are not decoded.
    pub fn build_scripts_ahead(&mut self) {
        let inputs = ssx_physics::session::ScriptsInputs::of(&self.session);
        let world = std::sync::Arc::clone(&self.world);
        let pool = bevy::tasks::AsyncComputeTaskPool::get_or_init(bevy::tasks::TaskPool::default);
        self.scripts_task = Some(pool.spawn(async move {
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
                let start = std::time::Instant::now();
                let build = ssx_physics::session::ScriptsBuild::ahead(inputs, world);
                info!("objectives: the level's scripts built ahead in {:.1} ms", start.elapsed().as_secs_f64() * 1e3);
                build
            }))
        }));
    }

    pub fn event_def(&self) -> Option<&ssx_physics::event::EventDef> {
        self.event_def.as_ref()
    }

    pub fn set_classic(&mut self, classic: bool) {
        self.classic = classic;
        self.state.tricks.classic = classic;
    }

    pub fn classic(&self) -> bool {
        self.classic
    }

    #[cfg(feature = "scripts")]
    pub fn world_arc(&self) -> std::sync::Arc<PhysicsWorld> {
        self.world.clone()
    }

    pub fn step(&self) -> (&Pose, f32) {
        (&self.curr, self.step_ticks)
    }

    pub fn previous_step(&self) -> &Pose {
        &self.prev
    }

    pub fn drawn_frame(&self) -> Option<DrawnFrame> {
        match self.clock {
            Clock::PerFrame { started: true, sub, drawn, phase, .. } if drawn > 0 => {
                Some(DrawnFrame { index: drawn - 1, phase, sub })
            }
            _ => None,
        }
    }

    pub fn render_frac(&self) -> f32 {
        (self.render_time * SIM_HZ - (self.steps as f32 - 1.0)).clamp(0.0, 1.0)
    }

    pub fn render_steps(&self) -> f32 {
        self.steps as f32 - self.render_lag * SIM_HZ
    }

    pub fn live_clock(&self) -> bool {
        matches!(self.clock, Clock::Fixed)
    }

    pub fn seconds(&self) -> f32 {
        self.steps as f32 / SIM_HZ
    }

    pub fn character(&self) -> &str {
        &self.character
    }

    pub fn wire_key(&self) -> u32 {
        self.wire_key
    }
}

pub struct RideLoad {
    ride: Ride,
    board: Option<LoadedBoard>,
    body: Option<LoadedRider>,
    fly: bool,
    load_secs: f32,
    spawn: (Vec3, Vec3),
    layers: Option<anyhow::Result<crate::render::post::layers::LayersLoad>>,
    overlay: Option<anyhow::Result<crate::render::post::overlay::OverlayLoad>>,
    topogrid: Option<anyhow::Result<crate::render::post::topogrid::TopogridLoad>>,
    avalanche: Option<crate::render::avalanche::AvalancheScene>,
    noise: Option<crate::camera::chase::CameraNoise>,
    ripple: Option<ssx_physics::controllers::ripple::RippleTuning>,
    label: String,
}

impl RideLoad {
    pub fn queue_images(&mut self, uploads: &mut crate::render::uploads::LevelUploads, images: &mut Assets<Image>) {
        if let Some(b) = self.body.as_mut() {
            b.queue_images(uploads, images);
        }
        if let Some(b) = self.board.as_mut() {
            b.queue_images(uploads, images);
        }
        if let Some(a) = self.avalanche.as_mut() {
            a.queue_images(uploads, images);
        }
    }

    pub fn queue_post_images(
        &mut self,
        uploads: &mut crate::render::uploads::LevelUploads,
        images: &mut Assets<Image>,
    ) {
        if let Some(Ok(o)) = self.overlay.as_mut() {
            o.queue_images(uploads, images);
        }
        if let Some(Ok(t)) = self.topogrid.as_mut() {
            t.queue_images(uploads, images);
        }
    }

    pub fn ride(&self) -> &Ride {
        &self.ride
    }

    pub fn ride_mut(&mut self) -> &mut Ride {
        &mut self.ride
    }

    pub fn avalanche(&self) -> Option<&crate::render::avalanche::AvalancheScene> {
        self.avalanche.as_ref()
    }

    pub fn rider_effect_inputs(
        &self,
    ) -> Option<(&str, &ssx_assets::character::Loadout, &ssx_formats::crsf::SceneGraph)> {
        self.body.as_ref().map(|b| (b.summary.as_str(), &self.ride.loadout, b.rig()))
    }

    pub fn heli_intro(&self) -> Option<std::sync::Arc<ssx_physics::nis::IntroAssets>> {
        self.ride.intro.clone().or_else(|| self.ride.heli.as_ref().map(|h| h.assets.clone()))
    }
}

// PORT(choice): only the asset viewer rides without a launched `ui_drop` (its `--ride`, its fly camera's ghosts); it
// takes the drop's event `SSX_RIDE_EVENT` names, default `race`. The console always launches one (the FE meta
// manager's +0xFB8, which 0x8239A550 hands `GameMode_StartDrop` 0x82483248), as the game shell does
// (`RideSetup::event_key`).
pub fn viewer_event_choice() -> String {
    std::env::var("SSX_RIDE_EVENT").unwrap_or_else(|_| "race".into())
}

pub fn prepare(
    mut setup: RideSetup,
    big_root: &std::path::Path,
    lighting: Option<&crate::render::lighting::GameLighting>,
) -> anyhow::Result<RideLoad> {
    use anyhow::Context;
    anyhow::ensure!(
        setup.live.is_none() || setup.screenshot_steps.is_none(),
        "a network session refuses the screenshot clock (one step per drawn frame): its steps follow the wall clock"
    );
    let board = setup.board.take();
    let mut body = setup.rider.take();
    let (load_secs, spawn, fly) = (setup.load_secs, setup.spawn, setup.fly_camera);
    let mut ride = Ride::new(setup).context("no ground within 100 m below the spawn point")?;
    let loadout = &ride.loadout;
    if let Some(b) = body.as_mut().filter(|_| *loadout != ssx_assets::character::Loadout::default()) {
        match b.dress(loadout) {
            Ok(()) => info!("Ride: dressed in {:?} {:?}", loadout.items, loadout.descriptions),
            Err(e) => warn!("Ride: the loadout's gear meshes failed ({e:#}); default outfit"),
        }
    }
    // PORT(choice): the rider's graph builds the intro's graphs from the bundles read ahead (`start::read_ahead`) on
    // the level's loading thread, not in the ride's first step (or a restart's) on the game thread, where its build
    // took 36 ms of alps0101's first ride frame (58 ms) and 53 ms of Mount Eddie's (105 ms; development laptop,
    // `trace_chrome`). The graph is the same one (built from the same bundles, NIS, rig, bind pose and float DOFs); the
    // console's loader threads are not decoded.
    if let Some(b) = body.as_mut() {
        for intro in [ride.intro.as_ref(), ride.intro_short.as_ref()].into_iter().flatten() {
            if let Err(e) = b.build_intro_ahead(intro) {
                debug!("intro: its graph not built ahead ({e:#}): the step that binds it builds it");
            }
        }
    }
    let s = &ride.world.survival;
    let drop = match (s.cold, s.low_oxygen, s.avalanche.avalanche_drop) {
        (true, _, _) => OverlayDrop::Cold,
        (false, true, _) => OverlayDrop::LowOxygen,
        (false, false, true) => OverlayDrop::Avalanche,
        _ => OverlayDrop::Default,
    };
    let mut presets = match ride.world.survival.objectives.as_deref() {
        Some(d) => {
            let event = ride.session.event.setup.objectives.as_ref().map(|e| &e.scripts);
            ssx_physics::controllers::postfx_presets(d, event)
        }
        None => Vec::new(),
    };
    let drop_postfx = lighting.and_then(|l| l.postfx.clone());
    if let Some(name) = drop_postfx.as_ref().filter(|n| !presets.contains(n)) {
        presets.push(name.clone());
    }
    let goggles = ride.world.survival.goggles_preset.as_ref().map(|(_, state)| state);
    let (layers, topogrid) = match ssx_assets::postfx::LevelPresets::load(big_root, &presets) {
        Ok(p) => {
            let layers = crate::render::post::layers::load(&p);
            ride.session.postfx = ssx_assets::postfx_state::PostFxService::new(layers.scene().layer_table());
            if let Some(name) = drop_postfx.as_deref()
                && ride.session.postfx.request(name, 0.0, 1.0).is_none()
            {
                warn!("post-FX: the drop's preset {name} names no layer (or did not load): the frame shows without it");
            }
            let topogrid = crate::render::post::topogrid::load(big_root, layers.scene(), goggles);
            (Ok(layers), topogrid)
        }
        Err(e) => (Err(anyhow::anyhow!("{e:#}")), Err(e)),
    };
    let overlay = crate::render::post::overlay::load(big_root, drop);
    let (layers, overlay, topogrid) = (Some(layers), Some(overlay), Some(topogrid));
    let avalanche = lighting.and_then(|l| crate::render::avalanche::load(big_root, l.disc_track()?, &ride));
    let noise = crate::camera::chase::load_noise(big_root);
    let ripple = ssx_physics::controllers::ripple::RippleTuning::load(big_root)
        .map_err(|e| warn!("landing ripple unavailable: {e:#}"))
        .ok();
    let visual = match &board {
        Some(_) => "board: common_board_geo (player_board.fx drawn with the object_default pass)",
        None => "board: placeholder box (board mesh unavailable)",
    };
    let body_label = match &body {
        Some(_) => "body: skinned character, player_body/hair/eye/apparel listing port (rim + fill, dynamicAOMult)",
        None => "body: placeholder capsule (character unavailable)",
    };
    let label = format!("{visual} | {body_label}");
    Ok(RideLoad {
        ride,
        board,
        body,
        fly,
        load_secs,
        spawn,
        layers,
        overlay,
        topogrid,
        avalanche,
        noise,
        ripple,
        label,
    })
}

pub fn insert(world: &mut World, r: RideLoad) -> anyhow::Result<()> {
    match r.layers {
        Some(Ok(l)) => {
            let (line, warning) = crate::render::post::layers::insert(world, l);
            info!("{line}");
            if let Some(w) = warning {
                warn!("{w}");
            }
        }
        Some(Err(e)) => warn!("post-FX layers unavailable: {e:#}"),
        None => {}
    }
    match r.overlay {
        Some(Ok(o)) => match crate::render::post::overlay::insert(world, o) {
            Ok(line) => info!("{line}"),
            Err(e) => warn!("screen effects unavailable: {e:#}"),
        },
        Some(Err(e)) => warn!("screen effects unavailable: {e:#}"),
        None => {}
    }
    match r.topogrid {
        Some(Ok(t)) => match crate::render::post::topogrid::insert(world, t) {
            Ok(line) => info!("{line}"),
            Err(e) => warn!("TOPOGRID unavailable: {e:#}"),
        },
        Some(Err(e)) => warn!("TOPOGRID unavailable: {e:#}"),
        None => {}
    }
    if let Some(scene) = r.avalanche {
        info!("{}", scene.summary());
        world.insert_resource(scene);
    }
    if let Some(noise) = r.noise {
        world.insert_resource(noise);
    }
    world.insert_resource(GearHud::default());
    world.insert_resource(TerrainRipple { tuning: r.ripple, ..TerrainRipple::default() });
    if let Some(s) = strip::GrindStrip::from_env() {
        world.insert_resource(s);
    }
    let (load_secs, spawn) = (r.load_secs, r.spawn);
    info!("Ride: physics world built in {load_secs:.2}s; spawn {:.1} heading {:.2}", spawn.0, spawn.1);
    world.insert_resource(r.ride);
    world.insert_resource(RiderLabel(r.label));
    world.insert_resource(if r.fly { CameraMode::Fly } else { CameraMode::Chase });
    rider_visual::insert(world, r.board, r.body);
    Ok(())
}

pub fn register(app: &mut App, hud: bool) {
    let riding = resource_exists::<Ride>;
    crate::shell::level::scoped::<key_player::TestPad>(app);
    crate::shell::level::scoped::<tap_script::TapScript>(app);
    rumble::register(app);
    app.add_observer(controls::pad_axes_as_read);
    app.init_resource::<PausedFrames>()
        .add_systems(FixedUpdate, fixed_step.in_set(RideStepSet).run_if(riding))
        .add_systems(FixedUpdate, tap_script::steps.after(RideStepSet).run_if(riding))
        .add_systems(
            PreUpdate,
            (
                key_player::test_pad.run_if(riding),
                tap_script::inject.run_if(riding),
                key_player::test_pad_end.run_if(not(riding)).run_if(resource_exists::<key_player::TestPad>),
            )
                .before(bevy::input::InputSystems),
        )
        .add_systems(
            Update,
            (
                viewer_key_presses,
                frame_step.in_set(RideStepSet),
                place_rider,
                crate::render::avalanche::update,
                shade_rider,
                animate_rider,
                light_gear,
                terrain_ripple,
                screen_effects,
                crate::camera::chase::toggle,
                crate::camera::chase::follow,
                report,
            )
                .chain()
                .run_if(riding),
        )
        .add_systems(RunFixedMainLoop, latch_frame.in_set(RunFixedMainLoopSystems::BeforeFixedMainLoop).run_if(riding))
        .add_systems(PostUpdate, latch_frame_end.run_if(riding))
        .add_systems(Update, draw_rail_debug.after(place_rider).run_if(riding))
        .add_systems(Update, strip::capture.after(report).run_if(riding))
        .add_systems(Update, world_render.after(place_rider))
        .add_systems(crate::shell::level::LevelStartup, spawn_outro_fade.run_if(riding))
        .add_systems(Update, outro_fade.after(place_rider).run_if(riding));
    crate::camera::chase::register(app);
    crate::render::avalanche::register(app);
    rider_visual::register(app);
    crate::shell::level::scoped::<Ride>(app);
    crate::shell::level::scoped::<GearHud>(app);
    crate::shell::level::scoped::<TerrainRipple>(app);
    crate::shell::level::scoped::<RiderLabel>(app);
    crate::shell::level::scoped::<CameraMode>(app);
    crate::shell::level::scoped::<strip::GrindStrip>(app);
    app.add_systems(crate::shell::level::LevelPostStartup, hide_viewer_hud.run_if(riding))
        .add_systems(Update, toggle_viewer_hud.run_if(riding));
    if hud {
        app.add_systems(crate::shell::level::LevelStartup, spawn_hud.run_if(riding)).add_systems(
            Update,
            (update_hud.after(animate_rider), update_trick_hud.after(animate_rider)).run_if(riding),
        );
    }
}

#[derive(Clone, Debug, Default)]
pub(crate) struct AudioTagLog {
    entries: std::collections::VecDeque<(u64, ssx_assets::anim_eval::AudioTagEvent)>,
}

impl AudioTagLog {
    pub const KEPT_STEPS: u64 = 30;

    pub fn record(&mut self, step: u64, tags: &[ssx_assets::anim_eval::AudioTagEvent]) {
        self.entries.extend(tags.iter().map(|t| (step, t.clone())));
        while self.entries.front().is_some_and(|(s, _)| s + Self::KEPT_STEPS < step) {
            self.entries.pop_front();
        }
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }

    pub fn after(&self, step: u64) -> impl Iterator<Item = &ssx_assets::anim_eval::AudioTagEvent> {
        self.entries.iter().filter(move |(s, _)| *s > step).map(|(_, t)| t)
    }
}

impl Ride {
    pub fn audio_tags_after(&self, step: u64) -> impl Iterator<Item = &ssx_assets::anim_eval::AudioTagEvent> {
        self.audio_tag_log.after(step)
    }
}

#[cfg(test)]
mod audio_tag_tests {
    use super::AudioTagLog;
    use ssx_assets::anim_eval::AudioTagEvent;

    fn tag(name: &str) -> AudioTagEvent {
        AudioTagEvent { name: name.into(), system: 4139, event: 0 }
    }

    fn names<'a>(it: impl Iterator<Item = &'a AudioTagEvent>) -> Vec<&'a str> {
        it.map(|t| t.name.as_str()).collect()
    }

    #[test]
    fn every_executions_tags_are_taken_once_in_order() {
        let mut log = AudioTagLog::default();
        log.record(1, &[tag("F_Board_Grab")]);
        log.record(2, &[tag("BootBinding_In")]);
        log.record(3, &[tag("F_Board_Release"), tag("Spin_Swish")]);
        assert_eq!(names(log.after(1)), ["BootBinding_In", "F_Board_Release", "Spin_Swish"]);
        assert!(log.after(3).next().is_none());
    }

    #[test]
    fn old_executions_are_dropped() {
        let mut log = AudioTagLog::default();
        log.record(1, &[tag("old")]);
        log.record(1 + AudioTagLog::KEPT_STEPS, &[tag("kept")]);
        assert_eq!(names(log.after(0)), ["old", "kept"]);
        log.record(2 + AudioTagLog::KEPT_STEPS, &[]);
        assert_eq!(names(log.after(0)), ["kept"]);
        log.clear();
        assert!(log.after(0).next().is_none());
    }
}
