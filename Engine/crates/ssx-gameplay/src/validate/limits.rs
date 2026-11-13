use glam::{Quat, Vec3};
use ssx_rules::course::SPLITS;
use ssx_rules::events::EventType;
use ssx_rules::gamemode::TimeUp;
use ssx_rules::intro::{IntroStart, START_FIRST};
use ssx_rules::scoring as table;
use ssx_rules::subset::{DropRules, TuningLimits};
use ssx_rules::triggers::{PLAYER_TOUCH_RADIUS, TriggerVolume, VolumeShape};

pub const TICKS_PER_STEP: f32 = 2.0;

pub const SAMPLE_INTERVAL_TICKS: f64 = 2.0;

pub const VELOCITY_ENCODING: f32 = 0.001 + 1.0 / 1024.0 + 1.0 / 511.0;

// PORT(choice): 1.163 — the honest largest horizontal speeds 1.4319, 1.4050, 1.3778 m/tick against the tuning's 1.2781
// (with the encoding): limit 1.4860 (321 km/h); tuning: the collision correction's largest displacement in one
// execution (the 24-ray fan's push-out, decoded in the riding logic); the original has no validation.
pub const HORIZONTAL_MARGIN: f32 = 1.163;

// PORT(choice): 1.165 m whole, 1.104 m horizontal — the honest largest steps beyond the path are 1.0203, 1.0203, 0.8762
// m whole (a collision's push up, exp_rockies_robson_drop_1_survive) and 0.9876, 0.8713, 0.8713 m horizontal (3.9 M
// sample pairs); the velocity change's share 1.0 (its whole travel); tuning: the collision correction's largest push in
// one execution (the 24-ray fan's push-out); the original has no validation.
pub const STEP_SLACK_M: f32 = 1.165;
pub const STEP_SLACK_HORIZONTAL_M: f32 = 1.104;
pub const STEP_VELOCITY_CHANGE: f32 = 1.0;

// PORT(choice): 0.727 m from 4 m cells — the honest largest landings 0.5505 m (the network proof on the execution
// counts, 2026-10-31), 0.4644 m (the first network proof, a lost sample's gap), 0.3741 m (offline) from the cells of
// the path ridden (49 971 warps, rewinds and respawns offline, 7 086 in that proof): the largest plus its distance from
// the third (0.555 m before that proof); the cell a port choice too (65 536 cells of an hour's run); tuning: the rewind
// buffer's snapshots on the wire (verified runs, phase 11); the original has no validation.
pub const REACH_M: f32 = 0.727;
pub const GRID_CELL_M: f32 = 4.0;
// PORT(choice): 65 536 cells (about 1.5 MB a rider at most), a port bound above an hour's run; tuning: the longest
// round a server allows (`round_limit`'s 3 600 s); the original has no validation.
pub const MAX_PLACES: usize = 65_536;

// PORT(choice): two executions of the fastest fall (2 × 2 ticks × 1.176 m/tick = 4.7 m); the honest splits lie at most
// 0.0007 m above their line and none deeper than one execution; tuning: the client's executions without a block counted
// (a respawn's), two in a row never seen; the original has no validation.
pub const SPLIT_EXECUTIONS: f32 = 2.0;

// PORT(choice): 0.501 m — the honest finishes' paths came at most 0.8473, 0.8441, 0.8433 m from the `end` volume when
// their finish was checked (694 finishes); tuning: a touch flag on the wire (the score manager's finish execution); the
// original has no validation.
pub const TOUCH_MARGIN_M: f32 = 0.501;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Start {
    pub handover: f64,
    pub slot: Vec3,
    pub rotation: Quat,
    pub place: Vec3,
    pub margin: f32,
    pub first: [Vec3; START_FIRST],
    pub first_margins: [f32; START_FIRST],
}

impl Start {
    pub fn of(rules: &DropRules) -> Result<Self, String> {
        Self::of_way(rules, &rules.intro.start)
    }

    pub fn of_way(rules: &DropRules, go: &IntroStart) -> Result<Self, String> {
        let (slot, rotation) =
            rules.start.slot(0).ok_or_else(|| format!("{}: its start locator has no slot", rules.event.name))?;
        Ok(Self {
            handover: f64::from(go.handover_execution()),
            slot,
            rotation: rotation.normalize(),
            place: Vec3::from_array(go.handover),
            margin: ssx_rules::intro::HANDOVER_MARGIN_M,
            first: go.first.map(Vec3::from_array),
            first_margins: go.first_margins(),
        })
    }

    pub fn place_distance(&self, p: Vec3) -> f32 {
        IntroStart::in_slot_frame(p, self.slot, self.rotation).distance(self.place)
    }

    pub fn handover_place(&self) -> Vec3 {
        self.slot + self.rotation * self.place
    }
}

