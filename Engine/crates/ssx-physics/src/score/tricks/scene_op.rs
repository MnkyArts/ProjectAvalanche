use super::*;

impl TrickState {
    pub fn grab_reset(&mut self) {
        self.hand = TrickHand::None;
        self.trick_type = TrickType::None;
        self.gesture_len = -1;
        self.tweak_blend = 0.0;
        self.grind.group = -1;
        self.virtual_stick_timer = -1.0;
        self.is_signature = false;
    }

    pub fn grab_exit(&mut self) {
        (self.tweak_enter, self.tweak_blend) = (0.0, 0.0);
        self.is_signature = false;
    }

    pub fn ground_trick_reset(&mut self) {
        self.hand = TrickHand::None;
        self.trick_type = TrickType::None;
        self.gesture_len = -1;
        self.grind.group = -1;
        self.virtual_stick_timer = -1.0;
        self.is_tricking = false;
    }

    pub fn script_tags(&mut self) -> ScriptCalls {
        let c = self.scripts.take();
        if c.grab_exit {
            self.grab_exit();
        }
        if c.grab_reset {
            self.grab_reset();
        }
        if c.ground_trick_reset {
            self.ground_trick_reset();
        }
        if c.air_trick_enter {
            self.input_index_on_enter = self.input_index;
        }
        c
    }

    pub fn begin_step(&mut self) {
        self.node_tags.begin_step();
        for t in [&mut self.ride_tags, &mut self.jump_tags, &mut self.riding_tags].into_iter().flatten() {
            t.begin_step();
        }
        for l in &mut self.left_sets {
            l.tags.begin_step();
        }
    }

    pub fn restart_flow(&mut self) {
        self.node_signals = true;
        self.set_node(Node::AirStates);
        (self.phase_ticks, self.rate) = (0.0, 1.0);
        self.node_tags = NodeTags::new(Node::AirStates.script());
    }

    pub fn enter_air_node(&mut self, jump: bool) {
        self.ride_tags = Some(NodeTags::new(NodeScript::OnExit(Script::GrabExit)));
        self.jump_tags = None;
        if jump {
            self.set_node(Node::AirStates);
            (self.phase_ticks, self.rate) = (0.0, 1.0);
            self.node_tags = NodeTags::new(NodeScript::None);
        } else {
            self.restart_flow();
        }
    }

    pub fn riding_node_update(&mut self, dt: f32) {
        self.riding_tags
            .get_or_insert_with(|| NodeTags::new(NodeScript::OnExit(Script::BailExit)))
            .update(dt, &mut self.scripts.now);
    }

    pub fn leave_riding_node(&mut self) {
        let Some(mut own) = self.riding_tags.take() else { return };
        if own.updates == 0 {
            own.update(0.0, &mut self.scripts.now);
        }
        own.exit(&mut self.scripts);
    }

    pub fn air_node_update(&mut self, dt: f32) {
        if let Some(t) = &mut self.ride_tags {
            t.update(dt, &mut self.scripts.now);
        }
    }

    pub fn enter_jump_airborne(&mut self) {
        self.jump_tags = Some(NodeTags::new(NodeScript::OnExit(Script::GrabReset)));
        self.restart_flow();
    }

    pub fn leave_air_node(&mut self) {
        let Some(mut own) = self.ride_tags.take() else { return };
        if own.updates == 0 {
            own.update(0.0, &mut self.scripts.now);
        }
        own.exit(&mut self.scripts);
        let air = self.left_air_tricks();
        self.left_sets.push(air);
        if let Some(j) = self.jump_tags.take() {
            self.left_sets.push(LeftSet::new(j, LeftFlow::Jump));
        }
        self.set_node(Node::AirStates);
        (self.phase_ticks, self.rate) = (0.0, 1.0);
    }

    pub fn land(&mut self) {
        self.scripts.now.grab_reset = true;
        self.leave_air_node();
    }

    pub fn mirror_rebuild(&mut self, jump: bool) {
        if self.ride_tags.is_none() {
            return;
        }
        let air = self.left_air_tricks();
        self.left_sets.push(air);
        if let Some(j) = self.jump_tags.take() {
            self.left_sets.push(LeftSet::new(j, LeftFlow::Jump));
        }
        if jump {
            self.enter_jump_airborne();
        } else {
            self.restart_flow();
        }
    }

