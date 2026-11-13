use serde_json::{Map, Value as Json};

mod file;
mod list;
#[cfg(test)]
mod tests;

pub use file::{read_member, write_member};
pub use list::{
    Backend, DisplayList, MAX_LIST_BYTES, MAX_MODES, MAX_MONITORS, MonitorInfo, SCALE_PERMILLE, scale_permille,
};

pub const MEMBER: &str = "display";

pub const MAX_SIDE: u32 = 8192;
pub const MIN_WINDOW: (u32, u32) = (320, 180);
pub const MIN_RENDER_HEIGHT: u32 = 240;
pub const DEFAULT_WINDOW: (u32, u32) = (1600, 900);
pub const MAX_NAME: usize = 128;
pub const REFRESH_MHZ: std::ops::RangeInclusive<u32> = 1_000..=1_000_000;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum WindowMode {
    #[default]
    Windowed,
    Borderless,
    Exclusive,
}

impl WindowMode {
    pub const ALL: [WindowMode; 3] = [WindowMode::Windowed, WindowMode::Borderless, WindowMode::Exclusive];

    pub fn name(self) -> &'static str {
        match self {
            WindowMode::Windowed => "Windowed",
            WindowMode::Borderless => "Borderless",
            WindowMode::Exclusive => "Exclusive",
        }
    }

    pub fn parse(s: &str) -> Option<WindowMode> {
        WindowMode::ALL.into_iter().find(|m| m.name().eq_ignore_ascii_case(s))
    }

    pub fn fullscreen(self) -> bool {
        self != WindowMode::Windowed
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct VideoMode {
    pub width: u32,
    pub height: u32,
    pub refresh_mhz: u32,
}

impl VideoMode {
    pub fn valid(&self) -> bool {
        (1..=MAX_SIDE).contains(&self.width)
            && (1..=MAX_SIDE).contains(&self.height)
            && REFRESH_MHZ.contains(&self.refresh_mhz)
    }

    pub fn parse(s: &str) -> Option<VideoMode> {
        let (size, hz) = s.split_once('@')?;
        let (width, height) = parse_size(size)?;
        let m = VideoMode { width, height, refresh_mhz: parse_millis(hz)? };
        m.valid().then_some(m)
    }

    pub fn hz(&self) -> String {
        let (whole, frac) = (self.refresh_mhz / 1000, self.refresh_mhz % 1000);
        if frac == 0 { format!("{whole}") } else { format!("{whole}.{frac:03}").trim_end_matches('0').to_string() }
    }

    pub(crate) fn from_json(v: &Json) -> Option<VideoMode> {
        let o = v.as_object()?;
        let m = VideoMode {
            width: json_u32(o.get("width")?)?,
            height: json_u32(o.get("height")?)?,
            refresh_mhz: json_u32(o.get("refresh_mhz")?)?,
        };
        m.valid().then_some(m)
    }

    pub(crate) fn to_json(self) -> Json {
        let mut o = Map::new();
        o.insert("width".into(), self.width.into());
        o.insert("height".into(), self.height.into());
        o.insert("refresh_mhz".into(), self.refresh_mhz.into());
        Json::Object(o)
    }
}

impl std::fmt::Display for VideoMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}x{}@{}", self.width, self.height, self.hz())
    }
}

pub fn parse_size(s: &str) -> Option<(u32, u32)> {
    let (w, h) = s.split_once('x')?;
    let side = |t: &str| t.parse::<u32>().ok().filter(|n| (1..=MAX_SIDE).contains(n));
    Some((side(w)?, side(h)?))
}

pub fn window_size_valid((w, h): (u32, u32)) -> bool {
    (MIN_WINDOW.0..=MAX_SIDE).contains(&w) && (MIN_WINDOW.1..=MAX_SIDE).contains(&h)
}

pub fn render_height_valid(height: u32) -> bool {
    (MIN_RENDER_HEIGHT..=MAX_SIDE).contains(&height)
}

// PORT(choice): the render resolution is a height in lines, the width following the window's shape (1080p on 16:9 is
// 1920 x 1080, on 21:9 2560 x 1080); the console had one output resolution.
pub fn render_size(window: (u32, u32), height: Option<u32>) -> Option<(u32, u32)> {
    let height = height.filter(|&h| render_height_valid(h))?;
    let (w, h) = window;
    if w == 0 || h == 0 || height >= h {
        return None;
    }
    let width = ((u64::from(w) * u64::from(height) + u64::from(h) / 2) / u64::from(h)) as u32;
    Some((width.clamp(1, MAX_SIDE), height))
}

fn parse_millis(s: &str) -> Option<u32> {
    let digits = |t: &str| !t.is_empty() && t.len() <= 7 && t.bytes().all(|b| b.is_ascii_digit());
    let (whole, frac) = match s.split_once('.') {
        Some((w, f)) if digits(f) && f.len() <= 3 => (w, f),
        Some(_) => return None,
        None => (s, ""),
    };
    if !digits(whole) {
        return None;
    }
    let millis = whole.parse::<u32>().ok()?.checked_mul(1000)?;
    let scale = match frac.len() {
        0 => return Some(millis),
        1 => 100,
        2 => 10,
        _ => 1,
    };
    millis.checked_add(frac.parse::<u32>().ok()?.checked_mul(scale)?)
}

