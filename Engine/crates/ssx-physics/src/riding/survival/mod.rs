use anyhow::Result;
use glam::Vec3;

use crate::character::ResolvedCharacterProperties;
use crate::curve::Curve;
use crate::tuning::Bundle;

mod update;

pub use update::*;

#[derive(Clone, Debug, Default)]
pub struct SurvivalTuning {
    pub vignette_health_stiffness: f32,
    pub vignette_health_stiffness_damaged: f32,
    pub vignette_min_permanent_health: f32,
    pub instantaneous_damage_display_time: f32,
    pub instantaneous_damage_display_amount: f32,
    pub health_recovery_over_time_regular: f32,
    pub health_recovery_over_time_survival: f32,
    pub health_recovery_delay_regular: f32,
    pub health_recovery_delay_survival: f32,
    pub permanent_health_loss_percent: f32,
    pub impact_speed_gain: f32,
    pub impact_damage_threshold_low: f32,
    pub impact_damage_threshold_high: f32,
    pub impact_damage_step: f32,
    pub impact_damage_amount: f32,
    pub uber_bail_damage_amount: f32,
    pub min_ticks_between_impact_damage: f32,
    pub base_armour_damage_modifier: f32,
    pub ice_axe_damage_threshold: f32,
    pub base_ice_axe_damage_rate: f32,
    pub base_temperature_loss_rate: f32,
    pub base_temperature_gain_rate: f32,
    pub permanent_temperature_loss_percent: f32,
    pub vignette_min_permanent_temperature: f32,
    pub base_oxygen_loss_threshold: f32,
    pub base_oxygen_loss_rate: f32,
    pub base_oxygen_gain_threshold: f32,
    pub base_oxygen_gain_rate: f32,
    pub permanent_oxygen_loss_percent: f32,
    pub oxygen_flow_worse_stiffness: f32,
    pub oxygen_flow_better_stiffness: f32,
    pub base_oxygen_tank_usage_rate: f32,
    pub base_oxygen_gain_from_tank_rate: f32,
    pub base_head_lamp_battery_life: f32,
    pub base_pulse_goggles_battery_life: f32,
    pub per_lap_health_bonus: f32,
    pub per_lap_temperature_bonus: f32,
    pub per_lap_oxygen_bonus: f32,
    pub per_lap_oxygen_tank_bonus: f32,
    pub health_vignette: Curve,
    pub death_vignette: Curve,
    pub temperature_vignette: Curve,
    pub oxygen_vignette: Curve,
    pub points_to_health: Curve,
    pub points_to_health_awesome_multiplier: f32,
    pub riding_on_rock_curve: Curve,
    pub riding_on_rock_threshold: f32,
    pub uber_crash_landed_recently_threshold: f32,
    pub min_air_time_to_bail_on_rock_landing: f32,
}

