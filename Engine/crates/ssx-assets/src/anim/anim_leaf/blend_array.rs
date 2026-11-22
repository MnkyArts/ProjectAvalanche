use super::*;

pub struct BlendArrayInst {
    ctrl: CtrlId,
    cur: f32,
    items: Vec<Option<Box<dyn Controller>>>,
    active: Vec<(usize, f32)>,
    phase: f32,
    master: Option<Box<dyn Controller>>,
    exited: bool,
}

struct ArrayData<'a> {
    signal: Option<SigId>,
    clamp: [f32; 2],
    damping: [f32; 2],
    items: &'a [crate::anim_graph::BlendItem],
    range: [f32; 2],
    master: Option<CtrlId>,
}

fn array_data<'a>(env: &Env<'a>, ctrl: CtrlId) -> Option<ArrayData<'a>> {
    match &env.g.ctrls[ctrl].1 {
        Ctrl::BlendArray { signal, clamp, damping, items, range, master, .. } => {
            Some(ArrayData { signal: *signal, clamp: *clamp, damping: *damping, items, range: *range, master: *master })
        }
        _ => None,
    }
}

pub(crate) fn select(xs: &[f32], pos: f32) -> Vec<(usize, f32)> {
    let n = xs.len();
    if n == 0 {
        return Vec::new();
    }
    if n == 1 || pos <= xs[0] {
        return vec![(0, 1.0)];
    }
    if pos >= xs[n - 1] {
        return vec![(n - 1, 1.0)];
    }
    for i in 0..n - 1 {
        let (a, b) = (xs[i], xs[i + 1]);
        if a - 0.001 <= pos && pos <= b + 0.001 {
            if (pos - a).abs() < 0.001 {
                return vec![(i, 1.0)];
            }
            let t = (pos - a) / (b - a);
            return if t < 0.03 {
                vec![(i, 1.0)]
            } else if t > 0.97 {
                vec![(i + 1, 1.0)]
            } else {
                vec![(i, 1.0 - t), (i + 1, t)]
            };
        }
    }
    vec![(n - 1, 1.0)]
}

pub(crate) fn damp(cur: f32, target: f32, clamp: [f32; 2], damping: [f32; 2], dt: f32) -> f32 {
    let diff = target - cur;
    let rate = if cur * diff < 0.0 { damping[1] } else { damping[0] };
    let max = rate * dt;
    let step = if max - diff.abs() >= 0.0 { diff.abs() } else { max };
    let cur = cur + if target >= cur { step } else { -step };
    let lo = if clamp[0] - cur >= 0.0 { clamp[0] } else { cur };
    if clamp[1] - lo >= 0.0 { lo } else { clamp[1] }
}

impl BlendArrayInst {
    pub fn new(env: &Env, gs: &GameState, ctrl: CtrlId) -> Self {
        let d = array_data(env, ctrl).expect("blend array");
        let v = d.signal.map_or(0.0, |s| gs.get(s));
        let cur = damp(v, v, d.clamp, d.damping, 0.0);
        let master = d.master.map(|m| instantiate(env, gs, m));
        let mut a = BlendArrayInst {
            ctrl,
            cur,
            items: (0..d.items.len()).map(|_| None).collect(),
            active: Vec::new(),
            phase: 0.0,
            master,
            exited: false,
        };
        a.activate(env, gs);
        a
    }

    fn activate(&mut self, env: &Env, gs: &GameState) {
        let d = array_data(env, self.ctrl).expect("blend array");
        let xs: Vec<f32> = d.items.iter().map(|i| i.x).collect();
        let pos = self.cur.clamp(d.range[0].min(d.range[1]), d.range[0].max(d.range[1]));
        let sel = select(&xs, pos);
        for &(i, _) in &sel {
            if !self.active.iter().any(|a| a.0 == i) {
                if self.items[i].is_none() {
                    self.items[i] = d.items[i].controller.map(|c| instantiate(env, gs, c));
                    if self.exited
                        && let Some(c) = &mut self.items[i]
                    {
                        c.exit(env, gs);
                    }
                }
                if let Some(c) = &mut self.items[i] {
                    set_phase(env, gs, c.as_mut(), self.phase);
                }
            }
        }
        self.active = sel;
    }
}

impl BlendArrayInst {
    fn weighted_length(&self) -> f32 {
        self.active.iter().map(|&(i, w)| w * self.items[i].as_ref().map_or(0.0, |c| c.length())).sum()
    }

