use std::sync::Arc;

use ssx_formats::gfx::avm1::Action;

use super::interp::Frame;
use super::object::{Kind, ObjId, Scope, ScopeKind, flags};
use super::value::Value;
use super::{ACTION_BUDGET, ClipEvent, Ctx, Fault, Job, Vm};
use crate::display::{InstanceId, Player, QueuedAction};
use crate::library::{Library, MovieId};

#[derive(Clone, Debug)]
pub enum LoadTarget {
    Clip(ObjId),
    Level(i32),
}

#[derive(Clone, Debug)]
pub struct LoadRequest {
    pub url: String,
    pub target: LoadTarget,
    pub loader: Option<ObjId>,
    pub by: Option<MovieId>,
}

pub const MAX_QUEUED_LOADS: usize = 256;

const GC_THRESHOLD: usize = 200_000;

impl Vm {
    pub fn ctx<'a>(&'a mut self, lib: &'a mut Library, player: &'a mut Player) -> Ctx<'a> {
        Ctx { vm: self, lib, player }
    }

    pub fn load_level(&mut self, lib: &mut Library, player: &mut Player, movie: MovieId, level: i32) -> InstanceId {
        player.script_events = true;
        let id = player.load_level(lib, movie, level);
        let mut ctx = self.ctx(lib, player);
        ctx.queue_import_inits(movie, id);
        ctx.run_queue();
        id
    }

    pub fn advance(&mut self, lib: &mut Library, player: &mut Player, dt: f64) -> u32 {
        player.script_events = true;
        player.no_invisible_advance = self.global_flag("noInvisibleAdvance");
        self.time += dt.max(0.0);
        let mut ctx = self.ctx(lib, player);
        ctx.run_pointer();
        ctx.run_intervals();
        let n = ctx.player.frames_due(ctx.lib, dt, 2);
        for _ in 0..n {
            ctx.run_frame();
        }
        n
    }

    pub fn step_frame(&mut self, lib: &mut Library, player: &mut Player) {
        player.script_events = true;
        player.no_invisible_advance = self.global_flag("noInvisibleAdvance");
        let frame_time =
            player.levels.values().next().and_then(|r| player.get(*r)).map(|r| lib.movie(r.movie).frame_rate);
        self.time += 1.0 / f64::from(frame_time.unwrap_or(30.0).max(1.0));
        let mut ctx = self.ctx(lib, player);
        ctx.run_intervals();
        ctx.run_frame();
    }

    pub fn global_flag(&self, name: &str) -> bool {
        let v = self.heap.get(self.global).and_then(|g| g.props.get(name)).map(|p| p.value.clone());
        v.is_some_and(|v| super::value::primitive_to_bool(&v, 7))
    }

    pub fn call_function(
        &mut self,
        lib: &mut Library,
        player: &mut Player,
        func: &Value,
        this: Value,
        args: &[Value],
    ) -> Value {
        self.ctx(lib, player).call_top(func, this, args)
    }
}

