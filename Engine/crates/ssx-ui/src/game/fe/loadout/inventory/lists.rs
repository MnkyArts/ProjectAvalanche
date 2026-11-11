use super::*;

pub(super) const PORTRAIT_LOCKED: &str = "assets/riderPortraitsLarge/locked/";
pub(super) const PORTRAIT_JACKET: &str = "assets/riderPortraitsLarge/jacket/";
pub(super) const PORTRAIT_PANTS: &str = "assets/riderPortraitsLarge/pants/";
pub(super) const ICON_NONE: &str = "assets/equipmentIcons/none.dds";
pub(super) const ICON_MISSING: &str = "assets/equipmentIcons/missingIcon.dds";
pub(super) const MYSTERY_ICON: &str = "assets/equipmentIcons/mystery.dds";

impl Inventory {
    pub fn ui_item(
        &self,
        db: &CareerDb,
        profile: &Profile,
        loc: &mut Localization,
        drop: Option<&DropInfo>,
        rec: &Record,
    ) -> UiItem {
        let empty = Self::is_empty_item(rec.kind, rec.key, rec.src);
        let mut u = UiItem { key: rec.key, src: rec.src, slot: rec.slot, rarity: rec.rarity, ..Default::default() };
        if rec.mystery {
            u.name = loc.text("TXT_MYSTERY_ITEM_NAME");
            u.description = loc.text("TXT_MYSTERY_ITEM_DESC");
            u.asset = MYSTERY_ICON.into();
            u.flags |= flag::MYSTERY;
        } else {
            u.name = rec.name.clone();
            u.description = self.perk_description(loc, rec);
            let icon = |k: u32| db.descriptions.iter().find(|d| d.key == k).map(|d| d.icon.clone()).unwrap_or_default();
            u.asset = icon(rec.descriptions[0]);
            u.secondary = icon(rec.descriptions[1]);
            let package_portrait = db
                .riders
                .iter()
                .find(|r| r.key == rec.descriptions[0])
                .and_then(|r| r.content.as_ref())
                .and_then(|c| c.portrait.as_ref())
                .filter(|p| p.namespace() != ssx_content::BASE_NAMESPACE)
                .and_then(ssx_assets::content::textures::ui_path_of);
            // TODO(guess): a package's rider shows its one portrait where the disc composes the locked picture or the
            // rider in its last suit (jacket + pants pictures by the suit's look ids, 0x8248D2D0…0x8248D5D0); a package
            // has no per-suit pictures, and no record field names them yet.
            if let Some(path) = package_portrait {
                u.asset = path;
                u.secondary.clear();
            } else if let Some(r) = db.riders.iter().find(|r| r.key == rec.descriptions[0]) {
                if !rec.owned {
                    u.asset = format!("{PORTRAIT_LOCKED}{}.dds", r.portrait);
                } else if let Some((suit, src)) = self.rider_suit(db, profile, r.key) {
                    let descs = match src {
                        0 => {
                            db.items.iter().find(|i| i.key == suit).map(|i| i.descriptions.clone()).unwrap_or_default()
                        }
                        _ => profile.generated.get(&suit).map(|g| g.descriptions.to_vec()).unwrap_or_default(),
                    };
                    let id = |k: Option<&u32>| {
                        k.and_then(|k| db.descriptions.iter().find(|d| d.key == *k)).map_or(0, |d| d.id)
                    };
                    u.asset = format!("{PORTRAIT_JACKET}{}_top{}.dds", r.portrait, id(descs.first()));
                    u.secondary = format!("{PORTRAIT_PANTS}{}_bottom{}.dds", r.portrait, id(descs.get(1)));
                }
            }
            if empty {
                u.asset = ICON_NONE.into();
                u.secondary.clear();
            } else if u.asset.is_empty() {
                u.asset = ICON_MISSING.into();
                u.secondary.clear();
            }
        }
        u.cost = if rec.owned {
            ssx_assets::career::store::purchase::sell_price(rec.cost, self.data.tables.sell_percent)
        } else {
            rec.cost
        };
        u.sort = self.sort_value(db, profile, drop, rec);
        u.quality = self.record_score(rec);
        for (k, &(t, v)) in rec.stats[..3].iter().enumerate() {
            u.stats[k] = (t, i64::from(v));
        }
        let kind_ok = (0..KINDS as i64).contains(&rec.kind);
        let equipped = kind_ok && self.equipped[rec.kind as usize].contains(&(rec.key, rec.src));
        if rec.recommended {
            u.flags |= flag::RECOMMENDED;
        }
        let boost_blocked =
            drop.is_some_and(|d| rec.kind == 3 && !d.survive() && rec.stats[..3].iter().any(|s| s.0 == 0x20));
        let masked = rec.slot >= 0 && self.gear_mask & (1u64 << (rec.slot & 0x3F)) as u32 != 0;
        let level_ok = rec.kind == RIDER || self.level_limit == -1 || self.level_limit >= rec.level;
        let available = empty || (rec.owned && !masked && level_ok);
        if available && !boost_blocked {
            u.flags |= flag::AVAILABLE;
        }
        if rec.cost > 0 {
            if !rec.owned {
                u.flags |= flag::PURCHASEABLE;
            } else if !equipped {
                u.flags |= flag::SELLABLE;
            }
        }
        if rec.kind != 4 && rec.kind != 0 && !rec.mystery && !empty {
            u.flags |= flag::SHOW_STATS;
        }
        if rec.special {
            u.flags |= flag::SPECIAL;
        }
        if kind_ok && self.new_items[rec.kind as usize].contains(&(rec.key, rec.src)) {
            u.flags |= flag::NEW;
        }
        if empty {
            u.flags |= flag::EMPTY;
        }
        if rec.kind != 0 {
            u.flags |= flag::SHOW_LEVEL;
        }
        if rec.kind == RIDER {
            let (level, percent) = Self::rider_level(db, profile, rec.key);
            u.level = level + 1;
            u.level_progress = percent;
            u.description =
                db.riders.iter().find(|r| r.key == rec.key).map(|r| loc.text(&r.description)).unwrap_or_default();
        } else {
            u.level = rec.level + 1;
        }
        u
    }

