pub mod array;
pub mod geom;
pub mod misc;
pub mod movieclip;
pub mod string;

use std::collections::HashMap;

use super::object::{Func, FuncCode, Heap, Kind, NativeFn, ObjId, Object, Property, flags};
use super::value::{Str, Value, to_int32};
use super::{Ctx, Fault, Interval, Protos};

pub(crate) type R = Result<Value, Fault>;

pub(crate) fn arg(args: &[Value], i: usize) -> Value {
    args.get(i).cloned().unwrap_or_default()
}

pub(crate) struct Builder<'h> {
    pub heap: &'h mut Heap,
    pub object_proto: ObjId,
    pub function_proto: ObjId,
}

const HIDDEN: u8 = flags::DONT_ENUM | flags::DONT_DELETE;

impl Builder<'_> {
    pub fn obj(&mut self) -> ObjId {
        self.heap.alloc(Object::new(Value::Object(self.object_proto), Kind::Plain))
    }

    pub fn native(&mut self, f: NativeFn) -> ObjId {
        let func = Func { code: FuncCode::Native(f), scope: None, base: None };
        self.heap.alloc(Object::new(Value::Object(self.function_proto), Kind::Function(Box::new(func))))
    }

    pub fn value(&mut self, target: ObjId, name: &str, v: Value) {
        if let Some(o) = self.heap.get_mut(target) {
            o.props.insert(Str::from(name), Property::new(v, HIDDEN));
        }
    }

    pub fn method(&mut self, target: ObjId, name: &str, f: NativeFn) {
        let n = self.native(f);
        self.value(target, name, Value::Object(n));
    }

    pub fn methods(&mut self, target: ObjId, list: &[(&str, NativeFn)]) {
        for (n, f) in list {
            self.method(target, n, *f);
        }
    }

    pub fn class(&mut self, package: ObjId, name: &str, ctor: NativeFn, proto: ObjId) -> ObjId {
        let c = self.native(ctor);
        self.value(c, "prototype", Value::Object(proto));
        self.value(proto, "constructor", Value::Object(c));
        self.value(package, name, Value::Object(c));
        c
    }
}

pub(crate) fn install(heap: &mut Heap) -> (ObjId, Protos, HashMap<ObjId, &'static str>) {
    let object_proto = heap.alloc(Object::new(Value::Null, Kind::Plain));
    let function_proto = heap.alloc(Object::new(Value::Object(object_proto), Kind::Plain));
    let mut b = Builder { heap, object_proto, function_proto };
    let global = b.obj();
    let mut names = HashMap::new();

    let object = b.class(global, "Object", object_ctor, object_proto);
    b.methods(object, &[("registerClass", object_register_class)]);
    b.methods(
        object_proto,
        &[
            ("toString", object_to_string),
            ("valueOf", object_value_of),
            ("hasOwnProperty", object_has_own),
            ("addProperty", object_add_property),
            ("isPropertyEnumerable", object_is_enumerable),
            ("isPrototypeOf", object_is_prototype_of),
            ("watch", object_watch),
            ("unwatch", object_watch),
        ],
    );
    let function = b.class(global, "Function", function_ctor, function_proto);
    let _ = function;
    b.methods(function_proto, &[("call", function_call), ("apply", function_apply)]);
    b.methods(
        global,
        &[
            ("ASSetPropFlags", as_set_prop_flags),
            ("isNaN", is_nan),
            ("isFinite", is_finite),
            ("parseInt", parse_int),
            ("parseFloat", parse_float),
            ("setInterval", set_interval),
            ("setTimeout", set_timeout),
            ("clearInterval", clear_interval),
            ("clearTimeout", clear_interval),
            ("trace", trace),
            ("escape", string::escape),
            ("unescape", string::unescape),
            ("getTimer", get_timer),
            ("updateAfterEvent", no_op),
        ],
    );
    b.value(global, "NaN", Value::Number(f64::NAN));
    b.value(global, "Infinity", Value::Number(f64::INFINITY));
    names.insert(object_proto, "Object");
    names.insert(function_proto, "Function");

    let array = array::install(&mut b, global);
    let (string, number, boolean, date) = string::install(&mut b, global);
    let (movieclip, textfield, textformat, loader) = movieclip::install(&mut b, global);
    let flash = b.obj();
    b.value(global, "flash", Value::Object(flash));
    let geom::GeomClasses { color, color_transform, transform, matrix, point, rectangle, filter } =
        geom::install(&mut b, global, flash);
    misc::install(&mut b, global);
    for (p, n) in [
        (array, "Array"),
        (string, "String"),
        (number, "Number"),
        (boolean, "Boolean"),
        (date, "Date"),
        (movieclip, "MovieClip"),
        (textfield, "TextField"),
        (textformat, "TextFormat"),
        (loader, "MovieClipLoader"),
        (color, "Color"),
        (color_transform, "ColorTransform"),
        (transform, "Transform"),
        (matrix, "Matrix"),
        (point, "Point"),
        (rectangle, "Rectangle"),
        (filter, "BitmapFilter"),
    ] {
        names.insert(p, n);
    }
    let protos = Protos {
        object: object_proto,
        function: function_proto,
        array,
        string,
        number,
        boolean,
        date,
        movieclip,
        textfield,
        textformat,
        color,
        color_transform,
        transform,
        matrix,
        point,
        rectangle,
        loader,
        filter,
    };
    super::natives::install(&mut b, global, &mut names);
    (global, protos, names)
}

