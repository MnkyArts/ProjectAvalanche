use std::collections::HashMap;
use std::sync::Arc;

use ssx_formats::gfx::avm1::{self, Action};

use super::value::{Value, format_g, number_to_radix, number_to_string, parse_number, to_int32};
use super::{Ctx, Vm};
use crate::display::Player;
use crate::display::tests::{empty_lib, movie};
use crate::library::{Library, MovieId};

#[derive(Clone, Copy)]
enum P<'a> {
    S(&'a str),
    I(i32),
    B(bool),
    Null,
    Undef,
    R(u8),
}

#[derive(Default)]
struct Asm {
    b: Vec<u8>,
    labels: HashMap<&'static str, usize>,
    fixups: Vec<(usize, &'static str)>,
}

impl Asm {
    fn new() -> Self {
        Self::default()
    }

    fn op(mut self, op: u8) -> Self {
        self.b.push(op);
        self
    }

    fn data(mut self, op: u8, d: &[u8]) -> Self {
        self.b.push(op);
        self.b.extend_from_slice(&(d.len() as u16).to_le_bytes());
        self.b.extend_from_slice(d);
        self
    }

    fn push(self, vals: &[P]) -> Self {
        let mut d = Vec::new();
        for v in vals {
            match v {
                P::S(s) => {
                    d.push(0);
                    d.extend_from_slice(s.as_bytes());
                    d.push(0);
                }
                P::I(i) => {
                    d.push(7);
                    d.extend_from_slice(&i.to_le_bytes());
                }
                P::B(b) => d.extend_from_slice(&[5, u8::from(*b)]),
                P::Null => d.push(2),
                P::Undef => d.push(3),
                P::R(r) => d.extend_from_slice(&[4, *r]),
            }
        }
        self.data(0x96, &d)
    }

    fn store(self, r: u8) -> Self {
        self.data(0x87, &[r])
    }

    fn label(mut self, name: &'static str) -> Self {
        self.labels.insert(name, self.b.len());
        self
    }

    fn branch(mut self, op: u8, to: &'static str) -> Self {
        self.b.extend_from_slice(&[op, 2, 0, 0, 0]);
        self.fixups.push((self.b.len() - 2, to));
        self
    }

    fn bytes(mut self) -> Vec<u8> {
        for (at, l) in std::mem::take(&mut self.fixups) {
            let delta = self.labels[l] as i64 - (at as i64 + 2);
            self.b[at..at + 2].copy_from_slice(&(delta as i16).to_le_bytes());
        }
        self.b
    }

    fn func2(mut self, name: &str, params: &[(u8, &str)], regs: u8, flags: u16, body: Asm) -> Self {
        let body = body.bytes();
        let mut d = name.as_bytes().to_vec();
        d.push(0);
        d.extend_from_slice(&(params.len() as u16).to_le_bytes());
        d.push(regs);
        d.extend_from_slice(&[(flags >> 8) as u8, flags as u8]);
        for (r, p) in params {
            d.push(*r);
            d.extend_from_slice(p.as_bytes());
            d.push(0);
        }
        d.extend_from_slice(&(body.len() as u16).to_le_bytes());
        self = self.data(0x8E, &d);
        self.b.extend_from_slice(&body);
        self
    }

    fn func(mut self, name: &str, params: &[&str], body: Asm) -> Self {
        let body = body.bytes();
        let mut d = name.as_bytes().to_vec();
        d.push(0);
        d.extend_from_slice(&(params.len() as u16).to_le_bytes());
        for p in params {
            d.extend_from_slice(p.as_bytes());
            d.push(0);
        }
        d.extend_from_slice(&(body.len() as u16).to_le_bytes());
        self = self.data(0x9B, &d);
        self.b.extend_from_slice(&body);
        self
    }

    fn try_catch(mut self, name: &str, t: Asm, c: Asm, f: Option<Asm>) -> Self {
        let (t, c) = (t.bytes(), c.bytes());
        let fin = f.map(Asm::bytes);
        let flags = 1 | if fin.is_some() { 2 } else { 0 };
        let mut d = vec![flags];
        d.extend_from_slice(&(t.len() as u16).to_le_bytes());
        d.extend_from_slice(&(c.len() as u16).to_le_bytes());
        d.extend_from_slice(&(fin.as_ref().map_or(0, Vec::len) as u16).to_le_bytes());
        d.extend_from_slice(name.as_bytes());
        d.push(0);
        self = self.data(0x8F, &d);
        self.b.extend_from_slice(&t);
        self.b.extend_from_slice(&c);
        if let Some(f) = fin {
            self.b.extend_from_slice(&f);
        }
        self
    }

