use super::*;

impl Helicopter {
    pub fn step(&mut self, x: &HeliStep) {
        let intro = x.intro.filter(|i| Arc::ptr_eq(&i.assets, &self.assets));
        let mut idle = false;
        match (intro, self.phase) {
            (Some(i), _) => {
                self.frame = nis_frame(i);
                self.phase = HeliPhase::Nis(i.scene);
                self.cabin = i.cabin;
                self.nis_chain = i.chain();
                if let Some(t) = i.heli(i.ticks()) {
                    self.traj = t;
                }
                let sc = i.current();
                idle = sc.actor(ssx_assets::nis::ROLE_HELICOPTER1).is_none()
                    && sc.heli_movement.as_deref() == Some("heli_move_idle");
                self.note_nis_pose();
            }
            (None, HeliPhase::Nis(from)) => {
                self.phase = HeliPhase::Gameplay { ticks: x.dt, from };
                self.cabin = None;
                self.nis_chain.clear();
                self.node = HeliNode::IdleStart;
            }
            (None, HeliPhase::Gameplay { ticks, from }) => {
                self.phase = HeliPhase::Gameplay { ticks: ticks + x.dt, from };
            }
        }
        let Some(flight) = self.flight.clone() else {
            (self.velocity, self.angular_velocity) = (Vec3::ZERO, Quat::IDENTITY);
            return;
        };
        let (traj, velocity, reverse_cam) = (self.traj.1, self.velocity, self.reverse_cam);
        self.board.update(&flight.update, flight.grid.as_ref(), (traj, velocity), &mut self.interaction, reverse_cam);
        let delta = if idle {
            self.hover_idle(&flight, x.dt)
        } else if self.nis_active() {
            None
        } else {
            self.fly(&flight, x)
        };
        self.rig_op(delta, x.dt);
        (self.board.target_position, self.board.target_velocity) = x.target;
    }

    pub(super) fn hover_idle(&mut self, f: &HeliFlightData, dt: f32) -> Option<(Vec3, Quat)> {
        let inputs = MoveInputs {
            target: self.traj.1,
            target_orientation: Some(self.move_to.map_or(self.traj.0, |m| m.1)),
            avoid: None,
            fixed: None,
            avoid_boarder: None,
        };
        let ctrl = self.ctrl.get_or_insert_with(MoveController::default);
        let out = ctrl.update(
            &f.to_position,
            f.posing.as_ref(),
            &inputs,
            self.traj,
            (self.velocity, self.angular_velocity),
            dt,
        )?;
        self.outputs = Some(out);
        let qc = self.traj.0.conjugate();
        Some((qc * out.displacement, (qc * out.rotation).normalize()))
    }

    pub(super) fn fly(&mut self, f: &HeliFlightData, x: &HeliStep) -> Option<(Vec3, Quat)> {
        let mut next = None;
        for _ in 0..4 {
            match self.transition(f, next.unwrap_or(self.node), x) {
                Some(n) => next = Some(n),
                None => break,
            }
        }
        if let Some(n) = next {
            self.enter(n);
        }
        if self.move_to.is_none()
            && let Some(due) = self.window_due(f).filter(|&due| due < x.dt)
        {
            let first = self.run(f, due);
            self.enter(HeliNode::ChaseTarget);
            let rest = x.dt - due;
            if rest <= 0.001 {
                return first;
            }
            return accumulate_delta(first, self.run(f, rest));
        }
        self.run(f, x.dt)
    }

    pub(super) fn enter(&mut self, n: HeliNode) {
        let old = (self.ctrl.take(), self.node);
        self.blend = Some((old.0, old.1, 0.0));
        self.node = n;
        self.ctrl = matches!(n, HeliNode::ChaseTarget | HeliNode::MoveToPosition | HeliNode::PlayerInteractTarget)
            .then(MoveController::default);
    }

    pub(super) fn window_due(&self, f: &HeliFlightData) -> Option<f32> {
        let HeliNode::Trick { seq, ticks, .. } = self.node else { return None };
        let (start, _) = f.tricks.get(seq)?.end_window?;
        Some((start - ticks) / self.trick_rate())
    }

