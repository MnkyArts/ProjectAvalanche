use std::fmt;
use std::str::FromStr;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum RangeMovies {
    #[default]
    EveryTime,
    FirstTimeOnly,
}

impl RangeMovies {
    pub const ALL: [RangeMovies; 2] = [RangeMovies::EveryTime, RangeMovies::FirstTimeOnly];
    pub const MEMBER: &str = "range_movies";
    pub const FLAG: &str = "--range-movies";

    pub fn name(self) -> &'static str {
        match self {
            RangeMovies::EveryTime => "EveryTime",
            RangeMovies::FirstTimeOnly => "FirstTimeOnly",
        }
    }

    pub fn from_name(name: &str) -> Option<RangeMovies> {
        RangeMovies::ALL.into_iter().find(|r| r.name() == name)
    }

    pub fn arg(self) -> &'static str {
        match self {
            RangeMovies::EveryTime => "every-time",
            RangeMovies::FirstTimeOnly => "first-time-only",
        }
    }
}

impl fmt::Display for RangeMovies {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.arg())
    }
}

impl FromStr for RangeMovies {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        RangeMovies::ALL.into_iter().find(|r| r.arg() == s).ok_or_else(|| {
            let values: Vec<&str> = RangeMovies::ALL.iter().map(|r| r.arg()).collect();
            format!("{}: one of {}", ssx_core::text::quoted_max(s, 64), values.join(", "))
        })
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum BootMovie {
    #[default]
    EveryStart,
    Skip,
}

impl BootMovie {
    pub const ALL: [BootMovie; 2] = [BootMovie::EveryStart, BootMovie::Skip];
    pub const MEMBER: &str = "boot_movie";
    pub const FLAG: &str = "--boot-movie";

    pub fn name(self) -> &'static str {
        match self {
            BootMovie::EveryStart => "EveryStart",
            BootMovie::Skip => "Skip",
        }
    }

    pub fn from_name(name: &str) -> Option<BootMovie> {
        BootMovie::ALL.into_iter().find(|b| b.name() == name)
    }

    pub fn arg(self) -> &'static str {
        match self {
            BootMovie::EveryStart => "every-start",
            BootMovie::Skip => "skip",
        }
    }

    pub fn skip_intro(self) -> bool {
        self == BootMovie::Skip
    }
}

impl fmt::Display for BootMovie {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.arg())
    }
}

impl FromStr for BootMovie {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        BootMovie::ALL.into_iter().find(|b| b.arg() == s).ok_or_else(|| {
            let values: Vec<&str> = BootMovie::ALL.iter().map(|b| b.arg()).collect();
            format!("{}: one of {}", ssx_core::text::quoted_max(s, 64), values.join(", "))
        })
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum MenuFocus {
    #[default]
    FirstItem,
    LastItem,
}

impl MenuFocus {
    pub const ALL: [MenuFocus; 2] = [MenuFocus::FirstItem, MenuFocus::LastItem];
    pub const MEMBER: &str = "menu_focus";
    pub const FLAG: &str = "--menu-focus";

    pub fn name(self) -> &'static str {
        match self {
            MenuFocus::FirstItem => "FirstItem",
            MenuFocus::LastItem => "LastItem",
        }
    }

    pub fn from_name(name: &str) -> Option<MenuFocus> {
        MenuFocus::ALL.into_iter().find(|r| r.name() == name)
    }

    pub fn arg(self) -> &'static str {
        match self {
            MenuFocus::FirstItem => "first-item",
            MenuFocus::LastItem => "last-item",
        }
    }
}

impl fmt::Display for MenuFocus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.arg())
    }
}

impl FromStr for MenuFocus {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        MenuFocus::ALL.into_iter().find(|r| r.arg() == s).ok_or_else(|| {
            let values: Vec<&str> = MenuFocus::ALL.iter().map(|r| r.arg()).collect();
            format!("{}: one of {}", ssx_core::text::quoted_max(s, 64), values.join(", "))
        })
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum DiscordPresence {
    On,
    // PORT(choice): off is the game's default without the flag (a run that is not the launcher's, a test's or a
    // developer's, never reaches the player's Discord); the launcher's switch is on by default and passes `on`. The
    // console had no Discord.
    #[default]
    Off,
}

impl DiscordPresence {
    pub const ALL: [DiscordPresence; 2] = [DiscordPresence::On, DiscordPresence::Off];
    pub const MEMBER: &str = "discord_presence";
    pub const FLAG: &str = "--discord-presence";

