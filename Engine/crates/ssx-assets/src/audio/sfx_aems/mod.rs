mod instances;
mod interp;
mod operators;
mod provenance;

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result};
use ssx_formats::abk::{AbkFile, RefKind, SampleEntry};
use ssx_formats::aems::{INSTANCE_HEADER, Node, Op};
use ssx_formats::csi::{CsiFile, Kind};

use crate::music::Bytes;
use crate::sfx_override::{Choice, OverrideSound};

pub use instances::*;
use interp::*;
use operators::*;
use provenance::*;

pub const MIXER_RATE: u32 = 48_000;
pub const FRAME_SAMPLES: usize = 256;
pub const TIMER_HZ: f32 = 41.6;

pub fn tick_frames() -> (u32, f32) {
    let period = FRAME_SAMPLES as f32 / MIXER_RATE as f32;
    let (mut n, mut acc) = (0u32, 0f32);
    loop {
        acc += period;
        n += 1;
        if acc + period >= 1.0 / TIMER_HZ {
            break;
        }
    }
    (n, n as f32 * period * 1000.0)
}

pub trait VoiceHost {
    fn start(&mut self, bank: usize, entry: &SampleEntry, params: &[(u8, i32)], choice: Option<&Choice>)
    -> Option<u32>;
    fn set_param(&mut self, voice: u32, id: u8, value: i32);
    fn destroy(&mut self, voice: u32);
    fn pause(&mut self, voice: u32);
    fn resume(&mut self, voice: u32);
    fn status(&mut self, voice: u32) -> Option<(i32, i32)>;
}

pub struct Bank {
    pub name: String,
    pub abk: AbkFile,
    pub data: Arc<Vec<u8>>,
    pub ast: Option<Arc<Bytes>>,
    links: Vec<ModuleLinks>,
}

#[derive(Default)]
struct ModuleLinks {
    program: Arc<Vec<Node>>,
    function_ref: Option<(u32, String)>,
    handle_refs: HashMap<u32, (u32, String)>,
    function: Option<u32>,
    handles: HashMap<u32, u32>,
    players: Vec<u32>,
    player_events: HashMap<u32, Vec<u32>>,
}

impl Bank {
    pub fn load(big_root: &Path, name: &str) -> Result<Self> {
        let rel = format!("data/xenon/audio/sfx/{name}.abk");
        let path = crate::music::find_file(big_root, &rel).with_context(|| rel.clone())?;
        let data = std::fs::read(&path)?;
        let abk = AbkFile::parse(&data).with_context(|| format!("parsing {rel}"))?;
        let ast = if abk.ast_word != 0 {
            let rel = format!("data/xenon/audio/sfx/{name}.ast");
            let p = crate::music::find_file(big_root, &rel).with_context(|| rel.clone())?;
            Some(Arc::new(Bytes::open(&p)?))
        } else {
            None
        };
        let mut links: Vec<ModuleLinks> = abk
            .modules
            .iter()
            .map(|m| ModuleLinks { program: Arc::new(m.program.clone()), ..Default::default() })
            .collect();
        for r in &abk.names {
            match r.kind {
                RefKind::Function => {
                    if let Some(i) = abk.modules.iter().position(|m| m.offset + 4 == r.target) {
                        links[i].function_ref = Some((r.id, r.name.clone()));
                    }
                }
                _ => {
                    let at = abk.modules.iter().position(|m| {
                        r.target >= m.data_offset + INSTANCE_HEADER && r.target < m.data_offset + m.data_size
                    });
                    if let Some(i) = at {
                        let m = &abk.modules[i];
                        let off = r.target - m.data_offset - INSTANCE_HEADER;
                        links[i].handle_refs.insert(off, (r.id, r.name.clone()));
                    }
                }
            }
        }
        Ok(Self { name: name.to_string(), abk, data: Arc::new(data), ast, links })
    }
}

#[derive(Default)]
struct CsisInstance {
    function: u32,
    modules: Vec<usize>,
    origin: Option<u32>,
}

struct Instance {
    bank: usize,
    module: usize,
    state: Vec<u8>,
    csis: usize,
    received: Vec<(u32, Option<u32>, u64)>,
    started: HashMap<u32, u64>,
    script: Option<u32>,
}

