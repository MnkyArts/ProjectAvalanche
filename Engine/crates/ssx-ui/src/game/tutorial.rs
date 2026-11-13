use ssx_lua::game::objectives::node::{Node, Wait};
use ssx_lua::game::objectives::{Effect, GsType, GsValue, game_state_type};
use ssx_physics::rider::RiderState;
use ssx_physics::session::Session;
use ssx_physics::session::tutorial::{HudMessage, TutorialEvent};

use super::GameUi;
use super::loc::Localization;
use super::values::{UiValue, UiValues, id};

pub const TRICK_IS_DIRTY: u16 = 0x21D;
pub const TRICK_IS_ENABLED: u16 = 0x21E;
pub const TRICK_HELP: u16 = 0x21F;
pub const TRICK_OBJECTIVE: u16 = 0x220;
pub const TRICK_SUMMARY: u16 = 0x221;
pub const TRICK_SUCCESS: u16 = 0x222;
pub const TRICK_BUTTON_ID: u16 = 0x223;
pub const TRICK_BUTTON_LOCATION: u16 = 0x224;
pub const TRICK_BUTTON_STRING: u16 = 0x225;
pub const TRICK_TWEAK_LEFT: u16 = 0x226;
pub const TRICK_TWEAK_RIGHT: u16 = 0x227;

pub const TUTORIAL_HUD_DROP: u32 = 0xC145_699A;