    fn with_block(mut self, body: Asm) -> Self {
        let body = body.bytes();
        self = self.data(0x94, &(body.len() as u16).to_le_bytes());
        self.b.extend_from_slice(&body);
        self
    }

    fn code(self) -> Arc<[Action]> {
        let mut b = self.bytes();
        b.push(0);
        let a = avm1::decode(&b).expect("assembled block decodes");
        avm1::validate(&a, b.len() as u32).expect("assembled block is well formed");
        Arc::from(a)
    }
}

struct World {
    vm: Vm,
    lib: Library,
    player: Player,
    movie: MovieId,
}

impl World {
    fn new() -> World {
        let mut lib = empty_lib();
        let m = movie(&mut lib, vec![vec![]]);
        let mut player = Player::new();
        let mut vm = Vm::new();
        vm.load_level(&mut lib, &mut player, m, 0);
        World { vm, lib, player, movie: m }
    }

    fn cx(&mut self) -> Ctx<'_> {
        self.vm.ctx(&mut self.lib, &mut self.player)
    }

    fn run(&mut self, code: Asm) {
        let movie = self.movie;
        let mut cx = self.cx();
        let root = cx.player.levels[&0];
        let obj = cx.object_for(root);
        cx.run_block(code.code(), obj, 8, movie);
        cx.run_queue();
    }

    fn var(&mut self, name: &str) -> Value {
        let mut cx = self.cx();
        let root = cx.player.levels[&0];
        let obj = Value::Object(cx.object_for(root));
        cx.get_member(&obj, name).unwrap_or_default()
    }

    fn num(&mut self, name: &str) -> f64 {
        match self.var(name) {
            Value::Number(n) => n,
            v => panic!("{name} is {v:?}, not a number"),
        }
    }

    fn text(&mut self, name: &str) -> String {
        let v = self.var(name);
        let mut cx = self.cx();
        cx.to_string(&v).map(|s| s.to_string()).unwrap_or_default()
    }

    fn errors(&self) -> u64 {
        self.vm.diag.counts.iter().filter(|((k, _), _)| k != "native call").map(|(_, n)| n).sum()
    }
}

#[test]
fn number_conversions_follow_scaleform() {
    assert_eq!(parse_number("0x1A"), Some(26.0));
    assert_eq!(parse_number("0XfF"), Some(255.0));
    assert_eq!(parse_number("010"), Some(8.0));
    assert_eq!(parse_number("-010"), Some(-8.0));
    assert_eq!(parse_number("08"), Some(8.0));
    assert_eq!(parse_number("  12"), Some(12.0));
    assert_eq!(parse_number("12 "), None);
    assert_eq!(parse_number(""), None);
    assert_eq!(parse_number("1e3"), Some(1000.0));
    assert_eq!(parse_number(".5"), Some(0.5));
    assert_eq!(parse_number("abc"), None);
    assert_eq!(parse_number("-").map(|v| v.is_sign_negative()), Some(true));
    assert_eq!(parse_number("0x"), None);
    assert_eq!(number_to_string(7.0), "7");
    assert_eq!(number_to_string(-0.0), "0");
    assert_eq!(number_to_string(0.1 + 0.2), "0.3");
    assert_eq!(number_to_string(1.0 / 3.0), "0.33333333333333");
    assert_eq!(number_to_string(2.5e-5), "2.5e-005");
    assert_eq!(number_to_string(1e20), "1e+020");
    assert_eq!(number_to_string(f64::NAN), "NaN");
    assert_eq!(number_to_string(f64::NEG_INFINITY), "-Infinity");
    assert_eq!(format_g(123456.789, 14), "123456.789");
    assert_eq!(number_to_radix(255.0, 16), "ff");
    assert_eq!(number_to_radix(-1.0, 16), "ffffffff");
    assert_eq!(number_to_radix(5.0, 2), "101");
    assert_eq!(to_int32(4294967297.0), 1);
    assert_eq!(to_int32(-4294967297.0), -1);
    assert_eq!(to_int32(f64::NAN), 0);
    assert_eq!(to_int32(-2.7), -2);
}

