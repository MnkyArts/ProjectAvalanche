use std::fmt;

use ssx_net_join::browse::{ServerAddress, State, parse_search};
use ssx_net_proto::join::RefusalCode;
use zeroize::Zeroizing;

use super::{PROVIDER, rows::selected};
use crate::game::fe::ava::keyboard::{self, Outcome, Page, Requester, TextRequest};
use crate::game::fe::{ActionParam, FeEnv, FrontEnd};

pub const CONNECTING: &str = "ava_connecting";
pub const CONNECTING_CLOSE: &str = "ava_connecting_close";
pub const DOWNLOADING: &str = "ava_downloading";
pub const DOWNLOADING_CLOSE: &str = "ava_downloading_close";
// PORT(choice): 6 lines (the popup's body holds about eight lines of the game's font at 720p); tuning: the popup with a
// server of many resources, looked at; the original has no server browser.
pub const DOWNLOAD_LINES: usize = 6;
pub const MESSAGE: &str = "ava_browser_message";
pub const LINK_CONFIRM: &str = "ava_link_confirm";
pub const MAX_SEARCH: usize = 64;
pub const MAX_ADDRESS: usize = 256;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Asked {
    Search,
    Address,
    Password { address: ServerAddress, name: Option<String>, again: bool, key: Option<ssx_net_proto::keys::ServerKey> },
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Flow {
    #[default]
    Idle,
    Keyboard(Asked),
    ConfirmLink(ServerAddress),
    Connecting {
        address: ServerAddress,
        name: Option<String>,
        password: bool,
        key: Option<ssx_net_proto::keys::ServerKey>,
    },
    InSession {
        address: ServerAddress,
    },
}

#[derive(Clone, PartialEq, Eq)]
pub struct ConnectRequest {
    pub address: ServerAddress,
    pub name: String,
    pub password: Zeroizing<Vec<u8>>,
    pub key: Option<ssx_net_proto::keys::ServerKey>,
}

impl fmt::Debug for ConnectRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let key = self
            .key
            .map_or_else(|| "trusted on first use".to_string(), |k| format!("listed key {}", &k.to_hex()[..16]));
        write!(f, "Connect {{ {} as {:?}, password of {} bytes, {key} }}", self.address, self.name, self.password.len())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NetRequest {
    Connect(ConnectRequest),
    Cancel,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NetProgress {
    Joined {
        name: String,
    },
    Launching,
    Ended {
        reason: Option<String>,
        refusal: Option<RefusalCode>,
    },
    Downloading {
        lines: Vec<ssx_net_join::resources::ResourceLine>,
        received: u64,
        total: u64,
        step: ssx_net_join::resources::Step,
    },
    Held {
        text: String,
    },
}

pub fn player_name(fe: &FrontEnd) -> String {
    crate::game::fe::ava::name_prompt::join_name(fe)
}

fn ask_keyboard(fe: &mut FrontEnd, env: &mut FeEnv, asked: Asked) {
    let (title, max_len, password, initial, page) = match &asked {
        Asked::Search => {
            ("TXT_AVA_BROWSER_SEARCH_TITLE", MAX_SEARCH, false, fe.ava.browser.search.clone(), Page::Letters)
        }
        Asked::Address => ("TXT_AVA_BROWSER_ADDRESS_TITLE", MAX_ADDRESS, false, String::new(), Page::Numbers),
        Asked::Password { again: false, .. } => ("TXT_AVA_PASSWORD_NEEDED", 64, true, String::new(), Page::Letters),
        Asked::Password { again: true, .. } => ("TXT_AVA_PASSWORD_WRONG", 64, true, String::new(), Page::Letters),
    };
    let req =
        TextRequest { requester: Requester::Provider(PROVIDER), title: title.into(), max_len, password, initial, page };
    match keyboard::request(fe, env, req) {
        Ok(()) => fe.ava.browser.flow = Flow::Keyboard(asked),
        Err(e) => fe.note(format!("browser: the keyboard: {e}")),
    }
}

pub fn message(fe: &mut FrontEnd, text: String) {
    fe.note(format!("browser: message: {text}"));
    fe.ava.browser.messages.push_back(text);
}

pub fn show_messages(fe: &mut FrontEnd, env: &mut FeEnv) {
    let b = &mut fe.ava.browser;
    if b.message_up || !matches!(b.flow, Flow::Idle) {
        return;
    }
    let Some(text) = b.messages.pop_front() else { return };
    let glyphs = fe.ava.glyphs.clone();
    env.values.set_str(crate::game::fe::popup::POPUP_BODY, crate::game::fe::ava::shown(&text, glyphs.as_ref()));
    env.nav_events.push(MESSAGE.into());
    fe.ava.browser.message_up = true;
}

fn start(
    fe: &mut FrontEnd,
    env: &mut FeEnv,
    address: ServerAddress,
    name: Option<String>,
    password: Zeroizing<Vec<u8>>,
    key: Option<ssx_net_proto::keys::ServerKey>,
) {
    let shown = name.clone().map_or_else(|| address.to_string(), |n| format!("{n} ({address})"));
    let glyphs = fe.ava.glyphs.clone();
    let body = env.loc.format("TXT_AVA_CONNECTING_BODY", &[&shown]);
    env.values.set_str(crate::game::fe::popup::POPUP_BODY, crate::game::fe::ava::shown(&body, glyphs.as_ref()));
    env.nav_events.push(CONNECTING.into());
    fe.busy.set_connecting(env, true);
    let sent = !password.is_empty();
    let req = ConnectRequest { address: address.clone(), name: player_name(fe), password, key };
    fe.note(format!("browser: {req:?}"));
    fe.ava.browser.requests.push(NetRequest::Connect(req));
    fe.ava.browser.flow = Flow::Connecting { address, name, password: sent, key };
}

fn connect_to(fe: &mut FrontEnd, env: &mut FeEnv, address: ServerAddress, name: Option<String>) {
    let entry = fe.ava.browser.entries.iter().find(|e| e.address == address).cloned();
    let key =
        match entry.as_ref().and_then(|e| e.listed.as_ref()).map(|l| ssx_net_proto::keys::ServerKey::from_bytes(l.key))
        {
            Some(Ok(k)) => Some(k),
            Some(Err(_)) => return message(fe, env.loc.text("TXT_AVA_BROWSER_BAD_LISTED_KEY")),
            None => None,
        };
    let over = entry.as_ref().and_then(|e| super::resources_refusal(fe, env.loc, e));
    match entry.as_ref().map(|e| (&e.state, e.fake, e.summary())) {
        Some((_, true, _)) => return message(fe, env.loc.text("TXT_AVA_BROWSER_FAKE_ENTRY")),
        Some((State::Incompatible { why, .. }, _, _)) => {
            return message(fe, env.loc.format("TXT_AVA_BROWSER_CANNOT_JOIN", &[why]));
        }
        Some((_, _, Some(s))) if s.map.is_none() && fe.career.event_named(&s.drop).is_none() => {
            return message(fe, env.loc.format("TXT_AVA_BROWSER_DROP_MISSING", &[&s.drop]));
        }
        Some(_) if over.is_some() => return message(fe, over.unwrap_or_default()),
        Some((_, _, Some(s))) if s.password => {
            return ask_keyboard(fe, env, Asked::Password { address, name, again: false, key });
        }
        _ => {}
    }
    start(fe, env, address, name, Zeroizing::new(Vec::new()), key);
}

pub fn action(fe: &mut FrontEnd, env: &mut FeEnv, action: &str, _param: &ActionParam) {
    if !matches!(fe.ava.browser.flow, Flow::Idle) {
        fe.note(format!("browser: {action} while {:?}: not now", fe.ava.browser.flow));
        return;
    }
    match action {
        "A_AvaBrowserConnect" => {
            let Some(e) = selected(&fe.ava.browser) else { return };
            let (address, name) = (e.address.clone(), Some(e.name()));
            connect_to(fe, env, address, name);
        }
        "A_AvaBrowserDirect" => ask_keyboard(fe, env, Asked::Address),
        "A_AvaBrowserSearch" => ask_keyboard(fe, env, Asked::Search),
        "A_AvaBrowserFavourite" => favourite(fe, env),
        _ => {}
    }
}

fn favourite(fe: &mut FrontEnd, env: &mut FeEnv) {
    let Some(e) = selected(&fe.ava.browser).cloned() else { return };
    if e.fake {
        return message(fe, env.loc.text("TXT_AVA_BROWSER_FAKE_ENTRY"));
    }
    let name = e.info().map(|i| i.name.clone()).or(e.saved_name.clone());
    let now = unix_now();
    let file = fe.ava.browser.file.get_or_insert_with(Default::default);
    match file.toggle_favourite(&e.address, name.as_deref(), now) {
        Ok(on) => {
            fe.note(format!("browser: {} {} the favourites", e.address, if on { "added to" } else { "removed from" }));
            save(fe, env);
            if fe.ava.browser.tab == super::Tab::Favourites {
                super::load_tab(fe);
            }
        }
        Err(why) => message(fe, why),
    }
}

fn unix_now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

fn save(fe: &mut FrontEnd, env: &mut FeEnv) {
    let (Some(dir), Some(file)) = (fe.ava.browser.app_dir.clone(), fe.ava.browser.file.clone()) else {
        return message(fe, env.loc.text("TXT_AVA_BROWSER_NOT_KEPT"));
    };
    match file.save(&dir) {
        Ok(None) => {}
        Ok(Some(note)) | Err(note) => message(fe, note),
    }
}

pub fn keyboard_results(fe: &mut FrontEnd, env: &mut FeEnv) {
    let Some(outcome) = keyboard::take_result(fe, PROVIDER) else { return };
    let Flow::Keyboard(asked) = std::mem::take(&mut fe.ava.browser.flow) else { return };
    let Outcome::Done(text) = outcome else { return };
    match asked {
        Asked::Search => {
            fe.ava.browser.terms = parse_search(&text);
            fe.ava.browser.search = text;
            fe.ava.browser.dirty = true;
            fe.ava.browser.restart = true;
        }
        Asked::Address => match ServerAddress::parse(text.trim(), Some(ssx_net_proto::DEFAULT_PORT)) {
            Ok(address) => connect_to(fe, env, address, None),
            Err(why) => message(fe, env.loc.format("TXT_AVA_BROWSER_BAD_ADDRESS", &[&why])),
        },
        Asked::Password { address, name, key, .. } => {
            start(fe, env, address, name, Zeroizing::new(text.into_bytes()), key);
        }
    }
}

pub fn progress(fe: &mut FrontEnd, env: &mut FeEnv, p: NetProgress) {
    fe.note(format!("browser: the host: {p:?}"));
    match p {
        NetProgress::Joined { name } => {
            if let Flow::Connecting { address, .. } = &fe.ava.browser.flow {
                let address = address.clone();
                fe.ava.browser.file.get_or_insert_with(Default::default).joined(&address, Some(&name), unix_now());
                save(fe, env);
            }
        }
        NetProgress::Downloading { lines, received, total, step } => {
            let Flow::Connecting { address, name, .. } = &fe.ava.browser.flow else { return };
            let server = name.clone().unwrap_or_else(|| address.to_string());
            let glyphs = fe.ava.glyphs.clone();
            let body: Vec<String> = download_body(env, &server, &lines, received, total, step)
                .iter()
                .map(|l| crate::game::fe::ava::shown(l, glyphs.as_ref()))
                .collect();
            env.values.set_str(crate::game::fe::popup::POPUP_BODY, body.join("\n"));
            if !fe.ava.browser.download_up {
                env.nav_events.push(CONNECTING_CLOSE.into());
                env.nav_events.push(DOWNLOADING.into());
                fe.ava.browser.download_up = true;
            }
            fe.publish.push(crate::game::fe::popup::DP_POPUP.into());
        }
        NetProgress::Held { text } => {
            let Flow::Connecting { address, name, .. } = &fe.ava.browser.flow else { return };
            let server = name.clone().map_or_else(|| address.to_string(), |n| format!("{n} ({address})"));
            let held = if text.is_empty() {
                env.loc.text("TXT_AVA_CONNECTING_HELD_QUIET")
            } else {
                env.loc.format("TXT_AVA_CONNECTING_HELD", &[&text])
            };
            let glyphs = fe.ava.glyphs.clone();
            let body = [env.loc.format("TXT_AVA_CONNECTING_BODY", &[&server]), held]
                .iter()
                .map(|l| crate::game::fe::ava::shown(l, glyphs.as_ref()))
                .collect::<Vec<_>>()
                .join("\n");
            env.values.set_str(crate::game::fe::popup::POPUP_BODY, body);
            fe.publish.push(crate::game::fe::popup::DP_POPUP.into());
        }
        NetProgress::Launching => {
            if let Flow::Connecting { address, .. } = std::mem::take(&mut fe.ava.browser.flow) {
                env.nav_events.push(close_popup(fe).into());
                fe.busy.set_connecting(env, false);
                fe.ava.browser.flow = Flow::InSession { address };
            }
        }
        NetProgress::Ended { reason, refusal } => {
            let flow = std::mem::take(&mut fe.ava.browser.flow);
            fe.busy.set_connecting(env, false);
            match flow {
                Flow::Connecting { address, name, password, key } => {
                    env.nav_events.push(close_popup(fe).into());
                    if refusal == Some(RefusalCode::Password) {
                        return ask_keyboard(fe, env, Asked::Password { address, name, again: password, key });
                    }
                    if let Some(r) = reason {
                        message(fe, r);
                    }
                }
                Flow::InSession { .. } => {
                    if let Some(r) = reason {
                        message(fe, r);
                    }
                }
                other => fe.ava.browser.flow = other,
            }
        }
    }
}

fn close_popup(fe: &mut FrontEnd) -> &'static str {
    if std::mem::take(&mut fe.ava.browser.download_up) { DOWNLOADING_CLOSE } else { CONNECTING_CLOSE }
}

fn mb(bytes: u64) -> String {
    format!("{:.1}", bytes as f64 / f64::from(1 << 20))
}

fn step_key(step: ssx_net_join::resources::Step) -> Option<&'static str> {
    use ssx_net_join::resources::Step;
    match step {
        Step::Reading | Step::Fetching => None,
        Step::CheckingFiles => Some("TXT_AVA_DOWNLOADING_CHECKING_FILES"),
        Step::CheckingContent => Some("TXT_AVA_DOWNLOADING_CHECKING_CONTENT"),
    }
}

