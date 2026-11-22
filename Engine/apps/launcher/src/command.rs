use std::ffi::OsString;
use std::path::{Path, PathBuf};

use ssx_assets::port_options::{
    BootMovie, DiscordPresence, FrameLimit, MenuFocus, RangeMovies, ResolutionScale, ShadowQuality, ViewDistance,
};

use crate::catalog;
use crate::settings::{Mode, Music, Settings, Shading};

fn next_to_launcher(name: &str) -> PathBuf {
    let name = if cfg!(windows) { format!("{name}.exe") } else { name.to_string() };
    std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.join(&name))).unwrap_or_else(|| PathBuf::from(name))
}

pub fn game_exe() -> PathBuf {
    next_to_launcher("avalanche-game")
}

pub fn viewer_exe() -> PathBuf {
    next_to_launcher("asset-viewer")
}

pub fn server_exe() -> PathBuf {
    next_to_launcher("avalanche-server")
}

pub const MANAGED_ENV: [&str; 9] = [
    "SSX_MUSIC",
    "SSX_MUSIC_VOLUME",
    "SSX_SFX",
    "SSX_SFX_VOLUME",
    "SSX_PARTICLES",
    "SSX_SNOW_TRACKS",
    "SSX_LOCAL_LIGHTS",
    "SSX_GAME_POST",
    "SSX_GAME_SKY",
];

#[derive(Clone, Debug, PartialEq)]
pub struct Invocation {
    pub program: PathBuf,
    pub args: Vec<OsString>,
    pub env: Vec<(String, String)>,
}

pub fn viewer(s: &Settings, big_root: &Path) -> Invocation {
    let mut args: Vec<OsString> =
        vec![catalog::track_dir(big_root, &s.track).into(), "--big-root".into(), big_root.into()];
    let mut push = |list: &[&str]| args.extend(list.iter().map(OsString::from));
    push(&["--subdivide", &s.subdivide.to_string()]);
    view_distance(s, &mut push);
    level_graphics(s, &mut push);
    match s.shading {
        Shading::Auto => {}
        Shading::Textured => push(&["--shading", "textured"]),
        Shading::Height => push(&["--shading", "height"]),
        Shading::Material => push(&["--shading", "material"]),
    }
    match (&s.lighting, s.drop) {
        (Some(preset), _) => push(&["--lighting", preset]),
        (None, 0) => {}
        (None, n) => push(&["--drop", &n.to_string()]),
    }
    for (on, name) in [(!s.hud, "--no-hud"), (!s.vsync, "--no-vsync"), (s.wireframe, "--wireframe")] {
        if on {
            push(&[name]);
        }
    }
    push(&["--character", &s.character]);
    if s.mode == Mode::Ride {
        push(&["--ride"]);
        if s.boost_meter > 0.0 {
            push(&["--ride-boost-meter", &s.boost_meter.to_string()]);
        }
    }
    Invocation { program: viewer_exe(), args, env: env(s) }
}

fn view_distance(s: &Settings, push: &mut impl FnMut(&[&str])) {
    if !s.view_distance.is_original() {
        push(&[ViewDistance::FLAG, &s.view_distance.to_string()]);
    }
}

fn level_graphics(s: &Settings, push: &mut impl FnMut(&[&str])) {
    if s.shadow_quality != ShadowQuality::Original {
        push(&[ShadowQuality::FLAG, s.shadow_quality.arg()]);
    }
    for (on, name) in [(!s.bloom, "--no-bloom"), (!s.fxaa, "--no-fxaa")] {
        if on {
            push(&[name]);
        }
    }
}

