use super::*;

// TODO(guess): a tap at delay D reads the sample written exactly D samples earlier (Delay; the reverb combs at their length L) — the framework's chunk and read-pointer bookkeeping (0x82A29EB8 / 0x82A29D70 / 0x82A29CC8) is not traced to the sample; settle: there.
#[derive(Clone, Debug)]
struct Ring {
    buf: Vec<Vec<f32>>,
    pos: usize,
}

impl Ring {
    pub(super) fn new(channels: usize, len: usize) -> Self {
        Self { buf: vec![vec![0.0; len.max(1)]; channels], pos: 0 }
    }

    pub(super) fn len(&self) -> usize {
        self.buf.first().map_or(1, Vec::len)
    }

    pub(super) fn clear(&mut self) {
        self.buf.iter_mut().for_each(|b| b.fill(0.0));
    }

    pub(super) fn get(&self, c: usize, back: usize) -> f32 {
        let n = self.len();
        self.buf[c][wrap(self.pos + n - wrap(back, n), n)]
    }

    pub(super) fn push(&mut self, c: usize, x: f32) {
        self.buf[c][self.pos] = x;
    }

    fn advance(&mut self) {
        self.pos = wrap(self.pos + 1, self.len());
    }
}

#[derive(Clone, Debug)]
pub struct Delay {
    ring: Ring,
    state: u8,
    delay: usize,
    old_delay: usize,
    fb: f32,
    old_fb: f32,
    fade: bool,
}

impl Delay {
    pub fn new(channels: usize, max_delay: f64) -> Self {
        let len = (max_delay as f32 * RATE + 0.5) as usize + 256;
        Self { ring: Ring::new(channels, len), state: 0, delay: 0, old_delay: 0, fb: 0.0, old_fb: 0.0, fade: false }
    }

    fn set_feedback(&mut self, fb: f32) {
        self.old_fb = self.fb;
        self.fb = fb.clamp(-0.99, 0.99);
    }

