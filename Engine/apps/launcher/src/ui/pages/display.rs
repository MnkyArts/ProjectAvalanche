use eframe::egui::Ui;
use ssx_assets::display::{Backend, DEFAULT_WINDOW, DisplayList, VideoMode, WindowMode};
use ssx_assets::port_options::FrameLimit;

use crate::app::Launcher;
use crate::ui::details::{self, Detail};
use crate::ui::rows::{self, Row};
use crate::ui::{theme, widgets};

fn mode_label(m: WindowMode) -> &'static str {
    match m {
        WindowMode::Windowed => "Window",
        WindowMode::Borderless => "Borderless fullscreen",
        WindowMode::Exclusive => "Exclusive fullscreen",
    }
}

fn frame_limit_label(l: FrameLimit) -> String {
    match l.fps() {
        None => "Off (as fast as VSync lets it)".to_string(),
        Some(fps) => format!("{fps} fps"),
    }
}

fn pace_note(vsync: bool, limit: FrameLimit) -> &'static str {
    match (vsync, limit.fps()) {
        (true, None) => "VSync on, no limit: a frame every refresh of the display, as fast as it refreshes.",
        (true, Some(_)) => {
            "VSync on with a limit: each frame waits for the limit and then for the display's refresh. Below the \
             refresh rate a limit saves power; one that does not divide the refresh rate (60 fps on 144 Hz) shows \
             its frames for an uneven number of refreshes. At or above the refresh rate a limit saves nothing, and \
             at the refresh rate itself the frames come less evenly than without it: leave it off then."
        }
        (false, None) => "VSync off, no limit: as many frames as the machine draws; the picture may tear.",
        (false, Some(_)) => {
            "VSync off with a limit: the limit alone sets the pace, without the display's wait; the picture may \
             tear. It saves power and heat."
        }
    }
}

fn video_label(v: &VideoMode) -> String {
    format!("{} × {} at {} Hz", v.width, v.height, v.hz())
}

fn no_exclusive(backend: Backend) -> String {
    format!(
        "Exclusive fullscreen is not available on {} (the compositor keeps the display's mode): it starts as \
         borderless fullscreen, at the desktop's resolution.",
        backend.name()
    )
}

const ALT_ENTER: &str = "Alt+Enter in the game switches between the window and the fullscreen mode chosen here. The \
     game keeps the mode you leave it in, the window's size and the render resolution.";
const RENDER_NOTE: &str = "Render resolution is the height the game draws every frame at, in all modes, scaled to the \
     window: a 4K monitor can run at 1080p. The window's own resolution is the sharpest.";
const EXCLUSIVE_NOTE: &str = "Exclusive fullscreen sets the monitor's mode while the game has the focus; switching \
     away (Alt+Tab) gives the desktop its mode back and minimises the game.";

const FRAME_RATE_APPLIES: &str =
    "The frame limit changes at once from the game's Options too; VSync at the next start.";

const WINDOW_SIZES: [(u32, u32); 7] =
    [(3840, 2160), (2560, 1440), (1920, 1080), (1600, 900), (1280, 720), (1024, 768), (960, 540)];

const RENDER_HEIGHTS: [u32; 6] = [2160, 1440, 1080, 900, 720, 540];

fn window_sizes(kept: Option<(u32, u32)>) -> Vec<Option<(u32, u32)>> {
    let mut all: Vec<_> = std::iter::once(None).chain(WINDOW_SIZES.into_iter().map(Some)).collect();
    if let Some(size) = kept.filter(|s| !WINDOW_SIZES.contains(s)) {
        all.push(Some(size));
    }
    all
}

fn window_size_label(size: &Option<(u32, u32)>) -> String {
    match *size {
        None => format!("The game's ({} × {})", DEFAULT_WINDOW.0, DEFAULT_WINDOW.1),
        Some((w, h)) if WINDOW_SIZES.contains(&(w, h)) => format!("{w} × {h}"),
        Some((w, h)) => format!("{w} × {h} (as you left it)"),
    }
}

