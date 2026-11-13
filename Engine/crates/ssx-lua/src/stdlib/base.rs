use super::{arg, none, one};
use crate::heap::{Function, Obj, UserData};
use crate::number::{self, strtoul};
use crate::value::{Str, Value};
use crate::vm::{Lua, LuaError, LuaResult};

pub fn open(l: &mut Lua) {
    let g = l.globals;
    l.set_global("_G", Value::Table(g));
    let _ = l.lib_table("_G");
    l.register("assert", |l, a| {
        let v = l.check_any(a, 1)?;
        if !v.truthy() {
            let msg = l.opt_str(a, 2, "assertion failed!")?;
            return Err(l.rt_error(&msg.to_str_lossy()));
        }
        Ok(a.to_vec())
    });
    l.register("collectgarbage", collectgarbage);
    l.register("dofile", |l, a| {
        let name = l.opt_str(a, 1, "")?.to_str_lossy();
        let f = match l.load_file(&name) {
            Ok(f) => f,
            Err(e) => return Err(LuaError::Error(e)),
        };
        l.call(f, &[])
    });
    l.register("error", |l, a| {
        let level = l.opt_int(a, 2, 1)?;
        let v = arg(a, 1).cloned().unwrap_or_default();
        if let (Some(s), true) = (v.to_str_coerce(), level > 0) {
            let w = l.where_(level as usize + 1);
            return Err(LuaError::Error(Value::str(&format!("{w}{}", s.to_str_lossy()))));
        }
        Err(LuaError::Error(v))
    });
    l.register("gcinfo", |l, _| one((l.heap.live() * 64 / 1024) as f64));
    l.register("getfenv", |l, a| {
        let f = getfunc(l, a)?;
        match f {
            Value::Func(r) => match l.heap.func(r) {
                Function::Lua(c) => one(Value::Table(c.env)),
                Function::Native(_) => one(Value::Table(l.globals)),
            },
            _ => one(Value::Table(l.globals)),
        }
    });
    l.register("getmetatable", |l, a| {
        let v = l.check_any(a, 1)?.clone();
        match l.metatable(&v) {
            None => one(Value::Nil),
            Some(m) => {
                let p = l.heap.table(m).get_str("__metatable");
                one(if p.is_nil() { Value::Table(m) } else { p })
            }
        }
    });
    l.register("loadfile", |l, a| {
        let name = l.opt_str(a, 1, "")?.to_str_lossy();
        match l.load_file(&name) {
            Ok(f) => one(f),
            Err(e) => Ok(vec![Value::Nil, e]),
        }
    });
    l.register("load", |l, a| {
        let f = l.check_func(a, 1)?;
        let name = l.opt_str(a, 2, "=(load)")?.to_str_lossy();
        let mut src = Vec::new();
        loop {
            let piece = l.call(f.clone(), &[])?.into_iter().next().unwrap_or_default();
            match piece {
                Value::Nil => break,
                Value::Str(s) if s.bytes().is_empty() => break,
                Value::Str(s) => src.extend_from_slice(s.bytes()),
                Value::Num(n) => src.extend_from_slice(number::tostring(n).as_bytes()),
                _ => return Ok(vec![Value::Nil, Value::str("reader function must return a string")]),
            }
        }
        load_result(l.load(&src, &name))
    });
    l.register("loadstring", |l, a| {
        let s = l.check_str(a, 1)?;
        let name = l.opt_str(a, 2, &s.to_str_lossy())?.to_str_lossy();
        load_result(l.load(s.bytes(), &name))
    });
    l.register("next", |l, a| {
        let t = l.check_table(a, 1)?;
        let k = arg(a, 2).cloned().unwrap_or_default();
        match l.heap.table(t).next(&k) {
            Ok(Some((k, v))) => Ok(vec![k, v]),
            Ok(None) => one(Value::Nil),
            Err(_) => Err(l.rt_error("invalid key to 'next'")),
        }
    });
    l.register("pcall", |l, a| {
        let f = l.check_any(a, 1)?.clone();
        match l.call(f, &a[1..]) {
            Ok(mut r) => {
                r.insert(0, Value::Bool(true));
                Ok(r)
            }
            Err(e) => Ok(vec![Value::Bool(false), error_value(e)]),
        }
    });
    l.register("print", |l, a| {
        let mut line = String::new();
        for (i, v) in a.iter().enumerate() {
            let ts = l.get_global("tostring");
            let s = l.call(ts, std::slice::from_ref(v))?.into_iter().next().unwrap_or_default();
            let Some(s) = (match s {
                Value::Str(s) => Some(s),
                Value::Num(n) => Some(Str::from(number::tostring(n).as_str())),
                _ => None,
            }) else {
                return Err(l.rt_error("'tostring' must return a string to 'print'"));
            };
            if i > 0 {
                line.push('\t');
            }
            line.push_str(&s.to_str_lossy());
        }
        l.printed.push(line);
        none()
    });
    l.register("rawequal", |l, a| {
        let x = l.check_any(a, 1)?;
        let y = l.check_any(a, 2)?;
        one(x.raw_eq(y))
    });
    l.register("rawget", |l, a| {
        let t = l.check_table(a, 1)?;
        let k = l.check_any(a, 2)?;
        one(l.heap.table(t).get(k))
    });
    l.register("rawset", |l, a| {
        let t = l.check_table(a, 1)?;
        let k = l.check_any(a, 2)?.clone();
        let v = l.check_any(a, 3)?.clone();
        l.raw_set(t, k, v)?;
        one(Value::Table(t))
    });
    l.register("select", |l, a| {
        let n = a.len() as i64;
        if let Some(Value::Str(s)) = arg(a, 1)
            && s.bytes().first() == Some(&b'#')
        {
            return one((n - 1) as f64);
        }
        let mut i = i64::from(l.check_int(a, 1)?);
        if i < 0 {
            i += n;
        } else if i > n {
            i = n;
        }
        if i < 1 {
            return Err(l.arg_error(1, "index out of range"));
        }
        Ok(a[i as usize..].to_vec())
    });
    l.register("setfenv", |l, a| {
        let t = l.check_table(a, 2)?;
        if let Some(Value::Num(n)) = arg(a, 1)
            && *n == 0.0
        {
            // PORT(choice): this state has one globals table, so `setfenv(0, t)` replaces it for every thread; no
            // script on the disc does it; Lua keeps one globals table per thread.
            l.globals = t;
            return none();
        }
        let f = getfunc(l, a)?;
        if let Value::Func(r) = f
            && let Function::Lua(c) = l.heap.func_mut(r)
        {
            c.env = t;
            return one(f);
        }
        Err(l.rt_error("'setfenv' cannot change environment of given object"))
    });
    l.register("setmetatable", |l, a| {
        let t = l.check_table(a, 1)?;
        let mt = match arg(a, 2) {
            Some(Value::Nil) => None,
            Some(Value::Table(m)) => Some(*m),
            _ => return Err(l.arg_error(2, "nil or table expected")),
        };
        if let Some(m) = l.heap.table(t).meta
            && !l.heap.table(m).get_str("__metatable").is_nil()
        {
            return Err(l.rt_error("cannot change a protected metatable"));
        }
        l.heap.table_mut(t).meta = mt;
        one(Value::Table(t))
    });
    l.register("tonumber", |l, a| {
        let base = l.opt_int(a, 2, 10)?;
        if base == 10 {
            let v = l.check_any(a, 1)?;
            return one(v.to_num().map_or(Value::Nil, Value::Num));
        }
        let s = l.check_str(a, 1)?;
        if !(2..=36).contains(&base) {
            return Err(l.arg_error(2, "base out of range"));
        }
        let b = s.bytes();
        let (n, end) = strtoul(b, base as u32);
        if end > 0 && b[end..].iter().all(|c| crate::lexer::is_space(*c)) {
            return one(f64::from(n));
        }
        one(Value::Nil)
    });
    l.register("tostring", |l, a| {
        let v = l.check_any(a, 1)?.clone();
        let tm = l.metamethod(&v, "__tostring");
        if !tm.is_nil() {
            return Ok(l.call(tm, &[v])?.into_iter().take(1).collect());
        }
        one(Value::str(&l.default_tostring(&v)))
    });
    l.register("type", |l, a| {
        let v = l.check_any(a, 1)?;
        one(v.type_name())
    });
    l.register("unpack", |l, a| {
        let t = l.check_table(a, 1)?;
        let i = l.opt_int(a, 2, 1)?;
        let e = match arg(a, 3) {
            None | Some(Value::Nil) => l.heap.table(t).length() as i32,
            _ => l.check_int(a, 3)?,
        };
        let n = e.wrapping_sub(i).wrapping_add(1);
        if n <= 0 {
            return none();
        }
        if a.len() as i64 + i64::from(n) > i64::from(crate::vm::MAXCSTACK) {
            return Err(l.rt_error("stack overflow (table too big to unpack)"));
        }
        Ok((i..=e).map(|k| l.heap.table(t).get_int(i64::from(k))).collect())
    });
    l.register("xpcall", |l, a| {
        let f = l.check_any(a, 1)?.clone();
        let h = l.check_any(a, 2)?.clone();
        match l.call(f, &[]) {
            Ok(mut r) => {
                r.insert(0, Value::Bool(true));
                Ok(r)
            }
            Err(e) => {
                // PORT(choice): here the handler runs after the unwind; it only matters for tracebacks, which no game
                // script reads; Lua runs the handler at the error point (the stack still there for `debug.traceback`).
                let hv = l.call(h, &[error_value(e)]).map(|r| r.into_iter().next().unwrap_or_default());
                let hv = hv.unwrap_or_else(error_value);
                Ok(vec![Value::Bool(false), hv])
            }
        }
    });
    l.set_global("_VERSION", Value::str("Lua 5.1"));
    let next = l.get_global("next");
    let pairs = l.create_function_with(
        "pairs",
        std::rc::Rc::new(|l: &mut Lua, a: &[Value]| {
            let t = l.check_table(a, 1)?;
            Ok(vec![l.upvalue(0), Value::Table(t), Value::Nil])
        }),
        vec![next],
    );
    l.set_global("pairs", pairs);
    let iaux = l.create_function("ipairs_aux", |l, a| {
        let i = l.check_int(a, 2)?;
        let t = l.check_table(a, 1)?;
        let i = i64::from(i) + 1;
        let v = l.heap.table(t).get_int(i);
        if v.is_nil() { none() } else { Ok(vec![Value::Num(i as f64), v]) }
    });
    let ipairs = l.create_function_with(
        "ipairs",
        std::rc::Rc::new(|l: &mut Lua, a: &[Value]| {
            let t = l.check_table(a, 1)?;
            Ok(vec![l.upvalue(0), Value::Table(t), Value::Num(0.0)])
        }),
        vec![iaux],
    );
    l.set_global("ipairs", ipairs);
    l.register("newproxy", |l, a| {
        let meta = match arg(a, 1) {
            Some(Value::Bool(true)) => Some(l.new_table(0, 0)),
            Some(Value::UserData(u)) => l.heap.userdata(*u).meta,
            None | Some(Value::Nil) | Some(Value::Bool(false)) => None,
            _ => return Err(l.arg_error(1, "boolean or proxy expected")),
        };
        let r = l.heap.alloc(Obj::UserData(UserData { data: Box::new(()), meta, env: None }));
        one(Value::UserData(r))
    });
    super::coroutine::open(l);
}

