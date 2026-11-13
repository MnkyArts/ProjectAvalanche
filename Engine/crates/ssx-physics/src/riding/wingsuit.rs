use glam::{Quat, Vec2, Vec3};

use crate::character::{PidGains, ResolvedCharacterProperties};
use crate::curve::Curve;
use crate::ice_axe::tag_spline;
use crate::rider::{TINY, drag_accel};
use crate::steering::{Pid, twist};
use crate::tags::{FloatTag, TagEvent, TagTracker, TagWindow};
use crate::tuning::Bundle;

#[derive(Clone, Debug, Default)]
pub struct WingsuitTuning {
    pub roll_pid: PidGains,
    pub roll_input: Curve,
    pub max_roll_vel: f32,
    pub max_roll_accel: f32,
    pub max_roll_angle: f32,
    pub max_roll_input: f32,
    pub pitch_pid: PidGains,
    pub pitch_input: Curve,
    pub pitch_vs_speed: Option<Curve>,
    pub max_pitch_vel: f32,
    pub max_pitch_accel: f32,
    pub max_pitch_angle: f32,
    pub max_pitch_input: f32,
    pub roll_to_turn: Curve,
    pub max_roll_to_lift_coefficient: f32,
    pub max_roll_to_lift: f32,
    pub roll_to_turn_time: Curve,
    pub yaw_pid: PidGains,
    pub yaw_input: Curve,
    pub max_yaw_vel: f32,
    pub max_yaw_accel: f32,
    pub max_yaw_angle: f32,
    pub max_yaw_input: f32,
    pub lift: Curve,
    pub max_lift_coefficient: f32,
    pub lift_time: Curve,
    pub cos_parallel_angle: f32,
    pub min_speed: f32,
    pub horizontal_terminal: Curve,
    pub vertical_terminal: Curve,
    pub gravity: Curve,
    pub maximum_y_velocity: f32,
    pub handle_decay_ticks: f32,
    pub base_damage_rate: f32,
}

impl WingsuitTuning {
    pub fn load(bundle: &Bundle) -> anyhow::Result<Self> {
        let a = bundle.view("wingsuit_fly", "WingsuitControllerAsset")?;
        let p = bundle.view("PlayerUpdateSceneOp", "PlayerUpdateSceneOpAsset")?;
        let pid = |f: &str| PidGains::load(&a.asset(f)?);
        Ok(Self {
            roll_pid: pid("RollPidAsset")?,
            roll_input: a.curve("RollInputCurveAsset")?,
            max_roll_vel: a.f32("MaxRollVel")?,
            max_roll_accel: a.f32("MaxRollAccel")?,
            max_roll_angle: a.f32("MaxRollAngle")?,
            max_roll_input: a.f32("MaxRollInput")?,
            pitch_pid: pid("PitchPidAsset")?,
            pitch_input: a.curve("PitchInputCurveAsset")?,
            pitch_vs_speed: a.asset_opt("PitchVsSpeedCurveAsset")?.map(|c| c.as_curve()).transpose()?,
            max_pitch_vel: a.f32("MaxPitchVel")?,
            max_pitch_accel: a.f32("MaxPitchAccel")?,
            max_pitch_angle: a.f32("MaxPitchAngle")?,
            max_pitch_input: a.f32("MaxPitchInput")?,
            roll_to_turn: a.curve("RollToPitchConversionCurveAsset")?,
            max_roll_to_lift_coefficient: a.f32("MaxRollToLiftCoefficient")?,
            max_roll_to_lift: a.f32("MaxRollToLift")?,
            roll_to_turn_time: a.curve("RollToPitchTimeDampeningCurveAsset")?,
            yaw_pid: pid("YawPidAsset")?,
            yaw_input: a.curve("YawInputCurveAsset")?,
            max_yaw_vel: a.f32("MaxYawVel")?,
            max_yaw_accel: a.f32("MaxYawAccel")?,
            max_yaw_angle: a.f32("MaxYawAngle")?,
            max_yaw_input: a.f32("MaxYawInput")?,
            lift: a.curve("LiftCurveAsset")?,
            max_lift_coefficient: a.f32("MaxLiftCoefficient")?,
            lift_time: a.curve("LiftTimeDampeningCurveAsset")?,
            cos_parallel_angle: a.f32("CosParallelAngle")?,
            min_speed: a.f32("MinSpeed")?,
            horizontal_terminal: a.curve("HorizontalTerminalSpeedModificationCurveAsset")?,
            vertical_terminal: a.curve("VerticalTerminalSpeedModificationCurveAsset")?,
            gravity: a.curve("GravityAccelerationModificationCurveAsset")?,
            maximum_y_velocity: a.f32("MaximumYVelocity")?,
            handle_decay_ticks: p.f32("WingsuitHandleDecayTicks")?,
            base_damage_rate: p.f32("BaseWingsuitDamageRate")?,
        })
    }
}

