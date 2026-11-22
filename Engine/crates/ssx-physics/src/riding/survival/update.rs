use super::*;

impl SurvivalState {
    pub fn init_outputs(&mut self, t: &SurvivalTuning, r: &ResolvedCharacterProperties) {
        if self.oxygen_flow < 0.0 {
            self.oxygen_flow = (t.base_oxygen_loss_threshold * r.oxygen_loss_threshold_modifier
                + t.base_oxygen_gain_threshold * r.oxygen_gain_threshold_modifier)
                * 0.5;
        }
        if self.armour < 0.0 {
            self.armour = if r.physical_damage_modifier > 0.0 { 1.0 } else { 0.0 };
        }
    }

    pub fn loop_to_top(&mut self, t: &SurvivalTuning, r: &ResolvedCharacterProperties) {
        self.lap_count += 1;
        self.health += t.per_lap_health_bonus;
        self.temperature += t.per_lap_temperature_bonus;
        self.oxygen_flow = (t.base_oxygen_loss_threshold * r.oxygen_loss_threshold_modifier
            + t.base_oxygen_gain_threshold * r.oxygen_gain_threshold_modifier)
            * 0.5;
        self.oxygen_flow_vel = 0.0;
        self.oxygen += t.per_lap_oxygen_bonus;
        self.oxygen_tank += t.per_lap_oxygen_tank_bonus;
    }

    pub fn update_health(
        &mut self,
        t: &SurvivalTuning,
        r: &ResolvedCharacterProperties,
        d: &DropSurvival,
        f: RunFlags,
        impact: ImpactInputs,
        dt: f32,
    ) {
        let permanent = d.death_permanent;
        let max_of = |total: f32| if permanent { 1.0 - total * t.permanent_health_loss_percent } else { 1.0 };
        self.health_max = max_of(self.total_damage_taken);
        let min_health = if permanent { 0.0 } else { 1.525_878_9e-5 };
        let laps = (self.lap_count + 1) as f32;
        if f.running() && self.health > 0.0 {
            let mut damage = 0.0;
            if self.ticks_since_impact_damage < t.min_ticks_between_impact_damage {
                self.ticks_since_impact_damage += dt;
            } else if impact.uber_bail {
                damage = t.uber_bail_damage_amount;
            } else if impact.this_frame {
                damage = impact_damage(t, impact.speed, impact.pre_velocity);
                self.ticks_since_impact_damage = 0.0;
            } else if self.one_off_damage != 0.0 {
                self.ticks_since_impact_damage = 0.0;
                damage = self.one_off_damage;
            }
            if impact.uber_bail {
                self.ticks_since_impact_damage = 0.0;
            }
            if self.carving_damage > 0.0 {
                damage += dt * self.carving_damage;
            }
            if damage > 0.0 {
                self.ticks_since_damaged = 0.0;
                let dmg = laps * d.per_level.impact_damage * damage;
                if self.armour > 0.0 {
                    let k = r.physical_damage_modifier * t.base_armour_damage_modifier;
                    self.armour = (self.armour - k * dmg).max(0.0);
                } else {
                    self.health -= dmg;
                    self.total_damage_taken += dmg;
                }
                self.health_max = max_of(self.total_damage_taken);
            } else {
                self.ticks_since_damaged += dt;
                if self.health > 0.0 || !permanent {
                    let (delay, rate) = if permanent {
                        (t.health_recovery_delay_survival, t.health_recovery_over_time_survival)
                    } else {
                        (t.health_recovery_delay_regular, t.health_recovery_over_time_regular)
                    };
                    if self.ticks_since_damaged >= delay {
                        self.health += dt * rate * (r.health_regen_modifier * d.per_level.impact_damage);
                    }
                    if self.health_gained_from_tricks > 0.0 {
                        self.health += d.per_level.impact_regen * self.health_gained_from_tricks;
                    }
                }
            }
            self.health = self.health.max(min_health).min(self.health_max);
        }
        self.at_min_health = (self.health - min_health).abs() < 1.525_878_9e-5;
        self.trigger_impact_bail = (self.at_min_health && impact.this_frame) || self.trigger_death_bail;
        self.trigger_death_bail = false;
        self.update_health_vignette(t, f, dt);
        (self.carving_damage, self.one_off_damage, self.health_gained_from_tricks) = (0.0, 0.0, 0.0);
    }

