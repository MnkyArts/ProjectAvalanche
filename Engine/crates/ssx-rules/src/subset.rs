use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use glam::Vec3;
use ssx_formats::attrib::Value;

use crate::course::Course;
use crate::events::{EventType, UiDropRecord, load_ui_drops};
use crate::locators::{CLASS_DROP, DropStart, FIELD_TRACK, drop_starts, drop_trigger_volumes, end_locator};
use crate::triggers::{IG_DEATH, IG_PLAYER, TriggerVolume};
use crate::vault_db::{fnv1, single, with_attribdb, with_track_db};

pub const TRACK_VAULTS: &str = "cache1/data/xenon/attribdb";

pub const TUTORIAL_DROPS: [u32; 2] = [0xC145_699A, 0xB0F3_51A2];

const MAX_TRACK_NAME: usize = 32;

// PORT(choice): 1 024, a port limit over the disc's 61 track vaults, so that a damaged or hostile folder cannot make
// the start list without bound; tuning: the track count of the largest data a server is meant to read (a map's vaults
// are phase 9's); the original has no dedicated server.
const MAX_TRACK_VAULTS: usize = 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EventKind {
    Any,
    Race,
    Trick,
    Survive,
}

impl EventKind {
    pub fn fits(self, t: Option<EventType>) -> bool {
        match (self, t) {
            (Self::Any, _) => true,
            (_, None) => false,
            (Self::Race, Some(t)) => t.is_race(),
            (Self::Trick, Some(t)) => t.is_trick(),
            (Self::Survive, Some(t)) => t.is_survive(),
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Any => "freeride",
            Self::Race => "race",
            Self::Trick => "trick",
            Self::Survive => "survive",
        }
    }
}

#[derive(Clone, Debug)]
pub struct RulesData {
    pub folder: PathBuf,
    pub events: Vec<UiDropRecord>,
    pub tracks: BTreeMap<u32, String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DropRules {
    pub event: UiDropRecord,
    pub track: String,
    pub start: DropStart,
    pub end_locator: Vec3,
    pub volumes: Vec<TriggerVolume>,
    pub intro: crate::intro::DropIntro,
}

impl DropRules {
    pub fn start_y(&self) -> f32 {
        self.start.slot(0).map_or(0.0, |(p, _)| p.y)
    }

    pub fn course(&self) -> Course {
        Course { start_y: self.start_y(), end_y: self.end_locator.y }
    }

    pub fn end_volumes(&self) -> impl Iterator<Item = &TriggerVolume> {
        self.volumes.iter().filter(|v| v.name == "end")
    }

    pub fn death_volumes(&self) -> impl Iterator<Item = &TriggerVolume> {
        self.volumes.iter().filter(|v| v.groups & IG_DEATH != 0)
    }
}

fn is_folder(folder: &Path) -> Result<()> {
    let meta = std::fs::metadata(folder).with_context(|| format!("the data folder {}", folder.display()))?;
    if !meta.is_dir() {
        bail!("the data folder {}: not a folder", folder.display());
    }
    Ok(())
}

pub fn track_name(name: &str) -> bool {
    (1..=MAX_TRACK_NAME).contains(&name.len()) && name.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
}

impl RulesData {
    pub fn open(folder: &Path) -> Result<Self> {
        is_folder(folder)?;
        let events = load_ui_drops(folder).with_context(|| format!("the events of {}", folder.display()))?;
        Self::with_events(folder, events)
    }

    pub fn with_events(folder: &Path, events: Vec<UiDropRecord>) -> Result<Self> {
        is_folder(folder)?;
        let dir = folder.join(TRACK_VAULTS);
        let mut tracks = BTreeMap::new();
        let entries = std::fs::read_dir(&dir).with_context(|| format!("the track vaults {}", dir.display()))?;
        for e in entries {
            let e = e.with_context(|| format!("listing {}", dir.display()))?;
            let name = e.file_name();
            let Some(stem) = name.to_str().and_then(|n| n.strip_suffix("_track.vlt")) else { continue };
            if !track_name(stem) {
                continue;
            }
            if tracks.len() == MAX_TRACK_VAULTS {
                bail!("{}: more than {MAX_TRACK_VAULTS} track vaults", dir.display());
            }
            tracks.insert(fnv1(&format!("{stem}_track")), stem.to_string());
        }
        Ok(Self { folder: folder.to_path_buf(), events, tracks })
    }

    pub fn event(&self, name: &str) -> Option<&UiDropRecord> {
        let key = fnv1(name);
        self.events.iter().find(|e| e.key == key && e.name == name)
    }

    pub fn check(&self, name: &str, kind: EventKind) -> Result<&UiDropRecord> {
        let Some(e) = self.event(name) else {
            bail!("{name} is not a drop of the data folder {}", self.folder.display());
        };
        if TUTORIAL_DROPS.contains(&e.key) {
            bail!("{name} is a tutorial drop, which multiplayer does not play");
        }
        if !kind.fits(e.event_type) {
            let t = e
                .event_type
                .map_or("an event of no type".to_string(), |t| format!("a {} event ({})", t.kind_word(), t.name()));
            bail!("{name} is not a {} event: it is {t}", kind.name());
        }
        Ok(e)
    }

    pub fn check_start(&self, name: &str, kind: EventKind) -> Result<crate::intro::DropIntro> {
        crate::intro::drop_intro(&self.folder, self.check(name, kind)?)
    }

