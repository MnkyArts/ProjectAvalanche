use std::collections::BTreeMap;

use ssx_physics::event::EventType;

use super::leaderboard::{Entry, entry_order, value_kind, value_text};
use super::loc::Localization;
use super::values::{LeaderboardRow, UiValue, UiValues, id};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SessionKind {
    #[default]
    Race,
    Trick,
    Survive,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SessionRacer {
    pub name: String,
    pub picture: String,
    pub place: Option<usize>,
    pub finish_ms: Option<u32>,
    pub out: bool,
    pub points: u32,
    pub descent_mm: u32,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SessionHud {
    pub kind: SessionKind,
    pub riders: BTreeMap<u16, SessionRacer>,
    pub player_place: Option<usize>,
    pub place_offset: usize,
}

#[derive(Clone, Debug)]
pub struct Entrant {
    pub name: String,
    pub picture: String,
    pub player: bool,
    pub finish_ms: Option<u32>,
    pub out: bool,
    pub y: f32,
    pub points: u32,
    pub descent_mm: u32,
    pub place: Option<usize>,
}

const SHOWN_ROWS: usize = 5;

fn ranked(kind: SessionKind, entrants: &[Entrant], course: Option<(f32, f32)>) -> Option<Vec<(usize, Entry)>> {
    let entry = |e: &Entrant, value: i32| Entry {
        kind: if e.player { 2 } else { 3 },
        value,
        name: e.name.clone(),
        picture: e.picture.clone(),
    };
    let sort = |list: &mut Vec<(usize, Entry)>, ascending: bool| {
        list.sort_by(|a, b| {
            if a.1.value == b.1.value { std::cmp::Ordering::Equal } else { entry_order(&a.1, &b.1, ascending, true) }
        });
    };
    Some(match kind {
        SessionKind::Race => {
            let (start, end) = course?;
            let f = 500.0 + (start - end);
            let racing = |y: f32| {
                let d = (start - y).max(0.0);
                (f - d.min(f)) as i32
            };
            let (mut finished, mut riding, mut out) = (Vec::new(), Vec::new(), Vec::new());
            for (k, e) in entrants.iter().enumerate() {
                match (e.out, e.finish_ms) {
                    (true, _) => out.push((k, entry(e, -1))),
                    (false, Some(ms)) => finished.push((k, entry(e, ms as i32))),
                    (false, None) => riding.push((k, entry(e, racing(e.y)))),
                }
            }
            sort(&mut finished, true);
            sort(&mut riding, true);
            finished.extend(riding);
            finished.extend(out);
            finished
        }
        SessionKind::Trick | SessionKind::Survive => {
            let mut list: Vec<(usize, Entry)> = entrants
                .iter()
                .enumerate()
                .map(|(k, e)| {
                    let value = if e.out {
                        -1
                    } else if kind == SessionKind::Trick {
                        e.points.min(i32::MAX as u32) as i32
                    } else {
                        e.descent_mm.min(i32::MAX as u32) as i32
                    };
                    (k, entry(e, value))
                })
                .collect();
            sort(&mut list, false);
            list
        }
    })
}

pub fn order(kind: SessionKind, entrants: &[Entrant], course: Option<(f32, f32)>) -> Option<Vec<usize>> {
    ranked(kind, entrants, course).map(|list| list.into_iter().map(|(k, _)| k).collect())
}

pub fn update(
    v: &mut UiValues,
    loc: &mut Localization,
    event_type: EventType,
    hud: &SessionHud,
    entrants: &[Entrant],
    course: Option<(f32, f32)>,
) {
    let Some(mut list) = ranked(hud.kind, entrants, course) else { return };
    let placed = entrants.iter().all(|e| e.place.is_some());
    if placed {
        list.sort_by_key(|(k, _)| entrants[*k].place);
    }
    let places: Vec<usize> =
        list.iter().enumerate().map(|(row, (k, _))| entrants[*k].place.unwrap_or(hud.place_offset + row)).collect();
    let list: Vec<Entry> = list.into_iter().map(|(_, e)| e).collect();
    if list.len() < 2 {
        return;
    }
    let kind = value_kind(event_type);
    let race = event_type.is_race();
    let at = list.iter().position(|e| e.kind == 2).unwrap_or(0);
    let start = at.saturating_sub(SHOWN_ROWS / 2).min(list.len().saturating_sub(SHOWN_ROWS));
    let rows = list
        .iter()
        .enumerate()
        .skip(start)
        .take(SHOWN_ROWS)
        .map(|(k, e)| {
            let place = places[k];
            LeaderboardRow {
                value: e.value,
                name: e.name.clone(),
                icon: e.picture.clone(),
                score: value_text(loc, kind, e.value, !race),
                rank_text: format!("{}", place + 1),
                reward: String::new(),
                medal: medal(e.kind, place),
                rank: place as i32 + 1,
                is_player: e.kind == 2,
                ..Default::default()
            }
        })
        .collect();
    v.set(id::LEADERBOARD, UiValue::Rows(rows));
    v.set_bool(id::EXPLORE_LEADERBOARD_IS_ACTIVE, true);
}

// PORT(choice): a round's riders take the medal labels by their place among every rider (gold, silver, bronze for the
// first three); the game's rows give them to its racers and medal ghosts by the ghosts above (0x82488230 / 0x82487F60),
// and the game has no board of more riders than its slots (its online races are not on the disc).
fn medal(kind: u8, place: usize) -> i32 {
    match (kind, place) {
        (2, _) => -1,
        (_, 0) => 2,
        (_, 1) => 1,
        (_, 2) => 0,
        _ => -1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::loc::Language;

    fn e(name: &str, player: bool, finish_ms: Option<u32>, y: f32) -> Entrant {
        Entrant {
            name: name.into(),
            picture: String::new(),
            player,
            finish_ms,
            out: false,
            y,
            points: 0,
            descent_mm: 0,
            place: None,
        }
    }

    #[test]
    fn a_round_board_ranks_the_window_with_its_places() {
        let mut v = UiValues::new();
        let mut loc = Localization::empty(Language::English);
        let hud = SessionHud { kind: SessionKind::Race, place_offset: 4, ..SessionHud::default() };
        let mut out = e("gone", false, None, 900.0);
        out.out = true;
        let list = [e("me", true, None, 400.0), e("a", false, None, 300.0), e("b", false, Some(61_000), 0.0), out];
        update(&mut v, &mut loc, EventType::RaceClock, &hud, &list, Some((1_000.0, 0.0)));
        let rows = v.rows(id::LEADERBOARD).to_vec();
        let names: Vec<&str> = rows.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, ["b", "a", "me", "gone"]);
        assert_eq!(rows.iter().map(|r| r.rank).collect::<Vec<_>>(), [5, 6, 7, 8]);
        assert!(rows[2].is_player && rows[2].medal == -1);
        assert_eq!(rows[0].medal, -1, "fifth overall: no medal label");
        let top = SessionHud { kind: SessionKind::Race, place_offset: 0, ..SessionHud::default() };
        update(&mut v, &mut loc, EventType::RaceClock, &top, &list, Some((1_000.0, 0.0)));
        let rows = v.rows(id::LEADERBOARD).to_vec();
        assert_eq!(rows.iter().map(|r| r.medal).collect::<Vec<_>>(), [2, 1, -1, -1]);
        let trick = SessionHud { kind: SessionKind::Trick, place_offset: 1, ..SessionHud::default() };
        let mut list = [e("me", true, None, 0.0), e("a", false, None, 0.0), e("b", false, None, 0.0)];
        (list[0].points, list[1].points, list[2].points) = (5_000, 9_000, 1_000);
        update(&mut v, &mut loc, EventType::TrickScore, &trick, &list, None);
        let rows = v.rows(id::LEADERBOARD).to_vec();
        assert_eq!(rows.iter().map(|r| (r.name.as_str(), r.rank)).collect::<Vec<_>>(), [("a", 2), ("me", 3), ("b", 4)]);
    }

    #[test]
    fn the_hosts_places_order_the_board() {
        let mut v = UiValues::new();
        let mut loc = Localization::empty(Language::English);
        let hud = SessionHud { kind: SessionKind::Race, place_offset: 0, ..SessionHud::default() };
        let mut list = [e("me", true, None, 400.0), e("a", false, None, 300.0), e("b", false, None, 500.0)];
        (list[0].place, list[1].place, list[2].place) = (Some(2), Some(1), Some(5));
        update(&mut v, &mut loc, EventType::RaceClock, &hud, &list, Some((1_000.0, 0.0)));
        let rows = v.rows(id::LEADERBOARD).to_vec();
        assert_eq!(rows.iter().map(|r| (r.name.as_str(), r.rank)).collect::<Vec<_>>(), [("a", 2), ("me", 3), ("b", 6)]);
        assert_eq!(order(SessionKind::Race, &list, Some((1_000.0, 0.0))), Some(vec![1, 0, 2]));
        assert_eq!(order(SessionKind::Race, &list, None), None, "a race needs its course line");
    }

    #[test]
    fn the_five_rows_shown_hold_the_player() {
        let mut v = UiValues::new();
        let mut loc = Localization::empty(Language::English);
        let hud = SessionHud { kind: SessionKind::Trick, place_offset: 0, ..SessionHud::default() };
        let names = ["a", "b", "c", "d", "e", "me", "f"];
        let mut list: Vec<Entrant> = names.iter().map(|n| e(n, *n == "me", None, 0.0)).collect();
        for (k, entrant) in list.iter_mut().enumerate() {
            entrant.points = 10_000 - 1_000 * k as u32;
        }
        update(&mut v, &mut loc, EventType::TrickScore, &hud, &list, None);
        let rows = v.rows(id::LEADERBOARD).to_vec();
        let shown: Vec<(&str, i32)> = rows.iter().map(|r| (r.name.as_str(), r.rank)).collect();
        assert_eq!(shown, [("c", 3), ("d", 4), ("e", 5), ("me", 6), ("f", 7)]);
    }

    #[test]
    fn ties_keep_the_standings_order() {
        let race = |name: &str, player: bool| e(name, player, Some(8_033), 0.0);
        let list = [race("a", false), race("b", false), race("me", true)];
        assert_eq!(order(SessionKind::Race, &list, Some((1_000.0, 0.0))), Some(vec![0, 1, 2]));
        let mut list = [e("a", false, None, 0.0), e("me", true, None, 0.0), e("b", false, None, 0.0)];
        list[2].points = 1;
        assert_eq!(
            order(SessionKind::Trick, &list, None),
            Some(vec![2, 0, 1]),
            "b's point first, then the tie as listed"
        );
    }
}
