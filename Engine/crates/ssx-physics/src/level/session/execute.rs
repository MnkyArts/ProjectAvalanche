use super::*;

impl Session {
    pub(super) fn frame_head(
        &mut self,
        s: &mut RiderState,
        w: &World,
        race_start: bool,
        dt: f32,
        first: bool,
    ) -> survive::SurviveStep {
        let sim_s = dt / TICKS_PER_SECOND;
        let dt_s = LOOP_STEP_S;
        if let Some(t) = self.tutorial.as_mut() {
            t.update(dt_s);
        }
        self.tracker.update(&w.triggers, s.traj);
        self.racer_end = [false; crate::event::MAX_RACERS];
        if let Some(b) = self.bots.as_ref() {
            for (t, r) in self.racer_end.iter_mut().zip(b.racers(w)) {
                *t = r.is_some_and(|r| r.touching_end);
            }
        }
        if self.heli_jump.is_none() || first {
            let nis_active = self.flow.nis_active || s.start.is_some();
            self.update_objectives(s, w, nis_active, dt_s);
        }
        let effects = self.new_effects().to_vec();
        let survive = match self.survive.as_mut() {
            Some(l) => l.update(s, w, &mut self.event, &effects, dt_s),
            None => survive::SurviveStep::None,
        };
        if let Some(b) = self.bots.as_mut() {
            b.step(s, race_start, sim_s);
        }
        survive
    }

    #[allow(clippy::too_many_arguments)]
    pub fn execute(
        &mut self,
        s: &mut RiderState,
        w: &World,
        input: &Input,
        tricks: &TrickInput,
        r2: f32,
        pad: &RewindPad,
        graph: Option<&mut RiderGraph>,
    ) -> Execution {
        self.execute_with(s, w, input, tricks, r2, pad, graph, &mut |_, _, _| {})
    }

