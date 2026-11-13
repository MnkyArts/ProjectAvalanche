use std::path::PathBuf;

use anyhow::{Context, Result};
use bevy::diagnostic::FrameTimeDiagnosticsPlugin;
use bevy::prelude::{App, AppExit, DefaultPlugins, PluginGroup, Startup, Vec2, Vec3, Window, WindowPlugin, default};
use bevy::window::PresentMode;
use clap::Parser;
use ssx_bevy::{RenderArgs, WINDOW_SIZE, level, lighting, music, screenshot, sfx};

#[derive(Parser, Debug)]
#[command(
    version,
    about = "View SSX (2012) track terrain and objects (a development tool; the game is avalanche-game)"
)]
struct Cli {
    /// Extracted track folder containing `*_mesh_subd.subd` files.
    /// Defaults to Game-Data/Extracted/big/data0/data/xenon/gameexplorer/alps0101,
    /// searched for upwards from the current directory, then alps0101 in the
    /// launcher's data folder (`ssx-assets setup` output).
    track_dir: Option<PathBuf>,

    /// Save a PNG of the window once the scene has rendered, then exit.
    #[arg(long, value_name = "OUT.png")]
    screenshot: Option<PathBuf>,

    /// Frames the fully drawn scene must have been on screen before the screenshot.
    #[arg(long, value_name = "N", default_value_t = 30)]
    frames: u32,

    /// Initial camera position (with --ride: start with the fly camera there; Tab: chase camera).
    #[arg(long, value_name = "X,Y,Z", value_parser = parse_vec3, allow_hyphen_values = true)]
    camera: Option<Vec3>,

    /// Initial camera target.
    #[arg(long, value_name = "X,Y,Z", value_parser = parse_vec3, allow_hyphen_values = true)]
    look_at: Option<Vec3>,

    #[command(flatten)]
    render: RenderArgs,

    /// Extracted archive root holding cache1/ (vaults) and cache3/ (textures).
    /// Defaults to the ancestor of TRACK_DIR that has them, or
    /// Game-Data/Extracted/big searched for upwards from the current directory.
    #[arg(long, value_name = "DIR")]
    big_root: Option<PathBuf>,

    /// Hide the text overlay (e.g. for close-up screenshots).
    #[arg(long)]
    no_hud: bool,

    /// Light the track like this drop (1-based, vault order); each drop picks a
    /// time-of-day lighting preset. Default: the first drop.
    #[arg(long, value_name = "N", conflicts_with = "lighting")]
    drop: Option<usize>,

    /// Light the track with the first drop using this preset (e.g. `day_sunny`, `dawn`).
    #[arg(long, value_name = "PRESET")]
    lighting: Option<String>,

    /// Ride the track: the rider simulated by the physics port (30 Hz), a chase
    /// camera (Tab: fly camera) and keyboard/gamepad controls (see the ride HUD).
    #[arg(long)]
    ride: bool,

    /// Ride start point (default: the physics port's spawn rule).
    #[arg(long, value_name = "X,Y,Z", value_parser = parse_vec3, allow_hyphen_values = true, requires = "ride")]
    spawn: Option<Vec3>,

    /// Ride start direction (default: steepest in-bounds descent, else the fall line).
    #[arg(long, value_name = "X,Y,Z", value_parser = parse_vec3, allow_hyphen_values = true, requires = "ride")]
    heading: Option<Vec3>,

    /// Fixed ride controls (stick x, brake, tuck), live input ignored — for
    /// reproducible screenshots. With --screenshot the ride advances one 30 Hz
    /// step per drawn frame and stops after --frames steps.
    #[arg(long, value_name = "STEER,BRAKE,TUCK", value_parser = parse_vec3, allow_hyphen_values = true, requires = "ride")]
    ride_input: Option<Vec3>,

    /// With --ride-input: release the jump button at these ride steps (the jump request is the
    /// release, as in the game): held for the steps before the first release (prewind), and for the
    /// 9 steps (0.3 s, as `ride_sim --jump-hold`) before each later one.
    #[arg(long, value_name = "N[,N...]", value_delimiter = ',', requires = "ride_input")]
    ride_jump: Vec<u32>,

