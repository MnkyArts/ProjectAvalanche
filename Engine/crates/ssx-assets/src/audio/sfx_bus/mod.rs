mod control;
mod core_voice;
mod director;
mod plugins;
mod process;

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result};
use ssx_formats::ptch::{ModuleV1Kind, PatchV1, Ptch, SnapshotV1, VoiceV1};

use crate::sfx_dsp::{self as dsp, BLOCK};
use crate::sfx_dsp_remix as fx;

pub use core_voice::*;
pub use director::*;
pub use plugins::*;
pub use process::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VarRef {
    pub patch: usize,
    pub offset: usize,
    pub kind: u8,
}

#[derive(Clone, Debug)]
struct Vars {
    data: Vec<u8>,
    names: Names,
}

#[derive(Default)]
pub struct VarStore {
    slots: Vec<Option<Vars>>,
}

impl VarStore {
    fn add(&mut self, p: &PatchV1, names: Names) -> usize {
        let v = Vars { data: p.data.clone(), names };
        match self.slots.iter().position(Option::is_none) {
            Some(i) => {
                self.slots[i] = Some(v);
                i
            }
            None => {
                self.slots.push(Some(v));
                self.slots.len() - 1
            }
        }
    }

    fn remove(&mut self, i: usize) {
        if i != 0 {
            self.slots[i] = None;
        }
    }

    pub fn resolve(&self, local: usize, name: &str) -> VarRef {
        for patch in [local, 0] {
            if let Some(&(offset, kind)) =
                self.slots.get(patch).and_then(Option::as_ref).and_then(|v| v.names.get(name))
            {
                return VarRef { patch, offset, kind };
            }
        }
        VarRef { patch: local, offset: 0, kind: 4 }
    }

