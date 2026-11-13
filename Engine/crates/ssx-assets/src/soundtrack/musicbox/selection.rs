use std::collections::{HashMap, VecDeque};
use std::sync::{Mutex, OnceLock};

use anyhow::{Context, Result, bail, ensure};
use ssx_formats::reader::Reader;
use ssx_formats::speech::grf::Graffiti;

use super::{MusicBox, RaceType};
use crate::sfx::speech::context::Lcg;

pub const EVENT: &str = "music_race_event";

#[derive(Clone, Debug)]
pub struct SelectionData {
    pub version: (u16, u16),
    pub unk_10: u32,
    pub buckets: u32,
    pub unk_bucket_ptr: u32,
    pub events: Vec<SelectionEvent>,
}

#[derive(Clone, Debug)]
pub struct SelectionEvent {
    pub name: String,
    pub rules: Vec<Rule>,
}

#[derive(Clone, Debug)]
pub struct Rule {
    pub priority: u32,
    pub tags: Vec<u32>,
}

pub fn fnv1(s: &str) -> u32 {
    s.bytes().fold(0x811C_9DC5u32, |h, b| h.wrapping_mul(0x0100_0193) ^ u32::from(b))
}

fn cstr(d: &[u8], o: usize) -> Result<String> {
    let s = d.get(o..).context("MBSE: string offset past the end")?;
    let end = s.iter().position(|&b| b == 0).context("MBSE: unterminated string")?;
    Ok(String::from_utf8_lossy(&s[..end]).into_owned())
}

