use std::collections::VecDeque;

use ssx_net_proto::chat::{MAX_CHAT_CHARS, QUICK_LINES, chat_text};

use super::{Body, ChatIn, ChatOut, Sender};
use crate::avm1::Ctx;
use crate::avm1::value::Value;
use crate::game::fe::ava::keyboard::{self, Outcome, Requester, TextRequest};
use crate::game::fe::{ActionParam, FeEnv, FrontEnd};
use crate::game::loc::Localization;

pub const DP: &str = "dp_ava_chat";
pub const DP_QUICK: &str = "dp_ava_quickchat";
pub const DP_QUICK_ITEMS: &str = "dp_ava_quickchat_items";
pub const DP_QUICK_INDEX: &str = "dp_ava_quickchat_items_index";
pub const PROVIDER: &str = "avachat";
pub const ENABLE: &str = "ava_chat_enable";
pub const DISABLE: &str = "ava_chat_disable";
pub const QUICK_OPEN: &str = "ava_quickchat_open";
pub const QUICK_CLOSE: &str = "ava_quickchat_close";

// PORT(choice): the chat layer is the game's event-timer layer (`LAYER_EVENT_TIMER`), which only the online live
// events' timer view `eventtimer` uses (the nav enables it on the Global Events drop screen and disables it at the quit
// stage; a session's level never has it); a layer holds one view (`ux.view.Layer.ChangeView` loads a second one in
// place of the first, at the layer's base depth), and `LAYER_HUD` holds the game's `hud` view — measured: on it the
// chat's rows were published and never drawn in a level; a layer of the port's own would replace it if the framework's
// layer manager is ever extended, or if Global Events in a session (phase 10) need the timer; the original has no chat.
pub const LAYER: &str = "LAYER_EVENT_TIMER";

// PORT(choice): six rows of the newest lines at the HUD's left (FiveM's chat shows about ten lines, MTA:SA's a
// configurable box), small enough to leave the ride's view: a box of 470 × 212 px at (36, 382) of the 1280 × 720 stage,
// below hud.gfx's time panel and notification (96, 54 / 61), above the ready loop's "(Y) Change Equipment" prompt
// (measured at 636–664 px) and left of the score bar (from 472 px, 576 px down); tuning: what players of phase 4's
// public servers ask for; the game has no chat.
pub const ROWS: usize = 6;
// PORT(choice): 10 s (FiveM's and MTA:SA's chat boxes fade their lines after a few seconds); tuning: as [`ROWS`]; the
// game has no chat.
pub const LINE_SECONDS: f64 = 10.0;
pub const KEPT: usize = 32;
// PORT(choice): three names of at most 32 characters keep the notice within about two of the layer's rows (a package
// name has at most 64); tuning: what players of phase 6's servers ask for; the game has no chat.
pub const MODS_NAMED: usize = 3;
pub const MOD_NAME_CHARS: usize = 32;

pub const QUICK_KEYS: [&str; QUICK_LINES as usize] = [
    "TXT_AVA_QUICK_1",
    "TXT_AVA_QUICK_2",
    "TXT_AVA_QUICK_3",
    "TXT_AVA_QUICK_4",
    "TXT_AVA_QUICK_5",
    "TXT_AVA_QUICK_6",
    "TXT_AVA_QUICK_7",
    "TXT_AVA_QUICK_8",
];

#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    pub name: String,
    pub text: String,
    pub from: Option<[u8; 8]>,
    pub at: f64,
}

#[derive(Default)]
pub struct Chat {
    pub active: bool,
    pub up: bool,
    pub lines: VecDeque<Line>,
    pub enabled: Option<bool>,
    pub hold_until: f64,
    pub out: Vec<ChatOut>,
    pub asked: bool,
    pub dirty: bool,
    shown: Vec<(String, String)>,
}

#[derive(Default)]
pub struct Quick {
    pub active: bool,
}

impl Chat {
    fn keep(&mut self, line: Line) {
        if self.lines.len() == KEPT {
            self.lines.pop_front();
        }
        self.lines.push_back(line);
        self.dirty = true;
    }