pub(crate) fn no_op(_: &mut Ctx, _: Value, _: &[Value]) -> R {
    Ok(Value::Undefined)
}

fn object_ctor(cx: &mut Ctx, this: Value, args: &[Value]) -> R {
    match arg(args, 0) {
        v @ Value::Object(_) => Ok(v),
        Value::Undefined | Value::Null if cx.vm.constructing => Ok(this),
        Value::Undefined | Value::Null => Ok(Value::Object(cx.new_object())),
        v => Ok(string::box_primitive(cx, v)),
    }
}

fn object_register_class(cx: &mut Ctx, _: Value, args: &[Value]) -> R {
    if args.len() < 2 {
        return Ok(Value::Bool(false));
    }
    let name = cx.to_string(&args[0])?;
    match &args[1] {
        Value::Null => {
            cx.vm.classes.remove(&name);
            Ok(Value::Bool(true))
        }
        f if cx.is_function(f) => {
            cx.vm.classes.insert(name, f.clone());
            Ok(Value::Bool(true))
        }
        _ => Ok(Value::Bool(false)),
    }
}

fn object_to_string(cx: &mut Ctx, this: Value, _: &[Value]) -> R {
    Ok(Value::str(if cx.is_function(&this) { "[type Function]" } else { "[object Object]" }))
}

fn object_value_of(_: &mut Ctx, this: Value, _: &[Value]) -> R {
    Ok(this)
}

fn object_has_own(cx: &mut Ctx, this: Value, args: &[Value]) -> R {
    let name = cx.to_string(&arg(args, 0))?;
    let Some(o) = this.as_object() else { return Ok(Value::Bool(false)) };
    let own = cx.vm.heap.get(o).is_some_and(|x| match &x.kind {
        Kind::Array(a) => super::members::array_index(&name).is_some_and(|i| i < a.len()) || x.props.contains(&name),
        _ => x.props.contains(&name),
    });
    Ok(Value::Bool(own))
}

fn object_add_property(cx: &mut Ctx, this: Value, args: &[Value]) -> R {
    let name = cx.to_string(&arg(args, 0))?;
    let (get, set) = (arg(args, 1), arg(args, 2));
    let ok = !name.is_empty() && cx.is_function(&get) && (matches!(set, Value::Null) || cx.is_function(&set));
    if let (true, Some(o)) = (ok, this.as_object().and_then(|o| cx.vm.heap.get_mut(o))) {
        o.props.insert(name, Property { value: Value::Undefined, flags: 0, accessor: Some(Box::new((get, set))) });
        return Ok(Value::Bool(true));
    }
    Ok(Value::Bool(false))
}

