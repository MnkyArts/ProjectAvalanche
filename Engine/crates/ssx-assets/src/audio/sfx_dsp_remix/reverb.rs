use super::*;

fn primes() -> Vec<u32> {
    let mut sieve = vec![true; 14_000];
    let mut out = Vec::new();
    for n in 2..14_000usize {
        if sieve[n] {
            out.push(n as u32);
            (n * n..14_000).step_by(n).for_each(|k| sieve[k] = false);
        }
    }
    out
}

#[derive(Clone, Debug, Default)]
struct Comb {
    ring: Vec<f32>,
    pos: usize,
    a: f32,
    b: f32,
    z: f32,
}

impl Comb {
    fn resize(&mut self, len: usize) {
        if self.ring.len() != len + 1 {
            self.ring = vec![0.0; len + 1];
            self.pos = 0;
        }
    }

    pub(super) fn tap(&self, back: usize) -> f32 {
        let n = self.ring.len();
        self.ring[wrap(self.pos + n - back, n)]
    }
}

#[derive(Clone, Debug)]
pub struct Reverb {
    primes: Vec<u32>,
    state: u8,
    cache: [f32; 4],
    pub(super) dist: [f32; 6],
    pub(super) len: [usize; 6],
    a: [f32; 6],
    b: [f32; 6],
    lines: [Comb; 6],
    ap: Comb,
    ap_len: usize,
}

impl Reverb {
    pub fn new(max_space: f64) -> Self {
        let mut r = Self {
            primes: primes(),
            state: 0,
            cache: [0.0, 0.0, 1.0, 0.0],
            dist: [0.0; 6],
            len: [0; 6],
            a: [0.0; 6],
            b: [0.0; 6],
            lines: Default::default(),
            ap: Comb::default(),
            ap_len: 0,
        };
        let mut space = max_space as f32;
        r.distances(&mut space);
        r.lengths();
        for (l, &n) in r.lines.iter_mut().zip(&r.len) {
            l.resize(n);
        }
        r.ap_len = ((RATE * 0.006) + 0.5) as usize;
        r.ap.resize(r.ap_len);
        r.ap.a = 0.7;
        r
    }

    pub(super) fn distances(&mut self, space: &mut f32) {
        if *space > 83.3 || *space < 2.0 {
            *space = if *space > 83.3 { 83.3 } else { 2.0 };
        }
        let mut d0 = *space * 0.8;
        let mut d5 = d0 * 1.5;
        if d5 > 100.0 {
            d0 = 66.666_664;
            *space = 83.333_33;
            d5 = 100.0;
        }
        let step = (d5 - d0) * 0.2;
        let d1 = step + d0;
        let d2 = d1 + step;
        let d3 = d2 + step;
        self.dist = [d0, d1, d2, d3, d3 + step, d5];
    }

    pub(super) fn lengths(&mut self) {
        let mut k = 0usize;
        for i in 0..6 {
            let target = (f64::from(RATE) * f64::from(self.dist[i] * 0.002_900_232)) as f32;
            while k < self.primes.len() {
                let p = self.primes[k];
                k += 1;
                if target < p as f32 {
                    self.len[i] = p as usize;
                    break;
                }
            }
        }
    }
}

impl Reverb {
    fn damping(&mut self, brightness: f32) {
        const T_HI: [f32; 9] = [0.0, 0.31, 0.45, 0.53, 0.57, 0.61, 0.64, 0.67, 0.7];
        const T_LO: [f32; 9] = [0.0, 0.12, 0.23, 0.3, 0.35, 0.4, 0.43, 0.46, 0.5];
        let fs = f64::from(RATE).clamp(10_000.0, 50_000.0);
        let (top, span) = if fs > 25_000.0 { (50_000.0, 25_000.0f32) } else { (25_000.0, 15_000.0) };
        let f = (top - fs) as f32 / span;
        let table: [f32; 9] = std::array::from_fn(|i| T_HI[i] * (1.0 - f) + T_LO[i] * f);
        let inv_step = 1.0f32 / 12.5;
        for i in 0..6 {
            let d = self.dist[i];
            let mut hi = 1;
            while hi < 8 && (hi as f32 * 12.5) < d {
                hi += 1;
            }
            let frac = (hi as f32 * 12.5 - d) * inv_step;
            self.a[i] = table[hi] * (1.0 - frac) + table[hi - 1] * frac;
        }
        if brightness != self.cache[2] {
            let mut div = f64::from(self.a[5] + 0.001);
            if div < f64::from(brightness) {
                div = f64::from(brightness);
            }
            let g = (1.0 / div) as f32;
            self.a.iter_mut().for_each(|a| *a *= g);
            self.lines.iter_mut().for_each(|l| l.z = 0.0);
        }
    }

