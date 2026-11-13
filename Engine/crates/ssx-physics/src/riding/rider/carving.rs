use super::*;

pub fn should_stay_dead_once_tumbled(s: &RiderState) -> bool {
    let v = &s.survival;
    v.health <= 0.0 || v.temperature <= 0.0 || v.oxygen <= 0.0
}

pub(super) fn carving_crashed(s: &RiderState, in_air: bool) -> bool {
    s.landing.tumbling || (s.landing.uber_crash && !in_air)
}

pub fn tuck_blend(s: &RiderState) -> f32 {
    (s.stick_y_filtered.clamp(0.0, 0.5) * 1.2 + s.player_speed_ratio * 0.4 + s.boost.blend).clamp(0.0, 1.0)
}

pub fn is_scoring_wipeout(s: &RiderState, in_air: bool) -> bool {
    scoring_wipeout(&s.landing, s.kin.fwd, in_air) || s.board_particle.enabled
}

pub fn scoring_wipeout(l: &LandingState, forward_sign: f32, in_air: bool) -> bool {
    let a = wrap_pi(l.pitch);
    let safe = if forward_sign > 0.0 {
        a < SCORING_BACKSIDE_SAFE_ZONE
    } else if forward_sign < 0.0 {
        a > -SCORING_BACKSIDE_SAFE_ZONE
    } else {
        false
    };
    (l.tumbling && !safe) || (l.uber_crash && !in_air)
}

pub(super) fn rock_riding_update(s: &mut RiderState, w: &World, effects: f32, dt: f32) {
    let t = &w.tuning.survival;
    if s.weights[crate::surface::ROCK] >= t.riding_on_rock_threshold {
        s.carving.ticks_on_rock += dt;
    } else if s.carving.have_made_first_contact {
        s.carving.ticks_on_rock = 0.0;
        s.landing.bailed_because_of_rock = false;
    }
    let recently = s.carving.ride_ticks < t.uber_crash_landed_recently_threshold;
    if w.survival.rock_damage
        && effects >= 1.0
        && s.carving.ticks_on_rock > 0.0
        && s.carving.have_made_first_contact
        && !s.landing.bailed_because_of_rock
        && (s.landing.stable || recently)
    {
        s.landing.bailed_because_of_rock = true;
        s.survival.carving_damage += 0.0625;
    }
}

pub const SCORING_BACKSIDE_SAFE_ZONE: f32 = 2.35;

pub(super) fn touchdown(s: &mut RiderState, w: &World, dt: f32) {
    new_carving(s, w, dt);
    s.tricks.land();
}

pub(super) fn conduit_init(s: &mut RiderState, w: &World, dt: f32) -> bool {
    (s.get_going, s.jump_airborne, s.janked_pin, s.heli_trick) = (None, false, None, None);
    (s.wingsuit.controller, s.wingsuit.blend, s.wingsuit.ticks) = (None, None, 0.0);
    if !s.close.is_close && !s.recovering_from_uber_bail() && can_go_airborne_from_grind_trick(s) {
        (s.state, s.first_air_tick, s.stopped) = (RideState::Airborne, true, false);
        return true;
    }
    s.state = RideState::Riding;
    new_carving(s, w, dt);
    false
}

pub(super) fn new_carving(s: &mut RiderState, w: &World, dt: f32) {
    s.alignment = s.traj_orientation;
    let t = &w.tuning.balance;
    let gate_open = s.uber_gate_timer <= 0.0;
    let board_particle = s.board_particle.enabled;
    if !board_particle && w.survival.avalanche.reverse_cam {
        s.uber_trigger = false;
    }
    if s.survival.trigger_impact_bail || s.tricks.ticks_since_sig_held <= t.max_ticks_since_sig_trick_held_for_uber_bail
    {
        (s.uber_trigger, s.uber_gate_timer) = (true, -1.0);
    } else if s.tricks.is_signature && !board_particle {
        s.uber_trigger = false;
    }
    let uber = (s.uber_trigger && gate_open) || board_particle;
    s.carving = CarvingState::default();
    s.carving.air_time = s.score.air_time;
    s.avalanche.fresh_controller();
    grind::carving_ctor(&mut s.grind.carving, &w.collision, s.contact.map(|c| c.point), s.traj);
    s.press.ctor();
    (s.nested_stable, s.request_stop, s.stopped) = (false, false, false);
    s.velocity = s.traj_velocity;
    let n = s.ground_normal;
    let (v, w_q) = (s.velocity, s.angular_velocity_q);
    let kick = crate::landing::ImpactKick {
        pre_orientation: s.impact.pre_impact_orientation,
        pre_velocity: s.impact.pre_impact_velocity,
        velocity: s.impact.impact_velocity,
    };
    let c = &w.tuning.carving;
    s.landing.touchdown(t, c, &mut s.alignment, &mut s.steering, v, n, w_q, uber, true, &kick, dt);
    if !uber {
        s.uber_trigger = false;
    }
    s.lean = 0.0;
    s.prewind_reset = true;
}

