use std::time::Instant;

use bevy::prelude::*;
use ssx_physics::SIM_HZ;
use ssx_physics::death::DeathTimerEvent;
use ssx_physics::rewind::RewindPad;
use ssx_physics::rider::{RideState, RiderState};
use ssx_physics::session::Execution;

use super::clock::{lerp_pose, pose_of};
use super::controls::PadLock;
use super::scripted::{AutoRewind, avalanche_override, back_script};
use super::start::level_start;
use super::{Clock, OutroRun, Ride};
use crate::rider::anim::RiderAnimation;

impl Ride {
    fn outro_after_execution(&mut self, anim: Option<&mut RiderAnimation>) {
        use ssx_physics::outro::{OutroScript, game_mode, outro_of_scripts, track_end_reaction};
        if self.outro.is_none() {
            if !self.session.event.allow_outro {
                return;
            }
            let def = self.event_def.as_ref();
            let script = def.and_then(|d| outro_of_scripts(&d.scripts));
            let reaction = match (def, &self.session.event.result) {
                (Some(d), Some(r)) => track_end_reaction(game_mode(&d.name), r),
                _ => None,
            };
            let assets = match (def, script, reaction) {
                _ if crate::render::post::env_is("SSX_OUTRO", "off") => None,
                (Some(d), Some(OutroScript::Standard), Some(r)) if !self.session.event.setup.free_ride => {
                    match ssx_physics::outro::prepare(&self.big_root, &d.name, &self.character, r) {
                        Ok(a) => Some(a),
                        Err(e) => {
                            warn!("outro: {e:#}");
                            None
                        }
                    }
                }
                _ => None,
            };
            info!(
                "outro: {script:?}, INT_TrackEndReaction {reaction:?}{}",
                assets.as_ref().map_or(" (not played)".into(), |a| format!(
                    ": {} then {} (unresolved: {:?})",
                    a.nis.ready.sequence,
                    a.nis.jump.sequence,
                    a.nis.ready.problems.iter().chain(&a.nis.jump.problems).collect::<Vec<_>>()
                ))
            );
            self.outro = Some(OutroRun { playing: false, assets, reaction });
        }
        let Some(run) = self.outro.as_mut() else { return };
        let Some(assets) = run.assets.clone() else { return };
        let asked = self.session.objectives.as_ref().and_then(|o| {
            o.effects.iter().find_map(|(_, e)| match e {
                ssx_physics::objectives::Effect::PlayNis(r) => Some(r.name == assets.choice.binding),
                _ => None,
            })
        });
        if run.playing || asked != Some(true) {
            return;
        }
        run.playing = true;
        match ssx_physics::outro::start(&self.world, assets.clone()) {
            Ok(state) => {
                if let Some(a) = anim
                    && let Err(e) = a.graph_mut().set_intro(&self.big_root, Some(&assets.nis))
                {
                    warn!("outro: the rider's NIS graph failed ({e:#})");
                }
                info!("Ride step {}: outro NIS {} bound", self.steps + 1, assets.nis.binding);
                self.intro_graph = false;
                self.state = state;
                self.curr = pose_of(&self.state);
                self.prev = self.curr;
            }
            Err(e) => {
                warn!("outro: {e:#}");
                run.assets = None;
            }
        }
    }

