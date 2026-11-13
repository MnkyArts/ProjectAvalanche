use ssx_physics::event::EventType;

use super::loc::Localization;
use super::values::{LeaderboardRow, SplitsCallout, UiValue, UiValues, id};

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Entry {
    pub(crate) kind: u8,
    pub(crate) value: i32,
    pub(crate) name: String,
    pub(crate) picture: String,
}

#[derive(Clone, Debug)]
pub struct ExploreLeaderboard {
    pub rivals: Vec<(String, u32)>,
    pub pictures: std::collections::BTreeMap<String, String>,
    rank: i32,
    beat_shown: bool,
}

impl Default for ExploreLeaderboard {
    fn default() -> Self {
        ExploreLeaderboard { rivals: Vec::new(), pictures: Default::default(), rank: -1, beat_shown: false }
    }
}

pub(crate) fn ghost_name(loc: &mut Localization, key: &str) -> String {
    let mut t = loc.text(key);
    if t.len() > 16 {
        let mut end = 16;
        while !t.is_char_boundary(end) {
            end -= 1;
        }
        t.truncate(end);
    }
    t
}

pub(crate) fn value_kind(event_type: EventType) -> u8 {
    match event_type as i32 {
        0..=2 => 1,
        5..=7 => 0,
        _ => 2,
    }
}

pub(crate) fn value_text(loc: &mut Localization, kind: u8, value: i32, with_unit: bool) -> String {
    if value == -1 {
        return loc.text("TXT_N_A");
    }
    match kind {
        0 => {
            let t = loc.race_time(value as u32);
            if with_unit { format!("{t} {}", loc.text("TXT_RANK_TIME_ABBR")) } else { t }
        }
        1 => loc.int(i64::from(value)),
        _ => {
            let m = f64::from(value as u32 as f32 / 1000.0);
            if with_unit { loc.distance_with_unit(m) } else { loc.distance(m) }
        }
    }
}

impl ExploreLeaderboard {
    pub fn update(
        &mut self,
        v: &mut UiValues,
        loc: &mut Localization,
        event_type: EventType,
        player: &str,
        value: f32,
    ) {
        let mut list: Vec<Entry> = self
            .rivals
            .iter()
            .map(|(n, x)| Entry {
                kind: 3,
                value: *x as i32,
                name: ghost_name(loc, n),
                picture: self.pictures.get(n).cloned().unwrap_or_default(),
            })
            .collect();
        list.push(Entry { kind: 2, value: value.max(0.0) as i32, name: player.to_string(), picture: String::new() });
        sort_entries(&mut list, event_type.ranks_ascending(), false);
        let place = list.iter().position(|e| e.kind == 2).map_or(-1, |p| p as i32);
        if self.rank < 0 {
            self.rank = list.len() as i32 - 1;
        }
        if place < self.rank && place < 3 {
            let medal_key =
                ["TXT_EXPLORE_GHOST_MEDAL_GOLD", "TXT_EXPLORE_GHOST_MEDAL_SILVER", "TXT_EXPLORE_GHOST_MEDAL_BRONZE"]
                    [place as usize];
            let medal = loc.text(medal_key);
            let body = loc.format("TXT_SPLIT_YOU_EARNED", &[&medal]);
            let callout = SplitsCallout {
                ahead: true,
                show_time: false,
                kind: 0,
                body,
                medal: place as u32,
                name: list[place as usize].name.clone(),
                ..Default::default()
            };
            v.set(id::SPLITS_CALLOUT, UiValue::Splits(callout));
            v.set_bool(id::SPLITS_CALLOUT_ENABLED, true);
        }
        let _ = self.beat_shown;
        let rows = rows(loc, &list, event_type);
        v.set(id::LEADERBOARD, UiValue::Rows(rows));
        v.set_bool(id::EXPLORE_LEADERBOARD_IS_ACTIVE, true);
        self.rank = place;
    }
}

