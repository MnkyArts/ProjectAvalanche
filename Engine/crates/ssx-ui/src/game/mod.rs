pub mod clip;
pub mod fe;
pub mod feed;
pub mod hud;
pub mod input;
pub mod kept;
pub mod leaderboard;
pub mod loc;
pub mod nav;
pub mod pointer;
pub mod port;
pub mod prompts;
pub mod ride;
pub mod session_board;
pub mod sim_hold;
pub mod stats;
pub mod store;
pub mod tricks;
pub mod tutorial;
pub mod utility;
pub mod value_names;
pub mod values;

mod calls;
pub mod display_class;
mod frame;
mod navigation;
mod relayout;

use anyhow::Result;

use crate::avm1::runtime::nav as navev;
use crate::avm1::value::Value;
use crate::avm1::{Ctx, Vm};
use crate::display::Player;
use crate::fs::FileSource;
use crate::library::Library;
use clip::Clip;
use hud::{HUD_DATA, HudEnv, HudProvider};
use loc::{Language, Localization};
use store::DataStore;
use values::UiValues;

pub const FE_HZ: f64 = 30.0;

const CPP_ACTIONS: &[(&str, &str)] = &[
    ("gameactions", "A_GameQuitComplete"),
    ("gameactions", "A_GameGameplayStarted"),
    ("gameactions", "ControllerPull"),
];

#[derive(Clone, Debug)]
struct ResourceRequest {
    path: String,
    kind: i32,
    tag: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NavWait {
    Enter(String),
    Exit { view: String, view_model: Option<String> },
}

#[derive(Clone, Debug, Default)]
pub struct BridgeStats {
    pub binds: kept::KeptMap<String, String>,
    pub taken: kept::KeptMap<String, String>,
    pub subscribes: kept::KeptMap<String, bool>,
    pub resources: kept::KeptMap<String, String>,
    pub publishes: u64,
    pub published: kept::Kept<String>,
    pub inputs: u64,
    pub dispatched: kept::Kept<String>,
    pub actions: kept::Kept<String>,
    pub content_notices: kept::Kept<String>,
}

pub struct GameUi {
    pub values: UiValues,
    pub loc: Localization,
    pub store: DataStore,
    pub hud: HudProvider,
    pub nav: nav::Nav,
    pub input: input::InputProvider,
    pub pointer: pointer::PointerRouter,
    pub event_type: i32,
    pub free_ride: bool,
    pub gear: [bool; 15],
    pub sounds: Vec<u32>,
    pub tutorial: tutorial::TutorialManager,
    pub stats: BridgeStats,
    resources: Vec<ResourceRequest>,
    pub handler: Option<String>,
    pub fe: Option<fe::FrontEnd>,
    dimmer: fe::user::Dimmer,
    pending_views: Vec<String>,
    due_views: Vec<String>,
    loading_views: Vec<String>,
    pub nav_wait: Option<NavWait>,
    pub port: Option<port::PortFe>,
    fe_clock: f64,
    pub display: display_class::DisplayClass,
    relayout_due: bool,
    relayout: Option<relayout::Relayout>,
    views_display: display_class::DisplayClass,
}

impl GameUi {
    pub fn new(files: &dyn FileSource, language: Language) -> Result<Self> {
        let mut loc = Localization::load(files, language)?;
        for (key, text) in port::builtin_strings()? {
            loc.add_port_string(&key, &text)?;
        }
        let refused = apply_package_strings(files, &mut loc);
        let store = DataStore::load_files(files)?;
        let nav = nav::Nav::load(files)?;
        let mut values = UiValues::new();
        set_display(&mut values, 1280, 720);
        let mut stats = BridgeStats::default();
        for line in refused {
            stats.content_notices.push(line);
        }
        Ok(GameUi {
            values,
            loc,
            store,
            hud: HudProvider::default(),
            nav,
            input: input::InputProvider::default(),
            pointer: pointer::PointerRouter::default(),
            event_type: -1,
            free_ride: false,
            gear: [false; 15],
            sounds: Vec::new(),
            tutorial: Default::default(),
            stats,
            resources: Vec::new(),
            handler: None,
            fe: None,
            dimmer: fe::user::Dimmer::default(),
            pending_views: Vec::new(),
            due_views: Vec::new(),
            loading_views: Vec::new(),
            nav_wait: None,
            port: None,
            fe_clock: 0.0,
            display: display_class::DisplayClass::WIDESCREEN,
            relayout_due: false,
            relayout: None,
            views_display: display_class::DisplayClass::WIDESCREEN,
        })
    }

