use std::collections::{BTreeSet, HashMap};

use super::value_names::UI_VALUE_NAMES;

#[derive(Clone, Debug, PartialEq)]
pub enum UiValue {
    Bool(bool),
    Int(i32),
    Float(f32),
    Str(String),
    List(Vec<String>),
    Splits(SplitsCallout),
    Rows(Vec<LeaderboardRow>),
    Boosts(Vec<BoostEntry>),
    Markers(Vec<Marker>),
    Panel(Box<DropPanel>),
    Rival(Box<RivalPanel>),
    Buttons(Vec<PopupButton>),
    Item(Box<super::fe::inventory::UiItem>),
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct PopupButton {
    pub label: String,
    pub event: String,
    pub event_data: i32,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct RivalPanel {
    pub title: String,
    pub gamertag_icon: String,
    pub score_heading: String,
    pub score_value: String,
    pub reward_heading: String,
    pub reward_value: String,
    pub rival_name: String,
    pub reward: i32,
    pub visible: bool,
    pub personal_best: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DropPanel {
    pub drop_name: String,
    pub peak_name: String,
    pub icon: String,
    pub event_type: String,
    pub difficulty_text: String,
    pub attributes: String,
    pub elevation: String,
    pub requirements: String,
    pub friends_geotags_text: String,
    pub friends_in_event_text: String,
    pub friends_geotags: i32,
    pub friends_in_event: i32,
    pub difficulty: i32,
    pub cost: i32,
    pub panel_type: i32,
    pub anim_type: i32,
    pub custom_seed: bool,
    pub custom_events_title: String,
    pub active_custom_events: String,
    pub cost_text: String,
    pub gear_restricted: bool,
    pub restriction_level: i32,
    pub restrictions: i32,
    pub restriction0: String,
    pub restriction1: String,
    pub max_gear: String,
    pub level: String,
    pub warning: i32,
    pub time_of_day: i32,
    pub event_type_raw: i32,
}

impl Default for DropPanel {
    fn default() -> Self {
        DropPanel {
            drop_name: String::new(),
            peak_name: String::new(),
            icon: String::new(),
            event_type: String::new(),
            difficulty_text: String::new(),
            attributes: String::new(),
            elevation: String::new(),
            requirements: String::new(),
            friends_geotags_text: String::new(),
            friends_in_event_text: String::new(),
            friends_geotags: -1,
            friends_in_event: -1,
            difficulty: 0,
            cost: 0,
            panel_type: 1,
            anim_type: 0,
            custom_seed: false,
            custom_events_title: String::new(),
            active_custom_events: String::new(),
            cost_text: String::new(),
            gear_restricted: false,
            restriction_level: 0,
            restrictions: 0,
            restriction0: String::new(),
            restriction1: String::new(),
            max_gear: String::new(),
            level: String::new(),
            warning: 0,
            time_of_day: 0,
            event_type_raw: -1,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Marker {
    pub id: u64,
    pub name: String,
    pub colour: u32,
    pub x: f32,
    pub y: f32,
    pub scale: f32,
    pub alpha: f32,
    pub distance: i32,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct BoostEntry {
    pub id: i32,
    pub image: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SplitsCallout {
    pub ahead: bool,
    pub show_time: bool,
    pub kind: u32,
    pub image: String,
    pub body: String,
    pub time: String,
    pub medal: u32,
    pub name: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct LeaderboardRow {
    pub value: i32,
    pub name: String,
    pub icon: String,
    pub score: String,
    pub rank_text: String,
    pub reward: String,
    pub medal: i32,
    pub rank: i32,
    pub is_player: bool,
    pub score_is_icon: bool,
    pub player_passed: bool,
}

#[derive(Clone, Debug, Default)]
pub struct UiValues {
    slots: HashMap<u16, UiValue>,
    dirty: BTreeSet<u16>,
    by_name: HashMap<&'static str, u16>,
}

impl UiValues {
    pub fn new() -> Self {
        let by_name = UI_VALUE_NAMES.iter().map(|&(id, n)| (n, id)).collect();
        UiValues { slots: HashMap::new(), dirty: BTreeSet::new(), by_name }
    }

    pub fn id(&self, name: &str) -> Option<u16> {
        self.by_name.get(name).copied()
    }

    pub fn name(id: u16) -> Option<&'static str> {
        UI_VALUE_NAMES.binary_search_by_key(&id, |&(i, _)| i).ok().map(|k| UI_VALUE_NAMES[k].1)
    }

    pub fn set(&mut self, id: u16, v: UiValue) {
        self.slots.insert(id, v);
        self.dirty.insert(id);
    }

    pub fn set_bool(&mut self, id: u16, v: bool) {
        self.set(id, UiValue::Bool(v));
    }

    pub fn set_int(&mut self, id: u16, v: i32) {
        self.set(id, UiValue::Int(v));
    }

    pub fn set_float(&mut self, id: u16, v: f32) {
        self.set(id, UiValue::Float(v));
    }

    pub fn set_str(&mut self, id: u16, v: impl Into<String>) {
        self.set(id, UiValue::Str(v.into()));
    }

    pub fn get(&self, id: u16) -> Option<&UiValue> {
        self.slots.get(&id)
    }

    pub fn written(&self, id: u16) -> bool {
        self.slots.contains_key(&id)
    }

    pub fn bool(&self, id: u16) -> bool {
        match self.slots.get(&id) {
            Some(UiValue::Bool(b)) => *b,
            Some(UiValue::Int(i)) => *i != 0,
            Some(UiValue::Float(f)) => *f != 0.0,
            _ => false,
        }
    }

    pub fn int(&self, id: u16) -> i32 {
        match self.slots.get(&id) {
            Some(UiValue::Int(i)) => *i,
            Some(UiValue::Bool(b)) => i32::from(*b),
            Some(UiValue::Float(f)) => *f as i32,
            _ => 0,
        }
    }

    pub fn float(&self, id: u16) -> f32 {
        match self.slots.get(&id) {
            Some(UiValue::Float(f)) => *f,
            Some(UiValue::Int(i)) => *i as f32,
            Some(UiValue::Bool(b)) => f32::from(u8::from(*b)),
            _ => 0.0,
        }
    }

    pub fn str(&self, id: u16) -> &str {
        match self.slots.get(&id) {
            Some(UiValue::Str(s)) => s,
            _ => "",
        }
    }

    pub fn list(&self, id: u16) -> &[String] {
        match self.slots.get(&id) {
            Some(UiValue::List(l)) => l,
            _ => &[],
        }
    }

    pub fn splits(&self, id: u16) -> Option<&SplitsCallout> {
        match self.slots.get(&id) {
            Some(UiValue::Splits(s)) => Some(s),
            _ => None,
        }
    }

    pub fn markers(&self, id: u16) -> &[Marker] {
        match self.slots.get(&id) {
            Some(UiValue::Markers(m)) => m,
            _ => &[],
        }
    }

    pub fn boosts(&self, id: u16) -> &[BoostEntry] {
        match self.slots.get(&id) {
            Some(UiValue::Boosts(b)) => b,
            _ => &[],
        }
    }

    pub fn buttons(&self, id: u16) -> &[PopupButton] {
        match self.slots.get(&id) {
            Some(UiValue::Buttons(b)) => b,
            _ => &[],
        }
    }

    pub fn panel(&self, id: u16) -> Option<&DropPanel> {
        match self.slots.get(&id) {
            Some(UiValue::Panel(p)) => Some(p),
            _ => None,
        }
    }

    pub fn rival(&self, id: u16) -> Option<&RivalPanel> {
        match self.slots.get(&id) {
            Some(UiValue::Rival(p)) => Some(p),
            _ => None,
        }
    }

    pub fn item(&self, id: u16) -> Option<&super::fe::inventory::UiItem> {
        match self.slots.get(&id) {
            Some(UiValue::Item(i)) => Some(i),
            _ => None,
        }
    }

    pub fn rows(&self, id: u16) -> &[LeaderboardRow] {
        match self.slots.get(&id) {
            Some(UiValue::Rows(r)) => r,
            _ => &[],
        }
    }

    pub fn is_dirty(&self, id: u16) -> bool {
        self.dirty.contains(&id)
    }

    pub fn clear_dirty(&mut self) {
        self.dirty.clear();
    }

    pub fn held_copy(&self) -> UiValues {
        UiValues { slots: self.slots.clone(), dirty: BTreeSet::new(), by_name: self.by_name.clone() }
    }

    pub fn copy_written(&mut self, from: &UiValues) {
        for &id in &from.dirty {
            if let Some(v) = from.slots.get(&id) {
                self.set(id, v.clone());
            }
        }
    }

    pub fn written_ids(&self) -> Vec<u16> {
        let mut v: Vec<u16> = self.slots.keys().copied().collect();
        v.sort_unstable();
        v
    }
}

pub mod id {
    pub const EQUIPMENT_LIMITED_TIME_OFFER: u16 = 0x66;
    pub const FE_OFFSET_X: u16 = 0x97;
    pub const FE_OFFSET_Y: u16 = 0x98;
    pub const FLOW_RECEIVE_NOTIFICATIONS: u16 = 0xA6;
    pub const HUDMESSAGE_DROPS_SURVIVED_LABEL: u16 = 0xCF;
    pub const HUDMESSAGE_DROPS_SURVIVED_TEXT: u16 = 0xD0;
    pub const HUDMESSAGE_IS_ENABLED: u16 = 0xD1;
    pub const HUDMESSAGE_IS_SURVIVAL_LOOP_MSG: u16 = 0xD2;
    pub const HUDMESSAGE_MESSAGE: u16 = 0xD3;
    pub const HUDMESSAGE_TITLE: u16 = 0xD4;
    pub const HUDMESSAGE_TOTAL_DESCENT_LABEL: u16 = 0xD5;
    pub const HUDMESSAGE_TOTAL_DESCENT_TEXT: u16 = 0xD6;
    pub const HUDMESSAGE_RIVAL_GAMER_TAG_ICON: u16 = 0xD7;
    pub const HUDMESSAGE_RIVAL_IS_ENABLED: u16 = 0xD8;
    pub const HUDMESSAGE_RIVAL_REWARD_HEADING: u16 = 0xD9;
    pub const HUDMESSAGE_RIVAL_REWARD_VALUE: u16 = 0xDA;
    pub const HUDMESSAGE_RIVAL_RIVAL_NAME: u16 = 0xDB;
    pub const HUDMESSAGE_RIVAL_TITLE: u16 = 0xDC;
    pub const ARMOUR: u16 = 0xDD;
    pub const AWESOME_WARN_TIER: u16 = 0xDE;
    pub const BOOST_NOTIFICATION: u16 = 0xDF;
    pub const BOOST_NOTIFICATION_ENABLED: u16 = 0xE0;
    pub const BOOST_PERCENT: u16 = 0xE1;
    pub const COMBO_BONUS_POINTS: u16 = 0xE2;
    pub const COMBO_CURRENT_TIMER_PERCENTAGE: u16 = 0xE3;
    pub const COMBO_INDICATOR_ACTIVE: u16 = 0xE4;
    pub const COMBO_LEVEL: u16 = 0xE5;
    pub const COMBO_SUCCESS: u16 = 0xE6;
    pub const COMBO_TOTAL_POINTS: u16 = 0xE7;
    pub const COMBO_TRICK_LIST: u16 = 0xE8;
    pub const COMBO_TRICK_POINTS: u16 = 0xE9;
    pub const DESCENT_ACTIVE: u16 = 0xEA;
    pub const DYNAMIC_LEADERBOARD_ENABLED: u16 = 0xEB;
    pub const EXPLORE_LEADERBOARD_IS_ACTIVE: u16 = 0xEC;
    pub const END_ELEVATION: u16 = 0xED;
    pub const FINISH_MESSAGE: u16 = 0xF2;
    pub const FINISH_MESSAGE_ACTIVE: u16 = 0xF3;
    pub const FLOW_LEVEL: u16 = 0xF4;
    pub const GHOST1_NAME: u16 = 0xF5;
    pub const GHOST1_RANK: u16 = 0xF6;
    pub const GHOST1_TIME: u16 = 0xF7;
    pub const GHOST2_NAME: u16 = 0xF8;
    pub const GHOST2_RANK: u16 = 0xF9;
    pub const GHOST2_TIME: u16 = 0xFA;
    pub const GHOST_LIST: u16 = 0xFB;
    pub const GHOST_TIME: u16 = 0xFC;
    pub const HEAD_LAMP_LEVEL: u16 = 0xFD;
    pub const HEALTH: u16 = 0xFE;
    pub const HEALTH_MAX: u16 = 0xFF;
    pub const ICE_AXE_EFFECTIVENESS: u16 = 0x100;
    pub const IS_AWESOME: u16 = 0x101;
    pub const IS_BOOSTING: u16 = 0x102;
    pub const IS_DEATH_PERMANENT: u16 = 0x103;
    pub const IS_ENABLED: u16 = 0x104;
    pub const IS_OXYGEN_USE_REQUESTED: u16 = 0x105;
    pub const IS_PLAYING_SCORE_ANIM: u16 = 0x106;
    pub const IS_REWINDING: u16 = 0x107;
    pub const IS_SUPER_AWESOME: u16 = 0x108;
    pub const IS_THERMAL_REQUIRED: u16 = 0x109;
    pub const LEADER_IS_ACTIVE: u16 = 0x10B;
    pub const LEADER_NAME: u16 = 0x10C;
    pub const LEADERBOARD: u16 = 0x10F;
    pub const LEADERBOARD_IS_ACTIVE: u16 = 0x110;
    pub const METERS_ACTIVE: u16 = 0x111;
    pub const NIS_PROMPT_ACTIVE: u16 = 0x112;
    pub const NIS_PROMPT_CALLOUT: u16 = 0x113;
    pub const NIS_PROMPT_MESSAGE: u16 = 0x114;
    pub const NIS_PROMPT_POPUP_IS_ACTIVE: u16 = 0x115;
    pub const NIS_TITLE_ACTIVE: u16 = 0x116;
    pub const NIS_TITLE_ENABLED: u16 = 0x117;
    pub const NIS_TITLE_INITIALIZED: u16 = 0x118;
    pub const NIS_TITLE_MESSAGE: u16 = 0x119;
    pub const NO_NIS_PROMPT: u16 = 0x11A;
    pub const NIS_IS_ACTIVE: u16 = 0x1B7;
    pub const PAUSE_MENU_DISABLE: u16 = 0x1BB;
    pub const OXYGEN_TANK_LEVEL: u16 = 0x11B;
    pub const PLAYER_DESCENT: u16 = 0x11C;
    pub const PLAYER_DESCENT_WITH_LAPS: u16 = 0x11D;
    pub const PLAYER_RANK: u16 = 0x11E;
    pub const PLAYER_SPEED: u16 = 0x11F;
    pub const PULSE_GOGGLE_LEVEL: u16 = 0x120;
    pub const RACE_STARTED: u16 = 0x121;
    pub const RACE_TIME: u16 = 0x122;
    pub const RANK_IS_ACTIVE: u16 = 0x123;
    pub const READY: u16 = 0x124;
    pub const REWIND_COUNT: u16 = 0x125;
    pub const REWIND_COUNT_ENABLED: u16 = 0x126;
    pub const REWIND_COUNT_INFINITE: u16 = 0x127;
    pub const REWIND_COUNT_INITIAL: u16 = 0x128;
    pub const REWIND_IS_ACTIVE: u16 = 0x129;
    pub const REWIND_POS_X: u16 = 0x12A;
    pub const REWIND_POS_Y: u16 = 0x12B;
    pub const REWIND_SCORE: u16 = 0x12C;
    pub const SPLITS_CALLOUT: u16 = 0x12D;
    pub const SPLITS_CALLOUT_ENABLED: u16 = 0x12E;
    pub const SPLITS_CALLOUT_FORCE_OFF: u16 = 0x12F;
    pub const SCORE: u16 = 0x130;
    pub const SCORE_ACTIVE: u16 = 0x131;
    pub const SCORE_STRING1: u16 = 0x132;
    pub const SCORE_STRING2: u16 = 0x133;
    pub const SCORE_STRING3: u16 = 0x134;
    pub const SCORE_STRING4: u16 = 0x135;
    pub const SCORE_TIMER_ACTIVE: u16 = 0x136;
    pub const SPEED_ACTIVE: u16 = 0x137;
    pub const START_ELEVATION: u16 = 0x138;
    pub const SUPER_AWESOME_ACTIVATION_PERCENTAGE: u16 = 0x139;
    pub const TEMPERATURE: u16 = 0x13A;
    pub const TEMPERATURE_MAX: u16 = 0x13B;
    pub const TIME_ACTIVE: u16 = 0x13C;
    pub const TOMBSTONE_LIST: u16 = 0x13D;
    pub const TRICK_AIR_DIST: u16 = 0x13E;
    pub const TRICK_FLIP_OR_SPIN: u16 = 0x141;
    pub const TRICK_GRAB_NAME: u16 = 0x142;
    pub const TRICK_INDICATOR_ACTIVE: u16 = 0x145;
    pub const TRICK_IS_DUPLICATE: u16 = 0x146;
    pub const TRICK_ROTATION_DEGREES: u16 = 0x147;
    pub const TRICK_TIER: u16 = 0x14A;
    pub const WINGSUIT_ACTIVE: u16 = 0x14B;
    pub const WINGSUIT_EFFECTIVENESS: u16 = 0x14C;
    pub const WINGSUIT_TIMER_POS_X: u16 = 0x14D;
    pub const WINGSUIT_TIMER_POS_Y: u16 = 0x14E;
    pub const WINGSUIT_USAGE: u16 = 0x14F;
    pub const TUTORIAL_HUD_IS_ENABLED: u16 = 0x21E;
    pub const RESULT_BONUS_DESCRIPTION_LIST: u16 = 0x1C8;
    pub const RESULT_BONUS_REWARD_LIST: u16 = 0x1C9;
    pub const RESULT_BONUS_DESCRIPTION_VALUE_LIST: u16 = 0x1CA;
    pub const RESULT_BONUS_ICON_LIST: u16 = 0x1CC;
    pub const RESULT_BONUS_LABEL_LIST: u16 = 0x1CD;
    pub const RESULT_BONUS_TOTAL_REWARD_LIST: u16 = 0x1CF;
    pub const RESULT_ZERO_CREDITS_STRING: u16 = 0x1D0;
    pub const RESULT_CAMPAIGN_TRACK_SUCCESS: u16 = 0x1D1;
    pub const RESULT_DEATH_BODY: u16 = 0x1D2;
    pub const RESULT_DESCENDED: u16 = 0x1D3;
    pub const RESULT_GAME_MODE: u16 = 0x1D4;
    pub const RESULT_GOAL_HEADING: u16 = 0x1D5;
    pub const RESULT_GOAL_ICON: u16 = 0x1D6;
    pub const RESULT_GOAL_IS_COMPLETE: u16 = 0x1D7;
    pub const RESULT_GOAL_PLAYER_SCORE: u16 = 0x1D8;
    pub const RESULT_GOAL_PLAYER_SCORE_LABEL: u16 = 0x1D9;
    pub const RESULT_GOAL_REWARD: u16 = 0x1DA;
    pub const RESULT_GOAL_SUCCESS_LABEL: u16 = 0x1DB;
    pub const RESULT_GOAL_TARGET_LABEL: u16 = 0x1DC;
    pub const RESULT_GOAL_TARGET_SCORE: u16 = 0x1DD;
    pub const RESULT_GOAL_MEDAL: u16 = 0x1DE;
    pub const RESULT_LEADERBOARD_IS_ENABLED: u16 = 0x1E0;
    pub const RESULT_LEADERBOARD_LEADERBOARD: u16 = 0x1E1;
    pub const RESULT_LEADERBOARD_TOTAL_PASSED_COUNT: u16 = 0x1E2;
    pub const RESULT_LEADERBOARD_TOTAL_REWARD: u16 = 0x1E3;
    pub const RESULT_LEVELUP_CHARACTER_PIC: u16 = 0x1E4;
    pub const RESULT_LEVELUP_OLD_PERCENT: u16 = 0x1E5;
    pub const RESULT_LEVELUP_OLD_LEVEL: u16 = 0x1E6;
    pub const RESULT_LEVELUP_NEW_PERCENT: u16 = 0x1E7;
    pub const RESULT_LEVELUP_NEW_LEVEL: u16 = 0x1E8;
    pub const RESULT_LEVELUP_XP_GAINED: u16 = 0x1E9;
    pub const RESULT_MONEY_PREVIOUS: u16 = 0x1EA;
    pub const RESULT_POINTS: u16 = 0x1EB;
    pub const RESULT_REWIND_COUNT: u16 = 0x1EC;
    pub const RESULT_SCREEN_DATA_READY_FOR_PUBLISH: u16 = 0x1ED;
    pub const RESULT_TIME: u16 = 0x1EE;
    pub const SETTINGS_GAMEPLAY_CONTROLLER_SCHEME: u16 = 0x1F0;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_defaults() {
        let mut v = UiValues::new();
        assert_eq!(v.id("HUD_PlayerSpeed"), Some(id::PLAYER_SPEED));
        assert_eq!(UiValues::name(id::RACE_TIME), Some("HUD_RaceTime"));
        assert_eq!(UI_VALUE_NAMES.len(), 556);
        assert!(UI_VALUE_NAMES.windows(2).all(|w| w[0].0 < w[1].0));
        assert_eq!(v.float(id::PLAYER_SPEED), 0.0);
        assert!(!v.is_dirty(id::PLAYER_SPEED));
        v.set_float(id::PLAYER_SPEED, 12.5);
        assert!(v.is_dirty(id::PLAYER_SPEED));
        assert_eq!(v.float(id::PLAYER_SPEED), 12.5);
        v.clear_dirty();
        assert!(!v.is_dirty(id::PLAYER_SPEED));
    }
}
