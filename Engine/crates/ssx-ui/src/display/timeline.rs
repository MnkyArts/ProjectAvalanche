use super::*;

pub(super) fn merge_place(a: &mut PlaceObject, b: &PlaceObject) {
    if b.character_id.is_some() {
        a.character_id = b.character_id;
    }
    if b.matrix.is_some() {
        a.matrix = b.matrix;
    }
    if b.color_transform.is_some() {
        a.color_transform = b.color_transform;
    }
    if b.ratio.is_some() {
        a.ratio = b.ratio;
    }
    if b.name.is_some() {
        a.name = b.name.clone();
    }
    if b.clip_depth.is_some() {
        a.clip_depth = b.clip_depth;
    }
    if b.flags3 & 0x01 != 0 {
        a.filters = b.filters.clone();
    }
    if b.blend_mode.is_some() {
        a.blend_mode = b.blend_mode;
    }
    if b.cache_as_bitmap.is_some() {
        a.cache_as_bitmap = b.cache_as_bitmap;
    }
    if b.visible.is_some() {
        a.visible = b.visible;
    }
    if b.clip_actions.is_some() {
        a.clip_actions = b.clip_actions.clone();
    }
    a.flags3 |= b.flags3;
}

impl Player {
    pub fn load_level(&mut self, lib: &Library, movie: MovieId, level: i32) -> InstanceId {
        if let Some(old) = self.levels.remove(&level) {
            self.free_tree(old);
        }
        let root = lib.movie(movie).root.clone();
        let inst = Instance {
            parent: None,
            depth: level,
            character: None,
            movie,
            name: format!("_level{level}"),
            matrix: Mat2d::IDENTITY,
            cxform: Cxform::IDENTITY,
            ratio: 0.0,
            clip_depth: None,
            blend: BlendMode::Normal,
            filter: None,
            visible: true,
            cache_as_bitmap: false,
            placed_frame: None,
            transformed_by_script: false,
            geom: None,
            clip_actions: None,
            scale9: None,
            kind: InstanceKind::Sprite(SpriteState {
                timeline: root,
                current_frame: 0,
                playing: true,
                children: BTreeMap::new(),
            }),
            scripted: false,
            created_tick: self.tick,
            owner: lib.origin(movie).map(|_| movie).or(self.making_for),
        };
        let id = self.alloc(inst);
        self.levels.insert(level, id);
        self.run_frame(lib, id, 1);
        id
    }

    pub fn advance(&mut self, lib: &Library, dt: f64, catch_up: u32) -> u32 {
        let n = self.frames_due(lib, dt, catch_up);
        for _ in 0..n {
            self.advance_frame(lib);
        }
        n
    }

    pub fn frames_due(&mut self, lib: &Library, dt: f64, catch_up: u32) -> u32 {
        let Some(root) = self.levels.values().next().and_then(|r| self.get(*r)) else { return 0 };
        let frame_time = 1.0 / f64::from(lib.movie(root.movie).frame_rate).max(1.0);
        self.time += dt.max(0.0);
        let mut n = 0;
        while self.time >= frame_time && n <= catch_up {
            self.time -= frame_time;
            n += 1;
        }
        n
    }

    pub fn tick(&mut self, lib: &Library, dt: f64) -> u32 {
        self.advance(lib, dt, 2)
    }

    pub fn advance_frame(&mut self, lib: &Library) {
        self.relayout_pending(lib);
        self.tick += 1;
        let roots: Vec<InstanceId> = self.levels.values().copied().collect();
        for r in roots {
            self.advance_tree(lib, r);
        }
    }

    pub(super) fn advance_tree(&mut self, lib: &Library, id: InstanceId) {
        let Some(inst) = self.get(id) else { return };
        if inst.created_tick == self.tick {
            return;
        }
        // A hidden sprite is dropped from the play list (`Sprite::SetVisible` 0x8268CEA0 clears its bit 0x80000
        // through 0x8268C000; `GFxMovieImpl::AdvanceFrame` 0x82644CF8 only calls the advance slot +0xFC on the
        // clips in it), so its own timeline and enter-frame events stop. Its children keep their own flags and are
        // listed on their own (`frontend.md` §10.2); hiding this sprite copied its flag to them through 0x8268C160
        // unless they set `_visible` again.
        let frozen = self.no_invisible_advance && !inst.visible;
        let Some(s) = inst.sprite() else { return };
        let (count, cur, playing) = (s.timeline.frame_count(), s.current_frame, s.playing);
        if inst.scripted && self.script_events && !frozen {
            self.actions.push(QueuedAction::EnterFrame { instance: id });
        }
        if playing && count > 1 && !frozen {
            if cur >= count {
                self.run_goto(lib, id, 1);
            } else {
                self.run_frame(lib, id, cur + 1);
            }
        }
        for (_, c) in self.children(id) {
            self.advance_tree(lib, c);
        }
    }

