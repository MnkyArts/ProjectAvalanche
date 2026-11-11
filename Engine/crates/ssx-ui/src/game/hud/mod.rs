pub mod billboards;
pub mod boost;
pub mod dynlb;
pub mod finish;
pub mod hint;
pub mod meters;
pub mod nis;
pub mod results;
pub mod rewind;
pub mod scoring;
pub mod text;
pub mod timepanel;
pub mod trick;

use super::clip::Clip;
use super::loc::Localization;
use super::values::{UiValue, UiValues, id};
use crate::avm1::Ctx;
use crate::avm1::value::Value;

pub struct HudEnv<'a> {
    pub values: &'a mut UiValues,
    pub loc: &'a mut Localization,
    pub event_type: i32,
    pub sounds: &'a mut Vec<u32>,
    pub nav_events: &'a mut Vec<String>,
    pub gear: [bool; 15],
}

pub trait Component {
    fn bindings(&self) -> &'static [&'static str];
    fn bind(&mut self, cx: &mut Ctx, env: &mut HudEnv, name: &str, clip: Clip);
    fn unbind(&mut self, name: &str);
    fn update(&mut self, cx: &mut Ctx, env: &mut HudEnv, dt: f64);
}

#[derive(Clone, Copy, Debug)]
pub struct DataField {
    pub value: u16,
    pub kind: u8,
    pub name: &'static str,
    pub fmt: u8,
}

#[derive(Clone, Copy, Debug)]
pub struct DataObject {
    pub name: &'static str,
    pub fields: &'static [DataField],
    pub statics: &'static [(&'static str, &'static str)],
}

const fn f(value: u16, kind: u8, name: &'static str) -> DataField {
    DataField { value, kind, name, fmt: 0 }
}

pub const HUD_DATA: &[DataObject] = &[
    DataObject {
        name: "hud_message",
        statics: &[],
        fields: &[
            f(0xD1, 0, "isEnabled"),
            f(0xD4, 4, "title"),
            f(0xD3, 4, "message"),
            f(0xCF, 4, "dropsSurvivedLabel"),
            f(0xD0, 4, "dropsSurvivedText"),
            f(0xD5, 4, "totalDescentLabel"),
            f(0xD6, 4, "totalDescentText"),
            f(0xD2, 0, "isRestartSurvivalMsg"),
        ],
    },
    DataObject {
        name: "hud_message_rival",
        statics: &[],
        fields: &[
            f(0xD8, 0, "isEnabled"),
            f(0xDC, 4, "title"),
            f(0xD7, 4, "gamertagIcon"),
            f(0xDB, 4, "rivalName"),
            f(0xDA, 4, "rewardValue"),
            f(0xD9, 4, "rewardHeading"),
        ],
    },
    DataObject {
        name: "tutorial_hud",
        statics: &[],
        fields: &[
            f(0x21D, 0, "isDirty"),
            f(0x21E, 0, "isEnabled"),
            f(0x21F, 4, "help"),
            f(0x220, 4, "objective"),
            f(0x221, 4, "summary"),
            f(0x222, 0, "success"),
            f(0x224, 7, "buttonlocation"),
            f(0x223, 7, "buttonid"),
            f(0x225, 5, "buttonstring"),
            f(0x226, 0, "showLeftTweak"),
            f(0x227, 0, "showRightTweak"),
        ],
    },
];

impl DataObject {
    pub fn dirty(&self, values: &UiValues) -> bool {
        self.fields.iter().any(|f| values.is_dirty(f.value))
    }

