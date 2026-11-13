mod alt_enter;
mod keeper;
mod list;
mod render_scale;
mod resolve;
#[cfg(test)]
mod tests;

pub(super) use render_scale::RenderScale;

use std::path::{Path, PathBuf};

use anyhow::Result;
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use bevy::window::{
    CursorGrabMode, CursorOptions, Monitor, PresentMode, PrimaryMonitor, PrimaryWindow, WindowFocused, WindowPosition,
    WindowResized, WindowResolution,
};
use ssx_assets::display::{Backend, DisplaySettings, VideoMode, WindowMode};
use ssx_ui::game::display_class::DisplayClass;
use ssx_ui::game::fe::ava::port_settings::WindowChoice;
use winit::raw_window_handle::HasDisplayHandle;

pub(super) use alt_enter::AltEnter;
pub use list::list_displays;
use resolve::{Resolved, Screen, resolve, shown};

/// The display options (`Docs/architecture/game-shell.md` §5, §12); each overrides the launcher's `settings.json`.
#[derive(clap::Args, Clone, Debug, Default)]
pub struct DisplayCli {
    /// The window: `windowed`, `borderless` (fullscreen at the desktop's resolution) or `exclusive` (fullscreen in one
    /// of the monitor's video modes; borderless where the windowing system has none, as Wayland). Default: the
    /// launcher's settings, else windowed.
    #[arg(long, value_name = "MODE", value_parser = mode_arg)]
    pub window_mode: Option<WindowMode>,
    /// The fullscreen Alt+Enter goes to from the window: `borderless` or `exclusive`.
    #[arg(long, value_name = "MODE", value_parser = fullscreen_arg)]
    pub fullscreen_mode: Option<WindowMode>,
    /// The monitor by its name (as `--list-displays` names it); one not connected: the primary.
    #[arg(long, value_name = "NAME", value_parser = monitor_arg)]
    pub monitor: Option<String>,
    /// Exclusive fullscreen's video mode `WxH@HZ` (e.g. `2560x1080@59.94`); default the monitor's current one.
    #[arg(long, value_name = "WxH@HZ", value_parser = video_arg)]
    pub video_mode: Option<VideoMode>,
    /// The window's size in Windowed mode (logical pixels, `WxH`).
    #[arg(long, value_name = "WxH", value_parser = size_arg)]
    pub window_size: Option<(u32, u32)>,
    /// The frame's height in lines it is rendered at before it is scaled to the window, in every mode (e.g. `1080` on
    /// a 4K monitor); default the window's own resolution.
    #[arg(long, value_name = "LINES", value_parser = render_height_arg)]
    pub render_height: Option<u32>,
    /// Print the monitors and the windowing system as JSON (the launcher's Display options read it) and exit.
    #[arg(long)]
    pub list_displays: bool,
}

fn mode_arg(s: &str) -> Result<WindowMode, String> {
    WindowMode::parse(s).ok_or_else(|| "windowed, borderless or exclusive".into())
}

fn fullscreen_arg(s: &str) -> Result<WindowMode, String> {
    WindowMode::parse(s).filter(|m| m.fullscreen()).ok_or_else(|| "borderless or exclusive".into())
}

fn monitor_arg(s: &str) -> Result<String, String> {
    let ok = ssx_assets::display::monitor_name_valid(s);
    ok.then(|| s.to_string())
        .ok_or_else(|| format!("1 to {} bytes, no control characters", ssx_assets::display::MAX_NAME))
}

fn video_arg(s: &str) -> Result<VideoMode, String> {
    VideoMode::parse(s).ok_or_else(|| "WxH@HZ, e.g. 2560x1080@59.94".into())
}

fn size_arg(s: &str) -> Result<(u32, u32), String> {
    let size = ssx_assets::display::parse_size(s).filter(|&s| ssx_assets::display::window_size_valid(s));
    size.ok_or_else(|| {
        let (w, h, max) =
            (ssx_assets::display::MIN_WINDOW.0, ssx_assets::display::MIN_WINDOW.1, ssx_assets::display::MAX_SIDE);
        format!("WxH from {w}x{h} to {max}x{max}")
    })
}

