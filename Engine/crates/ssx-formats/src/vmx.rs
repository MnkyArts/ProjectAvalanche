// TODO(guess): denormal operands and results flushed to signed zero (VSCR[NJ] = 1) — the lead is Xenia canary, which runs every VMX op with MXCSR FTZ + DAZ (`DEFAULT_VMX_MXCSR` 0x8040); nothing in the executable sets or reads VSCR; settle: a console's result for a denormal input to a packer (e.g. a quaternion lane of 1e-39).
#[inline]
pub fn daz(x: f32) -> f32 {
    if x.is_subnormal() { 0.0f32.copysign(x) } else { x }
}

// TODO(guess): which of two NaN operands passes through follows the operands' written order (a, b, c); the packers never meet two different NaNs, so no output depends on it; settle: the vector ISA's NaN precedence for vmaddfp / vnmsubfp on a console.
#[inline]
fn lane(r: f32, operands: &[f32]) -> f32 {
    if r.is_nan() {
        operands.iter().find(|x| x.is_nan()).map_or(f32::NAN, |x| f32::from_bits(x.to_bits() | 0x0040_0000))
    } else {
        daz(r)
    }
}

#[inline]
pub fn add(a: f32, b: f32) -> f32 {
    lane(daz(a) + daz(b), &[a, b])
}

#[inline]
pub fn sub(a: f32, b: f32) -> f32 {
    lane(daz(a) - daz(b), &[a, b])
}

#[inline]
pub fn mul(a: f32, b: f32) -> f32 {
    lane(daz(a) * daz(b), &[a, b])
}

// TODO(guess): the multiply-add rounds once (the PowerPC vector ISA's definition; Xenia canary emits FMA); the Xenon's own rounding of the product is not shown anywhere in the executable; settle: a console's packed word for an input on a rounding boundary.
#[inline]
pub fn madd(a: f32, b: f32, c: f32) -> f32 {
    lane(daz(a).mul_add(daz(b), daz(c)), &[a, b, c])
}

#[inline]
pub fn nmsub(a: f32, b: f32, c: f32) -> f32 {
    let r = daz(a).mul_add(daz(b), -daz(c));
    if r.is_nan() { lane(r, &[a, b, c]) } else { daz(-r) }
}

#[inline]
pub fn max(a: f32, b: f32) -> f32 {
    let (a, b) = (daz(a), daz(b));
    if a.is_nan() || b.is_nan() {
        f32::NAN
    } else if a == b {
        if a.is_sign_positive() { a } else { b }
    } else if a > b {
        a
    } else {
        b
    }
}

#[inline]
pub fn gt(a: f32, b: f32) -> bool {
    daz(a) > daz(b)
}

#[inline]
pub fn ge(a: f32, b: f32) -> bool {
    daz(a) >= daz(b)
}

#[inline]
pub fn eq(a: f32, b: f32) -> bool {
    daz(a) == daz(b)
}

#[inline]
pub fn in_bounds(a: f32, b: f32) -> bool {
    let (a, b) = (daz(a), daz(b));
    a <= b && a >= -b
}

#[inline]
pub fn floor(x: f32) -> f32 {
    daz(x).floor()
}

#[inline]
pub fn round(x: f32) -> f32 {
    daz(x).round_ties_even()
}

#[inline]
pub fn to_u32(x: f32) -> u32 {
    daz(x) as u32
}

#[inline]
pub fn from_u32(x: u32) -> f32 {
    x as f32
}

fn products<const N: usize>(a: [f32; N], b: [f32; N]) -> [f64; N] {
    std::array::from_fn(|i| f64::from(daz(a[i])) * f64::from(daz(b[i])))
}

fn dot_result(s: f64) -> f32 {
    let r = s as f32;
    if r.is_infinite() && s.is_finite() { f32::NAN } else { daz(r) }
}

// TODO(guess): the products summed in double precision as (x + z) + y and rounded once — Xenia canary's DOT_PRODUCT_3 (x64_sequences.cc), not the Xenon's documented behaviour; settle: a console's length for inputs whose partial sums round differently.
pub fn dot3(a: [f32; 3], b: [f32; 3]) -> f32 {
    let p = products(a, b);
    dot_result((p[0] + p[2]) + p[1])
}

pub fn dot4(a: [f32; 4], b: [f32; 4]) -> f32 {
    let p = products(a, b);
    dot_result((p[0] + p[2]) + (p[1] + p[3]))
}