    pub fn build(&self, cx: &mut Ctx, values: &UiValues, loc: &mut Localization) -> Value {
        let o = Value::Object(cx.new_object());
        for fd in self.fields {
            if fd.fmt > 6 {
                continue;
            }
            if fd.fmt != 0 {
                let v = formatted(cx, values, loc, fd);
                let _ = cx.set_member(&o, fd.name, v);
                continue;
            }
            let v = match fd.kind {
                0 => Value::Bool(values.bool(fd.value)),
                1 => Value::Number(f64::from(values.int(fd.value))),
                3 => Value::Number(f64::from(values.float(fd.value))),
                4 => Value::str(values.str(fd.value)),
                8 => {
                    let mut rows = Vec::new();
                    for r in values.rows(fd.value) {
                        let row = Value::Object(cx.new_object());
                        for (k, v) in [
                            ("strGamertag", Value::str(&r.name)),
                            ("strGamertagIcon", Value::str(&r.icon)),
                            ("strScore", Value::str(&r.score)),
                            ("strRank", Value::str(&r.rank_text)),
                            ("strAccumReward", Value::str(&r.reward)),
                            ("iMedal", Value::Number(f64::from(r.medal))),
                            ("iRank", Value::Number(f64::from(r.rank))),
                            ("bIsPlayer", Value::Bool(r.is_player)),
                            ("bScoreIsIcon", Value::Bool(r.score_is_icon)),
                            ("bPlayerPassed", Value::Bool(r.player_passed)),
                        ] {
                            let _ = cx.set_member(&row, k, v);
                        }
                        rows.push(row);
                    }
                    Value::Object(cx.new_array(rows))
                }
                9 => {
                    let p = values.rival(fd.value).cloned().unwrap_or_default();
                    let panel = Value::Object(cx.new_object());
                    for (k, v) in [
                        ("strTitle", Value::str(&p.title)),
                        ("strGamertagIcon", Value::str(&p.gamertag_icon)),
                        ("strScoreHeading", Value::str(&p.score_heading)),
                        ("strScoreValue", Value::str(&p.score_value)),
                        ("strRewardHeading", Value::str(&p.reward_heading)),
                        ("strRewardValue", Value::str(&p.reward_value)),
                        ("strRivalName", Value::str(&p.rival_name)),
                        ("iReward", Value::Number(f64::from(p.reward))),
                        ("bIsVisible", Value::Bool(p.visible)),
                    ] {
                        let _ = cx.set_member(&panel, k, v);
                    }
                    panel
                }
                10 => match values.panel(fd.value) {
                    Some(p) => crate::game::fe::globe::panel::object(cx, p),
                    None => crate::game::fe::globe::panel::object(cx, &Default::default()),
                },
                7 => {
                    let items = values.list(fd.value).iter().map(|s| Value::Number(s.parse().unwrap_or(0.0))).collect();
                    Value::Object(cx.new_array(items))
                }
                6 => {
                    let items = values.list(fd.value).iter().map(|s| Value::Bool(s == "true" || s == "1")).collect();
                    Value::Object(cx.new_array(items))
                }
                _ => {
                    let items = values.list(fd.value).iter().map(|s| Value::str(s)).collect();
                    Value::Object(cx.new_array(items))
                }
            };
            let _ = cx.set_member(&o, fd.name, v);
        }
        for (name, key) in self.statics {
            let text = loc.text(key);
            let _ = cx.set_member(&o, name, Value::str(&text));
        }
        o
    }
}

