use super::*;

impl RiderGraph {
    pub(super) fn uber_crash_orientation(&mut self, s: &RiderState, velocity: Vec3) {
        if !s.ballistic.valid {
            return;
        }
        let n = s.ballistic.normal;
        let vh = velocity - n * velocity.dot(n);
        self.put("FLT_uber_crash_ground_speed", vh.length());
        let (Some(hips), Some(head)) = (self.hips, self.head) else { return };
        let pose = &self.pose.nodes;
        if pose.len() != self.rig.nodes.len() {
            return;
        }
        let m = model_matrix(&self.rig, pose, hips);
        let h = model_matrix(&self.rig, pose, head)[3];
        let head_up = Vec3::new(h[0], h[1], h[2]).normalize_or_zero().dot(self.traj_rotation * Vec3::Y);
        let axis = |r: usize| Vec3::new(m[r][0], m[r][1], m[r][2]).normalize_or_zero();
        let (x, y, z) = (axis(0), axis(1), axis(2));
        let axes = [y, -y, -x, x, z, -z];
        let vh_n = vh.normalize_or_zero();
        let (mut start, mut best) = (0usize, f32::MIN_POSITIVE);
        for (i, a) in axes.iter().enumerate().take(4) {
            let d = a.dot(n);
            if d > best {
                (start, best) = (i, d);
            }
        }
        let (mut vel, mut best) = (0usize, f32::MIN_POSITIVE);
        for (i, a) in axes.iter().enumerate() {
            let d = (-*a).dot(vh_n);
            if d > best && i != start {
                (vel, best) = (i, d);
            }
        }
        self.put("GSENUM_UberCrashOrientation_start", start as f32);
        self.put("GSENUM_UberCrashOrientation_vel", vel as f32);
        self.put_bool("BLN_uber_crash_getup_head_pos_y", head_up > 0.0);
    }

    pub(super) fn write_signals(
        &mut self,
        s: &RiderState,
        input: &Input,
        tricks: &TrickInput,
        start: &StepStart,
        dt: f32,
    ) {
        let riding = s.state == RideState::Riding;
        self.put("deltaTicks", dt);
        self.put_bool("BLN_InAir", self.producers.in_air_tag);
        self.put_bool("BLN_IsCloseToGround", s.close.is_close);
        self.put_bool("BLN_IsVeryCloseToGround", s.close.is_very_close);
        self.put_bool("BLN_Janked", s.janked.janked);
        let take_off = s.state == RideState::Jump && self.producers.last_state == Some(RideState::Riding);
        let jumped = take_off || s.air_jumped;
        self.put_bool("BLN_AllowJumps", jumped);
        self.put_bool("BLN_JumpTrigger", jumped);
        self.put_bool("BLN_EnableAirTricks", !(s.state == RideState::Airborne && s.jump.allow));
        self.producers.last_state = Some(s.state);
        if riding {
            self.producers.carving(s, dt, &self.carve);
        } else {
            self.producers.carving_init = false;
        }
        let p = self.producers.clone();
        self.put("FLT_carving_normalized_angle_to_ground", p.angle_to_ground);
        self.put("FLT_carving_normalized_board_relative_move_angle", p.board_move);
        self.put("FLT_Lean", s.kin.lean_norm);
        self.forward = s.kin.fwd > 0.0;
        self.put_bool("BLN_Forward", self.forward);
        self.put_bool("BLN_Goofy", s.goofy);
        self.put("FLT_LandingPitchBalanceOffset", -wrap_pi(s.landing.pitch + s.kin.pitch) * s.kin.fwd);
        self.put("FLT_LandingBalanceValue", s.landing.pitch_balance.clamp(-1.0, 1.0));
        let spin = wrap_4pi(s.landing.spin * s.steering.forward_sign);
        self.put("FLT_landing_spin_value", spin);
        self.put("FLT_landing_spin_value_normalized", wrap_pi(spin));
        self.put_bool("BLN_landing_mirror_flag", s.landing.landed_switch);
        self.put_bool("BLN_carving_is_stable", s.landing.stable);
        self.put_bool("BLN_carving_sliding_on_back", s.landing.backslide);
        self.put_bool("BLN_State_Wipeout", start.wipeout);
        self.put_bool("BLN_carving_state_bail_on_landing", s.landing.bail_on_landing);
        self.put_bool("BLN_trigger_uber_crash", s.landing.uber_crash);
        self.put_bool("BLN_EnableBoardParticle", s.board_particle.enabled);
        self.put_bool("BLN_TriggerUberBailGateOpen", s.uber_gate_timer <= 0.0);
        self.put("FLT_TriggerUberCrashGatingTimer", s.uber_gate_timer);
        self.put_bool("BLN_carving_state_request_stop", s.request_stop);
        self.put_bool("BLN_Pad_GetGoing", crate::rider::get_going_gesture(start.raw_stick, s.stick_prev));
        self.put("FLT_DPad_Char_Rel_Angle", crate::rootmotion::GET_GOING_ANGLE);
        self.put_bool("BLN_start_uber_bail_recover", true);
        self.put_bool("BLN_is_recovering_from_uber_bail", s.recovering_from_uber_bail());
        self.uber_crash_orientation(s, start.velocity);
        self.put("FLT_carving_state_RideTicks", s.carving.ride_ticks);
        self.put_bool("BLN_carving_state_is_dead", s.carving.is_dead);
        self.put_bool("BLN_carving_state_BailedBecauseOfRock", s.landing.bailed_because_of_rock);
        self.put_bool("BLN_Tripped", s.tripped);
        let reentry = s.reentry;
        self.put_bool("BLN_TriggerImpactBail", reentry == Some(crate::rider::Reentry::ImpactBail));
        self.put_bool("BLN_TriggerImpact", reentry == Some(crate::rider::Reentry::Impact));
        self.put("FLT_SurfaceWeightRock", s.weights[crate::surface::ROCK]);
        self.put("FLT_PlayerSpeed", s.player_speed);
        self.put("FLT_PlayerSpeedRatio", s.player_speed_ratio);
        let traj = self.traj_rotation;
        self.put("FLT_PlayerFaceUpFactor", row(traj, 2).y.atan2(row(traj, 0).y) / PI);
        let h = &s.heli;
        self.put_bool("BLN_HelicopterInteraction", h.helicopter_interaction);
        self.put_bool("BLN_HeliInteractionGrind", h.grind);
        self.put_bool("BLN_HeliInteractCrossways", h.across);
        self.put_bool("BLN_GoofynessForHeliInteraction", h.goofyness);
        self.put_bool("BLN_HeliInteractionMirror", h.mirror);
        self.put("FLT_HeliInteractAnimTimeScale", h.anim_time_scale);
        let gc = &s.grind.carving;
        self.put_bool("BLN_IsGrinding", s.grind.is_grinding());
        self.put("FLT_GrindSpin", gc.spin);
        self.put("FLT_GrindVeeringAngle", gc.veering_angle);
        self.put("FLT_carving_state_GrindPressValue", gc.press.value);
        self.put("FLT_carving_state_GrindOrBoardPressBlendValue", gc.grind_or_press_blend);
        let g = &s.tricks.grind;
        self.put_bool("BLN_IsGrindTrickStarting", g.starting);
        self.put_bool("BLN_IsGrindTrick", g.is_grind_trick());
        self.put_bool("BLN_IsGrindTrickEnter", g.is_grind_trick_enter());
        self.put_bool("BLN_Pad_L2_Hold", s.grind.l2_hold);
        self.put("INT_GrindGroup", g.group as f32);
        self.write_air_trick_pad(s, input, tricks, start, dt);
    }
}