#[test]
fn arithmetic_and_operators() {
    let mut w = World::new();
    let code = Asm::new()
        .push(&[P::S("x"), P::I(1), P::I(2), P::I(3)])
        .op(0x0C)
        .op(0x47)
        .op(0x1D)
        .push(&[P::S("s"), P::S("5"), P::I(3)])
        .op(0x47)
        .op(0x1D)
        .push(&[P::S("d"), P::S("5"), P::I(3)])
        .op(0x0B)
        .op(0x1D)
        .push(&[P::S("f"), P::I(1), P::I(10)])
        .op(0x0D)
        .push(&[P::I(2), P::I(10)])
        .op(0x0D)
        .op(0x47)
        .op(0x4B)
        .op(0x1D)
        .push(&[P::S("m"), P::I(-7), P::I(3)])
        .op(0x3F)
        .op(0x1D)
        .push(&[P::S("bits"), P::I(-1), P::I(28)])
        .op(0x65)
        .op(0x1D)
        .push(&[P::S("u"), P::I(1), P::Undef])
        .op(0x48)
        .op(0x44)
        .op(0x1D)
        .push(&[P::S("nan"), P::I(0), P::I(0)])
        .op(0x0D)
        .op(0x4C)
        .op(0x49)
        .op(0x1D)
        .push(&[P::S("strict"), P::I(0), P::I(0)])
        .op(0x0D)
        .op(0x4C)
        .op(0x66)
        .op(0x1D)
        .push(&[P::S("eq"), P::S("1"), P::B(true)])
        .op(0x49)
        .op(0x1D)
        .push(&[P::S("nul"), P::Null, P::Undef])
        .op(0x49)
        .op(0x1D)
        .push(&[P::S("gt"), P::S("b"), P::S("a")])
        .op(0x67)
        .op(0x1D);
    w.run(code);
    assert_eq!(w.num("x"), 7.0);
    assert_eq!(w.text("s"), "53");
    assert_eq!(w.num("d"), 2.0);
    assert_eq!(w.text("f"), "0.3", "0.30000000000000004 through %.14g");
    assert_eq!(w.num("m"), -1.0);
    assert_eq!(w.num("bits"), 15.0);
    assert_eq!(w.text("u"), "undefined", "Less2 with undefined gives undefined (SWF 7+)");
    assert!(matches!(w.var("nan"), Value::Bool(true)), "Equals2: NaN == NaN for numbers (0x8277DDA8)");
    assert!(matches!(w.var("strict"), Value::Bool(false)));
    assert!(matches!(w.var("eq"), Value::Bool(true)));
    assert!(matches!(w.var("nul"), Value::Bool(true)));
    assert!(matches!(w.var("gt"), Value::Bool(true)));
    assert_eq!(w.errors(), 0);
}

#[test]
fn closures_capture_their_scope() {
    let inner = Asm::new()
        .push(&[P::S("n"), P::S("n")])
        .op(0x1C)
        .op(0x50)
        .op(0x4C)
        .store(0)
        .op(0x17)
        .op(0x1D)
        .push(&[P::R(0)])
        .op(0x3E);
    let outer = Asm::new().push(&[P::S("n"), P::I(0)]).op(0x3C).func("", &[], inner).op(0x3E);
    let code = Asm::new()
        .func("counter", &[], outer)
        .push(&[P::S("c"), P::I(0), P::S("counter")])
        .op(0x3D)
        .op(0x1D)
        .push(&[P::I(0), P::S("c")])
        .op(0x3D)
        .op(0x17)
        .push(&[P::I(0), P::S("c")])
        .op(0x3D)
        .op(0x17)
        .push(&[P::S("r"), P::I(0), P::S("c")])
        .op(0x3D)
        .op(0x1D)
        .push(&[P::S("leak"), P::S("n")])
        .op(0x1C)
        .op(0x44)
        .op(0x1D);
    let mut w = World::new();
    w.run(code);
    assert_eq!(w.num("r"), 3.0);
    assert_eq!(w.text("leak"), "undefined", "the local stays in the closure");
    assert_eq!(w.errors(), 0);
}

fn global(name: &str) -> Asm {
    Asm::new().push(&[P::S("_global")]).op(0x1C).push(&[P::S(name)]).op(0x4E)
}

fn append(mut a: Asm, b: Asm) -> Asm {
    assert!(b.fixups.is_empty() && b.labels.is_empty(), "appended code has no labels");
    a.b.extend_from_slice(&b.b);
    a
}