    pub fn process(&mut self, delay: f64, feedback: f64, chans: &mut [Vec<f32>]) {
        let t = delay as f32 * RATE;
        let d = (if t < 0.0 { t - 0.5 } else { t + 0.5 }) as i32;
        let cap = self.ring.len() as i32 - 1;
        match self.state {
            0 if d >= 1 => {
                self.ring.clear();
                self.set_feedback(feedback as f32);
                self.fade = false;
                self.delay = d as usize;
                self.state = 1;
            }
            1 | 2 if d < 1 => self.state = 0,
            1 | 2 if d <= cap => {
                self.set_feedback(feedback as f32);
                if d as usize != self.delay {
                    self.old_delay = self.delay;
                    self.delay = d as usize;
                    self.fade = true;
                }
                self.state = 1;
            }
            1 => self.state = 2,
            _ => {}
        }
        if self.state == 0 {
            return;
        }
        let channels = self.ring.buf.len();
        // PORT(choice): run in the FMA build of `fused` where the CPU has FMA, so the feedback's multiply-add a sample
        // is the instruction in line instead of a call to the runtime's `fmaf` (the same single rounding: the same
        // samples bit for bit); the flag is not used (it stays fused without FMA too, as before); the console has the
        // kernels 0x82A51EB8 / 0x82A51D40, whose multiply-add this is.
        crate::sfx_dsp::fused(
            #[inline(always)]
            |_| {
                for k in 0..BLOCK {
                    for (c, buf) in chans.iter_mut().enumerate().take(channels) {
                        let x = buf[k];
                        let new = self.ring.get(c, self.delay);
                        let (y, w) = if self.fade && k < 128 {
                            let r = 0.992_187_5 - k as f32 * 0.007_812_5;
                            let old = self.ring.get(c, self.old_delay);
                            let a = (1.0 - r) * new;
                            let w = f64::from(a) * f64::from(self.fb) + f64::from((old * self.old_fb) * r);
                            (a + old * r, (w as f32) + x)
                        } else {
                            (new, new.mul_add(self.fb, x))
                        };
                        self.ring.push(c, w);
                        buf[k] = y;
                    }
                    self.ring.advance();
                }
            },
        );
        self.fade = false;
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct ModTap {
    pub gain: f32,
    pub fixed: u32,
    pub freq: f32,
    pub depth: u32,
    pub phase: f32,
}

#[derive(Clone, Debug)]
pub struct ModDelay {
    ring: Vec<Vec<f32>>,
    pos: usize,
    pub dry: f32,
    pub taps: Vec<ModTap>,
    cache: [f32; 4],
    rebuild: bool,
}

impl ModDelay {
    pub fn new(channels: usize, max_delay: f64) -> Self {
        let len = ((max_delay as f32 * RATE) as usize + 512 + 0xFF) & !0xFF;
        Self {
            ring: vec![vec![0.0; len]; channels],
            pos: 0,
            dry: 1.0,
            taps: Vec::new(),
            cache: [0.0; 4],
            rebuild: true,
        }
    }

    pub fn chorus(&mut self, taps: i32, fixed: f64, depth: f64, freq: f64) {
        let key = [taps as f32, fixed as f32, depth as f32, freq as f32];
        if !self.rebuild && key == self.cache {
            return;
        }
        let n = taps.max(0) as u32;
        let c = f64::from(f32::from_bits(0x3F35_0481));
        let g = (f64::from((c * c) as f32) / f64::from(n.max(1))).sqrt() as f32;
        self.taps = (0..n)
            .map(|i| {
                let sign = if i % 2 == 1 { -1.0f32 } else { 1.0 };
                let s = (sign * i as f32) * 0.1f32 + 1.0;
                let fs_s = RATE * s;
                ModTap {
                    gain: g,
                    fixed: (f64::from(fs_s) * fixed) as u32,
                    freq: ((f64::from((1.0 / s) / RATE) * freq) as f32) * 6.283_185_5,
                    depth: (f64::from(fs_s) * depth) as u32,
                    phase: 0.0,
                }
            })
            .collect();
        self.dry = 1.0;
        self.finish(key);
    }

    pub fn vibrato(&mut self, depth: f64, freq: f64) {
        let key = [1.0, 0.0, depth as f32, freq as f32];
        if !self.rebuild && key == self.cache {
            return;
        }
        let rate = ((freq / f64::from(RATE)) as f32) * 6.283_185_5;
        self.taps =
            vec![ModTap { gain: 1.0, fixed: 0, freq: rate, depth: (f64::from(RATE) * depth) as u32, phase: 0.0 }];
        self.dry = 0.0;
        self.finish(key);
    }

    fn finish(&mut self, key: [f32; 4]) {
        for t in &mut self.taps {
            if t.depth as f32 * t.freq > 0.99 {
                t.depth = (0.99 / t.freq) as u32;
            }
        }
        self.cache = key;
        self.rebuild = false;
    }

    pub fn process(&mut self, chans: &mut [Vec<f32>]) {
        let len = self.ring.first().map_or(1, Vec::len);
        let input: Vec<Vec<f32>> = chans.to_vec();
        let (ring, taps, pos, dry) = (&self.ring, &self.taps, self.pos, self.dry);
        // PORT(choice): run in the FMA build of `fused` where the CPU has FMA, so the 2 multiply-adds a sample and tap
        // are the instruction in line instead of calls to the runtime's `fmaf` (the same single rounding: the same
        // samples bit for bit); the flag is not used (they stay fused without FMA too, as before); the console has the
        // engine 0x82A16D70, whose multiply-adds these are.
        crate::sfx_dsp::fused(
            #[inline(always)]
            |_| {
                for (c, buf) in chans.iter_mut().enumerate().take(ring.len()) {
                    buf.iter_mut().for_each(|x| *x *= dry);
                    for t in taps {
                        let mut ph = t.phase;
                        for (k, out) in buf.iter_mut().enumerate() {
                            let s = (f64::from(ph).sin() as f32) + 1.0;
                            let d = s.mul_add(t.depth as f32, t.fixed as f32) + 512.0;
                            let back = d - (k as f32 + 1.0);
                            let cb = back.ceil();
                            let frac = cb - back;
                            let at = |b: usize| ring[c][wrap(pos + len - wrap(b, len), len)];
                            let (older, newer) = (at(cb as usize), at(cb as usize - 1));
                            let v = (1.0 - frac) * older + (f64::from(newer) * f64::from(frac)) as f32;
                            *out = t.gain.mul_add(v, *out);
                            ph = (f64::from(t.freq) + f64::from(ph)) as f32;
                        }
                    }
                }
            },
        );
        for t in &mut self.taps {
            let p = (f64::from(t.freq) * 256.0 + f64::from(t.phase)) as f32;
            let turns = f64::from(p) * 0.159_154_936_671_257_02;
            t.phase = if (turns as f32) > 1.0 {
                -((turns as u32) as f64 * 6.283_185_482_025_146_5 - f64::from(p)) as f32
            } else {
                p
            };
        }
        for (c, x) in input.iter().enumerate().take(self.ring.len()) {
            for (k, v) in x.iter().enumerate() {
                self.ring[c][wrap(self.pos + k, len)] = *v;
            }
        }
        self.pos = (self.pos + BLOCK) % len;
    }
}