const RSQRTE_COEFFICIENTS: [u32; 32] = [
    0x0568_B4FD,
    0x04F3_AF97,
    0x048D_AAA5,
    0x0435_A618,
    0x03E7_A1E4,
    0x03A2_9DFE,
    0x0365_9A5C,
    0x032E_96F8,
    0x02FC_93CA,
    0x02D0_90CE,
    0x02A8_8DFE,
    0x0283_8B57,
    0x0261_88D4,
    0x0243_8673,
    0x0226_8431,
    0x020B_820B,
    0x03D2_7FFA,
    0x0380_7C29,
    0x0338_78AA,
    0x02F9_7572,
    0x02C2_7279,
    0x0292_6FB7,
    0x0266_6D26,
    0x023F_6AC0,
    0x021D_6881,
    0x01FD_6665,
    0x01E1_6468,
    0x01C7_6287,
    0x01AF_60C1,
    0x0199_5F12,
    0x0185_5D79,
    0x0173_5BF4,
];

// TODO(guess): the estimate of a positive normal follows Xenia canary's model (x64_backend.cc `ComputeNormalVRsqrteTableValue`: 32 base / slope pairs by the exponent's parity and mantissa bits 19…22, interpolated over bits 9…18); not checked against a console here — the game refines it twice (one step in the XNA arc sine), so only results on a rounding boundary depend on it; settle: a console's vrsqrtefp outputs.
pub fn rsqrte(x: f32) -> f32 {
    let x = daz(x);
    if x == 0.0 {
        return f32::INFINITY.copysign(x);
    }
    if x.is_nan() {
        return f32::from_bits(x.to_bits() | 0x0040_0000);
    }
    if x < 0.0 {
        return f32::NAN;
    }
    if x.is_infinite() {
        return 0.0;
    }
    let bits = x.to_bits();
    let m = bits & 0x7F_FFFF;
    let c = RSQRTE_COEFFICIENTS[((((bits >> 23) & 1) << 4) | (m >> 19)) as usize];
    let mut est = ((c << 10) & 0x03FF_FC00).wrapping_sub(((m >> 9) & 1023) * (c >> 16));
    let mut adjust = 0i32;
    if est & 0x0200_0000 == 0 {
        let lz = (est & 0x01FF_FFFF).leading_zeros();
        adjust = 6 - lz as i32;
        est <<= lz - 6;
    }
    if est & 5 != 0 && est & 2 != 0 {
        est += 4;
    }
    let canonical = 0x3F80_0000u32.wrapping_add((adjust as u32).wrapping_mul(0x0080_0000)) | ((est >> 2) & 0x7F_FFFF);
    f32::from_bits(canonical.wrapping_sub(((bits >> 24).wrapping_sub(63)) << 23))
}

// TODO(guess): the estimate is the exact quotient (Xenia canary's RECIP_V128 divides); the ISA only bounds its error by 1/4096 and the game refines it twice, so only results on a rounding boundary depend on it; settle: a console's vrefp outputs.
#[inline]
pub fn refp(x: f32) -> f32 {
    lane(1.0 / daz(x), &[x])
}

pub fn rsqrt(x: f32) -> f32 {
    let mut y = rsqrte(x);
    for _ in 0..2 {
        let e = nmsub(x, mul(y, y), 1.0);
        y = madd(mul(y, 0.5), e, y);
    }
    y
}

pub fn recip(x: f32) -> f32 {
    let mut y = refp(x);
    for _ in 0..2 {
        let e = nmsub(y, x, 1.0);
        y = madd(y, e, y);
    }
    y
}

const PI: f32 = f32::from_bits(0x4049_0FDB);
const TWO_PI: f32 = f32::from_bits(0x40C9_0FDB);
const INV_TWO_PI: f32 = f32::from_bits(0x3E22_F983);
const ONE_PLUS_EPSILON: f32 = f32::from_bits(0x3F80_0001);
const SIN: [u32; 11] = [
    0xBE2A_AAAB,
    0x3C08_8889,
    0xB950_0D01,
    0x3638_EF1D,
    0xB2D7_322B,
    0x2F30_9231,
    0xAB57_3F9F,
    0x274A_963C,
    0xA317_A4DA,
    0x1EB8_DC78,
    0x9A3B_0DA1,
];
const COS: [u32; 11] = [
    0xBF00_0000,
    0x3D2A_AAAB,
    0xBAB6_0B61,
    0x37D0_0D01,
    0xB493_F27E,
    0x310F_76C8,
    0xAD49_CBA5,
    0x2957_3F9F,
    0xA534_13C3,
    0x20F2_A15D,
    0x9C86_71CB,
];
const ARC_E: [u32; 4] = [0xBD6D_D42D, 0xBED6_5553, 0x3E66_3246, 0x400B_1889];
const ARC_F: [u32; 4] = [0x3F1D_D7B6, 0x4089_80BD, 0xBF98_3F2F, 0xC0D1_360E];
const ARC_G: [u32; 4] = [0xBFAF_4418, 0xC08F_6AD9, 0x3FB5_8485, 0x40AF_6AD8];

