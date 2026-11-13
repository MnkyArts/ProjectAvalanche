use std::collections::HashMap;

use ssx_formats::attrib::collection::{self, Collection, DecodedCollection, FieldData};
use ssx_formats::attrib::{Value, fnv1};

use crate::character::GearStats;

pub const CLASS_EQUIPMENT: u32 = fnv1(b"equipment");
pub const CLASS_CHARACTER: u32 = fnv1(b"character");
const FIELD_GAMEPLAY_ATTRIBUTES: u32 = 0xFE0C_3C3F;
const FIELD_CHARACTER: u32 = 0x432D_A24B;
const FIELD_LOCKED: u32 = 0x5650_1F0A;
const FIELD_DEFAULT: u32 = 0x349E_965B;
const FIELD_DESCRIPTIONS: u32 = 0x9F3E_A0B0;
const FIELD_TYPE: u32 = fnv1(b"type");
const FIELD_STAT_INFO: u32 = 0xE412_CD30;
const COLLECTION_BOOST_MAX: u32 = 0x3291_8AA5;
const FIELD_BOOST_MAX: u32 = 0x26D5_959E;
pub const TYPE_SUIT: i32 = 0;
pub const TYPE_BOARD: i32 = 1;
pub const TYPE_GEAR: i32 = 2;
pub const TYPE_BOOST: i32 = 3;

pub type ForcedItems = [u32; 5];

#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    pub key: u32,
    pub rider: u32,
    pub locked: bool,
    pub default: bool,
    pub kind: i32,
    pub slot: i32,
    pub attributes: [(i32, u8); 6],
    pub descriptions: Vec<u32>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct GearDb {
    pub items: Vec<Item>,
    pub bonuses: HashMap<u32, Vec<(i32, u8)>>,
    pub boost_max: Vec<u8>,
}

fn single(d: &FieldData) -> Option<&Value> {
    match d {
        FieldData::Single(v) => Some(v),
        FieldData::Array(vs) => vs.first(),
    }
}

fn raw_list(d: &FieldData) -> Vec<&[u8]> {
    let vs: Vec<&Value> = match d {
        FieldData::Single(v) => vec![v],
        FieldData::Array(vs) => vs.iter().collect(),
    };
    vs.into_iter()
        .filter_map(|v| if let Value::Raw { bytes, .. } = v { Some(bytes.as_slice()) } else { None })
        .collect()
}

fn ref_list(d: &FieldData) -> Vec<u32> {
    let vs: Vec<&Value> = match d {
        FieldData::Single(v) => vec![v],
        FieldData::Array(vs) => vs.iter().collect(),
    };
    vs.into_iter().filter_map(|v| if let Value::Ref(r) = v { Some(r.collection) } else { None }).collect()
}

fn be_i32(b: &[u8], at: usize) -> Option<i32> {
    b.get(at..at + 4).map(|w| i32::from_be_bytes([w[0], w[1], w[2], w[3]]))
}

fn stat_pair(b: &[u8], at: usize) -> Option<(i32, u8)> {
    Some((be_i32(b, at)?, *b.get(at + 4)?))
}