pub const BUTTONS: [(&str, &str, u8); 47] = [
    ("^RS_UP^", "^Y^", 0),
    ("^RS_DOWN^", "^A^", 1),
    ("^RS_LEFT^", "^X^", 2),
    ("^RS_RIGHT^", "^B^", 3),
    ("^LS_UP^", "^LS_UP^", 0),
    ("^LS_DOWN^", "^LS_DOWN^", 1),
    ("^LS_LEFT^", "^LS_LEFT^", 2),
    ("^LS_RIGHT^", "^LS_RIGHT^", 3),
    ("^RS_UP^", "^RS_UP^", 0),
    ("^RS_DOWN^", "^RS_DOWN^", 1),
    ("^RS_LEFT^", "^RS_LEFT^", 2),
    ("^RS_RIGHT^", "^RS_RIGHT^", 3),
    ("^RB^", "^RB^", 0),
    ("", "", 4),
    ("^RS_LEFT^ + ^RT^", "^X^ + ^RT^", 2),
    ("^RS_L_DOWN^", "^X^ ^A^", 1),
    ("^RS_L_DOWN^", "^X^ ^NEXT^ ^A^", 1),
    ("^RS_L_DOWN^ + ^RT^", "^X^ ^A^ + ^RT^", 1),
    ("^RS_L_UP^", "^X^ ^Y^", 0),
    ("^RS_L_UP^", "^X^ ^NEXT^ ^Y^", 0),
    ("^RS_L_UP^ + ^RT^", "^X^ ^Y^ + ^RT^", 0),
    ("^RS_LEFT^ ^RS_RIGHT^", "^X^ ^B^", 3),
    ("^RS_LEFT^ ^RS_RIGHT^", "^X^ ^NEXT^ ^B^", 3),
    ("^RS_LEFT^ ^RS_RIGHT^ + ^RT^", "^X^ ^B^ + ^RT^", 3),
    ("^RS_UP_R^", "^Y^ + ^RT^", 0),
    ("^RS_UP_L^", "^Y^ ^X^", 2),
    ("^RS_UP_L^", "^Y^ ^NEXT^ ^X^", 2),
    ("^RS_UP_L^ + ^RT^", "^Y^ ^X^ + ^RT^", 2),
    ("^RS_UP^ ^RS_DOWN^", "^Y^ ^A^", 1),
    ("^RS_UP^ ^RS_DOWN^", "^Y^ ^NEXT^ ^A^", 1),
    ("^RS_UP^ ^RS_DOWN^ + ^RT^", "^Y^ ^A^ + ^RT^", 1),
    ("^RS_UP_R^", "^Y^ ^B^", 3),
    ("^RS_UP_R^", "^Y^ ^NEXT^ ^B^", 3),
    ("^RS_UP_R^ + ^RT^", "^Y^ ^B^ + ^RT^", 3),
    ("^RS_RIGHT^ + ^RT^", "^B^ + ^RT^", 3),
    ("^RS_RIGHT^ ^RS_LEFT^", "^B^ ^X^", 2),
    ("^RS_RIGHT^ ^RS_LEFT^", "^B^ ^NEXT^ ^X^", 2),
    ("^RS_RIGHT^ ^RS_LEFT^ + ^RT^", "^B^ ^X^ + ^RT^", 2),
    ("^RS_R_UP^", "^B^ ^Y^", 0),
    ("^RS_R_UP^", "^B^ ^NEXT^ ^Y^", 0),
    ("^RS_R_UP^ + ^RT^", "^B^ ^Y^ + ^RT^", 0),
    ("^RS_R_DOWN^", "^B^ ^A^", 1),
    ("^RS_R_DOWN^", "^B^ ^NEXT^ ^A^", 1),
    ("^RS_R_DOWN^ + ^RT^", "^B^ ^A^ + ^RT^", 1),
    ("^LT^ + ^RT^ + ^RS_UP^", "^LT^ + ^RT^ + ^Y^", 0),
    ("^LT^ + ^RT^ + ^RS_RIGHT^", "^LT^ + ^RT^ + ^B^", 3),
    ("^LT^ + ^RT^ + ^RS_LEFT^", "^LT^ + ^RT^ + ^X^", 2),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Alt {
    Both,
    Analog,
    None,
}

pub fn text(loc: &Localization, controller: i32, key: &str, alt: Alt) -> String {
    if key.is_empty() {
        return String::new();
    }
    let other = match (controller == 2, alt) {
        (_, Alt::None) | (false, Alt::Analog) => None,
        (true, _) => Some(format!("{key}_ANALOG")),
        (false, Alt::Both) => Some(format!("{key}_DIGITAL")),
    };
    if let Some(t) = other.and_then(|k| loc.find(&k)) {
        return t;
    }
    loc.find(key).unwrap_or_else(|| format!("Unknown Hash {}", ssx_formats::t3db::loc_hash(key) as i32))
}

#[derive(Clone, Debug, PartialEq)]
pub struct RecordText {
    pub text: String,
    fresh: bool,
    shown: f32,
}

pub fn player_state(s: &RiderState, name: &str) -> Option<GsValue> {
    Some(match name {
        "FLT_ScoringAccumSpinTotal" => GsValue::Float(s.score.spin_fs.abs() + s.score.spin_bs.abs()),
        "FLT_ScoringAccumFlipTotal" => GsValue::Float(s.score.flip_fwd.abs() + s.score.flip_bwd.abs()),
        "INT_ScoringTrickId" => GsValue::Int(i32::from(s.score.trick_id.value())),
        "BLN_Goofy" => GsValue::Bool(s.goofy),
        "BLN_IsAwesome" => GsValue::Bool(s.score.is_awesome),
        "BLN_IsSuperAwesome" => GsValue::Bool(s.score.is_super_awesome),
        _ => return None,
    })
}

pub type StateReader<'a> = &'a dyn Fn(&str) -> Option<GsValue>;

impl RecordText {
    fn new(text: String) -> Self {
        RecordText { text, fresh: true, shown: 0.0 }
    }

    fn build(
        &mut self,
        r: &ssx_physics::session::tutorial::TrickRecord,
        read: StateReader,
        loc: &mut Localization,
        rider: &str,
    ) -> Result<Option<String>, String> {
        const EPSILON: f32 = 1.192_092_9e-7;
        let flag = |name: &str| matches!(read(name), Some(GsValue::Bool(true)));
        Ok(match (r.value, r.goofy_value) {
            (GsValue::Float(value), _) if r.scale > EPSILON => {
                let Some(GsValue::Float(now)) = read(&r.state) else { return Err(r.state.clone()) };
                let (target, now) = (value / r.scale, now / r.scale);
                let shown = if now - target >= 0.0 { target } else { now };
                if !self.fresh && (self.shown - shown).abs() < 1.525_878_9e-5 {
                    return Ok(None);
                }
                (self.fresh, self.shown) = (false, shown);
                Some(format!("{} / {target:.0} {}", loc.number(f64::from(shown), 1), self.text))
            }
            (GsValue::Int(value), _) if r.scale > EPSILON => {
                let Some(GsValue::Int(now)) = read(&r.state) else { return Err(r.state.clone()) };
                if !self.fresh && self.shown == now as f32 {
                    return Ok(None);
                }
                (self.fresh, self.shown) = (false, now as f32);
                Some(format!("{now} / {value} {}", self.text))
            }
            (GsValue::Int(value), GsValue::Int(goofy)) if r.id == 0xF3 && r.scale >= 0.0 => {
                let trick = if flag("BLN_Goofy") { goofy } else { value };
                let (awesome, super_awesome) = (flag("BLN_IsAwesome"), flag("BLN_IsSuperAwesome"));
                std::mem::take(&mut self.fresh)
                    .then(|| super::tricks::trick_type_name(loc, trick as u32, awesome, super_awesome, rider))
            }
            _ => std::mem::take(&mut self.fresh).then(|| self.text.clone()),
        })
    }
}

#[derive(Clone, Debug, Default)]
pub struct TutorialManager {
    changed: bool,
    shown: bool,
    published: bool,
    lessons: Option<u32>,
    lesson: u32,
    records: Vec<RecordText>,
    message: u32,
    clock: f64,
    pub unread: std::collections::BTreeSet<String>,
}

enum Op<'a> {
    Effect(&'a Effect),
    Test { success: bool, failure: Option<&'a str> },
}

fn queues_event(e: &Effect) -> bool {
    match e {
        Effect::TutorialMessage { .. }
        | Effect::TutorialPopup(_)
        | Effect::TutorialTrickReset { .. }
        | Effect::TutorialTrickShow { .. } => true,
        Effect::TutorialTrickAdd { state, cmp, .. } => {
            u16::try_from(*state).is_ok()
                && ssx_lua::game::objectives::Cmp::from_i32(*cmp).is_some()
                && matches!(game_state_type(*state), Some(GsType::Bool | GsType::Float | GsType::Int))
        }
        _ => false,
    }
}

fn is_result(e: &TutorialEvent) -> bool {
    matches!(e, TutorialEvent::TrickSuccess | TutorialEvent::TrickFailed(_))
}

fn event_of(event: &TutorialEvent, effect: &Effect) -> bool {
    matches!(
        (event, effect),
        (TutorialEvent::Message { .. }, Effect::TutorialMessage { .. })
            | (TutorialEvent::Popup(_), Effect::TutorialPopup(_))
            | (TutorialEvent::TrickReset(_), Effect::TutorialTrickReset { .. })
            | (TutorialEvent::TrickAdd(_), Effect::TutorialTrickAdd { .. })
            | (TutorialEvent::TrickShow { .. }, Effect::TutorialTrickShow { .. })
    )
}

fn waits_for_tricks(n: &Node) -> bool {
    match n {
        Node::Wait(Wait::TutorialTricks { .. }) => true,
        Node::And { a, b, .. } | Node::Or { a, b, .. } => {
            waits_for_tricks(a) || b.as_deref().is_some_and(waits_for_tricks)
        }
        Node::Not(n) => n.as_deref().is_some_and(waits_for_tricks),
        _ => false,
    }
}

pub trait TutorialHost {
    fn values(&mut self) -> &mut UiValues;
    fn loc(&mut self) -> &mut Localization;
    fn controller(&self) -> i32;
    fn popup(&mut self, key: &str) -> bool;
}

impl TutorialManager {
    pub fn published(&mut self) {
        self.published = true;
    }

    fn count_lessons(sets: &[Vec<ssx_lua::game::objectives::Objective>; 3]) -> u32 {
        use ssx_lua::game::objectives::node::Do;
        let with_help =
            |n: &&Node| matches!(n, Node::Do(Do::Effect(Effect::TutorialTrickReset { help, .. })) if !help.is_empty());
        sets.iter().flatten().map(|o| o.nodes.iter().filter(with_help).count() as u32).sum()
    }

    pub fn level_start(&mut self, v: &mut UiValues) {
        let unread = std::mem::take(&mut self.unread);
        *self = TutorialManager { message: HudMessage::READY, unread, ..Default::default() };
        v.set_bool(id::HUDMESSAGE_IS_ENABLED, false);
        v.set_bool(id::HUDMESSAGE_RIVAL_IS_ENABLED, false);
    }

    pub fn step(
        &mut self,
        host: &mut dyn TutorialHost,
        read: StateReader,
        x: &Session,
        events: &[TutorialEvent],
        rider: &str,
    ) -> usize {
        let Some(o) = x.objectives.as_ref() else { return 0 };
        if o.runner.time < self.clock {
            self.level_start(host.values());
        }
        if o.runner.time == self.clock {
            return 0;
        }
        self.clock = o.runner.time;
        let lessons = *self.lessons.get_or_insert_with(|| Self::count_lessons(&o.runner.sets));
        let place = |name: &str| {
            ssx_lua::game::objectives::find(&o.runner.sets, ssx_lua::game::objectives::fnv1(name.as_bytes()))
        };
        type Queue<'a> = std::iter::Peekable<std::slice::Iter<'a, TutorialEvent>>;
        fn results<'a>(ops: &mut Vec<(Op<'a>, Option<(usize, usize)>)>, queue: &mut Queue<'a>) {
            while let Some(e) = queue.next_if(|e| is_result(e)) {
                let failure = match e {
                    TutorialEvent::TrickFailed(t) => Some(t.as_str()),
                    _ => None,
                };
                ops.push((Op::Test { success: failure.is_none(), failure }, None));
            }
        }
        let mut ops: Vec<(Op, Option<(usize, usize)>)> = Vec::new();
        let mut queue = events.iter().peekable();
        results(&mut ops, &mut queue);
        for (name, e) in &o.effects {
            let shown = matches!(
                e,
                Effect::TutorialTrickClear | Effect::ObjectiveMessage { .. } | Effect::ObjectiveMessageHide
            );
            if !queues_event(e) && !shown {
                continue;
            }
            ops.push((Op::Effect(e), place(name)));
            if queues_event(e) {
                while queue.next_if(|ev| !is_result(ev) && !event_of(ev, e)).is_some() {}
                queue.next_if(|ev| event_of(ev, e));
                results(&mut ops, &mut queue);
            }
        }
        while queue.peek().is_some() {
            queue.next_if(|ev| !is_result(ev));
            results(&mut ops, &mut queue);
        }
        let waiting = (0..o.runner.sets.len()).flat_map(|si| (0..o.runner.sets[si].len()).map(move |i| (si, i))).find(
            |&(si, i)| {
                let ob = &o.runner.sets[si][i];
                !ob.overridden && ob.cur.and_then(|c| ob.nodes.get(c)).is_some_and(waits_for_tricks)
            },
        );
        let failed = ops.iter().any(|(op, _)| matches!(op, Op::Test { success: false, .. }));
        if let Some(w) = waiting.filter(|_| !failed) {
            let at = ops.iter().position(|(_, p)| p.is_some_and(|p| p > w)).unwrap_or(ops.len());
            ops.insert(at, (Op::Test { success: false, failure: None }, None));
        }
        let mut popups = 0;
        for (op, _) in ops {
            match op {
                Op::Effect(e) => popups += usize::from(self.effect(host, e, lessons)),
                Op::Test { success, failure } => self.test(host, read, x, success, failure, rider),
            }
        }
        let state = x.tutorial.as_ref().map_or(HudMessage::READY, |t| t.message.current);
        if state != self.message {
            self.message = state;
            host.values().set_bool(id::HUDMESSAGE_IS_ENABLED, state == HudMessage::SHOWING);
            if state != HudMessage::SHOWING {
                host.values().set_bool(id::HUDMESSAGE_RIVAL_IS_ENABLED, false);
            }
        }
        popups
    }
}