    pub fn respawn(&mut self) {
        if let Some(short) = self.intro_short.take() {
            info!("intro: a restart takes the short path");
            self.intro = Some(short);
            (self.intro_graph, self.cabin_seen) = (false, None);
        }
        if let Some(mut state) = level_start(&self.world, self.spawn, self.intro.as_ref()) {
            state.score.boost = self.boost_meter.clamp(0.0, self.world.tuning.scoring.max_boost);
            state.tricks.classic = self.classic;
            state.carry_controller_state(&self.state);
            self.curr = pose_of(&state);
            self.prev = self.curr;
            (self.render_lag, self.render_steps) = (0.0, 0);
            self.render = self.curr;
            self.heli = state
                .start
                .as_ref()
                .and_then(|l| l.intro.as_ref())
                .map(|i| ssx_physics::helicopter::Helicopter::with_flight(i, self.heli_flight.clone()));
            self.state = state;
            self.steps = 0;
            self.audio_tag_log.clear();
            self.distance = 0.0;
            self.session.reset();
            if let Some(m) = self.map.as_mut() {
                m.gates.reset(&mut self.session);
                m.player = m.player.new_run();
                m.finished = false;
            }
            self.outro = None;
            self.rewinding = false;
            self.auto_rewind = AutoRewind::default();
            self.autoplay_waited = 0;
            self.autoplay_sat_out = false;
            self.last_grind_capture = 0.0;
            self.rewind_backwards = false;
            self.last_exec = None;
            self.last_press_capture = 0.0;
            self.motion_blur_cue = 0;
            (self.ws_pressed, self.ws_deployed) = (None, None);
            (self.grind_since, self.grind_ended) = (None, None);
            self.a_lock = PadLock::default();
            self.messages_logged = 0.0;
            self.go_step = None;
            if let Some(pb) = self.pose_blocks.as_mut() {
                pb.respawned();
            }
        }
    }

