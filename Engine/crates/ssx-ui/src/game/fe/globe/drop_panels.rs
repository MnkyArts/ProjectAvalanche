use super::*;

pub fn drop_point(fe: &FrontEnd, drop: u32) -> (f32, f32) {
    let Some(e) = fe.career.event(drop) else { return (0.0, 0.0) };
    if (e.drop_type == DropType::Custom && e.custom_status == 2) || e.drop_type == DropType::Type2 {
        return (0.0, 0.0);
    }
    fe.career
        .drop_positions
        .entries
        .iter()
        .find(|(n, _, _)| fnv1(n.as_bytes()) == drop)
        .map_or((0.0, 0.0), |&(_, x, y)| (x, y))
}

pub fn write_panels(fe: &mut FrontEnd, env: &mut FeEnv) {
    match fe.screen {
        0x43 | 0x44 | 0xD | 0xE => write_peak(fe, env),
        _ => {}
    }
    match fe.screen {
        0x44 => write_campaign_drop(fe, env, fe.globe.drop),
        0xE => write_explore_drop(fe, env, fe.globe.drop),
        _ => {}
    }
    if matches!(fe.screen, 0x44 | 0xE | 0x11) {
        super::panel::write(fe, env, fe.globe.drop);
        let (x, y) = drop_point(fe, fe.globe.drop);
        env.values.set_float(DROP_INDICATOR_X, x);
        env.values.set_float(DROP_INDICATOR_Y, y);
    }
    fe.publish.push("dp_drop_metadata".into());
}

pub(super) fn write_peak(fe: &FrontEnd, env: &mut FeEnv) {
    let Some(p) = fe.career.peak(fe.globe.peak) else { return };
    let title = match p.campaign_type {
        1 => "TXT_PEAK_TYPE_EVENT",
        2 => "TXT_PEAK_TYPE_DD",
        3 => "TXT_PEAK_TYPE_BACKCOUNTRY",
        _ => "TXT_PEAK_TYPE_DETAILS",
    };
    let (enabled, t, asset, name, desc) =
        if world_tour(fe) { (0x46, 0x44, 0x41, 0x43, 0x42) } else { (0x8E, 0x8D, 0x8A, 0x8C, 0x8B) };
    let v = &mut *env.values;
    v.set_bool(enabled, true);
    v.set_str(t, title);
    v.set_str(asset, p.image.clone());
    v.set_str(name, p.title.clone());
    v.set_str(desc, p.info.clone());
}

pub(crate) fn drop_reward(fe: &FrontEnd, drop: u32) -> i64 {
    let reward = fe.career.event(drop).map_or(0, |e| e.reward);
    if fe.profile.drop_state(drop) == DropState::Completed {
        reward.min(ssx_assets::career::profile::CAMPAIGN_REPLAY_CAP)
    } else {
        reward
    }
}

pub(super) fn write_campaign_drop(fe: &FrontEnd, env: &mut FeEnv, drop: u32) {
    let Some(e) = fe.career.event(drop) else { return };
    let state = fe.profile.drop_state(drop);
    let loc = &mut *env.loc;
    let v = &mut *env.values;
    v.set_str(0x72, "");
    v.set_bool(0x84, false);
    v.set_str(0x75, "");
    v.set_str(0x76, "");
    v.set_int(0x3C, drop_reward(fe, drop) as i32);
    v.set_bool(0x3B, false);
    let (body, heading, completed, locked) = match state {
        DropState::Locked => (loc.text(&e.unlock_text), loc.text("TXT_LOCKED"), false, true),
        DropState::Unlocked => (loc.text(&e.goal_body), loc.text("TXT_DROP_DETAILS"), false, false),
        DropState::Completed => (loc.text(&e.goal_body), loc.text("TXT_COMPLETED"), true, false),
    };
    v.set_str(0x71, body);
    v.set_str(0x73, heading);
    v.set_bool(0x50, completed);
    v.set_bool(0x51, locked);
    v.set_bool(0x40, false);
    v.set_bool(0x79, true);
    v.set_str(0x37, "assets\\gamertagIcons\\zoe_round.dds");
    let (mut titles, mut images) = (Vec::new(), Vec::new());
    if e.deadly_descent {
        v.set_int(0x3E, 1);
        let (image, title) = DEADLY_DESCENTS
            .iter()
            .find(|(k, _, _)| *k == drop)
            .map_or(("assets/campaign/image_dd_whiteout.dds", None), |(_, i, t)| (*i, Some(*t)));
        images.push(image.to_string());
        titles.push(title.map_or_else(|| "undefined".to_string(), |t| loc.text(t)));
    } else if e.rivals.is_empty() {
        v.set_int(0x3E, 0);
    } else if e.rivals.len() < 4 {
        v.set_int(0x3E, 2);
        for r in &e.rivals {
            let Some(rider) = fe.career.riders.iter().find(|x| x.key == *r) else { continue };
            titles.push(loc.text(&rider.title));
            images.push(format!("assets\\campaign\\rider_{}.dds", rider.name.to_lowercase()));
        }
        if e.rivals.len() == 1 {
            v.set_bool(0x3B, true);
            images.truncate(1);
        }
    }
    v.set(0x39, crate::game::values::UiValue::List(titles));
    v.set(0x3A, crate::game::values::UiValue::List(images));
}

