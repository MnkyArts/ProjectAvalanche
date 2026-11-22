use crate::death::{DeathState, DeathTimer, DeathTimerEvent, GameFlowFlags, RiderDead};
use crate::event::{EventInputs, EventManager, EventUpdate};
use crate::graph::RiderGraph;
use crate::rewind::{RecordFlags, RewindBuffer, RewindManager, RewindPad, RewindStep, RewindTuning};
use crate::rider::{Input, RiderState, StepContext, TrickInput, World, scoring_context, step_with_trigger};
use crate::triggers::TriggerTracker;
use crate::{DT_TICKS, TICKS_PER_SECOND};

pub mod bots;
pub mod dilation;
pub mod geotags;

pub const LOOP_STEP_S: f32 = DT_TICKS / TICKS_PER_SECOND;
pub mod heli_jump;
pub mod recorder;
pub mod survive;
pub mod tombstones;
pub mod tutorial;

mod execute;
mod scripts;

pub use scripts::{ScriptsBuild, ScriptsInputs};

#[derive(Clone, Debug)]
pub struct Session {
    pub buffer: RewindBuffer,
    pub manager: RewindManager,
    pub dilation: f32,
    pub time_dilation: dilation::TimeDilation,
    pub executed_dilation: (dilation::DilationState, f32),
    pub tracker: TriggerTracker,
    pub timer: DeathTimer,
    pub flow: GameFlowFlags,
    pub event_ended: bool,
    pub pads_released: bool,
    pub event: EventManager,
    pub ui_rewind_count: i32,
    pub avalanche: crate::avalanche::AvalancheSim,
    pub survive: Option<survive::SurviveLoop>,
    pub bots: Option<bots::BotRace>,
    pub ghost_manager: bots::GhostLatches,
    pub recorder: recorder::GhostRecorder,
    pub ui_ghost_clock_ms: u32,
    pub heli_jump: Option<heli_jump::HeliJump>,
    pub tutorial: Option<tutorial::Tutorial>,
    pub nis_events: crate::nis_events::NisEvents,
    pub scripts: Option<std::sync::Arc<crate::objectives::LevelScripts>>,
    pub geotags: Option<geotags::GeotagRun>,
    pub tombstones: Option<tombstones::Tombstones>,
    pub objectives: Option<crate::objectives::LevelObjectives>,
    pub controllers: Option<crate::controllers::Controllers>,
    pub controllers_log: bool,
    pub workers: Option<std::sync::Arc<dyn crate::workers::Workers>>,
    pub named_entities: Vec<crate::controllers::NamedEntity>,
    pub flag_overrides: Vec<(String, bool)>,
    pub objectives_loaded: bool,
    pub scripts_ahead: Option<ScriptsBuild>,
    pub scripts_built_ahead: bool,
    pub objectives_log: bool,
    pub screen: crate::outro::ScreenFade,
    pub stats: crate::stats::StatsStore,
    pub stats_record: crate::scoring::TrickRecord,
    pub rewind_backwards: bool,
    pub named_flag_seed: std::collections::BTreeSet<u32>,
    pub ui_flags: std::collections::BTreeMap<u16, bool>,
    pub head_pending: bool,
    pub racer_end: [bool; crate::event::MAX_RACERS],
    pub effects_fresh: bool,
    pub effects_from: usize,
    pub last_sim_dt_s: f32,
    pub downwash: Option<crate::helicopter::downwash::Downwash>,
    pub presentation: i32,
    pub postfx: ssx_assets::postfx_state::PostFxService,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Execution {
    pub rewind: RewindStep,
    pub dt: f32,
    pub was_alive: bool,
    pub rider_dead: Option<RiderDead>,
    pub timer: DeathTimerEvent,
    pub recorded: Option<usize>,
    pub race_start: bool,
    pub event: EventUpdate,
    pub survive: survive::SurviveStep,
}

impl Default for Session {
    fn default() -> Self {
        Self::new(RewindTuning::default())
    }
}

impl Session {
    pub fn new(tuning: RewindTuning) -> Self {
        Session {
            buffer: RewindBuffer::new(),
            manager: RewindManager::new(tuning),
            dilation: 1.0,
            time_dilation: dilation::TimeDilation::default(),
            executed_dilation: (dilation::DilationState::None, 1.0),
            tracker: TriggerTracker::default(),
            timer: DeathTimer::default(),
            flow: GameFlowFlags::default(),
            event_ended: false,
            pads_released: false,
            event: EventManager::default(),
            ui_rewind_count: 0,
            avalanche: crate::avalanche::AvalancheSim::default(),
            survive: None,
            bots: None,
            ghost_manager: bots::GhostLatches::default(),
            recorder: recorder::GhostRecorder::default(),
            ui_ghost_clock_ms: 0,
            heli_jump: None,
            tutorial: None,
            nis_events: crate::nis_events::NisEvents::default(),
            scripts: None,
            geotags: None,
            tombstones: None,
            objectives: None,
            controllers: None,
            controllers_log: false,
            workers: None,
            named_entities: Vec::new(),
            flag_overrides: Vec::new(),
            objectives_loaded: false,
            scripts_ahead: None,
            scripts_built_ahead: false,
            objectives_log: false,
            screen: crate::outro::ScreenFade::default(),
            rewind_backwards: false,
            ui_flags: std::collections::BTreeMap::new(),
            named_flag_seed: Default::default(),
            stats: crate::stats::StatsStore::default(),
            stats_record: Default::default(),
            head_pending: true,
            racer_end: [false; crate::event::MAX_RACERS],
            effects_fresh: true,
            effects_from: 0,
            last_sim_dt_s: 0.0,
            downwash: None,
            presentation: 0,
            postfx: ssx_assets::postfx_state::PostFxService::default(),
        }
    }