pub(super) fn janked_motion(
    s: &mut RiderState,
    w: &World,
    input: &Input,
    before: RideState,
    dt: f32,
    running: Option<&[String]>,
) -> (Vec3, Option<Quat>, &'static [&'static str]) {
    let t = &w.tuning;
    let t0 = s.janked.anim_ticks;
    s.janked.advance_sequence(&t.janked, dt);
    let t1 = s.janked.anim_ticks;
    if s.janked_pin.is_none() && JANKED_PIN_TICKS - t1 <= TINY {
        s.janked_pin = Some((s.traj_cached, s.traj_orientation));
    }
    if let Some((p, q)) = s.janked_pin {
        (s.traj, s.traj_orientation) = (p, q);
    }
    if let Some(running) = running {
        if s.state == RideState::Riding {
            s.velocity = Vec3::ZERO;
            return (Vec3::ZERO, None, CARVING);
        }
        if running.iter().any(|n| AERIAL_NODES.contains(&n.as_str())) {
            return (air(s, w, input, before, dt), None, AERIAL);
        }
        return (Vec3::ZERO, Some(s.traj_orientation), &[]);
    }
    let clip = t.root_motion.janked.as_ref().filter(|_| s.state != RideState::Riding);
    let Some((d, r)) = clip.and_then(|c| c.delta(t0, t1).ok()) else {
        s.velocity = Vec3::ZERO;
        return (Vec3::ZERO, None, CARVING);
    };
    let q = s.traj_orientation;
    let dp_clip = q * d;
    let w_old = blend_out_weight(t1, dt);
    let (dp, rot) = if w_old > 0.0 {
        let dp_air = air(s, w, input, before, dt);
        let r_air = (q.conjugate() * s.alignment).normalize();
        (dp_air * w_old + dp_clip * (1.0 - w_old), crate::rootmotion::nlerp(r_air, r, 1.0 - w_old))
    } else {
        (dp_clip, r)
    };
    s.velocity = dp / dt;
    (dp, Some((q * rot).normalize()), &[])
}

pub(super) fn get_going_motion(
    s: &mut RiderState,
    w: &World,
    input: &Input,
    flags: (bool, bool),
    before: RideState,
    dt: f32,
    running: Option<&[String]>,
) -> (Vec3, Option<Quat>, &'static [&'static str]) {
    let Some(g) = s.get_going.clone() else { return (Vec3::ZERO, None, &[]) };
    let due = (GET_GOING_END_TICKS - g.ticks).max(0.0);
    if due < dt {
        let airborne = conduit_init(s, w, dt - due);
        if s.janked.janked {
            let air_node = [String::from(AERIAL_NODES[0])];
            let running = running.map(|r| if airborne { &air_node[..] } else { r });
            return janked_motion(s, w, input, before, dt - due, running);
        }
        if airborne {
            return (air(s, w, input, before, dt - due), None, AERIAL);
        }
        return (ride(s, w, input, flags, dt - due), None, CARVING);
    }
    let t1 = g.ticks + dt;
    if running.is_some() {
        s.get_going = Some(GetGoing { ticks: t1, ..g });
        return (Vec3::ZERO, Some(s.traj_orientation), &[]);
    }
    let (d, r) = get_going_delta(&w.tuning.root_motion, &g.items, g.ticks, t1).unwrap_or((Vec3::ZERO, Quat::IDENTITY));
    s.get_going = Some(GetGoing { ticks: t1, ..g });
    let q = s.traj_orientation;
    let w_new = 1.0 - blend_out_weight(t1, dt);
    let (dp, rot) = (q * d * w_new, crate::rootmotion::nlerp(Quat::IDENTITY, r, w_new));
    s.velocity = dp / dt;
    (dp, Some((q * rot).normalize()), &[])
}