pub const CLOCK_BEHIND_TICKS: f64 = 0.03;

// PORT(choice): 0.05 m in a step of two executions (0.0125 m per tick, below the samples' quantisation of a slow
// rider's speed); tuning: the dilation on the wire (`FLT_TimeDilation` in the sample); the original has no validation.
pub const DILATION_MIN_TRAVEL_M: f32 = 0.05;

pub fn clock_step_s() -> f64 {
    f64::from(TICKS_PER_STEP / 60.0)
}

pub fn clock_rounding_s(steps: f64) -> f64 {
    let d = clock_step_s();
    let n = steps.max(0.0).floor().min(4.0e6);
    let mut out = 0.0;
    let mut k = 1.0f64;
    while k <= n {
        let e = (k * d).log2().floor();
        let next = ((2f64.powf(e + 1.0) / d).ceil()).max(k + 1.0).min(n + 1.0);
        out += (next - k) * 0.5 * 2f64.powf(e - 23.0);
        k = next;
    }
    out
}

pub fn clock_max_ticks(steps: f64) -> f64 {
    let s = steps.max(0.0);
    (s * clock_step_s() + clock_rounding_s(s)) * f64::from(ssx_net_proto::TICK_HZ)
}

pub fn clock_rounding_between_ticks(from: f64, to: f64) -> f64 {
    ((clock_rounding_s(to) - clock_rounding_s(from)).max(0.0)) * f64::from(ssx_net_proto::TICK_HZ)
}

#[derive(Clone, Debug)]
pub struct Limits {
    pub drop: String,
    pub event_type: Option<EventType>,
    pub splits: [f32; SPLITS],
    pub starts: Vec<Vec3>,
    pub end: Vec<TriggerVolume>,
    pub fall: f32,
    pub horizontal: f32,
    pub split_depth: f32,
    pub tuning: TuningLimits,
    pub start: Start,
    pub rider_starts: Vec<RiderStart>,
    pub gates: Vec<TriggerVolume>,
    pub time_limit_ms: Option<u32>,
    pub time_up: TimeUp,
    pub map: bool,
}

// PORT(choice): 0.0042 m — the honest starts' farthest beyond the envelope 0.0021 m (alps0101 layout halfway down: a
// random gear with a random input sequence), 0.0000 m at the eleven other spots (22 032 starts: 8 spots of the alps0101
// layout and 4 of the template slope, 27 gears — the envelope's 3 and 24 random — × 34 inputs — its 10 held ones and 24
// random sequences — × 2 riders); tuning: the server re-simulating each rider's start with its own gear and inputs
// (phase 11's verified runs: the input log); the original has no validation.
pub const MAP_HANDOVER_MARGIN_M: f32 = 0.0042;
// PORT(choice): 0.0096, 0.0187, 0.0368, 0.0329 m — the spots' farthest beyond the envelope after 1, 2, 3 and 4
// executions 0.0074 / 0.0147 / 0.0292 / 0.0291 m, the third 0.0052 / 0.0107 / 0.0216 / 0.0253 m (the same 22 032
// starts; the farthest on the steep spots of the layout, growing by about 7 mm an execution: a random gear's speed
// beyond the envelope's three); tuning: as `MAP_HANDOVER_MARGIN_M`; the original has no validation.
pub const MAP_FIRST_MARGINS_M: [f32; START_FIRST] = [0.0096, 0.0187, 0.0368, 0.0329];

#[derive(Clone, Debug)]
pub struct MapLimits {
    pub id: String,
    pub event_type: Option<EventType>,
    pub spawns: Vec<Vec3>,
    pub end: Vec<TriggerVolume>,
    pub splits: [f32; SPLITS],
    pub gates: Vec<TriggerVolume>,
    pub time_limit_ms: Option<u32>,
    pub time_up: TimeUp,
    pub handover: u32,
    pub slot: Vec3,
    pub rotation: Quat,
    pub place: Vec3,
    pub first: [Vec3; START_FIRST],
    pub place_spread: f32,
    pub first_spread: [f32; START_FIRST],
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RiderStart {
    pub character: u32,
    pub start: Start,
    pub every_path: bool,
}

impl Limits {
    pub fn new(rules: &DropRules, tuning: &TuningLimits) -> Result<Self, String> {
        let fall = tuning.fall * (1.0 + VELOCITY_ENCODING);
        Ok(Self {
            drop: rules.event.name.clone(),
            event_type: rules.event.event_type,
            splits: rules.course().splits(),
            starts: rules.start.slots.iter().map(|s| Vec3::from_array(s.position)).collect(),
            end: rules.end_volumes().cloned().collect(),
            fall,
            horizontal: tuning.horizontal * (1.0 + VELOCITY_ENCODING) * HORIZONTAL_MARGIN,
            split_depth: SPLIT_EXECUTIONS * TICKS_PER_STEP * fall,
            tuning: *tuning,
            start: Start::of(rules)?,
            gates: Vec::new(),
            time_limit_ms: None,
            time_up: rules.event.event_type.map_or(TimeUp::Fail, TimeUp::of),
            map: false,
            rider_starts: rules
                .intro
                .riders
                .iter()
                .filter_map(|w| Some((w.rider?, w)))
                .map(|(rider, w)| {
                    let character = ssx_formats::attrib::fnv1(rider.as_bytes());
                    Ok(RiderStart { character, start: Start::of_way(rules, w)?, every_path: w.every_path })
                })
                .collect::<Result<_, String>>()?,
        })
    }

