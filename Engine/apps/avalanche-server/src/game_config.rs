use std::ops::Range;

use serde::Deserialize;
use ssx_gameplay::round::{CycleRules, MIN_COUNTDOWN_TICKS};
use ssx_net::limits::MAX_CLIENTS;
use ssx_net_proto::instance::{Level, MAX_REWIND_LIMIT, drop_name, gamemode_id};
use ssx_net_proto::{GhostStyle, InstanceSpec, Mode, Rewinds, TICK_HZ};
use toml::Spanned;

use crate::config::{Config, ConfigError, default_game};

impl Config {
    pub fn gamemode_level(&self) -> Option<&Level> {
        self.instances
            .iter()
            .flat_map(|g| std::iter::once(&g.spec.level).chain(g.rounds.iter().flat_map(|(_, r)| r.iter())))
            .find(|l| matches!(l, Level::Map { gamemode: Some(_), .. }))
    }

    pub fn game(&self) -> &InstanceSpec {
        &self.instances[0].spec
    }

    pub fn instance_config(&self) -> ssx_net::server::InstanceConfig {
        let instances = self
            .instances
            .iter()
            .map(|g| ssx_net::server::InstanceSetup {
                spec: g.spec.clone(),
                capacity: usize::from(g.capacity.unwrap_or(self.max_players).min(self.max_players)),
                rounds: g.rounds.clone(),
            })
            .collect();
        ssx_net::server::InstanceConfig { instances }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct GameConfig {
    pub spec: InstanceSpec,
    pub capacity: Option<u16>,
    pub rounds: Option<(CycleRules, Vec<Level>)>,
}

pub fn mode_of(name: &str) -> Option<(Mode, u8)> {
    let event_type = ssx_net_proto::instance::built_in_event_type(name)?;
    let mode = match name {
        "freeride" => Mode::Freeride,
        "race" => Mode::Race,
        "trick" => Mode::Trick,
        _ => Mode::Survive,
    };
    Some((mode, event_type))
}

pub fn map_mode_name(event_type: u8) -> Option<&'static str> {
    ssx_net_proto::instance::built_in_mode(event_type)
}

// PORT(choice): 32 drops (the disc has 262 named drops; a rotation is a playlist); tuning: what operators ask for once
// servers are public (phase 4); the original has no dedicated server.
pub const MAX_ROTATION: usize = 32;

// PORT(choice): 8 instances (each a room, a replicated entity and a standings entity per client of the lobby); tuning:
// phase 4's 32-player measurement with several instances; the original has no dedicated server.
pub const MAX_INSTANCES: usize = 8;

// PORT(choice): the defaults — a round forms with 1 player after 15 s of the lobby (a lone player rides; others joining
// in those 15 s ride with them), loading waits 60 s, the countdown 5 s (the plan's at least 3 s), a round lasts at most
// 600 s, the results 15 s; tuning: what operators set and players ask for (phase 4's public servers); the original has
// no dedicated server.
const DEFAULT_MIN_PLAYERS: i64 = 1;
const LOBBY_WAIT: (i64, i64, i64) = (15, 0, 600);
const LOAD_TIMEOUT: (i64, i64, i64) = (60, 10, 600);
const COUNTDOWN: (i64, i64, i64) = (5, (MIN_COUNTDOWN_TICKS / TICK_HZ) as i64, 60);
const ROUND_LIMIT: (i64, i64, i64) = (600, 30, 3_600);
const RESULTS_TIME: (i64, i64, i64) = (15, 5, 120);

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawGame {
    drop: Option<Spanned<String>>,
    map: Option<Spanned<String>>,
    rotation: Option<Spanned<Vec<Spanned<toml::Value>>>>,
    mode: Option<Spanned<String>>,
    gamemode: Option<Spanned<String>>,
    rewinds: Option<Spanned<toml::Value>>,
    ghosts: Option<Spanned<String>>,
    ghost_style: Option<Spanned<String>>,
    capacity: Option<Spanned<i64>>,
    min_players: Option<Spanned<i64>>,
    lobby_wait: Option<Spanned<i64>>,
    load_timeout: Option<Spanned<i64>>,
    countdown: Option<Spanned<i64>>,
    round_limit: Option<Spanned<i64>>,
    results_time: Option<Spanned<i64>>,
}

pub(crate) fn instances(
    game: Option<Spanned<RawGame>>,
    list: Option<Spanned<Vec<Spanned<RawGame>>>>,
    at: &dyn Fn(Range<usize>, String) -> ConfigError,
) -> Result<Option<Vec<GameConfig>>, ConfigError> {
    match (game, list) {
        (Some(_), Some(list)) => Err(at(list.span(), "[[instance]]: [game] or [[instance]] tables, not both".into())),
        (Some(game), None) => {
            let span = game.span();
            Ok(Some(vec![parse_game(game.into_inner(), span, false, at)?]))
        }
        (None, Some(list)) => {
            let span = list.span();
            let list = list.into_inner();
            if list.is_empty() || list.len() > MAX_INSTANCES {
                return Err(at(span, format!("[[instance]]: 1 to {MAX_INSTANCES} instances")));
            }
            let all = list
                .into_iter()
                .map(|g| {
                    let span = g.span();
                    parse_game(g.into_inner(), span, true, at)
                })
                .collect::<Result<_, _>>()?;
            Ok(Some(all))
        }
        (None, None) => Ok(None),
    }
}

fn parse_game(
    g: RawGame,
    span: Range<usize>,
    in_list: bool,
    at: &dyn Fn(Range<usize>, String) -> ConfigError,
) -> Result<GameConfig, ConfigError> {
    let d = default_game();
    let drop_rule = |name: &str, span: Range<usize>| {
        drop_name(name)
            .map(|_| name.to_string())
            .map_err(|e| at(span, format!("drop = {name:?}: {e} (a ui_drop's name)")))
    };
    let (mode, event_type) = match &g.mode {
        Some(m) => mode_of(m.get_ref()).ok_or_else(|| {
            at(
                m.span(),
                format!(
                    "mode = {:?}: \"freeride\", \"race\", \"trick\", \"survive\" or \"death\" (survive to death)",
                    m.get_ref()
                ),
            )
        })?,
        None => (d.mode, 8),
    };
    let gamemode_rule = |id: &str, span: Range<usize>| {
        gamemode_id(id)
            .map(str::to_string)
            .map_err(|e| at(span, format!("gamemode = {id:?}: {e} (a gamemode's content id)")))
    };
    let instance_gamemode = g.gamemode.as_ref().map(|v| gamemode_rule(v.get_ref(), v.span())).transpose()?;
    let map_rule = |id: &str, span: Range<usize>, gamemode: Option<&str>| {
        Level::map_under(id, event_type, gamemode)
            .map_err(|e| at(span, format!("map = {id:?}: {e} (a map's content id)")))
    };
    let entry_rule = |e: &Spanned<toml::Value>| -> Result<Level, ConfigError> {
        let shape = "rotation: an entry is a drop's name, a map's id or { map = \"<id>\", gamemode = \"<id>\" }";
        match e.get_ref() {
            toml::Value::String(id) if id.contains(':') => map_rule(id, e.span(), instance_gamemode.as_deref()),
            toml::Value::String(name) => drop_rule(name, e.span()).map(Level::Drop),
            toml::Value::Table(t) => match (t.get("map"), t.get("gamemode")) {
                (Some(toml::Value::String(id)), Some(toml::Value::String(gm))) if t.len() == 2 => {
                    let gm = gamemode_rule(gm, e.span())?;
                    map_rule(id, e.span(), Some(&gm))
                }
                _ => Err(at(e.span(), shape.into())),
            },
            _ => Err(at(e.span(), shape.into())),
        }
    };
    let rotation: Vec<Level> = match (&g.drop, &g.map, &g.rotation) {
        (Some(_), Some(m), _) => return Err(at(m.span(), "map: drop or map, not both".into())),
        (Some(_), None, Some(r)) | (None, Some(_), Some(r)) => {
            return Err(at(r.span(), "rotation: drop, map or rotation, not two of them".into()));
        }
        (Some(drop), None, None) => vec![Level::Drop(drop_rule(drop.get_ref(), drop.span())?)],
        (None, Some(map), None) => vec![map_rule(map.get_ref(), map.span(), instance_gamemode.as_deref())?],
        (None, None, Some(r)) => {
            let list = r.get_ref();
            if mode == Mode::Freeride {
                return Err(at(r.span(), "rotation: freeride rides one level (drop = … or map = …)".into()));
            }
            if list.is_empty() || list.len() > MAX_ROTATION {
                return Err(at(r.span(), format!("rotation: 1 to {MAX_ROTATION} drops and maps")));
            }
            list.iter().map(entry_rule).collect::<Result<_, _>>()?
        }
        (None, None, None) => vec![d.level.clone()],
    };
    if let (Some(v), Some(gm)) = (&g.gamemode, &instance_gamemode)
        && !rotation.iter().any(|l| matches!(l, Level::Map { gamemode: Some(g), .. } if g == gm))
    {
        return Err(at(
            v.span(),
            format!("gamemode = {gm:?}: the instance rides no map under it (a drop's event is the disc's)"),
        ));
    }
    let rewinds = match g.rewinds {
        Some(r) => {
            let span = r.span();
            match r.into_inner() {
                toml::Value::String(s) if s == "on" => Rewinds::On,
                toml::Value::String(s) if s == "off" => Rewinds::Off,
                toml::Value::Integer(n) if (1..=i64::from(MAX_REWIND_LIMIT)).contains(&n) => Rewinds::Limit(n as u8),
                v => {
                    return Err(at(span, format!("rewinds = {v}: \"on\", \"off\" or 1 to {MAX_REWIND_LIMIT}")));
                }
            }
        }
        None => d.rewinds,
    };
    let ghosts = match g.ghosts {
        Some(v) => match v.get_ref().as_str() {
            "on" => true,
            "off" => false,
            other => return Err(at(v.span(), format!("ghosts = {other:?}: \"on\" or \"off\""))),
        },
        None => d.ghosts,
    };
    let ghost_style = match g.ghost_style {
        Some(v) => GhostStyle::parse(v.get_ref())
            .ok_or_else(|| at(v.span(), format!("ghost_style = {:?}: \"character\" or \"hologram\"", v.get_ref())))?,
        None => d.ghost_style,
    };
    let capacity = match g.capacity {
        Some(c) if !in_list => {
            return Err(at(c.span(), "capacity: [game]'s instance takes [server] max_players".into()));
        }
        Some(c) => Some(
            u16::try_from(*c.get_ref())
                .ok()
                .filter(|&n| n >= 1 && usize::from(n) <= MAX_CLIENTS)
                .ok_or_else(|| at(c.span(), format!("capacity = {}: 1 to {MAX_CLIENTS}", c.get_ref())))?,
        ),
        None => None,
    };
    let seconds = |key: &str, v: &Option<Spanned<i64>>, (default, lo, hi): (i64, i64, i64)| match v {
        Some(v) if mode == Mode::Freeride => Err(at(v.span(), format!("{key}: freeride has no rounds"))),
        Some(v) if !(lo..=hi).contains(v.get_ref()) => {
            Err(at(v.span(), format!("{key} = {}: {lo} to {hi} seconds", v.get_ref())))
        }
        Some(v) => Ok(*v.get_ref() as u32 * TICK_HZ),
        None => Ok(default as u32 * TICK_HZ),
    };
    let rules = CycleRules {
        min_players: match &g.min_players {
            Some(v) if mode == Mode::Freeride => {
                return Err(at(v.span(), "min_players: freeride has no rounds".into()));
            }
            Some(v) => u16::try_from(*v.get_ref())
                .ok()
                .filter(|&n| n >= 1 && usize::from(n) <= MAX_CLIENTS)
                .ok_or_else(|| at(v.span(), format!("min_players = {}: 1 to {MAX_CLIENTS}", v.get_ref())))?,
            None => DEFAULT_MIN_PLAYERS as u16,
        },
        lobby_wait: seconds("lobby_wait", &g.lobby_wait, LOBBY_WAIT)?,
        load_timeout: seconds("load_timeout", &g.load_timeout, LOAD_TIMEOUT)?,
        countdown: seconds("countdown", &g.countdown, COUNTDOWN)?,
        round_limit: seconds("round_limit", &g.round_limit, ROUND_LIMIT)?,
        results: seconds("results_time", &g.results_time, RESULTS_TIME)?,
    };
    if let (Some(c), Some(m)) = (capacity, &g.min_players)
        && rules.min_players > c
    {
        return Err(at(m.span(), format!("min_players = {}: more than the capacity {c}", rules.min_players)));
    }
    let spec = InstanceSpec::on_level(rotation[0].clone(), mode, rewinds, ghosts, ghost_style)
        .map_err(|e| at(span, format!("[game]: {e}")))?;
    let rounds = mode.has_rounds().then_some((rules, rotation));
    Ok(GameConfig { spec, capacity, rounds })
}

pub type DropPlace = (String, Option<Range<usize>>);

pub fn drop_places(text: &str) -> Option<Vec<Vec<DropPlace>>> {
    #[derive(Deserialize)]
    struct Instances {
        game: Option<RawGame>,
        instance: Option<Vec<RawGame>>,
    }
    let raw: Instances = toml::from_str(text).ok()?;
    let games: Vec<RawGame> = match (raw.game, raw.instance) {
        (Some(g), None) => vec![g],
        (None, Some(list)) => list,
        (None, None) => return Some(vec![vec![(crate::config::DEFAULT_DROP.to_string(), None)]]),
        (Some(_), Some(_)) => return None,
    };
    let drops = |g: &RawGame| -> Vec<DropPlace> {
        match (&g.drop, &g.map, &g.rotation) {
            (Some(d), _, _) => vec![(d.get_ref().clone(), Some(d.span()))],
            (None, None, Some(r)) => r
                .get_ref()
                .iter()
                .filter_map(|e| match e.get_ref() {
                    toml::Value::String(name) if !name.contains(':') => Some((name.clone(), Some(e.span()))),
                    _ => None,
                })
                .collect(),
            (None, Some(_), _) => Vec::new(),
            (None, None, None) => vec![(crate::config::DEFAULT_DROP.to_string(), None)],
        }
    };
    Some(games.iter().map(drops).collect())
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::config::parse;

    fn err(text: &str) -> String {
        match parse(Path::new("server.toml"), text) {
            Err(e) => e.to_string(),
            Ok(_) => panic!("accepted: {text}"),
        }
    }

    #[test]
    fn rounds_rotations_and_instances() {
        let c = parse(
            Path::new("s.toml"),
            "[game]\nmode = \"race\"\nrotation = [\"exp_alps_vorassay_drop_3\", \"exp_rockies_x_drop_1\"]\n\
             min_players = 4\nlobby_wait = 0\nload_timeout = 30\ncountdown = 3\nround_limit = 300\nresults_time = 10\n",
        )
        .expect("a race");
        let g = &c.instances[0];
        assert_eq!((g.spec.mode, g.spec.level.name()), (Mode::Race, "exp_alps_vorassay_drop_3"));
        let (rules, rotation) = g.rounds.clone().expect("rounds");
        assert_eq!(
            rotation.iter().map(Level::name).collect::<Vec<_>>(),
            ["exp_alps_vorassay_drop_3", "exp_rockies_x_drop_1"]
        );
        assert_eq!(
            rules,
            CycleRules {
                min_players: 4,
                lobby_wait: 0,
                load_timeout: 30 * 30,
                countdown: 90,
                round_limit: 300 * 30,
                results: 10 * 30
            }
        );
        let d = parse(Path::new("s.toml"), "[game]\nmode = \"trick\"\ndrop = \"exp_alps_vorassay_drop_3\"\n").unwrap();
        let (rules, rotation) = d.instances[0].rounds.clone().expect("rounds");
        assert_eq!((rules.min_players, rules.countdown, rotation.len()), (1, 5 * 30, 1));
        assert!(d.instances[0].spec.mode.has_rounds());
        let two = parse(
            Path::new("s.toml"),
            "[server]\nmax_players = 12\n[[instance]]\nmode = \"survive\"\ncapacity = 8\n\
             [[instance]]\nmode = \"freeride\"\ndrop = \"exp_rockies_x_drop_1\"\n",
        )
        .expect("two instances");
        assert_eq!(two.instances.len(), 2);
        let net = two.instance_config();
        assert_eq!((net.instances[0].capacity, net.instances[1].capacity), (8, 12));
        assert!(net.instances[0].rounds.is_some() && net.instances[1].rounds.is_none());
        assert!(
            err("[game]\nmode = \"race\"\ncountdown = 2\n").starts_with("server.toml:3:13: countdown = 2: 3 to 60")
        );
        assert!(err("[game]\nmode = \"race\"\nround_limit = 7200\n").contains("30 to 3600"));
        assert!(err("[game]\nmode = \"race\"\nmin_players = 0\n").contains("min_players = 0"));
        assert!(err("[game]\nmode = \"race\"\nlobby_wait = -1\n").contains("lobby_wait"));
        assert!(err("[game]\nmode = \"race\"\nlobby_wait = 1.5\n").starts_with("server.toml:3:"));
        assert!(err("[game]\ncountdown = 5\n").contains("freeride has no rounds"));
        assert!(err("[game]\nmin_players = 2\n").contains("freeride has no rounds"));
        assert!(err("[game]\nrotation = [\"exp_alps_vorassay_drop_3\"]\n").contains("freeride rides one level"));
        assert!(err("[game]\nmode = \"race\"\nrotation = []\n").contains("1 to 32"));
        let many = (0..=MAX_ROTATION).map(|_| "\"a\"").collect::<Vec<_>>().join(", ");
        assert!(err(&format!("[game]\nmode = \"race\"\nrotation = [{many}]\n")).contains("1 to 32"));
        assert!(err("[game]\nmode = \"race\"\nrotation = [\"ok_drop\", \"Bad\"]\n").starts_with("server.toml:3:"));
        assert!(err("[game]\nmode = \"race\"\nrotation = [\"a\"]\ndrop = \"b\"\n").contains("not two of them"));
        assert!(err("[game]\ncapacity = 4\n").contains("max_players"));
        assert!(err("[game]\n[[instance]]\n").contains("not both"));
        assert!(
            err("[[instance]]\nmode = \"race\"\ncapacity = 2\nmin_players = 3\n").contains("more than the capacity")
        );
        assert!(err("[[instance]]\ncapacity = 65\n").contains("capacity = 65: 1 to 64"));
        let nine = "[[instance]]\n".repeat(MAX_INSTANCES + 1);
        assert!(err(&nine).contains("1 to 8 instances"));
        assert!(err("[[instance]]\nregion = \"eu\"\n").starts_with("server.toml:2:1"));
    }

    #[test]
    fn the_drops_places_follow_the_instances() {
        let text = "[[instance]]\nmode = \"race\"\nrotation = [\"one_drop\", \"a:map/b\", { map = \"c:map/d\", gamemode = \
                    \"c:gamemode/e\" }, \"two_drop\"]\n\n[[instance]]\ndrop = \"three_drop\"\n\n[[instance]]\nmode = \"freeride\"\n";
        let places = drop_places(text).expect("instances");
        let named = |i: usize| -> Vec<(&str, Option<&str>)> {
            places[i].iter().map(|(d, s)| (d.as_str(), s.clone().map(|s| &text[s]))).collect()
        };
        assert_eq!(named(0), [("one_drop", Some("\"one_drop\"")), ("two_drop", Some("\"two_drop\""))]);
        assert_eq!(named(1), [("three_drop", Some("\"three_drop\""))]);
        assert_eq!(named(2), [(crate::config::DEFAULT_DROP, None)]);
        assert_eq!(places.len(), 3);
        let one = drop_places("[game]\nmap = \"a:map/b\"\n").expect("a map");
        assert_eq!(one, [Vec::<DropPlace>::new()]);
        assert_eq!(
            drop_places("[server]\nname = \"x\"\n"),
            Some(vec![vec![(crate::config::DEFAULT_DROP.into(), None)]])
        );
        assert_eq!(drop_places("[game]\n[[instance]]\n"), None);
        assert_eq!(drop_places("[game\n"), None);
    }
}

#[cfg(test)]
mod map_tests {
    use std::path::Path;