pub(super) fn write_explore_drop(fe: &FrontEnd, env: &mut FeEnv, drop: u32) {
    let Some(e) = fe.career.event(drop) else { return };
    let loc = &mut *env.loc;
    let v = &mut *env.values;
    v.set_str(0x71, loc.text(&e.goal_body));
    v.set_str(0x73, loc.text(&e.hud_title));
    v.set_bool(0x79, true);
    v.set_str(0x7C, loc.text("TXT_EXPLORE_DROP_LEADERBOARDHEADING"));
    v.set_str(0x80, loc.text("TXT_EXPLORE_DROP_LEADERBOARD_RIDERLABEL"));
    let score_label = match e.event_type {
        5..=7 => Some("TXT_EXPLORE_DROP_LEADERBOARD_TIME"),
        0..=2 => Some("TXT_EXPLORE_DROP_LEADERBOARD_SCORE"),
        3 | 4 => Some("TXT_EXPLORE_DROP_LEADERBOARD_SURVIVED"),
        _ => None,
    };
    if let Some(key) = score_label {
        v.set_str(0x81, loc.text(key));
    }
    v.set_str(0x7D, loc.text("TXT_LEADERBOARD_NEXT_PAGE"));
    v.set_str(0x7E, loc.text("TXT_LEADERBOARD_PREV_PAGE"));
    let board = fe.career.ghost_board(e);
    if let [gold, silver, bronze, ..] = board[..] {
        v.set_int(0x87, gold.value as i32);
        v.set_int(0x88, gold.value as i32);
        v.set_int(0x92, silver.value as i32);
        v.set_int(0x6E, bronze.value as i32);
    }
    let mut rows = Vec::new();
    let mut ghosts = Vec::new();
    let ascending = matches!(e.event_type, 5..=7);
    for (i, g) in board.into_iter().enumerate() {
        let value = g.value;
        let score = if ascending { loc.race_time(value) } else { loc.int(i64::from(value)) };
        let rider = fe.career.riders.iter().find(|r| r.key == g.character);
        let name = rider.map(|r| crate::game::leaderboard::ghost_name(loc, &r.title)).unwrap_or_default();
        let row = crate::game::values::LeaderboardRow {
            value: value as i32,
            name,
            icon: rider.map(|r| r.icon.clone()).unwrap_or_default(),
            score,
            rank_text: (i + 1).to_string(),
            reward: loc.currency(0, true),
            medal: 2 - i as i32,
            rank: i as i32 + 1,
            ..Default::default()
        };
        ghosts.push(ghost_row(&row, rider));
        rows.push(row);
    }
    v.set(0x7B, crate::game::values::UiValue::Rows(ghosts));
    v.set(0x7A, crate::game::values::UiValue::Rows(rows));
    super::panel::write_cost(fe, env, drop);
    write_rival_panel(fe, env, drop);
}

fn ghost_row(
    row: &crate::game::values::LeaderboardRow,
    rider: Option<&ssx_assets::career::Rider>,
) -> crate::game::values::LeaderboardRow {
    crate::game::values::LeaderboardRow { icon: rider.map(|r| r.round_icon.clone()).unwrap_or_default(), ..row.clone() }
}

pub fn drop_node_selected(fe: &mut FrontEnd, env: &mut FeEnv, drop: u32) {
    match fe.game_mode {
        1 => write_campaign_drop(fe, env, drop),
        0 => {
            write_explore_drop(fe, env, drop);
            env.values.set_bool(0x89, false);
            env.values.set_bool(0x8E, false);
            env.values.set_bool(0x79, true);
        }
        _ => {}
    }
    super::panel::write(fe, env, drop);
    fe.publish.push("dp_drop_metadata".into());
}

pub const RIVAL_PANEL: u16 = 0x90;

pub(super) fn value_kind(event_type: i64) -> u32 {
    match event_type {
        0..=2 => 1,
        5..=7 => 0,
        3 | 4 | 8 => 2,
        _ => 3,
    }
}

