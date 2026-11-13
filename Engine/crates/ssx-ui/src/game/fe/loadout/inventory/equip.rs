use super::*;

impl Inventory {
    pub fn slot_counts(&self, db: &CareerDb, profile: &Profile, drop: u32) -> [i64; KINDS] {
        let e = db.event(drop);
        if let Some(e) = e.filter(|e| !e.unk_98e9ebcc.is_empty()) {
            let mut out = [0; KINDS];
            for (o, &v) in out.iter_mut().zip(e.unk_98e9ebcc.iter()) {
                *o = v & 0xFF;
            }
            return out;
        }
        if self.rider == 0 {
            return [1; KINDS];
        }
        let level = Self::rider_level(db, profile, self.rider).0.max(0) as usize;
        let rows = &db.progression.level_slots;
        let row = rows.get(level.min(rows.len().saturating_sub(1))).copied().unwrap_or([1; 5]);
        let mut out = row.map(|v| i64::from(v.min(4)));
        if e.is_some_and(|e| e.drop_type == ssx_assets::career::DropType::Campaign) {
            out[3] = 0;
            out[4] = 0;
        }
        out
    }

    pub(super) fn last_equipped(&self, db: &CareerDb, profile: &Profile, kind: i64, slot: usize) -> Option<(u32, u8)> {
        let stats = &self.rider_stats(db, profile, self.rider);
        let (key, src) = match kind {
            0 => stats.last_suit,
            1 => stats.last_board,
            2 => *stats.last_gear.get(slot)?,
            4 => *stats.last_geotags.get(slot)?,
            _ => return None,
        };
        let owned = |k: u32| self.catalog.entries.get(&k).is_some_and(|e| profile.offered(k, e));
        let ok = match src {
            0 => owned(key),
            1 => profile.generated.contains_key(&key),
            _ => false,
        };
        if ok {
            return Some((key, src));
        }
        if kind == 1 {
            let name = db.riders.iter().find(|r| r.key == self.rider).map(|r| r.name.to_lowercase())?;
            return Some((fnv1(format!("board_{name}_default").as_bytes()), 0));
        }
        None
    }

    pub fn select_rider(&mut self, db: &CareerDb, profile: &Profile, drop: u32, rider: u32) {
        self.rider = rider;
        let info = DropInfo::of(db, profile, drop);
        let ctx = self.store_context(db, profile, info.as_ref());
        let data = self.data.clone();
        self.store.update(&data, &ctx, false, self.now_ms);
        self.equip_rider(db, profile, drop, rider);
        self.store.clear_shown_special();
    }

    pub fn equip_rider(&mut self, db: &CareerDb, profile: &Profile, drop: u32, rider: u32) {
        self.rider = rider;
        let slots = self.slot_counts(db, profile, drop);
        self.category_slots = slots.to_vec();
        self.worn_gear = 0;
        let forced = DropInfo::of(db, profile, drop);
        for kind in 0..KINDS {
            let mut list = Vec::new();
            for slot in 0..slots[kind] as usize {
                let item = self.last_equipped(db, profile, kind as i64, slot);
                list.push(item.unwrap_or((fnv1(EMPTY[kind].as_bytes()), 0)));
            }
            if let Some(f) = forced.as_ref().and_then(|d| d.forced(kind as i64)) {
                if list.is_empty() {
                    list.push((f, 0));
                } else {
                    list[0] = (f, 0);
                }
            }
            self.equipped[kind] = list;
        }
        self.update_worn(db, profile);
    }

    pub fn open_category(&mut self, db: &CareerDb, profile: &Profile, kind: i64, slot: usize) {
        if kind != 2 {
            return;
        }
        let Some(&(key, src)) = self.equipped[2].get(slot) else { return };
        if let Some(t) = self.slot_of(db, profile, key, src).filter(|t| (0..15).contains(t)) {
            self.worn_gear &= !(1u32 << t);
        }
    }

    pub fn close_category(&mut self, db: &CareerDb, profile: &Profile, kind: i64, slot: usize) {
        if kind != 2 {
            return;
        }
        let Some(&(key, src)) = self.equipped[2].get(slot) else { return };
        if let Some(t) = self.slot_of(db, profile, key, src).filter(|&t| t >= 0) {
            self.worn_gear |= 1u32 << (t & 31);
        }
    }