    fn set_array_phase(&mut self, env: &Env, gs: &GameState, p: f32) {
        self.phase = p;
        for &(i, _) in &self.active {
            if let Some(c) = &mut self.items[i] {
                set_phase(env, gs, c.as_mut(), p);
            }
        }
    }
}

impl Controller for BlendArrayInst {
    fn update(&mut self, env: &Env, gs: &mut GameState, dt: f32) {
        let Some(d) = array_data(env, self.ctrl) else { return };
        let target = d.signal.map_or(0.0, |s| gs.get(s));
        self.cur = damp(self.cur, target, d.clamp, d.damping, dt);
        self.activate(env, gs);
        if let Some(m) = &self.master {
            let mp = phase(m.as_ref());
            if (mp - self.phase).abs() > 1.5e-5 {
                self.set_array_phase(env, gs, mp);
            }
        }
        let l = self.weighted_length();
        for &(i, _) in &self.active {
            if let Some(c) = &mut self.items[i] {
                let li = c.length();
                let step = if l > 0.0 { dt * li / l } else { dt };
                c.update(env, gs, step);
            }
        }
        if let Some(&(i, _)) = self.active.first()
            && let Some(c) = &self.items[i]
        {
            self.phase = phase(c.as_ref());
        }
        if let Some(m) = &mut self.master {
            let to = (m.length() * self.phase - m.time()).max(0.0);
            m.update(env, gs, to);
        }
    }

    fn delta(&self, env: &Env, gs: &GameState) -> Delta {
        let mut acc: Option<Delta> = None;
        for &(i, w) in &self.active {
            let d = self.items[i].as_ref().map_or(Delta::None, |c| c.delta(env, gs));
            acc = Some(acc.map_or(d, |a| Delta::blend(a, d, w, env.physics)));
        }
        acc.unwrap_or(Delta::None)
    }

    fn pose(&self, env: &Env, gs: &GameState, out: &mut AnimPose) {
        let mut acc: Option<AnimPose> = None;
        for &(i, w) in &self.active {
            let mut p = AnimPose::empty(env.bind);
            if let Some(c) = &self.items[i] {
                c.pose(env, gs, &mut p);
            }
            match &mut acc {
                None => acc = Some(p),
                Some(a) => a.blend(&p, w),
            }
        }
        if let Some(a) = acc {
            *out = a;
        }
    }

    fn time(&self) -> f32 {
        self.phase * self.weighted_length()
    }
    fn length(&self) -> f32 {
        self.weighted_length()
    }
    fn looping(&self) -> bool {
        self.active.first().and_then(|&(i, _)| self.items[i].as_ref()).is_some_and(|c| c.looping())
    }
    fn set_time(&mut self, env: &Env, gs: &GameState, t: f32) {
        let l = self.weighted_length();
        if l > 0.0 {
            self.set_array_phase(env, gs, t / l);
        }
    }
    fn windows(&self, env: &Env) -> Vec<Window> {
        self.active.first().and_then(|&(i, _)| self.items[i].as_ref()).map_or_else(Vec::new, |c| c.windows(env))
    }
    fn label(&self, env: &Env, weight: f32, out: &mut Vec<(String, f32)>) {
        for &(i, w) in &self.active {
            if let Some(c) = &self.items[i] {
                c.label(env, weight * w, out);
            }
        }
    }
    fn path(&self, env: &Env, out: &mut Vec<PathNode>) {
        if let Some(c) = self.active.first().and_then(|&(i, _)| self.items[i].as_ref()) {
            c.path(env, out);
        }
    }
    fn exit(&mut self, env: &Env, gs: &GameState) {
        self.exited = true;
        for c in self.items.iter_mut().flatten() {
            c.exit(env, gs);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::damp;

    #[test]
    fn the_signal_damping_follows_the_executable() {
        let (clamp, damping) = ([-1.0, 1.0], [0.03, 0.05]);
        assert!((damp(0.2, 0.9, clamp, damping, 2.0) - 0.26).abs() < 1e-6);
        assert!((damp(0.2, -0.9, clamp, damping, 2.0) - 0.1).abs() < 1e-6);
        assert_eq!(damp(0.2, 0.21, clamp, damping, 2.0), 0.21);
        assert_eq!(damp(0.95, 3.0, clamp, damping, 2.0), 1.0);
        assert!((damp(0.2, f32::NAN, clamp, damping, 2.0) - 0.14).abs() < 1e-6);
        assert_eq!(damp(-0.99, f32::NAN, clamp, damping, 2.0), -1.0);
        assert_eq!(damp(f32::NAN, f32::NAN, clamp, damping, 0.0), 1.0);
    }
}