    pub fn set_launch_data(&mut self, boot_movie: ssx_assets::port_options::BootMovie) {
        fe::boot::set_launch_data(&mut self.values, boot_movie.skip_intro());
    }

    pub fn game_input(&mut self, start: bool) {
        self.fe_call(|fe, env| fe::pause::game_input(fe, env, start));
    }

    pub fn pad_connection(&mut self, pad: usize, connected: bool) {
        self.fe_call(|fe, env| fe::pads::connection(fe, env, pad, connected));
    }

    pub fn system_ui(&mut self, shown: bool) {
        self.fe_call(|fe, env| fe::pads::system_ui(fe, env, shown));
    }

    pub fn active_pad(&self) -> Option<usize> {
        self.fe.as_ref().and_then(|f| f.pads.active_pad())
    }

    pub fn take_requests(&mut self) -> Vec<fe::pause::GameRequest> {
        self.fe.as_mut().map(|f| std::mem::take(&mut f.requests)).unwrap_or_default()
    }

    pub fn take_map_requests(&mut self) -> Vec<fe::ava::maps::MapRequest> {
        self.fe.as_mut().map(|f| std::mem::take(&mut f.ava.maps.requests)).unwrap_or_default()
    }

    pub fn take_ui_sounds(&mut self) -> Vec<i32> {
        let mut out = self.fe.as_mut().map(|f| std::mem::take(&mut f.ui_sounds)).unwrap_or_default();
        out.extend(std::mem::take(&mut self.sounds).into_iter().map(|id| id as i32));
        out
    }

    pub fn take_ui_posts(&mut self) -> Vec<fe::globe::audio::Post> {
        self.fe
            .as_mut()
            .and_then(|f| f.globe_scene.as_mut())
            .map(|s| std::mem::take(&mut s.audio.posts))
            .unwrap_or_default()
    }

    pub fn fe_audio_message(&mut self, screen: u32, state: i32, beat: Option<fe::globe::audio::Beat>) {
        let Some(audio) = self.fe.as_mut().and_then(|f| f.globe_scene.as_mut()).map(|s| &mut s.audio) else { return };
        audio.adopt(screen, state);
        if let Some(b) = beat {
            audio.beat_message(b);
        }
    }

    pub fn fe_audio_wind(&self) -> i32 {
        self.fe.as_ref().and_then(|f| f.globe_scene.as_ref()).map_or(0, |s| s.audio.wind_param)
    }

    pub fn take_fe_messages(&mut self) -> Vec<fe::FeMessage> {
        self.fe.as_mut().map(|f| std::mem::take(&mut f.messages)).unwrap_or_default()
    }

