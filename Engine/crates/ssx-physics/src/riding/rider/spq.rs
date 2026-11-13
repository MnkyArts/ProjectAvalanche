use super::*;

pub fn scoring_context(s: &RiderState, w: &World, cached: (Vec3, Vec3, Vec3)) -> ScoringContext {
    ScoringContext {
        track_started: s.track_started,
        is_uphill: s.is_uphill,
        player_speed: s.player_speed / 60.0,
        velocity_y: cached.0.y,
        position: cached.1,
        prev_position: cached.2,
        level_start_y: s.level_start.y,
        on_ground: s.on_ground,
        death_by_death_plane: s.death.dead_by_death_plane,
        in_wingsuit: s.wingsuit.signal,
        is_boosting: s.boost.is_boosting,
        tuning: w.tuning.combo_flow,
        ..ScoringContext::default()
    }
}

pub(super) fn score(s: &mut RiderState, w: &World, input: &Input, ctx: &ScoringContext, dt: f32) {
    let in_air = s.in_air();
    let wipeout = is_scoring_wipeout(s, in_air);
    let o = s.air.outputs;
    let inputs = ScoringInputs {
        in_air,
        wipeout,
        spin_speed: if in_air { o.spin_speed } else { 0.0 },
        flip_speed: if in_air { o.flip_speed } else { 0.0 },
        flip_accel: input.flip_acceleration(s.tricks.classic),
        goofy: s.goofy,
        trick_type: s.tricks.trick_type,
        tricking: s.tricks.is_tricking,
        trick_ending: s.tricks.ending(),
        trick_locked: s.tricks.trick_locked,
        tweaking: s.tricks.is_tweaking,
        is_grinding: s.grind.is_grinding(),
        is_grind_trick: s.tricks.is_grind_trick(),
        super_grind: s.grind.carving.super_grind,
        grind_spin_vel: s.grind.carving.spin_vel,
        grind_group: s.tricks.grind.group,
        is_board_press: s.press.is_board_press,
        press_value: s.grind.carving.press.value,
        lua_wipeout: s.landing.uber_crash || s.landing.tumbling,
        helicopter_interaction: s.heli.helicopter_interaction,
    };
    s.score.update_with(&w.tuning.scoring, &w.props, &inputs, ctx, dt);
    let gained = &mut s.survival.health_gained_from_tricks;
    crate::survival::health_from_points(&w.tuning.survival, gained, s.score.award, s.score.is_awesome, dt);
    s.press.is_board_press = s.score.is_board_press;
}

