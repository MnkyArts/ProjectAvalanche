use std::collections::BTreeMap;

use super::{KIND_GEAR, Request, Store, StoreContext, StoreData, StoreItem};
use crate::career::profile::{ATTRIB_FLAGS, GeneratedItem, Profile};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CatalogEntry {
    pub kind: i64,
    pub slot: i64,
    pub cost: i64,
    pub locked: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Catalog {
    pub entries: BTreeMap<u32, CatalogEntry>,
}

impl Catalog {
    pub fn from_db(db: &crate::career::CareerDb) -> Self {
        let mut entries = BTreeMap::new();
        for i in &db.items {
            entries.insert(i.key, CatalogEntry { kind: i.kind, slot: i.slot, cost: i.cost, locked: i.locked });
        }
        for r in &db.riders {
            entries.insert(r.key, CatalogEntry { kind: 5, slot: -1, cost: r.cost, locked: r.locked });
        }
        Catalog { entries }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OfferPurchase {
    pub kind: usize,
    pub id: u32,
    pub clock_s: u32,
    pub now_ms: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PurchaseError {
    Unknown,
    AlreadyOwned,
    NotEnoughCredits { cost: i64, credits: i64 },
}

pub fn sell_price(cost: i64, percent: i64) -> i64 {
    ((percent * cost) as f32 * 0.01).floor() as i64
}

impl Profile {
    pub fn offered(&self, key: u32, e: &CatalogEntry) -> bool {
        !e.locked
            || self.owned_items.contains(&key)
            || (e.kind != 4 && (self.unlocked_riders.contains(&key) || self.unlocked_items.contains(&key)))
    }

    fn pay(&mut self, cost: i64) -> Result<(), PurchaseError> {
        if cost > self.credits() {
            return Err(PurchaseError::NotEnoughCredits { cost, credits: self.credits() });
        }
        self.add_credits(-cost, true);
        Ok(())
    }

    pub fn buy_fixed(&mut self, catalog: &Catalog, key: u32) -> Result<i64, PurchaseError> {
        let e = *catalog.entries.get(&key).ok_or(PurchaseError::Unknown)?;
        if self.owned_items.contains(&key) {
            return Err(PurchaseError::AlreadyOwned);
        }
        self.pay(e.cost)?;
        self.owned_items.insert(key);
        if e.kind == KIND_GEAR && e.slot >= 0 {
            self.add_gear_type(e.slot);
        }
        if e.kind == 5 {
            self.riders.entry(key).or_default();
        }
        Ok(e.cost)
    }

    pub fn add_gear_type(&mut self, slot: i64) {
        let f = self.attribs.entry(ATTRIB_FLAGS).or_insert(0);
        *f |= 1 << slot;
    }

    pub fn gear_types(&self) -> u32 {
        self.attribs.get(&ATTRIB_FLAGS).copied().unwrap_or(0) as u32
    }

    pub fn buy_offer(
        &mut self,
        store: &mut Store,
        data: &StoreData,
        ctx: &StoreContext,
        purchase: OfferPurchase,
    ) -> Result<u32, PurchaseError> {
        let OfferPurchase { kind, id, clock_s, now_ms } = purchase;
        let item = store.find(kind, id).cloned().ok_or(PurchaseError::Unknown)?;
        self.pay(item.cost)?;
        let mut row = GeneratedItem::from_offer(&item);
        if item.wall_id == -1 && item.special && matches!(item.kind, 1 | 2) {
            row.wall_id = i64::from(clock_s);
        }
        let equip_id = self.next_equip_id();
        row.equip_id = equip_id;
        self.generated.insert(equip_id, row);
        if item.kind == KIND_GEAR && item.slot >= 0 {
            self.add_gear_type(item.slot);
        }
        store.stock[kind].retain(|i| i.id != id);
        if let Some(o) = store.offered[kind].iter_mut().find(|o| **o == id) {
            *o = equip_id;
        }
        store.last_bought = Some((id, equip_id));
        store.generate(data, ctx, kind, &Request::NORMAL, now_ms);
        Ok(equip_id)
    }

    // TODO(guess): the query 0x83026718(table, EQUIPID, 1, …) is taken as sorted ascending, so the last row is the highest id (its run 0x830347F8 turns mode 1 into descriptor flag 0 for the select 0x83021A10 → 0x83021520 → 0x8301D5C8); settle: that flag's use in 0x8301D5C8's sort.
    pub fn next_equip_id(&self) -> u32 {
        self.generated.keys().next_back().map_or(0, |&k| k.wrapping_add(1))
    }

    pub fn sell_generated(&mut self, data: &StoreData, equip_id: u32) -> Option<i64> {
        let row = self.generated.remove(&equip_id)?;
        let price = sell_price(row.cost, data.tables.sell_percent);
        self.add_credits(price, false);
        Some(price)
    }

    pub fn sell_fixed(&mut self, catalog: &Catalog, data: &StoreData, key: u32) -> Option<i64> {
        let e = catalog.entries.get(&key)?;
        if !self.owned_items.remove(&key) {
            return None;
        }
        let price = sell_price(e.cost, data.tables.sell_percent);
        self.add_credits(price, false);
        Some(price)
    }
}

impl GeneratedItem {
    pub fn from_offer(o: &StoreItem) -> Self {
        GeneratedItem {
            equip_id: 0,
            kind: o.kind,
            slot: o.slot,
            level: o.level,
            rarity: o.rarity,
            cost: o.cost,
            charlock: o.charlock,
            descriptions: o.descriptions,
            stats: o.stats,
            name_format: o.name_format,
            wall_id: o.wall_id,
            sponsor: o.sponsor,
        }
    }
}
