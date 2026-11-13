pub mod blocklists;
pub mod connect;
pub mod details;
pub mod filters;
pub mod icons;
pub mod pointer;
pub mod rows;
pub mod sources;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use ssx_net_join::browse::refresh::{Ask, Refresher, Reply};
use ssx_net_join::browse::{Entry, Filters, ServersFile, State, Term, fake::fake_servers};

use crate::avm1::Ctx;
use crate::avm1::value::Value;
use crate::game::fe::{ActionParam, FeEnv, FrontEnd};
use crate::game::loc::Localization;
pub use connect::{ConnectRequest, Flow, NetProgress, NetRequest};
pub use ssx_net_join::browse::{ServerAddress, parse_link};
pub use ssx_net_proto::join::RefusalCode;

pub const DP: &str = "dp_ava_browser";
pub const DP_ROWS: &str = "dp_ava_browser_rows";
pub const DP_ROWS_INDEX: &str = "dp_ava_browser_rows_index";
pub const DP_DETAILS: &str = "dp_ava_browser_details";
pub const DP_DROP: &str = "dp_ava_browser_drop";
pub const DP_WINDOW: &str = "dp_ava_browser_window";
pub const OPEN: &str = "go_to_ava_browser";
pub const PROVIDER: &str = "avaserverbrowser";

// PORT(choice): every 10 s for a tab's rows (MTA refreshes a list on request, FiveM every few minutes); tuning: phase
// 5's measured request load on public servers; the original has no server browser.
pub const LIST_INTERVAL: Duration = Duration::from_secs(10);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Tab {
    #[default]
    Internet,
    Lan,
    Favourites,
    Recent,
}

pub const TABS: [(Tab, &str); 4] = [
    (Tab::Internet, "TXT_AVA_BROWSER_TAB_INTERNET"),
    (Tab::Lan, "TXT_AVA_BROWSER_TAB_LAN"),
    (Tab::Favourites, "TXT_AVA_BROWSER_TAB_FAVOURITES"),
    (Tab::Recent, "TXT_AVA_BROWSER_TAB_RECENT"),
];

#[derive(Default)]
pub struct Browser {
    pub active: bool,
    pub app_dir: Option<PathBuf>,
    pub fake: Option<usize>,
    pub tab: Tab,
    pub entries: Vec<Entry>,
    pub file: Option<ServersFile>,
    pub filters: Filters,
    pub search: String,
    pub terms: Vec<Term>,
    pub view: Vec<usize>,
    pub window: rows::Window,
    pub dirty: bool,
    pub restart: bool,
    pub refresher: Option<Refresher>,
    pub flow: Flow,
    pub requests: Vec<NetRequest>,
    pub link: Option<ServerAddress>,
    pub messages: std::collections::VecDeque<String>,
    pub message_up: bool,
    pub download_up: bool,
    pub panel: Option<(String, Option<crate::game::values::DropPanel>)>,
    pub master: Option<ssx_net_join::browse::masters::MasterList>,
    pub said_old_masters: bool,
    pub source_problem: Option<String>,
    pub left_out: usize,
    pub pinger: Option<ssx_net_join::browse::ping::Pinger>,
    pub lan: Option<ssx_net_join::browse::lan::LanSearch>,
    pub icons: icons::Icons,
    pub blocklists: Option<ssx_net_join::blocklist::Refresh>,
}

impl Browser {
    pub fn enter_fresh(&mut self) {
        self.tab = Tab::default();
    }
}

fn fake_drops(fe: &FrontEnd) -> Vec<String> {
    let mut d: Vec<String> =
        fe.career.events.iter().map(|e| e.name.clone()).filter(|n| n.starts_with("exp_")).collect();
    d.sort();
    d
}

fn load_tab(fe: &mut FrontEnd) {
    let (fake, tab) = (fe.ava.browser.fake, fe.ava.browser.tab);
    let entries = match (fake, tab) {
        (Some(n), _) => fake_servers(n, &fake_drops(fe)),
        (None, Tab::Favourites | Tab::Recent) => {
            let file = fe.ava.browser.file.get_or_insert_with(ServersFile::default);
            let list = if tab == Tab::Favourites { &file.favourites } else { &file.recent };
            list.iter().map(|s| Entry::new(s.address.clone(), s.name.clone())).collect()
        }
        (None, Tab::Internet | Tab::Lan) => Vec::new(),
    };
    fe.ava.browser.entries = entries;
    sources::open_tab(fe, tab);
    fe.ava.browser.window = rows::Window { first: 0, focus: 0, ..fe.ava.browser.window };
    fe.ava.browser.dirty = true;
    fe.ava.browser.restart = true;
}

