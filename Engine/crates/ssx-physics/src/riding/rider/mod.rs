use std::f32::consts::{FRAC_PI_2, PI};

use glam::{Quat, Vec2, Vec3};

use crate::TICKS_PER_SECOND;
use crate::air::{AirInputs, AirRotation, Takeoff};
use crate::ballistic::BallisticRay;
use crate::character::{ResolvedCharacterProperties, Tuning};
use crate::collision::CollisionWorld;
use crate::correction::{CorrectionInputs, ImpactState};
use crate::death::{DeathState, GameFlowFlags, RiderDead};
use crate::graph::{Delta, GraphFeedback, RiderGraph, StepStart};
use crate::ground::{
    BalanceView, BoostThrust, BrakeInputs, CarvingState, GroundInputs, HeightBlends, Spray, SprayInputs,
    integrate_ground, riding_height_target, snow_compression, spray_filter, update_riding_height,
};
use crate::janked::JankedState;
use crate::jump::{JumpState, LaunchInputs, jump_launch};
use crate::landing::{
    LandingState, UBER_CRASH_GATE_TICKS, UBER_RECOVER_HOP_TICKS, UBER_RECOVER_TRIGGER_CLEAR_TICKS, UberRecovery,
};
use crate::levelstart::{GoInputs, LevelStart, Phase};
use crate::probe::{CloseToGround, ProbeInputs, close_to_ground};
use crate::rootmotion::{
    GET_GOING_ANGLE, GET_GOING_END_TICKS, blend_out_weight, get_going_blend, get_going_delta, step_physics,
    world_motion,
};
use crate::scoring::{BoostRequest, BoostState, ScoreState, ScoringContext, ScoringInputs};
use crate::spawn::DropStart;
use crate::steering::{
    Kinematics, SteeringInputs, SteeringState, apply_rotation, ground_frame, kinematics, pid_step, update_concavity,
    update_slip_state, wrap_pi,
};
use crate::surface::{ChatterInputs, ICE, SNOW_COVERAGE, SurfaceProperties, Weights, no_contact_weights};
use crate::tags::{BoolTag, TagTracker, TagWindow};
use crate::tricks::{TrickContext, TrickPhase, TrickState};
use crate::triggers::TriggerSet;

pub use crate::ground::{brake_factor, clamp_not_reversing, friction_accel};
pub use crate::probe::{GROUND_PROBE_LENGTH, GroundContact};
pub use crate::tricks::TrickInput;

pub mod board_particle;
pub mod boundary;
pub mod collision_steering;
pub mod grind;
pub mod heli_trick;
pub mod outputs;
pub mod wingsuit_flow;

mod carving;
mod graph_io;
mod ride;
mod spq;
mod step;

pub use carving::*;
use graph_io::*;
use ride::*;
pub use spq::*;
pub use step::*;

pub const TINY: f32 = 1.525_88e-5;
pub const LOWEST_GROUND_HEIGHT_DEFAULT: f32 = 99_999.0;
pub const TRAJECTORY_HEIGHT_DOF: f32 = 0.855_63;

pub struct World {
    pub collision: CollisionWorld,
    pub tuning: Tuning,
    pub props: ResolvedCharacterProperties,
    pub starts: Vec<DropStart>,
    pub triggers: TriggerSet,
    pub events: Vec<crate::event::EventDef>,
    pub end_locator: Option<Vec3>,
    pub survival: crate::survival::DropSurvival,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Input {
    pub steer: f32,
    pub brake: f32,
    pub tuck: f32,
    pub dpad: Vec2,
    pub dpad_held: bool,
    pub jump: bool,
}

impl Input {
    pub fn stick(&self) -> Vec2 {
        disc(self.steer, self.tuck.clamp(0.0, 1.0) - self.brake.clamp(0.0, 1.0))
    }

    pub fn carve_stick(&self, classic: bool) -> Vec2 {
        let (x, y) = (self.steer, self.tuck.clamp(0.0, 1.0) - self.brake.clamp(0.0, 1.0));
        if !classic {
            return disc(x, y);
        }
        let d = self.dpad.clamp(Vec2::splat(-1.0), Vec2::splat(1.0));
        disc((x + d.x).clamp(-1.0, 1.0), (y + d.y).clamp(-1.0, 1.0))
    }

