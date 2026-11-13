use std::path::PathBuf;

use serde_json::Value as Json;
use ssx_assets::display::DisplaySettings;

#[cfg(feature = "net")]
use super::keyboard::{self, Outcome, Requester, TextRequest};
use super::port_settings::{
    Item, Page, PortSettings, SaveError, WindowChoice, read_file, replace_unreadable, save_file,
};
use crate::avm1::Ctx;
use crate::avm1::value::Value;
use crate::game::fe::{ActionParam, FeEnv, FrontEnd, settings};
use crate::game::loc::Localization;

pub const DP_MENU: &str = "dp_ava_options_menu";
pub const DP_MENU_INDEX: &str = "dp_ava_options_menu_index";
pub const DP_INIT: &str = "dp_ava_options_init";
pub const DP_COMMIT: &str = "dp_ava_options_commit";
pub const OPEN: &str = "go_to_ava_options";
#[cfg(feature = "net")]
pub const PROVIDER: &str = "avaoptions";
pub const BROKEN_POPUP: &str = "ava_settings_broken";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Category {
    Game(i32),
    Port(Page),
    #[cfg(feature = "net")]
    PlayerName,
}

const BASE_CATEGORIES: [(Category, &str); 8] = [
    (Category::Game(1), "TXT_GAMEPLAY_SETTINGS"),
    (Category::Game(2), "TXT_AUDIO_SETTINGS"),
    (Category::Game(3), "TXT_VIDEO_CALIBRATION"),
    (Category::Port(Page::Display), "TXT_AVA_OPTIONS_DISPLAY"),
    (Category::Port(Page::Effects), "TXT_AVA_OPTIONS_EFFECTS"),
    (Category::Port(Page::Sound), "TXT_AVA_OPTIONS_SOUND"),
    (Category::Port(Page::Movies), "TXT_AVA_OPTIONS_MOVIES"),
    (Category::Port(Page::Menus), "TXT_AVA_OPTIONS_MENUS"),
];

#[cfg(not(feature = "net"))]
pub const CATEGORIES: &[(Category, &str)] = &BASE_CATEGORIES;
#[cfg(feature = "net")]
pub const CATEGORIES: &[(Category, &str)] = &{
    let mut all = [(Category::PlayerName, "TXT_AVA_OPTIONS_PLAYER_NAME"); BASE_CATEGORIES.len() + 2];
    let mut k = 0;
    while k < BASE_CATEGORIES.len() {
        all[k] = BASE_CATEGORIES[k];
        k += 1;
    }
    all[BASE_CATEGORIES.len() + 1] = (Category::Port(Page::Multiplayer), "TXT_AVA_OPTIONS_MULTIPLAYER");
    all
};

#[derive(Clone, Debug, Default)]
pub struct Options {
    pub file: Option<PathBuf>,
    pub settings: PortSettings,
    pub kept: PortSettings,
    pub host_window: Option<WindowChoice>,
    pub window_request: Option<DisplaySettings>,
    pub originals: PortSettings,
    pub page: Option<Page>,
    pub changed: bool,
    pub last: Option<usize>,
}

pub fn open(fe: &mut FrontEnd, env: &mut FeEnv) {
    reload(fe);
    env.nav_events.push(OPEN.into());
}

pub fn reload(fe: &mut FrontEnd) {
    let Some(path) = fe.ava.options.file.clone() else {
        fe.note("options: no settings file named; the launcher's defaults shown");
        fe.ava.options.settings = PortSettings::default();
        fe.ava.options.kept = PortSettings::default();
        take_window(fe);
        return;
    };
    match read_file(&path) {
        Ok(m) => {
            let (s, problems) = PortSettings::from_json(&Json::Object(m));
            for p in problems {
                fe.note(format!("options: {p}"));
            }
            fe.ava.options.kept = s.clone();
            fe.ava.options.settings = s;
        }
        Err(e) => {
            fe.note(format!("options: ERROR {e:#}; the launcher's defaults shown"));
            fe.ava.options.settings = PortSettings::default();
            fe.ava.options.kept = PortSettings::default();
        }
    }
    take_window(fe);
}

