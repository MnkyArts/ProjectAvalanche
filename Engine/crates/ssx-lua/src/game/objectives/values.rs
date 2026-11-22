use super::*;

#[derive(Clone, Debug)]
pub struct Name {
    pub hash: u32,
    pub text: Arc<str>,
}

impl Name {
    pub fn new(text: &str) -> Self {
        Self { hash: fnv1(text.as_bytes()), text: Arc::from(text) }
    }

    pub fn is(&self, text: &str) -> bool {
        self.hash == fnv1(text.as_bytes())
    }
}

impl PartialEq for Name {
    fn eq(&self, o: &Self) -> bool {
        self.hash == o.hash
    }
}

impl std::fmt::Display for Name {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.text)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GsType {
    Bool,
    Float,
    Int,
    Quat,
    Vector,
}

pub fn game_state_type(id: i32) -> Option<GsType> {
    let name = crate::game::bindings::GAME_STATE.get(usize::try_from(id).ok()?)?;
    Some(match name.split('_').next()? {
        "BLN" => GsType::Bool,
        "FLT" => GsType::Float,
        "INT" => GsType::Int,
        "QUAT" => GsType::Quat,
        "VEC" => GsType::Vector,
        _ => return None,
    })
}

pub fn game_state_name(id: u16) -> &'static str {
    crate::game::bindings::GAME_STATE.get(usize::from(id)).copied().unwrap_or("?")
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum GsValue {
    Bool(bool),
    Float(f32),
    Int(i32),
    Vector([f32; 3]),
    Quat([f32; 4]),
}

impl GsValue {
    pub fn as_f32(self) -> f32 {
        match self {
            GsValue::Bool(b) => f32::from(u8::from(b)),
            GsValue::Float(f) => f,
            GsValue::Int(i) => i as f32,
            GsValue::Vector(v) => v[0],
            GsValue::Quat(q) => q[0],
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cmp {
    LessThan,
    LessThanOrEqualTo,
    EqualTo,
    NotEqualTo,
    GreaterThanOrEqualTo,
    GreaterThan,
}

impl Cmp {
    pub fn from_i32(v: i32) -> Option<Self> {
        use Cmp::*;
        [LessThan, LessThanOrEqualTo, EqualTo, NotEqualTo, GreaterThanOrEqualTo, GreaterThan]
            .get(usize::try_from(v).ok()?)
            .copied()
    }

    pub fn f32(self, a: f32, b: f32) -> bool {
        match self {
            Cmp::LessThan => a < b,
            Cmp::LessThanOrEqualTo => a.partial_cmp(&b) != Some(std::cmp::Ordering::Greater),
            Cmp::EqualTo => a == b,
            Cmp::NotEqualTo => a != b,
            Cmp::GreaterThanOrEqualTo => a.partial_cmp(&b) != Some(std::cmp::Ordering::Less),
            Cmp::GreaterThan => a > b,
        }
    }

    pub fn i32(self, a: i32, b: i32) -> bool {
        match self {
            Cmp::LessThan => a < b,
            Cmp::LessThanOrEqualTo => a <= b,
            Cmp::EqualTo => a == b,
            Cmp::NotEqualTo => a != b,
            Cmp::GreaterThanOrEqualTo => a >= b,
            Cmp::GreaterThan => a > b,
        }
    }

    pub fn u32(self, a: u32, b: u32) -> bool {
        match self {
            Cmp::LessThan => a < b,
            Cmp::LessThanOrEqualTo => a <= b,
            Cmp::EqualTo => a == b,
            Cmp::NotEqualTo => a != b,
            Cmp::GreaterThanOrEqualTo => a >= b,
            Cmp::GreaterThan => a > b,
        }
    }

    pub fn value(self, a: GsValue, b: GsValue) -> bool {
        match (a, b) {
            (GsValue::Bool(x), GsValue::Bool(y)) => self.i32(i32::from(x), i32::from(y)),
            (GsValue::Float(x), GsValue::Float(y)) => self.f32(x, y),
            (GsValue::Int(x), GsValue::Int(y)) => self.i32(x, y),
            _ => false,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum FloatBinding {
    Const(f32),
    GameState { entity: Name, id: u16 },
    Expr(Box<FloatBinding>, u8, Box<FloatBinding>),
}

impl FloatBinding {
    pub fn eval(&self, cx: &mut Ctx) -> Option<f32> {
        Some(match self {
            FloatBinding::Const(c) => *c,
            FloatBinding::GameState { entity, id } => cx.game_state(entity, *id)?.as_f32(),
            FloatBinding::Expr(a, op, b) => {
                let y = b.eval(cx)?;
                let x = a.eval(cx)?;
                match op {
                    0 => x + y,
                    1 => x - y,
                    2 => x * y,
                    _ if y == 0.0 => 0.0,
                    _ => x / y,
                }
            }
        })
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct NamedFlags {
    pub entries: Vec<(u32, Arc<str>, bool)>,
}

impl NamedFlags {
    fn find(&self, hash: u32) -> Result<usize, usize> {
        self.entries.binary_search_by_key(&hash, |e| e.0)
    }

    pub fn set(&mut self, name: &Name, value: bool, temporary: bool) {
        match (self.find(name.hash), value) {
            (Ok(i), false) => {
                self.entries.remove(i);
            }
            (Err(i), true) => self.entries.insert(i, (name.hash, name.text.clone(), temporary)),
            _ => {}
        }
    }

    pub fn check(&self, name: &Name) -> bool {
        self.find(name.hash).is_ok()
    }

    pub fn clear_temporary(&mut self) {
        self.entries.retain(|e| !e.2);
    }

    pub fn permanent(&self) -> impl Iterator<Item = &str> {
        self.entries.iter().filter(|e| !e.2).map(|e| &*e.1)
    }

    pub fn permanent_hashes(&self) -> Vec<u32> {
        self.entries.iter().filter(|e| !e.2).map(|e| e.0).collect()
    }

    pub fn set_permanent_hash(&mut self, hash: u32, value: bool) {
        self.set(&Name { hash, text: Arc::from("") }, value, false);
    }

    pub fn names(&self) -> Vec<String> {
        let mut v: Vec<String> = self.entries.iter().map(|e| e.1.to_string()).collect();
        v.sort();
        v
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct StatKey {
    pub name: Name,
    pub scope: i32,
    pub location: Option<Name>,
}

impl StatKey {
    pub fn key(&self) -> u32 {
        match &self.location {
            None => self.name.hash,
            Some(l) => {
                let mut digits = [0u8; 10];
                let (mut n, mut i) = (l.hash, digits.len());
                loop {
                    i -= 1;
                    digits[i] = b'0' + (n % 10) as u8;
                    n /= 10;
                    if n == 0 {
                        break;
                    }
                }
                super::fnv1_continue(fnv1(self.name.text.as_bytes()), &digits[i..])
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum StatValue {
    Int(u32),
    Float(f32),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RacerQuery {
    RemainingDistance,
    Loops,
    Progress,
}