pub fn joint_of(t: Quat) -> Quat {
    (t * Quat::from_rotation_y(-std::f32::consts::FRAC_PI_2)).normalize()
}

pub fn traj_of(j: Quat) -> Quat {
    (j * Quat::from_rotation_y(std::f32::consts::FRAC_PI_2)).normalize()
}

pub fn velocity_frame(t: &WingsuitTuning, j: Quat, v: Vec3, speed: f32) -> Quat {
    if speed.abs() <= t.min_speed {
        return j;
    }
    let d = v / speed;
    let (x, z) = if d.y.abs() > t.cos_parallel_angle {
        let y_j = j * Vec3::Y;
        if y_j.y.abs() > t.cos_parallel_angle {
            let x = d.cross(j * Vec3::Z).normalize();
            (x, x.cross(d).normalize())
        } else {
            let z = d.cross(y_j).normalize();
            (d.cross(z).normalize(), z)
        }
    } else {
        let z = d.cross(Vec3::Y).normalize();
        (d.cross(z).normalize(), z)
    };
    Quat::from_mat3(&glam::Mat3::from_cols(x, d, z)).normalize()
}

pub fn rates_to_rotation(t: &WingsuitTuning, dt: f32, roll: f32, pitch: f32, yaw: f32) -> Quat {
    Quat::from_rotation_y(t.max_roll_vel * dt * roll)
        * Quat::from_rotation_z(t.max_pitch_vel * dt * pitch)
        * Quat::from_rotation_x(t.max_yaw_vel * dt * yaw)
}

fn fsel_sign(x: f32) -> f32 {
    if x >= 0.0 { 1.0 } else { -1.0 }
}

fn shaped(c: &Curve, x: f32) -> f32 {
    c.eval(x.abs()) * fsel_sign(x)
}

