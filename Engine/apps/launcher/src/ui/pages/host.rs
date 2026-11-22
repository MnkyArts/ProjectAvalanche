use eframe::egui::{self, RichText, Ui};

use super::HOST_APPLIES;
use crate::app::Launcher;
use crate::command;
use crate::host::{self, Listen, Process, Shown, Written};
use crate::ui::details::Detail;
use crate::ui::rows::{self, Field, Row};
use crate::ui::theme;
use crate::ui::widgets::{self, Kind};

pub const INTRO: &str = "A multiplayer server on this computer: the same dedicated server as on a rented machine, as \
     its own program. Your network finds it in the game's LAN tab; others join by your address, or find it on the \
     public server list while it is listed there.";

impl Launcher {
    pub(super) fn host_script(&mut self, ctx: &egui::Context) {
        if self.host.script.is_empty() && self.host.scripted.is_none() {
            return;
        }
        ctx.request_repaint_after(std::time::Duration::from_millis(100));
        if !self.host.scripted_done() || self.frame < 20 {
            return;
        }
        self.host.scripted = None;
        let Some(action) = self.host.script.pop_front() else { return };
        match action {
            host::Action::Prepare => self.host_prepare(ctx),
            host::Action::Start => self.host_start(ctx),
            host::Action::Status => {
                if let Some(p) = &mut self.host.server {
                    p.send("status");
                }
                self.host.shown = Shown::Server;
            }
            host::Action::Stop => {
                if let Some(p) = &mut self.host.server {
                    p.stop();
                }
                self.host.shown = Shown::Server;
            }
        }
        self.host.scripted = Some((action, std::time::Instant::now()));
    }

    pub(super) fn poll_host(&mut self, ctx: &egui::Context) {
        let h = &mut self.host;
        if let Some(job) = &h.loading_drops
            && let Some(drops) = job.take()
        {
            h.drops = Some(drops);
            h.loading_drops = None;
        }
        if h.drops.is_none() && h.loading_drops.is_none() && self.data_ready {
            let big = self.settings.big_root();
            h.loading_drops = Some(crate::jobs::Background::spawn(ctx, move || host::drops(&big)));
        }
        if let Some(job) = &h.loading_maps
            && let Some(maps) = job.take()
        {
            h.maps = Some(maps);
            h.loading_maps = None;
        }
        if h.maps.is_none() && h.loading_maps.is_none() {
            let app_dir = crate::settings::app_dir();
            h.loading_maps = Some(crate::jobs::Background::spawn(ctx, move || host::installed_maps(&app_dir)));
        }
        if let Some(job) = &h.staging
            && let Some(staged) = job.take()
        {
            h.staging = None;
            match staged {
                Ok(_) => self.host_setup(ctx),
                Err(e) => self.host.note = Some((false, format!("the map's packages were not copied: {e}"))),
            }
        }
    }

    fn host_map(&self) -> Result<Option<host::HostedMap>, String> {
        let Some(m) = &self.settings.host.map else { return Ok(None) };
        let Some(maps) = &self.host.maps else { return Err("the installed maps are still being read".into()) };
        maps.hosted(m).map(Some)
    }