fn with_parameter(text: &str, parameter: i32) -> String {
    crate::game::loc::format_positional(text, &[crate::game::loc::FormatArg::Int(i64::from(parameter))])
}

fn message(v: &mut UiValues, title: String, body: String) {
    v.set_str(id::HUDMESSAGE_TITLE, title);
    v.set_str(id::HUDMESSAGE_MESSAGE, body);
}

impl TutorialManager {
    fn clear(&mut self, v: &mut UiValues) {
        self.records.clear();
        v.set_bool(TRICK_IS_ENABLED, false);
        for list in [TRICK_BUTTON_STRING, TRICK_BUTTON_LOCATION, TRICK_BUTTON_ID] {
            v.set(list, UiValue::List(Vec::new()));
        }
        v.set_bool(TRICK_TWEAK_LEFT, false);
        v.set_bool(TRICK_TWEAK_RIGHT, false);
    }

    fn effect(&mut self, host: &mut dyn TutorialHost, e: &Effect, lessons: u32) -> bool {
        let controller = host.controller();
        match e {
            Effect::TutorialMessage { title, body, .. } => {
                let title = text(host.loc(), controller, title, Alt::None);
                let body = text(host.loc(), controller, body, Alt::Both);
                message(host.values(), title, body);
            }
            Effect::TutorialPopup(key) => return host.popup(key),
            Effect::TutorialTrickReset { help, .. } => {
                let help = text(host.loc(), controller, help, Alt::Both);
                let mut summary = String::new();
                if !help.is_empty() {
                    self.lesson += 1;
                    summary = format!("{} / {lessons}", self.lesson);
                }
                let v = host.values();
                self.clear(v);
                v.set_bool(TRICK_IS_DIRTY, true);
                v.set_str(TRICK_HELP, help);
                v.set_str(TRICK_OBJECTIVE, "");
                v.set_str(TRICK_SUMMARY, summary);
                v.set_bool(TRICK_TWEAK_LEFT, false);
                v.set_bool(TRICK_TWEAK_RIGHT, false);
                v.set_bool(id::TRICK_INDICATOR_ACTIVE, false);
                v.set_bool(id::COMBO_INDICATOR_ACTIVE, false);
                self.changed = true;
            }
            Effect::TutorialTrickAdd { scale, text: key, .. } if queues_event(e) && *scale >= 0.0 => {
                self.records.push(RecordText::new(text(host.loc(), controller, key, Alt::Analog)));
                self.changed = true;
            }
            Effect::TutorialTrickShow { button, text: key } => {
                let Some(&(analog, digital, place)) = usize::try_from(*button).ok().and_then(|b| BUTTONS.get(b)) else {
                    return false;
                };
                let tokens = if controller == 2 { analog } else { digital };
                let label = format!("{tokens} {}", text(host.loc(), controller, key, Alt::None));
                let v = host.values();
                for (list, item) in [(TRICK_BUTTON_STRING, label), (TRICK_BUTTON_LOCATION, place.to_string())] {
                    let mut items = v.list(list).to_vec();
                    items.push(item);
                    v.set(list, UiValue::List(items));
                }
            }
            Effect::ObjectiveMessage { title, body, parameter, .. } => {
                let title = text(host.loc(), controller, title, Alt::None);
                let body = with_parameter(&text(host.loc(), controller, body, Alt::None), *parameter);
                message(host.values(), title, body);
            }
            Effect::TutorialTrickClear | Effect::ObjectiveMessageHide => self.clear(host.values()),
            _ => {}
        }
        false
    }