fn formatted(cx: &mut Ctx, values: &UiValues, loc: &mut Localization, fd: &DataField) -> Value {
    match fd.fmt {
        1 => Value::str(&loc.race_time(values.int(fd.value).max(0) as u32)),
        2 => Value::str(&loc.currency(i64::from(values.int(fd.value)), true)),
        3 => {
            let key = values.str(fd.value).to_string();
            Value::str(&if key.is_empty() { String::new() } else { loc.text(&key) })
        }
        4 => {
            let items: Vec<Value> = values.list(fd.value).to_vec().iter().map(|k| Value::str(&loc.text(k))).collect();
            Value::Object(cx.new_array(items))
        }
        5 => Value::str(&loc.int(i64::from(values.int(fd.value)))),
        _ => Value::str(&loc.abbreviated_int(values.int(fd.value) as u32)),
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum HudMessage {
    BanksFlushed { captured: bool },
    ScoreComboEnded(trick::ComboEnded),
}

#[derive(Default)]
pub struct HudProvider {
    pub messages: Vec<HudMessage>,
    pub time_panel: timepanel::TimePanel,
    pub ghosts: billboards::GhostData,
    pub tombstones: billboards::Tombstones,
    pub scoring: scoring::Scoring,
    pub trick: trick::TrickIndicator,
    pub rewind: rewind::RewindIndicator,
    pub finish: finish::FinishMessage,
    pub meters: meters::HudMeters,
    pub boost: boost::BoostNotification,
    pub dynlb: dynlb::DynamicLeaderboard,
    pub nis_prompt: nis::NisPrompt,
    pub nis_title: nis::NisTitle,
    pub hint: hint::HintPanel,
    frame: Option<Clip>,
    frame_active: Option<Clip>,
    frame_started: bool,
    frame_animated: bool,
    sent: [bool; 5],
    pub unknown: Vec<String>,
}

impl HudProvider {
    pub fn release(&mut self, provider: &str) -> bool {
        match provider {
            "hud" => {
                let (finish, meters, boost, dynlb) = (
                    std::mem::take(&mut self.finish),
                    std::mem::take(&mut self.meters),
                    std::mem::take(&mut self.boost),
                    std::mem::take(&mut self.dynlb),
                );
                let (nis_prompt, nis_title, unknown, hint) = (
                    std::mem::take(&mut self.nis_prompt),
                    std::mem::take(&mut self.nis_title),
                    std::mem::take(&mut self.unknown),
                    std::mem::take(&mut self.hint),
                );
                *self = HudProvider {
                    finish,
                    meters,
                    boost,
                    dynlb,
                    nis_prompt,
                    nis_title,
                    unknown,
                    hint,
                    ..Default::default()
                };
            }
            "finishmessage" => self.finish = Default::default(),
            "hudmeters" => self.meters = Default::default(),
            "boostnotification" => self.boost = Default::default(),
            "dynamicleaderboard" => self.dynlb = Default::default(),
            "nisprompt" => self.nis_prompt = Default::default(),
            "nistitle" => self.nis_title = Default::default(),
            _ => return false,
        }
        true
    }

    pub fn binding_names(&self) -> Vec<&'static str> {
        let mut v = vec!["db_hud_frame", "db_hud_frame_active"];
        v.extend(self.time_panel.bindings());
        v.extend(self.ghosts.bindings());
        v.extend(self.tombstones.bindings());
        v.extend(self.scoring.bindings());
        v.extend(self.trick.bindings());
        v.extend(self.rewind.bindings());
        v.extend(self.finish.bindings());
        v.extend(self.meters.bindings());
        v.extend(self.boost.bindings());
        v.extend(self.dynlb.bindings());
        v.extend(self.nis_prompt.bindings());
        v.extend(self.nis_title.bindings());
        v.extend(hint::HintPanel::BINDINGS);
        v
    }

    pub fn bind(&mut self, cx: &mut Ctx, env: &mut HudEnv, name: &str, clip: Clip) -> bool {
        match name {
            "db_hud_frame" => self.frame = Some(clip),
            "db_hud_frame_active" => self.frame_active = Some(clip),
            n if self.time_panel.bindings().contains(&n) => self.time_panel.bind(cx, env, n, clip),
            n if self.ghosts.bindings().contains(&n) => self.ghosts.bind(cx, env, n, clip),
            n if self.tombstones.bindings().contains(&n) => self.tombstones.bind(cx, env, n, clip),
            n if self.scoring.bindings().contains(&n) => self.scoring.bind(cx, env, n, clip),
            n if self.trick.bindings().contains(&n) => self.trick.bind(cx, env, n, clip),
            n if self.rewind.bindings().contains(&n) => self.rewind.bind(cx, env, n, clip),
            n if self.finish.bindings().contains(&n) => self.finish.bind(cx, env, n, clip),
            n if self.meters.bindings().contains(&n) => self.meters.bind(cx, env, n, clip),
            n if self.boost.bindings().contains(&n) => self.boost.bind(cx, env, n, clip),
            n if self.dynlb.bindings().contains(&n) => self.dynlb.bind(cx, env, n, clip),
            n if self.nis_prompt.bindings().contains(&n) => self.nis_prompt.bind(cx, env, n, clip),
            n if self.nis_title.bindings().contains(&n) => self.nis_title.bind(cx, env, n, clip),
            n if hint::HintPanel::BINDINGS.contains(&n) => self.hint.bind(n, clip),
            n => {
                self.unknown.push(n.to_string());
                return false;
            }
        }
        true
    }

    pub fn unbind(&mut self, name: &str) {
        match name {
            "db_hud_frame" => self.frame = None,
            "db_hud_frame_active" => self.frame_active = None,
            n if self.time_panel.bindings().contains(&n) => self.time_panel.unbind(n),
            n if self.ghosts.bindings().contains(&n) => self.ghosts.unbind(n),
            n if self.tombstones.bindings().contains(&n) => self.tombstones.unbind(n),
            n if self.scoring.bindings().contains(&n) => self.scoring.unbind(n),
            n if self.trick.bindings().contains(&n) => self.trick.unbind(n),
            n if self.rewind.bindings().contains(&n) => self.rewind.unbind(n),
            n if self.finish.bindings().contains(&n) => self.finish.unbind(n),
            n if self.meters.bindings().contains(&n) => self.meters.unbind(n),
            n if self.boost.bindings().contains(&n) => self.boost.unbind(n),
            n if self.dynlb.bindings().contains(&n) => self.dynlb.unbind(n),
            n if self.nis_prompt.bindings().contains(&n) => self.nis_prompt.unbind(n),
            n if self.nis_title.bindings().contains(&n) => self.nis_title.unbind(n),
            n if hint::HintPanel::BINDINGS.contains(&n) => self.hint.unbind(n),
            _ => {}
        }
    }

    pub fn update(&mut self, cx: &mut Ctx, env: &mut HudEnv, dt: f64) {
        self.update_frame(cx, env);
        let v = &*env.values;
        let started = v.bool(id::RACE_STARTED);
        let toggles: [(bool, u16, &str); 5] = [
            (started && v.bool(id::METERS_ACTIVE), id::METERS_ACTIVE, "hudmeters_enable"),
            (!started && v.bool(0x112), 0x112, "nisprompt_enable"),
            (!started && v.bool(0x116), 0x116, "nistitle_enable"),
            (
                started && v.bool(id::DYNAMIC_LEADERBOARD_ENABLED),
                id::DYNAMIC_LEADERBOARD_ENABLED,
                "dynamicleaderboard_enable",
            ),
            (v.bool(id::FINISH_MESSAGE_ACTIVE), id::FINISH_MESSAGE_ACTIVE, "finishmessage_enable"),
        ];
        for (k, (fire, flag, event)) in toggles.into_iter().enumerate() {
            if !self.sent[k] {
                if fire {
                    env.nav_events.push(event.to_string());
                    self.sent[k] = true;
                }
            } else if !v.bool(flag) {
                self.sent[k] = false;
            }
        }
        // TODO(guess): 0x825E05A8 (a live event ended while paused → action 0x22) is not ported; settle: none (no live events in the port).
        for m in std::mem::take(&mut self.messages) {
            match m {
                HudMessage::BanksFlushed { captured } => self.trick.banks_flushed(cx, env, captured),
                HudMessage::ScoreComboEnded(e) => self.trick.combo_ended(cx, env, e, &mut self.scoring),
            }
        }
        self.time_panel.update(cx, env, dt);
        self.ghosts.update(cx, env, dt);
        self.tombstones.update(cx, env, dt);
        self.scoring.update(cx, env, dt);
        self.trick.update(cx, env, dt);
        self.rewind.update(cx, env, dt);
        self.finish.update(cx, env, dt);
        self.meters.update(cx, env, dt);
        self.boost.update(cx, env, dt);
        self.dynlb.update(cx, env, dt);
        self.nis_prompt.update(cx, env, dt);
        self.nis_title.update(cx, env, dt);
    }

    fn update_frame(&mut self, cx: &mut Ctx, env: &mut HudEnv) {
        let (Some(frame), Some(_)) = (self.frame.as_mut(), self.frame_active.as_ref()) else { return };
        let started = env.values.bool(id::RACE_STARTED);
        if started == self.frame_started {
            return;
        }
        self.frame_started = started;
        let method = match (started, self.frame_animated) {
            (true, false) => {
                self.frame_animated = true;
                "AnimateIn"
            }
            (true, true) => "GoToIdleState",
            (false, _) => "GoToInitialState",
        };
        frame.invoke(cx, method, &[]);
    }
}

pub fn elements_off(values: &mut UiValues) {
    for i in [
        id::RACE_STARTED,
        id::DESCENT_ACTIVE,
        id::SPEED_ACTIVE,
        id::TIME_ACTIVE,
        id::SCORE_ACTIVE,
        id::TRICK_INDICATOR_ACTIVE,
        id::COMBO_INDICATOR_ACTIVE,
        id::RANK_IS_ACTIVE,
        id::LEADER_IS_ACTIVE,
        id::LEADERBOARD_IS_ACTIVE,
        id::DYNAMIC_LEADERBOARD_ENABLED,
        id::METERS_ACTIVE,
        id::TUTORIAL_HUD_IS_ENABLED,
        id::IS_ENABLED,
        id::WINGSUIT_ACTIVE,
    ] {
        values.set_bool(i, false);
    }
}

pub fn set_hud_shown(values: &mut UiValues, show: bool, nis_active: bool, drop_key: u32) {
    if !show || nis_active {
        elements_off(values);
        return;
    }
    show_drop_elements(values, drop_key);
}

// TODO(guess): the game's first score-manager update of a level comes before the nav's `showHUD`; settle: the order of the level's first frame and `go_to_hud` (0x8259BCF8 on, `FENotificationRestartGameplay` 0x5A5A5C39).
pub fn level_start(values: &mut UiValues) {
    values.set_bool(id::NIS_IS_ACTIVE, true);
    // TODO(guess): the leaderboard rows are emptied with each level, so an event with no standings writer (Trick Score, Survive To Bottom, Race Clock, Free Ride) shows no board; known: `UiValues` lives for the app and the drop elements enable the dynamic leaderboard in every drop; settle: where the game clears UI 0x10F on a level's load.
    values.set(id::LEADERBOARD, UiValue::Rows(Vec::new()));
}

pub fn show_drop_elements(values: &mut UiValues, drop_key: u32) {
    for (i, b) in [
        (id::RACE_STARTED, true),
        (id::IS_ENABLED, true),
        (id::DESCENT_ACTIVE, false),
        (id::SPEED_ACTIVE, false),
        (id::TIME_ACTIVE, false),
        (id::SCORE_ACTIVE, false),
        (id::TRICK_INDICATOR_ACTIVE, false),
        (id::COMBO_INDICATOR_ACTIVE, false),
        (id::SCORE_TIMER_ACTIVE, false),
        (id::DYNAMIC_LEADERBOARD_ENABLED, true),
        (id::METERS_ACTIVE, true),
    ] {
        values.set_bool(i, b);
    }
    // TODO(guess): live / global events (0x82577E78 true: the event's +0x4C flags) are not in the port, every drop takes the World Tour / Explore branch; settle: none (no live events in the port).
    match drop_key {
        0xC145_699A => {
            for (i, b) in [
                (id::DESCENT_ACTIVE, false),
                (id::SPEED_ACTIVE, false),
                (id::SCORE_ACTIVE, true),
                (id::TRICK_INDICATOR_ACTIVE, true),
                (id::COMBO_INDICATOR_ACTIVE, true),
                (id::SCORE_TIMER_ACTIVE, true),
                (id::TIME_ACTIVE, false),
                (id::DYNAMIC_LEADERBOARD_ENABLED, true),
                (id::TUTORIAL_HUD_IS_ENABLED, true),
            ] {
                values.set_bool(i, b);
            }
        }
        0xE389_832F => {}
        _ => {
            for i in [
                id::DESCENT_ACTIVE,
                id::SPEED_ACTIVE,
                id::SCORE_ACTIVE,
                id::TRICK_INDICATOR_ACTIVE,
                id::COMBO_INDICATOR_ACTIVE,
                id::SCORE_TIMER_ACTIVE,
                id::TIME_ACTIVE,
                id::DYNAMIC_LEADERBOARD_ENABLED,
            ] {
                values.set_bool(i, true);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_hud_shown_under_an_nis_has_no_element() {
        let mut v = UiValues::new();
        show_drop_elements(&mut v, 0x1234_5678);
        assert!(v.bool(id::TIME_ACTIVE) && v.bool(id::SCORE_ACTIVE) && v.bool(id::RACE_STARTED));
        set_hud_shown(&mut v, true, true, 0x1234_5678);
        for i in [id::RACE_STARTED, id::TIME_ACTIVE, id::SCORE_ACTIVE, id::METERS_ACTIVE, id::IS_ENABLED] {
            assert!(!v.bool(i), "{i:#x} stays off under the NIS");
        }
        set_hud_shown(&mut v, true, false, 0x1234_5678);
        assert!(v.bool(id::TIME_ACTIVE) && v.bool(id::SPEED_ACTIVE) && v.bool(id::DESCENT_ACTIVE));
        set_hud_shown(&mut v, false, false, 0x1234_5678);
        assert!(!v.bool(id::TIME_ACTIVE) && !v.bool(id::IS_ENABLED));
    }

    #[test]
    fn a_new_level_drops_the_last_events_leaderboard() {
        use crate::game::leaderboard::{RaceEntrant, TrickBot, TrickStandings};
        use crate::game::loc::{Language, Localization};
        use ssx_physics::event::EventType;
        let mut v = UiValues::new();
        let mut loc = Localization::empty(Language::English);
        let player = RaceEntrant { name: "elise".into(), ..Default::default() };
        let griff = TrickBot { name: "griff".into(), live: 1_210_877.0, recorded: 1_210_877, ..Default::default() };
        let mut trick = TrickStandings::default();
        trick.update(&mut v, &mut loc, EventType::TrickBots, &player, 19_667_448.0, 1.0, true, &[griff]);
        assert_eq!(v.rows(id::LEADERBOARD).len(), 2);
        level_start(&mut v);
        show_drop_elements(&mut v, 0x1234_5678);
        assert!(v.bool(id::DYNAMIC_LEADERBOARD_ENABLED));
        assert!(v.rows(id::LEADERBOARD).is_empty(), "the trick event's standings stay off the next drop");
    }

    #[test]
    fn the_first_drops_elements() {
        let mut v = UiValues::new();
        show_drop_elements(&mut v, 0xC145_699A);
        assert!(v.bool(id::SCORE_ACTIVE) && v.bool(id::TUTORIAL_HUD_IS_ENABLED));
        assert!(!v.bool(id::TIME_ACTIVE) && !v.bool(id::SPEED_ACTIVE) && !v.bool(id::DESCENT_ACTIVE));
        let mut v = UiValues::new();
        show_drop_elements(&mut v, 0xE389_832F);
        assert!(v.bool(id::RACE_STARTED) && v.bool(id::METERS_ACTIVE));
        assert!(!v.bool(id::SCORE_ACTIVE) && !v.bool(id::TIME_ACTIVE));
    }
}