pub(super) fn air(s: &mut RiderState, w: &World, input: &Input, before: RideState, dt: f32) -> Vec3 {
    let t = &w.tuning;
    let prediction = s.ballistic.landing();
    s.velocity = s.traj_velocity;
    if let UberRecovery::Hop(h) = s.landing.uber_recovery {
        s.landing.uber_recovery = UberRecovery::Hop(h + dt);
        s.velocity = Vec3::new(0.0, s.velocity.y, 0.0);
        s.first_air_tick = false;
        return integrate_air(s, w, dt);
    }
    let takeoff = |s: &mut RiderState, kind: Takeoff| {
        s.alignment = s.traj_orientation;
        s.air = AirRotation::takeoff(
            &t.aerial,
            &w.props,
            kind,
            s.traj_orientation,
            s.filtered_angular_velocity,
            s.velocity,
            s.ground_normal,
            input.air_stick(s.tricks.classic),
            s.prewind,
            prediction,
        );
    };
    if s.first_air_tick {
        if s.state == RideState::Jump {
            s.velocity += s.jump.launch;
            s.last_launch = s.jump.launch;
            (s.jump_ticks, s.jump_airborne) = (0.0, false);
        }
        let kind =
            if s.state == RideState::Jump && before == RideState::Riding { Takeoff::Jump } else { Takeoff::RollOff };
        takeoff(s, kind);
    }
    s.first_air_tick = false;
    let variant = if s.state == RideState::Jump { &t.aerial.jump } else { &t.aerial.airborne };
    s.jump.allow = variant.enable_air_jumps && s.jump.allow;
    if s.jump.take_trigger() {
        s.velocity += s.jump.launch;
        s.last_launch = s.jump.launch;
        takeoff(s, Takeoff::AirJump);
        s.air_jumped = true;
    }
    if s.state == RideState::Jump {
        s.jump_ticks += dt;
    }
    let dp = integrate_air(s, w, dt);
    s.air.prediction = prediction;
    let gs = &s.grind.steering;
    let inp = AirInputs {
        stick: input.air_stick(s.tricks.classic),
        velocity: s.velocity,
        allow_stance_switch: !(s.tricks.is_tricking || s.tricks.anticipating()),
        grind_steering: gs.active.then_some(gs.time_to_land),
        tutorial_mode: s.wingsuit.force_descent_in_air,
        heli_interact: s.heli.abort_aerial_trick.then_some(s.heli.arrive_left_forwards),
    };
    s.alignment = s.air.step(&t.aerial, &w.props, &inp, s.alignment, &mut s.goofy, dt);
    dp
}

pub(super) fn integrate_air(s: &mut RiderState, w: &World, dt: f32) -> Vec3 {
    let r = &w.props;
    let mut a = drag_accel(r.k_h_air, r.k_v_air, s.velocity, s.grind.wind, dt);
    a.y -= r.g_air.y.abs();
    s.velocity += a * dt;
    s.velocity * dt
}

