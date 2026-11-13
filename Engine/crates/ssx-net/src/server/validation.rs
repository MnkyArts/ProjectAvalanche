use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};
use std::time::{Duration, Instant};

use bevy_ecs::prelude::*;
use bevy_time::{Real, Time};
use lightyear::prelude::{LocalTimeline, MessageSender};
use ssx_gameplay::validate::{Action, Arrival, Check, Finding, Limits, Policy, RiderValidation};
use ssx_net_proto::instance::Level;
use ssx_net_proto::{InstanceState, Mode, NetPose, ProgressReport, ServerControl};

use super::ServerStats;
use super::guard::{KickReason, Kicked, leave};
use super::instance::Instances;
use super::time::{LinkTime, Received, ServerTime};
use crate::protocol::{ControlChannel, Wire};

pub type LimitsLoader = Arc<dyn Fn(&Level, Mode) -> Result<Limits, String> + Send + Sync>;

#[derive(Resource, Clone)]
pub struct ValidationConfig {
    pub loader: LimitsLoader,
    pub policy: Policy,
    pub generation: Arc<AtomicU64>,
}

#[derive(Resource, Default)]
pub struct ValidationInbox(pub(super) Vec<Input>);

pub(super) enum Input {
    Sample { rider: u16, link: Entity, pose: Box<NetPose> },
    Ready { rider: u16, link: Entity, received: Received, sample: Box<NetPose>, second: Box<NetPose> },
    Report { rider: u16, report: Box<ProgressReport>, over: bool, received: Received },
    Gone { rider: u16 },
}

#[derive(Clone, Debug, PartialEq)]
pub struct ValidationEvent {
    pub instance: u16,
    pub round: u16,
    pub level: String,
    pub rider: Option<u16>,
    pub check: Option<Check>,
    pub action: Option<Action>,
    pub text: String,
}

#[derive(Resource, Default)]
pub struct ValidationLog {
    pub events: VecDeque<ValidationEvent>,
}

// PORT(choice): 1 024 events, a port bound (the dedicated server drains it every frame; a test app keeps the newest);
// the original has no validation.
const MAX_VALIDATION_LOG: usize = 1024;

impl ValidationLog {
    pub(super) fn push(&mut self, e: ValidationEvent) {
        if self.events.len() == MAX_VALIDATION_LOG {
            self.events.pop_front();
        }
        self.events.push_back(e);
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CheckOutcomes {
    pub made: u64,
    pub found: u64,
    pub warned: u64,
    pub invalidated: u64,
    pub kicked: u64,
}

#[derive(Resource, Clone, Debug, Default)]
pub struct ValidationMeasures(pub ssx_gameplay::validate::Counts);

#[derive(Default)]
struct DropState {
    level: Option<Level>,
    limits: Option<Arc<Limits>>,
    loading: bool,
    failures: usize,
    generation: u64,
}

struct Early {
    instance: u16,
    round: u16,
    samples: VecDeque<(Box<NetPose>, Arrival)>,
}

// PORT(choice): 64 samples (4.3 s of samples every 2 ticks), a port bound over a resent `Ready` (lightyear resends
// after about 1.5 round trips); a character of interest's own way reaches it (round net3d's coi proof: 64 for one run,
// 187 dropped) — its samples between the earliest hand-over its character may take (the short path's) and its own come
// before its `Ready`, and the run takes none not after `Ready`'s execution; tuning: `samples_before_ready_most` over
// the network proofs and their stalls, counted after the run's own hand-over; the original has no validation.
const MAX_EARLY_SAMPLES: usize = 64;

struct RiderRun {
    instance: u16,
    round: u16,
    link: Entity,
    v: RiderValidation,
    acted: [bool; 6],
}

struct Worker {
    requests: Sender<(u16, Level, Mode)>,
    results: std::sync::Mutex<Receiver<Loaded>>,
}

struct Loaded {
    instance: u16,
    level: Level,
    result: Result<Limits, String>,
    took: Duration,
}

#[derive(Resource, Default)]
pub(super) struct Validation {
    worker: Option<Worker>,
    drops: HashMap<u16, DropState>,
    riders: HashMap<u16, RiderRun>,
    early: HashMap<u16, Early>,
    begun: HashMap<u16, (u16, u16)>,
}

fn spawn_worker(loader: LimitsLoader) -> Result<Worker, String> {
    let (req_tx, req_rx) = channel::<(u16, Level, Mode)>();
    let (res_tx, res_rx) = channel::<Loaded>();
    std::thread::Builder::new()
        .name("rules loader".into())
        .spawn(move || {
            for (instance, level, mode) in req_rx {
                let t0 = Instant::now();
                let result = loader(&level, mode);
                if res_tx.send(Loaded { instance, level, result, took: t0.elapsed() }).is_err() {
                    break;
                }
            }
        })
        .map_err(|e| format!("the rules loader's thread: {e}"))?;
    Ok(Worker { requests: req_tx, results: std::sync::Mutex::new(res_rx) })
}

pub(super) fn start(world: &mut World) {
    if world.get_resource::<ValidationConfig>().is_none() {
        return;
    }
    if let Some(mut instances) = world.get_resource_mut::<Instances>() {
        for s in &mut instances.list {
            s.instance.require_rules();
        }
    }
    world.init_resource::<Validation>();
    world.init_resource::<ValidationInbox>();
    world.init_resource::<ValidationLog>();
    world.init_resource::<ValidationMeasures>();
}

struct Act<'a, 'lw, 'ls, 'cw, 'cs> {
    policy: Policy,
    instances: &'a mut Instances,
    stand_ins: &'a super::resources::StandIns,
    stats: &'a mut ServerStats,
    log: &'a mut ValidationLog,
    links: &'a mut Links<'lw, 'ls>,
    commands: &'a mut Commands<'cw, 'cs>,
    measures: &'a mut ValidationMeasures,
    wall: Duration,
}

impl Act<'_, '_, '_, '_, '_> {
    fn level_of(&self, instance: u16) -> String {
        self.instances.by_id(instance).map_or_else(String::new, |s| s.instance.spec().level.to_string())
    }

