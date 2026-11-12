use super::*;

impl Player {
    pub fn set_name(&mut self, id: InstanceId, name: &str) {
        if let Some(i) = self.get_mut(id) {
            i.name = name.to_string();
        }
    }

    pub fn skip_next_advance(&mut self, id: InstanceId) {
        let next = self.tick + 1;
        if let Some(i) = self.get_mut(id) {
            i.created_tick = next;
        }
    }

    pub fn set_scripted(&mut self, id: InstanceId) {
        if let Some(i) = self.get_mut(id) {
            i.scripted = true;
        }
    }

    pub fn root_of(&self, id: InstanceId) -> InstanceId {
        let mut cur = id;
        for _ in 0..1024 {
            match self.get(cur).and_then(|i| i.parent) {
                Some(p) => cur = p,
                None => break,
            }
        }
        cur
    }

    pub fn load_into(&mut self, lib: &Library, id: InstanceId, movie: MovieId) {
        self.init_done.retain(|&(m, _)| m != movie);
        let root = lib.movie(movie).root.clone();
        let next = self.tick + 1;
        let Some(old_owner) = self.get(id).map(|i| i.owner) else { return };
        let owner = lib.origin(movie).map(|_| movie).or(self.making_for).or(old_owner);
        if owner != old_owner {
            self.disown(old_owner);
            if let Some(o) = owner {
                *self.owned.entry(o).or_default() += 1;
            }
        }
        let Some(inst) = self.get_mut(id) else { return };
        inst.owner = owner;
        let old = std::mem::replace(
            &mut inst.kind,
            InstanceKind::Sprite(SpriteState {
                timeline: root,
                current_frame: 0,
                playing: true,
                children: BTreeMap::new(),
            }),
        );
        inst.movie = movie;
        inst.character = None;
        inst.scale9 = None;
        inst.created_tick = next;
        if let InstanceKind::Sprite(s) = old {
            for c in s.children.into_values() {
                self.free_tree(c);
            }
        }
        self.run_frame(lib, id, 1);
    }

    pub fn play(&mut self, id: InstanceId) {
        if let Some(s) = self.get_mut(id).and_then(Instance::sprite_mut) {
            s.playing = true;
        }
    }

    pub fn stop(&mut self, id: InstanceId) {
        if let Some(s) = self.get_mut(id).and_then(Instance::sprite_mut) {
            s.playing = false;
        }
    }

    pub fn step(&mut self, lib: &Library, id: InstanceId, forward: bool) {
        let Some(cur) = self.get(id).and_then(Instance::sprite).map(|s| s.current_frame) else { return };
        let target = if forward { cur.saturating_add(1) } else { cur.saturating_sub(1).max(1) };
        self.goto_frame(lib, id, target, false);
    }

    pub fn goto_everywhere(&mut self, lib: &Library, id: InstanceId, frame: u16) {
        if let Some((_, total)) = self.frames(id) {
            self.goto_frame(lib, id, frame.min(total), false);
        }
        for (_, c) in self.children(id) {
            self.goto_everywhere(lib, c, frame);
        }
    }

    pub fn local_bounds(&self, lib: &Library, id: InstanceId) -> RectF {
        let Some(inst) = self.get(id) else { return RectF::EMPTY };
        let ch = inst.character.and_then(|r| lib.resolve(r)).map(|(_, c)| c);
        match (&inst.kind, ch) {
            (InstanceKind::Sprite(s), _) => s.children.values().fold(RectF::EMPTY, |acc, &c| {
                let m = self.get(c).map_or(Mat2d::IDENTITY, |i| i.matrix);
                acc.union(&self.local_bounds(lib, c).transform(&m))
            }),
            (InstanceKind::EditText(t), _) => t.bounds,
            (_, Some(Character::Shape(s))) => RectF::from_swf(&s.bounds),
            (_, Some(Character::Morph(m))) => {
                let (a, b) = (RectF::from_swf(&m.start_bounds), RectF::from_swf(&m.end_bounds));
                let l = |x: f32, y: f32| x + (y - x) * inst.ratio;
                RectF::new(l(a.x0, b.x0), l(a.y0, b.y0), l(a.x1, b.x1), l(a.y1, b.y1))
            }
            (_, Some(Character::Text(t))) => RectF::from_swf(&t.bounds).transform(&Mat2d::from_swf(&t.matrix)),
            (_, Some(Character::Bitmap(b))) => RectF::new(0.0, 0.0, b.size[0] as f32 * 20.0, b.size[1] as f32 * 20.0),
            _ => RectF::EMPTY,
        }
    }

