use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use ssx_formats::speech::ctx::{Context, Contexts, Value};
use ssx_formats::speech::evt::EventSystem;
use ssx_formats::speech::gas::{GasFile, Script};

use super::vars::Vars;
use super::vm::{self, VmHost};

#[derive(Clone, Debug)]
pub struct Lcg(pub u32);

impl Lcg {
    pub fn seeded(seed: u32) -> Self {
        Lcg(if seed == 0 { 0xAAAA_AAAA } else { seed })
    }

    pub fn below(&mut self, n: u32) -> u32 {
        let x = u64::from(self.0) * 0x41C6_4E6D + 0x3039;
        self.0 = x as u32;
        ((((x >> 16) & 0xFFFF_FFFF) * u64::from(n)) >> 32) as u32
    }
}

#[derive(Clone, Debug)]
pub struct SpeechRandom(Arc<Mutex<Lcg>>);

impl SpeechRandom {
    pub fn boot() -> Self {
        Self(Arc::new(Mutex::new(Lcg(crate::career::random::time_base()))))
    }

    pub fn seeded(seed: u32) -> Self {
        Self(Arc::new(Mutex::new(Lcg::seeded(seed))))
    }

    pub fn with<R>(&self, f: impl FnOnce(&mut Lcg) -> R) -> R {
        f(&mut self.0.lock().unwrap_or_else(|e| e.into_inner()))
    }
}

#[derive(Clone, Debug)]
struct Entry {
    time: u32,
    params: Option<Arc<Vec<i32>>>,
    instance: u64,
    ctx: usize,
    active: bool,
    interrupts: bool,
    triggered: bool,
    priority: u32,
    probability: u32,
    window: u32,
}

type Chosen = (usize, Option<Arc<Vec<i32>>>, u64);

#[derive(Clone, Debug, Default)]
pub struct InterruptGroup {
    pub voices: Vec<String>,
    pub busy: bool,
    pub priority: u32,
}

#[derive(Clone, Debug)]
pub struct Fire {
    pub context: String,
    pub event: String,
    pub event_id: u32,
    pub params: Vec<i32>,
}

pub struct ContextSystem {
    pub data: Contexts,
    scripts: HashMap<u32, Script>,
    by_trigger: HashMap<String, Vec<usize>>,
    trigger_ids: HashMap<String, u32>,
    passives: Vec<Entry>,
    triggered: Vec<Entry>,
    chosen: Vec<Chosen>,
    pub groups: Vec<InterruptGroup>,
    playing: u32,
    any_busy: bool,
    pub passives_enabled: bool,
    stats: HashMap<(u8, u32), i32>,
    next_instance: u64,
}

struct Host<'a> {
    vars: &'a Vars,
    stats: &'a HashMap<(u8, u32), i32>,
    now: u32,
    this: u32,
    groups: usize,
}

impl Host<'_> {
    fn stamp(&self, kind: u8, id: u32) -> i32 {
        self.stats.get(&(kind, id)).copied().unwrap_or(i32::MAX)
    }

    fn since(&self, kind: u8, id: u32) -> i32 {
        match self.stamp(kind, id) {
            i32::MAX => i32::MAX,
            s => self.now.wrapping_sub(s as u32) as i32,
        }
    }

    fn count(&self, kind: u8, id: u32) -> i32 {
        self.stats.get(&(kind, id)).copied().unwrap_or(0)
    }
}

impl VmHost for Host<'_> {
    fn game_var(&self, id: i32) -> i32 {
        self.vars.get(id)
    }

    fn stat(&self, call: i16, id: u32) -> i32 {
        let group = (id as usize) < self.groups;
        match call {
            0x1F6 => self.stamp(0, id),
            0x1F7 => self.count(1, id),
            0x1F8 => self.since(0, id),
            0x200 if group => self.stamp(3, id),
            0x201 if group => self.count(4, id),
            0x202 if group => match self.since(3, id) {
                0 => i32::MAX,
                v => v,
            },
            0x20A => self.stamp(6, id),
            0x20B => self.count(7, id),
            0x20C => self.since(6, id),
            _ => 0,
        }
    }

    fn this_context(&self) -> u32 {
        self.this
    }
}

impl ContextSystem {
    pub fn new(data: Contexts, gas: GasFile, events: &EventSystem, groups: Vec<Vec<String>>) -> anyhow::Result<Self> {
        if let Some(c) = data.contexts.iter().find(|c| c.param_conditions.iter().any(|pc| (2..=5).contains(&pc.op))) {
            anyhow::bail!("context {}: an ordered parameter condition (none on the disc)", c.name);
        }
        let mut by_trigger: HashMap<String, Vec<usize>> = HashMap::new();
        let mut passives = Vec::new();
        for (i, c) in data.contexts.iter().enumerate() {
            match &c.trigger {
                Some(t) => by_trigger.entry(t.name.clone()).or_default().push(i),
                None if passives.len() < 200 => passives.push(Self::entry(c, i, 0, None, 0)),
                None => {}
            }
        }
        let trigger_ids = events.events.iter().map(|e| (e.name.clone(), e.id & 0xFFFF)).collect();
        Ok(Self {
            scripts: gas.scripts.into_iter().map(|s| (s.id, s)).collect(),
            data,
            by_trigger,
            trigger_ids,
            passives,
            triggered: Vec::new(),
            chosen: Vec::new(),
            groups: groups.into_iter().map(|voices| InterruptGroup { voices, ..Default::default() }).collect(),
            playing: 0,
            any_busy: false,
            passives_enabled: true,
            stats: HashMap::new(),
            next_instance: 1,
        })
    }