    fn word(&self, r: VarRef) -> u32 {
        let Some(v) = self.slots.get(r.patch).and_then(Option::as_ref) else { return 0 };
        v.data.get(r.offset..r.offset + 4).map_or(0, |b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }

    pub fn number(&self, r: VarRef) -> f64 {
        let w = self.word(r);
        if r.kind == 0 { f64::from(f32::from_bits(w)) } else { f64::from(w as i32) }
    }

    pub fn string(&self, r: VarRef) -> String {
        let Some(v) = self.slots.get(r.patch).and_then(Option::as_ref) else { return String::new() };
        let b = v.data.get(r.offset..(r.offset + 0x20).min(v.data.len())).unwrap_or(&[]);
        String::from_utf8_lossy(&b[..b.iter().position(|&c| c == 0).unwrap_or(b.len())]).into_owned()
    }

    fn set_word(&mut self, r: VarRef, w: u32) {
        if let Some(b) =
            self.slots.get_mut(r.patch).and_then(Option::as_mut).and_then(|v| v.data.get_mut(r.offset..r.offset + 4))
        {
            b.copy_from_slice(&w.to_be_bytes());
        }
    }

    pub fn set_number(&mut self, r: VarRef, x: f64) {
        let w = if r.kind == 0 { (x as f32).to_bits() } else { x as i32 as u32 };
        self.set_word(r, w);
    }

    pub fn set_string(&mut self, r: VarRef, s: &str) {
        if let Some(b) =
            self.slots.get_mut(r.patch).and_then(Option::as_mut).and_then(|v| v.data.get_mut(r.offset..r.offset + 0x20))
        {
            b.fill(0);
            let n = s.len().min(0x1F);
            b[..n].copy_from_slice(&s.as_bytes()[..n]);
        }
    }
}

pub struct PatchSet {
    pub global: PatchV1,
    pub buses: PatchV1,
    pub voices: HashMap<String, PatchV1>,
    names: HashMap<String, Names>,
}

type Names = Arc<HashMap<String, (usize, u8)>>;

impl PatchSet {
    pub fn bus_names(&self) -> Vec<String> {
        let mut v: Vec<String> = self
            .buses
            .modules
            .iter()
            .filter_map(|m| match &m.kind {
                ModuleV1Kind::Voice(v) if v.plugins.iter().any(|p| &p.fourcc == b"Sub0") => Some(v.name.clone()),
                _ => None,
            })
            .collect();
        v.sort();
        v
    }
}

fn names_of(p: &PatchV1) -> Names {
    Arc::new(p.variables.iter().map(|v| (v.name.clone(), (usize::from(v.offset), v.kind))).collect())
}

impl PatchSet {
    pub fn load(big_root: &Path) -> Result<Self> {
        let dir = "data/xenon/audio/rwaudiocontroller";
        let read = |p: &Path| -> Result<PatchV1> {
            match Ptch::parse(&std::fs::read(p)?).with_context(|| p.display().to_string())? {
                Ptch::V1(v) => Ok(v),
                Ptch::V4(_) => anyhow::bail!("{}: a v4 patch in the v1 mixer", p.display()),
            }
        };
        let gpath =
            crate::music::find_file(big_root, &format!("{dir}/mixer_global.ptc")).context("mixer_global.ptc")?;
        let global = read(&gpath)?;
        let bpath = crate::music::find_file(big_root, &format!("{dir}/mixer_ptc.big")).context("mixer_ptc.big")?;
        let big = std::fs::read(&bpath)?;
        let archive = ssx_formats::big::BigArchive::parse(&big)?;
        let mut voices = HashMap::new();
        let mut buses = None;
        for e in &archive.entries {
            let data = archive.member(&big, e)?;
            let Ptch::V1(p) = Ptch::parse(&data).with_context(|| e.path())? else { continue };
            if p.name == "buses_patch" {
                buses = Some(p);
            } else {
                voices.insert(p.name.to_ascii_lowercase(), p);
            }
        }
        let buses = buses.context("buses_patch not in mixer_ptc.big")?;
        let mut names = HashMap::new();
        names.insert("Global_patch".to_string(), names_of(&global));
        names.insert("buses_patch".to_string(), names_of(&buses));
        for (k, p) in &voices {
            names.insert(k.clone(), names_of(p));
        }
        Ok(Self { global, buses, voices, names })
    }
}

#[derive(Clone, Copy, Debug)]
struct FaderRamp {
    from: f32,
    to: f32,
    step: f32,
    n: u32,
    done: u32,
}

impl FaderRamp {
    fn next(&mut self) -> f32 {
        if self.done >= self.n {
            return self.to;
        }
        let g = self.from + self.step * self.done as f32;
        self.done += 1;
        g
    }
}

#[derive(Debug)]
enum Stage {
    Source,
    Pass,
    Limiter(dsp::Compressor),
    Gain { last: Option<f32> },
    Fader { fade: Option<FaderRamp> },
    Send { last: Option<f32>, target: Option<usize>, key: (String, u8) },
    Route { last: Option<f32>, target: Option<usize>, key: (String, u8), window: (usize, usize, usize) },
    SubMix { input: Option<usize> },
    Filter(dsp::FilterKind, dsp::Filter),
    Compressor { state: dsp::Compressor, side: Option<usize> },
    Clip,
    Matrix { last: [f32; 8] },
    Pan { last: Vec<[f32; 8]> },
    Pan3d(Box<dsp::pan3d::Pan3d>),
    Silent,
    ReverbIr(Box<dsp::reverb_ir::ReverbIr>),
    BandPass(Box<fx::BandPassFir64>),
    FreqShift(Box<fx::FrequencyShift>),
    Delay(Box<fx::Delay>),
    Chorus(Box<fx::ModDelay>),
    Vibrato(Box<fx::ModDelay>),
    Reverb(Box<fx::Reverb>),
    BeatDetect(Box<fx::BeatDetect>),
}

#[derive(Debug)]
struct Plugin {
    fourcc: [u8; 4],
    outputs: usize,
    attrs: Vec<f64>,
    ranges: &'static [Attr],
    ctor: Vec<f64>,
    stage: Stage,
    bypass: bool,
}

impl Plugin {
    fn attr(&self, i: usize) -> f64 {
        self.attrs.get(i).copied().unwrap_or(0.0)
    }
}

#[derive(Debug)]
pub struct MixVoice {
    pub name: String,
    pub level: u8,
    vars: usize,
    plugins: Vec<Plugin>,
    mute: Option<VarRef>,
    gain: VarRef,
    mute_output: VarRef,
    params: Vec<(VarRef, usize, usize)>,
    pub source: Option<u32>,
    core: bool,
    first: bool,
    meter: f64,
    rest: u16,
}

struct Accumulator {
    key: (String, u8),
    chans: Vec<Vec<f32>>,
    fed: bool,
    dirty: bool,
}

// TODO(guess): resting voices are a port approximation (EAAudioCore runs every plug-in each block on its own VMX thread); they cut the 116-voice reference mix from 10.2–10.4 to 6.3–6.5 % of real time; against `SSX_SFX_REST=off` one sample of 5.76 M of `sfx_ride alps0101 60 --event race` differs by one 16-bit step; settle: nothing to decode — drop it once the kernels are fast enough without.
const QUIET: f32 = 1e-12;
const REST_BLOCKS: u16 = 16;

fn quiet(buf: &[Vec<f32>]) -> bool {
    buf.iter().all(|c| c.iter().all(|x| x.abs() <= QUIET))
}

pub trait SourceHost {
    fn fill(&mut self, source: u32, pitch: f32, out: &mut [Vec<f32>]) -> bool;
}

struct SnapState {
    def: SnapshotV1,
    selector: VarRef,
    current: String,
    fade: Option<Fade>,
}

struct Fade {
    time: f32,
    elapsed: f32,
    values: Vec<(VarRef, VarRef, u8, u8, f64, f64)>,
}

pub struct Selector {
    pub variable: &'static str,
    pub table: &'static [&'static str],
    mask: u64,
    shown: Option<usize>,
}