    fn publish(&mut self, cx: &mut Ctx) {
        let explicit: Vec<String> = self.fe.as_mut().map(|f| std::mem::take(&mut f.publish)).unwrap_or_default();
        let dimmer: Vec<(i32, String)> = match self.fe.as_mut() {
            Some(f) => f.dimmer.drain(),
            None => self.dimmer.drain(),
        };
        for i in 0..self.store.subscribers.len() {
            let s = &self.store.subscribers[i];
            let (dp, served, path, name) = (s.dp.clone(), s.served, s.path.clone(), s.name.clone());
            if dp == fe::user::DP_DIMMER {
                for (action, state) in &dimmer {
                    let value = fe::user::dimmer_object(cx, *action, state);
                    let ok = invoke_path(cx, &format!("{path}.Publish"), &[Value::str(&name), value]).is_some();
                    self.stats.published.push(format!(
                        "{name} → {path} {{{action}, {state:?}}}{}",
                        if ok { "" } else { " (no Publish)" }
                    ));
                    self.store.subscribers[i].served = true;
                    self.stats.publishes += 1;
                }
                continue;
            }
            let asked = explicit.contains(&dp);
            let objects = HUD_DATA.iter().chain(hud::results::RESULTS_DATA).chain(fe::data::FE_DATA);
            let value = if let Some(obj) = objects.clone().find(|o| o.name == dp) {
                if served && !obj.dirty(&self.values) && !asked {
                    continue;
                }
                let v = obj.build(cx, &self.values, &mut self.loc);
                if dp == "dp_race_results_campaign_data" {
                    let campaign = match self.fe.as_ref() {
                        Some(f) => f.results_campaign(&mut self.loc),
                        None => None,
                    };
                    hud::results::campaign_fields(cx, &v, campaign);
                }
                v
            } else if let Some(front) = self.fe.as_mut() {
                if served && !asked {
                    continue;
                }
                match front.build(cx, &mut self.values, &mut self.loc, &dp) {
                    Some(v) => v,
                    None => continue,
                }
            } else {
                continue;
            };
            let ok = invoke_path(cx, &format!("{path}.Publish"), &[Value::str(&name), value]).is_some();
            self.stats.published.push(format!("{name} → {path}{}", if ok { "" } else { " (no Publish)" }));
            self.store.subscribers[i].served = true;
            self.stats.publishes += 1;
        }
    }

    pub fn set_drop(&mut self, drop_key: u32, event_type: i32, free_ride: bool, nis_active: bool) {
        self.nav.drop_key = drop_key;
        self.event_type = event_type;
        self.free_ride = free_ride;
        self.tutorial_level_start();
        hud::level_start(&mut self.values, nis_active);
        if let Some(fe) = self.fe.as_mut() {
            fe.level_start.nis = nis_active;
        }
    }
}

pub fn resolve_path(cx: &mut Ctx, path: &str) -> Option<Value> {
    let mut parts = path.split('.');
    let level = parts.next()?.strip_prefix("_level")?.parse::<i32>().ok()?;
    let mut cur = Value::Object(cx.object_for(*cx.player.levels.get(&level)?));
    for p in parts {
        cur = cx.get_member(&cur, p).ok()?;
        if cur.is_nullish() {
            return None;
        }
    }
    Some(cur)
}

pub fn apply_package_strings(files: &dyn FileSource, loc: &mut Localization) -> Vec<String> {
    let Some(content) = files.content() else { return Vec::new() };
    loc.content = Some(content.clone());
    let strings = content.package_strings(loc.content_lang());
    if strings.is_empty() {
        return Vec::new();
    }
    loc.apply_package_strings(&strings)
}

impl GameUi {
    pub fn reload_strings(&mut self, files: &dyn FileSource) -> Result<usize> {
        let mut loc = Localization::load(files, self.loc.language)?;
        for (key, text) in port::builtin_strings()? {
            loc.add_port_string(&key, &text)?;
        }
        let refused = apply_package_strings(files, &mut loc);
        if let Some(fe) = &self.fe {
            for (key, text) in ssx_assets::content::characters::career::port_strings(&fe.career) {
                let _ = loc.add_port_string(&key, &text);
            }
        }
        for line in refused {
            self.stats.content_notices.push(line);
        }
        let n = files.content().map_or(0, |c| c.package_strings(loc.content_lang()).len());
        self.loc = loc;
        Ok(n)
    }
}

pub fn set_display(values: &mut UiValues, width: i32, height: i32) {
    values.set_int(0x96, width);
    values.set_int(0x95, height);
    values.set_bool(0x94, true);
}

pub fn invoke_path(cx: &mut Ctx, path: &str, args: &[Value]) -> Option<Value> {
    let (obj_path, method) = path.rsplit_once('.')?;
    let obj = resolve_path(cx, obj_path)?;
    let (f, _) = cx.get_method(&obj, method).ok()?;
    if !cx.is_function(&f) {
        return None;
    }
    Some(cx.call_top(&f, obj, args))
}
