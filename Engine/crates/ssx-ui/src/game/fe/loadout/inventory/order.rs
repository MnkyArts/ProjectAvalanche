use std::collections::BTreeSet;

use super::lists::ListInputs;
use super::*;

pub(super) fn requirement_met_by_slot(slot: i64, requirement: i64) -> bool {
    match requirement {
        0..=6 => requirement_gear(requirement).and_then(|(g, _)| g) == Some(slot),
        7 => slot == 0,
        _ => false,
    }
}

const GEAR_ORDER: [i64; 7] = [1, 3, 2, 4, 5, 7, 6];

fn slot_weight(drop: Option<&DropInfo>, slot: i64) -> i64 {
    let Some(d) = drop else { return 0 };
    d.required
        .iter()
        .position(|&r| requirement_met_by_slot(slot, r))
        .and_then(|i| d.weights.get(i).copied())
        .unwrap_or(0)
}

pub(super) fn by_value(a: &UiItem, b: &UiItem) -> std::cmp::Ordering {
    (b.sort, b.rarity).cmp(&(a.sort, a.rarity))
}

fn move_to(list: &mut Vec<UiItem>, i: usize, at: usize) {
    let it = list.remove(i);
    list.insert(at.min(list.len()), it);
}

impl Inventory {
    pub(super) fn order_list(
        &self,
        db: &CareerDb,
        profile: &Profile,
        loc: &mut Localization,
        drop: Option<&DropInfo>,
        kind: i64,
        list: &mut Vec<UiItem>,
    ) {
        list.sort_by(by_value);
        match kind {
            2 => {
                let mut order = GEAR_ORDER;
                order.sort_by_key(|&s| std::cmp::Reverse(slot_weight(drop, s)));
                for &s in order.iter().rev() {
                    if let Some(i) = list.iter().position(|u| u.slot == s) {
                        move_to(list, i, 0);
                    }
                }
            }
            3 => self.lead_boosts(db, profile, loc, drop, list),
            _ => {}
        }
    }

    fn lead_boosts(
        &self,
        db: &CareerDb,
        profile: &Profile,
        loc: &mut Localization,
        drop: Option<&DropInfo>,
        list: &mut Vec<UiItem>,
    ) {
        let Some(d) = drop.filter(|d| !d.required.is_empty()) else { return };
        let worn = |slot: i64| (0..32).contains(&slot) && self.worn_gear & (1 << slot) != 0;
        let mut need: BTreeSet<i64> = BTreeSet::new();
        for (i, &r) in d.required.iter().enumerate() {
            if r == 7 || d.weights.get(i).copied().unwrap_or(0) <= 0 {
                continue;
            }
            if let Some(slot) = (0..15).find(|&s| requirement_met_by_slot(s, r))
                && worn(slot)
            {
                need.insert(slot);
            }
        }
        for &(k, s) in &self.equipped[3] {
            if let Some(rec) = self.record(db, profile, loc, k, s)
                && let Some(g) = stat_gear(rec.stats[0].0)
            {
                need.remove(&g);
            }
        }
        let mut front = Vec::new();
        let mut i = 0;
        while !need.is_empty() && i < list.len() {
            if stat_gear(list[i].stats[0].0).is_some_and(|g| need.remove(&g)) {
                front.push(list.remove(i));
            }
            i += 1;
        }
        front.sort_by(by_value);
        let lead = front.len();
        list.splice(0..0, front);
        if d.survive()
            && let Some(i) = (lead..list.len()).find(|&i| list[i].stats.iter().any(|s| s.0 == 0x20))
        {
            move_to(list, i, lead);
        }
    }