fn env(s: &Settings) -> Vec<(String, String)> {
    let mut env = Vec::new();
    let mut set = |k: &str, v: &str| env.push((k.to_string(), v.to_string()));
    match s.music {
        Music::Auto => {}
        Music::DeadlyDescent => set("SSX_MUSIC", "dd"),
        Music::Licensed => set("SSX_MUSIC", "licensed"),
        Music::Off => set("SSX_MUSIC", "off"),
    }
    if s.music != Music::Off && s.music_volume < 1.0 {
        set("SSX_MUSIC_VOLUME", &format!("{:.2}", s.music_volume));
    }
    if !s.sfx {
        set("SSX_SFX", "off");
    } else if s.sfx_volume < 1.0 {
        set("SSX_SFX_VOLUME", &format!("{:.2}", s.sfx_volume));
    }
    for (on, key) in [
        (s.particles, "SSX_PARTICLES"),
        (s.snow_tracks, "SSX_SNOW_TRACKS"),
        (s.local_lights, "SSX_LOCAL_LIGHTS"),
        (s.game_post, "SSX_GAME_POST"),
        (s.game_sky, "SSX_GAME_SKY"),
    ] {
        if !on {
            set(key, "off");
        }
    }
    env
}

pub fn save_path() -> PathBuf {
    save_path_in(&crate::settings::app_dir())
}

pub fn save_path_in(app_dir: &Path) -> PathBuf {
    app_dir.join("Saves").join("ssxvprofile")
}

pub fn game(s: &Settings, big_root: &Path) -> Invocation {
    let mut args: Vec<OsString> = vec!["--big-root".into(), big_root.into()];
    args.extend(["--save".into(), save_path().into()]);
    args.extend(["--dlc-root".into(), crate::settings::app_dir().join("DLC").into()]);
    args.extend(["--tuner".into(), crate::settings::app_dir().join("Tuner").into()]);
    args.extend(["--tu-root".into(), crate::settings::app_dir().join("DLC").join("Title Updates").into()]);
    if let Some(lang) =
        s.language.as_deref().filter(|l| crate::language::available(big_root).iter().any(|(c, _)| *c == *l))
    {
        args.extend(["--lang".into(), lang.into()]);
    }
    let mut push = |list: &[&str]| args.extend(list.iter().map(OsString::from));
    push(&["--subdivide", &s.subdivide.to_string()]);
    view_distance(s, &mut push);
    level_graphics(s, &mut push);
    match s.shading {
        Shading::Auto => {}
        Shading::Textured => push(&["--shading", "textured"]),
        Shading::Height => push(&["--shading", "height"]),
        Shading::Material => push(&["--shading", "material"]),
    }
    for (on, name) in [(!s.vsync, "--no-vsync"), (s.wireframe, "--wireframe")] {
        if on {
            push(&[name]);
        }
    }
    if s.frame_limit != FrameLimit::Off {
        push(&[FrameLimit::FLAG, s.frame_limit.arg()]);
    }
    if s.resolution_scale != ResolutionScale::Native {
        push(&[ResolutionScale::FLAG, s.resolution_scale.arg()]);
    }
    if s.range_movies != RangeMovies::EveryTime {
        push(&[RangeMovies::FLAG, s.range_movies.arg()]);
    }
    if s.boot_movie != BootMovie::EveryStart {
        push(&[BootMovie::FLAG, s.boot_movie.arg()]);
    }
    if s.menu_focus != MenuFocus::FirstItem {
        push(&[MenuFocus::FLAG, s.menu_focus.arg()]);
    }
    if s.discord_presence == DiscordPresence::On {
        push(&[DiscordPresence::FLAG, DiscordPresence::On.arg()]);
    }
    args.extend(s.display.args().into_iter().map(OsString::from));
    Invocation { program: game_exe(), args, env: env(s) }
}

pub fn benchmark_dir() -> PathBuf {
    crate::settings::app_dir().join("Benchmarks")
}

pub fn benchmark(s: &Settings, big_root: &Path) -> Invocation {
    let mut inv = game(s, big_root);
    inv.args.push("--benchmark".into());
    inv
}

