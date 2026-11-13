#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Matrix {
    Bt709,
    Bt601,
    Bt2020,
    Fcc,
    Smpte240,
}

impl Matrix {
    pub fn of(code: u8) -> Option<Matrix> {
        Some(match code {
            1 => Matrix::Bt709,
            // PORT(choice): for mod movies — an unspecified matrix (2) is taken as BT.601: FFmpeg's swscale converts
            // RGB with BT.601 when no colorspace is given and then leaves the stream untagged; tagged movies need none
            // of it (the template tags BT.709), else the author's tag; the game has no such movies.
            2 | 5 | 6 => Matrix::Bt601,
            9 => Matrix::Bt2020,
            4 => Matrix::Fcc,
            7 => Matrix::Smpte240,
            _ => return None,
        })
    }

    fn k(self) -> (f64, f64) {
        match self {
            Matrix::Bt709 => (0.2126, 0.0722),
            Matrix::Bt601 => (0.299, 0.114),
            Matrix::Bt2020 => (0.2627, 0.0593),
            Matrix::Fcc => (0.30, 0.11),
            Matrix::Smpte240 => (0.212, 0.087),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Siting {
    Centre,
    Left,
    TopLeft,
}

impl Siting {
    pub fn of(code: u8) -> Siting {
        match code {
            1 => Siting::Left,
            2 => Siting::TopLeft,
            // PORT(choice): for mod movies — an unknown chroma position (0) is taken as centred, as the disc's VP6
            // player converts (¾ / ¼ of the two nearest chroma samples per axis); a movie that states its position
            // needs none of it (FFmpeg's libaom writes it from `-chroma_sample_location`); the game has no such movies.
            _ => Siting::Centre,
        }
    }
}

pub struct Planes<'a> {
    pub y: &'a [u8],
    pub ystride: usize,
    pub chroma: Option<(&'a [u8], &'a [u8])>,
    pub cstride: usize,
    pub width: usize,
    pub height: usize,
}

const ONE: i64 = 1 << 16;

pub fn to_rgba(p: &Planes, matrix: Matrix, full_range: bool, siting: Siting, out: &mut [u8]) {
    let (kr, kb) = matrix.k();
    let kg = 1.0 - kr - kb;
    let (ys, cs, y0) = if full_range { (1.0, 1.0, 0) } else { (255.0 / 219.0, 255.0 / 224.0, 16) };
    let fix = |v: f64| (v * ONE as f64).round() as i64;
    let yk = fix(ys);
    let r_cr = fix(2.0 * (1.0 - kr) * cs);
    let g_cb = fix(2.0 * kb * (1.0 - kb) / kg * cs);
    let g_cr = fix(2.0 * kr * (1.0 - kr) / kg * cs);
    let b_cb = fix(2.0 * (1.0 - kb) * cs);
    let clamp = |v: i64| ((v + ONE / 2) >> 16).clamp(0, 255) as u8;
    let (w, h) = (p.width, p.height);
    let cw = w.div_ceil(2);
    let (mut cb_row, mut cr_row) = (vec![0i64; cw], vec![0i64; cw]);
    for y in 0..h {
        let row = &mut out[y * w * 4..(y + 1) * w * 4];
        let luma = &p.y[y * p.ystride..y * p.ystride + w];
        let Some((cb, cr)) = p.chroma else {
            for (px, &l) in row.as_chunks_mut::<4>().0.iter_mut().zip(luma) {
                let v = clamp(yk * (i64::from(l) - y0));
                *px = [v, v, v, 255];
            }
            continue;
        };
        let ch = h.div_ceil(2);
        let (a, b, wa, wb) = match siting {
            Siting::TopLeft if y % 2 == 0 => (y / 2, y / 2, 4, 0),
            Siting::TopLeft => (y / 2, (y / 2 + 1).min(ch - 1), 2, 2),
            _ if y % 2 == 0 => (y / 2, (y / 2).saturating_sub(1), 3, 1),
            _ => (y / 2, (y / 2 + 1).min(ch - 1), 3, 1),
        };
        for x in 0..cw {
            let at = |plane: &[u8], r: usize| i64::from(plane[r * p.cstride + x]);
            cb_row[x] = wa * at(cb, a) + wb * at(cb, b);
            cr_row[x] = wa * at(cr, a) + wb * at(cr, b);
        }
        for (x, (px, &l)) in row.as_chunks_mut::<4>().0.iter_mut().zip(luma).enumerate() {
            let (c, n) = (x / 2, (x / 2 + 1).min(cw - 1));
            let (cb16, cr16) = match siting {
                Siting::Left | Siting::TopLeft if x % 2 == 0 => (4 * cb_row[c], 4 * cr_row[c]),
                Siting::Left | Siting::TopLeft => (2 * (cb_row[c] + cb_row[n]), 2 * (cr_row[c] + cr_row[n])),
                _ => {
                    let o = if x % 2 == 0 { c.saturating_sub(1) } else { n };
                    (3 * cb_row[c] + cb_row[o], 3 * cr_row[c] + cr_row[o])
                }
            };
            let (u, v) = (cb16 - 128 * 16, cr16 - 128 * 16);
            let yy = yk * (i64::from(l) - y0);
            px[0] = clamp(yy + r_cr * v / 16);
            px[1] = clamp(yy - (g_cb * u + g_cr * v) / 16);
            px[2] = clamp(yy + b_cb * u / 16);
            px[3] = 255;
        }
    }
}

pub fn packed_len(width: usize, height: usize, mono: bool) -> usize {
    let (cw, ch) = (width.div_ceil(2), height.div_ceil(2));
    width * height + if mono { 0 } else { 2 * cw * ch }
}

pub fn packed_to_rgba(
    planes: &[u8],
    width: usize,
    height: usize,
    mono: bool,
    m: Matrix,
    full: bool,
    s: Siting,
) -> Vec<u8> {
    let (cw, ch) = (width.div_ceil(2), height.div_ceil(2));
    let y = &planes[..width * height];
    let chroma = (!mono).then(|| {
        let cb = &planes[width * height..width * height + cw * ch];
        (cb, &planes[width * height + cw * ch..width * height + 2 * cw * ch])
    });
    let mut out = vec![0u8; width * height * 4];
    to_rgba(&Planes { y, ystride: width, chroma, cstride: cw, width, height }, m, full, s, &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one(y: u8, cb: u8, cr: u8, m: Matrix, full: bool) -> [u8; 4] {
        let planes = [y, y, y, y, cb, cr];
        let out = packed_to_rgba(&planes, 2, 2, false, m, full, Siting::Centre);
        assert!(out.chunks(4).all(|p| p == &out[..4]));
        [out[0], out[1], out[2], out[3]]
    }

    #[test]
    fn limited_and_full_range_ends() {
        for m in [Matrix::Bt709, Matrix::Bt601, Matrix::Bt2020] {
            assert_eq!(one(16, 128, 128, m, false), [0, 0, 0, 255]);
            assert_eq!(one(235, 128, 128, m, false), [255, 255, 255, 255]);
            assert_eq!(one(0, 128, 128, m, true), [0, 0, 0, 255]);
            assert_eq!(one(255, 128, 128, m, true), [255, 255, 255, 255]);
        }
    }

    #[test]
    fn each_matrix_decodes_its_own_red() {
        let r709 = one(63, 102, 240, Matrix::Bt709, false);
        let r601 = one(81, 90, 240, Matrix::Bt601, false);
        for (p, name) in [(r709, "709"), (r601, "601")] {
            assert!(p[0] >= 253 && p[1] <= 2 && p[2] <= 2, "{name}: {p:?}");
        }
        let crossed = one(81, 90, 240, Matrix::Bt709, false);
        assert!(crossed[1] > 10 || crossed[0] < 250, "{crossed:?}");
    }

    #[test]
    fn siting_moves_the_chroma() {
        let planes = [128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 160];
        let red = |s: Siting| -> Vec<u8> {
            packed_to_rgba(&planes, 4, 2, false, Matrix::Bt709, true, s).chunks(4).take(4).map(|p| p[0]).collect()
        };
        let (left, centre) = (red(Siting::Left), red(Siting::Centre));
        assert!(left[1] > centre[1] && left[2] > centre[2], "{left:?} {centre:?}");
        assert_eq!(left[0], centre[0]);
    }
}