    pub(super) fn update_worn(&mut self, db: &CareerDb, profile: &Profile) {
        let mut mask = 0u32;
        for &(k, s) in &self.equipped[2] {
            if let Some(g) = self.slot_of(db, profile, k, s).filter(|&g| (0..32).contains(&g)) {
                mask |= 1 << g;
            }
        }
        self.worn_gear = mask;
    }

    pub fn equip(&mut self, db: &CareerDb, profile: &Profile, kind: i64, key: u32, src: u8, slot: usize) {
        self.store.clear_shown_special();
        if !(0..KINDS as i64).contains(&kind) {
            return;
        }
        let k = kind as usize;
        let ok = match src {
            0 => db.items.iter().any(|i| i.key == key && i.kind == kind),
            _ => profile.generated.get(&key).is_some_and(|g| g.kind == kind),
        };
        let item = if ok { (key, src) } else { (fnv1(EMPTY[k].as_bytes()), 0) };
        let max = self.category_slots.get(k).copied().unwrap_or(1).max(0) as usize;
        if slot < self.equipped[k].len() {
            self.equipped[k][slot] = item;
        } else if slot < max {
            self.equipped[k].push(item);
        }
        self.update_worn(db, profile);
    }

    pub fn kind_of(&self, db: &CareerDb, profile: &Profile, key: u32, src: u8) -> i64 {
        if src == 1 {
            let key = match self.store.last_bought {
                Some((stock, equip_id)) if stock == key => equip_id,
                _ => key,
            };
            if let Some(g) = profile.generated.get(&key) {
                return g.kind;
            }
            return self.store.stock.iter().flatten().find(|o| o.id == key).map_or(-1, |o| o.kind);
        }
        if let Some(i) = db.items.iter().find(|i| i.key == key) {
            return i.kind;
        }
        if db.riders.iter().any(|r| r.key == key) { RIDER } else { -1 }
    }

    pub fn equip_message(
        &mut self,
        db: &CareerDb,
        profile: &mut Profile,
        drop: u32,
        key: u32,
        src: u8,
        slot: i64,
    ) -> bool {
        let src = if src > 2 { 2 } else { src };
        let kind = self.kind_of(db, profile, key, src);
        let taken = slot >= 0
            && ((kind == RIDER && slot == 0)
                || (0..KINDS as i64).contains(&kind) && (slot as usize) < self.equipped[kind as usize].len());
        if !taken {
            return false;
        }
        if kind == RIDER {
            self.select_rider(db, profile, drop, key);
            self.save_slot(db, profile, RIDER, 0, (key, src));
            return true;
        }
        self.equip(db, profile, kind, key, src, slot as usize);
        if self.equipped[kind as usize].get(slot as usize) == Some(&(key, src)) {
            self.save_slot(db, profile, kind, slot as usize, (key, src));
        }
        true
    }

    // TODO(guess): the port writes every slot's row (the optimisation 0x82497A88 writes only the slots it changes); settle: port its per-slot equips (0x82497F2C / 0x8249819C).
    pub fn save_loadout(&self, db: &CareerDb, profile: &mut Profile) {
        for kind in 0..KINDS {
            for (slot, &item) in self.equipped[kind].iter().enumerate() {
                self.save_slot(db, profile, kind as i64, slot, item);
            }
        }
        self.save_slot(db, profile, RIDER, 0, (self.rider, 0));
    }

    pub fn consume_mods(&mut self, profile: &mut Profile) -> Vec<(u32, u8)> {
        let empty = fnv1(EMPTY[3].as_bytes());
        let mut consumed = Vec::new();
        for slot in 0..self.equipped[3].len() {
            let (key, src) = self.equipped[3][slot];
            self.equipped[3][slot] = (empty, 0);
            if key == empty && src == 0 {
                continue;
            }
            if src == 1 {
                self.remove_generated(profile, 3, key);
            } else {
                profile.owned_items.remove(&key);
            }
            consumed.push((key, src));
        }
        consumed
    }

