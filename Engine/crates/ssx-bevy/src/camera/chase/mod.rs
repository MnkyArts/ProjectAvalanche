use bevy::prelude::*;

use crate::camera::graph::{Channels, Flow, Inputs, Lowpass, Shared};
use crate::camera::{GAME_NEAR, GAME_REST_HFOV_DEG};
use crate::ride::Ride;
use crate::rider::anim::player_speed;
use crate::rider::skin::RiderRig;

mod collision;
mod follow;
mod meta;
mod noise;
mod spectate;
mod update;

use collision::{Queries, TICKS_SINCE_COLLISION};
use follow::step;
pub use follow::{follow, toggle};
use meta::TYPE_TRANSITION_SPEED_SET_DEFAULT;
use noise::CarvingNoise;
pub use noise::{CameraNoise, load_noise};
pub use spectate::SpectateTarget;
#[cfg(feature = "scripts")]
pub use spectate::spectate_inputs_at;

pub fn register(app: &mut App) {
    app.init_resource::<ChaseCamera>().add_systems(FixedPostUpdate, step.run_if(resource_exists::<crate::ride::Ride>));
    crate::shell::level::scoped::<SpectateTarget>(app);
    crate::shell::level::scoped::<CameraNoise>(app);
}

pub fn reset_level(world: &mut World) {
    world.insert_resource(ChaseCamera::default());
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Snapshot {
    position: Vec3,
    rotation: Quat,
    fov_deg: f32,
    near: f32,
}

const CAMERA_TYPE_15P: i32 = 4;
const CAMERA_TYPE_REVERSE: i32 = 2;
const CAMERA_TYPE_TUTORIAL: i32 = 6;
const MAX_TICKS_IN_INTRO_CAM: f32 = 5.0;
const HANDBACK_TICKS: f32 = 30.0;
const HANDBACK_CURVE: i64 = 1;

#[derive(Clone, Copy, Debug)]
struct MetaState {
    filtered_speed: f32,
    filtered_desired_speed: f32,
    transition_speed_current: f32,
    target_offset: Vec3,
    target_desired: Vec3,
    position_offset: Vec3,
    position_desired: Vec3,
    ideal_distance: f32,
    ticks_in_intro_cam: f32,
    camera_type: i32,
    camera_type_last: i32,
    fallback_type: i32,
    type_speed_set: f32,
    meta_speed: f32,
    meta_desired_speed: f32,
    type_transition_speed: f32,
    meta_target_offset: Vec3,
    meta_target_desired: Vec3,
    meta_position_offset: Vec3,
    meta_position_desired: Vec3,
    ticks_to_force_avoidance: f32,
    snap: bool,
    cycle_index: i32,
    trigger_blend_reset: bool,
}

impl Default for MetaState {
    fn default() -> Self {
        Self {
            filtered_speed: 0.0,
            filtered_desired_speed: 0.0,
            transition_speed_current: 1.0,
            target_offset: Vec3::ZERO,
            target_desired: Vec3::ZERO,
            position_offset: Vec3::ZERO,
            position_desired: Vec3::ZERO,
            ideal_distance: 4.0,
            ticks_in_intro_cam: 0.0,
            camera_type: 0,
            camera_type_last: 0,
            fallback_type: 0,
            type_speed_set: TYPE_TRANSITION_SPEED_SET_DEFAULT,
            meta_speed: 0.0,
            meta_desired_speed: 0.0,
            type_transition_speed: 1.0,
            meta_target_offset: Vec3::ZERO,
            meta_target_desired: Vec3::ZERO,
            meta_position_offset: Vec3::ZERO,
            meta_position_desired: Vec3::ZERO,
            ticks_to_force_avoidance: 0.0,
            snap: false,
            cycle_index: -1,
            trigger_blend_reset: false,
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct MetaFilters {
    roll: Lowpass<f32>,
    fov: Lowpass<f32>,
    velocity: Lowpass<Vec3>,
    pivot: Lowpass<Vec3>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CamNode {
    Start,
    Normal,
    Reverse,
    P15,
    Far,
}

const CAMERA_TYPE_NORMAL: i32 = 0;
const CAMERA_TYPE_FAR: i32 = 5;
const CAMERA_DISTANCE_FAR: f32 = 9.9;

#[derive(Clone, Copy, Debug)]
struct StartState {
    iterations: f32,
    state: bool,
    complete: bool,
    was_in_nis: bool,
    in_nis: bool,
    nis_active: bool,
    in_procedural: bool,
}

impl StartState {
    fn defaults() -> (f32, bool, bool) {
        (-1.0, true, false)
    }
}

#[derive(Clone, Copy, Debug)]
struct NisRig {
    trajectory: (Quat, Vec3),
    eye: (Quat, Vec3),
    fov_deg: f32,
    nis_orientation: Quat,
    start_position: Vec3,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Wipeout {
    None,
    OnWeight,
    Always,
}

#[derive(Clone, Copy, Debug)]
struct NoiseSignals {
    carving: (f32, f32),
    wipeout: (f32, f32),
}

#[derive(Clone, Copy, Debug)]
struct CamSnapshot {
    shared: Shared,
    meta: MetaState,
    queries: Queries,
    ticks_since_collision: f32,
    previous: Option<Vec3>,
    camera_pos: Option<Vec3>,
    pose: Channels,
    eye: (Quat, Vec3),
    start: StartState,
    camera_orientation: Quat,
    near: f32,
}

pub struct Chase {
    shared: Shared,
    meta: MetaState,
    flow: Flow,
    meta3p: MetaFilters,
    metac: MetaFilters,
    camera_fade: Option<f32>,
    camera_fade_ticks: f32,
    handback: Option<(f32, Channels)>,
    rig: Channels,
    near: f32,
    ticks_since_collision: f32,
    previous: Option<Vec3>,
    camera_pos: Option<Vec3>,
    queries: Queries,
    snapshots: Vec<Option<CamSnapshot>>,
    rewinding: bool,
    steps: u64,
    prev: Snapshot,
    curr: Snapshot,
    node: CamNode,
    start: StartState,
    camera_orientation: Quat,
    meta15: MetaFilters,
    nis_rig: Option<NisRig>,
    eye: (Quat, Vec3),
    shake_time: Option<f32>,
    carving: CarvingNoise,
    wipeout_time: Option<f32>,
    nis: bool,
    spectating: Option<u16>,
}

#[derive(Resource, Default)]
pub struct ChaseCamera(Option<Chase>);

impl ChaseCamera {
    pub fn reverse_camera(&self) -> Option<Quat> {
        let c = self.0.as_ref()?;
        (c.node == CamNode::Reverse && !c.nis).then_some(c.camera_orientation)
    }
}

fn inputs(ride: &Ride, rig: Option<&mut RiderRig>) -> Inputs {
    let s = &ride.state;
    let (pose, _) = ride.step();
    let target_pos = pose.traj;
    let (target_rot, speed) = match rig {
        Some(r) => {
            let graph = r.anim.graph_mut();
            let rot = graph.trajectory_joint().map_or(pose.traj_rot, |_| graph.trajectory_rotation());
            (rot, r.anim.player_speed())
        }
        None => (pose.traj_rot, player_speed(s, s.in_air())),
    };
    Inputs {
        target_pos,
        target_up: target_rot * Vec3::Y,
        velocity: s.filtered_velocity,
        speed,
        ground_normal: s.ground_normal,
        lean: s.kin.lean_norm,
        airborne: !s.on_ground,
        wipeout: s.landing.tumbling,
        boosting: s.boost.is_boosting,
        grinding: s.grind.is_grinding(),
        dead: s.death.dead_by_death_plane,
        track_finished: ride.session.flow.track_finished,
        aggregate_mass: s.avalanche.published.aggregate.mass,
        bailed_by_avalanche: s.avalanche.bailed_because_of_avalanche,
        alive: s.death.is_alive(),
        heli_position: s.heli.heli_position,
        heli_interact: ride.heli.as_ref().is_some_and(|h| h.interaction.interact),
        heli_time: ride.heli.as_ref().map_or(0.0, |h| h.interaction.time),
        back_press: ride.back_press,
    }
}

const WARP_HINGE_OFFSET: Vec3 = Vec3::new(0.0, 2000.0, -100.0);

impl Chase {
    fn restore_fallback(&mut self, saved: i32) {
        if self.meta.camera_type != CAMERA_TYPE_REVERSE {
            info!("chase camera: the race start's INT_CameraFallbackType {saved} (the profile's camera type)");
            self.meta.fallback_type = saved;
        }
    }

    fn base(steps: u64, shared: Shared, meta: MetaState, node: CamNode, rig: Channels, eye: (Quat, Vec3)) -> Self {
        let none =
            Snapshot { position: Vec3::ZERO, rotation: Quat::IDENTITY, fov_deg: GAME_REST_HFOV_DEG, near: GAME_NEAR };
        let carving = CarvingNoise::new(shared.carving_weight);
        Self {
            shared,
            meta,
            flow: match node {
                CamNode::Reverse => Flow::reverse(),
                CamNode::P15 => Flow::cam15p(),
                CamNode::Far => Flow::far(),
                CamNode::Start | CamNode::Normal => Flow::default(),
            },
            meta3p: MetaFilters::default(),
            metac: MetaFilters::default(),
            camera_fade: None,
            camera_fade_ticks: 8.0,
            handback: None,
            rig,
            near: GAME_NEAR,
            ticks_since_collision: TICKS_SINCE_COLLISION,
            previous: None,
            camera_pos: None,
            queries: Queries::default(),
            snapshots: Vec::new(),
            rewinding: false,
            steps,
            prev: none,
            curr: none,
            node,
            start: StartState {
                iterations: -1.0,
                state: true,
                complete: false,
                was_in_nis: false,
                in_nis: false,
                nis_active: false,
                in_procedural: false,
            },
            camera_orientation: Quat::IDENTITY,
            meta15: MetaFilters::default(),
            nis_rig: None,
            eye,
            shake_time: None,
            carving,
            wipeout_time: None,
            nis: false,
            spectating: None,
        }
    }

    fn new(ride: &Ride, i: &Inputs, view: Snapshot, clip: Option<&CameraNoise>) -> Self {
        let hinge = ride.state.level_start + WARP_HINGE_OFFSET;
        let rig = Channels {
            position: view.position,
            rotation: view.rotation,
            interest: view.position,
            fov_deg: view.fov_deg,
        };
        // TODO(guess): a camera without a NIS before it (viewer-only: every `ui_drop` starts with an intro NIS) starts as one long past its start — the camera's default level start (`toc_snow_start_ready_loop_camera`: a TrajectoryOffsetController by `VEC_LevelStartPosition` / `QUAT_LevelStartOrientation` over the 100-tick clip `snow_start_ready_loop_camera`) is not ported,
        // its `VEC_LevelStartPosition` writer not found (the image's id 0x61 setters are player 0's — 0x82398FF0, 0x824832FC, 0x82482A70 — and the racers', 0x824B72DC; the asset defaults would put the clip at the world origin; a relay in `anim_all.cba` was not checked); settle: the camera's level-start values, then port that clip with [`Chase::after_nis`]'s hand-over.
        let ty = initial_type(ride);
        let meta = MetaState {
            filtered_speed: 1.0,
            filtered_desired_speed: 1.0,
            meta_speed: 1.0,
            meta_desired_speed: 1.0,
            type_speed_set: 1.0,
            ticks_in_intro_cam: 1000.0,
            trigger_blend_reset: true,
            camera_type: ty,
            camera_type_last: ty,
            fallback_type: ty,
            ..MetaState::default()
        };
        let node = camera_node(ty).unwrap_or(CamNode::Normal);
        let mut c = Self::base(ride.steps, Shared::new(hinge), meta, node, rig, (Quat::IDENTITY, Vec3::ZERO));
        let dt = if ride.steps == 0 { 0.0 } else { ride.step().1 };
        c.curr = c.update(ride, i, dt, clip);
        c.prev = c.curr;
        c.meta.trigger_blend_reset = false;
        c
    }

    fn after_nis(ride: &Ride, i: &Inputs, nis: NisRig, last: Snapshot, clip: Option<&CameraNoise>) -> Self {
        let (q, p) = nis.trajectory;
        info!(
            "chase camera: the gameplay camera after the NIS at step {}: the rig's Trajectory {p:.2}, CameraEye local \
             {:.2} / {:.3}, FOV {:.1}",
            ride.steps, nis.eye.1, nis.eye.0, nis.fov_deg
        );
        let rig = Channels { position: p, rotation: q, interest: p, fov_deg: nis.fov_deg };
        let mut shared = Shared::new(nis.start_position + nis.nis_orientation * WARP_HINGE_OFFSET);
        shared.camera_pos = p;
        let ty = initial_type(ride);
        let meta = MetaState { camera_type: ty, camera_type_last: ty, fallback_type: ty, ..MetaState::default() };
        let mut c = Self::base(ride.steps, shared, meta, CamNode::Start, rig, nis.eye);
        (c.previous, c.camera_pos, c.camera_orientation) = (Some(p), Some(p), q);
        (c.start.in_nis, c.start.nis_active) = (true, true);
        let dt = if ride.steps == 0 { 0.0 } else { ride.step().1 };
        c.curr = c.update(ride, i, dt, clip);
        c.prev = last;
        c
    }
}

fn initial_type(ride: &Ride) -> i32 {
    if ride.world().survival.avalanche.reverse_cam {
        CAMERA_TYPE_REVERSE
    } else if crate::render::post::env_is("SSX_CAMERA_TYPE", "15p") {
        CAMERA_TYPE_15P
    } else if crate::render::post::env_is("SSX_CAMERA_TYPE", "far") {
        CAMERA_TYPE_FAR
    } else {
        CAMERA_TYPE_NORMAL
    }
}

fn camera_node(ty: i32) -> Option<CamNode> {
    match ty {
        CAMERA_TYPE_NORMAL => Some(CamNode::Normal),
        CAMERA_TYPE_REVERSE => Some(CamNode::Reverse),
        CAMERA_TYPE_15P => Some(CamNode::P15),
        CAMERA_TYPE_FAR => Some(CamNode::Far),
        _ => None,
    }
}

fn render_blend(a: &Snapshot, b: &Snapshot, t: f32) -> Snapshot {
    let q = if a.rotation.dot(b.rotation) > 0.0 { b.rotation } else { -b.rotation };
    Snapshot {
        position: a.position.lerp(b.position, t),
        rotation: Quat::from_vec4(Vec4::from(a.rotation).lerp(Vec4::from(q), t)).normalize(),
        fov_deg: a.fov_deg + (b.fov_deg - a.fov_deg) * t,
        near: a.near + (b.near - a.near) * t,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn view_blend_is_shortest_path_nlerp() {
        let a = Snapshot { position: Vec3::ZERO, rotation: Quat::IDENTITY, fov_deg: 80.0, near: 0.5 };
        let b = Snapshot {
            position: Vec3::new(2.0, 0.0, 0.0),
            rotation: Quat::from_rotation_y(1.0),
            fov_deg: 90.0,
            near: 0.1,
        };
        assert_eq!(render_blend(&a, &b, 0.0), a);
        assert!(render_blend(&a, &b, 1.0).rotation.angle_between(b.rotation) < 1e-5);
        let flipped = Snapshot { rotation: -b.rotation, ..b };
        for blend in [render_blend(&a, &b, 0.5), render_blend(&a, &flipped, 0.5)] {
            assert!(blend.rotation.angle_between(Quat::from_rotation_y(0.5)) < 1e-5);
            assert!((blend.position.x - 1.0).abs() < 1e-6 && (blend.fov_deg - 85.0).abs() < 1e-5);
            assert!((blend.near - 0.3).abs() < 1e-6, "0x828C22E8 lerps the near plane");
        }
    }
}