pub fn parse_selections(d: &[u8]) -> Result<SelectionData> {
    let mut r = Reader::be(d);
    r.expect(b"MBSE")?;
    let version = (r.u16()?, r.u16()?);
    let (flag, table, unk_10) = (r.u32()?, r.u32()? as usize, r.u32()?);
    ensure!(version == (2, 2) && flag == 0, "MBSE: version {version:?}, loaded flag {flag}");
    r.seek(table)?;
    let (buckets, unk_bucket_ptr) = (r.u32()?, r.u32()?);
    ensure!((1..0x1000).contains(&buckets), "MBSE: {buckets} buckets");
    let mut events = Vec::new();
    for b in 0..buckets as usize {
        r.seek(table + 8 + 8 * b)?;
        let (n, off) = (r.u32()? as usize, r.u32()? as i32);
        if off < 0 {
            continue;
        }
        for e in 0..n {
            r.seek(off as usize + 0xC * e)?;
            let (count, name_at, rules_at) = (r.u32()? as usize, r.u32()? as usize, r.u32()? as usize);
            let name = cstr(d, name_at)?;
            if fnv1(&name) % buckets != b as u32 {
                bail!("MBSE: event {name} in bucket {b}, its hash says {}", fnv1(&name) % buckets);
            }
            let mut rules = Vec::with_capacity(count);
            for k in 0..count {
                r.seek(rules_at + 0xC * k)?;
                let (priority, tags, tags_at) = (r.u32()?, r.u32()? as usize, r.u32()? as usize);
                r.seek(tags_at)?;
                let tags = (0..tags).map(|_| Ok(r.u32()?)).collect::<Result<Vec<u32>>>()?;
                rules.push(Rule { priority, tags });
            }
            events.push(SelectionEvent { name, rules });
        }
    }
    Ok(SelectionData { version, unk_10, buckets, unk_bucket_ptr, events })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RaceEvent {
    pub game_mode: u32,
    pub race_type: u32,
}

impl RaceEvent {
    pub fn new(race: RaceType) -> Self {
        match race {
            RaceType::RaceIt => Self { game_mode: 1, race_type: 2 },
            RaceType::TrickIt => Self { game_mode: 1, race_type: 8 },
            RaceType::SurviveIt => Self { game_mode: 1, race_type: 4 },
            RaceType::Menu => Self { game_mode: 0, race_type: 0x10 },
        }
    }

    fn values(&self) -> [u32; 2] {
        [self.game_mode, self.race_type]
    }
}

impl MusicBox {
    pub fn candidates(&self, ev: &RaceEvent) -> Vec<u32> {
        let Some(event) = self.events.event(EVENT) else { return Vec::new() };
        let values = ev.values();
        let mut rules: Vec<&Rule> = self
            .selections
            .iter()
            .flat_map(|sel| sel.events.iter().filter(|e| e.name == EVENT))
            .flat_map(|e| &e.rules)
            .collect();
        let mut max = rules.iter().map(|r| r.priority).max().unwrap_or(0);
        let (mut level, mut out) = (1i32, Vec::new());
        while out.is_empty() && !rules.is_empty() {
            let (now, rest): (Vec<&Rule>, Vec<&Rule>) = rules.iter().partition(|r| r.priority >= max);
            for rule in now {
                let tags: Vec<(u32, u32, bool)> = rule
                    .tags
                    .iter()
                    .filter_map(|&t| {
                        let ty = self.events.param_type(t)?;
                        let i = event.params.iter().position(|&p| p == ty.id).unwrap_or(0);
                        Some((ty.id, values.get(i).copied().unwrap_or(0), ty.kind == 1))
                    })
                    .collect();
                for id in self.query(&tags) {
                    let Some(s) = self.song(id) else { continue };
                    if s.weight() == 0 || (s.level() as i32) < level {
                        continue;
                    }
                    if s.level() as i32 > level {
                        out.clear();
                        level = s.level() as i32;
                    }
                    out.push(id);
                    if s.level() == 3 {
                        break;
                    }
                }
            }
            rules = rest;
            if out.is_empty() {
                max = rules.iter().map(|r| r.priority).max().unwrap_or(0);
            }
        }
        if !self.package.is_empty() {
            return self.with_packages(ev, out);
        }
        out
    }

    pub fn query(&self, tags: &[(u32, u32, bool)]) -> Vec<u32> {
        let mut out = Vec::new();
        for db in &self.graffiti {
            out.extend(query_db(db, tags));
        }
        out
    }
}

fn query_db(db: &Graffiti, tags: &[(u32, u32, bool)]) -> Vec<u32> {
    let n = tags.len() as u32;
    let lookup = |&(tag, value, bits): &(u32, u32, bool)| -> Option<Vec<u32>> {
        if !bits {
            return db.find(tag, value, n).map(|e| e.samples.clone());
        }
        let mut v: Option<Vec<u32>> = None;
        for bit in (0..32).map(|k| 1u32 << k).filter(|b| value & b != 0) {
            if let Some(e) = db.find(tag, bit, n) {
                let v = v.get_or_insert_with(Vec::new);
                v.extend(e.samples.iter().filter(|s| !v.contains(s)).copied().collect::<Vec<_>>());
            }
        }
        v
    };
    let Some(mut set) = tags.first().and_then(lookup) else { return Vec::new() };
    for t in &tags[1..] {
        if set.is_empty() {
            break;
        }
        let other = lookup(t).unwrap_or_default();
        set.retain(|s| other.contains(s));
    }
    set
}

#[derive(Clone, Debug)]
pub struct Session {
    seed: u32,
    rng: Lcg,
    history: HashMap<u32, VecDeque<u32>>,
    pub picks: Vec<u32>,
}

impl Session {
    pub fn seeded(seed: u32) -> Self {
        Self { seed, rng: Lcg(seed), history: HashMap::new(), picks: Vec::new() }
    }

    pub fn boot() -> Self {
        let fixed = std::env::var("SSX_MUSIC_SEED").ok().and_then(|v| {
            let v = v.trim();
            v.strip_prefix("0x").map_or_else(|| v.parse().ok(), |h| u32::from_str_radix(h, 16).ok())
        });
        Self::seeded(fixed.unwrap_or_else(crate::career::random::time_base))
    }

    pub fn seed(&self) -> u32 {
        self.seed
    }

    pub fn state(&self) -> u32 {
        self.rng.0
    }

    pub fn score(&self, pool: u32, id: u32) -> f32 {
        let h = self.history.get(&pool);
        h.and_then(|h| h.iter().rev().position(|&s| s == id)).map_or(0.0, |k| 1.0 / (k + 1) as f32)
    }

    pub fn note(&mut self, pool: u32, size: u32, id: u32) {
        let h = self.history.entry(pool).or_default();
        if size > 0 && h.len() >= size as usize {
            h.pop_front();
        }
        if (h.len() as u32) < size {
            h.push_back(id);
        }
    }

    pub fn select(&mut self, mb: &MusicBox, ev: &RaceEvent) -> Option<u32> {
        let info = |id: u32| {
            let s = mb.song(id)?;
            Some((s.weight(), mb.pools.pool(s.unk[5]).map(|p| (p.id, p.size))))
        };
        self.select_from(mb.candidates(ev), info)
    }

    pub fn select_from(
        &mut self,
        mut ids: Vec<u32>,
        info: impl Fn(u32) -> Option<(u32, Option<(u32, u32)>)>,
    ) -> Option<u32> {
        let pool = |id: u32| info(id).and_then(|(_, p)| p);
        let mut min = f64::from(1.1f32);
        for &id in &ids {
            if let Some((p, _)) = pool(id) {
                min = min.min(f64::from(self.score(p, id)));
            }
        }
        ids.retain(|&id| pool(id).is_none_or(|(p, _)| f64::from(self.score(p, id)) <= min));
        if ids.is_empty() {
            return None;
        }
        let weight = |id: u32| info(id).map_or(0, |(w, _)| w);
        let sum = ids.iter().map(|&id| u64::from(weight(id))).sum::<u64>();
        let draw = self.rng.below(sum as u32);
        let mut acc = 0u32;
        let i = ids.iter().position(|&id| {
            acc = acc.wrapping_add(weight(id));
            draw < acc
        });
        let id = ids[i.unwrap_or(ids.len() - 1)];
        if let Some((p, size)) = pool(id) {
            self.note(p, size, id);
        }
        self.picks.push(id);
        Some(id)
    }
}

static SESSION: OnceLock<Mutex<Session>> = OnceLock::new();

pub fn session() -> &'static Mutex<Session> {
    SESSION.get_or_init(|| Mutex::new(Session::boot()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mbse(names: &[&str]) -> Vec<u8> {
        let mut d = b"MBSE".to_vec();
        d.extend([0, 2, 0, 2, 0, 0, 0, 0, 0, 0, 0, 0x14, 0, 0, 0, 0]);
        let w = |d: &mut Vec<u8>, v: u32| d.extend(v.to_be_bytes());
        w(&mut d, 2);
        w(&mut d, 0);
        let at = |b: u32| 0x2C + 0xC * names.iter().filter(|n| fnv1(n) % 2 < b).count() as u32;
        for b in 0..2 {
            let n = names.iter().filter(|n| fnv1(n) % 2 == b).count() as u32;
            w(&mut d, n);
            w(&mut d, if n == 0 { u32::MAX } else { at(b) });
        }
        let mut sorted: Vec<&&str> = names.iter().collect();
        sorted.sort_by_key(|n| fnv1(n) % 2);
        let tail = 0x2C + 0xC * names.len() as u32;
        for (i, _) in sorted.iter().enumerate() {
            w(&mut d, 1);
            w(&mut d, tail + 0x20 * i as u32 + 0x14);
            w(&mut d, tail + 0x20 * i as u32);
        }
        for (i, n) in sorted.iter().enumerate() {
            let base = tail + 0x20 * i as u32;
            for v in [2, 2, base + 0xC, 1, 7] {
                w(&mut d, v);
            }
            let mut name = n.as_bytes().to_vec();
            name.resize(12, 0);
            d.extend(name);
        }
        d
    }

    #[test]
    fn selection_data_parses_events_into_their_buckets() {
        let d = mbse(&["race", "menu_x"]);
        let s = parse_selections(&d).unwrap();
        assert_eq!(s.buckets, 2);
        let mut names: Vec<&str> = s.events.iter().map(|e| e.name.as_str()).collect();
        names.sort();
        assert_eq!(names, ["menu_x", "race"]);
        for e in &s.events {
            assert_eq!(e.rules.len(), 1);
            assert_eq!((e.rules[0].priority, e.rules[0].tags.as_slice()), (2, [1, 7].as_slice()));
        }
    }

    #[test]
    fn the_draw_is_weighted_and_a_song_just_played_waits() {
        let info = |id: u32| Some((if id == 2 { 1 } else { 1000 }, Some((4, 2))));
        let mut s = Session::seeded(0x1234_5678);
        let mut lcg = Lcg(0x1234_5678);
        let draw = lcg.below(2001);
        let first = s.select_from(vec![1, 2, 3], info).unwrap();
        assert_eq!(
            first,
            if draw < 1000 {
                1
            } else if draw < 1001 {
                2
            } else {
                3
            }
        );
        assert_eq!(s.state(), lcg.0, "one draw per pick");
        for _ in 0..20 {
            let next = s.select_from(vec![1, 2, 3], info).unwrap();
            assert_ne!(next, s.picks[s.picks.len() - 2]);
        }
        assert!(s.select_from(Vec::new(), info).is_none());
    }

    #[test]
    fn the_pool_forgets_beyond_its_size() {
        let mut s = Session::seeded(1);
        for id in [5, 6, 7] {
            s.note(4, 2, id);
        }
        assert_eq!((s.score(4, 7), s.score(4, 6), s.score(4, 5)), (1.0, 0.5, 0.0));
        s.note(9, 0, 1);
        assert_eq!(s.score(9, 1), 0.0, "a pool of size 0 keeps nothing");
    }

    #[test]
    fn the_race_types_post_the_games_parameters() {
        assert_eq!(RaceEvent::new(RaceType::RaceIt).values(), [1, 2]);
        assert_eq!(RaceEvent::new(RaceType::TrickIt).values(), [1, 8]);
        assert_eq!(RaceEvent::new(RaceType::SurviveIt).values(), [1, 4]);
        assert_eq!(RaceEvent::new(RaceType::Menu).values(), [0, 0x10]);
    }
}
