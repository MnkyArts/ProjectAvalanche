use super::*;

fn twiddles(n: usize) -> Vec<(f64, f64)> {
    let mut t = Vec::with_capacity(n);
    let mut len = 2;
    while len <= n {
        let ang = -std::f64::consts::TAU / len as f64;
        t.extend((0..len / 2).map(|k| (ang * k as f64).sin_cos()));
        len <<= 1;
    }
    t
}

// PORT(choice): the twiddles computed once with the detector (`twiddles`, the same `sin_cos` of the same arguments:
// the same values) instead of 5 120 `sin_cos` calls per transform (one every two mixer blocks); the console's
// transform 0x82A125E0 has its own tables.
fn fft(re: &mut [f32], im: &mut [f32], twiddles: &[(f64, f64)]) {
    let n = re.len();
    let mut j = 0;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j |= bit;
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
    }
    let (mut len, mut stage) = (2, twiddles);
    while len <= n {
        let (w, rest) = stage.split_at(len / 2);
        stage = rest;
        for start in (0..n).step_by(len) {
            for (k, &(s, c)) in w.iter().enumerate() {
                let (a, b) = (start + k, start + k + len / 2);
                let tr = re[b] * c as f32 - im[b] * s as f32;
                let ti = re[b] * s as f32 + im[b] * c as f32;
                (re[b], im[b]) = (re[a] - tr, im[a] - ti);
                re[a] += tr;
                im[a] += ti;
            }
        }
        len <<= 1;
    }
}

#[derive(Clone, Debug)]
pub struct BeatDetect {
    n: usize,
    max_lag: usize,
    min_lag: usize,
    causal: bool,
    frames: [Vec<f32>; 2],
    cur: usize,
    half: usize,
    filled: usize,
    window: Vec<f32>,
    twiddles: Vec<(f64, f64)>,
    odf: Vec<f32>,
    odf_w: usize,
    odf_count: usize,
    lin: Vec<f32>,
    acf: Vec<f32>,
    rayleigh: Vec<f32>,
    ctx: Vec<f32>,
    phase_w: Vec<f32>,
    state: u8,
    busy: bool,
    lag: [usize; 3],
    tau: usize,
    center: usize,
    last_ctx: usize,
    mode: u8,
    meter: u32,
    meter_f: f32,
    meter_hist: [f32; 25],
    meter_i: usize,
    conf: f32,
    conf_hist: [f32; 25],
    conf_i: usize,
    n_avg: usize,
    phase_used: usize,
    next_pos: i64,
    beat_time: f32,
    period: f32,
    bpm: f32,
    silence: u8,
    quiet: u32,
    hold: u32,
    zero_run: u32,
    unreliable: bool,
    cache: [f32; 2],
    delay: Vec<Vec<Vec<f32>>>,
    delay_i: usize,
}

const HOP_TIME: f32 = f32::from_bits(0x3C2E_C33E);
const BLOCK_TIME: f32 = f32::from_bits(0x3BAE_C33E);

impl BeatDetect {
    pub fn new(channels: usize, frame_size: f64, causal: f64, max_bpm: f64) -> Self {
        let n = if frame_size == 0.0 { 256 } else { 512 };
        let max_lag = n / 4;
        let min_lag = (60.0 / (max_bpm as f32 * HOP_TIME)) as usize;
        let causal = causal != 0.0;
        let window = (0..1024)
            .map(|i| {
                let a = f64::from((i as f32 * (1.0 / 1024.0)) * 6.283_185_5);
                -((a.cos() as f32) * 0.5 - 0.5)
            })
            .collect();
        let beta = f32::from_bits(0x3A0D_C6AE);
        let rayleigh = (0..max_lag)
            .map(|t| {
                if t < min_lag {
                    0.0
                } else {
                    let e = std::f64::consts::E.powf(-f64::from((t * t) as f32 * beta) * 0.5);
                    t as f32 * beta * e as f32
                }
            })
            .collect();
        let blocks = 9 + if causal { 0 } else { 2 * n };
        Self {
            n,
            max_lag,
            min_lag,
            causal,
            frames: [vec![0.0; 1026], vec![0.0; 1026]],
            cur: 0,
            half: 0,
            filled: 0,
            window,
            twiddles: twiddles(1024),
            odf: vec![0.0; n + 8],
            odf_w: 0,
            odf_count: 0,
            lin: vec![0.0; n + 16],
            acf: vec![0.0; n],
            rayleigh,
            ctx: vec![0.0; max_lag],
            phase_w: vec![0.0; 2 * max_lag],
            state: 10,
            busy: false,
            lag: [0; 3],
            tau: 0,
            center: 0,
            last_ctx: 0,
            mode: 1,
            meter: 2,
            meter_f: 2.5,
            meter_hist: [2.5; 25],
            meter_i: 0,
            conf: 0.0,
            conf_hist: [0.0; 25],
            conf_i: 0,
            n_avg: 0,
            phase_used: 0,
            next_pos: -1,
            beat_time: 0.5,
            period: 0.5,
            bpm: 120.0,
            silence: 2,
            quiet: 0,
            hold: 0,
            zero_run: 0,
            unreliable: false,
            cache: [-1.0, -1.0],
            delay: vec![vec![vec![0.0; BLOCK]; channels]; blocks],
            delay_i: 0,
        }
    }

