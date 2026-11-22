use super::*;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ParticleTraits {
    pub gravity: f32,
    pub h_terminal: f32,
    pub v_terminal: f32,
    pub restitution: f32,
    pub stop_friction: f32,
    pub roll_friction: f32,
    pub energy_preservation: f32,
    pub stopping_speed: f32,
    pub surface_noise: f32,
    pub roll_factor: f32,
    pub max_lifetime: f32,
    pub max_lifetime_frozen: f32,
    pub aging_ground: f32,
    pub aging_air: f32,
    pub spawn_offset_depth: f32,
    pub spawn_offset_duration: f32,
}

impl ParticleTraits {
    pub fn load(a: &AssetView<'_>) -> Result<Self> {
        Ok(Self {
            gravity: a.f32("Gravity")?,
            h_terminal: a.f32("HorizontalTerminalVelocity")?,
            v_terminal: a.f32("VerticalTerminalVelocity")?,
            restitution: a.f32("CoeffRestitution")?,
            stop_friction: a.f32("CoeffStopFriction")?,
            roll_friction: a.f32("CoeffRollFriction")?,
            energy_preservation: a.f32("CoeffEnergyPreservation")?,
            stopping_speed: a.f32("StoppingSpeed")?,
            surface_noise: a.f32("SurfaceNoise")?,
            roll_factor: a.f32("RollFactor")?,
            max_lifetime: a.f32("MaxLifetime")?,
            max_lifetime_frozen: a.f32("MaxLifetimeFrozen")?,
            aging_ground: a.f32("AgingMultiplierGround")?,
            aging_air: a.f32("AgingMultiplierAir")?,
            spawn_offset_depth: a.f32("SpawnOffsetDepth")?,
            spawn_offset_duration: a.f32("SpawnOffsetDuration")?,
        })
    }

    pub fn drag(&self) -> (f32, f32, f32) {
        let g = self.gravity / 3600.0;
        let (vh, vv) = (self.h_terminal / 216.0, self.v_terminal / 216.0);
        (g / (vh * vh), g / (vv * vv), g.abs())
    }
}

#[derive(Clone, Debug, Default)]
pub struct ParticleTuning {
    pub name: String,
    pub spray: ParticleTraits,
    pub slough: ParticleTraits,
    pub slab: ParticleTraits,
    pub spray_vector_min: f32,
    pub spray_vector_max: f32,
    pub spray_velocity_min: f32,
    pub spray_velocity_max: f32,
    pub spray_velocity_exponent: f32,
    pub spray_density: f32,
    pub spray_width: f32,
    pub spray_height: f32,
    pub spray_scale_min: f32,
    pub spray_scale_max: f32,
    pub ice_spray_scale: f32,
    pub ramp_up_min: f32,
    pub ramp_up_max: f32,
    pub carving_vector_min: f32,
    pub carving_vector_max: f32,
    pub carving_density_min: f32,
    pub carving_density_max: f32,
    pub carving_density_exponent: f32,
    pub burst_size: i32,
    pub carving_throw: f32,
    pub carving_width: f32,
    pub carving_height: f32,
    pub slab_scale_min: f32,
    pub slab_scale_max: f32,
    pub impact_trigger_force_min: f32,
    pub impact_trigger_force_max: f32,
    pub impact_density_min: f32,
    pub impact_density_max: f32,
    pub impact_density_exponent: f32,
    pub impact_throw: f32,
    pub impact_lift: f32,
    pub impact_spread: f32,
    pub impact_size_ahead: f32,
    pub impact_size_behind: f32,
    pub impact_size_left_right: f32,
    pub slab_half_life: f32,
    pub slab_max_scale_post_impact: f32,
    pub slab_slough_conversion_rate: f32,
    pub slough_spread_ratio: f32,
    pub slough_scale_min: f32,
    pub slough_scale_max: f32,
    pub splosion_density: f32,
    pub splosion_slab_impulse: Curve,
    pub splosion_slough_impulse: Curve,
    pub spray_vector_filter_lag: f32,
    pub mass_multiplier: f32,
}

