use ssx_assets::port_options::MenuFocus;

use crate::avm1::Ctx;
use crate::avm1::value::Value;
use crate::game::fe::{FeEnv, FrontEnd, screens};

pub const GROUND_TUTORIAL: u32 = 0xB0F3_51A2;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct MenuItem {
    pub item: String,
    pub desc: String,
    pub enabled: bool,
    pub focusable: bool,
    pub advance: &'static str,
    pub highlight: &'static str,
    pub rider_net: bool,
    pub back: &'static str,
}

#[derive(Clone, Debug, Default)]
pub struct MainMenu {
    pub items: Vec<MenuItem>,
    pub index: i32,
    pub locked: bool,
    pub active: bool,
    pub last: Option<&'static str>,
}

pub fn write_progress(fe: &FrontEnd, env: &mut FeEnv) {
    let cpro = fe.profile.attribs.get(&u32::from_be_bytes(*b"CPRO")).copied().unwrap_or(0);
    env.values.set_int(0x45, cpro as i32);
    env.values.set_int(0x58, fe.profile.deadly_descents_completed() as i32);
    let (medals, total) = crate::game::fe::explore::node_counts(fe, 0, 0);
    env.values.set_int(0x6F, medals);
    env.values.set_int(0x70, total);
    env.values.set_int(0x16E, 0);
}

pub fn activate(fe: &mut FrontEnd, env: &mut FeEnv) {
    fe.game_mode = -1;
    fe.screen = screens::id::MAIN_MENU;
    let unlocked = fe.profile.drop_state(GROUND_TUTORIAL) == ssx_assets::career::profile::DropState::Completed;
    env.values.set_bool(0xA6, true);
    env.values.set_bool(0xA1, true);
    env.values.set_bool(0xA2, unlocked);
    env.values.set_bool(0xA3, unlocked);
    if unlocked {
        fe.profile.add_persistent_stat("TutorialComplete", 1);
        fe.note("menu: stat TutorialComplete += 1 (0x82588238(0x76, 2, 1))");
    }
    let home = env.loc.text("TXT_SSX_HOME");
    env.values.set_str(0x32, home);
    env.values.set_str(0x33, "");
    env.values.set_int(0x1F4, 0);
    fe.note("FE manager event 1 (message 0xAD162AF6 {1})");
    write_progress(fe, env);
    build_list(fe, env);
    fe.menu.active = true;
    fe.publish.extend(["dp_debug_menu".into(), "dp_debug_menu_index".into(), "dp_sponsored_events_data".into()]);
    env.nav_events.push("backgroundcontainer_disable".into());
}

pub fn build_list(fe: &mut FrontEnd, env: &mut FeEnv) {
    let v = &*env.values;
    let open = !v.bool(0x1B4);
    let campaign = open && v.bool(0xA1);
    let explore = open && v.bool(0xA2);
    let live = open && v.bool(0xA3);
    let (dd, done, total, earnings) = (v.int(0x58), v.int(0x6F), v.int(0x70), v.int(0x16E));
    let loc = &mut *env.loc;
    let earnings = loc.currency(i64::from(earnings), true);
    let back = if fe.ava.hub.on { crate::game::fe::ava::hub::MAIN_MENU_BACK } else { "" };
    fe.menu.items = vec![
        MenuItem {
            item: loc.text("TXT_M_GLOBE_CAMPAIGN"),
            desc: loc.format("TXT_MAIN_MENU_CAMPAIGN_DD_PROGRESS", &[&dd.to_string()]),
            enabled: campaign,
            focusable: campaign,
            advance: "A_MenuCampaign",
            highlight: "A_MenuCampaignHighlight",
            rider_net: false,
            back,
        },
        MenuItem {
            item: loc.text("TXT_M_GLOBE_EXPLORE"),
            desc: loc.format("TXT_MAIN_MENU_EXPLORE_PROGRESS", &[&done.to_string(), &total.to_string()]),
            enabled: explore,
            focusable: explore,
            advance: "A_MenuExplore",
            highlight: "A_MenuExploreHighlight",
            rider_net: false,
            back,
        },
        MenuItem {
            item: loc.text("TXT_M_GLOBE_LIVE_EVENTS"),
            desc: loc.format("TXT_MAIN_MENU_GLOBEL_EVENT_EARNINGS", &[&earnings]),
            enabled: live,
            focusable: live,
            advance: "A_MenuLiveEvents",
            highlight: "A_MenuLiveEventsHighlight",
            rider_net: false,
            back,
        },
        MenuItem {
            item: String::new(),
            desc: loc.text("TXT_MAIN_MENU_RIDERNET_DESC"),
            enabled: open,
            focusable: true,
            advance: "A_MenuRiderNet",
            highlight: "",
            rider_net: true,
            back,
        },
    ];
    let first_open = if campaign {
        0
    } else if explore {
        1
    } else if live {
        2
    } else {
        0
    };
    fe.menu.index = match fe.menu_focus {
        MenuFocus::FirstItem => first_open,
        // TODO(guess): a port choice — the option "Menus: the last item picked" focuses the item last picked while it is enabled and focusable (else the first open mode); settle: none (the game's build 0x825DC168 always focuses the first open mode).
        MenuFocus::LastItem => last_picked(&fe.menu).unwrap_or(first_open),
    };
}