    pub fn outputs(&self) -> [f64; 5] {
        let m = f64::from(self.meter_f);
        let r = if m < 0.0 { m - 0.5 } else { m + 0.5 } as i32;
        let conf = (0.5 - (self.meter_f - r as f32).abs()) * 2.0;
        [f64::from(self.bpm), f64::from(self.beat_time), f64::from(self.conf), f64::from(r), f64::from(conf)]
    }

    pub fn process(&mut self, response: f64, zero_run: f64, chans: &mut [Vec<f32>]) {
        if response as f32 != self.cache[0] {
            self.cache[0] = response as f32;
            self.n_avg = (response / f64::from(self.max_lag as f32 * HOP_TIME * 0.63)) as usize;
            self.conf_hist = [0.0; 25];
            self.conf_i = 0;
            self.meter_hist = [2.5; 25];
            self.meter_i = 0;
        }
        if zero_run as f32 != self.cache[1] {
            self.cache[1] = zero_run as f32;
            (self.quiet, self.unreliable, self.hold, self.silence) = (0, false, 0, 2);
            self.zero_run = (48_000.0 * zero_run) as u32;
        }
        self.beat_time -= BLOCK_TIME;
        if self.beat_time <= 0.0 {
            self.beat_time += self.period;
        }
        if self.busy && self.filled == 0 {
            if self.state < 10 {
                self.state += 1;
            }
            self.busy = false;
        }
        let mut energy = 0.0f32;
        let base = 512 + self.half * BLOCK;
        for k in 0..BLOCK {
            let mut s = chans.first().map_or(0.0, |c| c[k]);
            for c in chans.iter().skip(1) {
                s += c[k];
            }
            self.frames[self.cur][base + k] = s;
            if k % 8 == 0 {
                energy += s.abs();
            }
        }
        let (cur, other) = (self.cur, 1 - self.cur);
        let block: Vec<f32> = self.frames[cur][base..base + BLOCK].to_vec();
        self.frames[other][self.half * BLOCK..(self.half + 1) * BLOCK].copy_from_slice(&block);
        self.half = (self.half + 1) % 2;
        self.filled += BLOCK;
        if self.filled >= 512 {
            self.onset_frame();
        }
        self.level_gate(energy);
        self.analyse();
        let out = std::mem::replace(&mut self.delay[self.delay_i], chans.to_vec());
        for (c, o) in chans.iter_mut().zip(out) {
            *c = o;
        }
        self.delay_i = (self.delay_i + 1) % self.delay.len();
    }

    fn onset_frame(&mut self) {
        let f = &mut self.frames[self.cur];
        let mut re: Vec<f32> = f[..1024].iter().zip(&self.window).map(|(x, w)| x * w).collect();
        let mut im = vec![0.0f32; 1024];
        fft(&mut re, &mut im, &self.twiddles);
        // PORT(choice): run in the FMA build of `fused` where the CPU has FMA, so the 513 multiply-adds are the
        // instruction in line instead of calls to the runtime's `fmaf` (the same single rounding: the same value bit
        // for bit); the flag is not used (they stay fused without FMA too, as before); the console has the BeatDetect
        // process 0x82A2F5D0 with its own transform.
        let hfc = crate::sfx_dsp::fused(
            #[inline(always)]
            |_| (0..=512).fold(0.0f32, |hfc, k| (re[k] * re[k] + im[k] * im[k]).mul_add((k + 1) as f32, hfc)),
        );
        self.filled = 0;
        self.odf[self.odf_w] = hfc / 513.0;
        self.odf_count += 1;
        self.cur = 1 - self.cur;
        self.odf_w = (self.odf_w + 1) % self.odf.len();
        if self.odf_count >= self.max_lag {
            let len = self.odf.len();
            for i in 0..len {
                self.lin[i] = self.odf[(self.odf_w + i) % len];
            }
            for j in 0..8 {
                self.lin[len + j] = self.odf[len - 1 - j];
            }
            self.odf_count = 0;
            self.busy = true;
            self.state = 0;
        }
    }

