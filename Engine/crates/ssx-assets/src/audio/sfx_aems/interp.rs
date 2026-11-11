use super::*;

pub(super) fn node_at(program: &[Node], off: u32) -> Option<&Node> {
    program.iter().find(|n| n.state == off)
}

impl Aems {
    pub fn tick(&mut self, host: &mut dyn VoiceHost) {
        let keys = self.order.clone();
        for k in keys {
            let Some(mut inst) = self.instances.get_mut(k).and_then(Option::take) else { continue };
            if self.run(&mut inst, host) {
                self.instances[k] = Some(inst);
            } else {
                self.release(k, &inst);
            }
        }
        self.now_ms += f64::from(self.tick_ms);
        self.ticks += 1;
    }

    fn destroy_players(&self, bank: usize, module: usize, s: &[u8], host: &mut dyn VoiceHost) {
        let m = &self.banks[bank].abk.modules[module];
        for &slot in &m.slots[..m.num_players as usize] {
            let v = rd(s, (slot - INSTANCE_HEADER) as usize + 8);
            if v != 0 {
                host.destroy(v as u32);
            }
        }
    }

    fn release(&mut self, k: usize, inst: &Instance) {
        self.order.retain(|&x| x != k);
        if let Some(n) = self.live.get_mut(&(inst.bank, inst.module)) {
            *n = n.saturating_sub(1);
        }
        if let Some(c) = self.csis.get_mut(inst.csis).and_then(Option::as_mut) {
            c.modules.retain(|&x| x != k);
        }
        if self.verbose {
            let text = format!("  {} module {} instance {k} ended", self.banks[inst.bank].name, inst.module);
            self.logf(text);
        }
    }

    pub fn kill_bank(&mut self, bank: usize, host: &mut dyn VoiceHost) {
        let keys: Vec<usize> = self
            .order
            .iter()
            .copied()
            .filter(|&k| self.instances[k].as_ref().is_some_and(|i| i.bank == bank))
            .collect();
        if !keys.is_empty() {
            let text = format!("kill {}: {} instance(s)", self.banks[bank].name, keys.len());
            self.logf(text);
        }
        for k in keys {
            let Some(inst) = self.instances[k].take() else { continue };
            self.destroy_players(inst.bank, inst.module, &inst.state, host);
            self.release(k, &inst);
        }
    }

    fn run(&mut self, inst: &mut Instance, host: &mut dyn VoiceHost) -> bool {
        let program = self.banks[inst.bank].links[inst.module].program.clone();
        let mut alive = true;
        for n in program.iter() {
            let st = n.state as usize;
            let r = self.exec(n, inst, st, host, &mut alive);
            for w in &n.wires {
                let v = w.src.map_or(r, |s| rd(&inst.state, (st as i64 + s as i64) as usize));
                wr(&mut inst.state, (st as i64 + w.dst as i64) as usize, v);
            }
        }
        alive
    }

    fn notify_self(&self, inst: &mut Instance, id: u32, global: Option<i32>, params: &[i32]) {
        let links = &self.banks[inst.bank].links[inst.module];
        for (&off, &hid) in &links.handles {
            if hid != id {
                continue;
            }
            let Some(n) = node_at(&links.program, off) else { continue };
            let o = off as usize;
            match (n.op, global) {
                (Op::GlobalRead, Some(v)) => wr(&mut inst.state, o + 0x18, v),
                (Op::EventReceived, None) => {
                    let k = rb(&inst.state, o + 0x18) as usize;
                    for (j, &p) in params.iter().take(k).enumerate() {
                        wr(&mut inst.state, o + 0x1C + 4 * j, p);
                    }
                    wb(&mut inst.state, o + 0x19, 1);
                    self.remember(inst, id, None);
                }
                _ => {}
            }
        }
    }

    fn exec(&mut self, n: &Node, inst: &mut Instance, st: usize, host: &mut dyn VoiceHost, alive: &mut bool) -> i32 {
        let tick = self.tick_ms;
        let s = &mut inst.state;
        let r = |s: &[u8], x: usize| rd(s, st + x);
        match n.op {
            Op::Stopped => {
                let v = r(s, 0x10);
                wr(s, st + 0x10, 0);
                v
            }
            Op::Params => r(s, 0x14),
            Op::GlobalRead => r(s, 0x18),
            Op::Start => {
                let v = r(s, 0);
                wr(s, st, 0);
                v
            }
            Op::End => {
                if r(s, 0xC) != 0 {
                    self.destroy_players(inst.bank, inst.module, s, host);
                    *alive = false;
                }
                0
            }
            Op::FireEvent => {
                let (clamped, k) = (rb(s, st + 8) != 0, rb(s, st + 9) as usize);
                let mut p = st + 0xC;
                if clamped {
                    let pairs = p;
                    p += 8 * k;
                    for j in 0..k {
                        let v = clamp(rd(s, p + 4 + 4 * j), rd(s, pairs + 8 * j), rd(s, pairs + 8 * j + 4));
                        wr(s, p + 4 + 4 * j, v);
                    }
                }
                if rd(s, p) != 0 {
                    let params: Vec<i32> = (0..k).map(|j| rd(s, p + 4 + 4 * j)).collect();
                    if let Some(&id) = self.banks[inst.bank].links[inst.module].handles.get(&n.state) {
                        if self.verbose {
                            let name = self.name(id);
                            self.logf(format!("  fire {name} {params:?}"));
                        }
                        self.fire_event(id, &params);
                        self.notify_self(inst, id, None, &params);
                        self.fired.push((id, params));
                    }
                }
                0
            }
            _ => self.exec_value(n, inst, st, tick, host),
        }
    }
}