    fn left_air_tricks(&mut self) -> LeftSet {
        let first_update_trigger =
            if self.node_tags.updates == 0 { self.node().uber_trigger_at_first_update() } else { None };
        let tags = std::mem::replace(&mut self.node_tags, NodeTags::new(NodeScript::None));
        LeftSet { first_update_trigger, ..LeftSet::new(tags, LeftFlow::AirTricks) }
    }

    pub fn left_capture(&mut self, path: &[ssx_assets::anim_eval::PathNode]) {
        for l in self.left_sets.iter_mut().filter(|l| !l.captured) {
            l.serial = path.iter().find(|n| n.flow == l.flow.name()).map(|n| n.serial);
            l.captured = true;
        }
    }

    pub fn left_step(&mut self, updated: Option<&[u64]>, dt: f32) {
        let (scripts, trigger) = (&mut self.scripts, &mut self.uber_trigger_tag);
        self.left_sets.retain_mut(|l| {
            // PORT(choice): without the rider's graph (unit tests, `ride_sim --no-graph`) no node blends out, so a
            // left set exits in the step that left it; the game always runs the graph, where the node keeps updating
            // as the old side of a blend until the blend drops it (0x82B78B50, 0x82CD7838).
            let running = match updated {
                Some(u) => l.captured && l.serial.is_some_and(|x| u.contains(&x)),
                None => false,
            };
            if running {
                if l.tags.updates == 0
                    && let Some(v) = l.first_update_trigger
                {
                    *trigger = Some(v);
                }
                l.tags.update(dt, &mut scripts.now);
            } else {
                l.tags.exit(scripts);
            }
            running
        });
    }

    pub fn enter_grind_exit(&mut self) {
        self.scripts.now.grab_reset = true;
    }

    pub fn leave_trick_grind(&mut self, advanced: bool) {
        if advanced {
            self.scripts.next.ground_trick_reset = true;
        } else {
            self.scripts.now.ground_trick_reset = true;
        }
    }

    pub fn trick_grind_exited(&mut self) {
        let tags = NodeTags { updates: 1, ..NodeTags::new(NodeScript::OnExit(Script::GroundTrickReset)) };
        self.left_sets.push(LeftSet::new(tags, LeftFlow::RideNested));
    }

    pub fn double_input_gesture(&self) -> bool {
        (self.gesture_len > 0 && !self.classic) || self.trick_type.value() > 22
    }

    pub fn fresh_ride(&mut self) {
        (self.scripts, self.ride_tags, self.jump_tags, self.riding_tags) = (ScriptQueue::default(), None, None, None);
        self.left_sets.clear();
        self.set_node(Node::AirStates);
        (self.phase_ticks, self.rate) = (0.0, 1.0);
        self.node_tags = NodeTags::new(NodeScript::None);
    }

    pub fn anticipating(&self) -> bool {
        self.node_signals
            && matches!(self.node(), Node::Anticipate | Node::Enter(TrickLevel::Normal | TrickLevel::Uber))
    }

    pub(super) fn in_loop(&self) -> bool {
        use TrickLevel::*;
        self.node_signals
            && matches!(self.node(), Node::Loop(_) | Node::TweakLoop(_) | Node::Enter(Uber | SuperUber | Signature))
    }

    pub fn ending(&self) -> bool {
        self.phase == TrickPhase::Exit || self.grind.ending()
    }

    pub fn is_grind_trick(&self) -> bool {
        self.grind.is_grind_trick()
    }

    pub fn pad_grab_end(&self) -> bool {
        !self.has_valid_input || self.input_index_on_enter != self.input_index
    }

    pub(super) fn node(&self) -> Node {
        let l = self.level;
        match (self.phase, self.tweak_node) {
            (TrickPhase::None, _) => Node::AirStates,
            (TrickPhase::Recover, _) => Node::UberCrashRecover,
            (TrickPhase::AirJump, _) => Node::AirJump,
            (TrickPhase::Anticipate, _) => Node::Anticipate,
            (TrickPhase::Enter, false) => Node::Enter(l),
            (TrickPhase::Loop, false) => Node::Loop(l),
            (TrickPhase::Exit, false) if l == TrickLevel::Signature => Node::SigExit(self.sig_exit),
            (TrickPhase::Exit, false) => Node::Exit(l),
            (TrickPhase::Enter, true) => Node::TweakEnter(l),
            (TrickPhase::Loop, true) => Node::TweakLoop(l),
            (TrickPhase::Exit, true) => Node::TweakExit(l),
        }
    }

