pub mod files;
pub mod overlay;
pub mod scroll;
#[cfg(feature = "net")]
pub mod session;
pub mod view;

use std::sync::Arc;

use anyhow::{Context, Result, bail};

use crate::author::{Layout, json};
use crate::avm1::object::{Kind, Object};
use crate::avm1::value::Value;
use crate::avm1::{Ctx, Vm};
use crate::display::Player;
use crate::fs::FileSource;
use crate::library::Library;
use files::ROOT;

macro_rules! port_files {
    ($($path:literal),* $(,)?) => {
        &[$(($path, include_str!(concat!("../../../../../assets/fe/", $path)))),*]
    };
}

pub const FILES: &[(&str, &str)] = port_files![
    "views/ava_keyboard.json",
    // PORT(choice): the hub's rows show the item's name only, 60 px high: its cell plays `components.MainMenuCell`'s
    // title-only frames (`Off->Highlight`, `Highlight`, `Unhighlight`: the bottom line at 53 px, 20 px above the
    // `…Info` set's 73, a row of 60 where the `…Info` row is `DEFAULT_HEIGHT` 80) instead of the `…Info` set the class
    // always plays; six rows make a panel of 6 · 60 − 5 · 10 + 5 − 14 = 301 px (421 with 80-px rows, to y 639 of 720);
    // the disc has no such menu (its main menu shows a description under each of its items).
    "views/ava_hub.json",
    // PORT(choice): the maps' list (9 rows of 36 px, 2 apart, at (101, 252), 730 wide) gets the disc's scroll bar 1 px
    // right of its rows as globaldrop places its own (views/globaldrop.gfx: `ListMenu` at x -110, `ROW_WIDTH` 220, the
    // bar at x 111): x 832, from the rows' top for their height, 340 = 9 · 36 + 8 · 2 (the track listing's bar,
    // views/activityfeedtracklisting.gfx, is 275 long beside 8 rows of 34); the bar runs over the table panel's edge (x
    // 839) and ends 1 px before the details panel (x 849); the disc has no such list.
    "views/ava_maps.json",
    // PORT(choice): the port's Options list has 7 rows (9 with the network), where the disc's `SettingsMenu` (5 rows,
    // `MAX_VISIBLE_MENU_ROWS`) places no scroll bar; the bar (`components.ScrollBar`, 16 px wide) is placed as
    // globaldrop places its own (views/globaldrop.gfx: 1 px right of its rows, `ListMenu` at x -110 with `ROW_WIDTH`
    // 220, the bar at x 111) at x 571 (the menu's x 90 + its width 480 + 1), from the panel's top (y 190) for the
    // panel's height, 251 = `ListMenu.RedrawPanel`'s 5 · 60 − 4 · 10 + 5 − 14; the menu is set up at the view's
    // `Initialize` and shown at once, where `SettingsMenu` (0x05da…0x09be) hides it until `settings_init` is published
    // and sets it up then (the provider answers that subscription in the same update); the disc's list never scrolls.
    // PORT(choice): the rows are 480 wide where `SettingsMenu`'s are `MENU_WIDTH` 350: the cell's title fields
    // (`components.ListMenuCellLarge`, edit texts 14 and 16: 394 px from x 20, the right edge at 414) run 64 px past a
    // 350-px panel, under the bar, and the port's rows fill them ("Player Name: " and up to 20 characters; the disc's
    // labels are short); at 480 the fields end 66 px inside the panel. A longer label is still cut at the field's edge.
    "views/ava_options.json",
    "viewmodels/avakeyboard.vm",
    "viewmodels/avahub.vm",
    "viewmodels/avamaps.vm",
    "viewmodels/avacredits.vm",
    "viewmodels/avaoptions.vm",
    "viewmodels/avaoptionspage.vm",
    "viewmodels/avaoptionsgame.vm",
    "viewmodels/avaoptionscalibration.vm",
    "viewmodels/extensions.json",
    "viewmodels/datadefinitions/avakeyboard.dd",
    "viewmodels/datadefinitions/avahub.dd",
    "viewmodels/datadefinitions/avamaps.dd",
    "viewmodels/datadefinitions/avamainmenu.dd",
    "viewmodels/datadefinitions/avacredits.dd",
    "viewmodels/datadefinitions/avaoptions.dd",
    "nav/overlay.json",
    "popups/ava_exit.pop",
    "popups/ava_options_reset.pop",
    "popups/ava_net_message.pop",
    "popups/ava_maps_message.pop",
    "popups/ava_settings_broken.pop",
    "strings/eng_us.json",
    "credits.json",
    "third_party.json",
];