pub(super) fn spq(s: &mut RiderState, w: &World, dt: f32, correct: Option<()>) {
    let t = &w.tuning;
    let in_air = s.in_air();
    let prev_colliding = s.impact.colliding;
    let prev_very_close = s.close.is_very_close;
    let mut impact = ImpactState::default();
    s.correction_shift = Vec3::ZERO;
    if correct.is_some() {
        let out = w.collision_correction(&CorrectionInputs {
            from: s.traj_cached,
            to: s.traj,
            velocity: if in_air { s.traj_velocity } else { s.velocity },
            orientation: s.traj_orientation,
            ground_valid: s.contact.is_some(),
            distance_from_ground: s.contact.map_or(f32::MAX, |c| c.distance),
            prev_colliding,
            grinding: s.grind.is_grinding(),
            prev_grinding: s.grind.prev_is_grinding,
            dt,
        });
        s.correction_shift = out.position - s.traj;
        s.traj = out.position;
        if let Some(b) = out.bounce {
            (s.traj_velocity, s.velocity, s.filtered_velocity) = (b, b, b);
            s.impacts += 1;
        }
        if let Some(v) = out.soft
            && !in_air
        {
            s.velocity = v;
        }
        impact = out.impact;
        s.janked.update(&t.janked, impact.colliding, impact.in_soft_collision, s.traj, dt);
    }
    let wind = s.grind.wind;
    s.ballistic = w.ballistic_probe(s.traj, s.traj_velocity, wind, in_air, dt);
    let probe = ProbeInputs {
        traj: s.traj,
        joint: s.board_point(),
        prev_normal: s.ground_normal,
        velocity: if in_air { s.traj_velocity } else { s.velocity },
        in_air,
        short: s.grind.steering.active,
        grinding: s.grind.is_grinding(),
        uber_in_air: in_air && s.uber_trigger && !s.board_particle.enabled,
        wind,
    };
    s.contact = w.ground_probe(&probe, &s.ballistic);
    let sa = s.grind.sa.as_deref().unwrap_or(grind::surface_analysis::initial());
    s.grind.sa_ground = grind::surface_analysis::sa_ground(&s.grind.sa_ground, sa, s.traj, s.board_point());
    let chatter = ChatterInputs {
        distance_carved: s.carving.distance_carved,
        grind_blend: s.grind.carving.blend,
        player_speed: s.player_speed,
    };
    match &s.contact {
        Some(c) => {
            s.weights = c.weights;
            s.surface = t.surfaces.blend(&c.weights, &chatter);
            s.ground_normal = c.normal;
        }
        None => {
            s.weights = no_contact_weights();
            s.ground_normal = Vec3::Y;
        }
    }
    s.close = close_to_ground(s.contact.as_ref(), s.surface.snow_depth);
    let launch = LaunchInputs {
        velocity: s.traj_velocity,
        in_air,
        ground_normal: s.ground_normal,
        traj_up: s.traj_orientation * Vec3::Y,
        traj_pos: s.traj,
        compression: s.jump.compression,
        dt,
        grinding: s.grind.is_grinding(),
        stick_angle: s.grind.stick.0,
        stick_mag: s.grind.stick.1,
        wind,
    };
    if let Some(l) = jump_launch(w, &launch) {
        s.jump.launch = l;
    }
    if correct.is_some()
        && !prev_very_close
        && s.close.is_very_close
        && let Some(c) = s.contact
        && (s.traj_orientation * Vec3::Y).dot(c.normal) < t.queries.cos_angle_from_upright_for_impact_on_landing
    {
        let li = w.impact_surface_weights(s.traj_orientation, s.traj_velocity, c.normal, prev_colliding, c.entity);
        if li.trigger_impact {
            impact = ImpactState { in_soft_collision: impact.in_soft_collision, ..li };
        }
        impact.colliding = true;
    }
    if s.landing.uber_crash || s.landing.tumbling {
        impact.trigger_impact = false;
    }
    if let Some(g) = s.ghost_impact.take() {
        let v = s.traj_velocity;
        let b = w.ghost_bounce(v, g);
        (s.traj_velocity, s.velocity, s.filtered_velocity) = (b, b, b);
        impact = w.ghost_impact_state(impact, s.traj_orientation, v, prev_colliding);
    }
    s.impact = impact;
    if let Some(e) = impact.collision_entity {
        s.collision_entity = e;
    }
}

pub fn update_player_speed(s: &mut RiderState, in_air: bool) {
    let v = if in_air { s.traj_velocity } else { s.velocity };
    s.player_speed = v.length() * 60.0;
    s.player_speed_ratio = (s.player_speed / 45.0).min(1.0);
}

pub fn has_prewind_input(input: &Input, prewind_hold: bool, classic: bool) -> bool {
    input.jump || (prewind_hold && !classic)
}

pub(super) fn update_prewind(s: &mut RiderState, w: &World, input: &Input, prewind_hold: bool, dt: f32) {
    let r = &w.props;
    let in_air = s.state != RideState::Riding;
    let awesome = s.score.is_awesome;
    let accum = if awesome { r.awesome_prewind_accum_rate } else { r.prewind_accum_rate };
    let natural = if awesome { r.awesome_prewind_decay_rate_natural } else { r.prewind_decay_rate_natural };
    if carving_crashed(s, in_air) || s.prewind_reset {
        (s.prewind, s.prewind_analog, s.prewind_reset) = (0.0, 0.0, false);
        return;
    }
    let classic = s.tricks.classic;
    let has_input = has_prewind_input(input, prewind_hold, classic);
    let filter =
        |value: f32, (target, rate): (f32, f32)| (target + (value - target) * (-rate * dt).exp()).clamp(0.0, 1.0);
    let standard = if !in_air && has_input {
        (1.0, accum)
    } else if input.stick().length() >= 0.5 {
        (0.0, natural)
    } else {
        (0.0, r.prewind_decay_rate_reverse)
    };
    let classic_target =
        if !in_air && has_input && input.dpad_held { (1.0, accum) } else { (0.0, r.prewind_decay_rate_reverse) };
    s.prewind_analog = filter(s.prewind_analog, standard);
    s.prewind = filter(s.prewind, if classic { classic_target } else { standard });
}

pub fn board_axes(s: &RiderState) -> (Vec3, Vec3) {
    if let Some((_, q)) = s.board_particle.placement {
        return (q * Vec3::X, q * Vec3::Z);
    }
    if s.graph.driven {
        return (s.graph.board_x, s.graph.board_z);
    }
    (s.traj_orientation * Vec3::Y, s.traj_orientation * Vec3::X)
}

pub fn board_rotation((x, z): (Vec3, Vec3)) -> Quat {
    Quat::from_mat3(&glam::Mat3::from_cols(x, z.cross(x), z)).normalize()
}
