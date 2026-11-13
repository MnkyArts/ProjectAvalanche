const ENTER: u8 = 1;
const ACTIVE: u8 = 2;
const LEAVE: u8 = 4;

const EPS: f32 = 1.525_878_9e-5;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TagWindow {
    pub time: f32,
    pub duration: f32,
    pub always_active: bool,
}

impl TagWindow {
    pub const fn new(time: f32, duration: f32, always_active: bool) -> Self {
        Self { time, duration, always_active }
    }

    pub fn weight(&self, t: f32) -> f32 {
        if self.duration > 0.0 { ((t - self.time) / self.duration).clamp(0.0, 1.0) } else { 1.0 }
    }
}

pub fn tag_spline(s: &[f32], w: f32) -> f32 {
    let Some(&last) = s.last() else { return 0.0 };
    let x = (s.len() - 1) as f32 * w;
    let i = x as usize;
    if i + 1 >= s.len() {
        return last;
    }
    let f = x - i as f32;
    (1.0 - f) * s[i] + s[i + 1] * f
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TagEvent {
    Enter,
    Apply(f32),
    Leave(f32),
}

#[derive(Clone, Debug, PartialEq)]
pub struct TagTracker {
    windows: Vec<TagWindow>,
    prev: Vec<u8>,
    cur: Vec<u8>,
    deferred: Vec<u8>,
    t_prev: f32,
    t_now: f32,
    pending: bool,
}

impl TagTracker {
    pub fn new(windows: &[TagWindow]) -> Self {
        let n = windows.len();
        Self {
            windows: windows.to_vec(),
            prev: vec![0; n],
            cur: vec![0; n],
            deferred: vec![0; n],
            t_prev: -1.0,
            t_now: -1.0,
            pending: false,
        }
    }

    pub fn time(&self) -> f32 {
        self.t_now
    }

    pub fn set_time(&mut self, t: f32) {
        self.t_now = t;
    }

    pub fn update(&mut self, t: f32) -> bool {
        if !self.pending {
            let old_prev = std::mem::take(&mut self.prev);
            self.prev = std::mem::replace(&mut self.cur, std::mem::replace(&mut self.deferred, old_prev));
            self.deferred.iter_mut().for_each(|f| *f = 0);
        }
        self.pending = true;
        self.t_prev = self.t_now;
        self.t_now = if self.t_prev - t <= 0.5 { t.max(self.t_prev) } else { t };
        let mut any = false;
        for i in 0..self.windows.len() {
            let (c, d) = self.test(i);
            (self.cur[i], self.deferred[i]) = (c, d);
            any |= c != 0;
        }
        any
    }
}

impl TagTracker {
    fn test(&self, i: usize) -> (u8, u8) {
        let (w, prev) = (self.windows[i], self.prev[i]);
        let (mut cur, mut deferred) = (self.cur[i], self.deferred[i]);
        let (start, end, t) = (w.time, w.time + w.duration, self.t_now);
        let inside = (t >= start && t <= end) || ((start - t).abs() <= EPS && w.duration.abs() <= EPS);
        if w.always_active {
            if prev & (ENTER | ACTIVE) == 0 {
                cur |= ENTER;
            }
            return (cur | ACTIVE, deferred);
        }
        let crossed = start > self.t_prev && end < t;
        if crossed {
            return (cur | ENTER | ACTIVE, deferred | LEAVE);
        }
        if inside {
            if prev & (ENTER | ACTIVE) == 0 {
                cur |= ENTER;
            }
            return (cur | ACTIVE, deferred);
        }
        if prev & ACTIVE != 0 && prev & LEAVE == 0 {
            if cur & (ENTER | ACTIVE) != 0 {
                deferred |= LEAVE;
            } else {
                cur |= LEAVE;
            }
        }
        (cur, deferred)
    }

    pub fn process(&mut self) -> Vec<(usize, TagEvent)> {
        self.pending = false;
        let mut out = Vec::new();
        for (i, w) in self.windows.iter().enumerate() {
            if self.cur[i] & LEAVE != 0 {
                out.push((i, TagEvent::Leave(w.weight(self.t_now))));
            }
        }
        for (i, w) in self.windows.iter().enumerate() {
            if self.cur[i] & ENTER != 0 {
                out.push((i, TagEvent::Enter));
            }
            if self.cur[i] & ACTIVE != 0 {
                out.push((i, TagEvent::Apply(w.weight(self.t_now))));
            }
        }
        out
    }

    pub fn exit(&mut self) -> Vec<(usize, TagEvent)> {
        self.pending = false;
        let out = (0..self.windows.len())
            .filter(|&i| self.cur[i] & ACTIVE != 0 && self.cur[i] & LEAVE == 0)
            .map(|i| (i, TagEvent::Leave(self.windows[i].weight(self.t_now))))
            .collect();
        for a in [&mut self.prev, &mut self.cur, &mut self.deferred] {
            a.iter_mut().for_each(|f| *f = 0);
        }
        out
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BoolTag {
    pub value: bool,
    pub mode: u32,
    pub reset: Option<bool>,
}

impl BoolTag {
    pub fn on(&self, ev: TagEvent, var: &mut bool, saved: &mut bool) {
        if self.mode == 0 {
            return;
        }
        match ev {
            TagEvent::Enter => *saved = *var,
            TagEvent::Apply(_) => {
                *var = match self.mode {
                    1 => self.value,
                    2 => !*saved,
                    _ => false,
                }
            }
            TagEvent::Leave(_) => {
                if let Some(default) = self.reset {
                    *var = default;
                }
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FloatTag {
    pub value: f32,
    pub mode: u32,
    pub reset: Option<f32>,
    pub spline: f32,
}

impl FloatTag {
    pub fn on(&self, ev: TagEvent, var: &mut f32, saved: &mut f32) {
        if self.mode == 0 {
            return;
        }
        let (v, s) = (self.value * self.spline, self.spline);
        match ev {
            TagEvent::Enter => *saved = *var,
            TagEvent::Apply(_) => {
                *var = match self.mode {
                    1 => v,
                    2 => v * *saved,
                    3 => v + (1.0 - s) * *saved,
                    4 => v + *saved,
                    5 => *saved - v,
                    _ => 0.0,
                }
            }
            TagEvent::Leave(_) => {
                if let Some(default) = self.reset {
                    *var = default;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::TagEvent::{Apply, Enter, Leave};
    use super::*;

    fn run(tr: &mut TagTracker, t: f32) -> Vec<(usize, TagEvent)> {
        if tr.update(t) { tr.process() } else { Vec::new() }
    }

    #[test]
    fn a_node_window_fires_on_the_first_update() {
        let mut tr = TagTracker::new(&[TagWindow::new(0.0, 1.0, false)]);
        assert_eq!(run(&mut tr, 0.0), [(0, Enter), (0, Apply(0.0))]);
        assert_eq!(run(&mut tr, 2.0), [(0, Leave(1.0))]);
        assert_eq!(run(&mut tr, 4.0), []);
    }

    #[test]
    fn a_zero_window_fires_on_reaching_or_passing_it() {
        let mut tr = TagTracker::new(&[TagWindow::new(40.0, 0.0, false)]);
        assert_eq!(run(&mut tr, 38.0), []);
        assert_eq!(run(&mut tr, 40.0), [(0, Enter), (0, Apply(1.0))]);
        assert_eq!(run(&mut tr, 42.0), [(0, Leave(1.0))]);
        let mut tr = TagTracker::new(&[TagWindow::new(40.0, 0.0, false)]);
        assert_eq!(run(&mut tr, 39.0), []);
        assert_eq!(run(&mut tr, 41.0), [(0, Enter), (0, Apply(1.0))]);
        assert_eq!(run(&mut tr, 43.0), [(0, Leave(1.0))]);
        assert_eq!(run(&mut tr, 45.0), []);
    }

    #[test]
    fn always_active_until_the_exit() {
        let mut tr = TagTracker::new(&[TagWindow::new(0.0, 32767.0, true)]);
        let tag = BoolTag { value: false, mode: 1, reset: None };
        let (mut var, mut saved) = (true, false);
        for (i, t) in [0.0, 2.0, 4.0].into_iter().enumerate() {
            let ev = run(&mut tr, t);
            assert_eq!(ev.len(), if i == 0 { 2 } else { 1 });
            ev.into_iter().for_each(|(_, e)| tag.on(e, &mut var, &mut saved));
            assert!(!var && saved);
        }
        let ev = tr.exit();
        assert!(matches!(ev[..], [(0, Leave(_))]));
        ev.into_iter().for_each(|(_, e)| tag.on(e, &mut var, &mut saved));
        assert!(!var, "no ResetOnLeave: the value stays");
    }

    #[test]
    fn a_reset_on_leave_writes_the_default_not_the_saved_value() {
        let chop_end = FloatTag { value: 0.0, mode: 1, reset: Some(0.0), spline: 1.0 };
        let (mut weight, mut saved) = (1.0, 0.0);
        for e in [Enter, Apply(1.0)] {
            chop_end.on(e, &mut weight, &mut saved);
        }
        assert_eq!((weight, saved), (0.0, 1.0));
        chop_end.on(Leave(1.0), &mut weight, &mut saved);
        assert_eq!(weight, 0.0, "the default, not the saved 1");
        let offset = FloatTag { value: 0.3, mode: 1, reset: Some(0.8), spline: 1.0 };
        let (mut f, mut fs) = (0.5, 0.0);
        for e in [Enter, Apply(0.0)] {
            offset.on(e, &mut f, &mut fs);
        }
        assert_eq!(f, 0.3);
        offset.on(Leave(1.0), &mut f, &mut fs);
        assert_eq!(f, 0.8);
        let air = BoolTag { value: false, mode: 1, reset: Some(true) };
        let (mut b, mut bs) = (false, false);
        for e in [Enter, Apply(0.0), Leave(1.0)] {
            air.on(e, &mut b, &mut bs);
        }
        assert!(b && !bs, "the default true, not the saved false");
    }

    #[test]
    fn a_time_set_skips_the_passed_windows_and_leaves_the_active_ones() {
        let mut tr = TagTracker::new(&[TagWindow::new(0.0, 10.0, false), TagWindow::new(20.0, 5.0, false)]);
        assert_eq!(run(&mut tr, 1.0), vec![(0, Enter), (0, Apply(0.1))]);
        tr.set_time(30.0);
        assert_eq!(run(&mut tr, 31.0), vec![(0, Leave(1.0))], "tag 1's window (20…25) was jumped over");
        assert!(run(&mut tr, 32.0).is_empty());
    }
}