impl Aems {
    fn exec_value(&mut self, n: &Node, inst: &mut Instance, st: usize, tick: f32, host: &mut dyn VoiceHost) -> i32 {
        let s = &mut inst.state;
        let r = |s: &[u8], x: usize| rd(s, st + x);
        let count = |s: &[u8]| rb(s, st) as usize;
        match n.op {
            Op::Stepper => {
                let (x, min, max) = (r(s, 0x14), r(s, 0), r(s, 4));
                if x >= min && x <= max {
                    return x;
                }
                if r(s, 0x10) > 0 {
                    let cur = r(s, 8).wrapping_add(i32::from(rb(s, st + 0xC) as i8));
                    wr(s, st + 8, cur);
                    if cur > max {
                        wr(s, st + 8, min);
                        return min;
                    }
                    if cur < min {
                        wr(s, st + 8, max);
                    }
                }
                r(s, 8)
            }
            Op::Random => {
                if r(s, 0xC) == 0 {
                    return r(s, 8);
                }
                let range = r(s, 4) as u32;
                if range == 0 {
                    return r(s, 8);
                }
                let v = (self.random() % range).wrapping_add(r(s, 0) as u32) as i32;
                wr(&mut inst.state, st + 8, v);
                v
            }
            Op::Shuffle => {
                let trig = rh(s, st) as usize;
                if r(s, trig) == 0 {
                    return r(s, 0xC);
                }
                let (width, wrapped, pos, k) = (rb(s, st + 2), rb(s, st + 3) as i8, rh(s, st + 8), rh(s, st + 10));
                let range = (i32::from(k) - i32::from(wrapped) - i32::from(pos)) as u32;
                if range == 0 {
                    return r(s, 0xC);
                }
                let idx = usize::from(pos) + (self.random() % range) as usize;
                let s = &mut inst.state;
                let t = st + 0x10;
                let v = if width == 1 {
                    let v = rb(s, t + idx);
                    let at_pos = rb(s, t + usize::from(pos));
                    wb(s, t + idx, at_pos);
                    wb(s, t + usize::from(pos), v);
                    i32::from(v)
                } else {
                    let v = rh(s, t + 2 * idx);
                    let at_pos = rh(s, t + 2 * usize::from(pos));
                    wh(s, t + 2 * idx, at_pos);
                    wh(s, t + 2 * usize::from(pos), v);
                    i32::from(v)
                };
                let out = r(s, 4).wrapping_add(v);
                wr(s, st + 0xC, out);
                if pos + 1 < k {
                    wh(s, st + 8, pos + 1);
                    wb(s, st + 3, 0);
                } else {
                    wh(s, st + 8, 0);
                    wb(s, st + 3, 1);
                }
                out
            }
            Op::WeightedRandom => {
                if r(s, 0x10) != 0 {
                    let table = r(s, 0) as usize + 0x10;
                    let roll = self.random() % 100;
                    let data = self.banks[inst.bank].data.clone();
                    let s = &mut inst.state;
                    let mut cum = 0i32;
                    for i in 0..r(s, 8).max(0) {
                        cum += i32::from(data.get(table + i as usize).copied().unwrap_or(0) as i8);
                        if roll < cum as u32 {
                            let v = r(s, 4).wrapping_add(i);
                            wr(s, st + 0xC, v);
                            break;
                        }
                    }
                }
                r(&inst.state, 0xC)
            }
            Op::RangeTrigger => {
                let x = r(s, 0x14);
                if x >= r(s, 0) && x <= r(s, 4) {
                    if rb(s, st + 0x10) == 0 {
                        wb(s, st + 0x10, 1);
                        wb(s, st + 0x11, 1);
                        return 1;
                    }
                } else if x >= r(s, 8) && x <= r(s, 0xC) {
                    wb(s, st + 0x10, 0);
                }
                wb(s, st + 0x11, 0);
                0
            }
            Op::Timer => {
                if r(s, 8) != 0 {
                    wrf(s, st, 0.0);
                } else if rdf(s, st) < 0.0 {
                    wb(s, st + 4, 0);
                    return 0;
                }
                let t = rdf(s, st);
                if t >= r(s, 0xC) as f32 {
                    wb(s, st + 4, 1);
                    wrf(s, st, -1.0);
                    return 1;
                }
                wrf(s, st, t + tick);
                wb(s, st + 4, 0);
                0
            }
            Op::SelectOnTrigger => {
                let (off, k) = (rh(s, st) as usize, rb(s, st + 2) as usize);
                if let Some(j) = (0..k).find(|&j| r(s, off + 4 * j) != 0) {
                    let v = r(s, 8 + 4 * j);
                    wr(s, st + 4, v);
                }
                r(s, 4)
            }
            Op::AnyOf => i32::from((0..count(s)).any(|j| r(s, 4 + 4 * j) != 0)),
            Op::Envelope => envelope(s, st, tick),
            Op::Curve => {
                let x = r(s, 0xC);
                if x == r(s, 4) {
                    return r(s, 8);
                }
                wr(s, st + 4, x);
                let v = curve(&self.banks[inst.bank].data, r(s, 0) as usize, x);
                wr(s, st + 8, v);
                v
            }
            Op::Delay => delay(s, st, tick),
            _ => self.exec_arith(n, inst, st, tick, host),
        }
    }
}

