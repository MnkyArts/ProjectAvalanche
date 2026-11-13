pub mod purchase;
#[cfg(test)]
mod tests;

use std::collections::BTreeMap;

use super::data::{Description, StoreTables, StoreTemplate};
use super::random::{GameRandom, Mt19937, Source};

pub(crate) trait StoreDraws: GameRandom {
    fn below_i(&mut self, n: i64) -> i64 {
        i64::from(self.below(n as u32))
    }

    fn pick(&mut self, n: usize) -> usize {
        if n > 1 { self.below(n as u32) as usize } else { 0 }
    }

    fn range(&mut self, lo: i64, hi: i64) -> i64 {
        if lo < hi { lo + self.below_i(hi - lo + 1) } else { lo }
    }
}

impl<T: GameRandom + ?Sized> StoreDraws for T {}

pub const KINDS: usize = 5;
pub const KIND_SUIT: i64 = 0;
pub const KIND_GEAR: i64 = 2;
pub const KIND_BOOST: i64 = 3;
pub const KIND_GEOTAG: i64 = 4;
pub const SLOT_ANY: i64 = 0xF;
pub const SLOT_SUIT_TOP: i64 = 0xC;
pub const SLOT_SUIT_BOTTOM: i64 = 0xD;
pub const FIRST_STOCK_ID: u32 = 1_000_000;
pub const OFFERS_PER_KIND: usize = 4;
pub const FIRST_SPECIAL_MS: i64 = 60_000;

pub fn stat_gear_type(stat: i64) -> Option<i64> {
    (5..=25).contains(&stat).then(|| (stat - 5) / 3 + 1)
}

#[derive(Clone, Debug, Default)]
pub struct StoreData {
    pub tables: StoreTables,
    pub templates: BTreeMap<u32, StoreTemplate>,
    pub descriptions: Vec<Description>,
}

impl StoreData {
    pub fn from_db(db: &super::CareerDb) -> Self {
        StoreData {
            tables: db.store.clone(),
            templates: db.templates.iter().map(|t| (t.key, t.clone())).collect(),
            descriptions: db.descriptions.clone(),
        }
    }

    pub fn group(&self, kind: i64) -> &[u32] {
        let g = self.tables.groups.get(kind as usize).copied().unwrap_or(0);
        self.templates.get(&g).map_or(&[], |t| t.members.as_slice())
    }

