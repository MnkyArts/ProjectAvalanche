mod spectrum;
mod synthesis;

use std::sync::OnceLock;

use super::mp3_tables::{
    LINBITS, PAIR_TABLE_16, PAIR_TABLE_24, PAIR_TABLES, PairTable, QUAD_A_CODE, QUAD_A_LEN, SYNTH_WINDOW_Q16,
};
use crate::{Error, Result};

pub use spectrum::*;
pub use synthesis::*;

pub const GRANULE_SAMPLES: usize = 576;

const SFB_LONG: [[usize; 23]; 3] = [
    [0, 4, 8, 12, 16, 20, 24, 30, 36, 44, 52, 62, 74, 90, 110, 134, 162, 196, 238, 288, 342, 418, 576],
    [0, 4, 8, 12, 16, 20, 24, 30, 36, 42, 50, 60, 72, 88, 106, 128, 156, 190, 230, 276, 330, 384, 576],
    [0, 4, 8, 12, 16, 20, 24, 30, 36, 44, 54, 66, 82, 102, 126, 156, 194, 240, 296, 364, 448, 550, 576],
];
const SFB_SHORT: [[usize; 14]; 3] = [
    [0, 4, 8, 12, 16, 22, 30, 40, 52, 66, 84, 106, 136, 192],
    [0, 4, 8, 12, 16, 22, 28, 38, 50, 64, 80, 100, 126, 192],
    [0, 4, 8, 12, 16, 22, 30, 42, 58, 78, 104, 138, 180, 192],
];
#[rustfmt::skip]
const SLEN: [(u32, u32); 16] = [
    (0, 0), (0, 1), (0, 2), (0, 3), (3, 0), (1, 1), (1, 2), (1, 3),
    (2, 1), (2, 2), (2, 3), (3, 1), (3, 2), (3, 3), (4, 2), (4, 3),
];
const PRETAB: [u8; 22] = [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 3, 3, 3, 2, 0];
const ALIAS_C: [f64; 8] = [-0.6, -0.535, -0.33, -0.185, -0.095, -0.041, -0.0142, -0.0037];

