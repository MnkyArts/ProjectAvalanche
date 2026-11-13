pub mod backdrop;
pub mod footer;
pub mod header;
pub mod icons;
pub mod layout;
pub mod menu;
mod pages;
pub mod panel;
pub mod shapes;
pub mod text;
pub mod theme;
pub mod tiles;
pub mod widgets;
mod wordmark;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Page {
    #[default]
    Play,
    Host,
    Mods,
    Settings,
    Extras,
}

impl Page {
    pub const ALL: [Self; 5] = [Self::Play, Self::Host, Self::Mods, Self::Settings, Self::Extras];

    pub fn parse(name: &str) -> Option<Self> {
        match name.to_ascii_lowercase().as_str() {
            "play" => Some(Self::Play),
            "host" => Some(Self::Host),
            "mods" => Some(Self::Mods),
            "settings" => Some(Self::Settings),
            "extras" => Some(Self::Extras),
            _ => None,
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
        Self::Page(Page::Host),
        Self::Page(Page::Mods),
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
            Self::Page(Page::Host) => "menu_host",
            Self::Page(Page::Mods) => "menu_mods",
            Self::Page(Page::Settings) => "menu_settings",
            Self::Page(Page::Extras) => "menu_extras",
            Self::Exit => "menu_exit",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SettingsTab {
    #[default]
    GameData,
    Display,
    Graphics,
    Audio,
    Movies,
    Menus,
    Controls,
    Multiplayer,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ModsTab {
    #[default]
    Installed,
    Report,
}

impl ModsTab {
    pub const ALL: [(Self, &str); 2] = [(Self::Installed, "INSTALLED"), (Self::Report, "BEFORE PLAY")];

    pub fn parse(name: &str) -> Option<Self> {
        match name.to_ascii_lowercase().as_str() {
            "installed" => Some(Self::Installed),
            "report" => Some(Self::Report),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ExtrasTab {
    #[default]
    Tools,
    Output,
    About,
    Files,
    Updates,
}

impl SettingsTab {
    pub const ALL: [(Self, &str); 8] = [
        (Self::GameData, "GAME DATA"),
        (Self::Display, "DISPLAY"),
        (Self::Graphics, "GRAPHICS"),
        (Self::Audio, "AUDIO"),
        (Self::Movies, "MOVIES"),
        (Self::Menus, "MENUS"),
        (Self::Controls, "CONTROLS"),
        (Self::Multiplayer, "MULTIPLAYER"),
    ];

    pub fn parse(name: &str) -> Option<Self> {
        let name = name.to_ascii_lowercase();
        let names = ["data", "display", "graphics", "audio", "movies", "menus", "controls", "multiplayer"];
        names.iter().position(|n| *n == name).map(|i| Self::ALL[i].0)
    }
}

impl ExtrasTab {
    pub const ALL: [(Self, &str); 5] = [
        (Self::Tools, "TOOLS"),
        (Self::Output, "OUTPUT"),
        (Self::About, "ABOUT"),
        (Self::Files, "FILES"),
        (Self::Updates, "UPDATES"),
    ];

    pub fn parse(name: &str) -> Option<Self> {
        let name = name.to_ascii_lowercase();
        ["tools", "output", "about", "files", "updates"].iter().position(|n| *n == name).map(|i| Self::ALL[i].0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_and_tab_names() {
        assert_eq!(Page::parse("play"), Some(Page::Play));
        assert_eq!(Page::parse("Settings"), Some(Page::Settings));
        assert_eq!(Page::parse("setup"), None);
        assert_eq!(SettingsTab::parse("data"), Some(SettingsTab::GameData));
        assert_eq!(SettingsTab::parse("controls"), Some(SettingsTab::Controls));
        assert_eq!(SettingsTab::parse("display"), Some(SettingsTab::Display));
        assert_eq!(SettingsTab::parse("movies"), Some(SettingsTab::Movies));
        assert_eq!(SettingsTab::parse("menus"), Some(SettingsTab::Menus));
        assert_eq!(SettingsTab::parse("multiplayer"), Some(SettingsTab::Multiplayer));
        assert_eq!(ExtrasTab::parse("output"), Some(ExtrasTab::Output));
        assert_eq!(ExtrasTab::parse("updates"), Some(ExtrasTab::Updates));
        assert_eq!(ExtrasTab::parse("graphics"), None);
    }

    #[test]
    fn the_menu_cursor_stops_at_the_ends() {
        let play = MenuItem::Page(Page::Play);
        assert_eq!(play.moved(-1), play);
        assert_eq!(play.moved(1), MenuItem::Page(Page::Host));
        assert_eq!(MenuItem::Page(Page::Extras).moved(1), MenuItem::Exit);
        assert_eq!(MenuItem::Exit.moved(1), MenuItem::Exit);
    }
}