    #[allow(clippy::too_many_arguments)]
    pub fn execute_with(
        &mut self,
        s: &mut RiderState,
        w: &World,
        input: &Input,
        tricks: &TrickInput,
        r2: f32,
        pad: &RewindPad,
        mut graph: Option<&mut RiderGraph>,
        actors: &mut dyn FnMut(&mut Session, &mut RiderState, f32),
    ) -> Execution {
        self.load_objectives(w);
        self.effects_fresh = true;
        let dt = DT_TICKS * self.dilation;
        let mut survive = survive::SurviveStep::None;
        if std::mem::take(&mut self.head_pending) {
            crate::rider::resolve_item_flags(s, w);
            survive = self.frame_head(s, w, false, dt, true);
        }
        let released = (Input::default(), TrickInput::default(), RewindPad::default());
        let (input, tricks, r2, pad) =
            if self.pads_released { (&released.0, &released.1, 0.0, &released.2) } else { (input, tricks, r2, pad) };
        if self.pads_released {
            s.grind.release_l2();
            s.wingsuit.pad_r1 = false;
        }
        let was_alive = s.death.is_alive();
        let in_gameplay = s.gameplay.is_some();
        let mut ctx = scoring_context(s, w, (s.traj_velocity, s.traj_cached, s.traj_cached_prev));
        ctx.track_finished = self.flow.track_finished;
        let mut before: DeathState = s.death;
        let flow = GameFlowFlags { nis_active: self.flow.nis_active || s.start.is_some(), ..self.flow };
        let puppet_message = before.update_health(s.track_started, &flow, dt);
        let puppet_message = puppet_message.or(before.update_vitals(&crate::rider::vitals(s, w), dt));
        self.flow.race_proper_started = self.buffer.race_proper_started();
        self.flow.force_descent_in_air = self.event.setup.force_descent_in_air;
        let r = self.manager.step(&mut self.buffer, s, w, pad, &ctx, dt);
        let (k, rewind) = (&self.manager.tuning.time_dilation, self.buffer.time_dilation());
        self.executed_dilation = (self.time_dilation.state, self.dilation);
        self.dilation = self.time_dilation.update(k, s.heli.helicopter_interaction, rewind, DT_TICKS);
        self.rewind_backwards = r.active && r.speed <= 1.525_878_9e-5;
        let (rider_dead, race_start) = if r.physics {
            let x = StepContext {
                graph: graph.as_deref_mut(),
                rewind: Some(r),
                flow: self.flow,
                avalanche: Some(&mut self.avalanche),
            };
            let ev = step_with_trigger(s, w, input, tricks, r2, dt, x);
            (ev.rider_dead, ev.race_start)
        } else {
            let prev_vel = self.avalanche.inputs.vel;
            let i = crate::rider::avalanche_inputs(s, &s.spray, prev_vel, false, dt);
            self.avalanche.dispatch(&w.tuning.avalanche, i, &w.collision);
            s.board_particle.dispatch(&w.collision, s.traj, dt);
            if let Some(g) = graph.as_deref_mut() {
                s.graph = g.rewind_update(s, self.buffer.playback_slots(), dt);
            }
            crate::rider::particle_gather(s, w, Some(&mut self.avalanche));
            (puppet_message, false)
        };
        if let Some(c) = self.controllers.as_mut() {
            c.step_bodies(dt / crate::TICKS_PER_SECOND, &w.collision);
        }
        if let Some(r) = s.score.trick_update {
            self.stats_record = r;
        }
        for m in s.score.flushed.iter().flatten() {
            self.stats.banks_flushed(m, &self.stats_record);
            if let crate::tricks::TrickType::Added(a) = self.stats_record.trick_id
                && let Some(Some(id)) = w.tuning.added_trick_ids.get(usize::from(a.slot))
            {
                self.stats.added_trick_flushed(&self.stats_record, id);
            }
        }
        let event_type = self.event.setup.event_type.map(|t| t as i32);
        if let Some(p) = s.score.base_points {
            self.stats.base_points_awarded(p, event_type);
        }
        if let Some(m) = s.score.combo_ended {
            self.stats.combo_ended(&m, event_type);
        }
        if let Some(m) = s.score.track_ended {
            self.stats.track_ended(&m);
        }
        let intro = s.start.as_ref().and_then(|l| l.intro.as_ref());
        self.nis_events.update(intro);
        if let Some(o) = self.objectives.as_mut() {
            for e in &self.nis_events.posted {
                o.runner.events.post(crate::objectives::Name::new(e), None);
            }
        }
        if !in_gameplay && s.gameplay.is_some() {
            self.event.on_entered_gameplay();
            if let Some(o) = self.objectives.as_mut() {
                o.runner.events.on_race_started();
            }
        }
        if race_start {
            self.event.on_trigger_race_start(&mut self.timer);
            self.flow.track_finished = false;
            if let Some(t) = self.tombstones.as_mut() {
                t.on_race_start();
            }
        }
        let f = &self.flow;
        let flags = RecordFlags {
            track_started: s.track_started,
            track_finished: f.track_finished,
            nis_active: f.nis_active || s.start.is_some(),
            ..RecordFlags::default()
        };
        let recorded = self.buffer.record_with(s, &flags);
        if let (Some(slot), Some(g)) = (recorded, graph) {
            g.record_pose(slot);
        }
        if let Some(m) = &rider_dead {
            let c = &self.manager.count;
            self.timer.on_rider_dead(m, c.enabled, c.rewinds_left, self.ui_rewind_count > 0);
        }
        self.last_sim_dt_s = dt / TICKS_PER_SECOND;
        let event = self.score_manager(s, w, self.last_sim_dt_s, r.active && r.speed <= 1.525_878_9e-5, false);
        let timer = event.timer.unwrap_or(DeathTimerEvent::None);
        actors(self, s, dt);
        let next = self.frame_head(s, w, race_start, dt, false);
        if next != survive::SurviveStep::None {
            survive = next;
        }
        Execution { rewind: r, dt, was_alive, rider_dead, timer, recorded, race_start, event, survive }
    }

