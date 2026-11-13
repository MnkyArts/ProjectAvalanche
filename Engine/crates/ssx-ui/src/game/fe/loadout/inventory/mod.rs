use ssx_assets::career::store::purchase::Catalog;
use ssx_assets::career::store::{Request, Store, StoreContext, StoreData, StoreItem};
use ssx_assets::career::{CareerDb, Profile};
use ssx_formats::attrib::fnv1;

use crate::game::loc::Localization;

mod drop_info;
mod equip;
mod generated;
mod lists;
mod order;
mod score;

pub use drop_info::*;
pub use equip::*;
pub use generated::*;

pub const KINDS: usize = 5;
pub const RIDER: i64 = 5;
pub const EMPTY: [&str; KINDS] = ["suit_empty", "board_empty", "gear_empty", "boost_empty", "geotag_empty"];

pub mod flag {
    pub const AVAILABLE: u32 = 0x1;
    pub const PURCHASEABLE: u32 = 0x2;
    pub const SHOW_STATS: u32 = 0x4;
    pub const SELLABLE: u32 = 0x8;
    pub const RECOMMENDED: u32 = 0x10;
    pub const SPECIAL: u32 = 0x20;
    pub const LEVEL_PURCHASEABLE: u32 = 0x40;
    pub const NEW: u32 = 0x80;
    pub const EMPTY: u32 = 0x100;
    pub const EQUIPPED: u32 = 0x200;
    pub const MYSTERY: u32 = 0x400;
    pub const SHOW_LEVEL: u32 = 0x800;
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Record {
    pub owned: bool,
    pub special: bool,
    pub mystery: bool,
    pub recommended: bool,
    pub cost: i64,
    pub level: i64,
    pub rarity: i64,
    pub rider: u32,
    pub descriptions: [u32; 2],
    pub key: u32,
    pub src: u8,
    pub kind: i64,
    pub name: String,
    pub slot: i64,
    pub stats: [(i64, u8); 6],
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct UiItem {
    pub name: String,
    pub description: String,
    pub asset: String,
    pub secondary: String,
    pub recommend: String,
    pub gamertag: String,
    pub cost: i64,
    pub level: i64,
    pub level_progress: i64,
    pub sort: i64,
    pub quality: i64,
    pub rarity: i64,
    pub slot: i64,
    pub key: u32,
    pub src: u8,
    pub flags: u32,
    pub stats: [(i64, i64); 3],
}

impl UiItem {
    pub fn has(&self, f: u32) -> bool {
        self.flags & f != 0
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct RiderBio {
    pub gear_asset: String,
    pub gear_name: String,
    pub perks: Vec<(String, i64)>,
}

#[derive(Clone, Debug)]
pub struct Inventory {
    pub catalog: Catalog,
    pub data: StoreData,
    pub store: Store,
    pub now_ms: i64,
    pub rider: u32,
    pub equipped: [Vec<(u32, u8)>; KINDS],
    pub worn_gear: u32,
    pub gear_mask: u32,
    pub level_limit: i64,
    pub last_drop: u32,
    pub recommend_first: bool,
    pub new_items: [Vec<(u32, u8)>; KINDS + 1],
    pub owned_list: Vec<UiItem>,
    pub store_list: Vec<UiItem>,
    pub inventory_list: Vec<UiItem>,
    pub selected_gear: Vec<UiItem>,
    pub category_slots: Vec<i64>,
    pub survival: i32,
    pub inventory_size: i32,
    pub bio: RiderBio,
}

impl Inventory {
    pub fn new(db: &CareerDb) -> Self {
        let mut inv = Self::empty(db);
        let boot = Profile::default();
        if let Some(&rider) = db.default_equipment.get(RIDER as usize).filter(|&&r| r != 0) {
            inv.equip_rider(db, &boot, 0, rider);
            for (kind, &key) in db.default_equipment.iter().take(KINDS).enumerate() {
                if key != fnv1(EMPTY[kind].as_bytes()) {
                    inv.equip(db, &boot, kind as i64, key, 0, 0);
                }
            }
        }
        inv
    }

    fn empty(db: &CareerDb) -> Self {
        Inventory {
            catalog: Catalog::from_db(db),
            data: StoreData::from_db(db),
            store: Store::global(),
            now_ms: 0,
            rider: 0,
            equipped: Default::default(),
            worn_gear: 0,
            gear_mask: 0,
            level_limit: -1,
            last_drop: 0,
            recommend_first: true,
            new_items: Default::default(),
            owned_list: Vec::new(),
            store_list: Vec::new(),
            inventory_list: Vec::new(),
            selected_gear: Vec::new(),
            category_slots: Vec::new(),
            survival: 0,
            inventory_size: 0,
            bio: RiderBio::default(),
        }
    }

    pub fn rider_level(db: &CareerDb, profile: &Profile, rider: u32) -> (i64, i64) {
        let mut stats = profile.riders.get(&rider).cloned().unwrap_or_default();
        let p = &db.progression;
        ssx_assets::career::profile::update_rider_level(&mut stats, &p.level_thresholds, &p.level_prices)
    }

    pub fn store_context(&self, db: &CareerDb, profile: &Profile, drop: Option<&DropInfo>) -> StoreContext {
        StoreContext {
            rider: self.rider,
            level: Self::rider_level(db, profile, self.rider).0,
            gear_types: profile.gear_types(),
            equipped_gear: self.worn_gear,
            unk_gear_mask: self.gear_mask,
            required_gear: drop.map_or(0, DropInfo::required_gear),
            blocked: drop.map_or([false; KINDS], DropInfo::forced_kinds),
            geotags_online: false,
        }
    }

    pub fn fixed_record(&self, db: &CareerDb, profile: &Profile, loc: &mut Localization, key: u32) -> Option<Record> {
        let owned = |k: u32| self.catalog.entries.get(&k).is_some_and(|e| profile.offered(k, e));
        if let Some(i) = db.items.iter().find(|i| i.key == key) {
            let mut stats = [(-1i64, 0u8); 6];
            for (s, &(t, v)) in stats.iter_mut().zip(i.attributes.iter()) {
                *s = (i64::from(t), v);
            }
            return Some(Record {
                owned: owned(key),
                cost: i.cost,
                level: i.level,
                rarity: i.quality,
                rider: i.rider,
                descriptions: [
                    i.descriptions.first().copied().unwrap_or(0),
                    i.descriptions.get(1).copied().unwrap_or(0),
                ],
                key,
                src: 0,
                kind: i.kind,
                name: if i.title.is_empty() { String::new() } else { loc.text(&i.title) },
                slot: i.slot,
                stats,
                ..Default::default()
            });
        }
        let r = db.riders.iter().find(|r| r.key == key)?;
        Some(Record {
            owned: owned(key),
            cost: r.cost,
            descriptions: [key, 0],
            key,
            src: 0,
            kind: RIDER,
            name: loc.text(&r.title),
            slot: -1,
            stats: [(-1, 0); 6],
            ..Default::default()
        })
    }
}