fn last_picked(menu: &MainMenu) -> Option<i32> {
    let last = menu.last?;
    let at = menu.items.iter().position(|it| it.advance == last && it.enabled && it.focusable)?;
    i32::try_from(at).ok()
}

pub fn update(fe: &mut FrontEnd, env: &mut FeEnv) {
    if !fe.menu.active || fe.screen != screens::id::MAIN_MENU {
        return;
    }
    let locked = env.values.bool(0x1B4);
    if locked != fe.menu.locked {
        fe.menu.locked = locked;
        build_list(fe, env);
        fe.publish.push("dp_debug_menu".into());
    }
}

pub fn action(fe: &mut FrontEnd, env: &mut FeEnv, action: &str) {
    if let Some(it) = fe.menu.items.iter().find(|it| it.advance == action) {
        fe.menu.last = Some(it.advance);
    }
    match action {
        "A_MenuCampaign" => {
            fe.note("FE manager event 5 (FEMoveFromModeSelectToGlobe {1}): game mode 1");
            fe.game_mode = 1;
            fe.fe_mode = 1;
            fe.menu_selection = 1;
            if fe.profile.new_game_active() {
                fe.profile.start_new_game();
                fe.note(format!("Profile_StartNewGame → state {}", fe.profile.new_game_state));
            } else {
                fe.go_to_screen(env, screens::id::CAMPAIGN_RANGE);
            }
            fe.note("message 0x32B67D88");
            fe.menu.active = false;
        }
        "A_MenuExplore" if env.values.bool(0xA2) => {
            env.values.set_int(0x232, screens::id::EXPLORE_RANGE as i32);
            let online_enabled = env.values.bool(0x22D);
            if online_enabled && env.values.int(0x22F) != 2 {
                env.values.set_int(0x1F4, 2);
                env.nav_events.push("explore_gate_execute".into());
            } else {
                fe.note("FE manager event 2 (FEMoveFromModeSelectToGlobe {0}): game mode 0");
                fe.game_mode = 0;
                fe.go_to_screen(env, screens::id::EXPLORE_WALL_MENU);
                fe.menu.active = false;
            }
        }
        "A_MenuLiveEvents" if env.values.bool(0xA3) => {
            env.values.set_int(0x232, screens::id::GLOBAL_RANGE as i32);
            if !env.values.bool(0x22D) {
                env.nav_events.push("fe_online_user_required_gate".into());
            } else {
                env.values.set_int(0x1F4, 3);
                env.nav_events.push("go_to_fereconnect".into());
            }
        }
        "A_MenuFreeRide" => {
            fe.game_mode = 2;
            fe.fe_mode = 2;
            fe.menu_selection = 3;
            fe.go_to_screen(env, screens::id::CAMPAIGN_RANGE);
            fe.menu.active = false;
        }
        "A_MenuRiderNet" if !env.values.bool(0x1B4) => env.nav_events.push("open_activity_feed".into()),
        "A_MenuQuickSelect" => {
            fe.menu_selection = 4;
            fe.go_to_screen(env, screens::id::QUICKTRACK);
        }
        "A_MenuConnectEAServer" => fe.go_to_screen(env, screens::id::FE_RECONNECT),
        "A_MenuExploreHighlight"
        | "A_MenuCampaignHighlight"
        | "A_MenuLiveEventsHighlight"
        | "A_MenuActivityFeedHighlight" => {
            let event = match action {
                "A_MenuExploreHighlight" => 6,
                "A_MenuLiveEventsHighlight" => 7,
                "A_MenuCampaignHighlight" => 8,
                _ => 9,
            };
            fe.note(format!("FE manager event {event} (message 0xAD162AF6)"));
        }
        _ => {}
    }
}

