use super::*;

fn pow43() -> &'static [f32] {
    static T: OnceLock<Vec<f32>> = OnceLock::new();
    T.get_or_init(|| (0..8207).map(|i| (i as f64).powf(4.0 / 3.0) as f32).collect())
}

#[inline]
fn requant_run(q: &[i32], xr: &mut [f32], gain_quarter_steps: i32) {
    let gain = 2f64.powf(f64::from(gain_quarter_steps) / 4.0) as f32;
    let p = pow43();
    for (x, &v) in xr.iter_mut().zip(q) {
        let m = p[v.unsigned_abs() as usize] * gain;
        *x = if v < 0 { -m } else { m };
    }
}

pub(super) fn requantize(
    q: &[i32; 576],
    nz: usize,
    g: &GranuleChannel,
    sf: &ScaleFactors,
    sfi: usize,
    xr: &mut [f32; 576],
) {
    xr.fill(0.0);
    let shift = if g.scalefac_scale { 2 } else { 1 };
    let base = i32::from(g.global_gain) - 210;
    let long_bands = if !g.is_short() {
        22
    } else if g.mixed_block {
        8
    } else {
        0
    };
    for b in 0..long_bands {
        let (s, e) = (SFB_LONG[sfi][b], SFB_LONG[sfi][b + 1].min(nz));
        if s >= e {
            break;
        }
        let pre = if g.preflag { PRETAB[b] } else { 0 };
        requant_run(&q[s..e], &mut xr[s..e], base - (i32::from(sf.long[b] + pre) << shift));
    }
    if g.is_short() {
        for b in if g.mixed_block { 3 } else { 0 }..13 {
            let (s, width) = (3 * SFB_SHORT[sfi][b], SFB_SHORT[sfi][b + 1] - SFB_SHORT[sfi][b]);
            if s >= nz {
                break;
            }
            for w in 0..3 {
                let (a, e) = (s + w * width, s + (w + 1) * width);
                let gain = base - 8 * i32::from(g.subblock_gain[w]) - (i32::from(sf.short[b][w]) << shift);
                requant_run(&q[a..e], &mut xr[a..e], gain);
            }
        }
    }
}

fn ms_range(l: &mut [f32], r: &mut [f32]) {
    const ISQRT2: f32 = std::f32::consts::FRAC_1_SQRT_2;
    for (a, b) in l.iter_mut().zip(r.iter_mut()) {
        let (m, s) = (*a, *b);
        *a = (m + s) * ISQRT2;
        *b = (m - s) * ISQRT2;
    }
}

fn intensity_band(l: &mut [f32], r: &mut [f32], is_pos: u8, ms: bool) {
    if is_pos >= 7 {
        if ms {
            ms_range(l, r);
        }
        return;
    }
    let p = f64::from(is_pos) * std::f64::consts::PI / 12.0;
    let (s, c) = p.sin_cos();
    let (kl, kr) = ((s / (s + c)) as f32, (c / (s + c)) as f32);
    for (a, b) in l.iter_mut().zip(r.iter_mut()) {
        let x = *a;
        *a = x * kl;
        *b = x * kr;
    }
}

/// TODO(guess): the last band takes the band below's scale factor even when that band is not intensity-coded (mpg123 / FFmpeg; dist10 uses no IS, minimp3 is_pos 3), and is_pos 8–15 count as illegal like 7 (FFmpeg, minimp3; dist10 / mpg123 extrapolate `tan`) — no disc granule sets the intensity bit; settle: the exe's `Snd::CEALayer3DecF` if a sound ever uses intensity stereo.
pub(super) fn joint_stereo(
    xr: &mut [[f32; 576]; 2],
    g1: &GranuleChannel,
    sf1: &ScaleFactors,
    sfi: usize,
    mode_ext: u8,
) {
    let ms = mode_ext & 2 != 0;
    let [l, r] = xr;
    if mode_ext & 1 == 0 {
        if ms {
            ms_range(l, r);
        }
        return;
    }
    let zero = |r: &[f32]| r.iter().all(|&v| v == 0.0);
    let mut long_nonzero_found = false;
    let long_bands = if !g1.is_short() {
        22
    } else {
        let first = if g1.mixed_block { 3 } else { 0 };
        let mut found = [false; 3];
        for b in (first..13).rev() {
            let (s, width) = (3 * SFB_SHORT[sfi][b], SFB_SHORT[sfi][b + 1] - SFB_SHORT[sfi][b]);
            for w in (0..3).rev() {
                let rg = s + w * width..s + (w + 1) * width;
                found[w] |= !zero(&r[rg.clone()]);
                if found[w] {
                    if ms {
                        ms_range(&mut l[rg.clone()], &mut r[rg]);
                    }
                } else {
                    intensity_band(&mut l[rg.clone()], &mut r[rg], sf1.short[b.min(11)][w], ms);
                }
            }
        }
        long_nonzero_found = found.iter().any(|&f| f);
        if g1.mixed_block { 8 } else { 0 }
    };
    for b in (0..long_bands).rev() {
        let rg = SFB_LONG[sfi][b]..SFB_LONG[sfi][b + 1];
        long_nonzero_found |= !zero(&r[rg.clone()]);
        if long_nonzero_found {
            if ms {
                ms_range(&mut l[rg.clone()], &mut r[rg]);
            }
        } else {
            intensity_band(&mut l[rg.clone()], &mut r[rg], sf1.long[b.min(20)], ms);
        }
    }
}