    fn notice(&mut self, text: String, at: f64) {
        self.keep(Line { name: String::new(), text, from: None, at });
    }
}

fn shown(fe: &FrontEnd, text: &str, max: usize) -> String {
    crate::game::fe::ava::bounded(text, max, fe.ava.glyphs.as_ref())
}

pub fn receive(fe: &mut FrontEnd, loc: &mut Localization, items: Vec<ChatIn>) {
    let now = fe.time;
    for item in items {
        match item {
            ChatIn::Line { from, body } => {
                let text = match body {
                    Body::Text(t) => shown(fe, &t, MAX_CHAT_CHARS),
                    Body::Quick(i) => QUICK_KEYS.get(usize::from(i)).map(|k| loc.text(k)).unwrap_or_default(),
                };
                let (name, key) = match from {
                    Sender::Player { name, key_id } => {
                        (shown(fe, &name, ssx_net_proto::name::MAX_PLAYER_NAME_CHARS), Some(key_id))
                    }
                    Sender::Server => (loc.text("TXT_AVA_CHAT_SERVER"), None),
                };
                fe.ava.session.chat.keep(Line { name, text, from: key, at: now });
            }
            ChatIn::Flood { wait_ms } => {
                let wait = f64::from(wait_ms) / 1000.0;
                fe.ava.session.chat.hold_until = fe.ava.session.chat.hold_until.max(now + wait);
                let text = loc.format("TXT_AVA_CHAT_FLOOD", &[&format!("{:.0}", wait.ceil().max(1.0))]);
                fe.ava.session.chat.notice(text, now);
            }
            ChatIn::Disabled => {
                fe.ava.session.chat.enabled = Some(false);
                let text = loc.text("TXT_AVA_CHAT_OFF");
                fe.ava.session.chat.notice(text, now);
            }
            ChatIn::ModsOff { names, whole } => {
                let more = names.len().saturating_sub(MODS_NAMED);
                let more = (more > 0).then(|| loc.format("TXT_AVA_CHAT_MODS_MORE", &[&more.to_string()]));
                let listed = mods_named(&names, |n| shown(fe, n, MOD_NAME_CHARS), more);
                let key = if whole { "TXT_AVA_CHAT_MODS_OFF" } else { "TXT_AVA_CHAT_MODS_SIM_OFF" };
                let text = loc.format(key, &[&listed]);
                fe.ava.session.chat.notice(text, now);
            }
            ChatIn::Muted { until, reason } => {
                let mut text = match until {
                    Some(until) => loc.format("TXT_AVA_CHAT_MUTED", &[&shown(fe, &until, 32)]),
                    None => loc.text("TXT_AVA_CHAT_MUTED_FOREVER"),
                };
                if !reason.is_empty() {
                    text = format!("{text}: {}", shown(fe, &reason, MAX_CHAT_CHARS));
                }
                fe.ava.session.chat.notice(text, now);
            }
        }
    }
}

fn mods_named(names: &[String], shown: impl Fn(&str) -> String, more: Option<String>) -> String {
    let listed: Vec<String> = names.iter().take(MODS_NAMED).map(|n| shown(n)).chain(more).collect();
    listed.join(", ")
}

fn refusal(fe: &FrontEnd, loc: &mut Localization) -> Option<String> {
    let chat = &fe.ava.session.chat;
    if chat.enabled == Some(false) {
        return Some(loc.text("TXT_AVA_CHAT_OFF"));
    }
    let wait = chat.hold_until - fe.time;
    (wait > 0.0).then(|| loc.format("TXT_AVA_CHAT_FLOOD", &[&format!("{:.0}", wait.ceil().max(1.0))]))
}

fn send_text(fe: &mut FrontEnd, loc: &mut Localization, raw: &str) {
    let text = raw.trim_matches(' ');
    if text.is_empty() {
        return;
    }
    if let Some(why) = refusal(fe, loc) {
        let now = fe.time;
        fe.ava.session.chat.notice(why, now);
        return;
    }
    match chat_text(text) {
        Ok(t) => {
            fe.note(format!("chat: a line of {} characters sent", t.chars().count()));
            fe.ava.session.chat.out.push(ChatOut::Text(t.to_string()));
        }
        Err(e) => {
            fe.note(format!("chat: the line is not sent: {e}"));
            let now = fe.time;
            let text = loc.text("TXT_AVA_CHAT_UNSENDABLE");
            fe.ava.session.chat.notice(text, now);
        }
    }
}

