use std::collections::HashMap;

use super::builtins::{Builder, R};
use super::object::ObjId;
use super::value::Value;
use super::{Ctx, NativeCall};

#[derive(Clone, Debug, Default)]
pub struct NativeState {
    pub calls: HashMap<(&'static str, &'static str), std::collections::VecDeque<Vec<Value>>>,
    queue: Vec<(&'static str, &'static str, Vec<Value>)>,
    pub view_ready: Option<String>,
    pub refused: u64,
    pub sd_moves: HashMap<crate::display::InstanceId, bool>,
    pub sd_scales: HashMap<crate::display::InstanceId, [f64; 3]>,
    pub hud_wide: HashMap<crate::display::InstanceId, (bool, f64)>,
}

impl NativeState {
    pub fn roots(&self) -> Vec<Value> {
        let queued = self.queue.iter().flat_map(|(_, _, a)| a.iter());
        self.calls
            .values()
            .flatten()
            .flatten()
            .chain(queued)
            .filter(|v| matches!(v, Value::Object(_)))
            .cloned()
            .collect()
    }

    pub fn take_queue(&mut self) -> Vec<(&'static str, &'static str, Vec<Value>)> {
        std::mem::take(&mut self.queue)
    }

    pub fn of(&self, object: &str, method: &str) -> Vec<&Vec<Value>> {
        self.calls
            .iter()
            .find(|((o, m), _)| *o == object && *m == method)
            .map_or(Vec::new(), |(_, v)| v.iter().collect())
    }
}

fn record(cx: &mut Ctx, object: &'static str, method: &'static str, a: &[Value]) -> R {
    use super::sandbox::caps::CALLS_KEPT;
    if let Some((m, why)) = cx.vm.sandbox.native_call() {
        cx.vm.sandbox.stop(m, why);
        return Err(super::Fault::Stopped(m));
    }
    if let Err(why) = super::calls::check(cx, object, method, a) {
        cx.vm.natives.refused += 1;
        cx.vm.diag.note("native call refused", &format!("{object}.{method}: {why}"));
        return Ok(Value::Undefined);
    }
    let mut text = Vec::new();
    for v in a {
        text.push(describe(cx, v));
    }
    if cx.vm.native_calls.len() >= CALLS_KEPT {
        cx.vm.native_calls.pop_front();
    }
    cx.vm.native_calls.push_back(NativeCall { object, method, args: text });
    cx.vm.natives_total += 1;
    let kept = cx.vm.natives.calls.entry((object, method)).or_default();
    if kept.len() >= CALLS_KEPT {
        kept.pop_front();
    }
    kept.push_back(a.to_vec());
    if (object, method) == ("Dispatcher", "DispatchEvent")
        && let (Some(Value::Number(e)), Some(Value::String(p))) = (a.first(), a.get(1))
        && *e == f64::from(super::runtime::nav::VIEW_READY)
    {
        cx.vm.natives.view_ready = Some(p.to_string());
    }
    cx.vm.natives.queue.push((object, method, a.to_vec()));
    cx.vm.diag.note("native call", &format!("{object}.{method}"));
    Ok(Value::Undefined)
}

pub fn describe(cx: &mut Ctx, v: &Value) -> String {
    match v {
        Value::String(s) => format!("{s:?}"),
        Value::Object(_) if !cx.is_function(v) && cx.display_of(v).is_none() => {
            let keys = cx.enumerate(v);
            let mut parts = Vec::new();
            for k in keys.iter().take(12) {
                let x = cx.get_member(v, k).unwrap_or_default();
                let s = match &x {
                    Value::Object(_) => "[object]".to_string(),
                    Value::String(s) => format!("{s:?}"),
                    p => cx.to_string(p).map(|s| s.to_string()).unwrap_or_default(),
                };
                parts.push(format!("{k}: {s}"));
            }
            format!("{{{}}}", parts.join(", "))
        }
        _ => cx.to_string(v).map(|s| s.to_string()).unwrap_or_default(),
    }
}

macro_rules! stub {
    ($name:ident, $object:literal, $method:literal) => {
        fn $name(cx: &mut Ctx, _: Value, a: &[Value]) -> R {
            record(cx, $object, $method, a)
        }
    };
}

stub!(lds_subscribe, "LocalDataStore", "Subscribe");
stub!(lds_unsubscribe, "LocalDataStore", "Unsubscribe");
stub!(lds_bind, "LocalDataStore", "BindData");
stub!(lds_unbind, "LocalDataStore", "UnbindData");
stub!(d_dispatch, "Dispatcher", "DispatchEvent");
stub!(d_query, "Dispatcher", "DispatchQueryEvent");
stub!(d_data_action, "Dispatcher", "DataAction");
stub!(d_action, "Dispatcher", "Action");
stub!(d_register, "Dispatcher", "RegisterNavActionHandler");
stub!(d_unregister, "Dispatcher", "UnRegisterNavActionHandler");
stub!(i_subscribe, "InputProvider", "Subscribe");
stub!(i_unsubscribe, "InputProvider", "Unsubscribe");
stub!(r_acquire, "ResourceHandler", "AcquireResource");

pub(crate) fn install(b: &mut Builder, global: ObjId, names: &mut HashMap<ObjId, &'static str>) {
    type Methods = &'static [(&'static str, super::object::NativeFn)];
    let objects: [(&str, Methods); 4] = [
        (
            "LocalDataStore",
            &[
                ("Subscribe", lds_subscribe),
                ("Unsubscribe", lds_unsubscribe),
                ("BindData", lds_bind),
                ("UnbindData", lds_unbind),
            ],
        ),
        (
            "Dispatcher",
            &[
                ("DispatchEvent", d_dispatch),
                ("DispatchQueryEvent", d_query),
                ("DataAction", d_data_action),
                ("Action", d_action),
                ("RegisterNavActionHandler", d_register),
                ("UnRegisterNavActionHandler", d_unregister),
            ],
        ),
        ("InputProvider", &[("Subscribe", i_subscribe), ("Unsubscribe", i_unsubscribe)]),
        ("ResourceHandler", &[("AcquireResource", r_acquire)]),
    ];
    for (name, methods) in objects {
        let o = b.obj();
        b.methods(o, methods);
        b.value(global, name, Value::Object(o));
        let _ = names;
    }
}

pub fn set_game_globals(cx: &mut Ctx, width: u32, height: u32, widescreen: bool) {
    let g = Value::Object(cx.vm.global);
    for (k, v) in [
        ("isWidescreen", Value::Bool(widescreen)),
        ("displayWidth", Value::Number(f64::from(width))),
        ("displayHeight", Value::Number(f64::from(height))),
        ("gfxExtensions", Value::Bool(true)),
        ("noInvisibleAdvance", Value::Bool(true)),
        ("platform", Value::str("XENON")),
    ] {
        let _ = cx.set_member(&g, k, v);
    }
}