    pub(super) fn run(&mut self, f: &HeliFlightData, dt: f32) -> Option<(Vec3, Quat)> {
        let blend_time = match (self.node, self.blend.as_ref().map(|b| b.1)) {
            (HeliNode::IdleEnd { .. }, _) => f.idle_end_blend.0,
            (_, Some(HeliNode::Trick { seq, .. })) => f.tricks.get(seq).map_or(DEFAULT_BLEND_TICKS, |t| t.end_blend.0),
            _ => DEFAULT_BLEND_TICKS,
        };
        if let Some((_, _, e)) = self.blend
            && (blend_time - e <= dt || self.blend_weight(e) >= 0.99)
        {
            self.blend = None;
        }
        let mut old = None;
        if let Some((ctrl, node, elapsed)) = self.blend.take() {
            let saved = std::mem::replace(&mut self.ctrl, ctrl);
            old = Some((self.node_delta(f, node, dt, false), self.advance(node, dt), elapsed + dt));
            let ctrl = std::mem::replace(&mut self.ctrl, saved);
            self.blend = old.map(|(_, n, e)| (ctrl, n, e));
        }
        let new = self.node_delta(f, self.node, dt, true);
        let out = match old {
            Some((d, _, elapsed)) => blend_delta(d, new, self.blend_weight(elapsed)),
            None => new,
        };
        if let HeliNode::IdleEnd { ticks } = &mut self.node {
            *ticks += dt;
        }
        self.node = self.advance(self.node, dt);
        out
    }

    pub(super) fn transition(&self, f: &HeliFlightData, node: HeliNode, x: &HeliStep) -> Option<HeliNode> {
        let ib = &self.interaction;
        let to_move = self.move_to.is_some();
        let trick = || self.trick_entry(f);
        match node {
            HeliNode::IdleStart if x.race_proper_started => {
                Some(if to_move { HeliNode::MoveToPosition } else { HeliNode::ChaseTarget })
            }
            HeliNode::ChaseTarget if ib.interact => Some(HeliNode::PlayerInteractTarget),
            HeliNode::ChaseTarget if ib.helicopter_interaction && trick().is_some() => trick(),
            HeliNode::ChaseTarget if to_move => Some(HeliNode::MoveToPosition),
            HeliNode::MoveToPosition if ib.interact => Some(HeliNode::PlayerInteractTarget),
            HeliNode::MoveToPosition if ib.helicopter_interaction && trick().is_some() => trick(),
            HeliNode::MoveToPosition if !self.ends_with_heli_jump && x.track_finished && self.at_move_to => {
                Some(HeliNode::IdleEnd { ticks: 0.0 })
            }
            HeliNode::MoveToPosition if !to_move => Some(HeliNode::ChaseTarget),
            HeliNode::PlayerInteractTarget if ib.helicopter_interaction && trick().is_some() => trick(),
            HeliNode::PlayerInteractTarget if !ib.interact => {
                Some(if to_move { HeliNode::MoveToPosition } else { HeliNode::ChaseTarget })
            }
            HeliNode::Trick { seq, ticks, .. } if !to_move => {
                let (start, length) = f.tricks.get(seq)?.end_window?;
                (start..=start + length).contains(&ticks).then_some(HeliNode::ChaseTarget)
            }
            _ => None,
        }
    }

    pub(super) fn trick_entry(&self, f: &HeliFlightData) -> Option<HeliNode> {
        let ib = &self.interaction;
        let signal = |n: &str| match n {
            "BLN_HeliInteractCrossways" => ib.across,
            "BLN_HeliInteractionGrind" => ib.grind,
            "BLN_Goofy" => ib.goofy,
            "BLN_IceAxeEnabled" => ib.ice_axe,
            _ => false,
        };
        let seq = f
            .tricks
            .iter()
            .position(|t| t.true_signals.iter().all(|n| signal(n)) && !t.false_signals.iter().any(|n| signal(n)))?;
        Some(HeliNode::Trick { seq, ticks: 0.0, mirror: ib.mirror })
    }

    pub(super) fn trick_rate(&self) -> f32 {
        self.interaction.anim_time_scale.clamp(0.25, 3.0)
    }

    pub(super) fn advance(&self, node: HeliNode, dt: f32) -> HeliNode {
        match node {
            HeliNode::Trick { seq, ticks, mirror } => {
                HeliNode::Trick { seq, ticks: ticks + dt * self.trick_rate(), mirror }
            }
            n => n,
        }
    }

