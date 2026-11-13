use super::pattern::{Capture, MatchState};
use super::{arg, none, one};
use crate::number::{self, to_int};
use crate::value::{Str, Value};
use crate::vm::{Lua, LuaResult};

const SPECIALS: &[u8] = b"^$*+?.([%-";

fn posrelat(pos: i64, len: usize) -> i64 {
    let pos = if pos < 0 { pos + len as i64 + 1 } else { pos };
    pos.max(0)
}

pub fn open(l: &mut Lua) {
    let t = l.lib_table("string");
    l.lib_fn(t, "byte", |l, a| {
        let s = l.check_str(a, 1)?;
        let b = s.bytes();
        let posi = posrelat(i64::from(l.opt_int(a, 2, 1)?), b.len());
        let pose = posrelat(i64::from(l.opt_int(a, 3, posi as i32)?), b.len());
        let posi = posi.max(1);
        let pose = pose.min(b.len() as i64);
        if posi > pose {
            return none();
        }
        Ok((posi..=pose).map(|i| Value::Num(f64::from(b[i as usize - 1]))).collect())
    });
    l.lib_fn(t, "char", |l, a| {
        let mut out = Vec::new();
        for i in 1..=a.len() {
            let c = l.check_int(a, i)?;
            if !(0..=255).contains(&c) {
                return Err(l.arg_error(i, "invalid value"));
            }
            out.push(c as u8);
        }
        one(Value::bytes(&out))
    });
    l.lib_fn(t, "dump", |l, a| {
        l.check_func(a, 1)?;
        // PORT(choice): our bytecode is not Lua's, so no precompiled chunk can be produced (no game script calls
        // `string.dump`); Lua 5.1's dump format would be implemented if a script needs it.
        Err(l.rt_error("unable to dump given function"))
    });
    l.lib_fn(t, "find", |l, a| find_aux(l, a, true));
    l.lib_fn(t, "format", format);
    l.lib_fn(t, "gmatch", gmatch);
    l.lib_fn(t, "gfind", gmatch);
    l.lib_fn(t, "gsub", gsub);
    l.lib_fn(t, "len", |l, a| one(l.check_str(a, 1)?.bytes().len() as f64));
    l.lib_fn(t, "lower", |l, a| one(Value::bytes(&l.check_str(a, 1)?.bytes().to_ascii_lowercase())));
    l.lib_fn(t, "match", |l, a| find_aux(l, a, false));
    l.lib_fn(t, "rep", |l, a| {
        let s = l.check_str(a, 1)?;
        let n = l.check_int(a, 2)?;
        let n = n.max(0) as usize;
        if s.bytes().len().saturating_mul(n) > 1 << 28 {
            return Err(l.rt_error("not enough memory"));
        }
        one(Value::bytes(&s.bytes().repeat(n)))
    });
    l.lib_fn(t, "reverse", |l, a| {
        let mut b = l.check_str(a, 1)?.bytes().to_vec();
        b.reverse();
        one(Value::bytes(&b))
    });
    l.lib_fn(t, "sub", |l, a| {
        let s = l.check_str(a, 1)?;
        let b = s.bytes();
        let start = posrelat(i64::from(l.check_int(a, 2)?), b.len()).max(1);
        let end = posrelat(i64::from(l.opt_int(a, 3, -1)?), b.len()).min(b.len() as i64);
        if start <= end { one(Value::bytes(&b[start as usize - 1..end as usize])) } else { one("") }
    });
    l.lib_fn(t, "upper", |l, a| one(Value::bytes(&l.check_str(a, 1)?.bytes().to_ascii_uppercase())));
    let mt = l.new_table(0, 1);
    let _ = l.heap.table_mut(mt).set(Value::str("__index"), Value::Table(t));
    l.type_meta.retain(|(n, _)| *n != "string");
    l.type_meta.push(("string", mt));
}

fn cap_value(src: &[u8], c: Capture) -> Value {
    match c {
        Capture::Str(s, e) => Value::bytes(&src[s..e]),
        Capture::Pos(p) => Value::Num(p as f64),
    }
}

