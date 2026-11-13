use crate::NameError;

const EXTRA: [char; 35] = [
    '\u{0131}', '\u{0141}', '\u{0142}', '\u{0152}', '\u{0153}', '\u{0160}', '\u{0161}', '\u{0178}', '\u{017D}',
    '\u{017E}', '\u{0192}', '\u{02C6}', '\u{02C7}', '\u{02C9}', '\u{02D8}', '\u{02D9}', '\u{02DA}', '\u{02DB}',
    '\u{02DC}', '\u{02DD}', '\u{03A9}', '\u{03BC}', '\u{03C0}', '\u{2013}', '\u{2018}', '\u{2019}', '\u{201C}',
    '\u{201D}', '\u{201E}', '\u{2022}', '\u{2026}', '\u{20AC}', '\u{2122}', '\u{2126}', '\u{221E}',
];

// PORT(choice): 20 characters (an Xbox gamertag's 15 plus room for the port's names); tuning: the width of the HUD's
// ghost-name billboard text field and the players screen's name column at the widest glyphs (W, M, Œ); the original has
// no real-time multiplayer.
pub const MAX_PLAYER_NAME_CHARS: usize = 20;

// PORT(choice): 40 characters, FiveM's cap for a server's name in its list (net1 had 64); tuning: the server browser's
// name column measured at the widest glyphs (W, M, Œ) of its font; the original has no real-time multiplayer.
pub const MAX_SERVER_NAME_CHARS: usize = 40;

pub fn drawable(c: char) -> bool {
    matches!(c, '\u{20}'..='\u{7E}') || (matches!(c, '\u{A1}'..='\u{FF}') && c != '\u{AD}') || EXTRA.contains(&c)
}

pub fn player_name(raw: &str) -> Result<String, NameError> {
    normalize(raw, MAX_PLAYER_NAME_CHARS)
}

pub fn server_name(raw: &str) -> Result<String, NameError> {
    normalize(raw, MAX_SERVER_NAME_CHARS)
}

fn normalize(raw: &str, max: usize) -> Result<String, NameError> {
    if let Some(c) = raw.chars().find(|&c| !drawable(c)) {
        return Err(NameError::Undrawable(c));
    }
    let name = raw.split(' ').filter(|w| !w.is_empty()).collect::<Vec<_>>().join(" ");
    let chars = name.chars().count();
    if chars == 0 {
        return Err(NameError::Empty);
    }
    if chars > max {
        return Err(NameError::TooLong { max, actual: chars });
    }
    Ok(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_set_is_the_fonts_intersection() {
        let all: Vec<char> = (0..=0xFFFFu32).filter_map(char::from_u32).filter(|&c| drawable(c)).collect();
        assert_eq!(all.len(), 224);
        for c in ['\u{0}', '\u{A0}', '\u{AD}', '\u{F8FF}', '\u{2014}', '\t', '\n', '\u{7F}', '\u{9F}', 'Ā', 'Я', '日']
        {
            assert!(!drawable(c), "{c:?}");
        }
        for c in [' ', '~', 'é', 'ÿ', 'Ł', '€', '™', '∞', '–'] {
            assert!(drawable(c), "{c:?}");
        }
    }

    #[test]
    fn spaces_are_trimmed_and_collapsed() {
        assert_eq!(player_name("  Elise   Riggs ").as_deref(), Ok("Elise Riggs"));
        assert_eq!(player_name("Zoë"), Ok("Zoë".into()));
        assert_eq!(player_name("   "), Err(NameError::Empty));
        assert_eq!(player_name(""), Err(NameError::Empty));
    }

    #[test]
    fn undrawable_and_long_names_are_refused() {
        assert_eq!(player_name("Kaori\u{3000}"), Err(NameError::Undrawable('\u{3000}')));
        assert_eq!(player_name("a\tb"), Err(NameError::Undrawable('\t')));
        assert_eq!(player_name("Mac\u{2014}"), Err(NameError::Undrawable('\u{2014}')));
        assert_eq!(player_name(&"x".repeat(20)).map(|n| n.len()), Ok(20));
        assert_eq!(player_name(&"x".repeat(21)), Err(NameError::TooLong { max: 20, actual: 21 }));
        assert_eq!(player_name(&"€".repeat(21)), Err(NameError::TooLong { max: 20, actual: 21 }));
        assert!(server_name(&"Ł".repeat(40)).is_ok());
        assert_eq!(server_name(&"Ł".repeat(41)), Err(NameError::TooLong { max: 40, actual: 41 }));
    }
}