pub(super) fn rival_reward(fe: &FrontEnd, e: &ssx_assets::career::data::Event, value: u32) -> i64 {
    let time = value_kind(e.event_type) == 0;
    let (mut missed, mut target) = (0usize, 0u32);
    for g in fe.career.medal_targets(e).into_iter().take(3) {
        let reached = g == u32::MAX || (value != u32::MAX && if time { value <= g } else { g <= value });
        if reached {
            target = g;
            break;
        }
        missed += 1;
    }
    let Some(tier) = fe.career.explore_rewards.get(e.reward.clamp(0, 4) as usize) else { return 0 };
    let unk = |key: u32| tier.unk.iter().find(|(k, _)| *k == key).map_or(0.0, |(_, v)| *v);
    match missed {
        0 => {
            let (k1, k2) = (unk(0x659D_C832), unk(0xE810_3A31));
            let off = f64::from((value as i32).wrapping_sub(target as i32)).abs();
            let part = ((off / f64::from(target)) as f32 as f64 / k2) as f32;
            let part = if -part < 0.0 { part } else { 0.0 };
            let part = if 1.0 - part < 0.0 { 1.0 } else { part };
            let bonus = part * (tier.gold as f64 * k1) as f32;
            tier.gold + if bonus > 0.0 { i64::from(bonus as i32 / 100) * 100 } else { 0 }
        }
        1 => tier.silver,
        2 => tier.bronze,
        _ => unk(0x7CF2_365E) as i64,
    }
}

pub(super) fn rival_value(loc: &mut crate::game::loc::Localization, kind: u32, value: u32, flag: bool) -> String {
    if value == u32::MAX {
        return loc.text("TXT_N_A");
    }
    match kind {
        0 if flag => format!("{} {}", loc.race_time(value), loc.text("TXT_RANK_TIME_ABBR")),
        0 => loc.race_time(value),
        1 => loc.int(i64::from(value)),
        2 if flag => loc.distance_with_unit(f64::from(value as f32 / 1000.0)),
        2 => loc.distance(f64::from(value as f32 / 1000.0)),
        _ => String::new(),
    }
}

pub(super) fn write_rival_panel(fe: &FrontEnd, env: &mut FeEnv, drop: u32) {
    use crate::game::values::{RivalPanel, UiValue};
    let Some(e) = fe.career.event(drop) else { return };
    let own = fe.profile.explore_scores.get(&drop).map(|s| s.score).filter(|&s| s != u32::MAX);
    let Some(value) = own else {
        env.values.set(RIVAL_PANEL, UiValue::Rival(Box::default()));
        return;
    };
    let kind = value_kind(e.event_type);
    let time = kind == 0;
    let reward = rival_reward(fe, e, value);
    let loc = &mut *env.loc;
    let heading = if time { "TXT_TIME_TO_BEAT_COLON" } else { "TXT_SCORE_TO_BEAT_COLON" };
    let panel = RivalPanel {
        title: loc.text("TXT_EXPLORE_RIVAL_TARGETPERSONALBEST"),
        // TODO(guess): the player's gamer picture (0x82487EF8 → `img://gamerpic://…`, 0x823BD290) is left empty; settle: the image loader's `gamerpic` scheme.
        gamertag_icon: String::new(),
        score_heading: loc.text(heading),
        score_value: rival_value(loc, kind, value, !time),
        reward_heading: loc.text("TXT_EXPLORE_REWARD_COLON"),
        reward_value: loc.currency(reward, true),
        rival_name: String::new(),
        reward: reward as i32,
        visible: true,
        personal_best: true,
    };
    let name = loc.text("TXT_PERSONAL_BEST");
    env.values.set_str(0x85, name);
    env.values.set_int(0x91, value as i32);
    env.values.set(RIVAL_PANEL, UiValue::Rival(Box::new(panel)));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::values::LeaderboardRow;

    #[test]
    fn a_medal_target_shows_the_riders_round_icon() {
        let rider = ssx_assets::career::Rider {
            key: 0xF78F_F173,
            name: "zoe".into(),
            title: String::new(),
            display_name: String::new(),
            cost: 0,
            locked: false,
            default: false,
            unk_940b9b22: 0,
            stat_bonuses: Vec::new(),
            portrait: String::new(),
            description: String::new(),
            round_icon: "assets/gamertagIcons/zoe_round.dds".into(),
            icon: "assets/gamertagIcons/zoe.dds".into(),
            content: None,
        };
        let row = LeaderboardRow { icon: rider.icon.clone(), score: "02:40.00".into(), rank: 1, ..Default::default() };
        let ghost = ghost_row(&row, Some(&rider));
        assert_eq!(ghost.icon, "assets/gamertagIcons/zoe_round.dds");
        assert_eq!(LeaderboardRow { icon: row.icon.clone(), ..ghost }, row);
        assert_eq!(ghost_row(&row, None).icon, "");
    }
}
