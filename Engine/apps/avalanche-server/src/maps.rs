use std::collections::{BTreeMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use bevy::prelude::{Res, ResMut, Resource};
use ssx_assets::content::GameContent;
use ssx_content::ContentId;
use ssx_content::map::MapToml;
use ssx_gameplay::validate::Limits;
use ssx_net_proto::Mode;
use ssx_net_proto::instance::{GamemodeSummary, Level, MAX_MAP_TITLE_CHARS, MapSummary};
use ssx_physics::map::ResolvedMap;
use ssx_rules::subset::TuningLimits;

use crate::config::Config;

use ssx_physics::map::server_load::check_toml;
pub use ssx_physics::map::server_load::{LoadedStart, MapLoad, START_RIDER, load_resolved, mode_choice, record_rules};

pub const MAX_LOADED_MAPS: usize = 256;

type LoadKey = (String, u8, Option<String>, u64);

type KeptLoad = (LoadKey, Result<Arc<MapLoad>, String>);

pub struct MapLoader {
    rules: Arc<crate::rules::ServerRules>,
    loads: Mutex<VecDeque<KeptLoad>>,
}

impl MapLoader {
    pub fn new(rules: Arc<crate::rules::ServerRules>) -> Self {
        Self { rules, loads: Mutex::new(VecDeque::new()) }
    }

    pub fn rules(&self) -> &Arc<crate::rules::ServerRules> {
        &self.rules
    }

    pub fn load(&self, id: &str, event_type: u8, gamemode: Option<&str>) -> Result<Arc<MapLoad>, String> {
        let (_settled, generation) = self.rules.settled.reading();
        let key = (id.to_string(), event_type, gamemode.map(str::to_string), generation);
        if let Some((_, r)) = self.loads.lock().ok().and_then(|l| l.iter().find(|(k, _)| *k == key).cloned()) {
            return r;
        }
        let folder = self.rules.folder();
        let r = self
            .rules
            .of_mount(generation)
            .and_then(|rules| load_map(folder, id, (event_type, gamemode), &rules.tuning))
            .map(Arc::new);
        if let Ok(m) = &r {
            bevy::log::info!("{}", m.describe(id));
        }
        if let Ok(mut l) = self.loads.lock() {
            l.retain(|(k, _)| k.3 == key.3);
            if l.len() == MAX_LOADED_MAPS {
                l.pop_front();
            }
            l.push_back((key, r.clone()));
        }
        r
    }

    pub fn limits(&self, id: &str, event_type: u8, gamemode: Option<&str>) -> Result<Limits, String> {
        let load = self.load(id, event_type, gamemode)?;
        load.limits.clone().ok_or_else(|| format!("{id}: a freeride has no rounds to validate"))
    }
}

#[derive(Clone, Resource)]
pub struct ServerMaps {
    pub folder: PathBuf,
    pub loader: Arc<MapLoader>,
    pub generation: u64,
    pub titles: BTreeMap<String, String>,
    pub gamemode_titles: BTreeMap<String, String>,
}

impl ServerMaps {
    pub fn new(rules: Arc<crate::rules::ServerRules>) -> Self {
        let folder = rules.folder().to_path_buf();
        let loader = Arc::new(MapLoader::new(rules));
        Self { folder, loader, generation: 0, titles: BTreeMap::new(), gamemode_titles: BTreeMap::new() }
    }

    pub fn check(&mut self, config: &mut Config) -> Result<(), String> {
        for (k, g) in config.instances.iter_mut().enumerate() {
            let mode = g.spec.mode;
            let levels: Vec<&mut Level> = match &mut g.rounds {
                Some((_, rotation)) => rotation.iter_mut().collect(),
                None => vec![&mut g.spec.level],
            };
            for level in levels {
                let Level::Map { id, event_type, gamemode } = &mut *level else { continue };
                let checked = check_record(&self.folder, id, *event_type, gamemode.as_deref())
                    .map_err(|e| format!("instance {}: {e}", k + 1))?;
                *event_type = checked.event_type;
                if !level.fits(mode) {
                    return Err(format!(
                        "instance {}: {level} rides {} ({}), not a {} instance's event type (race 5…7, trick 0…2, \
                         survive and death 3…4, freeride 8)",
                        k + 1,
                        checked.event_name,
                        checked.event_type,
                        mode.name()
                    ));
                }
                let Level::Map { id, event_type, gamemode } = &*level else { continue };
                if mode == Mode::Freeride {
                    self.loader
                        .load(id, *event_type, gamemode.as_deref())
                        .map_err(|e| format!("instance {}: {e}", k + 1))?;
                }
                self.titles.insert(id.clone(), checked.toml.title);
                if let (Some(g), Some(t)) = (gamemode, checked.gamemode_title) {
                    self.gamemode_titles.insert(g.clone(), t);
                }
            }
            if let Some((_, rotation)) = &g.rounds
                && let Some(first) = rotation.first()
            {
                g.spec.level.clone_from(first);
            }
        }
        Ok(())
    }

    pub fn shown(&self, level: &Level) -> ShownLevel {
        shown(level, &self.titles, &self.gamemode_titles)
    }
}

pub fn follow_resources(resources: Res<crate::resources::ServerResources>, maps: Option<ResMut<ServerMaps>>) {
    let Some(mut maps) = maps else { return };
    if maps.generation == resources.generation {
        return;
    }
    let rules = Arc::clone(maps.loader.rules());
    let Some((_settled, mount)) = rules.settled.try_reading() else { return };
    maps.generation = resources.generation;
    let content = GameContent::of(&maps.folder);
    let maps = &mut *maps;
    for (id, title) in &mut maps.titles {
        let toml = id.parse::<ContentId>().ok().and_then(|cid| content.map_toml(&cid));
        match toml {
            Some(Ok(t)) => *title = t.title,
            Some(Err(e)) => bevy::log::warn!("maps: {}: {}", ssx_core::text::shown(id), ssx_core::text::message(&e)),
            None => bevy::log::warn!("maps: {}: no running resource provides it now", ssx_core::text::shown(id)),
        }
    }
    for (id, title) in &mut maps.gamemode_titles {
        match id
            .parse::<ContentId>()
            .map_err(|e| e.to_string())
            .and_then(|g| content.gamemode(&g).map_err(|e| e.to_string()))
        {
            Ok(g) => title.clone_from(&g.value.title),
            Err(e) => bevy::log::warn!("maps: {}: {}", ssx_core::text::shown(id), ssx_core::text::message(&e)),
        }
    }
    bevy::log::info!(
        "maps: the resource set's generation {} (the content's mount {mount}): the levels load their rules again at \
         their next round",
        resources.generation
    );
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ShownLevel {
    pub drop: String,
    pub map: Option<MapSummary>,
}

pub fn shown(level: &Level, titles: &BTreeMap<String, String>, gamemodes: &BTreeMap<String, String>) -> ShownLevel {
    match level {
        Level::Drop(d) => ShownLevel { drop: d.clone(), map: None },
        Level::Map { id, event_type, gamemode } => {
            let title_of = |titles: &BTreeMap<String, String>, id: &str| {
                titles
                    .get(id)
                    .filter(|t| {
                        (1..=MAX_MAP_TITLE_CHARS).contains(&t.chars().count()) && !t.chars().any(char::is_control)
                    })
                    .cloned()
                    .unwrap_or_else(|| id.chars().take(MAX_MAP_TITLE_CHARS).collect())
            };
            let title = title_of(titles, id);
            let gamemode = gamemode.as_ref().map(|g| GamemodeSummary { id: g.clone(), title: title_of(gamemodes, g) });
            let map = MapSummary { id: id.clone(), title, event_type: *event_type, gamemode };
            ShownLevel { drop: String::new(), map: Some(map) }
        }
    }
}

pub fn load_map(
    folder: &Path,
    id: &str,
    level_of: (u8, Option<&str>),
    tuning: &TuningLimits,
) -> Result<MapLoad, String> {
    let t0 = Instant::now();
    let cid: ContentId = id.parse().map_err(|e| format!("{}: {e}", ssx_core::text::quoted(id)))?;
    let content = GameContent::of(folder);
    let resolved =
        ResolvedMap::resolve(&content, &cid).map_err(|e| ssx_core::text::message(&format!("{e:#}")).to_string())?;
    let mut load = load_resolved(folder, &content, id, level_of, &resolved, tuning)?;
    load.took = t0.elapsed();
    Ok(load)
}

#[derive(Clone, Debug)]
pub struct CheckedRecord {
    pub toml: MapToml,
    pub event_type: u8,
    pub event_name: &'static str,
    pub gamemode_title: Option<String>,
}

pub fn check_record(folder: &Path, id: &str, event_type: u8, gamemode: Option<&str>) -> Result<CheckedRecord, String> {
    let cid: ContentId = id.parse().map_err(|e| format!("{}: {e}", ssx_core::text::quoted(id)))?;
    let choice = mode_choice(id, event_type, gamemode)?;
    let content = GameContent::of(folder);
    let toml = match content.map_toml(&cid) {
        Some(t) => t.map_err(|e| ssx_core::text::message(&e).to_string())?,
        None => return Err(format!("{id}: no package of the server's resources provides it")),
    };
    check_toml(folder, &content, id, &toml)?;
    let (rules, gamemode_title) = record_rules(&content, &cid, &toml, &choice)?;
    let t = rules.event_type();
    Ok(CheckedRecord { toml, event_type: t as u8, event_name: t.name(), gamemode_title })
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use ssx_assets::content::GameContent;
    use ssx_assets::content::modtool::{Template, new_package};

    use super::*;

    fn rules_with_map(name: &str, toml: Option<&str>) -> (PathBuf, PathBuf, anyhow::Result<()>) {
        let out = std::env::temp_dir().join(format!("ssx-server-maps-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&out);
        let rules = out.join("rules");
        std::fs::create_dir_all(&rules).expect("a temporary folder");
        let pkg = new_package(name, Template::Map, &out, Path::new("")).expect("the map template");
        if let Some(t) = toml {
            std::fs::write(pkg.join("map.toml"), t).expect("map.toml");
        }
        let mounted = GameContent::of(&rules).mount_folder(&pkg, &[]).map(|_| ());
        (out, rules, mounted)
    }

    fn server_rules(rules: &Path) -> Arc<crate::rules::ServerRules> {
        Arc::new(crate::rules::ServerRules::new(rules.to_path_buf(), Default::default()))
    }

    fn tuning() -> TuningLimits {
        TuningLimits {
            fall: 1.0,
            horizontal: 1.0,
            spin: 0.5,
            flip: 0.5,
            grind_spin: 0.5,
            trick_point_bonus: 1.0,
            heli_shut_out: 30.0,
        }
    }

    #[test]
    fn info_names_a_map_by_its_title_or_its_id() {
        let none = BTreeMap::new();
        let drop = shown(&Level::Drop("chapter_1_drop_1".into()), &BTreeMap::new(), &none);
        assert_eq!(drop, ShownLevel { drop: "chapter_1_drop_1".into(), map: None });
        let id = format!("a:map/{}", "c".repeat(100));
        let map = Level::map(&id, 5).expect("a map");
        let mut titles = BTreeMap::new();
        let title_of =
            |titles: &BTreeMap<String, String>| shown(&map, titles, &none).map.expect("a map's summary").title;
        assert_eq!(title_of(&titles), id.chars().take(MAX_MAP_TITLE_CHARS).collect::<String>());
        titles.insert(id.clone(), "Slope \u{1D11E}".into());
        assert_eq!(title_of(&titles), "Slope \u{1D11E}");
        for bad in ["", "a\u{7}b", "line\nbreak", &"x".repeat(MAX_MAP_TITLE_CHARS + 1)] {
            titles.insert(id.clone(), bad.to_string());
            assert_eq!(title_of(&titles).chars().count(), MAX_MAP_TITLE_CHARS, "{bad:?}");
        }
        let s = shown(&map, &titles, &none);
        assert!(s.drop.is_empty());
        let m = s.map.expect("a map's summary");
        assert_eq!((m.id.as_str(), m.event_type, m.gamemode.is_none()), (id.as_str(), 5, true));
        assert!(m.validate().is_ok());
    }

    #[test]
    fn a_configured_map_is_checked_and_refused_with_its_reason() {
        let (out, rules, mounted) = rules_with_map("tpl", None);
        mounted.expect("the template mounts");
        assert_eq!(check_record(&rules, "tpl:map/course", 5, None).expect("a race").toml.title, "Template slope");
        assert!(check_record(&rules, "tpl:map/course", 8, None).is_ok(), "a freeride");
        for (id, t, says) in [
            ("tpl:map/other", 5, "no package of the server's resources provides it"),
            ("tpl:map/course", 0, "offers the modes [\"race\", \"freeride\"], not trick"),
            ("tpl:map/course", 4, "not death"),
            ("tpl:map/course", 7, "event type 7 is no built-in mode's"),
            ("Tpl:map/course", 5, "\"Tpl:map/course\""),
        ] {
            let e = check_record(&rules, id, t, None).expect_err(id);
            assert!(e.contains(says), "{id} {t}: {e}");
        }
        let e = load_map(&rules, "tpl:map/course", (5, None), &tuning()).expect_err("no disc data");
        assert!(e.starts_with("tpl:map/course: ") && e.contains("node \"course/flare\" ava_object"), "{e}");
        let text = "[game]\nmode = \"race\"\nmap = \"tpl:map/course\"\n";
        let mut config = crate::config::parse(Path::new("server.toml"), text).expect("a map's configuration");
        let mut maps = ServerMaps::new(server_rules(&rules));
        maps.check(&mut config).expect("the template's race");
        assert_eq!(maps.shown(&config.instances[0].spec.level).map.expect("a map").title, "Template slope");
        let mut ride = |level: Level| {
            config.instances[0].rounds.as_mut().expect("a race's rotation").1 = vec![level];
            maps.check(&mut config)
        };
        let gamemode = Level::map_under("tpl:map/course", 5, Some("tpl:gamemode/x")).expect("ids");
        let e = ride(gamemode).expect_err("no such gamemode");
        assert!(e.starts_with("instance 1: ") && e.contains("tpl:gamemode/x"), "{e}");
        let gone = ride(Level::map("tpl:map/gone", 5).expect("an id")).expect_err("gone");
        assert!(gone.starts_with("instance 1: tpl:map/gone: no package"), "{gone}");
        let _ = std::fs::remove_dir_all(&out);
    }

    #[test]
    fn a_configured_gamemode_sets_its_event_type_or_is_refused() {
        let out = std::env::temp_dir().join(format!("ssx-server-maps-gamemode-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&out);
        let rules = out.join("rules");
        std::fs::create_dir_all(&rules).expect("a temporary folder");
        for (name, template) in [("gm", Template::Gamemode), ("tpl", Template::Map)] {
            let pkg = new_package(name, template, &out, Path::new("")).expect("a template");
            GameContent::of(&rules).mount_folder(&pkg, &[]).expect("the template mounts");
        }
        let g = "gm:gamemode/gully_race";
        let c = check_record(&rules, "gm:map/course", 5, Some(g)).expect("the gamemode's race");
        assert_eq!((c.event_type, c.gamemode_title.as_deref()), (6, Some("Gully Race")));
        let e = check_record(&rules, "tpl:map/course", 5, Some(g)).expect_err("not its map");
        assert!(e.starts_with("gm:gamemode/gully_race on tpl:map/course: "), "{e}");
        let text = format!("[game]\nmode = \"race\"\nmap = \"gm:map/course\"\ngamemode = \"{g}\"\n");
        let mut config = crate::config::parse(Path::new("server.toml"), &text).expect("a gamemode's configuration");
        assert_eq!(config.game().level, Level::map_under("gm:map/course", 5, Some(g)).expect("ids"), "the mode's");
        let mut maps = ServerMaps::new(server_rules(&rules));
        maps.check(&mut config).expect("the gamemode's race");
        let level = Level::map_under("gm:map/course", 6, Some(g)).expect("ids");
        assert_eq!(config.game().level, level, "the package's event type");
        assert_eq!(config.instances[0].rounds.as_ref().map(|r| r.1.clone()), Some(vec![level.clone()]));
        let summary = maps.shown(&level).map.expect("a map");
        assert_eq!((summary.event_type, summary.gamemode.map(|g| g.title)), (6, Some("Gully Race".into())));
        let trick = text.replace("\"race\"", "\"trick\"");
        let mut config = crate::config::parse(Path::new("server.toml"), &trick).expect("parses");
        let e = maps.check(&mut config).expect_err("a race's gamemode on a trick instance");
        assert!(e.contains("rides") && e.contains("not a trick instance's"), "{e}");
        let none = text.replace("gully_race", "none");
        let mut config = crate::config::parse(Path::new("server.toml"), &none).expect("parses");
        assert!(maps.check(&mut config).expect_err("no such gamemode").contains("gm:gamemode/none"));
        let _ = std::fs::remove_dir_all(&out);
    }

    #[test]
    fn a_hostile_map_record_is_refused_with_a_bounded_reason() {
        let head = "[map]\nscene = \"maps/course.gltf\"\n";
        let long = format!("[map]\ntitle = \"{}\"\nscene = \"maps/course.gltf\"\nmodes = [\"race\"]\n", "x".repeat(70));
        let big = format!(
            "[map]\ntitle = \"t\"\nscene = \"maps/course.gltf\"\nmodes = [\"race\"]\n#{}\n",
            "x".repeat(5 << 20)
        );
        let cases = [
            ("long", long),
            ("bell", format!("{head}title = \"a\\u0007b\"\nmodes = [\"race\"]\n")),
            ("nomodes", format!("{head}title = \"t\"\nmodes = []\n")),
            ("chess", format!("{head}title = \"t\"\nmodes = [\"chess\"]\n")),
            ("escape", "[map]\ntitle = \"t\"\nscene = \"../../../x.gltf\"\nmodes = [\"race\"]\n".to_string()),
            ("base", format!("{head}title = \"t\"\nmodes = [\"race\"]\nbase = \"ssx:drop/no_such_drop\"\n")),
            ("basekind", format!("{head}title = \"t\"\nmodes = [\"race\"]\nbase = \"tpl:object/x\"\n")),
            ("death", format!("{head}title = \"t\"\nmodes = [\"death\"]\nintro = \"spawn\"\n")),
            ("nottoml", "\u{0}\u{1}[[[".to_string()),
            ("big", big),
        ];
        for (name, toml) in &cases {
            let (out, rules, mounted) = rules_with_map(&format!("h{name}"), Some(toml));
            let checked = mounted
                .map_err(|e| e.to_string())
                .and_then(|()| check_record(&rules, &format!("h{name}:map/course"), 5, None).map(|c| c.toml.title));
            let e = checked.expect_err(name);
            assert!(e.chars().count() <= 4 * ssx_core::text::MESSAGE_CHARS, "{name}: {} characters", e.chars().count());
            assert!(!e.chars().any(|c| c.is_control() && c != '\n'), "{name}: {e:?}");
            let _ = std::fs::remove_dir_all(&out);
        }
    }
}
