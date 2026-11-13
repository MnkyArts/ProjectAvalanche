// PORT(choice): two noncharacters as the layout's in-band isolate marks (Unicode reserves them for a program's internal
// use; no text from outside reaches the layout with them, `shown` writes `?` for one); the original has no fallback
// fonts.
pub const OPEN: char = '\u{FDD0}';
pub const CLOSE: char = '\u{FDD1}';

pub fn is_mark(c: char) -> bool {
    c == OPEN || c == CLOSE
}

pub fn wrap(text: &str) -> String {
    let mut s = String::with_capacity(text.len() + 6);
    s.push(OPEN);
    s.push_str(text);
    s.push(CLOSE);
    s
}

pub fn outside(chars: impl IntoIterator<Item = char>) -> Vec<bool> {
    let mut depth = 0usize;
    chars
        .into_iter()
        .map(|c| {
            if c == OPEN {
                depth = depth.saturating_add(1);
            }
            let inside = depth > 0;
            if c == CLOSE {
                depth = depth.saturating_sub(1);
            }
            inside
        })
        .collect()
}

pub fn strip(text: &str) -> String {
    text.chars().filter(|&c| !is_mark(c)).collect()
}