fn download_body(
    env: &mut FeEnv,
    server: &str,
    lines: &[ssx_net_join::resources::ResourceLine],
    received: u64,
    total: u64,
    step: ssx_net_join::resources::Step,
) -> Vec<String> {
    let mut out = vec![env.loc.format("TXT_AVA_DOWNLOADING_BODY", &[server, &mb(received), &mb(total)])];
    let mut left = received;
    for l in lines.iter().take(DOWNLOAD_LINES) {
        let name = if l.title.is_empty() { l.name.clone() } else { l.title.clone() };
        let to_fetch = l.bytes - l.cached.min(l.bytes);
        if to_fetch == 0 {
            out.push(env.loc.format("TXT_AVA_DOWNLOADING_CACHED", &[&name, &mb(l.bytes)]));
            continue;
        }
        let got = if l.done { to_fetch } else { left.min(to_fetch) };
        left -= got.min(left);
        out.push(env.loc.format("TXT_AVA_DOWNLOADING_LINE", &[&name, &mb(l.cached + got), &mb(l.bytes)]));
    }
    if lines.len() > DOWNLOAD_LINES {
        out.push(env.loc.format("TXT_AVA_DOWNLOADING_MORE", &[&(lines.len() - DOWNLOAD_LINES).to_string()]));
    }
    if let Some(key) = step_key(step) {
        out.push(env.loc.format(key, &[]));
    }
    out
}

