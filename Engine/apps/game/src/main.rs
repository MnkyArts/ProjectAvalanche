use std::path::PathBuf;

use anyhow::{Context, Result};
use bevy::prelude::AppExit;
use clap::Parser;
use ssx_assets::port_options::{BootMovie, MenuFocus, RangeMovies};
use ssx_bevy::{RenderArgs, game_mode, terrain_look};

/// The game: the front end from the boot movie to a drop and back in one window, the profile loaded and saved. The
/// data come from --big-root (or the default search). Keys: Enter START,
/// Space A, Backspace B, X, Y, arrows the D-pad, Q / E the bumpers, Z / C or Page Up / Page Down the triggers, W / A /
/// S / D the globe's stick (or a gamepad); riding: the ride's controls, Esc the pause menu.
#[derive(Parser, Debug)]
#[command(name = "avalanche-game", version)]
struct Cli {
    /// The asset viewer's former switch for the game (`asset-viewer --game`): accepted and ignored, so the command
    /// lines of before keep working.
    #[arg(long = "game", hide = true)]
    _game: bool,

    /// Extracted archive root holding cache1/ (vaults) and cache3/ (textures).
    /// Defaults to the one above Game-Data/Extracted/big/data0/data/xenon/gameexplorer/alps0101, searched for upwards
    /// from the current directory, then in the game data folder the launcher set up.
    #[arg(long, value_name = "DIR")]
    big_root: Option<PathBuf>,

    #[command(flatten)]
    render: RenderArgs,

    /// The profile file (default `<app dir>/Saves/ssxvprofile`, the launcher's folder).
    #[arg(long, value_name = "FILE")]
    save: Option<PathBuf>,

    /// The text language (`eng_us`, `fre_fr`, `ger_de`, `ita_it`, `spa_es`, `jpn_jp`).
    #[arg(long, value_name = "LANG", default_value = "eng_us")]
    lang: String,

    /// When the World Tour's range screen plays a range's movie: `every-time` (the game's) or `first-time-only` (at
    /// the range's first selection in the profile; the record is kept beside the save). The launcher passes its
    /// setting.
    #[arg(long, value_name = "WHEN", default_value_t = RangeMovies::EveryTime)]
    range_movies: RangeMovies,

    /// Whether the game starts with its boot movie: `every-start` (the game's) or `skip` (the start the game makes
    /// after a sign-out: no boot movie, the start screen with its intro). The launcher passes its setting.
    #[arg(long, value_name = "WHEN", default_value_t = BootMovie::EveryStart)]
    boot_movie: BootMovie,

    /// Where the game's own menus (the main menu, the pause menu, the settings menu) put the highlight when they open
    /// again: `first-item` (the game's) or `last-item` (the item last picked). The launcher passes its setting.
    #[arg(long, value_name = "WHERE", default_value_t = MenuFocus::FirstItem)]
    menu_focus: MenuFocus,

    /// A test option: the window in pixels at scale factor 1 (e.g. `1280x720`), windowed; the launcher's display
    /// settings neither read nor kept.
    #[arg(long, value_name = "WxH")]
    window: Option<String>,

    /// The window's mode, monitor and sizes (`--window-mode`, `--monitor`, `--video-mode`, `--window-size`,
    /// `--list-displays`).
    #[command(flatten)]
    display: game_mode::DisplayCli,

    /// A test option: pad presses `<seconds>:<step>,…` on the front end's clock (`start`, `a`, `b`, `x`, `y`, `up`,
    /// `down`, `left`, `right`, `lb`, `rb`, `skip`, `shot=NAME`, `nav=EVENT`); `|` starts the next segment, timed
    /// from the next return from a drop.
    #[arg(long, value_name = "SCRIPT", allow_hyphen_values = true)]
    script: Option<String>,

    /// Test options: a movie's seconds at most; the front end's seconds to run; the drops to play before the session
    /// ends; per drop a ride time limit in seconds (0 = none); screenshots at these ride seconds.
    #[arg(long, value_name = "S")]
    movie_cap: Option<f64>,
    #[arg(long, value_name = "S")]
    seconds: Option<f64>,
    #[arg(long, value_name = "N")]
    rides: Option<u32>,
    #[arg(long, value_name = "S[,S...]", value_delimiter = ',')]
    ride_limit: Vec<f32>,
    #[arg(long, value_name = "S[,S...]", value_delimiter = ',')]
    ride_shots: Vec<f32>,
    /// A test option: pad presses in each level, `;` between the levels (`<seconds>:<step>,…` as --script, seconds
    /// since the level came up; `start` is the pause).
    #[arg(long, value_name = "SCRIPTS", allow_hyphen_values = true)]
    level_script: Option<String>,

    /// Where `shot=NAME` and the ride screenshots go (default: the development tree's Game-Data/Converted/screenshots,
    /// else `<app dir>/Screenshots`).
    #[arg(long, value_name = "DIR")]
    shot_dir: Option<PathBuf>,