fn object_is_enumerable(cx: &mut Ctx, this: Value, args: &[Value]) -> R {
    let name = cx.to_string(&arg(args, 0))?;
    let r = this
        .as_object()
        .and_then(|o| cx.vm.heap.get(o))
        .and_then(|o| o.props.get(&name))
        .is_some_and(|p| p.flags & flags::DONT_ENUM == 0);
    Ok(Value::Bool(r))
}

fn object_is_prototype_of(cx: &mut Ctx, this: Value, args: &[Value]) -> R {
    let Some(me) = this.as_object() else { return Ok(Value::Bool(false)) };
    let mut cur = cx.proto_of(&arg(args, 0)).as_object();
    for _ in 0..64 {
        let Some(c) = cur else { break };
        if c == me {
            return Ok(Value::Bool(true));
        }
        cur = cx.vm.heap.get(c).and_then(|o| o.proto.as_object());
    }
    Ok(Value::Bool(false))
}

fn object_watch(cx: &mut Ctx, _: Value, _: &[Value]) -> R {
    cx.vm.diag.note("unsupported", "Object.watch");
    Ok(Value::Bool(false))
}

fn function_ctor(_: &mut Ctx, this: Value, _: &[Value]) -> R {
    Ok(this)
}

fn function_call(cx: &mut Ctx, this: Value, args: &[Value]) -> R {
    let rest = if args.len() > 1 { &args[1..] } else { &[] };
    cx.call(&this, arg(args, 0), rest, Value::Undefined)
}

fn function_apply(cx: &mut Ctx, this: Value, args: &[Value]) -> R {
    let list = match cx.kind_of(&arg(args, 1)) {
        Some(Kind::Array(a)) => a.clone(),
        _ => Vec::new(),
    };
    cx.call(&this, arg(args, 0), &list, Value::Undefined)
}

fn as_set_prop_flags(cx: &mut Ctx, _: Value, args: &[Value]) -> R {
    let Some(o) = arg(args, 0).as_object() else { return Ok(Value::Undefined) };
    let set = to_int32(cx.to_number(&arg(args, 2))) as u8 & 7;
    let clear = if args.len() > 3 { to_int32(cx.to_number(&args[3])) as u8 & 7 } else { 0 };
    let names: Option<Vec<Str>> = match arg(args, 1) {
        Value::Null | Value::Undefined => None,
        v @ Value::Object(_) => match cx.kind_of(&v) {
            Some(Kind::Array(a)) => {
                let a = a.clone();
                let mut out = Vec::new();
                for x in &a {
                    out.push(cx.to_string(x)?);
                }
                Some(out)
            }
            _ => Some(vec![cx.to_string(&v)?]),
        },
        v => Some(cx.to_string(&v)?.split(',').map(|s| Str::from(s.trim())).collect()),
    };
    if let Some(obj) = cx.vm.heap.get_mut(o) {
        for (k, p) in obj.props.iter_mut() {
            if names.as_ref().is_none_or(|n| n.iter().any(|x| x == k)) {
                p.flags = (p.flags & !clear) | set;
            }
        }
    }
    Ok(Value::Undefined)
}

fn is_nan(cx: &mut Ctx, _: Value, args: &[Value]) -> R {
    Ok(Value::Bool(cx.to_number(&arg(args, 0)).is_nan()))
}

fn is_finite(cx: &mut Ctx, _: Value, args: &[Value]) -> R {
    Ok(Value::Bool(cx.to_number(&arg(args, 0)).is_finite()))
}

