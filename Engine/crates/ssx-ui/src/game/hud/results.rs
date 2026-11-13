use super::{DataField, DataObject};
use crate::avm1::Ctx;
use crate::avm1::value::Value;
use crate::game::loc::Localization;
use crate::game::values::{UiValue, UiValues, id};

const fn f(value: u16, kind: u8, name: &'static str) -> DataField {
    DataField { value, kind, name, fmt: 0 }
}

pub const RESULTS_DATA: &[DataObject] = &[
    DataObject {
        name: "dp_race_results_data",
        fields: &[
            f(0x1ED, 0, "isEnabled"),
            f(0x1EA, 1, "moneyPrevious"),
            f(0x1D4, 1, "gameMode"),
            f(0x1D7, 0, "goalComplete"),
            f(0x1D5, 4, "goalHeading"),
            f(0x1D6, 4, "goalIcon"),
            f(0x1DB, 4, "goalSuccessLabel"),
            f(0x1D8, 4, "goalPlayerScore"),
            f(0x1D9, 4, "goalPlayerScoreLabel"),
            f(0x1DC, 4, "goalTargetLabel"),
            f(0x1DD, 4, "goalTargetScore"),
            f(0x1DA, 1, "goalTargetReward"),
            f(0x1DE, 1, "goalMedal"),
            f(0x1CC, 5, "bonusIconList"),
            f(0x1CD, 5, "bonusLabelList"),
            f(0x1C8, 5, "bonusDescriptionList"),
            f(0x1CA, 5, "bonusDescriptionValueList"),
            f(0x1C9, 5, "bonusRewardList"),
            f(0x1CF, 7, "bonusTotalRewardList"),
            f(0x1D0, 4, "zeroCreditsString"),
            f(0x1E0, 0, "leaderboardIsEnabled"),
            f(0x1E2, 1, "leaderboardTotalPassedCount"),
            f(0x1E3, 1, "leaderboardTotalReward"),
            f(0x1E1, 8, "leaderboard"),
            f(0x1E4, 4, "levelupCharacterPic"),
            f(0x1E5, 1, "levelupOldPercent"),
            f(0x1E6, 1, "levelupOldLevel"),
            f(0x1E7, 1, "levelupNewPercent"),
            f(0x1E8, 1, "levelupNewLevel"),
            f(0x1E9, 4, "levelupXpGained"),
            f(0x1D2, 4, "deathBody"),
        ],
        statics: &[
            ("bonusHeading", "TXT_EXPLORE_RESULTS_BONUS_HEADING"),
            ("bonusTotalLabel", "TXT_EXPLORE_RESULTS_BONUS_TOTALLABEL"),
            ("leaderboardHeading", "TXT_EXPLORE_RESULTS_LEADERBOARD_HEADING"),
            ("leaderboardTotalPassedLabel", "TXT_EXPLORE_RESULTS_LEADERBOARD_TOTALPASSEDLABEL"),
            ("deathHeader", "TXT_RESULTS_DEATH_BURIED_CAPS"),
        ],
    },
    DataObject { name: "dp_race_results_campaign_data", fields: &[f(0x1D1, 0, "isDropSuccess")], statics: &[] },
];

pub fn campaign_fields(cx: &mut Ctx, o: &Value, campaign: Option<(bool, String, String)>) {
    match campaign {
        Some((success, title, body)) => {
            let _ = cx.set_member(o, "isCampaign", Value::Bool(true));
            let _ = cx.set_member(o, "isDropSuccess", Value::Bool(success));
            let _ = cx.set_member(o, "objectiveTitle", Value::str(&title));
            let _ = cx.set_member(o, "objectiveBody", Value::str(&body));
        }
        None => {
            let _ = cx.set_member(o, "isCampaign", Value::Bool(false));
        }
    }
}

pub fn death_body(values: &mut UiValues, loc: &mut Localization, course_mm: i64, descent_mm: u32, free_ride: bool) {
    if free_ride {
        values.set_str(id::RESULT_DEATH_BODY, String::new());
        return;
    }
    let left = course_mm - i64::from(descent_mm);
    let d = loc.distance_long(left as f32 as f64 / 1000.0);
    let body = loc.format("TXT_RESULTS_DEATH_BODY", &[&d]);
    values.set_str(id::RESULT_DEATH_BODY, body);
}

pub const EVENT_TYPE_NAMES: [&str; 9] = [
    "TXT_EVENTTYPE_TRICK_SCORE",
    "TXT_EVENTTYPE_TRICK_BOTS",
    "TXT_EVENTTYPE_TRICK_GHOSTS",
    "TXT_EVENTTYPE_SURVIVE_TO_BOTTOM",
    "TXT_EVENTTYPE_SURVIVE_TO_DEATH",
    "TXT_EVENTTYPE_RACE_CLOCK",
    "TXT_EVENTTYPE_RACE_GHOSTS",
    "TXT_EVENTTYPE_RACE_BOTS",
    "TXT_EVENTTYPE_FREERIDE",
];

