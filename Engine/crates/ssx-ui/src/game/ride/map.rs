use ssx_content::gamemode::{Rivals, Rules, RunOutcome, record};
use ssx_physics::event::EventResult;

use super::{GameUi, Posted};
use crate::game::hud::results::{CareerResults, ResultsInput, post_results_with};

#[derive(Clone, Copy, Debug)]
pub struct MapRun<'a> {
    pub map: &'a str,
    pub mode: &'a str,
    pub rules: &'a Rules,
    pub gamemode: bool,
    pub sim_content_id: Option<&'a str>,
}

impl MapRun<'_> {
    pub fn medals(&self) -> bool {
        self.gamemode && self.rules.rivals() != Rivals::None
    }
}

// PORT(choice): a map's ride pays nothing into the career: no reward, bonus, money, XP, badge or stats; its record is
// kept as `explore_scores` keeps a drop's (best, attempts, deaths, fewest rewinds) in the mod state; career progress
// from maps is the user's decision; the game's `Results_Process` 0x8251D5D8 pays the reward by `UiDrop_DropType` — 1
// World Tour 0x8251CAB0, 0 Explore 0x8251D008, else 0 — and always the bonuses 0x825195A0, money (AddCredits) and XP
// 0x8238DC18, and a map has no ui_drop.
pub fn post_map_results(
    ui: &mut GameUi,
    event_type: i32,
    character: &str,
    r: &EventResult,
    run: Option<&MapRun>,
) -> Posted {
    let mut posted = Posted { won: false, best_not_kept: None, medal: None };
    let Some(fe) = ui.fe.as_mut() else { return posted };
    let rules = &fe.rules;
    let rider = rules.riders.get(character).copied().unwrap_or(0);
    let mut stats = fe.profile.riders.get(&rider).cloned().unwrap_or_default();
    let level =
        ssx_assets::career::profile::update_rider_level(&mut stats, &rules.level_thresholds, &rules.level_prices);
    let career = CareerResults {
        money_previous: fe.profile.credits(),
        level_before: level,
        level_after: level,
        ..CareerResults::default()
    };
    let character_pic = super::results_portrait(&fe.career, rules, character);
    let medals = run.is_some_and(|m| m.medals());
    let medal = r.rank.filter(|_| medals && !r.failed).and_then(ssx_assets::career::profile::Medal::from_rank);
    if let Some(m) = run.filter(|m| m.medals()) {
        posted.won = m.rules.outcome(r.failed, r.value).1;
    }
    posted.medal = medal;
    let mut lines = Vec::new();
    if let Some(m) = run {
        let t = m.rules.event_type();
        let outcome = RunOutcome { value: r.value, failed: r.failed, rewinds: r.rewinds };
        match m.map.parse() {
            Ok(id) => match record::add_run(&mut fe.profile.mod_state, &id, m.mode, t, outcome, m.sim_content_id) {
                Ok(moved) => {
                    let kept = record::read(&fe.profile.mod_state, &id, m.mode).unwrap_or_default();
                    fe.note(format!(
                        "maps: {} in {}: run {} kept (best {:?}{}, {} attempts, {} deaths)",
                        m.map,
                        m.mode,
                        r.value,
                        kept.best,
                        if moved { ", new" } else { "" },
                        kept.attempts,
                        kept.deaths
                    ));
                    if let Some(best) = kept.best {
                        let key = if moved { "TXT_AVA_MAPS_NEW_BEST" } else { "TXT_AVA_MAPS_RESULT_BEST" };
                        let shown = value_text(&mut ui.loc, event_type, best);
                        lines.push(ui.loc.format(key, &[&shown]));
                    }
                }
                Err(e) => fe.note(format!("maps: {} in {}: the run is not kept: {e}", m.map, m.mode)),
            },
            Err(e) => {
                fe.note(format!("maps: {}: not a map id ({e}); the run is not kept", ssx_core::text::quoted(m.map)))
            }
        }
        if medals && let [gold, silver, bronze] = m.rules.board_values()[..] {
            let [g, s, b] = [gold, silver, bronze].map(|v| value_text(&mut ui.loc, event_type, v));
            lines.insert(0, ui.loc.format("TXT_AVA_MAPS_TARGETS", &[&g, &s, &b]));
        }
    }
    let Some(fe) = ui.fe.as_ref() else { return posted };
    let input = ResultsInput {
        event_type,
        game_mode: fe.game_mode as i32,
        points: r.points,
        time_ms: r.time_ms,
        descent_mm: r.descent_mm,
        rewinds: r.rewinds,
        failed: r.failed,
        rank: r.rank.filter(|_| medals),
        medal: medal.map_or(-1, |m| m as i32),
        career,
        tutorial: false,
        character_pic,
    };
    ui.values.set_str(0x71, lines.join("\n"));
    post_results_with(&mut ui.values, &mut ui.loc, &input, medals);
    posted
}

fn value_text(loc: &mut crate::game::loc::Localization, t: i32, v: u32) -> String {
    match t {
        5..=7 => loc.race_time(v),
        3 | 4 | 8 => loc.distance_with_unit(f64::from(v) / 1000.0),
        _ => loc.int(i64::from(v)),
    }
}
