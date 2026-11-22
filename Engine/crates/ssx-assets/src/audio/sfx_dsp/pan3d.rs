use super::{BLOCK, Coefs, PanParams, biquad, pan2d_gains};

const RATE: f32 = 48_000.0;
const RESTART_RAMP: i32 = 64;
const MAX_CH: usize = 8;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Packet {
    pub interval: f32,
    pub center: f32,
    pub angle: f32,
    pub distance: f32,
    pub gain: f32,
    pub cutoff: f32,
    pub delay: f32,
    pub delay_rate: f32,
    pub reset: bool,
    pub active: bool,
    pub shelf: f32,
}

#[derive(Clone, Copy, Debug, Default)]
struct Path {
    state: u8,
    lpf_state: [f32; 4],
    lpf: Coefs,
    inc: [f32; MAX_CH],
    gain: [f32; MAX_CH],
    ch: [u8; MAX_CH],
    ramp: i32,
    delay_target: f32,
    delay: f32,
    delay_inc: f32,
    delay_ramp: i32,
    shelf_state: [f32; 4],
    shelf: Coefs,
}

#[derive(Debug)]
pub struct Pan3d {
    outs: usize,
    chans: usize,
    front: f32,
    rear: f32,
    max_delay: f32,
    max_samples: i32,
    line: Vec<f32>,
    write_at: usize,
    state: u8,
    countdown: i32,
    carry: i32,
    bufs: [Packet; 3],
    write_buf: [usize; 2],
    read_buf: [usize; 2],
    full: [bool; 2],
    write: usize,
    read: usize,
    overflow: bool,
    paths: [Path; 2],
    slot: usize,
    level: f32,
}

fn round_half(x: f32) -> i32 {
    (if x < 0.0 { x - 0.5 } else { x + 0.5 }) as i32
}

impl Pan3d {
    pub fn new(outs: usize, max_delay: f32, front: f32, rear: f32) -> Self {
        let outs = outs.clamp(1, MAX_CH);
        let max_samples = round_half(max_delay * RATE);
        let len = ((round_half(RATE * max_delay).max(0) as usize + 0xFF) & !0xFF) + 0x200;
        let mut p = Self {
            outs,
            chans: if outs < 6 { outs } else { outs - 1 },
            front,
            rear,
            max_delay,
            max_samples,
            line: vec![0.0; len],
            write_at: 0,
            state: 0,
            countdown: 0,
            carry: 0,
            bufs: [Packet::default(); 3],
            write_buf: [0, 1],
            read_buf: [0, 1],
            full: [false; 2],
            write: 0,
            read: 0,
            overflow: false,
            paths: [Path::default(); 2],
            slot: 0,
            level: 1.0,
        };
        p.reset();
        p
    }

    pub fn reset(&mut self) {
        self.reset_paths();
        (self.write, self.read, self.overflow, self.full) = (0, 0, false, [false; 2]);
        (self.write_buf, self.read_buf, self.state) = ([0, 1], [0, 1], 0);
    }

    fn reset_paths(&mut self) {
        (self.countdown, self.slot) = (0, 0);
        for p in &mut self.paths {
            p.state = 0;
            p.ch = [0; MAX_CH];
            p.gain = [0.0; MAX_CH];
            p.lpf_state = [0.0; 4];
            p.shelf_state = [0.0; 4];
        }
    }

    pub fn send(&mut self, packet: Option<&Packet>) {
        let Some(packet) = packet else {
            self.reset();
            return;
        };
        self.state = match self.state {
            0 => 1,
            1 => 2,
            4 => 5,
            5 => 6,
            s => s,
        };
        if (self.write == self.read && self.full[self.read]) || self.overflow {
            if self.overflow {
                self.read_buf[self.read] = self.write_buf[self.read];
                self.read ^= 1;
            } else {
                self.overflow = true;
            }
            self.write_buf[self.write] = (0..3).find(|b| !self.write_buf.contains(b)).unwrap_or(0);
            self.countdown = -100;
        }
        self.bufs[self.write_buf[self.write]] = *packet;
        self.full[self.write] = true;
        self.write ^= 1;
    }

    pub fn waiting(&self) -> bool {
        self.state < 2
    }