#[cfg(feature = "net")]
pub const NET_FILES: &[(&str, &str)] = port_files![
    // PORT(choice): the server browser's list (as the maps': 9 rows of 36 px at (101, 252), 730 wide) gets the disc's
    // scroll bar 1 px right of its rows as globaldrop places its own (views/globaldrop.gfx: `ListMenu` at x -110,
    // `ROW_WIDTH` 220, the bar at x 111): x 832, 340 long (9 · 36 + 8 · 2); the disc has no such list.
    "views/ava_browser.json",
    "viewmodels/avaserverbrowser.vm",
    "viewmodels/avabrowserfilters.vm",
    "viewmodels/datadefinitions/avaserverbrowser.dd",
    "viewmodels/datadefinitions/avabrowserfilters.dd",
    "nav/overlay_net.json",
    "popups/ava_connecting.pop",
    "popups/ava_downloading.pop",
    "popups/ava_browser_message.pop",
    "popups/ava_link_confirm.pop",
    "popups/ava_options_message.pop",
    "popups/ava_name_refused.pop",
    "popups/ava_name_settings_broken.pop",
    // PORT(choice): the Players screen's list (9 rows of 34 px, 2 apart, at (190, 298), 900 wide) gets the disc's
    // scroll bar 1 px right of its rows as globaldrop places its own (views/globaldrop.gfx: the bar 1 px right of
    // `ROW_WIDTH` 220): x 1091, 322 long (9 · 34 + 8 · 2); the disc has no such list.
    "views/ava_players.json",
    "views/ava_chat.json",
    "views/ava_quickchat.json",
    "viewmodels/avaplayers.vm",
    "viewmodels/avachat.vm",
    "viewmodels/avaquickchat.vm",
    "viewmodels/datadefinitions/avaplayers.dd",
    "viewmodels/datadefinitions/avachat.dd",
    "viewmodels/datadefinitions/avaquickchat.dd",
    "viewmodels/datadefinitions/avapausemenu.dd",
    "viewmodels/extensions_net.json",
    "popups/ava_leave_server.pop",
];
#[cfg(not(feature = "net"))]
pub const NET_FILES: &[(&str, &str)] = &[];

pub fn all_files() -> impl Iterator<Item = &'static (&'static str, &'static str)> {
    FILES.iter().chain(NET_FILES.iter())
}

pub const OPENERS: &[(&str, &str)] = &[
    ("keyboard", "ava_keyboard_open"),
    ("credits", "go_to_ava_credits"),
    ("options", "go_to_ava_options"),
    ("maps", crate::game::fe::ava::maps::OPEN),
];

pub const OPEN_FROM: &str = "AvaHub";

pub const MAX_FILE: usize = 256 * 1024;

#[derive(Default)]
pub struct PortFe {
    pub layouts: Vec<Layout>,
    pub strings: Vec<(String, String)>,
    pub registered: bool,
    pub open: Option<String>,
    pub log: Vec<String>,
}

impl PortFe {
    pub fn new(open: Option<&str>) -> Result<PortFe> {
        let open = match open {
            Some(name) => {
                Some(OPENERS.iter().find(|(n, _)| *n == name).map(|(_, e)| e.to_string()).with_context(|| {
                    format!("--open-view {name}: one of {:?}", OPENERS.iter().map(|o| o.0).collect::<Vec<_>>())
                })?)
            }
            None => None,
        };
        Ok(PortFe { open, ..Default::default() })
    }
}

pub fn read_text(files: &dyn FileSource, path: &str) -> Result<String> {
    let data = files.read(path).with_context(|| format!("{path} not found"))?;
    if data.len() > MAX_FILE {
        bail!("{path}: {} bytes, at most {MAX_FILE}", data.len());
    }
    String::from_utf8(data).with_context(|| format!("{path}: not UTF-8"))
}