pub(super) fn transition(s: &mut RiderState, w: &World, stick: Vec2, dt: f32, graph: bool) {
    if s.janked.janked && !s.state.is_wingsuit() {
        return;
    }
    let c = s.close;
    match s.state {
        RideState::Riding => {
            if s.get_going.is_some() {
            } else if (if s.graph.driven { s.graph.carving_stopped } else { s.stopped })
                && get_going(s.reverse_camera.is_some(), stick, s.stick_prev)
            {
                let items = get_going_blend(GET_GOING_ANGLE).unwrap_or_default();
                s.get_going = Some(GetGoing { ticks: 0.0, items });
                (s.stopped, s.nested_stable) = (false, false);
            } else if !c.is_close
                && !s.recovering_from_uber_bail()
                && (if s.graph.driven {
                    s.graph.allow_airborne
                } else {
                    // TODO(guess): the no-graph unit-test path stands in for the four `SEQ_bail_ground_*` actors'
                    // `BLN_AllowAirborne` `BoolGameStateTag` (Time 0, Duration 20, Value false, Mode 1, ResetOnLeave)
                    // with `landing.ticks > 20`; `TagTracker_WindowTest` 0x8319E1D0 keeps a tag active through
                    // `Time + Duration`, and the leave is deferred to the next update (tick 22 in the `ride_sim`
                    // trace). A ride's graph supplies that signal. Settle: a unit-test fixture with the tag set.
                    !s.landing.is_bailing() || s.landing.ticks > crate::landing::BAIL_ALLOW_AIRBORNE_TICKS
                })
                && can_go_airborne_from_grind_trick(s)
            {
                s.state = RideState::Airborne;
                s.first_air_tick = true;
                s.stopped = false;
            } else if !s.landing.tumbling && !s.board_particle.enabled && s.jump.take_trigger() {
                s.state = RideState::Jump;
                s.first_air_tick = true;
                s.stopped = false;
            } else if s.survival.trigger_impact_bail && !s.uber_trigger && !s.board_particle.enabled {
                new_carving(s, w, dt);
                s.reentry = Some(Reentry::ImpactBail);
            } else if s.impact.trigger_impact && !s.uber_trigger && !s.board_particle.enabled && !s.tripped {
                new_carving(s, w, dt);
                s.impact.trigger_impact = false;
                s.reentry = Some(Reentry::Impact);
            } else if let UberRecovery::GettingUp(t) = s.landing.uber_recovery
                && t >= UBER_RECOVER_TRIGGER_CLEAR_TICKS
                && !s.landing.uber_crash
            {
                s.state = RideState::Airborne;
                s.landing.uber_recovery = UberRecovery::Hop(0.0);
                s.first_air_tick = false;
                s.stopped = false;
            }
        }
        RideState::Airborne => {
            let correcting = matches!(s.landing.uber_recovery, UberRecovery::Hop(h) if h < UBER_RECOVER_HOP_TICKS);
            if s.heli_trick.is_some() {
            } else if c.is_very_close && !correcting {
                s.state = RideState::Riding;
                s.wingsuit.blend = None;
            } else if !wingsuit_flow::transition(s, w, correcting) {
                heli_trick::enter(s, graph);
            }
        }
        RideState::Jump => {
            if c.is_very_close && (JUMP_EARLY_WINDOW_TICKS - s.jump_ticks < dt || s.jump_airborne) {
                s.state = RideState::Riding;
            } else if !wingsuit_flow::transition(s, w, false) {
                heli_trick::enter(s, graph);
            }
        }
        RideState::WingsuitEnter | RideState::WingsuitFly | RideState::WingsuitExit => {
            wingsuit_flow::transition(s, w, false);
        }
    }
}

pub(super) fn can_go_airborne_from_grind_trick(s: &RiderState) -> bool {
    !s.tricks.grind.is_grind_trick_enter() && (s.wingsuit.hold || !s.tricks.is_grind_trick())
}

pub const JUMP_EARLY_WINDOW_TICKS: f32 = 12.0;
pub const JUMP_END_WINDOW_TICKS: f32 = 15.0;

pub fn stop_test(
    t: &Tuning,
    speed: f32,
    accel: f32,
    i: &SteeringInputs,
    is_stopped: bool,
    vertical: bool,
    board_press: bool,
) -> bool {
    let c = &t.carving;
    let mag = i.mag.max(i.slip_mag);
    let ang = i.angle.max(i.slip_angle);
    let go = mag >= c.min_stick_magnitude_for_go;
    let pulling_back = go && wrap_pi(ang + PI).abs() <= FRAC_PI_2 + 1e-5;
    !vertical
        && !board_press
        && speed <= c.stop_speed_threshold
        && (pulling_back || accel <= c.stop_accel_threshold || (is_stopped && !go))
}

pub fn get_going(reverse_camera: bool, stick: Vec2, prev: Vec2) -> bool {
    if reverse_camera { omni_get_going_gesture(stick, prev) } else { get_going_gesture(stick, prev) }
}

pub fn get_going_gesture(stick: Vec2, prev: Vec2) -> bool {
    let inside = |s: Vec2| {
        let mag = s.length().min(1.0);
        let a = s.y.atan2(s.x).rem_euclid(std::f32::consts::TAU);
        let back = a > 210f32.to_radians() && a < 330f32.to_radians();
        (0.25..=1.0).contains(&mag) && !back
    };
    inside(stick) && !inside(prev)
}

pub fn omni_get_going_gesture(stick: Vec2, prev: Vec2) -> bool {
    let inside = |s: Vec2| (0.25..=1.0).contains(&s.length().min(1.0));
    inside(stick) && !inside(prev)
}
