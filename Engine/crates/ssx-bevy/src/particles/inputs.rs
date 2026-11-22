use bevy::prelude::*;
use ssx_physics::graph::LandingRumble;
use ssx_physics::surface::{ICE, POWDER, ROCK, SNOW_COVERAGE};

use crate::particles::spray::SprayInputs;
use crate::ride::Ride;

#[derive(Default)]
pub struct Trackers {
    rumble: LandingRumble,
    landing_impact: f32,
}

impl Trackers {
    pub fn step(&mut self, ride: &Ride) {
        self.landing_impact = self.rumble.step(&ride.state);
    }

    pub fn inputs(&self, ride: &Ride) -> SprayInputs {
        let s = &ride.state;
        let w = &s.weights;
        SprayInputs {
            is_awesome: s.score.is_awesome,
            velocity: s.traj_velocity,
            spray_velocity: s.spray.velocity,
            ice_spray: s.spray.ice,
            snow_spray: s.spray.carving,
            landing_impact: self.landing_impact,
            ground_contact: s.contact.is_some(),
            in_air: s.bln_in_air(),
            depth_below_snow: s.depth_below_snow,
            grinding: s.grind.is_grinding(),
            powder: w[POWDER],
            packed: [w[0], w[1], w[2], w[3], w[4]],
            ice: w[ICE],
            rock: w[ROCK],
            backwards_rewind: ride.rewind_backwards,
            ground_normal: s.ground_normal,
            snow_coverage: w[SNOW_COVERAGE],
            ao: ride.session.controllers.as_ref().map_or(1.0, |c| c.fx.world_ao),
            is_super_awesome: s.score.is_super_awesome,
            track_started: s.track_started,
            wipeout: s.landing.tumbling,
            ice_axe_active: s.ice_axe.frame.active,
            ice_axe_blend: s.ice_axe.blend_weight,
            lean: s.kin.lean_norm,
            downwash_visual_strength: 0.0,
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum Fx {
    Float(f32),
    UInt(u32),
    Int(i32),
    Bool(bool),
    Var(VarValue),
}

#[derive(Clone, Copy, Debug)]
enum VarValue {
    Bool(bool),
    Int(i32),
    Float(f32),
}

impl Fx {
    fn float(self) -> f32 {
        match self {
            Fx::Float(x) => x,
            Fx::UInt(x) => x as f32,
            Fx::Int(x) => x as f32,
            Fx::Bool(b) | Fx::Var(VarValue::Bool(b)) => b as u8 as f32,
            Fx::Var(VarValue::Int(x)) => x as f32,
            Fx::Var(VarValue::Float(x)) => x,
        }
    }
    fn int(self) -> i64 {
        match self {
            Fx::Float(x) | Fx::Var(VarValue::Float(x)) => x as i64,
            Fx::UInt(x) => x as i64,
            Fx::Int(x) | Fx::Var(VarValue::Int(x)) => x as i64,
            Fx::Bool(b) | Fx::Var(VarValue::Bool(b)) => b as i64,
        }
    }
}

const FX_OPS: [(&str, u8); 12] = [
    ("&&", 4),
    ("||", 4),
    ("==", 3),
    ("!=", 3),
    ("<=", 2),
    (">=", 2),
    ("<", 2),
    (">", 2),
    ("+", 1),
    ("-", 1),
    ("*", 0),
    ("/", 0),
];

#[derive(Clone, Debug)]
enum Node {
    Lit(Fx),
    Var(VarRef),
    Not(Box<Node>),
    Neg(Box<Node>),
    Bin(&'static str, Box<Node>, Box<Node>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum VarRef {
    Script(usize),
    AgeMs,
}

fn fx_compile(text: &str, leaf: &dyn Fn(&str) -> Option<VarRef>) -> Node {
    let s: String = text.chars().filter(|c| !c.is_whitespace()).collect();
    fx_build(&s, leaf)
}

fn fx_build(s: &str, leaf: &dyn Fn(&str) -> Option<VarRef>) -> Node {
    let b = s.as_bytes();
    let (mut depth, mut skip, mut best): (i32, bool, Option<(u8, usize, &'static str)>) = (0, true, None);
    for i in 0..b.len() {
        match b[i] {
            b'(' => depth += 1,
            b')' => depth -= 1,
            _ => {}
        }
        if skip {
            skip = false;
            continue;
        }
        if depth != 0 {
            continue;
        }
        if let Some(&(op, prio)) = FX_OPS.iter().find(|(op, _)| s[i..].starts_with(op)) {
            if best.is_none_or(|(p, _, _)| prio > p) {
                best = Some((prio, i, op));
            }
            skip = true;
        }
    }
    if let Some((_, i, op)) = best {
        let (l, r) = (fx_build(&s[..i], leaf), fx_build(&s[i + op.len()..], leaf));
        return Node::Bin(op, Box::new(l), Box::new(r));
    }
    if let Some(rest) = s.strip_prefix('!') {
        return Node::Not(Box::new(fx_build(rest, leaf)));
    }
    if let Some(rest) = s.strip_prefix('-') {
        return Node::Neg(Box::new(fx_build(rest, leaf)));
    }
    if let Some(inner) = s.strip_prefix('(').and_then(|r| r.strip_suffix(')')) {
        return fx_build(inner, leaf);
    }
    match s {
        "true" => Node::Lit(Fx::Bool(true)),
        "false" => Node::Lit(Fx::Bool(false)),
        _ if s.contains('.') => Node::Lit(Fx::Float(s.parse().unwrap_or(0.0))),
        _ => match leaf(s) {
            Some(v) => Node::Var(v),
            None => match s.strip_prefix("0x") {
                Some(h) => Node::Lit(Fx::UInt(u32::from_str_radix(h, 16).unwrap_or(0))),
                None => Node::Lit(Fx::Int(s.parse().unwrap_or(0))),
            },
        },
    }
}

fn fx_run(n: &Node, var: &dyn Fn(VarRef) -> VarValue) -> Fx {
    match n {
        Node::Lit(x) => *x,
        Node::Var(v) => Fx::Var(var(*v)),
        Node::Not(x) => Fx::Int((fx_run(x, var).int() == 0) as i32),
        Node::Neg(x) => match fx_run(x, var) {
            Fx::Int(x) => Fx::Int(-x),
            Fx::Float(x) => Fx::Float(-x),
            other => other,
        },
        Node::Bin(op, l, r) => {
            let l = fx_run(l, var);
            fx_binary(op, l, fx_run(r, var))
        }
    }
}

fn fx_binary(op: &str, l: Fx, r: Fx) -> Fx {
    match op {
        "&&" => return Fx::Bool(l.int() != 0 && r.int() != 0),
        "||" => return Fx::Bool(l.int() != 0 || r.int() != 0),
        _ => {}
    }
    enum Mode {
        Float,
        UInt,
        Int,
        Zero,
    }
    let is_var = |x: &Fx| matches!(x, Fx::Var(_));
    let int_like = |x: &Fx| matches!(x, Fx::Int(_) | Fx::UInt(_) | Fx::Var(_));
    let mode = if matches!(l, Fx::Float(_)) || matches!(r, Fx::Float(_)) {
        Mode::Float
    } else if matches!((l, r), (Fx::UInt(_), Fx::UInt(_))) {
        Mode::UInt
    } else if (matches!(l, Fx::Int(_)) && int_like(&r)) || (matches!(r, Fx::Int(_)) && int_like(&l)) {
        Mode::Int
    } else if is_var(&l) || is_var(&r) {
        Mode::Float
    } else {
        Mode::Zero
    };
    let cmp = |o: std::cmp::Ordering| match op {
        "==" => o.is_eq(),
        "!=" => o.is_ne(),
        "<" => o.is_lt(),
        ">" => o.is_gt(),
        "<=" => o.is_le(),
        _ => o.is_ge(),
    };
    let arith = matches!(op, "+" | "-" | "*" | "/");
    match mode {
        Mode::Float => {
            let (a, b) = (l.float(), r.float());
            match op {
                "+" => Fx::Float(a + b),
                "-" => Fx::Float(a - b),
                "*" => Fx::Float(a * b),
                "/" => Fx::Float(a / b),
                _ => Fx::Bool(a.partial_cmp(&b).is_some_and(cmp)),
            }
        }
        Mode::UInt | Mode::Int => {
            let (a, b) = (l.int(), r.int());
            if !arith {
                return Fx::Bool(cmp(a.cmp(&b)));
            }
            let v = match op {
                "+" => a.wrapping_add(b),
                "-" => a.wrapping_sub(b),
                "*" => a.wrapping_mul(b),
                _ => a.checked_div(b).unwrap_or(0),
            };
            if matches!(mode, Mode::UInt) { Fx::UInt(v as u32) } else { Fx::Int(v as i32) }
        }
        Mode::Zero => {
            if arith {
                Fx::Int(0)
            } else {
                Fx::Bool(false)
            }
        }
    }
}

// PORT(choice): a behaviour's expressions are split into trees and its variables' types and default texts read when
// the behaviour is loaded; an update walks the trees with the variables' values. The text was split, its numbers and
// its variables' defaults parsed and its names looked up at every evaluation: the switches and records of the bots',
// the rider's, the rider's spray's and the track's effects took 0.27 ms of the main thread a ride step on Mount Eddie
// (0.14, 0.06, 0.04, 0.03; a traced build with timers, round perf-x1). A tree gives the operations the split text
// gave, on the same values in the same order (`compiled_expressions_evaluate_as_the_text`). The console evaluates
// through FX::Expression (0x829D1180, its split 0x829CFC18); whether it keeps a split form is not decoded.
pub struct Script {
    variables: Vec<ScriptVar>,
    triggers: Vec<(i32, Vec<Node>)>,
    blend: Option<Vec<Axis>>,
}

struct Axis {
    weight: Node,
    from: [f32; 2],
    to: [f32; 2],
}

impl Script {
    pub fn new(b: &ssx_formats::lynx::Behaviour) -> Script {
        let script_var = |name: &str| b.variables.iter().position(|v| v.name == name).map(VarRef::Script);
        let trigger_var = |name: &str| if name == "EFFECT_AGE_MS" { Some(VarRef::AgeMs) } else { script_var(name) };
        let triggers = b
            .triggers
            .iter()
            .filter_map(|t| {
                let kind = match t.kind.as_str() {
                    "TURN_ON" => 0,
                    "TURN_OFF" => 1,
                    "TURN_OFF_INSTANTLY" => 2,
                    _ => return None,
                };
                Some((kind, t.conditions.iter().map(|c| fx_compile(&c.text, &trigger_var)).collect()))
            })
            .collect();
        let blend = b.blend_states.first().map(|state| {
            state
                .axes
                .iter()
                .map(|a| Axis { weight: fx_compile(&a.weight.text, &script_var), from: a.from, to: a.to })
                .collect()
        });
        Script { variables: b.variables.iter().map(ScriptVar::new).collect(), triggers, blend }
    }

    fn var(&self, v: VarRef, inputs: &SprayInputs, age_ms: i32) -> VarValue {
        match v {
            VarRef::Script(i) => self.variables[i].value(inputs),
            VarRef::AgeMs => VarValue::Int(age_ms),
        }
    }
}

struct ScriptVar {
    source: Option<Source>,
    ty: VarType,
    default: VarValue,
}

enum VarType {
    Bool,
    Int,
    Float,
}

#[derive(Clone, Copy)]
enum Source {
    SnowSprayVelocity,
    Velocity,
    InAir,
    IsAwesome,
    IsSuperAwesome,
    IsTrackStarted,
    StateWipeout,
    IceAxeEffectsActive,
    IceAxeBlendWeight,
    Lean,
    SurfaceWeightIce,
    SurfaceWeightPowder,
    SurfaceWeightRock,
    DownwashVisualStrength,
    Other,
}

impl ScriptVar {
    fn new(v: &ssx_formats::lynx::Variable) -> ScriptVar {
        let source = v.source.as_deref().and_then(|s| s.strip_prefix("gamestate:")).map(|name| match name {
            "VEC_SnowSprayVelocity" => Source::SnowSprayVelocity,
            "VEC_Velocity" => Source::Velocity,
            "BLN_InAir" => Source::InAir,
            "BLN_IsAwesome" => Source::IsAwesome,
            "BLN_IsSuperAwesome" => Source::IsSuperAwesome,
            "BLN_IsTrackStarted" => Source::IsTrackStarted,
            "BLN_State_Wipeout" => Source::StateWipeout,
            "BLN_IceAxeEffectsActive" => Source::IceAxeEffectsActive,
            "FLT_IceAxeBlendWeight" => Source::IceAxeBlendWeight,
            "FLT_Lean" => Source::Lean,
            "FLT_SurfaceWeightIce" => Source::SurfaceWeightIce,
            "FLT_SurfaceWeightPowder" => Source::SurfaceWeightPowder,
            "FLT_SurfaceWeightRock" => Source::SurfaceWeightRock,
            "FLT_DownwashVisualStrength" => Source::DownwashVisualStrength,
            _ => Source::Other,
        });
        let default = v.default.as_deref().unwrap_or("");
        let (ty, default) = match v.ty.to_ascii_uppercase().as_str() {
            "BOOL" => (VarType::Bool, VarValue::Bool(default.eq_ignore_ascii_case("true") || default == "1")),
            "INT" => (VarType::Int, VarValue::Int(default.trim().parse().unwrap_or(0))),
            _ => (VarType::Float, VarValue::Float(default.trim().parse().unwrap_or(0.0))),
        };
        ScriptVar { source, ty, default }
    }

    fn value(&self, inputs: &SprayInputs) -> VarValue {
        let Some(source) = self.source else { return self.default };
        let x = match source {
            Source::SnowSprayVelocity => inputs.spray_velocity.length(),
            Source::Velocity => inputs.velocity.length(),
            Source::InAir => inputs.in_air as u8 as f32,
            Source::IsAwesome => inputs.is_awesome as u8 as f32,
            Source::IsSuperAwesome => inputs.is_super_awesome as u8 as f32,
            Source::IsTrackStarted => inputs.track_started as u8 as f32,
            Source::StateWipeout => inputs.wipeout as u8 as f32,
            Source::IceAxeEffectsActive => inputs.ice_axe_active as u8 as f32,
            Source::IceAxeBlendWeight => inputs.ice_axe_blend,
            Source::Lean => inputs.lean,
            Source::SurfaceWeightIce => inputs.ice,
            Source::SurfaceWeightPowder => inputs.powder,
            Source::SurfaceWeightRock => inputs.rock,
            Source::DownwashVisualStrength => inputs.downwash_visual_strength,
            Source::Other => 0.0,
        };
        match self.ty {
            VarType::Bool => VarValue::Bool(x != 0.0),
            VarType::Int => VarValue::Int(x as i32),
            VarType::Float => VarValue::Float(x),
        }
    }
}

pub fn effect_age_ms(clock: f32, start: f32) -> i32 {
    if start == -1.0 { -1 } else { ((clock - start) * 1000.0) as i32 }
}

pub fn trigger_type(b: &Script, inputs: &SprayInputs, age_ms: i32) -> i32 {
    let var = |v: VarRef| b.var(v, inputs, age_ms);
    let mut best = -1;
    for (kind, conditions) in &b.triggers {
        if *kind > best && conditions.iter().any(|c| fx_run(c, &var).float() != 0.0) {
            best = *kind;
        }
    }
    best
}

pub fn apply_triggers(b: &Script, inputs: &SprayInputs, clock: f32, slot: (&mut f32, &mut f32, &mut bool)) {
    let (start, stop, instant) = slot;
    match (trigger_type(b, inputs, effect_age_ms(clock, *start)), *start != -1.0) {
        (1, true) => (*start, *stop) = (-1.0, clock),
        (2, true) => (*stop, *instant) = (clock, true),
        (0, false) => (*start, *stop, *instant) = (clock, -1.0, false),
        _ => {}
    }
}

pub fn parse_precipitation(v: &str) -> Option<f32> {
    v.parse::<f32>().ok().map(|x| x.clamp(0.0, 1.0))
}

pub fn default_blend(behaviour: Option<&Script>, inputs: &SprayInputs, precipitation: f32) -> Vec<(u8, f32)> {
    let Some(b) = behaviour else {
        return vec![(1, 1.0 - precipitation), (2, precipitation)];
    };
    let Some(axes) = &b.blend else { return Vec::new() };
    let var = |v: VarRef| b.var(v, inputs, 0);
    let axes: Vec<f32> = axes
        .iter()
        .map(|a| {
            let x = fx_run(&a.weight, &var).float();
            let t = ((x - a.from[0]) / (a.from[1] - a.from[0])).clamp(0.0, 1.0);
            a.to[0] + (a.to[1] - a.to[0]) * t
        })
        .collect();
    match axes[..] {
        [s] => {
            let i = s.trunc();
            let f = s - i;
            vec![(i as u8, 1.0 - f), (i as u8 + 1, f)]
        }
        [s0, s1] => vec![(1, (1.0 - s0) * (1.0 - s1)), (2, s0 * (1.0 - s1)), (3, (1.0 - s0) * s1), (4, s0 * s1)],
        _ => Vec::new(),
    }
}

pub fn player_shadow(q: Option<&crate::particles::shadow_query::PlayerShadowQuery>) -> f32 {
    q.map_or(1.0, |q| q.value)
}

#[cfg(test)]
mod tests {
    use super::*;

    const NAMES: [&str; 5] = ["V", "T", "M", "B", "I"];

    fn eval(s: &str) -> Fx {
        let values = [VarValue::Float(0.35), VarValue::Float(0.1), VarValue::Float(3.5), VarValue::Bool(true)];
        let leaf = |n: &str| NAMES[..4].iter().position(|x| *x == n).map(VarRef::Script);
        let var = |v: VarRef| match v {
            VarRef::Script(i) => values[i],
            VarRef::AgeMs => unreachable!("no age in these expressions"),
        };
        fx_run(&fx_compile(s, &leaf), &var)
    }

    fn text_eval(text: &str, var: &dyn Fn(&str) -> Option<VarValue>) -> Fx {
        let s: String = text.chars().filter(|c| !c.is_whitespace()).collect();
        text_parse(&s, var)
    }

    fn text_parse(s: &str, var: &dyn Fn(&str) -> Option<VarValue>) -> Fx {
        let b = s.as_bytes();
        let (mut depth, mut skip, mut best): (i32, bool, Option<(u8, usize, &str)>) = (0, true, None);
        for i in 0..b.len() {
            match b[i] {
                b'(' => depth += 1,
                b')' => depth -= 1,
                _ => {}
            }
            if skip {
                skip = false;
                continue;
            }
            if depth != 0 {
                continue;
            }
            if let Some(&(op, prio)) = FX_OPS.iter().find(|(op, _)| s[i..].starts_with(op)) {
                if best.is_none_or(|(p, _, _)| prio > p) {
                    best = Some((prio, i, op));
                }
                skip = true;
            }
        }
        if let Some((_, i, op)) = best {
            let (l, r) = (text_parse(&s[..i], var), text_parse(&s[i + op.len()..], var));
            return fx_binary(op, l, r);
        }
        if let Some(rest) = s.strip_prefix('!') {
            return Fx::Int((text_parse(rest, var).int() == 0) as i32);
        }
        if let Some(rest) = s.strip_prefix('-') {
            return match text_parse(rest, var) {
                Fx::Int(x) => Fx::Int(-x),
                Fx::Float(x) => Fx::Float(-x),
                other => other,
            };
        }
        if let Some(inner) = s.strip_prefix('(').and_then(|r| r.strip_suffix(')')) {
            return text_parse(inner, var);
        }
        match s {
            "true" => Fx::Bool(true),
            "false" => Fx::Bool(false),
            _ if s.contains('.') => Fx::Float(s.parse().unwrap_or(0.0)),
            _ => match var(s) {
                Some(v) => Fx::Var(v),
                None => match s.strip_prefix("0x") {
                    Some(h) => Fx::UInt(u32::from_str_radix(h, 16).unwrap_or(0)),
                    None => Fx::Int(s.parse().unwrap_or(0)),
                },
            },
        }
    }

    fn bits(x: Fx) -> (u8, u64) {
        match x {
            Fx::Float(v) | Fx::Var(VarValue::Float(v)) => {
                (if matches!(x, Fx::Var(_)) { 5 } else { 0 }, v.to_bits().into())
            }
            Fx::UInt(v) => (1, v.into()),
            Fx::Int(v) => (2, v as u32 as u64),
            Fx::Bool(v) => (3, v as u64),
            Fx::Var(VarValue::Int(v)) => (6, v as u32 as u64),
            Fx::Var(VarValue::Bool(v)) => (7, v as u64),
        }
    }

    #[test]
    fn compiled_expressions_evaluate_as_the_text() {
        let exprs = [
            "(V - T) * M",
            "10-4-3",
            "V*4",
            "V*4.0",
            "B && !false",
            "V > 0.5",
            "-V",
            "-(3)",
            "!V",
            "!!B",
            "- -I",
            "0x10 + 0x2",
            "0x10 - 0x20",
            "0xFFFFFFFF + 1",
            "7/0",
            "7/2",
            "7.0/2",
            "V/0",
            "0.0/0.0 == 0.0",
            "V == T",
            "V != T",
            "V <= 0.35",
            "V >= M",
            "B || false",
            "B == 1",
            "I + 2",
            "I * I",
            "I / 3",
            "I / 0",
            "-I",
            "I < 4 && V > 0.1",
            "(V+T)*(M-1.5)/2.0",
            "V - T - M",
            "V / T / M",
            "1 - - 2",
            "true + 1",
            "false * 3",
            "T*-1",
            "(((V)))",
            "U == 0",
            "X.Y + 1",
            "",
            "-",
            "()",
            "2147483647 + 1",
            "3 < 0x4",
            "B + B",
            "V >= 0.35 || I == 5",
            "!(I > 2) && (V*M < 1.5 || B)",
            "I - 1 < 2 == 1",
            "1.5e3",
            "0x",
            "0xZZ",
            "I*2.5",
            "M*-I",
            "( V + T",
            "V + T )",
            "B - I",
            "I == I && I != 0 || V",
        ];
        let sets = [
            [VarValue::Float(0.35), VarValue::Float(0.1), VarValue::Float(3.5), VarValue::Bool(true), VarValue::Int(5)],
            [
                VarValue::Float(-2.0),
                VarValue::Float(0.0),
                VarValue::Float(1e-8),
                VarValue::Bool(false),
                VarValue::Int(-7),
            ],
            [
                VarValue::Float(f32::NAN),
                VarValue::Float(-0.0),
                VarValue::Float(1e30),
                VarValue::Bool(true),
                VarValue::Int(0),
            ],
            [
                VarValue::Float(f32::INFINITY),
                VarValue::Float(0.1),
                VarValue::Float(-3.5),
                VarValue::Bool(false),
                VarValue::Int(i32::MAX),
            ],
        ];
        let leaf = |n: &str| NAMES.iter().position(|x| *x == n).map(VarRef::Script);
        for e in exprs {
            let tree = fx_compile(e, &leaf);
            for values in &sets {
                let by_name = |n: &str| NAMES.iter().position(|x| *x == n).map(|i| values[i]);
                let by_ref = |v: VarRef| match v {
                    VarRef::Script(i) => values[i],
                    VarRef::AgeMs => unreachable!("no age in these expressions"),
                };
                assert_eq!(bits(fx_run(&tree, &by_ref)), bits(text_eval(e, &by_name)), "{e} with {values:?}");
            }
        }
    }

    #[test]
    fn behaviour_expressions_follow_the_game() {
        assert!((eval("(V - T) * M").float() - 0.875).abs() < 1e-6);
        assert_eq!(eval("10-4-3").int(), 9);
        assert_eq!(eval("V*4").int(), 0);
        assert!((eval("V*4.0").float() - 1.4).abs() < 1e-6);
        assert!(matches!(eval("B && !false"), Fx::Bool(true)));
        assert!(matches!(eval("V > 0.5"), Fx::Bool(false)));
    }

    fn behaviour() -> ssx_formats::lynx::Behaviour {
        let xml = r#"<particleBehaviour name="t">
            <variable name="AIR" type="BOOL" source="gamestate:BLN_InAir"/>
            <trigger type="TURN_ON"><condition expression="AIR"/></trigger>
            <trigger type="TURN_OFF"><condition expression="!AIR"/></trigger>
            <trigger type="TURN_OFF_INSTANTLY"><condition expression="EFFECT_AGE_MS &gt; 500"/></trigger>
        </particleBehaviour>"#;
        ssx_formats::lynx::Behaviour::parse(xml.as_bytes()).unwrap()
    }

    #[test]
    fn triggers_switch_a_slot_as_the_game() {
        let b = Script::new(&behaviour());
        let mut inputs = SprayInputs { in_air: true, ..Default::default() };
        let (mut start, mut stop, mut instant) = (-1.0, -1.0, false);
        apply_triggers(&b, &inputs, 1.0, (&mut start, &mut stop, &mut instant));
        assert_eq!((start, stop, instant), (1.0, -1.0, false));
        apply_triggers(&b, &inputs, 1.2, (&mut start, &mut stop, &mut instant));
        assert_eq!((start, stop), (1.0, -1.0));
        apply_triggers(&b, &inputs, 1.6, (&mut start, &mut stop, &mut instant));
        assert_eq!((start, stop, instant), (1.0, 1.6, true));
        let (mut start, mut stop, mut instant) = (2.0, -1.0, false);
        inputs.in_air = false;
        apply_triggers(&b, &inputs, 2.1, (&mut start, &mut stop, &mut instant));
        assert_eq!((start, stop, instant), (-1.0, 2.1, false));
        assert_eq!(effect_age_ms(2.1, -1.0), -1);
    }
}
