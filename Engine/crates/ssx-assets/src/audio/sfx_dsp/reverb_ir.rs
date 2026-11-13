use std::sync::Arc;

use ssx_formats::irf::Irf;

use super::BLOCK;

#[derive(Debug)]
pub struct ImpulseResponse {
    pub channels: usize,
    pub samples: usize,
    pub block: usize,
    pub bins: usize,
    scales: Vec<f32>,
    data: Vec<f32>,
}

impl ImpulseResponse {
    pub fn new(irf: &Irf) -> Self {
        let bins = irf.bins as usize;
        let scales = irf.records.iter().map(|r| f32::from(r.scale as i16)).collect();
        let data = irf
            .records
            .iter()
            .flat_map(|r| r.bins.iter().flat_map(|&(re, im)| [f32::from(re), f32::from(im)]))
            .collect();
        Self {
            channels: irf.channels as usize,
            samples: irf.samples as usize,
            block: irf.block as usize,
            bins,
            scales,
            data,
        }
    }

    pub fn partitions(&self) -> usize {
        self.samples.div_ceil(self.block)
    }

    fn record(&self, i: usize) -> (f32, &[f32]) {
        let n = 2 * self.bins;
        (self.scales[i], &self.data[i * n..(i + 1) * n])
    }
}

#[derive(Debug)]
struct RealFft {
    n: usize,
    cos: Vec<f32>,
    sin: Vec<f32>,
    rev: Vec<usize>,
    tw: Vec<(f32, f32)>,
}

impl RealFft {
    fn new(n: usize) -> Self {
        let pi = f64::from(std::f32::consts::PI);
        let arg = |i: usize| f64::from((i as f64 * pi) as f32 / n as f32);
        let cos = (0..=n / 2).map(|i| arg(i).cos() as f32).collect();
        let sin = (0..=n / 2).map(|i| arg(i).sin() as f32).collect();
        let bits = n.trailing_zeros();
        let rev = (0..n).map(|i| i.reverse_bits() >> (usize::BITS - bits)).collect();
        let tw = (0..n / 2)
            .map(|k| {
                let a = -std::f64::consts::TAU * k as f64 / n as f64;
                (a.cos() as f32, a.sin() as f32)
            })
            .collect();
        Self { n, cos, sin, rev, tw }
    }

    fn complex(&self, z: &mut [f32], inverse: bool) {
        let n = self.n;
        for (i, &j) in self.rev.iter().enumerate().take(n) {
            if i < j {
                z.swap(2 * i, 2 * j);
                z.swap(2 * i + 1, 2 * j + 1);
            }
        }
        let mut len = 2;
        while len <= n {
            let step = n / len;
            for start in (0..n).step_by(len) {
                for k in 0..len / 2 {
                    let (c, s) = self.tw[k * step];
                    let s = if inverse { -s } else { s };
                    let (a, b) = (start + k, start + k + len / 2);
                    let (br, bi) = (z[2 * b], z[2 * b + 1]);
                    let tr = br * c - bi * s;
                    let ti = br * s + bi * c;
                    let (ar, ai) = (z[2 * a], z[2 * a + 1]);
                    z[2 * b] = ar - tr;
                    z[2 * b + 1] = ai - ti;
                    z[2 * a] = ar + tr;
                    z[2 * a + 1] = ai + ti;
                }
            }
            len <<= 1;
        }
    }
}

impl RealFft {
    fn forward(&self, buf: &mut [f32]) {
        let n = self.n;
        self.complex(&mut buf[..2 * n], false);
        (buf[2 * n], buf[2 * n + 1]) = (buf[0], buf[1]);
        for k in 0..=n / 2 {
            let (lo, hi) = (2 * k, 2 * (n - k));
            let a = buf[lo] - buf[hi];
            let b = buf[hi] + buf[lo];
            let c = buf[lo + 1] - buf[hi + 1];
            let d = buf[hi + 1] + buf[lo + 1];
            let (s, co) = (self.sin[k], self.cos[k]);
            let (sa, ca, cd, sd) = (s * a, co * a, co * d, s * d);
            buf[lo] = ((b - sa) + cd) * 0.5;
            buf[lo + 1] = ((c - ca) - sd) * 0.5;
            buf[hi] = ((sa + b) - cd) * 0.5;
            buf[hi + 1] = ((-c - ca) - sd) * 0.5;
        }
    }