impl GearDb {
    pub fn from_vault(
        cols: &[Collection],
        decoded: &[DecodedCollection],
        index: &HashMap<(u32, u32), usize>,
    ) -> ssx_formats::Result<Self> {
        let mut db = GearDb::default();
        db.add_riders(cols, decoded, index)?;
        for (i, c) in cols.iter().enumerate().filter(|(_, c)| c.class == CLASS_EQUIPMENT) {
            let fields = collection::with_inherited(cols, decoded, index, i)?;
            let field = |k: u32| fields.iter().find(|(_, f)| f.key == k).map(|(_, f)| &f.data);
            if c.key == COLLECTION_BOOST_MAX
                && let Some(b) = field(FIELD_BOOST_MAX).and_then(|d| raw_list(d).first().map(|b| b.to_vec()))
            {
                db.boost_max = b;
            }
            let Some(attrs) = field(FIELD_GAMEPLAY_ATTRIBUTES).and_then(|d| raw_list(d).first().copied()) else {
                continue;
            };
            let flag = |k: u32| field(k).and_then(single).is_some_and(|v| matches!(v, Value::Bool(true)));
            let rider = match field(FIELD_CHARACTER).and_then(single) {
                Some(Value::Ref(r)) => r.collection,
                _ => 0,
            };
            let kind = field(FIELD_TYPE).and_then(|d| raw_list(d).first().and_then(|b| be_i32(b, 0))).unwrap_or(-1);
            let mut attributes = [(-1, 0); 6];
            for (k, a) in attributes.iter_mut().enumerate() {
                *a = stat_pair(attrs, 4 + 8 * k).unwrap_or((-1, 0));
            }
            db.items.push(Item {
                key: c.key,
                rider,
                locked: flag(FIELD_LOCKED),
                default: flag(FIELD_DEFAULT),
                kind,
                slot: be_i32(attrs, 0).unwrap_or(-1),
                attributes,
                descriptions: field(FIELD_DESCRIPTIONS).map(ref_list).unwrap_or_default(),
            });
        }
        Ok(db)
    }

    pub fn add_riders(
        &mut self,
        cols: &[Collection],
        decoded: &[DecodedCollection],
        index: &HashMap<(u32, u32), usize>,
    ) -> ssx_formats::Result<()> {
        for (i, c) in cols.iter().enumerate().filter(|(_, c)| c.class == CLASS_CHARACTER) {
            let fields = collection::with_inherited(cols, decoded, index, i)?;
            if let Some((_, f)) = fields.iter().find(|(_, f)| f.key == FIELD_STAT_INFO) {
                let pairs = raw_list(&f.data).into_iter().filter_map(|b| stat_pair(b, 0)).collect();
                self.bonuses.insert(c.key, pairs);
            }
        }
        Ok(())
    }

    fn owned(&self, rider: u32) -> impl Iterator<Item = &Item> {
        self.items.iter().filter(move |i| i.rider == rider && !i.locked)
    }

    // TODO(guess): a port choice — without a front end (`ride_sim`, `--ride`) the event's loadout is this one: a forced
    // item, else the rider's default board / suit and the required slot's one unlocked item (nothing bought or
    // equipped); the game always rides with the player's profile (the shell commits the launch's setup, `GameSetup`),
    // and a fresh profile's inventory is the game's `default_equipment` (Elise, her default suit and board, a gear
    // item; `career::Inventory::new`), not this; settle: none — the game has no ride without a profile.
    pub fn fresh_profile_loadout(&self, rider: u32, forced: &ForcedItems, required: &[u32]) -> Option<GearStats> {
        crate::guess_hit!();
        fn unique(mut it: Vec<&Item>) -> Option<&Item> {
            if it.len() == 1 { it.pop() } else { None }
        }
        let default_of = |kind| unique(self.owned(rider).filter(|i| i.kind == kind && i.default).collect());
        let forced_of = |kind: i32| match forced[kind as usize] {
            0 => Ok(None),
            key => self.items.iter().find(|i| i.key == key).map(Some).ok_or(()),
        };
        let mut items = vec![match forced_of(TYPE_BOARD).ok()? {
            Some(board) => board,
            None => default_of(TYPE_BOARD)?,
        }];
        match forced_of(TYPE_SUIT).ok()? {
            Some(suit) => items.push(suit),
            None => items.extend(default_of(TYPE_SUIT)),
        }
        match forced_of(TYPE_GEAR).ok()? {
            Some(gear) => items.push(gear),
            None => {
                let slots = required.iter().filter_map(|&v| crate::survival::required_gear_slot(v));
                for slot in slots.filter(|&s| s != 0) {
                    items.push(unique(self.owned(rider).filter(|i| i.slot == slot as i32).collect())?);
                }
            }
        }
        items.extend(forced_of(TYPE_BOOST).ok()?);
        let bonus = self.bonuses.get(&rider).map_or(&[][..], Vec::as_slice);
        let committed: Vec<CommittedItem> = items.iter().map(|i| commit(i, bonus, &self.boost_max)).collect();
        Some(gear_stats(&committed))
    }
}