fn parse_int(cx: &mut Ctx, _: Value, args: &[Value]) -> R {
    let s = cx.to_string(&arg(args, 0))?;
    let mut radix = if args.len() > 1 { to_int32(cx.to_number(&args[1])) } else { 0 };
    let t = s.trim_start();
    let (neg, mut t) = match t.as_bytes().first() {
        Some(b'-') => (true, &t[1..]),
        Some(b'+') => (false, &t[1..]),
        _ => (false, t),
    };
    if (radix == 0 || radix == 16) && (t.starts_with("0x") || t.starts_with("0X")) {
        t = &t[2..];
        radix = 16;
    } else if radix == 0 {
        radix = if t.starts_with('0') && t.len() > 1 { 8 } else { 10 };
    }
    if !(2..=36).contains(&radix) {
        return Ok(Value::Number(f64::NAN));
    }
    let digits: String = t.chars().take_while(|c| c.is_digit(radix as u32)).collect();
    if digits.is_empty() {
        return Ok(Value::Number(f64::NAN));
    }
    let v =
        digits.chars().fold(0f64, |acc, c| acc * f64::from(radix) + f64::from(c.to_digit(radix as u32).unwrap_or(0)));
    Ok(Value::Number(if neg { -v } else { v }))
}

fn parse_float(cx: &mut Ctx, _: Value, args: &[Value]) -> R {
    let s = cx.to_string(&arg(args, 0))?;
    let t = s.trim_start();
    let mut end = 0;
    let mut seen_dot = false;
    let mut seen_e = false;
    let b = t.as_bytes();
    while end < b.len() {
        let c = b[end];
        let ok = c.is_ascii_digit()
            || (c == b'.' && !seen_dot && !seen_e)
            || ((c == b'-' || c == b'+') && (end == 0 || b[end - 1] | 0x20 == b'e'))
            || (c | 0x20 == b'e' && !seen_e && end > 0);
        if !ok {
            break;
        }
        seen_dot |= c == b'.';
        seen_e |= c | 0x20 == b'e';
        end += 1;
    }
    let mut cand = &t[..end];
    while !cand.is_empty() {
        if let Ok(v) = cand.parse::<f64>() {
            return Ok(Value::Number(v));
        }
        cand = &cand[..cand.len() - 1];
    }
    Ok(Value::Number(f64::NAN))
}

fn add_interval(cx: &mut Ctx, args: &[Value], once: bool) -> R {
    let (func, this, rest) = if cx.is_function(&arg(args, 0)) {
        (arg(args, 0), Value::Undefined, args.get(1..).unwrap_or(&[]))
    } else {
        let m = cx.to_string(&arg(args, 1))?;
        (Value::String(m), arg(args, 0), args.get(2..).unwrap_or(&[]))
    };
    let ms = cx.to_number(&arg(rest, 0));
    let period = if ms.is_finite() { ms.max(0.0) / 1000.0 } else { 0.0 };
    let id = cx.vm.next_interval;
    cx.vm.next_interval += 1;
    let next = cx.vm.time + period;
    let args = rest.get(1..).unwrap_or(&[]).to_vec();
    cx.vm.intervals.insert(id, Interval { func, this, args, period, next, once });
    Ok(Value::Number(f64::from(id)))
}

fn set_interval(cx: &mut Ctx, _: Value, args: &[Value]) -> R {
    add_interval(cx, args, false)
}

fn set_timeout(cx: &mut Ctx, _: Value, args: &[Value]) -> R {
    add_interval(cx, args, true)
}

fn clear_interval(cx: &mut Ctx, _: Value, args: &[Value]) -> R {
    let id = to_int32(cx.to_number(&arg(args, 0)));
    cx.vm.intervals.remove(&id);
    Ok(Value::Undefined)
}

fn trace(cx: &mut Ctx, _: Value, args: &[Value]) -> R {
    let s = cx.to_string(&arg(args, 0))?;
    cx.vm.trace(&s);
    Ok(Value::Undefined)
}

fn get_timer(cx: &mut Ctx, _: Value, _: &[Value]) -> R {
    Ok(Value::Number(cx.vm.timer_ms()))
}