const RECEIVED: usize = 16;

#[derive(Clone, Debug)]
pub struct LogLine {
    pub ms: f64,
    pub text: String,
}

pub struct Aems {
    pub banks: Vec<Bank>,
    pub entries: Vec<ssx_formats::csi::Entry>,
    globals: HashMap<u32, i32>,
    instances: Vec<Option<Instance>>,
    order: Vec<usize>,
    live: HashMap<(usize, usize), u16>,
    csis: Vec<Option<CsisInstance>>,
    rng: [u32; 6],
    pub tick_ms: f32,
    pub now_ms: f64,
    pub log: Vec<LogLine>,
    fired: Vec<(u32, Vec<i32>)>,
    watched: Vec<u32>,
    global_changes: Vec<(u32, i32)>,
    pub verbose: bool,
    files: Vec<String>,
    overrides: HashMap<u32, Arc<OverrideSound>>,
    pub script_routes: HashMap<u32, String>,
    ticks: u64,
    override_seen: HashSet<(u32, usize, usize, usize)>,
    pub voice_names: HashMap<i32, (String, String)>,
}

fn round(x: f32) -> i32 {
    if x < 0.0 { (x - 0.5) as i32 } else { (x + 0.5) as i32 }
}

fn clamp(v: i32, lo: i32, hi: i32) -> i32 {
    if v < lo {
        lo
    } else if v > hi {
        hi
    } else {
        v
    }
}

fn rd(s: &[u8], o: usize) -> i32 {
    s.get(o..o + 4).map_or(0, |b| i32::from_be_bytes(b.try_into().unwrap()))
}

fn wr(s: &mut [u8], o: usize, v: i32) {
    if let Some(b) = s.get_mut(o..o + 4) {
        b.copy_from_slice(&v.to_be_bytes());
    }
}

fn rdf(s: &[u8], o: usize) -> f32 {
    f32::from_bits(rd(s, o) as u32)
}

fn wrf(s: &mut [u8], o: usize, v: f32) {
    wr(s, o, v.to_bits() as i32);
}

fn rh(s: &[u8], o: usize) -> u16 {
    s.get(o..o + 2).map_or(0, |b| u16::from_be_bytes(b.try_into().unwrap()))
}

fn wh(s: &mut [u8], o: usize, v: u16) {
    if let Some(b) = s.get_mut(o..o + 2) {
        b.copy_from_slice(&v.to_be_bytes());
    }
}

fn rb(s: &[u8], o: usize) -> u8 {
    s.get(o).copied().unwrap_or(0)
}

fn wb(s: &mut [u8], o: usize, v: u8) {
    if let Some(b) = s.get_mut(o) {
        *b = v;
    }
}

fn sine_table(i: usize) -> i32 {
    ((65536.0 * (i as f64 / 256.0 * std::f64::consts::FRAC_PI_2).sin()) as i32).min(65535)
}

fn sine(phase: i32) -> i32 {
    let i = (phase & 0xFF) as usize;
    match (phase >> 8) & 3 {
        0 => sine_table(i),
        1 => sine_table(256 - i),
        2 => -sine_table(i),
        _ => -sine_table(256 - i),
    }
}

impl Default for Aems {
    fn default() -> Self {
        Self::new()
    }
}

impl Aems {
    pub fn new() -> Self {
        Self {
            banks: Vec::new(),
            entries: Vec::new(),
            globals: HashMap::new(),
            instances: Vec::new(),
            order: Vec::new(),
            live: HashMap::new(),
            csis: Vec::new(),
            rng: [0; 6],
            tick_ms: tick_frames().1,
            now_ms: 0.0,
            log: Vec::new(),
            fired: Vec::new(),
            watched: Vec::new(),
            global_changes: Vec::new(),
            verbose: false,
            files: Vec::new(),
            overrides: HashMap::new(),
            script_routes: HashMap::new(),
            ticks: 0,
            override_seen: HashSet::new(),
            voice_names: HashMap::new(),
        }
    }

    fn logf(&mut self, text: String) {
        self.log.push(LogLine { ms: self.now_ms, text });
    }