    fn entry(c: &Context, ctx: usize, time: u32, params: Option<Arc<Vec<i32>>>, instance: u64) -> Entry {
        let triggered = c.trigger.as_ref().is_some_and(|t| t.event & 0xFFFF != 0);
        Entry {
            time,
            params,
            instance,
            ctx,
            active: true,
            interrupts: c.flags & 1 != 0,
            triggered,
            priority: c.priority,
            probability: u32::from(c.probability),
            window: u32::from(if triggered { c.unk_10 } else { c.unk_12 }),
        }
    }

    pub fn trigger(&mut self, name: &str, params: &[i32], now: u32) -> usize {
        if let Some(&id) = self.trigger_ids.get(name) {
            self.stats.insert((6, id), now as i32);
            *self.stats.entry((7, id)).or_insert(0) += 1;
        }
        let Some(list) = self.by_trigger.get(name) else { return 0 };
        let params = Arc::new(params.to_vec());
        let instance = self.next_instance;
        self.next_instance += 1;
        let mut n = 0;
        for &i in list {
            if self.triggered.len() < 300 {
                self.triggered.push(Self::entry(&self.data.contexts[i], i, now, Some(params.clone()), instance));
                n += 1;
            }
        }
        n
    }

    pub fn set_busy(&mut self, voice: &str, busy: bool) {
        for g in self.groups.iter_mut().filter(|g| g.voices.iter().any(|v| v == voice)) {
            g.busy = busy;
        }
        self.any_busy = busy;
    }
}

impl ContextSystem {
    pub fn update(
        &mut self,
        now: u32,
        vars: &Vars,
        rng: &mut Lcg,
        fire: &mut dyn FnMut(&Fire, &mut Lcg) -> bool,
    ) -> Option<Fire> {
        for g in self.groups.iter_mut().filter(|g| !g.busy) {
            g.priority = 0;
        }
        if !self.any_busy {
            self.playing = 0;
        }
        let mut fired = None;
        if !self.chosen.is_empty() {
            let k = rng.below(self.chosen.len() as u32) as usize;
            let (ci, params, instance) = self.chosen[k].clone();
            let f = self.action(ci, params.as_ref().map(|p| p.as_slice()), vars, now);
            if fire(&f, rng) {
                let c = &self.data.contexts[ci];
                self.playing = c.priority;
                if let Some(g) = self.groups.get_mut(usize::from(c.interrupt_group)) {
                    g.priority = c.priority;
                }
                for e in &mut self.triggered {
                    if e.instance == instance || (e.window != u32::MAX && e.time.wrapping_add(e.window) < now) {
                        e.active = false;
                    }
                }
                self.played(ci, now);
                self.chosen.clear();
                fired = Some(f);
            } else {
                self.chosen.remove(k);
            }
        } else {
            self.evaluate(now, vars, rng);
        }
        for e in &mut self.triggered {
            if e.window != u32::MAX && e.time.wrapping_add(e.window) < now {
                e.active = false;
            }
        }
        self.triggered.retain(|e| e.active);
        fired
    }

    fn played(&mut self, ci: usize, now: u32) {
        let c = &self.data.contexts[ci];
        self.stats.insert((0, c.id), now as i32);
        let count = {
            let n = self.stats.entry((1, c.id)).or_insert(0);
            *n += 1;
            *n
        };
        for &g in &c.groups {
            let id = self.data.groups.get(g).copied().unwrap_or(g as u32);
            self.stats.insert((3, id), now as i32);
            let gc = self.stats.entry((4, id)).or_insert(0);
            if *gc < count {
                *gc = count;
            }
        }
    }

    fn evaluate(&mut self, now: u32, vars: &Vars, rng: &mut Lcg) {
        let mut queue: Vec<Entry> = self.triggered.iter().filter(|e| e.active).cloned().collect();
        if self.passives_enabled {
            queue.extend(self.passives.iter().cloned());
        }
        queue.sort_by_key(|e| std::cmp::Reverse(e.priority));
        let mut best = self.playing;
        for e in queue {
            let c = &self.data.contexts[e.ctx];
            let g = usize::from(c.interrupt_group);
            let threshold = self.groups.get(g).map_or(best, |g| g.priority);
            if threshold <= e.priority && !self.groups.get(g).is_some_and(|g| g.busy && !e.interrupts) {
                let pass = if !e.triggered {
                    rng.below(e.window) == 0
                } else {
                    !(e.window != u32::MAX && e.time.wrapping_add(e.window) < now)
                };
                if pass {
                    if rng.below(100) < e.probability
                        && self.valid(e.ctx, e.params.as_ref().map(|p| p.as_slice()), vars, now)
                    {
                        let p = c.priority;
                        if p == best {
                            self.chosen.push((e.ctx, e.params.clone(), e.instance));
                        } else if best < p {
                            self.chosen.clear();
                            self.chosen.push((e.ctx, e.params.clone(), e.instance));
                            best = p;
                            if let Some(gr) = self.groups.get_mut(g) {
                                gr.priority = p;
                            }
                        }
                    }
                    if e.triggered {
                        for t in self.triggered.iter_mut().filter(|t| t.instance == e.instance && t.ctx == e.ctx) {
                            t.active = false;
                        }
                    }
                }
            }
        }
    }