    pub fn of_map(map: MapLimits, tuning: &TuningLimits) -> Result<Self, String> {
        let fall = tuning.fall * (1.0 + VELOCITY_ENCODING);
        if map.spawns.is_empty() {
            return Err(format!("{}: no spawn slot", map.id));
        }
        let finite = |v: Vec3| v.is_finite();
        let spread = |m: f32| m.is_finite() && m >= 0.0;
        if !(finite(map.slot) && finite(map.place) && map.first.iter().all(|&f| finite(f)))
            || !map.rotation.is_finite()
            || map.rotation.length() < 1e-3
            || !map.splits.iter().all(|y| y.is_finite())
            || !(spread(map.place_spread) && map.first_spread.iter().all(|&m| spread(m)))
        {
            return Err(format!("{}: its start or course is not finite", map.id));
        }
        let volume = |v: &TriggerVolume| {
            let size = match v.shape {
                VolumeShape::Sphere { radius } => radius.is_finite() && radius >= 0.0,
                VolumeShape::Box { half_extents } => half_extents.is_finite() && half_extents.min_element() >= 0.0,
            };
            size && finite(v.centre) && v.rotation.is_finite() && v.rotation.length() >= 1e-3
        };
        if !map.spawns.iter().all(|&p| finite(p)) || !map.end.iter().all(volume) || !map.gates.iter().all(volume) {
            return Err(format!("{}: a spawn, a finish volume or a gate is not finite", map.id));
        }
        if map.time_up == TimeUp::Result && map.event_type.is_some_and(EventType::is_race) {
            return Err(format!("{}: a race has no result at its time limit", map.id));
        }
        Ok(Self {
            drop: map.id,
            event_type: map.event_type,
            splits: map.splits,
            starts: map.spawns,
            end: map.end,
            fall,
            horizontal: tuning.horizontal * (1.0 + VELOCITY_ENCODING) * HORIZONTAL_MARGIN,
            split_depth: SPLIT_EXECUTIONS * TICKS_PER_STEP * fall,
            tuning: *tuning,
            start: Start {
                handover: f64::from(map.handover),
                slot: map.slot,
                rotation: map.rotation.normalize(),
                place: map.place,
                margin: MAP_HANDOVER_MARGIN_M + map.place_spread,
                first: map.first,
                first_margins: std::array::from_fn(|k| MAP_FIRST_MARGINS_M[k] + map.first_spread[k]),
            },
            rider_starts: Vec::new(),
            gates: map.gates,
            time_limit_ms: map.time_limit_ms,
            time_up: map.time_up,
            map: true,
        })
    }

    pub fn start_words(&self) -> (&'static str, &'static str) {
        if self.map { ("the map's start", "the map's hand-over") } else { ("the drop's intro", "the drop's hand-over") }
    }

    pub fn gate_segment_distance(&self, k: usize, a: Vec3, b: Vec3) -> Option<f32> {
        let v = self.gates.get(k)?;
        let len = a.distance(b);
        let steps = ((len / (TOUCH_MARGIN_M * 0.1)).ceil() as usize).clamp(1, 4096);
        (0..=steps).map(|i| volume_distance(v, a.lerp(b, i as f32 / steps as f32))).min_by(f32::total_cmp)
    }

    pub fn time_limit_finish_ms(&self) -> Option<f64> {
        let limit = f64::from(self.time_limit_ms?);
        let s = limit / 1000.0;
        let half_ulp = if s > 0.0 { 2f64.powf(s.log2().floor() - 24.0) } else { 0.0 };
        Some(limit + (clock_step_s() + half_ulp) * 1000.0)
    }

    pub fn starts_of(&self, character: Option<u32>) -> Vec<Start> {
        let own: Vec<&RiderStart> = self.rider_starts.iter().filter(|r| Some(r.character) == character).collect();
        let mut out = Vec::with_capacity(own.len() + 1);
        if !own.iter().any(|r| r.every_path) {
            out.push(self.start);
        }
        out.extend(own.iter().map(|r| r.start));
        out
    }

