use super::{Interpolation, Keys};

pub use ssx_formats::ant_anim::nlerp;

fn value(keys: &Keys, width: usize, k: usize, out: &mut [f32; 4]) {
    let stride = width * keys.interpolation.per_key();
    let at = k * stride + if keys.interpolation == Interpolation::CubicSpline { width } else { 0 };
    out[..width].copy_from_slice(&keys.values[at..at + width]);
}

fn tangent(keys: &Keys, width: usize, k: usize, part: usize) -> [f32; 4] {
    let at = k * width * 3 + part * width;
    let mut out = [0.0; 4];
    out[..width].copy_from_slice(&keys.values[at..at + width]);
    out
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Blend {
    Rotation,
    Vector,
    Scalar,
}

impl Blend {
    fn width(self) -> usize {
        match self {
            Blend::Rotation => 4,
            Blend::Vector => 3,
            Blend::Scalar => 1,
        }
    }
}

fn normalised(q: [f32; 4]) -> [f32; 4] {
    nlerp(q, q, 0.0)
}

pub(super) fn sample(keys: &Keys, blend: Blend, seconds: f32) -> [f32; 4] {
    let width = blend.width();
    let rotation = blend == Blend::Rotation;
    let mut out = [0.0; 4];
    let times = &keys.times;
    let n = times.len();
    if n == 0 {
        return out;
    }
    if n == 1 {
        value(keys, width, 0, &mut out);
        return out;
    }
    let clamped = if seconds.is_nan() || seconds <= times[0] {
        Some(0)
    } else if seconds >= times[n - 1] {
        Some(n - 1)
    } else {
        None
    };
    if let Some(k) = clamped {
        value(keys, width, k, &mut out);
        return if rotation { normalised(out) } else { out };
    }
    let i = times.partition_point(|&t| t <= seconds) - 1;
    let (t0, t1) = (times[i], times[i + 1]);
    let u = (seconds - t0) / (t1 - t0);
    let (mut a, mut b) = ([0.0; 4], [0.0; 4]);
    value(keys, width, i, &mut a);
    value(keys, width, i + 1, &mut b);
    match keys.interpolation {
        Interpolation::Step if rotation => normalised(a),
        Interpolation::Step => a,
        // PORT(choice): rotations between linear keys by the game's own blend (sign-fixed nlerp, 0x82D82160, as every
        // disc codec samples between its keys) where glTF 2.0 asks a slerp; both agree at the keys, between keys whose
        // quaternions are θ apart they differ by less than θ³/50 (2e-5 at θ = 0.1), and nlerp needs no sin/acos, so it
        // is bit-identical on every platform; tuning: whether modders' clips with sparse keys need slerp (compare both
        // on an exported clip resampled to 10 keys a second); the game imports no glTF clips.
        Interpolation::Linear if rotation => nlerp(a, b, u),
        Interpolation::Linear if blend == Blend::Scalar => [(b[0] - a[0]).mul_add(u, a[0]), 0.0, 0.0, 0.0],
        Interpolation::Linear => {
            for k in 0..width {
                out[k] = (b[k] - a[k]) * u + a[k];
            }
            out
        }
        Interpolation::CubicSpline => {
            let dt = t1 - t0;
            let (m0, m1) = (tangent(keys, width, i, 2), tangent(keys, width, i + 1, 0));
            let (u2, u3) = (u * u, u * u * u);
            let h00 = 2.0 * u3 - 3.0 * u2 + 1.0;
            let h10 = u3 - 2.0 * u2 + u;
            let h01 = -2.0 * u3 + 3.0 * u2;
            let h11 = u3 - u2;
            for k in 0..width {
                out[k] = h00 * a[k] + h10 * dt * m0[k] + h01 * b[k] + h11 * dt * m1[k];
            }
            if rotation {
                let len = (out[0] * out[0] + out[1] * out[1] + out[2] * out[2] + out[3] * out[3]).sqrt();
                if len > 1e-6 && len.is_finite() {
                    out = out.map(|x| x / len);
                } else {
                    out = normalised(if u < 0.5 { a } else { b });
                }
            }
            out
        }
    }
}