impl Aems {
    fn exec_arith(&mut self, n: &Node, inst: &mut Instance, st: usize, tick: f32, host: &mut dyn VoiceHost) -> i32 {
        let s = &mut inst.state;
        let r = |s: &[u8], x: usize| rd(s, st + x);
        let k = rb(s, st) as usize;
        fn inputs(s: &[u8], st: usize, k: usize, from: usize) -> impl Iterator<Item = i32> + '_ {
            (0..k.max(1)).map(move |j| rd(s, st + from + 4 * j))
        }
        match n.op {
            Op::Switch => {
                let i = r(s, 4);
                if i > 0 && i as usize <= k { r(s, 4 + 4 * i as usize) } else { 0 }
            }
            Op::Demux => {
                let last = rh(s, st + 2) as i16 as i32;
                wr(s, (st as i64 + i64::from(last + 2) * 4) as usize, 0);
                let i = r(s, 4);
                if i > 0 && i as usize <= k {
                    let v = r(s, 8);
                    wr(s, st + (i as usize + 2) * 4, v);
                    wh(s, st + 2, i as u16);
                }
                r(s, 0xC)
            }
            Op::MinOf => inputs(s, st, k, 4).min().unwrap_or(0),
            Op::MaxOf => inputs(s, st, k, 4).max().unwrap_or(0),
            Op::SumOf => inputs(s, st, k, 4).fold(0i32, i32::wrapping_add),
            Op::ProductOf => {
                let p = inputs(s, st, k, 8).map(|v| v as f32).reduce(|a, b| a * b).unwrap_or(0.0);
                round(rdf(s, st + 4) * p)
            }
            Op::SumMax => inputs(s, st, k, 8).fold(0i32, i32::wrapping_add).min(r(s, 4)),
            Op::Sub => r(s, 0).wrapping_sub(r(s, 4)),
            Op::Mul => r(s, 0).wrapping_mul(r(s, 4)),
            Op::Div => {
                let d = r(s, 4);
                if d == 0 { 0 } else { r(s, 0).wrapping_div(d) }
            }
            Op::Mod => {
                let d = r(s, 4);
                if d == 0 { 0 } else { r(s, 0).wrapping_rem(d) }
            }
            Op::Add => r(s, 0).wrapping_add(r(s, 4)),
            Op::Min => r(s, 0).min(r(s, 4)),
            Op::Max => r(s, 0).max(r(s, 4)),
            Op::SubFloor => r(s, 4).wrapping_sub(r(s, 8)).max(r(s, 0)),
            Op::MulCeiling => r(s, 8).wrapping_mul(r(s, 4)).min(r(s, 0)),
            Op::Scale => round(r(s, 8) as f32 * r(s, 4) as f32 * rdf(s, st)),
            Op::Lfo => lfo(s, st, tick),
            Op::Glide => glide(s, st, tick),
            Op::EventReceived => {
                let v = i32::from(rb(s, st + 0x19));
                wb(s, st + 0x19, 0);
                v
            }
            Op::GlobalWrite => {
                let x = r(s, 0x14);
                if x != r(s, 0x10) {
                    wr(s, st + 0x10, x);
                    let v = clamp(x, r(s, 8), r(s, 0xC));
                    if let Some(&id) = self.banks[inst.bank].links[inst.module].handles.get(&n.state) {
                        self.set_global(id, v);
                        self.notify_self(inst, id, Some(v), &[]);
                    }
                }
                0
            }
            Op::Player => self.player(inst, st, host),
            _ => 0,
        }
    }
}
