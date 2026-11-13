use super::{
    KIND_BOOST, KIND_GEAR, KIND_GEOTAG, KIND_SUIT, Request, SLOT_ANY, SLOT_SUIT_BOTTOM, SLOT_SUIT_TOP, Store,
    StoreContext, StoreData, StoreDraws, StoreItem, fits_rider, stat_gear_type,
};

fn scale(value: i64, factor: f32) -> i64 {
    ((value as f32) * factor).floor() as i64
}

pub fn score(data: &StoreData, item: &StoreItem) -> i64 {
    let s = match item.kind {
        KIND_SUIT => data.tables.suit_score,
        KIND_GEOTAG => 5 * (item.level + 1) + item.rarity,
        _ => {
            let used: Vec<i64> = item.stats[..3].iter().filter(|s| s.0 != -1).map(|s| i64::from(s.1)).collect();
            if used.is_empty() {
                return 0;
            }
            (used.iter().sum::<i64>() as f32 / used.len() as f32).floor() as i64
        }
    };
    s.clamp(0, 100)
}

impl Store {
    fn choose_template(
        &mut self,
        data: &StoreData,
        ctx: &StoreContext,
        kind: i64,
        slot: i64,
        level: i64,
        rarity: i64,
    ) -> Option<u32> {
        let mut slot = slot;
        if kind == KIND_GEAR && slot == SLOT_ANY {
            let blocked = ctx.equipped_gear | ctx.unk_gear_mask;
            let have = ctx.gear_types | ctx.required_gear;
            let free: Vec<i64> = (0..15).filter(|&s| blocked & (1 << s) == 0 && have & (1 << s) != 0).collect();
            slot = if free.is_empty() { 3 } else { free[self.rng.pick(free.len())] };
        }
        let mut candidates = Vec::new();
        for key in data.group(kind) {
            let Some(t) = data.templates.get(key) else { continue };
            let fits = t.min_level <= level
                && level <= t.max_level
                && (slot == SLOT_ANY || t.slot == slot)
                && t.min_rarity <= rarity
                && rarity <= t.max_rarity;
            let gear_ok = kind != KIND_BOOST
                || t.stats.iter().find_map(|&s| stat_gear_type(s)).is_none_or(|g| ctx.equipped_gear & (1 << g) != 0);
            if fits && gear_ok {
                candidates.push(*key);
            }
        }
        if candidates.is_empty() {
            return None;
        }
        Some(candidates[self.rng.pick(candidates.len())])
    }

    fn choose_descriptions(&mut self, data: &StoreData, ctx: &StoreContext, item: &mut StoreItem) {
        if let Some(t) = data.templates.get(&item.template).filter(|t| !t.descriptions.is_empty()) {
            for (i, &d) in t.descriptions.iter().take(2).enumerate() {
                item.descriptions[i] = d;
            }
            if let Some(rider) = data.description(item.descriptions[0]).map(|d| d.rider).filter(|&r| r != 0) {
                item.charlock = rider;
            }
            fits_rider(data, item, ctx.rider);
            return;
        }
        let (mut tops, mut bottoms) = (Vec::new(), Vec::new());
        for d in &data.descriptions {
            if (d.rider == item.charlock || d.rider == 0) && !d.excluded {
                if d.slot == item.slot {
                    tops.push(d.key);
                }
                if item.slot == SLOT_SUIT_TOP && d.slot == SLOT_SUIT_BOTTOM {
                    bottoms.push(d.key);
                }
            }
        }
        if !tops.is_empty() {
            item.descriptions[0] = tops[self.rng.pick(tops.len())];
        }
        if item.slot == SLOT_SUIT_TOP && !bottoms.is_empty() {
            if self.rng.below_i(100) < data.tables.suit_match_chance {
                let id = data.description(item.descriptions[0]).map(|d| d.id);
                if let Some(b) = bottoms.iter().find(|&&b| data.description(b).map(|d| d.id) == id) {
                    item.descriptions[1] = *b;
                }
            } else {
                item.descriptions[1] = bottoms[self.rng.range(0, bottoms.len() as i64 - 1) as usize];
            }
        }
    }