    pub(super) fn score_manager(
        &mut self,
        s: &mut RiderState,
        w: &World,
        dt_s: f32,
        rewinding: bool,
        paused: bool,
    ) -> EventUpdate {
        let mut racers = self.bots.as_ref().map(|b| b.racers(w)).unwrap_or_default();
        for (r, &t) in racers.iter_mut().zip(&self.racer_end) {
            if let Some(r) = r {
                r.touching_end = t;
            }
        }
        let inputs = EventInputs {
            dt_s,
            rewinding,
            player_y: s.traj.y,
            touching_end: self.tracker.touching_name(&w.triggers, "end"),
            health: s.survival.health,
            allow_race_end: s.allow_race_end,
            points: s.score.score,
            descent: s.score.descent,
            nis_outro_active: s.start.as_ref().and_then(|l| l.intro.as_ref()).is_some_and(|i| i.outro_active()),
            racers,
            geotag_credits: self.geotags.as_ref().map_or(0, geotags::GeotagRun::credits),
            geotag_count: self.geotags.as_ref().map_or(0, |g| g.result_list.len() as u32),
            last_safe_ground: s.last_safe_ground.to_array(),
        };
        let finish_flag = crate::objectives::Name::new("CrossedFinishLine");
        let requested = self.objectives.as_ref().is_some_and(|o| o.runner.flags.check(&finish_flag));
        self.event.crossed_finish_line = requested;
        let running = self.event.state == crate::event::EventState::Running && !self.event.waiting;
        let event = if paused {
            self.event.update_paused(&mut self.timer, &inputs)
        } else {
            self.event.update(&mut self.timer, &inputs)
        };
        if running {
            let p = crate::stats::PlayerStats {
                awesome: s.score.is_awesome,
                super_awesome: s.score.is_super_awesome,
                wingsuit: s.wingsuit.signal,
                boosting: s.boost.is_boosting,
                speed: s.player_speed,
                avalanche_mass: s.avalanche.published.aggregate.mass,
                racers: inputs.racers.map(|r| {
                    r.map(|r| crate::stats::RacerStats { trigger_wipeout: r.trigger_wipeout, in_air: r.in_air })
                }),
            };
            let event_type = self.event.setup.event_type.map(|t| t as i32);
            self.stats.update(inputs.dt_s, self.event.clock.ms(), inputs.rewinding, event_type, &p);
        }
        if event.results
            && let Some(r) = self.event.result.as_ref()
        {
            let event_type = self.event.setup.event_type.map(|t| t as i32);
            self.stats.results(r.descent_mm, event_type, self.survive.is_some());
        }
        if requested
            && !self.event.crossed_finish_line
            && let Some(o) = self.objectives.as_mut()
        {
            o.runner.flags.set(&finish_flag, false, true);
        }
        if event.crossed
            && let Some(o) = self.objectives.as_mut()
        {
            o.runner.flags.set(&crate::objectives::Name::new("FreerideFinish"), self.event.setup.free_ride, true);
        }
        if let Some(b) = self.bots.as_mut() {
            for (bot, r) in b.bots.iter_mut().zip(&self.event.racers) {
                bot.finish_ms = r.finish_ms;
            }
        }
        self.flow.track_finished = self.event.track_finished;
        if let Some(o) = self.objectives.as_mut() {
            if event.finished {
                o.runner.events.on_crossed_finish_line();
            }
            if event.results && !self.event.result.as_ref().is_some_and(|r| r.failed) {
                o.runner.events.on_race_ended();
            }
        }
        if event.timer == Some(DeathTimerEvent::RewoundOutOfDeath) {
            self.stats.rewound_out_of_death();
        }
        self.event_ended |= event.results;
        if event.results
            && let Some(t) = self.tombstones.as_mut()
        {
            t.on_run_ended();
        }
        if event.finished || event.results {
            self.pads_released = true;
        }
        if event.looped {
            self.pads_released = false;
            self.stats.survive_lap();
            if let Some(t) = self.tombstones.as_mut() {
                t.on_loop();
            }
            if let Some(l) = self.survive.as_mut() {
                l.loop_message();
            }
            let nis_active = self.flow.nis_active || s.start.is_some();
            self.restart_objectives(s, w, nis_active);
            if let Some(h) = self.heli_jump.as_mut() {
                h.restart();
            }
        }
        self.ui_rewind_count = self.manager.count.rewinds_left;
        event
    }
}
