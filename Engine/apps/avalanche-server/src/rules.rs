use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use bevy::prelude::*;
use serde::Deserialize;
use ssx_gameplay::validate::{Action, Check, Limits, Policy};
use ssx_net::ServerStats;
use ssx_net::server::{PlayerSession, RiderLink, ValidationConfig, ValidationLog};
use ssx_net_proto::Mode;
use ssx_net_proto::instance::Level;
use ssx_rules::subset::{EventKind, RulesData, TuningLimits};
use toml::Spanned;

use crate::config::{Config, ConfigError};
use crate::resources::Settled;

mod configured;
pub use configured::{DropFinding, check_configured};

// PORT(choice): "big" below `--data` (the layout of the game's extraction), to be merged with the folder phase 4's
// `setup` writes (round net4a); the original has no dedicated server.
pub const DEFAULT_DATA_FOLDER: &str = "big";

// PORT(choice): 4 096 bytes (Linux's PATH_MAX), a port bound; the original has no dedicated server.
const MAX_FOLDER_PATH: usize = 4096;

#[derive(Clone)]
pub enum RulesSource {
    Config,
    Unchecked,
    Custom(ValidationConfig),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawData {
    folder: Option<Spanned<String>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawValidation {
    speed: Option<Spanned<String>>,
    step: Option<Spanned<String>>,
    splits: Option<Spanned<String>>,
    finish: Option<Spanned<String>>,
    clock: Option<Spanned<String>>,
    tricks: Option<Spanned<String>>,
}

type At<'a> = &'a dyn Fn(Range<usize>, String) -> ConfigError;

pub(crate) fn parse_data(raw: RawData, at: At) -> Result<PathBuf, ConfigError> {
    let Some(folder) = raw.folder else { return Ok(PathBuf::from(DEFAULT_DATA_FOLDER)) };
    let f = folder.get_ref();
    if f.is_empty() || f.len() > MAX_FOLDER_PATH || f.chars().any(char::is_control) {
        return Err(at(
            folder.span(),
            format!("folder: a path of 1 to {MAX_FOLDER_PATH} bytes, no control characters"),
        ));
    }
    Ok(PathBuf::from(f))
}

pub(crate) fn parse_validation(raw: RawValidation, at: At) -> Result<Policy, ConfigError> {
    let mut policy = Policy::default();
    let keys = [
        (Check::Speed, raw.speed),
        (Check::Step, raw.step),
        (Check::Splits, raw.splits),
        (Check::Finish, raw.finish),
        (Check::Clock, raw.clock),
        (Check::Tricks, raw.tricks),
    ];
    for (check, value) in keys {
        let Some(v) = value else { continue };
        let action = Action::parse(v.get_ref()).ok_or_else(|| {
            at(v.span(), format!("{} = {:?}: \"warn\", \"invalidate\" or \"kick\"", check.name(), v.get_ref()))
        })?;
        policy.actions[check.index()] = action;
    }
    Ok(policy)
}

pub fn kind_of(mode: Mode) -> EventKind {
    match mode {
        Mode::Freeride => EventKind::Any,
        Mode::Race => EventKind::Race,
        Mode::Trick => EventKind::Trick,
        Mode::Survive => EventKind::Survive,
    }
}

pub fn folder(config: &Config, data_dir: &Path) -> PathBuf {
    if config.data_folder.is_absolute() { config.data_folder.clone() } else { data_dir.join(&config.data_folder) }
}

pub fn prepare(
    config: &Config,
    data_dir: &Path,
    settled: Settled,
) -> (ValidationConfig, Arc<ServerRules>, crate::maps::ServerMaps) {
    let generation = settled.generation();
    let rules = Arc::new(ServerRules::new(folder(config, data_dir), settled));
    let maps = crate::maps::ServerMaps::new(Arc::clone(&rules));
    let map_loader = Arc::clone(&maps.loader);
    let drops = Arc::clone(&rules);
    let loader = Arc::new(move |level: &Level, mode: Mode| -> Result<Limits, String> {
        match level {
            Level::Drop(drop) => drops.drop_limits(drop, kind_of(mode)),
            Level::Map { id, event_type, gamemode } => map_loader.limits(id, *event_type, gamemode.as_deref()),
        }
    });
    (ValidationConfig { loader, policy: config.validation, generation }, rules, maps)
}

pub struct MountRules {
    pub generation: u64,
    pub data: RulesData,
    pub tuning: TuningLimits,
}

pub struct ServerRules {
    folder: PathBuf,
    pub settled: Settled,
    kept: Mutex<Option<Arc<MountRules>>>,
    said: Mutex<Said>,
}

#[derive(Default)]
struct Said {
    up: bool,
    waiting: Option<String>,
}

impl ServerRules {
    pub fn new(folder: PathBuf, settled: Settled) -> Self {
        Self { folder, settled, kept: Mutex::new(None), said: Mutex::new(Said::default()) }
    }

    pub fn log_is_up(&self) {
        let waiting = match self.said.lock() {
            Ok(mut s) => {
                s.up = true;
                s.waiting.take()
            }
            Err(_) => None,
        };
        if let Some(line) = waiting {
            info!("{line}");
        }
    }

    fn say(&self, line: String) {
        let now = match self.said.lock() {
            Ok(mut s) if !s.up => {
                s.waiting = Some(line);
                None
            }
            _ => Some(line),
        };
        if let Some(line) = now {
            info!("{line}");
        }
    }

    pub fn folder(&self) -> &Path {
        &self.folder
    }

    pub fn of_mount(&self, generation: u64) -> Result<Arc<MountRules>, String> {
        let kept = self.kept.lock().ok().and_then(|k| k.clone()).filter(|r| r.generation == generation);
        if let Some(r) = kept {
            return Ok(r);
        }
        let at = |e: anyhow::Error| {
            format!("[data] folder {}: {e:#} (`avalanche-server setup` writes it from the disc)", self.folder.display())
        };
        match std::fs::metadata(&self.folder) {
            Ok(m) if m.is_dir() => {}
            Ok(_) => return Err(at(anyhow::anyhow!("not a folder"))),
            Err(e) => return Err(at(e.into())),
        }
        let events = ssx_assets::content::data::events::ui_drop_records(&self.folder).map_err(at)?;
        let data = RulesData::with_events(&self.folder, events).map_err(at)?;
        let bundle = ssx_physics::tuning::load(&self.folder).map_err(at)?;
        let t = TuningLimits::from_bundle(&bundle).map_err(at)?;
        self.say(format!(
            "validation: the rules of the server's content (mount {generation}, the running resources' records \
             with it): {} events; the tuning's limits: fall {:.4} m/tick, horizontal {:.4} m/tick, spin {:.4}, flip \
             {:.4}, grind spin {:.4} rad/tick, trick point bonus {:.4}, helicopter shut-out {:.0} ticks",
            data.events.len(),
            t.fall,
            t.horizontal,
            t.spin,
            t.flip,
            t.grind_spin,
            t.trick_point_bonus,
            t.heli_shut_out
        ));
        let r = Arc::new(MountRules { generation, data, tuning: t });
        if let Ok(mut k) = self.kept.lock() {
            *k = Some(Arc::clone(&r));
        }
        Ok(r)
    }

    pub fn drop_limits(&self, drop: &str, kind: EventKind) -> Result<Limits, String> {
        let (_settled, generation) = self.settled.reading();
        let rules = self.of_mount(generation)?;
        let r = rules.data.drop_rules(&[(drop, kind)]).map_err(|e| format!("{e:#}"))?;
        let r = r.first().ok_or_else(|| format!("{drop}: no rules"))?;
        Limits::new(r, &rules.tuning)
    }

    pub fn check_drops(&self, config: &Config) -> Result<(), String> {
        let (_settled, generation) = self.settled.reading();
        let rules = self.of_mount(generation)?;
        for (k, g) in config.instances.iter().enumerate() {
            let kind = kind_of(g.spec.mode);
            let levels: Vec<&Level> = match &g.rounds {
                Some((_, rotation)) => rotation.iter().collect(),
                None => vec![&g.spec.level],
            };
            for d in levels.into_iter().filter_map(Level::as_drop) {
                match g.spec.mode {
                    Mode::Freeride => rules.data.check(d, kind).map(|_| ()),
                    _ => rules.data.check_start(d, kind).map(|_| ()),
                }
                .map_err(|e| format!("instance {}: {e:#}", k + 1))?;
            }
        }
        Ok(())
    }
}

pub fn log_validation(log: Option<ResMut<ValidationLog>>, sessions: Query<(&PlayerSession, &RiderLink)>) {
    let Some(mut log) = log else { return };
    let name = |rider: u16| {
        sessions
            .iter()
            .find(|(_, r)| r.rider().0 == rider)
            .map_or_else(|| format!("rider {rider}"), |(s, _)| s.name.clone())
    };
    for e in log.events.drain(..) {
        match (e.rider, e.check, e.action) {
            (None, ..) if e.instance == 0 => warn!("the server: {}", e.text),
            (None, ..) => info!("instance {} on {}: {}", e.instance, e.level, e.text),
            (Some(r), Some(c), Some(a)) => warn!(
                "instance {} on {}: {} failed the {} check ({}; {}): {}",
                e.instance,
                e.level,
                name(r),
                c.name(),
                a.name(),
                c.reason(),
                e.text
            ),
            (Some(r), check, _) => info!(
                "instance {} on {}: {}{}: {}",
                e.instance,
                e.level,
                name(r),
                check.map_or(String::new(), |c| format!(", the {} check", c.name())),
                e.text
            ),
        }
    }
}

pub fn status(stats: &ServerStats, out: &mut Vec<String>) {
    let checks: Vec<String> = Check::ALL
        .iter()
        .map(|c| {
            let o = stats.validation[c.index()];
            format!(
                "{} {}/{} (warned {}, invalidated {}, kicked {})",
                c.name(),
                o.found,
                o.made,
                o.warned,
                o.invalidated,
                o.kicked
            )
        })
        .collect();
    out.push(format!(
        "validation: {} runs; drops' rules loaded {}, refused {}; found / measured: {}; control messages held {} (at \
         most {} at once); samples before `Ready` {} (dropped {}, at most {} for a run, an earlier run's {}); frames \
         over 250 ms {} (the longest {} ms)",
        stats.rides_validated,
        stats.rules_loaded,
        stats.rules_failed,
        checks.join(", "),
        stats.control_held,
        stats.control_held_most,
        stats.samples_before_ready,
        stats.samples_before_ready_dropped,
        stats.samples_before_ready_most,
        stats.samples_before_ready_other_run,
        stats.frames_clamped,
        stats.frame_ms_most
    ));
}