impl RiderGraph {
    pub(super) fn write_air_trick_pad(
        &mut self,
        s: &RiderState,
        input: &Input,
        tricks: &TrickInput,
        start: &StepStart,
        dt: f32,
    ) {
        let riding = s.state == RideState::Riding;
        let o = s.air.outputs;
        self.put("FLT_FlipMagnitude", o.flip_magnitude);
        self.put("FLT_SpinMagnitude", o.spin_magnitude);
        self.put("FLT_FlipSpeed", o.flip_speed);
        self.put("FLT_SpinSpeed", o.spin_speed);
        self.put("FLT_FlipAngleDegrees", o.flip_angle_degrees);
        self.put("FLT_FlipWiggleAngle", o.flip_wiggle);
        self.put("FLT_SpinWiggleAngle", o.spin_wiggle);
        self.put("FLT_AirIdleTime", o.air_idle_time);
        self.put_bool("BLN_AirIdle", o.air_idle);
        self.put_bool("BLN_EnableAirborneAutoUprighting", s.airborne_auto_upright);
        self.put("FLT_NumTicksToLanding", s.ballistic.ticks);
        self.put_bool("BLN_BallisticRayValid", s.ballistic.valid);
        self.put("VEC_Velocity.y", start.velocity.y);
        let t = &s.tricks;
        self.put("GSENUM_TrickType", f32::from(t.trick_type.value()));
        self.put("GSENUM_TrickTypeLocked", f32::from(t.trick_type_locked.value()));
        self.put("GSENUM_TrickHand", hand_value(t.hand));
        self.put_bool("BLN_HasValidTrickInput", t.has_valid_input);
        self.put_bool("BLN_IsTweaking", t.is_tweaking);
        self.put_bool("BLN_IsSignatureTrick", t.is_signature);
        self.put_bool("BLN_IsTricking", t.is_tricking);
        self.put("INT_TrickInputIndex", t.input_index as f32);
        self.put("INT_TrickInputIndexOnTrickEnter", t.input_index_on_enter as f32);
        self.put("INT_TrickGestureLength", t.gesture_len as f32);
        self.put_bool("BLN_IsAwesome", s.score.is_awesome);
        self.put_bool("BLN_IsSuperAwesome", s.score.is_super_awesome);
        let stick = stick_axes(s.pad_stick);
        self.put("FLT_Pad_LStickXAxis", stick.x);
        self.put("FLT_Pad_LStickYAxis", stick.y);
        self.put_bool("BLN_Pad_A_Hold", input.jump);
        self.put("FLT_Pad_A_Hold_Filtered", s.jump.a_hold_filtered);
        let k = (-0.11 * dt).exp();
        let ry = stick_axes(tricks.right_stick).y;
        self.producers.rstick_filtered = ry + (self.producers.rstick_filtered - ry) * k;
        self.put("FLT_Pad_RStickYAxis_Filtered", self.producers.rstick_filtered);
        self.put_bool("BLN_Pad_Grab_Button_Signature", tricks.signature);
        self.put("FLT_BoostBlendValue", s.boost.blend);
        self.put_bool("BLN_IsBoosting", s.boost.is_boosting);
        self.put("FLT_Boost", s.score.boost);
        self.put_bool("BLN_Pad_R2_Hold", tricks.tweak);
        self.put("FLT_Pad_RTriggerMagnitude", s.boost.modifier);
        self.put("FLT_Health", s.survival.health);
        self.put("FLT_InternalTemperature", s.survival.temperature);
        self.put("FLT_OxygenLevel", s.survival.oxygen);
        let ws = &s.wingsuit;
        self.put_bool("BLN_Pad_Wingsuit_Press", ws.press());
        self.put_bool("BLN_Pad_Wingsuit_Hold", ws.hold);
        self.put_bool("BLN_WingsuitEnabled", ws.enabled);
        self.put_bool("BLN_IsRaceProperStarted", ws.race_proper_started);
        self.put_bool("BLN_ForceDescentInAir", ws.force_descent_in_air);
        self.put_bool("BLN_State_Wingsuit", ws.signal);
        self.put("FLT_WingsuitUsePercentage", ws.use_percentage);
        self.put("FLT_WingsuitEffectiveness", ws.effectiveness);
        let ia = &s.ice_axe;
        self.put_bool("BLN_IceAxeEnabled", ia.enabled);
        self.put_bool("BLN_ice_axe_start_chop_left", ia.start_chop[0]);
        self.put_bool("BLN_ice_axe_start_chop_right", ia.start_chop[1]);
        self.put_bool("BLN_ice_axe_deployed", ia.deployed);
        self.put_bool("BLN_ice_axe_chopped", ia.chopped);
        self.put("FLT_IceAxe_BlendWeight", ia.blend_weight);
        self.put_bool("BLN_carving_ice_axe_effects_enabled", ia.frame.active);
        self.put("FLT_ice_axe_layer_weight", ia.layer_weight);
        self.put("FLT_ice_axe_chop_weight_left", ia.chop_weight[0]);
        self.put("FLT_ice_axe_chop_weight_right", ia.chop_weight[1]);
        self.put("FLT_carving_state_ice_axe_noise_left", ia.noise[0]);
        self.put("FLT_carving_state_ice_axe_noise_right", ia.noise[1]);
        if riding || !self.producers.was_in_air {
            self.producers.airborne_timer = 0.0;
        } else {
            self.producers.airborne_timer += 1.0;
        }
        self.producers.was_in_air = !riding;
        self.put("INT_AirborneTimer", self.producers.airborne_timer);
        self.run_scripts();
    }