    pub fn carve_stick_y(&self, classic: bool) -> f32 {
        let y = self.tuck.clamp(0.0, 1.0) - self.brake.clamp(0.0, 1.0);
        if classic { (y + self.dpad.y.clamp(-1.0, 1.0)).clamp(-1.0, 1.0) } else { y.clamp(-1.0, 1.0) }
    }

    pub fn air_stick(&self, classic: bool) -> Vec2 {
        if classic { self.dpad.clamp(Vec2::splat(-1.0), Vec2::splat(1.0)) } else { self.stick() }
    }

    pub fn flip_acceleration(&self, classic: bool) -> f32 {
        self.air_stick(classic).y
    }

    pub fn with_stick(&self, stick: Vec2) -> Self {
        Input { steer: stick.x, tuck: stick.y.max(0.0), brake: (-stick.y).max(0.0), ..*self }
    }
}

pub fn reverse_camera_stick(stick: Vec2, camera: Quat, alignment: Quat, forward_sign: f32, velocity: Vec3) -> Vec2 {
    let mag = stick.length();
    if mag <= TINY {
        return Vec2::ZERO;
    }
    const SPEED_THRESHOLD_SQUARED: f32 = 0.0025;
    const CONE_SMALL: f32 = 0.261_799_4;
    const CONE_LARGE: f32 = std::f32::consts::FRAC_PI_4;
    let speed_squared = velocity.length_squared();
    let travel = if speed_squared <= SPEED_THRESHOLD_SQUARED {
        alignment * Vec3::new(forward_sign, 0.0, 0.0)
    } else {
        velocity / speed_squared.sqrt()
    };
    let d = camera.inverse() * travel;
    let on_screen = (-d.z).atan2(d.y);
    let opposite = if d.x < 0.0 { if on_screen >= 0.0 { PI } else { -PI } } else { 0.0 };
    let w = ((CONE_LARGE - d.x.abs().clamp(0.0, 1.0).acos()) / (CONE_LARGE - CONE_SMALL)).clamp(0.0, 1.0);
    let target = on_screen + w * (opposite - on_screen);
    let angle = wrap_pi(stick.y.atan2(stick.x) - target);
    Vec2::new(mag * angle.cos(), mag * angle.sin())
}

fn disc(x: f32, y: f32) -> Vec2 {
    let s = Vec2::new(x.clamp(-1.0, 1.0), y.clamp(-1.0, 1.0));
    if s.length() > 1.0 { s.normalize() } else { s }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RideState {
    Riding,
    Airborne,
    Jump,
    WingsuitEnter,
    WingsuitFly,
    WingsuitExit,
}

impl RideState {
    pub fn is_wingsuit(self) -> bool {
        matches!(self, Self::WingsuitEnter | Self::WingsuitFly | Self::WingsuitExit)
    }
}

#[derive(Clone, Debug)]
pub struct RiderState {
    pub position: Vec3,
    pub traj: Vec3,
    pub velocity: Vec3,
    pub traj_velocity: Vec3,
    pub filtered_velocity: Vec3,
    pub traj_cached: Vec3,
    pub heading: Vec3,
    pub lean: f32,
    pub on_ground: bool,
    pub state: RideState,
    pub stopped: bool,
    pub ground_normal: Vec3,
    pub contact: Option<GroundContact>,
    pub close: CloseToGround,
    pub surface: SurfaceProperties,
    pub weights: Weights,
    pub carving: CarvingState,
    pub lowest_ground_height: f32,
    pub last_safe_ground: Vec3,
    pub is_uphill: bool,
    pub ride_ticks: f32,
    pub air_ticks: f32,
    pub impacts: u32,
    pub correction_shift: Vec3,
    pub impact: ImpactState,
    pub collision_entity: u32,
    pub depth_below_snow: f32,
    pub alignment: Quat,
    pub traj_orientation: Quat,
    pub steering: SteeringState,
    pub kin: Kinematics,
    pub inputs: SteeringInputs,
    pub jump: JumpState,
    pub first_air_tick: bool,
    pub jump_ticks: f32,
    pub last_launch: Vec3,
    pub janked: JankedState,
    pub angular_velocity: Vec3,
    pub angular_velocity_q: Quat,
    pub filtered_angular_velocity: Quat,
    pub prewind: f32,
    pub prewind_analog: f32,
    pub prewind_reset: bool,
    pub goofy: bool,
    pub ballistic: BallisticRay,
    pub air: AirRotation,
    pub landing: LandingState,
    pub tricks: TrickState,
    pub score: ScoreState,
    pub boost: BoostState,
    pub spray: Spray,
    pub board_velocity_angle: f32,
    pub nose_tail: f32,
    pub stick_y_filtered: f32,
    pub uber_trigger: bool,
    pub uber_gate_timer: f32,
    pub uber_trigger_seen: bool,
    pub stick_prev: Vec2,
    pub reverse_camera: Option<Quat>,
    pub pad_stick: Vec2,
    pub nested_stable: bool,
    pub request_stop: bool,
    pub level_start: Vec3,
    pub traj_cached_prev: Vec3,
    pub player_speed: f32,
    pub player_speed_ratio: f32,
    pub air_jumped: bool,
    pub jump_airborne: bool,
    pub graph: GraphFeedback,
    pub audio_tags: Vec<ssx_assets::anim_eval::AudioTagEvent>,
    pub get_going: Option<GetGoing>,
    pub heli_trick: Option<heli_trick::HeliTrick>,
    pub heli: crate::helicopter::interaction::RiderHeliStates,
    pub heli_interaction_allowed: bool,
    pub janked_pin: Option<(Vec3, Quat)>,
    pub grind: grind::GrindRider,
    pub death: DeathState,
    pub start: Option<LevelStart>,
    pub gameplay: Option<GameplayNode>,
    pub track_started: bool,
    pub trigger_go: bool,
    pub press: crate::press::CarvingPress,
    pub force_stop: bool,
    pub allow_race_end: bool,
    pub force_race_end_timer: f32,
    pub wingsuit: crate::wingsuit::WingsuitState,
    pub props_resolved: bool,
    pub survival: crate::survival::SurvivalState,
    pub ice_axe: crate::ice_axe::IceAxeState,
    pub tripped: bool,
    pub trip: Option<crate::trip::Trip>,
    pub trip_walk: Option<crate::trip::TripWalk>,
    pub avalanche: crate::avalanche::RiderAvalanche,
    pub board_particle: board_particle::BoardParticle,
    pub reentry: Option<Reentry>,
    pub boundary: boundary::BoundaryWind,
    pub ghost_impact: Option<Vec3>,
    pub airborne_auto_upright: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reentry {
    ImpactBail,
    Impact,
}

#[derive(Clone, Debug, PartialEq)]
pub struct GameplayNode {
    pub ticks: f32,
    tags: TagTracker,
}

const TRIGGER_GO_WINDOW: TagWindow = TagWindow::new(0.0, 1.0, false);
const TRIGGER_GO_TAG: BoolTag = BoolTag { value: true, mode: 1, reset: None };

impl GameplayNode {
    pub fn new() -> Self {
        Self { ticks: 0.0, tags: TagTracker::new(&[TRIGGER_GO_WINDOW]) }
    }
}

impl Default for GameplayNode {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct GetGoing {
    pub ticks: f32,
    pub items: Vec<(usize, f32)>,
}

pub const JANKED_PIN_TICKS: f32 = 58.0;

pub fn drag_accel(kh: f32, kv: f32, v: Vec3, wind: Vec3, t_ticks: f32) -> Vec3 {
    let rel = v - wind;
    let vh = Vec3::new(rel.x, 0.0, rel.z);
    let s = vh.length();
    let t = t_ticks.max(1.0);
    let mut a = if vh.abs().max_element() <= TINY {
        Vec3::ZERO
    } else {
        -vh * (1.0 / t).min(kh / (s * (kh * t + 1.0 / s).powi(2)))
    };
    a.y = -kv * rel.y * rel.y.abs();
    a
}

pub fn nlerp(a: Quat, b: Quat, t: f32) -> Quat {
    let b = if a.dot(b) > 0.0 { b } else { -b };
    (a + (b - a) * t).normalize()
}

impl World {
    pub fn equip(&mut self, gear: &crate::character::GearStats) -> anyhow::Result<()> {
        self.props = self.tuning.resolve_gear(gear)?;
        self.survival.slots = if gear.null { [false; 15] } else { gear.slots };
        self.survival.gear_null = gear.null;
        self.survival.stats = gear.stats;
        Ok(())
    }

    pub fn board_radius(&self, lean: f32) -> f32 {
        let c = &self.tuning.carving;
        c.collision_radius_base + c.collision_radius_upright_offset * lean.abs().cos()
    }
}

pub const SPAWN_RAY_UP: f32 = 1.0;
pub const SPAWN_RAY_LENGTH: f32 = 101.0;

impl RiderState {
    pub fn spawn(world: &World, position: Vec3, heading: Vec3) -> Option<Self> {
        let hit = world.collision.ray_cast(position + Vec3::Y * SPAWN_RAY_UP, -Vec3::Y, SPAWN_RAY_LENGTH)?;
        let normal = hit.normal;
        let weights = world.collision.surface_weights(hit.cluster, hit.tri, hit.bary, normal.y);
        let surface = world.tuning.surfaces.blend(&weights, &ChatterInputs::default());
        let heading =
            (heading - normal * heading.dot(normal)).try_normalize().unwrap_or_else(|| normal.any_orthonormal_vector());
        let alignment = Quat::from_mat3(&glam::Mat3::from_cols(heading, normal, heading.cross(normal))).normalize();
        let traj = hit.point + normal * surface.snow_depth;
        let c = &world.tuning.carving;
        let mut s = Self {
            position: hit.point,
            traj,
            velocity: Vec3::ZERO,
            traj_velocity: Vec3::ZERO,
            filtered_velocity: Vec3::ZERO,
            traj_cached: traj,
            heading,
            lean: 0.0,
            on_ground: true,
            state: RideState::Riding,
            stopped: false,
            ground_normal: normal,
            contact: None,
            close: CloseToGround::default(),
            surface,
            weights,
            carving: CarvingState::default(),
            lowest_ground_height: LOWEST_GROUND_HEIGHT_DEFAULT,
            last_safe_ground: Vec3::ZERO,
            is_uphill: false,
            ride_ticks: 0.0,
            air_ticks: 0.0,
            impacts: 0,
            correction_shift: Vec3::ZERO,
            impact: ImpactState::default(),
            collision_entity: 0,
            depth_below_snow: 0.0,
            alignment,
            traj_orientation: alignment,
            steering: SteeringState::new(c),
            kin: Kinematics { fwd: 1.0, ..Kinematics::default() },
            inputs: SteeringInputs::default(),
            jump: JumpState::default(),
            first_air_tick: false,
            jump_ticks: 0.0,
            last_launch: Vec3::ZERO,
            janked: JankedState::new(traj),
            angular_velocity: Vec3::ZERO,
            angular_velocity_q: Quat::IDENTITY,
            filtered_angular_velocity: Quat::IDENTITY,
            prewind: 0.0,
            prewind_analog: 0.0,
            prewind_reset: false,
            goofy: false,
            ballistic: BallisticRay::default(),
            air: AirRotation::default(),
            landing: LandingState::default(),
            tricks: TrickState::default(),
            score: ScoreState::default(),
            boost: BoostState::default(),
            spray: Spray::default(),
            board_velocity_angle: 0.0,
            nose_tail: 0.0,
            stick_y_filtered: 0.0,
            uber_trigger: false,
            uber_gate_timer: 0.0,
            uber_trigger_seen: false,
            stick_prev: Vec2::ZERO,
            reverse_camera: None,
            pad_stick: Vec2::ZERO,
            nested_stable: false,
            request_stop: false,
            level_start: traj,
            traj_cached_prev: traj,
            player_speed: 0.0,
            player_speed_ratio: 0.0,
            air_jumped: false,
            jump_airborne: false,
            graph: GraphFeedback::default(),
            audio_tags: Vec::new(),
            get_going: None,
            heli_trick: None,
            heli: crate::helicopter::interaction::RiderHeliStates::default(),
            heli_interaction_allowed: true,
            janked_pin: None,
            grind: grind::GrindRider::default(),
            death: DeathState::default(),
            start: None,
            gameplay: Some(GameplayNode::new()),
            track_started: true,
            trigger_go: true,
            press: crate::press::CarvingPress::default(),
            force_stop: false,
            allow_race_end: false,
            force_race_end_timer: world.tuning.player.force_race_end_max_time * TICKS_PER_SECOND,
            wingsuit: crate::wingsuit::WingsuitState::default(),
            props_resolved: false,
            survival: crate::survival::SurvivalState::default(),
            ice_axe: crate::ice_axe::IceAxeState::default(),
            tripped: false,
            trip: None,
            trip_walk: None,
            avalanche: crate::avalanche::RiderAvalanche::default(),
            board_particle: board_particle::BoardParticle::default(),
            reentry: None,
            boundary: boundary::BoundaryWind::default(),
            ghost_impact: None,
            airborne_auto_upright: false,
        };
        s.grind.tail_pos = s.traj;
        spq(&mut s, world, crate::DT_TICKS, None);
        s.position = s.board_point();
        Some(s)
    }

    pub fn level_start(world: &World, position: Vec3, orientation: Quat) -> Option<Self> {
        let ls = LevelStart::new(world, position, orientation)?;
        let (traj, t) = ls.idle_pose(&world.tuning.root_motion)?;
        let mut s = Self::spawn(world, ls.offset, t * Vec3::X)?;
        (s.traj, s.traj_cached, s.traj_cached_prev, s.level_start) = (traj, traj, traj, position);
        (s.alignment, s.traj_orientation, s.heading) = (t, t, t * Vec3::X);
        (s.contact, s.close, s.ballistic) = (None, CloseToGround::default(), BallisticRay::default());
        (s.ground_normal, s.weights) = (Vec3::Y, no_contact_weights());
        s.janked = JankedState::new(traj);
        s.grind.tail_pos = traj;
        (s.start, s.gameplay, s.track_started, s.trigger_go) = (Some(ls), None, false, false);
        s.force_race_end_timer = 0.0;
        s.position = s.board_point();
        Some(s)
    }

    pub fn loop_to_top(
        &self,
        w: &World,
        at: Vec3,
        q: Quat,
        intro: Option<std::sync::Arc<crate::nis::IntroAssets>>,
    ) -> Option<Self> {
        let mut n = match intro {
            Some(i) => crate::nis::level_start(w, at, q, i)?,
            None => Self::level_start(w, at, q)?,
        };
        n.carry_game_state(self);
        n.survival.loop_to_top(&w.tuning.survival, &w.props);
        Some(n)
    }

    pub fn carry_game_state(&mut self, from: &Self) {
        self.survival = from.survival;
        self.airborne_auto_upright = from.airborne_auto_upright;
        self.wingsuit.effectiveness = from.wingsuit.effectiveness;
        self.death = from.death;
        self.boundary = from.boundary;
        self.avalanche.switches = from.avalanche.switches;
    }

    pub fn carry_controller_state(&mut self, from: &Self) {
        self.survival.low_oxygen = from.survival.low_oxygen;
        self.survival.temperature_enabled = from.survival.temperature_enabled;
    }

    pub fn speed(&self) -> f32 {
        self.velocity.length()
    }

    pub fn in_air(&self) -> bool {
        self.state != RideState::Riding
    }

    pub fn recovering_from_uber_bail(&self) -> bool {
        match self.landing.uber_recovery {
            UberRecovery::GettingUp(_) => true,
            UberRecovery::Hop(h) => h < UBER_RECOVER_HOP_TICKS,
            UberRecovery::Sliding => false,
        }
    }

    pub fn board_point(&self) -> Vec3 {
        if (self.in_air() || self.start.is_some()) && self.graph.driven {
            return self.traj + self.graph.ground_offset;
        }
        if let Some(ls) = &self.start {
            return if ls.idle() { ls.offset } else { self.traj - Vec3::Y * TRAJECTORY_HEIGHT_DOF };
        }
        let up = if self.in_air() { Vec3::Y } else { self.alignment * Vec3::Y };
        // PORT(choice): [`Self::spawn`]'s riding start probes with the joint at the riding pose's height before the
        // carving's first update (only a test fixture spawns a rider riding); nothing in the game does.
        let rh = if self.carving.riding_height < 0.0 {
            self.carving.riding_height
        } else {
            crate::guess_hit!();
            -TRAJECTORY_HEIGHT_DOF
        };
        self.traj + up * rh
    }

    pub fn traj_height_dof(&self) -> f32 {
        if self.graph.driven { self.graph.traj_height_dof } else { TRAJECTORY_HEIGHT_DOF }
    }
}

#[cfg(test)]
mod tests;