fn stem<'a>(path: &'a str, extension: &str) -> Result<&'a str> {
    let name = path.rsplit('/').next().and_then(|f| f.strip_suffix(extension)).unwrap_or_default();
    if name.is_empty() || !name.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_') {
        bail!("{path}: the name must be lower-case letters, digits and `_`");
    }
    Ok(name)
}

pub fn parse_strings(path: &str, text: &str) -> Result<Vec<(String, String)>> {
    let root = json::parse(text, &crate::author::limits::JSON)
        .map_err(|e| anyhow::anyhow!("{path}: {}: {}", e.pos, e.message))?;
    let json::Value::Object(members) = root.value else { bail!("{path}: not an object") };
    let mut out = Vec::new();
    for (k, v) in members {
        let ok = k.name.strip_prefix("TXT_AVA_").is_some_and(|r| {
            !r.is_empty() && r.bytes().all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_')
        });
        if !ok {
            bail!("{path}: {}: {:?} is not a TXT_AVA_ key", k.pos, k.name);
        }
        let json::Value::String(s) = v.value else { bail!("{path}: {}: {:?} is not a string", v.pos, k.name) };
        out.push((k.name, s));
    }
    Ok(out)
}

pub fn builtin_strings() -> Result<Vec<(String, String)>> {
    let path = format!("{ROOT}strings/eng_us.json");
    parse_strings(&path, &read_text(&files::PortFiles, &path)?)
}

pub fn parse_extensions(path: &str, text: &str) -> Result<Vec<(String, Vec<String>)>> {
    let root = json::parse(text, &crate::author::limits::JSON)
        .map_err(|e| anyhow::anyhow!("{path}: {}: {}", e.pos, e.message))?;
    let members = |n: &json::Node, allowed: &[&str]| -> Result<Vec<(String, json::Node)>> {
        let json::Value::Object(m) = &n.value else { bail!("{path}: {}: not an object", n.pos) };
        if let Some((k, _)) = m.iter().find(|(k, _)| !allowed.contains(&k.name.as_str())) {
            bail!("{path}: {}: unknown member `{}`", k.pos, k.name);
        }
        Ok(m.iter().map(|(k, v)| (k.name.clone(), v.clone())).collect())
    };
    let get = |m: &[(String, json::Node)], k: &str| m.iter().find(|(n, _)| n == k).map(|(_, v)| v.value.clone());
    let name = |v: Option<json::Value>, what: &str| -> Result<String> {
        match v {
            Some(json::Value::String(s))
                if !s.is_empty()
                    && s.len() <= 64
                    && s.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_') =>
            {
                Ok(s)
            }
            _ => bail!("{path}: `{what}` must be a name of lower-case letters, digits and `_`"),
        }
    };
    let top = members(&root, &["format", "extend"])?;
    if get(&top, "format") != Some(json::Value::Number(1.0)) {
        bail!("{path}: `format` must be 1");
    }
    let Some(json::Value::Array(list)) = get(&top, "extend") else { bail!("{path}: `extend` must be an array") };
    let mut out = Vec::new();
    for e in &list {
        let m = members(e, &["view_model", "providers"])?;
        let vm = name(get(&m, "view_model"), "view_model")?;
        let Some(json::Value::Array(ps)) = get(&m, "providers") else { bail!("{path}: `providers` must be an array") };
        let ps = ps.into_iter().map(|p| name(Some(p.value), "providers")).collect::<Result<Vec<_>>>()?;
        out.push((vm, ps));
    }
    Ok(out)
}