fn find_aux(l: &mut Lua, a: &[Value], find: bool) -> LuaResult<Vec<Value>> {
    let s = l.check_str(a, 1)?;
    let p = l.check_str(a, 2)?;
    let (sb, pb) = (s.bytes(), p.bytes());
    let mut init = posrelat(i64::from(l.opt_int(a, 3, 1)?), sb.len()) - 1;
    if init < 0 {
        init = 0;
    } else if init as usize > sb.len() {
        init = sb.len() as i64;
    }
    let init = init as usize;
    let pz = pb.iter().position(|c| *c == 0).map_or(pb, |i| &pb[..i]);
    let plain = arg(a, 4).is_some_and(Value::truthy) || !pz.iter().any(|c| SPECIALS.contains(c));
    if find && plain {
        if pb.is_empty() {
            return Ok(vec![Value::Num(init as f64 + 1.0), Value::Num(init as f64)]);
        }
        if let Some(i) = sb[init..].windows(pb.len()).position(|w| w == pb) {
            let st = init + i;
            return Ok(vec![Value::Num(st as f64 + 1.0), Value::Num((st + pb.len()) as f64)]);
        }
        return one(Value::Nil);
    }
    let (anchor, pat) = if pb.first() == Some(&b'^') { (true, &pb[1..]) } else { (false, pb) };
    let mut s1 = init;
    loop {
        let mut ms = MatchState::new(sb, pat);
        if let Some(e) = ms.do_match(s1, 0).map_err(|m| l.rt_error(&m))? {
            if find {
                let mut out = vec![Value::Num(s1 as f64 + 1.0), Value::Num(e as f64)];
                let caps = ms.captures(s1, e, false).map_err(|m| l.rt_error(&m))?;
                out.extend(caps.into_iter().map(|c| cap_value(sb, c)));
                return Ok(out);
            }
            let caps = ms.captures(s1, e, true).map_err(|m| l.rt_error(&m))?;
            return Ok(caps.into_iter().map(|c| cap_value(sb, c)).collect());
        }
        s1 += 1;
        if s1 > sb.len() || anchor {
            break;
        }
    }
    one(Value::Nil)
}

fn gmatch(l: &mut Lua, a: &[Value]) -> LuaResult<Vec<Value>> {
    let s = l.check_str(a, 1)?;
    let p = l.check_str(a, 2)?;
    let f = l.create_function_with(
        "gmatch_aux",
        std::rc::Rc::new(|l: &mut Lua, _: &[Value]| {
            let (Value::Str(s), Value::Str(p), Value::Num(start)) = (l.upvalue(0), l.upvalue(1), l.upvalue(2)) else {
                return none();
            };
            let (sb, pb) = (s.bytes(), p.bytes());
            let mut src = start as usize;
            while src <= sb.len() {
                let mut ms = MatchState::new(sb, pb);
                if let Some(e) = ms.do_match(src, 0).map_err(|m| l.rt_error(&m))? {
                    let next = if e == src { e + 1 } else { e };
                    set_upvalue(l, 2, Value::Num(next as f64));
                    let caps = ms.captures(src, e, true).map_err(|m| l.rt_error(&m))?;
                    return Ok(caps.into_iter().map(|c| cap_value(sb, c)).collect());
                }
                src += 1;
            }
            none()
        }),
        vec![Value::Str(s), Value::Str(p), Value::Num(0.0)],
    );
    one(f)
}

fn set_upvalue(l: &mut Lua, i: usize, v: Value) {
    if let Some(fr) = l.th.frames.last() {
        let f = fr.func;
        if let crate::heap::Function::Native(c) = l.heap.func_mut(f)
            && let Some(u) = c.upvals.get_mut(i)
        {
            *u = v;
        }
    }
}

fn gsub(l: &mut Lua, a: &[Value]) -> LuaResult<Vec<Value>> {
    let s = l.check_str(a, 1)?;
    let p = l.check_str(a, 2)?;
    let repl = arg(a, 3).cloned().unwrap_or_default();
    if !matches!(repl, Value::Num(_) | Value::Str(_) | Value::Func(_) | Value::Table(_)) {
        return Err(l.arg_error(3, "string/function/table expected"));
    }
    let (sb, pb) = (s.bytes(), p.bytes());
    let max_s = l.opt_int(a, 4, sb.len() as i32 + 1)?;
    let (anchor, pat) = if pb.first() == Some(&b'^') { (true, &pb[1..]) } else { (false, pb) };
    let mut out = Vec::new();
    let mut src = 0usize;
    let mut n = 0;
    while n < max_s {
        let mut ms = MatchState::new(sb, pat);
        let e = ms.do_match(src, 0).map_err(|m| l.rt_error(&m))?;
        if let Some(e) = e {
            n += 1;
            add_value(l, &ms, &mut out, src, e, &repl)?;
        }
        match e {
            Some(e) if e > src => src = e,
            _ if src < sb.len() => {
                out.push(sb[src]);
                src += 1;
            }
            _ => break,
        }
        if anchor {
            break;
        }
    }
    out.extend_from_slice(&sb[src..]);
    Ok(vec![Value::bytes(&out), Value::Num(f64::from(n))])
}

