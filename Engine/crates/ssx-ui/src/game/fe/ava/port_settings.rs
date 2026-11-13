use std::path::Path;

use anyhow::{Context, Result};
use serde_json::{Map, Value as Json};
use ssx_assets::display::{DisplaySettings, WindowMode};
use ssx_assets::port_options::{BootMovie, MenuFocus, RangeMovies};

use crate::game::loc::Localization;

pub const MAX_FILE: u64 = ssx_assets::settings_file::MAX_FILE;

// PORT(choice): "Player", the port's offline gamertag (`USER_gamertag`); the player sets a name in Options, Player
// Name; the console's gamertag comes from Xbox Live.
pub const DEFAULT_PLAYER_NAME: &str = "Player";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Shading {
    #[default]
    Auto,
    Textured,
    Height,
    Material,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Music {
    #[default]
    Auto,
    DeadlyDescent,
    Licensed,
    Off,
}

const SHADINGS: [(Shading, &str, &str); 4] = [
    (Shading::Auto, "Auto", "TXT_AVA_OPT_SHADING_AUTO"),
    (Shading::Textured, "Textured", "TXT_AVA_OPT_SHADING_TEXTURED"),
    (Shading::Height, "Height", "TXT_AVA_OPT_SHADING_HEIGHT"),
    (Shading::Material, "Material", "TXT_AVA_OPT_SHADING_MATERIAL"),
];
const RANGE_MOVIES: [(RangeMovies, &str); 2] = [
    (RangeMovies::EveryTime, "TXT_AVA_OPT_RANGE_MOVIES_EVERY_TIME"),
    (RangeMovies::FirstTimeOnly, "TXT_AVA_OPT_RANGE_MOVIES_FIRST_TIME_ONLY"),
];
const BOOT_MOVIES: [(BootMovie, &str); 2] =
    [(BootMovie::EveryStart, "TXT_AVA_OPT_BOOT_MOVIE_EVERY_START"), (BootMovie::Skip, "TXT_AVA_OPT_BOOT_MOVIE_SKIP")];
const MENU_FOCUS: [(MenuFocus, &str); 2] = [
    (MenuFocus::FirstItem, "TXT_AVA_OPT_MENU_FOCUS_FIRST_ITEM"),
    (MenuFocus::LastItem, "TXT_AVA_OPT_MENU_FOCUS_LAST_ITEM"),
];
const MUSICS: [(Music, &str, &str); 4] = [
    (Music::Auto, "Auto", "TXT_AVA_OPT_MUSIC_AUTO"),
    (Music::DeadlyDescent, "DeadlyDescent", "TXT_AVA_OPT_MUSIC_DEADLY_DESCENT"),
    (Music::Licensed, "Licensed", "TXT_AVA_OPT_MUSIC_LICENSED"),
    (Music::Off, "Off", "TXT_OFF"),
];

#[derive(Clone, Debug, PartialEq)]
pub struct PortSettings {
    pub subdivide: u32,
    pub shading: Shading,
    pub vsync: bool,
    pub objects: bool,
    pub wireframe: bool,
    pub particles: bool,
    pub snow_tracks: bool,
    pub local_lights: bool,
    pub game_post: bool,
    pub game_sky: bool,
    pub music: Music,
    pub music_volume: f32,
    pub sfx: bool,
    pub sfx_volume: f32,
    pub range_movies: RangeMovies,
    pub boot_movie: BootMovie,
    pub menu_focus: MenuFocus,
    #[cfg(feature = "net")]
    pub player_name: Option<String>,
    #[cfg(feature = "net")]
    pub resources: super::resource_settings::ResourceSettings,
    pub window: Option<WindowChoice>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct WindowChoice {
    pub settings: DisplaySettings,
    pub exclusive: bool,
}

impl Default for PortSettings {
    fn default() -> Self {
        PortSettings {
            subdivide: ssx_assets::subdiv::GAME_LEVELS as u32,
            shading: Shading::Auto,
            vsync: true,
            objects: true,
            wireframe: false,
            particles: true,
            snow_tracks: true,
            local_lights: true,
            game_post: true,
            game_sky: true,
            music: Music::Auto,
            music_volume: 1.0,
            sfx: true,
            sfx_volume: 1.0,
            range_movies: RangeMovies::EveryTime,
            boot_movie: BootMovie::EveryStart,
            menu_focus: MenuFocus::FirstItem,
            #[cfg(feature = "net")]
            player_name: None,
            #[cfg(feature = "net")]
            resources: Default::default(),
            window: None,
        }
    }
}

pub const SUBDIVIDE_MAX: u32 = 3;

impl PortSettings {
    pub fn from_json(v: &Json) -> (PortSettings, Vec<String>) {
        let mut s = PortSettings::default();
        let mut problems = Vec::new();
        let Some(m) = v.as_object() else {
            return (s, vec!["settings.json is not an object".into()]);
        };
        let mut bad = |k: &str| problems.push(format!("settings.json: `{k}` has a value the launcher cannot read"));
        if let Some(x) = m.get("subdivide") {
            match x.as_u64().filter(|&n| n <= u64::from(u32::MAX)) {
                Some(n) => s.subdivide = n as u32,
                None => bad("subdivide"),
            }
        }
        if let Some(x) = m.get("shading") {
            match SHADINGS.iter().find(|(_, n, _)| x.as_str() == Some(n)) {
                Some(&(v, _, _)) => s.shading = v,
                None => bad("shading"),
            }
        }
        if let Some(x) = m.get("music") {
            match MUSICS.iter().find(|(_, n, _)| x.as_str() == Some(n)) {
                Some(&(v, _, _)) => s.music = v,
                None => bad("music"),
            }
        }
        if let Some(x) = m.get(RangeMovies::MEMBER) {
            match x.as_str().and_then(RangeMovies::from_name) {
                Some(v) => s.range_movies = v,
                None => bad(RangeMovies::MEMBER),
            }
        }
        if let Some(x) = m.get(BootMovie::MEMBER) {
            match x.as_str().and_then(BootMovie::from_name) {
                Some(v) => s.boot_movie = v,
                None => bad(BootMovie::MEMBER),
            }
        }
        if let Some(x) = m.get(MenuFocus::MEMBER) {
            match x.as_str().and_then(MenuFocus::from_name) {
                Some(v) => s.menu_focus = v,
                None => bad(MenuFocus::MEMBER),
            }
        }
        for (k, field) in [
            ("vsync", &mut s.vsync),
            ("objects", &mut s.objects),
            ("wireframe", &mut s.wireframe),
            ("particles", &mut s.particles),
            ("snow_tracks", &mut s.snow_tracks),
            ("local_lights", &mut s.local_lights),
            ("game_post", &mut s.game_post),
            ("game_sky", &mut s.game_sky),
            ("sfx", &mut s.sfx),
        ] {
            if let Some(x) = m.get(k) {
                match x.as_bool() {
                    Some(b) => *field = b,
                    None => bad(k),
                }
            }
        }
        for (k, field) in [("music_volume", &mut s.music_volume), ("sfx_volume", &mut s.sfx_volume)] {
            if let Some(x) = m.get(k) {
                match x.as_f64().filter(|f| f.is_finite()) {
                    Some(f) => *field = f as f32,
                    None => bad(k),
                }
            }
        }
        #[cfg(feature = "net")]
        if let Some(x) = m.get("player_name") {
            match x.as_str().map(ssx_net_proto::name::player_name) {
                Some(Ok(name)) if Some(name.as_str()) == x.as_str() => s.player_name = Some(name),
                _ => problems.push("settings.json: `player_name` is not a name the servers accept".into()),
            }
        }
        #[cfg(feature = "net")]
        {
            s.resources = super::resource_settings::ResourceSettings::from_json(m.get("resources"), &mut problems);
        }
        (s, problems)
    }

    pub fn write_json(&self, m: &mut Map<String, Json>) {
        let shading = SHADINGS.iter().find(|x| x.0 == self.shading).map_or("Auto", |x| x.1);
        let music = MUSICS.iter().find(|x| x.0 == self.music).map_or("Auto", |x| x.1);
        m.insert("subdivide".into(), Json::from(self.subdivide));
        m.insert("shading".into(), Json::String(shading.into()));
        m.insert("music".into(), Json::String(music.into()));
        m.insert(RangeMovies::MEMBER.into(), Json::String(self.range_movies.name().into()));
        m.insert(BootMovie::MEMBER.into(), Json::String(self.boot_movie.name().into()));
        m.insert(MenuFocus::MEMBER.into(), Json::String(self.menu_focus.name().into()));
        for (k, b) in [
            ("vsync", self.vsync),
            ("objects", self.objects),
            ("wireframe", self.wireframe),
            ("particles", self.particles),
            ("snow_tracks", self.snow_tracks),
            ("local_lights", self.local_lights),
            ("game_post", self.game_post),
            ("game_sky", self.game_sky),
            ("sfx", self.sfx),
        ] {
            m.insert(k.into(), Json::Bool(b));
        }
        for (k, f) in [("music_volume", self.music_volume), ("sfx_volume", self.sfx_volume)] {
            let v = f.to_string().parse::<f64>().unwrap_or(f64::from(f));
            m.insert(k.into(), Json::from(v));
        }
        #[cfg(feature = "net")]
        if let Some(name) = &self.player_name {
            m.insert("player_name".into(), Json::String(name.clone()));
        }
        #[cfg(feature = "net")]
        m.insert("resources".into(), self.resources.to_json());
    }
}

#[cfg(feature = "net")]
pub fn old_masters_line(m: &Map<String, Json>) -> Option<String> {
    m.contains_key("masters").then(|| {
        format!(
            "settings.json: `masters` is not read any more: the master server is fixed ({})",
            ssx_core::OFFICIAL_MASTER
        )
    })
}

#[derive(Debug)]
pub enum SaveError {
    Unreadable(String),
    Io(anyhow::Error),
}

pub fn read_file(path: &Path) -> Result<Map<String, Json>> {
    ssx_assets::settings_file::read_object(path)
}

pub fn save_file(path: &Path, kept: &PortSettings, s: &PortSettings) -> Result<(), SaveError> {
    use ssx_assets::settings_file::{UpdateError, merge_changes, update_object};
    let (mut base, mut ours) = (Map::new(), Map::new());
    kept.write_json(&mut base);
    s.write_json(&mut ours);
    match update_object(path, |m| {
        merge_changes(m, &base, &ours, 2);
        Ok(())
    }) {
        Ok(_) => Ok(()),
        Err(UpdateError::Unreadable(why)) => Err(SaveError::Unreadable(why)),
        Err(UpdateError::Failed(e)) => Err(SaveError::Io(e.context(format!("writing {}", path.display())))),
    }
}

pub fn replace_unreadable(path: &Path, s: &PortSettings) -> Result<std::path::PathBuf> {
    let aside = super::files::move_aside(path).with_context(|| format!("moving {} aside", path.display()))?;
    let mut m = Map::new();
    s.write_json(&mut m);
    ssx_assets::settings_file::update_object(path, |file| {
        file.extend(m);
        Ok(())
    })
    .map_err(|e| anyhow::anyhow!("writing {}: {e}", path.display()))?;
    Ok(aside)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Item {
    WindowMode,
    RenderResolution,
    Subdivide,
    Shading,
    Vsync,
    Objects,
    Wireframe,
    Particles,
    SnowTracks,
    LocalLights,
    Post,
    Sky,
    Music,
    MusicVolume,
    Sfx,
    SfxVolume,
    RangeMovies,
    BootMovie,
    MenuFocus,
    #[cfg(feature = "net")]
    DownloadLimit,
    #[cfg(feature = "net")]
    ResourceCache,
    #[cfg(feature = "net")]
    ServerScripts,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Page {
    Display,
    Effects,
    Sound,
    Movies,
    Menus,
    #[cfg(feature = "net")]
    Multiplayer,
}

impl Page {
    pub fn items(self) -> &'static [Item] {
        match self {
            Page::Display => &[
                Item::WindowMode,
                Item::RenderResolution,
                Item::Subdivide,
                Item::Shading,
                Item::Vsync,
                Item::Objects,
                Item::Wireframe,
            ],
            Page::Effects => &[Item::Particles, Item::SnowTracks, Item::LocalLights, Item::Post, Item::Sky],
            Page::Sound => &[Item::Music, Item::MusicVolume, Item::Sfx, Item::SfxVolume],
            Page::Movies => &[Item::RangeMovies, Item::BootMovie],
            Page::Menus => &[Item::MenuFocus],
            #[cfg(feature = "net")]
            Page::Multiplayer => &[Item::DownloadLimit, Item::ResourceCache, Item::ServerScripts],
        }
    }
}

#[cfg(feature = "net")]
const NET_ITEMS: [Item; 3] = [Item::DownloadLimit, Item::ResourceCache, Item::ServerScripts];
#[cfg(not(feature = "net"))]
const NET_ITEMS: [Item; 0] = [];

const ALL: [Item; 19] = [
    Item::WindowMode,
    Item::RenderResolution,
    Item::Subdivide,
    Item::Shading,
    Item::Vsync,
    Item::Objects,
    Item::Wireframe,
    Item::Particles,
    Item::SnowTracks,
    Item::LocalLights,
    Item::Post,
    Item::Sky,
    Item::Music,
    Item::MusicVolume,
    Item::Sfx,
    Item::SfxVolume,
    Item::RangeMovies,
    Item::BootMovie,
    Item::MenuFocus,
];

const RENDER_HEIGHTS: [Option<u32>; 7] = [None, Some(2160), Some(1440), Some(1080), Some(900), Some(720), Some(540)];

// PORT(choice): the launcher's volumes (0 … 1 in hundredths on its slider) are shown and stepped in tenths, as the
// game's own speech and effect volumes step 0 … 10 (`AUDIOSETTING_*`); a step from 0.37 goes to 0.3 or 0.4; the port's
// own setting, not the game's.
const VOLUME_STEPS: f32 = 10.0;

impl Item {
    pub fn name(self) -> &'static str {
        match self {
            Item::WindowMode => "ava_opt_window",
            Item::RenderResolution => "ava_opt_render_resolution",
            Item::Subdivide => "ava_opt_subdivide",
            Item::Shading => "ava_opt_shading",
            Item::Vsync => "ava_opt_vsync",
            Item::Objects => "ava_opt_objects",
            Item::Wireframe => "ava_opt_wireframe",
            Item::Particles => "ava_opt_particles",
            Item::SnowTracks => "ava_opt_snow_tracks",
            Item::LocalLights => "ava_opt_local_lights",
            Item::Post => "ava_opt_post",
            Item::Sky => "ava_opt_sky",
            Item::Music => "ava_opt_music",
            Item::MusicVolume => "ava_opt_music_volume",
            Item::Sfx => "ava_opt_sfx",
            Item::SfxVolume => "ava_opt_sfx_volume",
            Item::RangeMovies => "ava_opt_range_movies",
            Item::BootMovie => "ava_opt_boot_movie",
            Item::MenuFocus => "ava_opt_menu_focus",
            #[cfg(feature = "net")]
            Item::DownloadLimit => "ava_opt_download_limit",
            #[cfg(feature = "net")]
            Item::ResourceCache => "ava_opt_resource_cache",
            #[cfg(feature = "net")]
            Item::ServerScripts => "ava_opt_server_scripts",
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Item::WindowMode => "TXT_AVA_OPT_WINDOW",
            Item::RenderResolution => "TXT_AVA_OPT_RENDER_RESOLUTION",
            Item::Subdivide => "TXT_AVA_OPT_SUBDIVIDE",
            Item::Shading => "TXT_AVA_OPT_SHADING",
            Item::Vsync => "TXT_AVA_OPT_VSYNC",
            Item::Objects => "TXT_AVA_OPT_OBJECTS",
            Item::Wireframe => "TXT_AVA_OPT_WIREFRAME",
            Item::Particles => "TXT_AVA_OPT_PARTICLES",
            Item::SnowTracks => "TXT_AVA_OPT_SNOW_TRACKS",
            Item::LocalLights => "TXT_AVA_OPT_LOCAL_LIGHTS",
            Item::Post => "TXT_AVA_OPT_POST",
            Item::Sky => "TXT_AVA_OPT_SKY",
            Item::Music => "TXT_AVA_OPT_MUSIC",
            Item::MusicVolume => "TXT_AVA_OPT_MUSIC_VOLUME",
            Item::Sfx => "TXT_AVA_OPT_SFX",
            Item::SfxVolume => "TXT_AVA_OPT_SFX_VOLUME",
            Item::RangeMovies => "TXT_AVA_OPT_RANGE_MOVIES",
            Item::BootMovie => "TXT_AVA_OPT_BOOT_MOVIE",
            Item::MenuFocus => "TXT_AVA_OPT_MENU_FOCUS",
            #[cfg(feature = "net")]
            Item::DownloadLimit => "TXT_AVA_OPT_DOWNLOAD_LIMIT",
            #[cfg(feature = "net")]
            Item::ResourceCache => "TXT_AVA_OPT_RESOURCE_CACHE",
            #[cfg(feature = "net")]
            Item::ServerScripts => "TXT_AVA_OPT_SERVER_SCRIPTS",
        }
    }

    pub fn of_dp(dp: &str) -> Option<Item> {
        ALL.into_iter().chain(NET_ITEMS).find(|i| dp.strip_prefix("dp_") == Some(i.name()))
    }

    fn flag(self, s: &mut PortSettings) -> Option<&mut bool> {
        Some(match self {
            Item::Vsync => &mut s.vsync,
            Item::Objects => &mut s.objects,
            Item::Wireframe => &mut s.wireframe,
            Item::Particles => &mut s.particles,
            Item::SnowTracks => &mut s.snow_tracks,
            Item::LocalLights => &mut s.local_lights,
            Item::Post => &mut s.game_post,
            Item::Sky => &mut s.game_sky,
            Item::Sfx => &mut s.sfx,
            #[cfg(feature = "net")]
            Item::ServerScripts => &mut s.resources.server_scripts,
            _ => return None,
        })
    }

    pub fn reset(self, s: &mut PortSettings, defaults: &PortSettings) {
        match self {
            Item::WindowMode => {
                if let Some(w) = s.window.as_mut() {
                    w.settings = w.settings.with_mode(DisplaySettings::default().mode);
                }
            }
            Item::RenderResolution => {
                if let Some(w) = s.window.as_mut() {
                    w.settings.render_height = DisplaySettings::default().render_height;
                }
            }
            Item::Subdivide => s.subdivide = defaults.subdivide,
            Item::Shading => s.shading = defaults.shading,
            Item::Music => s.music = defaults.music,
            Item::MusicVolume => s.music_volume = defaults.music_volume,
            Item::SfxVolume => s.sfx_volume = defaults.sfx_volume,
            Item::RangeMovies => s.range_movies = defaults.range_movies,
            Item::BootMovie => s.boot_movie = defaults.boot_movie,
            Item::MenuFocus => s.menu_focus = defaults.menu_focus,
            #[cfg(feature = "net")]
            Item::DownloadLimit => s.resources.download_limit_mib = defaults.resources.download_limit_mib,
            #[cfg(feature = "net")]
            Item::ResourceCache => s.resources.cache_mib = defaults.resources.cache_mib,
            i => {
                let mut d = defaults.clone();
                if let (Some(to), Some(from)) = (i.flag(s), i.flag(&mut d)) {
                    *to = *from;
                }
            }
        }
    }

    pub fn enabled(self, s: &PortSettings) -> bool {
        match self {
            Item::WindowMode | Item::RenderResolution => s.window.is_some(),
            Item::MusicVolume => s.music != Music::Off,
            Item::SfxVolume => s.sfx,
            _ => true,
        }
    }

    pub fn label(self, s: &PortSettings, loc: &mut Localization) -> String {
        let mut on_off = |b: bool| loc.text(if b { "TXT_ON" } else { "TXT_OFF" });
        let mut s = s.clone();
        match self {
            Item::WindowMode => loc.text(match s.window.as_ref().map(|w| w.settings.mode) {
                None => "TXT_AVA_OPT_WINDOW_NONE",
                Some(WindowMode::Windowed) => "TXT_AVA_OPT_WINDOW_WINDOWED",
                Some(WindowMode::Borderless) => "TXT_AVA_OPT_WINDOW_BORDERLESS",
                Some(WindowMode::Exclusive) => "TXT_AVA_OPT_WINDOW_EXCLUSIVE",
            }),
            Item::RenderResolution => match s.window.as_ref().and_then(|w| w.settings.render_height) {
                None => loc.text("TXT_AVA_OPT_RENDER_NATIVE"),
                Some(h) => format!("{h}p"),
            },
            Item::Subdivide => s.subdivide.to_string(),
            Item::Shading => loc.text(SHADINGS.iter().find(|x| x.0 == s.shading).map_or("", |x| x.2)),
            Item::Music => loc.text(MUSICS.iter().find(|x| x.0 == s.music).map_or("", |x| x.2)),
            Item::MusicVolume => volume_step(s.music_volume).to_string(),
            Item::SfxVolume => volume_step(s.sfx_volume).to_string(),
            Item::RangeMovies => loc.text(RANGE_MOVIES.iter().find(|x| x.0 == s.range_movies).map_or("", |x| x.1)),
            Item::BootMovie => loc.text(BOOT_MOVIES.iter().find(|x| x.0 == s.boot_movie).map_or("", |x| x.1)),
            Item::MenuFocus => loc.text(MENU_FOCUS.iter().find(|x| x.0 == s.menu_focus).map_or("", |x| x.1)),
            #[cfg(feature = "net")]
            Item::DownloadLimit => super::resource_settings::size_label(s.resources.download_limit_mib, loc),
            #[cfg(feature = "net")]
            Item::ResourceCache => super::resource_settings::size_label(s.resources.cache_mib, loc),
            i => on_off(i.flag(&mut s).is_some_and(|b| *b)),
        }
    }

    pub fn step(self, s: &mut PortSettings, next: bool) -> bool {
        fn wrap<T: Copy + PartialEq>(list: &[T], cur: T, next: bool) -> T {
            let i = list.iter().position(|x| *x == cur).unwrap_or(0);
            let n = list.len();
            list[if next { (i + 1) % n } else { (i + n - 1) % n }]
        }
        match self {
            Item::WindowMode => {
                let Some(w) = s.window.as_mut() else { return false };
                let offered: Vec<WindowMode> =
                    WindowMode::ALL.into_iter().filter(|&m| m != WindowMode::Exclusive || w.exclusive).collect();
                let to = wrap(&offered, w.settings.mode, next);
                let moved = to != w.settings.mode;
                w.settings = w.settings.with_mode(to);
                moved
            }
            Item::RenderResolution => {
                let Some(w) = s.window.as_mut() else { return false };
                let to = wrap(&RENDER_HEIGHTS, w.settings.render_height, next);
                let moved = to != w.settings.render_height;
                w.settings.render_height = to;
                moved
            }
            Item::Subdivide => {
                let v = s.subdivide.min(SUBDIVIDE_MAX);
                let to = if next { (v + 1).min(SUBDIVIDE_MAX) } else { v.saturating_sub(1) };
                let moved = to != s.subdivide;
                s.subdivide = to;
                moved
            }
            Item::Shading => {
                s.shading = wrap(&SHADINGS.map(|x| x.0), s.shading, next);
                true
            }
            Item::Music => {
                s.music = wrap(&MUSICS.map(|x| x.0), s.music, next);
                true
            }
            Item::MusicVolume => step_volume(&mut s.music_volume, next),
            Item::SfxVolume => step_volume(&mut s.sfx_volume, next),
            Item::RangeMovies => {
                s.range_movies = wrap(&RangeMovies::ALL, s.range_movies, next);
                true
            }
            Item::BootMovie => {
                s.boot_movie = wrap(&BootMovie::ALL, s.boot_movie, next);
                true
            }
            Item::MenuFocus => {
                s.menu_focus = wrap(&MenuFocus::ALL, s.menu_focus, next);
                true
            }
            #[cfg(feature = "net")]
            Item::DownloadLimit => super::resource_settings::step(
                &mut s.resources.download_limit_mib,
                &super::resource_settings::DOWNLOAD_STEPS_MIB,
                next,
            ),
            #[cfg(feature = "net")]
            Item::ResourceCache => super::resource_settings::step(
                &mut s.resources.cache_mib,
                &super::resource_settings::CACHE_STEPS_MIB,
                next,
            ),
            i => match i.flag(s) {
                Some(b) => {
                    *b = !*b;
                    true
                }
                None => false,
            },
        }
    }
}

fn volume_step(v: f32) -> i32 {
    (v.clamp(0.0, 1.0) * VOLUME_STEPS).round() as i32
}

fn step_volume(v: &mut f32, next: bool) -> bool {
    let cur = volume_step(*v);
    let to = if next { (cur + 1).min(VOLUME_STEPS as i32) } else { (cur - 1).max(0) };
    let new = to as f32 / VOLUME_STEPS;
    let moved = new != *v;
    *v = new;
    moved
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_launchers_file_is_read_member_by_member_and_written_back_whole() {
        let (s, problems) = PortSettings::from_json(&serde_json::json!({
            "iso": "/x.iso", "subdivide": 1, "shading": "Height", "vsync": false, "music": "Off",
            "music_volume": 0.37, "sfx": "yes", "sfx_volume": 2.5, "particles": false, "unknown": [1],
            "range_movies": "FirstTimeOnly", "boot_movie": "Skip", "menu_focus": "LastItem"
        }));
        assert_eq!(
            (s.subdivide, s.shading, s.vsync, s.music, s.particles, s.range_movies, s.boot_movie),
            (1, Shading::Height, false, Music::Off, false, RangeMovies::FirstTimeOnly, BootMovie::Skip)
        );
        assert_eq!(s.menu_focus, MenuFocus::LastItem);
        assert!(s.sfx && problems.len() == 1 && problems[0].contains("`sfx`"));
        assert_eq!((volume_step(s.music_volume), volume_step(s.sfx_volume)), (4, 10));
        let (d, p) = PortSettings::from_json(&serde_json::json!({}));
        assert!(p.is_empty() && d == PortSettings::default());
        assert!(!PortSettings::from_json(&serde_json::json!([1])).1.is_empty());
        for bad in [
            serde_json::json!({"subdivide": -1}),
            serde_json::json!({"subdivide": 1e300}),
            serde_json::json!({"shading": "textured"}),
            serde_json::json!({"music_volume": "1"}),
            serde_json::json!({"range_movies": "first-time-only"}),
            serde_json::json!({"range_movies": true}),
            serde_json::json!({"boot_movie": "skip"}),
            serde_json::json!({"boot_movie": 1}),
            serde_json::json!({"menu_focus": "last-item"}),
            serde_json::json!({"menu_focus": 1}),
        ] {
            let (s, p) = PortSettings::from_json(&bad);
            assert!(s == PortSettings::default() && p.len() == 1, "{bad}");
        }
    }

    #[test]
    fn steps_wrap_choices_and_stop_numbers_at_their_ends() {
        let mut s = PortSettings::default();
        assert!(Item::Subdivide.step(&mut s, true) && s.subdivide == 3);
        assert!(!Item::Subdivide.step(&mut s, true) && s.subdivide == 3);
        assert!(Item::Shading.step(&mut s, false) && s.shading == Shading::Material);
        assert!(Item::Music.step(&mut s, false) && s.music == Music::Off && !Item::MusicVolume.enabled(&s));
        assert!(!Item::SfxVolume.step(&mut s, true) && s.sfx_volume == 1.0);
        s.sfx_volume = 0.37;
        assert!(Item::SfxVolume.step(&mut s, false) && s.sfx_volume == 0.3);
        assert!(Item::Vsync.step(&mut s, true) && !s.vsync);
        for p in [Page::Display, Page::Effects, Page::Sound, Page::Movies, Page::Menus] {
            for i in p.items() {
                assert_eq!(Item::of_dp(&format!("dp_{}", i.name())), Some(*i));
            }
        }
        let pages = [Page::Display, Page::Effects, Page::Sound, Page::Movies, Page::Menus];
        assert_eq!(ALL.len(), pages.iter().map(|p| p.items().len()).sum::<usize>());
        assert!(Item::RangeMovies.step(&mut s, true) && s.range_movies == RangeMovies::FirstTimeOnly);
        assert!(Item::RangeMovies.step(&mut s, true) && s.range_movies == RangeMovies::EveryTime);
        assert!(Item::RangeMovies.step(&mut s, false) && s.range_movies == RangeMovies::FirstTimeOnly);
        Item::RangeMovies.reset(&mut s, &PortSettings::default());
        assert_eq!(s.range_movies, RangeMovies::EveryTime);
        assert!(Item::BootMovie.step(&mut s, true) && s.boot_movie == BootMovie::Skip);
        assert!(Item::BootMovie.step(&mut s, false) && s.boot_movie == BootMovie::EveryStart);
        assert!(Item::BootMovie.step(&mut s, false) && s.boot_movie == BootMovie::Skip);
        Item::BootMovie.reset(&mut s, &PortSettings::default());
        assert_eq!(s.boot_movie, BootMovie::EveryStart);
        assert!(Item::MenuFocus.step(&mut s, true) && s.menu_focus == MenuFocus::LastItem);
        assert!(Item::MenuFocus.step(&mut s, false) && s.menu_focus == MenuFocus::FirstItem);
        assert!(Item::MenuFocus.step(&mut s, false) && s.menu_focus == MenuFocus::LastItem);
        Item::MenuFocus.reset(&mut s, &PortSettings::default());
        assert_eq!(s.menu_focus, MenuFocus::FirstItem);
        #[cfg(feature = "net")]
        {
            assert_eq!(&NET_ITEMS[..], Page::Multiplayer.items());
            for i in Page::Multiplayer.items() {
                assert_eq!(Item::of_dp(&format!("dp_{}", i.name())), Some(*i));
            }
            assert!(Item::DownloadLimit.step(&mut s, false) && s.resources.download_limit_mib == 1024);
            assert!(Item::ServerScripts.step(&mut s, true) && !s.resources.server_scripts);
        }
    }

    #[test]
    fn saving_keeps_the_launchers_other_members() {
        let dir = std::env::temp_dir().join(format!("ava-options-{}", std::process::id()));
        let path = dir.join("settings.json");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&path, r#"{"iso": "/games/ssx.iso", "track": "alps0101", "vsync": true}"#).unwrap();
        let kept = PortSettings::from_json(&Json::Object(read_file(&path).unwrap())).0;
        let s = PortSettings { vsync: false, music_volume: 0.4, ..kept.clone() };
        save_file(&path, &kept, &s).unwrap();
        let m = read_file(&path).unwrap();
        assert_eq!(m["iso"], "/games/ssx.iso");
        assert_eq!(m["track"], "alps0101");
        assert_eq!(m["vsync"], false);
        assert_eq!(m["music_volume"], 0.4);
        assert!(!m.contains_key("subdivide") && !m.contains_key("particles"));
        assert_eq!(PortSettings::from_json(&Json::Object(m)).0, s);
        assert!(!path.with_extension("json.partial").exists());
        assert!(read_file(&dir.join("missing.json")).unwrap().is_empty());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_save_keeps_what_the_launcher_changed_meanwhile() {
        let dir = std::env::temp_dir().join(format!("ava-options-merge-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");
        std::fs::write(&path, r#"{"vsync": true, "music_volume": 1.0, "resources": {"cache_mib": 1024}}"#).unwrap();
        let kept = PortSettings::from_json(&Json::Object(read_file(&path).unwrap())).0;
        std::fs::write(
            &path,
            r#"{"vsync": false, "music_volume": 1.0, "future": 7, "resources": {"cache_mib": 1024}}"#,
        )
        .unwrap();
        let s = PortSettings { music_volume: 0.5, ..kept.clone() };
        save_file(&path, &kept, &s).unwrap();
        let m = read_file(&path).unwrap();
        assert_eq!(
            (&m["vsync"], &m["music_volume"], &m["future"]),
            (&Json::Bool(false), &Json::from(0.5), &Json::from(7))
        );
        let w =
            PortSettings { window: Some(WindowChoice { settings: Default::default(), exclusive: true }), ..s.clone() };
        save_file(&path, &s, &w).unwrap();
        assert_eq!(read_file(&path).unwrap(), m);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_window_mode_steps_through_what_is_offered() {
        let mut s = PortSettings::default();
        assert!(!Item::WindowMode.enabled(&s) && !Item::WindowMode.step(&mut s, true));
        s.window = Some(WindowChoice { settings: Default::default(), exclusive: false });
        let mode = |s: &PortSettings| s.window.as_ref().map(|w| (w.settings.mode, w.settings.fullscreen));
        assert!(Item::WindowMode.step(&mut s, true));
        assert_eq!(mode(&s), Some((WindowMode::Borderless, WindowMode::Borderless)));
        assert!(Item::WindowMode.step(&mut s, true));
        assert_eq!(mode(&s), Some((WindowMode::Windowed, WindowMode::Borderless)));
        if let Some(w) = s.window.as_mut() {
            w.exclusive = true;
        }
        assert!(Item::WindowMode.step(&mut s, false));
        assert_eq!(mode(&s), Some((WindowMode::Exclusive, WindowMode::Exclusive)));
        Item::WindowMode.reset(&mut s, &PortSettings::default());
        assert_eq!(mode(&s), Some((WindowMode::Windowed, WindowMode::Exclusive)));
    }

    #[test]
    fn the_render_resolution_steps_through_the_presets() {
        let mut s = PortSettings::default();
        assert!(!Item::RenderResolution.enabled(&s) && !Item::RenderResolution.step(&mut s, true));
        s.window = Some(WindowChoice { settings: Default::default(), exclusive: false });
        let height = |s: &PortSettings| s.window.as_ref().and_then(|w| w.settings.render_height);
        assert_eq!(height(&s), None);
        assert!(Item::RenderResolution.step(&mut s, false));
        assert_eq!(height(&s), Some(540));
        assert!(Item::RenderResolution.step(&mut s, true));
        assert_eq!(height(&s), None);
        assert!(Item::RenderResolution.step(&mut s, true));
        assert_eq!(height(&s), Some(2160));
        if let Some(w) = s.window.as_mut() {
            w.settings.render_height = Some(1080);
        }
        Item::RenderResolution.reset(&mut s, &PortSettings::default());
        assert_eq!(height(&s), None);
    }

    #[test]
    fn an_unreadable_file_is_never_written_over_unasked() {
        let dir = std::env::temp_dir().join(format!("ava-options-broken-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");
        let s = PortSettings { vsync: false, ..Default::default() };
        for broken in [&b"[1, 2"[..], b"{\"iso\": \"/games/ssx.iso\",", b"[]", &vec![b' '; MAX_FILE as usize + 1]] {
            std::fs::write(&path, broken).unwrap();
            assert!(matches!(save_file(&path, &PortSettings::default(), &s), Err(SaveError::Unreadable(_))));
            assert_eq!(std::fs::read(&path).unwrap(), broken, "left as it was");
        }
        let aside = replace_unreadable(&path, &s).unwrap();
        assert_eq!(std::fs::read(&aside).unwrap().len(), MAX_FILE as usize + 1, "kept aside whole");
        let (back, problems) = PortSettings::from_json(&Json::Object(read_file(&path).unwrap()));
        assert!(problems.is_empty() && back == s);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(feature = "net")]
    #[test]
    fn the_player_name_is_read_by_the_rule_and_written_back() {
        let s = PortSettings { player_name: Some("Elise".into()), ..Default::default() };
        let mut m = Map::new();
        s.write_json(&mut m);
        let (back, problems) = PortSettings::from_json(&Json::Object(m));
        assert!(problems.is_empty() && back == s);
        let (n, p) = PortSettings::from_json(&serde_json::json!({"player_name": " Elise "}));
        assert!(n.player_name.is_none() && p.len() == 1);
    }

    #[cfg(not(feature = "net"))]
    #[test]
    fn without_the_network_the_player_name_is_kept_untouched() {
        let mut m = Map::new();
        m.insert("player_name".into(), Json::String(" not checked here ".into()));
        let (s, problems) = PortSettings::from_json(&Json::Object(m.clone()));
        assert!(problems.is_empty());
        s.write_json(&mut m);
        assert_eq!(m["player_name"], " not checked here ");
    }
}