fn reduce(x: f32) -> f32 {
    nmsub(TWO_PI, round(mul(x, INV_TWO_PI)), x)
}

pub fn sin(x: f32) -> f32 {
    let v = reduce(x);
    let x2 = mul(v, v);
    let mut p = mul(x2, v);
    let mut acc = v;
    for c in SIN {
        acc = madd(f32::from_bits(c), p, acc);
        p = mul(p, x2);
    }
    acc
}

pub fn cos(x: f32) -> f32 {
    let v = reduce(x);
    let c = COS.map(f32::from_bits);
    let x2 = mul(v, v);
    let x4 = mul(x2, x2);
    let x6 = mul(x4, x2);
    let x8 = mul(x4, x4);
    let x10 = mul(x6, x4);
    let x12 = mul(x6, x6);
    let x14 = mul(x8, x6);
    let x16 = mul(x8, x8);
    let x18 = mul(x10, x8);
    let x20 = mul(x10, x10);
    let x22 = mul(x12, x10);
    let mut acc = madd(c[0], x2, 1.0);
    for (k, p) in c[1..].iter().zip([x4, x6, x8, x10, x12, x14, x16, x18, x20, x22]) {
        acc = madd(*k, p, acc);
    }
    acc
}

pub fn asin(x: f32) -> f32 {
    let [e, f, g] = [ARC_E, ARC_F, ARC_G].map(|v| v.map(f32::from_bits));
    let ax = f32::from_bits(x.to_bits() & 0x7FFF_FFFF);
    let x2 = mul(x, x);
    let d = sub(ONE_PLUS_EPSILON, ax);
    let xm = nmsub(ax, x, x);
    let t = mul(x2, ax);
    let p: [f32; 4] = std::array::from_fn(|i| madd(madd(e[3 - i], ax, f[3 - i]), ax, g[3 - i]));
    let y0 = rsqrte(d);
    let half_d = mul(d, 0.5);
    let a = madd(p[1], t, p[0]);
    let b = madd(p[3], t, p[2]);
    let step = nmsub(half_d, mul(y0, y0), 0.5);
    let y1 = madd(y0, step, y0);
    madd(mul(xm, b), y1, mul(x, a))
}

pub fn acos(x: f32) -> f32 {
    sub(mul(PI, 0.5), asin(x))
}

// TODO(guess): the mantissa rounds toward zero (the dropped 13 bits cut off) — Xenia's note on the encoding (12ff95197, 2022: "as used in vpkd3d128 (no denormals, rounding towards zero)"); canary since 1182f9d73 / 11815400c (2025-10 / 2026-02) rounds to nearest even, but only to meet pack_test.cc's PACK_FLOAT16_4 expectation (5678.0 → 0x6D8C, 9.0123 → 0x4882), which dates from 2014 (f74aafeb8) and was written against Microsoft's XMConvertFloatToHalf C code (poly::float_to_half, ties to even), not a console; the hardware-run instr_vpkd3d128.s packs only exactly representable values, and no SSX data holds a packed value of a known float; settle: a console's vpkd3d128 type 5 on an inexact value (5678.0: 0x6D8B toward zero, 0x6D8C to nearest even).
pub fn pack_half(x: f32) -> u16 {
    let bits = x.to_bits();
    let abs = bits & 0x7FFF_FFFF;
    let sign = ((bits & 0x8000_0000) >> 16) as u16;
    let magnitude = if abs >= 0x47FF_E000 {
        0x7FFF
    } else if abs < 0x3880_0000 {
        0
    } else {
        ((abs.wrapping_add(0xC800_0000) >> 13) & 0x7FFF) as u16
    };
    magnitude | sign
}