    pub fn name(self) -> &'static str {
        match self {
            DiscordPresence::On => "On",
            DiscordPresence::Off => "Off",
        }
    }

    pub fn from_name(name: &str) -> Option<DiscordPresence> {
        DiscordPresence::ALL.into_iter().find(|d| d.name() == name)
    }

    pub fn arg(self) -> &'static str {
        match self {
            DiscordPresence::On => "on",
            DiscordPresence::Off => "off",
        }
    }
}

impl fmt::Display for DiscordPresence {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.arg())
    }
}

impl FromStr for DiscordPresence {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        DiscordPresence::ALL.into_iter().find(|d| d.arg() == s).ok_or_else(|| {
            let values: Vec<&str> = DiscordPresence::ALL.iter().map(|d| d.arg()).collect();
            format!("{}: one of {}", ssx_core::text::quoted_max(s, 64), values.join(", "))
        })
    }
}

// PORT(choice): the view distance slider (the player's setting, in metres; the console's is fixed at 600 m).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ViewDistance(u32);

impl ViewDistance {
    pub const ORIGINAL: ViewDistance = ViewDistance(600);
    pub const MIN: u32 = 200;
    pub const MAX: u32 = 8000;
    pub const STEP: u32 = 100;
    pub const MEMBER: &str = "view_distance";
    pub const FLAG: &str = "--view-distance";

    pub fn nearest(meters: f64) -> Option<ViewDistance> {
        if !meters.is_finite() {
            return None;
        }
        let clamped = meters.clamp(f64::from(Self::MIN), f64::from(Self::MAX));
        let steps = (clamped / f64::from(Self::STEP)).round();
        Some(ViewDistance((steps as u32 * Self::STEP).clamp(Self::MIN, Self::MAX)))
    }

    pub fn meters(self) -> u32 {
        self.0
    }

    pub fn is_original(self) -> bool {
        self == Self::ORIGINAL
    }
}

impl Default for ViewDistance {
    fn default() -> Self {
        Self::ORIGINAL
    }
}

impl fmt::Display for ViewDistance {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl FromStr for ViewDistance {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.parse::<u32>() {
            Ok(m) if (Self::MIN..=Self::MAX).contains(&m) => Ok(ViewDistance(m)),
            _ => {
                Err(format!("{}: whole metres from {} to {}", ssx_core::text::quoted_max(s, 64), Self::MIN, Self::MAX))
            }
        }
    }
}

// PORT(choice): the frame limiter (the player's setting: off, 30, 60, 120 or 144 frames a second); the Xbox 360's
// game had one display at 60 Hz and no such setting.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum FrameLimit {
    #[default]
    Off,
    Fps30,
    Fps60,
    Fps120,
    Fps144,
}

impl FrameLimit {
    pub const ALL: [FrameLimit; 5] =
        [FrameLimit::Off, FrameLimit::Fps30, FrameLimit::Fps60, FrameLimit::Fps120, FrameLimit::Fps144];
    pub const MEMBER: &str = "frame_limit";
    pub const FLAG: &str = "--frame-limit";

    pub fn fps(self) -> Option<u32> {
        match self {
            FrameLimit::Off => None,
            FrameLimit::Fps30 => Some(30),
            FrameLimit::Fps60 => Some(60),
            FrameLimit::Fps120 => Some(120),
            FrameLimit::Fps144 => Some(144),
        }
    }

    pub fn from_fps(fps: u64) -> Option<FrameLimit> {
        FrameLimit::ALL.into_iter().find(|l| u64::from(l.fps().unwrap_or(0)) == fps)
    }

    pub fn arg(self) -> &'static str {
        match self {
            FrameLimit::Off => "off",
            FrameLimit::Fps30 => "30",
            FrameLimit::Fps60 => "60",
            FrameLimit::Fps120 => "120",
            FrameLimit::Fps144 => "144",
        }
    }

    pub fn period(self) -> Option<std::time::Duration> {
        self.fps().map(|f| std::time::Duration::from_nanos(1_000_000_000 / u64::from(f)))
    }
}

impl fmt::Display for FrameLimit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.arg())
    }
}

