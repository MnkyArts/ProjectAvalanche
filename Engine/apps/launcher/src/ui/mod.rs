pub mod backdrop;
pub mod details;
pub mod footer;
pub mod header;
pub mod icons;
pub mod layout;
pub mod menu;
pub mod nav;
mod pages;
pub mod panel;
pub mod prompts;
pub mod rows;
pub mod shapes;
pub mod text;
pub mod theme;
pub mod tiles;
pub mod titlebar;
pub mod widgets;
mod wordmark;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Page {
    #[default]
    Play,
    Online,
    Content,
    Settings,
    Extras,
}

impl Page {
    pub const ALL: [Self; 5] = [Self::Play, Self::Online, Self::Content, Self::Settings, Self::Extras];

    pub const NAMES: [&str; 5] = ["play", "online", "content", "settings", "extras"];

    pub fn parse(name: &str) -> Option<Self> {
        let name = name.to_ascii_lowercase();
        Self::NAMES.iter().position(|n| *n == name).map(|i| Self::ALL[i])
    }

    pub fn title(self) -> &'static str {
        match self {
            Self::Play => "PLAY",
            Self::Online => "ONLINE",
            Self::Content => "CONTENT",
            Self::Settings => "SETTINGS",
            Self::Extras => "EXTRAS",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuItem {
    Page(Page),
    Exit,
}

impl MenuItem {
    pub const ALL: [Self; 6] = [
        Self::Page(Page::Play),
        Self::Page(Page::Online),
        Self::Page(Page::Content),
        Self::Page(Page::Settings),
        Self::Page(Page::Extras),
        Self::Exit,
    ];

    pub fn moved(self, step: isize) -> Self {
        let at = Self::ALL.iter().position(|m| *m == self).unwrap_or(0) as isize;
        Self::ALL[(at + step).clamp(0, Self::ALL.len() as isize - 1) as usize]
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Page(Page::Play) => "menu_play",
            Self::Page(Page::Online) => "menu_online",
            Self::Page(Page::Content) => "menu_content",
            Self::Page(Page::Settings) => "menu_settings",
            Self::Page(Page::Extras) => "menu_extras",
            Self::Exit => "menu_exit",
        }
    }
}

pub trait Section: Copy + PartialEq + Default + 'static {
    const ALL: &'static [(Self, &'static str)];
    const NAMES: &'static [&'static str];

    fn wide(self) -> bool {
        false
    }

    fn parse(name: &str) -> Option<Self> {
        let name = name.to_ascii_lowercase();
        Self::NAMES.iter().position(|n| *n == name).map(|i| Self::ALL[i].0)
    }

    fn label(self) -> &'static str {
        Self::ALL.iter().find(|(s, _)| *s == self).map_or("", |(_, l)| l)
    }

    fn turned(self, step: isize) -> Self {
        let n = Self::ALL.len() as isize;
        let at = Self::ALL.iter().position(|(s, _)| *s == self).unwrap_or(0) as isize;
        Self::ALL[(at + step).rem_euclid(n) as usize].0
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum OnlineTab {
    #[default]
    Host,
    Profile,
}

impl Section for OnlineTab {
    const ALL: &'static [(Self, &'static str)] = &[(Self::Host, "HOST"), (Self::Profile, "PROFILE & LINKS")];
    const NAMES: &'static [&'static str] = &["host", "profile"];
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ContentTab {
    #[default]
    Mods,
    Dlc,
    TitleUpdate,
    GameData,
}

impl Section for ContentTab {
    const ALL: &'static [(Self, &'static str)] =
        &[(Self::Mods, "MODS"), (Self::Dlc, "DLC"), (Self::TitleUpdate, "TITLE UPDATE"), (Self::GameData, "GAME DATA")];
    const NAMES: &'static [&'static str] = &["mods", "dlc", "update", "data"];
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ModsView {
    #[default]
    Installed,
    Mount,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SettingsTab {
    #[default]
    Display,
    Graphics,
    Audio,
    Controls,
    Game,
}

impl Section for SettingsTab {
    const ALL: &'static [(Self, &'static str)] = &[
        (Self::Display, "DISPLAY"),
        (Self::Graphics, "GRAPHICS"),
        (Self::Audio, "AUDIO"),
        (Self::Controls, "CONTROLS"),
        (Self::Game, "GAME"),
    ];
    const NAMES: &'static [&'static str] = &["display", "graphics", "audio", "controls", "game"];

    fn wide(self) -> bool {
        self == Self::Controls
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ExtrasTab {
    #[default]
    Tools,
    Log,
    Files,
    Updates,
    About,
}

impl Section for ExtrasTab {
    const ALL: &'static [(Self, &'static str)] = &[
        (Self::Tools, "TOOLS"),
        (Self::Log, "LOG"),
        (Self::Files, "FILES"),
        (Self::Updates, "UPDATES"),
        (Self::About, "ABOUT"),
    ];
    const NAMES: &'static [&'static str] = &["tools", "log", "files", "updates", "about"];

    fn wide(self) -> bool {
        matches!(self, Self::Log | Self::Files | Self::About)
    }
}

