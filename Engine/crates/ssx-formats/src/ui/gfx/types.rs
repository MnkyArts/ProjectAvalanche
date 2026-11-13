use super::bits::SwfReader;
use crate::Result;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rect {
    pub x_min: i32,
    pub x_max: i32,
    pub y_min: i32,
    pub y_max: i32,
}

impl Rect {
    pub fn read(r: &mut SwfReader) -> Result<Self> {
        r.align();
        let n = r.ub(5)?;
        let rect = Rect { x_min: r.sb(n)?, x_max: r.sb(n)?, y_min: r.sb(n)?, y_max: r.sb(n)? };
        r.align();
        Ok(rect)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rgba {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Rgba {
    pub fn read_rgb(r: &mut SwfReader) -> Result<Self> {
        Ok(Rgba { r: r.u8()?, g: r.u8()?, b: r.u8()?, a: 255 })
    }

    pub fn read_rgba(r: &mut SwfReader) -> Result<Self> {
        Ok(Rgba { r: r.u8()?, g: r.u8()?, b: r.u8()?, a: r.u8()? })
    }

    pub fn read(r: &mut SwfReader, alpha: bool) -> Result<Self> {
        if alpha { Self::read_rgba(r) } else { Self::read_rgb(r) }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Matrix {
    pub a: f32,
    pub b: f32,
    pub c: f32,
    pub d: f32,
    pub tx: i32,
    pub ty: i32,
}

impl Default for Matrix {
    fn default() -> Self {
        Matrix::IDENTITY
    }
}

impl Matrix {
    pub const IDENTITY: Matrix = Matrix { a: 1.0, b: 0.0, c: 0.0, d: 1.0, tx: 0, ty: 0 };

    pub fn read(r: &mut SwfReader) -> Result<Self> {
        r.align();
        let mut m = Matrix::IDENTITY;
        if r.flag()? {
            let n = r.ub(5)?;
            m.a = r.fb(n)?;
            m.d = r.fb(n)?;
        }
        if r.flag()? {
            let n = r.ub(5)?;
            m.b = r.fb(n)?;
            m.c = r.fb(n)?;
        }
        let n = r.ub(5)?;
        m.tx = r.sb(n)?;
        m.ty = r.sb(n)?;
        r.align();
        Ok(m)
    }

    pub fn concat(&self, o: &Matrix) -> Matrix {
        Matrix {
            a: self.a * o.a + self.c * o.b,
            b: self.b * o.a + self.d * o.b,
            c: self.a * o.c + self.c * o.d,
            d: self.b * o.c + self.d * o.d,
            tx: (self.a * o.tx as f32 + self.c * o.ty as f32) as i32 + self.tx,
            ty: (self.b * o.tx as f32 + self.d * o.ty as f32) as i32 + self.ty,
        }
    }

    pub fn apply(&self, x: f32, y: f32) -> (f32, f32) {
        (self.a * x + self.c * y + self.tx as f32, self.b * x + self.d * y + self.ty as f32)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ColorTransform {
    pub mult: [i16; 4],
    pub add: [i16; 4],
}

impl Default for ColorTransform {
    fn default() -> Self {
        ColorTransform { mult: [256; 4], add: [0; 4] }
    }
}

impl ColorTransform {
    pub fn read(r: &mut SwfReader, alpha: bool) -> Result<Self> {
        r.align();
        let has_add = r.flag()?;
        let has_mult = r.flag()?;
        let n = r.ub(4)?;
        let mut cx = ColorTransform::default();
        let channels = if alpha { 4 } else { 3 };
        if has_mult {
            for m in cx.mult.iter_mut().take(channels) {
                *m = r.sb(n)? as i16;
            }
        }
        if has_add {
            for a in cx.add.iter_mut().take(channels) {
                *a = r.sb(n)? as i16;
            }
        }
        r.align();
        Ok(cx)
    }

    pub fn concat(&self, o: &ColorTransform) -> ColorTransform {
        let mut out = ColorTransform::default();
        for c in 0..4 {
            let m = i32::from(self.mult[c]);
            out.mult[c] = (m * i32::from(o.mult[c]) / 256) as i16;
            out.add[c] = (m * i32::from(o.add[c]) / 256 + i32::from(self.add[c])) as i16;
        }
        out
    }

    pub fn apply(&self, c: Rgba) -> Rgba {
        let ch = |v: u8, i: usize| {
            (i32::from(v) * i32::from(self.mult[i]) / 256 + i32::from(self.add[i])).clamp(0, 255) as u8
        };
        Rgba { r: ch(c.r, 0), g: ch(c.g, 1), b: ch(c.b, 2), a: ch(c.a, 3) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rect_and_matrix() {
        let mut r = SwfReader::new(&[0b0001_0011, 0b1000_1000, 0xAB]);
        assert_eq!(Rect::read(&mut r).unwrap(), Rect { x_min: 1, x_max: -1, y_min: 0, y_max: 1 });
        assert_eq!(r.u8().unwrap(), 0xAB);
        let mut r = SwfReader::new(&[0x00]);
        assert_eq!(Matrix::read(&mut r).unwrap(), Matrix::IDENTITY);
        assert!(r.at_end());
    }

    #[test]
    fn colour_transform_mult_only() {
        let mut bits = String::from("01");
        bits += "1010";
        for v in [256u32, 256, 256, 128] {
            bits += &format!("{v:010b}");
        }
        while bits.len() % 8 != 0 {
            bits.push('0');
        }
        let bytes: Vec<u8> = bits
            .as_bytes()
            .chunks(8)
            .map(|c| u8::from_str_radix(std::str::from_utf8(c).unwrap(), 2).unwrap())
            .collect();
        let cx = ColorTransform::read(&mut SwfReader::new(&bytes), true).unwrap();
        assert_eq!(cx.mult, [256, 256, 256, 128]);
        assert_eq!(cx.apply(Rgba { r: 10, g: 20, b: 30, a: 200 }).a, 100);
    }
}