impl Selector {
    const fn new(variable: &'static str, table: &'static [&'static str]) -> Self {
        Self { variable, table, mask: 0, shown: None }
    }

    pub fn set(&mut self, index: usize, mode: u8) {
        let bit = 1u64 << index;
        self.mask = match mode {
            2 => bit,
            1 => self.mask | bit,
            _ => self.mask & !bit,
        };
    }

    pub fn is_set(&self, index: usize) -> bool {
        self.mask >> index & 1 != 0
    }

    fn wanted(&self) -> usize {
        (0..self.table.len()).rev().find(|&i| self.is_set(i)).unwrap_or(0)
    }
}

pub const LAYER0: &[&str] = &["Home_Theatre", "Television"];
pub const LAYER1: &[&str] = &[
    "FE_Globe",
    "GP_Race_It",
    "GP_Trick_It",
    "GP_Survive_It",
    "DD_Survive_It",
    "Final_DD",
    "Tutorial",
    "Tutorial_Sky",
    "NIS",
    "DD_NIS",
    "EOG_NIS",
    "Chopper_Interior",
    "Cinematic",
    "Pause",
    "Transition_Duck",
];
pub const LAYER2: &[&str] = &[
    "GP_Default",
    "GP_RaceIt",
    "GP_TrickIt",
    "GP_SurviveIt",
    "Grinding",
    "Boost_1",
    "Awesome",
    "Boost_2",
    "Tricky_2",
    "Boost_3",
    "Big_Air",
    "WingSuit_1",
    "Trick_Level_1",
    "Trick_Level_2",
    "Trick_Level_3",
    "Time_Dilation",
    "Avalanche_1",
    "Avalanche_2",
    "Avalanche_3",
    "Max_Speed",
    "Big_TakeOff",
    "Big_Landing",
    "Rewind",
    "NIS_1",
    "NIS_Sur_Till_Die",
    "Game_Start_Screen",
    "Cinematic_1",
    "Cinematic_2",
    "Cinematic_3",
    "Cinematic_4",
    "Cinematic_5",
    "EOG_Good",
    "EOG_Not_Good",
];
pub const REVERB: &[&str] = &["Off", "Exterior", "SmallInterior_Channel0", "LargeInterior_Channel1"];
pub const CAMERA: &[&str] = &["Normal", "Far", "15p", "Reverse"];

pub struct Graph {
    pub set: Arc<PatchSet>,
    pub vars: VarStore,
    voices: Vec<Option<MixVoice>>,
    order: Vec<usize>,
    accs: Vec<Accumulator>,
    snaps: Vec<SnapState>,
    pub selectors: [Selector; 5],
    dac: usize,
    pub log: Vec<String>,
    meter: Option<(usize, Meter)>,
    profile: Option<std::collections::HashMap<String, (u64, u64)>>,
    rest: bool,
}

type Meter = std::collections::BTreeMap<String, (f64, usize)>;

const DAC: &str = "Master_DAC";

impl Graph {
    pub fn new(set: Arc<PatchSet>) -> Self {
        let mut vars = VarStore::default();
        vars.add(&set.global, set.names["Global_patch"].clone());
        let mut g = Self {
            set: set.clone(),
            vars,
            voices: Vec::new(),
            order: Vec::new(),
            accs: Vec::new(),
            snaps: Vec::new(),
            selectors: [
                Selector::new("gLayer0SnapshotSelector", LAYER0),
                Selector::new("gLayer1SnapshotSelector", LAYER1),
                Selector::new("gLayer2SnapshotSelector", LAYER2),
                Selector::new("gReverbSnapshotSelector", REVERB),
                Selector::new("gCameraSnapshotSelector", CAMERA),
            ],
            dac: 0,
            log: Vec::new(),
            meter: std::env::var_os("SSX_SFX_METER").map(|_| (0, Default::default())),
            profile: std::env::var("SSX_SFX_PROFILE").is_ok_and(|v| v == "1").then(Default::default),
            rest: !std::env::var("SSX_SFX_REST").is_ok_and(|v| matches!(v.as_str(), "off" | "0")),
        };
        g.dac = g.accumulator(DAC, 1, 6);
        for m in &set.global.modules {
            if let ModuleV1Kind::Snapshot(s) = &m.kind {
                let selector = g.vars.resolve(0, &s.selector);
                g.snaps.push(SnapState { def: s.clone(), selector, current: String::new(), fade: None });
            }
        }
        let bus_vars = g.vars.add(&set.buses, set.names["buses_patch"].clone());
        for m in &set.buses.modules {
            if let ModuleV1Kind::Voice(v) = &m.kind {
                g.add_voice(v.clone(), bus_vars, None);
            }
        }
        for s in g.selectors.iter_mut() {
            s.set(0, 1);
        }
        g
    }