    fn test(
        &mut self,
        host: &mut dyn TutorialHost,
        read: StateReader,
        x: &Session,
        success: bool,
        failure: Option<&str>,
        rider: &str,
    ) {
        let controller = host.controller();
        if self.changed {
            (self.changed, self.shown, self.published) = (false, true, false);
            host.values().set_bool(TRICK_IS_ENABLED, true);
            host.values().set_bool(id::LEADER_IS_ACTIVE, false);
        } else if self.shown && self.published {
            // PORT(choice): this branch waits for a UI update after the one above (the port can take two steps in a
            // drawn frame), to drop when the UI publishes once per simulation step; nothing to decode: the game
            // publishes once per simulation frame.
            host.values().set_bool(TRICK_IS_DIRTY, false);
            host.values().set_bool(TRICK_SUCCESS, false);
            self.shown = false;
        }
        for (r, record) in x.tutorial.iter().flat_map(|t| &t.success).zip(&mut self.records) {
            match record.build(r, read, host.loc(), rider) {
                Ok(Some(text)) => host.values().set_str(TRICK_OBJECTIVE, text),
                Ok(None) => {}
                Err(state) => _ = self.unread.insert(state),
            }
        }
        if success {
            host.values().set_bool(TRICK_SUCCESS, true);
        } else if let Some(key) = failure {
            let title = text(host.loc(), controller, "TXT_TUTORIAL_POPUP_TITLE", Alt::None);
            let body = text(host.loc(), controller, key, Alt::Analog);
            message(host.values(), title, body);
        }
    }
}

