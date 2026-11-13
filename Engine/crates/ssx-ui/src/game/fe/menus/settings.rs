use ssx_assets::port_options::MenuFocus;

use crate::avm1::Ctx;
use crate::avm1::value::Value;
use crate::game::fe::{ActionParam, FeEnv, FrontEnd, screens};
use crate::game::loc::Localization;
use crate::game::values::UiValues;

pub const CATEGORY: u16 = 0x1F2;
pub const IN_PAUSE_MENU: u16 = 0x1F3;
pub const AUDIO_RESET_FLAG: u16 = 0x25;

pub struct Item {
    pub dp: &'static str,
    pub value: u16,
    pub max: i32,
    pub wrap: bool,
    pub title: &'static str,
    pub labels: &'static [&'static str],
}

const VOLUME: &[&str] = &[];

pub const ITEMS: [Item; 11] = [
    Item {
        dp: "dp_settings_audio_setup",
        value: 0x28,
        max: 1,
        wrap: true,
        title: "TXT_SETTINGS_LABEL_AUDIO_SETUP",
        labels: &["TXT_SETTINGS_TOGGLE_TV", "TXT_SETTINGS_TOGGLE_HOME_THEATRE"],
    },
    Item {
        dp: "dp_settings_pilot_speech_volume",
        value: 0x23,
        max: 10,
        wrap: false,
        title: "TXT_SETTINGS_LABEL_PILOT_SPEECH_VOLUME",
        labels: VOLUME,
    },
    Item {
        dp: "dp_settings_rider_speech_volume",
        value: 0x26,
        max: 10,
        wrap: false,
        title: "TXT_SETTINGS_LABEL_RIDER_SPEECH_VOLUME",
        labels: VOLUME,
    },
    Item {
        dp: "dp_settings_game_sfx_volume",
        value: 0x21,
        max: 10,
        wrap: false,
        title: "TXT_SETTINGS_LABEL_GAME_SFX_VOLUME",
        labels: VOLUME,
    },
    Item {
        dp: "dp_settings_ui_volume",
        value: 0x2A,
        max: 10,
        wrap: false,
        title: "TXT_SETTINGS_LABEL_UI_SFX_VOLUME",
        labels: VOLUME,
    },
    Item {
        dp: "dp_settings_remix",
        value: 0x24,
        max: 1,
        wrap: true,
        title: "TXT_SETTINGS_LABEL_REMIX",
        labels: &["TXT_OFF", "TXT_ON"],
    },
    Item {
        dp: "dp_settings_ssx_music_volume",
        value: 0x27,
        max: 11,
        wrap: false,
        title: "TXT_SETTINGS_LABEL_SSX_MUSIC_VOLUME",
        labels: VOLUME,
    },
    Item {
        dp: "dp_settings_personal_music_volume",
        value: 0x22,
        max: 11,
        wrap: false,
        title: "TXT_SETTINGS_LABEL_PERSONAL_MUSIC_VOLUME",
        labels: VOLUME,
    },
    Item {
        dp: "dp_settings_controller_scheme",
        value: 0x1F0,
        max: 1,
        wrap: true,
        title: "TXT_SETTINGS_LABEL_CONT_CONFIG",
        labels: &["TXT_SETTINGS_TOGGLE_CONT_CONFIG_STANDARD", "TXT_SETTINGS_TOGGLE_CONT_CONFIG_CLASSIC"],
    },
    Item {
        dp: "dp_settings_reset_tutorial_messages",
        value: 0x1F1,
        max: 1,
        wrap: true,
        title: "TXT_SETTINGS_LABEL_RESET_TUTORIAL_MESSAGES",
        labels: &["TXT_NO", "TXT_YES"],
    },
    Item {
        dp: "dp_settings_tricky_music",
        value: 0x29,
        max: 2,
        wrap: true,
        title: "TXT_SETTINGS_LABEL_TRICKY_MUSIC",
        labels: &["TXT_OFF", "TXT_STANDARD", "TXT_ALWAYS"],
    },
];