impl FromStr for FrameLimit {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        FrameLimit::ALL.into_iter().find(|l| l.arg() == s).ok_or_else(|| {
            let values: Vec<&str> = FrameLimit::ALL.iter().map(|l| l.arg()).collect();
            format!("{}: one of {}", ssx_core::text::quoted_max(s, 64), values.join(", "))
        })
    }
}

// PORT(choice): the resolution scale with FSR 1's four modes (AMD's factors: Ultra Quality 1.3, Quality 1.5, Balanced
// 1.7, Performance 2.0 per axis); the console rendered at 1280 × 720 and its scaler brought that to the TV.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ResolutionScale {
    #[default]
    Native,
    UltraQuality,
    Quality,
    Balanced,
    Performance,
}

impl ResolutionScale {
    pub const ALL: [ResolutionScale; 5] = [
        ResolutionScale::Native,
        ResolutionScale::UltraQuality,
        ResolutionScale::Quality,
        ResolutionScale::Balanced,
        ResolutionScale::Performance,
    ];
    pub const MEMBER: &str = "resolution_scale";
    pub const FLAG: &str = "--resolution-scale";

    pub fn name(self) -> &'static str {
        match self {
            ResolutionScale::Native => "Native",
            ResolutionScale::UltraQuality => "UltraQuality",
            ResolutionScale::Quality => "Quality",
            ResolutionScale::Balanced => "Balanced",
            ResolutionScale::Performance => "Performance",
        }
    }

    pub fn from_name(name: &str) -> Option<ResolutionScale> {
        ResolutionScale::ALL.into_iter().find(|r| r.name() == name)
    }

    pub fn arg(self) -> &'static str {
        match self {
            ResolutionScale::Native => "native",
            ResolutionScale::UltraQuality => "ultra-quality",
            ResolutionScale::Quality => "quality",
            ResolutionScale::Balanced => "balanced",
            ResolutionScale::Performance => "performance",
        }
    }

    pub fn factor(self) -> Option<f64> {
        match self {
            ResolutionScale::Native => None,
            ResolutionScale::UltraQuality => Some(1.3),
            ResolutionScale::Quality => Some(1.5),
            ResolutionScale::Balanced => Some(1.7),
            ResolutionScale::Performance => Some(2.0),
        }
    }

    pub fn render_size(self, display: (u32, u32)) -> (u32, u32) {
        let side = |d: u32| match self.factor() {
            Some(f) => ((f64::from(d) / f).round() as u32).max(1),
            None => d,
        };
        (side(display.0), side(display.1))
    }
}

impl fmt::Display for ResolutionScale {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.arg())
    }
}

impl FromStr for ResolutionScale {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        ResolutionScale::ALL.into_iter().find(|r| r.arg() == s).ok_or_else(|| {
            let values: Vec<&str> = ResolutionScale::ALL.iter().map(|r| r.arg()).collect();
            format!("{}: one of {}", ssx_core::text::quoted_max(s, 64), values.join(", "))
        })
    }
}

// PORT(choice): two lower shadow levels for slower machines, each offered for a measured saving (1920 × 1080 on the
// development laptop's Arc iGPU: Medium 0.52 ms a frame less on alps0101's first Vorassay drop, Low 0.25 ms less GPU):
// the same three cascades (the terrain's LODs and the receiver's pick hold three) over a shorter reach, and at Low a
// 512² map; the console had only the game's 1024² over 400 m.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ShadowQuality {
    #[default]
    Original,
    Medium,
    Low,
}

impl ShadowQuality {
    pub const ALL: [ShadowQuality; 3] = [ShadowQuality::Original, ShadowQuality::Medium, ShadowQuality::Low];
    pub const MEMBER: &str = "shadow_quality";
    pub const FLAG: &str = "--shadow-quality";
    pub const GAME_MAP_SIZE: u32 = 1024;
    pub const GAME_REACH: f32 = 400.0;