fn take_window(fe: &mut FrontEnd) {
    let o = &mut fe.ava.options;
    o.settings.window = o.host_window.clone();
    o.kept.window = o.host_window.clone();
}

pub fn activate(fe: &mut FrontEnd, view_model: &str) {
    if view_model == "avaoptionspage" {
        take_window(fe);
        fe.ava.options.originals = fe.ava.options.settings.clone();
        fe.ava.options.changed = false;
        fe.publish.extend([DP_INIT.into(), DP_COMMIT.into()]);
        let page = fe.ava.options.page.map_or(&[][..], Page::items);
        fe.publish.extend(page.iter().map(|i| format!("dp_{}", i.name())));
    } else {
        fe.ava.options.page = None;
        fe.publish.extend([DP_MENU.into(), DP_MENU_INDEX.into()]);
    }
}

pub fn action(fe: &mut FrontEnd, env: &mut FeEnv, action: &str, param: &ActionParam) {
    match action {
        "A_AvaOptionsCategory" => {
            let k = param.number;
            let Some(&(cat, _)) = (k.is_finite() && k >= 0.0).then(|| CATEGORIES.get(k as usize)).flatten() else {
                fe.note(format!("options: no category {k}"));
                return;
            };
            fe.ava.options.last = Some(k as usize);
            match cat {
                Category::Game(c) => {
                    env.values.set_int(settings::CATEGORY, c);
                    env.values.set_bool(settings::IN_PAUSE_MENU, false);
                    let event = if c == 3 { "go_to_ava_options_calibration" } else { "go_to_ava_options_game" };
                    env.nav_events.push(event.into());
                }
                Category::Port(p) => {
                    fe.ava.options.page = Some(p);
                    env.nav_events.push("go_to_ava_options_page".into());
                }
                #[cfg(feature = "net")]
                Category::PlayerName => {
                    let initial = fe.ava.options.settings.player_name.clone().unwrap_or_default();
                    let req = TextRequest {
                        requester: Requester::Provider(PROVIDER),
                        title: "TXT_AVA_PLAYER_NAME_TITLE".into(),
                        max_len: ssx_net_proto::name::MAX_PLAYER_NAME_CHARS,
                        password: false,
                        initial,
                        page: keyboard::Page::Letters,
                    };
                    if let Err(e) = keyboard::request(fe, env, req) {
                        fe.note(format!("options: the keyboard: {e}"));
                    }
                }
            }
        }
        "A_AvaOptionsBack" => env.nav_events.push("ava_options_close".into()),
        _ => {}
    }
}

pub fn data_action(fe: &mut FrontEnd, env: &mut FeEnv, dp: &str, action: &str, _param: &ActionParam) {
    if dp == DP_COMMIT {
        match action {
            "A_GameAdvanceScreen" => {
                if fe.ava.options.changed {
                    save(fe, env, BROKEN_POPUP);
                }
                env.nav_events.push("ava_options_menu".into());
            }
            "A_GamePreviousScreen" if fe.ava.options.changed => {
                env.nav_events.push("go_to_exit_settings_without_save_popup".into());
            }
            "A_GamePreviousScreen" => env.nav_events.push("ava_options_menu".into()),
            "A_SettingsResetToDefault" => env.nav_events.push("ava_options_reset".into()),
            _ => {}
        }
        return;
    }
    let next = match action {
        "A_TogglePreviousItem" => false,
        "A_ToggleNextItem" => true,
        _ => return,
    };
    let page = fe.ava.options.page.map_or(&[][..], Page::items);
    let Some(item) = Item::of_dp(dp).filter(|i| page.contains(i)) else { return };
    if !item.enabled(&fe.ava.options.settings) {
        fe.play_sound(8);
        return;
    }
    if item.step(&mut fe.ava.options.settings, next) {
        fe.ava.options.changed = true;
        fe.note(format!("options: {} = {}", item.name(), item.label(&fe.ava.options.settings, env.loc)));
        fe.play_sound(7);
    } else {
        fe.play_sound(8);
    }
    fe.publish.extend(page.iter().map(|i| format!("dp_{}", i.name())));
}