pub fn unpack_half(h: u16) -> f32 {
    let sign = u32::from(h & 0x8000) << 16;
    let exponent = u32::from((h >> 10) & 0x1F);
    if exponent == 0 {
        return f32::from_bits(sign);
    }
    let mantissa = u32::from(h & 0x3FF);
    f32::from_bits(sign | ((exponent + 112) << 23) | (mantissa << 13))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_polynomials_follow_the_functions() {
        for k in -2000..=2000 {
            let x = k as f32 * 0.005;
            assert!((sin(x) - x.sin()).abs() < 8e-7, "sin {x}");
            assert!((cos(x) - x.cos()).abs() < 8e-7, "cos {x}");
        }
        let worst = (-100_000..=100_000)
            .map(|k| {
                let x = k as f32 / 100_000.0;
                (asin(x) - x.asin()).abs().max((acos(x) - x.acos()).abs())
            })
            .fold(0.0f32, f32::max);
        assert!(worst < 3e-5, "{worst}");
        assert!(asin(1.0).is_finite() && acos(-1.0).is_finite());
    }

    #[test]
    fn estimates_and_refinement() {
        for k in 1..20_000 {
            let x = k as f32 * 0.37;
            let exact = 1.0 / f64::from(x).sqrt();
            assert!(((f64::from(rsqrte(x)) - exact) / exact).abs() < 1.0 / 4096.0, "{x}");
            assert!(((f64::from(rsqrt(x)) - exact) / exact).abs() < 2.0 * f64::from(f32::EPSILON), "{x}");
        }
        assert_eq!(rsqrte(0.0), f32::INFINITY);
        assert_eq!(rsqrte(-0.0), f32::NEG_INFINITY);
        assert_eq!(rsqrte(f32::INFINITY), 0.0);
        assert!(rsqrte(-1.0).is_nan());
        assert_eq!(recip(2.0), 0.5);
        assert_eq!(recip(512.0), 1.0 / 512.0);
        assert_eq!(recip(1.5), 1.0 / 1.5);
        assert_eq!(rsqrt(4.0), 0.5);
    }

    #[test]
    fn denormals_and_zeros() {
        let tiny = f32::from_bits(1);
        assert_eq!(mul(tiny, 1.0).to_bits(), 0);
        assert_eq!(add(-tiny, 0.0).to_bits(), 0);
        assert!(!gt(0.0, -tiny));
        assert_eq!(max(-0.0, 0.0).to_bits(), 0);
        assert_eq!(max(0.0, -0.0).to_bits(), 0);
        assert!(max(f32::NAN, 1.0).is_nan());
        assert_eq!(nmsub(1.0, 0.0, 0.0).to_bits(), 0x8000_0000, "−(1·0 − 0) = −0");
        assert_eq!(to_u32(-3.5), 0);
        assert_eq!(to_u32(f32::NAN), 0);
        assert_eq!(to_u32(5e9), u32::MAX);
        assert_eq!(dot3([1e30; 3], [1e30; 3]).to_bits(), f32::NAN.to_bits());
    }

    #[test]
    fn xenos_halves() {
        assert_eq!(pack_half(1234.0), 0x64D2);
        assert_eq!(pack_half(4.5678), 0x4491);
        assert_eq!(pack_half(1.0), 0x3C00);
        assert_eq!(pack_half(-2.0), 0xC000);
        assert_eq!(pack_half(6.103_515_6e-5), 0x0400, "2^-14, the smallest normal");
        assert_eq!(pack_half(6.1e-5), 0, "below 2^-14: zero");
        assert_eq!(pack_half(-1e-6), 0x8000, "a negative zero");
        assert_eq!(pack_half(131_008.0), 0x7FFF);
        assert_eq!(pack_half(1e9), 0x7FFF);
        assert_eq!(pack_half(f32::NEG_INFINITY), 0xFFFF);
        assert_eq!(pack_half(f32::NAN), 0x7FFF);
        assert_eq!(pack_half(5678.0), 0x6D8B);
        assert_eq!(pack_half(9.0123), 0x4881);
        assert_eq!(unpack_half(0x7FFF), 131_008.0);
        assert_eq!(unpack_half(0x7C00), 65_536.0);
        assert_eq!(unpack_half(0x03FF).to_bits(), 0, "exponent 0 is zero");
        assert_eq!(unpack_half(0x8001).to_bits(), 0x8000_0000);
        for h in 0x0400..0x7FFF_u16 {
            assert_eq!(pack_half(unpack_half(h)), h);
            assert_eq!(pack_half(unpack_half(h | 0x8000)), h | 0x8000);
        }
    }
}