    pub(super) fn build_store_front(&mut self, s: ListInputs, loc: &mut Localization, kind: i64, refresh: bool) {
        let ListInputs { db, profile, drop, ctx } = s;
        let request_level = if ctx.level < self.level_limit { ctx.level } else { self.level_limit };
        let min_level = if request_level == -1 { ctx.level } else { request_level };
        let best = |list: &[UiItem], f: &dyn Fn(&UiItem) -> bool| {
            let (mut found, mut top) = (None, -1);
            for (i, u) in list.iter().enumerate() {
                if f(u) && top < u.quality {
                    (found, top) = (Some(i), u.quality);
                }
            }
            found
        };
        let front = if refresh {
            usize::from(self.store_list.first().is_some_and(|u| u.has(flag::SPECIAL)))
        } else {
            let special = best(&self.store_list, &|u| u.has(flag::SPECIAL) && !u.has(flag::RECOMMENDED));
            let recommended = best(&self.store_list, &|u| u.has(flag::RECOMMENDED) && u.level >= min_level - 1);
            let lead = match recommended {
                Some(i) if self.recommend_first => Some(i),
                _ => special,
            };
            if let Some(i) = lead {
                move_to(&mut self.store_list, i, 0);
            }
            let front = usize::from(lead.is_some());
            let mut i = front;
            while i < self.store_list.len() {
                if self.store_list[i].has(flag::SPECIAL) {
                    self.store_list.remove(i);
                } else {
                    i += 1;
                }
            }
            if kind == 0 {
                dedupe_suits(&mut self.store_list);
            }
            front
        };
        if kind == 2
            && let Some(slot) = drop.and_then(DropInfo::main_gear)
        {
            match self.store_list.iter().position(|u| u.slot == slot) {
                Some(i) => {
                    if !refresh && i > 0 {
                        move_to(&mut self.store_list, i, front);
                    }
                }
                None => {
                    let req = Request { level: request_level, slot, ..Request::NORMAL };
                    let id = self.store.generate(&self.data, ctx, 2, &req, self.now_ms);
                    if let Some(rec) = id.and_then(|id| self.record(db, profile, loc, id, 1)) {
                        let item = self.ui_item(db, profile, loc, drop, &rec);
                        let at = if refresh { self.store_list.len() } else { front.min(self.store_list.len()) };
                        self.store_list.insert(at, item);
                    }
                }
            }
        }
        // PORT(choice): a port limit — the port stops after STORE_FILL_DRAWS draws; the game's fill loop
        // (0x8249419C…0x82494398) draws again until the list holds four (an offer that fails, or a boost with sort
        // value 0, is drawn again; without end when none fits): a hang has no value to match.
        let Some(k) = usize::try_from(kind).ok().filter(|&k| k < KINDS) else { return };
        let mut draws = 0;
        while self.store_list.len() < 4 && draws < STORE_FILL_DRAWS {
            draws += 1;
            let req = Request { level: request_level, ..Request::NORMAL };
            let id = self.store.generate(&self.data, ctx, k, &req, self.now_ms);
            let Some(rec) = id.and_then(|id| self.record(db, profile, loc, id, 1)) else { continue };
            let item = self.ui_item(db, profile, loc, drop, &rec);
            if kind != 3 || item.sort > 0 {
                self.store_list.push(item);
                if kind == 0 {
                    dedupe_suits(&mut self.store_list);
                }
            }
        }
    }
}

const STORE_FILL_DRAWS: usize = 32;

fn dedupe_suits(list: &mut Vec<UiItem>) {
    let same = |a: &UiItem, b: &UiItem| {
        a.level == b.level && a.stats == b.stats && a.asset == b.asset && a.secondary == b.secondary
    };
    let mut i = 0;
    while i < list.len() && i < 4 {
        let first = list[i].clone();
        let mut j = i + 1;
        while j < list.len() {
            if same(&first, &list[j]) {
                list.remove(j);
            } else {
                j += 1;
            }
        }
        i += 1;
    }
}

#[cfg(test)]
mod tests {
    use ssx_assets::career::{CareerDb, Profile};

    use super::*;
    use crate::game::loc::{Language, Localization};

    fn entry(key: u32, slot: i64, sort: i64, quality: i64, flags: u32) -> UiItem {
        UiItem { key, slot, sort, quality, flags, asset: format!("a{key}"), ..Default::default() }
    }

    fn keys(list: &[UiItem]) -> Vec<u32> {
        list.iter().map(|u| u.key).collect()
    }