    pub(super) fn feedback(&mut self, time: f32) {
        let t = if f64::from(time) > 0.366 { f64::from(time) } else { 0.366_000_0 };
        let k = 1.0 - (0.366_000_0 / t) as f32;
        for i in 0..6 {
            self.b[i] = (1.0 - self.a[i]) * k;
        }
    }

    fn recompute(&mut self, time: f32, space: f32, brightness: f32) {
        let mut space = space;
        if space != self.cache[1] {
            self.distances(&mut space);
            self.lengths();
            self.damping(brightness);
        } else if RATE != self.cache[3] {
            self.lengths();
            self.damping(brightness);
        } else if brightness != self.cache[2] {
            self.damping(brightness);
        }
        self.feedback(time);
        self.cache = [time, space, brightness, RATE];
        for i in 0..6 {
            let (n, a, b) = (self.len[i], self.a[i], self.b[i]);
            let l = &mut self.lines[i];
            l.resize(n);
            (l.a, l.b) = (a, b);
        }
        self.state = 3;
    }

    pub fn process(&mut self, time: f64, space: f64, brightness: f64, chans: &mut [Vec<f32>]) {
        let (time, space, brightness) = (time as f32, space as f32, brightness as f32);
        if self.state == 3 {
            self.state = 4;
        }
        if time <= 0.0 {
            chans.iter_mut().for_each(|c| c.fill(0.0));
            if self.cache[0] > 0.0 {
                for l in &mut self.lines {
                    l.ring.fill(0.0);
                    l.z = 0.0;
                }
                self.ap.ring.fill(0.0);
            }
            self.state = 0;
            return;
        }
        let changed =
            time != self.cache[0] || space != self.cache[1] || brightness != self.cache[2] || RATE != self.cache[3];
        let mut silent = false;
        if changed {
            silent = self.state == 0;
            self.state = 1;
        }
        if silent {
            chans.iter_mut().for_each(|c| c.fill(0.0));
        } else if let Some(x) = chans.first_mut() {
            let (lines, ap, ap_len) = (&mut self.lines, &mut self.ap, self.ap_len);
            // PORT(choice): run in the FMA build of `fused` where the CPU has FMA, so the multiply-adds (4 a sample
            // and line, 2 a sample in the all-pass) are the instruction in line instead of calls to the runtime's
            // `fmaf` (the same single rounding: the same samples bit for bit); the flag is not used (they stay fused
            // without FMA too, as before); the console has the ReverbModel1 process, whose multiply-adds these are.
            crate::sfx_dsp::fused(
                #[inline(always)]
                |_| {
                    let mut wet = vec![0.0f32; x.len()];
                    for l in lines.iter_mut() {
                        let n = l.ring.len() - 1;
                        for (k, &xk) in x.iter().enumerate() {
                            let (old, next) = (l.tap(n), l.tap(n - 1));
                            let y = l.a.mul_add(l.z, l.b.mul_add(old, xk));
                            wet[k] = (-l.a).mul_add(next, old).mul_add(1.0 / 6.0, wet[k]);
                            l.ring[l.pos] = y;
                            l.pos = wrap(l.pos + 1, l.ring.len());
                            l.z = y;
                        }
                    }
                    let (g, n) = (ap.a, ap_len);
                    for (k, v) in wet.iter().enumerate() {
                        let old = ap.tap(n);
                        let w = (-g).mul_add(old, *v);
                        ap.ring[ap.pos] = w;
                        ap.pos = wrap(ap.pos + 1, ap.ring.len());
                        x[k] = g.mul_add(w, old) * 2.0;
                    }
                },
            );
        }
        if self.state == 1 {
            self.recompute(time, space, brightness);
        }
    }
}
