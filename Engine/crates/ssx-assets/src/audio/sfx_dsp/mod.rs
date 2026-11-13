pub use dynamics::*;
pub use pan2d::*;
pub use routing::*;
mod dynamics;
mod pan2d;
mod routing;

pub mod pan3d;
pub mod reverb_ir;
pub mod sound3d;

pub const BLOCK: usize = 256;
pub const RATE: f32 = 48_000.0;
const W_MIN: f64 = 0.003_141_592_955_216_765_4;
const W_MAX: f64 = 3.138_451_099_395_752;

#[inline(always)]
pub fn madd(fused: bool, a: f32, b: f32, c: f32) -> f32 {
    if fused { a.mul_add(b, c) } else { a * b + c }
}

// PORT(choice): on CPUs without FMA only — two roundings where the console has one: at most 2 of the 16-bit steps in
// 0.02–1.9 % of the reference rides' samples (0.02 % of a ride's, 1.9 % of `fe_audio`'s front-end walk;
// `SSX_SFX_FMA=off` forces this path); nothing to decode (a soft-float `fmaf` is exact and far too slow).
#[inline(always)]
pub fn fused<R>(kernel: impl FnOnce(bool) -> R) -> R {
    #[cfg(target_arch = "x86_64")]
    {
        #[target_feature(enable = "fma")]
        fn with_fma<R>(kernel: impl FnOnce(bool) -> R) -> R {
            kernel(true)
        }
        static FMA: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        let fma = *FMA.get_or_init(|| {
            std::arch::is_x86_feature_detected!("fma")
                && !std::env::var("SSX_SFX_FMA").is_ok_and(|v| matches!(v.as_str(), "off" | "0"))
        });
        if fma { unsafe { with_fma(kernel) } } else { kernel(false) }
    }
    #[cfg(not(target_arch = "x86_64"))]
    kernel(true)
}

pub fn omega(freq: f64) -> f64 {
    f64::from((freq / f64::from(RATE)) as f32 * 6.283_185_5)
}

pub type Coefs = [f32; 5];

pub fn high_pass(w: f64) -> Coefs {
    let (s, c) = (w.sin() as f32, w.cos() as f32);
    let alpha = s * 0.5;
    let (one_c, a0) = (c + 1.0, alpha + 1.0);
    let inv = 1.0 / a0;
    let a02 = a0 * 2.0;
    [c * -2.0 * inv, (1.0 - alpha) * inv, one_c / a02, -(one_c * inv), one_c / a02]
}

pub fn low_pass(w: f64) -> Coefs {
    let (s, c) = (w.sin() as f32, w.cos() as f32);
    let alpha = s * 0.5;
    let (one_c, a0) = (1.0 - c, alpha + 1.0);
    let inv = 1.0 / a0;
    let a02 = a0 * 2.0;
    [c * -2.0 * inv, (1.0 - alpha) * inv, one_c / a02, one_c * inv, one_c / a02]
}

pub fn high_shelf(w: f64, gain: f64) -> Coefs {
    let (sn, cs) = (w.sin() as f32, w.cos() as f32);
    let a = (gain as f32).sqrt();
    let (ap, am) = (a + 1.0, a - 1.0);
    let beta = sn * 0.707_106_5;
    let sa = a.sqrt() * beta;
    let a0 = sa.mul_add(2.0, (-am).mul_add(cs, ap));
    let inv = 1.0 / a0;
    let b0 = (sa.mul_add(2.0, am * cs) + ap) * inv;
    let b1 = ap.mul_add(cs, am) * inv;
    let b2 = (-sa).mul_add(2.0, am.mul_add(cs, ap)) * inv;
    let a1 = (-ap).mul_add(cs, am) * inv;
    let a2 = (-sa).mul_add(2.0, (-am).mul_add(cs, ap)) * inv;
    [a1 * 2.0, a2, b0 * a, b1 * a * -2.0, b2 * a]
}