    pub fn rest(&mut self, level: f32) {
        self.level = level;
        let len = self.line.len();
        while self.write_at >= len {
            self.write_at -= len;
        }
        self.line[self.write_at..self.write_at + BLOCK].fill(0.0);
        self.write_at += BLOCK;
    }

    pub fn outputs(&self) -> usize {
        self.outs
    }

    pub fn process(&mut self, first: bool, level: f32, buf: &mut Vec<Vec<f32>>) {
        self.level = level;
        let len = self.line.len();
        while self.write_at >= len {
            self.write_at -= len;
        }
        let input = buf.first().map_or(&[0.0; BLOCK][..], |c| &c[..]);
        self.line[self.write_at..self.write_at + BLOCK].copy_from_slice(&input[..BLOCK]);
        self.write_at += BLOCK;
        if first {
            self.reset_paths();
        }
        let mut out = vec![vec![0.0; BLOCK]; self.outs];
        if self.state >= 2 {
            self.render(&mut out);
        }
        *buf = out;
    }
}

impl Pan3d {
    fn render(&mut self, out: &mut [Vec<f32>]) {
        let base = self.write_at as i64 - BLOCK as i64;
        let mut left = BLOCK as i32;
        let mut at = 0usize;
        loop {
            if self.countdown < 1 {
                match self.state {
                    3 if self.full[self.read] => self.update(),
                    3 => (self.countdown, self.state) = (left, 4),
                    4 | 5 => self.countdown = left,
                    2 | 6 => {
                        self.update();
                        self.state = 3;
                    }
                    _ => {}
                }
            }
            // TODO(guess): the game's intervals are frame times (≥ 16 samples: the packet commit 0x83571308 takes the frame time as the interval, the Pan3D update 0x82A35C80 applies it as seconds·48 000 rounded in 16-sample steps, the remainder carried); a segment is kept to at least 16 samples so a degenerate interval cannot stall the loop; settle: the Sound3d clock source.
            let n = self.countdown.min(left).max(16.min(left));
            for slot in 0..2 {
                if self.paths[slot].state != 0 {
                    self.render_path(slot, base, at, n as usize, out);
                }
            }
            left -= n;
            self.countdown -= n;
            at += n as usize;
            if left < 1 {
                break;
            }
        }
    }

    fn update(&mut self) {
        let mut p = self.bufs[self.read_buf[self.read]];
        let n = round_half(p.interval.mul_add(RATE, 0.5));
        let v = self.carry + n + 8;
        let q = v / 16;
        self.countdown = q * 16;
        self.carry += n - q * 16;
        if (p.reset || !p.active) && self.paths[self.slot].state != 0 {
            self.fade_out(self.slot);
            self.slot ^= 1;
        }
        if p.active {
            let nyquist = f64::from(RATE * 0.5);
            let max = (nyquist * 0.990_000_009_536_743_2) as f32;
            if p.cutoff > max {
                p.cutoff = max;
            }
            let (countdown, max_samples, max_delay) = (self.countdown, self.max_samples, self.max_delay);
            let (chans, outs, front, rear, level) = (self.chans, self.outs, self.front, self.rear, self.level);
            let path = &mut self.paths[self.slot];
            path.shelf = shelf_coefs((1.0 / RATE) * 1000.0, p.shelf);
            if p.reset {
                path.shelf_state = [0.0; 4];
            }
            let target = if max_delay <= p.delay { max_samples as f32 } else { (p.delay * RATE).floor() };
            path.delay_target = target.max(256.0);
            delay_ramp(path, &p, countdown, max_samples);
            let w = p.cutoff * (1.0 / RATE);
            if w != 0.0 {
                path.lpf = lowpass_coefs(w);
            }
            let mut targets = [0.0f32; MAX_CH];
            if w != 0.0 {
                let params = PanParams {
                    angle: f64::from(p.angle),
                    distance: f64::from(p.distance),
                    center: f64::from(p.center),
                    main: f64::from(level * p.gain),
                    ..Default::default()
                };
                let g = pan2d_gains(&params, 1, outs, 0, front, rear);
                targets[..chans].copy_from_slice(&g[0][..chans]);
            }
            let step = 1.0 / path.ramp as f32;
            for (c, &t) in targets.iter().enumerate().take(chans) {
                if t > 0.0 {
                    path.ch[c] = 1;
                } else if path.ch[c] != 0 {
                    path.ch[c] = 2;
                } else {
                    continue;
                }
                path.inc[c] = (t - path.gain[c]) * step;
            }
            path.state = 1;
        }
        if self.overflow {
            self.read_buf[self.read] = self.write_buf[self.read];
            self.overflow = false;
        } else {
            self.full[self.read] = false;
        }
        self.read ^= 1;
    }