impl crate::game::GameUi {
    pub fn enable_port_fe(&mut self, files: &dyn FileSource, mut port: PortFe) -> Result<()> {
        if self.nav.started() {
            bail!("the port's front-end layer must be enabled before the nav starts");
        }
        let Some(fe) = self.fe.as_mut() else { bail!("the port's front-end layer needs the front end") };
        for path in files.list(&format!("{ROOT}viewmodels/datadefinitions/"), "dd") {
            let text = read_text(files, &path)?;
            self.store.add_definition(stem(&path, ".dd")?, &text).with_context(|| format!("port file {path}"))?;
        }
        for path in files.list(&format!("{ROOT}viewmodels/"), "vm") {
            let text = read_text(files, &path)?;
            self.store.add_view_model(stem(&path, ".vm")?, &text).with_context(|| format!("port file {path}"))?;
        }
        for path in [format!("{ROOT}viewmodels/extensions.json"), format!("{ROOT}viewmodels/extensions_net.json")] {
            if path.ends_with("_net.json") && !files.exists(&path) {
                continue;
            }
            for (vm, providers) in parse_extensions(&path, &read_text(files, &path)?)? {
                for p in providers {
                    self.store.extend_view_model(&vm, &p).with_context(|| format!("port file {path}"))?;
                }
            }
        }
        for path in files.list(&format!("{ROOT}popups/"), "pop") {
            let text = read_text(files, &path)?;
            let def = crate::game::fe::popup::parse(stem(&path, ".pop")?, &text)
                .with_context(|| format!("port file {path}: not a popup"))?;
            fe.popups.add(def).with_context(|| format!("port file {path}"))?;
        }
        let data = files
            .read(crate::game::nav::Nav::PATH)
            .with_context(|| format!("{} not found", crate::game::nav::Nav::PATH))?;
        let chart: serde_json::Value = serde_json::from_slice(&data).context("parsing ssxgame.nav")?;
        let overlay = overlay::Overlay::parse(&read_text(files, &format!("{ROOT}nav/overlay.json"))?)?;
        let mut chart = overlay.apply(&chart)?;
        let net = format!("{ROOT}nav/overlay_net.json");
        if files.exists(&net) {
            chart = overlay::Overlay::parse(&read_text(files, &net)?)?.apply(&chart)?;
        }
        self.nav = crate::game::nav::Nav::from_json(&chart)?;
        let path = format!("{ROOT}strings/eng_us.json");
        port.strings = parse_strings(&path, &read_text(files, &path)?)?;
        for (key, text) in &port.strings {
            self.loc.add_port_string(key, text)?;
        }
        for line in super::apply_package_strings(files, &mut self.loc) {
            self.stats.content_notices.push(line);
        }
        let path = format!("{ROOT}credits.json");
        fe.ava.credits = crate::game::fe::ava::credits::sections(&read_text(files, &path)?)?;
        let path = format!("{ROOT}third_party.json");
        fe.ava.third_party = crate::game::fe::ava::credits::third_party(&read_text(files, &path)?)?;
        for path in files.list(&format!("{ROOT}views/"), "json") {
            port.layouts.push(Layout::parse(&path, &read_text(files, &path)?).map_err(anyhow::Error::msg)?);
        }
        fe.ava.hub.on = true;
        self.port = Some(port);
        Ok(())
    }

    pub(crate) fn port_register(&mut self, vm: &mut Vm, lib: &mut Library, player: &mut Player) {
        let Some(port) = self.port.as_mut() else { return };
        if port.registered {
            return;
        }
        let Some(handler) = self.handler.clone() else { return };
        port.registered = true;
        let Some(app) = handler.strip_suffix(".mFrameworkEventHandler").map(str::to_string) else {
            port.log.push(format!("the framework's handler {handler} is not the Application's"));
            return;
        };
        let mut log = Vec::new();
        let layouts = port.layouts.clone();
        let keys: Vec<String> = port.strings.iter().map(|(k, _)| k.clone()).collect();
        let credits = self.fe.as_ref().map(|f| f.ava.credits.clone()).unwrap_or_default();
        log.extend(glyph_problems(lib, &mut self.loc, &keys, &credits));
        if let Err(e) = install_classes(&mut vm.ctx(lib, player)) {
            log.push(format!("Avalanche classes: {e:#}"));
            if let Some(port) = self.port.as_mut() {
                port.log.extend(log);
            }
            return;
        }
        for l in &layouts {
            match register_view(vm, lib, player, &app, l) {
                Ok(()) => log.push(format!(
                    "view {} registered ({} on {})",
                    l.view,
                    crate::author::movie_url(&l.view),
                    l.layer
                )),
                Err(e) => log.push(format!("view {}: {e:#}", l.view)),
            }
        }
        if let Some(fe) = self.fe.as_mut() {
            fe.ava.keyboard.glyphs = keyboard_glyphs(lib, &layouts);
            let all = lib
                .font_library()
                .values()
                .chain(lib.disc_fallback())
                .flat_map(|f| f.codes.iter().filter_map(|&c| char::from_u32(u32::from(c))));
            fe.ava.glyphs = Some(all.collect());
        }
        if let Some(port) = self.port.as_mut() {
            port.log.extend(log);
        }
    }