pub fn nav_action(fe: &mut FrontEnd, env: &mut FeEnv, name: &str) -> bool {
    let page = fe.ava.options.page.map_or(&[][..], Page::items);
    match name {
        "do_ava_options_discard" => {
            fe.ava.options.settings = fe.ava.options.originals.clone();
            fe.ava.options.changed = false;
            env.nav_events.push("ava_options_menu".into());
        }
        "do_ava_settings_replace" => match fe.ava.options.file.clone() {
            Some(path) => match replace_unreadable(&path, &fe.ava.options.settings) {
                Ok(aside) => {
                    fe.ava.options.kept = fe.ava.options.settings.clone();
                    fe.note(format!("options: the old file kept as {}; saved to {}", aside.display(), path.display()))
                }
                Err(e) => fe.note(format!("options: ERROR {e:#}")),
            },
            None => fe.note("options: ERROR no settings file named"),
        },
        "do_ava_settings_keep" => fe.note("options: the unreadable file is left as it is; the changes are not saved"),
        "do_ava_options_reset" => {
            let defaults = PortSettings::default();
            let mut s = fe.ava.options.settings.clone();
            for &i in page {
                i.reset(&mut s, &defaults);
            }
            if s != fe.ava.options.settings {
                fe.ava.options.settings = s;
                fe.ava.options.changed = true;
            }
            fe.publish.extend(page.iter().map(|i| format!("dp_{}", i.name())));
        }
        _ => return false,
    }
    true
}

pub(super) fn save(fe: &mut FrontEnd, env: &mut FeEnv, broken_popup: &str) {
    if fe.ava.options.settings.range_movies != fe.ava.options.kept.range_movies {
        fe.range_movies = fe.ava.options.settings.range_movies;
        fe.note(format!("options: range movies {} from now on", fe.range_movies));
    }
    if fe.ava.options.settings.menu_focus != fe.ava.options.kept.menu_focus {
        fe.menu_focus = fe.ava.options.settings.menu_focus;
        fe.note(format!("options: menu focus {} from now on", fe.menu_focus));
    }
    let o = &mut fe.ava.options;
    if o.settings.window != o.kept.window
        && let Some(w) = o.settings.window.clone()
    {
        o.window_request = Some(w.settings);
        o.kept.window = o.settings.window.clone();
        let line = format!(
            "options: the window's mode {} asked of the host",
            o.kept.window.as_ref().map_or("", |w| w.settings.mode.name())
        );
        fe.note(line);
    }
    let Some(path) = fe.ava.options.file.clone() else {
        fe.note("options: ERROR no settings file named: the changes are not saved");
        return;
    };
    match save_file(&path, &fe.ava.options.kept, &fe.ava.options.settings) {
        Ok(()) => {
            fe.ava.options.kept = fe.ava.options.settings.clone();
            fe.note(format!("options: saved to {}", path.display()))
        }
        Err(SaveError::Unreadable(why)) => {
            fe.note(format!("options: {} could not be read ({why}): asking before it is replaced", path.display()));
            let body = env.loc.format("TXT_AVA_SETTINGS_BROKEN_BODY", &[&path.display().to_string(), &why]);
            let glyphs = fe.ava.glyphs.clone();
            env.values.set_str(crate::game::fe::popup::POPUP_BODY, super::shown(&body, glyphs.as_ref()));
            env.nav_events.push(broken_popup.into());
        }
        Err(SaveError::Io(e)) => fe.note(format!("options: ERROR {e:#}")),
    }
}

#[cfg(feature = "net")]
pub fn update(fe: &mut FrontEnd, env: &mut FeEnv) {
    let Some(Outcome::Done(text)) = keyboard::take_result(fe, PROVIDER) else { return };
    match ssx_net_proto::name::player_name(&text) {
        Ok(name) => {
            fe.note(format!("options: player name {name:?}"));
            super::name_prompt::confirm(fe, name);
            save(fe, env, BROKEN_POPUP);
            fe.publish.push(DP_MENU.into());
        }
        Err(e) => {
            fe.note(format!("options: player name {text:?} refused: {e}"));
            let body = env.loc.format("TXT_AVA_PLAYER_NAME_REFUSED", &[&e.to_string()]);
            let glyphs = fe.ava.glyphs.clone();
            env.values.set_str(crate::game::fe::popup::POPUP_BODY, super::shown(&body, glyphs.as_ref()));
            env.nav_events.push("ava_options_message".into());
        }
    }
}