pub fn nav_action(fe: &mut FrontEnd, env: &mut FeEnv, name: &str) -> bool {
    match name {
        "do_ava_connect_cancel" => {
            if matches!(fe.ava.browser.flow, Flow::Connecting { .. }) {
                fe.note("browser: cancel");
                fe.ava.browser.download_up = false;
                fe.ava.browser.requests.push(NetRequest::Cancel);
                fe.ava.browser.flow = Flow::Idle;
                fe.busy.set_connecting(env, false);
            }
        }
        "do_ava_browser_message_ok" => fe.ava.browser.message_up = false,
        "do_ava_link_yes" | "do_ava_link_no" => {
            if let Flow::ConfirmLink(address) = std::mem::take(&mut fe.ava.browser.flow) {
                if name == "do_ava_link_yes" {
                    fe.note(format!("browser: the link's {address}: yes"));
                    connect_to(fe, env, address, None);
                } else {
                    fe.note(format!("browser: the link's {address}: no (nothing was sent)"));
                }
            }
        }
        _ => return false,
    }
    true
}

pub fn open_link(fe: &mut FrontEnd, address: ServerAddress) {
    fe.note(format!("browser: a link names {address}: asking the player"));
    fe.ava.browser.link = Some(address);
}

pub fn activated(fe: &mut FrontEnd, env: &mut FeEnv) {
    if !matches!(fe.ava.browser.flow, Flow::Idle) {
        return;
    }
    let Some(address) = fe.ava.browser.link.take() else { return };
    let glyphs = fe.ava.glyphs.clone();
    let body = env.loc.format("TXT_AVA_LINK_BODY", &[&address.to_string()]);
    env.values.set_str(crate::game::fe::popup::POPUP_BODY, crate::game::fe::ava::shown(&body, glyphs.as_ref()));
    env.nav_events.push(LINK_CONFIRM.into());
    fe.ava.browser.flow = Flow::ConfirmLink(address);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_download_popup_says_its_step_past_the_bytes() {
        use ssx_net_join::resources::Step;
        assert_eq!(step_key(Step::Reading), None);
        assert_eq!(step_key(Step::Fetching), None);
        let strings: std::collections::BTreeMap<String, String> =
            serde_json::from_str(include_str!("../../../../../../../assets/fe/strings/eng_us.json")).expect("strings");
        for step in [Step::CheckingFiles, Step::CheckingContent] {
            let key = step_key(step).expect("a line");
            let text = strings.get(key).unwrap_or_else(|| panic!("{key}"));
            assert!(!text.contains('%'), "{key}: {text:?}");
        }
    }
}
