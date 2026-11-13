mod beat;
mod delay;
mod reverb;

use ssx_formats::vmx;

use crate::sfx_dsp::{BLOCK, Coefs, RATE, biquad};

pub use beat::*;
pub use delay::*;
pub use reverb::*;

pub const FIR_HALF: usize = 33;

pub fn design_band_pass(w: f32, bw: f32) -> [f32; FIR_HALF] {
    let half = bw * 0.5;
    let w1 = (w - half).max(0.0);
    let w2 = (half + w).min(std::f32::consts::PI);
    let mut h = [0.0f32; FIR_HALF];
    for (n, v) in h.iter_mut().enumerate().take(32) {
        let k = n as f32 - 32.0;
        let inv = 1.0 / (std::f32::consts::PI * k);
        *v = (vmx::sin(w2 * k) - vmx::sin(w1 * k)) * inv;
    }
    h[32] = (w2 - w1) * f32::from_bits(0x3EA2_F983);
    let step = 6.283_185_5f32 / 64.0;
    for (n, v) in h.iter_mut().enumerate() {
        let c = if n < 32 { vmx::cos(n as f32 * step) } else { (f64::from(n as f32 * step)).cos() as f32 };
        let win = if n < 32 { 0.54f32 - c * 0.46 } else { (-c).mul_add(0.46, 0.54) };
        *v *= win;
    }
    h
}

#[derive(Clone, Debug)]
pub struct Fir65 {
    hist: Vec<[f32; 64]>,
}

impl Fir65 {
    pub fn new(channels: usize) -> Self {
        Self { hist: vec![[0.0; 64]; channels] }
    }

    pub fn clear(&mut self) {
        self.hist.iter_mut().for_each(|h| h.fill(0.0));
    }

    pub fn process(&mut self, h: &[f32; FIR_HALF], chans: &mut [Vec<f32>]) {
        let mut full = [0.0f32; 65];
        for j in 0..65 {
            full[j] = h[if j <= 32 { j } else { 64 - j }];
        }
        let hists = &mut self.hist;
        crate::sfx_dsp::fused(
            #[inline(always)]
            |f| {
                for (c, buf) in chans.iter_mut().enumerate() {
                    let Some(hist) = hists.get_mut(c) else { continue };
                    let mut ext = Vec::with_capacity(64 + buf.len());
                    ext.extend_from_slice(hist);
                    ext.extend_from_slice(buf);
                    for (n, y) in buf.iter_mut().enumerate() {
                        let taps = full.iter().zip(&ext[n..n + 65]);
                        *y = taps.fold(0.0f32, |s, (c, x)| crate::sfx_dsp::madd(f, *c, *x, s));
                    }
                    hist.copy_from_slice(&ext[ext.len() - 64..]);
                }
            },
        );
    }
}

#[derive(Clone, Debug)]
pub struct BandPassFir64 {
    fir: Fir65,
    taps: [f32; FIR_HALF],
    cached: (f32, f32),
}

impl BandPassFir64 {
    pub fn new(channels: usize) -> Self {
        Self { fir: Fir65::new(channels), taps: [0.0; FIR_HALF], cached: (0.0, 192_000.0) }
    }

    fn valid(w: f32, b: f32) -> bool {
        let half_pi = std::f32::consts::FRAC_PI_2;
        (half_pi < w || b <= (std::f32::consts::PI - w) * 2.0) && (w <= half_pi || b <= w * 2.0)
    }

    pub fn process(&mut self, freq: f64, bandwidth: f64, chans: &mut [Vec<f32>]) {
        let inv = 1.0 / RATE;
        let w = ((freq as f32 * inv) * 6.283_185_5).clamp(0.0, std::f32::consts::PI);
        let b = ((bandwidth as f32 * inv) * 6.283_185_5).max(0.0);
        if Self::valid(w, b) {
            if (w, b) != self.cached {
                self.taps = design_band_pass(w, b);
                self.cached = (w, b);
            }
            self.fir.process(&self.taps, chans);
        } else {
            if Self::valid(self.cached.0, self.cached.1) {
                self.fir.clear();
            }
            self.cached = (w, b);
        }
    }
}

const HILBERT: [Coefs; 4] = [
    [-1.922_875, 0.923_651_3, 0.923_651_3, -1.922_875, 1.0],
    [-0.401_794_9, -0.213_148_8, -0.213_148_8, -0.401_794_9, 1.0],
    [-1.967_767, 0.967_862_7, 0.967_862_7, -1.967_767, 1.0],
    [-1.262_757, 0.347_412_1, 0.347_412_1, -1.262_757, 1.0],
];

#[derive(Clone, Debug)]
pub struct FrequencyShift {
    filter: bool,
    fir: Fir65,
    taps: [f32; FIR_HALF],
    cached: f32,
    ap: [[f32; 4]; 4],
    phase: f32,
}

impl FrequencyShift {
    pub fn new(filter: bool) -> Self {
        Self { filter, fir: Fir65::new(1), taps: [0.0; FIR_HALF], cached: -100_000.0, ap: [[0.0; 4]; 4], phase: 0.0 }
    }