impl Ctx<'_> {
    fn run_frame(&mut self) {
        self.vm.sandbox.new_frame();
        self.vm.pointer.meshes.new_frame();
        let before = self.vm.diag.actions;
        self.serve_loads();
        self.player.advance_frame(self.lib);
        self.run_queue();
        self.settle_stops();
        self.vm.diag.frames += 1;
        let peaks = &mut self.vm.sandbox.peaks;
        peaks.frame_actions = peaks.frame_actions.max(self.vm.diag.actions - before);
        let made = self.vm.heap.since_gc as u64 * super::sandbox::OBJECT_BYTES;
        if self.vm.heap.since_gc > GC_THRESHOLD
            || self.vm.sandbox.heap_bytes.saturating_add(made) > super::sandbox::caps::HEAP_BYTES / 2
        {
            self.collect();
        }
    }

    fn settle_stops(&mut self) {
        for m in std::mem::take(&mut self.player.over_cap) {
            let cap = crate::display::MAX_OWNED_INSTANCES;
            self.vm.sandbox.stop(m, format!("more than {cap} clips of its own on the display list"));
        }
        let owning: Vec<MovieId> =
            self.vm.sandbox.stopped.keys().copied().filter(|&m| self.player.owned_by(m) > 0).collect();
        for m in owning {
            let n = self.player.unload_owned(m);
            let path = self.lib.movie(m).path.clone();
            self.vm.diag.note("package movie unloaded", &format!("{path}: {n} instances off the display list"));
        }
    }

    pub fn call_top(&mut self, func: &Value, this: Value, args: &[Value]) -> Value {
        self.vm.budget = ACTION_BUDGET;
        let height = self.vm.stack.len();
        let r = self.call(func, this, args, Value::Undefined);
        self.vm.stack.truncate(height);
        match r {
            Ok(v) => v,
            Err(e) => {
                self.note_fault(e);
                Value::Undefined
            }
        }
    }

    fn note_fault(&mut self, e: Fault) {
        match e {
            Fault::Throw(v) => {
                let s = self.to_string(&v).map(|s| s.to_string()).unwrap_or_default();
                self.vm.diag.note("uncaught exception", &s);
            }
            Fault::Abort(m) => self.vm.diag.note("abort", m),
            Fault::Stopped(m) => {
                let why = self.vm.sandbox.stopped.get(&m).cloned().unwrap_or_default();
                let path = self.lib.movie(m).path.clone();
                self.vm.diag.note("package movie stopped", &format!("{path}: {why}"));
            }
        }
    }

    pub fn run_block(&mut self, code: Arc<[Action]>, clip: ObjId, version: u8, movie: MovieId) {
        let scope = Arc::new(Scope { obj: clip, kind: ScopeKind::Target, parent: Some(self.vm.global_scope.clone()) });
        let end = code.len();
        let mut f = Frame {
            code,
            pc: 0,
            pool: Arc::new(Vec::new()),
            scope,
            regs: vec![Value::Undefined; 4],
            this: Value::Object(clip),
            target: Some(clip),
            version,
            movie,
            callee: Value::Undefined,
            home: Value::Undefined,
            withs: Vec::new(),
        };
        let saved = (self.vm.cur_version, self.vm.cur_movie);
        (self.vm.cur_version, self.vm.cur_movie) = (version, movie);
        self.vm.budget = ACTION_BUDGET;
        self.vm.diag.blocks += 1;
        let height = self.vm.stack.len();
        if let Err(e) = self.run(&mut f, 0, end) {
            self.note_fault(e);
        }
        let peaks = &mut self.vm.sandbox.peaks;
        peaks.block_actions = peaks.block_actions.max(ACTION_BUDGET - self.vm.budget);
        self.vm.stack.truncate(height);
        (self.vm.cur_version, self.vm.cur_movie) = saved;
    }
}

