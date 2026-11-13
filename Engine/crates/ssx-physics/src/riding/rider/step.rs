use super::*;

pub fn step(s: &mut RiderState, w: &World, input: &Input, dt: f32) {
    step_with(s, w, input, &TrickInput::default(), dt);
}

pub fn step_with(s: &mut RiderState, w: &World, input: &Input, tricks: &TrickInput, dt: f32) {
    step_full(s, w, input, tricks, dt, StepContext::default());
}

pub fn resolve_item_flags(s: &mut RiderState, w: &World) {
    if s.props_resolved {
        return;
    }
    s.wingsuit.enabled = w.survival.item_enabled(crate::character::SLOT_WINGSUIT);
    s.props_resolved = true;
}

#[derive(Default)]
pub struct StepContext<'a> {
    pub graph: Option<&'a mut RiderGraph>,
    pub rewind: Option<crate::rewind::RewindStep>,
    pub flow: GameFlowFlags,
    pub avalanche: Option<&'a mut crate::avalanche::AvalancheSim>,
}

pub fn avalanche_inputs(
    s: &RiderState,
    prev_spray: &Spray,
    prev_vel: Vec3,
    warped: bool,
    dt: f32,
) -> crate::avalanche::SimInputs {
    let (board_pos, board_z) = board_joint(s);
    crate::avalanche::SimInputs {
        dt,
        pos: s.traj,
        vel: s.traj_velocity,
        prev_vel,
        stopped: s.stopped,
        warped,
        in_air: s.in_air(),
        big_landing: s.score.is_big_landing,
        ground_valid: s.contact.is_some(),
        ground_normal: s.ground_normal,
        spray: prev_spray.carving,
        snow_coverage: s.weights[SNOW_COVERAGE],
        descent: s.score.descent,
        ice_spray: prev_spray.ice,
        ice_weight: s.weights[ICE],
        powder_weight: s.weights[crate::surface::POWDER],
        board_pos,
        board_z,
        switches: s.avalanche.switches,
        snow_depth: s.surface.snow_depth,
    }
}

pub fn board_joint(s: &RiderState) -> (Vec3, Vec3) {
    let (_, z) = board_axes(s);
    if let Some((p, _)) = s.board_particle.placement {
        return (p + s.correction_shift, z);
    }
    if s.graph.driven { (s.traj + s.graph.board_offset, z) } else { (s.position, z) }
}

pub(super) fn avalanche_gather(s: &mut RiderState, w: &World, x: &mut StepContext) {
    particle_gather(s, w, x.avalanche.as_deref_mut());
}

