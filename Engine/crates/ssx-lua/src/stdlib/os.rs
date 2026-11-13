use super::{arg, one};
use crate::value::Value;
use crate::vm::{Lua, LuaError};

pub fn open(l: &mut Lua) {
    let t = l.lib_table("os");
    // PORT(choice): these give the host's time; no game script reads them (only `navtester.lua` calls `os.exit`); the
    // console runtime's clock / time / date cannot be reproduced.
    let start = std::time::Instant::now();
    l.lib_fn(t, "clock", move |_, _| one(start.elapsed().as_secs_f64()));
    l.lib_fn(t, "time", |l, a| {
        if let Some(Value::Table(_)) = arg(a, 1) {
            return Err(l.rt_error("os.time with a date table is not supported"));
        }
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
        one(now as f64)
    });
    l.lib_fn(t, "difftime", |l, a| one(l.check_num(a, 1)? - l.opt_num(a, 2, 0.0)?));
    l.lib_fn(t, "date", |l, a| {
        let f = l.opt_str(a, 1, "%c")?.to_str_lossy();
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
        one(format!("{f} @{now}").as_str())
    });
    l.lib_fn(t, "exit", |_, _| Err(LuaError::Error(Value::str("os.exit"))));
    l.lib_fn(t, "getenv", |l, a| {
        l.check_str(a, 1)?;
        one(Value::Nil)
    });
    l.lib_fn(t, "execute", |_, _| one(0.0));
    l.lib_fn(t, "remove", |l, a| {
        let n = l.check_str(a, 1)?.to_str_lossy();
        Ok(vec![Value::Nil, Value::str(&format!("{n}: not supported")), Value::Num(0.0)])
    });
    l.lib_fn(t, "rename", |l, a| {
        let n = l.check_str(a, 1)?.to_str_lossy();
        Ok(vec![Value::Nil, Value::str(&format!("{n}: not supported")), Value::Num(0.0)])
    });
    l.lib_fn(t, "setlocale", |l, a| {
        let loc = l.opt_str(a, 1, "")?;
        one(if loc.bytes().is_empty() || loc.bytes() == b"C" { Value::str("C") } else { Value::Nil })
    });
    l.lib_fn(t, "tmpname", |l, _| Err(l.rt_error("unable to generate a unique filename")));
}