#[test]
fn classes_extends_and_super() {
    let mut c = Asm::new().push(&[P::S("_global")]).op(0x1C).push(&[P::S("A")]);
    c = c.func2("", &[], 2, 0x2900, Asm::new().push(&[P::R(1), P::S("v"), P::I(1)]).op(0x4F)).op(0x4F);
    c = append(c, global("A")).push(&[P::S("prototype")]).op(0x4E).push(&[P::S("who")]);
    c = c.func2("", &[], 1, 0x2A00, Asm::new().push(&[P::S("A")]).op(0x3E)).op(0x4F);
    c = c.push(&[P::S("_global")]).op(0x1C).push(&[P::S("B")]);
    let b_ctor =
        Asm::new().push(&[P::I(0), P::R(2), P::Undef]).op(0x52).op(0x17).push(&[P::R(1), P::S("w"), P::I(2)]).op(0x4F);
    c = c.func2("", &[], 3, 0x1900, b_ctor).op(0x4F);
    c = append(append(c, global("B")), global("A")).op(0x69);
    c = append(c, global("B")).push(&[P::S("prototype")]).op(0x4E).push(&[P::S("who")]);
    let who = Asm::new().push(&[P::S("B"), P::I(0), P::R(2), P::S("who")]).op(0x52).op(0x47).op(0x3E);
    c = c.func2("", &[], 3, 0x1900, who).op(0x4F);
    c = c.push(&[P::S("b"), P::I(0), P::S("B")]).op(0x40).op(0x1D);
    c = c.push(&[P::S("r1"), P::I(0), P::S("b")]).op(0x1C).push(&[P::S("who")]).op(0x52).op(0x1D);
    c = c.push(&[P::S("r2"), P::S("b")]).op(0x1C).push(&[P::S("v")]).op(0x4E);
    c = c.push(&[P::S("b")]).op(0x1C).push(&[P::S("w")]).op(0x4E).op(0x47).op(0x1D);
    c = c.push(&[P::S("r3"), P::S("b")]).op(0x1C).push(&[P::S("A")]).op(0x1C).op(0x54).op(0x1D);
    c = c.push(&[P::S("r4"), P::S("B")]).op(0x1C).op(0x44).op(0x1D);
    let mut w = World::new();
    w.run(c);
    assert_eq!(w.text("r1"), "BA");
    assert_eq!(w.num("r2"), 3.0);
    assert!(matches!(w.var("r3"), Value::Bool(true)));
    assert_eq!(w.text("r4"), "function");
    assert_eq!(w.errors(), 0);
}

#[test]
fn try_catch_finally() {
    let mut c = Asm::new().func("thrower", &[], Asm::new().push(&[P::S("boom")]).op(0x2A));
    let t = Asm::new().push(&[P::I(0), P::S("thrower")]).op(0x3D).op(0x17).push(&[P::S("log"), P::S("no")]).op(0x1D);
    let k = Asm::new().push(&[P::S("log"), P::S("caught "), P::S("e")]).op(0x1C).op(0x47).op(0x1D);
    let f = Asm::new().push(&[P::S("fin"), P::I(1)]).op(0x1D);
    c = c.try_catch("e", t, k, Some(f));
    let body = Asm::new().try_catch(
        "x",
        Asm::new().push(&[P::I(1)]).op(0x3E),
        Asm::new(),
        Some(Asm::new().push(&[P::S("g"), P::I(2)]).op(0x1D)),
    );
    c = c.func("f", &[], body).push(&[P::S("r"), P::I(0), P::S("f")]).op(0x3D).op(0x1D);
    let mut w = World::new();
    w.run(c);
    assert_eq!(w.text("log"), "caught boom");
    assert_eq!(w.num("fin"), 1.0);
    assert_eq!(w.num("r"), 1.0, "the return value survives the finally block");
    assert_eq!(w.num("g"), 2.0);
    assert_eq!(w.errors(), 0);
    let mut w = World::new();
    w.run(Asm::new().push(&[P::S("oops")]).op(0x2A).push(&[P::S("after"), P::I(1)]).op(0x1D));
    assert!(w.var("after").is_undefined());
    assert_eq!(w.vm.diag.counts.get(&("uncaught exception".into(), "oops".into())), Some(&1));
}

fn call(name: &str, obj: &str, method: &str, args: &[P]) -> Asm {
    let mut v: Vec<P> = vec![P::S(name)];
    v.extend(args.iter().rev());
    v.push(P::I(args.len() as i32));
    v.push(P::S(obj));
    Asm::new().push(&v).op(0x1C).push(&[P::S(method)]).op(0x52).op(0x1D)
}

