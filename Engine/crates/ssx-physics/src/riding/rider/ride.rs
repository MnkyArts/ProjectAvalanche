use super::*;

pub(super) fn ride(s: &mut RiderState, w: &World, input: &Input, flags: (bool, bool), dt: f32) -> Vec3 {
    let (recovery_input, level_complete) = flags;
    let (t, c) = (&w.tuning, &w.tuning.carving);
    let prev_request = s.request_stop;
    if s.graph.driven {
        s.stopped = s.graph.carving_stopped;
    } else if !s.stopped && !s.landing.uber_crash && !s.tricks.grind.is_grind_trick() {
        let bailing = s.landing.bail_on_landing;
        if !s.nested_stable && !bailing && s.landing.stable {
            s.nested_stable = true;
        }
        if s.nested_stable {
            if s.request_stop {
                s.stopped = true;
            } else if !s.landing.stable || bailing {
                s.nested_stable = false;
            }
        }
    }
    let gframe = grind::carving_frame(&mut s.grind, s.position, s.contact.map(|c| (c.point, c.normal)), s.velocity, dt);
    let has_contact = gframe.contact.is_some();
    let n = gframe.n;
    let stick = input.carve_stick(s.tricks.classic);
    let uber = s.landing.uber_crash || s.board_particle.enabled;
    let dead_on_ground = matches!(s.landing.uber_recovery, UberRecovery::GettingUp(_)) && uber;
    let is_dead = s.carving.is_dead;
    let vertical = n.y < c.vertical_surface_angle_threshold.cos();
    let has_prewind = super::spq::has_prewind_input(input, s.jump.prewind.hold, s.tricks.classic);
    let damp = if c.prewind_dampens_carving && has_prewind {
        let curve = if s.tricks.classic {
            &c.prewind_carve_input_classic_dampening_curve
        } else {
            &c.prewind_carve_input_dampening_curve
        };
        curve.eval(s.prewind).min(c.prewind_carve_input_dampening_curve.eval(s.prewind_analog))
    } else {
        1.0
    };
    let speed = s.velocity.length();
    let can_brake = s.landing.is_upright(&t.balance);
    let rock_effects = t.survival.riding_on_rock_curve.eval(s.carving.ticks_on_rock * 0.0625);
    let pt = &t.board_press;
    let press_in = s.tricks.press.inputs();
    let (gstate, last_press) = (s.grind.carving.state, s.press.is_board_press);
    let pressing = crate::press::board_press(
        pt,
        press_in,
        gstate,
        speed,
        last_press,
        s.landing.backslide,
        s.force_stop,
        s.landing.tumbling,
    );
    (s.press.inputs, s.press.pressing) = (press_in, pressing);
    let gt = &t.grind;
    {
        let gc = &mut s.grind.carving;
        gc.blend = grind::blend_update(gc.blend, gc.state != 0, gt.blend_value_gain, dt);
        s.press.blend = crate::press::blend_update(s.press.blend, pressing, pt.blend_gain, dt);
        gc.grind_or_press_blend = crate::press::grind_or_press(gc.blend, s.press.blend);
    }
    let or_press = s.grind.carving.grind_or_press_blend;
    let curves = (&gt.max_lean_input_curve, &pt.max_lean_input_curve);
    let ia = &c.ice_axe;
    let ice_blend = s.ice_axe.blend(ia, s.weights[ICE]);
    let max_in = crate::steering::max_lean_input_angle(c, curves, speed, or_press, pressing, ice_blend);
    let force = s.force_stop.then_some(s.press.force_stop_mag);
    let (zero, press) = ((uber, is_dead), (or_press, pressing));
    s.inputs = crate::steering::read_inputs(c, stick, max_in, damp, can_brake, zero, force, press, ice_blend);
    let eff = s.survival.ice_axe_effectiveness;
    let (lean_prev, angle, dig) = (s.steering.lean_norm, s.inputs.angle, w.props.ice_axe_dig_modifier);
    let ice_frame = s.ice_axe.read_inputs(ia, eff, s.weights[ICE], lean_prev, angle, dig, dt);
    let at = w.tuning.avalanche.carving(s.avalanche.switches.tuning_set);
    let av_on = !s.boost.is_boosting && !level_complete && s.grind.carving.state != 2;
    let av = s.avalanche.read_inputs(at, av_on, s.carving.ride_ticks);
    let alive = s.death.is_alive();
    let latch = (speed < c.stopped_threshold
        && (s.force_stop
            || (is_dead && !vertical)
            || (uber && !vertical && s.carving.ride_ticks >= c.uber_crash_ticks_to_full_friction)))
        || s.janked.janked;
    let stopped = s.stopped || latch;
    let mut q = ground_frame(s.alignment, n);
    update_concavity(&mut s.steering, c, has_contact, s.filtered_velocity, n, dt);
    let (lean_in, brake_in) = (s.inputs.lean_in, s.inputs.brake);
    update_slip_state(&mut s.steering, c, &mut s.alignment, &mut q, n, s.velocity, lean_in, brake_in, stopped, dt);
    s.kin = kinematics(&mut s.steering, c, q, s.alignment, s.velocity, dt, ice_blend);
    s.lean = s.kin.lean;
    let comp = snow_compression(c, &s.surface, s.carving.distance_carved);
    let h = gframe.contact.map_or(0.0, |p| (p - s.traj).dot(n));
    let f210 = h + comp;
    let f218 = c.on_ground_distance_threshold - (s.surface.snow_depth - comp);
    let blends = HeightBlends {
        tuck: tuck_blend(s),
        pre_jump: if input.jump { s.jump.a_hold_filtered.clamp(0.0, 1.0) } else { 0.0 },
        boost: s.boost.blend.clamp(0.0, 1.0),
    };
    let ground_trick = s.tricks.is_grind_trick();
    let forced = uber || is_dead || s.janked.janked || ground_trick;
    let kin30 = riding_height_target(
        c,
        blends,
        s.kin.lean,
        s.landing.stable_ticks,
        s.carving.initialized,
        s.traj_height_dof(),
        forced,
    );
    let kin34 = f210 - kin30;
    if !s.carving.have_made_first_contact {
        grind::landing_check(&mut s.grind.carving, &w.collision, s.contact.map(|c| c.point), s.traj);
    }
    let q_traj =
        if s.graph.driven { s.graph.traj_rotation } else { s.traj_orientation * Quat::from_rotation_y(-FRAC_PI_2) };
    let l = q_traj.conjugate() * s.velocity;
    let vel_angle = if l.x * l.x + l.z * l.z >= c.speed_threshold * c.speed_threshold {
        l.z.atan2(l.x)
    } else {
        s.grind.carving.board_angle
    };
    grind::update_board_angle(&mut s.grind.carving, q_traj, s.velocity, c.speed_threshold, c.max_spin_vel, dt);
    s.goofy = outputs::goofy_update(vel_angle, s.goofy, c.goofy_hysteresis_angle);
    let (raw_angle, raw_mag) = crate::steering::stick_angle_and_magnitude(stick);
    let (grind_lateral, grind_press) =
        grind::grind_input(gt.input_dead_zone_angle, -raw_angle, raw_mag, press_in.nose, press_in.tail);
    {
        let gc = &mut s.grind.carving;
        grind::press_update(&mut gc.press, gt, gc.board_angle, gc.grind_or_press_blend, grind_press, dt);
    }
    let first_contact = !s.carving.have_made_first_contact && has_contact && kin34 >= f218;
    if first_contact {
        s.carving.have_made_first_contact = true;
    }
    let brake = brake_factor(
        t,
        &t.balance.tumble_friction_curve,
        &BrakeInputs {
            slip_angle: s.inputs.slip_angle,
            slip_mag: s.inputs.slip_mag,
            pitch: s.landing.pitch,
            dead_on_ground,
            board_press: pressing,
            uber,
            grind_or_rock: s.grind.carving.state != 0 || s.landing.bailed_because_of_rock,
            vertical,
            ride_ticks: s.carving.ride_ticks,
            can_brake,
        },
    );
    let trip_in = crate::trip::TripInputs {
        allowed: w.survival.tripping,
        sa: s.grind.sa.as_deref(),
        traj: s.traj,
        velocity: s.velocity,
        speed,
        // TODO(guess): without a graph (unit tests) the rock-bail node is not emulated; settle: run the graph (the game always does).
        in_rock_bail: s.graph.in_rock_bail,
        grinding: s.grind.carving.state != 0,
        dt,
    };
    s.trip_walk = crate::trip::trip_walk(&t.trip, &trip_in);
    let trip = crate::trip::trip_test(&t.trip, &trip_in);
    (s.tripped, s.trip) = (trip.is_some(), trip);
    let l = &s.landing;
    let gc = s.grind.carving;
    let mut out = if let Some(tr) = trip {
        s.survival.one_off_damage += tr.damage;
        crate::ground::GroundOutput { dp: tr.dp, v: tr.v, a: tr.a, ..Default::default() }
    } else {
        integrate_ground(&GroundInputs {
            t,
            r: &w.props,
            sp: &s.surface,
            st: &s.carving,
            balance: BalanceView {
                pitch_balance: l.pitch_balance,
                pitch_balance_vel: l.pitch_balance_vel,
                bail_on_landing: l.bail_on_landing,
                pitch: l.pitch,
                stable: l.stable,
            },
            q,
            n,
            contact: has_contact,
            v_world: s.velocity,
            kin: &s.kin,
            kin30,
            kin34,
            f210,
            f218,
            stick_y: stick.y,
            brake,
            slip_dig: s.steering.slip_dig_angle,
            uber,
            board_press: pressing,
            stopped,
            height_above_lowest: s.traj.y - s.lowest_ground_height,
            boost: BoostThrust {
                is_boosting: s.boost.is_boosting,
                modifier: s.boost.modifier,
                accel_low: s.boost.accel_low,
                accel_high: s.boost.accel_high,
                max_speed: s.boost.max_speed,
            },
            grind: crate::ground::GrindGround {
                state: gc.state,
                blend: gc.blend,
                or_press_blend: gc.grind_or_press_blend,
                thrust: gt.thrust_curve.eval(speed),
            },
            wind: s.grind.wind * gt.wind_modifier + s.avalanche.wind(at, &av, alive, s.carving.air_time, n),
            ice_axe: ice_frame,
            avalanche_drag: crate::avalanche::RiderAvalanche::drag_factor(at, &av, alive),
            avalanche_friction: s.avalanche.friction_factor(at, &av, alive, s.carving.air_time),
            dt,
        })
    };
    let button = s.grind.button;
    let sa = s.grind.sa.clone();
    s.grind.debug = None;
    let sa = sa.as_deref().unwrap_or(grind::surface_analysis::initial());
    if trip.is_none() {
        let mi = grind::state::MachineInputs {
            c: &w.collision,
            t: gt,
            sa,
            dt,
            traj: s.traj,
            joint: s.position,
            n,
            f210,
            path: s.grind.path,
            button,
            stick_dir: grind::stick_direction(s.inputs.angle, s.inputs.mag, s.velocity, n),
            tail_dir: Some(s.grind.tail_dir),
            depth: grind::RAIL_MAX_DEPTH,
            follow_buried: grind::FOLLOW_BURIED_RAILS,
            wiping_out: s.landing.tumbling,
        };
        let first = s.carving.have_made_first_contact;
        let gate = out.v.abs().max_element() > 1.192_092_9e-7 && (first || s.grind.carving.auto_grind);
        s.grind.debug = grind::state::grind_update(&mut s.grind.carving, &mi, first, &mut out.dp, &mut out.v);
        if gate && s.grind.carving.state == 0 {
            let steered =
                collision_steering::collision_steering(&w.collision, &t.carving, s.traj, out.v, n, mi.stick_dir, dt);
            s.carving.collision_anticipation_side = 0.0;
            if let Some(st) = steered {
                (out.dp, out.v) = (st.dp, st.v);
                s.carving.collision_anticipation_side = st.side;
            }
        }
    }
    s.grind.carving.prev_input = button;
    s.steering.normal_force = out.normal_force;
    s.landing.pitch_balance_vel = out.pitch_balance_vel;
    s.carving.use_leg_spring = out.use_leg_spring;
    if let Some(u) = out.is_uphill {
        s.is_uphill = u;
    }
    let dpl = q.conjugate() * out.dp;
    let floor = has_contact.then_some(f210 - dpl.y);
    s.carving.riding_height = if ground_trick {
        kin30
    } else {
        update_riding_height(c, s.carving.riding_height, kin30, floor, !s.carving.initialized, dt)
    };
    s.depth_below_snow = ((s.surface.snow_depth - comp) + f210) - s.carving.riding_height;
    s.carving.chatter_height = s.surface.board_chatter_height;
    if s.carving.use_leg_spring {
        s.carving.distance_carved += Vec2::new(dpl.x, dpl.z).length();
    }
    s.carving.uls_ticks = if s.carving.use_leg_spring { s.carving.uls_ticks + dt } else { 0.0 };
    let speed_out = out.v.length();
    rock_riding_update(s, w, rock_effects, dt);
    let recovery = s.landing.recovery_input(&t.balance, s.kin.fwd, stick.y);
    s.survival.carving_damage += s.avalanche.balance(at, &mut s.landing.bail_on_landing, level_complete, recovery);
    let spared = s.avalanche.bailed_because_of_avalanche || is_dead || !s.death.is_alive();
    s.landing.landing_bail_limit(spared, dt);
    s.landing.balance_update(
        &t.balance,
        s.kin.fwd,
        stick.y,
        speed_out,
        recovery_input,
        s.carving.use_leg_spring,
        uber,
        s.grind.carving.state == 2,
        is_dead,
        level_complete,
        pressing,
        dt,
    );
    let can_brake_now = s.landing.is_upright(&t.balance);
    crate::press::force_stop_update(&mut s.press, pt.force_stop_spring_stiffness, can_brake_now, level_complete, dt);
    if !s.death.is_alive() && !level_complete {
        s.carving.is_dead = true;
    }
    let grinding = s.grind.carving.state != 0;
    let grind_spin = grinding.then_some((gt, grind_lateral));
    let dq = pid_step(&mut s.steering, c, &s.kin, &s.inputs, stopped, grind_spin, dt, ice_blend);
    s.ice_axe.chop_update(ia, s.steering.lean_norm);
    s.nose_tail = outputs::nose_tail_factor(n, q_traj, s.goofy);
    s.board_velocity_angle = outputs::angle_between_board_and_velocity(board_axes(s).1, out.v, n);
    {
        let gc = &mut s.grind.carving;
        grind::veering_update(
            &mut gc.veering_angle,
            &mut gc.veering_vel,
            gt.veering_angle_stiffness,
            n,
            s.velocity,
            out.v,
            dt,
        );
        let x = if gc.state == 2 { s.steering.spin_pid.x } else { 0.0 };
        (gc.spin, gc.spin_vel) = (x, x * c.max_spin_vel);
        match gc.state {
            0 => gc.is_grinding = false,
            2 => gc.is_grinding = true,
            _ => {}
        }
    }
    let (board_x, board_z) = board_axes(s);
    s.spray = spray_filter(
        &mut s.carving,
        &SprayInputs {
            contact: has_contact,
            first_contact,
            stopped: s.stopped,
            board_x,
            board_z,
            n,
            velocity: s.velocity,
            deceleration: out.deceleration,
            new_velocity: out.v,
            normal_force: out.normal_force,
            snow_coverage: s.weights[SNOW_COVERAGE],
            ice_coverage: s.weights[ICE],
            responsiveness: c.carving_spray_filter_responsiveness,
            dt,
        },
    );
    s.alignment = apply_rotation(s.alignment, q, dq);
    if s.carving.have_made_first_contact {
        s.carving.on_ground_ticks += dt;
    }
    s.carving.ride_ticks += dt;
    s.avalanche.end_of_update(s.landing.bail_on_landing, dt);
    s.carving.initialized = true;
    let request_stop = stop_test(t, speed_out, out.a.length(), &s.inputs, s.stopped, vertical, pressing);
    s.request_stop = request_stop;
    s.press.is_board_press = crate::press::is_board_press_output(pressing, s.grind.carving.grind_or_press_blend);
    let board = (board_joint(s).0, board_rotation(board_axes(s)));
    s.board_particle.carving_update(uber, dead_on_ground, board, out.v);
    match s.landing.uber_recovery {
        _ if !uber && !dead_on_ground => {}
        UberRecovery::Sliding if s.graph.driven => {
            if s.graph.uber_recover {
                s.landing.uber_recovery = UberRecovery::GettingUp(s.graph.recover_ticks);
            }
        }
        UberRecovery::Sliding => {
            if prev_request {
                s.landing.uber_recovery = UberRecovery::GettingUp(0.0);
                s.board_particle.stay_dead = should_stay_dead_once_tumbled(s);
            }
        }
        UberRecovery::GettingUp(_) if s.graph.driven => {}
        UberRecovery::GettingUp(r) => {
            let r = r + dt;
            s.landing.uber_recovery = UberRecovery::GettingUp(r);
            s.board_particle.recover_tag(r);
            if r >= UBER_RECOVER_TRIGGER_CLEAR_TICKS {
                s.landing.uber_crash = false;
            }
        }
        UberRecovery::Hop(_) => {}
    }
    s.velocity = out.v;
    out.dp
}