    pub fn remove_generated(&mut self, profile: &mut Profile, kind: usize, key: u32) {
        let id = match self.store.last_bought {
            Some((stock, equip_id)) if stock == key => equip_id,
            _ => key,
        };
        let stock = id > 999_999 && self.store.stock.get(kind).is_some_and(|l| l.iter().any(|o| o.id == id));
        if !stock && !profile.generated.contains_key(&id) {
            return;
        }
        profile.generated.remove(&id);
        if let Some(shown) = self.store.offered.get_mut(kind)
            && let Some(i) = shown.iter().position(|&k| k == key)
        {
            shown.swap_remove(i);
        }
    }

    pub fn save_slot(&self, db: &CareerDb, profile: &mut Profile, kind: i64, slot: usize, item: (u32, u8)) {
        if item.1 == 0 && UNSAVED_ITEMS.contains(&item.0) {
            return;
        }
        if kind == RIDER {
            profile.attribs.insert(ssx_assets::career::profile::ATTRIB_LAST_RIDER, i64::from(item.0));
            return;
        }
        let count = self.category_slots.get(kind as usize).copied().unwrap_or(0).max(0) as usize;
        if !(0..KINDS as i64).contains(&kind) || kind == 3 || slot >= count {
            return;
        }
        let row = self.rider_stats(db, profile, self.rider);
        let stats = profile.riders.entry(self.rider).or_insert(row);
        let target = match kind {
            0 => Some(&mut stats.last_suit),
            1 => Some(&mut stats.last_board),
            2 => stats.last_gear.get_mut(slot),
            _ => stats.last_geotags.get_mut(slot),
        };
        if let Some(t) = target {
            *t = item;
        }
    }
}

pub const UNSAVED_ITEMS: [u32; 8] =
    [0x8650_5C81, 0x371D_D13C, 0xE318_0443, 0xC492_2193, 0x2002_1A25, 0x2039_B98F, 0x2636_71F3, 0xEED8_4B39];

#[cfg(test)]
mod tests {
    use ssx_assets::career::profile::GeneratedItem;

    use super::*;

    #[test]
    fn leaving_the_drop_consumes_the_mods() {
        let db = CareerDb::default();
        let mut inv = Inventory::new(&db);
        let mut profile = Profile::default();
        let empty = fnv1(EMPTY[3].as_bytes());
        inv.category_slots = vec![0, 0, 0, 3, 0];
        inv.equipped[3] = vec![(0x1234, 1), (empty, 0), (0xABCD, 0)];
        profile.generated.insert(0x1234, GeneratedItem { equip_id: 0x1234, kind: 3, ..GeneratedItem::default() });
        profile.owned_items.insert(0xABCD);
        inv.store.offered[3] = vec![0x1234, 0x5678, 0x9ABC];
        assert_eq!(inv.consume_mods(&mut profile), [(0x1234, 1), (0xABCD, 0)]);
        assert_eq!(inv.equipped[3], [(empty, 0), (empty, 0), (empty, 0)]);
        assert!(!profile.generated.contains_key(&0x1234), "the equip_generated row is gone");
        assert!(!profile.owned_items.contains(&0xABCD), "the equipment row is gone");
        assert_eq!(inv.store.offered[3], [0x9ABC, 0x5678], "the last shown entry took its place");
        assert!(inv.consume_mods(&mut profile).is_empty());
        assert_eq!(inv.equipped[3], [(empty, 0), (empty, 0), (empty, 0)]);
        assert_eq!(inv.store.offered[3], [0x9ABC, 0x5678]);
    }

    #[test]
    fn the_remove_finds_the_record_first() {
        let db = CareerDb::default();
        let mut inv = Inventory::new(&db);
        let mut profile = Profile::default();
        profile.generated.insert(7, GeneratedItem { equip_id: 7, kind: 3, ..GeneratedItem::default() });
        inv.store.last_bought = Some((1_000_042, 7));
        inv.store.offered[3] = vec![1_000_042, 1_000_043];
        inv.remove_generated(&mut profile, 3, 1_000_042);
        assert!(!profile.generated.contains_key(&7), "the alias reached the row");
        assert_eq!(inv.store.offered[3], [1_000_043]);
        inv.remove_generated(&mut profile, 3, 1_000_043);
        assert_eq!(inv.store.offered[3], [1_000_043], "no stock offer and no row: nothing removed");
    }
}