pub const API_METHODS: &[&str] = &[
    "play",
    "stop",
    "gotoAndStop",
    "gotoAndPlay",
    "nextFrame",
    "prevFrame",
    "getBytesLoaded",
    "getBytesTotal",
    "getBounds",
    "getRect",
    "localToGlobal",
    "globalToLocal",
    "hitTest",
    "attachBitmap",
    "attachMovie",
    "duplicateMovieClip",
    "removeMovieClip",
    "createEmptyMovieClip",
    "createTextField",
    "getDepth",
    "swapDepths",
    "getNextHighestDepth",
    "getInstanceAtDepth",
    "getTextSnapshot",
    "getSWFVersion",
    "startDrag",
    "stopDrag",
    "setMask",
    "loadMovie",
    "unloadMovie",
    "loadVariables",
    "clear",
    "beginFill",
    "beginGradientFill",
    "beginBitmapFill",
    "lineGradientStyle",
    "endFill",
    "lineStyle",
    "moveTo",
    "lineTo",
    "curveTo",
    "getTextFormat",
    "getNewTextFormat",
    "setTextFormat",
    "setNewTextFormat",
    "replaceSel",
    "replaceText",
    "removeTextField",
    "appendText",
    "appendHtml",
    "copyToClipboard",
    "cutToClipboard",
    "getCharBoundaries",
    "getCharIndexAtPoint",
    "getExactCharBoundaries",
    "getFirstCharInParagraph",
    "getLineIndexAtPoint",
    "getLineIndexOfChar",
    "getLineOffset",
    "getLineMetrics",
    "getLineText",
    "getLineLength",
    "pasteFromClipboard",
    "concat",
    "join",
    "pop",
    "push",
    "reverse",
    "shift",
    "slice",
    "sort",
    "sortOn",
    "splice",
    "unshift",
    "addProperty",
    "hasOwnProperty",
    "watch",
    "unwatch",
    "isPropertyEnumerable",
    "isPrototypeOf",
    "toString",
    "valueOf",
    "registerClass",
    "call",
    "apply",
    "charAt",
    "charCodeAt",
    "fromCharCode",
    "indexOf",
    "lastIndexOf",
    "split",
    "substr",
    "substring",
    "toLowerCase",
    "toUpperCase",
    "abs",
    "acos",
    "asin",
    "atan",
    "atan2",
    "ceil",
    "cos",
    "exp",
    "floor",
    "log",
    "max",
    "min",
    "pow",
    "random",
    "round",
    "sin",
    "sqrt",
    "tan",
    "getProgress",
    "loadClip",
    "unloadClip",
    "addListener",
    "removeListener",
    "broadcastMessage",
    "getAscii",
    "getCode",
    "isDown",
    "isToggled",
    "hide",
    "show",
    "getFocus",
    "setFocus",
    "setSelection",
    "getBeginIndex",
    "getCaretIndex",
    "getEndIndex",
    "setRGB",
    "getRGB",
    "setTransform",
    "getTransform",
    "clone",
    "contains",
    "containsPoint",
    "containsRectangle",
    "equals",
    "inflate",
    "inflatePoint",
    "intersection",
    "intersects",
    "isEmpty",
    "offset",
    "offsetPoint",
    "setEmpty",
    "union",
    "createBox",
    "createGradientBox",
    "deltaTransformPoint",
    "identity",
    "invert",
    "rotate",
    "scale",
    "transformPoint",
    "translate",
    "normalize",
    "subtract",
    "interpolate",
    "distance",
    "polar",
    "getDate",
    "getDay",
    "getFullYear",
    "getHours",
    "getMilliseconds",
    "getMinutes",
    "getMonth",
    "getSeconds",
    "getTime",
    "getTimezoneOffset",
    "getYear",
    "getUTCDate",
    "getUTCDay",
    "getUTCFullYear",
    "getUTCHours",
    "getUTCMilliseconds",
    "getUTCMinutes",
    "getUTCMonth",
    "getUTCSeconds",
    "setDate",
    "setFullYear",
    "setHours",
    "setMilliseconds",
    "setMinutes",
    "setMonth",
    "setSeconds",
    "setTime",
    "setYear",
    "UTC",
];
