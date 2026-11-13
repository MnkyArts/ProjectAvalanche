use super::*;

impl EventManager {
    pub fn update(&mut self, timer: &mut crate::death::DeathTimer, i: &EventInputs) -> EventUpdate {
        self.update_with(timer, i, i.dt_s)
    }

    pub fn update_paused(&mut self, timer: &mut crate::death::DeathTimer, i: &EventInputs) -> EventUpdate {
        self.update_with(timer, i, crate::session::LOOP_STEP_S)
    }

    pub(super) fn update_with(
        &mut self,
        timer: &mut crate::death::DeathTimer,
        i: &EventInputs,
        widget_dt_s: f32,
    ) -> EventUpdate {
        use crate::death::DeathTimerEvent;
        let mut out = EventUpdate::default();
        if let Some(result) = self.round_over
            && self.result.is_none()
            && self.on_at_round_end()
        {
            self.end_at_limit(i, &mut out, !result);
            (self.request_loop_to_top, self.waiting) = (false, false);
        }
        match self.state {
            EventState::Running if !self.waiting => {
                self.clock.advance(i.dt_s);
                if i.rewinding && !timer.rewinding {
                    self.rewinds += 1;
                }
                let ev = timer.update(i.dt_s, i.rewinding);
                out.timer = Some(ev);
                if ev == DeathTimerEvent::EventEnds {
                    self.end_on_death(i, &mut out);
                }
                if std::mem::take(&mut self.time_up) && self.result.is_none() {
                    self.end_by_time(i, &mut out);
                }
                if !i.rewinding {
                    self.progress_and_finish(i, &mut out);
                }
            }
            EventState::DelayedFinish => {
                let ev = timer.countdown(i.dt_s);
                out.timer = Some(ev);
                if ev == DeathTimerEvent::EventEnds {
                    self.end_on_death(i, &mut out);
                }
                self.after_finish(i, &mut out);
            }
            EventState::Finished | EventState::LoopOutro => self.after_finish(i, &mut out),
            _ => {}
        }
        if self.finish_message.as_mut().is_some_and(|m| m.update(widget_dt_s)) {
            self.finish_message = None;
        }
        out
    }

    pub(super) fn progress_and_finish(&mut self, i: &EventInputs, out: &mut EventUpdate) {
        if let Some(c) = self.setup.course {
            let k = c.split_index(i.player_y);
            self.progress = c.progress(i.player_y);
            if self.split < k {
                self.split = k;
                let ms = self.clock.ms();
                self.split_times.push((k, ms));
                out.split = Some((k, ms));
            }
        }
        let ms = self.clock.ms();
        for (k, r) in i.racers.iter().enumerate() {
            let Some(r) = r else { break };
            if self.racers.len() <= k {
                self.racers.push(RacerProgress::default());
            }
            let p = &mut self.racers[k];
            p.points = r.points;
            if p.finish_ms.is_some() {
                continue;
            }
            if let Some(c) = self.setup.course {
                p.progress = c.progress(r.y);
                p.split = p.split.max(c.split_index(r.y));
            }
            if r.touching_end {
                p.finish_ms = Some(ms);
            }
        }
        if (i.health > 0.0 && i.touching_end) || self.crossed_finish_line {
            self.crossed_finish_line = false;
            self.clock.stop();
            self.finish_ms = Some(self.clock.ms());
            out.crossed = true;
            if self.setup.delayed_finish {
                self.state = EventState::DelayedFinish;
                self.rewind_disabled = true;
                self.result = Some(self.build_result(i, false));
            } else {
                self.finish_track();
                out.finished = true;
            }
        }
    }

    pub(super) fn finish_track(&mut self) {
        self.finish_message = Some(FinishMessage::default());
        self.track_finished = true;
        self.state = EventState::Finished;
        self.clock.stop();
    }

    pub(super) fn after_finish(&mut self, i: &EventInputs, out: &mut EventUpdate) {
        if !i.allow_race_end {
            return;
        }
        if self.state == EventState::DelayedFinish {
            self.finish_track();
            out.finished = true;
            self.rewind_disabled = false;
            return;
        }
        if self.finish_message.is_some() {
            return;
        }
        if !self.loops_after_finish() {
            self.allow_outro = true;
            self.state = EventState::Results;
            self.result = Some(self.build_result(i, false));
            out.results = true;
            return;
        }
        match self.state {
            EventState::Finished => {
                self.allow_outro = true;
                if i.nis_outro_active {
                    self.allow_outro = false;
                    self.state = EventState::LoopOutro;
                }
            }
            EventState::LoopOutro if !i.nis_outro_active => self.count_loop(out),
            _ => {}
        }
    }

