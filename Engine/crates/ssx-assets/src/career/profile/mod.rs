mod campaign;
pub mod mod_state;
pub mod range_movies;
mod results;

use std::collections::{BTreeMap, BTreeSet};

use ssx_formats::attrib::fnv1;
use ssx_formats::rnf::{CampaignGraph, NodeKind};

use super::data::DropType;
use super::globe::focus::{self, CampaignGlobe};
use super::world_tour::{self, MovieWhen};
use super::{CareerDb, UnlockKind};

pub use campaign::*;
pub use results::*;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GlobeProgression {
    pub from: u32,
    pub to: Vec<u32>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum DropState {
    #[default]
    Locked,
    Unlocked,
    Completed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Medal {
    Bronze,
    Silver,
    Gold,
}

impl Medal {
    pub fn from_rank(rank: usize) -> Option<Self> {
        [Medal::Gold, Medal::Silver, Medal::Bronze].get(rank).copied()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct EventRule {
    pub key: u32,
    pub name: String,
    pub drop_type: DropType,
    pub event_type: i64,
    pub reward: i64,
    pub places: Option<i64>,
    pub goal: Option<i64>,
    pub cost: i64,
    pub peak: u32,
    pub range: u32,
}

impl EventRule {
    pub fn ascending(&self) -> bool {
        matches!(self.event_type, 5..=7)
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Rules {
    pub graph: CampaignGraph,
    pub events: BTreeMap<u32, EventRule>,
    pub explore_rewards: Vec<[i64; 4]>,
    pub level_thresholds: Vec<i64>,
    pub level_prices: Vec<i64>,
    pub track_bonuses: BTreeMap<String, i64>,
    pub riders: BTreeMap<String, u32>,
    pub rider_costs: BTreeMap<u32, i64>,
    pub rider_names: BTreeMap<u32, String>,
    pub badge_texts: BTreeMap<u32, BadgeText>,
    pub track_bonus_texts: BTreeMap<String, BadgeText>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BadgeText {
    pub name: String,
    pub title: String,
    pub icon: String,
    pub description: String,
    pub category: i32,
    pub credits: i64,
    pub order: i32,
}

pub fn badge_texts(src: &[u8]) -> (BTreeMap<u32, BadgeText>, BTreeMap<String, BadgeText>) {
    use ssx_lua::game::Binder;
    use ssx_lua::game::graph::run_batch;
    let g = run_batch(Binder::Badges, &[("init_badges.lua".to_string(), src.to_vec())], None);
    let (mut badges, mut bonuses) = (BTreeMap::new(), BTreeMap::new());
    let mut order = 0;
    for c in &g.calls {
        let text = |i: usize| c.arg(i).str().unwrap_or_default().to_string();
        let int = |i: usize| ssx_lua::number::to_int(c.arg(i).num().unwrap_or(0.0));
        match c.name() {
            "InitBadge" => {
                let b = BadgeText {
                    name: text(1),
                    category: int(2),
                    title: text(3),
                    icon: text(4),
                    description: text(5),
                    credits: 0,
                    order,
                };
                order += 1;
                badges.insert(fnv1(b.name.as_bytes()), b);
            }
            "InitTrackBonus" => {
                let b = BadgeText {
                    name: text(1),
                    title: text(2),
                    icon: text(3),
                    description: text(4),
                    credits: i64::from(int(5)),
                    category: 0,
                    order: 0,
                };
                bonuses.insert(b.name.clone(), b);
            }
            _ => {}
        }
    }
    (badges, bonuses)
}

impl Rules {
    pub fn event(&self, key: u32) -> Option<&EventRule> {
        self.events.get(&key)
    }

    pub fn load_badge_texts(&mut self, big_root: &std::path::Path) -> anyhow::Result<()> {
        use anyhow::Context;
        let rel = "memresident/data/common/scripts/init_badges.lua";
        let src = std::fs::read(big_root.join(rel)).with_context(|| format!("reading {rel}"))?;
        (self.badge_texts, self.track_bonus_texts) = badge_texts(&src);
        Ok(())
    }

    pub fn paid_track_bonuses(&self, awarded: &[String]) -> Vec<&BadgeText> {
        let mut v: Vec<&BadgeText> = self.track_bonus_texts.values().filter(|b| awarded.contains(&b.name)).collect();
        v.sort_by_key(|b| fnv1(b.name.as_bytes()));
        v.sort_by_key(|b| std::cmp::Reverse(b.credits));
        v.truncate(PAID_TRACK_BONUSES);
        v
    }

    pub fn key_of(name: &str) -> u32 {
        fnv1(name.as_bytes())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RiderStats {
    pub money_earned: i64,
    pub next_level_offset: i64,
    pub max_achieved_level: i64,
    pub max_purchased_level: i64,
    pub last_suit: (u32, u8),
    pub last_board: (u32, u8),
    pub last_gear: [(u32, u8); 4],
    pub last_geotags: [(u32, u8); 4],
}

pub const SRC_NONE: u8 = 2;

impl Default for RiderStats {
    fn default() -> Self {
        RiderStats {
            money_earned: 0,
            next_level_offset: 0,
            max_achieved_level: 0,
            max_purchased_level: 0,
            last_suit: (0, SRC_NONE),
            last_board: (0, SRC_NONE),
            last_gear: [(0, SRC_NONE); 4],
            last_geotags: [(0, SRC_NONE); 4],
        }
    }
}

impl RiderStats {
    pub fn created_for(db: &crate::career::CareerDb, key: u32, exists: impl Fn(u32) -> bool) -> Self {
        let Some(r) = db.riders.iter().find(|r| r.key == key) else { return Self::created("", exists) };
        let name = match r.content.as_ref().filter(|c| c.added) {
            Some(c) => db.riders.iter().find(|s| s.key == c.stand_in).map_or("", |s| s.name.as_str()),
            None => r.name.as_str(),
        };
        let mut s = Self::created(name, &exists);
        if let Some(b) = r.content.as_ref().and_then(|c| c.default_board).filter(|&b| exists(b)) {
            s.last_board = (b, 0);
        }
        s
    }

    pub fn created(name: &str, exists: impl Fn(u32) -> bool) -> Self {
        let mut s = RiderStats::default();
        let suit = fnv1(format!("suit_default_{}", name.to_lowercase()).as_bytes());
        let board = fnv1(format!("board_{}_default", name.to_lowercase()).as_bytes());
        if exists(suit) {
            s.last_suit = (suit, 0);
        }
        if exists(board) {
            s.last_board = (board, 0);
        }
        s
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GeneratedItem {
    pub equip_id: u32,
    pub kind: i64,
    pub slot: i64,
    pub level: i64,
    pub rarity: i64,
    pub cost: i64,
    pub charlock: u32,
    pub descriptions: [u32; 2],
    pub stats: [(i64, u8); 6],
    pub name_format: [i64; 4],
    pub wall_id: i64,
    pub sponsor: i64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ExploreScore {
    pub score: u32,
    pub splits: [u32; 4],
    pub attempts: u32,
    pub deaths: u32,
    pub rewinds: u32,
    pub unk_score_modified: u32,
    pub unk_attempts_remaining: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Notice {
    DropUnlocked(u32),
    RangeOpened(String),
    RiderUnlocked(String),
    GearUnlocked(String),
    ChapterUnlocked(u32),
    ModesUnlocked,
    DropRemoved(String),
    Movie(String),
    LevelUp { rider: u32, level: i64 },
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DropCounts {
    pub completions: u32,
    pub failures: u32,
    pub unk_flag: u8,
}

pub const ATTRIB_CREDITS: u32 = u32::from_be_bytes(*b"BILL");
pub const ATTRIB_EARNED: u32 = u32::from_be_bytes(*b"EARN");
pub const ATTRIB_MPUR: u32 = u32::from_be_bytes(*b"MPUR");
pub const ATTRIB_CPRO: u32 = u32::from_be_bytes(*b"CPRO");
pub const ATTRIB_CTYP: u32 = u32::from_be_bytes(*b"CTYP");
pub const ATTRIB_LAST_RIDER: u32 = u32::from_be_bytes(*b"LURI");
pub const ATTRIB_FLAGS: u32 = u32::from_be_bytes(*b"GLFG");

pub const START_CREDITS: i64 = 4500;
pub const START_FLAG_BIT: u32 = 3;
pub const CAMPAIGN_REPLAY_CAP: i64 = 3000;
pub const FIRST_RUN_BONUS: i64 = 500;
pub const REPEAT_BONUS: i64 = 200;
pub const PAID_TRACK_BONUSES: usize = 3;
pub const RIDER_UNLOCK_STATS: [&str; 9] = ["Alex", "Elise", "Kaori", "Mac", "Moby", "Psymon", "Tane", "Tyler", "Zoe"];

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Profile {
    pub attribs: BTreeMap<u32, i64>,
    pub riders: BTreeMap<u32, RiderStats>,
    pub owned_items: BTreeSet<u32>,
    pub unlocked_riders: BTreeSet<u32>,
    pub unlocked_items: BTreeSet<u32>,
    pub generated: BTreeMap<u32, GeneratedItem>,
    pub collected_geotags: BTreeSet<u64>,
    pub explore_scores: BTreeMap<u32, ExploreScore>,
    pub drop_states: BTreeMap<u32, DropState>,
    pub peak_modes: BTreeMap<u32, [u32; 5]>,
    pub range_modes: BTreeMap<u32, [u32; 5]>,
    pub chapter_drops: Vec<u32>,
    pub drop_counts: BTreeMap<u32, DropCounts>,
    pub purchased: BTreeSet<u32>,
    pub medals: BTreeMap<u32, [u32; 3]>,
    pub movies: BTreeSet<String>,
    pub new_game_state: u32,
    pub globe_last_drop: u32,
    pub campaign_globe: CampaignGlobe,
    pub globe_progressions: Vec<GlobeProgression>,
    pub movie_queue: Vec<String>,
    pub game_mode: i64,
    pub stats: PersistentStats,
    pub recent_badges: Vec<u32>,
    pub named_flags: BTreeSet<u32>,
    pub mod_state: ssx_content::profile_state::ModState,
    pub mod_keys: BTreeMap<u32, mod_state::ModKey>,
    pub mod_disc_riders: BTreeMap<u32, String>,
}

pub const RECENT_BADGES: usize = 5;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PersistentStats {
    pub records: BTreeMap<u32, ssx_formats::save::chunks::StatRecord>,
    pub awards: BTreeMap<u32, u32>,
}

impl PersistentStats {
    pub fn awarded(&self, key: u32) -> bool {
        self.awards.get(&key).is_some_and(|w| w >> 24 != 0)
    }
}

impl Profile {
    pub fn new() -> Self {
        let mut p = Profile::default();
        p.attribs.insert(ATTRIB_CREDITS, START_CREDITS);
        p.attribs.insert(ATTRIB_LAST_RIDER, i64::from(fnv1(world_tour::NEW_GAME_RIDER.as_bytes())));
        p.attribs.insert(ATTRIB_FLAGS, 1 << START_FLAG_BIT);
        p
    }

    pub fn add_persistent_stat(&mut self, name: &str, n: u32) {
        let key = fnv1(name.as_bytes());
        let r = self.stats.records.entry(key).or_insert(ssx_formats::save::chunks::StatRecord {
            key,
            value: 0,
            key_again: key,
            scope: 2,
            kind: 0,
            initial: 0,
        });
        r.value = r.value.saturating_add(n);
    }

    // PORT(choice): for the port's own older saves, not the game's behaviour; to be removed once no such save is in
    // use; a console save never has these rows.
    pub fn own_campaign_riders(&mut self, rules: &Rules) {
        let nodes: Vec<(i32, Vec<String>)> = rules
            .graph
            .nodes
            .iter()
            .filter(|n| n.kind == NodeKind::UnlockChar)
            .map(|n| (n.id, n.attribs.clone()))
            .collect();
        for (id, names) in nodes {
            if !self.parents_completed(rules, id) {
                continue;
            }
            for a in names {
                let key = fnv1(a.as_bytes());
                if self.unlocked_riders.remove(&key) {
                    self.owned_items.insert(key);
                }
            }
        }
    }

    pub fn apply_unlockables(&mut self, db: &CareerDb) -> Vec<String> {
        let mut lines = Vec::new();
        for u in &db.unlockables {
            match u.kind {
                UnlockKind::Rider => {
                    self.unlocked_riders.insert(u.key);
                    if !self.riders.contains_key(&u.key) {
                        let created = {
                            let riders = &self.riders;
                            RiderStats::created_for(db, u.key, |k| riders.contains_key(&k))
                        };
                        self.riders.insert(u.key, created);
                    }
                    lines.push(format!("dlc: unlocked rider {} ({:#010x})", u.token, u.key));
                }
                UnlockKind::Geotag => {
                    self.owned_items.insert(u.key);
                    lines.push(format!("dlc: geotag {} ({:#010x}) in the inventory", u.token, u.key));
                }
                UnlockKind::Item => {
                    self.unlocked_items.insert(u.key);
                    lines.push(format!("dlc: item {} ({:#010x}) unlocked", u.token, u.key));
                }
                UnlockKind::Unknown => {}
            }
        }
        lines
    }

    pub fn credits(&self) -> i64 {
        self.attribs.get(&ATTRIB_CREDITS).copied().unwrap_or(0)
    }

    pub fn award_badge(&mut self, key: u32) -> bool {
        if self.stats.awarded(key) {
            return false;
        }
        let w = self.stats.awards.entry(key).or_insert(0);
        *w = (*w & 0x00FF_FFFF) | 1 << 24;
        self.recent_badges.insert(0, key);
        self.recent_badges.truncate(RECENT_BADGES);
        true
    }

    pub fn named_flag(&self, name: &str) -> bool {
        self.named_flags.contains(&fnv1(name.as_bytes()))
    }

    pub fn set_named_flag(&mut self, name: &str, value: bool) {
        let key = fnv1(name.as_bytes());
        if value {
            self.named_flags.insert(key);
        } else {
            self.named_flags.remove(&key);
        }
    }

    pub fn earned(&self) -> i64 {
        self.attribs.get(&ATTRIB_EARNED).copied().unwrap_or(0)
    }

    pub fn camera_type(&self) -> i32 {
        self.attribs.get(&ATTRIB_CTYP).map_or(-1, |&v| v as i32)
    }

    pub fn set_camera_type(&mut self, value: i32) {
        self.attribs.insert(ATTRIB_CTYP, i64::from(value));
    }

    pub fn drop_state(&self, drop: u32) -> DropState {
        self.drop_states.get(&drop).copied().unwrap_or_default()
    }

    pub fn add_credits(&mut self, amount: i64, earned: bool) {
        let max = i64::from(i32::MAX);
        let bill = match self.attribs.get(&ATTRIB_CREDITS) {
            None => amount.max(0),
            Some(&cur) if amount < 1 || cur <= max - amount => (cur + amount).max(0),
            Some(_) => max,
        };
        self.attribs.insert(ATTRIB_CREDITS, bill);
        if earned && amount > 0 {
            let earn =
                self.attribs.get(&ATTRIB_EARNED).map_or(amount, |&e| if max - amount <= e { max } else { e + amount });
            self.attribs.insert(ATTRIB_EARNED, earn);
        }
    }

    pub fn new_game(rules: &Rules) -> (Self, Vec<Notice>) {
        use super::new_game::{FlowAction, FrontEnd};
        let mut p = Profile::new();
        let mut notes = Vec::new();
        p.start_new_game();
        let fe = |state: u32| FrontEnd {
            hint_done: true,
            front_end_idle: true,
            movie_playing: state == 9,
            ..Default::default()
        };
        while p.new_game_state != 0xC && p.new_game_state != super::new_game::NEW_GAME_DONE {
            let f = fe(p.new_game_state);
            for a in p.new_game_update(rules, &f) {
                match a {
                    FlowAction::PlayMovie(m) => notes.push(Notice::Movie(m.to_string())),
                    FlowAction::GlobeProgression { from, to, flag } => {
                        p.campaign_progression(rules, from, &to, flag);
                        notes.extend(to.into_iter().map(Notice::DropUnlocked));
                    }
                    _ => {}
                }
            }
        }
        (p, notes)
    }

    pub fn unlock_drop(&mut self, rules: &Rules, drop: u32) {
        let Some(e) = rules.event(drop) else { return };
        let mode = match e.drop_type {
            DropType::Explore => 0,
            DropType::Campaign => 1,
            DropType::Type2 => 2,
            DropType::Custom => 3,
            DropType::None => return,
        };
        if e.peak == 0 || e.range == 0 {
            return;
        }
        let peak = self.peak_modes.entry(e.peak).or_default();
        peak[mode] = 1;
        if mode == 0 {
            peak[3] = 1;
        }
        let range = self.range_modes.entry(e.range).or_default();
        if range[mode] == 0 {
            range[mode] = 1;
        }
        if mode == 0 {
            range[3] = 1;
        } else if mode == 3 {
            return;
        }
        if self.drop_state(drop) == DropState::Locked {
            self.drop_states.insert(drop, DropState::Unlocked);
        }
    }

    pub fn unlock_explore(&mut self, rules: &Rules) {
        let keys: Vec<u32> = rules
            .events
            .values()
            .filter(|e| matches!(e.drop_type, DropType::Explore | DropType::Type2))
            .map(|e| e.key)
            .collect();
        for k in keys {
            self.unlock_drop(rules, k);
        }
    }
}
