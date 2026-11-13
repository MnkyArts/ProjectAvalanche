use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context, Result};
use ssx_formats::attrib::collection;
use ssx_formats::attrib::{Schema, Value, Vault};

use crate::gear::GearDb;

mod defs;
mod finish_message;
mod results;
mod update;

pub use defs::*;
pub use finish_message::FinishMessage;
pub use results::Ranking;
pub use ssx_rules::results::{GhostEntry, PADDING_CHARACTER, ResultsFlow, RunEnd, WorldTourRule};

pub use ssx_rules::course::{Course, SPLIT_TOP_OFFSET, SPLITS};

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct EventClock {
    pub seconds: f32,
    pub running: bool,
}

pub const EVENT_CLOCK_MAX_S: f32 = 3_599.999;

impl EventClock {
    pub fn restart(&mut self) {
        (self.seconds, self.running) = (0.0, true);
    }

    pub fn stop(&mut self) {
        self.running = false;
    }

    pub fn clear(&mut self) {
        (self.seconds, self.running) = (0.0, false);
    }

    pub fn advance(&mut self, dt_s: f32) {
        if self.running {
            self.seconds += dt_s;
        }
        if self.seconds - EVENT_CLOCK_MAX_S >= 0.0 {
            self.seconds = EVENT_CLOCK_MAX_S;
        }
    }

    pub fn ms(&self) -> u32 {
        (self.seconds * 1000.0) as u32
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum EventState {
    #[default]
    Idle,
    Running,
    DelayedFinish,
    Finished,
    LoopOutro,
    LoopRestart,
    Results,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Medal {
    Gold,
    Silver,
    Bronze,
}

impl Medal {
    pub fn from_rank(rank: usize) -> Option<Self> {
        [Medal::Gold, Medal::Silver, Medal::Bronze].get(rank).copied()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct EventResult {
    pub event_type: Option<EventType>,
    pub points: u32,
    pub time_ms: u32,
    pub descent_mm: u32,
    pub rewinds: u32,
    pub loops: u32,
    pub failed: bool,
    pub value: u32,
    pub rivals: Vec<u32>,
    pub rank: Option<usize>,
    pub medal: Option<Medal>,
    pub place: u32,
    pub geotag_credits: i64,
    pub geotag_count: u32,
}

pub const FINISH_MESSAGE_HOLD_S: f32 = 1.0;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct EventInputs {
    pub dt_s: f32,
    pub rewinding: bool,
    pub player_y: f32,
    pub touching_end: bool,
    pub health: f32,
    pub allow_race_end: bool,
    pub points: f32,
    pub descent: f32,
    pub nis_outro_active: bool,
    pub racers: [Option<RacerInput>; MAX_RACERS],
    pub geotag_credits: i64,
    pub geotag_count: u32,
    pub last_safe_ground: [f32; 3],
}

pub const MAX_RACERS: usize = 6;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct RacerInput {
    pub y: f32,
    pub touching_end: bool,
    pub points: f32,
    pub trigger_wipeout: bool,
    pub in_air: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RacerProgress {
    pub split: i32,
    pub progress: f32,
    pub finish_ms: Option<u32>,
    pub points: f32,
}

impl Default for RacerProgress {
    fn default() -> Self {
        Self { split: -1, progress: 0.0, finish_ms: None, points: 0.0 }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct EventUpdate {
    pub timer: Option<crate::death::DeathTimerEvent>,
    pub split: Option<(i32, u32)>,
    pub crossed: bool,
    pub finished: bool,
    pub results: bool,
    pub looped: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct EventSetup {
    pub event_type: Option<EventType>,
    pub delayed_finish: bool,
    pub rivals: Vec<u32>,
    pub results: Option<ResultsFlow>,
    pub places: Option<i64>,
    pub goal: Option<i64>,
    pub course: Option<Course>,
    pub force_descent_in_air: bool,
    pub objectives: Option<std::sync::Arc<crate::objectives::EventObjectives>>,
    pub message: crate::objectives::DropMessage,
    pub time_up_result: bool,
    pub free_ride: bool,
}

impl EventSetup {
    pub fn for_world(world: &crate::rider::World, want: &str, start_y: f32) -> Self {
        let def = if want == "none" { None } else { choose_event(world.events.iter(), want) };
        let course = world.end_locator.map(|e| Course { start_y, end_y: e.y });
        Self::new(def, course)
    }

    pub fn new(def: Option<&EventDef>, course: Option<Course>) -> Self {
        Self {
            event_type: def.and_then(|d| d.event_type),
            delayed_finish: def.is_some_and(EventDef::delayed_finish),
            rivals: def.map_or_else(Vec::new, EventDef::ranked_rivals),
            results: Self::flow_of(def),
            places: def.and_then(|d| d.places),
            goal: def.and_then(|d| d.goal),
            course,
            force_descent_in_air: def.is_some_and(EventDef::forces_wingsuit),
            objectives: def.and_then(|d| d.objectives.clone()),
            message: def.map(|d| d.message.clone()).unwrap_or_default(),
            time_up_result: false,
            free_ride: false,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct EventManager {
    pub setup: EventSetup,
    pub state: EventState,
    pub clock: EventClock,
    pub waiting: bool,
    pub split: i32,
    pub split_times: Vec<(i32, u32)>,
    pub progress: f32,
    pub finish_ms: Option<u32>,
    pub crossed_finish_line: bool,
    pub finish_message: Option<FinishMessage>,
    pub track_finished: bool,
    pub allow_outro: bool,
    pub rewind_disabled: bool,
    pub rewinds: u32,
    pub loops: u32,
    pub racers: Vec<RacerProgress>,
    pub request_loop_to_top: bool,
    pub nis_short: bool,
    pub result: Option<EventResult>,
    pub end_position: [f32; 3],
    pub time_up: bool,
    pub round_over: Option<bool>,
}

impl EventManager {
    pub fn new(setup: EventSetup) -> Self {
        Self { setup, split: -1, ..Self::default() }
    }

    pub fn reset(&mut self) {
        *self = Self::new(std::mem::take(&mut self.setup));
    }

    pub fn end_round(&mut self, map: bool) {
        use ssx_rules::gamemode::TimeUp;
        let result = if map {
            self.setup.time_up_result
        } else {
            self.setup.event_type.is_some_and(|t| TimeUp::of(t) == TimeUp::Result)
        };
        self.round_over = Some(result);
    }

    pub fn on_trigger_race_start(&mut self, timer: &mut crate::death::DeathTimer) {
        if self.round_over.is_some() {
            return;
        }
        match self.state {
            EventState::LoopRestart => self.loop_restart(timer),
            EventState::Running => {}
            _ => {
                self.reset();
                *timer = crate::death::DeathTimer::default();
            }
        }
        self.clock.restart();
        self.track_finished = false;
        self.state = EventState::Running;
    }

    fn loop_restart(&mut self, timer: &mut crate::death::DeathTimer) {
        self.track_finished = false;
        timer.rewinding = false;
        self.clock.clear();
        (self.split, self.progress, self.finish_ms) = (-1, 0.0, None);
        self.split_times.clear();
        self.finish_message = None;
    }

    pub fn on_entered_gameplay(&mut self) {
        self.waiting = false;
    }
}

#[cfg(test)]
mod tests;