    use super::*;
    use crate::config::parse;

    fn err(text: &str) -> String {
        match parse(Path::new("server.toml"), text) {
            Err(e) => e.to_string(),
            Ok(_) => panic!("accepted: {text}"),
        }
    }

    #[test]
    fn maps_in_instances_and_rotations() {
        let one = parse(Path::new("s.toml"), "[game]\nmode = \"race\"\nmap = \"slope:map/course\"\n").expect("a map");
        assert_eq!(one.game().level, Level::map("slope:map/course", 5).unwrap());
        assert_eq!(one.instances[0].rounds.as_ref().map(|r| r.1.len()), Some(1));
        let free = parse(Path::new("s.toml"), "[game]\nmap = \"slope:map/course\"\n").expect("a freeride map");
        assert_eq!(
            (free.game().mode, free.game().level.clone()),
            (Mode::Freeride, Level::map("slope:map/course", 8).unwrap())
        );
        for (mode, t) in [("race", 5), ("trick", 0), ("survive", 3), ("death", 4), ("freeride", 8)] {
            let c = parse(Path::new("s.toml"), &format!("[game]\nmode = \"{mode}\"\nmap = \"a:map/b\"\n")).expect(mode);
            assert_eq!(c.game().level, Level::map("a:map/b", t).unwrap(), "{mode}");
            assert_eq!(map_mode_name(t), Some(mode));
        }
        let mixed = parse(
            Path::new("s.toml"),
            "[game]\nmode = \"death\"\n\
             rotation = [\"slope:map/course\", \"exp_alaska_foraker_drop_4_survive\", \"lay1:map/layout\"]\n",
        )
        .expect("a mixed rotation");
        let (_, rotation) = mixed.instances[0].rounds.clone().expect("rounds");
        assert_eq!(
            rotation,
            [
                Level::map("slope:map/course", 4).unwrap(),
                Level::Drop("exp_alaska_foraker_drop_4_survive".into()),
                Level::map("lay1:map/layout", 4).unwrap()
            ]
        );
        assert_eq!(mixed.game().mode, Mode::Survive);
        for (text, says) in [
            ("[game]\nmode = \"race\"\nmap = \"slope:course\"\n", "server.toml:3:7: map = \"slope:course\""),
            ("[game]\nmode = \"race\"\nmap = \"slope:map/../x\"\n", "server.toml:3:7"),
            ("[game]\nmode = \"race\"\nmap = \"Slope:map/x\"\n", "server.toml:3:7"),
            ("[game]\nmode = \"race\"\nmap = \"slope:object/x\"\n", "not a map's id"),
            ("[game]\nmode = \"race\"\nmap = \"a:map/b\"\ndrop = \"c\"\n", "drop or map, not both"),
            ("[game]\nmode = \"race\"\nmap = \"a:map/b\"\nrotation = [\"c\"]\n", "not two of them"),
            ("[game]\nmode = \"race\"\nrotation = [\"ok_drop\", \"a:map/B\"]\n", "server.toml:3:"),
            ("[game]\nmode = \"chess\"\nmap = \"a:map/b\"\n", "\"death\" (survive to death)"),
            ("[game]\nmap = 3\n", "server.toml:2:"),
        ] {
            let e = err(text);
            assert!(e.contains(says), "{text:?}: {e}");
        }
        let long = format!("[game]\nmode = \"race\"\nmap = \"a:map/{}\"\n", "c".repeat(251));
        assert!(err(&long).contains("1 to 256"));
    }

