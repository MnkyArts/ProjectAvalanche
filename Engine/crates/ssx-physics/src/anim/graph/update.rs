use super::*;
use ssx_formats::ghost;

impl RiderGraph {
    pub fn take_audio_tags(&mut self) -> Vec<ssx_assets::anim_eval::AudioTagEvent> {
        self.animator.take_audio_tags()
    }

    pub fn update(
        &mut self,
        s: &RiderState,
        input: &Input,
        tricks: &TrickInput,
        start: &StepStart,
        dt: f32,
    ) -> GraphFeedback {
        self.update_controllers(s, input, tricks, start, dt, None);
        self.finish(s)
    }

    pub fn update_controllers(
        &mut self,
        s: &RiderState,
        input: &Input,
        tricks: &TrickInput,
        start: &StepStart,
        dt: f32,
        physics: Option<StepPhysics>,
    ) {
        self.animator.clear_audio_tags();
        if let Some(st) = &mut self.start {
            st.animator.clear_audio_tags();
        }
        if let Some(ls) = &s.start {
            if let Some(st) = &mut self.start {
                if !st.running {
                    st.animator.reset();
                    if let Some(i) = &mut st.intro {
                        i.reset(&st.animator.gs.random);
                    }
                    (st.running, st.elapsed) = (true, None);
                }
                if let (Some(i), Some(state)) = (&mut st.intro, &ls.intro) {
                    for a in i.animators_mut() {
                        a.set("deltaTicks", dt);
                        a.set("BLN_NisActive", 1.0);
                        a.set_physics(None);
                    }
                    i.update(state, dt);
                    return;
                }
                st.animator.set("deltaTicks", dt);
                st.animator.set("BLN_NisActive", 1.0);
                st.animator.set("BLN_NisTriggerTransition", f32::from(u8::from(ls.trigger_transition)));
                st.animator.set_physics(None);
                st.animator.update(dt);
            }
            return;
        }
        if let Some(st) = &mut self.start
            && st.running
        {
            (st.running, st.elapsed) = (false, Some(0.0));
            self.animator.reset_with_random(st.current().gs.random.clone());
        }
        if self.rewind.running {
            self.rewind.running = false;
            self.rewind.fade = self.rewind.assets.as_ref().map(|_| (0.0, self.pose.clone()));
            let random = self.animator.gs.random.clone();
            self.animator.reset_with_random(random);
            (self.producers.carving_init, self.producers.last_state) = (false, None);
        }
        self.write_signals(s, input, tricks, start, dt);
        self.animator.set_physics(physics);
        self.animator.update(dt);
        let path = self.animator.path();
        let in_loop = path.iter().any(|n| n.starts_with("state_trick_air") && n.ends_with("_loop"));
        if in_loop && !self.producers.in_loop {
            self.put("FLT_WobblyTimer", 1.0);
        }
        self.producers.in_loop = in_loop;
        match path.first().map(String::as_str) {
            Some("state_riding" | "state_bonk") => self.producers.in_air_tag = false,
            Some(n) if n.starts_with("state_airborne") || n.starts_with("state_wingsuit") => {
                self.producers.in_air_tag = true
            }
            _ => {}
        }
        if let Some(st) = &mut self.start
            && let Some(e) = st.elapsed
        {
            if st.blend.blend_time - e <= dt {
                st.elapsed = None;
            } else {
                match &mut st.intro {
                    Some(i) => i.advance(dt),
                    None => st.animator.update(dt),
                }
                st.elapsed = Some(e + dt);
            }
        }
        if let (Some((e, _)), Some(a)) = (&mut self.rewind.fade, &self.rewind.assets) {
            if a.blend.blend_time - *e <= dt {
                self.rewind.fade = None;
            } else {
                *e += dt;
            }
        }
    }

    pub fn root_motion(&self) -> Delta {
        match &self.start {
            Some(st) if st.running => match &st.intro {
                Some(i) => i.delta(),
                None => st.animator.delta(),
            },
            _ => self.animator.delta(),
        }
    }

    pub fn running_nodes(&self, dt: f32) -> Vec<String> {
        self.animator().running_nodes(dt)
    }

    pub fn finish(&mut self, s: &RiderState) -> GraphFeedback {
        let path = self.animator().path_times();
        let mut pose = match &self.start {
            Some(st) if st.running => st.anim_pose().pose,
            Some(st) if st.elapsed.is_some() => {
                let mut old = st.anim_pose();
                let w = ssx_assets::anim_eval::blend_new_weight(&st.blend, st.elapsed.unwrap_or(0.0));
                old.blend(&self.animator.anim_pose(), w);
                old.pose
            }
            _ => match (&self.rewind.fade, &self.rewind.assets) {
                (Some((e, old)), Some(a)) => {
                    let mut old = ssx_assets::anim_pose::AnimPose::full(old.clone());
                    old.blend(&self.animator.anim_pose(), ssx_assets::anim_eval::blend_new_weight(&a.blend, *e));
                    old.pose
                }
                _ => self.animator.pose(),
            },
        };
        let rotation = self.trajectory_joint_rotation(s);
        if let Some(t) = self.trajectory {
            for &r in &self.roots {
                pose.nodes[r] = SRT_IDENTITY;
            }
            pose.nodes[t] = Srt { rotation: rotation.to_array(), ..SRT_IDENTITY };
        }
        self.pose = pose;
        self.traj_rotation = rotation;
        self.updates += 1;
        self.feedback(&path, rotation)
    }