pub fn low_shelf(w: f64, gain: f64) -> Coefs {
    let (sn, cs) = (w.sin() as f32, w.cos() as f32);
    let a = (gain as f32).sqrt();
    let (am, ap) = (a - 1.0, a + 1.0);
    let beta = sn * 0.707_106_5;
    let sa = a.sqrt() * beta;
    let a0 = sa.mul_add(2.0, am * cs) + ap;
    let inv = 1.0 / a0;
    let b0 = sa.mul_add(2.0, (-am).mul_add(cs, ap)) * inv;
    let b1 = (-ap).mul_add(cs, am) * inv;
    let b2 = (-sa).mul_add(2.0, (-am).mul_add(cs, ap)) * inv;
    let a1 = ap.mul_add(cs, am) * inv;
    let a2 = (-sa).mul_add(2.0, am.mul_add(cs, ap)) * inv;
    [a1 * -2.0, a2, b0 * a, b1 * a * 2.0, b2 * a]
}

pub fn peaking(w: f64, gain: f64, q: f64) -> Coefs {
    let (sn, cs) = (w.sin() as f32, w.cos() as f32);
    let a = (gain as f32).sqrt();
    let alpha = sn / (q as f32 * 2.0);
    let (da, ma) = (alpha / a, a * alpha);
    let inv = 1.0 / (da + 1.0);
    let a1 = inv * (cs * -2.0);
    [a1, (1.0 - da) * inv, (ma + 1.0) * inv, a1, (1.0 - ma) * inv]
}

pub fn biquad(c: &Coefs, state: &mut [f32; 4], buf: &mut [f32]) {
    fused(
        #[inline(always)]
        |f| {
            let [mut x1, mut x2, mut y1, mut y2] = *state;
            for v in buf.iter_mut() {
                let x = *v;
                let t = madd(f, x2, c[4], x * c[2]);
                let t = madd(f, x1, c[3], t) + 1e-18;
                let y = madd(f, -y1, c[0], t);
                let y = madd(f, -y2, c[1], y);
                (x2, x1, y2, y1) = (x1, x, y1, y);
                *v = y;
            }
            *state = [x1, x2, y1, y2];
        },
    )
}

#[derive(Clone, Debug, Default)]
pub struct Filter {
    pub state: Vec<[f32; 4]>,
    coefs: Coefs,
    made_for: Option<(f32, f32, f32)>,
    active: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FilterKind {
    HighPass,
    LowPass,
    HighShelf,
    LowShelf,
    Peaking,
}

impl Filter {
    pub fn process(&mut self, kind: FilterKind, freq: f64, gain: f64, q: f64, chans: &mut [Vec<f32>]) {
        use FilterKind::*;
        if self.state.len() < chans.len() {
            self.state.resize(chans.len(), [0.0; 4]);
        }
        let mut w = omega(freq);
        let bypass = match kind {
            HighPass => w <= W_MIN,
            LowPass => w >= W_MAX,
            HighShelf => w >= W_MAX || gain == 1.0,
            LowShelf => w <= W_MIN || gain == 1.0,
            Peaking => gain == 1.0,
        };
        if bypass {
            if self.active {
                self.state.iter_mut().for_each(|s| *s = [0.0; 4]);
            }
            self.active = false;
            self.made_for = None;
            return;
        }
        self.active = true;
        w = match kind {
            HighPass | LowShelf => w.min(W_MAX),
            LowPass | HighShelf => w.max(W_MIN),
            Peaking => w.clamp(W_MIN, W_MAX),
        };
        let key = (w as f32, gain as f32, q as f32);
        if self.made_for != Some(key) {
            self.coefs = match kind {
                HighPass => high_pass(w),
                LowPass => low_pass(w),
                HighShelf => high_shelf(w, gain),
                LowShelf => low_shelf(w, gain),
                Peaking => peaking(w, gain, q),
            };
            self.made_for = Some(key);
        }
        for (ch, st) in chans.iter_mut().zip(self.state.iter_mut()) {
            biquad(&self.coefs, st, ch);
        }
    }

