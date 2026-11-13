use crate::geom::{Mat2d, RectF};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScaleMode {
    NoScale,
    ShowAll,
    ExactFit,
    NoBorder,
}

pub fn stage_scale(stage: (f32, f32), viewport: (f32, f32), mode: ScaleMode) -> (f32, f32) {
    let ((sw, sh), (vw, vh)) = (stage, viewport);
    match mode {
        ScaleMode::NoScale => (1.0, 1.0),
        ScaleMode::ExactFit => (vw / sw, vh / sh),
        ScaleMode::ShowAll => {
            let k = (vw / sw).min(vh / sh);
            (k, k)
        }
        ScaleMode::NoBorder => {
            // PORT(choice): a viewport wider than the stage (21:9, 32:9 — wider than any display the game knew: 16:9 at
            // 1280 × 720, or 4:3 at 960 × 720, 0x82FB57A0) shows the whole stage at the viewport's height, centred, the
            // sides beyond it showing what the movies draw there and the 3-D scene behind (`noBorder` would scale to
            // the width and crop the top and bottom: the HUD's and the menus' edges lost); 16:9 and narrower keep
            // `noBorder` exactly (the FE view set-up's SetViewScaleMode(3), 0x82593800); the console never drew wider.
            let k = if vw * sh > vh * sw { vh / sh } else { (vw / sw).max(vh / sh) };
            (k, k)
        }
    }
}

pub fn stage_transform(stage: RectF, viewport: (u32, u32), mode: ScaleMode) -> Mat2d {
    let (sw, sh) = ((stage.width() / 20.0).max(1.0), (stage.height() / 20.0).max(1.0));
    let (vw, vh) = (viewport.0 as f32, viewport.1 as f32);
    let (kx, ky) = stage_scale((sw, sh), (vw, vh), mode);
    let (ox, oy) = ((vw - sw * kx) / 2.0, (vh - sh * ky) / 2.0);
    Mat2d::translate(ox, oy).mul(&Mat2d::scale(kx / 20.0, ky / 20.0)).mul(&Mat2d::translate(-stage.x0, -stage.y0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_border_fills_and_centres() {
        let stage = RectF::new(0.0, 0.0, 1280.0 * 20.0, 720.0 * 20.0);
        let m = stage_transform(stage, (1024, 768), ScaleMode::NoBorder);
        let k = 768.0 / 720.0;
        assert!((m.a - k / 20.0).abs() < 1e-6);
        let (x, _) = m.apply(0.0, 0.0);
        assert!((x - (1024.0 - 1280.0 * k) / 2.0).abs() < 1e-3, "{x}");
        let s = stage_transform(stage, (1024, 768), ScaleMode::ShowAll);
        assert!((s.a - 0.8 / 20.0).abs() < 1e-6);
    }

    #[test]
    fn the_stage_at_each_ratio() {
        let stage = RectF::new(0.0, 0.0, 1280.0 * 20.0, 720.0 * 20.0);
        let place = |w: u32, h: u32| {
            let m = stage_transform(stage, (w, h), ScaleMode::NoBorder);
            let (x0, y0) = m.apply(0.0, 0.0);
            let (x1, y1) = m.apply(1280.0 * 20.0, 720.0 * 20.0);
            (x0, y0, x1, y1)
        };
        for (w, h) in [(1920u32, 1080u32), (1280, 720)] {
            let (vw, vh) = (w as f32, h as f32);
            let k = (vw / 1280.0).max(vh / 720.0);
            let before = Mat2d::translate((vw - 1280.0 * k) / 2.0, (vh - 720.0 * k) / 2.0)
                .mul(&Mat2d::scale(k / 20.0, k / 20.0))
                .mul(&Mat2d::translate(-stage.x0, -stage.y0));
            assert_eq!(stage_transform(stage, (w, h), ScaleMode::NoBorder), before, "{w}x{h}");
            let (x0, y0, x1, y1) = place(w, h);
            assert!(x0.abs() < 1e-3 && y0.abs() < 1e-3 && (x1 - vw).abs() < 1e-3 && (y1 - vh).abs() < 1e-3, "{w}x{h}");
        }
        let near = |got: (f32, f32, f32, f32), want: (f32, f32, f32, f32)| {
            [(got.0, want.0), (got.1, want.1), (got.2, want.2), (got.3, want.3)]
                .iter()
                .all(|(g, w)| (g - w).abs() < 1e-3)
        };
        for (w, h, x0, x1) in [(1024, 768, -170.666_67, 1_194.666_7), (1280, 800, -71.111_11, 1_351.111_1)] {
            assert!(near(place(w, h), (x0, 0.0, x1, h as f32)), "{w}x{h}: {:?}", place(w, h));
        }
        for (w, h, side) in [(2560, 1080, 320.0), (3440, 1440, 440.0), (5120, 1440, 1280.0)] {
            assert!(near(place(w, h), (side, 0.0, w as f32 - side, h as f32)), "{w}x{h}: {:?}", place(w, h));
        }
    }
}