    fn inverse(&self, buf: &mut [f32]) {
        let n = self.n;
        for k in 0..=n / 2 {
            let (lo, hi) = (2 * k, 2 * (n - k));
            let a = buf[lo] - buf[hi];
            let d = buf[hi + 1] + buf[lo + 1];
            let b = buf[hi] + buf[lo];
            let c = buf[lo + 1] - buf[hi + 1];
            let (s, co) = (self.sin[k], self.cos[k]);
            let (sa, ca, cd, sd) = (s * a, co * a, co * d, s * d);
            buf[lo] = (b - sa) - cd;
            buf[lo + 1] = (ca + c) - sd;
            buf[hi] = (cd + sa) + b;
            buf[hi + 1] = (ca - c) - sd;
        }
        self.complex(&mut buf[..2 * n], true);
        let scale = 1.0 / n as f32;
        buf[..2 * n].iter_mut().for_each(|x| *x *= scale);
    }
}

fn mac(slots: [Option<(f32, &[f32])>; 2], gains: [f32; 2], x: &[f32], y: &mut [f32]) {
    let cmul = |hr: f32, hi: f32, k: usize, y: &mut [f32]| {
        let (xr, xi) = (x[2 * k], x[2 * k + 1]);
        let re = hr * xr - hi * xi;
        let im = hr.mul_add(xi, hi * xr);
        y[2 * k] += re;
        y[2 * k + 1] += im;
    };
    let bins = x.len() / 2;
    if let [Some((s0, h0)), Some((s1, h1))] = slots
        && gains[0] > 0.0
        && gains[1] > 0.0
    {
        let (c0, c1) = (gains[0] / s0, gains[1] / s1);
        for k in 0..bins {
            let hr = h1[2 * k].mul_add(c1, h0[2 * k] * c0);
            let hi = h1[2 * k + 1].mul_add(c1, h0[2 * k + 1] * c0);
            cmul(hr, hi, k, y);
        }
        return;
    }
    for (slot, g) in slots.into_iter().zip(gains) {
        let Some((s, h)) = slot else { continue };
        if g == 0.0 {
            continue;
        }
        let c = g / s;
        for k in 0..bins {
            cmul(h[2 * k] * c, h[2 * k + 1] * c, k, y);
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Work {
    pub mac: usize,
    pub fft: usize,
    pub ifft: usize,
}

pub fn schedule(block: usize, parts: [usize; 2], ins: usize, outs: usize, cut: f32, subs: usize) -> Vec<Work> {
    let mut l = 0i32;
    let mut v = (2 * block) as i32;
    while v > 1 {
        v /= 2;
        l += 1;
    }
    let (most, least) = if parts[0] < parts[1] { (parts[1], parts[0]) } else { (parts[0], parts[1]) };
    let (f_least, f_most, f_l, f_out, f_in) = (least as f32, most as f32, (l - 1) as f32, outs as f32, ins as f32);
    let kept = (-cut).mul_add(0.01, 1.0);
    let f3 = f_l * 20.92;
    let ratio = f_least * 0.08 / f_most + 1.0;
    let w = kept * f_most * f_out * 20.07 * ratio / f3;
    let mac_cost = w / f_most;
    let mut total = (f_out * 42.51 / f3 + f_in + f_out) + w;
    let mut unit = 1.0f32;
    let (mut nf, mut nm, mut ni) = (0, 0, 0);
    let mut table = Vec::with_capacity(subs);
    for j in 0..subs {
        let mut e = Work::default();
        let share = total / (subs - j) as f32;
        let mut left = share;
        if share >= unit * 0.5 {
            loop {
                if nf < ins {
                    (e.fft, nf, left) = (e.fft + 1, nf + 1, left - 1.0);
                    if nf >= ins {
                        unit = mac_cost;
                    }
                } else if nm < most {
                    (e.mac, nm, left) = (e.mac + 1, nm + 1, left - mac_cost);
                    if nm >= most {
                        unit = 1.0;
                    }
                } else if ni < outs {
                    (e.ifft, ni, left) = (e.ifft + 1, ni + 1, left - 1.0);
                } else {
                    left = 0.0;
                }
                if left < unit * 0.5 {
                    break;
                }
            }
        }
        total -= share - left;
        if j + 1 == subs && ni < outs {
            e.ifft = e.ifft + outs - ni;
        }
        table.push(e);
    }
    table
}

#[derive(Debug)]
struct Conv {
    block: usize,
    bins: usize,
    ins: usize,
    outs: usize,
    ring: usize,
    pos: usize,
    sub: usize,
    sel: usize,
    parts: [usize; 2],
    avail: [usize; 2],
    fft_done: usize,
    mac_done: usize,
    ifft_done: usize,
    started: bool,
    input: [Vec<f32>; 2],
    fdl: Vec<f32>,
    acc: Vec<f32>,
    out: [Vec<f32>; 2],
    table: Vec<Work>,
    fft: RealFft,
}

impl Conv {
    fn new(block: usize, bins: usize, ins: usize, outs: usize, ring: usize) -> Self {
        let stride = Self::stride(block);
        Self {
            block,
            bins,
            ins,
            outs,
            ring,
            pos: 0,
            sub: 0,
            sel: 0,
            parts: [0; 2],
            avail: [0; 2],
            fft_done: 0,
            mac_done: 0,
            ifft_done: 0,
            started: false,
            input: [vec![0.0; ins * stride], vec![0.0; ins * stride]],
            fdl: vec![0.0; ring * ins * 2 * bins],
            acc: vec![0.0; outs * stride],
            out: [vec![0.0; outs * block], vec![0.0; outs * block]],
            table: Vec::new(),
            fft: RealFft::new(block),
        }
    }

    fn stride(block: usize) -> usize {
        (2 * block + 2).div_ceil(16) * 16
    }

    fn grow(&mut self, parts: usize) {
        if parts <= self.ring {
            return;
        }
        let per = self.ins * 2 * self.bins;
        let mut fdl = vec![0.0; parts * per];
        fdl[..self.pos * per].copy_from_slice(&self.fdl[..self.pos * per]);
        let tail = (self.ring - self.pos) * per;
        let at = (self.pos + parts - self.ring) * per;
        fdl[at..at + tail].copy_from_slice(&self.fdl[self.pos * per..]);
        (self.fdl, self.ring) = (fdl, parts);
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum SlotState {
    #[default]
    Empty,
    Pending,
    Released,
    Active,
}

#[derive(Debug, Default)]
struct Slot {
    state: SlotState,
    ir: Option<Arc<ImpulseResponse>>,
    env: Vec<f32>,
    built: (i32, f32, f32),
}

#[derive(Clone, Copy, Debug, Default)]
pub struct ReverbAttrs {
    pub gain: [f32; 2],
    pub env_type: [i32; 2],
    pub env_start: [f32; 2],
    pub env_length: [f32; 2],
}

#[derive(Debug)]
pub struct ReverbIr {
    max_length: f32,
    envelopes: bool,
    ins: usize,
    outs: usize,
    slots: [Slot; 2],
    running: bool,
    conv: Option<Conv>,
}

impl ReverbIr {
    pub fn new(max_length: f32, envelope_mode: i32, ins: usize, outs: usize) -> Self {
        Self {
            max_length,
            envelopes: envelope_mode != 0,
            ins,
            outs,
            slots: Default::default(),
            running: false,
            conv: None,
        }
    }

    pub fn apply(&mut self, slot: usize, ir: Arc<ImpulseResponse>) {
        let other = self.slots[1 - slot].state;
        let parts = ir.partitions();
        if matches!(other, SlotState::Empty | SlotState::Released) || self.conv.is_none() {
            if self.slots[slot].state != SlotState::Empty || other != SlotState::Empty {
                self.release_all();
            }
            let max = (f64::from(self.max_length * 48_000.0)).ceil() as usize;
            let ring = parts.max(max.div_ceil(ir.block));
            self.conv = Some(Conv::new(ir.block, ir.bins, self.ins, self.outs, ring));
        } else if let Some(c) = self.conv.as_mut() {
            c.grow(parts);
        }
        let ring = self.conv.as_ref().map_or(parts, |c| c.ring);
        let s = &mut self.slots[slot];
        if self.envelopes && (s.env.is_empty() || s.env.len() < parts) {
            s.env.resize((parts.max(ring) + 3) & !3, 0.0);
        }
        s.ir = Some(ir);
        s.state = SlotState::Pending;
    }

    pub fn release_all(&mut self) {
        self.slots = Default::default();
        self.running = false;
        self.conv = None;
    }
}

impl ReverbIr {
    pub fn process(&mut self, a: &ReverbAttrs, buf: &mut Vec<Vec<f32>>) {
        if self.running
            && let Some(k) = self.slots.iter().position(|s| s.state == SlotState::Released)
        {
            self.slots[k].state = SlotState::Empty;
            if let Some(c) = self.conv.as_mut() {
                (c.parts[k], c.avail[k]) = (0, 0);
            }
        }
        if self.slots.iter().any(|s| s.state == SlotState::Pending) {
            for (k, s) in self.slots.iter_mut().enumerate() {
                if s.state == SlotState::Pending {
                    if let Some(c) = self.conv.as_mut() {
                        c.parts[k] = s.ir.as_ref().map_or(0, |ir| ir.partitions());
                    }
                    (s.state, s.built.0) = (SlotState::Active, -1);
                }
            }
            if let Some(c) = self.conv.as_mut() {
                let cut = (c.block - c.bins) as f32 / c.block as f32 * 100.0;
                c.table = schedule(c.block, c.parts, c.ins, c.outs, cut, c.block / BLOCK);
            }
            self.running = true;
        }
        let Some(conv) = self.conv.as_mut().filter(|_| self.running) else {
            *buf = vec![vec![0.0; BLOCK]; self.outs];
            return;
        };
        let mut env_on = [false; 2];
        if self.envelopes {
            for (k, s) in self.slots.iter_mut().enumerate() {
                if !matches!(s.state, SlotState::Pending | SlotState::Active) || a.env_type[k] == 0 {
                    continue;
                }
                let want = (a.env_type[k], a.env_start[k], a.env_length[k]);
                if s.built != want {
                    s.built = want;
                    build_envelope(&mut s.env, want);
                }
                env_on[k] = true;
            }
        }
        *buf = conv.process(buf, &self.slots, a.gain, env_on);
    }
}

fn build_envelope(env: &mut [f32], (kind, start, length): (i32, f32, f32)) {
    let n = env.len();
    let m = (n as f32 * length) as i32;
    let count = if length > 1.0 { n } else { m.max(0) as usize };
    match kind {
        1 => {
            let d = start / m as f32;
            let mut v = start;
            for e in env.iter_mut().take(count) {
                *e = v;
                v -= d;
            }
        }
        2 => {
            let r = 1.0 / m as f32;
            for (k, e) in env.iter_mut().take(count).enumerate() {
                let t = (-(k as f32)).mul_add(r, 1.0);
                *e = t * t * start;
            }
        }
        _ => {}
    }
    env.iter_mut().skip(count).for_each(|e| *e = 0.0);
}

impl Conv {
    fn process(&mut self, input: &[Vec<f32>], slots: &[Slot; 2], gain: [f32; 2], env_on: [bool; 2]) -> Vec<Vec<f32>> {
        let (block, stride, values) = (self.block, Self::stride(self.block), 2 * self.bins);
        if !self.started {
            self.input[1].fill(0.0);
            self.acc.fill(0.0);
            self.out[0].fill(0.0);
            self.out[1].fill(0.0);
        }
        for (c, x) in input.iter().enumerate().take(self.ins) {
            let at = c * stride + BLOCK * self.sub;
            self.input[self.sel][at..at + BLOCK].copy_from_slice(&x[..BLOCK]);
        }
        let w = self.table.get(self.sub).copied().unwrap_or_default();
        if w.fft > 0 {
            let other = 1 - self.sel;
            for c in self.fft_done..(self.fft_done + w.fft).min(self.ins) {
                let buf = &mut self.input[other][c * stride..(c + 1) * stride];
                buf[block..2 * block + 2].fill(0.0);
                if self.started {
                    self.fft.forward(buf);
                }
                let at = (self.pos * self.ins + c) * values;
                self.fdl[at..at + values].copy_from_slice(&buf[..values]);
            }
            self.fft_done += w.fft;
            if self.fft_done == self.ins {
                for (a, p) in self.avail.iter_mut().zip(self.parts) {
                    if *a < p {
                        *a += 1;
                    }
                }
            }
        }
        if w.mac > 0 {
            self.multiply(w.mac, slots, gain, env_on);
            self.mac_done += w.mac;
        }
        if w.ifft > 0 {
            for c in self.ifft_done..(self.ifft_done + w.ifft).min(self.outs) {
                self.fft.inverse(&mut self.acc[c * stride..(c + 1) * stride]);
            }
            self.ifft_done += w.ifft;
        }
        if self.sub + 1 < self.table.len() {
            self.sub += 1;
        } else {
            let (t, o) = (1 - self.sel, self.sel);
            for c in 0..self.outs {
                let y = &self.acc[c * stride..c * stride + 2 * block];
                for k in 0..block {
                    self.out[t][c * block + k] += y[k];
                    self.out[o][c * block + k] = y[block + k];
                }
            }
            self.pos = if self.pos + 1 >= self.ring { 0 } else { self.pos + 1 };
            self.sel = 1 - self.sel;
            (self.sub, self.fft_done, self.mac_done, self.ifft_done) = (0, 0, 0, 0);
        }
        self.started = true;
        (0..self.outs)
            .map(|c| {
                let at = c * block + BLOCK * self.sub;
                self.out[self.sel][at..at + BLOCK].to_vec()
            })
            .collect()
    }
}

impl Conv {
    fn multiply(&mut self, n: usize, slots: &[Slot; 2], gain: [f32; 2], env_on: [bool; 2]) {
        let (stride, values) = (Self::stride(self.block), 2 * self.bins);
        let end = (self.mac_done + n).min(self.avail[0].max(self.avail[1]));
        for c in 0..self.outs {
            let y = &mut self.acc[c * stride..(c + 1) * stride];
            if self.mac_done == 0 {
                y.fill(0.0);
            }
            for k in self.mac_done..end {
                let ring_at = (self.pos + self.ring - k % self.ring) % self.ring;
                let x_at = if self.ins == 1 { ring_at } else { ring_at * self.ins + c } * values;
                let x = &self.fdl[x_at..x_at + values];
                let mut recs: [Option<(f32, &[f32])>; 2] = [None, None];
                let mut g = gain;
                for s in 0..2 {
                    let Some(ir) = slots[s].ir.as_ref() else { continue };
                    if k < self.parts[s] && k < self.avail[s] {
                        let i = if ir.channels == 1 { k } else { k * ir.channels + c };
                        recs[s] = Some(ir.record(i));
                    }
                    if env_on[s] && k < slots[s].env.len() {
                        g[s] = slots[s].env[k] * gain[s];
                    }
                }
                mac(recs, g, x, &mut y[..values]);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flat_ir(parts: usize) -> Arc<ImpulseResponse> {
        let (bins, scale) = (192, 1023.0);
        let data = (0..parts).flat_map(|_| (0..bins).flat_map(|_| [scale, 0.0])).collect();
        Arc::new(ImpulseResponse {
            channels: 1,
            samples: parts * 1024,
            block: 1024,
            bins,
            scales: vec![scale; parts],
            data,
        })
    }

    #[test]
    fn real_fft_round_trip_doubles_and_matches_the_dft() {
        let f = RealFft::new(1024);
        let mut buf = vec![0.0f32; 2064];
        let x: Vec<f32> = (0..2048).map(|i| ((i * 37 % 101) as f32 / 50.0 - 1.0) * 0.5).collect();
        buf[..2048].copy_from_slice(&x);
        f.forward(&mut buf);
        let (mut re, mut im) = (0.0f64, 0.0f64);
        for (m, &v) in x.iter().enumerate() {
            let a = -std::f64::consts::PI * 5.0 * m as f64 / 1024.0;
            (re, im) = (re + f64::from(v) * a.cos(), im + f64::from(v) * a.sin());
        }
        assert!((f64::from(buf[10]) - re).abs() < 2e-3 && (f64::from(buf[11]) - im).abs() < 2e-3);
        f.inverse(&mut buf);
        for (y, v) in buf.iter().zip(&x) {
            assert!((y - 2.0 * v).abs() < 1e-4, "{y} vs {}", 2.0 * v);
        }
    }

    #[test]
    fn schedule_transforms_first_then_multiplies_then_inverts() {
        let t = schedule(1024, [108, 109], 4, 4, 81.25, 4);
        assert_eq!(t.len(), 4);
        let sum = |f: fn(&Work) -> usize| t.iter().map(f).sum::<usize>();
        assert_eq!((sum(|w| w.fft), sum(|w| w.mac), sum(|w| w.ifft)), (4, 109, 4));
        assert_eq!(t[0].fft, 4);
        let first_ifft = t.iter().position(|w| w.ifft > 0).unwrap();
        assert!(t[first_ifft + 1..].iter().all(|w| w.mac == 0));
    }

    #[test]
    fn envelopes_follow_their_shapes() {
        let mut e = vec![9.0; 112];
        build_envelope(&mut e, (2, 1.0, 1.0));
        assert_eq!(e[0], 1.0);
        assert!((e[56] - 0.25).abs() < 1e-6 && e[111] > 0.0 && e[111] < 1e-4);
        build_envelope(&mut e, (1, 1.0, 0.5));
        assert!((e[28] - 0.5).abs() < 1e-5 && e[56] == 0.0 && e[111] == 0.0);
    }

    #[test]
    fn an_impulse_comes_out_once_per_partition_1792_samples_late() {
        let mut r = ReverbIr::new(0.0, 0, 1, 1);
        r.apply(0, flat_ir(2));
        let a = ReverbAttrs { gain: [1.0, 0.0], ..Default::default() };
        let mut out = Vec::new();
        for b in 0..24 {
            let mut buf = vec![vec![0.0f32; BLOCK]];
            if b == 0 {
                buf[0][0] = 1.0;
            }
            r.process(&a, &mut buf);
            out.extend_from_slice(&buf[0]);
        }
        let peak = 2.0 * 383.0 / 2048.0;
        for at in [1792, 2816] {
            assert!((out[at] - peak).abs() < 1e-3, "{at}: {}", out[at]);
        }
        for at in [1792, 2816] {
            assert!(out[at - 3..at].iter().chain(&out[at + 1..at + 4]).all(|v| v.abs() < out[at]));
        }
        assert!(out[..1700].iter().all(|v| v.abs() < 1e-6));
    }

    #[test]
    fn silent_until_applied_and_after_release() {
        let mut r = ReverbIr::new(2.35, 1, 4, 4);
        let a = ReverbAttrs { gain: [1.0, 1.0], ..Default::default() };
        let mut buf = vec![vec![1.0f32; BLOCK]; 4];
        r.process(&a, &mut buf);
        assert_eq!(buf.len(), 4);
        assert!(buf.iter().flatten().all(|&v| v == 0.0));
        r.apply(0, flat_ir(3));
        r.release_all();
        let mut buf = vec![vec![1.0f32; BLOCK]; 4];
        r.process(&a, &mut buf);
        assert!(buf.iter().flatten().all(|&v| v == 0.0));
    }
}