impl ParticleTuning {
    pub fn load(a: &AssetView<'_>) -> Result<Self> {
        Ok(Self {
            name: a.name().to_string(),
            spray: ParticleTraits::load(&a.asset("ParticleTraitsSpray")?)?,
            slough: ParticleTraits::load(&a.asset("ParticleTraitsSlough")?)?,
            slab: ParticleTraits::load(&a.asset("ParticleTraitsSlab")?)?,
            spray_vector_min: a.f32("SprayVectorMin")?,
            spray_vector_max: a.f32("SprayVectorMax")?,
            spray_velocity_min: a.f32("SprayVelocityMin")?,
            spray_velocity_max: a.f32("SprayVelocityMax")?,
            spray_velocity_exponent: a.f32("SprayVelocityExponent")?,
            spray_density: a.f32("SprayDensity")?,
            spray_width: a.f32("SprayWidth")?,
            spray_height: a.f32("SprayHeight")?,
            spray_scale_min: a.f32("SprayScaleMin")?,
            spray_scale_max: a.f32("SprayScaleMax")?,
            ice_spray_scale: a.f32("IceSprayScale")?,
            ramp_up_min: a.f32("SlabRampUpDistanceMin")?,
            ramp_up_max: a.f32("SlabRampUpDistanceMax")?,
            carving_vector_min: a.f32("SlabCarvingVectorMin")?,
            carving_vector_max: a.f32("SlabCarvingVectorMax")?,
            carving_density_min: a.f32("SlabCarvingDensityMin")?,
            carving_density_max: a.f32("SlabCarvingDensityMax")?,
            carving_density_exponent: a.f32("SlabCarvingDensityExponent")?,
            burst_size: a.f32("SlabBurstSize")? as i32,
            carving_throw: a.f32("SlabCarvingThrow")?,
            carving_width: a.f32("SlabCarvingWidth")?,
            carving_height: a.f32("SlabCarvingHeight")?,
            slab_scale_min: a.f32("SlabScaleMin")?,
            slab_scale_max: a.f32("SlabScaleMax")?,
            impact_trigger_force_min: a.f32("SlabImpactTriggerForceMin")?,
            impact_trigger_force_max: a.f32("SlabImpactTriggerForceMax")?,
            impact_density_min: a.f32("SlabImpactDensityMin")?,
            impact_density_max: a.f32("SlabImpactDensityMax")?,
            impact_density_exponent: a.f32("SlabImpactDensityExponent")?,
            impact_throw: a.f32("SlabImpactThrow")?,
            impact_lift: a.f32("SlabImpactLift")?,
            impact_spread: a.f32("SlabImpactSpread")?,
            impact_size_ahead: a.f32("SlabImpactSizeAhead")?,
            impact_size_behind: a.f32("SlabImpactSizeBehind")?,
            impact_size_left_right: a.f32("SlabImpactSizeLeftRight")?,
            slab_half_life: a.f32("SlabHalfLife")?,
            slab_max_scale_post_impact: a.f32("SlabMaxScalePostImpact")?,
            slab_slough_conversion_rate: a.f32("SlabSloughConversionRate")?,
            slough_spread_ratio: a.f32("SloughSpreadRatio")?,
            slough_scale_min: a.f32("SloughScaleMin")?,
            slough_scale_max: a.f32("SloughScaleMax")?,
            splosion_density: a.f32("SplosionDensity")?,
            splosion_slab_impulse: a.curve("SplosionSlabImpulseCurve")?,
            splosion_slough_impulse: a.curve("SplosionSloughImpulseCurve")?,
            spray_vector_filter_lag: a.f32("SprayVectorFilterLag")?,
            mass_multiplier: a.f32("MassMultiplier")?,
        })
    }
}

#[derive(Clone, Debug, Default)]
pub struct CarvingAvalancheTuning {
    pub friction_modifier: Curve,
    pub friction_ramp_up: Curve,
    pub drag_modifier: Curve,
    pub wind_modifier: Curve,
    pub tumble_enter: f32,
    pub tumble_exit: f32,
    pub effect_ramp_up: Curve,
    pub tumble_start_ramp_up_threshold: f32,
    pub min_air_time_for_ramp_up: f32,
    pub damage: Curve,
    pub damage_multiplier: f32,
    pub wind_decay: Curve,
    pub tumble_exit_decay: Curve,
    pub delay_reentry: f32,
    pub reentry_multiplier: Curve,
    pub tumble_damage_multiplier: f32,
    pub rumble: [Curve; 2],
}

