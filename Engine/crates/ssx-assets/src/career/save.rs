use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use anyhow::{Context, Result, anyhow, bail};
use ssx_formats::attrib::fnv1;
use ssx_formats::save::ProfileFile;
use ssx_formats::save::chunks::{
    ActivityChunk, BadgeChunk, DatabaseChunk, FlowChunk, FriendsChunk, FrontEndChunk, GeotagChunk, KeySetChunk,
    ONLINE_BYTES, StatRecord, StatsChunk, TuningChunk,
};
use ssx_formats::t3db::{Database, MetaSchema, Table};

use super::profile::{DropCounts, DropState, ExploreScore, GeneratedItem, Profile, RiderStats};
use super::world_tour;
use crate::sfx::settings::AudioSettings;
use crate::sfx::store::AudioStore;

pub const PROFILE_GROUP: &str = "SSXV_PROFILE";

#[derive(Clone, Debug)]
pub struct SaveSchema {
    pub database: Database,
    pub meta: MetaSchema,
    pub riders: Vec<(u32, String)>,
    pub items: BTreeSet<u32>,
    pub drops: Vec<u32>,
    pub audio: Option<(AudioSettings, Vec<(u32, u32)>)>,
}

impl SaveSchema {
    pub fn load(big_root: &Path, db: &super::CareerDb) -> Result<Self> {
        let dir = big_root.join("bootcache/data/xenon/database");
        let database = Database::parse(&std::fs::read(dir.join("ssxv.db")).context("reading ssxv.db")?)?;
        let meta = MetaSchema::parse(&std::fs::read_to_string(dir.join("ssxv-meta.xml")).context("reading meta")?)?;
        Ok(SaveSchema {
            database,
            meta,
            riders: db
                .riders
                .iter()
                .filter(|r| r.content.as_ref().is_none_or(|c| !c.added))
                .map(|r| (r.key, r.name.clone()))
                .collect(),
            items: db.items.iter().filter(|i| i.content.as_ref().is_none_or(|c| !c.added)).map(|i| i.key).collect(),
            drops: db.events.iter().map(|e| e.key).collect(),
            audio: Self::load_audio(big_root),
        })
    }

    fn load_audio(big_root: &Path) -> Option<(AudioSettings, Vec<(u32, u32)>)> {
        let ranges = crate::sfx::settings::FeRanges::load(big_root).ok()?;
        let songs = crate::sfx::store::load_in_game_music(big_root).ok()?;
        Some((AudioSettings::defaults(&ranges), songs))
    }

    pub fn fresh_audio_store(&self) -> Option<AudioStore> {
        self.audio.as_ref().map(|(fe, songs)| AudioStore::new(fe, songs))
    }

    fn short(&self, table: &str, field: &str) -> Result<String> {
        self.field(table, field).map(|f| f.short.clone())
    }

    pub fn coded(&self, table: &str, field: &str) -> Result<(String, bool)> {
        let f = self.field(table, field)?;
        Ok((f.short.clone(), stored_offset_binary(f)))
    }

