use eframe::egui::Ui;
use ssx_assets::port_options::DiscordPresence;

use crate::app::Launcher;
use crate::ui::details::{self, Detail};
use crate::ui::rows::{self, Row};
use crate::ui::{theme, widgets};

impl Launcher {
    pub fn profile_section(&mut self, ui: &mut Ui) {
        use crate::link_handler::{self, Status};
        ui.spacing_mut().item_spacing.y = 0.0;
        rows::group(ui, "Your name", None);
        let name = self.settings.player_name.clone();
        let (shown, colour) = match &name {
            Some(name) => (widgets::shown(name), theme::TEXT),
            None => ("None set yet.".to_owned(), theme::TEXT_DIM),
        };
        let detail = || {
            Detail::new("Name on servers")
                .text("The name other players see on servers.")
                .text("Set in the game: Options, Player Name.")
                .now(shown.clone(), None)
        };
        rows::info(ui, Row::new("Name on servers"), &shown, colour, detail);

        rows::group(ui, "Server links", None);
        let status = self.link_status.get_or_insert_with(link_handler::status).clone();
        let mut on = status == Status::Ours;
        let available = !matches!(status, Status::Unavailable(_));
        let warn = match &status {
            Status::Other(cmd) => Some(format!("Another program opens these links now: {cmd}")),
            Status::Unavailable(why) => Some(format!("Not available on this system: {why}")),
            Status::Ours | Status::None => None,
        };
        let note = self.link_note.clone();
        let now = on;
        let detail = || {
            let mut d = Detail::new("Open avalanche:// links")
                .text("Open avalanche:// links with Project Avalanche.")
                .text(LINK_NOTE)
                .warn(warn.clone());
            if let Some(note) = note {
                d = d.text(note);
            }
            d.now(if now { "On" } else { "Off" }, None).applies("Changed on the system at once.")
        };
        let row = Row::new("Open avalanche:// links").enabled(available);
        if rows::toggle(ui, row, &mut on, detail) {
            let done = if on { link_handler::register() } else { link_handler::unregister() };
            self.link_status = None;
            self.link_note = Some(match done {
                Ok(what) => what,
                Err(e) => format!("not changed: {e}"),
            });
        }
        if let Some(warn) = &warn {
            widgets::note(ui, warn);
        }
        if let Some(note) = &self.link_note {
            widgets::status(ui, theme::TEXT_SOFT, note.clone());
        }

        rows::group(ui, "Discord", None);
        let s = &mut self.settings;
        let mut on = s.discord_presence == DiscordPresence::On;
        let now = on;
        let detail = || {
            Detail::new("Discord Rich Presence")
                .text("Show what you are playing on your Discord profile.")
                .text("Your Discord friends see Project Avalanche on your profile while you play.")
                .text(DISCORD_NOTE)
                .now(if now { "On" } else { "Off" }, None)
                .applies(details::APPLIES_NEXT_START)
        };
        if rows::toggle(ui, Row::new("Discord Rich Presence"), &mut on, detail) {
            s.discord_presence = if on { DiscordPresence::On } else { DiscordPresence::Off };
        }
    }
}

const DISCORD_NOTE: &str = "While the game runs and the Discord app is open on this computer, your Discord profile \
     shows what you are doing in Project Avalanche: the menus, the mode and event you ride, the drop and its mountain \
     or the map, a timer and your result, with buttons to get the game and to join its Discord server. The game talks \
     only to the Discord app on this computer and sends nothing else: not your name, your save, your rider or the \
     server you play on. Off: the game does not connect to Discord at all. A change takes effect the next time the \
     game starts.";

const LINK_NOTE: &str = if cfg!(windows) {
    "For your user only: the key HKCU\\Software\\Classes\\avalanche with the command \"<this launcher>\" --link \"%1\". \
     Switching it off deletes that key. A link never joins by itself: the game shows the server and asks you first."
} else {
    "For your user only: the file ~/.local/share/applications/project-avalanche-link.desktop and the line \
     x-scheme-handler/avalanche=project-avalanche-link.desktop; in ~/.config/mimeapps.list. Switching it off deletes both. \
     A link never joins by itself: the game shows the server and asks you first."
};
