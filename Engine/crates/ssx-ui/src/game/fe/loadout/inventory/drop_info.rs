use super::*;

#[derive(Clone, Debug, Default)]
pub struct DropInfo {
    pub event_type: i64,
    pub difficulty: i64,
    pub required: Vec<i64>,
    pub weights: Vec<i64>,
    pub forced: Vec<u32>,
    pub rider_forced: bool,
}

impl DropInfo {
    pub fn of(db: &CareerDb, profile: &Profile, drop: u32) -> Option<Self> {
        use ssx_assets::career::world_tour::{CHAPTERS, chapter_drops};
        let e = db.event(drop)?;
        let mut rider_forced = false;
        if e.drop_type == ssx_assets::career::DropType::Campaign {
            for c in 1..=CHAPTERS {
                let list = chapter_drops(c);
                if list.iter().any(|n| fnv1(n.as_bytes()) == drop) {
                    let last = list.last().map(|n| fnv1(n.as_bytes())).unwrap_or(0);
                    rider_forced = profile.drop_state(last) != ssx_assets::career::profile::DropState::Completed;
                }
            }
        }
        Some(DropInfo {
            event_type: e.event_type,
            difficulty: e.difficulty,
            required: e.required.clone(),
            weights: e.unk_b683b553.clone(),
            forced: e.forced.clone(),
            rider_forced,
        })
    }

    pub fn forced(&self, kind: i64) -> Option<u32> {
        self.forced.get(kind as usize).copied().filter(|&k| k != 0)
    }

    pub fn required_gear(&self) -> u32 {
        self.required
            .iter()
            .filter(|&&r| r != 7)
            .filter_map(|&r| requirement_gear(r).and_then(|(g, _)| g))
            .filter(|g| (0..32).contains(g))
            .fold(0, |m, g| m | 1 << g)
    }

    pub fn main_gear(&self) -> Option<i64> {
        let (mut top, mut main) = (0, 0);
        for (i, &r) in self.required.iter().enumerate().filter(|&(_, &r)| r != 7) {
            let w = self.weights.get(i).copied().unwrap_or(0);
            if top < w {
                (top, main) = (w, r);
            }
        }
        if top <= 0 {
            return None;
        }
        requirement_gear(main).and_then(|(g, _)| g)
    }

    pub fn forced_kinds(&self) -> [bool; KINDS] {
        std::array::from_fn(|k| self.forced(k as i64).is_some())
    }

    pub(super) fn survive(&self) -> bool {
        matches!(self.event_type, 3 | 4)
    }
}

pub(super) fn requirement_gear(r: i64) -> Option<(Option<i64>, Vec<i64>)> {
    let gear = match r {
        0 => 1,
        1 => 2,
        2 => 5,
        3 => 4,
        4 => 3,
        5 => 6,
        6 => 7,
        7 => return Some((None, vec![4, 3, 0, 1])),
        _ => return None,
    };
    Some((Some(gear), gear_stats(gear)))
}

pub(super) fn gear_stats(g: i64) -> Vec<i64> {
    if (1..=7).contains(&g) { (0..3).map(|k| 5 + 3 * (g - 1) + k).collect() } else { Vec::new() }
}

impl Inventory {
    pub fn is_empty_item(kind: i64, key: u32, src: u8) -> bool {
        (0..KINDS as i64).contains(&kind) && src == 0 && key == fnv1(EMPTY[kind as usize].as_bytes())
    }

    pub fn sort_value(&self, db: &CareerDb, profile: &Profile, drop: Option<&DropInfo>, rec: &Record) -> i64 {
        if rec.kind == 0 || rec.kind == 4 || rec.kind == RIDER {
            return self.record_score(rec).clamp(0, 100);
        }
        let mut relevant: Vec<i64> = Vec::new();
        if let Some(d) = drop {
            if rec.kind == 3 {
                for &(k, s) in &self.equipped[2] {
                    if let Some(g) = self.slot_of(db, profile, k, s) {
                        relevant.extend(gear_stats(g));
                    }
                }
            } else {
                for &r in &d.required {
                    if let Some((Some(g), _)) = requirement_gear(r) {
                        relevant.extend(gear_stats(g));
                    }
                }
            }
            let by_type: &[i64] = match d.event_type {
                3 | 4 => &[0, 1, 2, 0x21, 0x1F, 0x20],
                0..=2 => &[1, 2, 3, 0x21, 0x1F, 0x1C],
                5..=7 => &[0, 1, 3, 0x21, 0x1F],
                _ => &[],
            };
            relevant.extend_from_slice(by_type);
        }
        let (mut n, mut sum) = (0i64, 0i64);
        for &(t, v) in &rec.stats[..3] {
            if t == -1 {
                continue;
            }
            if relevant.is_empty() || relevant.contains(&t) {
                n += 1;
                sum += i64::from(v);
            }
        }
        if n == 0 { 0 } else { ((sum as f32 / n as f32).floor() as i64).clamp(0, 100) }
    }

    pub fn slot_of(&self, db: &CareerDb, profile: &Profile, key: u32, src: u8) -> Option<i64> {
        match src {
            0 => db.items.iter().find(|i| i.key == key).map(|i| i.slot),
            _ => profile.generated.get(&key).map(|g| g.slot),
        }
    }

    pub fn record_score(&self, rec: &Record) -> i64 {
        let item =
            StoreItem { kind: rec.kind, level: rec.level, rarity: rec.rarity, stats: rec.stats, ..Default::default() };
        ssx_assets::career::store::score(&self.data, &item)
    }

    pub fn rider_stats(&self, db: &CareerDb, profile: &Profile, rider: u32) -> ssx_assets::career::profile::RiderStats {
        use ssx_assets::career::profile::{RiderStats, SRC_NONE};
        let created = || RiderStats::created_for(db, rider, |k| self.catalog.entries.contains_key(&k));
        match profile.riders.get(&rider) {
            Some(s) if s.last_suit.1 == SRC_NONE && s.last_board.1 == SRC_NONE => {
                let c = created();
                RiderStats { last_suit: c.last_suit, last_board: c.last_board, ..s.clone() }
            }
            Some(s) => s.clone(),
            None => created(),
        }
    }

    pub(super) fn rider_suit(&self, db: &CareerDb, profile: &Profile, rider: u32) -> Option<(u32, u8)> {
        let (key, src) = self.rider_stats(db, profile, rider).last_suit;
        match src {
            0 => self.catalog.entries.get(&key).filter(|e| profile.offered(key, e)).map(|_| (key, 0)),
            1 => Some((key, 1)),
            _ => None,
        }
    }
}