pub const AUDIO_PAGE: [&str; 9] = [
    "settings_audio_setup",
    "settings_pilot_speech_volume",
    "settings_rider_speech_volume",
    "settings_game_sfx_volume",
    "settings_ui_volume",
    "settings_remix",
    "settings_tricky_music",
    "settings_ssx_music_volume",
    "settings_personal_music_volume",
];

pub fn audio_values(s: &ssx_assets::sfx::settings::AudioSettings) -> [(u16, i32); 9] {
    s.ui_values().map(|(id, v)| (id as u16, v))
}

#[derive(Clone, Debug, Default)]
pub struct Settings {
    pub active: bool,
    pub originals: Vec<(u16, i32)>,
    pub from_pause: bool,
    pub category: i32,
    pub changed: bool,
    pub close_feed: bool,
    pub last_category: Option<i32>,
}

pub fn init_values(values: &mut UiValues, store: &[(u16, i32); 9]) {
    for &(id, v) in store {
        values.set_int(id, v);
    }
}

pub fn activate(fe: &mut FrontEnd, env: &mut FeEnv) {
    let originals = ITEMS.iter().map(|it| (it.value, env.values.int(it.value))).collect();
    fe.settings = Settings {
        active: true,
        originals,
        from_pause: env.values.bool(IN_PAUSE_MENU),
        category: env.values.int(CATEGORY),
        changed: false,
        close_feed: false,
        last_category: fe.settings.last_category,
    };
    fe.publish.extend(["dp_settings_init".into(), "dp_settings_menu".into(), "dp_settings_menu_index".into()]);
    for it in &ITEMS {
        fe.publish.push(it.dp.into());
    }
}

pub fn deactivate(fe: &mut FrontEnd) {
    fe.settings.active = false;
}

pub fn action(fe: &mut FrontEnd, env: &mut FeEnv, action: &str, param: &ActionParam) {
    match action {
        "A_GameAdvanceScreen" => {
            let category = param.number as i32;
            env.values.set_int(CATEGORY, category);
            let (screen, feed) = match category {
                1 => (10, "go_to_activity_feed_settings"),
                2 => (screens::id::SETTINGS, "go_to_activity_feed_settings"),
                3 => (9, "go_to_activity_feed_video_calibration"),
                _ => return,
            };
            fe.settings.last_category = Some(category);
            if fe.settings.from_pause {
                fe.go_to_screen(env, screen);
            } else {
                env.nav_events.push(feed.into());
            }
        }
        "A_GamePreviousScreen" => {
            if fe.settings.from_pause {
                fe.go_to_screen(env, screens::id::PAUSE);
            } else {
                env.nav_events.push("go_to_activity_feed_home".into());
            }
        }
        _ => {}
    }
}

fn back_to_menu(fe: &mut FrontEnd, env: &mut FeEnv) {
    if fe.settings.from_pause {
        fe.go_to_screen(env, screens::id::SETTINGS_MENU);
    } else {
        env.nav_events.push("go_to_activity_feed_settings_menu".into());
    }
}

fn commit(fe: &mut FrontEnd, env: &mut FeEnv, gameplay: bool) {
    if gameplay {
        fe.note("settings: message 0xFB0E809D");
    }
    fe.note("settings: save (FE manager +0xC4)");
    fe.requests.push(crate::game::fe::pause::GameRequest::SaveSettings);
    back_to_menu(fe, env);
}

