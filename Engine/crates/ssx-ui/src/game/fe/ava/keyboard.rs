use std::collections::BTreeSet;

use crate::avm1::Ctx;
use crate::avm1::value::Value;
use crate::game::fe::{ActionParam, FeEnv, FrontEnd};

pub const DP: &str = "dp_ava_keyboard";
pub const DP_KEYS: &str = "dp_ava_keyboard_keys";
pub const DP_KEYS_INDEX: &str = "dp_ava_keyboard_keys_index";
pub const OPEN: &str = "ava_keyboard_open";
pub const CLOSE: &str = "ava_keyboard_close";
pub const KEY_ACTION: &str = "A_AvaKeyboardKey";
// PORT(choice): 32 characters when the requester asks for no length; tuning: the fields phase F2 opens the keyboard for
// (server names 40, the chat line, a password); the game has no text entry.
pub const DEFAULT_MAX: usize = 32;
pub const MAX_LEN: usize = 256;
// PORT(choice): a password shows `•` per character when the entry field's font draws it, else `*` (the port's own
// field); the game has no password field.
pub const PASSWORD_MASKS: [char; 2] = ['\u{2022}', '*'];
// PORT(choice): the caret is a steady `|` drawn into the field's text (a drawn caret sprite would need a text field's
// glyph positions, which no AS2 call gives); the game draws no caret anywhere.
pub const CARET: char = '|';
pub const COLUMNS: usize = 10;

const LETTERS: [&str; 4] = ["1234567890", "qwertyuiop", "asdfghjkl'", "zxcvbnm-. "];
const SYMBOLS: [&str; 4] =
    ["!@#$%&*()?", "+=/\\:;\"_~^", "<>[]{}|`,*", "\u{e4}\u{f6}\u{fc}\u{df}\u{e9}\u{e8}\u{e0}\u{e7}\u{f1} "];
const NUMBERS: [&str; 4] = ["1234567890", "abcdef.:[]", "ABCDEF-_/~", "+=*#%,;!? "];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Page {
    #[default]
    Letters,
    Symbols,
    Numbers,
}

