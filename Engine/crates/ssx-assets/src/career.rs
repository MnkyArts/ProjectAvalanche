pub mod data;
pub mod geotags;
pub mod globe;
pub mod new_game;
pub mod profile;
pub mod random;
pub mod save;
pub mod store;
#[cfg(test)]
mod tests;
pub mod world_tour;

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use anyhow::{Context, Result};
use ssx_formats::attrib::fnv1;
use ssx_formats::csvdb::{Geotag, GhostRow};
use ssx_formats::rnf::{CampaignGraph, DropPositions};

pub use data::{
    Description, DropType, Event, ExploreRewards, Item, ItemContent, Peak, Progression, Range, Rider, RiderContent,
    StoreTables, StoreTemplate, UnlockKind, Unlockable,
};
pub use profile::{EventOutcome, EventRule, Medal, Notice, Profile, ResultReport, Rules};

#[derive(Clone, Debug, Default)]
pub struct CareerDb {
    pub ranges: Vec<Range>,
    pub peaks: Vec<Peak>,
    pub drops: Vec<data::Drop>,
    pub events: Vec<Event>,
    pub riders: Vec<Rider>,
    pub items: Vec<Item>,
    pub templates: Vec<StoreTemplate>,
    pub store: StoreTables,
    pub descriptions: Vec<Description>,
    pub progression: Progression,
    pub explore_rewards: Vec<ExploreRewards>,
    pub graph: CampaignGraph,
    pub drop_positions: DropPositions,
    pub ghosts: Vec<GhostRow>,
    pub geotags: BTreeMap<String, Vec<Geotag>>,
    pub track_bonuses: Vec<(String, i64)>,
    pub badges: Vec<(String, i32)>,
    pub boost_max: Vec<u8>,
    pub default_equipment: Vec<u32>,
    pub unlockables: Vec<Unlockable>,
    pub unlock_problems: Vec<String>,
    pub unknown_classes: Vec<(u32, usize)>,
}

pub type BadgeDefinitions = (Vec<(String, i32)>, Vec<(String, i64)>);

pub fn badge_definitions(src: &[u8]) -> BadgeDefinitions {
    use ssx_lua::game::Binder;
    use ssx_lua::game::graph::run_batch;
    let g = run_batch(Binder::Badges, &[("init_badges.lua".to_string(), src.to_vec())], None);
    let (mut badges, mut bonuses) = (Vec::new(), Vec::new());
    for c in &g.calls {
        let text = |i: usize| c.arg(i).str().unwrap_or_default().to_string();
        let int = |i: usize| ssx_lua::number::to_int(c.arg(i).num().unwrap_or(0.0));
        match c.name() {
            "InitBadge" => badges.push((text(1), int(2))),
            "InitTrackBonus" => bonuses.push((text(1), i64::from(int(5)))),
            _ => {}
        }
    }
    (badges, bonuses)
}

impl CareerDb {
    pub fn load(big_root: &Path) -> Result<Self> {
        let mut db = Self::read(big_root, true)?;
        crate::content::characters::career::apply(big_root, &mut db);
        Ok(db)
    }

    pub fn load_disc(big_root: &Path) -> Result<Self> {
        Self::read(big_root, false)
    }