    pub(super) fn node_delta(
        &mut self,
        f: &HeliFlightData,
        node: HeliNode,
        dt: f32,
        current: bool,
    ) -> Option<(Vec3, Quat)> {
        let ib = &self.interaction;
        let fixed = ib.interact.then_some(FixedTarget {
            time: ib.time,
            initial: self.initial_interact_time,
            orientation: Some(ib.orientation),
        });
        let (tuning, inputs) = match node {
            HeliNode::ChaseTarget | HeliNode::PlayerInteractTarget => (
                &f.follow,
                MoveInputs {
                    target: self.board.desired_position,
                    target_orientation: None,
                    avoid: self.board.avoid,
                    fixed,
                    avoid_boarder: ib.avoid.then_some(ib.avoid_distance),
                },
            ),
            HeliNode::MoveToPosition => {
                let (p, q) = self.move_to.unwrap_or((self.traj.1, self.traj.0));
                (
                    &f.to_position,
                    MoveInputs {
                        target: p,
                        target_orientation: Some(q),
                        avoid: self.board.avoid,
                        fixed: fixed.map(|x| FixedTarget { orientation: None, ..x }),
                        avoid_boarder: ib.avoid.then_some(ib.avoid_distance),
                    },
                )
            }
            HeliNode::Trick { seq, ticks, mirror } => return self.trick_delta(f, seq, ticks, mirror, dt),
            HeliNode::IdleEnd { ticks } => {
                if current {
                    self.place_end_level(f, ticks + dt);
                }
                return None;
            }
            HeliNode::IdleStart => return None,
        };
        let ctrl = self.ctrl.get_or_insert_with(MoveController::default);
        let out =
            ctrl.update(tuning, f.posing.as_ref(), &inputs, self.traj, (self.velocity, self.angular_velocity), dt)?;
        self.initial_interact_time = out.initial_time;
        if current {
            self.outputs = Some(out);
            if node == HeliNode::MoveToPosition {
                self.at_move_to = out.at_target;
            }
        }
        let qc = self.traj.0.conjugate();
        Some((qc * out.displacement, (qc * out.rotation).normalize()))
    }
}

impl Helicopter {
    pub(super) fn place_end_level(&mut self, f: &HeliFlightData, t: f32) {
        let (Some(clip), Some((p, q))) = (&f.end_level, self.move_to) else { return };
        let Ok(pose) = clip.sample(t.rem_euclid(600.0)) else { return };
        let rig = self.assets.nis.rig(ssx_assets::nis::ROLE_HELICOPTER1);
        let (cq, ct) = clip.joint(&pose, rig, "AITrajectory");
        let cq = Quat::from_array(cq).normalize();
        self.traj = ((q * cq).normalize(), p + q * Vec3::from_array(ct));
    }

    pub(super) fn trick_delta(
        &self,
        f: &HeliFlightData,
        seq: usize,
        ticks: f32,
        mirror: bool,
        dt: f32,
    ) -> Option<(Vec3, Quat)> {
        let clip = &f.tricks.get(seq)?.clip;
        let sample = |t: f32| -> Option<(Quat, Vec3)> {
            let p = clip.sample(t).ok()?;
            Some((
                Quat::from_array(clip.quat(&p, "AITrajectory.q")?).normalize(),
                Vec3::from_array(clip.vec3(&p, "AITrajectory.t")?),
            ))
        };
        let ((q0, p0), (q1, p1)) = (sample(ticks)?, sample(ticks + dt * self.trick_rate())?);
        let c = q0.conjugate();
        let (d, r) = (c * (p1 - p0), (c * q1).normalize());
        Some(if mirror { (Vec3::new(d.x, d.y, -d.z), Quat::from_xyzw(-r.x, -r.y, r.z, r.w)) } else { (d, r) })
    }

    pub fn floor_joint(&self) -> Vec3 {
        let rig = &self.assets.nis.heli;
        let pose = self.gameplay_pose();
        let Some(chain) = rig.chain("Floor", "AITrajectory") else { return self.traj.1 };
        let (mut q, mut p) = self.traj;
        for j in chain.iter().rev() {
            let lq = pose.q.get(j).map_or(Quat::IDENTITY, |v| Quat::from_array(*v));
            let lt = pose.t.get(j).map_or(Vec3::ZERO, |v| Vec3::from_array(*v));
            p += q * lt;
            q = (q * lq).normalize();
        }
        p
    }

    pub fn interaction(
        &mut self,
        rider: &mut interaction::RiderHeliStates,
        mut x: interaction::InteractionInputs,
    ) -> Option<Vec3> {
        let t = self.flight.as_ref().and_then(|f| f.interaction.clone())?;
        (x.heli_position, x.heli_orientation, x.heli_velocity) = (self.traj.1, self.traj.0, self.velocity);
        x.floor_joint = self.floor_joint();
        self.interaction.location = self.board.desired_position;
        let v = interaction::execute(&t, &mut self.interaction_node, &mut self.interaction, rider, &x);
        self.board.desired_position = self.interaction.location;
        v
    }

    pub(super) fn rig_op(&mut self, delta: Option<(Vec3, Quat)>, dt: f32) {
        let (dt_local, dq) = delta.unwrap_or((Vec3::ZERO, Quat::IDENTITY));
        let (q, p) = self.traj;
        self.traj = ((q * dq).normalize(), p + q * dt_local);
        if dt > EPS16 {
            let inv = 1.0 / dt;
            self.velocity = q * dt_local * inv;
            let w = (q * dq * q.conjugate()).normalize();
            let w = if w.w < 0.0 { -w } else { w };
            self.angular_velocity = Quat::IDENTITY.lerp(w, inv).normalize();
        }
    }
}