    pub fn rest(&mut self, kind: FilterKind, freq: f64, gain: f64) {
        use FilterKind::*;
        let w = omega(freq);
        let bypass = match kind {
            HighPass => w <= W_MIN,
            LowPass => w >= W_MAX,
            HighShelf => w >= W_MAX || gain == 1.0,
            LowShelf => w <= W_MIN || gain == 1.0,
            Peaking => gain == 1.0,
        };
        if bypass {
            if self.active {
                self.state.iter_mut().for_each(|s| *s = [0.0; 4]);
            }
            self.made_for = None;
        }
        self.active = !bypass;
    }
}

pub fn ramp_gain(k: usize, old: f32, new: f32) -> f32 {
    ramp_gain_in(true, k, old, new)
}

#[inline(always)]
pub fn ramp_gain_in(f: bool, k: usize, old: f32, new: f32) -> f32 {
    let d = (new - old) * 0.015_625;
    if k >= 64 {
        return madd(f, d, 64.0, old);
    }
    let lane = match k % 4 {
        0 => old,
        1 => old + d,
        2 => madd(f, d, 2.0, old),
        _ => madd(f, d, 3.0, old),
    };
    let d4 = d * 4.0;
    let base = if k >= 32 { madd(f, 8.0, d4, lane) } else { lane };
    let j = ((k % 32) / 4) as f32;
    if j == 0.0 { base } else { madd(f, j, d4, base) }
}

pub fn apply_gain(buf: &mut [f32], old: f32, new: f32) {
    if old == new {
        if new != 1.0 {
            buf.iter_mut().for_each(|x| *x *= new);
        }
        return;
    }
    fused(
        #[inline(always)]
        |f| {
            for (k, x) in buf.iter_mut().enumerate() {
                *x *= ramp_gain_in(f, k, old, new);
            }
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rbj(kind: &str, w: f64, gain: f64, q: f64) -> [f64; 5] {
        let (s, c) = (w.sin(), w.cos());
        let a = gain.sqrt();
        let (b, a_) = match kind {
            "lp" | "hp" => {
                let al = s / 2.0;
                let (b0, b1) = if kind == "lp" { ((1.0 - c) / 2.0, 1.0 - c) } else { ((1.0 + c) / 2.0, -(1.0 + c)) };
                ([b0, b1, b0], [1.0 + al, -2.0 * c, 1.0 - al])
            }
            "pk" => {
                let al = s / (2.0 * q);
                ([1.0 + al * a, -2.0 * c, 1.0 - al * a], [1.0 + al / a, -2.0 * c, 1.0 - al / a])
            }
            _ => {
                let al = s / 2.0 * 2f64.sqrt();
                let sa = 2.0 * a.sqrt() * al;
                let hs = kind == "hs";
                let k = if hs { 1.0 } else { -1.0 };
                (
                    [
                        a * ((a + 1.0) + k * (a - 1.0) * c + sa),
                        -2.0 * k * a * ((a - 1.0) + k * (a + 1.0) * c),
                        a * ((a + 1.0) + k * (a - 1.0) * c - sa),
                    ],
                    [
                        (a + 1.0) - k * (a - 1.0) * c + sa,
                        2.0 * k * ((a - 1.0) - k * (a + 1.0) * c),
                        (a + 1.0) - k * (a - 1.0) * c - sa,
                    ],
                )
            }
        };
        [a_[1] / a_[0], a_[2] / a_[0], b[0] / a_[0], b[1] / a_[0], b[2] / a_[0]]
    }

    #[test]
    fn filter_coefficients_match_the_cookbook() {
        for &f in &[40.0, 440.0, 2000.0, 9000.0, 20_000.0] {
            let w = omega(f);
            for (kind, got) in [
                ("lp", low_pass(w)),
                ("hp", high_pass(w)),
                ("hs", high_shelf(w, 2.5)),
                ("ls", low_shelf(w, 0.3)),
                ("pk", peaking(w, 1.8, 3.0)),
            ] {
                let gain = match kind {
                    "hs" => 2.5,
                    "ls" => 0.3,
                    _ => 1.8,
                };
                let want = rbj(kind, w, gain, 3.0);
                for (g, w2) in got.iter().zip(want) {
                    assert!(
                        (f64::from(*g) - w2).abs() < 2e-5 * w2.abs().max(1.0),
                        "{kind} {f} Hz: {got:?} vs {want:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn biquad_passes_dc_through_the_low_pass() {
        let c = low_pass(omega(1000.0));
        let mut st = [0.0; 4];
        let mut buf = vec![1.0f32; 4096];
        biquad(&c, &mut st, &mut buf);
        assert!((buf[4095] - 1.0).abs() < 1e-4, "{}", buf[4095]);
        let c = high_pass(omega(1000.0));
        let (mut st, mut buf) = ([0.0; 4], vec![1.0f32; 4096]);
        biquad(&c, &mut st, &mut buf);
        assert!(buf[4095].abs() < 1e-4);
    }

    #[test]
    fn gain_ramps_over_64_samples() {
        assert_eq!(ramp_gain(0, 0.5, 1.5), 0.5);
        assert!((ramp_gain(33, 0.5, 1.5) - (0.5 + 33.0 / 64.0)).abs() < 1e-6);
        assert_eq!(ramp_gain(64, 0.5, 1.5), 1.5);
        assert_eq!(ramp_gain(200, 0.5, 1.5), 1.5);
        let r: Vec<f32> = (0..64).map(|k| ramp_gain(k, 1.0, 0.0)).collect();
        assert!(r.windows(2).all(|w| w[1] < w[0]));
    }

    #[test]
    fn channel_conversions_are_the_tables() {
        let mut t = channel_terms(6, 2);
        t.sort_by(|a, b| a.partial_cmp(b).unwrap());
        assert_eq!(t, vec![(0, 0, 1.0), (1, 0, 0.707), (1, 1, 0.707), (2, 1, 1.0), (3, 0, 1.0), (4, 1, 1.0)]);
        assert_eq!(channel_terms(1, 6), vec![(0, 1, 1.0)]);
        assert_eq!(channel_terms(1, 2), vec![(0, 0, 0.707), (0, 1, 0.707)]);
        assert_eq!(channel_terms(2, 1), vec![(0, 0, 1.0), (1, 0, 1.0)]);
        assert!(channel_terms(3, 2).is_empty());
        let out = rechannel(&[vec![1.0; 4]], 6);
        assert_eq!(out[1][0], 1.0);
        assert_eq!(out[0][0], 0.0);
    }

    #[test]
    fn resampler_steps_in_16_16() {
        assert_eq!(Resampler::step(1.0), 65536);
        assert_eq!(Resampler::step(9.0), 0x40000);
        assert_eq!(Resampler::step(44_100.0 / 48_000.0), 60211);
        let mut r = Resampler::default();
        r.advance(0x18000);
        assert_eq!((r.index, r.frac), (1, 0x8000));
        assert!((r.sample(2.0, 4.0) - 3.0).abs() < 1e-4);
    }

    #[test]
    fn compressor_settles_on_the_static_curve() {
        let setup = CompressorSetup::new(-12.0, 4.0, 0.001, 0.05, 1);
        let mut c = Compressor::default();
        let level = 0.8f32;
        let mut last = 0.0;
        for _ in 0..200 {
            let mut buf = vec![vec![level; BLOCK]];
            c.process(-12.0, 4.0, &setup, &mut buf, None);
            last = buf[0][BLOCK - 1];
        }
        let thr = 10f32.powf(-12.0 / 20.0);
        let want = level * (level / thr).powf(1.0 / 4.0 - 1.0);
        assert!((last - want).abs() < 2e-3, "{last} vs {want}");
        let mut quiet = vec![vec![0.1f32; BLOCK]];
        c.process(20.0, 4.0, &setup, &mut quiet, None);
        assert_eq!(quiet[0][0], 0.1);
    }

    #[test]
    fn pan2d_places_mono_on_the_speakers() {
        let g = pan2d_gains(&PanParams::default(), 1, 6, 2, 30.0, 110.0);
        assert!((g[0][1] - 1.0).abs() < 1e-5 && g[0][0].abs() < 1e-5 && g[0][2].abs() < 1e-5, "{g:?}");
        let right = PanParams { angle: 30.0, ..Default::default() };
        let g = pan2d_gains(&right, 1, 6, 2, 30.0, 110.0);
        assert!((g[0][2] - 1.0).abs() < 1e-4, "{g:?}");
        let rear = PanParams { angle: -110.0, ..Default::default() };
        let g = pan2d_gains(&rear, 1, 6, 2, 30.0, 110.0);
        assert!((g[0][3] - 1.0).abs() < 1e-4, "{g:?}");
        let g = pan2d_gains(&PanParams::default(), 1, 2, 2, 30.0, 110.0);
        assert!((g[0][0] - g[0][1]).abs() < 1e-6 && (g[0][0].powi(2) * 2.0 - 1.0).abs() < 1e-5);
    }
}