pub fn particle_gather(s: &mut RiderState, w: &World, mut sim: Option<&mut crate::avalanche::AvalancheSim>) {
    s.board_particle.rig_op_view();
    let set = s.avalanche.switches.tuning_set;
    if let Some(sim) = sim.as_deref_mut() {
        sim.gather_spray_records(&w.tuning.avalanche);
    }
    s.board_particle.update_record(&w.tuning.board_particle, set, sim.as_deref_mut());
    if let Some(sim) = sim.as_deref_mut() {
        let (board_pos, board_z) = board_joint(s);
        let sa = s.grind.sa.as_deref().unwrap_or(grind::surface_analysis::initial());
        sim.gather(&w.tuning.avalanche, s.traj, board_pos, board_z, Some(sa));
    }
    s.board_particle.spawn_or_clear(sim.as_deref_mut());
    if let Some(sim) = sim {
        s.avalanche.published = sim.published(&w.tuning.avalanche);
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct StepEvents {
    pub rider_dead: Option<RiderDead>,
    pub race_start: bool,
}

pub fn step_full(
    s: &mut RiderState,
    w: &World,
    input: &Input,
    tricks: &TrickInput,
    dt: f32,
    x: StepContext,
) -> StepEvents {
    step_with_trigger(s, w, input, tricks, if tricks.tweak { 1.0 } else { 0.0 }, dt, x)
}

pub fn step_with_trigger(
    s: &mut RiderState,
    w: &World,
    input: &Input,
    tricks: &TrickInput,
    r2: f32,
    dt: f32,
    mut x: StepContext,
) -> StepEvents {
    if dt.abs() <= TINY {
        return StepEvents::default();
    }
    s.audio_tags.clear();
    let t = &w.tuning;
    if let Some(sig) = x.graph.as_ref().and_then(|g| g.signature()) {
        s.tricks.signature = sig;
    }
    resolve_item_flags(s, w);
    s.impacts = 0;
    let in_air = s.in_air();
    let raw_stick = input.stick();
    s.pad_stick = match s.reverse_camera {
        Some(camera) => reverse_camera_stick(raw_stick, camera, s.alignment, s.kin.fwd, s.traj_velocity),
        None => raw_stick,
    };
    let input = &input.with_stick(s.pad_stick);
    let stick = s.pad_stick;
    let l2_last = s.grind.l2_hold;
    let sa_next = grind::dispatch(
        &mut s.grind,
        &grind::DispatchState {
            c: &w.collision,
            r: &w.props,
            traj: s.traj,
            velocity: s.traj_velocity,
            orientation: s.traj_orientation * Quat::from_rotation_y(-FRAC_PI_2),
            in_air,
            ballistic: &s.ballistic,
            l2: l2_last,
            dt,
        },
    );
    (s.grind.button, s.grind.l2_hold) = (s.grind.pad_l2, s.grind.pad_l2_hold);
    (s.wingsuit.hold_prev, s.wingsuit.hold) = (s.wingsuit.hold, s.wingsuit.pad_r1);
    s.grind.stick = crate::steering::stick_angle_and_magnitude(stick);
    let cached = (s.traj_velocity, s.traj_cached, s.traj_cached_prev);
    let start = StepStart { velocity: s.traj_velocity, wipeout: s.landing.tumbling, raw_stick };
    if let Some(ls) = s.start.as_mut() {
        ls.nis_scene_op();
    }
    let a_prev = s.jump.held;
    s.jump.prewind.tick(crate::tricks::PadSample::new(tricks));
    let prewind_release = s.jump.prewind.release && !s.tricks.classic;
    let prev_spray = s.spray;
    s.spray = Spray::default();
    s.survival.init_outputs(&t.survival, &w.props);
    s.survival.shade = match (s.survival.temperature_enabled, s.survival.shade_query, w.survival.sun) {
        (false, ..) => 1.0,
        (true, Some(q), _) => q.clamp(0.0, 1.0),
        (true, None, Some(sun)) => crate::survival::shade_value(&w.collision, s.traj, sun),
        (true, None, None) => 1.0,
    };
    update_prewind(s, w, input, s.jump.prewind.hold, dt);
    update_player_speed(s, in_air);
    let grid = w.collision.boundary.as_ref();
    s.boundary.update(&t.player.out_of_bounds, grid, s.traj_cached, s.traj_velocity, dt);
    let r = &w.props;
    grind::player_update(
        &mut s.grind,
        &grind::PlayerUpdateInputs {
            c: &w.collision,
            steering: &t.grind_steering,
            in_air,
            traj: s.traj,
            velocity: s.traj_velocity,
            k_h_air: r.k_h_air,
            k_v_air: r.k_v_air,
            g_air: r.g_air.y.abs(),
            in_wingsuit: s.wingsuit.signal,
            board_particle: s.board_particle.enabled,
            boundary_wind: s.boundary.wind,
            dt,
        },
    );
    let boost_request = BoostRequest {
        requested: crate::tricks::boost_requested(s.tricks.classic, tricks),
        in_air,
        player_speed: s.player_speed,
        crashed: carving_crashed(s, in_air),
        awesome: s.score.is_awesome,
    };
    s.boost.update(&mut s.score.boost, &w.props, &boost_request, dt);
    s.boost.modifier = if s.tricks.classic { if tricks.x { 1.0 } else { 0.0 } } else { r2.clamp(0.0, 1.0) };
    let a_release = s.jump.held && !input.jump;
    let recovery_input =
        crate::tricks::wipeout_recovery_input(prewind_release, a_release, s.tricks.classic, tricks, s.score.boost);
    s.jump.update(input.jump, prewind_release, s.close.is_very_close, dt, &t.player);
    s.jump.update_compression(input.jump, tricks.right_stick.y, dt, &t.player);
    if s.death.is_alive() && !in_air {
        s.last_safe_ground = s.traj;
        if s.track_started && s.close.is_very_close && s.contact.is_some() {
            s.lowest_ground_height = s.lowest_ground_height.min(s.traj.y);
        }
    }
    let raises_descent = s.death.raises_descent();
    if s.uber_trigger && !s.uber_trigger_seen {
        s.uber_gate_timer = UBER_CRASH_GATE_TICKS;
    }
    s.uber_trigger_seen = s.uber_trigger;
    s.uber_gate_timer -= dt;
    let running = s.track_started && !x.flow.track_finished && !(x.flow.nis_active || s.start.is_some());
    let run_flags = crate::survival::RunFlags {
        started: s.track_started,
        finished: x.flow.track_finished,
        nis: x.flow.nis_active || s.start.is_some(),
    };
    let impact = crate::survival::ImpactInputs {
        this_frame: s.impact.impact_this_frame,
        speed: s.impact.impact_speed,
        pre_velocity: s.impact.pre_impact_velocity,
        uber_bail: s.landing.uber_crash && matches!(s.landing.uber_recovery, UberRecovery::Sliding),
    };
    let (st, ds) = (&t.survival, &w.survival);
    s.survival.update_health(st, &w.props, ds, run_flags, impact, dt);
    s.wingsuit.wear(&t.wingsuit, &w.props, running, dt * (s.survival.lap_count + 1) as f32);
    let tsd = s.death.time_since_death;
    s.survival.ice_axe_wear(st, &w.props, run_flags, s.ice_axe.chop(), dt);
    s.survival.update_temperature(st, &w.props, ds, run_flags, tsd, dt);
    s.survival.update_oxygen(st, &w.props, ds, run_flags, s.player_speed, s.wingsuit.hold, tsd, dt);
    s.survival.update_batteries(st, &w.props, run_flags, dt);
    let race_start = !s.track_started && s.trigger_go;
    if race_start {
        s.force_race_end_timer = t.player.force_race_end_max_time * TICKS_PER_SECOND;
    }
    s.track_started = s.track_started || s.trigger_go;
    finish_force_stop(s, t, x.flow.track_finished, dt);
    s.wingsuit.decay_handles(&t.wingsuit, dt);
    let carve_y = input.carve_stick_y(s.tricks.classic);
    s.stick_y_filtered = carve_y + (s.stick_y_filtered - carve_y) * (-0.11 * dt).exp();
    let ctx = trick_context(s, in_air);
    if is_scoring_wipeout(s, in_air) {
        s.tricks.scene_op_reset(&t.tricks, tricks, &ctx, dt);
    } else {
        s.tricks.scene_op(&t.tricks, tricks, &ctx, dt);
    }
    let flow = GameFlowFlags { nis_active: x.flow.nis_active || s.start.is_some(), ..x.flow };
    let rider_dead = s.death.update_health(s.track_started, &flow, dt);
    let rider_dead = rider_dead.or(s.death.update_vitals(&vitals(s, w), dt));
    (s.wingsuit.race_proper_started, s.wingsuit.force_descent_in_air) =
        (x.flow.race_proper_started, x.flow.force_descent_in_air);
    s.air_jumped = false;
    if let Some(sim) = x.avalanche.as_deref_mut() {
        let i = avalanche_inputs(s, &prev_spray, sim.inputs.vel, false, dt);
        sim.dispatch(&t.avalanche, i, &w.collision);
    }
    s.board_particle.dispatch(&w.collision, s.traj, dt);
    let handover = match &s.start {
        Some(ls) if ls.trigger_stop => {
            s.start = None;
            true
        }
        Some(_) => {
            let go = GoInputs {
                a_press: input.jump && !a_prev,
                right_stick: tricks.right_stick,
                get_going: get_going(s.reverse_camera.is_some(), raw_stick, s.stick_prev),
            };
            return level_start_step(s, w, (input, tricks, &go), &start, dt, x, sa_next, rider_dead);
        }
        None => false,
    };
    if s.graph.driven
        && let UberRecovery::GettingUp(_) = s.landing.uber_recovery
    {
        s.landing.uber_recovery = UberRecovery::GettingUp(s.graph.recover_ticks);
        if s.graph.recover_ticks >= UBER_RECOVER_TRIGGER_CLEAR_TICKS {
            s.landing.uber_crash = false;
        }
    }
    let enter_gameplay = handover || x.rewind.is_some_and(|r| r.terminating);
    if enter_gameplay {
        s.gameplay = Some(GameplayNode::new());
        s.ice_axe.enter_gameplay();
    }
    let (before, goofy_before) = (s.state, s.goofy);
    s.reentry = None;
    if enter_gameplay {
        conduit_init(s, w, dt);
    } else {
        transition(s, w, raw_stick, dt, x.graph.is_some());
    }
    if s.state == RideState::Airborne && before != RideState::Airborne && !s.janked.janked && s.heli_trick.is_none() {
        let correcting = matches!(s.landing.uber_recovery, UberRecovery::Hop(h) if h < UBER_RECOVER_HOP_TICKS);
        wingsuit_flow::transition(s, w, correcting);
    }
    s.on_ground = s.state == RideState::Riding;
    if !enter_gameplay && !s.janked.janked && before != RideState::Riding && s.state == RideState::Riding {
        touchdown(s, w, dt);
    }
    let running = x.graph.as_deref().map(|g| g.running_nodes(dt));
    let (dp, root, simulated) = if s.heli_trick.is_some() {
        heli_trick::motion(s, w, input, before, dt, running.as_deref())
    } else if s.janked.janked && !s.state.is_wingsuit() && s.get_going.is_none() {
        janked_motion(s, w, input, before, dt, running.as_deref())
    } else if s.state.is_wingsuit() || (s.state == RideState::Airborne && s.wingsuit.blend.is_some()) {
        let (dp, root) = wingsuit_flow::motion(s, w, input, before, (recovery_input, x.flow.track_finished), dt);
        (dp, root, wingsuit_flow::WINGSUIT_CLASSES)
    } else if s.get_going.is_some() {
        get_going_motion(s, w, input, (recovery_input, x.flow.track_finished), before, dt, running.as_deref())
    } else {
        match s.state {
            RideState::Riding => (ride(s, w, input, (recovery_input, x.flow.track_finished), dt), None, CARVING),
            RideState::Airborne | RideState::Jump => (air(s, w, input, before, dt), None, AERIAL),
            RideState::WingsuitEnter | RideState::WingsuitFly | RideState::WingsuitExit => (Vec3::ZERO, None, AERIAL),
        }
    };
    if s.state == RideState::Jump
        && (s.goofy != goofy_before && before == RideState::Jump || JUMP_END_WINDOW_TICKS - (s.jump_ticks - dt) < dt)
    {
        s.jump_airborne = true;
    }
    if !s.janked.janked
        && let Some(left) = air_tricks_ticks(s, dt)
    {
        let ctx = TrickContext { air_jump: s.air_jumped, ..trick_context(s, true) };
        s.tricks.flow(&ctx, left);
    }
    if s.state == RideState::Riding && !s.janked.janked && s.get_going.is_none() {
        let g = &mut s.tricks.grind;
        if !g.is_grind_trick() && g.starting && !s.stopped && !s.landing.bail_on_landing && !s.landing.uber_crash {
            g.enter();
        }
        let double = s.tricks.trick_type.value() > 22;
        let pad_end = s.tricks.pad_grab_end();
        let ev = s.tricks.grind.flow(s.grind.is_grinding(), double, pad_end, dt);
        if ev.grab_reset {
            s.tricks.grab_reset();
        }
        if ev.ground_trick_reset {
            s.tricks.ground_trick_reset();
            s.nested_stable = true;
        }
    } else if s.tricks.grind.abort() {
        s.tricks.ground_trick_reset();
    }
    s.uber_trigger = if s.landing.uber_crash {
        true
    } else {
        s.in_air() && matches!(s.tricks.phase, TrickPhase::Enter | TrickPhase::Loop)
    };
    if s.on_ground {
        s.ride_ticks += dt;
    } else {
        s.air_ticks += dt;
    }
    let mut audio_tags = Vec::new();
    let (dp, root) = match x.graph.as_deref_mut() {
        Some(g) => {
            let physics = step_physics(s.traj_orientation, dp, rig_op_rotation(s, root), simulated);
            g.update_controllers(s, input, tricks, &start, dt, Some(physics));
            audio_tags = g.take_audio_tags();
            resolve_root_motion(s, g.root_motion(), (dp, root), dt)
        }
        None => (dp, root),
    };
    s.audio_tags.extend(audio_tags);
    if let Some(g) = s.gameplay.as_mut() {
        if g.tags.update(g.ticks) {
            let mut saved = false;
            for (_, e) in g.tags.process() {
                TRIGGER_GO_TAG.on(e, &mut s.trigger_go, &mut saved);
            }
        }
        g.ticks += dt;
        s.ice_axe.layers_update(w.survival.slots[crate::character::SLOT_ICE_AXE], dt);
    }
    let node = match s.state {
        RideState::WingsuitEnter => Some(crate::wingsuit::WingsuitNode::Enter),
        RideState::WingsuitFly => Some(crate::wingsuit::WingsuitNode::Fly),
        RideState::WingsuitExit => Some(crate::wingsuit::WingsuitNode::Exit),
        _ => None,
    };
    s.wingsuit.handle_tags(node, s.wingsuit.ticks);
    rig_ops(s, w, dp, root, dt);
    let was_recover = s.graph.uber_recover;
    s.graph = match x.graph.as_deref_mut() {
        Some(g) => g.finish(s),
        None => GraphFeedback::default(),
    };
    if s.graph.driven && s.graph.uber_recover {
        if !was_recover {
            s.board_particle.stay_dead = should_stay_dead_once_tumbled(s);
        }
        s.board_particle.recover_tag(s.graph.recover_ticks);
    }
    let mut ctx = scoring_context(s, w, cached);
    ctx.death_by_death_plane = !raises_descent;
    ctx.force_descent_in_air = x.flow.force_descent_in_air;
    ctx.track_finished = x.flow.track_finished;
    if let Some(r) = &x.rewind {
        r.apply_to(&mut ctx);
    }
    score(s, w, input, &ctx, dt);
    spq(s, w, dt, Some(()));
    s.traj_cached_prev = s.traj_cached;
    s.traj_cached = s.traj;
    avalanche_gather(s, w, &mut x);
    s.grind.sa = Some(sa_next);
    s.heading = (s.alignment * Vec3::X).try_normalize().unwrap_or(s.heading);
    s.position = s.board_point();
    s.stick_prev = raw_stick;
    StepEvents { rider_dead, race_start }
}

pub fn vitals(s: &RiderState, w: &World) -> crate::death::Vitals {
    crate::death::Vitals {
        health: s.survival.health,
        temperature: s.survival.temperature,
        oxygen: s.survival.oxygen,
        death_permanent: w.survival.death_permanent,
        in_air: s.in_air(),
    }
}

pub fn finish_force_stop(s: &mut RiderState, t: &Tuning, track_finished: bool, dt: f32) {
    s.force_stop = track_finished;
    if track_finished {
        s.allow_race_end =
            s.stopped || s.player_speed < t.player.stopped_speed_threshold || s.force_race_end_timer < 0.0;
        s.force_race_end_timer -= dt;
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn level_start_step(
    s: &mut RiderState,
    w: &World,
    (input, tricks, go): (&Input, &TrickInput, &GoInputs),
    start: &StepStart,
    dt: f32,
    mut x: StepContext,
    sa_next: std::sync::Arc<grind::surface_analysis::SurfaceAnalysis>,
    rider_dead: Option<RiderDead>,
) -> StepEvents {
    let rm = &w.tuning.root_motion;
    s.audio_tags.clear();
    let Some(ls) = s.start.as_mut() else { return StepEvents { rider_dead, ..StepEvents::default() } };
    ls.controller_update(dt, || crate::levelstart::go_pick(rm.random_seed));
    let (phase, idle_pose) = (ls.phase, ls.idle_pose(rm));
    let tags: Vec<_> = ls
        .intro
        .as_ref()
        .map_or_else(Vec::new, |i| i.game_state_events().into_iter().map(|(t, e)| (t.clone(), e)).collect());
    for (t, e) in tags {
        let w = match e {
            crate::tags::TagEvent::Apply(w) | crate::tags::TagEvent::Leave(w) => w,
            crate::tags::TagEvent::Enter => 0.0,
        };
        let ws = &mut s.wingsuit;
        match (t.state.as_str(), t.float) {
            ("BLN_State_Wingsuit", false) => {
                let reset = t.reset_on_leave.then_some(t.default != 0.0);
                let tag = crate::tags::BoolTag { value: t.value != 0.0, mode: t.mode, reset };
                let mut saved = ws.signal;
                tag.on(e, &mut ws.signal, &mut saved);
            }
            ("FLT_Wingsuit_BlendWeight", true) => {
                let spline = crate::ice_axe::tag_spline(&t.spline, w);
                let reset = t.reset_on_leave.then_some(t.default);
                let tag = crate::tags::FloatTag { value: t.value, mode: t.mode, reset, spline };
                tag.on(e, &mut ws.handle_weight, &mut ws.handle_saved);
            }
            _ => {}
        }
    }
    let q = s.traj_orientation;
    let (dp, root) = match phase {
        Phase::Idle { .. } => {
            if let Some((p, t)) = idle_pose {
                (s.traj, s.traj_orientation) = (p, t);
            }
            (Vec3::ZERO, Some(s.traj_orientation))
        }
        Phase::Go { ticks, clip } => {
            match rm.go.as_ref().and_then(|g| g[clip].delta((ticks - dt).max(0.0), ticks).ok()) {
                Some((d, r)) => (q * d, Some((q * r).normalize())),
                None => (Vec3::ZERO, Some(q)),
            }
        }
    };
    let mut audio_tags = Vec::new();
    let (dp, root) = match x.graph.as_deref_mut() {
        Some(g) => {
            g.update_controllers(s, input, tricks, start, dt, None);
            audio_tags = g.take_audio_tags();
            if matches!(phase, Phase::Idle { .. }) {
                (dp, root)
            } else {
                resolve_root_motion(s, g.root_motion(), (dp, root), dt)
            }
        }
        None => (dp, root),
    };
    s.audio_tags.extend(audio_tags);
    rig_ops(s, w, dp, root, dt);
    s.alignment = s.traj_orientation;
    s.graph = match x.graph.as_deref_mut() {
        Some(g) => g.finish(s),
        None => GraphFeedback::default(),
    };
    if let Some(ls) = s.start.as_mut() {
        ls.tags(go);
    }
    spq(s, w, dt, None);
    s.traj_cached_prev = s.traj_cached;
    s.traj_cached = s.traj;
    avalanche_gather(s, w, &mut x);
    s.grind.sa = Some(sa_next);
    s.heading = (s.traj_orientation * Vec3::X).try_normalize().unwrap_or(s.heading);
    s.position = s.board_point();
    s.stick_prev = start.raw_stick;
    StepEvents { rider_dead, ..StepEvents::default() }
}
