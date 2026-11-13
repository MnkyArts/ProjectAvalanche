#![cfg_attr(windows, windows_subsystem = "windows")]

mod app;
mod args;
mod art;
mod catalog;
mod command;
mod data_state;
mod display;
mod dlc_list;
mod host;
mod jobs;
mod language;
mod link_handler;
mod main_screen;
mod masters;
mod mod_list;
mod settings;
mod settings_flow;
mod setup_flow;
mod tu_list;
mod tuner_page;
mod ui;
mod update;

use std::path::Path;

use eframe::egui;

use crate::ui::{ExtrasTab, MenuItem, ModsTab, SettingsTab, layout};

fn main() -> eframe::Result {
    let started = std::time::Instant::now();
    let words: Vec<String> = std::env::args().skip(1).collect();
    if words.first().is_some_and(|w| w == "--apply-update") {
        std::process::exit(update::install::helper(&words[1..]));
    }
    let args = if link_handler::link_start_asked(&words) {
        match link_handler::link_start(&words) {
            Ok(link) => args::Args { link: Some(link), ..Default::default() },
            Err(reason) => {
                ssx_core::eprintln!("error: the link was not opened: {reason}");
                refused(reason);
                std::process::exit(2);
            }
        }
    } else {
        match args::Args::parse(&words) {
            Ok(args) => args,
            Err(e) => {
                ssx_core::eprintln!("error: {e}");
                std::process::exit(2);
            }
        }
    };
    update::install::cleanup_leftovers();
    let settings = settings::Settings::load();
    if let Some(note) = masters::old_member_note(settings.old_masters) {
        ssx_core::eprintln!("note: {note}");
    }
    if args.print {
        std::process::exit(print(&settings));
    }
    if let Some(dir) = &args.dump_art {
        std::process::exit(dump_art(&settings, dir));
    }
    let data_state = settings.data_state();
    let gallery = art::Gallery::start(app::art_request(&settings, data_state.playable()));
    let mut viewport = egui::ViewportBuilder::default()
        .with_title("Project Avalanche")
        .with_app_id("project-avalanche")
        .with_inner_size(args.window.unwrap_or([layout::CANVAS.x, layout::CANVAS.y]));
    if args.window.is_none() {
        viewport = viewport.with_min_inner_size([960.0, 540.0]);
    }
    let options = eframe::NativeOptions { viewport, ..Default::default() };
    eframe::run_native(
        "Project Avalanche",
        options,
        Box::new(move |cc| {
            ui::theme::install(&cc.egui_ctx);
            let mut app = app::Launcher::new(
                &cc.egui_ctx,
                settings,
                args.screenshot.clone(),
                gallery,
                !args.no_update_check && update::auto_check_enabled(),
                data_state,
            );
            if let Some(page) = args.start_page() {
                app.go(page);
            }
            if let Some(tab) = &args.tab {
                app.settings_tab = SettingsTab::parse(tab).unwrap_or_default();
                app.extras_tab = ExtrasTab::parse(tab).unwrap_or_default();
                app.mods_tab = ModsTab::parse(tab).unwrap_or_default();
            }
            if args.hover.as_deref() == Some(MenuItem::Exit.name()) {
                app.cursor = MenuItem::Exit;
            }
            app.forced_hover = args.hover.clone();
            app.window_pixels = args.window;
            app.scripted_keys = args.keys.iter().copied().collect();
            app.scroll = args.scroll;
            app.timing = std::env::var_os("PROJECT_AVALANCHE_TIMING").map(|_| started);
            app.auto_extract = args.extract;
            app.shot_after_setup = args.after_setup;
            app.auto_launch = args.launch;
            app.auto_launch_pending = args.launch;
            app.auto_play_pending = args.play || args.link.is_some();
            app.link = args.link.clone();
            app.host.script = args.host.iter().copied().collect();
            if let Some(package) = &args.install {
                app.mods.start_install(&cc.egui_ctx, settings::app_dir(), package.clone(), false);
            }
            if let Some(package) = &args.dlc_install {
                let root = crate::dlc_list::DlcPage::root(&settings::app_dir());
                let big = app.data_ready.then(|| app.settings.big_root());
                app.dlc.start_install(&cc.egui_ctx, root, big, package.clone(), false);
            }
            if let Some(file) = &args.tuner_install {
                let root = crate::tuner_page::TunerPage::root(&settings::app_dir());
                app.tuner.start_install(&cc.egui_ctx, root, command::save_path(), file.clone(), false);
            }
            Ok(Box::new(app))
        }),
    )
}

