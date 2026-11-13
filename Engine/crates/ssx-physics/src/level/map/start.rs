use anyhow::{Context, Result, bail};
use glam::{Quat, Vec3};

use super::MapLevel;
use crate::character::GearStats;
use crate::event::EventManager;
use crate::rewind::RewindPad;
use crate::rider::{Input, RiderState, TrickInput};
use crate::session::Session;

pub const START_FIRST: usize = ssx_rules::intro::START_FIRST;

pub const START_IDLE: usize = 30;

const MAX_HANDOVER: usize = 600;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MapStart {
    pub handover: u32,
    pub slot: Vec3,
    pub rotation: Quat,
    pub place: Vec3,
    pub first: [Vec3; START_FIRST],
}

impl MapStart {
    pub fn in_slot_frame(&self, p: Vec3) -> Vec3 {
        self.rotation.inverse() * (p - self.slot)
    }
}

pub type StartControls<'a> = dyn FnMut(usize, &RiderState) -> Input + 'a;

pub fn neutral(_: usize, _: &RiderState) -> Input {
    Input::default()
}

pub fn map_start(
    level: &MapLevel,
    controls: &mut StartControls<'_>,
    graph: Option<&mut crate::graph::RiderGraph>,
) -> Result<MapStart> {
    map_start_after(level, START_IDLE, controls, graph)
}

pub fn start_rider(level: &MapLevel) -> Result<(Vec3, Quat, RiderState)> {
    let w = &level.world;
    let (slot, rotation) = w
        .starts
        .get(level.drop)
        .and_then(|d| d.slot(0))
        .context("the map has no spawn slot 0 (and its base drop no start slot)")?;
    let (at, q) = crate::spawn::level_start_pose(w, level.drop, None, None)?;
    let rider = match &level.start_intro {
        Some(i) => crate::nis::level_start_or_below(w, at, q, i.clone()).with_context(|| {
            format!("no ground below spawn slot 0 at {at:.2}: the helicopter intro's start needs a surface below it")
        })?,
        None => RiderState::level_start(w, at, q).with_context(|| {
            format!(
                "no ground below spawn slot 0 at {at:.2}: the level start stands the rider on the surface from {} m \
                 above it to {} m below",
                crate::rider::SPAWN_RAY_UP,
                crate::rider::SPAWN_RAY_LENGTH - crate::rider::SPAWN_RAY_UP
            )
        })?,
    };
    Ok((slot, rotation, rider))
}

pub fn start_deaths(level: &MapLevel, deaths: &[ssx_content::map::Volume], rider: &RiderState) -> StartDeaths {
    deaths_at(&level.world.triggers, deaths, rider.traj)
}

pub fn deaths_at(triggers: &crate::triggers::TriggerSet, deaths: &[ssx_content::map::Volume], at: Vec3) -> StartDeaths {
    use crate::triggers::{IG_DEATH, IG_PLAYER, PLAYER_TOUCH_RADIUS};
    let all = triggers
        .touching(at, PLAYER_TOUCH_RADIUS, IG_PLAYER)
        .filter(|&i| triggers.volumes[i].groups & IG_DEATH != 0)
        .count();
    let nodes: Vec<String> = deaths
        .iter()
        .filter(|v| {
            let made = super::trigger_volume(v, "death", 0);
            made.interacts & IG_PLAYER != 0
                && made.groups & IG_DEATH != 0
                && made.overlaps_sphere(at, PLAYER_TOUCH_RADIUS)
        })
        .map(|v| v.node.clone())
        .collect();
    StartDeaths { base: all.saturating_sub(nodes.len()), nodes, at }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct StartDeaths {
    pub nodes: Vec<String>,
    pub base: usize,
    pub at: Vec3,
}

impl StartDeaths {
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty() && self.base == 0
    }
}

pub fn map_start_after(
    level: &MapLevel,
    idle: usize,
    controls: &mut StartControls<'_>,
    mut graph: Option<&mut crate::graph::RiderGraph>,
) -> Result<MapStart> {
    let w = &level.world;
    let (slot, rotation, mut rider) = start_rider(level)?;
    let mut session = Session { event: EventManager::new(level.setup.clone()), ..Session::default() };
    let mut gates = level.gates.clone();
    gates.reset(&mut session);
    let rotation = rotation.normalize();
    let frame = |p: Vec3| rotation.inverse() * (p - slot);
    let mut handover: Option<(u32, Vec3)> = None;
    let mut first = [Vec3::ZERO; START_FIRST];
    for i in 0..idle + MAX_HANDOVER + START_FIRST {
        let at_start = rider.start.is_some();
        let input = match i.cmp(&idle) {
            std::cmp::Ordering::Less => Input::default(),
            std::cmp::Ordering::Equal => Input { jump: true, ..Input::default() },
            std::cmp::Ordering::Greater => controls(i - idle, &rider),
        };
        let pad = RewindPad { flip_accel: input.flip_acceleration(rider.tricks.classic), ..RewindPad::default() };
        session.execute(&mut rider, w, &input, &TrickInput::default(), 0.0, &pad, graph.as_deref_mut());
        gates.update(&mut session, rider.traj);
        if !rider.traj.is_finite() {
            bail!("the rider's trajectory point is not finite at the start's execution {i}");
        }
        let exec = (i - idle.min(i)) as u32;
        match handover {
            None if at_start && rider.start.is_none() => {
                if i <= idle {
                    bail!("the level start handed over before the GO (execution {i})");
                }
                handover = Some((exec, rider.traj));
            }
            None => {}
            Some((h, p)) => {
                let k = (exec - h) as usize;
                first[k - 1] = rotation.inverse() * (rider.traj - p);
                if k == START_FIRST {
                    return Ok(MapStart { handover: h, slot, rotation, place: frame(p), first });
                }
            }
        }
        if i == idle + MAX_HANDOVER && handover.is_none() {
            break;
        }
    }
    bail!("the level start did not hand over within {MAX_HANDOVER} executions of the GO")
}