pub fn build(fe: &FrontEnd, cx: &mut Ctx, loc: &mut Localization, dp: &str) -> Option<Value> {
    let o = &fe.ava.options;
    match dp {
        DP_MENU => {
            let mut out = Vec::new();
            for (k, (cat, key)) in CATEGORIES.iter().enumerate() {
                let it = Value::Object(cx.new_object());
                let text = match cat {
                    #[cfg(feature = "net")]
                    Category::PlayerName => {
                        let name = o
                            .settings
                            .player_name
                            .clone()
                            .unwrap_or_else(|| super::port_settings::DEFAULT_PLAYER_NAME.into());
                        loc.format(key, &[&name])
                    }
                    _ => loc.text(key),
                };
                let _ = cx.set_member(&it, "strItem", Value::str(&text));
                let _ = cx.set_member(&it, "bEnabled", Value::Bool(true));
                let _ = cx.set_member(&it, "bFocusable", Value::Bool(true));
                let _ = cx.set_member(&it, "eventAdvance", Value::str("A_AvaOptionsCategory"));
                let _ = cx.set_member(&it, "eventAdvanceParam", Value::Number(k as f64));
                let _ = cx.set_member(&it, "eventBack", Value::str("A_AvaOptionsBack"));
                out.push(it);
            }
            Some(Value::Object(cx.new_array(out)))
        }
        // TODO(guess): a port choice — the port's menu follows the game's menus' option "Menus" (the first row, or the row last picked); settle: none (the original has no such menu).
        DP_MENU_INDEX => {
            let row = match fe.menu_focus {
                ssx_assets::port_options::MenuFocus::FirstItem => None,
                ssx_assets::port_options::MenuFocus::LastItem => o.last.filter(|&k| k < CATEGORIES.len()),
            };
            Some(Value::Number(row.unwrap_or(0) as f64))
        }
        DP_INIT => {
            let v = Value::Object(cx.new_object());
            let _ = cx.set_member(&v, "doneButtonLabel", Value::str(&loc.text("TXT_SETTINGS_SAVE")));
            let _ = cx.set_member(&v, "isFromPauseMenu", Value::Bool(false));
            let names: Vec<Value> = o.page.map_or(&[][..], Page::items).iter().map(|i| Value::str(i.name())).collect();
            let names = Value::Object(cx.new_array(names));
            let _ = cx.set_member(&v, "settingsItems", names);
            let _ = cx.set_member(&v, "showDescription", Value::Bool(true));
            let note = match o.page {
                Some(Page::Display) => "TXT_AVA_OPTIONS_DISPLAY_NOTE",
                Some(Page::Movies) => "TXT_AVA_OPTIONS_MOVIES_NOTE",
                Some(Page::Menus) => "TXT_AVA_OPTIONS_MENUS_NOTE",
                #[cfg(feature = "net")]
                Some(Page::Multiplayer) => "TXT_AVA_OPTIONS_NEXT_JOIN",
                _ => "TXT_AVA_OPTIONS_NEXT_START",
            };
            let _ = cx.set_member(&v, "descriptionText", Value::str(&loc.text(note)));
            Some(v)
        }
        DP_COMMIT => Some(Value::Object(cx.new_object())),
        _ => {
            let item = Item::of_dp(dp)?;
            let v = Value::Object(cx.new_object());
            let _ = cx.set_member(&v, "label", Value::str(&item.label(&o.settings, loc)));
            let _ = cx.set_member(&v, "bEnabled", Value::Bool(item.enabled(&o.settings)));
            let _ = cx.set_member(&v, "title", Value::str(&loc.text(item.title())));
            Some(v)
        }
    }
}