pub(super) fn reorder(xr: &mut [f32; 576], sfi: usize, mixed: bool) {
    let first = if mixed { 3 } else { 0 };
    let mut tmp = [0f32; 576];
    for b in first..13 {
        let (s, width) = (SFB_SHORT[sfi][b], SFB_SHORT[sfi][b + 1] - SFB_SHORT[sfi][b]);
        for w in 0..3 {
            for j in 0..width {
                tmp[3 * (s + j) + w] = xr[3 * s + w * width + j];
            }
        }
    }
    let start = 3 * SFB_SHORT[sfi][first];
    xr[start..].copy_from_slice(&tmp[start..]);
}

fn alias_reduce(xr: &mut [f32; 576], g: &GranuleChannel) {
    let boundaries = if !g.is_short() {
        32
    } else if g.mixed_block {
        2
    } else {
        return;
    };
    let t = tables();
    for sb in 1..boundaries {
        for i in 0..8 {
            let (lo, hi) = (18 * sb - 1 - i, 18 * sb + i);
            let (a, b) = (xr[lo], xr[hi]);
            xr[lo] = a * t.cs[i] - b * t.ca[i];
            xr[hi] = b * t.cs[i] + a * t.ca[i];
        }
    }
}

struct Tables {
    cs: [f32; 8],
    ca: [f32; 8],
    imdct_long: [[f32; 18]; 36],
    imdct_short: [[f32; 6]; 12],
    win_long: [[f32; 36]; 3],
    win_short: [f32; 12],
    dct32: [[f32; 32]; 32],
    window: [f32; 512],
}

fn tables() -> &'static Tables {
    static T: OnceLock<Tables> = OnceLock::new();
    T.get_or_init(|| {
        use std::f64::consts::PI;
        let mut t = Tables {
            cs: [0.0; 8],
            ca: [0.0; 8],
            imdct_long: [[0.0; 18]; 36],
            imdct_short: [[0.0; 6]; 12],
            win_long: [[0.0; 36]; 3],
            win_short: [0.0; 12],
            dct32: [[0.0; 32]; 32],
            window: [0.0; 512],
        };
        for (i, &c) in ALIAS_C.iter().enumerate() {
            let d = (1.0 + c * c).sqrt();
            t.cs[i] = (1.0 / d) as f32;
            t.ca[i] = (c / d) as f32;
        }
        for (i, row) in t.imdct_long.iter_mut().enumerate() {
            for (k, v) in row.iter_mut().enumerate() {
                *v = (PI / 72.0 * (2 * i + 1 + 18) as f64 * (2 * k + 1) as f64).cos() as f32;
            }
        }
        for (i, row) in t.imdct_short.iter_mut().enumerate() {
            for (k, v) in row.iter_mut().enumerate() {
                *v = (PI / 24.0 * (2 * i + 1 + 6) as f64 * (2 * k + 1) as f64).cos() as f32;
            }
        }
        let long_sin = |i: usize| (PI / 36.0 * (i as f64 + 0.5)).sin();
        let short_sin = |i: usize| (PI / 12.0 * (i as f64 + 0.5)).sin();
        for i in 0..36 {
            t.win_long[0][i] = long_sin(i) as f32;
            t.win_long[1][i] = match i {
                0..18 => long_sin(i),
                18..24 => 1.0,
                24..30 => short_sin(i - 18),
                _ => 0.0,
            } as f32;
            t.win_long[2][i] = match i {
                0..6 => 0.0,
                6..12 => short_sin(i - 6),
                12..18 => 1.0,
                _ => long_sin(i),
            } as f32;
        }
        for (i, w) in t.win_short.iter_mut().enumerate() {
            *w = short_sin(i) as f32;
        }
        for (k, row) in t.dct32.iter_mut().enumerate() {
            for (m, v) in row.iter_mut().enumerate() {
                *v = ((m * (2 * k + 1)) as f64 * PI / 64.0).cos() as f32;
            }
        }
        for (w, &d) in t.window.iter_mut().zip(SYNTH_WINDOW_Q16.iter()) {
            *w = (f64::from(d) / 65536.0) as f32;
        }
        t
    })
}