pub(crate) fn json_u32(v: &Json) -> Option<u32> {
    v.as_u64().and_then(|n| u32::try_from(n).ok())
}

pub fn monitor_name_valid(s: &str) -> bool {
    !s.is_empty() && s.len() <= MAX_NAME && !s.chars().any(char::is_control)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DisplaySettings {
    pub mode: WindowMode,
    pub fullscreen: WindowMode,
    pub monitor: Option<String>,
    pub video_mode: Option<VideoMode>,
    pub window: Option<(u32, u32)>,
    pub render_height: Option<u32>,
}

impl Default for DisplaySettings {
    fn default() -> Self {
        DisplaySettings {
            mode: WindowMode::Windowed,
            fullscreen: WindowMode::Borderless,
            monitor: None,
            video_mode: None,
            window: None,
            render_height: None,
        }
    }
}

impl DisplaySettings {
    pub fn from_json(v: Option<&Json>) -> (DisplaySettings, Vec<String>) {
        let mut s = DisplaySettings::default();
        let mut problems = Vec::new();
        let Some(v) = v.filter(|v| !v.is_null()) else { return (s, problems) };
        let Some(o) = v.as_object() else {
            problems.push(format!("settings.json: `{MEMBER}` is not an object: the defaults (a window) used"));
            return (s, problems);
        };
        let mut bad = |k: &str, kept: &str| {
            problems.push(format!("settings.json: `{MEMBER}.{k}` has a value the game cannot read: {kept}"))
        };
        if let Some(x) = o.get("mode") {
            match x.as_str().and_then(WindowMode::parse) {
                Some(m) => s.mode = m,
                None => bad("mode", "a window"),
            }
        }
        if let Some(x) = o.get("fullscreen") {
            match x.as_str().and_then(WindowMode::parse).filter(|m| m.fullscreen()) {
                Some(m) => s.fullscreen = m,
                None => bad("fullscreen", "borderless"),
            }
        }
        if s.mode.fullscreen() {
            s.fullscreen = s.mode;
        }
        if let Some(x) = o.get("monitor").filter(|x| !x.is_null()) {
            match x.as_str().filter(|n| monitor_name_valid(n)) {
                Some(n) => s.monitor = Some(n.to_string()),
                None => bad("monitor", "the primary monitor"),
            }
        }
        if let Some(x) = o.get("video_mode").filter(|x| !x.is_null()) {
            match VideoMode::from_json(x) {
                Some(m) => s.video_mode = Some(m),
                None => bad("video_mode", "the monitor's current mode"),
            }
        }
        if let Some(x) = o.get("window").filter(|x| !x.is_null()) {
            let size = x.as_object().and_then(|o| Some((json_u32(o.get("width")?)?, json_u32(o.get("height")?)?)));
            match size.filter(|&s| window_size_valid(s)) {
                Some(size) => s.window = Some(size),
                None => bad("window", "the default size"),
            }
        }
        if let Some(x) = o.get("render_height").filter(|x| !x.is_null()) {
            match json_u32(x).filter(|&h| render_height_valid(h)) {
                Some(h) => s.render_height = Some(h),
                None => bad("render_height", "the window's own resolution"),
            }
        }
        (s, problems)
    }

    pub fn to_json(&self) -> Json {
        let mut o = Map::new();
        o.insert("mode".into(), self.mode.name().into());
        o.insert("fullscreen".into(), self.fullscreen.name().into());
        let opt = |v: Option<Json>| v.unwrap_or(Json::Null);
        o.insert("monitor".into(), opt(self.monitor.clone().map(Json::String)));
        o.insert("video_mode".into(), opt(self.video_mode.map(VideoMode::to_json)));
        let window = self.window.map(|(w, h)| {
            let mut m = Map::new();
            m.insert("width".into(), w.into());
            m.insert("height".into(), h.into());
            Json::Object(m)
        });
        o.insert("window".into(), opt(window));
        o.insert("render_height".into(), opt(self.render_height.map(Json::from)));
        Json::Object(o)
    }

    pub fn args(&self) -> Vec<String> {
        let mut out = vec![format!("--window-mode={}", self.mode.name().to_ascii_lowercase())];
        out.push(format!("--fullscreen-mode={}", self.fullscreen.name().to_ascii_lowercase()));
        if let Some(m) = self.monitor.as_deref().filter(|m| monitor_name_valid(m)) {
            out.push(format!("--monitor={m}"));
        }
        if let Some(v) = self.video_mode.filter(VideoMode::valid) {
            out.push(format!("--video-mode={v}"));
        }
        if let Some((w, h)) = self.window.filter(|&s| window_size_valid(s)) {
            out.push(format!("--window-size={w}x{h}"));
        }
        if let Some(h) = self.render_height.filter(|&h| render_height_valid(h)) {
            out.push(format!("--render-height={h}"));
        }
        out
    }

    pub fn with_mode(&self, mode: WindowMode) -> DisplaySettings {
        let fullscreen = if mode.fullscreen() { mode } else { self.fullscreen };
        DisplaySettings { mode, fullscreen, ..self.clone() }
    }

    pub fn toggled(&self) -> DisplaySettings {
        let mode = if self.mode.fullscreen() { WindowMode::Windowed } else { self.fullscreen };
        DisplaySettings { mode, ..self.clone() }
    }
}
