use std::f32::consts::{FRAC_PI_2, PI, TAU};

use anyhow::Result;
use glam::{Quat, Vec3};

use crate::actuator::crit_damped;
use crate::character::CarvingTuning;
use crate::curve::Curve;
use crate::steering::{SteeringState, ground_frame, slip_angle, wrap_pi};
use crate::tuning::Bundle;

pub const BAIL_ON_LANDING_PITCH: f32 = 2.356_194;
pub const PITCH_BALANCE_STIFFNESS: f32 = 0.01;
pub const PITCH_BALANCE_STABLE: f32 = 0.05;
pub const UBER_RECOVER_TRIGGER_CLEAR_TICKS: f32 = 110.0;
pub const UBER_RECOVER_HOP_TICKS: f32 = 4.0;
pub const UBER_CRASH_GATE_TICKS: f32 = 30.0;
pub const BAIL_ON_LANDING_MAX_TICKS: f32 = 180.0;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum UberRecovery {
    #[default]
    Sliding,
    GettingUp(f32),
    Hop(f32),
}

#[derive(Clone, Debug, Default)]
pub struct BalanceTuning {
    pub bail_angle_curve: Curve,
    pub bail_tumble_angular_velocity_curve: Curve,
    pub recovery: Curve,
    pub restore: Curve,
    pub recovery_helping: Curve,
    pub restore_helping: Curve,
    pub recovery_hindering: Curve,
    pub restore_hindering: Curve,
    pub impact_stiffness_multiplier: f32,
    pub spin_spring_stiffness: f32,
    pub impact_spin_spring_stiffness: f32,
    pub back_slide_threshold: f32,
    pub input_dead_zone: f32,
    pub stabilized_spin_vel: f32,
    pub stabilized_spin_angle: f32,
    pub stabilized_pitch_vel: f32,
    pub stabilized_pitch_angle: f32,
    pub pitch_balance_stable_threshold: f32,
    pub max_ticks_in_auto_backslide: f32,
    pub max_ticks_since_sig_trick_held_for_uber_bail: f32,
    pub uber_crash_ticks_to_full_friction: f32,
    pub max_pitch_angle_vel_to_prevent_braking: f32,
    pub min_landing_impact_velocity_ticks: f32,
    pub tumble_friction_curve: Curve,
    pub spin_impact: Curve,
    pub pitch_impact: Curve,
}

impl BalanceTuning {
    pub fn spin_stiffness(&self, use_impact_tuning: bool) -> f32 {
        if use_impact_tuning { self.impact_spin_spring_stiffness } else { self.spin_spring_stiffness }
    }