pub fn game_joining(s: &Settings, big_root: &Path, server: std::net::SocketAddr) -> Invocation {
    let mut inv = game(s, big_root);
    inv.args.extend(["--connect".into(), server.to_string().into()]);
    inv
}

pub fn game_with_link(s: &Settings, big_root: &Path, link: &str) -> Invocation {
    let mut inv = game(s, big_root);
    inv.args.extend(["--link".into(), link.into()]);
    inv
}

impl Invocation {
    pub fn display(&self) -> String {
        let quote = |s: &str| -> String {
            let plain = !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || "-_./\\:,=+".contains(c));
            match (plain, cfg!(windows)) {
                (true, _) => s.to_string(),
                (false, true) => format!("'{}'", s.replace('\'', "''")),
                (false, false) => format!("'{}'", s.replace('\'', r"'\''")),
            }
        };
        let mut parts = Vec::new();
        for (k, v) in &self.env {
            parts.push(if cfg!(windows) { format!("$env:{k}={};", quote(v)) } else { format!("{k}={}", quote(v)) });
        }
        let program = quote(&self.program.to_string_lossy());
        parts.push(if cfg!(windows) { format!("& {program}") } else { program });
        parts.extend(self.args.iter().map(|a| quote(&a.to_string_lossy())));
        parts.join(" ")
    }

    pub fn command(&self) -> std::process::Command {
        let mut c = std::process::Command::new(&self.program);
        c.args(&self.args);
        for k in MANAGED_ENV {
            c.env_remove(k);
        }
        for (k, v) in &self.env {
            c.env(k, v);
        }
        c
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file_name(inv: &Invocation) -> String {
        inv.program.file_name().unwrap().to_string_lossy().into_owned()
    }

    #[test]
    fn play_starts_the_game_and_tools_the_viewer() {
        let s = Settings { music: Music::Off, vsync: false, ..Settings::default() };
        let big = Path::new("data/big");
        let game = game(&s, big);
        assert_eq!(file_name(&game), if cfg!(windows) { "avalanche-game.exe" } else { "avalanche-game" });
        assert_eq!(game.program.parent(), viewer_exe().parent(), "both next to the launcher");
        let args: Vec<_> = game.args.iter().map(|a| a.to_string_lossy().into_owned()).collect();
        assert_eq!(args[..2], ["--big-root", "data/big"]);
        assert_eq!(args[2..4], ["--save".to_string(), save_path().to_string_lossy().into_owned()]);
        assert!(args.windows(2).any(|w| w[0] == "--dlc-root" && w[1].ends_with("DLC")), "{args:?}");
        assert!(args.windows(2).any(|w| w[0] == "--tuner" && w[1].ends_with("Tuner")), "{args:?}");
        assert!(args.iter().any(|a| a == "--no-vsync"));
        assert!(!args.iter().any(|a| a == "--game" || a == "--character" || a == "--ride"));
        assert!(game.env.contains(&("SSX_MUSIC".to_string(), "off".to_string())));

        let tool = viewer(&s, big);
        assert_eq!(file_name(&tool), if cfg!(windows) { "asset-viewer.exe" } else { "asset-viewer" });
        assert_eq!(tool.env, game.env);
        assert!(!tool.args.iter().any(|a| a == "--game"));
    }

    #[test]
    fn the_range_movies_choice_is_passed_when_not_the_games() {
        let big = Path::new("data/big");
        let args = |s: &Settings| -> Vec<String> {
            game(s, big).args.iter().map(|a| a.to_string_lossy().into_owned()).collect()
        };
        let once = Settings { range_movies: RangeMovies::FirstTimeOnly, ..Settings::default() };
        assert!(args(&once).windows(2).any(|w| w[0] == "--range-movies" && w[1] == "first-time-only"));
        assert!(!args(&Settings::default()).iter().any(|a| a == "--range-movies"));
        assert!(!viewer(&once, big).args.iter().any(|a| a == "--range-movies"));
    }

    #[test]
    fn the_view_distance_is_passed_when_not_the_originals() {
        let big = Path::new("data/big");
        let strings =
            |inv: Invocation| -> Vec<String> { inv.args.iter().map(|a| a.to_string_lossy().into_owned()).collect() };
        let far = Settings { view_distance: ViewDistance::nearest(2000.0).unwrap(), ..Settings::default() };
        for args in [strings(game(&far, big)), strings(viewer(&far, big)), strings(benchmark(&far, big))] {
            assert!(args.windows(2).any(|w| w[0] == "--view-distance" && w[1] == "2000"), "{args:?}");
        }
        assert!(!strings(game(&Settings::default(), big)).iter().any(|a| a == "--view-distance"));
    }

    #[test]
    fn the_graphics_settings_are_passed_when_not_the_games() {
        let big = Path::new("data/big");
        let strings =
            |inv: Invocation| -> Vec<String> { inv.args.iter().map(|a| a.to_string_lossy().into_owned()).collect() };
        let low = Settings {
            frame_limit: FrameLimit::Fps60,
            resolution_scale: ResolutionScale::Balanced,
            shadow_quality: ShadowQuality::Low,
            bloom: false,
            fxaa: false,
            ..Settings::default()
        };
        let pair = |args: &[String], flag: &str, value: &str| args.windows(2).any(|w| w[0] == flag && w[1] == value);
        for args in [strings(game(&low, big)), strings(benchmark(&low, big))] {
            assert!(pair(&args, "--frame-limit", "60") && pair(&args, "--resolution-scale", "balanced"), "{args:?}");
            assert!(pair(&args, "--shadow-quality", "low"), "{args:?}");
            assert!(args.iter().any(|a| a == "--no-bloom") && args.iter().any(|a| a == "--no-fxaa"), "{args:?}");
        }
        let tool = strings(viewer(&low, big));
        assert!(pair(&tool, "--shadow-quality", "low") && tool.iter().any(|a| a == "--no-fxaa"), "{tool:?}");
        assert!(!tool.iter().any(|a| a == "--frame-limit" || a == "--resolution-scale"), "{tool:?}");
        for args in [strings(game(&Settings::default(), big)), strings(viewer(&Settings::default(), big))] {
            for flag in ["--frame-limit", "--resolution-scale", "--shadow-quality", "--no-bloom", "--no-fxaa"] {
                assert!(!args.iter().any(|a| a == flag), "{flag}: {args:?}");
            }
        }
    }

    #[test]
    fn no_start_turns_the_placed_objects_off() {
        let big = Path::new("data/big");
        let s = Settings::from_object(serde_json::json!({"objects": false, "vsync": false}).as_object().unwrap());
        assert!(s.old_objects && !s.vsync, "the member is read only to be noted");
        let server = std::net::SocketAddr::from(([127, 0, 0, 1], 27450));
        for inv in [
            game(&s, big),
            game_joining(&s, big, server),
            game_with_link(&s, big, "avalanche://connect/127.0.0.1:27450"),
            benchmark(&s, big),
            viewer(&s, big),
            viewer(&Settings { mode: Mode::Ride, ..s.clone() }, big),
        ] {
            assert!(!inv.args.iter().any(|a| a == "--no-objects"), "{inv:?}");
            assert!(inv.args.iter().any(|a| a == "--no-vsync"), "the rest still passed: {inv:?}");
        }
    }

    #[test]
    fn the_benchmark_runs_the_game_with_plays_options() {
        let s = Settings { vsync: false, ..Settings::default() };
        let big = Path::new("data/big");
        let bench = benchmark(&s, big);
        let play = game(&s, big);
        assert_eq!(bench.program, play.program);
        assert_eq!(bench.env, play.env);
        assert_eq!(bench.args[..play.args.len()], play.args[..]);
        let rest: Vec<_> = bench.args[play.args.len()..].iter().map(|a| a.to_string_lossy().into_owned()).collect();
        assert_eq!(rest, ["--benchmark"]);
        assert!(benchmark_dir().ends_with("Benchmarks"));
    }

    #[test]
    fn the_boot_movie_choice_is_passed_when_not_the_games() {
        let big = Path::new("data/big");
        let args = |s: &Settings| -> Vec<String> {
            game(s, big).args.iter().map(|a| a.to_string_lossy().into_owned()).collect()
        };
        let skip = Settings { boot_movie: BootMovie::Skip, ..Settings::default() };
        assert!(args(&skip).windows(2).any(|w| w[0] == "--boot-movie" && w[1] == "skip"));
        assert!(!args(&Settings::default()).iter().any(|a| a == "--boot-movie"));
        assert!(!viewer(&skip, big).args.iter().any(|a| a == "--boot-movie"));
    }

    #[test]
    fn the_menu_focus_choice_is_passed_when_not_the_games() {
        let big = Path::new("data/big");
        let args = |s: &Settings| -> Vec<String> {
            game(s, big).args.iter().map(|a| a.to_string_lossy().into_owned()).collect()
        };
        let last = Settings { menu_focus: MenuFocus::LastItem, ..Settings::default() };
        assert!(args(&last).windows(2).any(|w| w[0] == "--menu-focus" && w[1] == "last-item"));
        assert!(!args(&Settings::default()).iter().any(|a| a == "--menu-focus"));
        assert!(!viewer(&last, big).args.iter().any(|a| a == "--menu-focus"));
    }

    #[test]
    fn the_discord_switch_on_is_passed_and_off_is_nothing() {
        let big = Path::new("data/big");
        let args = |s: &Settings| -> Vec<String> {
            game(s, big).args.iter().map(|a| a.to_string_lossy().into_owned()).collect()
        };
        let on = Settings::default();
        assert!(args(&on).windows(2).any(|w| w[0] == "--discord-presence" && w[1] == "on"));
        let off = Settings { discord_presence: DiscordPresence::Off, ..Settings::default() };
        assert!(!args(&off).iter().any(|a| a.starts_with("--discord")));
        assert!(!viewer(&on, big).args.iter().any(|a| a.to_string_lossy().starts_with("--discord")));
        let has = |inv: &Invocation| inv.args.windows(2).any(|w| w[0] == "--discord-presence" && w[1] == "on");
        assert!(has(&game_with_link(&on, big, "avalanche://connect/127.0.0.1:27450")));
        assert!(!has(&game_with_link(&off, big, "avalanche://connect/127.0.0.1:27450")));
        let server = std::net::SocketAddr::from(([127, 0, 0, 1], 27450));
        assert!(has(&game_joining(&on, big, server)) && !has(&game_joining(&off, big, server)));
    }

    #[test]
    fn the_chosen_language_starts_the_game_in_it() {
        let root = std::env::temp_dir().join(format!("pa-command-lang-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let loc = root.join("bootcache/data/xenon/loc");
        std::fs::create_dir_all(&loc).unwrap();
        std::fs::write(loc.join("fre_fr.db"), b"").unwrap();
        let args = |s: &Settings| -> Vec<String> {
            game(s, &root).args.iter().map(|a| a.to_string_lossy().into_owned()).collect()
        };
        let chosen = args(&Settings { language: Some("fre_fr".into()), ..Settings::default() });
        assert!(chosen.windows(2).any(|w| w[0] == "--lang" && w[1] == "fre_fr"), "{chosen:?}");
        let missing = args(&Settings { language: Some("xx_yy".into()), ..Settings::default() });
        assert!(!missing.iter().any(|a| a == "--lang"), "{missing:?}");
        let none = args(&Settings::default());
        assert!(!none.iter().any(|a| a == "--lang"), "{none:?}");
        let _ = std::fs::remove_dir_all(&root);
    }
}