fn add_value(l: &mut Lua, ms: &MatchState, out: &mut Vec<u8>, s: usize, e: usize, repl: &Value) -> LuaResult<()> {
    let src = ms.src;
    let v = match repl {
        Value::Num(_) | Value::Str(_) => {
            let news = repl.to_str_coerce().unwrap_or_else(|| Str::from(""));
            let nb = news.bytes();
            let mut i = 0;
            while i < nb.len() {
                if nb[i] != b'%' {
                    out.push(nb[i]);
                } else {
                    i += 1;
                    let c = nb.get(i).copied().unwrap_or(0);
                    if !c.is_ascii_digit() {
                        out.push(c);
                    } else if c == b'0' {
                        out.extend_from_slice(&src[s..e]);
                    } else {
                        let cap = ms.capture(usize::from(c - b'1'), s, e).map_err(|m| l.rt_error(&m))?;
                        if let Some(t) = cap_value(src, cap).to_str_coerce() {
                            out.extend_from_slice(t.bytes());
                        }
                    }
                }
                i += 1;
            }
            return Ok(());
        }
        Value::Func(_) => {
            let caps = ms.captures(s, e, true).map_err(|m| l.rt_error(&m))?;
            let args: Vec<Value> = caps.into_iter().map(|c| cap_value(src, c)).collect();
            l.call(repl.clone(), &args)?.into_iter().next().unwrap_or_default()
        }
        Value::Table(_) => {
            let k = cap_value(src, ms.capture(0, s, e).map_err(|m| l.rt_error(&m))?);
            l.index(repl, &k, None)?
        }
        _ => Value::Nil,
    };
    if !v.truthy() {
        out.extend_from_slice(&src[s..e]);
    } else if let Some(t) = v.to_str_coerce() {
        out.extend_from_slice(t.bytes());
    } else {
        return Err(l.rt_error(&format!("invalid replacement value (a {})", v.type_name())));
    }
    Ok(())
}

struct Spec {
    flags: Vec<u8>,
    width: usize,
    prec: Option<usize>,
}

impl Spec {
    fn has(&self, f: u8) -> bool {
        self.flags.contains(&f)
    }

    fn pad(&self, sign: &str, body: &str, numeric: bool) -> String {
        let len = sign.len() + body.len();
        if len >= self.width {
            return format!("{sign}{body}");
        }
        let fill = self.width - len;
        if self.has(b'-') {
            format!("{sign}{body}{}", " ".repeat(fill))
        } else if numeric && self.has(b'0') {
            format!("{sign}{}{body}", "0".repeat(fill))
        } else {
            format!("{}{sign}{body}", " ".repeat(fill))
        }
    }
}