    fn level_gate(&mut self, energy: f32) {
        if f32::from_bits(0x3800_0100) <= energy {
            if self.silence == 0 {
                (self.silence, self.quiet, self.hold) = (1, 0, (self.n as f32 * 2.0) as u32);
            }
        } else {
            self.quiet += BLOCK as u32;
            if self.zero_run <= self.quiet {
                self.silence = 0;
            }
        }
        if self.silence == 1 {
            self.hold = self.hold.saturating_sub(1);
            if self.hold == 0 {
                (self.quiet, self.silence) = (0, 2);
            }
        }
        self.unreliable = self.silence != 2;
    }
}

impl BeatDetect {
    fn analyse(&mut self) {
        let n = self.n;
        if self.state == 1 && !self.busy {
            let inv = f32::from_bits(0x3D70_F0F1);
            let mut sum =
                self.lin[..16].iter().step_by(2).sum::<f32>() + self.lin[1..16].iter().step_by(2).sum::<f32>();
            sum += self.lin[16];
            let mut mean = vec![sum * inv];
            for i in 1..n {
                sum = (sum - self.lin[i - 1]) + self.lin[i + 16];
                mean.push(sum * inv);
            }
            for (i, m) in mean.iter().enumerate() {
                self.lin[i] = (self.lin[i + 8] - m).max(0.0);
            }
            self.busy = true;
        }
        if self.state == 2 && !self.busy {
            for lag in 0..n {
                let s: f32 = (0..n - lag).map(|k| self.lin[k] * self.lin[k + lag]).sum();
                self.acf[lag] = s * (1.0 / (n - lag) as f32);
            }
            self.busy = true;
        }
        if self.state == 3 && !self.busy {
            self.lag = [self.comb(4, false), self.lag[0], self.lag[1]];
            self.state = 4;
        }
        if self.state == 4 && !self.busy {
            self.meter = if self.triple(self.lag[0]) { 3 } else { 2 };
            self.state = 5;
        }
        if self.state == 5 && !self.busy {
            if self.unreliable {
                (self.meter_hist, self.meter_i) = ([2.5; 25], 0);
            } else {
                self.meter_hist[self.meter_i] = self.meter as f32;
                self.meter_i = if self.n_avg == self.meter_i + 1 { 0 } else { self.meter_i + 1 };
            }
            self.meter_f = self.meter_hist[..self.n_avg.min(25)].iter().sum::<f32>() / self.n_avg as f32;
            self.busy = true;
        }
        if self.state == 6 && !self.busy {
            self.tempo();
            self.busy = true;
        }
        if self.state == 7 && !self.busy {
            self.phase_used = self.phase(self.tau, self.mode);
            self.beat_time = self.next_beat(self.tau, self.phase_used);
            self.state = 8;
        }
        if self.state == 8 && !self.busy {
            let now = self.phase(self.lag[0], 1);
            let step = if self.unreliable || self.mode == 1 {
                0.0
            } else if self.mode == 3 {
                0.5
            } else if self.tau.abs_diff(self.lag[0]) > 8 {
                0.0
            } else if (self.phase_used.abs_diff(now) as f32) < self.tau as f32 * 0.25 {
                1.0
            } else {
                0.5
            };
            if self.unreliable {
                (self.conf_hist, self.conf_i) = ([0.0; 25], 0);
            } else {
                self.conf_hist[self.conf_i] = step;
                self.conf_i = if self.n_avg == self.conf_i + 1 { 0 } else { self.conf_i + 1 };
            }
            self.state = 9;
        }
        if self.state == 9 && !self.busy {
            self.conf = self.conf_hist[..self.n_avg.min(25)].iter().sum::<f32>() / self.n_avg as f32;
            self.busy = true;
        }
    }