    pub(super) fn perk_description(&self, loc: &mut Localization, rec: &Record) -> String {
        if rec.kind == 4 {
            return String::new();
        }
        let Some(i) = (3..6).rev().find(|&i| rec.stats[i].0 != -1) else { return String::new() };
        let (t, v) = rec.stats[i];
        loc.format(&format!("TXT_PERK_DESC_{t}"), &[&v.to_string()])
    }
}

impl Inventory {
    pub fn fixed_shown(&self, db: &CareerDb, profile: &Profile, drop: Option<&DropInfo>, kind: i64) -> Vec<u32> {
        let owned = |k: u32| self.catalog.entries.get(&k).is_some_and(|e| profile.offered(k, e));
        let forced = drop.and_then(|d| d.forced(kind));
        if kind == RIDER {
            return db
                .riders
                .iter()
                .filter(|r| r.default || owned(r.key))
                .filter(|r| forced.is_none_or(|f| f == r.key))
                .filter(|r| !drop.is_some_and(|d| d.rider_forced) || drop.and_then(|d| d.forced(RIDER)) == Some(r.key))
                .map(|r| r.key)
                .collect();
        }
        let level = Self::rider_level(db, profile, self.rider).0;
        db.items
            .iter()
            .filter(|i| i.kind == kind)
            .filter(|i| {
                let own = owned(i.key);
                let mut shown = i.default || own;
                if i.rider != 0 && self.rider != 0 {
                    shown &= i.rider == self.rider;
                }
                if !own {
                    shown &= level >= i.level;
                }
                match forced {
                    Some(f) => f == i.key,
                    None => shown,
                }
            })
            .map(|i| i.key)
            .collect()
    }

    pub fn generated_shown(&self, db: &CareerDb, profile: &Profile, kind: i64, ctx: &StoreContext) -> Vec<u32> {
        let _ = db;
        let mut out: Vec<u32> = profile
            .generated
            .values()
            .filter(|g| g.kind == kind && (g.charlock == 0 || g.charlock == self.rider))
            .map(|g| g.equip_id)
            .collect();
        if (0..KINDS as i64).contains(&kind) {
            out.extend(self.store.offers(&self.data, ctx, kind as usize).iter().map(|o| o.id));
        }
        out
    }

