use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BuyError {
    UnknownDrop,
    NotEnoughCredits { cost: i64, credits: i64 },
}

impl Profile {
    fn node_drop_key(node: &ssx_formats::rnf::Node) -> u32 {
        node.attribs.first().map_or(0, |a| fnv1(a.as_bytes()))
    }

    pub(super) fn parents_completed(&self, rules: &Rules, id: i32) -> bool {
        rules.graph.parents(id).all(|p| {
            rules.graph.node(p).is_some_and(|n| self.drop_state(Self::node_drop_key(n)) == DropState::Completed)
        })
    }

    pub(super) fn fire_movies(&mut self, drop: u32, success: Option<bool>, notes: &mut Vec<Notice>) {
        for (movie, d, when) in world_tour::MOVIES {
            if fnv1(d.as_bytes()) != drop {
                continue;
            }
            let fires = matches!(
                (when, success),
                (MovieWhen::Success, Some(true))
                    | (MovieWhen::Failure, Some(false))
                    | (MovieWhen::Any, Some(_))
                    | (MovieWhen::Unlocked, None)
            );
            if fires {
                self.movies.insert(movie.to_string());
                self.movie_queue.push(movie.to_string());
                notes.push(Notice::Movie(movie.to_string()));
            }
        }
    }

    pub fn campaign_progression(&mut self, rules: &Rules, from: u32, to: &[u32], flag: bool) {
        self.campaign_globe.progress(rules, to);
        if flag {
            self.globe_progressions.push(GlobeProgression { from, to: to.to_vec() });
        }
        self.globe_last_drop = focus::return_node(from);
    }

    pub fn unlock_chapter(&mut self, rules: &Rules, chapter: u32) {
        for name in world_tour::chapter_drops(chapter) {
            let k = fnv1(name.as_bytes());
            if !self.chapter_drops.contains(&k) {
                self.chapter_drops.push(k);
            }
        }
        let h2h = fnv1(format!("chapter_{chapter}_drop_h2h").as_bytes());
        if let Some(range) = rules.event(h2h).map(|e| e.range).filter(|&r| r != 0) {
            self.range_modes.entry(range).or_default()[1] = 2;
        }
    }

    pub fn complete_campaign_drop(&mut self, rules: &Rules, drop: u32) -> Vec<Notice> {
        let mut notes = Vec::new();
        let Some(name) = rules.event(drop).map(|e| e.name.clone()) else { return notes };
        if let Some(id) = rules.graph.drop_node(&name).map(|n| n.id) {
            self.complete_node(rules, id, &mut notes);
        }
        notes
    }

    fn complete_node(&mut self, rules: &Rules, id: i32, notes: &mut Vec<Notice>) {
        let Some(node) = rules.graph.node(id).cloned() else { return };
        let children: Vec<i32> = rules.graph.children(id).collect();
        let recurse = |p: &mut Self, notes: &mut Vec<Notice>| {
            for &c in &children {
                p.complete_node(rules, c, notes);
            }
        };
        match node.kind {
            NodeKind::Drop => {
                let drop = Self::node_drop_key(&node);
                self.drop_states.insert(drop, DropState::Completed);
                let mut unlocked = Vec::new();
                for &c in &children {
                    let Some(child) = rules.graph.node(c) else { continue };
                    if child.kind != NodeKind::Drop {
                        self.complete_node(rules, c, notes);
                        continue;
                    }
                    let key = Self::node_drop_key(child);
                    if rules.event(key).is_some() && self.parents_completed(rules, c) {
                        unlocked.push(key);
                        self.unlock_drop(rules, key);
                        notes.push(Notice::DropUnlocked(key));
                        self.fire_movies(key, None, notes);
                    }
                }
                let tutorial = [world_tour::PROLOGUE_DROP, world_tour::FIRST_DROP].map(|n| fnv1(n.as_bytes()));
                let flag = !tutorial.contains(&drop) || !self.new_game_active();
                self.campaign_progression(rules, drop, &unlocked, flag);
            }
            NodeKind::Unlock => {
                for a in &node.attribs {
                    self.unlock_drop(rules, fnv1(a.as_bytes()));
                }
                recurse(self, notes);
            }
            NodeKind::OpenRange => {
                if self.parents_completed(rules, id) {
                    notes.push(Notice::RangeOpened(node.attribs.first().cloned().unwrap_or_default()));
                }
            }
            NodeKind::UnlockGear => {
                for a in &node.attribs {
                    if self.owned_items.insert(fnv1(a.as_bytes())) {
                        notes.push(Notice::GearUnlocked(a.clone()));
                    }
                }
                recurse(self, notes);
            }
            NodeKind::UnlockChar => {
                for a in &node.attribs {
                    let key = fnv1(a.as_bytes());
                    if !rules.riders.values().any(|&k| k == key) || !self.owned_items.insert(key) {
                        continue;
                    }
                    self.riders.entry(key).or_default();
                    if let Some(name) = rules.rider_names.get(&key).filter(|n| RIDER_UNLOCK_STATS.contains(&n.as_str()))
                    {
                        self.add_persistent_stat(&format!("RiderUnlock_{name}"), 1);
                    }
                    notes.push(Notice::RiderUnlocked(a.clone()));
                }
                recurse(self, notes);
            }
            NodeKind::UnlockChapter => {
                let n: u32 = node.attribs.first().and_then(|a| a.parse().ok()).unwrap_or(0);
                self.unlock_chapter(rules, n);
                notes.push(Notice::ChapterUnlocked(n));
            }
            NodeKind::UnlockMode => notes.push(Notice::ModesUnlocked),
            NodeKind::RemoveDrop => notes.push(Notice::DropRemoved(node.attribs.first().cloned().unwrap_or_default())),
        }
    }