    pub fn host_section(&mut self, ui: &mut Ui) {
        ui.spacing_mut().item_spacing.y = 0.0;
        let exe = command::server_exe();
        if !exe.is_file() {
            widgets::status(
                ui,
                theme::ERROR,
                format!("The server is missing: {} (it ships next to the launcher).", exe.display()),
            );
        }
        let have_server = exe.is_file();
        let ctx = ui.ctx().clone();
        let running = self.host.server_running();
        let busy = self.host.busy();
        self.host_fields(ui, !running && !busy);
        ui.spacing_mut().item_spacing.y = 6.0;
        rows::group(ui, "Folder", None);
        widgets::path(ui, &self.settings.host.folder_in(&self.app_dir));
        rows::buttons(ui, |ui| {
            if widgets::button(ui, "Prepare the folder", Kind::Plain, !running && !busy && have_server).clicked() {
                self.host_prepare(&ctx);
            }
            if widgets::button(ui, "Check", Kind::Plain, !busy && have_server).clicked() && self.host_save(false) {
                self.host_run_check(&ctx);
            }
            if self.host.busy() {
                ui.spinner();
                let setting_up = self.host.setup.as_ref().is_some_and(Process::running);
                if setting_up
                    && widgets::button(ui, "Cancel", Kind::Plain, true).clicked()
                    && let Some(p) = &mut self.host.setup
                {
                    p.kill();
                }
            }
        });
        if self.host.edited_by_hand {
            widgets::status(ui, theme::WARN, "server.toml was changed outside the launcher: it is kept as it is.");
            rows::buttons(ui, |ui| {
                if widgets::button(ui, "Replace it with these settings", Kind::Plain, !running).clicked() {
                    self.host_save(true);
                }
                if widgets::button(ui, "Keep it", Kind::Plain, true).clicked() {
                    self.host.edited_by_hand = false;
                    self.host.note = Some((true, "server.toml kept; the server reads it as you wrote it.".into()));
                }
            });
        }
        if let Some((ok, text)) = &self.host.note {
            widgets::status(ui, if *ok { theme::GOOD } else { theme::ERROR }, text.clone());
        }
        self.host_run_rows(ui, have_server);
    }