impl TutorialHost for GameUi {
    fn values(&mut self) -> &mut UiValues {
        &mut self.values
    }
    fn loc(&mut self) -> &mut Localization {
        &mut self.loc
    }
    fn controller(&self) -> i32 {
        self.tutorial_controller()
    }
    fn popup(&mut self, key: &str) -> bool {
        self.tutorial_popup(key)
    }
}

impl GameUi {
    pub fn tutorial_controller(&self) -> i32 {
        self.fe.as_ref().map_or(0, |f| f.tutorial_pause.controller)
    }

    pub fn tutorial_level_start(&mut self) {
        self.tutorial.level_start(&mut self.values);
    }

    pub fn tutorial_step(&mut self, s: &RiderState, x: &Session, events: &[TutorialEvent], rider: &str) -> usize {
        let mut manager = std::mem::take(&mut self.tutorial);
        let popups = manager.step(self, &|name| player_state(s, name), x, events, rider);
        self.tutorial = manager;
        popups
    }

    pub fn tutorial_popup(&mut self, key: &str) -> bool {
        // PORT(choice): a network session withholds the tutorial popups (above); the original has no real-time
        // multiplayer, and its popups pause the simulation.
        if let Some(fe) = self.fe.as_mut().filter(|fe| fe.network_session) {
            fe.withheld_popups.push(key.to_string());
            return false;
        }
        if key.eq_ignore_ascii_case("InputChoice") {
            self.fe_call(super::fe::tutorial_pause::show_input_choice);
        } else {
            let message = text(&self.loc, self.tutorial_controller(), key, Alt::Analog);
            self.fe_call(|fe, env| super::fe::tutorial_pause::show(fe, env, key, &message));
        }
        self.fe.is_some()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use ssx_physics::objectives::{LevelObjectives, Name, ObjectiveWorld, Runner, SetId, load_batch};
    use ssx_physics::session::tutorial::Tutorial;

    use super::*;
    use crate::game::loc::Language;

    struct Host {
        v: UiValues,
        loc: Localization,
        controller: i32,
        popups: Vec<String>,
    }

    impl TutorialHost for Host {
        fn values(&mut self) -> &mut UiValues {
            &mut self.v
        }
        fn loc(&mut self) -> &mut Localization {
            &mut self.loc
        }
        fn controller(&self) -> i32 {
            self.controller
        }
        fn popup(&mut self, key: &str) -> bool {
            self.popups.push(key.to_string());
            true
        }
    }

    fn host(texts: &[(&str, &str)]) -> Host {
        let mut loc = Localization::empty(Language::English);
        for (k, t) in texts {
            loc.strings.by_hash.insert(ssx_formats::t3db::loc_hash(k), (k.to_string(), Some(t.to_string())));
        }
        Host { v: UiValues::new(), loc, controller: 0, popups: Vec::new() }
    }

    #[derive(Default)]
    struct W {
        t: Tutorial,
        state: BTreeMap<u16, GsValue>,
        effects: Vec<(String, Effect)>,
        message: ssx_lua::game::objectives::DropMessage,
        ui: BTreeMap<u16, bool>,
    }

    impl ObjectiveWorld for W {
        fn game_state(&mut self, _e: &Name, id: u16) -> Option<GsValue> {
            self.state.get(&id).copied()
        }
        fn tutorial_message_ready(&mut self) -> Option<bool> {
            Some(self.t.message_ready())
        }
        fn ui_flag(&mut self, id: u16) -> Option<bool> {
            self.ui.get(&id).copied()
        }
        fn drop_message(&mut self) -> Option<ssx_lua::game::objectives::DropMessage> {
            Some(self.message.clone())
        }
        fn tutorial_tricks(&mut self, _e: &Name) -> Option<bool> {
            let state = self.state.clone();
            Some(self.t.test_tricks(&mut |i| state.get(&i).copied()))
        }
        fn effect(&mut self, o: &Name, e: &Effect) {
            self.t.effect(e);
            self.effects.push((o.text.to_string(), e.clone()));
        }
    }

    const SCRIPT: &str = r#"
BeginObjective("A")
	DO_TutorialMessage("TXT_T", "TXT_B", 1)
	WAIT_TutorialMessageReady()
	DO_TutorialTrickObjectiveReset("Player1", "HELP")
	DO_TutorialTrickObjectiveAdd(FLT_ScoringAccumSpinTotal, GreaterThan, 19, 6.28, 1, "ROT")
	DO_TutorialTrickObjectiveShow(LS_LEFT, "SPIN")
	DO_TutorialTrickObjectiveShow(TOP_BUTTON, "")
	WAIT_TutorialTrickObjective("Player1")
	DO_TutorialTrickObjectiveReset("Player1", "")
	DO_TutorialPopup("inputchoice")
EndObjective()
"#;

    fn update(r: &mut Runner, w: &mut W, m: &mut TutorialManager, h: &mut Host, spin: f32) -> usize {
        let id = ssx_lua::game::bindings::GAME_STATE.iter().position(|n| *n == "FLT_ScoringAccumSpinTotal");
        w.state.insert(id.expect("id") as u16, GsValue::Float(spin));
        w.effects.clear();
        r.update(0.5, w);
        w.t.update(0.5);
        let events = w.t.take_events();
        let objectives = LevelObjectives { runner: r.clone(), effects: w.effects.clone(), ..Default::default() };
        let x = Session { objectives: Some(objectives), tutorial: Some(w.t.clone()), ..Default::default() };
        let read = |name: &str| (name == "FLT_ScoringAccumSpinTotal").then_some(GsValue::Float(spin));
        m.step(h, &read, &x, &events, "zoe")
    }

    #[test]
    fn a_lesson_from_its_message_to_its_success() {
        let r = Runner { level_active: true, ..Runner::default() };
        let (mut r, mut w, rep) =
            load_batch(r, SetId::Level, &[("t.lua".into(), SCRIPT.as_bytes().to_vec())], W::default(), None);
        assert!(rep.problems.is_empty(), "{:?}", rep.problems);
        let texts = [("TXT_T", "tip"), ("TXT_B", "plain"), ("TXT_B_DIGITAL", "buttons"), ("TXT_B_ANALOG", "stick")];
        let mut h = host(&[&texts[..], &[("HELP", "help"), ("ROT", "rot"), ("SPIN", "spin")]].concat());
        let mut m = TutorialManager::default();
        m.level_start(&mut h.v);
        assert_eq!(update(&mut r, &mut w, &mut m, &mut h, 0.0), 0);
        assert_eq!((h.v.str(id::HUDMESSAGE_TITLE), h.v.str(id::HUDMESSAGE_MESSAGE)), ("tip", "buttons"));
        assert!(h.v.bool(id::HUDMESSAGE_IS_ENABLED));
        for _ in 0..3 {
            update(&mut r, &mut w, &mut m, &mut h, 0.0);
        }
        assert!(!h.v.bool(id::HUDMESSAGE_IS_ENABLED), "the component went back to state 7");
        update(&mut r, &mut w, &mut m, &mut h, 0.0);
        assert!(h.v.bool(TRICK_IS_DIRTY) && h.v.bool(TRICK_IS_ENABLED) && !h.v.bool(TRICK_SUCCESS));
        assert_eq!((h.v.str(TRICK_HELP), h.v.str(TRICK_SUMMARY)), ("help", "1 / 1"));
        assert_eq!(h.v.str(TRICK_OBJECTIVE), "0.0 / 3 rot");
        assert_eq!(h.v.list(TRICK_BUTTON_STRING), ["^LS_LEFT^ spin", "^Y^ "]);
        assert_eq!(h.v.list(TRICK_BUTTON_LOCATION), ["2", "0"]);
        update(&mut r, &mut w, &mut m, &mut h, 6.25);
        assert!(h.v.bool(TRICK_IS_DIRTY));
        assert_eq!(h.v.str(TRICK_OBJECTIVE), "1.0 / 3 rot");
        m.published();
        update(&mut r, &mut w, &mut m, &mut h, 6.25);
        assert!(!h.v.bool(TRICK_IS_DIRTY) && h.v.bool(TRICK_IS_ENABLED));
        assert_eq!(update(&mut r, &mut w, &mut m, &mut h, 20.0), 1);
        assert!(h.v.bool(TRICK_SUCCESS) && !h.v.bool(TRICK_IS_ENABLED) && h.v.bool(TRICK_IS_DIRTY));
        assert_eq!((h.v.str(TRICK_HELP), h.v.str(TRICK_SUMMARY), h.v.str(TRICK_OBJECTIVE)), ("", "", ""));
        assert!(h.v.list(TRICK_BUTTON_STRING).is_empty() && h.popups == ["inputchoice"]);
    }

    #[test]
    fn a_text_by_the_controller_type() {
        let h = host(&[("K", "plain"), ("K_ANALOG", "stick"), ("K_DIGITAL", "buttons"), ("P", "only")]);
        assert_eq!(text(&h.loc, 0, "K", Alt::Both), "buttons");
        assert_eq!(text(&h.loc, 2, "K", Alt::Both), "stick");
        assert_eq!(
            (text(&h.loc, 1, "K", Alt::Analog), text(&h.loc, 2, "K", Alt::Analog)),
            ("plain".into(), "stick".into())
        );
        assert_eq!((text(&h.loc, 2, "K", Alt::None), text(&h.loc, 2, "P", Alt::Both)), ("plain".into(), "only".into()));
        assert_eq!(text(&h.loc, 0, "", Alt::Both), "");
        let missing = ssx_formats::t3db::loc_hash("NOPE") as i32;
        assert_eq!(text(&h.loc, 0, "NOPE", Alt::Both), format!("Unknown Hash {missing}"));
    }

    #[test]
    fn the_buttons_of_the_show_node() {
        assert_eq!(BUTTONS[13], ("", "", 4), "CLEAR");
        assert!(BUTTONS.iter().all(|b| b.2 <= 4));
        assert!(BUTTONS[4..13].iter().all(|b| b.0 == b.1));
        assert_eq!(BUTTONS[19], ("^RS_L_UP^", "^X^ ^NEXT^ ^Y^", 0), "LEFT_NEXT_TOP_BUTTON");
    }
    #[test]
    fn the_drops_message_shows_through_the_ready_loop() {
        let ready_loop = ssx_lua::game::bindings::GAME_STATE.iter().position(|n| *n == "BLN_IsNisReadyLoop");
        let ready_loop = ready_loop.expect("id") as u16;
        let message = ssx_lua::game::objectives::DropMessage {
            title: Some("TXT_GOAL".into()),
            body: Some("TXT_BEAT".into()),
            parameter: 7,
            kind: 0,
        };
        let mut w = W { message, ..W::default() };
        w.state.insert(ready_loop, GsValue::Bool(true));
        let script = "BeginObjective(\"M\")\n\tWAIT_ObjectiveMessage(\"Player1\")\nEndObjective()\n";
        let r = Runner { level_active: true, ..Runner::default() };
        let (mut r, mut w, rep) = load_batch(r, SetId::Level, &[("m.lua".into(), script.as_bytes().to_vec())], w, None);
        assert!(rep.problems.is_empty(), "{:?}", rep.problems);
        let mut h = host(&[("TXT_GOAL", "Goal"), ("TXT_BEAT", "Beat %1d rivals, 100%% (%d)")]);
        let mut m = TutorialManager::default();
        m.level_start(&mut h.v);
        update(&mut r, &mut w, &mut m, &mut h, 0.0);
        assert!(
            !h.v.bool(id::HUDMESSAGE_IS_ENABLED) && h.v.str(id::HUDMESSAGE_MESSAGE).is_empty(),
            "the HUD not ready"
        );
        w.ui.insert(id::READY, true);
        update(&mut r, &mut w, &mut m, &mut h, 0.0);
        assert!(h.v.bool(id::HUDMESSAGE_IS_ENABLED));
        assert_eq!(
            (h.v.str(id::HUDMESSAGE_TITLE), h.v.str(id::HUDMESSAGE_MESSAGE)),
            ("Goal", "Beat 7 rivals, 100%% (%d)")
        );
        update(&mut r, &mut w, &mut m, &mut h, 0.0);
        assert!(h.v.bool(id::HUDMESSAGE_IS_ENABLED), "held while the ready loop runs");
        w.state.insert(ready_loop, GsValue::Bool(false));
        update(&mut r, &mut w, &mut m, &mut h, 0.0);
        update(&mut r, &mut w, &mut m, &mut h, 0.0);
        assert!(!h.v.bool(id::HUDMESSAGE_IS_ENABLED), "the loop ended: the component back in state 7");
    }
}