pub(crate) fn sort_entries(list: &mut [Entry], ascending: bool, player_wins_ties: bool) {
    list.sort_by(|a, b| entry_order(a, b, ascending, player_wins_ties));
}

pub(crate) fn entry_order(a: &Entry, b: &Entry, ascending: bool, player_wins_ties: bool) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    if a.value == b.value {
        return match (a.kind == 2, b.kind == 2) {
            (true, false) if player_wins_ties => Ordering::Less,
            (true, false) => Ordering::Greater,
            (false, true) if player_wins_ties => Ordering::Greater,
            (false, true) => Ordering::Less,
            _ => Ordering::Equal,
        };
    }
    if a.value == -1 {
        return Ordering::Greater;
    }
    if b.value == -1 {
        return Ordering::Less;
    }
    let o = (a.value as u32).cmp(&(b.value as u32));
    if ascending { o } else { o.reverse() }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct RaceEntrant {
    pub name: String,
    pub picture: String,
    pub active: bool,
    pub finish_ms: Option<u32>,
    pub y: Option<f32>,
}

pub fn race_update(
    v: &mut UiValues,
    loc: &mut Localization,
    event_type: EventType,
    player: &RaceEntrant,
    racers: &[RaceEntrant],
    start_y: f32,
    end_y: f32,
) {
    let f = 500.0 + (start_y - end_y);
    let racing_value = |y: f32| {
        let d = start_y - y;
        let c = if d <= 0.0 { 0.0 } else { d };
        let m = if f - c >= 0.0 { c } else { f };
        (f - m) as i32
    };
    let (mut finished, mut racing) = (Vec::new(), Vec::new());
    for (kind, r) in std::iter::once((2u8, player)).chain(racers.iter().map(|r| (3u8, r))) {
        let mut e = Entry { kind, value: -1, name: r.name.clone(), picture: r.picture.clone() };
        match (r.active || kind == 2, r.finish_ms) {
            (true, Some(ms)) => {
                e.value = ms as i32;
                finished.push(e);
            }
            (true, None) => {
                e.value = r.y.map_or(-1, racing_value);
                racing.push(e);
            }
            (false, _) => racing.push(e),
        }
    }
    let ascending = value_kind(event_type) == 0;
    sort_entries(&mut finished, ascending, true);
    sort_entries(&mut racing, ascending, true);
    finished.extend(racing);
    if finished.len() > 1 {
        let rows = rows(loc, &finished, event_type);
        v.set(id::LEADERBOARD, UiValue::Rows(rows));
        v.set_bool(id::EXPLORE_LEADERBOARD_IS_ACTIVE, true);
    }
}

pub const TRICK_BOT_SCALED_BELOW: f32 = 0.85;
pub const TRICK_BOT_RECORDED_FROM: f32 = 0.92;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct TrickBot {
    pub name: String,
    pub picture: String,
    pub live: f32,
    pub recorded: u32,
}

#[derive(Clone, Copy)]
pub struct TrickFrame<'a> {
    pub event_type: EventType,
    pub player: &'a RaceEntrant,
    pub points: f32,
    pub progress: f32,
    pub world_tour: bool,
    pub bots: &'a [TrickBot],
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct TrickStandings {
    seen: Vec<u32>,
    best: Vec<u32>,
}

impl TrickStandings {
    pub fn reset(&mut self, bots: usize) {
        self.seen = vec![0; bots];
        self.best = vec![0; bots];
    }

    fn value(&mut self, k: usize, bot: &TrickBot, progress: f32, world_tour: bool) -> i32 {
        if !world_tour {
            return bot.live as i64 as i32;
        }
        if self.seen.len() <= k {
            self.seen.resize(k + 1, 0);
            self.best.resize(k + 1, 0);
        }
        let live = bot.live as i64 as u32;
        if progress >= TRICK_BOT_RECORDED_FROM || progress.is_nan() {
            self.best[k] = bot.recorded;
            return bot.recorded as i32;
        }
        if self.seen[k] == live {
            return self.best[k] as i32;
        }
        let v = if progress < TRICK_BOT_SCALED_BELOW {
            self.seen[k] = live;
            ((bot.recorded as f32 * progress) as i64 as u32).max(live)
        } else {
            self.best[k] = bot.recorded;
            bot.recorded
        };
        let v = v.max(self.best[k]);
        self.best[k] = v;
        v as i32
    }

