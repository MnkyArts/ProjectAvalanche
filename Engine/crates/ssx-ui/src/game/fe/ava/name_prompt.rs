use super::keyboard::{self, Outcome, Requester, TextRequest};
use super::options;
use super::port_settings::DEFAULT_PLAYER_NAME;
use crate::game::GameUi;
use crate::game::fe::{FeEnv, FrontEnd};

pub const PROVIDER: &str = "avanameprompt";
pub const TITLE: &str = "TXT_AVA_NAME_PROMPT_TITLE";
pub const NOTE: &str = "TXT_AVA_NAME_PROMPT_NOTE";
pub const REFUSED: &str = "ava_name_refused";
pub const BROKEN: &str = "ava_name_settings_broken";

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Stage {
    #[default]
    Off,
    Due,
    Asking,
    Refused {
        text: String,
        shown: bool,
    },
    Named(String),
    Cancelled,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Do {
    Nothing,
    Read,
    Ask(String),
    Save(String),
    Refuse(String),
}

#[derive(Clone, Debug, Default)]
pub struct NamePrompt {
    pub stage: Stage,
}

impl NamePrompt {
    pub fn waiting(&self) -> bool {
        matches!(self.stage, Stage::Due | Stage::Asking | Stage::Refused { .. })
    }

    pub fn step(&mut self, idle: bool, refusal_up: bool, result: Option<Outcome>) -> Do {
        match &mut self.stage {
            Stage::Due if idle => Do::Read,
            Stage::Asking => match result {
                None => Do::Nothing,
                Some(Outcome::Cancelled) => {
                    self.stage = Stage::Cancelled;
                    Do::Nothing
                }
                Some(Outcome::Done(text)) => match ssx_net_proto::name::player_name(&text) {
                    Ok(name) => {
                        self.stage = Stage::Named(name.clone());
                        Do::Save(name)
                    }
                    Err(e) => {
                        self.stage = Stage::Refused { text, shown: false };
                        Do::Refuse(e.to_string())
                    }
                },
            },
            Stage::Refused { shown, .. } if refusal_up => {
                *shown = true;
                Do::Nothing
            }
            Stage::Refused { text, shown: true } if idle => {
                let text = std::mem::take(text);
                self.stage = Stage::Asking;
                Do::Ask(text)
            }
            _ => Do::Nothing,
        }
    }

    pub fn found(&mut self, stored: Option<String>) -> Do {
        match stored {
            Some(name) => {
                self.stage = Stage::Named(name);
                Do::Nothing
            }
            None => {
                self.stage = Stage::Asking;
                Do::Ask(String::new())
            }
        }
    }
}

pub fn join_name(fe: &FrontEnd) -> String {
    name_for_join(fe.ava.confirmed_name.as_deref(), fe.ava.options.settings.player_name.as_deref())
}

fn name_for_join(confirmed: Option<&str>, file: Option<&str>) -> String {
    confirmed.or(file).unwrap_or(DEFAULT_PLAYER_NAME).to_string()
}

pub fn confirm(fe: &mut FrontEnd, name: String) {
    fe.ava.options.settings.player_name = Some(name.clone());
    fe.ava.confirmed_name = Some(name);
    fe.ava.profile_name_due = true;
}

pub const USER_GAMERTAG: u16 = 0x22B;

fn run(fe: &mut FrontEnd, env: &mut FeEnv, ready: bool) {
    let k = &fe.ava.keyboard;
    let free = !k.active && k.request.is_none() && fe.popups.stack.is_empty();
    let idle = ready && fe.ava.hub.active && !fe.in_drop && free;
    let refusal_up = fe.popups.stack.iter().any(|p| p == REFUSED);
    let result = (fe.ava.name_prompt.stage == Stage::Asking).then(|| keyboard::take_result(fe, PROVIDER)).flatten();
    let cancelled = result == Some(Outcome::Cancelled);
    let mut todo = fe.ava.name_prompt.step(idle, refusal_up, result);
    if cancelled {
        fe.note("name prompt: cancelled; nothing written, the default name for this start");
    }
    if todo == Do::Read {
        options::reload(fe);
        todo = fe.ava.name_prompt.found(fe.ava.options.settings.player_name.clone());
        if let Stage::Named(name) = &fe.ava.name_prompt.stage {
            fe.note(format!("name prompt: the settings name {name:?}; not asked"));
        }
    }
    match todo {
        Do::Nothing | Do::Read => {}
        Do::Ask(initial) => {
            let req = TextRequest {
                requester: Requester::Provider(PROVIDER),
                title: TITLE.into(),
                max_len: ssx_net_proto::name::MAX_PLAYER_NAME_CHARS,
                password: false,
                initial,
                page: keyboard::Page::Letters,
            };
            match keyboard::request_with_note(fe, env, req, NOTE) {
                Ok(()) => fe.note("name prompt: asked"),
                Err(e) => {
                    fe.note(format!("name prompt: ERROR the keyboard: {e}; the default name for this start"));
                    fe.ava.name_prompt.stage = Stage::Cancelled;
                }
            }
        }
        Do::Save(name) => {
            fe.note(format!("name prompt: player name {name:?}"));
            confirm(fe, name);
            options::save(fe, env, BROKEN);
        }
        Do::Refuse(why) => {
            fe.note(format!("name prompt: refused: {why}"));
            let body = env.loc.format("TXT_AVA_PLAYER_NAME_REFUSED", &[&why]);
            let glyphs = fe.ava.glyphs.clone();
            env.values.set_str(crate::game::fe::popup::POPUP_BODY, super::shown(&body, glyphs.as_ref()));
            env.nav_events.push(REFUSED.into());
        }
    }
}

impl GameUi {
    pub fn arm_name_prompt(&mut self) {
        if self.port.is_none() {
            return;
        }
        if let Some(fe) = self.fe.as_mut()
            && fe.ava.name_prompt.stage == Stage::Off
        {
            fe.ava.name_prompt.stage = Stage::Due;
            fe.note("name prompt: armed");
        }
    }

    pub fn name_prompt_waiting(&self) -> bool {
        self.fe.as_ref().is_some_and(|f| f.ava.name_prompt.waiting())
    }

    pub fn join_name(&self) -> String {
        self.fe.as_ref().map_or_else(|| DEFAULT_PLAYER_NAME.into(), join_name)
    }

    pub(crate) fn name_prompt_step(&mut self, ready: bool) -> bool {
        if self.name_prompt_waiting() {
            self.fe_call(|fe, env| run(fe, env, ready));
        }
        if let Some(fe) = self.fe.as_mut()
            && std::mem::take(&mut fe.ava.profile_name_due)
        {
            let name = join_name(fe);
            fe.note(format!("name prompt: the profile shows {name:?}"));
            // PORT(choice): the profile's shown name follows the player name the moment it is confirmed; written after
            // the seen flags' clearing (0x8256C9A8), so `userdata`'s next update (0x825BA5E8) sees the change and
            // republishes `dp_user_data` (the dock's `username`); no Live in the port: the game's `USER_gamertag` (UI
            // 0x22B) is written by the sign-in 0x82393A18 from Live's gamertag, unchanged for the session.
            self.values.set_str(USER_GAMERTAG, name);
        }
        self.name_prompt_waiting()
    }
}

#[cfg(test)]
mod tests;