    fn fade_out(&mut self, slot: usize) {
        let chans = self.chans;
        let p = &mut self.paths[slot];
        for c in 0..chans {
            if p.gain[c] > 0.0 {
                p.ch[c] = 2;
                p.inc[c] = -p.gain[c] * (1.0 / RESTART_RAMP as f32);
            } else {
                (p.ch[c], p.gain[c], p.inc[c]) = (0, 0.0, 0.0);
            }
        }
        (p.delay_target, p.delay_inc, p.ramp, p.state, p.delay_ramp) = (p.delay, 0.0, RESTART_RAMP, 2, RESTART_RAMP);
    }
}

fn delay_ramp(path: &mut Path, p: &Packet, countdown: i32, max_samples: i32) {
    if p.reset {
        let still =
            |path: &mut Path| (path.delay_inc, path.delay, path.delay_ramp) = (0.0, path.delay_target, RESTART_RAMP);
        if p.delay_rate == 0.0 {
            still(path);
        } else {
            path.delay_inc = p.delay_rate;
            path.delay_ramp = countdown;
            path.delay = -(countdown as f32).mul_add(p.delay_rate, -path.delay_target);
            if path.delay < 256.0 || (max_samples as f32) < path.delay {
                still(path);
            }
        }
        path.ramp = RESTART_RAMP;
        path.gain = [0.0; MAX_CH];
        path.lpf_state = [0.0; 4];
        return;
    }
    path.delay_ramp = countdown;
    let inc = (path.delay_target - path.delay) / countdown as f32;
    path.delay_inc = if inc.abs() > 0.99 { 0.99f32.copysign(inc) } else { inc };
    path.ramp = path.delay_ramp;
}

impl Pan3d {
    fn render_path(&mut self, slot: usize, base: i64, at: usize, n: usize, out: &mut [Vec<f32>]) {
        // PORT(choice): run in the FMA build of `fused` where the CPU has FMA, so the multiply-adds of the delay read
        // and the low-pass (6 a sample) and of the mix (1 a sample and channel) are the instruction in line instead of
        // calls to the runtime's `fmaf` (the same single rounding: the same samples bit for bit); the flag is not used
        // (they stay fused without FMA too, as before); the console has 0x82A52D08, whose multiply-adds these are.
        super::fused(
            #[inline(always)]
            |_| self.render_path_in(slot, base, at, n, out),
        );
    }

    #[inline(always)]
    fn render_path_in(&mut self, slot: usize, base: i64, mut at: usize, mut n: usize, out: &mut [Vec<f32>]) {
        let chans = self.chans;
        let mut x = [0.0f32; BLOCK];
        while n > 0 {
            if self.paths[slot].state == 0 {
                return;
            }
            let path = &mut self.paths[slot];
            let m = if path.delay_ramp < 1 {
                (path.delay_inc, path.delay_ramp) = (0.0, 0);
                n
            } else {
                let m = n.min(path.delay_ramp as usize);
                path.delay_ramp -= m as i32;
                m
            };
            let (delay, inc) = (path.delay, path.delay_inc);
            read_delay(&self.line, base + at as i64, delay, inc, &mut x[..m]);
            let path = &mut self.paths[slot];
            path.delay = (m as f32).mul_add(inc, delay);
            lowpass(&path.lpf, &mut path.lpf_state, &mut x[..m]);
            biquad(&path.shelf, &mut path.shelf_state, &mut x[..m]);
            let (mut k, mut pos) = (m, 0usize);
            while k > 0 && path.state != 0 {
                let j = if path.ramp < 1 {
                    if path.state == 2 {
                        path.gain[..chans].fill(0.0);
                    }
                    path.inc[..chans].fill(0.0);
                    k
                } else {
                    let j = k.min(path.ramp as usize);
                    path.ramp -= j as i32;
                    j
                };
                mix(path, chans, &x[pos..pos + j], out, at + pos);
                pos += j;
                k -= j;
                if path.ramp < 1 && path.state == 2 {
                    path.state = 0;
                }
            }
            at += m;
            n -= m;
        }
    }
}