    pub fn update(&mut self, v: &mut UiValues, loc: &mut Localization, f: TrickFrame) {
        let TrickFrame { event_type, player, points, progress, world_tour, bots } = f;
        let mut list = vec![Entry {
            kind: 2,
            value: points as i64 as i32,
            name: player.name.clone(),
            picture: player.picture.clone(),
        }];
        for (k, b) in bots.iter().enumerate() {
            let value = self.value(k, b, progress, world_tour);
            list.push(Entry { kind: 3, value, name: b.name.clone(), picture: b.picture.clone() });
        }
        sort_entries(&mut list, value_kind(event_type) == 0, true);
        let rows = rows(loc, &list, event_type);
        v.set(id::LEADERBOARD, UiValue::Rows(rows));
        v.set_bool(id::EXPLORE_LEADERBOARD_IS_ACTIVE, true);
    }
}

fn rows(loc: &mut Localization, list: &[Entry], event_type: EventType) -> Vec<LeaderboardRow> {
    let race = event_type.is_race();
    let kind = value_kind(event_type);
    let mut ghosts_above = 0;
    list.iter()
        .enumerate()
        .map(|(k, e)| {
            let medal = if e.kind == 3 {
                let m = match ghosts_above {
                    0 => 2,
                    1 => 1,
                    _ => 0,
                };
                ghosts_above += 1;
                m
            } else {
                -1
            };
            LeaderboardRow {
                value: e.value,
                name: e.name.clone(),
                icon: e.picture.clone(),
                score: value_text(loc, kind, e.value, !race),
                rank_text: format!("{}", k + 1),
                reward: String::new(),
                medal,
                rank: k as i32 + 1,
                is_player: e.kind == 2,
                ..Default::default()
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::loc::Language;

    fn e(kind: u8, value: i32) -> Entry {
        Entry { kind, value, name: format!("{kind}/{value}"), picture: String::new() }
    }

    #[test]
    fn the_sort_puts_none_last_and_breaks_ties_by_the_flag() {
        let mut l = vec![e(3, 500), e(3, -1), e(2, 300), e(3, 300), e(3, 100)];
        sort_entries(&mut l, true, false);
        let v: Vec<_> = l.iter().map(|x| (x.kind, x.value)).collect();
        assert_eq!(v, [(3, 100), (3, 300), (2, 300), (3, 500), (3, -1)]);
        sort_entries(&mut l, true, true);
        let v: Vec<_> = l.iter().map(|x| (x.kind, x.value)).collect();
        assert_eq!(v, [(3, 100), (2, 300), (3, 300), (3, 500), (3, -1)]);
        sort_entries(&mut l, false, true);
        assert_eq!(l.iter().map(|x| x.value).collect::<Vec<_>>(), [500, 300, 300, 100, -1]);
    }

    #[test]
    fn the_trick_standings_scale_the_recorded_score_by_the_progress() {
        let mut v = UiValues::new();
        let mut loc = Localization::empty(Language::English);
        let player = RaceEntrant { name: "me".into(), ..Default::default() };
        let bot = |live: f32| TrickBot { name: "bot".into(), live, recorded: 10_000, ..Default::default() };
        let mut t = TrickStandings::default();
        t.reset(1);
        let mut values = |t: &mut TrickStandings, points: f32, progress: f32, live: f32| {
            let bots = [bot(live)];
            let f = TrickFrame {
                event_type: EventType::TrickBots,
                player: &player,
                points,
                progress,
                world_tour: true,
                bots: &bots,
            };
            t.update(&mut v, &mut loc, f);
            v.rows(id::LEADERBOARD).iter().map(|r| (r.name.clone(), r.value)).collect::<Vec<_>>()
        };
        assert_eq!(values(&mut t, 0.0, 0.2, 0.0), [("me".into(), 0), ("bot".into(), 0)]);
        assert_eq!(values(&mut t, 2_000.0, 0.3, 500.0)[0], ("bot".into(), 3_000));
        assert_eq!(values(&mut t, 2_000.0, 0.5, 500.0)[0], ("bot".into(), 3_000));
        assert_eq!(values(&mut t, 2_000.0, 0.4, 6_000.0)[0], ("bot".into(), 6_000));
        assert_eq!(values(&mut t, 2_000.0, 0.45, 6_001.0)[0], ("bot".into(), 6_001));
        assert_eq!(values(&mut t, 2_000.0, 0.5, 6_002.0)[0], ("bot".into(), 6_002));
        assert_eq!(values(&mut t, 2_000.0, 0.86, 6_100.0)[0], ("bot".into(), 10_000));
        assert_eq!(values(&mut t, 12_000.0, 0.95, 6_100.0), [("me".into(), 12_000), ("bot".into(), 10_000)]);
        assert!(v.bool(id::EXPLORE_LEADERBOARD_IS_ACTIVE));
        let mut u = TrickStandings::default();
        let bots = [bot(777.9)];
        let f = TrickFrame {
            event_type: EventType::TrickBots,
            player: &player,
            points: 1.0,
            progress: 0.5,
            world_tour: false,
            bots: &bots,
        };
        u.update(&mut v, &mut loc, f);
        assert_eq!(v.rows(id::LEADERBOARD)[0].value, 777);
    }

    #[test]
    fn the_race_board_ranks_the_finished_first_then_the_height_left() {
        let mut v = UiValues::new();
        let mut loc = Localization::empty(Language::English);
        let r = |name: &str, finish_ms: Option<u32>, y: f32| RaceEntrant {
            name: name.into(),
            active: true,
            finish_ms,
            y: Some(y),
            ..Default::default()
        };
        let player = r("me", None, 400.0);
        let racers = [r("a", None, 300.0), r("b", Some(61_000), -20.0), r("c", None, 1_200.0)];
        race_update(&mut v, &mut loc, EventType::RaceGhosts, &player, &racers, 1_000.0, 0.0);
        let rows = v.rows(id::LEADERBOARD).to_vec();
        let names: Vec<_> = rows.iter().map(|x| x.name.as_str()).collect();
        assert_eq!(names, ["b", "a", "me", "c"]);
        assert_eq!(rows.iter().map(|x| x.value).collect::<Vec<_>>(), [61_000, 800, 900, 1_500]);
        assert!(v.bool(id::EXPLORE_LEADERBOARD_IS_ACTIVE));
        assert!(rows[2].is_player);
    }

    #[test]
    fn an_entrant_with_no_entity_ranks_by_its_finish_alone() {
        let mut v = UiValues::new();
        let mut loc = Localization::empty(Language::English);
        let r = |name: &str, finish_ms: Option<u32>, y: Option<f32>| RaceEntrant {
            name: name.into(),
            active: true,
            finish_ms,
            y,
            ..Default::default()
        };
        let player = r("me", None, Some(400.0));
        let racers = [r("silver", Some(15_000), None), r("gold", Some(13_000), None), r("odd", None, None)];
        race_update(&mut v, &mut loc, EventType::RaceGhosts, &player, &racers, 1_000.0, 0.0);
        let rows = v.rows(id::LEADERBOARD).to_vec();
        let names: Vec<_> = rows.iter().map(|x| x.name.as_str()).collect();
        assert_eq!(names, ["gold", "silver", "me", "odd"]);
        assert_eq!(rows.iter().map(|x| x.value).collect::<Vec<_>>(), [13_000, 15_000, 900, -1]);
    }
}