    pub(super) fn feedback(&self, path: &[(String, f32)], rotation: Quat) -> GraphFeedback {
        let ground = self.ground.filter(|_| self.pose.nodes.len() == self.rig.nodes.len()).map(|g| {
            let m = model_matrix(&self.rig, &self.pose.nodes, g);
            Vec3::new(m[3][0], m[3][1], m[3][2])
        });
        let board = self.board.filter(|_| self.pose.nodes.len() == self.rig.nodes.len()).map(|b| {
            let m = model_matrix(&self.rig, &self.pose.nodes, b);
            let axis = |r: usize| Vec3::new(m[r][0], m[r][1], m[r][2]).normalize_or_zero();
            (axis(0), axis(2), Vec3::new(m[3][0], m[3][1], m[3][2]))
        });
        let dof = self.height_dof.and_then(|i| self.pose.floats.get(i)).copied();
        let recover = path.iter().find(|n| n.0 == "state_uber_bail_recover").map(|n| n.1);
        let (board_x, board_z, board_offset) = board.unwrap_or((Vec3::X, Vec3::Z, Vec3::ZERO));
        GraphFeedback {
            driven: dof.is_some() && ground.is_some() && board.is_some(),
            traj_height_dof: dof.unwrap_or(0.0),
            ground_offset: ground.unwrap_or(Vec3::ZERO),
            board_x,
            board_z,
            board_offset,
            carving_stopped: self.signal("BLN_IsCarvingStopped") != 0.0,
            allow_airborne: self.signal("BLN_AllowAirborne") != 0.0,
            uber_bail_sliding: self.signal("BLN_uber_bail_sliding") != 0.0,
            uber_recover: recover.is_some(),
            recover_ticks: recover.unwrap_or(0.0),
            jump_start: path.first().is_some_and(|n| n.0 == "state_jump_start"),
            in_rock_bail: path.iter().any(|n| n.0 == "state_riding_rock_bail"),
            traj_rotation: rotation,
        }
    }

    pub fn record_pose(&mut self, slot: usize) {
        let nodes = &self.pose.nodes;
        let values: Vec<u32> = self
            .rewind
            .channels
            .iter()
            .map(|&(n, rotation)| match nodes.get(n) {
                Some(srt) if rotation => ghost::pack_quaternion(srt.rotation),
                Some(srt) => ghost::pack_vector(srt.translation, [rewind_pack::TRANSLATION_RANGE; 3]),
                None if rotation => ghost::pack_quaternion([0.0, 0.0, 0.0, 1.0]),
                None => ghost::pack_vector([0.0; 3], [rewind_pack::TRANSLATION_RANGE; 3]),
            })
            .collect();
        let records = &mut self.rewind.records;
        if records.len() <= slot {
            records.resize(crate::rewind::REWIND_SNAPSHOTS.max(slot + 1), None);
        }
        records[slot] = Some(values);
    }

    pub fn rewind_update(&mut self, s: &RiderState, slots: Option<(usize, usize, f32)>, dt: f32) -> GraphFeedback {
        let entering = !self.rewind.running;
        self.rewind.running = true;
        self.rewind.fade = None;
        if let Some(a) = &mut self.rewind.assets {
            if entering {
                a.animator.reset();
            }
            a.animator.set("deltaTicks", dt);
            a.animator.update(dt);
            self.pose = a.animator.pose();
        }
        let get = |i: usize| self.rewind.records.get(i).and_then(Option::as_ref);
        let record = slots.and_then(|(cur, nb, t)| {
            let c = get(cur)?;
            Some(match get(nb) {
                Some(n) => (self.rewind.channels.iter().zip(c.iter().zip(n)))
                    .map(|(&(_, rotation), (&a, &b))| {
                        if rotation {
                            rewind_pack::blend_quaternion(a, b, t)
                        } else {
                            rewind_pack::blend_vector(a, b, t, rewind_pack::TRANSLATION_RANGE)
                        }
                    })
                    .collect(),
                None => c.clone(),
            })
        });
        if let Some(values) = &record {
            for (&(n, rotation), &word) in self.rewind.channels.iter().zip(values) {
                if let Some(srt) = self.pose.nodes.get_mut(n) {
                    if rotation {
                        srt.rotation = ghost::unpack_quaternion(word);
                    } else {
                        srt.translation = ghost::unpack_vector(word, [rewind_pack::TRANSLATION_RANGE; 3]);
                    }
                }
            }
        }
        let rotation = s.traj_orientation * Quat::from_rotation_y(-FRAC_PI_2);
        if let Some(t) = self.trajectory {
            for &r in &self.roots {
                self.pose.nodes[r] = SRT_IDENTITY;
            }
            self.pose.nodes[t] = Srt { rotation: rotation.to_array(), ..SRT_IDENTITY };
        }
        self.traj_rotation = rotation;
        self.updates += 1;
        self.feedback(&[], rotation)
    }

    pub fn path(&self) -> Vec<String> {
        if self.rewind.running {
            return vec!["state_rewind".into()];
        }
        self.animator().path()
    }
}
