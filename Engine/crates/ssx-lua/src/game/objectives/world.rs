use super::*;

#[derive(Clone, Debug, PartialEq)]
pub struct NisRequest {
    pub name: String,
    pub for_debug: bool,
    pub origin: i32,
    pub location: Option<[f32; 4]>,
    pub locator: Option<Name>,
    pub locator_alternate: Option<Name>,
    pub actors: Vec<NisActorBinding>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct NisActorBinding {
    pub role: Name,
    pub entity: Name,
    pub set_index: Option<i32>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Effect {
    SetGameState { entity: Name, id: u16, value: GsValue },
    WriteLocatorToGameState { entity: Name, locator: Name, position_id: i32, rotation_id: i32, offset: [f32; 3] },
    AiChaseTarget { entity: Name, target: Name },
    AiMoveToLocator { entity: Name, locator: Name, offset: [f32; 3], yaw_deg: f32 },
    WarpEntity { entity: Name, locator: Name },
    MoveToNisStartLocation { nis: String, role: Name, entity: Name, locator: Name },
    PlayNis(NisRequest),
    SetNisAudioType(Name),
    Fade { fade_out: bool, seconds: f32, delay: f32 },
    EndLevel(bool),
    Visibility { set: Name, state: String },
    PostEffectCue { name: Name, cue: i32 },
    PostEvent { name: Name, entity: Option<Name> },
    PostFxStart { name: Name, start: f32, end: f32 },
    PostFxStop { name: Name, blend: f32 },
    LightingOverrideFromParam { target: Name, source: Name },
    LightingOverrideRemove(Name),
    LightingFlush(f32),
    CascadeShadowOverride([f32; 5]),
    CascadeShadowRemove,
    SurviveFogBegin { source: Name },
    SurviveFogTarget { target: Name, on: bool, blend: f32 },
    SurviveFogEnd,
    AudioPathEvent { command: i32, value: i32 },
    AudioSetGameVar { var: i32, value: i32 },
    AudioSetPresentationType(i32),
    AudioPostEvent { entity: Name, event: Name, parameter: Name },
    AwardBadge { badge: Name, category: i32 },
    AwardTrackBonus { bonus: Name, flag: bool },
    AwardAchievement(Name),
    SetStatsWriteable(bool),
    BadgeProgress { badge: Name, progress: f32 },
    TutorialPopup(String),
    TutorialMessage { title: String, body: String, seconds: f32 },
    TutorialTrickReset { entity: Name, help: String },
    TutorialTrickAdd { state: i32, cmp: i32, value: i32, goofy: i32, scale: f32, count: i32, text: String },
    TutorialTrickShow { button: i32, text: String },
    TutorialTrickClear,
    BoostNotification,
    DisableSplitsCallout(bool),
    NisPrompt,
    NisTitle,
    ObjectiveMessage { title: String, body: String, seconds: f32, parameter: i32 },
    ObjectiveMessageHide,
    LoadPostFxOverlayTextures(Vec<String>),
}

pub trait World {
    fn game_state(&mut self, _entity: &Name, _id: u16) -> Option<GsValue> {
        None
    }
    fn event_type(&mut self) -> Option<i32> {
        None
    }
    fn touching_name(&mut self, _volume: &Name) -> Option<bool> {
        None
    }
    fn touching_group(&mut self, _mask: u16) -> Option<bool> {
        None
    }
    fn racer(&mut self, _entity: &Name, _q: RacerQuery) -> Option<f32> {
        None
    }
    fn equipped(&mut self, _item: i32, _equipment_type: i32) -> Option<bool> {
        None
    }
    fn night(&mut self) -> Option<bool> {
        None
    }
    fn badge_set(&mut self, _badge: &Name, _category: i32) -> Option<bool> {
        None
    }
    fn track_bonus_set(&mut self, _bonus: &Name) -> Option<bool> {
        None
    }
    fn stat(&mut self, _key: &StatKey) -> Option<StatValue> {
        None
    }
    fn tutorial_message_ready(&mut self) -> Option<bool> {
        None
    }
    fn tutorial_tricks(&mut self, _entity: &Name) -> Option<bool> {
        None
    }
    fn ui_flag(&mut self, _id: u16) -> Option<bool> {
        None
    }
    fn free_ride(&mut self) -> Option<bool> {
        None
    }
    fn drop_message(&mut self) -> Option<DropMessage> {
        None
    }
    fn effect(&mut self, _objective: &Name, _e: &Effect) {}
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct DropMessage {
    pub title: Option<String>,
    pub body: Option<String>,
    pub parameter: i32,
    pub kind: i32,
}

pub struct NoWorld;

impl World for NoWorld {}

pub struct Ctx<'a> {
    pub world: &'a mut dyn World,
    pub flags: &'a mut NamedFlags,
    pub events: &'a mut crate::game::events::PostedEvents,
    pub complete: &'a dyn Fn(u32) -> Option<bool>,
    pub objective: &'a Name,
    pub restart: bool,
    pub unprovided: &'a mut std::collections::BTreeSet<String>,
    pub log: Option<&'a mut Vec<LogEntry>>,
    pub time: f64,
}

impl Ctx<'_> {
    fn note(&mut self, what: impl FnOnce() -> String) {
        let w = what();
        if !self.unprovided.contains(&w) {
            self.unprovided.insert(w);
        }
    }

    pub fn game_state(&mut self, entity: &Name, id: u16) -> Option<GsValue> {
        let v = self.world.game_state(entity, id);
        if v.is_none() {
            self.note(|| format!("{entity}.{}", game_state_name(id)));
        }
        v
    }

    pub fn ask<T>(&mut self, what: &str, f: impl FnOnce(&mut dyn World) -> Option<T>) -> Option<T> {
        let v = f(self.world);
        if v.is_none() {
            self.note(|| what.to_string());
        }
        v
    }

    pub fn effect(&mut self, e: Effect) {
        self.world.effect(self.objective, &e);
        if let Some(log) = self.log.as_deref_mut() {
            log.push(LogEntry { time: self.time, objective: self.objective.text.to_string(), what: Logged::Effect(e) });
        }
    }

    pub fn flag(&mut self, name: &Name, value: bool, temporary: bool) {
        self.flags.set(name, value, temporary);
        if let Some(log) = self.log.as_deref_mut() {
            let what = Logged::Flag { name: name.text.to_string(), value, temporary };
            log.push(LogEntry { time: self.time, objective: self.objective.text.to_string(), what });
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct LogEntry {
    pub time: f64,
    pub objective: String,
    pub what: Logged,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Logged {
    Effect(Effect),
    Flag { name: String, value: bool, temporary: bool },
    Restarted,
    Completed,
}