impl Ctx<'_> {
    // TODO(guess): three priority levels after Flash Player's AVM1 queue; settle: Scaleform's queue levels (DoAction / DoInitAction via the AS2 support vtable +0x2C / +0x30, 0x8269E390 / 0x8269E3B8).
    fn drain_player(&mut self) {
        for a in self.player.take_actions() {
            match a {
                QueuedAction::Init { instance, sprite_id, actions } => {
                    let Some(movie) = self.player.get(instance).map(|i| i.movie) else { continue };
                    if self.vm.init_ran.insert((movie, sprite_id)) {
                        self.vm.queue[0].push_back(Job::Code { clip: instance, code: actions, movie });
                    }
                }
                QueuedAction::Frame { instance, actions } => {
                    let Some(i) = self.player.get(instance) else { continue };
                    let movie = i.sprite().map_or(i.movie, |s| s.timeline.movie);
                    self.vm.queue[2].push_back(Job::Code { clip: instance, code: actions, movie });
                }
                QueuedAction::Construct { instance } => self.vm.queue[1].push_back(Job::Construct { clip: instance }),
                QueuedAction::Load { instance } => {
                    self.vm.queue[2].push_back(Job::Event { clip: instance, event: ClipEvent::Load })
                }
                QueuedAction::EnterFrame { instance } => {
                    self.vm.queue[2].push_back(Job::Event { clip: instance, event: ClipEvent::EnterFrame })
                }
                QueuedAction::Unload { instance } => {
                    if let Some(obj) = self.vm.display_objs.remove(&instance) {
                        self.vm.queue[2].push_back(Job::Unload { obj });
                    }
                }
            }
        }
    }

    pub fn run_queue(&mut self) {
        let mut guard = 0u32;
        loop {
            self.drain_player();
            let Some(job) = self.vm.queue.iter_mut().find_map(|q| q.pop_front()) else { break };
            self.run_job(job);
            guard += 1;
            if guard > 1_000_000 {
                self.vm.diag.note("abort", "action queue does not drain");
                for q in &mut self.vm.queue {
                    q.clear();
                }
                break;
            }
        }
    }

    fn run_job(&mut self, job: Job) {
        match job {
            Job::Code { clip, code, movie } => {
                if self.player.get(clip).is_none() {
                    return;
                }
                let obj = self.object_for(clip);
                let version = self.lib.movie(movie).version;
                self.run_block(code, obj, version, movie);
            }
            Job::Construct { clip } => {
                if let Err(e) = self.construct_clip(clip, None) {
                    self.note_fault(e);
                }
            }
            Job::Event { clip, event } => self.clip_event(clip, event),
            Job::Unload { obj } => {
                let (f, home) = self.get_method(&Value::Object(obj), "onUnload").unwrap_or_default();
                if self.is_function(&f) {
                    self.vm.budget = ACTION_BUDGET;
                    if let Err(e) = self.call(&f, Value::Object(obj), &[], home) {
                        self.note_fault(e);
                    }
                }
            }
            Job::Call { func, this, args } => {
                self.call_top(&func, this, &args);
            }
        }
    }

    pub fn registered_class(&self, clip: InstanceId) -> Option<Value> {
        let r = self.player.get(clip)?.character?;
        let m = self.lib.movie(r.movie);
        m.exports
            .iter()
            .filter(|(_, id)| **id == r.id)
            .find_map(|(name, _)| self.vm.classes.get(name.as_str()).cloned())
    }

    pub fn construct_clip(&mut self, clip: InstanceId, init: Option<&Value>) -> Result<(), Fault> {
        let Some(inst) = self.player.get(clip) else { return Ok(()) };
        let clip_actions = inst.clip_actions.clone();
        let movie = inst.movie;
        let class = self.registered_class(clip);
        if class.is_none() && clip_actions.is_none() && init.is_none() {
            return Ok(());
        }
        let obj = self.object_for(clip);
        let mut proto = Value::Undefined;
        if let Some(ctor) = &class {
            proto = self.get_member(ctor, "prototype")?;
            if let Some(o) = self.vm.heap.get_mut(obj) {
                o.proto = proto.clone();
            }
            self.define(obj, "__constructor__", ctor.clone(), flags::DONT_ENUM);
        }
        if let Some(init) = init {
            for k in self.enumerate(init) {
                let v = self.get_member(init, &k)?;
                self.set_member(&Value::Object(obj), &k, v)?;
            }
        }
        if let Some(ca) = clip_actions {
            let version = self.lib.movie(movie).version;
            for r in &ca.records {
                if r.events & (0x200 | 0x4_0000) != 0 {
                    self.run_block(Arc::from(r.actions.as_slice()), obj, version, movie);
                }
            }
        }
        if let Some(ctor) = class {
            self.vm.budget = ACTION_BUDGET;
            self.vm.call_names.push(format!("constructor of {}", self.player.path(clip)));
            let r = self.call(&ctor, Value::Object(obj), &[], proto);
            self.vm.call_names.pop();
            r?;
        }
        Ok(())
    }

    fn clip_event(&mut self, clip: InstanceId, event: ClipEvent) {
        let Some(inst) = self.player.get(clip) else { return };
        let movie = inst.movie;
        let records = inst.clip_actions.clone();
        let Some(&obj) = self.vm.display_objs.get(&clip) else { return };
        if let Some(ca) = records {
            let version = self.lib.movie(movie).version;
            for r in &ca.records {
                if r.events & event.flag() != 0 {
                    self.run_block(Arc::from(r.actions.as_slice()), obj, version, movie);
                }
            }
        }
        let (f, home) = match self.get_method(&Value::Object(obj), event.method()) {
            Ok(x) => x,
            Err(e) => return self.note_fault(e),
        };
        if self.is_function(&f) {
            self.vm.budget = ACTION_BUDGET;
            self.vm.call_names.push(format!("{} of {}", event.method(), self.player.path(clip)));
            if let Err(e) = self.call(&f, Value::Object(obj), &[], home) {
                self.note_fault(e);
            }
            self.vm.call_names.pop();
        }
    }
}