pub fn open_text(fe: &mut FrontEnd, env: &mut FeEnv) -> bool {
    if !fe.ava.session.active || !fe.ava.session.chat.up || fe.ava.session.chat.asked {
        return false;
    }
    if let Some(why) = refusal(fe, env.loc) {
        let now = fe.time;
        fe.ava.session.chat.notice(why, now);
        return false;
    }
    let req = TextRequest {
        requester: Requester::Provider(PROVIDER),
        title: "TXT_AVA_CHAT_TITLE".into(),
        max_len: MAX_CHAT_CHARS,
        ..TextRequest::default()
    };
    match keyboard::request(fe, env, req) {
        Ok(()) => {
            fe.ava.session.chat.asked = true;
            fe.ava.session.chat.dirty = true;
            true
        }
        Err(e) => {
            fe.note(format!("chat: the keyboard is not free: {e}"));
            false
        }
    }
}

pub fn open_quick(fe: &mut FrontEnd, env: &mut FeEnv) -> bool {
    let s = &fe.ava.session;
    if !s.active || !s.chat.up || s.chat.asked || s.quick.active || fe.ava.keyboard.active {
        return false;
    }
    if let Some(why) = refusal(fe, env.loc) {
        let now = fe.time;
        fe.ava.session.chat.notice(why, now);
        return false;
    }
    env.nav_events.push(QUICK_OPEN.into());
    true
}

pub fn close_entries(fe: &mut FrontEnd, env: &mut FeEnv) {
    if std::mem::take(&mut fe.ava.session.chat.asked) && keyboard::withdraw(fe, env, PROVIDER) {
        fe.note("chat: the entry is withdrawn (the layer goes)");
    }
    if fe.ava.session.quick.active {
        env.nav_events.push(QUICK_CLOSE.into());
        fe.note("chat: the quick chat closes (the layer goes)");
    }
}

pub fn held(fe: &FrontEnd) -> bool {
    let s = &fe.ava.session;
    s.active && (s.quick.active || (s.chat.asked && fe.ava.keyboard.active))
}

pub fn update(fe: &mut FrontEnd, env: &mut FeEnv) {
    if !fe.ava.session.active {
        return;
    }
    if fe.ava.session.chat.asked
        && let Some(outcome) = keyboard::take_result(fe, PROVIDER)
    {
        fe.ava.session.chat.asked = false;
        fe.ava.session.chat.dirty = true;
        if let Outcome::Done(text) = outcome {
            send_text(fe, env.loc, &text);
        }
    }
    let rows = rows(fe);
    let banner = fe.ava.session.banner != fe.ava.session.banner_shown;
    if banner {
        fe.ava.session.banner_shown.clone_from(&fe.ava.session.banner);
    }
    if fe.ava.session.chat.active
        && (std::mem::take(&mut fe.ava.session.chat.dirty) || rows != fe.ava.session.chat.shown || banner)
    {
        if rows.len() != fe.ava.session.chat.shown.len() {
            fe.note(format!("chat: {} rows shown", rows.len()));
        }
        fe.ava.session.chat.shown = rows;
        fe.publish.push(DP.into());
    }
}

fn rows(fe: &FrontEnd) -> Vec<(String, String)> {
    let s = &fe.ava.session;
    visible_rows(&s.chat.lines, &s.muted, s.quick.active || s.chat.asked, fe.time)
}

pub fn visible_rows(
    lines: &VecDeque<Line>,
    muted: &std::collections::BTreeSet<[u8; 8]>,
    open: bool,
    now: f64,
) -> Vec<(String, String)> {
    let mut rows: Vec<(String, String)> = lines
        .iter()
        .rev()
        .filter(|l| l.from.is_none_or(|k| !muted.contains(&k)))
        .take(ROWS)
        .filter(|l| open || now - l.at < LINE_SECONDS)
        .map(|l| (l.name.clone(), l.text.clone()))
        .collect();
    rows.reverse();
    rows
}