    pub fn load(bundle: &Bundle) -> Result<Self> {
        let a = bundle.view("carve_riding", "CarvingControllerAsset")?;
        Ok(Self {
            bail_angle_curve: a.curve("BailAngleCurve")?,
            bail_tumble_angular_velocity_curve: a.curve("BailTumbleAngularVelocityCurve")?,
            recovery: a.curve("AutoBalanceRecoveryStiffnessCurve")?,
            restore: a.curve("AutoBalanceRestoreStiffnessCurve")?,
            recovery_helping: a.curve("AutoBalanceRecoveryStiffnessCurveHelping")?,
            restore_helping: a.curve("AutoBalanceRestoreStiffnessCurveHelping")?,
            recovery_hindering: a.curve("AutoBalanceRecoveryStiffnessCurveHindering")?,
            restore_hindering: a.curve("AutoBalanceRestoreStiffnessCurveHindering")?,
            impact_stiffness_multiplier: a.f32("ImpactAutoBalanceStiffnessMultiplier")?,
            spin_spring_stiffness: a.f32("BalanceSpinSpringStiffness")?,
            impact_spin_spring_stiffness: a.f32("ImpactBalanceSpinSpringStiffness")?,
            back_slide_threshold: a.f32("BalanceBackSlideThreshold")?,
            input_dead_zone: a.f32("BalanceInputDeadZone")?,
            stabilized_spin_vel: a.f32("StabilizedSpinVel")?,
            stabilized_spin_angle: a.f32("StabilizedSpinAngle")?,
            stabilized_pitch_vel: a.f32("StabilizedPitchVel")?,
            stabilized_pitch_angle: a.f32("StabilizedPitchAngle")?,
            pitch_balance_stable_threshold: a.f32("PitchBalanceStableThreshold")?,
            max_ticks_in_auto_backslide: a.f32("MaxTicksInAutoBackslide")?,
            max_ticks_since_sig_trick_held_for_uber_bail: a.f32("MaxTicksSinceSigTrickHeldForUberBail")?,
            uber_crash_ticks_to_full_friction: a.f32("UberCrashTicksToFullFriction")?,
            max_pitch_angle_vel_to_prevent_braking: a.f32("MaxPitchAngleVelToPreventBraking")?,
            min_landing_impact_velocity_ticks: a.f32("MinLandingImpactVelocityTicks")?,
            tumble_friction_curve: a.curve("TumbleFrictionCurve")?,
            spin_impact: a.curve("SpinImpactCurve")?,
            pitch_impact: a.curve("PitchImpactCurve")?,
        })
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LandingQuality {
    #[default]
    None,
    Sloppy,
    Clean,
    Tumble,
    Bail,
    UberCrash,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LandingState {
    pub spin: f32,
    pub spin_vel: f32,
    pub spin_target: f32,
    pub pitch: f32,
    pub pitch_vel: f32,
    pub pitch_target: f32,
    pub landing_pitch_vel: f32,
    pub pitch_balance: f32,
    pub pitch_balance_vel: f32,
    pub bail_on_landing: bool,
    pub bailed_because_of_rock: bool,
    pub uber_crash: bool,
    pub landing_score: f32,
    pub stable: bool,
    pub stable_ticks: f32,
    pub tumbling: bool,
    pub backslide: bool,
    pub backslide_ticks: f32,
    pub auto_recover_backslide: bool,
    pub landed_switch: bool,
    pub ticks: f32,
    pub uber_recovery: UberRecovery,
    pub quality: LandingQuality,
    pub touchdown_pitch: f32,
    pub touchdown_spin: f32,
    pub use_impact_tuning: bool,
    pub spin_kick: f32,
    pub pitch_kick: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ImpactKick {
    pub pre_orientation: Quat,
    pub pre_velocity: Vec3,
    pub velocity: Vec3,
}

impl Default for ImpactKick {
    fn default() -> Self {
        Self { pre_orientation: Quat::IDENTITY, pre_velocity: Vec3::ZERO, velocity: Vec3::ZERO }
    }
}

impl ImpactKick {
    pub fn use_impact_tuning(&self) -> bool {
        self.velocity.abs().max_element() > 1.192_092_9e-7
    }

    pub fn spin_kick(&self, curve: &Curve, alignment: Quat, speed_threshold: f32) -> f32 {
        let speed = self.pre_velocity.length();
        if speed <= speed_threshold {
            return 0.0;
        }
        let up = alignment * Vec3::Y;
        let side = (self.pre_velocity / speed).cross(up).normalize_or_zero();
        let lat = side.dot(self.pre_orientation * self.velocity);
        fsel_sign(lat) * curve.eval(lat.abs())
    }

    pub fn pitch_kick(&self, curve: &Curve, ground: Quat) -> f32 {
        let y = (ground.conjugate() * (self.pre_orientation * self.velocity)).y;
        fsel_sign(-y) * curve.eval(y.abs())
    }
}

fn sgn(x: f32) -> f32 {
    if x > 0.0 {
        1.0
    } else if x < 0.0 {
        -1.0
    } else {
        0.0
    }
}

fn fsel_sign(x: f32) -> f32 {
    if x >= 0.0 { 1.0 } else { -1.0 }
}

pub use crate::steering::twist;

pub fn spring_accel_after(k: f32, angle: f32, vel: f32, target: f32, dt: f32) -> f32 {
    let e0 = angle - target;
    let c = vel + k * e0;
    (k * k * (e0 + c * dt) - 2.0 * k * c) * (-k * dt).exp()
}

pub fn spin_target(k: f32, spin: f32, vel: f32, kick: f32, s: f32, dt: f32) -> (f32, f32) {
    let sgn = fsel_sign(vel);
    let round_pi = |x: f32| (x / PI + 0.5).floor() * PI;
    let accel = |target: f32| spring_accel_after(k, spin, vel, target, dt);
    let (mut best, mut best_a) = (0.0, accel(0.0));
    let mut consider = |target: f32| {
        let a = accel(target);
        if a.abs() <= best_a.abs() {
            (best, best_a) = (target, a);
        }
    };
    consider(round_pi(spin + sgn * PI));
    if kick.abs() > 1.192_092_9e-7 {
        consider(round_pi(spin + sgn * TAU));
    }
    if best * s < -FRAC_PI_2 { (spin + s * TAU, best + s * TAU) } else { (spin, best) }
}

impl LandingState {
    #[allow(clippy::too_many_arguments)]
    pub fn touchdown(
        &mut self,
        t: &BalanceTuning,
        c: &CarvingTuning,
        alignment: &mut Quat,
        steering: &mut SteeringState,
        v: Vec3,
        n: Vec3,
        w: Quat,
        uber: bool,
        allow_bails: bool,
        impact: &ImpactKick,
        dt: f32,
    ) {
        *steering = SteeringState::new(c);
        if uber {
            *self =
                Self { uber_crash: true, landing_score: 1.0, quality: LandingQuality::UberCrash, ..Self::default() };
            return;
        }
        let mut a = *alignment;
        let use_impact_tuning = impact.use_impact_tuning();
        let spin_kick = impact.spin_kick(&t.spin_impact, a, c.speed_threshold);
        let spin = slip_angle(c, ground_frame(a, n).conjugate() * v, steering.cached_slip);
        let spin_vel = twist(a.conjugate() * w * a, Vec3::Y) + spin_kick;
        a = (a * Quat::from_rotation_y(-spin)).normalize();
        let pitch_kick = impact.pitch_kick(&t.pitch_impact, ground_frame(a, n));
        let pitch_vel = twist(a.conjugate() * w * a, Vec3::Z);
        let pitch = twist(ground_frame(a, n).conjugate() * a, Vec3::Z);
        let bail = allow_bails && pitch.abs() > BAIL_ON_LANDING_PITCH;
        a = (a * Quat::from_rotation_z(-pitch)).normalize();
        *alignment = a;
        let g = ground_frame(a, n);
        let vg = a.conjugate() * (v - n * v.dot(n));
        let landed_switch =
            vg.x * vg.x + vg.z * vg.z > c.speed_threshold * c.speed_threshold && vg.z.atan2(vg.x).abs() > FRAC_PI_2;
        let v_fwd = (g.conjugate() * v).x;
        let s = if v_fwd.abs() <= c.speed_threshold { steering.forward_sign } else { fsel_sign(v_fwd) };
        let pitch_vel = pitch_vel + s * pitch_kick;
        let k = t.spin_stiffness(use_impact_tuning).max(0.0).sqrt();
        let yaw = spin;
        let (spin, spin_target) = spin_target(k, spin, spin_vel, spin_kick, s, dt);
        *self = Self {
            spin,
            spin_vel,
            spin_target,
            pitch,
            pitch_vel,
            landing_pitch_vel: pitch_vel,
            use_impact_tuning,
            spin_kick,
            pitch_kick: s * pitch_kick,
            bail_on_landing: bail,
            landing_score: 1.0,
            landed_switch,
            quality: if bail { LandingQuality::Bail } else { LandingQuality::Sloppy },
            touchdown_pitch: pitch,
            touchdown_spin: yaw,
            ..Self::default()
        };
    }

    pub fn is_upright(&self, t: &BalanceTuning) -> bool {
        !self.tumbling
            && (self.stable
                || sgn(self.landing_pitch_vel) != fsel_sign(self.pitch_vel)
                || self.pitch_vel.abs() < t.max_pitch_angle_vel_to_prevent_braking)
    }

    pub fn is_bailing(&self) -> bool {
        self.bail_on_landing || self.uber_crash
    }
}

impl LandingState {
    pub fn recovery_input(&self, t: &BalanceTuning, fwd: f32, stick_y: f32) -> bool {
        stick_y.abs() > t.input_dead_zone && fsel_sign(stick_y) == fsel_sign(fwd * self.pitch)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn balance_update(
        &mut self,
        t: &BalanceTuning,
        fwd: f32,
        stick_y: f32,
        speed: f32,
        recovery_input: bool,
        use_leg_spring: bool,
        uber_sliding: bool,
        grinding: bool,
        is_dead: bool,
        level_complete: bool,
        board_press: bool,
        dt: f32,
    ) {
        self.ticks += dt;
        // TODO(guess): a landing bail ends after BAIL_ON_LANDING_MAX_TICKS (180, a stand-in) on the ground; without it only an air phase, an impact or level complete end one (issue #5: riders stay head-down until they reach a gap); settle: what ends `BLN_carving_state_bail_on_landing` in the game's graph or carving controller, and after how long.
        if self.bail_on_landing && self.ticks >= BAIL_ON_LANDING_MAX_TICKS {
            self.bail_on_landing = false;
        }
        let input_active = stick_y.abs() > t.input_dead_zone;
        let helping_input = input_active && fsel_sign(stick_y) == fsel_sign(fwd * self.pitch);
        let bail_angle = t.bail_angle_curve.eval(wrap_pi(self.spin).abs());
        let wp = wrap_pi(self.pitch);
        let backslide = (fwd < 0.0 && wp < -t.back_slide_threshold) || (fwd > 0.0 && wp > t.back_slide_threshold);
        if input_active {
            self.backslide_ticks = 0.0;
            self.auto_recover_backslide = false;
        }
        if is_dead && !self.bailed_because_of_rock && !backslide {
            self.bailed_because_of_rock = true;
        }
        let rock_or_dead = self.bailed_because_of_rock || is_dead;
        let lpv = sgn(self.landing_pitch_vel);
        let restore = lpv != fsel_sign(self.pitch_vel);
        let same_side = lpv == fsel_sign(self.pitch);
        let mut backslide_target = false;
        if self.pitch.abs() >= PI && !self.bail_on_landing && !rock_or_dead {
            self.pitch_vel = -(t.bail_tumble_angular_velocity_curve.eval(speed) * fwd);
            self.pitch += dt * self.pitch_vel;
        } else {
            let p = self.pitch.abs();
            let helping = helping_input || level_complete || self.auto_recover_backslide || grinding;
            let stiffness = if !rock_or_dead && helping {
                self.pitch_target = 0.0;
                if restore { t.restore_helping.eval(p) } else { t.recovery_helping.eval(p) }
            } else {
                let s = match (input_active && same_side, restore) {
                    (true, true) => t.restore_hindering.eval(p),
                    (true, false) => t.recovery_hindering.eval(p),
                    (false, true) => t.restore.eval(p),
                    (false, false) => t.recovery.eval(p),
                };
                if backslide {
                    backslide_target = true;
                    self.pitch_target = fwd * FRAC_PI_2;
                }
                s
            };
            let mut target = self.pitch_target;
            if restore
                && fsel_sign(self.pitch_balance) == fsel_sign(self.pitch_balance_vel)
                && self.pitch_balance.abs() > t.pitch_balance_stable_threshold
                && self.pitch.abs() < FRAC_PI_2
            {
                target = self.pitch;
            }
            if self.bail_on_landing {
                target = fsel_sign(self.pitch) * PI;
            }
            let stiffness = if self.use_impact_tuning { stiffness * t.impact_stiffness_multiplier } else { stiffness };
            let (e, r) = crit_damped(self.pitch - target, self.pitch_vel, stiffness.max(0.0).sqrt(), dt);
            (self.pitch, self.pitch_vel) = (e + target, r);
        }
        if !self.bail_on_landing && ((fwd < 0.0 && self.pitch > bail_angle) || (fwd > 0.0 && self.pitch < -bail_angle))
        {
            self.landing_score = 0.0;
            self.pitch += fwd * TAU;
        }
        let (e, r) = crit_damped(self.pitch_balance, self.pitch_balance_vel, PITCH_BALANCE_STIFFNESS.sqrt(), dt);
        (self.pitch_balance, self.pitch_balance_vel) = (e, r);
        let k = t.spin_stiffness(self.use_impact_tuning).max(0.0).sqrt();
        let mut target = if backslide_target { fwd * FRAC_PI_2 } else { self.spin_target };
        if self.bail_on_landing {
            target = self.spin;
        }
        let (e, r) = crit_damped(self.spin - target, self.spin_vel, k, dt);
        (self.spin, self.spin_vel) = (e + target, r);
        let leg_spring = use_leg_spring;
        self.stable = grinding
            || self.pitch_vel.abs() < t.stabilized_pitch_vel
                && self.pitch.abs() < t.stabilized_pitch_angle
                && self.spin_vel.abs() < t.stabilized_spin_vel
                && wrap_pi(target - self.spin).abs() < t.stabilized_spin_angle
                && (self.pitch_balance.abs() < PITCH_BALANCE_STABLE || board_press)
                && (leg_spring || board_press);
        self.stable_ticks = if self.is_upright(t) { self.stable_ticks + dt } else { 0.0 };
        let wp = wrap_pi(self.pitch);
        let over = (fwd < 0.0 && wp > bail_angle) || (fwd > 0.0 && wp < -bail_angle);
        self.backslide = backslide_target;
        self.tumbling = self.bail_on_landing || backslide || uber_sliding || over;
        self.backslide_ticks = if backslide_target { self.backslide_ticks + dt } else { 0.0 };
        if recovery_input || self.backslide_ticks > t.max_ticks_in_auto_backslide {
            self.auto_recover_backslide = true;
        }
        self.use_impact_tuning = self.use_impact_tuning && !self.stable;
        self.update_quality();
    }

    fn update_quality(&mut self) {
        self.quality = match self.quality {
            LandingQuality::Sloppy | LandingQuality::Clean if self.landing_score == 0.0 => LandingQuality::Tumble,
            LandingQuality::Sloppy if self.stable => LandingQuality::Clean,
            q => q,
        };
    }

    pub fn fresh_controller(&mut self) {
        *self = Self {
            landing_score: 1.0,
            quality: self.quality,
            touchdown_pitch: self.touchdown_pitch,
            touchdown_spin: self.touchdown_spin,
            ..Self::default()
        };
    }

    pub fn adhesion_factor(&self, curve: &Curve) -> f32 {
        if self.bail_on_landing {
            0.0
        } else if !self.stable {
            curve.eval(wrap_pi(self.pitch).abs())
        } else {
            1.0
        }
    }
}

#[cfg(test)]
mod tests;