    pub fn skip_campaign_drop(&mut self, rules: &Rules, drop: u32) -> Vec<Notice> {
        let mut notes = Vec::new();
        if rules.event(drop).is_none() {
            return notes;
        }
        self.drop_counts.entry(drop).or_default().completions += 1;
        self.globe_last_drop = drop;
        if self.drop_state(drop) == DropState::Unlocked {
            notes = self.complete_campaign_drop(rules, drop);
        }
        self.fire_movies(drop, Some(true), &mut notes);
        self.drop_counts.entry(drop).or_default().completions += 1;
        notes
    }

    pub fn buy_drop(&mut self, rules: &Rules, drop: u32) -> Result<i64, BuyError> {
        let e = rules.event(drop).ok_or(BuyError::UnknownDrop)?;
        let cost = if self.purchased.contains(&drop) || e.cost == 0 { 0 } else { e.cost };
        if self.credits() < cost {
            return Err(BuyError::NotEnoughCredits { cost, credits: self.credits() });
        }
        if cost > 0 {
            self.purchased.insert(drop);
            self.add_credits(-cost, true);
        }
        Ok(cost)
    }

    pub fn explore_menu_enabled(&self) -> bool {
        self.drop_state(fnv1(world_tour::MENUS_DROP.as_bytes())) == DropState::Completed
    }

    pub fn deadly_descents_completed(&self) -> usize {
        world_tour::DEADLY_DESCENTS
            .iter()
            .filter(|d| self.drop_state(fnv1(d.as_bytes())) == DropState::Completed)
            .count()
    }

    pub fn world_tour_finished(&self) -> bool {
        self.drop_state(fnv1(world_tour::FINAL_DROP.as_bytes())) == DropState::Completed
    }
}

pub fn update_rider_level(stats: &mut RiderStats, thresholds: &[i64], prices: &[i64]) -> (i64, i64) {
    let n = thresholds.len();
    let price = |i: usize| prices.get(i).copied().unwrap_or(0);
    let (money, purchased) = (stats.money_earned, stats.max_purchased_level);
    let (mut level, mut percent) = (0, 0);
    for i in 0..n {
        let mut hi = thresholds[i];
        let mut lo = if i == 0 { 0 } else { thresholds[i - 1] };
        let gate = i + 1 < n && price(i + 1) != 0;
        let stop = i + 1 == n || (gate && purchased < i as i64 + 1);
        let shifted = purchased >= 1 && i as i64 >= purchased - 1 && (price(i) != 0 || gate);
        if shifted {
            hi += stats.next_level_offset;
            lo += stats.next_level_offset;
        }
        if money <= hi || stop {
            if !stop && hi != lo {
                percent = ((money - lo) * 100 / (hi - lo)).min(100);
            }
            level = i as i64;
            if stats.max_achieved_level < level {
                stats.max_achieved_level = level;
                stats.next_level_offset = if price(i) != 0 { money - hi } else { 0 };
            }
            break;
        }
    }
    (level, percent)
}

pub fn level_purchase(rules: &Rules, stats: &RiderStats) -> Option<(i64, i64)> {
    level_purchase_tables(stats, &rules.level_thresholds, &rules.level_prices)
}

pub fn level_purchase_tables(stats: &RiderStats, thresholds: &[i64], prices: &[i64]) -> Option<(i64, i64)> {
    let (level, percent) = update_rider_level(&mut stats.clone(), thresholds, prices);
    if percent != 0 {
        return None;
    }
    let next = level.checked_add(1)?;
    let price = prices.get(usize::try_from(next).ok()?)?;
    (*price > 0 && stats.max_purchased_level < next).then_some((next, *price))
}

impl Profile {
    pub fn purchase_rider_level(
        &mut self,
        rules: &Rules,
        rider: u32,
        level: i64,
    ) -> Result<i64, super::super::store::purchase::PurchaseError> {
        use super::super::store::purchase::PurchaseError;
        let row = |table: &[i64], index: Option<i64>| {
            index.and_then(|i| usize::try_from(i).ok()).and_then(|i| table.get(i)).copied().unwrap_or(0)
        };
        let price = row(&rules.level_prices, Some(level));
        let previous = row(&rules.level_thresholds, level.checked_sub(2));
        if price > self.credits() {
            return Err(PurchaseError::NotEnoughCredits { cost: price, credits: self.credits() });
        }
        self.add_credits(-price, true);
        let stats = self.riders.entry(rider).or_default();
        update_rider_level(stats, &rules.level_thresholds, &rules.level_prices);
        stats.max_purchased_level = level;
        stats.next_level_offset = stats.money_earned - previous;
        Ok(price)
    }
}
