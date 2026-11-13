use ssx_assets::career::DropType;
use ssx_assets::career::profile::DropState;

use crate::avm1::Ctx;
use crate::avm1::value::Value;
use crate::game::fe::{FeEnv, FrontEnd};
use crate::game::values::{DropPanel, UiValue};

pub const DROP_PANEL: u16 = 0xB8;
const PURCHASED_ANIM_KEY: u16 = 0xB9;
const PROGRESSION_ANIM_KEY: u16 = 0xC0;
const PROGRESSION_ANIM_TYPE: u16 = 0xC1;

pub const DIFFICULTY_NAMES: [&str; 4] =
    ["TXT_DIFFICULTY_EASY", "TXT_DIFFICULTY_MEDIUM", "TXT_DIFFICULTY_HARD", "TXT_DIFFICULTY_EXTREME"];
pub const REQUIRED_EQUIPMENT_NAMES: [&str; 7] = [
    "TXT_REQUIREDEQUIPMENT_WINGSUIT",
    "TXT_REQUIREDEQUIPMENT_ICEAXE",
    "TXT_REQUIREDEQUIPMENT_OXYGEN",
    "TXT_REQUIREDEQUIPMENT_THERMALPROTECTION",
    "TXT_REQUIREDEQUIPMENT_ARMOUR",
    "TXT_REQUIREDEQUIPMENT_NIGHTVISION",
    "TXT_REQUIREDEQUIPMENT_WHITEOUTVISION",
];

pub fn requirements(fe: &FrontEnd, env: &mut FeEnv, drop: u32) -> String {
    requirements_text(fe, env.loc, drop)
}

pub fn requirements_text(fe: &FrontEnd, loc: &mut crate::game::loc::Localization, drop: u32) -> String {
    let Some(e) = fe.career.event(drop) else { return String::new() };
    if e.required.is_empty() {
        return String::new();
    }
    let (mut best, mut weight) = (4i64, 0i64);
    for (k, &item) in e.required.iter().enumerate() {
        let w = e.unk_b683b553.get(k).copied().unwrap_or(0);
        if w > weight {
            weight = w;
            best = item;
        }
    }
    let name = |loc: &mut crate::game::loc::Localization, i: i64| {
        REQUIRED_EQUIPMENT_NAMES.get(i as usize).map(|k| loc.text(k)).unwrap_or_default()
    };
    let mut out = name(loc, best);
    for &item in &e.required {
        if item == best {
            continue;
        }
        let t = name(loc, item);
        if t.is_empty() {
            continue;
        }
        if !out.is_empty() {
            out.push_str(&loc.text("TXT_COMMA_SPACE"));
        }
        out.push_str(&t);
    }
    out
}

pub fn drop_fields(fe: &FrontEnd, loc: &mut crate::game::loc::Localization, drop: u32) -> Option<DropPanel> {
    let e = fe.career.event(drop)?;
    let drop_name = fe.career.drop(e.drop).map(|d| loc.text(&d.title)).unwrap_or_default();
    let mut p = DropPanel { drop_name, ..DropPanel::default() };
    let (peak, _) = fe.career.peak_and_range(e);
    p.peak_name = fe.career.peak(peak).map(|pk| loc.text(&pk.title)).unwrap_or_default();
    let type_key = if fe.free_ride_drop.is_some() {
        "TXT_EVENTTYPE_FREERIDE"
    } else if crate::game::fe::globe::TUTORIAL_DROPS.contains(&drop) {
        "TXT_EVENTTYPE_TUTORIAL"
    } else {
        crate::game::hud::results::EVENT_TYPE_NAMES.get(e.event_type as usize).copied().unwrap_or("")
    };
    p.event_type = if type_key.is_empty() { String::new() } else { loc.text(type_key) };
    p.event_type_raw = e.event_type as i32;
    p.difficulty = e.difficulty as i32;
    p.difficulty_text = DIFFICULTY_NAMES.get(e.difficulty as usize).map(|k| loc.text(k)).unwrap_or_default();
    p.requirements = requirements_text(fe, loc, drop);
    p.warning = e.warning as i32;
    p.time_of_day = e.time_of_day as i32;
    p.elevation = loc.distance_with_unit(0.0);
    Some(p)
}