    pub fn loops_after_finish(&self) -> bool {
        self.setup.event_type == Some(EventType::SurviveToDeath) || self.setup.free_ride
    }

    fn count_loop(&mut self, out: &mut EventUpdate) {
        self.loops += 1;
        self.waiting = true;
        self.state = EventState::LoopRestart;
        self.request_loop_to_top = true;
        self.nis_short = true;
        out.looped = true;
    }

    pub(super) fn end_by_time(&mut self, i: &EventInputs, out: &mut EventUpdate) {
        self.end_at_limit(i, out, !self.setup.time_up_result);
    }

    fn end_at_limit(&mut self, i: &EventInputs, out: &mut EventUpdate, failed: bool) {
        self.state = if failed { EventState::Idle } else { EventState::Results };
        if failed {
            self.clock.stop();
        }
        self.result = Some(self.build_result(i, failed));
        out.results = true;
    }

    fn on_at_round_end(&self) -> bool {
        match self.state {
            EventState::Running => true,
            EventState::Finished | EventState::LoopOutro | EventState::LoopRestart => self.loops_after_finish(),
            EventState::Idle | EventState::DelayedFinish | EventState::Results => false,
        }
    }

    pub(super) fn end_on_death(&mut self, i: &EventInputs, out: &mut EventUpdate) {
        let survive = self.setup.event_type == Some(EventType::SurviveToDeath);
        self.state = if survive { EventState::Results } else { EventState::Idle };
        if !survive {
            self.clock.stop();
        }
        self.result = Some(self.build_result(i, !survive));
        out.results = true;
    }

    pub fn descent_mm(&self, descent: f32) -> u32 {
        let own = (descent * 1000.0) as i64 as u32;
        let course = self.setup.course.map_or(0, |c| ((c.start_y - c.end_y) * 1000.0) as i32 as i64);
        ((course * i64::from(self.loops)) as u32).wrapping_add(own)
    }

    pub fn explore_splits(&self) -> Option<[u32; 4]> {
        let r = self.result.as_ref()?;
        let [x, y, z] = self.end_position.map(f32::to_bits);
        Some([r.loops, x, y, z])
    }

    pub(super) fn build_result(&mut self, i: &EventInputs, failed: bool) -> EventResult {
        self.end_position = i.last_safe_ground;
        let clamp = |v: f32| (v as i64).clamp(0, i64::from(i32::MAX)) as u32;
        let points = clamp(i.points);
        let time_ms = self.clock.ms();
        let descent_mm = self.descent_mm(i.descent);
        let value = match self.setup.event_type {
            Some(t) if t.is_race() => time_ms,
            Some(t) if t.is_trick() => points,
            _ => descent_mm,
        };
        let ascending = self.setup.event_type.is_some_and(EventType::ranks_ascending);
        let mut racers = if self.racers.is_empty() {
            Vec::new()
        } else {
            let trick = self.setup.event_type.is_some_and(EventType::is_trick);
            let projected = |v: u32, progress: f32| v.wrapping_add(((1.0 - progress) * v as f32) as u32);
            self.racers
                .iter()
                .map(|r| match (trick, r.finish_ms) {
                    (false, Some(t)) => t,
                    (false, None) => projected(time_ms, r.progress),
                    (true, Some(_)) => r.points.max(0.0) as u32,
                    (true, None) => projected(r.points.max(0.0) as u32, r.progress),
                })
                .collect()
        };
        if ascending {
            racers.sort_unstable();
        } else {
            racers.sort_unstable_by(|a, b| b.cmp(a));
        }
        let Ranking { rivals, rank, medal, place } = self.setup.rank(value, failed, racers);
        EventResult {
            event_type: self.setup.event_type,
            points,
            time_ms,
            descent_mm,
            rewinds: self.rewinds,
            loops: self.loops,
            failed,
            value,
            rivals,
            rank,
            medal,
            place,
            geotag_credits: i.geotag_credits,
            geotag_count: i.geotag_count,
        }
    }
}