fn format(l: &mut Lua, a: &[Value]) -> LuaResult<Vec<Value>> {
    let fs = l.check_str(a, 1)?;
    let f = fs.bytes();
    let mut out: Vec<u8> = Vec::new();
    let mut argn = 1;
    let mut i = 0;
    while i < f.len() {
        if f[i] != b'%' {
            out.push(f[i]);
            i += 1;
            continue;
        }
        i += 1;
        if f.get(i) == Some(&b'%') {
            out.push(b'%');
            i += 1;
            continue;
        }
        argn += 1;
        if argn > a.len() {
            return Err(l.arg_error(argn, "no value"));
        }
        let start = i;
        while i < f.len() && b"-+ #0".contains(&f[i]) {
            i += 1;
        }
        if i - start >= 6 {
            return Err(l.rt_error("invalid format (repeated flags)"));
        }
        let flags = f[start..i].to_vec();
        let digits = |i: &mut usize| {
            let s = *i;
            for _ in 0..2 {
                if *i < f.len() && f[*i].is_ascii_digit() {
                    *i += 1;
                }
            }
            std::str::from_utf8(&f[s..*i]).ok().and_then(|t| t.parse::<usize>().ok())
        };
        let width = digits(&mut i).unwrap_or(0);
        let mut prec = None;
        if f.get(i) == Some(&b'.') {
            i += 1;
            prec = Some(digits(&mut i).unwrap_or(0));
        }
        if f.get(i).is_some_and(|c| c.is_ascii_digit()) {
            return Err(l.rt_error("invalid format (width or precision too long)"));
        }
        let conv = f.get(i).copied().unwrap_or(0);
        i += 1;
        let spec = Spec { flags, width, prec };
        let item: Vec<u8> = match conv {
            b'c' => {
                let c = to_int(l.check_num(a, argn)?) as u8;
                spec.pad("", &(c as char).to_string(), false).into_bytes()
            }
            b'd' | b'i' => {
                let n = i64::from(to_int(l.check_num(a, argn)?));
                let mut body = n.unsigned_abs().to_string();
                if let Some(p) = spec.prec {
                    if body.len() < p {
                        body = format!("{}{body}", "0".repeat(p - body.len()));
                    }
                    if p == 0 && n == 0 {
                        body.clear();
                    }
                }
                let sign = if n < 0 {
                    "-"
                } else if spec.has(b'+') {
                    "+"
                } else if spec.has(b' ') {
                    " "
                } else {
                    ""
                };
                spec.pad(sign, &body, spec.prec.is_none()).into_bytes()
            }
            b'o' | b'u' | b'x' | b'X' => {
                let x = l.check_num(a, argn)?;
                // TODO(guess): (unsigned long) of a negative double is taken to wrap like the 64-bit `fctidz` conversion; settle: disassemble `str_format`'s 'x' case.
                let u = if x < 0.0 { (x as i64) as u32 } else { x.min(u32::MAX as f64) as u32 };
                let mut body = match conv {
                    b'o' => format!("{u:o}"),
                    b'u' => u.to_string(),
                    b'x' => format!("{u:x}"),
                    _ => format!("{u:X}"),
                };
                if let Some(p) = spec.prec
                    && body.len() < p
                {
                    body = format!("{}{body}", "0".repeat(p - body.len()));
                }
                let prefix = match conv {
                    b'o' if spec.has(b'#') && !body.starts_with('0') => "0",
                    b'x' if spec.has(b'#') && u != 0 => "0x",
                    b'X' if spec.has(b'#') && u != 0 => "0X",
                    _ => "",
                };
                spec.pad(prefix, &body, spec.prec.is_none()).into_bytes()
            }
            b'e' | b'E' | b'f' | b'g' | b'G' => {
                let x = l.check_num(a, argn)?;
                let body = number::fmt_float(x.abs(), conv, spec.prec.unwrap_or(6), spec.has(b'#'));
                let neg = x.is_sign_negative() && !(x.is_nan() && !x.is_sign_negative());
                let sign = if neg {
                    "-"
                } else if spec.has(b'+') {
                    "+"
                } else if spec.has(b' ') {
                    " "
                } else {
                    ""
                };
                spec.pad(sign, &body, x.is_finite()).into_bytes()
            }
            b'q' => {
                let s = l.check_str(a, argn)?;
                let mut q = vec![b'"'];
                for &c in s.bytes() {
                    match c {
                        b'"' | b'\\' | b'\n' => {
                            q.push(b'\\');
                            q.push(c);
                        }
                        b'\r' => q.extend_from_slice(b"\\r"),
                        0 => q.extend_from_slice(b"\\000"),
                        c => q.push(c),
                    }
                }
                q.push(b'"');
                q
            }
            b's' => {
                let s = l.check_str(a, argn)?;
                let b = s.bytes();
                if spec.prec.is_none() && b.len() >= 100 {
                    b.to_vec()
                } else {
                    let z = b.iter().position(|c| *c == 0).map_or(b, |i| &b[..i]);
                    let z = match spec.prec {
                        Some(p) if p < z.len() => &z[..p],
                        _ => z,
                    };
                    if spec.width > z.len() {
                        let fill = vec![b' '; spec.width - z.len()];
                        if spec.has(b'-') { [z, &fill[..]].concat() } else { [&fill[..], z].concat() }
                    } else {
                        z.to_vec()
                    }
                }
            }
            c => {
                return Err(l.rt_error(&format!("invalid option '%{}' to 'format'", c as char)));
            }
        };
        let item = if conv == b'c' { item.split(|c| *c == 0).next().unwrap_or(&[]).to_vec() } else { item };
        out.extend_from_slice(&item);
    }
    one(Value::bytes(&out))
}