pub fn data_action(fe: &mut FrontEnd, env: &mut FeEnv, dp: &str, action: &str, param: &ActionParam) {
    if dp == "dp_settings_commit" {
        let category = fe.settings.category;
        match action {
            "A_GamePreviousScreen" => {
                if !fe.settings.changed || category == 3 {
                    env.values.set_int(CATEGORY, 0);
                    back_to_menu(fe, env);
                } else {
                    fe.settings.close_feed = param.number != 0.0 && !param.number.is_nan();
                    fe.note("settings: FE manager +0x88(7)");
                    env.nav_events.push("go_to_exit_settings_without_save_popup".into());
                }
            }
            "A_GameAdvanceScreen" => {
                if category == 1 && env.values.int(0x1F1) == 1 {
                    env.nav_events.push("go_to_reset_tutorial_messages_popup".into());
                } else {
                    env.values.set_int(CATEGORY, 0);
                    match category {
                        1 => commit(fe, env, true),
                        3 => back_to_menu(fe, env),
                        _ => commit(fe, env, false),
                    }
                }
            }
            "A_SettingsResetToDefault" => match category {
                1 => env.nav_events.push("go_to_reset_gameplay_settings_popup".into()),
                2 => env.nav_events.push("go_to_reset_audio_settings_popup".into()),
                _ => {}
            },
            _ => {}
        }
        return;
    }
    let next = match action {
        "A_TogglePreviousItem" => false,
        "A_ToggleNextItem" => true,
        _ => return,
    };
    let Some(it) = ITEMS.iter().find(|it| it.dp == dp) else { return };
    fe.settings.changed = true;
    let v = env.values.int(it.value);
    let stepped = if next {
        if v < it.max {
            Some(v + 1)
        } else if it.wrap {
            Some(0)
        } else {
            None
        }
    } else if v > 0 {
        Some(v - 1)
    } else if it.wrap {
        Some(it.max)
    } else {
        None
    };
    match stepped {
        Some(n) => {
            env.values.set_int(it.value, n);
            fe.note(format!("settings: {} = {n}", it.dp));
            fe.play_sound(7);
        }
        None => fe.play_sound(8),
    }
    fe.publish.push(it.dp.into());
}

pub fn nav_action(fe: &mut FrontEnd, env: &mut FeEnv, name: &str) -> bool {
    match name {
        "do_exit_settings_without_save" => {
            let keep: &[u16] = match fe.settings.category {
                1 => &[0x1F0, 0x1F1],
                2 => &[0x28, 0x23, 0x26, 0x21, 0x2A, 0x24, 0x29, 0x27, 0x22],
                _ => &[],
            };
            for (id, v) in fe.settings.originals.clone() {
                if keep.contains(&id) {
                    env.values.set_int(id, v);
                }
            }
            env.values.set_int(CATEGORY, 0);
            if fe.settings.close_feed {
                env.nav_events.push("close_activity_feed".into());
            } else {
                back_to_menu(fe, env);
            }
        }
        "do_reset_gameplay_settings" => {
            env.values.set_int(0x1F0, 0);
            fe.publish.push("dp_settings_controller_scheme".into());
            if !fe.settings.from_pause {
                env.values.set_int(0x1F1, 0);
                fe.publish.push("dp_settings_reset_tutorial_messages".into());
            }
        }
        "do_reset_audio_settings" => {
            env.values.set_int(AUDIO_RESET_FLAG, 1);
            fe.audio_store = fe.audio_defaults;
            init_values(env.values, &fe.audio_defaults);
            env.values.set_int(AUDIO_RESET_FLAG, 0);
            for it in &ITEMS[..8] {
                fe.publish.push(it.dp.into());
            }
            fe.publish.push(ITEMS[10].dp.into());
        }
        "do_reset_tutorial_messages" => {
            fe.note("settings: message 0x270D5F4C (the hints' seen stats cleared, 0x824D2CC8)");
            crate::game::fe::hint::reset(fe);
            env.values.set_int(0x1F1, 0);
            commit(fe, env, true);
            env.values.set_int(CATEGORY, 0);
        }
        _ => return false,
    }
    true
}

pub fn scheme_unlocked(fe: &FrontEnd) -> bool {
    fe.profile.drop_state(crate::game::fe::menu::GROUND_TUTORIAL) == ssx_assets::career::profile::DropState::Completed
}