    pub(super) fn advance(&mut self, mut anim: Option<&mut RiderAnimation>, reverse_camera: Option<Quat>) {
        self.state.reverse_camera = reverse_camera;
        if let Some(i) = self.state.start.as_mut().and_then(|l| l.intro.as_mut())
            && i.held()
        {
            if let super::live::StartGate::Go { .. } = self.start_gate {
                i.round_go();
                info!(
                    "Ride step {}: the round's start: the GO given to the held intro (its last scene from the next step)",
                    self.steps + 1
                );
            }
            self.steps += 1;
            self.prev = self.curr;
            return;
        }
        let (before, was_air) = (self.state.position, self.state.state != RideState::Riding);
        let was_janked = self.state.janked.janked;
        let t0 = Instant::now();
        if self.steps == 0
            && let Some(a) = anim.as_deref_mut()
        {
            if !self.intro_graph {
                self.intro_graph = true;
                self.cabin_seen = self.intro.as_deref().filter(|i| !i.nis.cabin.is_empty()).map(|_| 0);
                if let Err(e) = a.graph_mut().set_intro(&self.big_root, self.intro.as_deref().map(|i| &i.nis)) {
                    warn!("intro: the rider's NIS graph failed ({e:#}); the level start poses nothing");
                }
            }
            a.reset();
        }
        let graph = anim.as_deref_mut().map(RiderAnimation::graph_mut);
        let was_in_area = self.state.avalanche.switches.carving_sensitivity > 0.0;
        if let Some(race) = self.session.bots.as_mut()
            && let Some((at, k)) = std::env::var("SSX_BOT_WIPEOUT").ok().and_then(|v| {
                let mut it = v.split(',');
                Some((
                    it.next()?.trim().parse::<f32>().ok()?,
                    it.next().and_then(|k| k.trim().parse().ok()).unwrap_or(0),
                ))
            })
            && (self.steps as f32 / SIM_HZ..(self.steps + 1) as f32 / SIM_HZ).contains(&at)
        {
            race.forced_wipeout = Some(k);
        }
        if let Ok(list) = std::env::var("SSX_NIS_POST")
            && let Some(o) = self.session.objectives.as_mut()
        {
            for (event, step) in list.split(',').filter_map(|e| e.split_once('@')) {
                if step.trim().parse::<u64>().is_ok_and(|n| n == self.steps + 1) {
                    o.runner.events.post(ssx_physics::objectives::Name::new(event.trim()), None);
                }
            }
        }
        let (input, tricks) = self.locked_controls();
        let at_start = self.state.start.is_some();
        let (grind_held, wingsuit_held, rewind_held) = if self.classic {
            (tricks.y, input.jump, tricks.b)
        } else {
            (self.l2, self.r1, self.rewind_pad.rewind_held)
        };
        self.state.tricks.classic = self.classic;
        self.state.grind.pad_l2 = grind_held;
        self.state.grind.pad_l2_hold = self.l2_hold;
        self.state.wingsuit.pad_r1 = wingsuit_held;
        let pad = RewindPad { flip_accel: input.flip_acceleration(self.classic), rewind_held, ..self.rewind_pad };
        let gate = if at_start { self.start_gate } else { super::live::StartGate::Free };
        let released =
            || (ssx_physics::rider::Input::default(), ssx_physics::rider::TrickInput::default(), RewindPad::default());
        let (input, tricks, pad) = match gate {
            super::live::StartGate::Hold => {
                self.state.grind.release_l2();
                self.state.wingsuit.pad_r1 = false;
                released()
            }
            super::live::StartGate::Go { pressed } => {
                self.state.grind.release_l2();
                self.state.wingsuit.pad_r1 = false;
                let releasing = self
                    .state
                    .start
                    .as_ref()
                    .and_then(|l| l.intro.as_ref())
                    .is_some_and(|i| i.round_hold() == ssx_physics::nis::RoundHold::Held { go: true });
                if releasing {
                    released()
                } else {
                    self.start_gate = super::live::StartGate::Go { pressed: !pressed };
                    let (mut i, t, p) = released();
                    i.jump = !pressed;
                    (i, t, p)
                }
            }
            super::live::StartGate::Free if self.pads_held || self.chat_held => {
                self.state.grind.release_l2();
                self.state.wingsuit.pad_r1 = false;
                released()
            }
            super::live::StartGate::Free => (input, tricks, pad),
        };
        let ready = |s: &RiderState| s.start.as_ref().and_then(|l| l.intro.as_ref()).is_some_and(|i| i.in_ready_loop());
        let waiting =
            |s: &RiderState| s.start.as_ref().is_some_and(|l| l.intro.as_ref().map_or(l.idle(), |i| i.waits_for_go()));
        let was_ready = ready(&self.state);
        let was_waiting = waiting(&self.state);
        if let Some(h) = self.heli.as_mut() {
            let (was, was_prepared) = (h.interaction.interact, h.interaction.prepare);
            self.session.heli_interaction(h, &mut self.state, &self.world, &tricks);
            if h.interaction.prepare != was_prepared {
                info!(
                    "Ride step {}: heli interaction: prepare {} (BLN_HeliPrepareForInteraction: the right stick down \
                     on the ground; the helicopter {:.1} m away)",
                    self.steps + 1,
                    if h.interaction.prepare { "on" } else { "off" },
                    h.interaction.avoid_distance
                );
            }
            if !was && h.interaction.interact {
                info!(
                    "Ride step {}: heli interaction: meeting in {:.1} ticks, kind {:?}",
                    self.steps + 1,
                    h.interaction.time,
                    h.interaction_node.kind
                );
            }
            if self.state.heli.helicopter_interaction {
                info!(
                    "Ride step {}: heli interaction: the trick starts (kind {:?}, rate {:.3})",
                    self.steps + 1,
                    h.interaction_node.kind,
                    self.state.heli.anim_time_scale
                );
            }
        }
        let (heli, world, steps) = (&mut self.heli, &self.world, self.steps);
        let mut actors = |session: &mut ssx_physics::session::Session, state: &mut RiderState, dt: f32| {
            let Some(h) = heli.as_mut() else { return };
            let course = session.event.setup.course;
            h.step(&ssx_physics::helicopter::HeliStep {
                intro: state.start.as_ref().and_then(|l| l.intro.as_ref()),
                dt,
                time: (steps + 1) as f32 / SIM_HZ,
                race_proper_started: session.flow.race_proper_started,
                track_finished: session.flow.track_finished,
                remaining: course.map(|c| (1.0 - session.event.progress) * (c.start_y - c.end_y)),
                target: (state.traj, state.traj_velocity),
            });
            session.heli_physics_queries(h, world);
        };
        let held = self.pads_held || self.chat_held || gate != super::live::StartGate::Free;
        let r2 = if held { 0.0 } else { self.r2 };
        let e = self.session.execute_with(&mut self.state, &self.world, &input, &tricks, r2, &pad, graph, &mut actors);
        self.log_messages();
        if let Some(m) = self.map.as_mut() {
            let g = m.gates.update(&mut self.session, self.state.traj);
            if let Some(k) = g.passed {
                let ms = m.gates.times.last().copied().unwrap_or(0);
                let opened = if g.opened { "; the finish opens" } else { "" };
                info!(
                    "Ride step {}: map gate {} of {} passed ({ms} ms){opened}",
                    self.steps + 1,
                    k + 1,
                    m.gates.checkpoints.len()
                );
            }
            if g.time_up {
                info!("Ride step {}: the map's time limit ends the event", self.steps + 1);
            }
            if !m.finished {
                m.player.hash(&self.state);
                let (f, n) = (m.player.fingerprint, m.player.hashed);
                if e.rider_dead.is_some() && e.was_alive {
                    info!(
                        "Ride step {}: map ride: the rider dies at {:.2}; fingerprint {f:016x} over {n} steps",
                        self.steps + 1,
                        self.state.traj
                    );
                }
                if self.session.event.track_finished {
                    m.finished = true;
                    let ms = self.session.event.finish_ms.unwrap_or(0);
                    info!(
                        "Ride step {}: map ride: finish at {:.2}: {ms} ms on the event clock; fingerprint {f:016x} over \
                         {n} steps",
                        self.steps + 1,
                        self.state.traj
                    );
                }
            }
        }
        if was_waiting && !waiting(&self.state) {
            self.go_step = Some(self.steps + 1);
            if let super::live::StartGate::Go { .. } = self.start_gate {
                info!("Ride step {}: the round's start: the GO taken", self.steps + 1);
                self.start_gate = super::live::StartGate::Free;
            }
        }
        if was_ready && !ready(&self.state) {
            info!(
                "Ride step {}: the ready loop ends (GO; this step's controls: A {}, the left stick {:.2} / {:.2})",
                self.steps + 1,
                input.jump,
                input.steer,
                input.tuck - input.brake
            );
        }
        if let Some(v) = ssx_physics::outro::session_vista(&self.session, &self.world.triggers) {
            self.vista = Some(v);
        }
        let intro_now = self.state.start.as_ref().and_then(|l| l.intro.as_ref());
        self.world_hidden = intro_now.is_some_and(|i| i.signals().iter().any(|x| x == "BLN_DisableWorldRender"));
        if let Some(i) = intro_now
            && i.cabin.map(|c| c.index) != self.cabin_seen
        {
            self.cabin_seen = i.cabin.map(|c| c.index);
            info!("Ride step {}: intro {} ({})", self.steps + 1, i.current().node, i.current().sequence);
        }
        if e.survive != ssx_physics::session::survive::SurviveStep::None
            && let Some(l) = self.session.survive.as_ref()
        {
            let nis = match e.survive {
                ssx_physics::session::survive::SurviveStep::Outro => l.outro.clone(),
                _ => l.intro.clone(),
            };
            if let Some(a) = anim.as_deref_mut()
                && let Err(e) = a.graph_mut().set_intro(&self.big_root, nis.as_deref().map(|i| &i.nis))
            {
                warn!("survive loop: the rider's NIS graph failed ({e:#})");
            }
            self.heli = self
                .state
                .start
                .as_ref()
                .and_then(|l| l.intro.as_ref())
                .map(|i| ssx_physics::helicopter::Helicopter::with_flight(i, self.heli_flight.clone()));
            info!(
                "Ride step {}: survive loop {:?} ({}; loops {}, lap {}, health {:.2})",
                self.steps + 1,
                e.survive,
                nis.as_ref().map_or("the default level start", |i| i.nis.binding.as_str()),
                self.session.event.loops,
                self.state.survival.lap_count,
                self.state.survival.health
            );
            self.curr = pose_of(&self.state);
            self.prev = self.curr;
        }
        let end_nis = self.session.objectives.as_ref().and_then(|o| {
            o.effects.iter().find_map(|(_, e)| match e {
                ssx_physics::objectives::Effect::PlayNis(r) if r.name == ssx_assets::nis::TUTORIAL_END_BINDING => {
                    Some(r.name.clone())
                }
                _ => None,
            })
        });
        if let Some(binding) = end_nis.filter(|_| self.state.start.is_none()) {
            let event = self.event_def.as_ref().map_or("", |d| d.name.as_str());
            let (at, heading) = self.spawn;
            let q = ssx_physics::spawn::level_start_orientation(&self.world, at, heading);
            let ground = self.world.collision.ray_cast(at, -Vec3::Y, 20_000.0).map_or(at, |h| h.point + Vec3::Y);
            let bound = ssx_physics::outro::prepare_sequence(&self.big_root, event, &self.character, "", &binding)
                .and_then(|a| {
                    let n = ssx_physics::outro::start_sequence(&self.world, &self.state, a.clone(), at, q, ground)?;
                    Ok((a, n))
                });
            match bound {
                Ok((assets, state)) => {
                    if let Some(a) = anim.as_deref_mut()
                        && let Err(e) = a.graph_mut().set_intro(&self.big_root, Some(&assets.nis))
                    {
                        warn!("end NIS: the rider's NIS graph failed ({e:#})");
                    }
                    self.intro_graph = false;
                    info!(
                        "Ride step {}: end NIS {} bound, the rider from {:.1} to {:.2}",
                        self.steps + 1,
                        assets.nis.binding,
                        self.state.traj,
                        state.traj
                    );
                    self.state = state;
                    self.curr = pose_of(&self.state);
                    self.prev = self.curr;
                }
                Err(e) => warn!("end NIS: {e:#}"),
            }
        }
        if self.session.event_ended && self.session.heli_jump.as_ref().is_none_or(|h| h.outro.is_none()) {
            self.outro_after_execution(anim.as_deref_mut());
        }
        if self.world.survival.avalanche.objective_tuning_set.is_some()
            && !was_in_area
            && self.state.avalanche.switches.carving_sensitivity > 0.0
        {
            self.motion_blur_cue -= 1;
        }
        if let Some((set, k)) = avalanche_override() {
            let w = &mut self.state.avalanche.switches;
            (w.tuning_set, w.carving_sensitivity, w.impact_sensitivity) = (set, k, k);
        }
        if self.session.heli_jump.is_some() {
            let nodes = anim.as_deref().map(|a| a.graph().path());
            match self.session.heli_jump_update(&mut self.state, &self.world, self.heli.as_mut(), nodes.as_deref()) {
                Ok(u) => {
                    if u.intercept_changed
                        && let Some(h) = self.session.heli_jump.as_ref()
                    {
                        info!("Ride step {}: heli jump assistance {:?}", self.steps + 1, h.intercept);
                    }
                    if u.outro
                        && let Some(assets) = self.session.heli_jump.as_ref().and_then(|h| h.outro.clone())
                    {
                        self.cabin_seen = (!assets.nis.cabin.is_empty()).then_some(0);
                        if let Some(a) = anim.as_deref_mut()
                            && let Err(e) = a.graph_mut().set_intro(&self.big_root, Some(&assets.nis))
                        {
                            warn!("heli jump: the rider's NIS graph failed ({e:#})");
                        }
                        let move_to = self.heli.as_ref().and_then(|h| h.move_to);
                        self.heli = self.state.start.as_ref().and_then(|l| l.intro.as_ref()).map(|i| {
                            let mut h = ssx_physics::helicopter::Helicopter::with_flight(i, self.heli_flight.clone());
                            h.move_to = move_to.or(h.move_to);
                            h
                        });
                        info!(
                            "Ride step {}: heli jump — BLN_PlayerHeliContact, {} at worlds_heli_end_location ({} → {}), \
                             BLN_AllowRaceEnd",
                            self.steps + 1,
                            assets.nis.binding,
                            assets.nis.ready.node,
                            assets.nis.jump.node
                        );
                        self.curr = pose_of(&self.state);
                        self.prev = self.curr;
                    }
                }
                Err(err) => warn!("heli jump: {err:#}"),
            }
        }
        if let Some(h) = self.heli.as_mut() {
            self.session.drive_helicopter(h);
        }
        let r = e.rewind;
        if self.autoplay && !self.autoplay_sat_out {
            let time = (self.steps - self.autoplay_waited) as f32 / SIM_HZ;
            match self.course.as_mut() {
                Some(p) => p.after(&r, time),
                None => self.auto_rewind.after(&r, time),
            }
        }
        if let Some(c) = self.session.controllers.as_ref() {
            let pushes = c.entities.iter().map(|e| e.fx.physics.pushes.len()).sum::<usize>();
            if pushes > self.flare_pushes {
                for e in &c.entities {
                    if let Some(p) = e.fx.physics.pushes.last().filter(|p| p.time > c.time - 0.5 / SIM_HZ) {
                        info!(
                            "Ride step {}: {} pushed at {:.1} (object {:08x}): linear {:.1} m/s, angular {:.1} rad/s",
                            self.steps + 1,
                            e.script,
                            e.position,
                            e.object,
                            p.linear,
                            p.angular
                        );
                    }
                }
                self.flare_pushes = pushes;
            }
        }
        if self.state.score.interaction_capture != 0.0 {
            info!(
                "Ride step {}: interactions bank captured {:+.0} (record kind {:?}, type {})",
                self.steps + 1,
                self.state.score.interaction_capture,
                self.state.score.last_pushed.map(|r| r.kind),
                self.state.score.last_pushed.map_or(-1, |r| r.interaction)
            );
        }
        if !was_janked && self.state.janked.janked {
            info!(
                "Ride step {}: janked (BLN_Janked, JankedDetect 0x82CDF2D8) at {:.1}: frozen until a rewind (L1)",
                self.steps + 1,
                self.state.traj
            );
        }
        self.last_exec = Some(e);
        self.step_ticks = e.dt;
        if let Some(a) = anim {
            a.after_step(&self.state);
            if let Some(pb) = self.pose_blocks.as_mut()
                && let Err(err) = pb.record(self.steps + 1, &self.state, a.graph(), a.posed().0, &e, r.active)
                && pb.failures == 1
            {
                warn!("live: the pose block was not recorded: {err:#}");
            }
        }
        self.rewinding = r.active;
        if self.state.score.grind_capture > 0.0 {
            self.last_grind_capture = self.state.score.grind_capture;
        }
        if self.state.score.press_capture > 0.0 {
            self.last_press_capture = self.state.score.press_capture;
        }
        if matches!(self.clock, Clock::PerFrame { .. }) {
            log_event(self, &e);
        }
        if matches!(self.clock, Clock::PerFrame { .. }) {
            if e.rider_dead.is_some() && e.was_alive && !r.active {
                info!("Ride step {}: MsgRiderDead (death volume) at {:.1}", self.steps + 1, self.state.traj);
            }
            match e.timer {
                DeathTimerEvent::RewoundOutOfDeath => info!("Ride step {}: rewound out of death", self.steps + 1),
                DeathTimerEvent::EventEnds => info!("Ride step {}: the death timer ended the event", self.steps + 1),
                DeathTimerEvent::None => {}
            }
        }
        self.rewind_backwards = r.active && r.speed <= 1.525_878_9e-5;
        if was_air && self.state.state == RideState::Riding && matches!(self.clock, Clock::PerFrame { .. }) {
            let (l, sc) = (&self.state.landing, &self.state.score);
            info!(
                "Ride touchdown after step {}: {:?}, pitch {:+.0} deg, yaw {:+.0} deg, spin {:.0} / flip {:.0} deg",
                self.steps + 1,
                l.quality,
                l.touchdown_pitch.to_degrees(),
                l.touchdown_spin.to_degrees(),
                (sc.spin_fs + sc.spin_bs).to_degrees(),
                (sc.flip_fwd + sc.flip_bwd).to_degrees(),
            );
        }
        let ms = t0.elapsed().as_secs_f32() * 1000.0;
        self.step_ms = if self.steps == 0 { ms } else { self.step_ms + 0.05 * (ms - self.step_ms) };
        self.distance += f64::from(before.distance(self.state.position));
        self.steps += 1;
        self.audio_tag_log.record(self.steps, &self.state.audio_tags);
        if self.scripted()
            && let Some(times) = back_script()
        {
            let t = self.seconds();
            self.back_held = times.iter().any(|&b| t > b && t <= b + 1.0 / SIM_HZ);
        }
        (self.back_press, self.back_was) = (self.back_held && !self.back_was, self.back_held);
        self.prev = self.curr;
        self.curr = match self.session.buffer.playback() {
            Some((a, b, alpha)) => lerp_pose(&pose_of(a), &pose_of(b), alpha),
            None => pose_of(&self.state),
        };
    }