    pub fn drop_rules(&self, names: &[(&str, EventKind)]) -> Result<Vec<DropRules>> {
        let mut events = Vec::new();
        let mut intros = Vec::new();
        for &(name, kind) in names {
            let e = self.check(name, kind)?;
            intros.push(crate::intro::drop_intro(&self.folder, e)?);
            events.push(e.clone());
        }
        let keys: Vec<u32> = events.iter().map(|e| e.drop_key).collect();
        let track_keys = with_attribdb(&self.folder, |db| {
            keys.iter()
                .map(|&k| {
                    let r = match db.field(CLASS_DROP, k, FIELD_TRACK)?.and_then(|f| single(&f.data)) {
                        Some(Value::Ref(r)) => r.collection,
                        _ => 0,
                    };
                    Ok(r)
                })
                .collect::<Result<Vec<u32>>>()
        })?;
        let mut by_track: BTreeMap<String, Vec<usize>> = BTreeMap::new();
        for (i, (e, tk)) in events.iter().zip(&track_keys).enumerate() {
            let Some(track) = self.tracks.get(tk) else {
                bail!("{}: its ge_drop {:08x} names no track vault of the data folder", e.name, e.drop_key);
            };
            by_track.entry(track.clone()).or_default().push(i);
        }
        let mut out: Vec<Option<DropRules>> = vec![None; events.len()];
        for (track, list) in by_track {
            with_track_db(&self.folder, &track, |db| {
                let starts = drop_starts(db, &track)?;
                for &i in &list {
                    let e = &events[i];
                    let Some(start) = starts.iter().find(|s| s.drop_key == e.drop_key).cloned() else {
                        bail!("{}: its ge_drop {:08x} has no start locator on {track}", e.name, e.drop_key);
                    };
                    let Some(end) = end_locator(db, &track, e.drop_key)? else {
                        bail!("{}: its ge_drop {:08x} has no end locator on {track}", e.name, e.drop_key);
                    };
                    let volumes = drop_trigger_volumes(db, &track, Some(e.drop_key))?
                        .into_iter()
                        .filter(|v| v.interacts & IG_PLAYER != 0)
                        .collect();
                    out[i] = Some(DropRules {
                        event: e.clone(),
                        track: track.clone(),
                        start,
                        end_locator: end,
                        volumes,
                        intro: intros[i].clone(),
                    });
                }
                Ok(())
            })
            .with_context(|| format!("the rules of the drops on {track}"))?;
        }
        out.into_iter().zip(&events).map(|(r, e)| r.with_context(|| format!("{}: no rules", e.name))).collect()
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TuningLimits {
    pub fall: f32,
    pub horizontal: f32,
    pub spin: f32,
    pub flip: f32,
    pub grind_spin: f32,
    pub trick_point_bonus: f32,
    pub heli_shut_out: f32,
}

impl TuningLimits {
    pub fn load(folder: &Path) -> Result<Self> {
        let bundle = crate::tuning::Bundle::load(folder)?;
        Self::from_bundle(&bundle)
    }

    pub fn from_bundle(bundle: &crate::tuning::Bundle) -> Result<Self> {
        let (min, max) = crate::character::CharacterProperties::load_min_max(bundle)?;
        let most = |f: &str| -> Result<f32> {
            let (a, b) = (min.get(f)?, max.get(f)?);
            if !a.is_finite() || !b.is_finite() {
                bail!("{f}: {a} / {b}");
            }
            Ok(a.max(b))
        };
        let kmh = |f: &str| -> Result<f32> { Ok(most(f)? / crate::units::KMH_PER_M_PER_TICK) };
        let tuck = most("TuckedVelocityMultiplier")?.max(1.0);
        let fall = kmh("VerticalTerminalVelocity_Air")?
            .max(kmh("VerticalTerminalVelocity_Ground")? * tuck)
            .max(kmh("VerticalTerminalVelocity_Wingsuit")?);
        let horizontal = (kmh("HorizontalTerminalVelocity_Ground")? * tuck)
            .max(kmh("HorizontalTerminalVelocity_Air")?)
            .max(kmh("HorizontalTerminalVelocity_Wingsuit")?)
            .max(kmh("MaxBoostSpeed")?)
            .max(kmh("AwesomeMaxBoostSpeed")?);
        let rad = |fields: [&str; 3]| -> Result<f32> {
            let mut m = 0.0f32;
            for f in fields {
                m = m.max(most(f)?.to_radians());
            }
            Ok(m)
        };
        let spin = rad(["SpinMaxVelocity", "SpinMaxPrewindVelocity", "SpinMaxLaunchVelocity"])?;
        let flip = rad(["FlipMaxVelocity", "FlipMaxPrewindVelocity", "FlipMaxLaunchVelocity"])?;
        let grind_spin = bundle.view("carve_riding", "CarvingControllerAsset")?.f32("MaxSpinVel")?;
        let trick_point_bonus = most("TrickPointBonus")?;
        let heli_shut_out = bundle
            .view("Heli Interaction SceneOp", "HeliInteractionSceneOpAsset")?
            .f32("MinShutOutTimeBeforeNewInteraction")?;
        let out = Self { fall, horizontal, spin, flip, grind_spin, trick_point_bonus, heli_shut_out };
        for (name, v) in [
            ("fall", out.fall),
            ("horizontal", out.horizontal),
            ("spin", out.spin),
            ("flip", out.flip),
            ("grind spin", out.grind_spin),
            ("trick point bonus", out.trick_point_bonus),
            ("heli shut-out", out.heli_shut_out),
        ] {
            if !(v.is_finite() && v > 0.0) {
                bail!("the tuning's {name} limit is {v}");
            }
        }
        Ok(out)
    }
}
