use super::*;

#[derive(Clone, Debug, Default)]
pub struct CarvingTuning {
    pub friction_force_multiplier: f32,
    pub normal_force_multiplier: f32,
    pub vertical_surface_angle_threshold: f32,
    pub collision_radius_base: f32,
    pub collision_radius_upright_offset: f32,
    pub height_to_allow_full_unstuck_assist: f32,
    pub height_to_allow_partial_unstuck_assist: f32,
    pub height_to_allow_full_boost: f32,
    pub height_to_allow_partial_boost: f32,
    pub height_considered_uphill: f32,
    pub on_ground_distance_threshold: f32,
    pub max_lean_vel: f32,
    pub max_lean_accel: f32,
    pub max_lean_angle: f32,
    pub max_lean_input: f32,
    pub equilibrium_lean_angle: f32,
    pub max_spin_vel: f32,
    pub max_spin_accel: f32,
    pub max_spin_angle: f32,
    pub max_spin_input: f32,
    pub max_pitch_vel: f32,
    pub max_pitch_accel: f32,
    pub max_pitch_angle: f32,
    pub lean_pid: PidGains,
    pub spin_pid: PidGains,
    pub pitch_pid: PidGains,
    pub early_slip_input_limit: f32,
    pub early_slip_max_angle: f32,
    pub slip_edge_hold_threshold: f32,
    pub speed_threshold: f32,
    pub goofy_hysteresis_angle: f32,
    pub stopped_threshold: f32,
    pub riding_height_neutral: f32,
    pub stop_speed_threshold: f32,
    pub stop_accel_threshold: f32,
    pub min_stick_magnitude_for_go: f32,
    pub min_landing_impact_velocity: f32,
    pub slip_friction_input_curve: Curve,
    pub landing_velocity_decay_curve: Curve,
    pub lean_input_curve: Curve,
    pub max_lean_vs_speed_curve: Curve,
    pub lean_input_angle_transform: Curve,
    pub pitch_vs_normal_curve: Curve,
    pub balance_surface_adhesion_modifier_curve: Curve,
    pub bias: f32,
    pub bias_weight: f32,
    pub uphill_coefficient: f32,
    pub concavity_gain: f32,
    pub concavity_coefficient: f32,
    pub max_slip_angle_delta: f32,
    pub max_slip_dig_angle_delta: f32,
    pub riding_height_tucked: f32,
    pub riding_height_pre_jump: f32,
    pub riding_height_boost: f32,
    pub riding_height_min: f32,
    pub riding_height_max: f32,
    pub max_riding_height_delta: f32,
    pub riding_height_anim_blend_curve: Curve,
    pub landing_riding_height_spring_stiffness_curve: Curve,
    pub landing_absorption_threshold: f32,
    pub min_landing_impact_velocity_ticks: f32,
    pub pitch_balance_stable_threshold: f32,
    pub snow_compression_restore_curve: Curve,
    pub snow_compression_from_dig_curve: Curve,
    pub prewind_carve_input_dampening_curve: Curve,
    pub prewind_carve_input_classic_dampening_curve: Curve,
    pub prewind_dampens_carving: bool,
    pub board_press_friction_modifier: f32,
    pub collision_look_ahead: f32,
    pub collision_scan_forward_angle: f32,
    pub collision_scan_side_angle: f32,
    pub collision_steering_angular_vel: f32,
    pub scoring_vertical_surface_angle_threshold: f32,
    pub uber_crash_ticks_to_full_friction: f32,
    pub carving_spray_filter_responsiveness: f32,
    pub ice_axe: crate::ice_axe::IceAxeTuning,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PidGains {
    pub p: f32,
    pub i: f32,
    pub d: f32,
}

impl PidGains {
    pub fn load(a: &AssetView<'_>) -> Result<Self> {
        if a.class() != "PidAsset" {
            bail!("{:?} is a {}, expected a PidAsset", a.name(), a.class());
        }
        Ok(Self { p: a.f32("ProportionalGain")?, i: a.f32("IntegralGain")?, d: a.f32("DerivativeGain")? })
    }
}

impl CarvingTuning {
    pub fn load(a: &AssetView<'_>) -> Result<Self> {
        Ok(Self {
            friction_force_multiplier: a.f32("FrictionForceMultiplier")?,
            normal_force_multiplier: a.f32("NormalForceMultiplier")?,
            vertical_surface_angle_threshold: a.f32("VerticalSurfaceAngleThreshold")?,
            collision_radius_base: a.f32("CollisionRadiusBase")?,
            collision_radius_upright_offset: a.f32("CollisionRadiusUprightOffset")?,
            height_to_allow_full_unstuck_assist: a.f32("HeightToAllowFullUnstuckAssistAsset")?,
            height_to_allow_partial_unstuck_assist: a.f32("HeightToAllowPartialUnstuckAssistAsset")?,
            height_to_allow_full_boost: a.f32("HeightToAllowFullBoostAsset")?,
            height_to_allow_partial_boost: a.f32("HeightToAllowPartialBoostAsset")?,
            height_considered_uphill: a.f32("HeightConsideredUphill")?,
            on_ground_distance_threshold: a.f32("OnGroundDistanceThreshold")?,
            max_lean_vel: a.f32("MaxLeanVel")?,
            max_lean_accel: a.f32("MaxLeanAccel")?,
            max_lean_angle: a.f32("MaxLeanAngle")?,
            max_lean_input: a.f32("MaxLeanInput")?,
            equilibrium_lean_angle: a.f32("EquilibriumLeanAngle")?,
            max_spin_vel: a.f32("MaxSpinVel")?,
            max_spin_accel: a.f32("MaxSpinAccel")?,
            max_spin_angle: a.f32("MaxSpinAngle")?,
            max_spin_input: a.f32("MaxSpinInput")?,
            max_pitch_vel: a.f32("MaxPitchVel")?,
            max_pitch_accel: a.f32("MaxPitchAccel")?,
            max_pitch_angle: a.f32("MaxPitchAngle")?,
            lean_pid: PidGains::load(&a.asset("LeanPidAsset")?)?,
            spin_pid: PidGains::load(&a.asset("SpinPidAsset")?)?,
            pitch_pid: PidGains::load(&a.asset("PitchPidAsset")?)?,
            early_slip_input_limit: a.f32("EarlySlipInputLimit")?,
            early_slip_max_angle: a.f32("EarlySlipMaxAngle")?,
            slip_edge_hold_threshold: a.f32("SlipEdgeHoldThreshold")?,
            speed_threshold: a.f32("SpeedThreshold")?,
            goofy_hysteresis_angle: a.f32("GoofyHysteresisAngle")?,
            stopped_threshold: a.f32("StoppedThreshold")?,
            riding_height_neutral: a.f32("RidingHeightNeutral")?,
            stop_speed_threshold: a.f32("StopSpeedThreshold")?,
            stop_accel_threshold: a.f32("StopAccelThreshold")?,
            min_stick_magnitude_for_go: a.f32("MinStickMagnitudeForGo")?,
            min_landing_impact_velocity: a.f32("MinLandingImpactVelocity")?,
            slip_friction_input_curve: a.curve("SlipFrictionInputCurve")?,
            landing_velocity_decay_curve: a.curve("LandingVelocityDecayCurve")?,
            lean_input_curve: a.curve("LeanInputCurveAsset")?,
            max_lean_vs_speed_curve: a.curve("SpeedVsMaxLeanInputAngleCurveAsset")?,
            lean_input_angle_transform: a.curve("LeanInputAngleTransform")?,
            pitch_vs_normal_curve: a.curve("PitchVsNormalCurveAsset")?,
            balance_surface_adhesion_modifier_curve: a.curve("BalanceSurfaceAdhesionModifierCurve")?,
            bias: a.f32("Bias")?,
            bias_weight: a.f32("BiasWeight")?,
            uphill_coefficient: a.f32("UphillCoefficient")?,
            concavity_gain: a.f32("ConcavityGain")?,
            concavity_coefficient: a.f32("ConcavityCoefficient")?,
            max_slip_angle_delta: a.f32("MaxSlipAngleDelta")?,
            max_slip_dig_angle_delta: a.f32("MaxSlipDigAngleDelta")?,
            riding_height_tucked: a.f32("RidingHeightTucked")?,
            riding_height_pre_jump: a.f32("RidingHeightPreJump")?,
            riding_height_boost: a.f32("RidingHeightBoost")?,
            riding_height_min: a.f32("RidingHeightMin")?,
            riding_height_max: a.f32("RidingHeightMax")?,
            max_riding_height_delta: a.f32("MaxRidingHeightDelta")?,
            riding_height_anim_blend_curve: a.curve("RidingHeightAnimBlendCurve")?,
            landing_riding_height_spring_stiffness_curve: a.curve("LandingRidingHeightSpringStiffnessCurve")?,
            landing_absorption_threshold: a.f32("LandingGroundRelYVelocityAbsorptionThreshold")?,
            min_landing_impact_velocity_ticks: a.f32("MinLandingImpactVelocityTicks")?,
            pitch_balance_stable_threshold: a.f32("PitchBalanceStableThreshold")?,
            snow_compression_restore_curve: a.curve("SnowCompressionRestoreCurve")?,
            snow_compression_from_dig_curve: a.curve("SnowCompressionFromDigCurve")?,
            prewind_carve_input_dampening_curve: a.curve("PrewindCarveInputDampeningCurveAsset")?,
            prewind_carve_input_classic_dampening_curve: a.curve("PrewindCarveInputClassicDampeningCurveAsset")?,
            prewind_dampens_carving: a.int("PrewindDampensCarving")? != 0,
            board_press_friction_modifier: a.f32("BoardPressFrictionModifer")?,
            collision_look_ahead: a.f32("CollisionLookAhead")?,
            collision_scan_forward_angle: a.f32("CollisionScanForwardAngle")?,
            collision_scan_side_angle: a.f32("CollisionScanSideAngle")?,
            collision_steering_angular_vel: a.f32("CollisionSteeringAngularVel")?,
            scoring_vertical_surface_angle_threshold: a.f32("ScoringVerticalSurfaceAngleThreshold")?,
            uber_crash_ticks_to_full_friction: a.f32("UberCrashTicksToFullFriction")?,
            carving_spray_filter_responsiveness: a.f32("CarvingSprayFilterResponsiveness")?,
            ice_axe: crate::ice_axe::IceAxeTuning::load(&a.asset("IceAxeProperties")?)?,
        })
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct BallisticTuning {
    pub cos_angle_from_vertical: f32,
    pub cos_parallel_angle: f32,
    pub min_speed: f32,
}

#[derive(Clone, Debug, Default)]
pub struct QueryTuning {
    pub cos_surface_is_horizontal_angle_threshold: f32,
    pub ballistic_probe_lookahead_ticks: f32,
    pub ballistic_probe_ray_radius: f32,
    pub fat_probe_ray_radius: f32,
    pub coefficient_of_restitution: f32,
    pub coefficient_of_friction: f32,
    pub min_bounce_speed: f32,
    pub max_ghost_bounce_speed: f32,
    pub jump_launch_speed_vs_speed: Curve,
    pub jump_launch_angle_vs_speed: Curve,
    pub jump_launch_compression_factor: f32,
    pub jump_launch_forward_factor: f32,
    pub jump_launch_cos_small_angle: f32,
    pub up_bias_vs_ground_normal_angle: Curve,
    pub normal_bias_vs_lean_angle: Curve,
    pub num_jump_launch_probe_rays: u32,
    pub jump_launch_lookahead_ticks: f32,
    pub jump_launch_max_up_angle: f32,
    pub jump_launch_max_down_angle: f32,
    pub jump_launch_max_side_angle: f32,
    pub jump_launch_max_lateral_angle: f32,
    pub jump_launch_forward_factor_bonking: f32,
    pub correction_offset: f32,
    pub collision_probe_lookahead: f32,
    pub probe_ray_separation: Curve,
    pub soft_ray_acceleration: Curve,
    pub energy_preservation: Curve,
    pub cos_angle_from_upright_for_no_impact_on_landing: f32,
    pub min_impact_speed_threshold: f32,
    pub max_impact_speed_threshold: f32,
    pub cos_angle_from_upright_for_impact_on_landing: f32,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct PlayerTuning {
    pub min_on_ground_jump_ticks: f32,
    pub max_in_air_jump_ticks: f32,
    pub filter_gain: f32,
    pub force_race_end_max_time: f32,
    pub stopped_speed_threshold: f32,
    pub out_of_bounds: crate::rider::boundary::BoundaryTuning,
}

#[derive(Clone, Debug)]
pub struct Tuning {
    pub cp_min: CharacterProperties,
    pub cp_max: CharacterProperties,
    pub carving: CarvingTuning,
    pub ballistic: BallisticTuning,
    pub queries: QueryTuning,
    pub player: PlayerTuning,
    pub janked: crate::janked::JankedTuning,
    pub surfaces: crate::surface::SurfaceTable,
    pub aerial: crate::air::AerialTuning,
    pub balance: crate::landing::BalanceTuning,
    pub tricks: crate::tricks::TrickTuning,
    pub added_trick_ids: crate::tricks::AddedTrickIds,
    pub scoring: crate::scoring::ScoringTuning,
    pub combo_flow: crate::scoring::ComboFlowTuning,
    pub root_motion: crate::rootmotion::RootMotion,
    pub grind: crate::rider::grind::GrindTuning,
    pub grind_steering: crate::rider::grind::steering::SteeringTuning,
    pub board_press: crate::press::BoardPressTuning,
    pub wingsuit: crate::wingsuit::WingsuitTuning,
    pub survival: crate::survival::SurvivalTuning,
    pub trip: crate::trip::TripTuning,
    pub avalanche: crate::avalanche::AvalancheTuning,
    pub board_particle: crate::rider::board_particle::BoardParticleTuning,
    pub posted_events: std::sync::Arc<crate::nis_events::PostedEventsChunk>,
}

impl Tuning {
    pub fn load(bundle: &Bundle) -> Result<Self> {
        let (cp_min, cp_max) = CharacterProperties::load_min_max(bundle)?;
        let carving = CarvingTuning::load(&bundle.view("carve_riding", "CarvingControllerAsset")?)?;
        let b = bundle.view("ballistic_airborne", "BallisticControllerAsset")?;
        let ballistic = BallisticTuning {
            cos_angle_from_vertical: b.f32("CosAngleFromVertical")?,
            cos_parallel_angle: b.f32("CosParallelAngle")?,
            min_speed: b.f32("MinSpeed")?,
        };
        let q = bundle.view("SnowboarderPhysicsQueriesSceneOp", "SnowboarderPhysicsQueriesSceneOpAsset")?;
        let queries = QueryTuning {
            cos_surface_is_horizontal_angle_threshold: q.f32("CosSurfaceIsHorizontalAngleThreshold")?,
            ballistic_probe_lookahead_ticks: q.f32("BallisticProbeLookaheadTicks")?,
            ballistic_probe_ray_radius: q.f32("BallisticProbeRayRadius")?,
            fat_probe_ray_radius: q.f32("FatProbeRayRadius")?,
            coefficient_of_restitution: q.f32("CoefficientOfRestitution")?,
            coefficient_of_friction: q.f32("CoefficientOfFriction")?,
            min_bounce_speed: q.f32("MinBounceSpeed")?,
            max_ghost_bounce_speed: q.f32("MaxGhostBounceSpeed")?,
            jump_launch_speed_vs_speed: q.curve("JumpLaunchSpeedVsSpeedCurve")?,
            jump_launch_angle_vs_speed: q.curve("JumpLaunchAngleVsSpeedCurve")?,
            jump_launch_compression_factor: q.f32("JumpLaunchCompressionFactor")?,
            jump_launch_forward_factor: q.f32("JumpLaunchForwardFactor")?,
            jump_launch_cos_small_angle: q.f32("JumpLaunchCosSmallAngle")?,
            up_bias_vs_ground_normal_angle: q.curve("UpBiasVsGroundNormalAngleCurve")?,
            normal_bias_vs_lean_angle: q.curve("NormalBiasVsLeanAngleCurve")?,
            num_jump_launch_probe_rays: q.int("NumJumpLaunchProbeRays")?.max(0) as u32,
            jump_launch_lookahead_ticks: q.f32("JumpLaunchLookaheadTicks")?,
            jump_launch_max_up_angle: q.f32("JumpLaunchMaxUpAngle")?,
            jump_launch_max_down_angle: q.f32("JumpLaunchMaxDownAngle")?,
            jump_launch_max_side_angle: q.f32("JumpLaunchMaxSideAngle")?,
            jump_launch_max_lateral_angle: q.f32("JumpLaunchMaxLateralAngle")?,
            jump_launch_forward_factor_bonking: q.f32("JumpLaunchForwardFactorBonking")?,
            correction_offset: q.f32("CorrectionOffset")?,
            collision_probe_lookahead: q.f32("CollisionProbeLookahead")?,
            probe_ray_separation: q.curve("ProbeRaySeparationCurveAsset")?,
            soft_ray_acceleration: q.curve("SoftRayAccelerationCurveAsset")?,
            energy_preservation: q.curve("EnergyPreservationCurve")?,
            cos_angle_from_upright_for_no_impact_on_landing: q.f32("CosAngleFromUprightForNoImpactOnLanding")?,
            min_impact_speed_threshold: q.f32("MinImpactSpeedThreshold")?,
            max_impact_speed_threshold: q.f32("MaxImpactSpeedThreshold")?,
            cos_angle_from_upright_for_impact_on_landing: q.f32("CosAngleFromUprightForImpactOnLanding")?,
        };
        let p = bundle.view("PlayerUpdateSceneOp", "PlayerUpdateSceneOpAsset")?;
        let player = PlayerTuning {
            min_on_ground_jump_ticks: p.f32("MinOnGroundJumpTicks")?,
            max_in_air_jump_ticks: p.f32("MaxInAirJumpTicks")?,
            filter_gain: p.f32("FilterGain")?,
            force_race_end_max_time: p.f32("ForceRaceEndMaxTime")?,
            stopped_speed_threshold: p.f32("StoppedSpeedThreshold")?,
            out_of_bounds: crate::rider::boundary::BoundaryTuning {
                time_pow: p.f32("OutOfBoundsTimePow")?,
                direction_pow: p.f32("OutOfBoundsDirectionPow")?,
                strength_max: p.f32("OutOfBoundsCorrectionStrengthMax")?,
                timer_max: p.f32("OutOfBoundsCorrectionTimerMax")?,
            },
        };
        let janked = crate::janked::JankedTuning::load(bundle)?;
        let surfaces = crate::surface::SurfaceTable::load(bundle)?;
        let aerial = crate::air::AerialTuning::load(bundle)?;
        let balance = crate::landing::BalanceTuning::load(bundle)?;
        let tricks = crate::tricks::TrickTuning::load(bundle)?;
        let added_trick_ids = crate::tricks::AddedAirTricks::load(bundle.data_root()).1;
        let scoring = crate::scoring::ScoringTuning::load(bundle)?;
        let combo_flow = crate::scoring::ComboFlowTuning::load(bundle)?;
        Ok(Self {
            cp_min,
            cp_max,
            carving,
            ballistic,
            queries,
            player,
            janked,
            surfaces,
            aerial,
            balance,
            tricks,
            added_trick_ids,
            scoring,
            combo_flow,
            root_motion: crate::rootmotion::RootMotion::load(bundle)?,
            grind: crate::rider::grind::GrindTuning::load(bundle)?,
            grind_steering: crate::rider::grind::steering::SteeringTuning::load(bundle)?,
            board_press: crate::press::BoardPressTuning::load(bundle)?,
            wingsuit: crate::wingsuit::WingsuitTuning::load(bundle)?,
            survival: crate::survival::SurvivalTuning::load(bundle)?,
            trip: crate::trip::TripTuning::load(&bundle.view("carve_riding", "CarvingControllerAsset")?)?,
            avalanche: crate::avalanche::AvalancheTuning::load(bundle)?,
            board_particle: crate::rider::board_particle::BoardParticleTuning::load(bundle)?,
            posted_events: std::sync::Arc::new(crate::nis_events::PostedEventsChunk::load(bundle)?),
        })
    }

    pub fn resolve(&self, gear_mix: f32) -> Result<ResolvedCharacterProperties> {
        let gear = if gear_mix == DEFAULT_GEAR_MIX { GearStats::default_gear() } else { GearStats::uniform(gear_mix) };
        self.resolve_gear(&gear)
    }

    pub fn resolve_gear(&self, gear: &GearStats) -> Result<ResolvedCharacterProperties> {
        ResolvedCharacterProperties::resolve(&CharacterProperties::mix_gear(&self.cp_min, &self.cp_max, gear)?)
    }
}