    /// With --ride-input: the left stick in the air (x = spin, y = flip: > 0 front flip) and on the
    /// jump's release step, which the take-off reads (a prewound launch after --ride-jump).
    #[arg(long, value_name = "SPIN,FLIP", value_parser = parse_vec2, allow_hyphen_values = true, requires = "ride_input")]
    ride_air: Option<Vec2>,

    /// With --ride-input: hold the right stick in this direction in the air (a grab: up = Rocket,
    /// left = Melon, right = Indy, down = Truckdriver for a regular rider).
    #[arg(long, value_name = "DIR", value_enum, requires = "ride_input")]
    ride_grab: Option<GrabDir>,

    /// With --ride-input: hold R2 in the air (the tweak; with --ride-grab); with --ride-classic, X (the classic
    /// tweak, `BLN_Pad_X_Hold`).
    #[arg(long, requires = "ride_input")]
    ride_tweak: bool,

    /// With --ride-input and --ride-classic: hold this set of shoulder buttons in the air (a classic grab: `rb`,
    /// `lb+lt`, … — the classic tables' button sets).
    #[arg(long, value_name = "SET", value_parser = parse_shoulders, requires = "ride_classic")]
    ride_shoulders: Option<[bool; 4]>,

    /// With --ride-input: hold R2 fully on the ground (boost; needs a filled meter).
    #[arg(long, requires = "ride_input")]
    ride_boost: bool,

    /// With --ride: start with this boost meter (a test setup; the game's starts empty and fills from
    /// landed tricks).
    #[arg(long, value_name = "N", default_value_t = 0.0, requires = "ride")]
    ride_boost_meter: f32,

    /// With --ride: the classic controls (the shoulder trick tables, D-pad, X boost, Y grind, B rewind, A wingsuit).
    #[arg(long, requires = "ride")]
    ride_classic: bool,

    /// The rider drawn on the board (a `character` collection of the vault, e.g. `alex`, `zoe`).
    #[arg(long, value_name = "NAME", default_value = ssx_assets::character::DEFAULT_CHARACTER)]
    character: String,

    /// Mount the client mods the enabled list (`<app dir>/mods.toml`) names, as the game does.
    #[arg(long)]
    mods: bool,

    /// Mount the installed DLC packages below this folder (the game's `--dlc-root`): a DLC track's vaults, objects and
    /// materials are read through them.
    #[arg(long, value_name = "DIR")]
    dlc: Option<PathBuf>,
}

/// A right-stick grab direction (`--ride-grab`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
enum GrabDir {
    Up,
    Left,
    Right,
    Down,
}

impl GrabDir {
    fn stick(self) -> Vec2 {
        match self {
            GrabDir::Up => Vec2::Y,
            GrabDir::Left => Vec2::NEG_X,
            GrabDir::Right => Vec2::X,
            GrabDir::Down => Vec2::NEG_Y,
        }
    }
}

fn main() -> AppExit {
    if std::env::args_os().skip(1).any(|a| a == "--game") {
        ssx_core::eprintln!(
            "error: the game is its own program now: run avalanche-game (next to the launcher), not asset-viewer --game"
        );
        return AppExit::error();
    }
    match run(Cli::parse()) {
        Ok(exit) => exit,
        Err(e) => {
            ssx_core::eprintln!("error: {}", ssx_core::text::one_line(e.as_ref()));
            AppExit::error()
        }
    }
}

