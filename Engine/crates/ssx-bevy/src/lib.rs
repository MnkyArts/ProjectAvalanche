mod audio;
mod camera;
mod frame_task;
mod load_pool;
#[cfg(feature = "net")]
mod net;
mod particles;
mod render;
mod ride;
mod rider;
#[cfg(feature = "scripts")]
mod script;
mod shell;
pub mod ui;

pub use audio::{music, sfx, sfx_input};
pub use camera::chase::replay as camera_replay;
pub use camera::schedules::CameraSchedulesPlugin;
pub use render::swap_chain::SwapChainPlugin;
pub use render::terrain::look as terrain_look;
pub use render::{hooks, lighting};
pub use rider::anim as rider_anim;
pub use shell::{benchmark, game_mode, level, screenshot};
#[cfg(feature = "net")]
pub use ssx_ui::game::fe::ava::browser;

use std::path::PathBuf;

use anyhow::{Context, Result};
use bevy::math::UVec2;

pub const DEFAULT_TRACK: &str = "Game-Data/Extracted/big/data0/data/xenon/gameexplorer/alps0101";
pub const DEFAULT_TRACK_IN_BIG: &str = "data0/data/xenon/gameexplorer/alps0101";

pub const WINDOW_SIZE: UVec2 = UVec2::new(ssx_assets::display::DEFAULT_WINDOW.0, ssx_assets::display::DEFAULT_WINDOW.1);

/// The options both programs build and draw a level with (the game: every drop it loads; the viewer: its track).
#[derive(clap::Args, Debug, Clone)]
pub struct RenderArgs {
    /// Refine every patch this many Catmull-Clark levels with the game's rules
    /// (0 = the stored cage); the game's CPU snow-depth displacement is applied.
    /// Default: 2, the game's refinement.
    #[arg(long, value_name = "LEVELS")]
    pub subdivide: Option<u32>,

    /// Surface shading. Default: textured when the track's vault materials and
    /// textures are found (Game-Data/Extracted/big), else height.
    #[arg(long, value_enum, alias = "color")]
    pub shading: Option<level::ShadingArg>,

    /// Don't load the objects placed on the track (trees, buildings, rails…).
    #[arg(long)]
    pub no_objects: bool,

    /// Start with the wireframe overlay on (toggle with F).
    #[arg(long)]
    pub wireframe: bool,

    /// Present without vsync, so the logged/HUD frame time is not capped at
    /// the display refresh rate.
    #[arg(long)]
    pub no_vsync: bool,

    /// The terrain's view distance in metres, 200 … 8000 (the original's 600 by default): the cut-off, the streaming,
    /// the far detail past 600 m and the proxy's push-back follow it. The launcher passes its setting.
    #[arg(long, value_name = "METRES", default_value_t = ssx_assets::port_options::ViewDistance::ORIGINAL)]
    pub view_distance: ssx_assets::port_options::ViewDistance,

    /// The sun's shadows: `original` (the default: the game's three 1024² cascades over 400 m), `medium` (the same
    /// maps over 250 m) or `low` (512² maps over 200 m). The launcher passes its setting.
    #[arg(long, value_name = "LEVEL", default_value_t = ssx_assets::port_options::ShadowQuality::Original)]
    pub shadow_quality: ssx_assets::port_options::ShadowQuality,

    /// Draw the scene without its bloom (the game draws it). The launcher passes its setting.
    #[arg(long)]
    pub no_bloom: bool,

    /// Draw the scene without its FXAA (the game draws it). The launcher passes its setting.
    #[arg(long)]
    pub no_fxaa: bool,
}

impl RenderArgs {
    pub fn post_levels(&self) -> render::post::PostLevels {
        render::post::PostLevels::of(!self.no_bloom, !self.no_fxaa)
    }
}

pub fn find_default_track() -> Result<PathBuf> {
    let cwd = std::env::current_dir().context("reading the current directory")?;
    let setup = ssx_assets::setup::big_root(&ssx_assets::setup::default_data_dir()).join(DEFAULT_TRACK_IN_BIG);
    cwd.ancestors().map(|dir| dir.join(DEFAULT_TRACK)).chain([setup.clone()]).find(|p| p.is_dir()).with_context(|| {
        format!(
            "neither {DEFAULT_TRACK} (in {} or any parent directory) nor {} exists; \
             set the game data up with the launcher (Project Avalanche) first",
            cwd.display(),
            setup.display()
        )
    })
}