#[inline(always)]
fn read_delay(line: &[f32], at: i64, delay: f32, inc: f32, x: &mut [f32]) {
    let len = line.len() as i64;
    let start = (at as f64 - f64::from(delay)) as f32;
    let i = f64::from(start).floor();
    let frac = (i as f32 - start).abs();
    let pos0 = round_half(frac * 65_536.0) as u32;
    let step = round_half((1.0 - inc) * 65_536.0) as u32;
    let scale = f32::from_bits(0x377F_FC9C);
    let first = (i as i64).rem_euclid(len) as usize;
    let sample = |off: usize| line[super::wrap(first + off, line.len())];
    for (k, v) in x.iter_mut().enumerate() {
        let pos = pos0.wrapping_add(step.wrapping_mul(k as u32));
        let off = (pos >> 16) as usize;
        let f = (pos & 0xFFFF) as f32 * scale;
        *v = (1.0 - f).mul_add(sample(off), f * sample(off + 1));
    }
}

#[inline(always)]
fn lowpass(c: &Coefs, state: &mut [f32; 4], buf: &mut [f32]) {
    let [mut x1, mut x2, mut y1, mut y2] = *state;
    for v in buf.iter_mut() {
        let x = *v;
        let ff = c[4].mul_add(x2, c[3].mul_add(x1, c[2].mul_add(x, 1e-18)));
        let y = (-y2).mul_add(c[1], (-y1).mul_add(c[0], ff));
        (x2, x1, y2, y1) = (x1, x, y1, y);
        *v = y;
    }
    *state = [x1, x2, y1, y2];
}

#[inline(always)]
fn mix(path: &mut Path, chans: usize, x: &[f32], out: &mut [Vec<f32>], at: usize) {
    let groups = x.len() / 16;
    for c in 0..chans.min(out.len()) {
        if path.ch[c] == 0 {
            continue;
        }
        let (g, d) = (path.gain[c], path.inc[c]);
        if groups > 0 {
            let mut l0 = [g, g + d, d.mul_add(2.0, g), d.mul_add(3.0, g)];
            let d4 = d * 4.0;
            let d8 = d * 8.0;
            let mut l1 = l0.map(|v| v + d4);
            let o = &mut out[c][at..at + groups * 16];
            for (ys, xs) in o.as_chunks_mut::<8>().0.iter_mut().zip(x.as_chunks::<8>().0) {
                for i in 0..4 {
                    ys[i] = l0[i].mul_add(xs[i], ys[i]);
                    ys[i + 4] = l1[i].mul_add(xs[i + 4], ys[i + 4]);
                }
                l0 = l0.map(|v| v + d8);
                l1 = l1.map(|v| v + d8);
            }
            path.gain[c] = l0[0];
        }
        if path.ch[c] == 2 && path.gain[c] < d.abs() {
            (path.ch[c], path.gain[c]) = (0, 0.0);
        }
    }
}

fn lowpass_coefs(w: f32) -> Coefs {
    let w = w.max(0.001);
    let k = f64::from(w * std::f32::consts::PI).tan() as f32;
    let (k2, kq) = (k * k, k * 1.414);
    let inv = 1.0 / ((kq + k2) + 1.0);
    let b0 = inv * k2;
    [((k2 - 1.0) * inv) * 2.0, ((1.0 - kq) + k2) * inv, b0, b0 * 2.0, b0]
}

