// PORT(choice): length, cutoff and window picked for a flat band to 18 kHz and 80 dB rejection at a 44.1 kHz device (a
// 48 kHz device never comes here); nothing of the game's behind it (the console mixes at 48 kHz).
const HALF: usize = 32;
const TAPS: usize = 2 * HALF;
const PHASES: usize = 256;
const CUTOFF: f64 = 0.913;
const BETA: f64 = 8.0;

pub struct Resampler {
    step: f64,
    pos: f64,
    channels: usize,
    buf: Vec<f32>,
    table: Vec<f32>,
}

fn bessel_i0(x: f64) -> f64 {
    let (mut sum, mut term, mut k) = (1.0, 1.0, 1.0);
    while term > 1e-12 * sum {
        term *= (x / (2.0 * k)) * (x / (2.0 * k));
        sum += term;
        k += 1.0;
    }
    sum
}

impl Resampler {
    pub fn new(from: u32, to: u32) -> Self {
        Self::with_channels(from, to, 2)
    }

    pub fn with_channels(from: u32, to: u32, channels: usize) -> Self {
        let channels = channels.max(1);
        let (from, to) = (f64::from(from.max(1)), f64::from(to.max(1)));
        let fc = CUTOFF * (to / from).min(1.0);
        let mut table = vec![0.0f32; (PHASES + 1) * TAPS];
        for (phase, row) in table.chunks_mut(TAPS).enumerate() {
            let frac = phase as f64 / PHASES as f64;
            let mut w = [0.0f64; TAPS];
            for (k, w) in w.iter_mut().enumerate() {
                let d = k as f64 - (HALF as f64 - 1.0) - frac;
                let x = std::f64::consts::PI * fc * d;
                let sinc = if x.abs() < 1e-9 { 1.0 } else { x.sin() / x };
                let r = (d / HALF as f64).abs().min(1.0);
                *w = fc * sinc * bessel_i0(BETA * (1.0 - r * r).sqrt()) / bessel_i0(BETA);
            }
            let sum: f64 = w.iter().sum();
            for (o, w) in row.iter_mut().zip(w) {
                *o = (w / sum) as f32;
            }
        }
        Self { step: from / to, pos: HALF as f64 - 1.0, channels, buf: vec![0.0; channels * (HALF - 1)], table }
    }

    pub fn need(&self, out_frames: usize) -> usize {
        if out_frames == 0 {
            return 0;
        }
        let last = (self.pos + (out_frames - 1) as f64 * self.step).floor() as usize + HALF;
        (last + 1).saturating_sub(self.buf.len() / self.channels)
    }

    pub fn process(&mut self, input: &[f32], out: &mut [f32]) {
        let ch = self.channels;
        self.buf.extend_from_slice(input);
        let have = self.buf.len() / ch;
        for frame in out.chunks_exact_mut(ch) {
            let i = self.pos.floor();
            let first = i as usize + 1 - HALF;
            if first + TAPS > have {
                frame.fill(0.0);
                self.pos += self.step;
                continue;
            }
            let phase = (self.pos - i) * PHASES as f64;
            let (p0, t) = (phase.floor() as usize, (phase - phase.floor()) as f32);
            let (a, b) = (&self.table[p0 * TAPS..(p0 + 1) * TAPS], &self.table[(p0 + 1) * TAPS..(p0 + 2) * TAPS]);
            frame.fill(0.0);
            for (k, s) in self.buf[ch * first..ch * (first + TAPS)].chunks_exact(ch).enumerate() {
                let w = a[k] + (b[k] - a[k]) * t;
                for (o, x) in frame.iter_mut().zip(s) {
                    *o += w * x;
                }
            }
            self.pos += self.step;
        }
        let drop = ((self.pos.floor() as usize + 1).saturating_sub(HALF)).min(self.buf.len() / ch);
        self.buf.drain(..ch * drop);
        self.pos -= drop as f64;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(hz: f64, to: u32, seconds: f64) -> (f64, i64) {
        let mut r = Resampler::new(48_000, to);
        let (mut n_in, mut sum, mut n_out) = (0usize, 0.0f64, 0usize);
        let total = (seconds * f64::from(to)) as usize;
        let mut chunk = 0;
        while n_out < total {
            chunk = (chunk * 7 + 13) % 997 + 1;
            let need = r.need(chunk);
            let input: Vec<f32> = (n_in..n_in + need)
                .flat_map(|i| {
                    let s = (2.0 * std::f64::consts::PI * hz * i as f64 / 48_000.0).sin() as f32;
                    [s, -s]
                })
                .collect();
            n_in += need;
            let mut out = vec![0.0f32; 2 * chunk];
            r.process(&input, &mut out);
            for f in out.chunks(2).skip(if n_out == 0 { 64 } else { 0 }) {
                assert!((f[0] + f[1]).abs() < 1e-6, "the channels stay apart");
                sum += f64::from(f[0]) * f64::from(f[0]);
            }
            n_out += chunk;
        }
        ((sum / n_out as f64).sqrt(), n_in as i64 - (n_out as f64 * 48_000.0 / f64::from(to)).round() as i64)
    }

    #[test]
    fn a_tone_in_the_band_passes_and_one_above_it_is_removed() {
        let (rms, extra) = tone(1000.0, 44_100, 1.0);
        assert!((rms - std::f64::consts::FRAC_1_SQRT_2).abs() < 0.01, "1 kHz: rms {rms}");
        assert!((0..=TAPS as i64).contains(&extra), "{extra} input frames more than the output's duration");
        let (rms, _) = tone(23_000.0, 44_100, 1.0);
        assert!(rms < 1e-3, "23 kHz: rms {rms}");
        let (rms, _) = tone(15_000.0, 44_100, 1.0);
        assert!((rms - std::f64::consts::FRAC_1_SQRT_2).abs() < 0.01, "15 kHz: rms {rms}");
        let (rms, extra) = tone(1000.0, 96_000, 0.5);
        assert!((rms - std::f64::consts::FRAC_1_SQRT_2).abs() < 0.01, "to 96 kHz: rms {rms}");
        assert!((0..=TAPS as i64).contains(&extra), "{extra} input frames more than the output's duration");
    }
}