    pub fn process(&mut self, shift: f64, chans: &mut [Vec<f32>]) {
        let shift = shift as f32;
        if self.filter {
            if shift != self.cached {
                let (inv, nyq) = (1.0 / RATE, RATE * 0.5);
                let bw = (inv * nyq) * 6.283_185_5;
                let w = (nyq.mul_add(0.5, -shift) * inv) * 6.283_185_5;
                self.taps = design_band_pass(w, bw);
                self.cached = shift;
            }
            self.fir.process(&self.taps, &mut chans[..1]);
        }
        let x = chans[0].clone();
        let (mut i_path, mut q_path) = (x.clone(), x);
        biquad(&HILBERT[0], &mut self.ap[0], &mut i_path);
        biquad(&HILBERT[1], &mut self.ap[1], &mut i_path);
        biquad(&HILBERT[2], &mut self.ap[2], &mut q_path);
        biquad(&HILBERT[3], &mut self.ap[3], &mut q_path);
        let d = (shift / RATE) * 6.283_185_5;
        let mut ph = [self.phase, self.phase + d, d.mul_add(2.0, self.phase), d.mul_add(3.0, self.phase)];
        let step = d * 4.0;
        for k in (0..BLOCK).step_by(4) {
            for l in 0..4 {
                chans[0][k + l] = i_path[k + l] * vmx::cos(ph[l]) - q_path[k + l] * vmx::sin(ph[l]);
                ph[l] += step;
            }
        }
        let end = d.mul_add(256.0, self.phase);
        let turns = (end * f32::from_bits(0x3E22_F983)) as i32 as f32;
        self.phase = (-turns).mul_add(6.283_185_5, end);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(freq: f32, n: usize) -> Vec<f32> {
        (0..n).map(|k| (std::f32::consts::TAU * freq * k as f32 / RATE).sin()).collect()
    }

    fn level(x: &[f32], freq: f32) -> f32 {
        let (mut re, mut im) = (0.0f64, 0.0f64);
        for (k, &v) in x.iter().enumerate() {
            let a = std::f64::consts::TAU * f64::from(freq) * k as f64 / f64::from(RATE);
            re += f64::from(v) * a.cos();
            im += f64::from(v) * a.sin();
        }
        (2.0 * (re * re + im * im).sqrt() / x.len() as f64) as f32
    }

    #[test]
    fn band_pass_taps_are_the_windowed_ideal_response() {
        let (w, bw) = (1.0f32, 0.8f32);
        let h = design_band_pass(w, bw);
        let (w1, w2) = (f64::from(w - bw / 2.0), f64::from(w + bw / 2.0));
        for (n, &v) in h.iter().enumerate() {
            let k = n as f64 - 32.0;
            let ideal = if k == 0.0 {
                (w2 - w1) / std::f64::consts::PI
            } else {
                ((w2 * k).sin() - (w1 * k).sin()) / (std::f64::consts::PI * k)
            };
            let win = 0.54 - 0.46 * (std::f64::consts::TAU * n as f64 / 64.0).cos();
            assert!((f64::from(v) - ideal * win).abs() < 2e-6, "tap {n}");
        }
    }

    #[test]
    fn band_pass_filters_inside_and_bypasses_outside_its_range() {
        let mut f = BandPassFir64::new(1);
        let mut x = vec![[tone(2000.0, 2048), tone(12_000.0, 2048)].concat()];
        let blocks: Vec<Vec<f32>> = x[0].chunks(BLOCK).map(<[f32]>::to_vec).collect();
        let mut out = Vec::new();
        for b in blocks {
            let mut c = vec![b];
            f.process(2000.0, 2000.0, &mut c);
            out.extend_from_slice(&c[0]);
        }
        assert!((level(&out[512..2048], 2000.0) - 1.0).abs() < 0.05);
        assert!(level(&out[2560..4096], 12_000.0) < 0.01);
        let mut c = vec![x[0][..BLOCK].to_vec()];
        f.process(2000.0, 48_000.0, &mut c);
        assert_eq!(c[0], x[0][..BLOCK]);
        x.clear();
    }

    #[test]
    fn frequency_shifter_moves_a_tone() {
        let mut fs = FrequencyShift::new(true);
        let x = tone(3000.0, BLOCK * 32);
        let mut out = Vec::new();
        for b in x.chunks(BLOCK) {
            let mut c = vec![b.to_vec()];
            fs.process(-1000.0, &mut c);
            out.extend_from_slice(&c[0]);
        }
        let tail = &out[BLOCK * 8..];
        assert!(level(tail, 2000.0) > 0.9, "shifted tone {}", level(tail, 2000.0));
        assert!(level(tail, 3000.0) < 0.05 && level(tail, 4000.0) < 0.05);
    }

    #[test]
    fn delay_repeats_with_feedback() {
        let mut d = Delay::new(1, 1.0);
        let mut x = vec![0.0f32; BLOCK * 8];
        x[0] = 1.0;
        let mut out = Vec::new();
        for b in x.chunks(BLOCK) {
            let mut c = vec![b.to_vec()];
            d.process(300.0 / f64::from(RATE), 0.5, &mut c);
            out.extend_from_slice(&c[0]);
        }
        assert_eq!(out[0], 0.0, "the dry signal is not output");
        assert_eq!(out[300], 1.0);
        assert_eq!(out[600], 0.5);
        assert_eq!(out[900], 0.25);
        let mut c = vec![vec![0.3; BLOCK]];
        d.process(0.0, 0.5, &mut c);
        assert!(c[0].iter().all(|&v| v == 0.3));
    }

    #[test]
    fn vibrato_without_depth_is_a_511_sample_delay_and_chorus_without_taps_is_dry() {
        let mut v = ModDelay::new(1, 0.03);
        let mut x = vec![0.0f32; BLOCK * 4];
        x[10] = 1.0;
        let mut out = Vec::new();
        for b in x.chunks(BLOCK) {
            let mut c = vec![b.to_vec()];
            v.vibrato(0.0, 0.5);
            v.process(&mut c);
            out.extend_from_slice(&c[0]);
        }
        assert_eq!(out[10 + 511], 1.0);
        assert_eq!(out.iter().filter(|&&s| s != 0.0).count(), 1);
        let mut ch = ModDelay::new(2, 0.06);
        let mut c = vec![vec![0.25; BLOCK], vec![-0.5; BLOCK]];
        ch.chorus(0, 0.01, 0.02, 0.3);
        ch.process(&mut c);
        assert!(c[0].iter().all(|&s| s == 0.25) && c[1].iter().all(|&s| s == -0.5));
    }

    #[test]
    fn reverb_rings_out_and_mutes_at_time_zero() {
        let mut r = Reverb::new(50.0);
        let mut first = vec![vec![1.0f32; BLOCK]];
        r.process(3.0, 40.0, 50.0, &mut first);
        assert!(first[0].iter().all(|&s| s == 0.0), "the block that starts it is silent");
        let mut energy = Vec::new();
        for k in 0..400 {
            let mut c = vec![vec![if k == 0 { 1.0 } else { 0.0 }; BLOCK]];
            r.process(3.0, 40.0, 50.0, &mut c);
            energy.push(c[0].iter().map(|s| s * s).sum::<f32>());
        }
        let early: f32 = energy[1..60].iter().sum();
        let late: f32 = energy[340..400].iter().sum();
        assert!(early > 0.0 && late < early * 0.1, "decays: {early} → {late}");
        let mut c = vec![vec![1.0f32; BLOCK]];
        r.process(0.0, 40.0, 50.0, &mut c);
        assert!(c[0].iter().all(|&s| s == 0.0));
    }

    #[test]
    fn beat_detector_finds_a_click_track_tempo() {
        let mut b = BeatDetect::new(2, 1.0, 1.0, 300.0);
        let (mut bpm, mut conf, mut meter) = (0.0, 0.0, 0.0);
        for blk in 0..(48_000 * 16 / BLOCK) {
            let block: Vec<f32> = (0..BLOCK)
                .map(|k| {
                    let t = blk * BLOCK + k;
                    let noise = ((t * 7919) % 1000) as f32 / 500.0 - 1.0;
                    noise * if t % 24_000 < 256 { 0.5 } else { 0.01 }
                })
                .collect();
            let mut c = vec![block.clone(), block];
            b.process(4.0, 0.03, &mut c);
            [bpm, _, conf, meter, _] = b.outputs();
        }
        assert!((bpm - 120.0).abs() < 3.0, "bpm {bpm}");
        assert!(conf >= 0.5, "confidence {conf}");
        assert_eq!(meter, 2.0);
    }

    #[test]
    fn beat_detector_delays_the_audio_nine_blocks() {
        let mut b = BeatDetect::new(1, 1.0, 1.0, 300.0);
        let mut out = Vec::new();
        for blk in 0..12 {
            let mut c = vec![vec![0.0f32; BLOCK]];
            if blk == 0 {
                c[0][5] = 1.0;
            }
            b.process(4.0, 0.03, &mut c);
            out.extend_from_slice(&c[0]);
        }
        assert_eq!(out[9 * BLOCK + 5], 1.0);
        assert_eq!(out.iter().filter(|&&v| v != 0.0).count(), 1);
    }

    #[test]
    fn reverb_line_lengths_are_the_next_primes() {
        let mut r = Reverb::new(50.0);
        let mut space = 40.0f32;
        r.distances(&mut space);
        r.lengths();
        let want: Vec<usize> = r.dist.iter().map(|&d| (RATE * d * 0.002_900_232) as usize).collect();
        for (l, w) in r.len.iter().zip(want) {
            assert!(*l > w && (w + 1..*l).all(|n| (2..n).any(|k| n % k == 0)), "{l} after {w}");
        }
    }
}