impl SurvivalTuning {
    pub fn load(bundle: &Bundle) -> Result<Self> {
        let p = bundle.view("PlayerUpdateSceneOp", "PlayerUpdateSceneOpAsset")?;
        let s = bundle.view("ScoringSceneOp", "ScoringSceneOpAsset")?;
        let c = bundle.view("carve_riding", "CarvingControllerAsset")?;
        Ok(Self {
            vignette_health_stiffness: p.f32("VignetteHealthStiffness")?,
            vignette_health_stiffness_damaged: p.f32("VignetteHealthStiffnessDamaged")?,
            vignette_min_permanent_health: p.f32("VignetteMinPermanentHealth")?,
            instantaneous_damage_display_time: p.f32("InstantaneousDamageDisplayTime")?,
            instantaneous_damage_display_amount: p.f32("InstantaneousDamageDisplayAmount")?,
            health_recovery_over_time_regular: p.f32("HealthRecoveryOverTimeRegular")?,
            health_recovery_over_time_survival: p.f32("HealthRecoveryOverTimeSurvival")?,
            health_recovery_delay_regular: p.f32("HealthRecoveryDelayRegular")?,
            health_recovery_delay_survival: p.f32("HealthRecoveryDelaySurvival")?,
            permanent_health_loss_percent: p.f32("PermanentHealthLossPercent")?,
            impact_speed_gain: p.f32("ImpactSpeedGain")?,
            impact_damage_threshold_low: p.f32("ImpactDamageThresholdLow")?,
            impact_damage_threshold_high: p.f32("ImpactDamageThresholdHigh")?,
            impact_damage_step: p.f32("ImpactDamageStep")?,
            impact_damage_amount: p.f32("ImpactDamageAmount")?,
            uber_bail_damage_amount: p.f32("UberBailDamageAmount")?,
            min_ticks_between_impact_damage: p.f32("MinTicksBetweenImpactDamage")?,
            base_armour_damage_modifier: p.f32("BaseArmourDamageModifier")?,
            ice_axe_damage_threshold: p.f32("IceAxeDamageThreshold")?,
            base_ice_axe_damage_rate: p.f32("BaseIceAxeDamageRate")?,
            base_temperature_loss_rate: p.f32("BaseTemperatureLossRate")?,
            base_temperature_gain_rate: p.f32("BaseTemperatureGainRate")?,
            permanent_temperature_loss_percent: p.f32("PermanentTemperatureLossPercent")?,
            vignette_min_permanent_temperature: p.f32("VignetteMinPermanentTemperature")?,
            base_oxygen_loss_threshold: p.f32("BaseOxygenLossThreshold")?,
            base_oxygen_loss_rate: p.f32("BaseOxygenLossRate")?,
            base_oxygen_gain_threshold: p.f32("BaseOxygenGainThreshold")?,
            base_oxygen_gain_rate: p.f32("BaseOxygenGainRate")?,
            permanent_oxygen_loss_percent: p.f32("PermanentOxygenLossPercent")?,
            oxygen_flow_worse_stiffness: p.f32("OxygenFlowWorseStiffness")?,
            oxygen_flow_better_stiffness: p.f32("OxygenFlowBetterStiffness")?,
            base_oxygen_tank_usage_rate: p.f32("BaseOxygenTankUsageRate")?,
            base_oxygen_gain_from_tank_rate: p.f32("BaseOxygenGainFromTankRate")?,
            base_head_lamp_battery_life: p.f32("BaseHeadLampBatteryLife")?,
            base_pulse_goggles_battery_life: p.f32("BasePulseGogglesBatteryLife")?,
            per_lap_health_bonus: p.f32("PerLapHealthBonus")?,
            per_lap_temperature_bonus: p.f32("PerLapTemperatureBonus")?,
            per_lap_oxygen_bonus: p.f32("PerLapOxygenBonus")?,
            per_lap_oxygen_tank_bonus: p.f32("PerLapOxygenTankBonus")?,
            health_vignette: p.curve("HealthVignetteCurveAsset")?,
            death_vignette: p.curve("DeathVignetteCurveAsset")?,
            temperature_vignette: p.curve("TemperatureVignetteCurveAsset")?,
            oxygen_vignette: p.curve("OxygenVignetteCurveAsset")?,
            points_to_health: s.curve("PointsToHealthCurve")?,
            points_to_health_awesome_multiplier: s.f32("PointsToHealthAwesomeMultiplier")?,
            riding_on_rock_curve: c.curve("RidingOnRockImpactOverTimeCurve")?,
            riding_on_rock_threshold: c.f32("RidingOnRockSurfacePropertyThreshold")?,
            uber_crash_landed_recently_threshold: c.f32("UberCrashLandedRecentlyThreshold")?,
            min_air_time_to_bail_on_rock_landing: c.f32("MinAirTimeToBailOnRockLanding")?,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PerLevelMods {
    pub impact_damage: f32,
    pub impact_regen: f32,
    pub temperature_damage: f32,
    pub temperature_regen: f32,
    pub oxygen_damage: f32,
    pub oxygen_regen: f32,
}

impl Default for PerLevelMods {
    fn default() -> Self {
        Self {
            impact_damage: 1.0,
            impact_regen: 1.0,
            temperature_damage: 1.0,
            temperature_regen: 1.0,
            oxygen_damage: 1.0,
            oxygen_regen: 1.0,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct DropSurvival {
    pub cold: bool,
    pub low_oxygen: bool,
    pub rock_damage: bool,
    pub tripping: bool,
    pub avalanche: crate::avalanche::DropAvalanche,
    pub death_permanent: bool,
    pub required: Vec<u32>,
    pub sun: Option<Vec3>,
    pub night: bool,
    pub slots: [bool; 15],
    pub gear_null: bool,
    pub stats: [f32; 37],
    pub head_lamp_scale: f32,
    pub whiteout_lighting: bool,
    pub goggles_preset: Option<(u32, ssx_assets::postfx_state::PostFxState)>,
    pub per_level: PerLevelMods,
    pub loadout: Option<crate::character::GearStats>,
    pub objectives: Option<std::sync::Arc<crate::objectives::DropScripts>>,
    pub camera_nis_wind: Option<f32>,
}

impl Default for DropSurvival {
    fn default() -> Self {
        Self {
            cold: false,
            low_oxygen: false,
            rock_damage: false,
            tripping: false,
            avalanche: crate::avalanche::DropAvalanche::default(),
            death_permanent: false,
            required: Vec::new(),
            sun: None,
            night: false,
            slots: crate::character::GearStats::default_gear().slots,
            stats: crate::character::GearStats::default_gear().stats,
            gear_null: false,
            head_lamp_scale: 0.0,
            whiteout_lighting: false,
            goggles_preset: None,
            per_level: PerLevelMods::default(),
            loadout: None,
            objectives: None,
            camera_nis_wind: None,
        }
    }
}

impl DropSurvival {
    pub fn item_enabled(&self, slot: usize) -> bool {
        self.gear_null || self.slots.get(slot).copied().unwrap_or(false)
    }

    pub fn apply_event(&mut self, event: Option<&crate::event::EventDef>) {
        self.death_permanent = event.is_some_and(|e| e.death_permanent());
        self.required = event.map_or_else(Vec::new, |e| e.required_equipment.clone());
        self.loadout = event.and_then(|e| e.loadout.clone());
    }
}

pub fn required_gear_slot(value: u32) -> Option<usize> {
    [1, 2, 5, 4, 3, 6, 7, 0].get(value as usize).copied()
}

pub const ITEM_STATS: [&[usize]; 8] =
    [&[0, 1, 2], &[5, 6, 7], &[8, 9], &[11, 12], &[14, 15], &[17, 18], &[20, 21, 22], &[23, 24, 25]];

pub fn event_loadout(d: &DropSurvival) -> crate::character::GearStats {
    d.loadout.clone().unwrap_or_else(|| loadout(&d.required, TEMPLATE_VALUE))
}

pub const TEMPLATE_VALUE: f32 = 100.0;

// TODO(guess): without a front end (`ride_sim`, `--ride`) an Explore drop's rider wears the default gear plus the templates at `value` (the game shell equips the launch's setup, `crate::gear::GameSetup`); settle: none — the profile is the player's state.
pub fn loadout(required: &[u32], value: f32) -> crate::character::GearStats {
    crate::guess_hit!();
    let mut g = crate::character::GearStats::default_gear();
    for slot in required.iter().filter_map(|&v| required_gear_slot(v)).filter(|&s| s != 0) {
        g.slots[slot] = true;
        for &i in ITEM_STATS[slot] {
            g.stats[i] = (g.stats[i] + value).min(255.0);
        }
    }
    g
}

pub fn spring(x: f32, v: f32, target: f32, omega: f32, dt: f32) -> (f32, f32) {
    let e = (-(omega * dt)).exp();
    let x0 = x - target;
    let k = v + x0 * omega;
    ((x0 + k * dt) * e + target, (v - k * dt * omega) * e)
}

fn omega(stiffness: f32) -> f32 {
    if stiffness == 0.0 { 0.0 } else { stiffness.sqrt() }
}

fn approach(v: f32, target: f32, rate: f32) -> f32 {
    if v > target { (v - rate).max(target) } else { (v + rate).min(target) }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SurvivalState {
    pub health: f32,
    pub health_max: f32,
    pub at_min_health: bool,
    pub total_damage_taken: f32,
    pub ticks_since_damaged: f32,
    pub ticks_since_impact_damage: f32,
    pub vignette_health: f32,
    pub vignette_health_vel: f32,
    pub carving_damage: f32,
    pub one_off_damage: f32,
    pub health_gained_from_tricks: f32,
    pub trigger_impact_bail: bool,
    pub trigger_death_bail: bool,
    pub armour: f32,
    pub temperature: f32,
    pub temperature_max: f32,
    pub total_temperature_loss: f32,
    pub vignette_temperature: f32,
    pub shade: f32,
    pub shade_query: Option<f32>,
    pub oxygen_flow: f32,
    pub oxygen_flow_vel: f32,
    pub oxygen: f32,
    pub total_oxygen_loss: f32,
    pub vignette_oxygen: f32,
    pub oxygen_tank: f32,
    pub oxygen_use_requested: bool,
    pub low_oxygen: bool,
    pub oxygen_pad: OxygenPad,
    pub temperature_enabled: bool,
    pub head_lamp_battery: f32,
    pub pulse_goggles_battery: f32,
    pub ice_axe_effectiveness: f32,
    pub lap_count: i32,
}

impl Default for SurvivalState {
    fn default() -> Self {
        Self {
            health: 1.0,
            health_max: 1.0,
            at_min_health: false,
            total_damage_taken: 0.0,
            ticks_since_damaged: 1000.0,
            ticks_since_impact_damage: 0.0,
            vignette_health: 1.0,
            vignette_health_vel: 0.0,
            carving_damage: 0.0,
            one_off_damage: 0.0,
            health_gained_from_tricks: 0.0,
            trigger_impact_bail: false,
            trigger_death_bail: false,
            armour: -1.0,
            temperature: 1.0,
            temperature_max: 1.0,
            total_temperature_loss: 0.0,
            vignette_temperature: 1.0,
            shade: 1.0,
            shade_query: None,
            oxygen_flow: -1.0,
            oxygen_flow_vel: 0.0,
            oxygen: 1.0,
            total_oxygen_loss: 0.0,
            vignette_oxygen: 1.0,
            oxygen_tank: 1.0,
            oxygen_use_requested: false,
            low_oxygen: false,
            oxygen_pad: OxygenPad::default(),
            temperature_enabled: false,
            head_lamp_battery: 1.0,
            pulse_goggles_battery: 1.0,
            ice_axe_effectiveness: 1.0,
            lap_count: 0,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct OxygenPad {
    pub use_oxygen: bool,
    pub use_oxygen_classic: bool,
}

impl OxygenPad {
    pub fn requested(&self, classic: bool) -> bool {
        if classic { self.use_oxygen_classic } else { self.use_oxygen }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct RunFlags {
    pub started: bool,
    pub finished: bool,
    pub nis: bool,
}

impl RunFlags {
    pub fn running(&self) -> bool {
        self.started && !self.finished && !self.nis
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ImpactInputs {
    pub this_frame: bool,
    pub speed: f32,
    pub pre_velocity: Vec3,
    pub uber_bail: bool,
}

pub fn impact_damage(t: &SurvivalTuning, speed: f32, pre_velocity: Vec3) -> f32 {
    let v = pre_velocity.length();
    let s = (speed - v) * t.impact_speed_gain + v;
    let lo = t.impact_damage_threshold_low;
    if s <= lo {
        return 0.0;
    }
    let x = (s - lo) / (t.impact_damage_threshold_high - lo) * t.impact_damage_amount / t.impact_damage_step + 0.5;
    (x.floor() * t.impact_damage_step).min(t.impact_damage_amount)
}

#[cfg(test)]
mod tests;