    pub fn generate(
        &mut self,
        data: &StoreData,
        ctx: &StoreContext,
        kind: usize,
        req: &Request,
        now_ms: i64,
    ) -> Option<u32> {
        let tb = &data.tables;
        let mut item = StoreItem {
            level: if req.level < 0 { ctx.level } else { req.level },
            wall_id: -1,
            stats: [(-1, 0); 6],
            ..Default::default()
        };
        let roll = self.rng.below_i(1000);
        let mut cum = 0;
        for (i, w) in tb.rarity_weights.iter().enumerate() {
            cum += w;
            if roll < cum {
                item.rarity = i as i64;
                break;
            }
        }
        item.rarity = item.rarity.max(req.min_rarity);
        item.template = self.choose_template(data, ctx, kind as i64, req.slot, item.level, item.rarity)?;
        let t = data.templates.get(&item.template)?;
        item.id = self.next_id;
        self.next_id += 1;
        item.kind = t.kind;
        item.special = req.special;
        item.slot = t.slot;
        item.charlock = req.charlock;
        if req.charlock == 0 && self.rng.below_i(100) < t.charlock_chance && ctx.rider != 0 {
            item.charlock = ctx.rider;
        }
        item.mystery = req.special && self.rng.below_i(100) < tb.mystery_chance;
        let n = tb.durations.len();
        let d = if req.special { self.rng.pick(n) } else { n.saturating_sub(1) };
        item.duration_ms = tb.durations.get(d).copied().unwrap_or(0) * 60_000;
        item.expires_ms = now_ms + item.duration_ms;
        let li = (item.level.max(0) as usize).min(tb.stat_points.len().saturating_sub(1));
        let (min, max) = tb.stat_points.get(li).copied().unwrap_or((0, 0));
        let (mut lo, mut hi) = (min, min);
        for (i, &b) in tb.rarity_bands.iter().enumerate().take(5) {
            let part = b * (max - min + 1) / 100;
            hi += part;
            if i as i64 == item.rarity {
                break;
            }
            lo += part;
        }
        let lo = lo.max(min).min(max);
        let hi = hi.max(lo).min(max);
        let fixed = t.fixed_points > 0;
        let mut points = if fixed { t.fixed_points } else { self.rng.range(lo, hi) };
        let n = t.stats.len().min(6);
        for (i, &s) in t.stats.iter().take(6).enumerate() {
            item.stats[i] = (s, 0);
        }
        let cap = tb.stat_cap.min(100);
        if n >= 1 {
            let fav = self.rng.below_i(n as i64 + 1) as usize;
            let bump = if fav != n { 5 } else { 0 };
            let per = (100 / n as i64 - bump).max(0);
            let mut w: Vec<i64> = (0..n).map(|i| if i == fav { per + n as i64 * bump } else { per }).collect();
            let total: i64 = w.iter().sum();
            if total < 100 {
                w[fav.min(n - 1)] += 100 - total;
            }
            if !fixed {
                points = n as i64 * points / 3;
            }
            let mut range = 99;
            let mut full = 0;
            while points > 0 && full < n {
                full = 0;
                let r = if range > 0 { self.rng.below_i(range + 1) } else { 0 };
                let mut acc = 0;
                for (wi, stat) in w.iter_mut().zip(item.stats.iter_mut()) {
                    acc += *wi;
                    if i64::from(stat.1) < cap {
                        if r < acc {
                            stat.1 += 1;
                            points -= 1;
                            if i64::from(stat.1) >= cap {
                                range = (range - *wi).max(0);
                                *wi = 0;
                            }
                            break;
                        }
                    } else {
                        full += 1;
                    }
                }
            }
        }
        let mut bonus = false;
        if item.rarity >= tb.bonus_min_rarity && self.rng.below_i(100) < t.bonus_chance {
            if !t.bonus_stats.is_empty() {
                item.stats[5].0 = t.bonus_stats[self.rng.pick(t.bonus_stats.len())];
            }
            if item.stats[5].0 != -1 {
                let (bmin, bmax) = tb.bonus_points.get(li).copied().unwrap_or((0, 0));
                item.stats[5].1 = self.rng.range(bmin, bmax) as u8;
                bonus = item.stats[5].1 != 0;
                if !bonus {
                    item.stats[5].0 = -1;
                }
            }
        }
        let sc = score(data, &item);
        if item.kind < 5 {
            self.choose_descriptions(data, ctx, &mut item);
        }
        item.cost = self.price(data, &item, sc, li, bonus);
        for (i, &c) in tb.name_variants.iter().enumerate() {
            item.name_format[i] = self.rng.pick(c.max(0) as usize) as i64;
        }
        if kind as i64 == KIND_SUIT {
            item.level = 0;
        }
        let id = item.id;
        self.stock[kind].push(item);
        self.offered[kind].push(id);
        Some(id)
    }

    fn price(&mut self, data: &StoreData, item: &StoreItem, score: i64, level_index: usize, bonus: bool) -> i64 {
        let tb = &data.tables;
        let Some(t) = data.templates.get(&item.template) else { return 100 };
        let n = tb.prices.len().max(1) as i64;
        let idx = (n * score / 100).min(n - 1);
        let lo_pct = (100 / n) * idx;
        let hi_pct = ((idx + 1) * (100 / n)).min(100);
        let (pmin, pmax) = tb.prices.get(idx as usize).copied().unwrap_or((0, 0));
        let base = (pmax - pmin) * (score - lo_pct) / (hi_pct - lo_pct).max(1) + pmin;
        let si = if item.kind == KIND_SUIT { 5 } else { level_index };
        let spread = tb.price_spread.get(si.min(tb.price_spread.len().saturating_sub(1))).copied().unwrap_or(0);
        // TODO(guess): a geotag offer (only with the geotag server's configuration) is priced from the server's table (0x824408D0[level][rarity] ± spread); the port prices it like the others; settle: the server configuration (+0xBFD0), gone with EA's servers.
        let lo = (base - spread).max(pmin).min(pmax);
        let hi = (base + spread).max(lo).min(pmax);
        let mut price = self.rng.range(lo, hi).max(100);
        if item.kind != KIND_GEOTAG {
            price = scale(price, t.price_scale);
            if item.kind != KIND_SUIT {
                price = scale(price, tb.rarity_price.get(item.rarity as usize).copied().unwrap_or(0.0));
            }
            if bonus {
                price = scale(price, tb.bonus_price);
            }
        }
        (price / 100 * 100).max(100)
    }
}