    pub fn text_focused(&self) -> bool {
        self.fe.as_ref().is_some_and(|f| f.ava.keyboard.active)
    }

    pub fn text_input(&mut self, input: crate::game::fe::ava::keyboard::TextInput) {
        self.fe_call(|fe, env| crate::game::fe::ava::keyboard::text_input(fe, env, input));
    }

    pub fn request_text(&mut self, req: crate::game::fe::ava::keyboard::TextRequest) -> Result<()> {
        if self.port.is_none() {
            bail!("the on-screen keyboard is the port's front-end layer's (off with the classic front end)");
        }
        let mut out = Ok(());
        self.fe_call(|fe, env| out = crate::game::fe::ava::keyboard::request(fe, env, req));
        out.map_err(anyhow::Error::msg)
    }

    #[cfg(feature = "net")]
    pub fn take_net_requests(&mut self) -> Vec<crate::game::fe::ava::browser::NetRequest> {
        self.fe.as_mut().map(|f| std::mem::take(&mut f.ava.browser.requests)).unwrap_or_default()
    }

    #[cfg(feature = "net")]
    pub fn net_progress(&mut self, p: crate::game::fe::ava::browser::NetProgress) {
        self.fe_call(|fe, env| crate::game::fe::ava::browser::connect::progress(fe, env, p));
    }

    #[cfg(feature = "net")]
    pub fn open_link(&mut self, address: ssx_net_join::browse::ServerAddress) {
        if let Some(fe) = self.fe.as_mut() {
            fe.ava.browser.enter_fresh();
            crate::game::fe::ava::browser::connect::open_link(fe, address);
        }
    }

    pub fn chat_open(&mut self, quick: bool) -> bool {
        #[cfg(feature = "net")]
        {
            let mut opened = false;
            self.fe_call(|fe, env| {
                use crate::game::fe::ava::session::chat;
                opened = if quick { chat::open_quick(fe, env) } else { chat::open_text(fe, env) };
            });
            opened
        }
        #[cfg(not(feature = "net"))]
        {
            let _ = quick;
            false
        }
    }

    pub fn chat_held(&self) -> bool {
        #[cfg(feature = "net")]
        return self.fe.as_ref().is_some_and(crate::game::fe::ava::session::chat::held);
        #[cfg(not(feature = "net"))]
        false
    }

    pub fn browser_active(&self) -> bool {
        #[cfg(feature = "net")]
        return self.fe.as_ref().is_some_and(|f| f.ava.browser.active);
        #[cfg(not(feature = "net"))]
        false
    }

    pub fn take_text_results(&mut self) -> Vec<crate::game::fe::ava::keyboard::TextResult> {
        self.fe.as_mut().map(|f| f.ava.keyboard.take_host()).unwrap_or_default()
    }

    pub(crate) fn port_open(&mut self) {
        let ready = self.nav.is_active(OPEN_FROM) && !self.views_loading();
        #[cfg(feature = "net")]
        if self.name_prompt_step(ready) {
            return;
        }
        let Some(port) = self.port.as_mut() else { return };
        if !ready || !port.registered {
            return;
        }
        if let Some(event) = port.open.take() {
            port.log.push(format!("--open-view: {event}"));
            self.nav.raise(&event);
        }
    }
}