pub fn tab_names(page: Page) -> &'static [&'static str] {
    match page {
        Page::Play => &[],
        Page::Online => OnlineTab::NAMES,
        Page::Content => ContentTab::NAMES,
        Page::Settings => SettingsTab::NAMES,
        Page::Extras => ExtrasTab::NAMES,
    }
}

pub const TAB_MOUNT: &str = "mount";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_and_tab_names() {
        assert_eq!(Page::parse("play"), Some(Page::Play));
        assert_eq!(Page::parse("Settings"), Some(Page::Settings));
        assert_eq!(Page::parse("online"), Some(Page::Online));
        assert_eq!(Page::parse("content"), Some(Page::Content));
        for old in ["setup", "host", "mods"] {
            assert_eq!(Page::parse(old), None, "{old}");
        }
        assert_eq!(OnlineTab::parse("profile"), Some(OnlineTab::Profile));
        assert_eq!(ContentTab::parse("data"), Some(ContentTab::GameData));
        assert_eq!(ContentTab::parse("update"), Some(ContentTab::TitleUpdate));
        assert_eq!(SettingsTab::parse("controls"), Some(SettingsTab::Controls));
        assert_eq!(SettingsTab::parse("game"), Some(SettingsTab::Game));
        assert_eq!(ExtrasTab::parse("log"), Some(ExtrasTab::Log));
        assert_eq!(ExtrasTab::parse("updates"), Some(ExtrasTab::Updates));
        for old in ["output", "movies", "menus", "multiplayer", "discord"] {
            assert_eq!((SettingsTab::parse(old), ExtrasTab::parse(old)), (None, None), "{old}");
        }
        assert_eq!(ExtrasTab::parse("graphics"), None);
        fn named<T: Section + std::fmt::Debug>() {
            assert_eq!(T::ALL.len(), T::NAMES.len());
            for (i, (s, label)) in T::ALL.iter().enumerate() {
                assert_eq!(T::parse(T::NAMES[i]), Some(*s));
                assert_eq!(s.label(), *label);
            }
        }
        named::<OnlineTab>();
        named::<ContentTab>();
        named::<SettingsTab>();
        named::<ExtrasTab>();
    }

    #[test]
    fn sections_turn_round_the_ends() {
        assert_eq!(SettingsTab::Display.turned(1), SettingsTab::Graphics);
        assert_eq!(SettingsTab::Display.turned(-1), SettingsTab::Game);
        assert_eq!(SettingsTab::Game.turned(1), SettingsTab::Display);
        assert_eq!(OnlineTab::Profile.turned(1), OnlineTab::Host);
        assert_eq!(ExtrasTab::About.turned(2), ExtrasTab::Log);
        assert!(SettingsTab::Controls.wide() && !SettingsTab::Display.wide());
        assert!(ExtrasTab::Log.wide() && ExtrasTab::Files.wide() && ExtrasTab::About.wide());
        assert!(!ExtrasTab::Tools.wide() && !ExtrasTab::Updates.wide() && !ContentTab::Mods.wide());
    }

    #[test]
    fn the_menu_cursor_stops_at_the_ends() {
        let play = MenuItem::Page(Page::Play);
        assert_eq!(play.moved(-1), play);
        assert_eq!(play.moved(1), MenuItem::Page(Page::Online));
        assert_eq!(MenuItem::Page(Page::Extras).moved(1), MenuItem::Exit);
        assert_eq!(MenuItem::Exit.moved(1), MenuItem::Exit);
    }
}