pub fn write(fe: &FrontEnd, env: &mut FeEnv, drop: u32) {
    let Some(e) = fe.career.event(drop) else { return };
    let Some(mut p) = drop_fields(fe, env.loc, drop) else { return };
    match e.drop_type {
        DropType::Explore => {
            let medal = crate::game::fe::explore::fe_medal(fe, 3, drop).0;
            if medal >= 0 {
                p.icon = crate::game::hud::results::medal_icon(medal).to_string();
            }
            p.panel_type = 0;
            // TODO(guess): that list is taken as empty offline (friends' geotags come from the online feed); settle: the list's writer.
            p.friends_geotags = 0;
            p.friends_geotags_text = "0".into();
            if !fe.profile.purchased.contains(&drop) && e.cost != 0 {
                p.cost = e.cost as i32;
                let price = env.loc.currency(e.cost, true);
                p.cost_text = env.loc.format("TXT_UNLOCK_DROP", &[&price]);
                p.icon = "assets\\drop_selector_icons\\drop_locked.dds".into();
            }
            if env.values.int(PURCHASED_ANIM_KEY) as u32 == drop {
                p.anim_type = 3;
                p.icon = "assets\\drop_selector_icons\\drop_locked.dds".into();
                env.values.set_int(PURCHASED_ANIM_KEY, 0);
            }
        }
        DropType::Campaign => {
            p.panel_type = 0;
            p.icon = match fe.profile.drop_state(drop) {
                DropState::Completed => "assets\\drop_selector_icons\\campaign_complete.dds".into(),
                DropState::Locked => "assets\\drop_selector_icons\\campaign_locked.dds".into(),
                DropState::Unlocked if e.deadly_descent => "assets\\drop_selector_icons\\campaign_deadly.dds".into(),
                DropState::Unlocked => String::new(),
            };
            if env.values.int(PROGRESSION_ANIM_KEY) as u32 == drop {
                p.anim_type = env.values.int(PROGRESSION_ANIM_TYPE);
                if p.anim_type == 2 {
                    p.icon = "assets\\drop_selector_icons\\campaign_locked.dds".into();
                }
            }
        }
        _ => {}
    }
    if !fe.profile.purchased.contains(&drop) {
        p.cost = p.cost.max(e.cost as i32);
    }
    env.values.set(DROP_PANEL, UiValue::Panel(Box::new(p)));
}

pub fn write_cost(fe: &FrontEnd, env: &mut FeEnv, drop: u32) {
    let Some(e) = fe.career.event(drop) else { return };
    if !matches!(e.drop_type, DropType::Explore | DropType::Campaign) {
        return;
    }
    let member = fe.profile.explore_scores.get(&drop).is_some_and(|r| r.unk_attempts_remaining != 0);
    env.values.set_int(0x59, e.cost as i32);
    env.values.set_bool(0x5B, member);
    let text = env.loc.currency(e.cost, true);
    env.values.set_str(0x5A, text);
}

pub fn object(cx: &mut Ctx, p: &DropPanel) -> Value {
    let o = Value::Object(cx.new_object());
    let s = |t: &str| Value::str(t);
    let n = |v: i32| Value::Number(f64::from(v));
    let warning = match p.warning {
        1 => "avalanche1",
        2 => "avalanche2",
        3 => "avalanche3",
        4 => "visibility",
        _ => "",
    };
    let time = match p.time_of_day {
        0 => "day",
        1 => "dusk",
        2 => "night",
        _ => "",
    };
    for (k, v) in [
        ("strDropName", s(&p.drop_name)),
        ("strPeakName", s(&p.peak_name)),
        ("strIcon", s(&p.icon)),
        ("strEventType", s(&p.event_type)),
        ("strDifficulty", s(&p.difficulty_text)),
        ("strDropAttributes", s(&p.attributes)),
        ("strElevation", s(&p.elevation)),
        ("strRequirements", s(&p.requirements)),
        ("strNumFriendsGeotags", s(&p.friends_geotags_text)),
        ("strNumFriendsInGlobalEvent", s(&p.friends_in_event_text)),
        ("iNumFriendsGeotags", n(p.friends_geotags)),
        ("iNumFriendsInGlobalEvent", n(p.friends_in_event)),
        ("iDifficulty", n(p.difficulty)),
        ("iDropCost", n(p.cost)),
        ("eEventType", n(p.event_type_raw)),
        ("ePanelType", n(p.panel_type)),
        ("bIsCustomSeed", Value::Bool(p.custom_seed)),
        ("strCustomEventsTitle", s(&p.custom_events_title)),
        ("strNumActiveCustomEvents", s(&p.active_custom_events)),
        ("strDropCost", s(&p.cost_text)),
        ("bIsGearRestricted", Value::Bool(p.gear_restricted)),
        ("iRestrictionLevel", n(p.restriction_level)),
        ("iNumRestrictions", n(p.restrictions)),
        ("strRestriction0", s(&p.restriction0)),
        ("strRestriction1", s(&p.restriction1)),
        ("strMaxGear", s(&p.max_gear)),
        ("strLevel", s(&p.level)),
        ("eAnimType", n(p.anim_type)),
        ("bHasWarning", Value::Bool(p.warning != 0)),
        ("strWarning", s(warning)),
        ("strTime", s(time)),
    ] {
        let _ = cx.set_member(&o, k, v);
    }
    o
}