    pub(super) fn update_health_vignette(&mut self, t: &SurvivalTuning, f: RunFlags, dt: f32) {
        let (target, stiffness) = if self.health_max <= 0.0 {
            (0.0, t.vignette_health_stiffness)
        } else {
            let vmin = t.vignette_min_permanent_health;
            let denom = if self.health_max < vmin { (self.health_max + vmin) * 0.5 } else { self.health_max };
            let x = self.health / denom;
            if f.nis || f.finished {
                (1.0, t.vignette_health_stiffness)
            } else if self.ticks_since_damaged < t.instantaneous_damage_display_time {
                ((x - t.instantaneous_damage_display_amount).max(0.0), t.vignette_health_stiffness_damaged)
            } else {
                (x, t.vignette_health_stiffness)
            }
        };
        let c = t.health_vignette.eval(target);
        let (v, vel) = spring(self.vignette_health, self.vignette_health_vel, c, omega(stiffness), dt);
        self.vignette_health = v.clamp(0.0, 1.0);
        self.vignette_health_vel = vel;
    }
}

impl SurvivalState {
    pub fn ice_axe_wear(
        &mut self,
        t: &SurvivalTuning,
        r: &ResolvedCharacterProperties,
        f: RunFlags,
        chop: f32,
        dt: f32,
    ) {
        let thr = t.ice_axe_damage_threshold;
        if !f.running() || chop <= thr {
            return;
        }
        let rate = t.base_ice_axe_damage_rate / r.ice_axe_durability_modifier;
        let wear = (chop - thr) / (1.0 - thr) * rate * (self.lap_count + 1) as f32;
        self.ice_axe_effectiveness = (self.ice_axe_effectiveness - wear * dt).max(0.0);
    }

    pub fn death_vignette_scale(t: &SurvivalTuning, time_since_death: f32) -> f32 {
        if time_since_death > 0.0 {
            t.death_vignette.eval((time_since_death * 0.008_333_334).clamp(0.0, 1.0))
        } else {
            1.0
        }
    }

    pub fn update_temperature(
        &mut self,
        t: &SurvivalTuning,
        r: &ResolvedCharacterProperties,
        d: &DropSurvival,
        f: RunFlags,
        time_since_death: f32,
        dt: f32,
    ) {
        if f.running() {
            if self.shade < 0.5 {
                let loss = (self.lap_count + 1) as f32
                    * d.per_level.temperature_damage
                    * dt
                    * t.base_temperature_loss_rate
                    * r.temperature_loss_modifier;
                self.total_temperature_loss += loss;
                self.temperature -= loss;
            } else if self.temperature > 0.0 {
                self.temperature +=
                    d.per_level.temperature_regen * dt * t.base_temperature_gain_rate * r.temperature_gain_modifier;
            }
        }
        if f.nis || f.finished {
            self.vignette_temperature = (self.vignette_temperature + dt * 0.01).min(1.0);
            return;
        }
        let p = if r.temperature_gain_modifier == 0.0 { 1.0 } else { t.permanent_temperature_loss_percent };
        self.temperature_max = 1.0 - self.total_temperature_loss * p;
        self.temperature = self.temperature.max(0.0).min(self.temperature_max);
        let scale = Self::death_vignette_scale(t, time_since_death);
        let vmin = t.vignette_min_permanent_temperature;
        let max = self.temperature_max;
        let denom = if max < vmin { (max + vmin) * 0.5 } else { max };
        let x = if denom > 0.0 { (self.temperature / denom).clamp(0.0, 1.0) } else { 0.0 };
        let c = t.temperature_vignette.eval(x) * scale;
        self.vignette_temperature = approach(self.vignette_temperature, c, dt * 0.01);
    }

    #[allow(clippy::too_many_arguments)]
    pub fn update_oxygen(
        &mut self,
        t: &SurvivalTuning,
        r: &ResolvedCharacterProperties,
        d: &DropSurvival,
        f: RunFlags,
        player_speed: f32,
        classic: bool,
        time_since_death: f32,
        dt: f32,
    ) {
        if f.running() {
            let stiffness = if player_speed >= self.oxygen_flow {
                t.oxygen_flow_better_stiffness
            } else {
                t.oxygen_flow_worse_stiffness
            };
            let (x, v) = spring(self.oxygen_flow, self.oxygen_flow_vel, player_speed, omega(stiffness), dt);
            (self.oxygen_flow, self.oxygen_flow_vel) = (x.max(0.0), v);
            self.oxygen_use_requested = self.oxygen_pad.requested(classic);
            let usage = r.oxygen_tank_usage_modifier;
            if self.oxygen_use_requested && self.oxygen_tank > 0.0 && usage > 0.0 {
                let used = t.base_oxygen_tank_usage_rate * dt * usage;
                let mut frac = 1.0;
                if used > self.oxygen_tank {
                    frac = self.oxygen_tank / used;
                    self.oxygen_tank = 0.0;
                } else {
                    self.oxygen_tank -= used;
                }
                self.oxygen += t.base_oxygen_gain_from_tank_rate * dt * r.gain_from_tank_rate_modifier * frac;
            } else {
                let loss_thr = t.base_oxygen_loss_threshold * r.oxygen_loss_threshold_modifier;
                let gain_thr = t.base_oxygen_gain_threshold * r.oxygen_gain_threshold_modifier;
                let low = self.low_oxygen;
                let losing = low && self.oxygen_flow < loss_thr;
                let steady = self.oxygen > 0.0 && (self.oxygen_flow > gain_thr || !low);
                let rate = if losing {
                    (loss_thr - self.oxygen_flow) / loss_thr * t.base_oxygen_loss_rate
                } else if steady {
                    0.0
                } else {
                    t.base_oxygen_gain_rate * 0.1
                };
                let loss = (self.lap_count + 1) as f32 * d.per_level.oxygen_damage * dt * rate;
                self.total_oxygen_loss += loss;
                self.oxygen -= loss;
            }
        }
        if f.nis || f.finished {
            self.vignette_oxygen = (self.vignette_oxygen + dt * 0.01).min(1.0);
            return;
        }
        let max = 1.0 - self.total_oxygen_loss * t.permanent_oxygen_loss_percent;
        self.oxygen = self.oxygen.min(max);
        let scale = Self::death_vignette_scale(t, time_since_death);
        let x = if max > 0.0 { (self.oxygen / max).clamp(0.0, 1.0) } else { 0.0 };
        let c = t.oxygen_vignette.eval(x) * scale;
        self.vignette_oxygen = approach(self.vignette_oxygen, c, dt * 0.01);
    }