    pub fn name(self) -> &'static str {
        match self {
            ShadowQuality::Original => "Original",
            ShadowQuality::Medium => "Medium",
            ShadowQuality::Low => "Low",
        }
    }

    pub fn from_name(name: &str) -> Option<ShadowQuality> {
        ShadowQuality::ALL.into_iter().find(|q| q.name() == name)
    }

    pub fn arg(self) -> &'static str {
        match self {
            ShadowQuality::Original => "original",
            ShadowQuality::Medium => "medium",
            ShadowQuality::Low => "low",
        }
    }

    pub fn map_size(self) -> u32 {
        match self {
            ShadowQuality::Original | ShadowQuality::Medium => ShadowQuality::GAME_MAP_SIZE,
            ShadowQuality::Low => 512,
        }
    }

    pub fn reach(self) -> f32 {
        match self {
            ShadowQuality::Original => ShadowQuality::GAME_REACH,
            ShadowQuality::Medium => 250.0,
            ShadowQuality::Low => 200.0,
        }
    }
}

impl fmt::Display for ShadowQuality {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.arg())
    }
}

impl FromStr for ShadowQuality {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        ShadowQuality::ALL.into_iter().find(|q| q.arg() == s).ok_or_else(|| {
            let values: Vec<&str> = ShadowQuality::ALL.iter().map(|q| q.arg()).collect();
            format!("{}: one of {}", ssx_core::text::quoted_max(s, 64), values.join(", "))
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GraphicsLevers {
    pub resolution_scale: ResolutionScale,
    pub shadow_quality: ShadowQuality,
    pub bloom: bool,
    pub fxaa: bool,
    pub view_distance: ViewDistance,
}

// PORT(choice): presets of the port's own graphics settings, chosen by the player (never by the adapter's name: the
// port has no measurements of other GPUs, and a wrong pick would change the picture unasked); the console had one
// picture.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum GraphicsPreset {
    #[default]
    Original,
    High,
    Medium,
    Low,
}

impl GraphicsPreset {
    pub const ALL: [GraphicsPreset; 4] =
        [GraphicsPreset::Original, GraphicsPreset::High, GraphicsPreset::Medium, GraphicsPreset::Low];

    pub fn name(self) -> &'static str {
        match self {
            GraphicsPreset::Original => "Original",
            GraphicsPreset::High => "High",
            GraphicsPreset::Medium => "Medium",
            GraphicsPreset::Low => "Low",
        }
    }

    pub fn levers(self) -> GraphicsLevers {
        let original = GraphicsLevers {
            resolution_scale: ResolutionScale::Native,
            shadow_quality: ShadowQuality::Original,
            bloom: true,
            fxaa: true,
            view_distance: ViewDistance::ORIGINAL,
        };
        match self {
            GraphicsPreset::Original => original,
            GraphicsPreset::High => GraphicsLevers { resolution_scale: ResolutionScale::UltraQuality, ..original },
            GraphicsPreset::Medium => GraphicsLevers {
                resolution_scale: ResolutionScale::Quality,
                shadow_quality: ShadowQuality::Medium,
                ..original
            },
            GraphicsPreset::Low => GraphicsLevers {
                resolution_scale: ResolutionScale::Performance,
                shadow_quality: ShadowQuality::Low,
                ..original
            },
        }
    }

    pub fn of(levers: &GraphicsLevers) -> Option<GraphicsPreset> {
        GraphicsPreset::ALL.into_iter().find(|p| p.levers() == *levers)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_choices_read_back_by_name_and_by_flag_value() {
        assert_eq!(RangeMovies::default(), RangeMovies::EveryTime);
        for r in RangeMovies::ALL {
            assert_eq!(RangeMovies::from_name(r.name()), Some(r));
            assert_eq!(r.arg().parse::<RangeMovies>(), Ok(r));
            assert_eq!(r.to_string(), r.arg());
        }
        for bad in ["", "everytime", "firsttimeonly", "FIRST-TIME-ONLY", " every-time", "EveryTime", "first"] {
            assert!(bad.parse::<RangeMovies>().is_err(), "{bad:?}");
        }
        for bad in ["", "everyTime", "first-time-only", "FirstTimeOnly ", "Once"] {
            assert_eq!(RangeMovies::from_name(bad), None, "{bad:?}");
        }
        let e = "x\u{1b}[2J".repeat(100).parse::<RangeMovies>().unwrap_err();
        assert!(!e.contains('\u{1b}') && e.len() < 200 && e.ends_with("every-time, first-time-only"), "{e}");
    }

    #[test]
    fn the_boot_movie_choices_read_back_by_name_and_by_flag_value() {
        assert_eq!(BootMovie::default(), BootMovie::EveryStart);
        assert!(!BootMovie::EveryStart.skip_intro() && BootMovie::Skip.skip_intro());
        for b in BootMovie::ALL {
            assert_eq!(BootMovie::from_name(b.name()), Some(b));
            assert_eq!(b.arg().parse::<BootMovie>(), Ok(b));
            assert_eq!(b.to_string(), b.arg());
        }
        for bad in ["", "Skip", "skip ", "SKIP", "every_start", "EveryStart", "off", "true"] {
            assert!(bad.parse::<BootMovie>().is_err(), "{bad:?}");
        }
        for bad in ["", "skip", "every-start", "Skip ", "Off", "true"] {
            assert_eq!(BootMovie::from_name(bad), None, "{bad:?}");
        }
        let e = "x\u{1b}[2J".repeat(100).parse::<BootMovie>().unwrap_err();
        assert!(!e.contains('\u{1b}') && e.len() < 200 && e.ends_with("every-start, skip"), "{e}");
    }

    #[test]
    fn the_menu_focus_reads_back_by_name_and_by_flag_value() {
        assert_eq!(MenuFocus::default(), MenuFocus::FirstItem);
        for m in MenuFocus::ALL {
            assert_eq!(MenuFocus::from_name(m.name()), Some(m));
            assert_eq!(m.arg().parse::<MenuFocus>(), Ok(m));
            assert_eq!(m.to_string(), m.arg());
        }
        for bad in ["", "firstitem", "LAST-ITEM", " last-item", "LastItem", "last", "remember"] {
            assert!(bad.parse::<MenuFocus>().is_err(), "{bad:?}");
        }
        for bad in ["", "firstItem", "last-item", "LastItem ", "Remember"] {
            assert_eq!(MenuFocus::from_name(bad), None, "{bad:?}");
        }
        let e = "x\u{1b}[2J".repeat(100).parse::<MenuFocus>().unwrap_err();
        assert!(!e.contains('\u{1b}') && e.len() < 200 && e.ends_with("first-item, last-item"), "{e}");
    }

    #[test]
    fn the_discord_presence_choices_read_back_by_name_and_by_flag_value() {
        assert_eq!(DiscordPresence::default(), DiscordPresence::Off);
        for d in DiscordPresence::ALL {
            assert_eq!(DiscordPresence::from_name(d.name()), Some(d));
            assert_eq!(d.arg().parse::<DiscordPresence>(), Ok(d));
            assert_eq!(d.to_string(), d.arg());
        }
        for bad in ["", "On", "ON", "true", "1", "yes", " on", "off "] {
            assert!(bad.parse::<DiscordPresence>().is_err(), "{bad:?}");
        }
        for bad in ["", "on", "off", "true", "ON"] {
            assert_eq!(DiscordPresence::from_name(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn the_view_distance_reads_whole_metres_and_clamps_the_files_value() {
        assert_eq!(ViewDistance::default().meters(), 600);
        assert!(ViewDistance::default().is_original());
        assert_eq!("650".parse::<ViewDistance>().map(ViewDistance::meters), Ok(650));
        assert_eq!(ViewDistance::ORIGINAL.to_string().parse::<ViewDistance>(), Ok(ViewDistance::ORIGINAL));
        for bad in ["", "199", "8001", "600.0", "-600", " 600", "6e2", "NaN", "inf", "99999999999"] {
            assert!(bad.parse::<ViewDistance>().is_err(), "{bad:?}");
        }
        let e = "x\u{1b}[2J".repeat(100).parse::<ViewDistance>().unwrap_err();
        assert!(!e.contains('\u{1b}') && e.len() < 200, "{e}");
        let nearest = |m: f64| ViewDistance::nearest(m).map(ViewDistance::meters);
        assert_eq!(
            (nearest(600.0), nearest(649.0), nearest(650.0), nearest(0.0)),
            (Some(600), Some(600), Some(700), Some(200))
        );
        assert_eq!((nearest(-1e300), nearest(1e300), nearest(7990.0)), (Some(200), Some(8000), Some(8000)));
        assert_eq!((nearest(f64::NAN), nearest(f64::INFINITY)), (None, None));
    }

    #[test]
    fn the_frame_limit_reads_back_by_flag_value_and_by_number() {
        assert_eq!(FrameLimit::default(), FrameLimit::Off);
        assert_eq!(FrameLimit::Off.period(), None);
        assert_eq!(FrameLimit::Fps60.period(), Some(std::time::Duration::from_nanos(16_666_666)));
        for l in FrameLimit::ALL {
            assert_eq!(l.arg().parse::<FrameLimit>(), Ok(l));
            assert_eq!(FrameLimit::from_fps(u64::from(l.fps().unwrap_or(0))), Some(l));
            assert_eq!(l.to_string(), l.arg());
        }
        for bad in ["", "Off", "0", "60fps", " 60", "61", "1e2", "-30", "300"] {
            assert!(bad.parse::<FrameLimit>().is_err(), "{bad:?}");
        }
        for bad in [1, 29, 59, 75, 144_000, u64::MAX] {
            assert_eq!(FrameLimit::from_fps(bad), None, "{bad}");
        }
        let e = "x\u{1b}[2J".repeat(100).parse::<FrameLimit>().unwrap_err();
        assert!(!e.contains('\u{1b}') && e.len() < 200 && e.ends_with("off, 30, 60, 120, 144"), "{e}");
    }

    #[test]
    fn the_resolution_scale_reads_back_and_divides_the_screen() {
        assert_eq!(ResolutionScale::default(), ResolutionScale::Native);
        for r in ResolutionScale::ALL {
            assert_eq!(ResolutionScale::from_name(r.name()), Some(r));
            assert_eq!(r.arg().parse::<ResolutionScale>(), Ok(r));
            assert_eq!(r.to_string(), r.arg());
        }
        let sizes: Vec<_> = ResolutionScale::ALL.iter().map(|r| r.render_size((2560, 1440))).collect();
        assert_eq!(sizes, [(2560, 1440), (1969, 1108), (1707, 960), (1506, 847), (1280, 720)]);
        assert_eq!(ResolutionScale::Performance.render_size((1, 1)), (1, 1));
        assert_eq!(ResolutionScale::Performance.render_size((u32::MAX, 3)), (2_147_483_648, 2));
        for bad in ["", "Native", "ultra_quality", "UltraQuality", " quality", "perf", "1.5"] {
            assert!(bad.parse::<ResolutionScale>().is_err(), "{bad:?}");
        }
        for bad in ["", "native", "ultra-quality", "Quality ", "Ultra"] {
            assert_eq!(ResolutionScale::from_name(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn the_presets_are_told_apart_and_original_is_every_default() {
        let original = GraphicsPreset::Original.levers();
        assert_eq!(
            (original.resolution_scale, original.shadow_quality, original.bloom, original.fxaa),
            (ResolutionScale::default(), ShadowQuality::default(), true, true)
        );
        assert!(original.view_distance.is_original());
        for p in GraphicsPreset::ALL {
            assert_eq!(GraphicsPreset::of(&p.levers()), Some(p), "{p:?}");
        }
        let custom = GraphicsLevers { fxaa: false, ..original };
        assert_eq!(GraphicsPreset::of(&custom), None);
        let moved = GraphicsLevers { view_distance: ViewDistance::nearest(800.0).unwrap(), ..original };
        assert_eq!(GraphicsPreset::of(&moved), None);
    }

    #[test]
    fn the_shadow_quality_reads_back_and_original_is_the_games() {
        assert_eq!(ShadowQuality::default(), ShadowQuality::Original);
        for q in ShadowQuality::ALL {
            assert_eq!(ShadowQuality::from_name(q.name()), Some(q));
            assert_eq!(q.arg().parse::<ShadowQuality>(), Ok(q));
            assert_eq!(q.to_string(), q.arg());
        }
        assert_eq!((ShadowQuality::Original.map_size(), ShadowQuality::Original.reach()), (1024, 400.0));
        for w in ShadowQuality::ALL.windows(2) {
            assert!(w[1].map_size() <= w[0].map_size() && w[1].reach() < w[0].reach(), "{w:?}");
        }
        for bad in ["", "Original", "high", " low", "0"] {
            assert!(bad.parse::<ShadowQuality>().is_err(), "{bad:?}");
        }
        for bad in ["", "original", "Low ", "High"] {
            assert_eq!(ShadowQuality::from_name(bad), None, "{bad:?}");
        }
    }
}