    fn findings(&mut self, rider: u16, run: &mut RiderRun, findings: Vec<Finding>) {
        for f in findings {
            let k = f.check.index();
            self.stats.validation[k].found += 1;
            if run.acted[k] {
                continue;
            }
            run.acted[k] = true;
            let action = self.policy.of(f.check);
            let level = self.level_of(run.instance);
            let current = self.instances.by_id(run.instance).is_some_and(|s| s.instance.round() == run.round);
            match action {
                Action::Warn => self.stats.validation[k].warned += 1,
                Action::Invalidate | Action::Kick => {
                    let changed = current
                        && self
                            .instances
                            .by_id_mut(run.instance)
                            .is_some_and(|s| s.instance.invalidate(rider, f.check));
                    if changed {
                        self.stats.validation[k].invalidated += 1;
                        if let Ok(mut sender) = self.links.get_mut(run.link) {
                            let told = ServerControl::Invalidated { round: run.round, check: f.check };
                            sender.send::<ControlChannel>(Wire::control(&told));
                        }
                    }
                    if action == Action::Kick {
                        self.stats.validation[k].kicked += 1;
                        let text = format!("validation: {}", f.check.reason());
                        leave(self.commands, run.link, KickReason::Validation(f.check), &text, self.wall);
                    }
                }
            }
            self.log.push(ValidationEvent {
                instance: run.instance,
                round: run.round,
                level,
                rider: Some(rider),
                check: Some(f.check),
                action: Some(action),
                text: format!("round {} at execution {:.0}: {}", run.round, f.execution, f.detail),
            });
        }
    }

    fn finish(&mut self, rider: u16, mut run: RiderRun) {
        let found = run.v.end();
        let mut by_check = [0u64; 6];
        for f in &found {
            by_check[f.check.index()] += 1;
        }
        self.findings(rider, &mut run, found);
        for c in Check::ALL {
            let (made, found) = run.v.counts.check(c);
            self.stats.validation[c.index()].made += made;
            if found > 1 {
                self.log.push(ValidationEvent {
                    instance: run.instance,
                    round: run.round,
                    level: self.level_of(run.instance),
                    rider: Some(rider),
                    check: Some(c),
                    action: None,
                    text: format!("round {}: {found} of {made} measures beyond their limit in the run", run.round),
                });
            }
        }
        self.stats.rides_validated += 1;
        self.measures.0.add(&run.v.counts);
    }
}