    pub fn name(&self, key: u32) -> String {
        self.entries.get(key as usize).map_or_else(|| format!("#{key}"), |e| e.name.clone())
    }

    pub fn add_interface(&mut self, csi: &CsiFile) {
        self.add_interface_file(csi, "");
    }

    pub fn add_interface_file(&mut self, csi: &CsiFile, file: &str) {
        for e in &csi.entries {
            let key = self.entries.len() as u32;
            if e.kind == Kind::Global {
                self.globals.insert(key, e.default);
            }
            self.entries.push(e.clone());
            self.files.push(file.to_string());
        }
    }

    pub fn entry_file(&self, key: u32) -> Option<(&str, &str)> {
        Some((self.files.get(key as usize)?.as_str(), self.entries.get(key as usize)?.name.as_str()))
    }

    pub fn key_in(&self, file: &str, name: &str) -> Option<u32> {
        (0..self.entries.len()).find(|&k| self.files[k] == file && self.entries[k].name == name).map(|k| k as u32)
    }

    pub fn set_overrides(&mut self, overrides: HashMap<u32, Arc<OverrideSound>>) {
        self.overrides = overrides;
        self.override_seen.clear();
    }

    pub fn has_overrides(&self) -> bool {
        !self.overrides.is_empty()
    }

    pub fn resolve(&self, id: u32, name: &str) -> Option<u32> {
        let find =
            |f: &dyn Fn(&ssx_formats::csi::Entry) -> bool| self.entries.iter().position(|e| e.name == name && f(e));
        find(&|e| e.id == id).or_else(|| find(&|e| e.id & 0xFFFF == id & 0xFFFF)).map(|k| k as u32)
    }