pub fn activate(fe: &mut FrontEnd, env: &mut FeEnv) {
    fe.game_mode = -1;
    fe.screen = crate::game::fe::screens::id::MAIN_MENU;
    env.nav_events.push("backgroundcontainer_disable".into());
    env.values.set_str(0x32, env.loc.text("TXT_AVA_HUB_TITLE"));
    env.values.set_str(0x33, env.loc.text("TXT_AVA_BROWSER_TITLE"));
    crate::game::fe::ava::options::reload(fe);
    if fe.ava.browser.file.is_none() {
        match fe.ava.browser.app_dir.clone() {
            Some(dir) => {
                let loaded = ServersFile::load(&dir);
                if let Some(p) = loaded.problem {
                    fe.note(format!("browser: {p}"));
                    fe.ava.browser.messages.push_back(p);
                }
                fe.ava.browser.file = Some(loaded.file);
            }
            None => fe.note("browser: no app folder named: favourites and recent servers are not kept"),
        }
    }
    if fe.ava.browser.fake.is_none() && fe.ava.browser.refresher.is_none() {
        let cache = fe.ava.browser.app_dir.as_ref().map(|d| {
            ssx_net_join::resources::CacheView::new(
                d.join(ssx_net_join::resources::CACHE_FOLDER),
                ssx_content::blocklist::Follow::Player(d.clone()),
            )
        });
        match Refresher::start(cache) {
            Ok(r) => fe.ava.browser.refresher = Some(r),
            Err(e) => fe.note(format!("browser: ERROR the worker threads: {e}")),
        }
    }
    sources::start_pings(fe);
    fe.ava.browser.active = true;
    load_tab(fe);
    fe.note(format!("browser: open on {:?} ({} servers)", fe.ava.browser.tab, fe.ava.browser.entries.len()));
    connect::activated(fe, env);
    fe.publish.extend([DP.into(), DP_ROWS.into(), DP_ROWS_INDEX.into(), DP_DETAILS.into(), DP_DROP.into()]);
}

pub fn deactivate(fe: &mut FrontEnd) {
    let b = &mut fe.ava.browser;
    b.active = false;
    b.refresher = None;
    b.pinger = None;
    b.master = None;
    b.lan = None;
    b.icons.forget_asked();
}

pub fn update(fe: &mut FrontEnd, env: &mut FeEnv) {
    if !fe.ava.browser.active {
        return;
    }
    connect::keyboard_results(fe, env);
    let now = Instant::now();
    let answers = fe.ava.browser.refresher.as_mut().map(Refresher::take).unwrap_or_default();
    for a in answers {
        if let Reply::Icon(hash, result) = a.reply {
            let note = fe.ava.browser.icons.answered(hash, result);
            fe.note(note);
            fe.ava.browser.dirty = true;
            continue;
        }
        let b = &mut fe.ava.browser;
        let Some(e) = b.entries.iter_mut().find(|e| e.address == a.address) else { continue };
        if a.socket.is_some() {
            e.socket = a.socket;
        }
        if matches!(a.reply, Reply::Info(_)) {
            e.need = a.need;
            e.blocked = a.blocked;
        }
        match a.reply {
            Reply::Info(Ok((ssx_net_join::browse::query::InfoAnswer::Info(info), _))) => e.state = State::Info(info),
            Reply::Info(Ok((ssx_net_join::browse::query::InfoAnswer::Incompatible { head, why }, _))) => {
                e.state = State::Incompatible { head, why };
            }
            Reply::Info(Err(why)) => e.state = State::Failed(why),
            Reply::Players(Ok(list)) => {
                e.players = Some(list.players);
                e.players_why = None;
            }
            Reply::Players(Err(why)) => {
                e.players = None;
                e.players_why = Some(why);
            }
            Reply::Icon(..) => {}
        }
        b.dirty = true;
    }
    let selected = rows::selected(&fe.ava.browser).map(|e| e.address.clone());
    if sources::update(fe, env, selected.clone(), now) {
        fe.ava.browser.dirty = true;
    }
    let internet = fe.ava.browser.tab == Tab::Internet;
    if let Some(r) = fe.ava.browser.refresher.as_mut() {
        for e in fe.ava.browser.entries.iter().filter(|e| !e.fake) {
            let detail = selected.as_ref() == Some(&e.address);
            let changed = sources::info_changed(e);
            if internet && !detail && !changed {
                continue;
            }
            let interval = if detail || changed { Duration::ZERO } else { LIST_INTERVAL };
            r.ask(&e.address, Ask::Info, interval, now);
            if detail || !internet {
                r.ask(&e.address, Ask::Players, if detail { Duration::ZERO } else { LIST_INTERVAL }, now);
            }
            if detail {
                fe.ava.browser.icons.ask(r, e, now);
            }
        }
    }
    if fe.ava.browser.dirty {
        rows::rebuild(fe, env);
    }
    connect::show_messages(fe, env);
}