impl Ctx<'_> {
    // TODO(guess): all of the imported movie's root init actions queued here, in the importing clip's context; settle: Scaleform's ExecuteImportedInitActions (not located).
    pub fn queue_import_inits(&mut self, movie: MovieId, clip: InstanceId) {
        let mut seen = std::collections::HashSet::from([movie]);
        let mut order = Vec::new();
        self.import_order(movie, &mut seen, &mut order);
        for m in order {
            let root = self.lib.movie(m).root.clone();
            for frame in &root.frames {
                for (sid, code) in &frame.init_actions {
                    if self.vm.init_ran.insert((m, *sid)) {
                        self.vm.queue[0].push_back(Job::Code { clip, code: code.clone(), movie: m });
                    }
                }
            }
        }
    }

    fn import_order(&self, movie: MovieId, seen: &mut std::collections::HashSet<MovieId>, order: &mut Vec<MovieId>) {
        let mut imports: Vec<(u16, MovieId)> =
            self.lib.movie(movie).imports.iter().filter_map(|(cid, i)| Some((*cid, i.target?.movie))).collect();
        imports.sort();
        for (_, m) in imports {
            if seen.insert(m) {
                self.import_order(m, seen, order);
                order.push(m);
            }
        }
    }

    fn serve_loads(&mut self) {
        for req in std::mem::take(&mut self.vm.loads) {
            let making = std::mem::replace(&mut self.player.making_for, req.by);
            self.serve_load(req);
            self.player.making_for = making;
        }
    }

    pub(crate) fn queue_load(&mut self, url: String, target: LoadTarget, loader: Option<ObjId>) -> Result<(), Fault> {
        if self.vm.loads.len() >= MAX_QUEUED_LOADS {
            if let Some(m) = self.vm.sandbox.charged {
                self.vm.sandbox.stop(m, format!("more than {MAX_QUEUED_LOADS} movie loads asked for in one frame"));
                return Err(Fault::Stopped(m));
            }
            self.vm
                .diag
                .note("load refused", &format!("{}: the load queue holds {MAX_QUEUED_LOADS}", super::cut(&url, 64)));
            return Ok(());
        }
        let by = self.vm.sandbox.charged;
        self.vm.loads.push(LoadRequest { url, target, loader, by });
        let peaks = &mut self.vm.sandbox.peaks;
        peaks.loads_queued = peaks.loads_queued.max(self.vm.loads.len());
        Ok(())
    }

    fn serve_load(&mut self, req: LoadRequest) {
        let target = match req.target {
            LoadTarget::Clip(o) => self.display_of(&Value::Object(o)),
            LoadTarget::Level(_) => None,
        };
        if req.url.is_empty() {
            if let Some(id) = target {
                for (_, c) in self.player.children(id) {
                    self.player.remove(c);
                }
                self.player.stop(id);
            }
            return;
        }
        let mut loaded = self.lib.load_movie(&req.url);
        if let (Ok(m), LoadTarget::Level(n)) = (&loaded, &req.target)
            && !self.player.levels.contains_key(n)
            && !self.player.room_for(None, self.lib.origin(*m).map(|_| *m).or(req.by))
        {
            loaded = Err(anyhow::anyhow!("the display list is full"));
        }
        let id = match (&loaded, &req.target, target) {
            (Ok(m), LoadTarget::Level(n), _) => {
                let id = self.player.load_level(self.lib, *m, *n);
                self.player.skip_next_advance(id);
                Some(id)
            }
            (Ok(m), LoadTarget::Clip(_), Some(id)) => {
                // TODO(guess): a movie loaded into a clip again runs its init actions (and its imports') again, as a new SWF instance in Flash; settle: Scaleform's init-action bookkeeping per loaded movie (not located).
                let mut closure = std::collections::HashSet::from([*m]);
                let mut order = Vec::new();
                self.import_order(*m, &mut closure, &mut order);
                self.vm.init_ran.retain(|(mm, _)| !closure.contains(mm));
                self.player.load_into(self.lib, id, *m);
                if let Some(&o) = self.vm.display_objs.get(&id) {
                    let proto = Value::Object(self.vm.protos.movieclip);
                    if let Some(obj) = self.vm.heap.get_mut(o) {
                        obj.props.clear();
                        obj.proto = proto;
                    }
                }
                Some(id)
            }
            _ => None,
        };
        match (loaded, id) {
            (Ok(m), Some(id)) => {
                if self.lib.origin(m).is_some() {
                    self.vm.package_loads.insert(m, req.clone());
                }
                self.vm.loaded.push(req.url.clone());
                self.queue_import_inits(m, id);
                if let Some(l) = req.loader {
                    let t = Value::Object(self.object_for(id));
                    self.notify_loader(l, "onLoadStart", vec![t.clone()]);
                    self.notify_loader(l, "onLoadComplete", vec![t.clone()]);
                    self.drain_player();
                    self.notify_loader(l, "onLoadInit", vec![t]);
                }
            }
            (r, _) => {
                let why = r.err().map_or_else(|| "target clip gone".to_string(), |e| format!("{e:#}"));
                self.vm.diag.note("load failed", &format!("{}: {why}", req.url));
                if let Some(l) = req.loader {
                    let t = req.target_value();
                    self.notify_loader(l, "onLoadError", vec![t, Value::str("URLNotFound")]);
                }
            }
        }
    }

    fn notify_loader(&mut self, loader: ObjId, method: &str, args: Vec<Value>) {
        let ls = self.get_member(&Value::Object(loader), "_listeners").unwrap_or_default();
        let listeners: Vec<Value> = match self.kind_of(&ls) {
            Some(Kind::Array(a)) => a.clone(),
            _ => Vec::new(),
        };
        for l in listeners {
            let f = self.get_member(&l, method).unwrap_or_default();
            if self.is_function(&f) {
                self.vm.queue[2].push_back(Job::Call { func: f, this: l, args: args.clone() });
            }
        }
    }

    fn run_intervals(&mut self) {
        let now = self.vm.time;
        let due: Vec<i32> = self.vm.intervals.iter().filter(|(_, iv)| iv.next <= now).map(|(k, _)| *k).collect();
        for k in due {
            let Some(iv) = self.vm.intervals.get_mut(&k) else { continue };
            let iv2 = iv.clone();
            if iv.once {
                self.vm.intervals.remove(&k);
            } else {
                iv.next = (iv.next + iv.period).max(now + iv.period.min(1e-3));
            }
            let func = match &iv2.func {
                Value::String(m) => self.get_member(&iv2.this, m).unwrap_or_default(),
                f => f.clone(),
            };
            self.call_top(&func, iv2.this.clone(), &iv2.args);
        }
        self.run_queue();
    }

    pub fn collect(&mut self) -> usize {
        let player = &*self.player;
        self.vm.display_objs.retain(|id, _| player.get(*id).is_some());
        let mut roots: Vec<ObjId> = vec![self.vm.global];
        roots.extend(self.vm.class_data.keys().copied());
        let p = self.vm.protos;
        roots.extend([
            p.object,
            p.function,
            p.array,
            p.string,
            p.number,
            p.boolean,
            p.date,
            p.movieclip,
            p.textfield,
            p.textformat,
            p.color,
            p.color_transform,
            p.transform,
            p.matrix,
            p.point,
            p.rectangle,
            p.loader,
            p.filter,
        ]);
        roots.extend(self.vm.display_objs.values().copied());
        let mut vals: Vec<Value> = self.vm.classes.values().cloned().collect();
        vals.extend(self.vm.stack.iter().cloned());
        for iv in self.vm.intervals.values() {
            vals.push(iv.func.clone());
            vals.push(iv.this.clone());
            vals.extend(iv.args.iter().cloned());
        }
        for q in &self.vm.queue {
            for j in q {
                match j {
                    Job::Unload { obj } => roots.push(*obj),
                    Job::Call { func, this, args } => {
                        vals.push(func.clone());
                        vals.push(this.clone());
                        vals.extend(args.iter().cloned());
                    }
                    _ => {}
                }
            }
        }
        for l in self.vm.loads.iter().chain(self.vm.package_loads.values()) {
            if let LoadTarget::Clip(o) = l.target {
                roots.push(o);
            }
            roots.extend(l.loader);
        }
        vals.extend(self.vm.natives.roots());
        roots.extend(vals.iter().filter_map(Value::as_object));
        let freed = self.vm.heap.collect(roots);
        let live = self.vm.heap.live();
        let peaks = &mut self.vm.sandbox.peaks;
        peaks.objects = peaks.objects.max(live);
        self.vm.sandbox.heap_bytes = self.vm.heap.script_bytes();
        freed
    }
}