    fn host_run_rows(&mut self, ui: &mut Ui, have_server: bool) {
        let ctx = ui.ctx().clone();
        let running = self.host.server_running();
        let busy = self.host.busy();
        rows::group(ui, "Running", None);
        let state = match &self.host.server {
            Some(p) if p.running() => {
                (theme::CYAN, format!("Running (process {}) on port {}", p.pid, self.settings.host.port))
            }
            Some(p) => (theme::TEXT_DIM, format!("Stopped: {}", p.status().unwrap_or_default())),
            None => (theme::TEXT_DIM, "Not started".to_string()),
        };
        widgets::status(ui, state.0, state.1);
        rows::buttons(ui, |ui| {
            if widgets::button(ui, "Start", Kind::Primary, !running && !busy && have_server).clicked() {
                self.host_start(&ctx);
            }
            if widgets::button(ui, "Stop", Kind::Plain, running).clicked()
                && let Some(p) = &mut self.host.server
            {
                p.stop();
            }
            if widgets::button(ui, "Status", Kind::Plain, running).clicked()
                && let Some(p) = &mut self.host.server
            {
                p.send("status");
                self.host.shown = Shown::Server;
            }
            let can_join = running && self.data_state.is_complete() && !self.running();
            if widgets::button(ui, "Join with the game", Kind::Plain, can_join).clicked() {
                let server = Listen::join_address(self.settings.host.port);
                let inv = command::game_joining(&self.settings, &self.settings.big_root(), server);
                self.start(&ctx, &inv, crate::jobs::Program::Game);
            }
        });
        rows::group(ui, "Output", None);
        let name = |s: &Shown| match s {
            Shown::Server => "Server".to_owned(),
            Shown::Setup => "Setup".to_owned(),
            Shown::Check => "Check".to_owned(),
        };
        let now = name(&self.host.shown);
        let detail = || {
            Detail::new("Output")
                .text("Whose output the lines below show: the server's, the folder's preparation, its check.")
                .now(now, None)
        };
        let all = [Shown::Server, Shown::Setup, Shown::Check];
        rows::pick(ui, Row::new("Output"), &mut self.host.shown, &all, name, detail);
        let lines = self.host.lines();
        egui::ScrollArea::vertical().max_height(200.0).stick_to_bottom(true).auto_shrink([false, false]).show(
            ui,
            |ui| {
                ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                    for line in lines.iter().rev().take(400).rev() {
                        let text = RichText::new(host::compact(line)).monospace().size(11.0).color(theme::TEXT_SOFT);
                        ui.add(egui::Label::new(text).wrap());
                    }
                });
            },
        );
    }

    fn host_fields(&mut self, ui: &mut Ui, enabled: bool) {
        let drops = self.host.drops.clone();
        let maps = self.host.maps.clone();
        let h = &mut self.settings.host;
        let row = |label| Row::new(label).enabled(enabled);
        rows::group(ui, "Server", None);
        let now = h.name.clone();
        let detail = || {
            Detail::new("Server name")
                .text("The name players see in the game's server list.")
                .now(now, None)
                .applies(HOST_APPLIES)
        };
        rows::text_field(ui, row("Server name"), &mut h.name, Field { chars: 40, password: false, hint: "" }, detail);
        match &drops {
            Some(Ok(list)) => {
                let now = h.drop.clone();
                let detail =
                    || Detail::new("Drop").text("The drop the server hosts.").now(now, None).applies(HOST_APPLIES);
                rows::pick(ui, row("Drop"), &mut h.drop, list, |d| d.clone(), detail);
            }
            Some(Err(e)) => {
                let text = format!("The drops do not read: {e}");
                let detail = || Detail::new("Drop").warn(Some(text.clone()));
                rows::info(ui, Row::new("Drop"), &text, theme::ERROR, detail);
            }
            None => {
                let text = "(the drops come from the game data once it is set up)";
                rows::info(ui, Row::new("Drop"), text, theme::TEXT_DIM, || Detail::new("Drop").text(text));
            }
        }
        if host_map_rows(ui, h, maps.as_ref(), enabled) {
            self.host.maps = None;
        }
        let h = &mut self.settings.host;
        let now = h.max_players;
        let detail = || {
            Detail::new("Players")
                .text("The most players on the server at once (1 to 64).")
                .now(now.to_string(), None)
                .applies(HOST_APPLIES)
        };
        let counts: Vec<u16> = (1..=64).collect();
        rows::pick(ui, row("Players"), &mut h.max_players, &counts, |n| n.to_string(), detail);
        let set = if h.password.is_empty() { "none" } else { "set" };
        let detail = || {
            Detail::new("Password")
                .text("Players need it to join. Empty: anyone can join.")
                .now(set, None)
                .applies(HOST_APPLIES)
        };
        rows::text_field(
            ui,
            row("Password"),
            &mut h.password,
            Field { chars: 64, password: true, hint: "none" },
            detail,
        );
        let now = h.port;
        let detail = || {
            Detail::new("Port")
                .text("The port players connect to (1 to 65534).")
                .now(now.to_string(), None)
                .applies(HOST_APPLIES)
        };
        rows::number(ui, row("Port"), &mut h.port, 1..=65534, detail);
        let listen_name = |l: &Listen| Listen::ALL.iter().find(|(v, _)| v == l).map_or("", |(_, t)| *t).to_owned();
        let now = listen_name(&h.listen);
        let detail = || {
            Detail::new("Listen on").text("Which network the server answers on.").now(now, None).applies(HOST_APPLIES)
        };
        let listens: Vec<Listen> = Listen::ALL.iter().map(|(v, _)| *v).collect();
        rows::pick(ui, row("Listen on"), &mut h.listen, &listens, listen_name, detail);
        rows::group(ui, "Who can find it", None);
        let lan = h.lan;
        rows::toggle(ui, row("LAN and server query"), &mut h.lan, || {
            Detail::new("LAN and server query")
                .text("Answer the LAN tab and the server query.")
                .now(if lan { "On" } else { "Off" }, None)
                .applies(HOST_APPLIES)
        });
        let local = h.listen == Listen::Local;
        let warn = local.then(|| "A server for this computer only is not listed.".to_owned());
        let public = h.public && !local;
        let detail = || {
            Detail::new("Public server list")
                .text("Listed on the public server list.")
                .warn(warn)
                .now(if public { "On" } else { "Off" }, None)
                .applies(HOST_APPLIES)
        };
        if local {
            rows::toggle(ui, Row::new("Public server list").enabled(false), &mut false, detail);
        } else {
            rows::toggle(ui, row("Public server list"), &mut h.public, detail);
        }
        let mapping = h.port_mapping;
        rows::toggle(ui, row("Map ports on my router"), &mut h.port_mapping, || {
            Detail::new("Map ports on my router")
                .text("Map the ports on my router (UPnP, PCP, NAT-PMP).")
                .now(if mapping { "On" } else { "Off" }, None)
                .applies(HOST_APPLIES)
        });
        ui.add_space(4.0);
        for p in host::problems(h) {
            widgets::status(ui, theme::WARN, p);
        }
    }

    fn host_save(&mut self, replace: bool) -> bool {
        let problems = host::problems(&self.settings.host);
        if !problems.is_empty() {
            self.host.note = Some((false, problems.join("; ")));
            return false;
        }
        let map = match self.host_map() {
            Ok(m) => m,
            Err(e) => {
                self.host.note = Some((false, e));
                return false;
            }
        };
        let h = &self.settings.host;
        let text = match host::config_text(h, map.as_ref()) {
            Ok(t) => t,
            Err(e) => {
                self.host.note = Some((false, e));
                return false;
            }
        };
        match host::write_config(&h.config_path(), &text, h.written, replace) {
            Ok(Written::Yes(hash)) => {
                self.settings.host.written = Some(hash);
                self.host.edited_by_hand = false;
                self.save_settings_now();
                true
            }
            Ok(Written::EditedByHand) => {
                self.host.edited_by_hand = true;
                true
            }
            Err(e) => {
                self.host.note = Some((false, e));
                false
            }
        }
    }

    fn host_prepare(&mut self, ctx: &egui::Context) {
        if !self.host_save(false) {
            return;
        }
        match self.host_map() {
            Ok(Some(map)) => {
                let (h, app_dir) = (self.settings.host.clone(), crate::settings::app_dir());
                self.host.staging =
                    Some(crate::jobs::Background::spawn(ctx, move || host::stage_map(&h, &map, &app_dir)));
                self.host.shown = Shown::Setup;
            }
            Ok(None) => self.host_setup(ctx),
            Err(e) => self.host.note = Some((false, e)),
        }
    }

    fn host_setup(&mut self, ctx: &egui::Context) {
        let map = match self.host_map() {
            Ok(m) => m,
            Err(e) => {
                self.host.note = Some((false, e));
                return;
            }
        };
        let h = &self.settings.host;
        let map = map.as_ref().map(|m| &m.map);
        let inv = if self.data_state.is_complete() {
            host::setup_from_extraction(h, &self.settings.game_data(), map)
        } else if let Some(iso) = self.settings.iso.clone().filter(|p| p.is_file()) {
            host::setup_from_image(h, &iso, map)
        } else {
            self.host.note = Some((
                false,
                "Set up or repair the game data first (Content, Game data), or select the disc image there: the \
                 server's rules come from it."
                    .into(),
            ));
            return;
        };
        let log = h.folder().join("launcher-setup.log");
        self.host.setup = spawn(ctx, &inv, &log, false, &mut self.host.note);
        self.host.shown = Shown::Setup;
    }

    fn host_run_check(&mut self, ctx: &egui::Context) {
        let h = &self.settings.host;
        let log = h.folder().join("launcher-check.log");
        self.host.check = spawn(ctx, &host::check(h), &log, false, &mut self.host.note);
        self.host.shown = Shown::Check;
    }

    fn host_start(&mut self, ctx: &egui::Context) {
        if !self.host_save(false) {
            return;
        }
        if !ssx_assets::subset::is_complete(&self.settings.host.data_dir()) {
            let text = "Prepare the folder first: the server's rules are taken from your game data there.";
            self.host.note = Some((false, text.into()));
            return;
        }
        if let Ok(Some(map)) = self.host_map() {
            let dir = host::resources_dir(&self.settings.host);
            if let Some((_, missing)) = map.packages().iter().find(|(_, name)| !dir.join(name).is_dir()) {
                let text = format!("Prepare the folder first: the map's package {missing} is copied there.");
                self.host.note = Some((false, text));
                return;
            }
        }
        let h = &self.settings.host;
        let log = h.folder().join("launcher-server.log");
        self.host.server = spawn(ctx, &host::run(h), &log, true, &mut self.host.note);
        self.host.shown = Shown::Server;
    }
}

