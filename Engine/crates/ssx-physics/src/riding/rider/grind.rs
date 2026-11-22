pub mod measure;
pub mod path;
pub mod state;
pub mod steering;
pub mod surface_analysis;
pub mod tricks;

#[cfg(test)]
mod tests;

use std::f32::consts::{PI, TAU};

use std::sync::Arc;

use anyhow::Result;
use glam::{Quat, Vec3};

use crate::collision::CollisionWorld;
use crate::curve::Curve;
use crate::tuning::{AssetView, Bundle};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct GrindProperties {
    pub max_active_angular_vel_on_entry: f32,
    pub max_active_angular_vel_steady_state: f32,
    pub max_approach_angular_vel: f32,
    pub max_approach_angular_vel_gentle: f32,
    pub max_joystick_exit_angle: f32,
    pub max_approach_angle: f32,
    pub max_tail_angle: f32,
    pub max_path_angle: f32,
    pub max_path_height_fall: f32,
    pub max_path_height_rise: f32,
    pub min_ridgiprocity_active: f32,
    pub min_ridgiprocity_start: f32,
    pub min_speed_threshold_active: f32,
    pub min_speed_threshold_start: f32,
    pub min_target_speed: f32,
    pub min_target_speed_gain: f32,
}

impl GrindProperties {
    pub fn load(a: &AssetView<'_>) -> Result<Self> {
        Ok(Self {
            max_active_angular_vel_on_entry: a.f32("MaxActiveAngularVelOnEntry")?,
            max_active_angular_vel_steady_state: a.f32("MaxActiveAngularVelSteadyState")?,
            max_approach_angular_vel: a.f32("MaxApproachAngularVel")?,
            max_approach_angular_vel_gentle: a.f32("MaxApproachAngularVelGentle")?,
            max_joystick_exit_angle: a.f32("MaxJoystickExitAngle")?,
            max_approach_angle: a.f32("MaxApproachAngle")?,
            max_tail_angle: a.f32("MaxTailAngle")?,
            max_path_angle: a.f32("MaxPathAngle")?,
            max_path_height_fall: a.f32("MaxPathHeightFall")?,
            max_path_height_rise: a.f32("MaxPathHeightRise")?,
            min_ridgiprocity_active: a.f32("MinRidgiprocityActive")?,
            min_ridgiprocity_start: a.f32("MinRidgiprocityStart")?,
            min_speed_threshold_active: a.f32("MinSpeedThresholdActive")?,
            min_speed_threshold_start: a.f32("MinSpeedThresholdStart")?,
            min_target_speed: a.f32("MinTargetSpeed")?,
            min_target_speed_gain: a.f32("MinTargetSpeedGain")?,
        })
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct GrindTuning {
    pub regular: GrindProperties,
    pub super_: GrindProperties,
    pub input_dead_zone_angle: f32,
    pub spin_velocity_gain: f32,
    pub spin_angle_gain: f32,
    pub blend_value_gain: f32,
    pub press_stiffness: f32,
    pub normal_decay_gain: f32,
    pub veering_angle_stiffness: f32,
    pub thrust_curve: Curve,
    pub max_lean_input_curve: Curve,
    pub wind_modifier: f32,
}

impl GrindTuning {
    pub fn load(bundle: &Bundle) -> Result<Self> {
        let c = bundle.view("carve_riding", "CarvingControllerAsset")?;
        Ok(Self {
            regular: GrindProperties::load(&c.asset("GrindTuningRegularAsset")?)?,
            super_: GrindProperties::load(&c.asset("GrindTuningSuperAsset")?)?,
            input_dead_zone_angle: c.f32("GrindInputDeadZoneAngle")?,
            spin_velocity_gain: c.f32("GrindSpinVelocityGain")?,
            spin_angle_gain: c.f32("GrindSpinAngleGain")?,
            blend_value_gain: c.f32("GrindBlendValueGain")?,
            press_stiffness: c.f32("GrindPressStiffness")?,
            normal_decay_gain: c.f32("GrindNormalDecayGain")?,
            veering_angle_stiffness: c.f32("GrindVeeringAngleStiffness")?,
            thrust_curve: c.curve("GrindThrustCurve")?,
            max_lean_input_curve: c.curve("SpeedVsMaxLeanInputAngleCurveGrindingAsset")?,
            wind_modifier: c.f32("OutOfBoundsWindModifier")?,
        })
    }
}

pub fn grind_input(dead_zone: f32, angle: f32, mag: f32, nose: bool, tail: bool) -> (f32, f32) {
    let s: f32 = if angle >= 0.0 { 1.0 } else { -1.0 };
    let t = s.mul_add(angle, -dead_zone) / -dead_zone.mul_add(2.0, -PI);
    let t = if -t >= 0.0 { 0.0 } else { t };
    let t = if 1.0 - t >= 0.0 { t } else { 1.0 };
    let th = f64::from(t * s * PI);
    let press = if nose {
        1.0
    } else if tail {
        -1.0
    } else {
        th.cos() as f32 * mag
    };
    (th.sin() as f32 * mag, press)
}

pub fn damped_spring(stiffness: f32, x: f32, v: f32, target: f32, t: f32) -> (f32, f32) {
    let w = stiffness.max(0.0).sqrt();
    let x0 = x - target;
    let c = x0 * w + v;
    let e = (-w * t).exp();
    ((x0 + c * t) * e + target, (v - w * c * t) * e)
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Press {
    pub value: f32,
    pub vel: f32,
    pub sign: f32,
}

pub fn press_update(p: &mut Press, t: &GrindTuning, board_angle: f32, grind_or_press_blend: f32, input: f32, dt: f32) {
    let within = |x: f32, e: f32| x.abs() <= e || x.is_nan();
    let mut sign = p.sign;
    if within(sign, 1.192_092_9e-7) || (within(p.value, 0.05) && within(input, 0.05)) {
        let s = if board_angle >= 0.0 { 1.0 } else { -1.0 };
        let a = s * board_angle;
        sign = if a < t.input_dead_zone_angle || a > PI - t.input_dead_zone_angle { 0.0 } else { s };
    }
    let target = grind_or_press_blend * sign * input;
    let w = crate::intercept::sqrt(t.press_stiffness);
    (p.value, p.vel) = crate::ground::critically_damped_step(w, p.value, p.vel, target, dt);
    p.sign = sign;
}

pub fn blend_update(blend: f32, grinding: bool, gain: f32, dt: f32) -> f32 {
    let target = if grinding { 1.0 } else { 0.0 };
    let e = f64::from(-(gain * dt)).exp() as f32;
    e.mul_add(blend - target, target)
}

pub fn veering_update(angle: &mut f32, vel: &mut f32, stiffness: f32, n: Vec3, v_old: Vec3, v_new: Vec3, dt: f32) {
    let a = v_old - n * v_old.dot(n);
    let b = v_new - n * v_new.dot(n);
    let unit = |p: Vec3| p * (1.0 / p.length());
    let c = unit(a).dot(unit(b)).clamp(-1.0, 1.0);
    let s = if b.cross(a).dot(n) >= 0.0 { 1.0 } else { -1.0 };
    let target = c.acos() * s * 57.295_78 / dt;
    (*angle, *vel) = damped_spring(stiffness, *angle, *vel, target, dt);
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpinController {
    pub max_vel: f32,
    pub vel_gain: f32,
    pub angle_gain: f32,
    pub angle: f32,
    pub rate: f32,
}

const SPIN_EPS: f32 = 1.525_878_9e-5;
const SPIN_DIVISIONS: f32 = 4.0;

impl SpinController {
    pub fn update(&mut self, dt: f32, target: f32) {
        if !(-SPIN_EPS..=SPIN_EPS).contains(&target) {
            return self.free(dt, target);
        }
        let step = TAU / SPIN_DIVISIONS;
        let goal = if self.rate.abs() <= SPIN_EPS {
            ((step * 0.5 + self.angle) / step).floor() * step
        } else {
            let s = if self.rate >= 0.0 { 1.0 } else { -1.0 };
            let e1 = (-self.vel_gain).exp();
            let e2 = (-self.angle_gain * dt).exp();
            let predicted = ((self.max_vel * s * (e1 - 1.0)) * dt + self.rate) * dt + self.angle;
            let settle = (predicted - self.angle * e2) / (1.0 - e2);
            ((settle * s + step) / step).floor() * (s * step)
        };
        let e = (-self.angle_gain * dt).exp();
        let next = e * (self.angle - goal) + goal;
        let rate = (next - self.angle) / dt;
        let sign = |x: f32| if x >= 0.0 { 1.0 } else { -1.0 };
        if sign(rate) != sign(self.rate) || rate.abs() > self.rate.abs() {
            return self.free(dt, rate);
        }
        self.rate = rate;
        self.angle = wrap_tau(next);
    }

    fn free(&mut self, dt: f32, target: f32) {
        let t = target.clamp(-self.max_vel, self.max_vel);
        self.rate = t + (self.rate - t) * (-self.vel_gain * dt).exp();
        self.angle = wrap_tau(self.rate * dt + self.angle);
    }
}

fn wrap_tau(x: f32) -> f32 {
    let f = x / TAU;
    (f - f.floor()) * TAU
}

#[derive(Clone, Debug, Default)]
pub struct GrindRider {
    pub carving: state::GrindCarving,
    pub path: state::PathFrame,
    pub sa: Option<Arc<surface_analysis::SurfaceAnalysis>>,
    pub feature: surface_analysis::SaFeature,
    pub sa_ground: surface_analysis::SaGround,
    pub steering: steering::GrindSteering,
    pub ticks_airborne: f32,
    pub prev_is_grinding: bool,
    pub tail_pos: Vec3,
    pub tail_dir: Vec3,
    pub wind: Vec3,
    pub debug: Option<state::MachineDebug>,
    pub pad_l2: bool,
    pub button: bool,
    pub pad_l2_hold: bool,
    pub l2_hold: bool,
    pub stick: (f32, f32),
}

impl GrindRider {
    pub fn set_l2(&mut self, held: bool) {
        (self.pad_l2, self.pad_l2_hold) = (held, held);
    }

    pub fn release_l2(&mut self) {
        self.set_l2(false);
    }

    pub fn is_grinding(&self) -> bool {
        self.carving.is_grinding
    }
}

pub const VELOCITY_TAIL_DISTANCE: f32 = 8.0;

pub fn player_update(g: &mut GrindRider, i: &PlayerUpdateInputs) {
    g.prev_is_grinding = g.carving.is_grinding;
    g.ticks_airborne = if i.in_air && !i.nis { g.ticks_airborne + i.dt } else { 0.0 };
    g.carving.is_grinding &= !i.in_air && !i.nis;
    let sa = g.sa.clone();
    let si = steering::SteeringInputs {
        sa: sa.as_deref(),
        c: i.c,
        t: i.steering,
        in_air: i.in_air,
        ticks_airborne: g.ticks_airborne,
        disabled: i.nis || i.in_wingsuit || i.board_particle,
        traj: i.traj,
        velocity: i.velocity,
        look_ahead_vel: g.feature.look_ahead.vel,
        k_h_air: i.k_h_air,
        k_v_air: i.k_v_air,
        g_air: i.g_air,
        dt: i.dt,
    };
    g.steering.update(&si);
    g.wind = g.steering.wind + i.boundary_wind;
    let n = (i.traj - g.tail_pos).normalize_or_zero();
    (g.tail_dir, g.tail_pos) = (n, i.traj - n * VELOCITY_TAIL_DISTANCE);
}

pub struct PlayerUpdateInputs<'a> {
    pub c: &'a CollisionWorld,
    pub steering: &'a steering::SteeringTuning,
    pub in_air: bool,
    pub nis: bool,
    pub traj: Vec3,
    pub velocity: Vec3,
    pub k_h_air: f32,
    pub k_v_air: f32,
    pub g_air: f32,
    pub in_wingsuit: bool,
    pub board_particle: bool,
    pub boundary_wind: Vec3,
    pub dt: f32,
}

pub fn stick_direction(angle: f32, mag: f32, v: Vec3, n: Vec3) -> Vec3 {
    use crate::steering::{vmx_cross, vmx_quat_mul, vmx_rotate};
    use ssx_formats::vmx::{add, dot3, eq, ge, gt, mul, negate, rsqrt, sin_cos, sub};
    const EPS: f32 = 1.192_092_9e-7;
    let scale = |a: [f32; 3], k: f32| a.map(|x| mul(x, k));
    let (v, n) = (v.to_array(), n.to_array());
    let nh = scale(n, rsqrt(dot3(n, n)));
    let c = vmx_cross(v, n);
    let c2 = dot3(c, c);
    let rc = rsqrt(c2);
    let len = if eq(c2, 0.0) { 0.0 } else { mul(c2, rc) };
    let (r, len) = if gt(len, 0.0) { (scale(c, rc), len) } else { ([0.0; 3], 0.0) };
    if ge(EPS, mag.abs()) || ge(EPS, len.abs()) {
        return Vec3::ZERO;
    }
    let fc = vmx_cross(nh, r);
    let f = scale(fc, rsqrt(dot3(fc, fc)));
    let (fx, ny, rz) = (f[0], nh[1], r[2]);
    let lanes =
        [(fx, ny, rz), (fx, negate(ny), negate(rz)), (negate(fx), ny, negate(rz)), (negate(fx), negate(ny), rz)];
    let t4 = lanes.map(|(a, b, c)| add(add(a, b), add(c, 1.0)));
    let rs = t4.map(rsqrt);
    let sq: [f32; 4] = std::array::from_fn(|k| mul(t4[k], rs[k]));
    let hr = rs.map(|x| mul(0.5, x));
    let d = [sub(nh[2], r[1]), sub(r[0], f[2]), sub(f[1], nh[0])];
    let e = [add(nh[2], r[1]), add(r[0], f[2]), add(f[1], nh[0])];
    let qw = [mul(d[0], hr[0]), mul(d[1], hr[0]), mul(d[2], hr[0]), mul(0.5, sq[0])];
    let qx = [mul(0.5, sq[1]), mul(e[2], hr[1]), mul(e[1], hr[1]), mul(d[0], hr[1])];
    let qy = [mul(e[2], hr[2]), mul(0.5, sq[2]), mul(e[0], hr[2]), mul(d[1], hr[2])];
    let qz = [mul(e[1], hr[3]), mul(e[0], hr[3]), mul(0.5, sq[3]), mul(d[2], hr[3])];
    let sel = if gt(ny, rz) { qy } else { qz };
    let sel = if gt(fx, ny) && gt(fx, rz) { qx } else { sel };
    let q = Quat::from_array(if gt(add(add(fx, ny), rz), 0.0) { qw } else { sel });
    let (sn, cs) = sin_cos(mul(angle, 0.5));
    let turn = Quat::from_xyzw(mul(-0.0, sn), mul(-1.0, sn), mul(-0.0, sn), cs);
    vmx_rotate(vmx_quat_mul(q, turn), Vec3::X)
}

pub const RAIL_MAX_DEPTH: f32 = 0.1;
pub const FOLLOW_BURIED_RAILS: bool = false;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CarvingFrame {
    pub contact: Option<Vec3>,
    pub n: Vec3,
}

pub fn carving_frame(
    g: &mut GrindRider,
    joint: Vec3,
    ground: Option<(Vec3, Vec3)>,
    velocity: Vec3,
    dt: f32,
) -> CarvingFrame {
    let sa = g.sa.as_deref().unwrap_or(surface_analysis::initial());
    g.path = path::carving_path(sa, joint, velocity, dt);
    let st = g.carving.state;
    let sa_contact =
        (ground.is_none() && g.sa_ground.contact_valid && st == 2 && g.path.valid).then_some(g.sa_ground.contact);
    let contact = ground.map(|(p, _)| p).or(sa_contact);
    let base = ground.map_or(Vec3::Y, |(_, n)| n);
    let n = match contact {
        Some(p) => state::grind_contact_normal(st, p, &mut g.path, velocity, base),
        None => base,
    };
    CarvingFrame { contact, n }
}

pub fn landing_check(gc: &mut state::GrindCarving, c: &CollisionWorld, ground: Option<Vec3>, traj: Vec3) {
    gc.auto_grind_clearing = false;
    let r = state::landing_rail(c, ground, traj, RAIL_MAX_DEPTH);
    (gc.auto_grind, gc.super_grind) = (r, r);
}

pub fn carving_ctor(gc: &mut state::GrindCarving, c: &CollisionWorld, ground: Option<Vec3>, traj: Vec3) {
    (gc.board_angle, gc.veering_angle, gc.veering_vel) = (0.0, 0.0, 0.0);
    landing_check(gc, c, ground, traj);
    (gc.state, gc.blend) = if gc.auto_grind { (2, 1.0) } else { (0, 0.0) };
    gc.press = Press::default();
    (gc.prev_input, gc.ticks) = (false, 0.0);
    gc.grind_or_press_blend = gc.blend.max(0.0);
}

pub struct DispatchState<'a> {
    pub r: &'a crate::character::ResolvedCharacterProperties,
    pub traj: Vec3,
    pub velocity: Vec3,
    pub orientation: glam::Quat,
    pub in_air: bool,
    pub ballistic: &'a crate::ballistic::BallisticRay,
    pub l2: bool,
    pub dt: f32,
}

pub fn dispatch(g: &mut GrindRider, d: &DispatchState) -> surface_analysis::SaJob {
    let gs = &g.steering;
    let i = surface_analysis::DispatchInputs {
        traj: d.traj,
        velocity: d.velocity,
        orientation: d.orientation,
        in_air: d.in_air,
        grind_steering: gs.active.then_some((gs.target_pos, gs.target_vel)),
        ballistic_valid: d.ballistic.valid,
        ballistic_end: (d.ballistic.end_position, d.ballistic.end_velocity),
        ticks_to_land: d.ballistic.ticks,
        l2: d.l2,
        super_grind: g.carving.super_grind,
        wind: g.wind,
        k_h_air: d.r.k_h_air,
        k_v_air: d.r.k_v_air,
        g_air: d.r.g_air.y.abs(),
        dt: d.dt,
    };
    g.feature.prepare(&i)
}