fn render_height_arg(s: &str) -> Result<u32, String> {
    let lines = || format!("{} to {} lines", ssx_assets::display::MIN_RENDER_HEIGHT, ssx_assets::display::MAX_SIDE);
    let h: u32 = s.parse().map_err(|_| lines())?;
    ssx_assets::display::render_height_valid(h).then_some(h).ok_or_else(lines)
}

pub fn print_displays() -> AppExit {
    use std::io::Write as _;
    let written = list_displays().and_then(|list| {
        let line = ssx_core::text::Text::with_max(list.to_json(), ssx_assets::display::MAX_LIST_BYTES);
        let mut out = std::io::stdout().lock();
        let written = writeln!(out, "{line}").and_then(|()| out.flush());
        anyhow::Context::context(written, "writing the list to stdout")
    });
    match written {
        Ok(()) => AppExit::Success,
        Err(e) => {
            ssx_core::eprintln!("error: the monitors could not be listed: {e:#}");
            AppExit::error()
        }
    }
}

#[derive(Clone, Debug)]
pub struct WindowSpec {
    pub width: u32,
    pub height: u32,
    pub physical: bool,
    pub vsync: bool,
    pub position: Option<(i32, i32)>,
    pub display: DisplaySettings,
    pub file: Option<PathBuf>,
    pub notes: Vec<String>,
}

impl WindowSpec {
    pub fn new(window: Option<&str>, cli: &DisplayCli, vsync: bool, file: Option<&Path>) -> Result<WindowSpec> {
        let (mut display, notes, file) = match (window, file) {
            (None, Some(f)) => {
                let (d, notes) = ssx_assets::display::read_member(f);
                (d, notes, Some(f.to_path_buf()))
            }
            _ => (DisplaySettings::default(), Vec::new(), None),
        };
        if let Some(m) = cli.fullscreen_mode {
            display.fullscreen = m;
        }
        if let Some(m) = cli.window_mode {
            display.mode = m;
        }
        if display.mode.fullscreen() {
            display.fullscreen = display.mode;
        }
        if let Some(m) = &cli.monitor {
            display.monitor = Some(m.clone());
        }
        if let Some(v) = cli.video_mode {
            display.video_mode = Some(v);
        }
        if let Some(s) = cli.window_size {
            display.window = Some(s);
        }
        if let Some(h) = cli.render_height {
            display.render_height = Some(h);
        }
        let ((width, height), physical) = match window {
            Some(s) => {
                let size = ssx_assets::display::parse_size(s);
                (size.ok_or_else(|| anyhow::anyhow!("--window {s}: WxH, each side 1 … 8192"))?, true)
            }
            None => (display.window.unwrap_or((crate::WINDOW_SIZE.x, crate::WINDOW_SIZE.y)), false),
        };
        Ok(WindowSpec { width, height, physical, vsync, position: None, display, file, notes })
    }

    pub fn expected_display(&self) -> DisplayClass {
        match self.display.mode {
            WindowMode::Windowed => DisplayClass::of_window(self.width, self.height),
            _ => DisplayClass::WIDESCREEN,
        }
    }

    pub fn window(&self, title: &str) -> Window {
        let resolution = if self.physical {
            WindowResolution::new(self.width, self.height).with_scale_factor_override(1.0)
        } else {
            UVec2::new(self.width, self.height).into()
        };
        let deferred = self.display.mode.fullscreen() || self.display.monitor.is_some();
        Window {
            title: title.into(),
            resolution,
            present_mode: if self.vsync { PresentMode::default() } else { PresentMode::AutoNoVsync },
            position: self.position.map_or(WindowPosition::Automatic, |(x, y)| WindowPosition::At(IVec2::new(x, y))),
            visible: !deferred,
            ..default()
        }
    }
}

pub fn alt_held(keys: &ButtonInput<KeyCode>) -> bool {
    keys.any_pressed([KeyCode::AltLeft, KeyCode::AltRight])
}