    #[allow(clippy::too_many_arguments)]
    pub fn build_lists(
        &mut self,
        db: &CareerDb,
        profile: &Profile,
        loc: &mut Localization,
        drop: Option<&DropInfo>,
        ctx: &StoreContext,
        kind: i64,
    ) {
        self.owned_list.clear();
        self.store_list.clear();
        self.inventory_list.clear();
        let mut keys: Vec<(u32, u8)> = self.fixed_shown(db, profile, drop, kind).into_iter().map(|k| (k, 0)).collect();
        if kind != RIDER {
            keys.extend(self.generated_shown(db, profile, kind, ctx).into_iter().map(|k| (k, 1)));
        }
        let bit = |t: i64| if (0..32).contains(&t) { 1u32 << t } else { 0 };
        for (k, src) in keys {
            let Some(rec) = self.record(db, profile, loc, k, src) else { continue };
            if Self::is_empty_item(rec.kind, rec.key, rec.src) {
                continue;
            }
            let item = self.ui_item(db, profile, loc, drop, &rec);
            let worn = (self.worn_gear | self.gear_mask) & bit(rec.slot) != 0;
            let over_level = rec.kind != RIDER && self.level_limit != -1 && self.level_limit < rec.level;
            let equipped =
                (0..KINDS as i64).contains(&kind) && self.equipped[kind as usize].contains(&(rec.key, rec.src));
            if !worn && !over_level {
                if !rec.owned {
                    let gear_ok = rec.kind != 2 || (ctx.gear_types | ctx.required_gear) & bit(rec.slot) != 0;
                    let boost_ok = rec.kind != 3 || item.sort >= 1;
                    if kind == RIDER {
                        self.owned_list.push(item.clone());
                    } else if gear_ok && boost_ok {
                        self.store_list.push(item.clone());
                    }
                } else if !equipped || kind == RIDER {
                    self.owned_list.push(item.clone());
                }
            }
            if rec.owned && kind != RIDER {
                let mut it = item;
                if equipped || worn {
                    it.flags &= !flag::AVAILABLE;
                }
                self.inventory_list.push(it);
            }
        }
        let by_value = |a: &UiItem, b: &UiItem| (b.sort, b.rarity).cmp(&(a.sort, a.rarity));
        if kind != RIDER {
            self.owned_list.sort_by(by_value);
            self.inventory_list.sort_by(by_value);
            let with_empty = matches!(kind, 2..=4);
            self.owned_list.truncate(if with_empty { 6 } else { 7 });
            if with_empty && let Some(rec) = self.record(db, profile, loc, fnv1(EMPTY[kind as usize].as_bytes()), 0) {
                let e = self.ui_item(db, profile, loc, drop, &rec);
                self.owned_list.push(e.clone());
                self.inventory_list.push(e);
            }
            self.store_list.sort_by(by_value);
            if kind == 2
                && let Some(slot) = drop.and_then(DropInfo::main_gear)
            {
                self.offer_main_gear(db, profile, loc, drop, ctx, slot);
            }
            self.store_list.truncate(4);
            let equipped = self.equipped[kind as usize].clone();
            for it in &mut self.inventory_list {
                if equipped.contains(&(it.key, it.src)) {
                    it.flags |= flag::EQUIPPED;
                }
            }
        }
        self.survival = self.survival_score(db, profile, drop);
        self.inventory_size = Self::inventory_size(db, profile);
    }

    fn offer_main_gear(
        &mut self,
        db: &CareerDb,
        profile: &Profile,
        loc: &mut Localization,
        drop: Option<&DropInfo>,
        ctx: &StoreContext,
        slot: i64,
    ) {
        if let Some(i) = self.store_list.iter().position(|u| u.slot == slot) {
            let it = self.store_list.remove(i);
            self.store_list.insert(0, it);
            return;
        }
        let level = if ctx.level < self.level_limit { ctx.level } else { self.level_limit };
        let req = Request { level, slot, ..Request::NORMAL };
        let Some(id) = self.store.generate(&self.data, ctx, 2, &req, self.now_ms) else { return };
        if let Some(rec) = self.record(db, profile, loc, id, 1) {
            let item = self.ui_item(db, profile, loc, drop, &rec);
            self.store_list.insert(0, item);
        }
    }

    pub fn seen(&mut self, kind: i64, entry: (u32, u8)) {
        if let Some(l) = usize::try_from(kind).ok().and_then(|k| self.new_items.get_mut(k)) {
            l.retain(|&e| e != entry);
        }
        if !(0..KINDS as i64).contains(&kind) {
            return;
        }
        for list in [&mut self.owned_list, &mut self.inventory_list] {
            if let Some(u) = list.iter_mut().find(|u| (u.key, u.src) == entry) {
                u.flags &= !flag::NEW;
            }
        }
    }

    pub fn inventory_size(db: &CareerDb, profile: &Profile) -> i32 {
        let rows = |n: usize| i64::from(u16::try_from(n % 0x1_0000).unwrap_or(0));
        let riders = i64::try_from(Self::rider_count(db, profile)).unwrap_or(i64::MAX);
        let n = rows(profile.owned_items.len()) - riders + rows(profile.generated.len());
        i32::try_from(n.max(0)).unwrap_or(i32::MAX)
    }

    pub fn rider_count(db: &CareerDb, profile: &Profile) -> usize {
        db.riders.iter().filter(|r| !r.locked || profile.owned_items.contains(&r.key)).count()
    }
}