fn shelf_coefs(w: f32, g: f32) -> Coefs {
    if g < 1.001 {
        return [0.0, 0.0, 1.0, 0.0, 0.0];
    }
    let k = f64::from(w * std::f32::consts::PI).tan() as f32;
    let s = (g * 2.0).sqrt();
    let (k2, kq) = (k * k, k * 1.414);
    let sk = s * k;
    let gk2 = k2 * g;
    let inv = 1.0 / ((kq + k2) + 1.0);
    [
        ((k2 - 1.0) * inv) * 2.0,
        ((1.0 - kq) + k2) * inv,
        ((sk + gk2) + 1.0) * inv,
        ((gk2 - 1.0) * inv) * 2.0,
        ((1.0 - sk) + gk2) * inv,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn packet(angle: f32, reset: bool) -> Packet {
        Packet {
            interval: 1.0 / 30.0,
            center: 1.0,
            angle,
            distance: 1.0,
            gain: 1.0,
            cutoff: 64_000.0,
            reset,
            active: true,
            shelf: 1.0,
            ..Default::default()
        }
    }

    fn run(p: &mut Pan3d, input: f32, blocks: usize) -> Vec<Vec<f32>> {
        let mut last = Vec::new();
        for _ in 0..blocks {
            let mut buf = vec![vec![input; BLOCK]];
            let first = p.write_at == 0;
            p.process(first, 1.0, &mut buf);
            last = buf;
        }
        last
    }

    #[test]
    fn silent_until_the_second_packet() {
        let mut p = Pan3d::new(6, 0.0, 30.0, 110.0);
        assert_eq!(p.line.len(), 512);
        p.send(Some(&packet(0.0, true)));
        assert!(run(&mut p, 1.0, 4).iter().flatten().all(|&v| v == 0.0));
        p.send(Some(&packet(0.0, false)));
        let out = run(&mut p, 1.0, 1);
        assert!(out[1].iter().any(|&v| v > 0.0), "centre after the second packet");
        p.send(None);
        assert!(run(&mut p, 1.0, 2).iter().flatten().all(|&v| v == 0.0));
    }

    #[test]
    fn straight_ahead_settles_on_the_centre_one_block_late() {
        let mut p = Pan3d::new(6, 0.0, 30.0, 110.0);
        p.send(Some(&packet(0.0, true)));
        p.send(Some(&packet(0.0, false)));
        let out = run(&mut p, 1.0, 3);
        assert!((out[1][255] - 1.0).abs() < 1e-3, "centre {}", out[1][255]);
        assert!(out[0][255].abs() < 1e-4 && out[2][255].abs() < 1e-4 && out[5].iter().all(|&v| v == 0.0));
        let mut q = Pan3d::new(6, 0.0, 30.0, 110.0);
        q.send(Some(&packet(0.0, true)));
        q.send(Some(&packet(0.0, false)));
        let mut buf = vec![vec![0.0; BLOCK]];
        buf[0][10] = 1.0;
        q.process(true, 1.0, &mut buf);
        assert!(buf.iter().flatten().all(|&v| v.abs() < 1e-12), "the impulse is still in the delay line");
        let mut buf = vec![vec![0.0; BLOCK]];
        q.process(false, 1.0, &mut buf);
        assert!(buf[1][10] > 0.0 && buf[1][..10].iter().all(|&v| v.abs() < 1e-12));
    }

    #[test]
    fn a_turn_ramps_over_the_interval_and_a_restart_cross_fades() {
        let mut p = Pan3d::new(6, 0.0, 30.0, 110.0);
        p.send(Some(&packet(30.0, true)));
        p.send(Some(&packet(30.0, false)));
        let out = run(&mut p, 1.0, 8);
        assert!(out[2][255] > 0.99, "front right {}", out[2][255]);
        p.send(Some(&packet(330.0, false)));
        p.send(Some(&packet(330.0, false)));
        let mid = run(&mut p, 1.0, 6);
        assert!(mid[0][128] > 0.05 && mid[2][128] > 0.05, "{} {}", mid[0][128], mid[2][128]);
        let end = run(&mut p, 1.0, 8);
        assert!(end[0][255] > 0.99 && end[2][255].abs() < 1e-3);
        p.send(Some(&packet(30.0, true)));
        let out = run(&mut p, 1.0, 16);
        assert!(out[2][255] > 0.99 && out[0][255].abs() < 1e-3);
    }

    #[test]
    fn coefficients_match_the_bilinear_butterworth() {
        let c = lowpass_coefs(1000.0 / RATE);
        let dc = (c[2] + c[3] + c[4]) / (1.0 + c[0] + c[1]);
        assert!((dc - 1.0).abs() < 1e-5);
        let s = shelf_coefs(1000.0 / RATE, 2.0);
        let dc = (s[2] + s[3] + s[4]) / (1.0 + s[0] + s[1]);
        let ny = (s[2] - s[3] + s[4]) / (1.0 - s[0] + s[1]);
        assert!((dc - 2.0).abs() < 1e-3 && (ny - 1.0).abs() < 1e-3);
        assert_eq!(shelf_coefs(0.02, 1.0005), [0.0, 0.0, 1.0, 0.0, 0.0]);
    }
}