    #[test]
    fn the_special_leads_and_the_main_gear_follows_it() {
        let db = CareerDb::default();
        let profile = Profile::default();
        let mut loc = Localization::empty(Language::English);
        let mut inv = Inventory::new(&db);
        let drop = DropInfo { required: vec![0], weights: vec![80], ..Default::default() };
        let ctx = inv.store_context(&db, &profile, Some(&drop));
        inv.store_list = vec![
            entry(1, 3, 50, 5, 0),
            entry(2, 3, 40, 3, flag::SPECIAL),
            entry(3, 1, 30, 1, 0),
            entry(4, 4, 20, 7, flag::SPECIAL),
        ];
        inv.build_store_front(
            ListInputs { db: &db, profile: &profile, drop: Some(&drop), ctx: &ctx },
            &mut loc,
            2,
            false,
        );
        assert_eq!(
            keys(&inv.store_list),
            [4, 3, 1],
            "special 4 (quality 7) first, wingsuit 3 after it, special 2 gone"
        );

        inv.store_list = vec![entry(1, 3, 50, 5, 0), entry(3, 1, 30, 1, 0)];
        inv.build_store_front(
            ListInputs { db: &db, profile: &profile, drop: Some(&drop), ctx: &ctx },
            &mut loc,
            2,
            false,
        );
        assert_eq!(keys(&inv.store_list), [3, 1], "no special: the wingsuit first");

        inv.store_list = vec![entry(1, 3, 50, 5, flag::SPECIAL), entry(5, 3, 10, 2, flag::RECOMMENDED)];
        inv.recommend_first = false;
        inv.build_store_front(ListInputs { db: &db, profile: &profile, drop: None, ctx: &ctx }, &mut loc, 1, false);
        assert_eq!(keys(&inv.store_list), [1, 5]);
        inv.store_list = vec![entry(1, 3, 50, 5, flag::SPECIAL), entry(5, 3, 10, 2, flag::RECOMMENDED)];
        inv.recommend_first = true;
        inv.build_store_front(ListInputs { db: &db, profile: &profile, drop: None, ctx: &ctx }, &mut loc, 1, false);
        assert_eq!(keys(&inv.store_list), [5], "the recommended entry leads, the special is erased");

        inv.store_list = vec![
            entry(2, 3, 40, 3, flag::SPECIAL),
            entry(1, 3, 50, 5, 0),
            entry(3, 1, 30, 1, 0),
            entry(4, 4, 20, 7, flag::SPECIAL),
        ];
        let s = ListInputs { db: &db, profile: &profile, drop: Some(&drop), ctx: &ctx };
        inv.build_store_front(s, &mut loc, 2, true);
        assert_eq!(keys(&inv.store_list), [2, 1, 3, 4]);
    }

    #[test]
    fn a_gear_list_opens_with_one_entry_per_type_by_the_drops_weights() {
        let db = CareerDb::default();
        let profile = Profile::default();
        let mut loc = Localization::empty(Language::English);
        let inv = Inventory::new(&db);
        let list = || vec![entry(1, 2, 90, 0, 0), entry(2, 2, 80, 0, 0), entry(3, 3, 70, 0, 0), entry(4, 1, 10, 0, 0)];
        let mut l = list();
        inv.order_list(&db, &profile, &mut loc, None, 2, &mut l);
        assert_eq!(keys(&l), [4, 3, 1, 2], "no drop: types 1, 3, 2 in that order, then the rest by value");
        let drop = DropInfo { required: vec![1], weights: vec![60], ..Default::default() };
        let mut l = list();
        inv.order_list(&db, &profile, &mut loc, Some(&drop), 2, &mut l);
        assert_eq!(keys(&l), [1, 4, 3, 2], "the weighted type first");
    }

    #[test]
    fn suits_lose_the_copies_of_their_first_four() {
        let suit = |key, level| UiItem { key, level, asset: "x".into(), ..Default::default() };
        let mut l = vec![suit(1, 0), suit(2, 1), suit(3, 0), suit(4, 1), suit(5, 2)];
        dedupe_suits(&mut l);
        assert_eq!(keys(&l), [1, 2, 5]);
    }
}
