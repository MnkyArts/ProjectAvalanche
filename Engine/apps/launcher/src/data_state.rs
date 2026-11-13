use eframe::egui::Color32;
use ssx_assets::folder::State;

use crate::ui::theme;

const DETAIL_CHARS: usize = 160;

pub fn status(state: &State) -> (Color32, &'static str) {
    match state {
        State::Complete { .. } => (theme::GOOD, "Ready"),
        State::Missing => (theme::WARN, "Not set up"),
        State::Unfinished => (theme::WARN, "Setup unfinished"),
        State::NoIndex | State::IndexDamaged(_) => (theme::WARN, "Repair needed"),
        State::NoMovies | State::MarkerDamaged(_) | State::FilesDamaged(_) => (theme::ERROR, "Repair needed"),
    }
}

pub fn button(state: &State) -> &'static str {
    if state.needs_repair() { "Repair game data" } else { "Extract game data" }
}

pub fn notice(state: &State) -> Option<(Color32, String)> {
    let detail = |why: &str| ssx_core::text::shown_max(why, DETAIL_CHARS).to_string();
    let text = match state {
        State::Complete { .. } | State::Missing => return None,
        State::Unfinished => {
            return Some((
                theme::WARN,
                "An earlier setup into this folder stopped; it resumes where it left off.".into(),
            ));
        }
        State::NoMovies => {
            "This folder was set up by an earlier version: it has no movies and no content index, so the \
             game cannot play its intro, stops after the World Tour tutorial pages and cannot join a server. Select \
             the disc image and click Repair game data: the archives are compared with the image and kept, the \
             movies (about 1.2 GB) are added, the index is made."
                .to_string()
        }
        State::NoIndex => "This folder was set up by an earlier version and has no content index: single player \
             works, but no server can be joined (a join checks the server's levels against the index). Select your \
             disc image and click Repair game data: every file is compared with the image, only what differs is \
             written again, and the index is made from the image."
            .to_string(),
        State::IndexDamaged(why) => format!(
            "The folder's content index is damaged ({}): single player works, but no server can be joined. Select your \
             disc image and click Repair game data: what the index lost is read from the image again.",
            detail(why)
        ),
        State::MarkerDamaged(why) => format!(
            "The folder's setup record is damaged ({}). Select your disc image and click Repair game data: every \
             file is compared with the image, only what differs is written again.",
            detail(why)
        ),
        State::FilesDamaged(why) => format!(
            "Files of this folder are missing or damaged ({}): the game cannot start on it. Select your disc image \
             and click Repair game data: the archives (or the movies) that hold them are read from the image again, \
             only what differs is written again.",
            detail(why)
        ),
    };
    let colour = if state.playable() { theme::WARN } else { theme::ERROR };
    Some((colour, text))
}

pub fn hero_unplayable(state: &State) -> (&'static str, &'static str, &'static str) {
    match state {
        State::NoMovies => (
            "REPAIR GAME DATA",
            "This data folder was set up by an earlier version: select your disc image and repair it.",
            "REPAIR NEEDED",
        ),
        State::MarkerDamaged(_) => (
            "REPAIR GAME DATA",
            "The setup record of this data folder is damaged: repair it from your disc image.",
            "REPAIR NEEDED",
        ),
        State::FilesDamaged(_) => (
            "REPAIR GAME DATA",
            "Files of this data folder are missing or damaged: repair it from your disc image.",
            "REPAIR NEEDED",
        ),
        State::Unfinished => {
            ("SET UP GAME DATA", "The setup stopped before it finished: start it again to resume.", "SETUP NEEDED")
        }
        _ => (
            "SET UP GAME DATA",
            "Choose your SSX (2012) disc image: the game data is unpacked from it once.",
            "SETUP NEEDED",
        ),
    }
}

pub fn menu_play(state: &State) -> &'static str {
    if state.playable() {
        "START THE GAME"
    } else if state.needs_repair() {
        "REPAIR THE GAME DATA FIRST"
    } else {
        "SET UP THE GAME DATA FIRST"
    }
}

pub const HERO_JOIN_REPAIR: &str = "Multiplayer needs a repair of the game data first: Settings, Game data.";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_state_names_its_repair() {
        for state in [
            State::NoIndex,
            State::IndexDamaged("x".into()),
            State::NoMovies,
            State::MarkerDamaged("y".into()),
            State::FilesDamaged("z".into()),
        ] {
            assert_eq!(status(&state).1, "Repair needed", "{state:?}");
            assert_eq!(button(&state), "Repair game data", "{state:?}");
            assert!(notice(&state).is_some_and(|(_, t)| t.contains("Repair game data")), "{state:?}");
        }
        assert_eq!(status(&State::Complete { marked: true, files: 1 }).1, "Ready");
        assert_eq!(button(&State::Missing), "Extract game data");
        assert_eq!(button(&State::Unfinished), "Extract game data");
        assert!(notice(&State::Missing).is_none());
        assert_eq!(hero_unplayable(&State::FilesDamaged("z".into())).0, "REPAIR GAME DATA");
        for state in [State::NoMovies, State::MarkerDamaged("y".into()), State::FilesDamaged("z".into())] {
            assert_eq!(hero_unplayable(&state).0, "REPAIR GAME DATA", "{state:?}");
            assert_eq!(menu_play(&state), "REPAIR THE GAME DATA FIRST", "{state:?}");
        }
        for state in [State::Missing, State::Unfinished] {
            assert_eq!(hero_unplayable(&state).0, "SET UP GAME DATA", "{state:?}");
            assert_eq!(menu_play(&state), "SET UP THE GAME DATA FIRST", "{state:?}");
        }
        assert_eq!(menu_play(&State::NoIndex), "START THE GAME");
    }

    #[test]
    fn a_damage_detail_is_escaped_and_cut() {
        let why = format!("bad {}\u{1b}[2J{}", "x".repeat(400), "\n");
        let (_, text) = notice(&State::IndexDamaged(why)).unwrap();
        assert!(!text.contains('\u{1b}') && !text.contains('\n'), "{text:?}");
        assert!(text.len() < 700, "{}", text.len());
    }
}