    pub fn update_batteries(&mut self, t: &SurvivalTuning, r: &ResolvedCharacterProperties, f: RunFlags, dt: f32) {
        if !f.running() {
            return;
        }
        let laps = (self.lap_count + 1) as f32;
        if self.head_lamp_battery > 0.0 {
            let k = 1.0 / (t.base_head_lamp_battery_life * r.head_lamp_battery_modifier) * laps;
            self.head_lamp_battery = (self.head_lamp_battery - k * dt).max(0.0);
        }
        if self.pulse_goggles_battery > 0.0 {
            let k = 1.0 / (t.base_pulse_goggles_battery_life * r.pulse_goggles_battery_modifier) * laps;
            self.pulse_goggles_battery = (self.pulse_goggles_battery - k * dt).max(0.0);
        }
    }
}

impl SurvivalState {
    pub fn trigger_reset(&mut self, t: &SurvivalTuning, r: &ResolvedCharacterProperties) {
        let d = SurvivalState::default();
        let keep = (self.armour, self.oxygen_tank, self.total_oxygen_loss, self.low_oxygen);
        let temperature_enabled = self.temperature_enabled;
        *self = d;
        (self.armour, self.oxygen_tank, self.total_oxygen_loss, self.low_oxygen) = keep;
        self.temperature_enabled = temperature_enabled;
        self.oxygen_flow = -1.0;
        self.init_outputs(t, r);
    }

    pub fn hud(&self) -> Hud {
        Hud {
            armour: self.armour,
            head_lamp_battery: self.head_lamp_battery,
            pulse_goggles_battery: self.pulse_goggles_battery,
            health: self.health,
            health_max: self.health_max,
            temperature: self.temperature,
            temperature_max: self.temperature_max,
            ice_axe_effectiveness: self.ice_axe_effectiveness,
            oxygen_tank: self.oxygen_tank,
            oxygen_use_requested: self.oxygen_use_requested,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Hud {
    pub armour: f32,
    pub head_lamp_battery: f32,
    pub pulse_goggles_battery: f32,
    pub health: f32,
    pub health_max: f32,
    pub temperature: f32,
    pub temperature_max: f32,
    pub ice_axe_effectiveness: f32,
    pub oxygen_tank: f32,
    pub oxygen_use_requested: bool,
}

pub fn health_from_points(t: &SurvivalTuning, gained: &mut f32, points: f32, awesome: bool, dt: f32) {
    if points <= 0.0 {
        return;
    }
    let mut h = t.points_to_health.eval(points);
    if awesome {
        h *= t.points_to_health_awesome_multiplier;
    }
    *gained += h * dt;
}

pub const SHADE_RAY_LENGTH: f32 = 2000.0;

// TODO(guess): headless only — the game's value is the GPU shadow query's read-back (0x82FBB620, 0x82FBB7B0, 0x829A3980 → 0x82AC5C78; shadowtest.fxo: one tap of the cascades at the rider's trajectory point into a 16×16 texture, read back as byte / 255 at least a frame late; the riding rider casts into no cascade);
// one ray against the collision stands in; settle: a CPU evaluation of the cascades (needs a rasteriser) — the ray stays for tools.
pub fn shade_value(c: &crate::collision::CollisionWorld, p: Vec3, sun: Vec3) -> f32 {
    crate::guess_hit!();
    if c.ray_cast(p + sun * 0.5, sun, SHADE_RAY_LENGTH).is_some() { 0.0 } else { 1.0 }
}