fn run(cli: Cli) -> Result<AppExit> {
    let track_dir = match cli.track_dir.clone() {
        Some(dir) => dir,
        None => ssx_bevy::find_default_track().context("no TRACK_DIR given")?,
    };
    let ride = cli.ride.then(|| level::RideOptions {
        spawn: cli.spawn,
        heading: cli.heading,
        input: cli.ride_input,
        jump_release: cli.ride_jump.clone(),
        air_input: cli.ride_air,
        grab: cli.ride_grab.map(GrabDir::stick),
        tweak: cli.ride_tweak,
        shoulders: cli.ride_shoulders,
        boost: cli.ride_boost,
        boost_meter: cli.ride_boost_meter,
        classic: cli.ride_classic,
        fly_camera: cli.camera.is_some(),
        screenshot_steps: cli.screenshot.as_ref().map(|_| cli.frames),
        ..default()
    });
    if cli.mods {
        let big = cli.big_root.clone().or_else(|| ssx_bevy::terrain_look::find_big_root(&track_dir));
        let big = big.context("--mods needs the extracted archive root (--big-root)")?;
        ssx_assets::content::data::tuning::set_bundle_reader(ssx_physics::tuning::read_check);
        let report = ssx_assets::content::GameContent::of(&big).mount_client_mods(&ssx_assets::setup::app_dir());
        report.lines().iter().for_each(|l| ssx_core::eprintln!("{l}"));
    }
    if let Some(root) = &cli.dlc {
        let big = cli.big_root.clone().or_else(|| ssx_bevy::terrain_look::find_big_root(&track_dir));
        let big = big.context("--dlc needs the extracted archive root (--big-root)")?;
        let report = ssx_assets::content::GameContent::of(&big).mount_dlc(root);
        report.lines().iter().for_each(|l| ssx_core::eprintln!("{l}"));
    }
    let spec = level::LevelSpec {
        track_dir,
        big_root: cli.big_root.clone(),
        subdivide: cli.render.subdivide.unwrap_or(ssx_assets::subdiv::GAME_LEVELS as u32),
        shading: cli.render.shading,
        objects: !cli.render.no_objects,
        drop: match (&cli.lighting, cli.drop) {
            (Some(name), _) => lighting::DropSelect::Preset(name.clone()),
            (None, n) => lighting::DropSelect::Index(n.unwrap_or(1).saturating_sub(1)),
        },
        drop_explicit: cli.drop.is_some() || cli.lighting.is_some(),
        camera: cli.camera,
        look_at: cli.look_at,
        aspect: WINDOW_SIZE.x as f32 / WINDOW_SIZE.y as f32,
        ride,
        character: cli.character.clone(),
        screenshot: cli.screenshot.is_some(),
        map: None,
    };
    let data = level::LevelData::load(&spec)?;
    let name = data.name.clone();
    let big_root = data.big_root.clone();
    let drop_key = data.lighting.as_ref().map(|l| l.drop_key);
    let max_boost = data.max_boost();

    let mut app = App::new();
    let window = Window {
        title: format!("SSX-Rust asset viewer — {}", ssx_core::text::shown(&name)),
        resolution: WINDOW_SIZE.into(),
        present_mode: if cli.render.no_vsync { PresentMode::AutoNoVsync } else { PresentMode::default() },
        ..default()
    };
    app.add_plugins((
        DefaultPlugins
            .set(WindowPlugin { primary_window: Some(window), ..default() })
            .set(level::render_plugin())
            .set(level::log_plugin())
            .disable::<bevy::audio::AudioPlugin>(),
        FrameTimeDiagnosticsPlugin::default(),
    ));
    level::register(&mut app, level::Registration { wireframe: cli.render.wireframe, viewer_hud: !cli.no_hud });
    level::begin(app.world_mut());
    data.insert(app.world_mut())?;
    app.add_systems(Startup, level::startup_system);
    music::insert(&mut app, big_root.as_deref(), &name, drop_key, cli.screenshot.is_some());
    sfx::insert(&mut app, big_root.as_deref(), &name, &cli.character, max_boost, cli.screenshot.is_some());
    if let Some(path) = cli.screenshot {
        app.add_plugins(screenshot::AutoScreenshotPlugin { path, frames: cli.frames });
    }
    Ok(app.run())
}

fn parse_shoulders(s: &str) -> Result<[bool; 4], String> {
    let mut held = [false; 4];
    for b in s.split('+') {
        let i = match b.trim() {
            "lb" => 0,
            "rb" => 1,
            "lt" => 2,
            "rt" => 3,
            other => return Err(format!("{other:?}: one of lb, rb, lt, rt")),
        };
        held[i] = true;
    }
    Ok(held)
}

fn parse_vec2(s: &str) -> Result<Vec2, String> {
    let parts = s
        .split(',')
        .map(|p| p.trim().parse::<f32>())
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("{s:?}: {e}"))?;
    match parts[..] {
        [x, y] => Ok(Vec2::new(x, y)),
        _ => Err(format!("expected x,y, got {s:?}")),
    }
}

fn parse_vec3(s: &str) -> Result<Vec3, String> {
    let parts = s
        .split(',')
        .map(|p| p.trim().parse::<f32>())
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("{s:?}: {e}"))?;
    match parts[..] {
        [x, y, z] => Ok(Vec3::new(x, y, z)),
        _ => Err(format!("expected x,y,z, got {s:?}")),
    }
}