pub fn medal_icon(medal: i32) -> &'static str {
    match medal {
        0 => "assets\\drop_selector_icons\\explore_bronze_large.dds",
        1 => "assets\\drop_selector_icons\\explore_silver_large.dds",
        2 => "assets\\drop_selector_icons\\explore_gold_large.dds",
        _ => "",
    }
}

#[derive(Clone, Debug, Default)]
pub struct ResultsInput {
    pub event_type: i32,
    pub game_mode: i32,
    pub points: u32,
    pub time_ms: u32,
    pub descent_mm: u32,
    pub rewinds: u32,
    pub failed: bool,
    pub rank: Option<usize>,
    pub medal: i32,
    pub career: CareerResults,
    pub tutorial: bool,
    pub character_pic: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct BonusLine {
    pub icon: String,
    pub label: String,
    pub arg: Option<String>,
    pub credits: i64,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct CareerResults {
    pub reward: i64,
    pub bonuses: Vec<BonusLine>,
    pub money_previous: i64,
    pub total: i64,
    pub level_before: (i64, i64),
    pub level_after: (i64, i64),
    pub success: bool,
    pub medal: Option<ssx_assets::career::profile::Medal>,
    pub notices: Vec<ssx_assets::career::profile::Notice>,
    pub best_not_kept: Option<ssx_assets::career::profile::BestNotKept>,
}

pub fn bonus_lines(
    first_run: bool,
    track: &[&ssx_assets::career::profile::BadgeText],
    repeat: bool,
    failed: bool,
    geotag_credits: i64,
    geotag_count: u32,
) -> Vec<BonusLine> {
    if failed {
        return Vec::new();
    }
    let mut out = Vec::new();
    if first_run {
        out.push(BonusLine {
            icon: "assets\\results_Icons\\bonus_FirstDescent.dds".into(),
            label: "TXT_BONUS_FIRST_DESCENT".into(),
            arg: None,
            credits: ssx_assets::career::profile::FIRST_RUN_BONUS,
        });
    }
    for b in track {
        out.push(BonusLine { icon: b.icon.clone(), label: b.title.clone(), arg: None, credits: b.credits });
    }
    if !first_run && repeat {
        out.push(BonusLine {
            icon: "assets\\results_Icons\\bonus_RunCompleted.dds".into(),
            label: "TXT_BONUS_RUN_COMPLETED".into(),
            arg: None,
            credits: ssx_assets::career::profile::REPEAT_BONUS,
        });
    }
    if geotag_credits > 0 {
        let (label, arg) = if geotag_count == 1 {
            ("TXT_GEOTAG_TITLE", None)
        } else {
            ("TXT_MULTIPLE_GEOTAG_TITLE", Some(geotag_count.to_string()))
        };
        out.push(BonusLine {
            icon: "assets\\results_Icons\\bonus_Geotag.dds".into(),
            label: label.into(),
            arg,
            credits: geotag_credits,
        });
    }
    // TODO(guess): the rival panel's line (Explore only, 0x90 +0x280, 0x82461A78) is never active, as in `ssx_assets::career`; settle: the panel's writer (the drop selector's rival choice).
    out
}

pub fn value_text(loc: &mut Localization, event_type: i32, v: u32) -> String {
    match event_type {
        0..=2 => loc.int(i64::from(v)),
        5..=7 => loc.race_time(v),
        3 | 4 | 8 => loc.distance_with_unit(f64::from(v) / 1000.0),
        _ => loc.int(i64::from(v)),
    }
}

pub fn post_results(values: &mut UiValues, loc: &mut Localization, r: &ResultsInput) {
    post_results_with(values, loc, r, r.game_mode == 0);
}

pub fn post_results_with(values: &mut UiValues, loc: &mut Localization, r: &ResultsInput, explore: bool) {
    values.set_int(id::RESULT_POINTS, r.points as i32);
    values.set_int(id::RESULT_TIME, r.time_ms as i32);
    values.set_int(id::RESULT_DESCENDED, r.descent_mm as i32);
    values.set_int(id::RESULT_REWIND_COUNT, r.rewinds as i32);
    values.set_int(id::RESULT_GAME_MODE, r.game_mode);
    let heading = if r.game_mode == 1 && r.tutorial {
        loc.text("TXT_EVENTTYPE_TUTORIAL")
    } else {
        EVENT_TYPE_NAMES.get(r.event_type as usize).map_or(String::new(), |k| loc.text(k))
    };
    let label_key = match r.event_type {
        5..=7 => "TXT_YOUR_TIME_COLON",
        3 | 4 => "TXT_YOU_DESCENDED_COLON",
        _ => "TXT_YOUR_SCORE_COLON",
    };
    let label = loc.text(label_key);
    let value = match r.event_type {
        5..=7 => r.time_ms,
        3 | 4 => r.descent_mm,
        _ => r.points,
    };
    let score = value_text(loc, r.event_type, value);
    let complete = if explore { !r.failed && r.medal >= 0 } else { r.career.success };
    let success = if explore {
        match r.medal {
            2 => loc.text("TXT_YOU_WON_GOLD_CAPS"),
            1 => loc.text("TXT_YOU_WON_SILVER_CAPS"),
            0 => loc.text("TXT_YOU_WON_BRONZE_CAPS"),
            _ => loc.text("TXT_NO_MEDAL_AWARDED_CAPS"),
        }
    } else if complete {
        loc.text("EXPLORE_RESULTS_GOAL_SUCCESS")
    } else {
        String::new()
    };
    values.set_bool(id::RESULT_GOAL_IS_COMPLETE, complete);
    values.set_str(id::RESULT_GOAL_HEADING, heading);
    let icon = if explore { medal_icon(r.medal).to_string() } else { values.str(0x74).to_string() };
    values.set_str(id::RESULT_GOAL_ICON, icon);
    values.set_str(id::RESULT_GOAL_PLAYER_SCORE, score);
    let goal_body = values.str(0x71).to_string();
    values.set_str(id::RESULT_GOAL_TARGET_LABEL, goal_body);
    let target = if !explore && !complete { loc.text("EXPLORE_RESULTS_GOAL_FAIL") } else { " ".to_string() };
    values.set_str(id::RESULT_GOAL_TARGET_SCORE, target);
    values.set_int(id::RESULT_GOAL_MEDAL, r.medal);
    values.set_str(id::RESULT_GOAL_PLAYER_SCORE_LABEL, label);
    if explore || complete {
        values.set_str(id::RESULT_GOAL_SUCCESS_LABEL, success);
    }
    values.set_bool(id::RESULT_CAMPAIGN_TRACK_SUCCESS, if r.game_mode == 1 { r.career.success } else { !r.failed });
    post_career(values, loc, &r.career);
    values.set_bool(id::RESULT_SCREEN_DATA_READY_FOR_PUBLISH, true);
    let c = &r.career;
    values.set_str(id::RESULT_LEVELUP_CHARACTER_PIC, &r.character_pic);
    values.set_int(id::RESULT_LEVELUP_OLD_PERCENT, c.level_before.1 as i32);
    values.set_int(id::RESULT_LEVELUP_OLD_LEVEL, c.level_before.0 as i32 + 1);
    values.set_int(id::RESULT_LEVELUP_NEW_PERCENT, c.level_after.1 as i32);
    values.set_int(id::RESULT_LEVELUP_NEW_LEVEL, c.level_after.0 as i32 + 1);
    let gained = loc.int(c.total);
    let xp = loc.format("TXT_XP_GAINED", &[&gained]);
    values.set_str(id::RESULT_LEVELUP_XP_GAINED, xp);
}

fn post_career(values: &mut UiValues, loc: &mut Localization, c: &CareerResults) {
    values.set_int(id::RESULT_GOAL_REWARD, c.reward as i32);
    let mut total = 0;
    let mut cols: [Vec<String>; 6] = Default::default();
    for b in &c.bonuses {
        total += b.credits;
        cols[0].push(b.icon.clone());
        cols[1].push(match &b.arg {
            Some(a) => loc.format(&b.label, &[a]),
            None => loc.text(&b.label),
        });
        cols[2].push(String::new());
        cols[3].push(String::new());
        cols[4].push(loc.currency(b.credits, true));
        cols[5].push(total.to_string());
    }
    let [icons, labels, descs, desc_values, rewards, totals] = cols;
    values.set(id::RESULT_BONUS_ICON_LIST, UiValue::List(icons));
    values.set(id::RESULT_BONUS_LABEL_LIST, UiValue::List(labels));
    values.set(id::RESULT_BONUS_DESCRIPTION_LIST, UiValue::List(descs));
    values.set(id::RESULT_BONUS_DESCRIPTION_VALUE_LIST, UiValue::List(desc_values));
    values.set(id::RESULT_BONUS_REWARD_LIST, UiValue::List(rewards));
    values.set(id::RESULT_BONUS_TOTAL_REWARD_LIST, UiValue::List(totals));
    let zero = loc.currency(0, true);
    values.set_str(id::RESULT_ZERO_CREDITS_STRING, zero);
    values.set_bool(id::RESULT_LEADERBOARD_IS_ENABLED, false);
    values.set(id::RESULT_LEADERBOARD_LEADERBOARD, UiValue::Rows(Vec::new()));
    values.set_int(id::RESULT_LEADERBOARD_TOTAL_PASSED_COUNT, 0);
    values.set_int(id::RESULT_LEADERBOARD_TOTAL_REWARD, 0);
    values.set_int(id::RESULT_MONEY_PREVIOUS, c.money_previous as i32);
}
