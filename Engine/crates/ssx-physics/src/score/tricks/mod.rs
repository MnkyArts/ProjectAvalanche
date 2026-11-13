use std::f32::consts::{FRAC_PI_4, TAU};

use anyhow::Result;
use glam::Vec2;

use crate::actuator::wrap_angle;
use crate::rider::grind::tricks::{GRIND_TRICKS, GRIND_TRICKS_CLASSIC, GrindTricks};
use crate::tuning::Bundle;

mod added;
mod flow;
mod gestures;
mod scene_op;

pub use added::*;
pub use gestures::*;

pub const RSTICK_ACTIVE: f32 = 0.3;
pub const TWEAK_STICK_DEAD_ZONE: f32 = 0.1;
pub const TWEAK_BLEND_ACTIVE: f32 = 0.9;
pub fn boost_requested(classic: bool, t: &TrickInput) -> bool {
    if classic { t.x } else { t.tweak }
}

pub fn wipeout_recovery_input(
    standard_flick: bool,
    a_release: bool,
    classic: bool,
    t: &TrickInput,
    boost: f32,
) -> bool {
    standard_flick || a_release || (boost_requested(classic, t) && boost > 0.0)
}
pub const ANTICIPATE_TICKS: f32 = 15.0;
pub const ENTER_TICKS: f32 = 15.0;
pub const EXIT_TICKS: f32 = 10.0;
pub const MAX_TRANSITIONS_PER_UPDATE: usize = 25;
pub const MIN_NODE_UPDATE_TICKS: f32 = 0.001;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum TrickType {
    #[default]
    None,
    Signature,
    FlyingSquirrel,
    Indy,
    Melon,
    Mute,
    Nose,
    Rocket,
    Seatbelt,
    Stalefish,
    Stiffy,
    Tail,
    Truckdriver,
    Wheelman,
    Grind1,
    Grind1Uber,
    Grind1SUber,
    Grind2,
    Grind2Uber,
    Grind2SUber,
    Grind3,
    Grind3Uber,
    Grind3SUber,
    Grind4,
    Grind4Uber,
    Grind4SUber,
    Grind5,
    Grind5Uber,
    Grind5SUber,
    Grind6,
    Grind6Uber,
    Grind6SUber,
    Added(AddedTrick),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum TrickHand {
    #[default]
    None,
    Backhand,
    Forehand,
    Both,
}

const DISC: [TrickType; 32] = {
    use TrickType::*;
    [
        None,
        Signature,
        FlyingSquirrel,
        Indy,
        Melon,
        Mute,
        Nose,
        Rocket,
        Seatbelt,
        Stalefish,
        Stiffy,
        Tail,
        Truckdriver,
        Wheelman,
        Grind1,
        Grind1Uber,
        Grind1SUber,
        Grind2,
        Grind2Uber,
        Grind2SUber,
        Grind3,
        Grind3Uber,
        Grind3SUber,
        Grind4,
        Grind4Uber,
        Grind4SUber,
        Grind5,
        Grind5Uber,
        Grind5SUber,
        Grind6,
        Grind6Uber,
        Grind6SUber,
    ]
};

impl TrickType {
    pub fn value(self) -> u8 {
        match self {
            TrickType::Added(a) => ssx_content::trick::FIRST_VALUE + a.slot,
            t => DISC.iter().position(|&d| d == t).map_or(0, |i| i as u8),
        }
    }

    pub fn disc(v: u8) -> Option<Self> {
        DISC.get(usize::from(v)).copied()
    }

    pub fn flow_base(self) -> Self {
        match self {
            TrickType::Added(a) => Self::disc(a.base).unwrap_or_default(),
            t => t,
        }
    }

    pub fn is_grind(self) -> bool {
        (14..=31).contains(&self.value())
    }

    pub fn grind(index: u8) -> Option<Self> {
        Self::disc(index).filter(|t| t.is_grind())
    }

    pub fn hand(self) -> TrickHand {
        use TrickType::*;
        match self {
            None => TrickHand::None,
            Indy | Stalefish | Tail | Wheelman => TrickHand::Backhand,
            Melon | Mute | Nose | Seatbelt => TrickHand::Forehand,
            Signature | FlyingSquirrel | Rocket | Stiffy | Truckdriver => TrickHand::Both,
            Added(a) => a.hand,
            _ => TrickHand::Both,
        }
    }

    pub fn points(self) -> f32 {
        use TrickType::*;
        use ssx_rules::scoring as p;
        match self {
            None => 0.0,
            Signature => p::TRICK_POINTS_SIGNATURE,
            Indy | Melon | Rocket | Truckdriver => p::TRICK_POINTS_1000,
            FlyingSquirrel | Nose | Seatbelt | Stiffy | Tail | Wheelman => p::TRICK_POINTS_1200,
            Mute | Stalefish => p::TRICK_POINTS_1400,
            Added(a) => f32::from(a.points),
            _ if self.is_grind() && self.value() < 23 => p::TRICK_POINTS_GRIND_1_3,
            _ => p::TRICK_POINTS_GRIND_4_6,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PadSample {
    pub angle: f32,
    pub mag: f32,
    pub y: bool,
    pub x: bool,
    pub b: bool,
    pub a: bool,
    pub lb: bool,
    pub rb: bool,
    pub lt: bool,
    pub rt: bool,
    pub r2: bool,
    pub l2_r2: bool,
}

impl PadSample {
    pub fn new(i: &TrickInput) -> Self {
        let s = i.right_stick;
        Self {
            angle: s.y.atan2(s.x).rem_euclid(TAU),
            mag: s.length(),
            y: i.y,
            x: i.x,
            b: i.b,
            a: i.a,
            lb: i.lb,
            rb: i.rb,
            lt: i.lt,
            rt: i.rt,
            r2: i.tweak,
            l2_r2: i.signature,
        }
    }

    fn button(&self, b: Button) -> bool {
        match b {
            Button::Y => self.y,
            Button::X => self.x,
            Button::B => self.b,
            Button::A => self.a,
            Button::Lb => self.lb,
            Button::Rb => self.rb,
            Button::Lt => self.lt,
            Button::Rt => self.rt,
        }
    }
}

pub fn rstick_mag(s: Vec2) -> f32 {
    ((s.length().min(1.0) - 0.25) / 0.75).clamp(0.0, 1.0)
}

const UP_LEFT: usize = 1;
const UP_RIGHT: usize = 2;
const LEFT_RIGHT: usize = 5;
const LEFT_UP: usize = 6;
const RIGHT_LEFT: usize = 9;
const RIGHT_UP: usize = 10;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TrickTuning {
    pub tweak_switch_threshold: f32,
    pub virtual_stick_persist_time: f32,
    pub added: AddedAirTricks,
}

impl TrickTuning {
    pub fn load(bundle: &Bundle) -> Result<Self> {
        let a = bundle.view("TrickSceneOp", "TrickSceneOpAsset")?;
        Ok(Self {
            tweak_switch_threshold: a.f32("TweakSwitchThreshold")?,
            virtual_stick_persist_time: a.f32("VirtualStickPersistTime")?,
            added: AddedAirTricks::load(bundle.data_root()).0,
        })
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TrickInput {
    pub right_stick: Vec2,
    pub y: bool,
    pub x: bool,
    pub b: bool,
    pub a: bool,
    pub lb: bool,
    pub rb: bool,
    pub lt: bool,
    pub rt: bool,
    pub tweak: bool,
    pub signature: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct GrabGestures {
    pub prev: PadSample,
    pub cur: PadSample,
    pub button_state: [GestureState; 15],
    pub stick_state: [GestureState; 15],
    pub button: [bool; 15],
    pub stick: [bool; 15],
    pub signature: bool,
    pub tweak: bool,
    pub added: AddedGestures,
}

impl GrabGestures {
    pub fn tick(&mut self, pad: PadSample, gates: &Gates, added: &AddedAirTricks, classic: bool) {
        (self.prev, self.cur) = (self.cur, pad);
        let (prev, cur) = (self.prev, self.cur);
        for (i, e) in air_table(classic).iter().enumerate() {
            self.button[i] = e.button.update(&mut self.button_state[i], gates, &prev, &cur);
            self.stick[i] = e.stick.update(&mut self.stick_state[i], gates, &prev, &cur);
        }
        self.added.tick(added, gates, &prev, &cur);
        self.signature = gates.in_air && cur.l2_r2;
        self.tweak = gates.in_air && cur.r2;
    }

    pub fn any(&self, i: usize) -> bool {
        self.button[i] || self.stick[i]
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TrickPhase {
    #[default]
    None,
    Anticipate,
    Enter,
    Loop,
    Exit,
    Recover,
    AirJump,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TrickLevel {
    #[default]
    Normal,
    Uber,
    SuperUber,
    Signature,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct TrickContext {
    pub in_air: bool,
    pub enable_air_tricks: bool,
    pub goofy: bool,
    pub awesome: bool,
    pub super_awesome: bool,
    pub ticks_to_land: f32,
    pub recovering: bool,
    pub board_particle: bool,
    pub air_jump: bool,
    pub is_grinding: bool,
    pub abort: bool,
}

pub fn trick_time_scale(ticks_to_land: f32) -> f32 {
    (2.0 - ticks_to_land / 20.0).clamp(1.0, 2.0)
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SignatureTiming {
    pub out_blend_1: (f32, f32),
    pub out_blend_2: (f32, f32),
    pub enter_end: (f32, f32),
    pub loop_len: f32,
    pub loop_ends: &'static [(f32, f32)],
    pub exit_end: (f32, f32),
}

const fn sig(
    out: (f32, f32),
    blend: f32,
    end: f32,
    loop_len: f32,
    ends: &'static [(f32, f32)],
    exit: f32,
) -> SignatureTiming {
    SignatureTiming {
        out_blend_1: (out.0, blend),
        out_blend_2: (out.1, blend),
        enter_end: (end, 4.0),
        loop_len,
        loop_ends: ends,
        exit_end: (exit, 2.0),
    }
}

pub const SIGNATURE_TIMINGS: [(&str, SignatureTiming); 12] = [
    ("alex", sig((75.0, 135.0), 2.0, 171.0, 120.0, &[(0.0, 4.0), (40.0, 2.0), (80.0, 2.0), (116.0, 4.0)], 50.0)),
    ("eddie", sig((75.0, 135.0), 2.0, 181.0, 60.0, &[(0.0, 4.0)], 30.0)),
    (
        "elise",
        sig((75.0, 135.0), 8.0, 191.0, 120.0, &[(0.0, 4.0), (34.0, 2.0), (60.0, 2.0), (92.0, 2.0), (116.0, 4.0)], 30.0),
    ),
    ("griff", sig((75.0, 135.0), 2.0, 191.0, 60.0, &[(0.0, 4.0)], 30.0)),
    (
        "kaori",
        sig((62.0, 123.0), 2.0, 192.0, 120.0, &[(0.0, 4.0), (31.0, 2.0), (60.0, 2.0), (91.0, 2.0), (116.0, 4.0)], 30.0),
    ),
    ("mac", sig((75.0, 135.0), 2.0, 191.0, 60.0, &[(0.0, 4.0)], 30.0)),
    ("moby", sig((75.0, 135.0), 2.0, 190.0, 60.0, &[(0.0, 4.0), (30.0, 2.0), (56.0, 4.0)], 30.0)),
    (
        "psymon",
        sig((75.0, 135.0), 2.0, 171.0, 60.0, &[(0.0, 4.0), (15.0, 2.0), (30.0, 2.0), (45.0, 2.0), (56.0, 4.0)], 30.0),
    ),
    ("tane", sig((70.0, 135.0), 2.0, 171.0, 60.0, &[(0.0, 4.0), (30.0, 2.0), (56.0, 4.0)], 30.0)),
    ("travisrice", sig((75.0, 135.0), 2.0, 191.0, 60.0, &[(0.0, 4.0), (30.0, 2.0), (56.0, 4.0)], 30.0)),
    ("ty", sig((75.0, 135.0), 2.0, 190.0, 60.0, &[(0.0, 4.0)], 30.0)),
    ("zoe", sig((75.0, 135.0), 2.0, 191.0, 60.0, &[(0.0, 4.0), (30.0, 2.0), (56.0, 4.0)], 30.0)),
];

impl SignatureTiming {
    pub fn for_rider(name: &str) -> Option<Self> {
        let key = match name.to_ascii_lowercase().as_str() {
            "travis" => "travisrice".to_owned(),
            "tyler" => "ty".to_owned(),
            n => n.to_owned(),
        };
        SIGNATURE_TIMINGS.iter().find(|(n, _)| n.eq_ignore_ascii_case(&key)).map(|(_, t)| *t)
    }
}

impl Default for SignatureTiming {
    fn default() -> Self {
        SIGNATURE_TIMINGS[5].1
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TrickState {
    pub classic: bool,
    pub gestures: GrabGestures,
    pub press: crate::press::PressGestures,
    pub gesture_clock: f32,
    pub valid_up_start: bool,
    pub valid_left_start: bool,
    pub valid_right_start: bool,
    pub trick_type: TrickType,
    pub trick_type_locked: TrickType,
    pub hand: TrickHand,
    pub gesture_len: i32,
    pub trick_locked: bool,
    pub is_signature: bool,
    pub is_tricking: bool,
    pub is_tweaking: bool,
    pub tweak_blend: f32,
    pub tweak_enter: f32,
    pub tweak_wind: f32,
    pub virtual_stick: Vec2,
    pub virtual_stick_timer: f32,
    pub has_valid_input: bool,
    pub input_index: i32,
    pub input_index_on_enter: i32,
    pub ticks_since_sig_held: f32,
    pub phase: TrickPhase,
    pub level: TrickLevel,
    pub tweak_node: bool,
    pub sig_exit: u8,
    pub phase_ticks: f32,
    pub trick_ticks: f32,
    pub rate: f32,
    pub signature: SignatureTiming,
    pub grind: GrindTricks,
}

impl Default for TrickState {
    fn default() -> Self {
        Self {
            classic: false,
            gestures: GrabGestures::default(),
            press: crate::press::PressGestures::default(),
            gesture_clock: 0.0,
            valid_up_start: false,
            valid_left_start: false,
            valid_right_start: false,
            trick_type: TrickType::None,
            trick_type_locked: TrickType::None,
            hand: TrickHand::None,
            gesture_len: -1,
            trick_locked: false,
            is_signature: false,
            is_tricking: false,
            is_tweaking: false,
            tweak_blend: 0.0,
            tweak_enter: 0.0,
            tweak_wind: 0.0,
            virtual_stick: Vec2::ZERO,
            virtual_stick_timer: 0.0,
            has_valid_input: false,
            input_index: 0,
            input_index_on_enter: 0,
            ticks_since_sig_held: 0.0,
            phase: TrickPhase::None,
            level: TrickLevel::Normal,
            tweak_node: false,
            sig_exit: 0,
            phase_ticks: 0.0,
            trick_ticks: 0.0,
            rate: 1.0,
            signature: SignatureTiming::default(),
            grind: GrindTricks::default(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Node {
    AirStates,
    UberCrashRecover,
    AirJump,
    Anticipate,
    Enter(TrickLevel),
    Loop(TrickLevel),
    Exit(TrickLevel),
    SigExit(u8),
    TweakEnter(TrickLevel),
    TweakLoop(TrickLevel),
    TweakExit(TrickLevel),
}

#[cfg(test)]
mod tests;

type Transition<'a> = (Node, Option<&'a [(f32, f32)]>, bool);
#[cfg(test)]
mod added_tests;