    pub fn heli_jump_update(
        &mut self,
        s: &mut crate::rider::RiderState,
        w: &crate::rider::World,
        heli: Option<&mut crate::helicopter::Helicopter>,
        nodes: Option<&[String]>,
    ) -> anyhow::Result<heli_jump::HeliJumpUpdate> {
        let dt_s = LOOP_STEP_S;
        let Some(h) = self.heli_jump.as_mut() else { return Ok(heli_jump::HeliJumpUpdate::default()) };
        h.contact_query(s, heli.as_deref(), nodes);
        let before = h.intercept;
        let nis_active = self.flow.nis_active || s.start.is_some();
        self.update_objectives(s, w, nis_active, dt_s);
        let effects = self.objectives.as_ref().map_or(&[][..], |o| o.effects.as_slice());
        let next_dt = DT_TICKS * self.dilation;
        match self.heli_jump.as_mut() {
            Some(h) => h.apply_objectives(s, w, &self.event, effects, before, next_dt),
            None => Ok(heli_jump::HeliJumpUpdate::default()),
        }
    }

    pub fn executed_time_dilation(&self) -> (bool, bool, f32) {
        let (state, factor) = self.executed_dilation;
        (state != dilation::DilationState::None, state == dilation::DilationState::HeliTrick, factor)
    }

    pub fn reset(&mut self) {
        if let Some(o) = self.objectives.as_mut() {
            o.restart_level();
        }
        if let Some(t) = self.tutorial.as_mut() {
            t.message.request(tutorial::HudMessage::READY, 0.0);
        }
        self.manager.restart(&mut self.buffer);
        self.recorder.rewind_node_rebuilt();
        (self.dilation, self.time_dilation) = (1.0, dilation::TimeDilation::default());
        self.tracker = TriggerTracker::default();
        self.timer = DeathTimer::default();
        self.event_ended = false;
        self.pads_released = false;
        if let Some(c) = self.controllers.as_mut() {
            c.reset_bodies();
        }
        self.event.reset();
        self.flow.track_finished = false;
        self.avalanche.warp();
        if let Some(b) = self.bots.as_mut() {
            b.reset();
        }
        self.ghost_manager = bots::GhostLatches::default();
        self.recorder.on_restart();
        if let Some(t) = self.tombstones.as_mut() {
            t.reset();
        }
        (self.head_pending, self.racer_end) = (true, [false; crate::event::MAX_RACERS]);
        self.downwash = None;
        if let Some(h) = self.heli_jump.as_mut() {
            h.restart();
        }
    }

    pub fn pause_begin(&mut self) {
        self.event.waiting = true;
        self.recorder.pause(true);
    }

    pub fn pause_end(&mut self) {
        self.event.waiting = false;
        self.recorder.pause(false);
    }

    pub fn paused_frame(&mut self, s: &mut RiderState, w: &World) -> EventUpdate {
        let rewinding = self.rewind_backwards;
        if let Some(t) = self.tutorial.as_mut() {
            t.update(LOOP_STEP_S);
        }
        self.recorder.sample(s.traj, s.score.score);
        self.recorder.after_execution(None);
        self.score_manager(s, w, self.last_sim_dt_s, rewinding, true)
    }

    pub fn update_live_racers(&mut self, now: f64, player: glam::Vec3) {
        let Some(b) = self.bots.as_mut() else { return };
        for k in b.update_live(now, player) {
            if let Some(r) = self.event.racers.get_mut(k) {
                *r = crate::event::RacerProgress::default();
            }
            self.racer_end[k] = false;
        }
    }

    pub fn rewinding(&self) -> bool {
        self.buffer.is_active()
    }

    pub fn seed_named_flags(&mut self, keys: impl IntoIterator<Item = u32>) {
        for key in keys {
            self.set_named_flag(key, true);
        }
    }

    pub fn set_ui_flag(&mut self, id: u16, on: bool) {
        self.ui_flags.insert(id, on);
    }

    pub fn permanent_named_flags(&self) -> Vec<u32> {
        match &self.objectives {
            Some(o) => o.runner.flags.permanent_hashes(),
            None => self.named_flag_seed.iter().copied().collect(),
        }
    }

    pub fn set_named_flag(&mut self, key: u32, value: bool) {
        if value {
            self.named_flag_seed.insert(key);
        } else {
            self.named_flag_seed.remove(&key);
        }
        if let Some(o) = self.objectives.as_mut() {
            o.runner.flags.set_permanent_hash(key, value);
        }
    }
}

#[cfg(test)]
mod tests;