fn drops(state: &mut Validation, config: &ValidationConfig, act: &mut Act<'_, '_, '_, '_, '_>, now: u32) {
    if state.worker.is_none() {
        match spawn_worker(Arc::clone(&config.loader)) {
            Ok(w) => state.worker = Some(w),
            Err(why) => {
                if act.stats.rules_failed == 0 {
                    act.log.push(ValidationEvent {
                        instance: 0,
                        round: 0,
                        level: String::new(),
                        rider: None,
                        check: None,
                        action: None,
                        text: why,
                    });
                }
                act.stats.rules_failed += 1;
                return;
            }
        }
    }
    let mut stopped = false;
    while let Some(w) = state.worker.as_ref() {
        let received = match w.results.lock() {
            Ok(results) => results.try_recv(),
            Err(_) => Err(TryRecvError::Disconnected),
        };
        let loaded = match received {
            Ok(l) => l,
            Err(TryRecvError::Empty) => break,
            Err(TryRecvError::Disconnected) => {
                stopped = true;
                break;
            }
        };
        let Some(d) = state.drops.get_mut(&loaded.instance).filter(|d| d.level.as_ref() == Some(&loaded.level)) else {
            continue;
        };
        d.loading = false;
        if d.generation != config.generation.load(Ordering::Acquire) {
            continue;
        }
        let Some(s) = act.instances.by_id_mut(loaded.instance) else { continue };
        let ms = loaded.took.as_secs_f64() * 1000.0;
        let round = s.instance.round();
        let text = match loaded.result {
            Ok(limits) => {
                s.instance.set_results_at_limit(limits.results_at_time_up());
                d.limits = Some(Arc::new(limits));
                d.failures = 0;
                s.instance.set_rules_ready(true);
                act.stats.rules_loaded += 1;
                format!("its rules loaded in {ms:.0} ms")
            }
            Err(why) => {
                d.failures += 1;
                act.stats.rules_failed += 1;
                if d.failures < s.instance.rotation().len() && s.instance.skip_drop(now) {
                    format!("its rules did not load ({ms:.0} ms): {why}; the rotation moves on")
                } else {
                    format!(
                        "its rules did not load ({ms:.0} ms): {why}; no level of the rotation loads, the lobby holds"
                    )
                }
            }
        };
        act.log.push(ValidationEvent {
            instance: loaded.instance,
            round,
            level: loaded.level.to_string(),
            rider: None,
            check: None,
            action: None,
            text,
        });
    }
    if stopped {
        state.worker = None;
        for d in state.drops.values_mut() {
            d.loading = false;
        }
    }
    let generation = config.generation.load(Ordering::Acquire);
    for s in &mut act.instances.list {
        if !s.instance.spec().mode.has_rounds() {
            continue;
        }
        let (id, level, mode) = (s.instance.id(), s.instance.spec().level.clone(), s.instance.spec().mode);
        let d = state.drops.entry(id).or_default();
        if d.level.as_ref() != Some(&level) {
            let (failures, stamp) = (d.failures, d.generation);
            *d = DropState { level: Some(level.clone()), failures, generation: stamp, ..DropState::default() };
        }
        if d.generation != generation && !s.instance.rules_ready() && !d.loading {
            (d.limits, d.failures, d.generation) = (None, 0, generation);
        }
        if let Some(limits) = d.limits.as_ref() {
            if !s.instance.rules_ready() {
                s.instance.set_results_at_limit(limits.results_at_time_up());
                s.instance.set_rules_ready(true);
            }
            continue;
        }
        if d.loading || d.failures >= s.instance.rotation().len().max(1) {
            continue;
        }
        if let Some(w) = state.worker.as_ref()
            && w.requests.send((id, level, mode)).is_ok()
        {
            (d.loading, d.generation) = (true, generation);
        }
    }
}

fn character_of(
    s: &super::instance::ServerInstance,
    rider: u16,
    stand_ins: &super::resources::StandIns,
) -> Option<u32> {
    s.instance.progress(rider).and_then(|p| p.appearance()).map(|a| stand_ins.of(a.character))
}

type Links<'w, 's> = Query<'w, 's, &'static mut MessageSender<Wire<ServerControl>>, Without<Kicked>>;