    fn comb(&self, harmonics: usize, context: bool) -> usize {
        let w = if context { &self.ctx } else { &self.rayleigh };
        let (mut best, mut best_v) = (self.min_lag, 0.0f32);
        for (t, &wt) in w.iter().enumerate().take(self.max_lag).skip(self.min_lag) {
            let mut s = 0.0f32;
            for a in 1..=harmonics {
                let start = a * t - (a - 1);
                let width = 2 * a - 1;
                let sum: f32 = self.acf.get(start..start + width).map_or(0.0, |v| v.iter().sum());
                s += sum / width as f32;
            }
            let v = wt * s;
            if best_v < v {
                (best, best_v) = (t, v);
            }
        }
        best
    }

    fn triple(&self, t: usize) -> bool {
        let mut a = [0.0f32; 5];
        for (k, m) in [2usize, 4, 3, 6].into_iter().enumerate() {
            let end = m + m * t - 1;
            if end < self.n {
                let start = (t - 1) * m + 1;
                let width = 2 * m - 1;
                a[k + 1] = self.acf[start..start + width].iter().sum::<f32>() / width as f32;
            } else if m > 3 {
                a[k + 1] = a[k];
            }
        }
        a[2] + a[1] < a[4] + a[3]
    }
}

impl BeatDetect {
    fn tempo(&mut self) {
        let [l0, l1, l2] = self.lag.map(|l| l as i64);
        let consistent = (2 * l0 - l2 - l1).unsigned_abs() <= 3;
        let enter = match self.mode {
            1 => consistent && !self.unreliable,
            3 => true,
            _ => false,
        };
        if enter {
            if self.mode == 1 {
                self.center = self.lag[0];
            }
            let inv = f32::from_bits(0x3D80_0000);
            for t in 0..self.max_lag {
                self.ctx[t] = if t < self.min_lag {
                    0.0
                } else {
                    let d = t as f32 - self.center as f32;
                    (std::f64::consts::E.powf(-f64::from(d * d * inv) * 0.5) as f32 as f64 + 1.000_000_045_813_705e-18)
                        as f32
                };
            }
            self.mode = 2;
        }
        if self.mode == 1 {
            self.tau = self.lag[0];
        } else {
            let tc = self.comb(if self.meter == 2 { 4 } else { 3 }, true);
            if tc.abs_diff(self.lag[0]) >= 9 && consistent {
                self.center = self.lag[0];
                self.mode = 3;
                self.tau = self.lag[0];
            } else {
                if tc != self.last_ctx {
                    let f = if self.causal { 0.125f32 } else { 0.25 };
                    let w = tc as f32 * f;
                    let inv = 1.0 / (w * w);
                    for (i, v) in self.phase_w.iter_mut().enumerate() {
                        let d = i as f32 - tc as f32;
                        *v = (std::f64::consts::E.powf(-f64::from(d * d * inv) * 0.5) as f32 as f64
                            + 1.000_000_045_813_705e-18) as f32;
                    }
                    self.last_ctx = tc;
                }
                self.tau = tc;
            }
        }
        self.period = HOP_TIME * self.tau as f32;
        self.bpm = 60.0 / self.period;
    }

    pub(super) fn phase(&self, tau: usize, mode: u8) -> usize {
        let n = self.n;
        if tau == 0 {
            return 0;
        }
        let beats = n / tau;
        let use_w = mode == 2 && self.next_pos > 0;
        let off = self.max_lag as i64 - self.next_pos;
        let inv = 1.0 / n as f32;
        let (mut best_phi, mut best) = (1usize, 0.0f32);
        for phi in 1..=tau {
            let mut s = 0.0f32;
            for b in 1..=beats {
                let i = b * tau - phi;
                let v = if self.causal { self.lin[n - i] } else { self.lin[i] };
                s = (v * (n - i) as f32).mul_add(inv, s);
            }
            if use_w {
                let idx = if self.causal { phi as i64 + off } else { off - phi as i64 + tau as i64 };
                s *= self.phase_w.get(idx as usize).copied().unwrap_or(0.0);
            }
            if best < s {
                (best, best_phi) = (s, phi);
            }
        }
        tau - best_phi
    }

    fn next_beat(&mut self, tau: usize, phi: usize) -> f32 {
        let (t, p, m) = (tau as f32, phi as f32, self.max_lag as f32);
        let q = if self.causal { (m - t + p) / t } else { (m - p) / t };
        let mut k = f64::from(q).floor() as f32;
        if q <= k {
            k -= 1.0;
        }
        let k = k as i64;
        let ahead = if self.causal {
            self.next_pos = (k + 1) * tau as i64 - phi as i64;
            tau - phi
        } else {
            self.next_pos = k * tau as i64 + phi as i64;
            phi
        };
        (512 * ahead + 2) as f32 / 48_000.0
    }
}