    pub(super) fn set_node(&mut self, n: Node) {
        use TrickPhase::*;
        (self.phase, self.level, self.tweak_node) = match n {
            Node::AirStates => (None, TrickLevel::Normal, false),
            Node::UberCrashRecover => (Recover, TrickLevel::Normal, false),
            Node::AirJump => (TrickPhase::AirJump, TrickLevel::Normal, false),
            Node::Anticipate => (Anticipate, TrickLevel::Normal, false),
            Node::Enter(l) => (Enter, l, false),
            Node::Loop(l) => (Loop, l, false),
            Node::Exit(l) => (Exit, l, false),
            Node::SigExit(k) => {
                self.sig_exit = k;
                (Exit, TrickLevel::Signature, false)
            }
            Node::TweakEnter(l) => (Enter, l, true),
            Node::TweakLoop(l) => (Loop, l, true),
            Node::TweakExit(l) => (Exit, l, true),
        };
    }

    pub fn update(&mut self, t: &TrickTuning, inp: &TrickInput, ctx: &TrickContext, dt: f32) {
        self.scene_op(t, inp, ctx, dt);
        self.step_flows(ctx, dt);
    }

    pub fn update_reset(&mut self, t: &TrickTuning, inp: &TrickInput, ctx: &TrickContext, dt: f32) {
        self.scene_op_reset(t, inp, ctx, dt);
        self.step_flows(ctx, dt);
    }

    fn step_flows(&mut self, ctx: &TrickContext, dt: f32) {
        if ctx.in_air {
            self.air_node_update(dt);
            self.flow(ctx, dt);
        }
        self.left_step(None, dt);
        self.script_tags();
    }

    pub fn scene_op(&mut self, t: &TrickTuning, inp: &TrickInput, ctx: &TrickContext, dt: f32) {
        self.begin_step();
        let pad = self.gesture_ticks(t, inp, ctx, dt);
        self.trick_scene_op(t, inp, &pad, ctx, dt);
    }

    pub fn scene_op_reset(&mut self, t: &TrickTuning, inp: &TrickInput, ctx: &TrickContext, dt: f32) {
        self.begin_step();
        self.gesture_ticks(t, inp, ctx, dt);
        self.reset_trick_state();
    }

    pub fn flow(&mut self, ctx: &TrickContext, dt: f32) {
        if let Some(j) = &mut self.jump_tags {
            j.update(dt, &mut self.scripts.now);
        }
        self.state_flow(ctx, dt);
    }

    pub fn reset_trick_state(&mut self) {
        (self.tweak_blend, self.tweak_enter, self.tweak_wind) = (0.0, 0.0, 0.0);
        (self.virtual_stick, self.virtual_stick_timer) = (Vec2::ZERO, 0.0);
        (self.is_signature, self.trick_locked, self.was_air_starting, self.node_signals) = (false, false, false, false);
        (self.trick_type, self.trick_type_locked, self.hand, self.gesture_len) =
            (TrickType::None, TrickType::None, TrickHand::None, -1);
        let g = &mut self.grind;
        (g.group, g.group_locked, g.gesture_len_locked, g.index, g.enabled) = (-1, -1, -1, -1, true);
    }

    pub(super) fn gesture_ticks(
        &mut self,
        t: &TrickTuning,
        inp: &TrickInput,
        ctx: &TrickContext,
        _dt: f32,
    ) -> PadSample {
        let pad = PadSample::new(inp);
        let gates = Gates {
            in_air: ctx.in_air,
            up_start: self.valid_up_start,
            left_start: self.valid_left_start,
            right_start: self.valid_right_start,
        };
        self.gestures.tick(pad, &gates, &t.added, self.classic);
        let (prev, cur) = (self.gestures.prev, self.gestures.cur);
        self.grind.tick_gestures(&prev, &cur, ctx.is_grinding, self.classic);
        self.press.tick(&prev, &cur, self.classic);
        self.gesture_clock += 1.0;
        pad
    }