fn refused(reason: String) {
    let viewport = egui::ViewportBuilder::default()
        .with_title("Project Avalanche")
        .with_app_id("project-avalanche")
        .with_inner_size([640.0, 280.0]);
    let options = eframe::NativeOptions { viewport, ..Default::default() };
    let _ = eframe::run_native(
        "Project Avalanche",
        options,
        Box::new(move |cc| {
            ui::theme::install(&cc.egui_ctx);
            Ok(Box::new(Refused { reason }))
        }),
    );
}

struct Refused {
    reason: String,
}

impl eframe::App for Refused {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        ui::theme::NIGHT.to_normalized_gamma_f32()
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::Frame::NONE.inner_margin(24.0).show(ui, |ui| {
            ui::widgets::heading(ui, "This link was not opened");
            ui::widgets::paragraph(ui, &self.reason);
            ui::widgets::note(
                ui,
                "A server link starts Project Avalanche with the link alone. Nothing was started and nothing was \
                 written.",
            );
            ui.add_space(12.0);
            if ui::widgets::button(ui, "Close", ui::widgets::Kind::Primary, true).clicked() {
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
            }
        });
    }
}

fn print(s: &settings::Settings) -> i32 {
    ssx_core::println!("settings {}", settings::Settings::path().display());
    let state = s.data_state();
    ssx_core::println!("game data {} (ready: {}; {state})", s.game_data().display(), state.playable());
    if !state.playable() {
        return 1;
    }
    let big = s.big_root();
    let tracks = catalog::tracks(&big);
    ssx_core::println!("{} tracks: {}", tracks.len(), tracks.join(" "));
    match catalog::riders(&big) {
        Ok(r) => ssx_core::println!(
            "{} riders: {}",
            r.len(),
            r.iter().map(|r| format!("{} ({})", r.name, r.label)).collect::<Vec<_>>().join(", ")
        ),
        Err(e) => ssx_core::println!("riders: {e:#}"),
    }
    match catalog::DropCache::default().get(&big, &s.track) {
        Ok(drops) => {
            for (i, d) in drops.iter().enumerate() {
                ssx_core::println!(
                    "  {} drop {}: {} peak {} lighting {}",
                    s.track,
                    i + 1,
                    d.text_id,
                    d.peak,
                    d.lighting
                );
            }
            ssx_core::println!("  presets: {}", catalog::presets(drops).join(", "));
        }
        Err(e) => ssx_core::println!("  drops: {e}"),
    }
    ssx_core::println!("{}", command::viewer(s, &big).display());
    ssx_core::println!("{}", command::game(s, &big).display());
    0
}

fn dump_art(s: &settings::Settings, dir: &Path) -> i32 {
    if let Err(e) = std::fs::create_dir_all(dir) {
        ssx_core::eprintln!("error: {}: {e}", dir.display());
        return 1;
    }
    let started = std::time::Instant::now();
    let mut failed = false;
    let problems = art::load(&app::art_request(s, s.data_ready()), &mut |l| {
        let file = dir.join(format!("{}.png", l.slot.name()));
        let (w, h) = (l.picture.width, l.picture.height);
        ssx_core::println!(
            "{:>6.0} ms  {:<13} {:?}: {w} x {h}  {}",
            started.elapsed().as_secs_f64() * 1e3,
            l.slot.name(),
            l.source,
            l.note
        );
        if let Err(e) = l.picture.write_png(&file) {
            ssx_core::eprintln!("error: {e:#}");
            failed = true;
        }
    });
    for p in &problems {
        ssx_core::println!("note: {p}");
    }
    failed as i32
}

fn save_png(path: &Path, image: &egui::ColorImage) -> anyhow::Result<()> {
    let [w, h] = image.size;
    let file = std::io::BufWriter::new(std::fs::File::create(path)?);
    let mut enc = png::Encoder::new(file, w as u32, h as u32);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.set_compression(png::Compression::Fast);
    let rgba: Vec<u8> = image.pixels.iter().flat_map(|c| c.to_array()).collect();
    enc.write_header()?.write_image_data(&rgba)?;
    Ok(())
}
