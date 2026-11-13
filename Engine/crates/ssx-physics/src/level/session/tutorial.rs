use crate::objectives::{Effect, GsValue};
use ssx_lua::game::objectives::{Cmp, GsType, game_state_type};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct HudMessage {
    pub requested: u32,
    pub current: u32,
    pub timer: f32,
    pub title: String,
    pub body: String,
}

impl HudMessage {
    pub const SHOWING: u32 = 5;
    pub const READY: u32 = 7;

    pub fn request(&mut self, state: u32, seconds: f32) {
        if state != self.current {
            (self.requested, self.timer) = (state, seconds);
        }
    }

    pub fn update(&mut self, dt: f32) {
        if self.requested == self.current {
            if self.timer > 0.0 {
                self.timer -= dt;
                if self.timer <= 0.0 {
                    self.requested = Self::READY;
                }
            }
            return;
        }
        self.current = self.requested;
    }

    pub fn showing(&self) -> bool {
        self.current == Self::SHOWING
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct TrickRecord {
    pub id: u16,
    pub state: String,
    pub cmp: Cmp,
    pub value: GsValue,
    pub goofy_value: GsValue,
    pub scale: f32,
    pub needed: u32,
    pub count: u32,
    pub text: String,
}

impl TrickRecord {
    fn test(&mut self, current: Option<GsValue>, goofy: bool) -> bool {
        let reference = if goofy { self.goofy_value } else { self.value };
        if current.is_some_and(|c| self.cmp.value(c, reference)) {
            self.count = self.count.wrapping_add(1);
            self.needed <= self.count
        } else {
            self.count = 0;
            false
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum TutorialEvent {
    Message { title: String, body: String, seconds: f32 },
    Popup(String),
    TrickReset(String),
    TrickAdd(TrickRecord),
    TrickShow { button: i32, text: String },
    TrickSuccess,
    TrickFailed(String),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Tutorial {
    pub message: HudMessage,
    pub help: String,
    pub success: Vec<TrickRecord>,
    pub failure: Vec<TrickRecord>,
    pub events: Vec<TutorialEvent>,
}

// PORT(choice): the bound is the port's, to drop once every caller of `Session::execute` takes the events; nothing to
// decode: the game hands each call to the UI at once (0x824D5F40 / 0x824D32B0).
pub const MAX_EVENTS: usize = 256;

impl Default for Tutorial {
    fn default() -> Self {
        let message = HudMessage { requested: HudMessage::READY, current: HudMessage::READY, ..Default::default() };
        Tutorial { message, help: String::new(), success: Vec::new(), failure: Vec::new(), events: Vec::new() }
    }
}

const ID_GOOFY: u16 = 0x66;
pub const ID_FORCE_CAPTURE_SCORING: u16 = 0x14;
pub const ID_FORCE_RESET_SCORING: u16 = 0x16;

impl Tutorial {
    pub fn for_drop(_big_root: &std::path::Path, scripts: &[String]) -> Option<Self> {
        scripts.iter().any(|s| s.starts_with("tutorial") && s.ends_with("/tutorialobjectives.lua")).then(Self::default)
    }

    fn show(&mut self, title: &str, body: &str, seconds: f32) {
        (self.message.title, self.message.body) = (title.to_string(), body.to_string());
        self.message.request(HudMessage::SHOWING, seconds);
    }

    pub fn take_events(&mut self) -> Vec<TutorialEvent> {
        std::mem::take(&mut self.events)
    }

    pub fn effect(&mut self, e: &Effect) {
        if self.events.len() >= MAX_EVENTS {
            self.events.remove(0);
        }
        match e {
            Effect::TutorialMessage { title, body, seconds } => {
                self.show(title, body, *seconds);
                let (title, body, seconds) = (title.clone(), body.clone(), *seconds);
                self.events.push(TutorialEvent::Message { title, body, seconds });
            }
            Effect::TutorialPopup(text) => self.events.push(TutorialEvent::Popup(text.clone())),
            Effect::TutorialTrickReset { help, .. } => {
                (self.help, self.success, self.failure) = (help.clone(), Vec::new(), Vec::new());
                self.events.push(TutorialEvent::TrickReset(help.clone()));
            }
            Effect::TutorialTrickAdd { state, cmp, value, goofy, scale, count, text } => {
                let (Ok(id), Some(cmp)) = (u16::try_from(*state), Cmp::from_i32(*cmp)) else { return };
                let typed = |v: i32| match game_state_type(*state) {
                    Some(GsType::Bool) => Some(GsValue::Bool(true)),
                    Some(GsType::Float) => Some(GsValue::Float(v as f32)),
                    Some(GsType::Int) => Some(GsValue::Int(v)),
                    _ => None,
                };
                let (Some(value), Some(goofy_value)) = (typed(*value), typed(*goofy)) else { return };
                let r = TrickRecord {
                    id,
                    state: crate::objectives::game_state_name(id).to_string(),
                    cmp,
                    value,
                    goofy_value,
                    scale: *scale,
                    needed: *count as u32,
                    count: 0,
                    text: text.clone(),
                };
                self.events.push(TutorialEvent::TrickAdd(r.clone()));
                if *scale < 0.0 { self.failure.push(r) } else { self.success.push(r) }
            }
            Effect::TutorialTrickShow { button, text } => {
                self.events.push(TutorialEvent::TrickShow { button: *button, text: text.clone() })
            }
            Effect::TutorialTrickClear | Effect::ObjectiveMessageHide => {
                (self.success, self.failure) = (Vec::new(), Vec::new());
                self.message.request(HudMessage::READY, 0.0);
            }
            Effect::ObjectiveMessage { title, body, seconds, .. } => self.show(title, body, *seconds),
            _ => {}
        }
    }

    pub fn message_ready(&self) -> bool {
        self.message.current == HudMessage::READY
    }

    pub fn test_tricks(&mut self, read: &mut dyn FnMut(u16) -> Option<GsValue>) -> bool {
        let goofy = matches!(read(ID_GOOFY), Some(GsValue::Bool(true)));
        let mut all = true;
        for r in &mut self.success {
            if all {
                all = r.test(read(r.id), goofy);
            }
        }
        if all {
            self.events.push(TutorialEvent::TrickSuccess);
            return true;
        }
        for k in 0..self.failure.len() {
            let r = &mut self.failure[k];
            if r.test(read(r.id), goofy) {
                let text = r.text.clone();
                self.show("TXT_TUTORIAL_POPUP_TITLE", &text, 2.0);
                self.events.push(TutorialEvent::TrickFailed(text));
                break;
            }
        }
        false
    }

    pub fn update(&mut self, dt: f32) {
        self.message.update(dt);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::objectives::{Name, ObjectiveWorld, Runner, SetId, load_batch};
    use std::collections::BTreeMap;

    #[derive(Default)]
    struct W {
        t: Tutorial,
        state: BTreeMap<u16, GsValue>,
        sets: Vec<String>,
        message: crate::objectives::DropMessage,
        ui: BTreeMap<u16, bool>,
        effects: Vec<Effect>,
    }

    fn id(name: &str) -> u16 {
        ssx_lua::game::bindings::GAME_STATE.iter().position(|n| *n == name).expect("id") as u16
    }

    impl ObjectiveWorld for W {
        fn game_state(&mut self, _e: &Name, id: u16) -> Option<GsValue> {
            self.state.get(&id).copied()
        }
        fn tutorial_message_ready(&mut self) -> Option<bool> {
            Some(self.t.message_ready())
        }
        fn tutorial_tricks(&mut self, _e: &Name) -> Option<bool> {
            let state = self.state.clone();
            let done = self.t.test_tricks(&mut |i| state.get(&i).copied());
            if done {
                self.sets.push("BLN_ForceCaptureScoring".into());
            }
            Some(done)
        }
        fn ui_flag(&mut self, id: u16) -> Option<bool> {
            self.ui.get(&id).copied()
        }
        fn drop_message(&mut self) -> Option<crate::objectives::DropMessage> {
            Some(self.message.clone())
        }
        fn effect(&mut self, _o: &Name, e: &Effect) {
            self.t.effect(e);
            self.effects.push(e.clone());
        }
    }

    const MESSAGE_SCRIPT: &str = r#"
BeginObjective("ObjectiveMessage")
	WAIT_ObjectiveMessage("Player1")
EndObjective()
"#;

    fn message_world(kind: i32) -> W {
        let message = crate::objectives::DropMessage {
            title: Some("TXT_TITLE".into()),
            body: Some("TXT_BODY".into()),
            parameter: 1000,
            kind,
        };
        W { message, ..W::default() }
    }

    fn load_message(w: W) -> (Runner, W) {
        let r = Runner { level_active: true, ..Runner::default() };
        let batch = [("t.lua".into(), MESSAGE_SCRIPT.as_bytes().to_vec())];
        let (r, w, rep) = load_batch(r, SetId::Level, &batch, w, None);
        assert!(rep.problems.is_empty(), "{:?}", rep.problems);
        (r, w)
    }

    #[test]
    fn the_objective_message_holds_through_the_ready_loop() {
        let (mut r, mut w) = load_message(message_world(0));
        let ready = id("BLN_IsNisReadyLoop");
        w.state.insert(ready, GsValue::Bool(true));
        let step = |r: &mut Runner, w: &mut W| {
            w.effects.clear();
            r.update(0.5, w);
            w.t.update(0.5);
        };
        step(&mut r, &mut w);
        assert!(w.effects.is_empty() && !w.t.message.showing(), "UI 0x124 is not set yet");
        w.ui.insert(crate::objectives::UI_HUD_READY, true);
        step(&mut r, &mut w);
        let shown = Effect::ObjectiveMessage {
            title: "TXT_TITLE".into(),
            body: "TXT_BODY".into(),
            seconds: 0.0,
            parameter: 1000,
        };
        assert_eq!(w.effects, [shown]);
        assert!(
            w.t.message.showing()
                && (w.t.message.title.as_str(), w.t.message.body.as_str()) == ("TXT_TITLE", "TXT_BODY")
        );
        for _ in 0..20 {
            step(&mut r, &mut w);
        }
        assert!(w.t.message.showing() && w.effects.is_empty(), "held: no time runs in the ready loop");
        w.t.effect(&Effect::TutorialTrickAdd {
            state: id("BLN_IsTweaking") as i32,
            cmp: 2,
            value: 1,
            goofy: 1,
            scale: 0.0,
            count: 1,
            text: "T".into(),
        });
        w.state.insert(ready, GsValue::Bool(false));
        step(&mut r, &mut w);
        assert_eq!(w.effects, [Effect::ObjectiveMessageHide]);
        assert!(!w.t.message.showing() && w.t.success.is_empty());
        assert!(r.objective("ObjectiveMessage").is_some_and(|o| o.complete()));
    }

    #[test]
    fn the_objective_message_after_the_race_start() {
        let (mut r, mut w) = load_message(message_world(1));
        w.ui.insert(crate::objectives::UI_HUD_READY, true);
        let step = |r: &mut Runner, w: &mut W| {
            w.effects.clear();
            r.update(0.5, w);
            w.t.update(0.5);
        };
        step(&mut r, &mut w);
        assert!(w.effects.is_empty());
        w.ui.insert(crate::objectives::UI_HUD_RACE_STARTED, true);
        step(&mut r, &mut w);
        assert!(matches!(w.effects[..], [Effect::ObjectiveMessage { seconds, .. }] if seconds == 4.0));
        let mut updates = 0;
        while !r.objective("ObjectiveMessage").is_some_and(|o| o.complete()) && updates < 100 {
            step(&mut r, &mut w);
            updates += 1;
        }
        assert_eq!(updates, 8);
        assert_eq!(w.effects, [Effect::ObjectiveMessageHide]);

        let (mut r, mut w) = load_message(message_world(0));
        w.ui.extend([(crate::objectives::UI_HUD_READY, true), (crate::objectives::UI_HUD_RACE_STARTED, true)]);
        step(&mut r, &mut w);
        assert!(w.effects.is_empty() && !r.objective("ObjectiveMessage").is_some_and(|o| o.complete()));

        let (mut r, mut w) = load_message(W::default());
        step(&mut r, &mut w);
        assert!(w.effects.is_empty() && r.objective("ObjectiveMessage").is_some_and(|o| o.complete()));
    }

    const SCRIPT: &str = r#"
BeginObjective("A")
	WAIT_GameStateValue("Player1", FLT_Descent, GreaterThan, 10)
	DO_TutorialMessage("TXT_T", "TXT_B", 1)
	WAIT_TutorialMessageReady()
	DO_TutorialTrickObjectiveReset("Player1", "HELP")
	DO_TutorialTrickObjectiveAdd(INT_ScoringTrickId, EqualTo, Grab_Melon, 0, 3, "MELON")
	DO_TutorialTrickObjectiveAdd(INT_ScoringTrickId, EqualTo, Grab_Rocket, -1, 2, "FAIL")
	WAIT_TutorialTrickObjective("Player1")
	DO_SetTemporaryNamedFlag("CrossedFinishLine", true)
EndObjective()
"#;

    #[test]
    fn the_runner_drives_messages_and_trick_objectives() {
        let r = Runner { level_active: true, ..Runner::default() };
        let (mut r, mut w, rep) =
            load_batch(r, SetId::Level, &[("t.lua".into(), SCRIPT.as_bytes().to_vec())], W::default(), None);
        assert!(rep.problems.is_empty(), "{:?}", rep.problems);
        let (descent, trick) = (id("FLT_Descent"), id("INT_ScoringTrickId"));
        w.state.insert(descent, GsValue::Float(0.0));
        w.state.insert(trick, GsValue::Int(0));
        let step = |r: &mut Runner, w: &mut W| {
            r.update(0.5, w);
            w.t.update(0.5);
        };
        step(&mut r, &mut w);
        assert!(w.t.events.is_empty());
        w.state.insert(descent, GsValue::Float(20.0));
        step(&mut r, &mut w);
        assert!(w.t.message.showing(), "the component takes the request in this update's own pass");
        for _ in 0..3 {
            step(&mut r, &mut w);
        }
        assert_eq!(w.t.message.current, HudMessage::READY);
        step(&mut r, &mut w);
        assert_eq!((w.t.success.len(), w.t.failure.len()), (1, 1));
        w.t.events.clear();
        w.state.insert(trick, GsValue::Int(7));
        step(&mut r, &mut w);
        step(&mut r, &mut w);
        assert!(w.t.events.contains(&TutorialEvent::TrickFailed("FAIL".into())));
        w.state.insert(trick, GsValue::Int(4));
        for _ in 0..3 {
            step(&mut r, &mut w);
        }
        assert!(w.t.events.contains(&TutorialEvent::TrickSuccess) && w.sets == ["BLN_ForceCaptureScoring"]);
        assert!(r.flags.check(&Name::new("CrossedFinishLine")) && r.objective("A").is_some_and(|o| o.complete()));
        assert_eq!(w.t.success.len(), 1, "a pass leaves the lists (only the wait's reset or a reset empties them)");
    }

    #[test]
    fn a_goofy_rider_is_asked_for_the_nodes_goofy_value() {
        let mut t = Tutorial::default();
        let trick = id("INT_ScoringTrickId") as i32;
        let add = Effect::TutorialTrickAdd {
            state: trick,
            cmp: 2,
            value: 3,
            goofy: 4,
            scale: 0.0,
            count: 1,
            text: "".into(),
        };
        t.effect(&add);
        let read = |g: bool, v: i32| {
            move |i: u16| match i {
                0x66 => Some(GsValue::Bool(g)),
                _ => Some(GsValue::Int(v)),
            }
        };
        assert!(!t.test_tricks(&mut read(true, 3)) && t.test_tricks(&mut read(true, 4)));
        assert!(t.test_tricks(&mut read(false, 3)));
    }

    #[test]
    fn events_queue_until_taken() {
        let mut t = Tutorial::default();
        t.effect(&Effect::TutorialPopup("TXT_A".into()));
        t.update(1.0);
        t.effect(&Effect::TutorialPopup("TXT_B".into()));
        let taken = t.take_events();
        assert_eq!(taken, [TutorialEvent::Popup("TXT_A".into()), TutorialEvent::Popup("TXT_B".into())]);
        assert!(t.events.is_empty() && t.take_events().is_empty());
        for k in 0..MAX_EVENTS + 3 {
            t.effect(&Effect::TutorialPopup(format!("TXT_{k}")));
        }
        assert_eq!(t.events.len(), MAX_EVENTS);
        assert_eq!(t.events[0], TutorialEvent::Popup("TXT_3".into()));
    }
}