fn render_heights(kept: Option<u32>) -> Vec<Option<u32>> {
    let mut all: Vec<_> = std::iter::once(None).chain(RENDER_HEIGHTS.into_iter().map(Some)).collect();
    if let Some(h) = kept.filter(|h| !RENDER_HEIGHTS.contains(h)) {
        all.push(Some(h));
    }
    all
}

fn render_height_label(height: &Option<u32>) -> String {
    match *height {
        None => "The window's".to_string(),
        Some(h) if RENDER_HEIGHTS.contains(&h) => format!("{h}p"),
        Some(h) => format!("{h}p (as you left it)"),
    }
}

fn monitors(list: Option<&DisplayList>, chosen: Option<&str>) -> Vec<Option<String>> {
    let mut all = vec![None];
    all.extend(list.map(|l| l.monitors.as_slice()).unwrap_or_default().iter().filter_map(|m| m.name.clone()).map(Some));
    if let Some(name) = chosen.filter(|n| !all.iter().any(|m| m.as_deref() == Some(*n))) {
        all.push(Some(name.to_owned()));
    }
    all
}

fn monitor_label(list: Option<&DisplayList>, monitor: &Option<String>) -> String {
    match monitor.as_deref() {
        None => "The primary monitor".to_string(),
        Some(name) => match list.and_then(|l| l.monitors.iter().find(|m| m.name.as_deref() == Some(name))) {
            Some(m) => widgets::shown(&m.label()),
            None => format!("{} (not connected)", widgets::shown(name)),
        },
    }
}

impl Launcher {
    pub fn display_section(&mut self, ui: &mut Ui) {
        ui.spacing_mut().item_spacing.y = 0.0;
        self.displays.ask(ui.ctx(), false);
        let list = self.displays.list.as_ref().and_then(|l| l.as_ref().ok()).cloned();
        let exclusive = list.as_ref().is_none_or(|l| l.backend.exclusive());
        let backend_note = (!exclusive).then(|| list.as_ref().map(|l| no_exclusive(l.backend))).flatten();
        let s = &mut self.settings.display;
        rows::group(ui, "Window", None);
        let modes: Vec<WindowMode> =
            WindowMode::ALL.into_iter().filter(|m| *m != WindowMode::Exclusive || exclusive).collect();
        let shown = match s.mode {
            WindowMode::Exclusive if !exclusive => "Exclusive fullscreen (borderless here)",
            m => mode_label(m),
        };
        let exclusive_chosen = s.fullscreen == WindowMode::Exclusive && exclusive;
        let detail = || {
            let mut d = Detail::new("Mode").text("How the game's window sits on the screen.").text(ALT_ENTER);
            if exclusive_chosen {
                d = d.text(EXCLUSIVE_NOTE);
            }
            d.warn(backend_note.clone()).now(shown, None).applies(details::APPLIES_GAME)
        };
        let mut mode = s.mode;
        if rows::pick(ui, Row::new("Mode"), &mut mode, &modes, |m| mode_label(*m).to_owned(), detail) {
            *s = s.with_mode(mode);
        }
        let all = monitors(list.as_ref(), s.monitor.as_deref());
        let now = monitor_label(list.as_ref(), &s.monitor);
        let detail = || {
            Detail::new("Monitor")
                .text("The monitor the game opens on, as the game lists them; the primary one when none is chosen.")
                .now(now.clone(), None)
                .applies(details::APPLIES_GAME)
        };
        rows::pick(ui, Row::new("Monitor"), &mut s.monitor, &all, |m| monitor_label(list.as_ref(), m), detail);
        let monitor = list.as_ref().and_then(|l| l.find(s.monitor.as_deref()));
        let listed = |v: &VideoMode| monitor.is_some_and(|m| m.modes.contains(v));
        let video_name = |v: &Option<VideoMode>| match v {
            None => "The monitor's current mode".to_string(),
            Some(v) if listed(v) => video_label(v),
            Some(v) => format!("{} (not listed: the current one)", video_label(v)),
        };
        let videos: Vec<Option<VideoMode>> = std::iter::once(None)
            .chain(monitor.map(|m| m.modes.as_slice()).unwrap_or_default().iter().copied().map(Some))
            .collect();
        let now = video_name(&s.video_mode);
        let unused =
            (!exclusive_chosen).then(|| "Not used with the mode chosen above: exclusive fullscreen only.".to_owned());
        let detail = || {
            Detail::new("Resolution")
                .text("The resolution and refresh rate the monitor switches to in exclusive fullscreen.")
                .warn(unused.clone())
                .now(now.clone(), None)
                .applies(details::APPLIES_GAME)
        };
        let row = Row::new("Resolution").enabled(exclusive_chosen);
        rows::pick(ui, row, &mut s.video_mode, &videos, video_name, detail);
        let sizes = window_sizes(s.window);
        let now = window_size_label(&s.window);
        let detail = || {
            Detail::new("Window size")
                .text("The window's size in Window mode; the window can be resized freely afterwards.")
                .text(ALT_ENTER)
                .now(now.clone(), None)
                .applies(details::APPLIES_GAME)
        };
        rows::pick(ui, Row::new("Window size"), &mut s.window, &sizes, window_size_label, detail);
        let heights = render_heights(s.render_height);
        let now = render_height_label(&s.render_height);
        let detail =
            || Detail::new("Render resolution").text(RENDER_NOTE).now(now.clone(), None).applies(details::APPLIES_GAME);
        rows::pick(ui, Row::new("Render resolution"), &mut s.render_height, &heights, render_height_label, detail);
        for note in &self.settings.display_notes {
            widgets::note(ui, &widgets::shown(note));
        }
        rows::buttons(ui, |ui| {
            if widgets::button(ui, "Find monitors again", widgets::Kind::Plain, !self.displays.asking()).clicked() {
                self.displays.ask(ui.ctx(), true);
            }
        });
        match (&self.displays.list, self.displays.asking()) {
            (_, true) => widgets::status(ui, theme::CYAN_SOFT, "Asking the game for the monitors…"),
            (Some(Err(e)), false) => widgets::status(
                ui,
                theme::ERROR,
                format!("Monitors not listed: {} (the game uses the primary one)", widgets::shown(e)),
            ),
            (Some(Ok(l)), false) => widgets::status(
                ui,
                theme::TEXT_SOFT,
                format!("{} monitor(s) on {}", l.monitors.len(), l.backend.name()),
            ),
            (None, false) => {}
        }
        self.frame_rate_rows(ui);
    }