    fn read(big_root: &Path, routed: bool) -> Result<Self> {
        let mut v = if routed { data::Vaults::load(big_root)? } else { data::Vaults::load_disc(big_root)? };
        let read = |rel: &str| std::fs::read(big_root.join(rel)).with_context(|| format!("reading {rel}"));
        let graph = CampaignGraph::parse(&read("bootcache/data/common/fe/campaign.rnf")?)?;
        for a in graph.nodes.iter().flat_map(|n| &n.attribs) {
            v.dict.insert(a);
        }
        let drop_positions = DropPositions::parse(&read("fecache/data/common/fe/droppos.rnf")?)?;
        let ghost_text = if routed {
            crate::content::data::events::ghost_db_text(big_root)
        } else {
            std::fs::read_to_string(big_root.join(crate::content::data::events::GHOST_DB))
        };
        let ghosts = ssx_formats::csvdb::parse_ghosts(&ghost_text.context("reading ghostdb.csv")?)?;
        let mut geotags = BTreeMap::new();
        let mut seen: BTreeSet<String> = BTreeSet::new();
        for path in crate::data_roots::files_in(big_root, "data/common/geocaches", "csv") {
            let name = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
            let Some(track) = name.strip_suffix("_track_geotagdb.csv") else { continue };
            let track = track.to_string();
            if !seen.insert(name) {
                continue;
            }
            let text = std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
            geotags.insert(track, ssx_formats::csvdb::parse_geotags(&text)?);
        }
        let (badges, track_bonuses) = badge_definitions(&read("memresident/data/common/scripts/init_badges.lua")?);
        let (unlockables, unlock_problems) = if routed {
            data::unlockables(&v, crate::content::dlc::unlockables_from_roots(&crate::content::dlc::active_roots()))
        } else {
            (Vec::new(), Vec::new())
        };
        let unknown_classes = v.unknown.iter().map(|(&k, &n)| (k, n)).collect();
        Ok(CareerDb {
            ranges: data::ranges(&v)?,
            peaks: data::peaks(&v)?,
            drops: data::drops(&v)?,
            events: data::events(&v)?,
            riders: data::riders(&v)?,
            items: data::items(&v)?,
            templates: data::templates(&v)?,
            store: data::store_tables(&v)?,
            descriptions: data::descriptions(&v)?,
            progression: data::progression(&v)?,
            explore_rewards: data::explore_rewards(&v)?,
            graph,
            drop_positions,
            ghosts,
            geotags,
            track_bonuses,
            badges,
            boost_max: data::boost_max(&v)?,
            default_equipment: data::default_equipment(&v)?,
            unlockables,
            unlock_problems,
            unknown_classes,
        })
    }

    pub fn event(&self, key: u32) -> Option<&Event> {
        self.events.iter().find(|e| e.key == key)
    }

    pub fn event_named(&self, name: &str) -> Option<&Event> {
        self.event(fnv1(name.as_bytes())).filter(|e| e.name == name)
    }

    pub fn drop(&self, key: u32) -> Option<&data::Drop> {
        self.drops.iter().find(|d| d.key == key)
    }

    pub fn peak(&self, key: u32) -> Option<&Peak> {
        self.peaks.iter().find(|p| p.key == key)
    }

    pub fn range(&self, key: u32) -> Option<&Range> {
        self.ranges.iter().find(|r| r.key == key)
    }

    pub fn peak_and_range(&self, e: &Event) -> (u32, u32) {
        let peak = if e.peak_ref != 0 { e.peak_ref } else { self.drop(e.drop).map_or(0, |d| fnv1(d.peak.as_bytes())) };
        (peak, self.peak(peak).map_or(0, |p| p.range))
    }

    pub fn rivals(&self, event: u32) -> impl Iterator<Item = &GhostRow> {
        self.ghosts.iter().filter(move |g| g.ui_drop == event)
    }

    pub fn ghost_board(&self, e: &Event) -> Vec<ssx_rules::results::GhostEntry> {
        let t = u32::try_from(e.event_type).ok().and_then(ssx_rules::events::EventType::from_raw);
        ssx_rules::results::ghost_board(self.rivals(e.key).map(|g| (g.value(), g.character)), t)
    }

    pub fn medal_targets(&self, e: &Event) -> Vec<u32> {
        self.ghost_board(e).iter().map(|g| g.value).collect()
    }

    pub fn rules(&self) -> Rules {
        let mut rules = Rules {
            graph: self.graph.clone(),
            explore_rewards: self
                .explore_rewards
                .iter()
                .map(|t| [t.bronze, t.silver, t.gold, t.per_place_passed])
                .collect(),
            level_thresholds: self.progression.level_thresholds.clone(),
            level_prices: self.progression.level_prices.clone(),
            track_bonuses: self.track_bonuses.iter().cloned().collect(),
            riders: self.riders.iter().map(|r| (r.name.clone(), r.key)).collect(),
            rider_costs: self.riders.iter().map(|r| (r.key, r.cost)).collect(),
            rider_names: self.riders.iter().map(|r| (r.key, r.display_name.clone())).collect(),
            ..Default::default()
        };
        for e in &self.events {
            let (peak, range) = self.peak_and_range(e);
            rules.events.insert(
                e.key,
                EventRule {
                    key: e.key,
                    name: e.name.clone(),
                    drop_type: e.drop_type,
                    event_type: e.event_type,
                    reward: e.reward,
                    places: e.places,
                    goal: e.targets.first().copied(),
                    cost: e.cost,
                    peak,
                    range,
                },
            );
        }
        rules
    }
}