    /// A test option: launch this `ui_drop` directly (as the globe's tutorial branch does, without the equipment
    /// screen) at this front-end second, e.g. `exp_alps_grandgolliat_drop_1_race@12`.
    #[arg(long, value_name = "UI_DROP@S")]
    launch: Option<String>,

    /// A test option: ride this installed map (`<namespace>:map/<name>`) from the Project Avalanche menu at this
    /// front-end second (default 0: once the menu is idle).
    #[arg(long, value_name = "MAP_ID[@S]", conflicts_with_all = ["launch", "classic_front_end"])]
    launch_map: Option<String>,

    /// A test option: the mode `--launch-map` rides — a built-in mode (`race`, `trick`, `survive`, `death`,
    /// `freeride`) or an installed gamemode's id (`<namespace>:gamemode/<name>`); default the map's first mode. With
    /// `--launch`, only `freeride`: the drop's world without its event (the TU's mode).
    #[arg(long, value_name = "MODE")]
    launch_mode: Option<String>,

    /// A test option: the drops are ridden by the HUD tools' scripted player.
    #[arg(long)]
    autoplay: bool,

    /// A test option: the drops are played by a player that reads the screen's prompts (a tutorial popup, the
    /// tutorial HUD's buttons, the start message) and presses the keyboard keys they name.
    #[arg(long, conflicts_with = "autoplay")]
    follow_prompts: bool,

    /// A test option: a ride without a result after S ride seconds ends won — a result that beats every rival of the
    /// event (the window's counterpart of the example `fe_run --save IN --complete DROP`, for the front end's flow
    /// after a drop the test player cannot finish).
    #[arg(long, value_name = "S")]
    ride_win: Option<f32>,

    /// The asset viewer's debug text over the ride (off: the game's HUD only).
    #[arg(long)]
    debug_hud: bool,

    /// Start without the client mods the enabled list (`<app dir>/mods.toml`) names.
    #[arg(long)]
    no_mods: bool,

    /// Mod development: mount this package folder (in place of an installed package of its name) and reload it while
    /// the game runs — a changed texture is decoded and uploaded again; an overlay lists the mods, overrides and
    /// conflicts.
    #[arg(long, value_name = "DIR")]
    mod_dev: Option<PathBuf>,

    /// The installed DLC packages' folder (`<app dir>/DLC`): its enabled packages are mounted as the game's own base
    /// content above the disc (`dlc.toml`; the launcher's Downloadable Content section writes it).
    #[arg(long, value_name = "DIR")]
    dlc_root: Option<PathBuf>,

    /// The player's level-15 data: a level-15 save (`ssxvprofile`, the console file or its 0x57800 buffer), a tuner
    /// pack (the chunk-7 EB) or the launcher's folder holding `tuner.bin`. Without it the profile's own chunk 7 is used;
    /// without either the game keeps the disc's 10 levels.
    #[arg(long, value_name = "FILE|DIR")]
    tuner: Option<PathBuf>,

    /// The installed title updates' folder (`<app dir>/DLC/Title Updates`): one update (the newest enabled) is applied
    /// above the disc and the DLC packages (`tu.toml`; the launcher's Title Update section writes it).
    #[arg(long, value_name = "DIR")]
    tu_root: Option<PathBuf>,

    /// A test option: screenshots across the screen fades — every third frame while the fade is up (its alpha above
    /// 0) and the frame it ends, at most N (`fade_<FE seconds>_<alpha>.png` in the shot folder).
    #[arg(long, value_name = "N")]
    fade_shots: Option<u32>,

    /// The disc's front end alone, without the Project Avalanche menu (also `SSX_FE_HUB=off`).
    #[arg(long)]
    classic_front_end: bool,

    /// A test option: a screen of the Project Avalanche menu (keyboard, credits, options) opened once the menu is up.
    #[arg(long, value_name = "VIEW", conflicts_with = "classic_front_end")]
    open_view: Option<String>,

    /// A test option: a movie played by its id at the start, before the boot movie (`ssx:movie/<name>`, or a package's
    /// added movie `<package>:movie/<path>`).
    #[arg(long, value_name = "ID")]
    play_movie: Option<String>,

    /// Join this multiplayer server (`host:port`, an address, or a name; the port 27450 when none is given) and ride
    /// its drop with the others; a refusal or the session's end shows its reason on the Project Avalanche menu. Without
    /// --name and a stored Player Name the join waits for the name the menu asks for first.
    #[arg(long, value_name = "ADDR", conflicts_with_all = ["classic_front_end", "launch", "launch_map"])]
    connect: Option<String>,

    /// The file holding the server's password (one line).
    #[arg(long, value_name = "FILE", requires = "connect")]
    password_file: Option<PathBuf>,

    /// The name the server shows over your rider (the game's fonts' characters, at most 20; default: the Options'
    /// Player Name, which the menu asks for when there is none; "Player" if that is cancelled).
    #[arg(long, value_name = "NAME", requires = "connect")]
    name: Option<String>,

    /// An `avalanche://connect/host:port` link (the launcher passes one the system handed it): the server browser opens
    /// with the server and asks before anything is sent to it.
    #[arg(long, value_name = "URL", conflicts_with_all = ["classic_front_end", "connect", "launch", "launch_map"])]
    link: Option<String>,