fn spawn(
    ctx: &egui::Context,
    inv: &crate::command::Invocation,
    log: &std::path::Path,
    console: bool,
    note: &mut Option<(bool, String)>,
) -> Option<Process> {
    match Process::spawn(ctx, inv, log, console) {
        Ok(p) => {
            *note = None;
            Some(p)
        }
        Err(e) => {
            *note = Some((false, format!("could not start {}: {e}", inv.program.display())));
            None
        }
    }
}

fn host_map_rows(ui: &mut Ui, h: &mut host::HostSettings, maps: Option<&host::HostMaps>, enabled: bool) -> bool {
    const NONE: &str = "None (the drop above)";
    let Some(maps) = maps else {
        let text = "(reading the installed maps)";
        rows::info(ui, Row::new("Map"), text, theme::TEXT_DIM, || Detail::new("Map").text(text));
        return false;
    };
    let title = |id: &Option<String>| match id {
        None => NONE.to_owned(),
        Some(id) => {
            maps.maps.iter().find(|x| x.id == *id).map_or_else(|| id.clone(), |x| format!("{} ({})", x.title, x.id))
        }
    };
    let mut chosen = h.map.as_ref().map(|m| m.id.clone());
    let all: Vec<Option<String>> = std::iter::once(None).chain(maps.maps.iter().map(|m| Some(m.id.clone()))).collect();
    let now = title(&chosen);
    let none_installed =
        maps.maps.is_empty().then(|| "(no map installed: Content, Mods installs map packages)".to_owned());
    let detail = || {
        Detail::new("Map")
            .text("An installed map instead of the drop above, then its mode.")
            .warn(none_installed.clone())
            .now(now, None)
            .applies(HOST_APPLIES)
    };
    if rows::pick(ui, Row::new("Map").enabled(enabled), &mut chosen, &all, title, detail) {
        h.map = chosen.and_then(|id| {
            let m = maps.maps.iter().find(|x| x.id == id)?;
            Some(host::HostMap { id: m.id.clone(), mode: m.modes.first().cloned().unwrap_or_default() })
        });
    }
    if let Some(chosen) = h.map.as_mut()
        && let Some(m) = maps.maps.iter().find(|x| x.id == chosen.id)
    {
        let modes = maps.modes_of(m);
        let mode_name = |v: &String| modes.iter().find(|(x, _)| x == v).map_or_else(|| v.clone(), |x| x.1.clone());
        let values: Vec<String> = modes.iter().map(|(v, _)| v.clone()).collect();
        let now = mode_name(&chosen.mode);
        let detail = || {
            Detail::new("Mode")
                .text("The map's mode: those its map.toml offers and the installed gamemodes that take it.")
                .now(now, None)
                .applies(HOST_APPLIES)
        };
        rows::pick(ui, Row::new("Mode").enabled(enabled), &mut chosen.mode, &values, mode_name, detail);
    }
    if let Some(none) = &none_installed {
        widgets::note(ui, none);
    }
    for why in maps.unusable.iter().take(3) {
        widgets::status(ui, theme::WARN, format!("Cannot host {why}"));
    }
    rows::buttons(ui, |ui| widgets::button(ui, "Read the installed maps again", Kind::Plain, true).clicked())
}