pub fn action(fe: &mut FrontEnd, env: &mut FeEnv, action: &str, param: &ActionParam) {
    match action {
        "A_AvaBrowserSetup" | "A_AvaBrowserRequestData" | "A_AvaBrowserPage" => rows::action(fe, env, action, param),
        "A_AvaBrowserTab" => {
            let i = TABS.iter().position(|t| t.0 == fe.ava.browser.tab).unwrap_or(0) as i64;
            let step = if param.number < 0.0 { -1 } else { 1 };
            fe.ava.browser.tab = TABS[(i + step).rem_euclid(TABS.len() as i64) as usize].0;
            load_tab(fe);
            fe.note(format!("browser: tab {:?} ({} servers)", fe.ava.browser.tab, fe.ava.browser.entries.len()));
            fe.publish.push(DP.into());
        }
        "A_AvaBrowserRefresh" => {
            sources::refresh(fe, Instant::now());
            if let Some(r) = fe.ava.browser.refresher.as_mut() {
                let now = Instant::now();
                for e in fe.ava.browser.entries.iter().filter(|e| !e.fake) {
                    r.ask(&e.address, Ask::Info, Duration::ZERO, now);
                    r.ask(&e.address, Ask::Players, Duration::ZERO, now);
                }
            }
            fe.note("browser: refresh");
        }
        "A_AvaBrowserFilters" => filters::open(fe, env),
        "A_AvaBrowserBack" => env.nav_events.push(crate::game::fe::ava::hub::OPEN.into()),
        _ => connect::action(fe, env, action, param),
    }
}

pub fn resources_refusal(fe: &FrontEnd, loc: &mut Localization, e: &Entry) -> Option<String> {
    if let Some(why) = &e.blocked {
        return Some(loc.format("TXT_AVA_BROWSER_BLOCKED", &[why]));
    }
    let limit = fe.ava.options.settings.resources.download_limit();
    let need = e.need.filter(|n| n.over(limit))?;
    let least = match need {
        ssx_net_join::resources::Need::Exactly(n) | ssx_net_join::resources::Need::Between { least: n, .. } => n,
    };
    Some(loc.format("TXT_AVA_BROWSER_OVER_LIMIT", &[&rows::megabytes(least), &rows::megabytes(limit)]))
}

pub fn build(fe: &FrontEnd, cx: &mut Ctx, loc: &mut Localization, dp: &str) -> Option<Value> {
    match dp {
        DP => Some(rows::header(fe, cx, loc)),
        DP_ROWS | DP_ROWS_INDEX => Some(rows::build(fe, cx, loc, dp)),
        DP_DETAILS => Some(details::build(fe, cx, loc)),
        DP_DROP => Some(details::drop_panel(fe, cx)),
        DP_WINDOW => {
            Some(crate::game::fe::ava::window_object(cx, fe.ava.browser.view.len(), fe.ava.browser.window.first))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fresh_entry_opens_on_internet() {
        let mut b = Browser::default();
        assert_eq!((b.tab, TABS[0].0), (Tab::Internet, Tab::Internet));
        b.tab = Tab::Favourites;
        b.enter_fresh();
        assert_eq!(b.tab, Tab::Internet);
    }
}