    pub(super) fn trick_scene_op(
        &mut self,
        t: &TrickTuning,
        inp: &TrickInput,
        pad: &PadSample,
        ctx: &TrickContext,
        dt: f32,
    ) {
        let (air_starting_node, in_loop) = (ctx.in_air && self.anticipating(), self.in_loop());
        let pad_end = self.pad_grab_end();
        self.ticks_since_sig_held = if !self.is_signature || pad_end { self.ticks_since_sig_held + dt } else { 0.0 };
        if self.hand == TrickHand::None || in_loop {
            self.update_virtual_stick(t, inp, dt);
        }
        let rs_mag = rstick_mag(inp.right_stick);
        let stick_active = self.virtual_stick_timer >= 0.0 || rs_mag > RSTICK_ACTIVE;
        self.trick_locked = false;
        self.is_signature = false;
        self.grind.starting = false;
        if ctx.in_air {
            if air_starting_node {
                self.choose_air_trick(t, inp, pad, ctx, dt);
                if self.gesture_len == 2 {
                    self.update_tweak_and_lock(pad, rs_mag, t);
                }
            } else if in_loop {
                self.update_tweak_and_lock(pad, rs_mag, t);
            }
            let (valid, idx) = self.valid_air_trick_input(&t.added, ctx.goofy);
            self.has_valid_input = valid;
            if valid {
                self.input_index = idx;
            }
        } else if ctx.is_grinding {
            self.grind_branch(t, inp, ctx, dt);
        }
        if self.trick_locked {
            self.trick_type_locked = self.trick_type;
        }
        self.is_signature = self.trick_type == TrickType::Signature;
        self.is_tricking = self.trick_type != TrickType::None;
        self.is_tweaking = self.tweak_blend.abs() >= TWEAK_BLEND_ACTIVE && stick_active && !self.is_signature;
        self.was_air_starting = air_starting_node;
        let b = &self.gestures.button;
        self.valid_up_start = ctx.in_air && !b[RIGHT_UP] && !b[LEFT_UP];
        self.valid_left_start = ctx.in_air && !b[UP_LEFT] && !b[RIGHT_LEFT];
        self.valid_right_start = ctx.in_air && !b[UP_RIGHT] && !b[LEFT_RIGHT];
    }

    pub(super) fn grind_branch(&mut self, t: &TrickTuning, inp: &TrickInput, ctx: &TrickContext, dt: f32) {
        let prev = self.trick_type;
        let was_tricking = prev != TrickType::None;
        let gl_locked = self.grind.gesture_len_locked;
        self.choose_grind_trick(t, inp, ctx, dt);
        self.input_index = self.grind.index;
        let tt = self.trick_type;
        let valid = self.grind.enabled && self.grind.button.iter().any(|&b| b);
        self.has_valid_input = valid && tt == prev;
        self.grind.starting = !was_tricking && tt.is_grind();
        self.trick_locked = if self.ending() {
            false
        } else if !was_tricking || (!self.grind.is_grind_trick_enter() && self.has_valid_input) {
            true
        } else if self.classic || self.gesture_len <= gl_locked {
            false
        } else {
            self.grind.group == self.grind.group_locked || self.grind.group_locked == -1
        };
        if self.trick_locked {
            self.input_index_on_enter = self.grind.index;
            self.grind.group_locked = self.grind.group;
            self.grind.gesture_len_locked = self.gesture_len;
        }
    }

    pub(super) fn choose_grind_trick(&mut self, t: &TrickTuning, inp: &TrickInput, ctx: &TrickContext, dt: f32) {
        let g = &self.grind;
        if !g.enabled {
            return;
        }
        let prev_held = g.index > -1 && g.button[g.index as usize];
        let (gl, gg) = (self.gesture_len, g.group);
        let table: &[crate::rider::grind::tricks::GrindEntry] =
            if self.classic { &GRIND_TRICKS_CLASSIC } else { &GRIND_TRICKS };
        for (i, e) in table.iter().enumerate() {
            if self.grind.button[i] && (e.len > gl || (!prev_held && e.group != gg && (!self.classic || gl < 2))) {
                self.trick_type = e.trick(ctx.awesome, ctx.super_awesome);
                self.grind.group = e.group;
                self.gesture_len = e.len;
                self.hand = self.trick_type.hand();
                self.update_virtual_stick(t, inp, dt);
                self.grind.index = i as i32;
                return;
            }
        }
        if !prev_held {
            self.grind.index = -1;
        }
    }