    fn value(&self, v: &Value, params: Option<&[i32]>, vars: &Vars, now: u32, this: u32) -> i32 {
        match v {
            Value::TriggerParam(k) => params.and_then(|p| p.get(*k).copied()).unwrap_or(*k as i32 + 1),
            Value::Script { script, args } => {
                let a: Vec<i32> = args.iter().map(|x| self.value(x, params, vars, now, this)).collect();
                self.run(*script, &a, vars, now, this)
            }
        }
    }

    fn run(&self, script: u32, args: &[i32], vars: &Vars, now: u32, this: u32) -> i32 {
        let Some(s) = self.scripts.get(&script) else { return 0 };
        let host = Host { vars, stats: &self.stats, now, this, groups: self.data.groups.len() };
        vm::run(s, args, &host)
    }

    fn valid(&self, ci: usize, params: Option<&[i32]>, vars: &Vars, now: u32) -> bool {
        let c = &self.data.contexts[ci];
        for pc in &c.param_conditions {
            let v = if pc.source == 0 {
                params.and_then(|p| p.get(pc.index as usize).copied()).unwrap_or(-1)
            } else {
                vars.get(pc.index as i32)
            };
            let k = pc.value;
            let ok = match pc.op {
                0 => v == k,
                1 => v != k,
                6 => v & k != 0,
                _ => false,
            };
            if !ok {
                return false;
            }
        }
        c.conditions.iter().all(|k| {
            let args: Vec<i32> = k.args.iter().map(|a| self.value(a, params, vars, now, c.id)).collect();
            self.run(k.script, &args, vars, now, c.id) != 0
        })
    }

    fn action(&self, ci: usize, params: Option<&[i32]>, vars: &Vars, now: u32) -> Fire {
        let c = &self.data.contexts[ci];
        Fire {
            context: c.name.clone(),
            event: c.action.name.clone(),
            event_id: c.action.event,
            params: c.action.params.iter().map(|v| self.value(v, params, vars, now, 0)).collect(),
        }
    }

    pub fn stat(&self, kind: u8, id: u32) -> Option<i32> {
        self.stats.get(&(kind, id)).copied()
    }
}

impl ContextSystem {
    pub fn explain(&self, trigger: &str, params: &[i32], vars: &Vars, now: u32) -> Vec<(String, u32, Option<String>)> {
        let list = self.by_trigger.get(trigger).cloned().unwrap_or_default();
        list.into_iter()
            .map(|i| {
                let c = &self.data.contexts[i];
                let fail = c.conditions.iter().find(|k| {
                    let args: Vec<i32> = k.args.iter().map(|a| self.value(a, Some(params), vars, now, c.id)).collect();
                    self.run(k.script, &args, vars, now, c.id) == 0
                });
                (c.name.clone(), c.priority, fail.map(|k| k.name.clone()))
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_speech_rng_is_the_games_lcg() {
        let mut g = Lcg(0);
        assert_eq!(g.below(100), 0);
        assert_eq!(g.0, 0x3039);
        let x = 0x3039u64 * 0x41C6_4E6D + 0x3039;
        assert_eq!(g.below(100), ((((x >> 16) & 0xFFFF_FFFF) * 100) >> 32) as u32);
    }

    #[test]
    fn the_speech_rng_is_seeded_once_and_shared() {
        assert_eq!(Lcg::seeded(0).0, 0xAAAA_AAAA);
        assert_eq!(Lcg::seeded(7).0, 7);
        let (a, mut g) = (SpeechRandom::seeded(7), Lcg::seeded(7));
        let b = a.clone();
        let drawn = [a.with(|r| r.below(100)), b.with(|r| r.below(100)), a.with(|r| r.below(100))];
        assert_eq!(drawn, [g.below(100), g.below(100), g.below(100)]);
    }

    #[test]
    fn statistics_of_what_never_played() {
        let (vars, stats) = (Vars::default(), HashMap::new());
        let h = Host { vars: &vars, stats: &stats, now: 5000, this: 3, groups: 25 };
        assert_eq!(h.stat(0x1F8, 3), i32::MAX);
        assert_eq!(h.stat(0x1F7, 3), 0);
        assert_eq!(h.stat(0x202, 4), i32::MAX);
        assert_eq!(h.stat(0x201, 99), 0);
    }
}