pub fn error_value(e: LuaError) -> Value {
    match e {
        LuaError::Error(v) => v,
        LuaError::Yield(_) => Value::str("attempt to yield across metamethod/C-call boundary"),
    }
}

fn load_result(r: LuaResult<Value>) -> LuaResult<Vec<Value>> {
    match r {
        Ok(f) => one(f),
        Err(e) => Ok(vec![Value::Nil, error_value(e)]),
    }
}

fn getfunc(l: &Lua, a: &[Value]) -> LuaResult<Value> {
    if let Some(f @ Value::Func(_)) = arg(a, 1) {
        return Ok(f.clone());
    }
    let level = l.opt_int(a, 1, 1)?;
    if level < 0 {
        return Err(l.arg_error(1, "level must be non-negative"));
    }
    if level == 0 {
        return Ok(Value::Nil);
    }
    let frames = &l.th.frames;
    let idx = frames.len() as i64 - 1 - i64::from(level);
    if idx < 0 {
        return Err(l.arg_error(1, "invalid level"));
    }
    Ok(Value::Func(frames[idx as usize].func))
}

fn collectgarbage(l: &mut Lua, a: &[Value]) -> LuaResult<Vec<Value>> {
    let opt = l.opt_str(a, 1, "collect")?.to_str_lossy();
    // PORT(choice): "count" reports our object count × 64 bytes, not the game allocator's bytes, which are not
    // reproducible (only the front end's telemetry reads it).
    let kb = (l.heap.live() * 64) as f64 / 1024.0;
    match opt.as_str() {
        "stop" => l.gc_stopped = true,
        "restart" => l.gc_stopped = false,
        "collect" | "step" => {
            l.collect_garbage();
        }
        "count" => return one(kb),
        "setpause" | "setstepmul" => return one(200.0),
        _ => return Err(l.arg_error(1, &format!("invalid option '{opt}'"))),
    }
    one(0.0)
}

impl Lua {
    pub fn load_file(&mut self, name: &str) -> Result<Value, Value> {
        let data = self.file_reader.as_ref().and_then(|r| r(name));
        let Some(src) = data else {
            return Err(Value::str(&format!("cannot open {name}")));
        };
        let src = if src.first() == Some(&b'#') {
            let end = src.iter().position(|c| *c == b'\n').unwrap_or(src.len());
            src[end..].to_vec()
        } else {
            src
        };
        self.load(&src, &format!("@{name}")).map_err(error_value)
    }
}