pub(super) fn imdct(x: &[f32], bt: u8, z: &mut [f32; 36]) {
    let t = tables();
    z.fill(0.0);
    if bt == 2 {
        for w in 0..3 {
            for (i, cos) in t.imdct_short.iter().enumerate() {
                let y: f32 = (0..6).map(|k| x[3 * k + w] * cos[k]).sum();
                z[6 + 6 * w + i] += y * t.win_short[i];
            }
        }
    } else {
        let win = &t.win_long[match bt {
            0 => 0,
            1 => 1,
            _ => 2,
        }];
        for (i, cos) in t.imdct_long.iter().enumerate() {
            let y: f32 = x.iter().zip(cos).map(|(a, b)| a * b).sum();
            z[i] = y * win[i];
        }
    }
}

#[derive(Clone)]
pub struct ChannelState {
    overlap: [[f32; 18]; 32],
    v: [[f32; 64]; 16],
    head: usize,
    pub(super) prev_sf: ScaleFactors,
}

impl Default for ChannelState {
    fn default() -> Self {
        Self { overlap: [[0.0; 18]; 32], v: [[0.0; 64]; 16], head: 0, prev_sf: ScaleFactors::default() }
    }
}

impl ChannelState {
    pub(super) fn synth(&mut self, s: &[f32; 32], out: &mut [f32]) {
        let t = tables();
        let mut a = [0f32; 32];
        for (k, row) in t.dct32.iter().enumerate() {
            for (am, c) in a.iter_mut().zip(row) {
                *am += s[k] * c;
            }
        }
        self.head = (self.head + 15) & 15;
        let v = &mut self.v[self.head];
        v[..16].copy_from_slice(&a[16..]);
        v[16] = 0.0;
        for i in 17..49 {
            v[i] = -a[48 - i];
        }
        for i in 49..64 {
            v[i] = -a[i - 48];
        }
        let mut acc = [0f32; 32];
        for i in 0..8 {
            let v0 = &self.v[(self.head + 2 * i) & 15];
            let v1 = &self.v[(self.head + 2 * i + 1) & 15];
            let (d0, d1) = (&t.window[64 * i..64 * i + 32], &t.window[64 * i + 32..64 * i + 64]);
            for j in 0..32 {
                acc[j] += v0[j] * d0[j] + v1[32 + j] * d1[j];
            }
        }
        out[..32].copy_from_slice(&acc);
    }

    pub(super) fn hybrid_synth(&mut self, xr: &mut [f32; 576], g: &GranuleChannel, nz: usize, out: &mut [f32; 576]) {
        alias_reduce(xr, g);
        let active = (nz.div_ceil(18) + 1).min(32);
        let mut sub = [[0f32; 18]; 32];
        let mut z = [0f32; 36];
        for sb in 0..32 {
            let bt = if !g.window_switching || (g.mixed_block && sb < 2) { 0 } else { g.block_type };
            if sb < active {
                imdct(&xr[18 * sb..18 * sb + 18], bt, &mut z);
            } else {
                z.fill(0.0);
            }
            for i in 0..18 {
                sub[sb][i] = z[i] + self.overlap[sb][i];
                self.overlap[sb][i] = z[18 + i];
            }
            if sb & 1 == 1 {
                for i in (1..18).step_by(2) {
                    sub[sb][i] = -sub[sb][i];
                }
            }
        }
        let mut s = [0f32; 32];
        for ts in 0..18 {
            for (sb, v) in s.iter_mut().enumerate() {
                *v = sub[sb][ts];
            }
            self.synth(&s, &mut out[32 * ts..32 * ts + 32]);
        }
    }
}