fn install_classes(cx: &mut Ctx) -> Result<()> {
    let view_proto = view::global_path(cx, "ux.view.View.prototype");
    let fe_view_proto = view::global_path(cx, "ssx.FEView.prototype");
    let feed_view_proto = view::global_path(cx, "ssx.FEActivityFeedView.prototype");
    let cell_proto = view::global_path(cx, "ux.view.component.Cell.prototype");
    let grid_cell = view::global_path(cx, "components.IGridMenuCell");
    let (Some(view_proto), Some(fe_view_proto), Some(feed_view_proto), Some(cell_proto), Some(grid_cell)) = (
        view_proto.as_object(),
        fe_view_proto.as_object(),
        feed_view_proto.as_object(),
        cell_proto.as_object(),
        grid_cell.as_object(),
    ) else {
        bail!(
            "the framework's classes are missing (ux.view.View, ssx.FEView, ssx.FEActivityFeedView, \
             ux.view.component.Cell, components.IGridMenuCell)"
        );
    };
    let pkg = cx.new_object();
    let classes = [
        ("View", view_proto, view::VIEW_METHODS, view::view_ctor as crate::avm1::object::NativeFn),
        ("FEView", fe_view_proto, view::VIEW_METHODS, view::view_ctor),
        ("ActivityFeedView", feed_view_proto, view::VIEW_METHODS, view::view_ctor),
        ("Cell", cell_proto, view::CELL_METHODS, view::cell_ctor),
    ];
    for (name, base, methods, ctor) in classes {
        let proto = cx.vm.heap.alloc(Object::new(Value::Object(base), Kind::Plain));
        for (m, f) in methods {
            let fun = cx.new_native(*f);
            view::hidden(cx, proto, m, Value::Object(fun));
        }
        if name == "Cell"
            && let Some(o) = cx.vm.heap.get_mut(proto)
        {
            o.interfaces.push(grid_cell);
        }
        let c = cx.new_native(ctor);
        view::hidden(cx, c, "prototype", Value::Object(proto));
        view::hidden(cx, proto, "constructor", Value::Object(c));
        view::hidden(cx, pkg, name, Value::Object(c));
    }
    let global = cx.vm.global;
    view::hidden(cx, global, "Avalanche", Value::Object(pkg));
    Ok(())
}

fn register_view(vm: &mut Vm, lib: &mut Library, player: &mut Player, app: &str, l: &Layout) -> Result<()> {
    let map = format!("{app}.mNavigator.mViewMapReference");
    if vm.get_path(lib, player, &map).is_none() {
        bail!("no ViewMap at {map}");
    }
    let taken = vm.invoke(lib, player, &format!("{map}.GetViewById"), &[Value::str(&l.view)]);
    if taken.is_some_and(|v| !v.is_nullish()) {
        bail!("the ViewMap has a view {}", l.view);
    }
    l.movie(lib).map_err(anyhow::Error::msg)?;
    let layout = l.clone();
    lib.add_builder(
        &crate::author::movie_url(&l.view),
        Arc::new(move |lib| layout.build(lib).map_err(anyhow::Error::msg)),
    )?;
    let view_base = match l.logic.base {
        crate::author::Base::View => "View",
        crate::author::Base::FeView => "FEView",
        crate::author::Base::ActivityFeedView => "ActivityFeedView",
    };
    let classes: Vec<(String, Arc<dyn std::any::Any + Send + Sync>, &str)> = l
        .symbols
        .iter()
        .filter_map(|s| {
            if s.name == l.view_symbol {
                Some((s.name.clone(), Arc::new(view::ViewClass { logic: l.logic.clone() }) as Arc<_>, view_base))
            } else {
                s.cell
                    .as_ref()
                    .map(|c| (s.name.clone(), Arc::new(view::CellClass { def: c.clone() }) as Arc<_>, "Cell"))
            }
        })
        .collect();
    for (export, data, base) in classes {
        if vm.classes.contains_key(export.as_str()) {
            bail!("a class is registered as {export}");
        }
        let mut cx = vm.ctx(lib, player);
        let base_proto = view::global_path(&mut cx, &format!("Avalanche.{base}.prototype"));
        let Some(base_proto) = base_proto.as_object() else { bail!("Avalanche.{base} is missing") };
        let proto = cx.vm.heap.alloc(Object::new(Value::Object(base_proto), Kind::Plain));
        let f = if base == "Cell" { view::cell_ctor as crate::avm1::object::NativeFn } else { view::view_ctor };
        let ctor = cx.new_native(f);
        view::hidden(&mut cx, ctor, "prototype", Value::Object(proto));
        view::hidden(&mut cx, proto, "constructor", Value::Object(ctor));
        cx.vm.class_data.insert(proto, data);
        let ok = vm.invoke(lib, player, "_global.Object.registerClass", &[Value::str(&export), Value::Object(ctor)]);
        if !matches!(ok, Some(Value::Bool(true))) {
            bail!("Object.registerClass({export}) failed");
        }
    }
    let layer = vm.get_path(lib, player, &format!("_global.ViewMap.{}", l.layer)).context("the layer's constant")?;
    let url = Value::str(&crate::author::movie_url(&l.view));
    vm.invoke(lib, player, &format!("{map}.AddToViewMap"), &[Value::str(&l.view), layer, url])
        .context("AddToViewMap")?;
    Ok(())
}