    fn description(&self, key: u32) -> Option<&Description> {
        self.descriptions.iter().find(|d| d.key == key)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StoreContext {
    pub rider: u32,
    pub level: i64,
    pub gear_types: u32,
    pub equipped_gear: u32,
    pub unk_gear_mask: u32,
    pub required_gear: u32,
    pub blocked: [bool; KINDS],
    pub geotags_online: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct StoreItem {
    pub id: u32,
    pub template: u32,
    pub kind: i64,
    pub slot: i64,
    pub level: i64,
    pub rarity: i64,
    pub charlock: u32,
    pub descriptions: [u32; 2],
    pub cost: i64,
    pub stats: [(i64, u8); 6],
    pub name_format: [i64; 4],
    pub special: bool,
    pub mystery: bool,
    pub wall_id: i64,
    pub sponsor: i64,
    pub duration_ms: i64,
    pub expires_ms: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Request {
    pub special: bool,
    pub charlock: u32,
    pub level: i64,
    pub min_rarity: i64,
    pub slot: i64,
}

impl Request {
    pub const NORMAL: Request = Request { special: false, charlock: 0, level: -1, min_rarity: 0, slot: SLOT_ANY };
}

#[derive(Clone, Debug)]
pub struct Store {
    pub stock: [Vec<StoreItem>; KINDS],
    pub offered: [Vec<u32>; KINDS],
    pub next_id: u32,
    pub special_due_ms: i64,
    pub last_bought: Option<(u32, u32)>,
    pub shown_special: Option<(usize, u32)>,
    pub rng: Source,
}

impl Store {
    pub fn new(seed: u32) -> Self {
        Self::with(Source::Own(Mt19937::new(seed)))
    }

    pub fn global() -> Self {
        Self::with(Source::Global)
    }

    fn with(rng: Source) -> Self {
        Store {
            stock: Default::default(),
            offered: Default::default(),
            next_id: FIRST_STOCK_ID,
            special_due_ms: FIRST_SPECIAL_MS,
            last_bought: None,
            shown_special: None,
            rng,
        }
    }

    pub fn find(&self, kind: usize, id: u32) -> Option<&StoreItem> {
        self.stock.get(kind)?.iter().find(|i| i.id == id)
    }

    pub fn show_special(&mut self, kind: usize, id: u32) {
        self.shown_special = Some((kind, id));
    }

    pub fn clear_shown_special(&mut self) {
        self.shown_special = None;
    }

    pub fn shown_special_left(&self, now_ms: i64) -> Option<i64> {
        let (kind, id) = self.shown_special?;
        if id < FIRST_STOCK_ID {
            return None;
        }
        let left = self.find(kind, id)?.expires_ms - now_ms;
        Some(left.max(0))
    }

    pub fn drop_expired_specials(&mut self, kind: usize, now_ms: i64) {
        if let Some(list) = self.stock.get_mut(kind) {
            list.retain(|i| !i.special || i.expires_ms >= now_ms);
        }
    }

    // TODO(guess): an expired item with the flag +0x33 also leaves its +0x4C in a 25-entry list at +0x6C (no reader traced); settle: the readers of the store's +0x6C…+0x70.
    pub fn purge(&mut self, force: bool, now_ms: i64) {
        self.offered = Default::default();
        self.next_id = FIRST_STOCK_ID;
        for list in &mut self.stock {
            list.retain(|i| i.expires_ms > now_ms && !(force && !i.special));
            for i in list.iter_mut() {
                i.id = self.next_id;
                self.next_id += 1;
            }
        }
    }

    pub fn offers(&self, data: &StoreData, ctx: &StoreContext, kind: usize) -> Vec<StoreItem> {
        let mut out = Vec::new();
        for &id in self.offered.get(kind).into_iter().flatten() {
            let Some(item) = self.find(kind, id) else { continue };
            let mut it = item.clone();
            if fits_rider(data, &mut it, ctx.rider) {
                out.push(it);
            }
        }
        out
    }

    pub fn refresh(&mut self, data: &StoreData, ctx: &StoreContext, now_ms: i64) {
        if ctx.rider == 0 {
            return;
        }
        let blocked = |k: usize| ctx.blocked[k] || (k == KIND_GEOTAG as usize && !ctx.geotags_online);
        for k in 0..KINDS {
            if blocked(k) {
                continue;
            }
            let mut shown = 0;
            for item in &mut self.stock[k] {
                if item.level == ctx.level && fits_rider(data, item, ctx.rider) {
                    self.offered[k].push(item.id);
                    shown += 1;
                }
            }
            for _ in shown..OFFERS_PER_KIND {
                self.generate(data, ctx, k, &Request::NORMAL, now_ms);
            }
        }
        if self.special_due_ms - now_ms < 0 {
            let k = self.rng.range(0, KINDS as i64 - 1) as usize;
            if !blocked(k) {
                let req = Request { special: true, min_rarity: data.tables.special_min_rarity, ..Request::NORMAL };
                self.generate(data, ctx, k, &req, now_ms);
            }
            let (lo, hi) = data.tables.special_interval;
            self.special_due_ms = now_ms + self.rng.range(lo * 60_000, hi * 60_000);
        }
    }

    pub fn update(&mut self, data: &StoreData, ctx: &StoreContext, force: bool, now_ms: i64) {
        self.purge(force, now_ms);
        self.refresh(data, ctx, now_ms);
    }
}

pub fn fits_rider(data: &StoreData, item: &mut StoreItem, rider: u32) -> bool {
    if rider == 0 || item.charlock == 0 || item.charlock == rider {
        return true;
    }
    let mut ok = true;
    let mut new = [0u32; 2];
    for (i, &d) in item.descriptions.iter().enumerate() {
        let Some(desc) = data.description(d) else { continue };
        if !ok {
            continue;
        }
        if desc.rider == 0 {
            new[i] = d;
            continue;
        }
        let slot = if i == 1 && item.slot == SLOT_SUIT_TOP { SLOT_SUIT_BOTTOM } else { item.slot };
        match data.descriptions.iter().find(|e| e.rider == rider && e.slot == slot && e.id == desc.id) {
            Some(e) => new[i] = e.key,
            None => ok = false,
        }
    }
    if ok {
        item.descriptions = new;
        item.charlock = rider;
    }
    ok
}

mod generate;
pub use generate::score;