    pub fn earliest_handover(&self, character: Option<u32>) -> f64 {
        self.starts_of(character).iter().map(|s| s.handover).fold(f64::INFINITY, f64::min)
    }

    pub fn timed_out(&self, clock_ms: u32) -> bool {
        self.time_limit_ms.is_some_and(|l| clock_ms >= l)
    }

    pub fn results_at_time_up(&self) -> bool {
        self.time_up == TimeUp::Result
    }

    pub fn result_at_time_up(&self, clock_ms: u32) -> bool {
        self.time_up == TimeUp::Result && self.timed_out(clock_ms)
    }

    pub fn finish_at_end(&self) -> bool {
        self.event_type != Some(EventType::SurviveToDeath)
    }

    pub fn loops(&self) -> bool {
        self.event_type == Some(EventType::SurviveToDeath)
    }

    pub fn end_distance(&self, p: Vec3) -> Option<f32> {
        self.end.iter().map(|v| volume_distance(v, p)).min_by(f32::total_cmp)
    }

    pub fn end_segment_distance(&self, a: Vec3, b: Vec3) -> Option<f32> {
        let len = a.distance(b);
        let steps = ((len / (TOUCH_MARGIN_M * 0.1)).ceil() as usize).clamp(1, 4096);
        let mut best: Option<f32> = None;
        for k in 0..=steps {
            let p = a.lerp(b, k as f32 / steps as f32);
            if let Some(d) = self.end_distance(p) {
                best = Some(best.map_or(d, |x| x.min(d)));
            }
        }
        best
    }

    pub fn touch_reach(&self) -> f32 {
        PLAYER_TOUCH_RADIUS + TOUCH_MARGIN_M
    }

    pub fn trick_points_max(&self, executions: f64) -> f64 {
        let e = executions.max(0.0).ceil();
        let ticks = e * f64::from(TICKS_PER_STEP);
        let tp = f64::from(table::TRICK_POINTS_MAX);
        let tricky = f64::from(table::SUPER_TRICKY_MULTIPLIER.max(table::TRICKY_MULTIPLIER));
        let terrain = f64::from(table::GRIND_TERRAIN_MULTIPLIER.max(table::SUPER_GRIND_TERRAIN_MULTIPLIER));
        let speed = f64::from(self.fall.hypot(self.horizontal));
        let trick_level = f64::from(table::trick_time_level(f32::MAX));
        let grind_level = (0..=1200).map(|t| f64::from(table::grind_trick_time_level(t as f32))).fold(0.0, f64::max);
        let air = tp * tricky * trick_level / 60.0 * f64::from(TICKS_PER_STEP)
            + tp * f64::from(table::TWEAK_MULT_PER_EXECUTION) * tricky;
        let grind = tp * terrain * tricky * grind_level / 60.0 * f64::from(TICKS_PER_STEP)
            + f64::from(table::GRIND_DISTANCE_POINTS) * terrain * tricky * speed * f64::from(TICKS_PER_STEP);
        let press = f64::from(table::BOARDPRESS_DIST_POINTS) * speed * f64::from(TICKS_PER_STEP);
        let deg = |rate: f32| f64::from(rate).to_degrees() * ticks;
        let rotation = f64::from(table::spin_points(deg(self.tuning.spin) as f32))
            + f64::from(table::flip_points(deg(self.tuning.flip) as f32))
            + f64::from(table::grind_spin_points(deg(self.tuning.grind_spin) as f32)) * terrain;
        let heli =
            f64::from(table::HELI_INTERACTION_POINTS) * (1.0 + (ticks / f64::from(self.tuning.heli_shut_out)).floor());
        let objects = f64::from(table::FLARE_POINTS.max(table::BONK_POINTS)) * (e / 2.0).ceil();
        let banks = e * (air + grind + press) + rotation + heli + objects;
        let flow = f64::from(table::FLOW_LEVEL_MAX + 1);
        let bonus_count = (4.0 * e).min(500.0) as u32;
        2.0 * flow * f64::from(self.tuning.trick_point_bonus) * banks + f64::from(table::combo_bonus(bonus_count))
    }
}

pub fn volume_distance(v: &TriggerVolume, p: Vec3) -> f32 {
    match v.shape {
        VolumeShape::Sphere { radius } => (p.distance(v.centre) - radius).max(0.0),
        VolumeShape::Box { half_extents: h } => {
            let d = v.centre - p;
            let axes = [v.rotation * Vec3::X, v.rotation * Vec3::Y, v.rotation * Vec3::Z];
            let local = Vec3::new(d.dot(axes[0]), d.dot(axes[1]), d.dot(axes[2]));
            (local - local.clamp(-h, h)).length()
        }
    }
}
