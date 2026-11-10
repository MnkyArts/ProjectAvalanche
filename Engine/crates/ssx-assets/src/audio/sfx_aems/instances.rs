use super::*;

pub type CsisHandle = usize;

impl Aems {
    pub fn call(&mut self, function: u32, params: &[i32]) -> CsisHandle {
        self.call_from(function, params, None)
    }

    pub fn call_from(&mut self, function: u32, params: &[i32], origin: Option<u32>) -> CsisHandle {
        let key = match self.csis.iter().skip(1).position(Option::is_none) {
            Some(i) => i + 1,
            None => {
                if self.csis.is_empty() {
                    self.csis.push(None);
                }
                self.csis.push(None);
                self.csis.len() - 1
            }
        };
        self.csis[key] = Some(CsisInstance { function, modules: Vec::new(), origin });
        let mut created = 0;
        for b in (0..self.banks.len()).rev() {
            for m in (0..self.banks[b].links.len()).rev() {
                if self.banks[b].links[m].function == Some(function) && self.create(b, m, key, params) {
                    created += 1;
                }
            }
        }
        let name = self.name(function);
        self.logf(format!("call {name} {params:?} → {created} instance(s)"));
        key
    }

    fn create(&mut self, b: usize, m: usize, csis: usize, params: &[i32]) -> bool {
        let module = &self.banks[b].abk.modules[m];
        let live = self.live.entry((b, m)).or_default();
        if *live >= module.max_instances {
            if self.verbose {
                let text = format!("  {} module {m}: {} live, max reached", self.banks[b].name, *live);
                self.logf(text);
            }
            return false;
        }
        *live += 1;
        let data = &self.banks[b].data;
        let from = (module.data_offset + INSTANCE_HEADER) as usize;
        let mut state = data[from..(module.data_offset + module.data_size) as usize].to_vec();
        let end = (module.end_state - INSTANCE_HEADER) as usize;
        wr(&mut state, end, module.offset as i32);
        let key = self.instances.iter().position(Option::is_none).unwrap_or(self.instances.len());
        wr(&mut state, end + 4, key as i32);
        wr(&mut state, end + 8, csis as i32);
        let links = &self.banks[b].links[m];
        for n in links.program.iter() {
            match n.op {
                Op::GlobalRead => {
                    if let Some(v) = links.handles.get(&n.state).and_then(|id| self.globals.get(id)) {
                        wr(&mut state, n.state as usize + 0x18, *v);
                    }
                }
                Op::Params => {
                    let o = n.state as usize;
                    let count = rb(&state, o + 0x10) as usize;
                    for (j, &p) in params.iter().take(count).enumerate() {
                        wr(&mut state, o + 0x14 + 4 * j, p);
                    }
                }
                _ => {}
            }
        }
        if key == self.instances.len() {
            self.instances.push(None);
        }
        let script = self.csis[csis].as_ref().and_then(|c| c.origin).filter(|o| self.script_routes.contains_key(o));
        self.instances[key] =
            Some(Instance { bank: b, module: m, state, csis, received: Vec::new(), started: HashMap::new(), script });
        self.order.insert(0, key);
        if let Some(c) = self.csis[csis].as_mut() {
            c.modules.push(key);
        }
        if self.verbose {
            let text = format!("  {} module {m} (id {:#x}) instance {key}", self.banks[b].name, module.id);
            self.logf(text);
        }
        true
    }

    pub fn update(&mut self, handle: CsisHandle, params: &[i32]) {
        let Some(c) = self.csis.get(handle).and_then(Option::as_ref) else { return };
        for &k in &c.modules {
            let Some(inst) = self.instances.get_mut(k).and_then(Option::as_mut) else { continue };
            let program = &self.banks[inst.bank].links[inst.module].program;
            for n in program.iter().filter(|n| n.op == Op::Params) {
                let o = n.state as usize;
                let count = rb(&inst.state, o + 0x10) as usize;
                for (j, &p) in params.iter().take(count).enumerate() {
                    wr(&mut inst.state, o + 0x14 + 4 * j, p);
                }
            }
        }
    }

    pub fn discard_if_empty(&mut self, handle: CsisHandle) -> bool {
        let empty = self.csis.get(handle).and_then(Option::as_ref).is_some_and(|c| c.modules.is_empty());
        if empty {
            self.csis[handle] = None;
        }
        empty
    }

    pub fn stop(&mut self, handle: CsisHandle) {
        let Some(c) = self.csis.get_mut(handle).and_then(Option::take) else { return };
        let name = self.name(c.function);
        self.logf(format!("stop {name}"));
        for &k in &c.modules {
            let Some(inst) = self.instances.get_mut(k).and_then(Option::as_mut) else { continue };
            let program = &self.banks[inst.bank].links[inst.module].program;
            if let Some(n) = program.iter().find(|n| n.op == Op::Stopped) {
                wr(&mut inst.state, n.state as usize + 0x10, 1);
            }
        }
    }

    pub fn live_instances(&self) -> usize {
        self.order.len()
    }
}