    fn accumulator(&mut self, name: &str, namespace: u8, channels: usize) -> usize {
        let key = (name.to_string(), namespace);
        if let Some(i) = self.accs.iter().position(|a| a.key == key) {
            return i;
        }
        self.accs.push(Accumulator { key, chans: vec![vec![0.0; BLOCK]; channels], fed: false, dirty: false });
        self.accs.len() - 1
    }

    fn add_voice(&mut self, def: VoiceV1, vars: usize, source: Option<u32>) -> usize {
        let mut ch_vars = def.channel_vars.iter();
        let mut prev = 1usize;
        let mut plugins = Vec::with_capacity(def.plugins.len());
        let mut ctor_vars = def.ctor_params.iter();
        for p in &def.plugins {
            let outputs = match p.channels {
                0xFE => ch_vars.next().map_or(1, |b| self.vars.number(self.vars.resolve(vars, &b.variable)) as usize),
                0xFF => prev,
                n => usize::from(n),
            }
            .clamp(1, 8);
            let (n_ctor, ranges) = descriptor(&p.fourcc);
            let ctor: Vec<f64> = (0..usize::from(p.n_ctor).max(n_ctor))
                .map(|i| {
                    if i < usize::from(p.n_ctor) {
                        ctor_vars.next().map_or(0.0, |b| self.vars.number(self.vars.resolve(vars, &b.variable)))
                    } else {
                        ctor_default(&p.fourcc, i)
                    }
                })
                .collect();
            let stage = make_stage(&p.fourcc, outputs, &ctor);
            plugins.push(Plugin {
                fourcc: p.fourcc,
                outputs,
                attrs: ranges.iter().map(|r| r.0).collect(),
                ranges,
                ctor,
                stage,
                bypass: false,
            });
            prev = outputs;
        }
        let resolve = |s: &Self, n: &str| s.vars.resolve(vars, n);
        let mute = (!def.mute.is_empty()).then(|| resolve(self, &def.mute));
        let (gain, mute_output) = (resolve(self, &def.gain), resolve(self, &def.mute_output));
        let params = def
            .params
            .iter()
            .map(|b| (resolve(self, &b.variable), usize::from(b.plugin), usize::from(b.index)))
            .collect();
        for l in &def.links {
            let pl = &mut plugins[usize::from(l.plugin)];
            let consumer = matches!(&pl.fourcc, b"Sen0" | b"Rou0");
            if consumer {
                continue;
            }
            let ch = pl.outputs;
            let acc = self.accumulator(&l.target, l.namespace, ch);
            match &mut pl.stage {
                Stage::SubMix { input } => *input = Some(acc),
                Stage::Compressor { side, .. } => *side = Some(acc),
                _ => {}
            }
        }
        for l in &def.links {
            let k = (l.target.clone(), l.namespace);
            match &mut plugins[usize::from(l.plugin)].stage {
                Stage::Send { key, .. } => *key = k,
                Stage::Route { key, window, .. } => {
                    *key = k;
                    *window = (usize::from(l.source_channel), usize::from(l.target_channel), usize::from(l.channels));
                }
                _ => {}
            }
        }
        let level = def.level;
        let v = MixVoice {
            name: def.name.clone(),
            level,
            vars,
            plugins,
            mute,
            gain,
            mute_output,
            params,
            source,
            core: false,
            first: true,
            meter: 0.0,
            rest: 0,
        };
        let slot = self.voices.iter().position(Option::is_none).unwrap_or(self.voices.len());
        if slot == self.voices.len() {
            self.voices.push(None);
        }
        self.voices[slot] = Some(v);
        let at = self
            .order
            .iter()
            .position(|&i| self.voices[i].as_ref().is_some_and(|o| o.level >= level))
            .unwrap_or(self.order.len());
        self.order.insert(at, slot);
        self.update_voice(slot);
        slot
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_dac_hands_on_five_one_and_folds_it_as_ea_does() {
        let dac: Vec<Vec<f32>> =
            (0..DAC_CHANNELS).map(|c| (0..BLOCK).map(|k| (c * 1000 + k) as f32 * 1e-4).collect()).collect();
        let mut six = vec![0.0; DAC_CHANNELS * BLOCK];
        interleave_dac(&dac, &mut six);
        assert_eq!(&six[..6], &[dac[0][0], dac[2][0], dac[1][0], dac[5][0], dac[3][0], dac[4][0]]);
        let (mut a, mut b) = (vec![0.0; 2 * BLOCK], vec![0.0; 2 * BLOCK]);
        downmix(&dac, &mut a);
        fold_interleaved(&six, &mut b);
        assert_eq!(a, b);
        let (l, r) = fold(1.0, 0.0, 0.0, 0.0, 0.0);
        assert!((l - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-6 && r == 0.0);
        let (l, r) = fold(0.0, 1.0, 0.0, 0.0, 0.0);
        assert!((l - 0.5).abs() < 1e-6 && (r - 0.5).abs() < 1e-6, "the centre at −6 dB a side");
    }

    #[test]
    fn selectors_show_the_highest_entry() {
        let mut s = Selector::new("x", LAYER2);
        s.set(0, 1);
        s.set(4, 1);
        s.set(21, 1);
        assert_eq!(LAYER2[s.wanted()], "Big_Landing");
        s.set(21, 0);
        assert_eq!(LAYER2[s.wanted()], "Grinding");
        s.set(5, 2);
        assert_eq!(LAYER2[s.wanted()], "Boost_1");
        assert!(!s.is_set(0));
    }

    #[test]
    fn variables_resolve_locally_then_globally() {
        let mut store = VarStore::default();
        let names = |v: &[(&str, usize, u8)]| Arc::new(v.iter().map(|(n, o, k)| (n.to_string(), (*o, *k))).collect());
        let mk = |data: Vec<u8>| PatchV1 { name: String::new(), variables: Vec::new(), data, modules: Vec::new() };
        store.add(&mk(vec![0, 0, 0, 0, 0x3F, 0x80, 0, 0]), names(&[("g", 4, 0)]));
        let local = store.add(&mk(vec![0; 8]), names(&[("i", 4, 2)]));
        let g = store.resolve(local, "g");
        assert_eq!((g.patch, store.number(g)), (0, 1.0));
        let i = store.resolve(local, "i");
        store.set_number(i, 6.0);
        assert_eq!(store.number(i), 6.0);
        assert_eq!(store.resolve(local, "missing"), VarRef { patch: local, offset: 0, kind: 4 });
    }
}