    fn log_messages(&mut self) {
        use ssx_physics::objectives::Effect;
        let Some(o) = self.session.objectives.as_ref() else { return };
        if o.runner.time == self.messages_logged {
            return;
        }
        self.messages_logged = o.runner.time;
        for (objective, e) in &o.effects {
            let what = match e {
                Effect::TutorialMessage { body, seconds, .. } => format!("tutorial message {body} ({seconds} s)"),
                Effect::TutorialPopup(text) => format!("tutorial popup {text}"),
                Effect::ObjectiveMessage { title, body, seconds, .. } => {
                    format!("objective message {title} / {body} ({seconds} s)")
                }
                Effect::ObjectiveMessageHide => "objective message hidden".to_string(),
                _ => continue,
            };
            info!("Ride step {}: {objective}: {what}", self.steps + 1);
        }
    }

    pub(super) fn shade_query(&self, q: Option<&crate::particles::shadow_query::PlayerShadowQuery>) -> Option<f32> {
        static MODE: std::sync::OnceLock<Option<bool>> = std::sync::OnceLock::new();
        let forced = *MODE.get_or_init(|| match std::env::var("SSX_SHADE_QUERY").as_deref() {
            Ok("cpu") => Some(true),
            Ok("gpu") => Some(false),
            _ => None,
        });
        if forced.unwrap_or(self.autoplay) { None } else { q.map(|q| q.value) }
    }
}