impl Page {
    pub fn next(self) -> Page {
        match self {
            Page::Letters => Page::Symbols,
            Page::Symbols => Page::Numbers,
            Page::Numbers => Page::Letters,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    Done(String),
    Cancelled,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Requester {
    Provider(&'static str),
    Host(u32),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextRequest {
    pub requester: Requester,
    pub title: String,
    pub max_len: usize,
    pub password: bool,
    pub initial: String,
    pub page: Page,
}

impl Default for TextRequest {
    fn default() -> Self {
        TextRequest {
            requester: Requester::Host(0),
            title: "TXT_AVA_KEYBOARD_TITLE".into(),
            max_len: DEFAULT_MAX,
            password: false,
            initial: String::new(),
            page: Page::Letters,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextResult {
    pub requester: Requester,
    pub outcome: Outcome,
    pub password: bool,
}

impl TextResult {
    pub fn shown(&self) -> String {
        match &self.outcome {
            Outcome::Done(t) if self.password => {
                format!("Done({} characters) for {:?}", t.chars().count(), self.requester)
            }
            o => format!("{o:?} for {:?}", self.requester),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextInput {
    Char(char),
    Backspace,
    Delete,
    Left,
    Right,
    Home,
    End,
    Enter,
    Escape,
}

#[derive(Clone, Debug, Default)]
pub struct Keyboard {
    pub active: bool,
    pub text: Vec<char>,
    pub caret: usize,
    pub max_len: usize,
    pub caps: bool,
    pub page: Page,
    pub title: String,
    pub password: bool,
    pub request: Option<TextRequest>,
    pub note: String,
    pub glyphs: Option<BTreeSet<char>>,
    pub results: Vec<TextResult>,
}

impl Keyboard {
    pub fn key(&self, index: usize) -> Option<char> {
        let page = match self.page {
            Page::Letters => LETTERS,
            Page::Symbols => SYMBOLS,
            Page::Numbers => NUMBERS,
        };
        let c = page.get(index / COLUMNS)?.chars().nth(index % COLUMNS)?;
        Some(if self.caps && self.page == Page::Letters { c.to_uppercase().next().unwrap_or(c) } else { c })
    }

    fn drawable(&self, c: char) -> bool {
        self.glyphs.as_ref().is_none_or(|g| g.contains(&c))
    }

    pub fn insert(&mut self, c: char) -> bool {
        if self.text.len() >= self.max_len || c.is_control() || !self.drawable(c) {
            return false;
        }
        self.text.insert(self.caret.min(self.text.len()), c);
        self.caret = (self.caret + 1).min(self.text.len());
        true
    }

    pub fn field(&self) -> String {
        let mask = PASSWORD_MASKS.into_iter().find(|&c| self.drawable(c)).unwrap_or('*');
        let shown = |c: &char| if self.password { mask } else { *c };
        let mut s: String = self.text.iter().take(self.caret).map(shown).collect();
        s.push(CARET);
        s.extend(self.text.iter().skip(self.caret).map(shown));
        s
    }
}

impl Keyboard {
    pub fn accept(&mut self, req: TextRequest) -> Result<(), String> {
        if self.active || self.request.is_some() {
            return Err("the keyboard serves another request".into());
        }
        if !(1..=MAX_LEN).contains(&req.max_len) {
            return Err(format!("a length of {} (1 … {MAX_LEN})", req.max_len));
        }
        if req.initial.chars().count() > req.max_len {
            return Err(format!("an initial text longer than {}", req.max_len));
        }
        self.request = Some(req);
        self.note.clear();
        Ok(())
    }

    pub fn begin(&mut self) {
        let req = self.request.get_or_insert_with(TextRequest::default).clone();
        self.active = true;
        self.text.clear();
        self.caret = 0;
        self.caps = false;
        self.page = req.page;
        self.max_len = req.max_len.clamp(1, MAX_LEN);
        self.title = req.title;
        self.password = req.password;
        for c in req.initial.chars() {
            self.insert(c);
        }
    }

    pub fn finish(&mut self, done: bool) -> TextResult {
        let outcome = if done { Outcome::Done(self.text.iter().collect()) } else { Outcome::Cancelled };
        let requester = self.request.take().map_or(Requester::Host(0), |r| r.requester);
        let result = TextResult { requester, outcome, password: self.password };
        self.results.push(result.clone());
        self.text.clear();
        self.caret = 0;
        self.note.clear();
        result
    }

    pub fn end(&mut self) -> Option<TextResult> {
        self.active = false;
        let unanswered = self.request.is_some().then(|| self.finish(false));
        self.text.clear();
        self.caret = 0;
        self.note.clear();
        unanswered
    }

    pub fn withdraw(&mut self, provider: &str) -> Option<bool> {
        let ours =
            self.request.as_ref().is_some_and(|r| matches!(r.requester, Requester::Provider(p) if p == provider));
        if !ours {
            return None;
        }
        if self.active {
            self.finish(false);
            self.take_for(provider);
            Some(true)
        } else {
            self.request = None;
            Some(false)
        }
    }

    pub fn take_for(&mut self, provider: &str) -> Option<Outcome> {
        let i = self.results.iter().position(|r| matches!(r.requester, Requester::Provider(p) if p == provider))?;
        Some(self.results.remove(i).outcome)
    }

    pub fn take_host(&mut self) -> Vec<TextResult> {
        let (host, rest) =
            std::mem::take(&mut self.results).into_iter().partition(|r| matches!(r.requester, Requester::Host(_)));
        self.results = rest;
        host
    }
}

pub fn request(fe: &mut FrontEnd, env: &mut FeEnv, req: TextRequest) -> Result<(), String> {
    let by = req.requester.clone();
    fe.ava.keyboard.accept(req)?;
    fe.note(format!("keyboard: requested by {by:?}"));
    env.nav_events.push(OPEN.into());
    Ok(())
}

pub fn request_with_note(fe: &mut FrontEnd, env: &mut FeEnv, req: TextRequest, note: &str) -> Result<(), String> {
    request(fe, env, req)?;
    fe.ava.keyboard.note = note.to_string();
    Ok(())
}

pub fn withdraw(fe: &mut FrontEnd, env: &mut FeEnv, provider: &str) -> bool {
    let Some(open) = fe.ava.keyboard.withdraw(provider) else { return false };
    env.nav_events.push(CLOSE.into());
    fe.note(format!("keyboard: withdrawn by {provider} ({})", if open { "open" } else { "not opened yet" }));
    true
}

pub fn take_result(fe: &mut FrontEnd, provider: &str) -> Option<Outcome> {
    fe.ava.keyboard.take_for(provider)
}

pub fn activate(fe: &mut FrontEnd) {
    fe.ava.keyboard.begin();
    fe.publish.extend([DP.into(), DP_KEYS.into(), DP_KEYS_INDEX.into()]);
}

pub fn deactivate(fe: &mut FrontEnd) {
    if let Some(r) = fe.ava.keyboard.end() {
        fe.note(format!("keyboard: unloaded unanswered: {}", r.shown()));
    }
}

pub fn action(fe: &mut FrontEnd, env: &mut FeEnv, action: &str, param: &ActionParam) {
    let k = &mut fe.ava.keyboard;
    let mut keys = false;
    match action {
        KEY_ACTION => {
            let index = param.number;
            let typed = (index.is_finite() && index >= 0.0)
                .then(|| k.key(index as usize))
                .flatten()
                .is_some_and(|c| k.insert(c));
            if !typed {
                fe.note(format!("keyboard: key {index} not typed (full, or no glyph)"));
            }
        }
        "A_AvaKeyboardDelete" => edit(k, TextInput::Backspace),
        "A_AvaKeyboardCaretLeft" => edit(k, TextInput::Left),
        "A_AvaKeyboardCaretRight" => edit(k, TextInput::Right),
        "A_AvaKeyboardCaps" => {
            k.caps = !k.caps;
            keys = true;
        }
        "A_AvaKeyboardSymbols" => {
            k.page = k.page.next();
            keys = true;
        }
        "A_AvaKeyboardDone" => return close(fe, env, true),
        "A_AvaKeyboardCancel" => return close(fe, env, false),
        _ => return,
    }
    fe.publish.push(DP.into());
    if keys {
        fe.publish.push(DP_KEYS.into());
    }
}

fn edit(k: &mut Keyboard, input: TextInput) {
    match input {
        TextInput::Char(c) => {
            k.insert(c);
        }
        TextInput::Backspace if k.caret > 0 => {
            k.caret -= 1;
            k.text.remove(k.caret);
        }
        TextInput::Delete if k.caret < k.text.len() => {
            k.text.remove(k.caret);
        }
        TextInput::Left => k.caret = k.caret.saturating_sub(1),
        TextInput::Right => k.caret = (k.caret + 1).min(k.text.len()),
        TextInput::Home => k.caret = 0,
        TextInput::End => k.caret = k.text.len(),
        _ => {}
    }
}

fn close(fe: &mut FrontEnd, env: &mut FeEnv, done: bool) {
    let result = fe.ava.keyboard.finish(done);
    fe.note(format!("keyboard: {}", result.shown()));
    env.nav_events.push(CLOSE.into());
}

pub fn text_input(fe: &mut FrontEnd, env: &mut FeEnv, input: TextInput) {
    if !fe.ava.keyboard.active {
        return;
    }
    match input {
        TextInput::Enter => close(fe, env, true),
        TextInput::Escape => close(fe, env, false),
        i => {
            edit(&mut fe.ava.keyboard, i);
            fe.publish.push(DP.into());
        }
    }
}

pub fn build(fe: &FrontEnd, cx: &mut Ctx, loc: &mut crate::game::loc::Localization, dp: &str) -> Option<Value> {
    let k = &fe.ava.keyboard;
    match dp {
        DP => {
            let o = Value::Object(cx.new_object());
            let _ = cx.set_member(&o, "title", Value::str(&loc.text(&k.title)));
            let _ = cx.set_member(&o, "text", Value::str(&k.field()));
            let _ = cx.set_member(&o, "count", Value::str(&format!("{}/{}", k.text.len(), k.max_len)));
            let note = if k.note.is_empty() { String::new() } else { loc.text(&k.note) };
            let _ = cx.set_member(&o, "note", Value::str(&note));
            Some(o)
        }
        DP_KEYS => {
            let space = loc.text("TXT_AVA_KEYBOARD_SPACE");
            let mut items = Vec::new();
            for i in 0..COLUMNS * 4 {
                let Some(c) = k.key(i) else { continue };
                let o = Value::Object(cx.new_object());
                let label = if c == ' ' { space.clone() } else { c.to_string() };
                let on = k.drawable(c);
                let _ = cx.set_member(&o, "strItem", Value::str(&label));
                let _ = cx.set_member(&o, "bEnabled", Value::Bool(on));
                let _ = cx.set_member(&o, "bFocusable", Value::Bool(true));
                let _ = cx.set_member(&o, "eventAdvance", Value::str(KEY_ACTION));
                let _ = cx.set_member(&o, "eventAdvanceParam", Value::Number(i as f64));
                items.push(o);
            }
            Some(Value::Object(cx.new_array(items)))
        }
        DP_KEYS_INDEX => Some(Value::Number(0.0)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typing_and_editing() {
        let mut k = Keyboard { max_len: 4, ..Default::default() };
        for c in "abcde".chars() {
            k.insert(c);
        }
        assert_eq!(k.field(), "abcd|");
        edit(&mut k, TextInput::Left);
        edit(&mut k, TextInput::Left);
        edit(&mut k, TextInput::Backspace);
        assert_eq!(k.field(), "a|cd");
        edit(&mut k, TextInput::Delete);
        assert_eq!(k.field(), "a|d");
        edit(&mut k, TextInput::End);
        k.insert('\n');
        assert_eq!(k.field(), "ad|");
        k.glyphs = Some("abd".chars().collect());
        assert!(!k.insert('z'));
        assert!(k.insert('b'));
        assert_eq!(k.field(), "adb|");
    }

    #[test]
    fn a_withdrawn_request_leaves_nothing_behind() {
        let req = |requester| TextRequest { requester, max_len: 8, ..TextRequest::default() };
        let mut k = Keyboard::default();
        k.accept(req(Requester::Provider("avachat"))).unwrap();
        k.begin();
        k.insert('h');
        assert_eq!(k.withdraw("avaserverbrowser"), None, "another provider's entry is left alone");
        assert!(k.active && k.request.is_some());
        assert_eq!(k.withdraw("avachat"), Some(true));
        assert!(k.request.is_none() && k.text.is_empty());
        assert_eq!(k.take_for("avachat"), None, "no cancelled result kept for the requester");
        assert!(k.end().is_none(), "the view's unload answers nothing again");
        k.accept(req(Requester::Provider("avachat"))).unwrap();
        assert_eq!(k.withdraw("avachat"), Some(false), "a request not opened yet");
        assert!(k.request.is_none() && !k.active);
        k.accept(req(Requester::Host(3))).unwrap();
        assert_eq!(k.withdraw("avachat"), None, "the host's request is not a provider's");
    }

    #[test]
    fn a_request_is_served_once_and_its_result_goes_to_its_requester() {
        let mut k = Keyboard { glyphs: Some("abc*".chars().collect()), ..Default::default() };
        let req = |requester, max_len, password, initial: &str| TextRequest {
            requester,
            title: "TXT_T".into(),
            max_len,
            password,
            initial: initial.into(),
            page: Page::Letters,
        };
        assert!(k.accept(req(Requester::Host(1), 0, false, "")).is_err());
        assert!(k.accept(req(Requester::Host(1), MAX_LEN + 1, false, "")).is_err());
        assert!(k.accept(req(Requester::Host(1), 2, false, "abc")).is_err());
        k.accept(req(Requester::Provider("avaserverbrowser"), 4, true, "a\u{e9}b")).unwrap();
        assert!(k.accept(req(Requester::Host(2), 4, false, "")).is_err(), "one request at a time");
        k.begin();
        assert_eq!((k.title.as_str(), k.max_len, k.field()), ("TXT_T", 4, "**|".to_string()));
        assert!(k.insert('c') && k.insert('a') && !k.insert('b'));
        let r = k.finish(true);
        assert_eq!(r.shown(), "Done(4 characters) for Provider(\"avaserverbrowser\")");
        assert!(k.take_host().is_empty());
        assert_eq!(k.take_for("avaserverbrowser"), Some(Outcome::Done("abca".into())));
        assert_eq!(k.take_for("avaserverbrowser"), None);
        assert!(k.text.is_empty() && k.request.is_none());
        assert!(k.accept(req(Requester::Host(7), 8, false, "")).is_err());
        assert!(k.end().is_none(), "an answered request is not answered again");
        k.accept(req(Requester::Host(7), 8, false, "")).unwrap();
        k.begin();
        assert_eq!(k.end().map(|r| r.outcome), Some(Outcome::Cancelled));
        assert_eq!(k.take_host().iter().map(|r| r.requester.clone()).collect::<Vec<_>>(), [Requester::Host(7)]);
        assert!(k.end().is_none());
    }

    #[test]
    fn pages_and_capitals() {
        let mut k = Keyboard::default();
        assert_eq!(k.key(10), Some('q'));
        k.caps = true;
        assert_eq!(k.key(10), Some('Q'));
        assert_eq!(k.key(39), Some(' '));
        assert_eq!(k.key(40), None);
        k.page = Page::Symbols;
        assert_eq!(k.key(0), Some('!'));
        k.page = Page::Numbers;
        assert_eq!((k.key(0), k.key(17), k.key(39)), (Some('1'), Some(':'), Some(' ')));
        assert_eq!(Page::Numbers.next(), Page::Letters);
        for page in [LETTERS, SYMBOLS, NUMBERS] {
            assert!(page.iter().all(|r| r.chars().count() == COLUMNS));
        }
    }
}