#[derive(Clone, Copy, Debug)]
pub struct ControllerInputs {
    pub j: Quat,
    pub velocity: Vec3,
    pub angular_velocity: Quat,
    pub wind: Vec3,
    pub stick: Vec2,
    pub effectiveness: f32,
    pub time_scalar: f32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ControllerOutputs {
    pub t: Vec3,
    pub dq: Quat,
    pub velocity: Vec3,
    pub lift: Vec3,
    pub use_percentage: f32,
    pub attitude: Vec3,
}

#[derive(Clone, Debug, PartialEq)]
pub struct WingsuitController {
    pub roll: Pid,
    pub pitch: Pid,
    pub yaw: Pid,
    pub first: bool,
}

impl WingsuitController {
    pub fn new(t: &WingsuitTuning) -> Self {
        Self {
            roll: Pid::new(t.roll_pid, t.max_roll_accel),
            pitch: Pid::new(t.pitch_pid, t.max_pitch_accel),
            yaw: Pid::new(t.yaw_pid, t.max_yaw_accel),
            first: true,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ControllerGameState {
    pub time: f32,
    pub has_touched_ground: bool,
    pub goofy: bool,
}

impl WingsuitController {
    pub fn update(
        &mut self,
        t: &WingsuitTuning,
        r: &ResolvedCharacterProperties,
        i: &ControllerInputs,
        g: &mut ControllerGameState,
        dt: f32,
    ) -> Option<ControllerOutputs> {
        g.time += dt;
        let eff = if i.effectiveness - 0.001 >= 0.0 { i.effectiveness } else { 0.001 };
        let t_eff = g.time / i.time_scalar / eff;
        if dt.abs() <= TINY {
            return None;
        }
        let v = i.velocity;
        let speed = v.length();
        let vf = velocity_frame(t, i.j, v, speed);
        if self.first {
            g.goofy = false;
            let w = i.j.conjugate() * i.angular_velocity * i.j;
            self.roll.set_driven(twist(w, Vec3::Y) / t.max_roll_vel);
            self.pitch.set_driven(twist(w, Vec3::Z) / t.max_pitch_vel);
            self.yaw.set_driven(twist(w, Vec3::X) / t.max_yaw_vel);
            if g.has_touched_ground {
                (g.time, g.has_touched_ground) = (0.0, false);
            }
            self.first = false;
        }
        let q = vf.conjugate() * i.j;
        let (roll, pitch, yaw) = (twist(q, Vec3::Y), twist(q, Vec3::Z), twist(q, Vec3::X));
        let attitude = Vec3::new(
            (roll / t.max_roll_angle).clamp(-1.0, 1.0),
            (pitch / t.max_pitch_angle).clamp(-1.0, 1.0),
            (yaw / t.max_yaw_angle).clamp(-1.0, 1.0),
        );
        let s_roll = t.max_roll_input * shaped(&t.roll_input, i.stick.x) - attitude.x;
        let pitch_speed = t.pitch_vs_speed.as_ref().map_or(0.0, |c| c.eval(speed));
        let s_pitch = (pitch_speed - shaped(&t.pitch_input, i.stick.y)) * t.max_pitch_input - attitude.y;
        let s_yaw = t.max_yaw_input * shaped(&t.yaw_input, i.stick.x) - attitude.z;
        self.roll.update(dt, s_roll);
        self.pitch.update(dt, s_pitch);
        self.yaw.update(dt, s_yaw);
        let dq = rates_to_rotation(t, dt, self.roll.x, self.pitch.x, self.yaw.x);
        let tau = (t_eff / r.htv_falloff_wingsuit).clamp(0.0, 1.0);
        let (hm, vm, gm) = (t.horizontal_terminal.eval(tau), t.vertical_terminal.eval(tau), t.gravity.eval(tau));
        let g_abs = r.gravity_wingsuit.abs();
        let kh = g_abs / (r.htv_wingsuit * hm).powi(2);
        let kv = g_abs / (r.vtv_wingsuit * vm).powi(2);
        let mut a = drag_accel(kh, kv, v, i.wind * dt, dt);
        a.y -= (r.gravity_wingsuit * gm).abs();
        let v1 = v + a * dt;
        let (x_axis, z_axis) = (i.j * Vec3::X, i.j * Vec3::Z);
        let lift_mag = t.lift.eval(attitude.y) * t.max_lift_coefficient * t.lift_time.eval(t_eff) * speed * speed * dt;
        let lift = -x_axis * lift_mag;
        let turn_mag = (t.roll_to_turn.eval(roll)
            * t.max_roll_to_lift_coefficient
            * t.roll_to_turn_time.eval(t_eff)
            * speed
            * speed)
            .min(t.max_roll_to_lift);
        let (s, c) = roll.sin_cos();
        let sg = if s.abs() <= 1.192_092_9e-7 { 1.0 } else { s.signum() };
        let turn = (-x_axis * (1.0 - c) + z_axis * (c * sg)).normalize() * turn_mag * dt;
        let mut v2 = v1 + lift + turn;
        if v2.y > t.maximum_y_velocity {
            v2.y = t.maximum_y_velocity;
        }
        Some(ControllerOutputs {
            t: i.j.conjugate() * (v2 * dt),
            dq,
            velocity: v2,
            lift,
            use_percentage: tau,
            attitude,
        })
    }
}

pub fn ballistic_step(
    r: &ResolvedCharacterProperties,
    v: Vec3,
    wind: Vec3,
    j: Quat,
    goofy: bool,
    dt: f32,
) -> (Vec3, bool) {
    let d = (j * Vec3::X).cross(Vec3::Y).dot(v);
    let goofy = if d.abs() > 1.192_092_9e-7 { d > 0.0 } else { goofy };
    if dt.abs() <= TINY {
        return (v, goofy);
    }
    let mut a = drag_accel(r.k_h_air, r.k_v_air, v, wind, dt);
    a.y -= r.g_air.y.abs();
    (v + a * dt, goofy)
}

#[derive(Clone, Debug, PartialEq)]
pub enum OldSide {
    Aerial { alignment: Quat, jump: bool, first: bool },
    Wingsuit(WingsuitController),
    Ballistic,
}

#[derive(Clone, Debug, PartialEq)]
pub struct OldNode {
    pub side: OldSide,
    pub blend: Option<NodeBlend>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct NodeBlend {
    pub old: Box<OldNode>,
    pub blend_time: f32,
    pub elapsed: f32,
}

impl NodeBlend {
    pub fn new(old: OldNode, blend_time: f32) -> Self {
        Self { old: Box::new(old), blend_time, elapsed: 0.0 }
    }

    pub fn advance(&mut self, dt: f32) -> Option<f32> {
        self.elapsed += dt;
        let w = ssx_assets::anim_eval::curve_weight(self.elapsed / self.blend_time.max(TINY), 0);
        (w > 0.01).then_some(w)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WingsuitNode {
    Enter,
    Fly,
    Exit,
}

pub const DEPLOY_END_TICKS: f32 = 17.0;
pub const RETRACT_END_TICKS: f32 = 20.0;
pub const BLEND_DEFAULT: f32 = 8.0;
pub const BLEND_JUMP_TO_ENTER: f32 = 12.0;
pub const BLEND_ENTER_TO_FLY: f32 = 30.0;
pub const BLEND_FLY_TO_EXIT: f32 = 15.0;
pub const BLEND_EXIT_TO_AIRBORNE: f32 = 15.0;

pub const HANDLE_DEPLOY_WINDOW: TagWindow = TagWindow::new(4.0, 6.0, false);
pub const HANDLE_DEPLOY_SPLINE: [f32; 7] = [0.0, 0.137_995_81, 0.316_406_25, 0.5, 0.683_593_75, 0.862_004_16, 1.0];
pub const HANDLE_RETRACT_WINDOW: TagWindow = TagWindow::new(10.0, 5.0, false);
pub const HANDLE_RETRACT_SPLINE: [f32; 6] = [0.0, 0.172_361_37, 0.389_489_53, 0.610_510_47, 0.827_638_6, 1.0];

#[derive(Clone, Debug, PartialEq)]
pub struct WingsuitState {
    pub signal: bool,
    pub controller: Option<WingsuitController>,
    pub ticks: f32,
    pub blend: Option<NodeBlend>,
    pub time: f32,
    pub effectiveness: f32,
    pub time_scalar: f32,
    pub handle_weight: f32,
    pub handle_tags: Option<(WingsuitNode, TagTracker)>,
    pub handle_saved: f32,
    pub lift_velocity: Vec3,
    pub use_percentage: f32,
    pub enabled: bool,
    pub race_proper_started: bool,
    pub force_descent_in_air: bool,
    pub pad_r1: bool,
    pub hold: bool,
    pub hold_prev: bool,
    pub last: Option<ControllerOutputs>,
}

impl Default for WingsuitState {
    fn default() -> Self {
        Self {
            signal: false,
            controller: None,
            ticks: 0.0,
            blend: None,
            time: 0.0,
            effectiveness: 1.0,
            time_scalar: 1.0,
            handle_weight: 0.0,
            handle_tags: None,
            handle_saved: 0.0,
            lift_velocity: Vec3::ZERO,
            use_percentage: 1.0,
            enabled: true,
            race_proper_started: false,
            force_descent_in_air: false,
            pad_r1: false,
            hold: false,
            hold_prev: false,
            last: None,
        }
    }
}

impl WingsuitState {
    pub fn can_use(&self) -> bool {
        self.race_proper_started || self.force_descent_in_air
    }

    pub fn press(&self) -> bool {
        self.hold && !self.hold_prev
    }

    pub fn wear(&mut self, t: &WingsuitTuning, r: &ResolvedCharacterProperties, running: bool, dt: f32) {
        if running && self.signal {
            let rate = t.base_damage_rate / r.wingsuit_durability_modifier;
            self.effectiveness = (self.effectiveness - rate * dt).max(0.0);
        }
    }

    pub fn decay_handles(&mut self, t: &WingsuitTuning, dt: f32) {
        if !self.signal {
            self.handle_weight = (self.handle_weight - dt / t.handle_decay_ticks).max(0.0);
        }
    }

    pub fn handle_tags(&mut self, node: Option<WingsuitNode>, time: f32) {
        let seq = node.filter(|n| *n != WingsuitNode::Fly);
        if let Some((n, tr)) = self.handle_tags.as_mut()
            && (Some(*n) != seq || time < tr.time())
        {
            let n = *n;
            for (_, e) in tr.exit() {
                self.handle_event(n, e);
            }
            self.handle_tags = None;
        }
        let Some(n) = seq else { return };
        let window = if n == WingsuitNode::Enter { HANDLE_DEPLOY_WINDOW } else { HANDLE_RETRACT_WINDOW };
        let tr = &mut self.handle_tags.get_or_insert_with(|| (n, TagTracker::new(&[window]))).1;
        let events = if tr.update(time) { tr.process() } else { Vec::new() };
        for (_, e) in events {
            self.handle_event(n, e);
        }
    }

    fn handle_event(&mut self, n: WingsuitNode, e: TagEvent) {
        let w = match e {
            TagEvent::Apply(w) | TagEvent::Leave(w) => w,
            TagEvent::Enter => 0.0,
        };
        let (value, spline) = match n {
            WingsuitNode::Enter => (1.0, tag_spline(&HANDLE_DEPLOY_SPLINE, w)),
            _ => (0.0, tag_spline(&HANDLE_RETRACT_SPLINE, w)),
        };
        FloatTag { value, mode: 3, reset: None, spline }.on(e, &mut self.handle_weight, &mut self.handle_saved);
    }
}

#[cfg(test)]
mod tests;