fn log_event(ride: &Ride, e: &Execution) {
    let (s, ev, step) = (&ride.state, &ride.session.event, ride.steps + 1);
    if e.race_start {
        info!("Ride step {step}: MsgTriggerRaceStart, the event clock runs ({:?})", ev.setup.event_type);
    }
    if let Some((k, ms)) = e.event.split {
        info!("Ride step {step}: split {k} at {:.2} s", ms as f32 / 1000.0);
    }
    if e.event.crossed {
        info!("Ride step {step}: crossed the finish line at {:.1}, clock {:.3} s", s.traj, ev.clock.seconds);
    }
    if e.event.results
        && let Some(r) = &ev.result
    {
        info!(
            "Ride step {step}: results: time {:.3} s, points {}, place {:?} of {}, medal {:?}{}",
            r.time_ms as f32 / 1000.0,
            r.points,
            r.rank.map(|k| k + 1),
            r.rivals.len() + 1,
            r.medal,
            if r.failed { " (failed)" } else { "" }
        );
    }
    if s.score.press_capture > 0.0 {
        info!("Ride step {step}: board press scored {:.0}", s.score.press_capture);
    }
    for b in ride.session.bots.iter().flat_map(|r| r.bots.iter()).filter(|b| b.trigger_wipeout) {
        info!(
            "Ride step {step}: bot {} BLN_TriggerWipeout (contacts within 0.75 m: {}; its VEC_Velocity {:.1} m/s)",
            b.replay.id,
            b.contacts,
            b.velocity * 60.0
        );
    }
}