    pub fn frames(&self, id: InstanceId) -> Option<(u16, u16)> {
        self.get(id).and_then(Instance::sprite).map(|s| (s.current_frame, s.timeline.frame_count()))
    }

    pub fn attach_movie(
        &mut self,
        lib: &Library,
        parent: InstanceId,
        export: &str,
        name: &str,
        depth: i32,
    ) -> Option<InstanceId> {
        let movie = self.get(parent)?.movie;
        let r = lib.export(movie, export).or_else(|| {
            let live: HashSet<MovieId> =
                self.slots.iter().filter_map(|s| s.instance.as_ref().map(|i| i.movie)).collect();
            lib.export_from_importers(movie, export, |m| live.contains(&m))
        })?;
        self.remove_at(parent, depth);
        let id = self.instantiate(lib, Some(parent), depth, r)?;
        if let Some(i) = self.get_mut(id) {
            i.name = name.to_string();
        }
        self.start(lib, id);
        Some(id)
    }

    pub fn place_character(&mut self, lib: &Library, parent: InstanceId, r: CharRef, depth: i32) -> Option<InstanceId> {
        self.remove_at(parent, depth);
        let id = self.instantiate(lib, Some(parent), depth, r)?;
        self.start(lib, id);
        Some(id)
    }

    pub fn create_empty_movie_clip(&mut self, parent: InstanceId, name: &str, depth: i32) -> Option<InstanceId> {
        let movie = self.get(parent)?.movie;
        self.get(parent)?.sprite()?;
        self.remove_at(parent, depth);
        let owner = self.owner_for(Some(parent), None);
        if !self.room_for(Some(parent), owner) {
            return None;
        }
        let timeline = Arc::new(TimelineDef { movie, id: 0xFFFF, frames: vec![Default::default()] });
        let inst = Instance {
            parent: Some(parent),
            depth,
            character: None,
            movie,
            name: name.to_string(),
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
                timeline,
                current_frame: 1,
                playing: false,
                children: BTreeMap::new(),
            }),
            scripted: false,
            created_tick: self.tick,
            owner,
        };
        let id = self.alloc(inst);
        self.get_mut(parent)?.sprite_mut()?.children.insert(depth, id);
        Some(id)
    }

    pub fn duplicate_movie_clip(
        &mut self,
        lib: &Library,
        id: InstanceId,
        name: &str,
        depth: i32,
    ) -> Option<InstanceId> {
        let src = self.get(id)?.clone();
        let parent = src.parent?;
        let r = src.character?;
        self.remove_at(parent, depth);
        let new = self.instantiate(lib, Some(parent), depth, r)?;
        if let Some(i) = self.get_mut(new) {
            i.name = name.to_string();
            i.matrix = src.matrix;
            i.cxform = src.cxform;
            i.ratio = src.ratio;
            i.blend = src.blend;
            i.filter = src.filter;
            i.visible = src.visible;
            i.clip_actions = src.clip_actions.clone();
            i.transformed_by_script = true;
        }
        self.start(lib, new);
        Some(new)
    }

    pub(super) fn remove_at(&mut self, parent: InstanceId, depth: i32) {
        if let Some(old) = self.child_at_depth(parent, depth) {
            self.remove(old);
        }
    }

    pub fn swap_depths(&mut self, id: InstanceId, depth: i32) {
        let Some(inst) = self.get(id) else { return };
        let (Some(parent), old_depth) = (inst.parent, inst.depth) else { return };
        let Some(s) = self.get_mut(parent).and_then(Instance::sprite_mut) else { return };
        let other = s.children.remove(&depth);
        s.children.remove(&old_depth);
        s.children.insert(depth, id);
        if let Some(o) = other {
            s.children.insert(old_depth, o);
        }
        if let Some(o) = other.and_then(|o| self.get_mut(o)) {
            o.depth = old_depth;
            o.transformed_by_script = true;
        }
        if let Some(i) = self.get_mut(id) {
            i.depth = depth;
            i.transformed_by_script = true;
        }
    }

    pub fn next_highest_depth(&self, parent: InstanceId) -> i32 {
        self.get(parent)
            .and_then(Instance::sprite)
            .and_then(|s| s.children.keys().next_back().copied())
            .map_or(0, |d| (d + 1).max(0))
    }

    pub fn ensure_geom(&mut self, id: InstanceId) -> Option<GeomData> {
        let i = self.get_mut(id)?;
        i.transformed_by_script = true;
        let m = i.matrix;
        Some(*i.geom.get_or_insert_with(|| GeomData::from_matrix(&m)))
    }

    pub fn set_geom(&mut self, id: InstanceId, g: GeomData, m: Mat2d) {
        if let Some(i) = self.get_mut(id) {
            i.geom = Some(g);
            if matrix_in_range(&m) {
                i.matrix = m;
            }
            i.transformed_by_script = true;
        }
    }

    pub fn set_matrix(&mut self, id: InstanceId, m: Mat2d) {
        if let Some(i) = self.get_mut(id) {
            i.matrix = m;
            i.transformed_by_script = true;
        }
    }

    pub fn set_cxform(&mut self, id: InstanceId, c: Cxform) {
        if let Some(i) = self.get_mut(id) {
            i.cxform = c;
            i.transformed_by_script = true;
        }
    }

    pub fn set_filter(&mut self, id: InstanceId, f: Option<TextFilter>) {
        if let Some(i) = self.get_mut(id) {
            i.filter = f;
        }
    }

    /// `_visible` (`Sprite::SetVisible` 0x8268CEA0): the clip's own flag, and — with `noInvisibleAdvance` on — copied
    /// to every child sprite through slot +0x184 (0x8268C160) when the clip's parent is visible (the parent's own flag
    /// clear). A hidden parent therefore freezes its subtree; a child set visible again clears only its own flag (its
    /// parent is hidden, so the copy stops there), which is how the equipment's hidden image buffer keeps its loader's
    /// `onEnterFrame` alive (`frontend.md` §10.2).
    pub fn set_visible(&mut self, id: InstanceId, v: bool) {
        let Some(i) = self.get(id) else { return };
        if i.visible == v {
            return;
        }
        let parent = i.parent;
        if let Some(i) = self.get_mut(id) {
            i.visible = v;
        }
        let copy_down = parent.is_some_and(|p| self.get(p).is_some_and(|p| p.visible));
        if !self.no_invisible_advance || !copy_down {
            return;
        }
        let mut stack = vec![id];
        while let Some(p) = stack.pop() {
            let children: Vec<InstanceId> = self.children(p).into_iter().map(|(_, c)| c).collect();
            for c in children {
                if let Some(ci) = self.get_mut(c) {
                    ci.visible = v;
                }
                stack.push(c);
            }
        }
    }

    pub fn set_text(&mut self, lib: &Library, id: InstanceId, source: &str, html: bool) {
        let Some(i) = self.get_mut(id) else { return };
        let movie = i.movie;
        if let InstanceKind::EditText(t) = &mut i.kind {
            t.source = source.to_string();
            t.html = html && t.def.flags2 & EditText::HTML != 0;
            relayout(lib, movie, t);
        }
    }

    pub fn update_text(&mut self, lib: &Library, id: InstanceId, f: impl FnOnce(&mut TextState)) {
        let Some(i) = self.get_mut(id) else { return };
        let movie = i.movie;
        if let InstanceKind::EditText(t) = &mut i.kind {
            f(t);
            relayout(lib, movie, t);
        }
    }

    pub fn text_state(&self, id: InstanceId) -> Option<&TextState> {
        self.get(id)?.text()
    }

    pub fn text(&self, id: InstanceId) -> Option<String> {
        let t = self.get(id)?.text()?;
        Some(
            t.paragraphs
                .iter()
                .map(|p| p.runs.iter().map(|r| r.text.as_str()).collect::<String>())
                .collect::<Vec<_>>()
                .join("\r"),
        )
    }
}