type Books<'w> =
    (Option<ResMut<'w, ValidationInbox>>, Option<ResMut<'w, ValidationLog>>, Option<ResMut<'w, ValidationMeasures>>);

pub(super) fn run(
    (state, config): (Option<ResMut<Validation>>, Option<Res<ValidationConfig>>),
    (instances, stand_ins): (Option<ResMut<Instances>>, Option<Res<super::resources::StandIns>>),
    (inbox, log, measures): Books,
    mut stats: ResMut<ServerStats>,
    (timeline, time, clock): (Res<LocalTimeline>, Res<Time<Real>>, Res<ServerTime>),
    (mut links, link_times): (Links, Query<&LinkTime>),
    mut commands: Commands,
) {
    let (Some(mut state), Some(config), Some(mut instances), Some(mut inbox), Some(mut log), Some(mut measures)) =
        (state, config, instances, inbox, log, measures)
    else {
        return;
    };
    let state = &mut *state;
    let now = timeline.tick().0;
    let none = super::resources::StandIns::default();
    let mut act = Act {
        policy: config.policy,
        instances: &mut instances,
        stand_ins: stand_ins.as_deref().unwrap_or(&none),
        stats: &mut stats,
        log: &mut log,
        links: &mut links,
        commands: &mut commands,
        measures: &mut measures,
        wall: time.elapsed(),
    };
    drops(state, &config, &mut act, now);
    let read = clock.received();
    let arrival = |instances: &Instances, (instance, round): (u16, u16), link: Entity, received: Received| {
        let link = link_times.get(link).ok();
        clock.arrival(instances, (instance, round), now, link, received)
    };
    for input in std::mem::take(&mut inbox.0) {
        match input {
            Input::Ready { rider, link, received, sample, second } => {
                if let Some(old) = state.riders.remove(&rider) {
                    act.finish(rider, old);
                }
                let early = state.early.remove(&rider);
                let Some(s) = act.instances.of_rider(rider) else { continue };
                let (id, round, mode) = (s.instance.id(), s.instance.round(), s.instance.spec().mode);
                let character = character_of(s, rider, act.stand_ins);
                state.begun.insert(rider, (id, round));
                let Some(limits) = state.drops.get(&id).and_then(|d| d.limits.clone()) else { continue };
                let v = RiderValidation::new(limits, mode, character);
                let mut run = RiderRun { instance: id, round, link, v, acted: [false; 6] };
                let Some(at) = arrival(act.instances, (id, round), link, received) else { continue };
                let mut found = run.v.ready(&sample, &second, at);
                let go = sample.time() - f64::from(sample.execution());
                for (pose, at) in
                    early.filter(|e| (e.instance, e.round) == (id, round)).into_iter().flat_map(|e| e.samples)
                {
                    if pose.time() < go {
                        act.stats.samples_before_ready_other_run += 1;
                        continue;
                    }
                    found.extend(run.v.sample(&pose, at));
                }
                act.findings(rider, &mut run, found);
                state.riders.insert(rider, run);
            }
            Input::Sample { rider, link, pose } => {
                if let Some(run) = state.riders.get_mut(&rider) {
                    if let Some(at) = arrival(act.instances, (run.instance, run.round), run.link, read) {
                        let found = run.v.sample(&pose, at);
                        act.findings(rider, run, found);
                    }
                    continue;
                }
                let Some(s) = act.instances.of_rider(rider) else { continue };
                let (id, round) = (s.instance.id(), s.instance.round());
                if state.begun.get(&rider) == Some(&(id, round)) {
                    continue;
                }
                let Some(limits) = state.drops.get(&id).and_then(|d| d.limits.as_ref()) else { continue };
                if f64::from(pose.execution()) <= limits.earliest_handover(character_of(s, rider, act.stand_ins)) {
                    continue;
                }
                let Some(at) = arrival(act.instances, (id, round), link, read) else { continue };
                let e =
                    state.early.entry(rider).or_insert_with(|| Early { instance: id, round, samples: VecDeque::new() });
                if (e.instance, e.round) != (id, round) {
                    *e = Early { instance: id, round, samples: VecDeque::new() };
                }
                if e.samples.len() == MAX_EARLY_SAMPLES {
                    e.samples.pop_front();
                    act.stats.samples_before_ready_dropped += 1;
                }
                e.samples.push_back((pose, at));
                act.stats.samples_before_ready += 1;
                act.stats.samples_before_ready_most = act.stats.samples_before_ready_most.max(e.samples.len() as u64);
            }
            Input::Report { rider, report, over, received } => {
                if let Some(run) = state.riders.get_mut(&rider)
                    && let Some(at) = arrival(act.instances, (run.instance, run.round), run.link, received)
                {
                    let found = run.v.report(&report, at);
                    act.findings(rider, run, found);
                }
                if over && let Some(run) = state.riders.remove(&rider) {
                    act.finish(rider, run);
                }
            }
            Input::Gone { rider } => {
                state.early.remove(&rider);
                state.begun.remove(&rider);
                if let Some(run) = state.riders.remove(&rider) {
                    act.finish(rider, run);
                }
            }
        }
    }
    state.early.retain(|_, e| {
        act.instances.by_id(e.instance).is_some_and(|s| {
            s.instance.round() == e.round
                && matches!(s.instance.state(), InstanceState::Countdown | InstanceState::Running)
        })
    });
    state.begun.retain(|_, &mut (id, round)| act.instances.by_id(id).is_some_and(|s| s.instance.round() == round));
    let ended: Vec<u16> = state
        .riders
        .iter()
        .filter(|(_, run)| {
            act.instances.by_id(run.instance).is_none_or(|s| {
                s.instance.round() != run.round
                    || !matches!(s.instance.state(), InstanceState::Countdown | InstanceState::Running)
            })
        })
        .map(|(&r, _)| r)
        .collect();
    for rider in ended {
        if let Some(run) = state.riders.remove(&rider) {
            act.finish(rider, run);
        }
    }
}