#[test]
fn string_and_array_methods() {
    let mut c = Asm::new().push(&[P::S("s"), P::S("Hello, World")]).op(0x1D);
    c = append(c, call("a1", "s", "substr", &[P::I(7), P::I(5)]));
    c = append(c, call("a2", "s", "substring", &[P::I(7)]));
    c = append(c, call("a3", "s", "indexOf", &[P::S("o")]));
    c = append(c, call("a4", "s", "lastIndexOf", &[P::S("o")]));
    c = append(c, call("a5", "s", "toUpperCase", &[]));
    c = append(c, call("a6", "s", "charAt", &[P::I(1)]));
    c = append(c, call("parts", "s", "split", &[P::S(", ")]));
    c = c.push(&[P::S("a7"), P::S("s")]).op(0x1C).push(&[P::S("length")]).op(0x4E).op(0x1D);
    c = c.push(&[P::S("arr"), P::I(2), P::I(1), P::I(3), P::I(3)]).op(0x42).op(0x1D);
    c = append(c, call("x", "arr", "sort", &[]));
    c = append(c, call("j", "arr", "join", &[P::S("-")]));
    c = append(c, call("n", "arr", "push", &[P::I(4)]));
    c = append(c, call("sp", "arr", "splice", &[P::I(1), P::I(2)]));
    c = append(c, call("j2", "arr", "join", &[]));
    c = append(c, call("j3", "sp", "join", &[]));
    let mut w = World::new();
    w.run(c);
    assert_eq!(w.text("a1"), "World");
    assert_eq!(w.text("a2"), "World");
    assert_eq!(w.num("a3"), 4.0);
    assert_eq!(w.num("a4"), 8.0);
    assert_eq!(w.text("a5"), "HELLO, WORLD");
    assert_eq!(w.text("a6"), "e");
    assert_eq!(w.text("parts"), "Hello,World");
    assert_eq!(w.num("a7"), 12.0);
    assert_eq!(w.text("j"), "1-2-3");
    assert_eq!(w.num("n"), 4.0);
    assert_eq!(w.text("j3"), "2,3");
    assert_eq!(w.text("j2"), "1,4");
    assert_eq!(w.errors(), 0);
}

#[test]
fn accessors_flags_and_resolve() {
    let mut c = Asm::new().push(&[P::S("o"), P::I(0), P::S("Object")]).op(0x40).op(0x1D);
    c = c.push(&[P::Null]).func2("", &[], 1, 0x2A00, Asm::new().push(&[P::I(42)]).op(0x3E));
    c = c.push(&[P::S("p"), P::I(3), P::S("o")]).op(0x1C).push(&[P::S("addProperty")]).op(0x52).op(0x17);
    c = c.push(&[P::S("r"), P::S("o")]).op(0x1C).push(&[P::S("p")]).op(0x4E).op(0x1D);
    c = c.push(&[P::S("o")]).op(0x1C).push(&[P::S("q"), P::I(1)]).op(0x4F);
    c = c.push(&[P::S("o")]).op(0x1C).push(&[P::S("k"), P::I(2)]).op(0x4F);
    c = c.push(&[P::I(1), P::S("q"), P::S("o")]).op(0x1C).push(&[P::I(3), P::S("ASSetPropFlags")]).op(0x3D).op(0x17);
    c = c.push(&[P::S("o2"), P::I(0)]).op(0x43).op(0x1D);
    c = c.push(&[P::S("o2")]).op(0x1C).push(&[P::S("__resolve")]);
    c = c.func2("", &[(1, "n")], 2, 0x2A00, Asm::new().push(&[P::S("R:"), P::R(1)]).op(0x47).op(0x3E)).op(0x4F);
    c = c.push(&[P::S("r2"), P::S("o2")]).op(0x1C).push(&[P::S("zz")]).op(0x4E).op(0x1D);
    let mut w = World::new();
    w.run(c);
    assert_eq!(w.num("r"), 42.0);
    assert_eq!(w.text("r2"), "R:zz");
    let o = w.var("o");
    let names: Vec<String> = w.cx().enumerate(&o).iter().map(|s| s.to_string()).collect();
    assert!(names.contains(&"k".to_string()) && !names.contains(&"q".to_string()), "{names:?}");
    assert_eq!(w.errors(), 0);
}

#[test]
fn unload_movie_waits_for_the_load_queue() {
    let mut c = Asm::new().push(&[P::S("mc"), P::I(1), P::S("holder"), P::I(2), P::S("this")]).op(0x1C);
    c = c.push(&[P::S("createEmptyMovieClip")]).op(0x52).op(0x1D);
    c = c.push(&[P::S("kid"), P::I(1), P::S("kid"), P::I(2), P::S("mc")]).op(0x1C);
    c = c.push(&[P::S("createEmptyMovieClip")]).op(0x52).op(0x1D);
    c = c.push(&[P::I(0), P::S("mc")]).op(0x1C).push(&[P::S("unloadMovie")]).op(0x52).op(0x17);
    let mut w = World::new();
    w.run(c);
    let (mc, kid) = (w.var("mc"), w.var("kid"));
    assert_eq!(w.vm.loads.len(), 1, "one queue entry");
    assert!(w.vm.loads[0].url.is_empty(), "the unload's entry has an empty URL");
    assert!(w.cx().display_of(&kid).is_some(), "the clip's content stays until the queue is served");
    w.vm.step_frame(&mut w.lib, &mut w.player);
    assert!(w.vm.loads.is_empty());
    assert!(w.cx().display_of(&kid).is_none(), "served: the content is gone");
    assert!(w.cx().display_of(&mc).is_some(), "the clip itself keeps its place");
    assert_eq!(w.errors(), 0);
}