pub fn build(fe: &FrontEnd, cx: &mut Ctx, dp: &str) -> Option<Value> {
    match dp {
        "dp_debug_menu" => Some(items_value(cx, &fe.menu.items)),
        "dp_debug_menu_index" => Some(Value::Number(f64::from(fe.menu.index))),
        _ => Some(no_sponsored_events(cx)),
    }
}

pub fn items_value(cx: &mut Ctx, items: &[MenuItem]) -> Value {
    let mut out = Vec::new();
    for (k, it) in items.iter().enumerate() {
        let o = Value::Object(cx.new_object());
        let _ = cx.set_member(&o, "strItem", Value::str(&it.item));
        let _ = cx.set_member(&o, "strModeDesc", Value::str(&it.desc));
        let _ = cx.set_member(&o, "bEnabled", Value::Bool(it.enabled));
        let _ = cx.set_member(&o, "bFocusable", Value::Bool(it.focusable));
        if it.rider_net {
            let _ = cx.set_member(&o, "bUseRiderNetCell", Value::Bool(true));
        }
        let _ = cx.set_member(&o, "eventAdvance", Value::str(it.advance));
        if !it.back.is_empty() {
            let _ = cx.set_member(&o, "eventBack", Value::str(it.back));
        }
        if !it.highlight.is_empty() {
            let _ = cx.set_member(&o, "eventHighlight", Value::str(it.highlight));
            let _ = cx.set_member(&o, "eventHighlightParam", Value::Number(k as f64));
        }
        out.push(o);
    }
    Value::Object(cx.new_array(out))
}

pub fn no_sponsored_events(cx: &mut Ctx) -> Value {
    // TODO(guess): the sponsored events' object (0x825DC800) is online data; offline an empty `arrEventsList` (what the main menu's `Publish` reads); settle: the object's offline fields (0x825DC800).
    let o = Value::Object(cx.new_object());
    let list = Value::Object(cx.new_array(Vec::new()));
    let _ = cx.set_member(&o, "arrEventsList", list);
    let _ = cx.set_member(&o, "bIsMenuItemList", Value::Bool(false));
    o
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(advance: &'static str, open: bool) -> MenuItem {
        MenuItem { enabled: open, focusable: open, advance, ..MenuItem::default() }
    }

    #[test]
    fn the_last_pick_is_found_only_while_it_can_be_picked() {
        let mut m = MainMenu {
            items: vec![item("A_MenuCampaign", true), item("A_MenuExplore", true), item("A_MenuRiderNet", true)],
            ..MainMenu::default()
        };
        assert_eq!(last_picked(&m), None, "nothing picked yet");
        m.last = Some("A_MenuExplore");
        assert_eq!(last_picked(&m), Some(1));
        m.items[1] = item("A_MenuExplore", false);
        assert_eq!(last_picked(&m), None, "a locked item is not focused");
        m.last = Some("A_MenuQuickSelect");
        assert_eq!(last_picked(&m), None, "an item the list does not have");
    }
}