pub const FINAL_DROP: u32 = 0x28F6_34F8;
pub const FINAL_DROP_EQUIPS: [(u32, u32); 5] =
    [(0x55BB_536F, 0), (0x2002_1A25, 0), (0x2039_B98F, 0), (0x2636_71F3, 1), (0xEED8_4B39, 2)];

impl GearDb {
    pub fn equipped_loadout(&self, rider: u32, equips: &[(u32, u32)], slot_counts: [u32; 5]) -> Option<GearStats> {
        let mut items = Vec::new();
        for &(key, slot) in equips {
            let item = self.items.iter().find(|i| i.key == key)?;
            let count = usize::try_from(item.kind).ok().and_then(|k| slot_counts.get(k)).copied().unwrap_or(0);
            if slot < count {
                items.push(item);
            }
        }
        let bonus = self.bonuses.get(&rider).map_or(&[][..], Vec::as_slice);
        let committed: Vec<CommittedItem> = items.iter().map(|i| commit(i, bonus, &self.boost_max)).collect();
        Some(gear_stats(&committed))
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct CommittedItem {
    pub slot: i32,
    pub stats: Vec<(usize, u8)>,
}

impl CommittedItem {
    fn set(&mut self, stat: usize, value: u8) {
        if let Some(e) = self.stats.iter_mut().find(|(s, _)| *s == stat) {
            e.1 = value;
        } else if self.stats.len() < 6 {
            self.stats.push((stat, value));
        }
    }
}

pub fn committed_value(stat: i32, raw: u32, scaled: bool, boost_max: &[u8]) -> u8 {
    if stat < 0 || stat == 37 {
        return 0;
    }
    let v = if scaled {
        let m = boost_max.get(stat as usize).copied().unwrap_or(0) as i32;
        ((m * raw as i32) as f32 * 0.01).ceil() as i32
    } else {
        raw as i32
    };
    v.clamp(0, 100) as u8
}

pub fn commit(item: &Item, bonus: &[(i32, u8)], boost_max: &[u8]) -> CommittedItem {
    let boost = item.kind == TYPE_BOOST;
    let mut out = CommittedItem { slot: item.slot, stats: Vec::new() };
    for (k, &(stat, value)) in item.attributes.iter().enumerate() {
        if stat == -1 {
            continue;
        }
        let mut raw = value as u32;
        if !boost && let Some(&(_, b)) = bonus.iter().find(|(s, _)| *s == stat) {
            raw += b as u32;
        }
        let v = committed_value(stat, raw, boost || k > 2, boost_max);
        if (0..37).contains(&stat) {
            out.set(stat as usize, v);
        }
    }
    out
}

pub fn gear_stats(items: &[CommittedItem]) -> GearStats {
    let mut setup = GameSetup::default();
    let mut boosts = 0;
    for i in items {
        let slot = if i.slot == BOOST_TYPE {
            let s = BOOST_TYPE + boosts;
            boosts = (boosts + 1).min(3);
            s
        } else {
            i.slot
        };
        let Ok(t) = usize::try_from(slot) else { continue };
        if t >= GEAR_TYPES {
            continue;
        }
        let mut g = GearItem::CLEARED;
        g.set_slot(slot);
        for &(s, v) in &i.stats {
            g.set_stat(s as i32, v);
        }
        setup.items[t] = g;
        if t == JACKET_TYPE {
            setup.items[PANTS_TYPE] = GearItem { slot: PANTS_TYPE as i32, ..GearItem::CLEARED };
        }
    }
    setup.gear_stats()
}

pub const GEAR_TYPES: usize = 15;
pub const GEAR_TYPE_NAMES: [&str; GEAR_TYPES] = [
    "Board",
    "Wingsuit",
    "IceAxe",
    "Armour",
    "Thermal",
    "Oxygen",
    "HeadLamp",
    "PulseGoggles",
    "Boost1",
    "Boost2",
    "Boost3",
    "Boost4",
    "Jacket",
    "Pants",
    "SmokeCanister",
];
pub const BOOST_TYPE: i32 = 8;
pub const JACKET_TYPE: usize = 12;
pub const PANTS_TYPE: usize = 13;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GearItem {
    pub stats: [(i8, u8); 6],
    pub slot: i32,
    pub description: u32,
}

impl Default for GearItem {
    fn default() -> Self {
        GearItem::CLEARED
    }
}

impl GearItem {
    pub const CLEARED: GearItem = GearItem { stats: [(-1, 0); 6], slot: -1, description: 0 };

    pub fn set_slot(&mut self, slot: i32) {
        if self.slot != slot {
            *self = GearItem::CLEARED;
        }
        self.slot = slot;
    }

    pub fn set_stat(&mut self, stat: i32, value: u8) {
        let stat = stat as i8;
        if let Some(e) = self.stats.iter_mut().find(|e| e.0 == stat || e.0 == -1) {
            *e = (stat, value);
        }
    }

    pub fn stat(&self, stat: i32) -> Option<u8> {
        self.stats.iter().find(|e| i32::from(e.0) == stat).map(|e| e.1)
    }

    pub fn is_cleared(&self) -> bool {
        self.slot == -1
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct GameSetup {
    pub character: u32,
    pub items: [GearItem; GEAR_TYPES],
}

impl GameSetup {
    pub fn actor_gear(&self) -> [GearItem; GEAR_TYPES] {
        let mut out = [GearItem::CLEARED; GEAR_TYPES];
        for (dst, src) in out.iter_mut().zip(self.items.iter().filter(|g| !g.is_cleared())) {
            *dst = *src;
        }
        out
    }

    pub fn gear_stats(&self) -> GearStats {
        let gear = self.actor_gear();
        let mut stats = [0.0f32; 37];
        for (s, out) in stats.iter_mut().enumerate() {
            let sum = gear.iter().filter_map(|g| g.stat(s as i32)).fold(0u8, u8::wrapping_add);
            *out = f32::from(sum);
        }
        let slots = std::array::from_fn(|t| gear.iter().any(|g| g.slot == t as i32));
        GearStats { stats, slots, null: false }
    }

    pub fn item(&self, t: usize) -> Option<&GearItem> {
        self.items.get(t).filter(|g| !g.is_cleared())
    }
}

impl GearDb {
    pub fn setup(&self, rider: u32, items: &[u32]) -> Result<GameSetup, u32> {
        let found: Vec<&Item> =
            items.iter().map(|&k| self.items.iter().find(|i| i.key == k).ok_or(k)).collect::<Result<_, _>>()?;
        let bonus = self.bonuses.get(&rider).map_or(&[][..], Vec::as_slice);
        let mut setup = GameSetup { character: rider, ..GameSetup::default() };
        for kind in [TYPE_SUIT, TYPE_BOARD, TYPE_GEAR, TYPE_BOOST] {
            let mut boosts = 0;
            for item in found.iter().filter(|i| i.kind == kind) {
                let mut slot = item.slot;
                if slot == BOOST_TYPE {
                    slot = BOOST_TYPE + boosts;
                    boosts = (boosts + 1).min(3);
                }
                if let Ok(t) = usize::try_from(slot)
                    && t < GEAR_TYPES
                {
                    let mut g = GearItem::CLEARED;
                    g.set_slot(slot);
                    g.description = item.descriptions.first().copied().unwrap_or(0);
                    for (s, v) in commit(item, bonus, &self.boost_max).stats {
                        g.set_stat(s as i32, v);
                    }
                    setup.items[t] = g;
                }
                if slot == JACKET_TYPE as i32 {
                    let description = item.descriptions.get(1).copied().unwrap_or(0);
                    setup.items[PANTS_TYPE] = GearItem { slot: PANTS_TYPE as i32, description, ..GearItem::CLEARED };
                }
            }
        }
        Ok(setup)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(kind: i32, slot: i32, attrs: &[(i32, u8)]) -> Item {
        let mut attributes = [(-1, 0); 6];
        attributes[..attrs.len()].copy_from_slice(attrs);
        Item { key: 1, rider: 7, locked: false, default: true, kind, slot, attributes, descriptions: Vec::new() }
    }

    #[test]
    fn rider_bonus_is_added_and_clamped() {
        let boost_max = [15u8; 37];
        let tank = item(2, 5, &[(17, 38), (18, 48)]);
        let c = commit(&tank, &[(18, 5), (17, 5)], &boost_max);
        assert_eq!(c.stats, vec![(17, 43), (18, 53)]);
        let big = item(1, 0, &[(0, 98)]);
        assert_eq!(commit(&big, &[(0, 10)], &boost_max).stats, vec![(0, 100)]);
    }

    #[test]
    fn late_attributes_and_boosts_are_scaled_up() {
        let boost_max = [15u8; 37];
        let board = item(1, 0, &[(0, 38), (1, 38), (2, 54), (-1, 0), (-1, 0), (2, 10)]);
        assert_eq!(commit(&board, &[], &boost_max).stats, vec![(0, 38), (1, 38), (2, 2)]);
        let boost = item(TYPE_BOOST, 8, &[(18, 100)]);
        assert_eq!(commit(&boost, &[(18, 5)], &boost_max).stats, vec![(18, 15)]);
        assert_eq!(committed_value(37, 50, false, &boost_max), 0);
    }

    #[test]
    fn a_forced_kind_holds_the_forced_item_alone() {
        let mut boost_max = vec![15u8; 37];
        boost_max[26] = 100;
        let own_board = item(TYPE_BOARD, 0, &[(0, 30), (1, 38), (2, 27)]);
        let mut sky = item(TYPE_BOARD, 0, &[(0, 0), (1, 0), (2, 100), (26, 100)]);
        (sky.key, sky.rider, sky.locked, sky.default) = (2, 0, true, false);
        let mut suit = item(TYPE_GEAR, 1, &[(5, 100), (6, 100), (7, 0)]);
        (suit.key, suit.rider, suit.locked, suit.default) = (3, 0, true, false);
        let mut armour = item(TYPE_GEAR, 3, &[(11, 34), (12, 52)]);
        (armour.key, armour.default) = (4, false);
        let db = GearDb { items: vec![own_board, sky, suit, armour], bonuses: HashMap::new(), boost_max };
        let g = db.fresh_profile_loadout(7, &[0; 5], &[4]).unwrap();
        assert_eq!((g.stats[0], g.stats[26], g.stats[11]), (30.0, 0.0, 34.0));
        assert!(g.slots[0] && g.slots[3] && !g.slots[1]);
        let g = db.fresh_profile_loadout(7, &[0, 2, 3, 0, 0], &[4]).unwrap();
        assert_eq!((g.stats[0], g.stats[2], g.stats[26], g.stats[5], g.stats[11]), (0.0, 100.0, 100.0, 100.0, 0.0));
        assert!(g.slots[0] && g.slots[1] && !g.slots[3]);
        assert!(db.fresh_profile_loadout(7, &[0, 99, 0, 0, 0], &[]).is_none());
    }

    #[test]
    fn equips_beyond_the_drops_slots_are_dropped() {
        let boost_max = vec![15u8; 37];
        let mut board = item(TYPE_BOARD, 0, &[(0, 50), (1, 62), (2, 43)]);
        board.key = 10;
        let mut wingsuit = item(TYPE_GEAR, 1, &[(5, 50), (6, 50), (7, 30)]);
        wingsuit.key = 11;
        let mut lamp = item(TYPE_GEAR, 6, &[(20, 64), (21, 75), (22, 77)]);
        lamp.key = 12;
        let mut armour = item(TYPE_GEAR, 3, &[(11, 24), (12, 62)]);
        armour.key = 13;
        let bonuses = HashMap::from([(7, vec![(0, 5), (1, 5), (2, 5)])]);
        let db = GearDb { items: vec![board, wingsuit, lamp, armour], bonuses, boost_max };
        let equips = [(10, 0), (11, 0), (12, 1), (13, 2)];
        let g = db.equipped_loadout(7, &equips, [1, 1, 3, 0, 0]).unwrap();
        assert!(g.slots[0] && g.slots[1] && g.slots[6] && g.slots[3]);
        assert_eq!((g.stats[0], g.stats[5], g.stats[20], g.stats[11]), (55.0, 50.0, 64.0, 24.0));
        let g = db.equipped_loadout(7, &equips, [1, 1, 2, 0, 0]).unwrap();
        assert!(g.slots[1] && g.slots[6] && !g.slots[3]);
        assert!(db.equipped_loadout(7, &[(99, 0)], [1; 5]).is_none());
    }

    #[test]
    fn stats_sum_over_items_and_jackets_bring_pants() {
        let a = CommittedItem { slot: 0, stats: vec![(0, 16)] };
        let b = CommittedItem { slot: 5, stats: vec![(17, 43)] };
        let s = CommittedItem { slot: 12, stats: vec![] };
        let g = gear_stats(&[a, b, s]);
        assert_eq!((g.stats[0], g.stats[17]), (16.0, 43.0));
        assert!(g.slots[0] && g.slots[5] && g.slots[12] && g.slots[13] && !g.slots[1]);
    }

    #[test]
    fn the_actor_packs_the_setup_and_sums_first_entries() {
        let mut s = GameSetup { character: 7, ..GameSetup::default() };
        let mut board = GearItem::CLEARED;
        board.set_slot(0);
        board.set_stat(0, 200);
        board.set_stat(2, 10);
        board.set_stat(0, 250);
        s.items[0] = board;
        let mut armour = GearItem::CLEARED;
        armour.set_slot(3);
        armour.set_stat(0, 10);
        armour.set_stat(11, 34);
        s.items[3] = armour;
        s.items[13] = GearItem { slot: 13, ..GearItem::CLEARED };
        let gear = s.actor_gear();
        assert_eq!(gear.iter().map(|g| g.slot).collect::<Vec<_>>()[..4], [0, 3, 13, -1]);
        let g = s.gear_stats();
        assert_eq!((g.stats[0], g.stats[2], g.stats[11]), (4.0, 10.0, 34.0));
        assert!(g.slots[0] && g.slots[3] && g.slots[13] && !g.slots[1] && !g.null);
        assert_eq!(GameSetup::default().gear_stats().stats, [0.0; 37], "an empty setup: nothing, not the defaults");
    }

    #[test]
    fn a_list_of_items_commits_like_the_front_end() {
        let boost_max = vec![15u8; 37];
        let mut suit = item(TYPE_SUIT, 12, &[]);
        (suit.key, suit.descriptions) = (1, vec![0x901, 0x902]);
        let mut board = item(TYPE_BOARD, 0, &[(0, 30), (1, 38), (2, 27)]);
        (board.key, board.descriptions) = (2, vec![0x900]);
        let mut boost_a = item(TYPE_BOOST, 8, &[(31, 40)]);
        boost_a.key = 3;
        let mut boost_b = item(TYPE_BOOST, 8, &[(0, 100)]);
        boost_b.key = 4;
        let bonuses = HashMap::from([(7, vec![(0, 5)])]);
        let db = GearDb { items: vec![suit, board, boost_a, boost_b], bonuses, boost_max };
        let s = db.setup(7, &[4, 2, 3, 1]).unwrap();
        assert_eq!((s.items[0].stats[0], s.items[0].description), ((0, 35), 0x900));
        assert_eq!((s.items[12].description, s.items[13].description), (0x901, 0x902));
        assert_eq!((s.items[8].stats[0], s.items[9].stats[0]), ((0, 15), (31, 6)));
        let g = s.gear_stats();
        assert_eq!((g.stats[0], g.stats[31]), (50.0, 6.0));
        assert_eq!(db.setup(7, &[99]), Err(99));
    }
}