const MONITOR_WAIT: u32 = 30;
const SETTLE: f64 = 1.0;

#[derive(Resource)]
pub struct DisplayState {
    pub settings: DisplaySettings,
    kept: DisplaySettings,
    keeper: Option<keeper::Keeper>,
    physical: bool,
    follow_focus: bool,
    notes: Vec<String>,
    applied: Option<Resolved>,
    waited: u32,
    backend: Option<Backend>,
    suspended: bool,
    changed_at: f64,
    requests: Vec<(DisplaySettings, &'static str)>,
}

impl DisplayState {
    pub fn request(&mut self, settings: DisplaySettings, by: &'static str) {
        self.requests.push((settings, by));
    }

    pub fn effective(&self) -> Option<WindowMode> {
        self.applied.as_ref().map(|r| r.effective)
    }
}

pub fn register(app: &mut App, spec: &WindowSpec, keep: bool, follow_focus: bool) {
    let mut notes = spec.notes.clone();
    if !keep && spec.file.is_some() {
        notes.push("a test run: the window's state is not kept in settings.json".into());
    }
    let keeper = match spec.file.clone().filter(|_| keep).map(|f| keeper::Keeper::start(f, spec.display.clone())) {
        Some(Ok(k)) => Some(k),
        Some(Err(e)) => {
            notes.push(format!("no thread for the settings' writes ({e}): the window's state is not kept"));
            None
        }
        None => None,
    };
    app.insert_resource(DisplayState {
        settings: spec.display.clone(),
        kept: spec.display.clone(),
        keeper,
        physical: spec.physical,
        follow_focus,
        notes,
        applied: None,
        waited: 0,
        backend: None,
        suspended: false,
        changed_at: 0.0,
        requests: Vec::new(),
    });
    alt_enter::register(app);
    render_scale::register(app, &spec.display, (spec.width, spec.height));
    app.add_systems(PreUpdate, display_frame.after(bevy::input::InputSystems)).add_systems(Last, display_exit);
}

#[derive(SystemParam)]
struct DisplayParams<'w, 's> {
    alt_enter: Res<'w, AltEnter>,
    windows: Query<'w, 's, (Entity, &'static mut Window, &'static mut CursorOptions), With<PrimaryWindow>>,
    monitors: Query<'w, 's, (Entity, &'static Monitor, Has<PrimaryMonitor>)>,
    focus: MessageReader<'w, 's, WindowFocused>,
    resized: MessageReader<'w, 's, WindowResized>,
    state: ResMut<'w, DisplayState>,
    time: Res<'w, Time<Real>>,
    shell: Option<ResMut<'w, super::Shell>>,
    ui: Option<ResMut<'w, super::GameUiRes>>,
}

fn display_frame(_main_thread: bevy::ecs::system::NonSendMarker, mut p: DisplayParams) {
    let Ok((entity, mut window, mut cursor)) = p.windows.single_mut() else { return };
    let state = &mut *p.state;
    let now = p.time.elapsed_secs_f64();
    for line in std::mem::take(&mut state.notes) {
        warn!("display: {line}");
    }
    if state.backend.is_none() {
        let handle = |w: &bevy::winit::WinitWindows| w.get_window(entity).map(|w| list::backend(w.display_handle()));
        state.backend = bevy::winit::WINIT_WINDOWS.with_borrow(handle);
        if let Some(b) = state.backend {
            info!("display: the windowing system: {} (exclusive fullscreen {})", b.name(), b.exclusive());
        }
    }
    let screens: Vec<Screen> = p
        .monitors
        .iter()
        .map(|(e, m, primary)| Screen {
            entity: e,
            name: m.name.as_deref(),
            primary,
            size: m.physical_size(),
            modes: &m.video_modes,
        })
        .collect();
    if state.applied.is_none() {
        if screens.is_empty() && state.waited < MONITOR_WAIT {
            state.waited += 1;
            return;
        }
        let s = state.settings.clone();
        apply(state, &mut window, &mut cursor, &screens, &s, "the start", now);
        window.visible = true;
    }
    if p.alt_enter.pressed {
        let next = state.settings.toggled();
        state.request(next, "Alt+Enter");
    }
    let mut options = p.ui.as_deref_mut().and_then(|u| u.fe.as_mut()).map(|fe| &mut fe.ava.options);
    if let Some(next) = options.as_deref_mut().and_then(|o| o.window_request.take()) {
        state.request(next, "the Options");
    }
    for (next, by) in std::mem::take(&mut state.requests) {
        state.settings = next.clone();
        state.suspended = false;
        apply(state, &mut window, &mut cursor, &screens, &next, by, now);
        keep(state);
    }
    if let Some(o) = options {
        let exclusive = state.backend.is_none_or(Backend::exclusive);
        o.host_window = Some(WindowChoice { settings: state.settings.clone(), exclusive });
    }
    focus_frame(state, &mut p.focus, entity, &mut window, &mut cursor, &screens, now);
    let gone = state.applied.as_ref().and_then(|r| r.monitor.filter(|_| r.effective.fullscreen()));
    if let Some(m) = gone.filter(|m| !screens.is_empty() && !screens.iter().any(|s| s.entity == *m)) {
        warn!("display: the window's monitor ({m}) is gone: placed again");
        let s = state.settings.clone();
        apply(state, &mut window, &mut cursor, &screens, &s, "a monitor gone", now);
    }
    if p.resized.read().any(|r| r.window == entity) {
        size_frame(state, entity, &window, now);
    }
    let size = window.physical_size();
    if let Some(shell) = p.shell.as_deref_mut()
        && size.x > 0
        && size.y > 0
    {
        shell.aspect = size.x as f32 / size.y as f32;
    }
}

const DISPLAY_SETTLE: f64 = 0.5;

pub(super) fn display_class_frame(
    pending: &mut Option<(DisplayClass, f64)>,
    started: bool,
    ui: &mut ssx_ui::game::GameUi,
    scene: &mut crate::ui::UiScene,
    window: &Window,
    now: f64,
) {
    let (w, h) = (window.physical_width(), window.physical_height());
    if w == 0 || h == 0 {
        return;
    }
    if let Some(vm) = scene.vm.as_mut() {
        let extra = ssx_ui::game::display_class::hud_extra_for(w, h);
        ui.set_hud_extra(vm, &mut scene.lib, &mut scene.player, extra);
    }
    let Some(class) = display_due(pending, DisplayClass::of_window(w, h), ui.display, started, now) else { return };
    let Some(vm) = scene.vm.as_mut() else { return };
    *pending = None;
    let (tw, th) = class.size();
    info!("display: the window is {w} × {h}: the movies are told {tw} × {th} (widescreen {})", class.widescreen);
    ui.set_display(vm, &mut scene.lib, &mut scene.player, class);
}

fn display_due(
    pending: &mut Option<(DisplayClass, f64)>,
    class: DisplayClass,
    told: DisplayClass,
    started: bool,
    now: f64,
) -> Option<DisplayClass> {
    if class == told {
        *pending = None;
        return None;
    }
    let since = match *pending {
        Some((c, at)) if c == class => at,
        _ => now,
    };
    *pending = Some((class, since));
    (!started || now - since >= DISPLAY_SETTLE).then_some(class)
}

fn apply(
    state: &mut DisplayState,
    window: &mut Window,
    cursor: &mut CursorOptions,
    screens: &[Screen],
    s: &DisplaySettings,
    by: &str,
    now: f64,
) {
    let r = resolve(s, screens, state.backend);
    for note in &r.notes {
        warn!("display: {note}");
    }
    let leaving = state.applied.as_ref().is_some_and(|a| a.effective.fullscreen());
    window.mode = r.mode;
    if r.effective == WindowMode::Windowed {
        if let Some(at) = r.position {
            window.position = at;
        }
        if leaving && !state.physical {
            let (w, h) = s.window.unwrap_or((crate::WINDOW_SIZE.x, crate::WINDOW_SIZE.y));
            window.resolution.set(w as f32, h as f32);
        }
    }
    // PORT(choice): the pointer is kept in the window in exclusive fullscreen (a click on another monitor would take
    // the focus and leave the video mode); free in borderless fullscreen and in the window; the console has no pointer.
    let grab = if r.effective == WindowMode::Exclusive { CursorGrabMode::Confined } else { CursorGrabMode::None };
    if cursor.grab_mode != grab {
        cursor.grab_mode = grab;
    }
    let on = screens.iter().find(|m| Some(m.entity) == r.monitor);
    info!(
        "display: {by}: {} on {}{}",
        describe(r.effective, r.video),
        on.map_or_else(
            || "the primary monitor".into(),
            |m| match m.name {
                Some(n) => format!("the monitor {} ({} × {})", shown(n), m.size.x, m.size.y),
                None => format!("an unnamed monitor ({} × {})", m.size.x, m.size.y),
            }
        ),
        if r.notes.is_empty() { "" } else { " (see above)" }
    );
    state.applied = Some(r);
    state.changed_at = now;
}

fn describe(mode: WindowMode, video: Option<VideoMode>) -> String {
    match (mode, video) {
        (WindowMode::Windowed, _) => "a window".into(),
        (WindowMode::Borderless, _) => "borderless fullscreen".into(),
        (WindowMode::Exclusive, Some(v)) => {
            format!("exclusive fullscreen at {} × {}, {} Hz", v.width, v.height, v.hz())
        }
        (WindowMode::Exclusive, None) => "exclusive fullscreen at the monitor's current mode".into(),
    }
}

// PORT(choice): exclusive fullscreen is left and the window minimised when the focus goes (as DXGI's own exclusive
// fullscreen does on Windows; winit 0.30.13 switches the video mode with `ChangeDisplaySettingsExW` / XRandR and keeps
// it when the focus goes); the console has no window.
fn focus_frame(
    state: &mut DisplayState,
    focus: &mut MessageReader<WindowFocused>,
    entity: Entity,
    window: &mut Window,
    cursor: &mut CursorOptions,
    screens: &[Screen],
    now: f64,
) {
    for f in focus.read().filter(|f| f.window == entity) {
        if !state.follow_focus {
            continue;
        }
        let exclusive = state.effective() == Some(WindowMode::Exclusive);
        if !f.focused && exclusive && !state.suspended && now - state.changed_at > SETTLE {
            state.suspended = true;
            window.mode = bevy::window::WindowMode::Windowed;
            window.set_minimized(true);
            cursor.grab_mode = CursorGrabMode::None;
            info!(
                "display: the focus is gone in exclusive fullscreen: the desktop's video mode back, the window minimised"
            );
        } else if f.focused && state.suspended {
            state.suspended = false;
            let s = state.settings.clone();
            apply(state, window, cursor, screens, &s, "the focus back", now);
        }
    }
}

fn size_frame(state: &mut DisplayState, entity: Entity, window: &Window, now: f64) {
    if state.physical || state.effective() != Some(WindowMode::Windowed) || now - state.changed_at <= SETTLE {
        return;
    }
    let maximised = bevy::winit::WINIT_WINDOWS.with_borrow(|w| w.get_window(entity).map(|w| w.is_maximized()));
    if maximised != Some(false) {
        return;
    }
    let size = window.size().round();
    let size = (size.x as u32, size.y as u32);
    if ssx_assets::display::window_size_valid(size) && state.settings.window != Some(size) {
        state.settings.window = Some(size);
    }
}

fn keep(state: &mut DisplayState) {
    let Some(keeper) = &state.keeper else { return };
    if state.settings == state.kept {
        return;
    }
    keeper.keep(state.settings.clone());
    state.kept = state.settings.clone();
}

fn display_exit(mut exit: MessageReader<AppExit>, mut state: ResMut<DisplayState>) {
    if exit.read().next().is_some() {
        keep(&mut state);
        if let Some(keeper) = &mut state.keeper {
            keeper.finish();
        }
    }
}