    pub(super) fn run_frame(&mut self, lib: &Library, id: InstanceId, f: u16) {
        let Some(s) = self.get_mut(id).and_then(Instance::sprite_mut) else { return };
        s.current_frame = f;
        let tl = s.timeline.clone();
        let Some(frame) = tl.frames.get(usize::from(f) - 1) else { return };
        for tag in &frame.tags {
            match tag {
                FrameTag::Place(p) => self.place(lib, id, tl.movie, p, f, false),
                FrameTag::Remove(d) => {
                    if let Some(c) = self.child_at_depth(id, i32::from(*d) + TIMELINE_DEPTH_OFFSET) {
                        self.remove(c);
                    }
                }
            }
        }
        self.queue_frame_actions(id, &tl, f);
    }

    pub(super) fn queue_frame_actions(&mut self, id: InstanceId, tl: &TimelineDef, f: u16) {
        let Some(frame) = tl.frames.get(usize::from(f) - 1) else { return };
        if tl.id == 0 {
            for (sprite_id, code) in &frame.init_actions {
                if self.init_done.insert((tl.movie, *sprite_id)) {
                    self.actions.push(QueuedAction::Init {
                        instance: id,
                        sprite_id: *sprite_id,
                        actions: code.clone(),
                    });
                }
            }
        }
        for a in &frame.actions {
            self.actions.push(QueuedAction::Frame { instance: id, actions: a.clone() });
        }
    }

    pub(super) fn place(
        &mut self,
        lib: &Library,
        parent: InstanceId,
        movie: MovieId,
        p: &PlaceObject,
        frame: u16,
        rewind: bool,
    ) {
        let depth = i32::from(p.depth) + TIMELINE_DEPTH_OFFSET;
        let existing = self.child_at_depth(parent, depth);
        let is_move = p.flags & PlaceObject::MOVE != 0;
        let Some(cid) = p.character_id else {
            if is_move && let Some(e) = existing {
                self.apply_place(e, p, false);
            }
            return;
        };
        let r = CharRef { movie, id: cid };
        let resolved = lib.resolve(r).map(|(x, _)| x);
        let same = existing.filter(|e| self.get(*e).is_some_and(|i| i.character.is_some() && i.character == resolved));
        if let Some(e) = same.filter(|_| is_move || rewind) {
            if !is_move {
                self.reset_placement(e);
            }
            self.apply_place(e, p, false);
            return;
        }
        let old = existing
            .and_then(|e| self.get(e))
            .map(|i| (i.matrix, i.cxform, i.ratio, i.name.clone(), i.clip_depth, i.blend, i.filter));
        let Some(new) = self.instantiate(lib, Some(parent), depth, r) else {
            self.unresolved.push((movie, cid));
            return;
        };
        if let Some(i) = self.get_mut(new) {
            if is_move && let Some((m, c, ratio, name, clip, blend, filter)) = old {
                (i.matrix, i.cxform, i.ratio, i.name, i.clip_depth, i.blend, i.filter) =
                    (m, c, ratio, name, clip, blend, filter);
            }
            i.placed_frame = Some(frame);
        }
        self.apply_place(new, p, true);
        self.auto_name(new);
        let sprite = self.get(new).is_some_and(|i| i.sprite().is_some());
        self.start(lib, new);
        // TODO(guess): a placed sprite's construction is queued after its first frame (children first, post-order: parent first, 3 views' component constructors call methods — `SetDimmer`, `SetHeader` — of child panels not yet of their class, 6 calls over all movies; post-order fixes those 6 and breaks no call on the disc; Scaleform's queue executor was not located); settle: AvmSprite's construction order.
        if sprite && self.script_events {
            self.actions.push(QueuedAction::Construct { instance: new });
            self.actions.push(QueuedAction::Load { instance: new });
        }
    }

    pub(super) fn reset_placement(&mut self, id: InstanceId) {
        let Some(i) = self.get_mut(id) else { return };
        if !i.transformed_by_script {
            i.matrix = Mat2d::IDENTITY;
            i.cxform = Cxform::IDENTITY;
        }
        i.ratio = 0.0;
        i.blend = BlendMode::Normal;
        i.filter = None;
        i.cache_as_bitmap = false;
    }