pub fn activate(fe: &mut FrontEnd) {
    fe.ava.session.chat.active = true;
    fe.ava.session.chat.dirty = true;
    fe.note(format!("chat: the layer's view is up ({} lines kept)", fe.ava.session.chat.lines.len()));
}

pub fn deactivate(fe: &mut FrontEnd) {
    fe.ava.session.chat.active = false;
}

pub fn quick_activate(fe: &mut FrontEnd) {
    fe.ava.session.quick.active = true;
    fe.ava.session.chat.dirty = true;
    fe.publish.extend([DP_QUICK.into(), DP_QUICK_ITEMS.into(), DP_QUICK_INDEX.into()]);
}

pub fn quick_deactivate(fe: &mut FrontEnd) {
    fe.ava.session.quick.active = false;
    fe.ava.session.chat.dirty = true;
}

pub fn quick_action(fe: &mut FrontEnd, env: &mut FeEnv, action: &str, param: &ActionParam) {
    match action {
        "A_AvaQuickChat" => {
            let n = param.number;
            env.nav_events.push(QUICK_CLOSE.into());
            if !(n.is_finite() && (0.0..=f64::from(QUICK_LINES)).contains(&n)) {
                return;
            }
            let n = n as u8;
            if n == 0 {
                fe.ava.session.quick.active = false;
                open_text(fe, env);
            } else if let Some(why) = refusal(fe, env.loc) {
                let now = fe.time;
                fe.ava.session.chat.notice(why, now);
            } else {
                fe.note(format!("chat: quick line {} sent", n - 1));
                fe.ava.session.chat.out.push(ChatOut::Quick(n - 1));
            }
        }
        "A_AvaQuickChatClose" => env.nav_events.push(QUICK_CLOSE.into()),
        _ => {}
    }
}

pub fn build(fe: &FrontEnd, cx: &mut Ctx, loc: &mut Localization, dp: &str) -> Option<Value> {
    match dp {
        DP => {
            let rows = &fe.ava.session.chat.shown;
            let o = Value::Object(cx.new_object());
            for k in 0..ROWS {
                let at = (k + rows.len()).checked_sub(ROWS).and_then(|i| rows.get(i));
                let (name, text) = at.map_or((String::new(), String::new()), |(n, t)| (n.clone(), t.clone()));
                let name = if name.is_empty() || text.is_empty() { name } else { format!("{name}:") };
                let _ = cx.set_member(&o, &format!("name{k}"), Value::str(&name));
                let _ = cx.set_member(&o, &format!("text{k}"), Value::str(&text));
            }
            let _ = cx.set_member(&o, "bShown", Value::Bool(!rows.is_empty()));
            let banner = &fe.ava.session.banner_shown;
            let _ = cx.set_member(&o, "banner", Value::str(banner));
            let _ = cx.set_member(&o, "bBanner", Value::Bool(!banner.is_empty()));
            Some(o)
        }
        DP_QUICK => {
            let o = Value::Object(cx.new_object());
            let _ = cx.set_member(&o, "title", Value::str(&loc.text("TXT_AVA_QUICK_TITLE")));
            Some(o)
        }
        DP_QUICK_ITEMS => {
            let mut items = Vec::new();
            let texts = std::iter::once(loc.text("TXT_AVA_QUICK_TYPE")).chain(QUICK_KEYS.iter().map(|k| loc.text(k)));
            for (i, text) in texts.enumerate() {
                let item = Value::Object(cx.new_object());
                let _ = cx.set_member(&item, "strItem", Value::str(&text));
                let _ = cx.set_member(&item, "bEnabled", Value::Bool(true));
                let _ = cx.set_member(&item, "eventAdvance", Value::str("A_AvaQuickChat"));
                let _ = cx.set_member(&item, "eventAdvanceParam", Value::Number(i as f64));
                items.push(item);
            }
            Some(Value::Object(cx.new_array(items)))
        }
        DP_QUICK_INDEX => Some(Value::Number(0.0)),
        _ => None,
    }
}

#[cfg(test)]
mod tests;