#[test]
fn registered_class_on_attached_clip() {
    let mut c = Asm::new().push(&[P::S("_global")]).op(0x1C).push(&[P::S("C")]);
    c = c.func2("", &[], 2, 0x2900, Asm::new().push(&[P::R(1), P::S("made"), P::I(1)]).op(0x4F)).op(0x4F);
    c = append(c, global("C")).push(&[P::S("MovieClip")]).op(0x1C).op(0x69);
    c = append(c, global("C")).push(&[P::S("prototype")]).op(0x4E).push(&[P::S("onLoad")]);
    let on_load = Asm::new().push(&[P::R(1), P::S("loaded"), P::R(1), P::S("_name")]).op(0x4E).op(0x4F);
    c = c.func2("", &[], 2, 0x2900, on_load).op(0x4F);
    c = append(c.push(&[P::S("Clip")]), Asm::new()).push(&[P::S("C")]).op(0x1C);
    c = c.op(0x4D).push(&[P::I(2), P::S("Object")]).op(0x1C).push(&[P::S("registerClass")]).op(0x52).op(0x17);
    c = c.push(&[P::S("mc"), P::I(5), P::S("kid"), P::S("Clip"), P::I(3), P::S("this")]).op(0x1C);
    c = c.push(&[P::S("attachMovie")]).op(0x52).op(0x1D);
    c = c.push(&[P::S("mc")]).op(0x1C).push(&[P::S("_x"), P::I(15)]).op(0x4F);
    c = c.push(&[P::S("t"), P::S("mc")]).op(0x1C).op(0x44).op(0x1D);
    c = c.push(&[P::S("depth"), P::I(0), P::S("mc")]).op(0x1C).push(&[P::S("getDepth")]).op(0x52).op(0x1D);
    let mut w = World::new();
    w.run(c);
    let mc = w.var("mc");
    let mut cx = w.cx();
    assert_eq!(cx.get_member(&mc, "made").unwrap().as_object(), None);
    assert!(matches!(cx.get_member(&mc, "made").unwrap(), Value::Number(n) if n == 1.0));
    let loaded = cx.get_member(&mc, "loaded").unwrap();
    assert_eq!(cx.to_string(&loaded).unwrap().as_ref(), "kid");
    let id = cx.display_of(&mc).expect("a clip");
    assert_eq!(cx.player.get(id).unwrap().matrix.tx, 300.0);
    assert_eq!(cx.player.path(id), "_level0.kid");
    assert_eq!(w.text("t"), "movieclip");
    assert_eq!(w.num("depth"), 5.0);
    assert_eq!(w.errors(), 0);
}

#[test]
fn branches_and_loops() {
    let c = Asm::new()
        .push(&[P::S("i"), P::I(0)])
        .op(0x1D)
        .push(&[P::S("s"), P::I(0)])
        .op(0x1D)
        .label("top")
        .push(&[P::S("i")])
        .op(0x1C)
        .push(&[P::I(5)])
        .op(0x48)
        .op(0x12)
        .branch(0x9D, "end")
        .push(&[P::S("s"), P::S("s")])
        .op(0x1C)
        .push(&[P::S("i")])
        .op(0x1C)
        .op(0x47)
        .op(0x1D)
        .push(&[P::S("i"), P::S("i")])
        .op(0x1C)
        .op(0x50)
        .op(0x1D)
        .branch(0x99, "top")
        .label("end");
    let mut w = World::new();
    w.run(c);
    assert_eq!(w.num("s"), 10.0);
    assert_eq!(w.num("i"), 5.0);
}

#[test]
fn compiler_package_idiom_pops_nothing() {
    let c = append(Asm::new(), global("ux"))
        .op(0x12)
        .op(0x12)
        .branch(0x9D, "skip")
        .push(&[P::S("_global")])
        .op(0x1C)
        .push(&[P::S("ux"), P::I(0), P::S("Object")])
        .op(0x40)
        .op(0x4F)
        .label("skip")
        .op(0x17);
    let mut w = World::new();
    w.run(c);
    assert_eq!(w.vm.diag.empty_pops, 1);
    assert_eq!(w.errors(), 0);
    let g = Value::Object(w.vm.global);
    let ux = w.cx().get_member(&g, "ux").unwrap();
    assert!(ux.as_object().is_some());
}

