use eframe::egui::{self, RichText, Ui};

use crate::app::Launcher;
use crate::command;
use crate::host::{self, Listen, Process, Shown, Written};
use crate::ui::layout::Layout;
use crate::ui::widgets::{self, Kind};
use crate::ui::{panel, theme};

impl Launcher {
    pub fn host_page(&mut self, ui: &mut Ui, l: &Layout) {
        let body = panel::frame(ui.painter(), l, "HOST A SERVER");
        self.poll_host(ui.ctx());
        self.host_script(ui.ctx());
        panel::body(ui, body, "host", |ui| self.host_body(ui));
    }

    fn host_script(&mut self, ctx: &egui::Context) {
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

    fn poll_host(&mut self, ctx: &egui::Context) {
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

    fn host_body(&mut self, ui: &mut Ui) {
        ui.spacing_mut().item_spacing.y = 6.0;
        widgets::paragraph(
            ui,
            "A multiplayer server on this computer: the same dedicated server as on a rented machine, as its own \
             program. Your network finds it in the game's LAN tab; others join by your address, or find it on the \
             public server list while it is listed there.",
        );
        let exe = command::server_exe();
        if !exe.is_file() {
            widgets::status(
                ui,
                theme::ERROR,
                format!("The server is missing: {} (it ships next to the launcher).", exe.display()),
            );
        }
        ui.columns(2, |cols| {
            let (left, right) = cols.split_at_mut(1);
            left[0].spacing_mut().item_spacing.y = 6.0;
            right[0].spacing_mut().item_spacing.y = 6.0;
            self.host_setup_column(&mut left[0], exe.is_file());
            self.host_run_column(&mut right[0], exe.is_file());
        });
    }

    fn host_setup_column(&mut self, ui: &mut Ui, have_server: bool) {
        let ctx = ui.ctx().clone();
        let running = self.host.server_running();
        let busy = self.host.busy();
        widgets::heading(ui, "Server");
        self.host_fields(ui, !running && !busy);
        ui.add_space(4.0);
        widgets::heading(ui, "Folder");
        widgets::path(ui, &self.settings.host.folder_in(&self.app_dir));
        ui.horizontal(|ui| {
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
            ui.horizontal(|ui| {
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
    }

    fn host_run_column(&mut self, ui: &mut Ui, have_server: bool) {
        let ctx = ui.ctx().clone();
        let running = self.host.server_running();
        let busy = self.host.busy();
        widgets::heading(ui, "Running");
        let state = match &self.host.server {
            Some(p) if p.running() => {
                (theme::CYAN, format!("Running (process {}) on port {}", p.pid, self.settings.host.port))
            }
            Some(p) => (theme::TEXT_DIM, format!("Stopped: {}", p.status().unwrap_or_default())),
            None => (theme::TEXT_DIM, "Not started".to_string()),
        };
        widgets::status(ui, state.0, state.1);
        ui.horizontal_wrapped(|ui| {
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
                self.start(&ctx, &inv, true);
            }
        });
        ui.add_space(4.0);
        widgets::heading(ui, "Output");
        ui.horizontal(|ui| {
            for (shown, label) in [(Shown::Server, "Server"), (Shown::Setup, "Setup"), (Shown::Check, "Check")] {
                ui.selectable_value(&mut self.host.shown, shown, label);
            }
        });
        let lines = self.host.lines();
        egui::ScrollArea::vertical()
            .max_height(ui.available_height().max(120.0))
            .stick_to_bottom(true)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                    for line in lines.iter().rev().take(400).rev() {
                        let text = RichText::new(host::compact(line)).monospace().size(11.0).color(theme::TEXT_SOFT);
                        ui.add(egui::Label::new(text).wrap());
                    }
                });
            });
    }

    fn host_fields(&mut self, ui: &mut Ui, enabled: bool) {
        let drops = self.host.drops.clone();
        let maps = self.host.maps.clone();
        let mut read_again = false;
        let h = &mut self.settings.host;
        ui.add_enabled_ui(enabled, |ui| {
            egui::Grid::new("host_fields").num_columns(2).spacing([16.0, 6.0]).show(ui, |ui| {
                widgets::field(ui, "Name");
                ui.add(egui::TextEdit::singleline(&mut h.name).char_limit(40).desired_width(240.0));
                ui.end_row();
                widgets::field(ui, "Drop");
                match &drops {
                    Some(Ok(list)) => {
                        let combo =
                            egui::ComboBox::from_id_salt("host_drop").selected_text(h.drop.clone()).width(240.0);
                        widgets::combo(ui, combo, |ui| {
                            for d in list {
                                ui.selectable_value(&mut h.drop, d.clone(), d);
                            }
                        });
                    }
                    Some(Err(e)) => widgets::status(ui, theme::ERROR, format!("The drops do not read: {e}")),
                    None => widgets::note(ui, "(the drops come from the game data once it is set up)"),
                }
                ui.end_row();
                widgets::field(ui, "Map");
                read_again = host_map_fields(ui, h, maps.as_ref());
                ui.end_row();
                widgets::field(ui, "Players");
                ui.add(egui::DragValue::new(&mut h.max_players).range(1..=64));
                ui.end_row();
                widgets::field(ui, "Password");
                ui.add(egui::TextEdit::singleline(&mut h.password).password(true).char_limit(64).desired_width(240.0));
                ui.end_row();
                widgets::field(ui, "Port");
                ui.add(egui::DragValue::new(&mut h.port).range(1..=65534));
                ui.end_row();
                widgets::field(ui, "Listen on");
                let current = Listen::ALL.iter().find(|(v, _)| *v == h.listen).map_or("", |(_, t)| *t);
                let combo = egui::ComboBox::from_id_salt("host_listen").selected_text(current).width(240.0);
                widgets::combo(ui, combo, |ui| {
                    for (v, t) in Listen::ALL {
                        ui.selectable_value(&mut h.listen, v, t);
                    }
                });
                ui.end_row();
            });
            ui.checkbox(&mut h.lan, "Answer the LAN tab and the server query");
            if h.listen == Listen::Local {
                ui.add_enabled(false, egui::Checkbox::new(&mut false, "Listed on the public server list"));
                widgets::note(ui, "A server for this computer only is not listed.");
            } else {
                ui.checkbox(&mut h.public, "Listed on the public server list");
            }
            ui.checkbox(&mut h.port_mapping, "Map the ports on my router (UPnP, PCP, NAT-PMP)");
        });
        for p in host::problems(h) {
            widgets::status(ui, theme::WARN, p);
        }
        if read_again {
            self.host.maps = None;
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
                "Set up or repair the game data first (Settings, Game data), or select the disc image there: the \
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

fn host_map_fields(ui: &mut Ui, h: &mut host::HostSettings, maps: Option<&host::HostMaps>) -> bool {
    let Some(maps) = maps else {
        widgets::note(ui, "(reading the installed maps)");
        return false;
    };
    let mut again = false;
    ui.vertical(|ui| {
        let current = h.map.as_ref().map_or_else(
            || "None (the drop above)".to_string(),
            |m| {
                maps.maps
                    .iter()
                    .find(|x| x.id == m.id)
                    .map_or_else(|| m.id.clone(), |x| format!("{} ({})", x.title, x.id))
            },
        );
        widgets::combo(ui, egui::ComboBox::from_id_salt("host_map").selected_text(current).width(240.0), |ui| {
            if ui.selectable_label(h.map.is_none(), "None (the drop above)").clicked() {
                h.map = None;
            }
            for m in &maps.maps {
                let chosen = h.map.as_ref().is_some_and(|x| x.id == m.id);
                if ui.selectable_label(chosen, format!("{} ({})", m.title, m.id)).clicked() && !chosen {
                    let mode = m.modes.first().cloned().unwrap_or_default();
                    h.map = Some(host::HostMap { id: m.id.clone(), mode });
                }
            }
        });
        if let Some(chosen) = h.map.as_mut()
            && let Some(m) = maps.maps.iter().find(|x| x.id == chosen.id)
        {
            let modes = maps.modes_of(m);
            let shown =
                modes.iter().find(|(v, _)| *v == chosen.mode).map_or_else(|| chosen.mode.clone(), |x| x.1.clone());
            let combo = egui::ComboBox::from_id_salt("host_map_mode").selected_text(shown).width(240.0);
            widgets::combo(ui, combo, |ui| {
                for (value, label) in &modes {
                    ui.selectable_value(&mut chosen.mode, value.clone(), label);
                }
            });
        }
        if maps.maps.is_empty() {
            widgets::note(ui, "(no map installed: the Mods page installs map packages)");
        }
        for why in maps.unusable.iter().take(3) {
            widgets::status(ui, theme::WARN, format!("Cannot host {why}"));
        }
        again = widgets::button(ui, "Read the installed maps again", Kind::Plain, true).clicked();
    });
    again
}