    fn frame_rate_rows(&mut self, ui: &mut Ui) {
        let s = &mut self.settings;
        rows::group(ui, "Frame rate", None);
        let pace = pace_note(s.vsync, s.frame_limit);
        let vsync = s.vsync;
        rows::toggle(ui, Row::new("VSync"), &mut s.vsync, || {
            Detail::new("VSync")
                .text("On: at most a frame a refresh, no tearing; off: no wait for the display, may tear.")
                .text(pace)
                .now(if vsync { "On" } else { "Off" }, None)
                .applies(FRAME_RATE_APPLIES)
        });
        let now = frame_limit_label(s.frame_limit);
        let detail = || {
            Detail::new("Frame limit")
                .text("The most frames a second the game draws.")
                .text(pace)
                .now(now.clone(), None)
                .applies(FRAME_RATE_APPLIES)
        };
        rows::pick(
            ui,
            Row::new("Frame limit"),
            &mut s.frame_limit,
            &FrameLimit::ALL,
            |l| frame_limit_label(*l),
            detail,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_kept_size_or_height_stays_choosable() {
        assert_eq!(window_sizes(None).len(), 1 + WINDOW_SIZES.len());
        assert_eq!(window_sizes(Some((1920, 1080))).len(), 1 + WINDOW_SIZES.len());
        let kept = window_sizes(Some((1234, 777)));
        assert_eq!(kept.last(), Some(&Some((1234, 777))));
        assert_eq!(window_size_label(&Some((1234, 777))), "1234 × 777 (as you left it)");
        assert_eq!(window_size_label(&None), "The game's (1600 × 900)");
        assert_eq!(render_heights(Some(1000)).last(), Some(&Some(1000)));
        assert_eq!(render_height_label(&None), "The window's");
        assert_eq!(monitors(None, Some("DP-9")), [None, Some("DP-9".to_owned())]);
        assert_eq!(monitor_label(None, &Some("DP-9".to_owned())), "DP-9 (not connected)");
    }
}
