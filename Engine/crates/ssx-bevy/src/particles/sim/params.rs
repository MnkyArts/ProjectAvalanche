use std::collections::HashMap;

use bevy::math::{Vec3, Vec4};
use ssx_formats::lynx::{Param, Value};
use ssx_formats::lynx_schema::{self, ParamDefault};

#[derive(Clone, Debug)]
pub(super) struct ParamDef {
    base: Vec<Vec4>,
    states: Vec<(u8, Vec4)>,
    pub(super) text: Option<String>,
}

#[derive(Clone, Copy)]
pub struct NameHasher(u64);

impl Default for NameHasher {
    fn default() -> Self {
        NameHasher(0xcbf2_9ce4_8422_2325)
    }
}

impl std::hash::Hasher for NameHasher {
    fn finish(&self) -> u64 {
        self.0
    }
    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.0 = (self.0 ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
}

type NameMap<K, V> = HashMap<K, V, std::hash::BuildHasherDefault<NameHasher>>;

// PORT(choice): every object resolves these 90 names once, when its definition is compiled, to its own parameter, its
// class's default or nothing; a read by such a name is then an array index, where it was an FNV-1a hash of the name
// and a hash-map probe on every read (16 to 19 reads per emitted particle in the emitters, about 30 per draw). A read
// gives the same bits as the read by the name's string (`found_names_read_as_their_strings`). The console's actions
// read their parameters through slots by the class's index (`GetParamDef`, action vtable slot 4), not by name.
pub const PARAM_NAMES: [&str; 90] = [
    "bActive",
    "bDepthSort",
    "bEmitRatePerCenter",
    "bEmitRatePerFrame",
    "bFollowCamera",
    "bHalfSizeRender",
    "bInject",
    "bModuloPosition",
    "bRespectExternalTrigger",
    "bStretchPerCamera",
    "bStretchPerParticle",
    "bToneMapRender",
    "bUseGlobalLighting",
    "fAdoption",
    "fAgeWarp",
    "fAmountNoise",
    "fAngularVelocityAdoption",
    "fAngularVelocityAdoptionSpread",
    "fCurl",
    "fDragCoefficient",
    "fFieldFraction",
    "fFrequency",
    "fGustChangeGain",
    "fLevelOfDetail",
    "fLightFalloff",
    "fLightRadius",
    "fMagnitude",
    "fMassDependenceOnSize",
    "fMassMean",
    "fMassSpread",
    "fRadialVelocityMean",
    "fRadialVelocitySpread",
    "fRadius",
    "fScale",
    "fSizeDependence",
    "fSizeMean",
    "fSizeSpread",
    "fSpeedMax",
    "fSpeedMin",
    "fStretchSpeedMin",
    "fStretchSpeedScale",
    "fTwist",
    "fVelocityAdoption",
    "fVelocityAdoptionSpread",
    "fWeatherScalar",
    "fZFeatherDist",
    "fZFeatherFalloff",
    "fZFeatherHeight",
    "fZFeatherOffset",
    "iAgeMs",
    "iAgeSpreadMs",
    "iCullBehavior",
    "iEmitCycles",
    "iEmitDelayMs",
    "iEmitDelayVarianceMs",
    "iEmitDurationMs",
    "iEmitRate",
    "iGustIntervalMs",
    "iInstancing",
    "iNumParticlesMax",
    "iObeyLocator",
    "iOrdering",
    "iPeriodicBoxNum",
    "iRenderLayer",
    "iSpace",
    "iSpeedPower",
    "iSubFrameTechnique",
    "iType",
    "iZeroVelocity",
    "rKillAge",
    "vAccelVariance",
    "vAcceleration",
    "vAmbientWind",
    "vAngularVelocityMean",
    "vAngularVelocitySpread",
    "vCenter",
    "vColorMult",
    "vColorRamp",
    "vDimensions",
    "vGustVariance",
    "vHemisphereShape",
    "vInnerOuterSkew",
    "vLightColor",
    "vLightPosition",
    "vOrientationMean",
    "vOrientationSpread",
    "vStretchVelocity",
    "vTurbulentAmplitude",
    "vVelocityMean",
    "vVelocitySpread",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Name(usize);

pub const fn param_name(name: &str) -> Name {
    let mut i = 0;
    while i < PARAM_NAMES.len() {
        let (a, b) = (PARAM_NAMES[i].as_bytes(), name.as_bytes());
        if a.len() == b.len() {
            let mut j = 0;
            while j < a.len() && a[j] == b[j] {
                j += 1;
            }
            if j == a.len() {
                return Name(i);
            }
        }
        i += 1;
    }
    panic!("not in PARAM_NAMES")
}

macro_rules! name {
    ($name:literal) => {
        const { $crate::particles::sim::params::param_name($name) }
    };
}
pub(crate) use name;

#[derive(Clone, Copy, Debug)]
pub enum Found {
    Def(usize),
    Default(Vec4),
    Absent,
}

pub trait Key: Copy {
    fn find(self, params: &Params) -> Found;
}

impl Key for Name {
    fn find(self, params: &Params) -> Found {
        params.known[self.0]
    }
}

impl Key for &str {
    fn find(self, params: &Params) -> Found {
        params.find(self)
    }
}

#[derive(Clone, Debug)]
pub struct Params {
    map: NameMap<String, usize>,
    pub(super) defs: Vec<ParamDef>,
    defaults: NameMap<&'static str, Vec4>,
    known: Box<[Found]>,
}

fn lanes(v: &Value, previous: Vec4) -> Vec4 {
    match v {
        Value::Bool(b) => Vec4::new(*b as u8 as f32, 0.0, 0.0, 0.0),
        Value::Int(i) => Vec4::new(*i as f32, 0.0, 0.0, 0.0),
        Value::Float(f) => Vec4::new(*f, 0.0, 0.0, 0.0),
        Value::Vector(c) => {
            let mut out = previous;
            for (i, x) in c.iter().enumerate().take(4) {
                out[i] = *x;
            }
            out
        }
        Value::Reference(_) | Value::String(_) => Vec4::ZERO,
    }
}

pub(super) fn default_lanes(d: ParamDefault) -> Vec4 {
    match d {
        ParamDefault::Bool(b) => Vec4::new(b as u8 as f32, 0.0, 0.0, 0.0),
        ParamDefault::Int(i) => Vec4::new(i as f32, 0.0, 0.0, 0.0),
        ParamDefault::Float(f) => Vec4::new(f, 0.0, 0.0, 0.0),
        ParamDefault::Vector(v) => Vec4::from_array(v),
        ParamDefault::None => Vec4::ZERO,
    }
}

impl Params {
    pub(super) fn new(class: &str, base: &[Param], states: &[(u8, &[Param])]) -> Self {
        let schema = lynx_schema::class(class);
        let default_of =
            |name: &str| schema.and_then(|c| c.param(name)).map_or(Vec4::ZERO, |p| default_lanes(p.default));
        let mut defaults: NameMap<&'static str, Vec4> = NameMap::default();
        for row in schema.map_or(&[][..], |c| c.params) {
            defaults.entry(row.name).or_insert_with(|| default_lanes(row.default));
        }
        let mut map: NameMap<String, ParamDef> = NameMap::default();
        for p in base {
            let def = map.entry(p.name.clone()).or_insert_with(|| ParamDef {
                base: vec![default_of(&p.name)],
                states: Vec::new(),
                text: None,
            });
            let i = p.index as usize;
            if def.base.len() <= i {
                def.base.resize(i + 1, default_of(&p.name));
            }
            def.base[i] = lanes(&p.value, def.base[i]);
            if let Value::String(s) | Value::Reference(s) = &p.value {
                def.text = Some(s.clone());
            }
        }
        for (state, list) in states {
            for p in list.iter().filter(|p| p.index == 0) {
                let def = map.entry(p.name.clone()).or_insert_with(|| ParamDef {
                    base: vec![default_of(&p.name)],
                    states: Vec::new(),
                    text: None,
                });
                let v = lanes(&p.value, default_of(&p.name));
                def.states.push((*state, v));
            }
        }
        let (mut by_name, mut defs) = (NameMap::default(), Vec::with_capacity(map.len()));
        for (name, def) in map {
            by_name.insert(name, defs.len());
            defs.push(def);
        }
        let mut params = Params { map: by_name, defs, defaults, known: Box::default() };
        params.known = PARAM_NAMES.iter().map(|n| params.find(n)).collect();
        params
    }

    pub(super) fn find(&self, name: &str) -> Found {
        match self.map.get(name) {
            Some(&i) => Found::Def(i),
            None => self.defaults.get(name).map_or(Found::Absent, |v| Found::Default(*v)),
        }
    }

    pub(super) fn iter(&self) -> impl Iterator<Item = (&str, &ParamDef)> {
        self.map.iter().map(|(n, &i)| (n.as_str(), &self.defs[i]))
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Weights([f32; 16]);

impl Weights {
    pub const NONE: Weights = Weights([0.0; 16]);

    pub fn from_list(list: &[(u8, f32)]) -> Self {
        let mut w = [0.0; 16];
        for &(s, x) in list {
            if (s as usize) < 16 {
                w[s as usize] = x;
            }
        }
        Weights(w)
    }
}

#[derive(Clone, Copy)]
pub struct View<'a> {
    params: &'a Params,
    w: &'a Weights,
}

impl<'a> View<'a> {
    pub fn new(params: &'a Params, w: &'a Weights) -> Self {
        View { params, w }
    }

    pub fn v(&self, name: impl Key, i: usize) -> Vec4 {
        match name.find(self.params) {
            Found::Def(d) => self.blended(&self.params.defs[d], i),
            Found::Default(v) => v,
            Found::Absent => Vec4::ZERO,
        }
    }
    pub(super) fn blended(&self, def: &ParamDef, i: usize) -> Vec4 {
        let base = def.base[i.min(def.base.len() - 1)];
        if i != 0 || def.states.is_empty() {
            return base;
        }
        let (mut sum_w, mut acc) = (0.0, Vec4::ZERO);
        for &(s, v) in &def.states {
            let w = self.w.0[s as usize & 15];
            sum_w += w;
            acc += v * w;
        }
        base * (1.0 - sum_w) + acc
    }
    pub fn v3(&self, name: impl Key) -> Vec3 {
        self.v(name, 0).truncate()
    }
    pub fn f(&self, name: impl Key) -> f32 {
        self.v(name, 0).x
    }
    pub fn i(&self, name: impl Key) -> i32 {
        match name.find(self.params) {
            Found::Def(d) if !self.params.defs[d].states.is_empty() => {
                (self.blended(&self.params.defs[d], 0).x + 0.5).trunc() as i32
            }
            Found::Def(d) => self.blended(&self.params.defs[d], 0).x as i32,
            Found::Default(v) => v.x as i32,
            Found::Absent => 0,
        }
    }
    pub fn b(&self, name: impl Key) -> bool {
        self.v(name, 0).x > 0.5
    }
    pub fn count(&self, name: impl Key) -> usize {
        match name.find(self.params) {
            Found::Def(d) => self.params.defs[d].base.len(),
            Found::Default(_) | Found::Absent => 1,
        }
    }
    pub fn text(&self, name: impl Key) -> Option<&'a str> {
        match name.find(self.params) {
            Found::Def(d) => self.params.defs[d].text.as_deref(),
            Found::Default(_) | Found::Absent => None,
        }
    }
    pub fn built_in(&self, name: &str) -> bool {
        self.params.defaults.contains_key(name)
    }
}