pub fn build(fe: &FrontEnd, cx: &mut Ctx, values: &UiValues, loc: &mut Localization, dp: &str) -> Option<Value> {
    match dp {
        "dp_settings_menu" => {
            let mut out = Vec::new();
            for (k, key) in ["TXT_GAMEPLAY_SETTINGS", "TXT_AUDIO_SETTINGS", "TXT_VIDEO_CALIBRATION"].iter().enumerate()
            {
                let o = Value::Object(cx.new_object());
                let text = loc.text(key);
                let _ = cx.set_member(&o, "strItem", Value::str(&text));
                let _ = cx.set_member(&o, "bEnabled", Value::Bool(true));
                let _ = cx.set_member(&o, "bFocusable", Value::Bool(true));
                let _ = cx.set_member(&o, "eventAdvance", Value::str("A_GameAdvanceScreen"));
                let _ = cx.set_member(&o, "eventAdvanceParam", Value::Number(k as f64 + 1.0));
                let _ = cx.set_member(&o, "eventBack", Value::str("A_GamePreviousScreen"));
                out.push(o);
            }
            Some(Value::Object(cx.new_array(out)))
        }
        "dp_settings_menu_index" => {
            let index = match fe.menu_focus {
                MenuFocus::FirstItem => 0,
                // TODO(guess): a port choice — the option "Menus: the last item picked" focuses the page last opened from the menu (its items 0 … 2 are the pages 1 … 3); settle: none (the game's publisher 0x825C3580 always publishes 0).
                MenuFocus::LastItem => fe.settings.last_category.map_or(0, |c| c - 1),
            };
            Some(Value::Number(f64::from(index)))
        }
        "dp_settings_init" => {
            let o = Value::Object(cx.new_object());
            let done = loc.text("TXT_SETTINGS_SAVE");
            let _ = cx.set_member(&o, "doneButtonLabel", Value::str(&done));
            let _ = cx.set_member(&o, "isFromPauseMenu", Value::Bool(fe.settings.from_pause));
            let mut items: Vec<&str> = Vec::new();
            match fe.settings.category {
                1 => {
                    items.push("settings_controller_scheme");
                    if !fe.settings.from_pause {
                        items.push("settings_reset_tutorial_messages");
                    }
                    let _ = cx.set_member(&o, "showDescription", Value::Bool(true));
                    let key = if scheme_unlocked(fe) {
                        "TXT_SETTINGS_CONTROL_SET_NOTICE"
                    } else {
                        "TXT_SETTINGS_CONTROL_SET_UNLOCK_DESC"
                    };
                    let d = loc.text(key);
                    let _ = cx.set_member(&o, "descriptionText", Value::str(&d));
                }
                2 => {
                    items.extend(AUDIO_PAGE);
                    let _ = cx.set_member(&o, "showDescription", Value::Bool(false));
                }
                _ => {}
            }
            let arr: Vec<Value> = items.iter().map(|s| Value::str(s)).collect();
            let arr = Value::Object(cx.new_array(arr));
            let _ = cx.set_member(&o, "settingsItems", arr);
            Some(o)
        }
        _ => {
            let it = ITEMS.iter().find(|it| it.dp == dp)?;
            let v = values.int(it.value);
            let label = if it.labels.is_empty() {
                v.to_string()
            } else {
                loc.text(it.labels.get(v.clamp(0, it.max) as usize).copied().unwrap_or(""))
            };
            let o = Value::Object(cx.new_object());
            let _ = cx.set_member(&o, "label", Value::str(&label));
            let enabled = it.value != 0x1F0 || scheme_unlocked(fe);
            let _ = cx.set_member(&o, "bEnabled", Value::Bool(enabled));
            let title = loc.text(it.title);
            let _ = cx.set_member(&o, "title", Value::str(&title));
            if it.value == 0x1F0 {
                let _ = cx.set_member(&o, "index", Value::Number(f64::from(v)));
            }
            Some(o)
        }
    }
}