impl LoadRequest {
    fn target_value(&self) -> Value {
        match self.target {
            LoadTarget::Clip(o) => Value::Object(o),
            LoadTarget::Level(_) => Value::Undefined,
        }
    }
}

pub mod nav {
    pub const SYS_INIT: i32 = 0x2000_0000;
    pub const CHANGE_VIEW: i32 = 0x2000_0001;
    pub const POST_ON_ENTER_VIEW: i32 = 0x2000_0003;
    pub const POST_ON_EXIT_VIEW: i32 = 0x2000_0005;
    pub const LOAD_VIEW: i32 = 0x2000_0006;
    pub const UNLOAD_VIEW: i32 = 0x2000_0007;
    pub const VIEW_READY: i32 = 0x2000_0008;
}

impl Vm {
    pub fn invoke(&mut self, lib: &mut Library, player: &mut Player, path: &str, args: &[Value]) -> Option<Value> {
        let (obj_path, method) = path.rsplit_once('.')?;
        let cur = self.get_path(lib, player, obj_path)?;
        let mut cx = self.ctx(lib, player);
        let (f, home) = cx.get_method(&cur, method).ok()?;
        if !cx.is_function(&f) {
            return None;
        }
        cx.vm.budget = ACTION_BUDGET;
        cx.vm.call_names.push(path.to_string());
        let r = match cx.call(&f, cur, args, home) {
            Ok(v) => v,
            Err(e) => {
                cx.note_fault(e);
                Value::Undefined
            }
        };
        cx.vm.call_names.pop();
        Some(r)
    }