#[derive(Clone)]
pub struct BitReader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> BitReader<'a> {
    pub fn new(data: &'a [u8], bit_pos: usize) -> Self {
        Self { data, pos: bit_pos }
    }

    pub fn pos(&self) -> usize {
        self.pos
    }

    #[inline]
    fn peek32(&self) -> u32 {
        let byte = self.pos >> 3;
        let mut w = 0u64;
        for i in 0..5 {
            w = (w << 8) | u64::from(self.data.get(byte + i).copied().unwrap_or(0));
        }
        ((w << (self.pos & 7)) >> 8) as u32
    }

    #[inline]
    pub fn bits(&mut self, n: u32) -> u32 {
        if n == 0 {
            return 0;
        }
        let v = self.peek32() >> (32 - n);
        self.pos += n as usize;
        v
    }

    #[inline]
    fn skip(&mut self, n: u32) {
        self.pos += n as usize;
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GranuleChannel {
    pub part2_3_length: u16,
    pub big_values: u16,
    pub global_gain: u8,
    pub scalefac_compress: u8,
    pub window_switching: bool,
    pub block_type: u8,
    pub mixed_block: bool,
    pub table_select: [u8; 3],
    pub subblock_gain: [u8; 3],
    pub region0_count: u8,
    pub region1_count: u8,
    pub preflag: bool,
    pub scalefac_scale: bool,
    pub count1table_select: bool,
}

impl GranuleChannel {
    pub fn read(r: &mut BitReader) -> Result<Self> {
        let mut g = GranuleChannel {
            part2_3_length: r.bits(12) as u16,
            big_values: r.bits(9) as u16,
            global_gain: r.bits(8) as u8,
            scalefac_compress: r.bits(4) as u8,
            window_switching: r.bits(1) != 0,
            ..Default::default()
        };
        if g.window_switching {
            g.block_type = r.bits(2) as u8;
            g.mixed_block = r.bits(1) != 0;
            for t in &mut g.table_select[..2] {
                *t = r.bits(5) as u8;
            }
            for s in &mut g.subblock_gain {
                *s = r.bits(3) as u8;
            }
            if g.block_type == 0 {
                return Err(Error::invalid("Layer III: window switching with block_type 0"));
            }
        } else {
            for t in &mut g.table_select {
                *t = r.bits(5) as u8;
            }
            g.region0_count = r.bits(4) as u8;
            g.region1_count = r.bits(3) as u8;
        }
        g.preflag = r.bits(1) != 0;
        g.scalefac_scale = r.bits(1) != 0;
        g.count1table_select = r.bits(1) != 0;
        if g.big_values > 288 {
            return Err(Error::invalid(format!("Layer III: big_values {} > 288", g.big_values)));
        }
        Ok(g)
    }

    fn is_short(&self) -> bool {
        self.window_switching && self.block_type == 2
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ScaleFactors {
    pub long: [u8; 22],
    pub short: [[u8; 3]; 13],
}

impl ScaleFactors {
    fn read(r: &mut BitReader, g: &GranuleChannel, scfsi: u8, prev: &ScaleFactors) -> Self {
        let (slen1, slen2) = SLEN[g.scalefac_compress as usize];
        let mut sf = ScaleFactors::default();
        if g.is_short() {
            let first_short = if g.mixed_block {
                for v in sf.long.iter_mut().take(8) {
                    *v = r.bits(slen1) as u8;
                }
                3
            } else {
                0
            };
            for (b, windows) in sf.short.iter_mut().enumerate().take(12).skip(first_short) {
                let n = if b < 6 { slen1 } else { slen2 };
                for v in windows {
                    *v = r.bits(n) as u8;
                }
            }
        } else {
            const GROUPS: [(usize, usize); 4] = [(0, 6), (6, 11), (11, 16), (16, 21)];
            for (i, &(a, b)) in GROUPS.iter().enumerate() {
                let n = if i < 2 { slen1 } else { slen2 };
                let reuse = scfsi & (8 >> i) != 0;
                for band in a..b {
                    sf.long[band] = if reuse { prev.long[band] } else { r.bits(n) as u8 };
                }
            }
        }
        sf
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Granule {
    pub sample_rate_index: u8,
    pub channel_mode: u8,
    pub mode_extension: u8,
    pub channels: usize,
    pub granule: u8,
    pub scfsi: [u8; 2],
    pub ch: [GranuleChannel; 2],
}

pub fn decode_granule(
    states: &mut [ChannelState],
    g: &Granule,
    main: &[u8],
    bit_pos: usize,
    pcm: &mut [[f32; 576]; 2],
) -> Result<[ChannelReport; 2]> {
    let nch = g.channels;
    if !(1..=2).contains(&nch) || states.len() < nch || g.sample_rate_index > 2 {
        return Err(Error::invalid(format!("Layer III granule: {nch} channels, rate index {}", g.sample_rate_index)));
    }
    let sfi = g.sample_rate_index as usize;
    let mut q = [0i32; 576];
    let mut xr = [[0f32; 576]; 2];
    let mut sfs = [ScaleFactors::default(); 2];
    let mut reports = [ChannelReport::default(); 2];
    let mut pos = bit_pos;
    for c in 0..nch {
        let gc = &g.ch[c];
        let end = pos + gc.part2_3_length as usize;
        let mut r = BitReader::new(main, pos);
        let scfsi = if g.granule == 1 { g.scfsi[c] } else { 0 };
        sfs[c] = ScaleFactors::read(&mut r, gc, scfsi, &states[c].prev_sf);
        reports[c] = huffman(&mut r, end, gc, sfi, &mut q)?;
        requantize(&q, reports[c].nonzero_end, gc, &sfs[c], sfi, &mut xr[c]);
        states[c].prev_sf = sfs[c];
        pos = end;
    }
    let mut nz = [reports[0].nonzero_end, reports[1].nonzero_end];
    if nch == 2 && g.channel_mode == 1 && g.mode_extension != 0 {
        joint_stereo(&mut xr, &g.ch[1], &sfs[1], sfi, g.mode_extension);
        nz = [576; 2];
    }
    for c in 0..nch {
        if g.ch[c].is_short() {
            reorder(&mut xr[c], sfi, g.ch[c].mixed_block);
            nz[c] = 576;
        }
        states[c].hybrid_synth(&mut xr[c], &g.ch[c], nz[c], &mut pcm[c]);
    }
    Ok(reports)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::PI;

    fn noise(seed: &mut u64) -> f64 {
        *seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (*seed >> 11) as f64 / (1u64 << 52) as f64 - 1.0
    }

    type Entries = Vec<(u16, u32, u8)>;

    fn all_tables() -> Vec<(String, Entries)> {
        let mut t: Vec<_> = PAIR_TABLES
            .iter()
            .enumerate()
            .filter_map(|(i, t)| t.as_ref().map(|t| (format!("table {i}"), pair_entries(t))))
            .collect();
        t.push(("table 16".into(), pair_entries(&PAIR_TABLE_16)));
        t.push(("table 24".into(), pair_entries(&PAIR_TABLE_24)));
        t.push(("count1 A".into(), (0..16).map(|i| (i as u16, QUAD_A_CODE[i], QUAD_A_LEN[i])).collect()));
        t
    }

    #[test]
    fn huffman_tables_are_complete_prefix_codes() {
        for (name, entries) in all_tables() {
            let kraft: u64 = entries.iter().map(|e| 1u64 << (20 - e.2)).sum();
            assert_eq!(kraft, 1 << 20, "{name}: Kraft sum");
            for &(_, code, len) in &entries {
                assert!(len >= 1 && code < 1 << len, "{name}: code {code:#x} / {len}");
            }
            for (i, a) in entries.iter().enumerate() {
                for b in entries.iter().skip(i + 1) {
                    let (short, long) = if a.2 <= b.2 { (a, b) } else { (b, a) };
                    assert_ne!(long.1 >> (long.2 - short.2), short.1, "{name}: prefix");
                }
            }
        }
    }

    #[test]
    fn huffman_lookup_decodes_every_code() {
        let mut seed = 1;
        for (name, entries) in all_tables() {
            let huff = Huff::new(&entries);
            for &(sym, code, len) in &entries {
                let pad = (noise(&mut seed).to_bits() >> 19) & ((1 << (64 - u32::from(len))) - 1);
                let word = (u64::from(code) << (64 - u32::from(len))) | pad;
                let bytes = word.to_be_bytes();
                let mut r = BitReader::new(&bytes, 0);
                assert_eq!(huff.decode(&mut r), u32::from(sym), "{name}");
                assert_eq!(r.pos(), len as usize, "{name}");
            }
        }
    }

    #[test]
    fn bit_reader_reads_msb_first_and_pads_with_zeros() {
        let mut r = BitReader::new(&[0b1010_1100, 0xFF], 1);
        assert_eq!(r.bits(3), 0b010);
        assert_eq!(r.bits(8), 0b1100_1111);
        assert_eq!(r.bits(8), 0b1111_0000);
        assert_eq!(r.pos(), 20);
    }

    #[test]
    fn scfsi_reuses_granule0_band_groups() {
        let g = GranuleChannel { scalefac_compress: 15, ..Default::default() };
        let prev = ScaleFactors { long: std::array::from_fn(|i| i as u8), ..Default::default() };
        let data = [0xFFu8; 5];
        let mut r = BitReader::new(&data, 0);
        let sf = ScaleFactors::read(&mut r, &g, 0b1010, &prev);
        assert_eq!(r.pos(), 35);
        assert_eq!(&sf.long[0..6], &prev.long[0..6]);
        assert!(sf.long[6..11].iter().all(|&v| v == 15));
        assert_eq!(&sf.long[11..16], &prev.long[11..16]);
        assert!(sf.long[16..21].iter().all(|&v| v == 7));
        assert_eq!(sf.long[21], 0);
    }

    fn imdct_direct(x: &[f64; 18], bt: u8) -> [f64; 36] {
        let tr = |input: &[f64], n: usize| -> Vec<f64> {
            (0..n)
                .map(|i| {
                    (0..n / 2)
                        .map(|k| {
                            input[k] * (PI / (2 * n) as f64 * (2 * i + 1 + n / 2) as f64 * (2 * k + 1) as f64).cos()
                        })
                        .sum()
                })
                .collect()
        };
        let s36 = |i: usize| (PI / 36.0 * (i as f64 + 0.5)).sin();
        let s12 = |i: usize| (PI / 12.0 * (i as f64 + 0.5)).sin();
        let mut z = [0.0; 36];
        if bt == 2 {
            for w in 0..3 {
                let xw: Vec<f64> = (0..6).map(|k| x[3 * k + w]).collect();
                for (i, y) in tr(&xw, 12).into_iter().enumerate() {
                    z[6 + 6 * w + i] += y * s12(i);
                }
            }
            return z;
        }
        for (i, y) in tr(x, 36).into_iter().enumerate() {
            let win = match (bt, i) {
                (0, _) => s36(i),
                (1, 0..18) | (3, 18..36) => s36(i),
                (1, 18..24) | (3, 12..18) => 1.0,
                (1, 24..30) => s12(i - 18),
                (3, 6..12) => s12(i - 6),
                _ => 0.0,
            };
            z[i] = y * win;
        }
        z
    }

    #[test]
    fn imdct_matches_direct_formula() {
        let mut seed = 7;
        for bt in [0u8, 1, 2, 3] {
            for _ in 0..20 {
                let x: [f64; 18] = std::array::from_fn(|_| noise(&mut seed));
                let xf: Vec<f32> = x.iter().map(|&v| v as f32).collect();
                let mut z = [0f32; 36];
                imdct(&xf, bt, &mut z);
                for (a, b) in z.iter().zip(imdct_direct(&x, bt)) {
                    assert!((f64::from(*a) - b).abs() < 1e-5, "block type {bt}: {a} vs {b}");
                }
            }
        }
    }

    #[test]
    fn synthesis_matches_direct_formula() {
        let d: Vec<f64> = SYNTH_WINDOW_Q16.iter().map(|&v| f64::from(v) / 65536.0).collect();
        let mut v = vec![0f64; 1024];
        let mut state = ChannelState::default();
        let mut seed = 3;
        for slot in 0..40 {
            let s: [f64; 32] = std::array::from_fn(|_| noise(&mut seed));
            v.copy_within(0..960, 64);
            for (i, vi) in v[..64].iter_mut().enumerate() {
                *vi = (0..32).map(|k| ((16 + i) as f64 * (2 * k + 1) as f64 * PI / 64.0).cos() * s[k]).sum();
            }
            let mut u = [0f64; 512];
            for i in 0..8 {
                for j in 0..32 {
                    u[64 * i + j] = v[128 * i + j];
                    u[64 * i + 32 + j] = v[128 * i + 96 + j];
                }
            }
            let mut out = [0f32; 32];
            state.synth(&std::array::from_fn(|k| s[k] as f32), &mut out);
            for (j, o) in out.iter().enumerate() {
                let direct: f64 = (0..16).map(|i| u[j + 32 * i] * d[j + 32 * i]).sum();
                assert!((f64::from(*o) - direct).abs() < 2e-5, "slot {slot} sample {j}: {o} vs {direct}");
            }
        }
    }

    #[test]
    fn intensity_stereo_long_bands() {
        let mut xr = [[0f32; 576]; 2];
        xr[0].fill(1.0);
        xr[1][..SFB_LONG[0][5]].fill(0.5);
        let mut sf = ScaleFactors::default();
        sf.long[5] = 0;
        sf.long[6] = 7;
        sf.long[7] = 3;
        sf.long[20] = 6;
        joint_stereo(&mut xr, &GranuleChannel::default(), &sf, 0, 3);
        let at = |b: usize| SFB_LONG[0][b];
        let isq = std::f32::consts::FRAC_1_SQRT_2;
        assert!((xr[0][0] - 1.5 * isq).abs() < 1e-6 && (xr[1][0] - 0.5 * isq).abs() < 1e-6);
        assert_eq!((xr[0][at(5)], xr[1][at(5)]), (0.0, 1.0));
        assert!((xr[0][at(6)] - isq).abs() < 1e-6 && (xr[1][at(6)] - isq).abs() < 1e-6);
        assert!((xr[0][at(7)] - 0.5).abs() < 1e-6 && (xr[1][at(7)] - 0.5).abs() < 1e-6);
        assert!((xr[0][at(21)] - 1.0).abs() < 1e-6 && xr[1][at(21)].abs() < 1e-6);
    }
}