#[test]
fn garbage_collection_keeps_what_scripts_reach() {
    let mut c = Asm::new()
        .push(&[P::S("keep"), P::S("inner"), P::I(2), P::I(1), P::I(2)])
        .op(0x42)
        .push(&[P::I(1)])
        .op(0x43)
        .op(0x1D);
    for _ in 0..50 {
        c = c.push(&[P::S("tmp"), P::I(0), P::S("Object")]).op(0x40).op(0x1D);
    }
    let mut w = World::new();
    w.run(c);
    let before = w.vm.heap.live();
    let freed = w.cx().collect();
    assert!(freed >= 49, "the overwritten temporaries go ({freed} freed of {before})");
    let keep = w.var("keep");
    let inner = w.cx().get_member(&keep, "inner").unwrap();
    let mut cx = w.cx();
    let joined = super::builtins::array::join_values(&mut cx, &[inner], ",").unwrap();
    assert_eq!(joined, "1,2");
    assert!(cx.vm.heap.live() < before);
}

#[test]
fn filters_round_trip_through_the_instance() {
    let mut c = Asm::new().push(&[P::S("this")]).op(0x1C).push(&[P::S("filters")]);
    c = c.push(&[P::S("blurX"), P::I(2), P::S("blurY"), P::I(3), P::I(2)]).op(0x43);
    c = c
        .push(&[
            P::S("color"),
            P::I(0xFF0000),
            P::S("blurX"),
            P::I(5),
            P::S("blurY"),
            P::I(5),
            P::S("strength"),
            P::I(2),
            P::I(4),
        ])
        .op(0x43);
    c = c.op(0x4D).push(&[P::I(2)]).op(0x42).op(0x4F);
    let get = |name: &'static str, idx: i32, member: &'static str| {
        Asm::new()
            .push(&[P::S(name), P::S("this")])
            .op(0x1C)
            .push(&[P::S("filters")])
            .op(0x4E)
            .push(&[P::I(idx)])
            .op(0x4E)
            .push(&[P::S(member)])
            .op(0x4E)
            .op(0x1D)
    };
    c = append(append(c, get("b", 0, "blurY")), get("g", 1, "color"));
    c = c
        .push(&[P::S("n"), P::S("this")])
        .op(0x1C)
        .push(&[P::S("filters")])
        .op(0x4E)
        .push(&[P::S("length")])
        .op(0x4E)
        .op(0x1D);
    let mut w = World::new();
    w.run(c);
    assert_eq!(w.num("b"), 3.0);
    assert_eq!(w.num("g"), f64::from(0xFF0000));
    assert_eq!(w.num("n"), 2.0);
    let root = w.player.levels[&0];
    let f = w.player.get(root).unwrap().filter.expect("a text filter");
    assert_eq!(f.blur, (40.0, 60.0));
    assert_eq!(f.shadow.map(|s| s.offset), Some((0.0, 0.0)), "a colour without distance is a glow");
}

#[test]
fn remaining_opcodes() {
    let mut c =
        Asm::new().push(&[P::S("r1"), P::I(27), P::I(10)]).op(0x0D).push(&[P::I(-1)]).op(0x0C).op(0x18).op(0x1D);
    c = c.push(&[P::S("r2"), P::I(-8), P::I(1)]).op(0x64).op(0x1D);
    c = c.push(&[P::S("r3"), P::I(3), P::I(4)]).op(0x63).op(0x1D);
    c = c.push(&[P::S("r4"), P::I(6), P::I(3)]).op(0x62).op(0x1D);
    c = c.push(&[P::S("r5"), P::I(1), P::I(2)]).op(0x4D).op(0x0B).op(0x1D);
    c = c.push(&[P::S("r6")]).op(0x34).op(0x44).op(0x1D);
    c = c.push(&[P::S("r7"), P::S(""), P::I(13)]).op(0x22).op(0x1D);
    c = c.push(&[P::S("r9"), P::I(7), P::I(4)]).op(0x3F).op(0x1D);
    c = c.push(&[P::S("o"), P::S("a"), P::I(5), P::I(1)]).op(0x43).op(0x1D);
    c = c.push(&[P::S("o")]).op(0x1C).with_block(Asm::new().push(&[P::S("r8"), P::S("a")]).op(0x1C).op(0x1D));
    let mut w = World::new();
    w.run(c);
    assert_eq!(w.num("r1"), -2.0, "ToInteger truncates");
    assert_eq!(w.num("r2"), -4.0);
    assert_eq!(w.num("r3"), 48.0);
    assert_eq!(w.num("r4"), 5.0);
    assert_eq!(w.num("r5"), 1.0, "StackSwap");
    assert_eq!(w.text("r6"), "number");
    assert_eq!(w.text("r7"), "_level0");
    assert_eq!(w.num("r9"), 3.0);
    assert_eq!(w.num("r8"), 5.0, "with scope");
    assert!(w.var("a").is_undefined(), "the with object's member stays there");
    assert_eq!(w.errors(), 0);
}