fn glyph_problems(
    lib: &Library,
    loc: &mut crate::game::loc::Localization,
    keys: &[String],
    credits: &[crate::game::fe::ava::credits::Section],
) -> Vec<String> {
    let fonts: Vec<_> = lib.font_library().values().cloned().collect();
    let mut out = Vec::new();
    let drawn = |c: char| fonts.iter().any(|f| crate::author::has_glyph(f, c));
    for key in keys {
        let text = loc.text(key);
        if let Some(c) = text.chars().find(|&c| !drawn(c)) {
            out.push(format!("{key}: no font of the library draws {c:?}"));
        }
    }
    for s in credits {
        for (name, by) in &s.entries {
            if let Some(c) = name.chars().chain(by.chars()).find(|&c| !drawn(c)) {
                out.push(format!("credits {name} - {by}: no font of the library draws {c:?}"));
            }
        }
    }
    out
}

fn keyboard_glyphs(lib: &Library, layouts: &[Layout]) -> Option<std::collections::BTreeSet<char>> {
    let l = layouts.iter().find(|l| l.view == "ava_keyboard")?;
    let binding = l.logic.texts.iter().find(|t| t.data == "ava_keyboard" && t.field == "text")?;
    let crate::author::ChildKind::Text(t) = &l.resolve_path(&binding.target)?.kind else { return None };
    let font = lib.font_library().get(&t.font)?;
    Some(font.codes.iter().filter_map(|&c| char::from_u32(u32::from(c))).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_port_files_read_strictly() {
        let ext =
            parse_extensions("e", r#"{"format": 1, "extend": [{"view_model": "mainmenu", "providers": ["a_b"]}]}"#);
        assert_eq!(ext.unwrap(), [("mainmenu".to_string(), vec!["a_b".to_string()])]);
        for bad in [
            r#"{"format": 2, "extend": []}"#,
            r#"{"format": 1, "extend": [], "x": 1}"#,
            r#"{"format": 1, "extend": [{"view_model": "Main", "providers": []}]}"#,
            r#"{"format": 1, "extend": [{"view_model": "m", "providers": ["../x"]}]}"#,
            r#"{"format": 1, "extend": [{"view_model": "m"}]}"#,
        ] {
            assert!(parse_extensions("e", bad).is_err(), "{bad}");
        }
        assert!(parse_strings("s", r#"{"TXT_AVA_A": "a", "TXT_B": "b"}"#).is_err());
        assert!(parse_strings("s", r#"{"TXT_AVA_A": 1}"#).is_err());
        assert_eq!(stem("ava/fe/popups/ava_exit.pop", ".pop").unwrap(), "ava_exit");
        assert!(stem("ava/fe/popups/Ava Exit.pop", ".pop").is_err());
        let files = files::PortFiles;
        for (path, _) in all_files() {
            let text = read_text(&files, &format!("{ROOT}{path}")).unwrap();
            if let Some(view) = path.strip_prefix("views/") {
                Layout::parse(view, &text).unwrap();
            }
        }
        parse_strings("s", &read_text(&files, &format!("{ROOT}strings/eng_us.json")).unwrap()).unwrap();
        parse_extensions("e", &read_text(&files, &format!("{ROOT}viewmodels/extensions.json")).unwrap()).unwrap();
        overlay::Overlay::parse(&read_text(&files, &format!("{ROOT}nav/overlay.json")).unwrap()).unwrap();
        #[cfg(feature = "net")]
        {
            let net = read_text(&files, &format!("{ROOT}viewmodels/extensions_net.json")).unwrap();
            assert_eq!(parse_extensions("e", &net).unwrap(), [("pausemenu".into(), vec!["avapausemenu".into()])]);
            overlay::Overlay::parse(&read_text(&files, &format!("{ROOT}nav/overlay_net.json")).unwrap()).unwrap();
        }
    }

    #[test]
    fn the_port_strings_join_the_table_once_in_either_mode() {
        let strings = builtin_strings().unwrap();
        let mut loc = crate::game::loc::Localization::empty(crate::game::loc::Language::English);
        for key in ["TXT_AVA_BEST_NOT_KEPT_TITLE", "TXT_AVA_BEST_NOT_KEPT_FULL", "TXT_AVA_BEST_NOT_KEPT_NO_ID"] {
            assert!(strings.iter().any(|(k, t)| k == key && !t.is_empty()), "{key}");
        }
        for _ in 0..2 {
            for (k, t) in &strings {
                loc.add_port_string(k, t).unwrap();
            }
        }
        assert_eq!(loc.text("TXT_AVA_BEST_NOT_KEPT_TITLE"), "Best Not Saved");
        loc.strings.by_hash.insert(ssx_formats::t3db::loc_hash("TXT_AVA_X"), ("TXT_AVA_X".into(), None));
        assert!(loc.add_port_string("TXT_AVA_X", "x").is_err());
    }

    #[test]
    fn every_action_a_port_view_dispatches_is_declared() {
        let files = files::PortFiles;
        let mut store = crate::game::store::DataStore::default();
        for (path, _) in all_files() {
            if let Some(dd) = path.strip_prefix("viewmodels/datadefinitions/").and_then(|p| p.strip_suffix(".dd")) {
                store.add_definition(dd, &read_text(&files, &format!("{ROOT}{path}")).unwrap()).unwrap();
            }
        }
        let declared = |name: &str| store.definitions.values().any(|d| d.actions.iter().any(|(v, _)| v == name));
        let mut checked = 0;
        for (path, _) in all_files() {
            let Some(view) = path.strip_prefix("views/") else { continue };
            let layout = Layout::parse(view, &read_text(&files, &format!("{ROOT}{path}")).unwrap()).unwrap();
            let inputs = layout.logic.inputs.iter().filter_map(|i| match &i.what {
                crate::author::InputDo::Action { name, .. } => Some(name.as_str()),
                _ => None,
            });
            for name in inputs.chain(layout.logic.paging.iter().map(|p| p.setup.as_str())) {
                assert!(declared(name), "{view}: {name} is declared by no data definition");
                checked += 1;
            }
        }
        assert!(checked > 0);
    }

    #[test]
    fn every_data_a_port_view_binds_is_declared_by_its_providers() {
        let files = files::PortFiles;
        let mut store = crate::game::store::DataStore::default();
        for (path, _) in all_files() {
            let text = || read_text(&files, &format!("{ROOT}{path}")).unwrap();
            if let Some(dd) = path.strip_prefix("viewmodels/datadefinitions/").and_then(|p| p.strip_suffix(".dd")) {
                store.add_definition(dd, &text()).unwrap();
            } else if let Some(vm) = path.strip_prefix("viewmodels/").and_then(|p| p.strip_suffix(".vm")) {
                store.add_view_model(vm, &text()).unwrap();
            }
        }
        let mut checked = 0;
        for (path, _) in all_files() {
            let Some(view) = path.strip_prefix("views/") else { continue };
            let layout = Layout::parse(view, &read_text(&files, &format!("{ROOT}{path}")).unwrap()).unwrap();
            let providers: Vec<&String> =
                store.view_models.values().filter(|m| m.view == layout.view).flat_map(|m| &m.providers).collect();
            for name in &layout.logic.data {
                let declared = providers
                    .iter()
                    .filter_map(|p| store.definitions.get(p.as_str()))
                    .any(|d| d.data.iter().any(|(v, _, _)| v == name));
                assert!(declared, "{view}: {name} is declared by none of the providers {providers:?}");
                checked += 1;
            }
        }
        assert!(checked > 0);
    }
}