    pub fn load_interfaces(&mut self, big_root: &Path) -> Result<usize> {
        let mut n = 0;
        for (dir, part) in [("data/xenon/audio/sfx", "sfx"), ("data/xenon/audio/tuna", "tuna")] {
            for p in crate::music::find_files(big_root, dir, "csi") {
                let csi = CsiFile::parse(&std::fs::read(&p)?).with_context(|| p.display().to_string())?;
                let stem = p.file_stem().map(|s| s.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
                self.add_interface_file(&csi, &format!("{part}/{stem}"));
                n += 1;
            }
        }
        Ok(n)
    }

    pub fn add_bank(&mut self, mut bank: Bank) -> usize {
        let mut unbound = Vec::new();
        for l in &mut bank.links {
            l.function = l.function_ref.as_ref().and_then(|(id, n)| self.resolve(*id, n));
            if l.function.is_none() {
                unbound.extend(l.function_ref.iter().map(|r| r.1.clone()));
            }
            l.handles.clear();
            for (&off, (id, n)) in &l.handle_refs {
                match self.resolve(*id, n) {
                    Some(k) => {
                        l.handles.insert(off, k);
                    }
                    None => unbound.push(n.clone()),
                }
            }
            (l.players, l.player_events) = player_events(&l.program, &l.handles);
        }
        if !unbound.is_empty() {
            unbound.sort();
            unbound.dedup();
            let text = format!("{}: unbound references {unbound:?}", bank.name);
            self.logf(text);
        }
        self.banks.push(bank);
        self.banks.len() - 1
    }

    pub fn id(&self, name: &str) -> Option<u32> {
        self.entries.iter().position(|e| e.name == name).map(|k| k as u32)
    }

    pub fn rng_state(&self) -> [u32; 6] {
        self.rng
    }

    pub fn set_rng_state(&mut self, state: [u32; 6]) {
        self.rng = state;
    }

    fn random(&mut self) -> u32 {
        let s = &mut self.rng;
        let (s4, c) = s[4].overflowing_add(s[5]);
        let add = |a: u32, b: u32, c: bool| -> (u32, bool) {
            let r = a.wrapping_add(b).wrapping_add(u32::from(c));
            (r, r < a)
        };
        let (s3, c) = add(s[3], s4, c);
        let (s2, c) = add(s[2], s3, c);
        let (s1, c) = add(s[1], s2, c);
        let (s0, _) = add(s[0], s1, c);
        *s = [s0, s1, s2, s3, s4, s[5].wrapping_add(1)];
        for k in (0..5).rev() {
            if s[k + 1] != 0 {
                break;
            }
            s[k] = s[k].wrapping_add(1);
        }
        s[0]
    }

    pub fn global(&self, id: u32) -> i32 {
        self.globals.get(&id).copied().unwrap_or(0)
    }

    pub fn set_global(&mut self, id: u32, value: i32) {
        if self.globals.get(&id) == Some(&value) {
            return;
        }
        self.globals.insert(id, value);
        if self.verbose {
            let name = self.name(id);
            self.logf(format!("global {name} = {value}"));
        }
        if self.watched.contains(&id) {
            self.global_changes.push((id, value));
        }
        for k in 0..self.instances.len() {
            let Some(inst) = self.instances[k].as_mut() else { continue };
            let links = &self.banks[inst.bank].links[inst.module];
            for (&off, &gid) in &links.handles {
                let m = &self.banks[inst.bank].abk.modules[inst.module];
                if gid == id && node_at(&m.program, off).is_some_and(|n| n.op == Op::GlobalRead) {
                    wr(&mut inst.state, off as usize + 0x18, value);
                }
            }
        }
    }

    pub fn fire_event(&mut self, id: u32, params: &[i32]) {
        self.fire_event_from(id, params, None);
    }

    pub fn fire_event_from(&mut self, id: u32, params: &[i32], origin: Option<u32>) {
        if self.verbose {
            let name = self.name(id);
            self.logf(format!("event {name} {params:?}"));
        }
        for k in 0..self.instances.len() {
            let Some(mut inst) = self.instances[k].take() else { continue };
            let bank = &self.banks[inst.bank];
            let mut got = false;
            for (&off, &eid) in &bank.links[inst.module].handles {
                let m = &bank.abk.modules[inst.module];
                if eid == id && node_at(&m.program, off).is_some_and(|n| n.op == Op::EventReceived) {
                    let o = off as usize;
                    let n = rb(&inst.state, o + 0x18) as usize;
                    for (j, &p) in params.iter().take(n).enumerate() {
                        wr(&mut inst.state, o + 0x1C + 4 * j, p);
                    }
                    wb(&mut inst.state, o + 0x19, 1);
                    got = true;
                }
            }
            if got {
                self.remember(&mut inst, id, origin);
            }
            self.instances[k] = Some(inst);
        }
    }

    pub fn watch_global(&mut self, id: u32) {
        if !self.watched.contains(&id) {
            self.watched.push(id);
        }
        self.global_changes.push((id, self.global(id)));
    }

    pub fn take_global_changes(&mut self) -> Vec<(u32, i32)> {
        std::mem::take(&mut self.global_changes)
    }

    pub fn take_fired(&mut self) -> Vec<(u32, Vec<i32>)> {
        std::mem::take(&mut self.fired)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tick_is_four_mixer_frames() {
        let (n, ms) = tick_frames();
        assert_eq!(n, 4);
        assert!((ms - 21.333).abs() < 1e-2, "{ms}");
    }

    #[test]
    fn sine_table_matches_the_image() {
        assert_eq!(
            [sine_table(0), sine_table(1), sine_table(8), sine_table(255), sine_table(256)],
            [0, 402, 3215, 65534, 65535]
        );
        assert_eq!(sine(256), 65535);
        assert_eq!(sine(512 + 8), -3215);
        assert_eq!(sine(1024), 0);
    }

    struct NoVoices;

    impl VoiceHost for NoVoices {
        fn start(&mut self, _: usize, _: &SampleEntry, _: &[(u8, i32)], _: Option<&Choice>) -> Option<u32> {
            None
        }
        fn set_param(&mut self, _: u32, _: u8, _: i32) {}
        fn destroy(&mut self, _: u32) {}
        fn pause(&mut self, _: u32) {}
        fn resume(&mut self, _: u32) {}
        fn status(&mut self, _: u32) -> Option<(i32, i32)> {
            None
        }
    }

    fn one_module_bank(function: u32) -> Bank {
        let module = ssx_formats::abk::Module {
            offset: 0,
            id: 1,
            max_instances: 1,
            num_globals: 0,
            num_events: 0,
            num_players: 0,
            has_stop: 0,
            has_params: 0,
            num_extra_slots: 0,
            script_offset: 0,
            data_offset: 0,
            data_size: INSTANCE_HEADER + 12,
            end_state: INSTANCE_HEADER,
            slots: Vec::new(),
            program: Vec::new(),
        };
        let abk = AbkFile {
            unk_04: [0; 6],
            file_size: 0,
            s10a_size: 0,
            ast_word: 0,
            s10a_offset: 0,
            section_a_offset: 0,
            section_b_offset: 0,
            section_c_offset: 0,
            modules: vec![module],
            sample_tables: Vec::new(),
            s10a: ssx_formats::abk::S10a { unk_04: 0, sounds: Vec::new() },
            section_a: Vec::new(),
            section_b: Vec::new(),
            names: Vec::new(),
            names_end: 0,
        };
        let links = vec![ModuleLinks { function: Some(function), ..Default::default() }];
        Bank { name: "wind".into(), abk, data: Arc::new(vec![0; (INSTANCE_HEADER + 12) as usize]), ast: None, links }
    }

    #[test]
    fn a_bank_kill_frees_the_slots_a_stopped_loop_holds() {
        let mut a = Aems::new();
        a.banks.push(one_module_bank(0x1234));
        let modules = |a: &Aems, h: CsisHandle| a.csis[h].as_ref().map_or(0, |c| c.modules.len());
        let first = a.call(0x1234, &[]);
        assert_eq!(modules(&a, first), 1);
        a.stop(first);
        let again = a.call(0x1234, &[]);
        assert_eq!(modules(&a, again), 0);
        a.kill_bank(0, &mut NoVoices);
        assert_eq!(a.live_instances(), 0);
        let after = a.call(0x1234, &[]);
        assert_eq!(modules(&a, after), 1);
        assert_eq!(a.live_instances(), 1);
    }

    #[test]
    fn rng_starts_from_the_zero_state() {
        let mut a = Aems::new();
        let got: Vec<u32> = (0..4).map(|_| a.random()).collect();
        assert_eq!(got, [0, 1, 7, 28]);
    }

    #[test]
    fn curve_interpolates_like_the_operator() {
        let mut d = vec![2u8, 0, 0, 3];
        d.extend_from_slice(&0i32.to_be_bytes());
        d.extend_from_slice(&100i32.to_be_bytes());
        d.extend_from_slice(&0.02f32.to_bits().to_be_bytes());
        for v in [0i16, 1000, 3000] {
            d.extend_from_slice(&v.to_be_bytes());
        }
        assert_eq!(curve(&d, 0, 0), 0);
        assert_eq!(curve(&d, 0, 25), 500);
        assert_eq!(curve(&d, 0, 75), 2000);
        assert_eq!(curve(&d, 0, 500), 3000);
    }

    #[test]
    fn glide_reaches_its_target() {
        let mut s = vec![0u8; 28];
        wr(&mut s, 0x10, 100);
        wr(&mut s, 0x14, 4096);
        wr(&mut s, 0x18, 1000);
        let tick = 21.333;
        let first = glide(&mut s, 0, tick);
        assert_eq!(first, 213);
        let mut v = first;
        for _ in 0..10 {
            v = glide(&mut s, 0, tick);
        }
        assert_eq!(v, 1000);
    }

    #[test]
    fn timer_fires_once() {
        let mut s = vec![0u8; 16];
        wrf(&mut s, 0, -1.0);
        wr(&mut s, 0xC, 40);
        assert_eq!(round(-0.5), -1);
        wr(&mut s, 8, 1);
        let mut fired = Vec::new();
        for k in 0..5 {
            fired.push(timer_step(&mut s, 21.333));
            if k == 0 {
                wr(&mut s, 8, 0);
            }
        }
        assert_eq!(fired, [0, 0, 1, 0, 0]);
    }

    fn timer_step(s: &mut [u8], tick: f32) -> i32 {
        if rd(s, 8) != 0 {
            wrf(s, 0, 0.0);
        } else if rdf(s, 0) < 0.0 {
            return 0;
        }
        let t = rdf(s, 0);
        if t >= rd(s, 0xC) as f32 {
            wrf(s, 0, -1.0);
            return 1;
        }
        wrf(s, 0, t + tick);
        0
    }
}