#[test]
fn geometry_setters_keep_the_record_scaleform_keeps() {
    let mut c = Asm::new().push(&[P::S("mc"), P::I(1), P::S("geo"), P::I(2), P::S("this")]).op(0x1C);
    c = c.push(&[P::S("createEmptyMovieClip")]).op(0x52).op(0x1D);
    let mut w = World::new();
    w.run(c);
    let mc = w.var("mc");
    let mut cx = w.cx();
    let id = cx.display_of(&mc).expect("the clip");
    let set = |cx: &mut Ctx, p: &str, v: f64| assert!(cx.display_set(id, p, &Value::Number(v)).unwrap());
    let get = |cx: &mut Ctx, p: &str| match cx.display_get(id, p).unwrap() {
        Some(Value::Number(n)) => n,
        v => panic!("{p} is {v:?}"),
    };
    set(&mut cx, "_yscale", -100.0);
    assert_eq!(cx.player.get(id).unwrap().matrix.d, -1.0);
    assert_eq!(get(&mut cx, "_yscale"), -100.0);
    set(&mut cx, "_yscale", 236.0);
    assert!((cx.player.get(id).unwrap().matrix.d - 2.36).abs() < 1e-5, "the axis is not flipped back");
    assert_eq!(get(&mut cx, "_rotation"), 0.0);
    set(&mut cx, "_rotation", 270.0);
    assert_eq!(get(&mut cx, "_rotation"), -90.0);
    let m = cx.player.get(id).unwrap().matrix;
    assert!(m.a.abs() < 1e-5 && (m.b + 1.0).abs() < 1e-5 && (m.c - 2.36).abs() < 1e-5 && m.d.abs() < 1e-5);
    set(&mut cx, "_x", 12.345);
    assert!((get(&mut cx, "_x") - 12.3).abs() < 1e-9);
    assert_eq!(cx.player.get(id).unwrap().matrix.tx, 246.0);
    set(&mut cx, "_xscale", f64::NAN);
    set(&mut cx, "_xscale", f64::INFINITY);
    assert_eq!(get(&mut cx, "_xscale"), 100.0);
    set(&mut cx, "_height", -5.0);
    assert_eq!(get(&mut cx, "_yscale"), 0.0);
    assert_eq!(w.errors(), 0);
}

#[test]
fn a_peek_reads_data_only() {
    use crate::display::hit::tests::{at, hit_movie};
    fn marks(cx: &mut Ctx, _: Value, _: &[Value]) -> Result<Value, super::Fault> {
        let global = Value::Object(cx.vm.global);
        cx.set_member(&global, "ran", Value::Bool(true))?;
        Ok(Value::Number(1.0))
    }
    let mut lib = empty_lib();
    let m = hit_movie(&mut lib, vec![at(1, 20, "a", 0, 0, 1.0), at(2, 20, "b", 10, 0, 1.0)]);
    let (mut vm, mut player) = (Vm::new(), Player::new());
    vm.load_level(&mut lib, &mut player, m, 0);
    {
        let mut cx = vm.ctx(&mut lib, &mut player);
        let root = cx.player.levels[&0];
        cx.object_for(root);
        let a = cx.player.child_by_name(root, "a", true).unwrap();
        let a = Value::Object(cx.object_for(a));
        let data = Value::Object(cx.new_object());
        cx.set_member(&data, "path", Value::str("views/x.swf")).unwrap();
        cx.set_member(&a, "data", data.clone()).unwrap();
        let marks = Value::Object(cx.new_native(marks));
        let (add, home) = cx.get_method(&data, "addProperty").unwrap();
        cx.call(&add, data.clone(), &[Value::str("lazy"), marks.clone(), Value::Null], home).unwrap();
        cx.set_member(&data, "__resolve", marks).unwrap();
    }
    let objects = vm.display_objs.len();
    let mut peek = |path: &str| vm.peek_path(&mut lib, &mut player, path);
    assert!(matches!(peek("_level0.a.data.path"), Some(Value::String(s)) if &*s == "views/x.swf"));
    assert!(matches!(peek("a.data.path"), Some(Value::String(s)) if &*s == "views/x.swf"));
    assert!(peek("_level0.a.data.toString").is_some(), "the prototype chain's data");
    for path in ["_level0.b", "_level0.a._x", "_level0.a.data.lazy", "_level0.a.data.missing", "_level0.c.data"] {
        assert!(peek(path).is_none(), "{path}");
    }
    assert_eq!(vm.display_objs.len(), objects, "no object made for b");
    assert!(vm.get_path(&mut lib, &mut player, "_global.ran").is_none(), "no getter and no __resolve ran");
}
