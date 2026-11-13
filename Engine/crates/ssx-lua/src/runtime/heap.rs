use std::any::Any;
use std::cell::RefCell;
use std::rc::Rc;

use crate::bytecode::Proto;
use crate::table::Table;
use crate::value::{GcRef, Value};
use crate::vm::{Lua, LuaResult, ThreadState};

pub type Cell = Rc<RefCell<Value>>;

pub type NativeFn = Rc<dyn Fn(&mut Lua, &[Value]) -> LuaResult<Vec<Value>>>;

pub struct LuaClosure {
    pub proto: Rc<Proto>,
    pub upvals: Vec<Cell>,
    pub env: GcRef,
}

pub struct NativeClosure {
    pub name: Rc<str>,
    pub f: NativeFn,
    pub upvals: Vec<Value>,
    pub env: GcRef,
}

pub enum Function {
    Lua(LuaClosure),
    Native(NativeClosure),
}

pub struct UserData {
    pub data: Box<dyn Any>,
    pub meta: Option<GcRef>,
    pub env: Option<GcRef>,
}

pub enum Obj {
    Table(Table),
    Func(Function),
    UserData(UserData),
    Thread(Box<ThreadState>),
}

#[derive(Default)]
pub struct Heap {
    slots: Vec<Option<Obj>>,
    free: Vec<u32>,
    live: usize,
    pub debt: usize,
    pub threshold: usize,
    pub stress: bool,
}

impl Heap {
    pub fn alloc(&mut self, o: Obj) -> GcRef {
        self.live += 1;
        self.debt += 1;
        if let Some(i) = self.free.pop() {
            self.slots[i as usize] = Some(o);
            GcRef(i)
        } else {
            self.slots.push(Some(o));
            GcRef(self.slots.len() as u32 - 1)
        }
    }

    pub fn live(&self) -> usize {
        self.live
    }

    pub fn get(&self, r: GcRef) -> &Obj {
        match self.slots.get(r.0 as usize) {
            Some(Some(o)) => o,
            _ => panic!("ssx-lua: dangling object reference {r:?} (a collector root is missing)"),
        }
    }

    pub fn get_mut(&mut self, r: GcRef) -> &mut Obj {
        match self.slots.get_mut(r.0 as usize) {
            Some(Some(o)) => o,
            _ => panic!("ssx-lua: dangling object reference {r:?} (a collector root is missing)"),
        }
    }

    pub fn table(&self, r: GcRef) -> &Table {
        match self.get(r) {
            Obj::Table(t) => t,
            _ => panic!("ssx-lua: {r:?} is not a table"),
        }
    }

    pub fn table_mut(&mut self, r: GcRef) -> &mut Table {
        match self.get_mut(r) {
            Obj::Table(t) => t,
            _ => panic!("ssx-lua: {r:?} is not a table"),
        }
    }

    pub fn func(&self, r: GcRef) -> &Function {
        match self.get(r) {
            Obj::Func(f) => f,
            _ => panic!("ssx-lua: {r:?} is not a function"),
        }
    }

    pub fn func_mut(&mut self, r: GcRef) -> &mut Function {
        match self.get_mut(r) {
            Obj::Func(f) => f,
            _ => panic!("ssx-lua: {r:?} is not a function"),
        }
    }

    pub fn userdata(&self, r: GcRef) -> &UserData {
        match self.get(r) {
            Obj::UserData(u) => u,
            _ => panic!("ssx-lua: {r:?} is not a userdata"),
        }
    }

    pub fn userdata_mut(&mut self, r: GcRef) -> &mut UserData {
        match self.get_mut(r) {
            Obj::UserData(u) => u,
            _ => panic!("ssx-lua: {r:?} is not a userdata"),
        }
    }

    pub fn thread(&self, r: GcRef) -> &ThreadState {
        match self.get(r) {
            Obj::Thread(t) => t,
            _ => panic!("ssx-lua: {r:?} is not a thread"),
        }
    }

    pub fn thread_mut(&mut self, r: GcRef) -> &mut ThreadState {
        match self.get_mut(r) {
            Obj::Thread(t) => t,
            _ => panic!("ssx-lua: {r:?} is not a thread"),
        }
    }

    // PORT(choice): no weak tables (`__mode`) or `__gc` finalizers, and the collection points differ from Lua's
    // incremental collector; no disc script uses `__mode` / `__gc` (a search of the 177 scripts), only memory use
    // differs.
    pub fn collect(&mut self, roots: &[Value]) -> usize {
        let mut marked = vec![false; self.slots.len()];
        let mut work: Vec<GcRef> = roots.iter().filter_map(Value::gc).collect();
        while let Some(r) = work.pop() {
            let i = r.0 as usize;
            if i >= marked.len() || marked[i] {
                continue;
            }
            marked[i] = true;
            let mut push = |v: &Value| {
                if let Some(g) = v.gc() {
                    work.push(g);
                }
            };
            match &self.slots[i] {
                Some(Obj::Table(t)) => {
                    if let Some(m) = t.meta {
                        push(&Value::Table(m));
                    }
                    t.for_each_ref(&mut push);
                }
                Some(Obj::Func(Function::Lua(c))) => {
                    push(&Value::Table(c.env));
                    for u in &c.upvals {
                        push(&u.borrow());
                    }
                }
                Some(Obj::Func(Function::Native(c))) => {
                    push(&Value::Table(c.env));
                    c.upvals.iter().for_each(&mut push);
                }
                Some(Obj::UserData(u)) => {
                    if let Some(m) = u.meta {
                        push(&Value::Table(m));
                    }
                    if let Some(e) = u.env {
                        push(&Value::Table(e));
                    }
                }
                Some(Obj::Thread(t)) => t.for_each_ref(&mut push),
                None => {}
            }
        }
        let mut freed = 0;
        for (i, m) in marked.iter().enumerate() {
            if !m && self.slots[i].is_some() {
                self.slots[i] = None;
                self.free.push(i as u32);
                freed += 1;
            }
        }
        self.live -= freed;
        self.debt = 0;
        self.threshold = if self.stress { 0 } else { (self.live * 2).max(10_000) };
        freed
    }
}

pub fn downcast<T: Any>(u: &UserData) -> Option<&T> {
    u.data.downcast_ref::<T>()
}