    pub(super) fn run_scripts(&mut self) {
        let sc = self.scripts.get_mut().unwrap_or_else(std::sync::PoisonError::into_inner);
        sc.update_players();
        for (n, v) in sc.take_written() {
            self.animator.set(&n, v);
        }
        for (i, function, float) in &self.script_signals {
            if let Some(v) = sc.expression(function, *float) {
                self.animator.gs.set(*i, v);
            }
        }
        sc.collect();
        self.script_updates += 1;
    }

    pub(super) fn trajectory_joint_rotation(&mut self, s: &RiderState) -> Quat {
        let traj = s.traj_orientation * Quat::from_rotation_y(-FRAC_PI_2);
        let sliding = s.state == RideState::Riding
            && ((s.landing.uber_crash && s.uber_gate_timer <= 0.0) || s.board_particle.enabled);
        if !sliding {
            self.producers.uber_traj = None;
            return traj;
        }
        let q = match self.producers.uber_traj {
            Some(q) => {
                self.producers.uber_frames += 1;
                if self.producers.uber_frames > 1 { uber_tilt(q, s.ground_normal, self.carve.uber_traj_max) } else { q }
            }
            None => {
                self.producers.uber_frames = 0;
                traj
            }
        };
        self.producers.uber_traj = Some(q);
        q
    }
}