pub const ENVELOPE_INPUTS: [(&str, StartInput); 10] = [
    ("none", neutral),
    ("tuck", |_, _| Input { tuck: 1.0, ..Input::default() }),
    ("left", |_, _| Input { steer: -1.0, ..Input::default() }),
    ("right", |_, _| Input { steer: 1.0, ..Input::default() }),
    ("brake", |_, _| Input { brake: 1.0, ..Input::default() }),
    ("jump", |_, _| Input { jump: true, ..Input::default() }),
    ("tuck-left", |_, _| Input { tuck: 1.0, steer: -1.0, ..Input::default() }),
    ("tuck-right-jump", |_, _| Input { tuck: 1.0, steer: 1.0, jump: true, ..Input::default() }),
    ("flick", |k, _| Input { tuck: if k % 2 == 1 { 0.0 } else { 1.0 }, ..Input::default() }),
    ("A-again", |k, _| Input { jump: k % 2 == 0, ..Input::default() }),
];

pub type StartInput = fn(usize, &RiderState) -> Input;

pub type BoxedInput = Box<dyn Fn(usize, &RiderState) -> Input>;

pub const ENVELOPE_SEQUENCES: u64 = 32;

pub fn envelope_sequence(n: u64) -> impl Fn(usize, &RiderState) -> Input {
    move |k, _| sequence_input(0x6E65_7438_0000_0000 ^ n, k)
}

pub fn sequence_input(seed: u64, k: usize) -> Input {
    let mut x = seed ^ (k as u64 + 1).wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    let mut unit = || {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        (x >> 40) as f32 / (1u64 << 24) as f32
    };
    let steer = unit() * 2.0 - 1.0;
    let tuck = if unit() < 0.5 { 0.0 } else { unit() };
    let brake = if unit() < 0.7 { 0.0 } else { unit() };
    let jump = unit() < 0.2;
    Input { steer, tuck, brake, jump, ..Input::default() }
}

pub fn envelope_gears() -> [GearStats; 3] {
    [GearStats::uniform(0.0), GearStats::default_gear(), GearStats::uniform(1.0)]
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StartEnvelope {
    pub start: MapStart,
    pub place: f32,
    pub first: [f32; START_FIRST],
}

pub fn start_envelope(level: &mut MapLevel, mut graph: Option<&mut crate::graph::RiderGraph>) -> Result<StartEnvelope> {
    let gears = envelope_gears();
    level.world.props = level.world.tuning.resolve_gear(&gears[1])?;
    let start = start_with(level, &mut neutral, &mut graph)?;
    let (mut place, mut first) = (0.0f32, [0.0f32; START_FIRST]);
    let mut outcome = Ok(());
    let mut inputs: Vec<(String, BoxedInput)> =
        ENVELOPE_INPUTS.iter().map(|&(n, f)| (n.to_string(), Box::new(f) as BoxedInput)).collect();
    for n in 0..ENVELOPE_SEQUENCES {
        inputs.push((format!("sequence {n}"), Box::new(envelope_sequence(n))));
    }
    for gear in &gears {
        level.world.props = level.world.tuning.resolve_gear(gear)?;
        for (name, input) in &inputs {
            let mut controls = |k: usize, r: &RiderState| input(k, r);
            let s = match start_with(level, &mut controls, &mut graph) {
                Ok(s) => s,
                Err(e) => {
                    outcome = Err(e.context(format!("the start with the input {name}")));
                    break;
                }
            };
            if s.handover != start.handover {
                outcome = Err(anyhow::anyhow!(
                    "the start with the input {name} hands over at execution {}, the default start's at {}",
                    s.handover,
                    start.handover
                ));
                break;
            }
            place = place.max(s.place.distance(start.place));
            for ((most, at), at_start) in first.iter_mut().zip(&s.first).zip(&start.first) {
                *most = most.max(at.distance(*at_start));
            }
        }
        if outcome.is_err() {
            break;
        }
    }
    level.world.props = level.world.tuning.resolve_gear(&gears[1])?;
    outcome?;
    Ok(StartEnvelope { start, place, first })
}

fn start_with(
    level: &MapLevel,
    controls: &mut StartControls<'_>,
    graph: &mut Option<&mut crate::graph::RiderGraph>,
) -> Result<MapStart> {
    if let Some(g) = graph.as_deref_mut() {
        g.reset();
    }
    map_start(level, controls, graph.as_deref_mut())
}