    fn field<'a>(&'a self, table: &str, field: &str) -> Result<&'a ssx_formats::t3db::MetaField> {
        let t = self.meta.table(table).ok_or_else(|| anyhow!("schema has no table {table}"))?;
        t.fields.iter().find(|f| f.name == field).ok_or_else(|| anyhow!("no {table}.{field}"))
    }

    fn offset_binary(&self, db: &ssx_formats::t3db::Database) -> bool {
        let known: [u32; 7] = [
            crate::career::profile::ATTRIB_CREDITS,
            crate::career::profile::ATTRIB_EARNED,
            crate::career::profile::ATTRIB_MPUR,
            crate::career::profile::ATTRIB_CPRO,
            crate::career::profile::ATTRIB_CTYP,
            crate::career::profile::ATTRIB_LAST_RIDER,
            crate::career::profile::ATTRIB_FLAGS,
        ];
        let vote = |table: &str, field: &str, keys: &dyn Fn(u32) -> bool| -> (usize, usize) {
            let Ok(short_table) = self.table_short(table) else { return (0, 0) };
            let Some(t) = db.table(&short_table) else { return (0, 0) };
            let Ok(name) = self.short(table, field) else { return (0, 0) };
            let mut raw = 0;
            let mut coded = 0;
            for row in t.live_rows() {
                if let Some(v) = t.get(row, &name) {
                    raw += usize::from(keys(v));
                    coded += usize::from(keys(v ^ 0x8000_0000));
                }
            }
            (raw, coded)
        };
        let (ra, ca) = vote("attribs", "NAME", &|v| known.contains(&v));
        let (rc, cc) = vote("character_stats", "CHARID", &|v| self.riders.iter().any(|r| r.0 == v));
        let (raw, coded) = (ra + rc, ca + cc);
        if raw == coded { true } else { coded > raw }
    }

    fn table_short(&self, table: &str) -> Result<String> {
        Ok(self.meta.table(table).ok_or_else(|| anyhow!("schema has no table {table}"))?.short.clone())
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct SaveExtras {
    pub online: Vec<u8>,
    pub friends: FriendsChunk,
    pub tuning: TuningChunk,
    pub activity: ActivityChunk,
    pub front_end: FrontEndChunk,
    pub online_geotags: Vec<u64>,
    pub flow_time: u32,
    pub audio: Vec<u8>,
    pub tail: Vec<u8>,
}

impl SaveExtras {
    pub fn audio_settings(&self) -> Option<AudioSettings> {
        AudioStore::parse(&self.audio).ok()?.settings()
    }

    pub fn store_audio(&mut self, schema: &SaveSchema, fe: &AudioSettings) {
        let Some((_, songs)) = schema.audio.as_ref() else { return };
        let base = AudioStore::parse(&self.audio).ok().or_else(|| schema.fresh_audio_store());
        if let Some(st) = base {
            self.audio = st.with_session(fe, songs).write();
        }
    }

    pub fn song_flags(&self) -> Vec<(u32, bool)> {
        AudioStore::parse(&self.audio)
            .map(|st| st.in_game_music.iter().filter(|e| e.0 != 0).map(|&(id, on)| (id, on != 0)).collect())
            .unwrap_or_default()
    }

    pub fn set_song_flags(&mut self, schema: &SaveSchema, flags: &[(u32, bool)]) {
        let Some(mut st) = AudioStore::parse(&self.audio).ok().or_else(|| schema.fresh_audio_store()) else { return };
        for e in &mut st.in_game_music {
            if let Some(&(_, on)) = flags.iter().find(|(id, _)| *id == e.0 && e.0 != 0) {
                e.1 = u32::from(on);
            }
        }
        self.audio = st.write();
    }

    // TODO(guess): the fresh contents of the online record (0x410 bytes), the tuning buffer, the activity words, the controller scheme and the save times taken as zero / empty (the audio store is `SaveSchema::fresh_audio_store`); settle: a console save of a new profile.
    pub fn fresh() -> Self {
        SaveExtras {
            online: vec![0; ONLINE_BYTES],
            friends: FriendsChunk::default(),
            tuning: TuningChunk::default(),
            activity: ActivityChunk::default(),
            front_end: FrontEndChunk::default(),
            online_geotags: Vec::new(),
            flow_time: 0,
            audio: Vec::new(),
            tail: Vec::new(),
        }
    }
}

const RIDER_SLOTS: [&str; 10] =
    ["SUIT", "BOARD", "GEAR0", "GEAR1", "GEAR2", "GEAR3", "GEOTAG0", "GEOTAG1", "GEOTAG2", "GEOTAG3"];

// TODO(guess): the flip's place in the game is not found, so the rule is generalised from those values to every
// signed 32-bit field; tried: `T3db_PutField` 0x830161C8 and the getter 0x830160F0 move the bits unchanged, the
// profile getter 0x824A8A28 -> 0x83030A38 -> 0x8302B6A8 returns the query's 4-byte cell (+0x28) unchanged, the
// insert 0x8301DC50 passes the caller's values through, and an `xoris 0x8000` scan of the image found no flip; not
// traced: the lazy cell fill (0x8302B6A8 calls the column's +0x10 for a type-3 or 0xDEAD cell). It is not
// `value − rangelow`: the 7-bit `STATTYPE1…6` (`rangelow` −1) hold a real 33 raw. Settle: that callback's conversion.
fn stored_offset_binary(f: &ssx_formats::t3db::MetaField) -> bool {
    f.depth == 32 && f.range.0 < 0
}

fn stat(key: u32, value: u32) -> StatRecord {
    StatRecord { key, value, key_again: key, scope: 2, kind: 0, initial: 0 }
}

const MEDAL_STATS: [&str; 3] = ["MedalBronzeNumEarned_", "MedalSilverNumEarned_", "MedalGoldNumEarned_"];

pub fn medal_stat_name(medal: usize, drop: u32) -> String {
    format!("{}{drop}", MEDAL_STATS[medal])
}

fn medal_key(medal: usize, drop: u32) -> u32 {
    fnv1(medal_stat_name(medal, drop).as_bytes())
}

pub fn movie_stat_name(movie: &str) -> String {
    format!("MovieUnlocked_{movie}")
}

fn movie_key(movie: &str) -> u32 {
    fnv1(movie_stat_name(movie).as_bytes())
}

pub fn modelled_stats(schema: &SaveSchema) -> BTreeMap<u32, String> {
    let mut out = BTreeMap::new();
    for &d in &schema.drops {
        for m in 0..3 {
            out.insert(medal_key(m, d), medal_stat_name(m, d));
        }
    }
    for m in world_tour::MOVIES {
        out.insert(movie_key(m.0), movie_stat_name(m.0));
    }
    out
}

impl Profile {
    pub fn persistent_records(&self) -> Vec<StatRecord> {
        let mut out: BTreeMap<u32, StatRecord> = self.stats.records.clone();
        for (&drop, counts) in &self.medals {
            for (m, &c) in counts.iter().enumerate().filter(|(_, c)| **c > 0) {
                out.insert(medal_key(m, drop), stat(medal_key(m, drop), c));
            }
        }
        for m in &self.movies {
            out.insert(movie_key(m), stat(movie_key(m), 1));
        }
        out.into_values().collect()
    }
}

fn fill(table: &mut Table, capacity: u16, rows: &[Vec<(String, u32)>]) -> Result<()> {
    // TODO(guess): the game's capacity is the live rows at the last open + `maxinsert` (0x83035628) with its insert / delete history; the port writes `maxinsert` (or the live rows) in its own order, no free list; settle: a console save.
    table.reserve(capacity.max(rows.len() as u16));
    for r in rows {
        let i = table.insert_row()?;
        for (f, v) in r {
            table.put(i, f, *v)?;
        }
    }
    Ok(())
}

impl Profile {
    pub fn to_save(&self, schema: &SaveSchema, extras: &SaveExtras) -> Result<ProfileFile> {
        if self.mod_keys.is_empty() {
            return self.disc_save(schema, extras);
        }
        self.disc_view().disc_save(schema, extras)
    }

    fn disc_save(&self, schema: &SaveSchema, extras: &SaveExtras) -> Result<ProfileFile> {
        let mut db = schema.database.clone();
        let group_tables: Vec<(String, u16)> = schema
            .meta
            .tables
            .iter()
            .filter(|t| t.save_groups.as_deref() == Some(PROFILE_GROUP))
            .map(|t| (t.short.clone(), t.max_insert.unwrap_or(0).clamp(0, 0xFFFF) as u16))
            .collect();
        for t in &mut db.tables {
            if !group_tables.iter().any(|(s, _)| *s == t.name_str()) {
                t.flags &= !ssx_formats::t3db::FLAG_WRITTEN;
            }
        }
        let cap = |s: &str| group_tables.iter().find(|(n, _)| n == s).map_or(0, |(_, c)| *c);
        let f = |t: &str, n: &str| schema.short(t, n);
        let coded: std::collections::BTreeMap<(String, String), bool> = schema
            .meta
            .tables
            .iter()
            .flat_map(|t| t.fields.iter().map(move |f| ((t.short.clone(), f.short.clone()), stored_offset_binary(f))))
            .filter(|(_, signed)| *signed)
            .collect();
        let mut put_table = |name: &str, mut rows: Vec<Vec<(String, u32)>>| -> Result<()> {
            let short = schema.table_short(name)?;
            for row in &mut rows {
                for (field, value) in row {
                    if coded.contains_key(&(short.clone(), field.clone())) {
                        *value ^= 0x8000_0000;
                    }
                }
            }
            let t = db.table_mut(&short).ok_or_else(|| anyhow!("ssxv.db has no {short}"))?;
            fill(t, cap(&short), &rows)
        };
        let rows = self
            .attribs
            .iter()
            .map(|(&k, &v)| Ok(vec![(f("attribs", "NAME")?, k), (f("attribs", "VALUE")?, v as u32)]))
            .collect::<Result<Vec<_>>>()?;
        put_table("attribs", rows)?;
        let mut riders = schema.riders.clone();
        for &key in self.riders.keys() {
            if !riders.iter().any(|(k, _)| *k == key) {
                riders.push((key, String::new()));
            }
        }
        let mut rows = Vec::new();
        for (key, name) in &riders {
            let created = RiderStats::created(name, |k| schema.items.contains(&k));
            let mut s = self.riders.get(key).cloned().unwrap_or_else(|| created.clone());
            if s.last_suit == RiderStats::default().last_suit {
                s.last_suit = created.last_suit;
            }
            if s.last_board == RiderStats::default().last_board {
                s.last_board = created.last_board;
            }
            let mut r = vec![
                (f("character_stats", "CHARID")?, *key),
                (f("character_stats", "MONEYEARNED")?, s.money_earned as u32),
                (f("character_stats", "NEXTLEVELOFFSET")?, s.next_level_offset as u32),
                (f("character_stats", "MAXACHIEVEDLEVEL")?, s.max_achieved_level as u32),
                (f("character_stats", "MAXPURCHASEDLEVEL")?, s.max_purchased_level as u32),
            ];
            let slots: Vec<(u32, u8)> =
                [s.last_suit, s.last_board].into_iter().chain(s.last_gear).chain(s.last_geotags).collect();
            for (slot, (id, src)) in RIDER_SLOTS.iter().zip(slots) {
                r.push((f("character_stats", &format!("LASTEQUIPPED_{slot}_ID"))?, id));
                r.push((f("character_stats", &format!("LASTEQUIPPED_{slot}_SRC"))?, u32::from(src)));
            }
            rows.push(r);
        }
        put_table("character_stats", rows)?;
        let keys = |t: &str, field: &str, set: &BTreeSet<u32>| -> Result<Vec<Vec<(String, u32)>>> {
            set.iter().map(|&k| Ok(vec![(f(t, field)?, k)])).collect()
        };
        put_table("equipment", keys("equipment", "NAME", &self.owned_items)?)?;
        let unlocked: BTreeSet<u32> = self.unlocked_riders.union(&self.unlocked_items).copied().collect();
        put_table("equip_unlockables", keys("equip_unlockables", "EQUIPID", &unlocked)?)?;
        let rows = self.generated.values().map(|g| generated_row(g, &f)).collect::<Result<Vec<_>>>()?;
        put_table("equip_generated", rows)?;
        let mut rows = Vec::new();
        for (&drop, e) in &self.explore_scores {
            let t = "explore_scores";
            let mut r = vec![
                (f(t, "DROPID")?, drop),
                (f(t, "SCORE")?, e.score),
                (f(t, "SCOREMODIFIED")?, e.unk_score_modified),
                (f(t, "ATTEMPTS")?, e.attempts),
                (f(t, "ATTEMPTSREMAINING")?, e.unk_attempts_remaining),
                (f(t, "DEATHS")?, e.deaths),
                (f(t, "REWINDS")?, e.rewinds),
            ];
            for (i, s) in e.splits.iter().enumerate() {
                r.push((f(t, &format!("SPLIT{}", i + 1))?, *s));
            }
            rows.push(r);
        }
        put_table("explore_scores", rows)?;
        let database = DatabaseChunk::from_database(&db);
        let flow = FlowChunk {
            peaks: self.peak_modes.iter().map(|(&k, &v)| (k, v)).collect(),
            ranges: self.range_modes.iter().map(|(&k, &v)| (k, v)).collect(),
            drops: self.drop_states.iter().map(|(&k, &v)| (k, v as u32)).collect(),
            chapter_drops: self.chapter_drops.clone(),
            counts: self.drop_counts.iter().map(|(&k, c)| (k, c.completions, c.failures, c.unk_flag)).collect(),
            bought: self.purchased.iter().map(|&k| (k, 1)).collect(),
            time: extras.flow_time,
            new_game_state: self.new_game_state,
            focus: [
                self.campaign_globe.range_peak.iter().map(|(&k, &v)| (k, v)).collect(),
                self.campaign_globe.peak_drop.iter().map(|(&k, &v)| (k, v)).collect(),
                self.campaign_globe.range_selections.iter().map(|(&k, &v)| (k, v)).collect(),
            ],
            current_range: self.campaign_globe.range,
            last_drop: self.globe_last_drop,
            audio: match (extras.audio.is_empty(), schema.fresh_audio_store()) {
                (true, Some(st)) => st.write(),
                _ => extras.audio.clone(),
            },
        };
        let stats = StatsChunk {
            awards: self.stats.awards.iter().map(|(&k, &w)| (k, w)).collect(),
            persistent: self.persistent_records(),
        };
        let collected: Vec<u64> = self.collected_geotags.iter().copied().collect();
        let keys = KeySetChunk { keys: self.named_flags.iter().copied().collect() };
        let mut tail = extras.tail.clone();
        if tail.len() < 3 {
            tail.resize(3, 0);
        }
        tail[..3].copy_from_slice(&keys.spill());
        let chunks = vec![
            database.write(),
            extras.online.clone(),
            flow.write(),
            stats.write(),
            BadgeChunk { badges: self.recent_badges.clone() }.write(),
            GeotagChunk::from_sets(&collected, &extras.online_geotags).write(),
            extras.friends.write(),
            extras.tuning.write(),
            extras.activity.write(),
            extras.front_end.write(),
            keys.write(),
        ];
        Ok(ProfileFile { chunks, tail })
    }

    pub fn from_save(file: &ProfileFile, schema: &SaveSchema) -> Result<(Profile, SaveExtras)> {
        if file.chunks.len() != ssx_formats::save::PROFILE_CHUNKS {
            bail!("{} chunks", file.chunks.len());
        }
        let c = &file.chunks;
        let mut p = Profile::default();
        let db = DatabaseChunk::parse(&c[0])?.database()?;
        let coded = schema.offset_binary(&db);
        let table = |name: &str| -> Result<&Table> {
            let s = schema.table_short(name)?;
            db.table(&s).ok_or_else(|| anyhow!("the save has no table {name}"))
        };
        let get = |t: &Table, row: usize, name: &str, field: &str| -> Result<u32> {
            let (short, flip) = schema.coded(name, field)?;
            let v = t.get(row, &short).ok_or_else(|| anyhow!("{name}.{field} row {row}"))?;
            Ok(if coded && flip { v ^ 0x8000_0000 } else { v })
        };
        let t = table("attribs")?;
        for r in t.live_rows() {
            p.attribs.insert(get(t, r, "attribs", "NAME")?, i64::from(get(t, r, "attribs", "VALUE")? as i32));
        }
        let t = table("character_stats")?;
        for r in t.live_rows() {
            let g = |field: &str| get(t, r, "character_stats", field);
            let slot = |s: &str| -> Result<(u32, u8)> {
                Ok((g(&format!("LASTEQUIPPED_{s}_ID"))?, g(&format!("LASTEQUIPPED_{s}_SRC"))? as u8))
            };
            let mut s = RiderStats {
                money_earned: i64::from(g("MONEYEARNED")? as i32),
                next_level_offset: i64::from(g("NEXTLEVELOFFSET")? as i32),
                max_achieved_level: i64::from(g("MAXACHIEVEDLEVEL")? as i32),
                max_purchased_level: i64::from(g("MAXPURCHASEDLEVEL")? as i32),
                last_suit: slot("SUIT")?,
                last_board: slot("BOARD")?,
                ..Default::default()
            };
            for i in 0..4 {
                s.last_gear[i] = slot(&format!("GEAR{i}"))?;
                s.last_geotags[i] = slot(&format!("GEOTAG{i}"))?;
            }
            p.riders.insert(g("CHARID")?, s);
        }
        let t = table("equipment")?;
        for r in t.live_rows() {
            p.owned_items.insert(get(t, r, "equipment", "NAME")?);
        }
        let riders: BTreeSet<u32> = schema.riders.iter().map(|r| r.0).collect();
        let t = table("equip_unlockables")?;
        for r in t.live_rows() {
            let k = get(t, r, "equip_unlockables", "EQUIPID")?;
            if riders.contains(&k) {
                p.unlocked_riders.insert(k)
            } else {
                p.unlocked_items.insert(k)
            };
        }
        let t = table("equip_generated")?;
        for r in t.live_rows() {
            let g = read_generated(&|field: &str| get(t, r, "equip_generated", field))?;
            p.generated.insert(g.equip_id, g);
        }
        let t = table("explore_scores")?;
        for r in t.live_rows() {
            let g = |field: &str| get(t, r, "explore_scores", field);
            let e = ExploreScore {
                score: g("SCORE")?,
                splits: [g("SPLIT1")?, g("SPLIT2")?, g("SPLIT3")?, g("SPLIT4")?],
                attempts: g("ATTEMPTS")?,
                deaths: g("DEATHS")?,
                rewinds: g("REWINDS")?,
                unk_score_modified: g("SCOREMODIFIED")?,
                unk_attempts_remaining: g("ATTEMPTSREMAINING")?,
            };
            p.explore_scores.insert(g("DROPID")?, e);
        }
        let flow = FlowChunk::parse(&c[2])?;
        p.peak_modes = flow.peaks.iter().copied().collect();
        p.range_modes = flow.ranges.iter().copied().collect();
        for &(k, s) in &flow.drops {
            let s = match s {
                0 => DropState::Locked,
                1 => DropState::Unlocked,
                2 => DropState::Completed,
                x => bail!("drop {k:08x} in state {x}"),
            };
            p.drop_states.insert(k, s);
        }
        p.chapter_drops = flow.chapter_drops.clone();
        p.drop_counts = flow
            .counts
            .iter()
            .map(|&(k, completions, failures, unk_flag)| (k, DropCounts { completions, failures, unk_flag }))
            .collect();
        p.purchased = flow.bought.iter().map(|b| b.0).collect();
        p.load_new_game_state(flow.new_game_state);
        p.globe_last_drop = flow.last_drop;
        let [range_peak, peak_drop, range_selections] = &flow.focus;
        p.campaign_globe = super::globe::focus::CampaignGlobe {
            range: flow.current_range,
            range_peak: range_peak.iter().copied().collect(),
            peak_drop: peak_drop.iter().copied().collect(),
            range_selections: range_selections.iter().copied().collect(),
        };
        let stats = StatsChunk::parse(&c[3])?;
        let mut named: BTreeMap<u32, (u32, usize)> = BTreeMap::new();
        for &d in &schema.drops {
            for m in 0..3 {
                named.insert(medal_key(m, d), (d, m));
            }
        }
        let movies: BTreeMap<u32, &str> = world_tour::MOVIES.iter().map(|m| (movie_key(m.0), m.0)).collect();
        for s in &stats.persistent {
            if let Some(&(drop, m)) = named.get(&s.key) {
                p.medals.entry(drop).or_default()[m] = s.value;
            } else if let Some(movie) = movies.get(&s.key) {
                p.movies.insert(movie.to_string());
            } else {
                p.stats.records.insert(s.key, *s);
            }
        }
        p.stats.awards = stats.awards.iter().copied().collect();
        p.recent_badges = BadgeChunk::parse(&c[4])?.badges;
        p.named_flags = KeySetChunk::parse(&c[10], &file.tail)?.keys.into_iter().collect();
        let geo = GeotagChunk::parse(&c[5])?;
        let (collected, online) = geo.sets();
        p.collected_geotags = collected.into_iter().collect();
        let extras = SaveExtras {
            online: c[1].clone(),
            friends: FriendsChunk::parse(&c[6])?,
            tuning: TuningChunk::parse(&c[7])?,
            activity: ActivityChunk::parse(&c[8])?,
            front_end: FrontEndChunk::parse(&c[9])?,
            online_geotags: online,
            flow_time: flow.time,
            audio: flow.audio,
            tail: if file.tail.iter().skip(3).all(|&b| b == 0) { Vec::new() } else { file.tail.clone() },
        };
        Ok((p, extras))
    }
}

fn generated_row(g: &GeneratedItem, f: &dyn Fn(&str, &str) -> Result<String>) -> Result<Vec<(String, u32)>> {
    let t = "equip_generated";
    let mut r = vec![
        (f(t, "EQUIPID")?, g.equip_id),
        (f(t, "TYPE")?, g.kind as u32),
        (f(t, "GEARTYPE")?, g.slot as u32),
        (f(t, "LEVEL")?, g.level as u32),
        (f(t, "RARITY")?, g.rarity as u32),
        (f(t, "COST")?, g.cost as u32),
        (f(t, "CHARLOCK")?, g.charlock),
        (f(t, "DESCKEY")?, g.descriptions[0]),
        (f(t, "DESCKEY2")?, g.descriptions[1]),
        (f(t, "WALLID")?, g.wall_id as u32),
        (f(t, "SPONSORID")?, g.sponsor as u32),
        (f(t, "NAMEFORMATID")?, g.name_format[0] as u32),
        (f(t, "DESCRIPTORID")?, g.name_format[1] as u32),
        (f(t, "STATTYPEID")?, g.name_format[2] as u32),
        (f(t, "EQUIPTYPEID")?, g.name_format[3] as u32),
    ];
    for (i, (ty, v)) in g.stats.iter().enumerate() {
        r.push((f(t, &format!("STATTYPE{}", i + 1))?, *ty as u32));
        r.push((f(t, &format!("STAT{}", i + 1))?, u32::from(*v)));
    }
    Ok(r)
}

fn read_generated(g: &dyn Fn(&str) -> Result<u32>) -> Result<GeneratedItem> {
    let signed7 = |v: u32| if v == 0x7F { -1 } else { i64::from(v) };
    let mut item = GeneratedItem {
        equip_id: g("EQUIPID")?,
        kind: i64::from(g("TYPE")?),
        slot: signed7(g("GEARTYPE")?),
        level: i64::from(g("LEVEL")?),
        rarity: i64::from(g("RARITY")?),
        cost: i64::from(g("COST")? as i32),
        charlock: g("CHARLOCK")?,
        descriptions: [g("DESCKEY")?, g("DESCKEY2")?],
        wall_id: i64::from(g("WALLID")? as i32),
        sponsor: i64::from(g("SPONSORID")? as i32),
        name_format: [
            i64::from(g("NAMEFORMATID")?),
            i64::from(g("DESCRIPTORID")?),
            i64::from(g("STATTYPEID")?),
            i64::from(g("EQUIPTYPEID")?),
        ],
        ..Default::default()
    };
    for (i, stat) in item.stats.iter_mut().enumerate() {
        *stat = (signed7(g(&format!("STATTYPE{}", i + 1))?), g(&format!("STAT{}", i + 1))? as u8);
    }
    Ok(item)
}

pub fn write_file(profile: &Profile, schema: &SaveSchema, extras: &SaveExtras, path: &Path) -> Result<Vec<u8>> {
    let bytes = profile.to_save(schema, extras)?.write();
    ssx_files::replace(path, &bytes, &ssx_files::Options::new())
        .with_context(|| format!("writing {}", path.display()))?;
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::career::profile::{DropState, GeneratedItem};

    type TestTable = (&'static str, &'static str, i64, &'static str, Vec<(String, u32)>);

    fn schema_with(signed: bool) -> SaveSchema {
        let slots = RIDER_SLOTS
            .iter()
            .flat_map(|s| [(format!("LASTEQUIPPED_{s}_ID"), 32), (format!("LASTEQUIPPED_{s}_SRC"), 2)]);
        let stats = (1..=6).flat_map(|i| [(format!("STATTYPE{i}"), 7), (format!("STAT{i}"), 7)]);
        let tables: Vec<TestTable> = vec![
            ("attribs", "taaa", 100, PROFILE_GROUP, vec![("NAME".into(), 32), ("VALUE".into(), 32)]),
            (
                "character_stats",
                "tbbb",
                20,
                PROFILE_GROUP,
                ["CHARID", "MONEYEARNED", "NEXTLEVELOFFSET", "MAXACHIEVEDLEVEL", "MAXPURCHASEDLEVEL"]
                    .iter()
                    .map(|n| (n.to_string(), 32))
                    .chain(slots)
                    .collect(),
            ),
            ("equipment", "tccc", 150, PROFILE_GROUP, vec![("NAME".into(), 32)]),
            ("equip_unlockables", "tddd", 50, PROFILE_GROUP, vec![("EQUIPID".into(), 32)]),
            (
                "equip_generated",
                "teee",
                500,
                PROFILE_GROUP,
                [("EQUIPID", 32), ("COST", 32), ("CHARLOCK", 32), ("DESCKEY", 32), ("DESCKEY2", 32), ("WALLID", 32)]
                    .iter()
                    .map(|(n, d)| (n.to_string(), *d))
                    .chain(
                        [("SPONSORID", 32), ("TYPE", 4), ("GEARTYPE", 7), ("LEVEL", 6), ("RARITY", 3)]
                            .map(|(n, d)| (n.to_string(), d)),
                    )
                    .chain(["NAMEFORMATID", "DESCRIPTORID", "STATTYPEID", "EQUIPTYPEID"].map(|n| (n.to_string(), 5)))
                    .chain(stats)
                    .collect(),
            ),
            (
                "explore_scores",
                "tfff",
                400,
                PROFILE_GROUP,
                [
                    "DROPID",
                    "SCORE",
                    "SCOREMODIFIED",
                    "SPLIT1",
                    "SPLIT2",
                    "SPLIT3",
                    "SPLIT4",
                    "ATTEMPTS",
                    "ATTEMPTSREMAINING",
                    "DEATHS",
                    "REWINDS",
                ]
                .iter()
                .map(|n| (n.to_string(), 32))
                .collect(),
            ),
            ("common_explore_scores", "tggg", 300, "SSXV_COMMON", vec![("TRACKID".into(), 32)]),
        ];
        let mut xml = String::from("<database name=\"x\" shortname=\"y\" version=\"6\">");
        let mut n = 0;
        for (name, short, max, group, fields) in &tables {
            xml += &format!(
                "<table name=\"{name}\" shortname=\"{short}\" maxinsert=\"{max}\" savegroups=\"{group}\"><fields>"
            );
            for (f, d) in fields {
                n += 1;
                let (low, high) = if signed && *d == 32 { ("-2147483648", "2147483647") } else { ("0", "1") };
                xml += &format!(
                    "<field name=\"{f}\" shortname=\"f{n:03}\" type=\"FIELDTYPE_INTEGER\" depth=\"{d}\" rangelow=\"{low}\" rangehigh=\"{high}\" />"
                );
            }
            xml += "</fields></table>";
        }
        xml += "</database>";
        let meta = MetaSchema::parse(&xml).unwrap();
        let order: Vec<&str> = tables.iter().map(|t| t.1).collect();
        let database = Database::from_meta(&meta, &order).unwrap();
        SaveSchema {
            database,
            meta,
            riders: vec![(fnv1(b"zoe"), "zoe".into()), (fnv1(b"elise"), "elise".into())],
            items: [fnv1(b"suit_default_zoe")].into_iter().collect(),
            drops: vec![77],
            audio: None,
        }
    }

    fn schema() -> SaveSchema {
        schema_with(true)
    }

    #[test]
    fn a_signed_32_bit_field_is_stored_offset_binary() {
        let s = schema();
        let p = Profile::new();
        let file = p.to_save(&s, &SaveExtras::fresh()).unwrap();
        let db = DatabaseChunk::parse(&file.chunks[0]).unwrap().database().unwrap();
        let t = db.table("taaa").unwrap();
        let name = s.short("attribs", "NAME").unwrap();
        let mut found = false;
        for row in t.live_rows() {
            let v = t.get(row, &name).unwrap();
            assert_ne!(v, crate::career::profile::ATTRIB_CREDITS, "a console save's BILL is not stored raw");
            if v == crate::career::profile::ATTRIB_CREDITS ^ 0x8000_0000 {
                found = true;
            }
        }
        assert!(found, "the BILL row is stored with its sign bit flipped");
        let (q, _) = Profile::from_save(&ProfileFile::parse(&file.write()).unwrap(), &s).unwrap();
        assert_eq!((q.credits(), q.earned()), (p.credits(), p.earned()));
    }

    #[test]
    fn an_old_raw_save_still_reads() {
        let raw = schema_with(false);
        let signed = schema();
        let mut p = Profile::new();
        p.add_credits(12_345, true);
        let bytes = p.to_save(&raw, &SaveExtras::fresh()).unwrap().write();
        let (q, _) = Profile::from_save(&ProfileFile::parse(&bytes).unwrap(), &signed).unwrap();
        assert_eq!((q.credits(), q.earned()), (p.credits(), p.earned()));
        let again = q.to_save(&signed, &SaveExtras::fresh()).unwrap();
        let db = DatabaseChunk::parse(&again.chunks[0]).unwrap().database().unwrap();
        let name = signed.short("attribs", "NAME").unwrap();
        assert!(
            db.table("taaa").unwrap().live_rows().any(|r| db.table("taaa").unwrap().get(r, &name)
                == Some(crate::career::profile::ATTRIB_CREDITS ^ 0x8000_0000))
        );
    }

    #[test]
    fn profiles_round_trip_through_the_save_buffer() {
        let s = schema();
        let mut p = Profile::new();
        p.add_credits(12_345, true);
        let zoe = fnv1(b"zoe");
        p.owned_items.extend([zoe, 5, 6]);
        p.unlocked_riders.insert(fnv1(b"elise"));
        p.riders.insert(zoe, RiderStats { money_earned: 9000, max_achieved_level: 1, ..Default::default() });
        p.generated.insert(
            0,
            GeneratedItem {
                equip_id: 0,
                kind: 2,
                slot: 3,
                level: 4,
                rarity: 2,
                cost: 5600,
                stats: [(11, 40), (12, 50), (-1, 0), (-1, 0), (-1, 0), (30, 7)],
                wall_id: -1,
                ..Default::default()
            },
        );
        p.explore_scores.insert(77, ExploreScore { score: 90_000, attempts: 2, ..Default::default() });
        p.drop_states.insert(77, DropState::Completed);
        p.drop_states.insert(78, DropState::Unlocked);
        p.peak_modes.insert(3, [1, 0, 0, 1, 0]);
        p.chapter_drops = vec![78, 77];
        p.purchased.insert(77);
        p.medals.insert(77, [1, 1, 0]);
        p.movies.insert("comic_zoe".into());
        p.collected_geotags.insert(super::super::geotags::geotag_key(9, 4));
        p.new_game_state = 22;
        p.named_flags.extend([0x1122_3344, 0xC289_5FBE]);
        p.visit_range_by(0x0C0F_FEE0, "Zoe");
        p.visit_range_by(0x0C0F_FEE0, "Zoe");
        p.visit_range_by(0x0C0F_FEE0, "Elise");
        assert_eq!(p.camera_type(), -1);
        p.set_camera_type(5);
        let extras = SaveExtras::fresh();
        let file = p.to_save(&s, &extras).unwrap();
        let bytes = file.write();
        assert_eq!(bytes.len(), ssx_formats::save::PROFILE_BUFFER_SIZE);
        let (q, ex) = Profile::from_save(&ProfileFile::parse(&bytes).unwrap(), &s).unwrap();
        assert_eq!(q.to_save(&s, &ex).unwrap().write(), bytes, "write → read → write");
        assert_eq!(q.named_flags, p.named_flags, "every permanent flag survives the save");
        assert_eq!(
            (q.range_visits_by(0x0C0F_FEE0, "Zoe"), q.range_visits_by(0x0C0F_FEE0, "Elise")),
            (2, 1),
            "the range visits survive the save (the intro's path in a later session)"
        );
        assert_eq!(q.camera_type(), 5, "the camera type survives the save (the next session's camera)");
        assert_eq!((q.credits(), q.earned()), (p.credits(), p.earned()));
        assert_eq!(
            (&q.owned_items, &q.unlocked_riders, &q.generated),
            (&p.owned_items, &p.unlocked_riders, &p.generated)
        );
        assert_eq!(
            (&q.explore_scores, &q.drop_states, &q.medals, &q.movies),
            (&p.explore_scores, &p.drop_states, &p.medals, &p.movies)
        );
        assert_eq!(
            (&q.collected_geotags, &q.chapter_drops, q.new_game_state),
            (&p.collected_geotags, &p.chapter_drops, 22)
        );
        assert_eq!(q.riders[&zoe].last_suit, (fnv1(b"suit_default_zoe"), 0));
        assert_eq!(q.riders[&fnv1(b"elise")].last_suit, (0, 2));
        assert_eq!(q.riders[&zoe].money_earned, 9000);
        let db = DatabaseChunk::parse(&file.chunks[0]).unwrap().database().unwrap();
        assert_eq!(db.tables.len(), 6, "the common table is not in the profile group");
        assert_eq!(db.table("taaa").unwrap().row_count, 100);
    }

    #[test]
    fn a_loaded_new_game_state_takes_the_games_load_rule() {
        let s = schema();
        let extras = SaveExtras::fresh();
        for (stored, loaded) in [(0, 0), (1, 0), (9, 0), (11, 0), (12, 22), (13, 22), (21, 22), (22, 22)] {
            let mut p = Profile::new();
            p.new_game_state = stored;
            let bytes = p.to_save(&s, &extras).unwrap().write();
            let (q, _) = Profile::from_save(&ProfileFile::parse(&bytes).unwrap(), &s).unwrap();
            assert_eq!(q.new_game_state, loaded, "stored {stored}");
        }
    }

    #[test]
    fn a_loaded_save_that_grows_still_fills_the_buffer() {
        let s = schema();
        let bytes = Profile::new().to_save(&s, &SaveExtras::fresh()).unwrap().write();
        let (mut q, ex) = Profile::from_save(&ProfileFile::parse(&bytes).unwrap(), &s).unwrap();
        q.explore_scores.insert(77, ExploreScore { score: 1, ..Default::default() });
        q.medals.insert(77, [1, 0, 0]);
        let again = q.to_save(&s, &ex).unwrap().write();
        assert_eq!(again.len(), ssx_formats::save::PROFILE_BUFFER_SIZE);
    }
}