    pub fn get_path(&mut self, lib: &mut Library, player: &mut Player, path: &str) -> Option<Value> {
        let mut cx = self.ctx(lib, player);
        let mut parts = path.split('.');
        let first = parts.next()?;
        let mut cur = if first == "_global" {
            Value::Object(cx.vm.global)
        } else if let Some(l) = first.strip_prefix("_level").and_then(|n| n.parse::<i32>().ok()) {
            Value::Object(cx.object_for(*cx.player.levels.get(&l)?))
        } else {
            let root = Value::Object(cx.object_for(*cx.player.levels.get(&0)?));
            cx.get_member(&root, first).ok()?
        };
        for p in parts {
            if cur.is_nullish() {
                return None;
            }
            cur = cx.get_member(&cur, p).ok()?;
        }
        (!cur.is_nullish()).then_some(cur)
    }

    pub fn peek_path(&mut self, lib: &mut Library, player: &mut Player, path: &str) -> Option<Value> {
        let cx = self.ctx(lib, player);
        let level = |n: i32| {
            let o = *cx.vm.display_objs.get(cx.player.levels.get(&n)?)?;
            cx.vm.heap.get(o).map(|_| Value::Object(o))
        };
        let mut parts = path.split('.');
        let first = parts.next()?;
        let mut cur = if first == "_global" {
            Value::Object(cx.vm.global)
        } else if let Some(l) = first.strip_prefix("_level").and_then(|n| n.parse::<i32>().ok()) {
            level(l)?
        } else {
            cx.peek_member(&level(0)?, first)?
        };
        for p in parts {
            cur = cx.peek_member(&cur, p)?;
        }
        (!cur.is_nullish()).then_some(cur)
    }

    pub fn member_names(&mut self, lib: &mut Library, player: &mut Player, v: &Value) -> Vec<String> {
        self.ctx(lib, player).enumerate(v).iter().map(|s| s.to_string()).collect()
    }

    pub fn framework_handler(&self) -> Option<String> {
        self.natives.view_ready.clone()
    }

    pub fn game_event(&mut self, lib: &mut Library, player: &mut Player, event: i32, data: Value) -> bool {
        let Some(h) = self.framework_handler() else { return false };
        self.invoke(lib, player, &format!("{h}.__HandleGameEvent"), &[Value::Number(f64::from(event)), data]).is_some()
    }

    pub fn init_blocks_run(&self) -> usize {
        self.init_ran.len()
    }
}