    pub(super) fn valid_air_trick_input(&self, added: &AddedAirTricks, goofy: bool) -> (bool, i32) {
        let g = &self.gestures;
        let table = air_table(self.classic);
        for i in 0..table.len() {
            if g.button[i] {
                return (true, i as i32);
            }
            if g.stick[i] {
                return (true, -1);
            }
        }
        for i in 0..added.by_input.len() {
            if added.trick_for(i, goofy).is_some() {
                if g.added.button[i] {
                    return (true, (table.len() + i) as i32);
                }
                if g.added.stick[i] {
                    return (true, -1);
                }
            }
        }
        (false, -1)
    }

    pub(super) fn choose_air_trick(
        &mut self,
        t: &TrickTuning,
        inp: &TrickInput,
        pad: &PadSample,
        ctx: &TrickContext,
        dt: f32,
    ) {
        if self.hand == TrickHand::None {
            self.gesture_len = -1;
        }
        let mut found = false;
        for (i, e) in air_table(self.classic).iter().enumerate() {
            let button = self.gestures.button[i];
            if e.len > self.gesture_len && (button || self.gestures.stick[i]) {
                self.trick_type = if ctx.goofy { e.goofy } else { e.regular };
                self.gesture_len = e.len;
                self.hand = self.trick_type.hand();
                if button {
                    self.update_virtual_stick(t, inp, dt);
                }
                found = true;
                break;
            }
        }
        if !found {
            for i in 0..t.added.by_input.len() {
                let Some(trick) = t.added.trick_for(i, ctx.goofy) else { continue };
                let button = self.gestures.added.button[i];
                if ADDED_LEN > self.gesture_len && (button || self.gestures.added.stick[i]) {
                    (self.trick_type, self.gesture_len, self.hand) = (trick, ADDED_LEN, trick.hand());
                    if button {
                        self.update_virtual_stick(t, inp, dt);
                    }
                    found = true;
                    break;
                }
            }
        }
        if !found {
            return;
        }
        let signature = if self.classic { self.gestures.classic_signature } else { self.gestures.signature };
        if ctx.super_awesome && signature {
            (self.is_signature, self.hand, self.trick_type, self.gesture_len) =
                (true, TrickHand::Both, TrickType::Signature, 2);
        }
        self.tweak_blend = 0.0;
        let a = self.tweak_reference_angle(pad);
        (self.tweak_enter, self.tweak_wind) = (a, a);
    }

    pub(super) fn tweak_reference_angle(&self, pad: &PadSample) -> f32 {
        if self.virtual_stick_timer < 0.0 { pad.angle } else { self.virtual_stick.y.atan2(self.virtual_stick.x) }
    }

    pub(super) fn update_tweak_and_lock(&mut self, pad: &PadSample, rs_mag: f32, t: &TrickTuning) {
        if self.was_air_starting {
            self.tweak_blend = 0.0;
            let a = self.tweak_reference_angle(pad);
            (self.tweak_enter, self.tweak_wind) = (a, a);
        }
        self.trick_locked = true;
        if self.classic {
            self.tweak_blend = if self.gestures.tweak { 1.0 } else { 0.0 };
            return;
        }
        if self.gestures.tweak {
            self.tweak_blend = 1.0;
        } else if rs_mag > TWEAK_STICK_DEAD_ZONE {
            self.tweak_wind += wrap_angle(pad.angle - self.tweak_wind);
            let th = t.tweak_switch_threshold;
            let d = wrap_angle(self.tweak_wind - self.tweak_enter);
            self.tweak_blend = if d > th {
                1.0
            } else if d < -th {
                -1.0
            } else {
                0.0
            };
            let excess = if d > 2.0 * th {
                d - 2.0 * th
            } else if d < -2.0 * th {
                d + 2.0 * th
            } else {
                0.0
            };
            self.tweak_enter = wrap_angle(self.tweak_enter + excess);
        } else {
            self.tweak_blend = 0.0;
        }
    }

    pub(super) fn update_virtual_stick(&mut self, t: &TrickTuning, inp: &TrickInput, dt: f32) {
        self.virtual_stick_timer -= dt;
        let v = &mut self.virtual_stick;
        let pressed = if inp.y {
            v.y = 1.0;
            true
        } else if inp.a {
            v.y = -1.0;
            true
        } else if inp.b {
            v.x = 1.0;
            true
        } else if inp.x {
            v.x = -1.0;
            true
        } else {
            false
        };
        if pressed {
            self.virtual_stick_timer = t.virtual_stick_persist_time;
        }
        if self.virtual_stick_timer < 0.0 {
            self.virtual_stick = Vec2::ZERO;
        }
    }
}