    /// A test option: the server browser's tabs show N generated servers (also `SSX_FAKE_SERVERS=N`).
    #[arg(long, value_name = "N")]
    fake_servers: Option<usize>,
}

fn main() -> AppExit {
    let cli = Cli::parse();
    if cli.display.list_displays {
        return game_mode::print_displays();
    }
    match game_args(cli).and_then(game_mode::run) {
        Ok(exit) => exit,
        Err(e) => {
            ssx_core::eprintln!("error: {e:#}");
            AppExit::error()
        }
    }
}

fn game_args(cli: Cli) -> Result<game_mode::GameArgs> {
    let big_root = match cli.big_root {
        Some(r) => r,
        None => {
            let needs = "the game needs the extracted archive root (--big-root)";
            let track_dir = ssx_bevy::find_default_track().context(needs)?;
            terrain_look::find_big_root(&track_dir).context(needs)?
        }
    };
    let vsync = !cli.render.no_vsync;
    let settings = ssx_assets::setup::app_dir().join("settings.json");
    let window = game_mode::WindowSpec::new(cli.window.as_deref(), &cli.display, vsync, Some(&settings))?;
    let classic_front_end = cli.classic_front_end || std::env::var("SSX_FE_HUB").is_ok_and(|v| v == "off");
    if classic_front_end && cli.open_view.is_some() {
        anyhow::bail!("--open-view opens a screen of the Project Avalanche menu: not with SSX_FE_HUB=off");
    }
    let shot_dir = cli.shot_dir.unwrap_or_else(|| {
        let cwd = std::env::current_dir().unwrap_or_default();
        let found = cwd.ancestors().map(|d| d.join("Game-Data/Converted/screenshots")).find(|p| p.is_dir());
        found.unwrap_or_else(|| ssx_assets::setup::app_dir().join("Screenshots"))
    });
    Ok(game_mode::GameArgs {
        big_root,
        save: cli.save,
        lang: cli.lang,
        range_movies: cli.range_movies,
        boot_movie: cli.boot_movie,
        menu_focus: cli.menu_focus,
        window,
        script: cli.script.unwrap_or_default(),
        movie_cap: cli.movie_cap,
        seconds: cli.seconds,
        rides: cli.rides,
        ride_limits: cli.ride_limit,
        ride_shots: cli.ride_shots,
        level_scripts: cli
            .level_script
            .as_deref()
            .map_or_else(Vec::new, |s| s.split(';').map(str::to_string).collect()),
        shot_dir,
        autoplay: cli.autoplay,
        follow_prompts: cli.follow_prompts,
        ride_win: cli.ride_win,
        debug_hud: cli.debug_hud,
        fade_shots: cli.fade_shots.unwrap_or(0),
        subdivide: cli.render.subdivide,
        shading: cli.render.shading,
        no_objects: cli.render.no_objects,
        wireframe: cli.render.wireframe,
        mods: !cli.no_mods,
        classic_front_end,
        open_view: cli.open_view,
        play_movie: cli.play_movie,
        mod_dev: cli.mod_dev,
        dlc_root: cli.dlc_root,
        tuner: cli.tuner,
        tu_root: cli.tu_root,
        link: match &cli.link {
            Some(l) => Some(ssx_ui_link(l).with_context(|| format!("--link {l:?}"))?),
            None => None,
        },
        fake_servers: match cli.fake_servers {
            Some(n) => Some(n),
            None => match std::env::var("SSX_FAKE_SERVERS") {
                Ok(v) => Some(v.parse().with_context(|| format!("SSX_FAKE_SERVERS={v}"))?),
                Err(_) => None,
            },
        },
        connect: cli.connect.map(|server| game_mode::NetConnect {
            server,
            password_file: cli.password_file,
            name: cli.name,
        }),
        launch: match &cli.launch {
            Some(s) => {
                let (name, at) = s.rsplit_once('@').with_context(|| format!("--launch {s}: UI_DROP@SECONDS"))?;
                Some((name.to_string(), at.parse().with_context(|| format!("--launch {s}"))?))
            }
            None => None,
        },
        launch_map: match &cli.launch_map {
            Some(s) => Some(match s.rsplit_once('@') {
                Some((id, at)) => (id.to_string(), at.parse().with_context(|| format!("--launch-map {s}"))?),
                None => (s.clone(), 0.0),
            }),
            None => None,
        },
        launch_mode: match cli.launch_mode.clone() {
            Some(m) if cli.launch.is_none() && cli.launch_map.is_none() => {
                anyhow::bail!("--launch-mode {m}: needs --launch (freeride) or --launch-map")
            }
            Some(m) if cli.launch.is_some() && m != "freeride" => {
                anyhow::bail!("--launch-mode {m}: a drop's direct launch rides only freeride")
            }
            m => m,
        },
    })
}

fn ssx_ui_link(link: &str) -> Result<ssx_bevy::browser::ServerAddress> {
    ssx_bevy::browser::parse_link(link).map_err(anyhow::Error::msg)
}