    #[test]
    fn gamemodes_for_an_instance_and_a_rotations_maps() {
        let text = "[game]\nmode = \"race\"\nmap = \"slope:map/course\"\ngamemode = \"g:gamemode/bots\"\n";
        let one = parse(Path::new("s.toml"), text).expect("a map under a gamemode");
        assert_eq!(one.game().level, Level::map_under("slope:map/course", 5, Some("g:gamemode/bots")).unwrap());
        let mixed = parse(
            Path::new("s.toml"),
            "[game]\nmode = \"trick\"\ngamemode = \"g:gamemode/score\"\nrotation = [\"slope:map/course\", \
             { map = \"lay1:map/layout\", gamemode = \"g:gamemode/ghosts\" }, \"exp_alps_vorassay_drop_3\"]\n",
        )
        .expect("a rotation under gamemodes");
        let (_, rotation) = mixed.instances[0].rounds.clone().expect("rounds");
        assert_eq!(
            rotation,
            [
                Level::map_under("slope:map/course", 0, Some("g:gamemode/score")).unwrap(),
                Level::map_under("lay1:map/layout", 0, Some("g:gamemode/ghosts")).unwrap(),
                Level::Drop("exp_alps_vorassay_drop_3".into()),
            ]
        );
        let entry = |e: &str| format!("[game]\nmode = \"race\"\nrotation = [{e}]\n");
        for (text, says) in [
            (entry("{ map = \"a:map/b\" }"), "server.toml:3:13: rotation: an entry is"),
            (entry("{ map = \"a:map/b\", gamemode = \"g:gamemode/x\", x = 1 }"), "an entry is"),
            (entry("{ map = \"a:map/b\", gamemode = 3 }"), "an entry is"),
            (entry("3"), "an entry is"),
            (entry("{ map = \"a:map/b\", gamemode = \"g:map/x\" }"), "gamemode = \"g:map/x\""),
            (entry("{ map = \"a:map/b\", gamemode = \"G:gamemode/x\" }"), "server.toml:3:13"),
            ("[game]\nmode = \"race\"\ndrop = \"ok_drop\"\ngamemode = \"g:gamemode/x\"\n".into(), "rides no map"),
            ("[game]\nmode = \"race\"\nmap = \"a:map/b\"\ngamemode = \"g:object/x\"\n".into(), "server.toml:4:12"),
        ] {
            let e = err(&text);
            assert!(e.contains(says), "{text:?}: {e}");
        }
    }
}
