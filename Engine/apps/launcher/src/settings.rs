use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value as Json};
use ssx_assets::port_options::{BootMovie, MenuFocus, RangeMovies};
use ssx_assets::settings_file::{self, UpdateError};

const MERGE_LEVELS: u32 = 2;
const NAME_CHARS: usize = 64;

pub fn app_dir() -> PathBuf {
    ssx_assets::setup::app_dir()
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Mode {
    #[default]
    Viewer,
    Ride,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Shading {
    #[default]
    Auto,
    Textured,
    Height,
    Material,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Music {
    #[default]
    Auto,
    DeadlyDescent,
    Licensed,
    Off,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub iso: Option<PathBuf>,
    pub game_data: Option<PathBuf>,
    pub mode: Mode,
    pub track: String,
    pub drop: usize,
    pub lighting: Option<String>,
    pub character: String,
    pub subdivide: u32,
    pub shading: Shading,
    pub objects: bool,
    pub hud: bool,
    pub vsync: bool,
    pub wireframe: bool,
    pub music: Music,
    pub music_volume: f32,
    pub sfx: bool,
    pub sfx_volume: f32,
    pub particles: bool,
    pub snow_tracks: bool,
    pub local_lights: bool,
    pub game_post: bool,
    pub game_sky: bool,
    pub boost_meter: f32,
    #[serde(deserialize_with = "range_movies_in", serialize_with = "range_movies_out")]
    pub range_movies: RangeMovies,
    #[serde(deserialize_with = "boot_movie_in", serialize_with = "boot_movie_out")]
    pub boot_movie: BootMovie,
    #[serde(deserialize_with = "menu_focus_in", serialize_with = "menu_focus_out")]
    pub menu_focus: MenuFocus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub player_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    #[serde(rename = "masters", deserialize_with = "crate::masters::present", skip_serializing)]
    pub old_masters: bool,
    #[serde(deserialize_with = "host_leniently", skip_serializing_if = "host_is_default")]
    pub host: crate::host::HostSettings,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resources: Option<serde_json::Value>,
    #[serde(deserialize_with = "display_leniently", serialize_with = "display_out")]
    pub display: ssx_assets::display::DisplaySettings,
    #[serde(skip)]
    pub display_notes: Vec<String>,
    #[serde(skip)]
    pub notes: Vec<String>,
}

fn display_leniently<'de, D: serde::Deserializer<'de>>(d: D) -> Result<ssx_assets::display::DisplaySettings, D::Error> {
    let value = serde_json::Value::deserialize(d)?;
    Ok(ssx_assets::display::DisplaySettings::from_json(Some(&value)).0)
}

fn display_out<S: serde::Serializer>(s: &ssx_assets::display::DisplaySettings, out: S) -> Result<S::Ok, S::Error> {
    s.to_json().serialize(out)
}

fn range_movies_in<'de, D: serde::Deserializer<'de>>(d: D) -> Result<RangeMovies, D::Error> {
    let name = String::deserialize(d)?;
    RangeMovies::from_name(&name).ok_or_else(|| serde::de::Error::custom("not a range movies choice"))
}

fn range_movies_out<S: serde::Serializer>(r: &RangeMovies, out: S) -> Result<S::Ok, S::Error> {
    out.serialize_str(r.name())
}

fn boot_movie_in<'de, D: serde::Deserializer<'de>>(d: D) -> Result<BootMovie, D::Error> {
    let name = String::deserialize(d)?;
    BootMovie::from_name(&name).ok_or_else(|| serde::de::Error::custom("not a boot movie choice"))
}

fn boot_movie_out<S: serde::Serializer>(b: &BootMovie, out: S) -> Result<S::Ok, S::Error> {
    out.serialize_str(b.name())
}

fn menu_focus_in<'de, D: serde::Deserializer<'de>>(d: D) -> Result<MenuFocus, D::Error> {
    let name = String::deserialize(d)?;
    MenuFocus::from_name(&name).ok_or_else(|| serde::de::Error::custom("not a menu focus choice"))
}

fn menu_focus_out<S: serde::Serializer>(m: &MenuFocus, out: S) -> Result<S::Ok, S::Error> {
    out.serialize_str(m.name())
}

fn host_is_default(h: &crate::host::HostSettings) -> bool {
    *h == crate::host::HostSettings::default()
}

fn host_leniently<'de, D: serde::Deserializer<'de>>(d: D) -> Result<crate::host::HostSettings, D::Error> {
    let value = serde_json::Value::deserialize(d)?;
    Ok(serde_json::from_value(value).unwrap_or_default())
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            iso: None,
            game_data: None,
            mode: Mode::Viewer,
            track: "alps0101".into(),
            drop: 0,
            lighting: None,
            character: ssx_assets::character::DEFAULT_CHARACTER.into(),
            subdivide: ssx_assets::subdiv::GAME_LEVELS as u32,
            shading: Shading::Auto,
            objects: true,
            hud: true,
            vsync: true,
            wireframe: false,
            music: Music::Auto,
            music_volume: 1.0,
            sfx: true,
            sfx_volume: 1.0,
            particles: true,
            snow_tracks: true,
            local_lights: true,
            game_post: true,
            game_sky: true,
            boost_meter: 0.0,
            range_movies: RangeMovies::EveryTime,
            boot_movie: BootMovie::EveryStart,
            menu_focus: MenuFocus::FirstItem,
            player_name: None,
            language: None,
            old_masters: false,
            resources: None,
            host: crate::host::HostSettings::default(),
            display: Default::default(),
            display_notes: Vec::new(),
            notes: Vec::new(),
        }
    }
}

impl Settings {
    pub fn path() -> PathBuf {
        app_dir().join("settings.json")
    }

    pub fn load() -> Self {
        Self::load_from(&Self::path())
    }

    pub fn load_from(path: &Path) -> Self {
        match settings_file::read_object(path) {
            Ok(m) => Self::from_object(&m),
            Err(e) => Settings {
                notes: vec![format!(
                    "{e:#}: the launcher's defaults are used; the file is kept until a change is saved"
                )],
                ..Settings::default()
            },
        }
    }

    pub fn from_object(m: &Map<String, Json>) -> Self {
        let mut notes = Vec::new();
        let mut s = match serde_json::from_value::<Settings>(Json::Object(m.clone())) {
            Ok(s) => s,
            Err(_) => {
                let mut good = Map::new();
                for (k, v) in m {
                    let one = Map::from_iter([(k.clone(), v.clone())]);
                    if serde_json::from_value::<Settings>(Json::Object(one)).is_ok() {
                        good.insert(k.clone(), v.clone());
                    } else {
                        let k = ssx_core::text::quoted_max(k, NAME_CHARS);
                        notes
                            .push(format!("settings.json: {k} has a value the launcher cannot read: its default used"));
                    }
                }
                serde_json::from_value(Json::Object(good)).unwrap_or_else(|e| {
                    notes.push(format!("settings.json: {e}: the launcher's defaults used"));
                    Settings::default()
                })
            }
        };
        (s.display, s.display_notes) =
            ssx_assets::display::DisplaySettings::from_json(m.get(ssx_assets::display::MEMBER));
        s.notes = notes;
        s
    }

    fn object(&self) -> Result<Map<String, Json>, UpdateError> {
        match serde_json::to_value(self) {
            Ok(Json::Object(m)) => Ok(m),
            Ok(_) => Err(UpdateError::Failed(anyhow::anyhow!("the settings are not an object"))),
            Err(e) => Err(UpdateError::Failed(e.into())),
        }
    }

    pub fn save_to(&self, path: &Path, base: &Settings) -> Result<Settings, UpdateError> {
        let (ours, base) = (self.object()?, base.object()?);
        let written = settings_file::update_object(path, |m| {
            settings_file::merge_changes(m, &base, &ours, MERGE_LEVELS);
            m.remove("masters");
            Ok(())
        })?;
        Ok(Self::from_object(&written))
    }

    pub fn reload(&mut self, base: &mut Settings) -> Result<(), String> {
        self.reload_from(&Self::path(), base)
    }

    pub fn reload_from(&mut self, path: &Path, base: &mut Settings) -> Result<(), String> {
        let theirs = settings_file::read_object(path).map_err(|e| format!("{e:#}"))?;
        let (ours, old) = (self.object().map_err(|e| e.to_string())?, base.object().map_err(|e| e.to_string())?);
        let mut merged = theirs.clone();
        settings_file::merge_changes(&mut merged, &old, &ours, MERGE_LEVELS);
        *self = Self::from_object(&merged);
        *base = Self::from_object(&theirs);
        Ok(())
    }

    pub fn replace_unreadable_at(&self, path: &Path) -> anyhow::Result<(PathBuf, Settings)> {
        let members = self.object().map_err(|e| anyhow::anyhow!("{e}"))?;
        let aside = settings_file::replace_unreadable(path, members)?;
        let written = settings_file::read_object(path)?;
        Ok((aside, Self::from_object(&written)))
    }

    pub fn rebased(&self, written: &Settings, snapshot: &Settings) -> Settings {
        match (written.object(), snapshot.object(), self.object()) {
            (Ok(mut merged), Ok(base), Ok(ours)) => {
                settings_file::merge_changes(&mut merged, &base, &ours, MERGE_LEVELS);
                Self::from_object(&merged)
            }
            _ => self.clone(),
        }
    }

    pub fn data_state(&self) -> ssx_assets::folder::State {
        ssx_assets::folder::state(&self.game_data())
    }

    pub fn data_ready(&self) -> bool {
        self.data_state().playable()
    }

    pub fn game_data(&self) -> PathBuf {
        self.game_data.clone().unwrap_or_else(ssx_assets::setup::default_data_dir)
    }

    pub fn big_root(&self) -> PathBuf {
        ssx_assets::setup::big_root(&self.game_data())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_games_player_name_survives_the_launchers_save() {
        let s: Settings =
            serde_json::from_str(r#"{"vsync": false, "player_name": "Elise", "unknown_member": "x"}"#).unwrap();
        assert_eq!(s.player_name.as_deref(), Some("Elise"));
        let back = serde_json::to_string(&s).unwrap();
        assert!(back.contains(r#""player_name":"Elise""#), "{back}");
        let none = serde_json::to_string(&Settings::default()).unwrap();
        assert!(!none.contains("player_name"), "no member written when the game set none");
    }

    #[test]
    fn the_language_survives_the_launchers_save() {
        let s: Settings = serde_json::from_str(r#"{"language": "fre_fr"}"#).unwrap();
        assert_eq!(s.language.as_deref(), Some("fre_fr"));
        let back = serde_json::to_string(&s).unwrap();
        assert!(back.contains(r#""language":"fre_fr""#), "{back}");
        assert!(!serde_json::to_string(&Settings::default()).unwrap().contains("language"));
    }

    #[test]
    fn the_range_movies_choice_is_kept_by_its_name() {
        let s: Settings = serde_json::from_str(r#"{"range_movies": "FirstTimeOnly"}"#).unwrap();
        assert_eq!(s.range_movies, RangeMovies::FirstTimeOnly);
        assert!(serde_json::to_string(&s).unwrap().contains(r#""range_movies":"FirstTimeOnly""#));
        assert_eq!(Settings::default().range_movies, RangeMovies::EveryTime, "the game's by default");
        for bad in [r#""first-time-only""#, r#""Once""#, "true", "1", "null", r#"["FirstTimeOnly"]"#] {
            let m: Map<String, Json> = serde_json::from_str(&format!(r#"{{"range_movies": {bad}, "vsync": false}}"#))
                .unwrap_or_else(|e| panic!("{bad}: {e}"));
            let s = Settings::from_object(&m);
            assert_eq!((s.range_movies, s.vsync), (RangeMovies::EveryTime, false), "{bad}: the rest read");
            assert!(s.notes.len() == 1 && s.notes[0].contains("range_movies"), "{bad}: {:?}", s.notes);
        }
    }

    #[test]
    fn the_boot_movie_choice_is_kept_by_its_name() {
        let s: Settings = serde_json::from_str(r#"{"boot_movie": "Skip"}"#).unwrap();
        assert_eq!(s.boot_movie, BootMovie::Skip);
        assert!(serde_json::to_string(&s).unwrap().contains(r#""boot_movie":"Skip""#));
        assert_eq!(Settings::default().boot_movie, BootMovie::EveryStart, "the game's by default");
        for bad in [r#""skip""#, r#""Off""#, "true", "0", "null", r#"["Skip"]"#] {
            let m: Map<String, Json> = serde_json::from_str(&format!(r#"{{"boot_movie": {bad}, "vsync": false}}"#))
                .unwrap_or_else(|e| panic!("{bad}: {e}"));
            let s = Settings::from_object(&m);
            assert_eq!((s.boot_movie, s.vsync), (BootMovie::EveryStart, false), "{bad}: the rest read");
            assert!(s.notes.len() == 1 && s.notes[0].contains("boot_movie"), "{bad}: {:?}", s.notes);
        }
    }

    #[test]
    fn the_menu_focus_choice_is_kept_by_its_name() {
        let s: Settings = serde_json::from_str(r#"{"menu_focus": "LastItem"}"#).unwrap();
        assert_eq!(s.menu_focus, MenuFocus::LastItem);
        assert!(serde_json::to_string(&s).unwrap().contains(r#""menu_focus":"LastItem""#));
        assert_eq!(Settings::default().menu_focus, MenuFocus::FirstItem, "the game's by default");
        for bad in [r#""last-item""#, r#""Remember""#, "true", "1", "null", r#"["LastItem"]"#] {
            let m: Map<String, Json> = serde_json::from_str(&format!(r#"{{"menu_focus": {bad}, "vsync": false}}"#))
                .unwrap_or_else(|e| panic!("{bad}: {e}"));
            let s = Settings::from_object(&m);
            assert_eq!((s.menu_focus, s.vsync), (MenuFocus::FirstItem, false), "{bad}: the rest read");
            assert!(s.notes.len() == 1 && s.notes[0].contains("menu_focus"), "{bad}: {:?}", s.notes);
        }
    }

    #[test]
    fn the_games_resource_limits_survive_the_launchers_save() {
        let text = r#"{"vsync": false, "resources": {"download_limit_mib": 512, "server_scripts": false}}"#;
        let s: Settings = serde_json::from_str(text).unwrap();
        let back = serde_json::to_string(&s).unwrap();
        assert!(back.contains(r#""resources":{"download_limit_mib":512,"server_scripts":false}"#), "{back}");
        assert!(!serde_json::to_string(&Settings::default()).unwrap().contains("resources"));
    }

    #[test]
    fn an_old_master_list_is_read_noted_and_dropped_at_the_save() {
        for old in [r#"["https://m.example.org"]"#, "[]", r#""https://m.example.org""#, r#"[1, null]"#, "null"] {
            let text = format!(r#"{{"vsync": false, "player_name": "Elise", "masters": {old}}}"#);
            let s: Settings = serde_json::from_str(&text).unwrap_or_else(|e| panic!("{old}: {e}"));
            assert!(s.old_masters && !s.vsync && s.player_name.as_deref() == Some("Elise"), "{old}: the rest kept");
            let back = serde_json::to_string(&s).unwrap();
            assert!(!back.contains("masters") && back.contains(r#""vsync":false"#), "{old}: {back}");
        }
        let fresh: Settings = serde_json::from_str(r#"{"vsync": false}"#).unwrap();
        assert!(!fresh.old_masters);
        assert!(!Settings::default().old_masters);
        assert!(!serde_json::to_string(&Settings::default()).unwrap().contains("masters"));
        assert!(crate::masters::old_member_note(true).is_some_and(|n| n.contains(ssx_core::OFFICIAL_MASTER)));
        assert_eq!(crate::masters::old_member_note(false), None);
    }

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("launcher-settings-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("settings.json")
    }

    fn file(path: &Path) -> Json {
        serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
    }

    #[test]
    fn the_games_name_survives_a_launcher_save_from_an_older_copy() {
        let path = temp("name");
        std::fs::write(&path, r#"{"vsync": true, "track": "alps0101", "language": "fr"}"#).unwrap();
        let mut saved = Settings::load_from(&path);
        let mut settings = saved.clone();
        settings_file::update_object(&path, |m| {
            m.insert("player_name".into(), Json::String("Elise".into()));
            Ok(())
        })
        .unwrap();
        let base = ssx_assets::display::DisplaySettings::default();
        let sized = ssx_assets::display::DisplaySettings { window: Some((1600, 900)), ..base.clone() };
        ssx_assets::display::write_member(&path, &base, &sized).unwrap();
        settings.display.mode = ssx_assets::display::WindowMode::Borderless;
        let written = settings.save_to(&path, &saved).unwrap();
        (settings, saved) = (written.clone(), written);
        let v = file(&path);
        assert_eq!((v["player_name"].clone(), v["language"].clone()), (Json::from("Elise"), Json::from("fr")));
        assert_eq!(v["display"]["mode"], "Borderless");
        assert_eq!(v["display"]["window"], serde_json::json!({"width": 1600, "height": 900}), "the game's size kept");
        assert_eq!(settings.player_name.as_deref(), Some("Elise"), "the launcher shows the game's name");
        assert_eq!(saved.display.window, Some((1600, 900)));
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn the_launcher_takes_what_the_game_wrote_when_it_closes() {
        let path = temp("reload");
        std::fs::write(&path, r#"{"vsync": true, "sfx_volume": 1.0}"#).unwrap();
        let mut saved = Settings::load_from(&path);
        let mut settings = Settings { vsync: false, ..saved.clone() };
        settings_file::update_object(&path, |m| {
            m.insert("player_name".into(), Json::String("Elise".into()));
            m.insert("sfx_volume".into(), Json::from(0.5));
            Ok(())
        })
        .unwrap();
        settings.reload_from(&path, &mut saved).unwrap();
        assert_eq!((settings.player_name.as_deref(), settings.sfx_volume, settings.vsync), (Some("Elise"), 0.5, false));
        assert_eq!((saved.sfx_volume, saved.vsync), (0.5, true));
        assert_ne!(settings, saved, "the launcher's own change is still to be saved");
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn a_broken_file_is_never_written_over_and_unknown_members_are_kept() {
        let path = temp("broken");
        let settings = Settings { vsync: false, ..Settings::default() };
        std::fs::write(&path, b"{\"vsync\": tru").unwrap();
        let loaded = Settings::load_from(&path);
        assert!(loaded == Settings { notes: loaded.notes.clone(), ..Settings::default() } && loaded.notes.len() == 1);
        assert!(matches!(settings.save_to(&path, &loaded), Err(UpdateError::Unreadable(_))));
        assert_eq!(std::fs::read(&path).unwrap(), b"{\"vsync\": tru", "left as it was");
        let (aside, written) = settings.replace_unreadable_at(&path).unwrap();
        assert_eq!(std::fs::read(&aside).unwrap(), b"{\"vsync\": tru");
        assert!(!written.vsync && file(&path)["vsync"] == false);

        let text = r#"{"future": {"a": [1]}, "hud": "yes", "objects": false, "masters": ["https://m.example.org"]}"#;
        std::fs::write(&path, text).unwrap();
        let saved = Settings::load_from(&path);
        assert!(!saved.objects && saved.hud, "the rest read, `hud` its default");
        assert!(saved.notes.len() == 1 && saved.notes[0].contains("\"hud\""), "{:?}", saved.notes);
        let settings = Settings { sfx: false, ..saved.clone() };
        let written = settings.save_to(&path, &saved).unwrap();
        let v = file(&path);
        assert_eq!((v["future"].clone(), v["hud"].clone()), (serde_json::json!({"a": [1]}), Json::from("yes")));
        assert!(v["sfx"] == false && v.get("masters").is_none(), "{v}");
        assert!(!written.sfx && !written.objects);
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }
}