    pub(super) fn apply_place(&mut self, id: InstanceId, p: &PlaceObject, new: bool) {
        let Some(i) = self.get_mut(id) else { return };
        let timeline_transform = new || !i.transformed_by_script;
        if let Some(m) = &p.matrix
            && timeline_transform
        {
            i.matrix = Mat2d::from_swf(m);
        }
        if let Some(c) = &p.color_transform
            && timeline_transform
        {
            i.cxform = Cxform::from_swf(c);
        }
        if let Some(r) = p.ratio {
            i.ratio = f32::from(r) / 65535.0;
        }
        if let Some(n) = &p.name {
            i.name = n.clone();
        }
        if let Some(cd) = p.clip_depth {
            i.clip_depth = Some(i32::from(cd) + TIMELINE_DEPTH_OFFSET);
        }
        if let Some(b) = p.blend_mode {
            i.blend = BlendMode::from_swf(b);
        }
        if p.flags3 & 0x01 != 0 {
            i.filter = TextFilter::from_filters(&p.filters);
        }
        if let Some(c) = p.cache_as_bitmap {
            i.cache_as_bitmap = c != 0;
        }
        if let Some(v) = p.visible {
            i.visible = v != 0;
        }
        if let Some(ca) = &p.clip_actions {
            i.clip_actions = Some(Arc::new(ca.clone()));
        }
    }

    pub(super) fn run_goto(&mut self, lib: &Library, id: InstanceId, target: u16) {
        let Some(s) = self.get(id).and_then(Instance::sprite) else { return };
        let tl = s.timeline.clone();
        let cur = s.current_frame;
        let target = target.clamp(1, tl.frame_count());
        if target == cur {
            return;
        }
        let rewind = target < cur;
        if rewind {
            let doomed: Vec<InstanceId> = s
                .children
                .values()
                .copied()
                .filter(|c| self.get(*c).and_then(|i| i.placed_frame).is_some_and(|f| f > target))
                .collect();
            for c in doomed {
                self.remove(c);
            }
        }
        let from = if rewind { 1 } else { cur + 1 };
        let mut merged: BTreeMap<i32, GotoPlace> = BTreeMap::new();
        for f in from..=target {
            for tag in &tl.frames[usize::from(f) - 1].tags {
                match tag {
                    FrameTag::Place(p) => {
                        let depth = i32::from(p.depth) + TIMELINE_DEPTH_OFFSET;
                        let fresh = p.character_id.is_some() && p.flags & PlaceObject::MOVE == 0;
                        match merged.get_mut(&depth) {
                            Some(g) if !fresh => merge_place(&mut g.tag, p),
                            _ => drop(merged.insert(depth, GotoPlace { tag: (**p).clone(), frame: f })),
                        }
                    }
                    FrameTag::Remove(d) => {
                        let depth = i32::from(*d) + TIMELINE_DEPTH_OFFSET;
                        merged.remove(&depth);
                        if let Some(c) = self.child_at_depth(id, depth)
                            && self.get(c).and_then(|i| i.placed_frame).is_none_or(|pf| pf < f || !rewind)
                        {
                            self.remove(c);
                        }
                    }
                }
            }
        }
        if let Some(s) = self.get_mut(id).and_then(Instance::sprite_mut) {
            s.current_frame = target;
        }
        for g in merged.into_values() {
            self.place(lib, id, tl.movie, &g.tag, g.frame, rewind);
        }
        self.queue_frame_actions(id, &tl, target);
    }

    pub fn goto_frame(&mut self, lib: &Library, id: InstanceId, frame: u16, play: bool) {
        self.run_goto(lib, id, frame);
        if let Some(s) = self.get_mut(id).and_then(Instance::sprite_mut) {
            s.playing = play;
        }
    }

    pub fn goto_label(&mut self, lib: &Library, id: InstanceId, label: &str, play: bool) -> bool {
        let Some(f) = self.get(id).and_then(Instance::sprite).and_then(|s| s.timeline.label_frame(label)) else {
            return false;
        };
        self.goto_frame(lib, id, f, play);
        true
    }

    // TODO(guess): names given when a timeline places the instance, to sprites and text fields only; settle: the callers of the virtual 0x8266CB08 (seven vtables).
    pub(super) fn auto_name(&mut self, id: InstanceId) {
        let n = self.instance_names + 1;
        let Some(i) = self.get_mut(id) else { return };
        if i.name.is_empty() && matches!(i.kind, InstanceKind::Sprite(_) | InstanceKind::EditText(_)) {
            i.name = format!("instance{n}");
            self.instance_names = n;
        }
    }

    pub fn goto(&mut self, lib: &Library, id: InstanceId, frame: u16) {
        self.run_goto(lib, id, frame);
    }

    pub fn label_frame(&self, id: InstanceId, label: &str) -> Option<u16> {
        self.get(id).and_then(Instance::sprite).and_then(|s| s.timeline.label_frame(label))
    }

    pub fn is_playing(&self, id: InstanceId) -> bool {
        self.get(id).and_then(Instance::sprite).is_some_and(|s| s.playing)
    }
}
