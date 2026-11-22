use glam::Vec3;
use ssx_formats::vmx;

use crate::curve::Curve;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PadRumble {
    pub motors: [f32; 2],
    pub landing_weight: f32,
    pub landing_timer: f32,
    pub landing_total_time: f32,
    pub prev_in_air: bool,
    pub landing_impact_magnitude: f32,
    velocities: [Vec3; 2],
    pub sources: RumbleSources,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct RumbleSources {
    pub carving_inputs: CarvingRumbleInputs,
    pub carving: [f32; 2],
    pub landing: f32,
    pub bail_exit: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CarvingRumbleInputs {
    pub surface: f32,
    pub uber_sliding: bool,
    pub dead_on_ground: bool,
    pub speed: f32,
    pub avalanche_vel: Vec3,
    pub avalanche_mass: f32,
    pub grind_state: u32,
}

#[derive(Clone, Debug, Default)]
pub struct LandingRumbleTuning {
    pub min_speed: f32,
    pub max_speed: f32,
    pub min_weight: f32,
    pub max_weight: f32,
    pub min_time: f32,
    pub max_time: f32,
    pub time_curve: Curve,
}

fn clamp_unit(x: f32) -> f32 {
    let x = if -x >= 0.0 { 0.0 } else { x };
    if 1.0 - x >= 0.0 { x } else { 1.0 }
}

fn min_one(x: f32) -> f32 {
    if x - 1.0 >= 0.0 { 1.0 } else { x }
}

impl PadRumble {
    pub fn reset_motors(&mut self) {
        self.motors = [0.0; 2];
        self.sources = RumbleSources::default();
    }

    pub fn bail_exit(&mut self) {
        self.motors = [0.0; 2];
        self.sources.bail_exit = true;
    }

    pub fn carving(&mut self, i: &CarvingRumbleInputs, surface: &[Curve; 2], avalanche: &[Curve; 2]) {
        let slide = if i.uber_sliding && !i.dead_on_ground { 1.0 } else { 0.0 };
        let s = i.surface + slide;
        let v = i.avalanche_vel.to_array();
        let a = vmx::mul(crate::intercept::sqrt(vmx::dot3(v, v)), i.avalanche_mass);
        let (g0, g1) = if i.grind_state == 2 { (0.2, 0.0) } else { (1.0, 1.0) };
        let m0 = clamp_unit((s * surface[0].eval(i.speed)).mul_add(g0, avalanche[0].eval(a)));
        let m1 = clamp_unit((s * g1).mul_add(surface[1].eval(i.speed), avalanche[1].eval(a)));
        (self.sources.carving_inputs, self.sources.carving) = (*i, [m0, m1]);
        for (m, v) in self.motors.iter_mut().zip([m0, m1]) {
            if v > *m {
                *m = v;
            }
        }
    }

    pub fn landing(&mut self, t: &LandingRumbleTuning, in_air: bool, ground_valid: bool, ground_normal: Vec3, dt: f32) {
        let prev = self.velocities[1].to_array();
        let moving = prev.iter().any(|c| vmx::gt(c.abs(), f32::EPSILON));
        if !in_air && self.prev_in_air && ground_valid && moving {
            let d = vmx::dot3(prev, ground_normal.to_array());
            let m = vmx::max(f32::from_bits(d.to_bits() ^ 0x8000_0000), 0.0);
            self.landing_impact_magnitude = m;
            if m > t.min_speed {
                let x = min_one((m - t.min_speed) / (t.max_speed - t.min_speed));
                self.landing_weight = min_one(x).mul_add(t.max_weight - t.min_weight, t.min_weight);
                self.landing_total_time = (t.max_time - t.min_time).mul_add(x, t.min_time);
                self.landing_timer = 0.0;
            }
        } else {
            self.landing_impact_magnitude = 0.0;
        }
        self.prev_in_air = in_air;
        let total = self.landing_total_time;
        if total > 0.0 {
            let timer = self.landing_timer;
            let value = vmx::mul(t.time_curve.eval(timer / total), self.landing_weight);
            self.landing_timer = dt + timer;
            if self.landing_timer > total {
                self.landing_total_time = -1.0;
            }
            self.sources.landing = value;
            if value > self.motors[1] {
                self.motors[0] = value * 0.6;
            }
        }
    }

    pub fn cache_velocity(&mut self, traj_velocity: Vec3) {
        self.velocities = [traj_velocity, self.velocities[0]];
    }
}

#[cfg(test)]
mod tests;
