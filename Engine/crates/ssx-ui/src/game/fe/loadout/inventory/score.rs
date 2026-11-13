use super::*;

impl Inventory {
    pub(super) fn stat_level(&self, db: &CareerDb, profile: &Profile, wanted: &[i64]) -> f32 {
        if wanted.is_empty() {
            return 0.0;
        }
        let mut sums: Vec<(i64, i64)> = wanted.iter().map(|&s| (s, 0)).collect();
        sums.sort();
        sums.dedup();
        for equipped in self.equipped.iter().take(4) {
            for &(k, s) in equipped {
                let stats: Vec<(i64, u8)> = match s {
                    0 => db
                        .items
                        .iter()
                        .find(|i| i.key == k)
                        .map(|i| i.attributes.iter().map(|&(t, v)| (i64::from(t), v)).collect())
                        .unwrap_or_default(),
                    _ => profile.generated.get(&k).map(|g| g.stats.to_vec()).unwrap_or_default(),
                };
                for (t, v) in stats.into_iter().take(6) {
                    if let Some(e) = sums.iter_mut().find(|e| e.0 == t) {
                        e.1 = (e.1 + i64::from(v)).clamp(0, 100);
                    }
                }
            }
        }
        let total: i64 = sums.iter().map(|e| e.1).sum();
        total as f32 / sums.len() as f32 * 0.01
    }

    pub fn survival_score(&self, db: &CareerDb, profile: &Profile, drop: Option<&DropInfo>) -> i32 {
        if self.equipped[0].is_empty() {
            return 0;
        }
        let Some(d) = drop else { return 0 };
        let (mut sum_w, mut have, mut covered) = (0f32, 0f32, 0usize);
        for (i, &r) in d.required.iter().enumerate() {
            let w = d.weights.get(i).copied().unwrap_or(0) as f32;
            sum_w += w;
            let Some((gear, stats)) = requirement_gear(r) else { continue };
            let worn = gear.is_none_or(|g| (0..32).contains(&g) && self.worn_gear & (1 << g) != 0);
            if worn {
                covered += 1;
                have += (self.stat_level(db, profile, &stats) * 0.69 + 0.31) * w;
            }
        }
        let survive = d.survive();
        let mut base = match d.difficulty {
            1 => {
                if survive {
                    85.0
                } else {
                    95.0
                }
            }
            2 => {
                if survive {
                    70.0
                } else {
                    85.0
                }
            }
            3 => {
                if survive {
                    50.0
                } else {
                    70.0
                }
            }
            _ => 100.0f32,
        };
        if covered < d.required.len() {
            base = 29.0;
        }
        let f = base * 0.01;
        let mut score = f * have + (base - f * sum_w);
        if covered >= d.required.len() && score < 31.0 {
            score = 31.0;
        }
        score as i32
    }

    pub fn build_selected_gear(
        &mut self,
        db: &CareerDb,
        profile: &Profile,
        loc: &mut Localization,
        drop: Option<&DropInfo>,
    ) {
        self.selected_gear.clear();
        let Some(rec) = self.record(db, profile, loc, self.rider, 0) else { return };
        let rider = self.ui_item(db, profile, loc, drop, &rec);
        self.selected_gear.push(rider);
        for (kind, empty) in EMPTY.iter().enumerate().take(KINDS) {
            for &(k, s) in &self.equipped[kind].clone() {
                let rec = self
                    .record(db, profile, loc, k, s)
                    .or_else(|| self.record(db, profile, loc, fnv1(empty.as_bytes()), 0));
                if let Some(rec) = rec {
                    let mut u = self.ui_item(db, profile, loc, drop, &rec);
                    u.flags |= flag::EQUIPPED;
                    self.selected_gear.push(u);
                }
            }
        }
    }

    pub fn rider_bio(&mut self, db: &CareerDb, profile: &Profile, loc: &mut Localization, rider: u32) {
        self.bio = RiderBio::default();
        let Some(r) = db.riders.iter().find(|r| r.key == rider) else { return };
        let gear = fnv1(format!("gear_{}_worldtour_gear", r.name.to_lowercase()).as_bytes());
        if let Some(rec) = self.fixed_record(db, profile, loc, gear) {
            let u = self.ui_item(db, profile, loc, None, &rec);
            self.bio.gear_asset = u.asset;
            self.bio.gear_name = u.name;
        }
        let mut gear_done = false;
        for &(stat, value) in r.stat_bonuses.iter().take(4) {
            let g = ssx_assets::career::store::stat_gear_type(i64::from(stat));
            let key = match g {
                Some(t) => format!("TXT_GEN_EQUIP_GEAR{t}_0"),
                None => format!("TXT_EQUIPSCREEN_STAT{stat}_LONG"),
            };
            if g.is_some() && gear_done {
                continue;
            }
            gear_done |= g.is_some();
            let name = loc.text(&key);
            self.bio.perks.push((name, i64::from(value)));
        }
    }
}

impl Inventory {
    pub fn optimize(&mut self, db: &CareerDb, profile: &Profile, loc: &mut Localization, drop: Option<&DropInfo>) {
        let ctx = self.store_context(db, profile, drop);
        for (kind, empty) in EMPTY.iter().enumerate().take(4).skip(1) {
            let slots = self.category_slots.get(kind).copied().unwrap_or(0).max(0) as usize;
            self.equipped[kind] = vec![(fnv1(empty.as_bytes()), 0); slots];
            self.update_worn(db, profile);
            self.build_lists(db, profile, loc, drop, &ctx, kind as i64);
            let mut taken: Vec<i64> = Vec::new();
            let mut slot = 0;
            for u in self.owned_list.clone() {
                if slot >= slots {
                    break;
                }
                if u.sort <= 0 || u.has(flag::EMPTY) {
                    continue;
                }
                let group = match kind {
                    2 => Some(u.slot),
                    3 => u.stats.iter().find_map(|s| ssx_assets::career::store::stat_gear_type(s.0)),
                    _ => None,
                };
                if let Some(g) = group {
                    if taken.contains(&g) {
                        continue;
                    }
                    taken.push(g);
                }
                self.equip(db, profile, kind as i64, u.key, u.src, slot);
                slot += 1;
            }
        }
        self.build_selected_gear(db, profile, loc, drop);
        self.survival = self.survival_score(db, profile, drop);
    }
}