impl CarvingAvalancheTuning {
    pub fn load(a: &AssetView<'_>, tumble_damage_multiplier: f32) -> Result<Self> {
        Ok(Self {
            friction_modifier: a.curve("AvalancheFrictionModifierCurve")?,
            friction_ramp_up: a.curve("AvalancheFrictionRampUpCurve")?,
            drag_modifier: a.curve("AvalancheDragModifierCurve")?,
            wind_modifier: a.curve("AvalancheWindModifierCurve")?,
            tumble_enter: a.f32("AvalancheTumbleEnterThreshold")?,
            tumble_exit: a.f32("AvalancheTumbleExitThreshold")?,
            effect_ramp_up: a.curve("AvalancheEffectRampUpCurve")?,
            tumble_start_ramp_up_threshold: a.f32("AvalancheTumbleStartRampUpThreshold")?,
            min_air_time_for_ramp_up: a.f32("AvalancheMinAirTimeForRampUp")?,
            damage: a.curve("AvalancheDamageCurve")?,
            damage_multiplier: a.f32("AvalancheDamageMultiplier")?,
            wind_decay: a.curve("AvalancheWindDecayCurve")?,
            tumble_exit_decay: a.curve("AvalancheTumbleExitThresholdDecayCurve")?,
            delay_reentry: a.f32("AvalancheDelayReEntryTimer")?,
            reentry_multiplier: a.curve("AvalancheReEntryValueThresholdMultiplierCurve")?,
            tumble_damage_multiplier,
            rumble: [a.curve("AvalancheRumbleMotor0Curve")?, a.curve("AvalancheRumbleMotor1Curve")?],
        })
    }
}

#[derive(Clone, Debug, Default)]
pub struct AvalancheTuning {
    pub large: ParticleTuning,
    pub medium: ParticleTuning,
    pub small: ParticleTuning,
    pub surface_epsilon: f32,
    pub culling_distance: f32,
    pub board_half_length: f32,
    pub aggregate_falloff: Curve,
    pub aggregate_lifetime: Curve,
    pub aggregate_gain: f32,
    pub probe_falloff: Curve,
    pub probe_lifetime: Curve,
    pub probe_gain: f32,
    pub carving_sets: [CarvingAvalancheTuning; 3],
}

pub const DEFAULT_TUNING_SET: i32 = 3;

impl AvalancheTuning {
    pub fn load(bundle: &Bundle) -> Result<Self> {
        let p = bundle.view("particle_system", "ParticleSystemAsset")?;
        let c = bundle.view("carve_riding", "CarvingControllerAsset")?;
        let tdm = c.f32("TumbleDamageMultiplier")?;
        let set = |n: &str| CarvingAvalancheTuning::load(&c.asset(n)?, tdm);
        Ok(Self {
            large: ParticleTuning::load(&p.asset("ParticleTuningLarge")?)?,
            medium: ParticleTuning::load(&p.asset("ParticleTuningMedium")?)?,
            small: ParticleTuning::load(&p.asset("ParticleTuningSmall")?)?,
            surface_epsilon: p.f32("SurfaceEpsilon")?,
            culling_distance: p.f32("CullingDistance")?,
            board_half_length: p.f32("BoardHalfLength")?,
            aggregate_falloff: p.curve("AggregateFalloffCurve")?,
            aggregate_lifetime: p.curve("AggregateLifetimeCurve")?,
            aggregate_gain: p.f32("AggregateFilterGain")?,
            probe_falloff: p.curve("ProbeFalloffCurve")?,
            probe_lifetime: p.curve("ProbeLifetimeCurve")?,
            probe_gain: p.f32("ProbeFilterGain")?,
            carving_sets: [
                set("AvalancheCarvingPropertiesReverse")?,
                set("AvalancheCarvingPropertiesLarge")?,
                set("AvalancheCarvingPropertiesSmall")?,
            ],
        })
    }

    pub fn particles(&self, set: i32) -> &ParticleTuning {
        match set {
            2 => &self.medium,
            3 => &self.small,
            _ => &self.large,
        }
    }

    pub fn carving(&self, set: i32) -> &CarvingAvalancheTuning {
        match set {
            1 => &self.carving_sets[1],
            2 => &self.carving_sets[2],
            _ => &self.carving_sets[0],
        }
    }
}
