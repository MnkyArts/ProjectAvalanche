use eframe::egui::{Pos2, Rect, Vec2, pos2, vec2};

pub const CANVAS: Vec2 = vec2(1280.0, 720.0);
pub const MARGIN: f32 = 48.0;
pub const GUTTER: f32 = 16.0;
pub const MENU_W: f32 = 256.0;
pub const MENU_GAP: f32 = 32.0;
pub const TAB_TIP: f32 = 16.0;
pub const HEADER_TOP: f32 = 40.0;
pub const HEADER_H: f32 = 96.0;
pub const CONTENT_TOP: f32 = 168.0;
pub const HERO_H: f32 = 288.0;
pub const TILE_H: f32 = 184.0;
pub const ENTRY_H: f32 = 72.0;
pub const EXIT_H: f32 = 48.0;
pub const FOOTER_H: f32 = 32.0;
pub const CUT_HERO: f32 = 16.0;
pub const CUT_TILE: f32 = 12.0;
pub const CUT_BUTTON: f32 = 6.0;
pub const PAD: f32 = 24.0;
pub const PANEL_HEAD: f32 = 64.0;
pub const PANEL_GAP: f32 = 16.0;
pub const DETAILS_W: f32 = 288.0;

pub fn pixels_per_point(physical: Vec2) -> f32 {
    (physical.x / CANVAS.x).min(physical.y / CANVAS.y).max(0.25)
}

#[derive(Clone, Debug, PartialEq)]
pub struct Layout {
    pub screen: Rect,
    pub canvas: Rect,
    pub ppp: f32,
    pub header: Rect,
    pub menu: Rect,
    pub entries: [Rect; 5],
    pub exit: Rect,
    pub content: Rect,
    pub hero: Rect,
    pub tiles: [Rect; 3],
    pub footer: Rect,
    pub body: Rect,
    pub body_wide: Rect,
    pub details: Rect,
}

impl Layout {
    pub fn new(screen: Rect, ppp: f32) -> Self {
        let snap = |v: f32| (v * ppp).round() / ppp;
        let origin = screen.center() - CANVAS / 2.0;
        let (ox, oy) = (snap(origin.x), snap(origin.y));
        let x = |u: f32| snap(ox + u);
        let y = |u: f32| snap(oy + u);
        let rect = |l: f32, t: f32, r: f32, b: f32| Rect::from_min_max(pos2(x(l), y(t)), pos2(x(r), y(b)));

        let menu_l = MARGIN;
        let menu_r = menu_l + MENU_W;
        let content_l = menu_r + MENU_GAP;
        let content_r = CANVAS.x - MARGIN;
        let hero_b = CONTENT_TOP + HERO_H;
        let tiles_t = hero_b + GUTTER;
        let content_b = tiles_t + TILE_H;
        let tile_w = (content_r - content_l - 2.0 * GUTTER) / 3.0;
        let entry = |i: usize| {
            let t = CONTENT_TOP + ENTRY_H * i as f32;
            rect(menu_l, t, menu_r, t + ENTRY_H)
        };
        let tile = |i: usize| {
            let l = content_l + (tile_w + GUTTER) * i as f32;
            rect(l, tiles_t, l + tile_w, content_b)
        };
        let footer_t = content_b + (CANVAS.y - content_b - FOOTER_H) / 2.0;
        let body_t = CONTENT_TOP + PANEL_HEAD + PANEL_GAP;
        let details_l = content_r - PAD - DETAILS_W;
        Self {
            screen,
            canvas: Rect::from_min_size(Pos2::new(ox, oy), CANVAS),
            ppp,
            header: rect(MARGIN, HEADER_TOP, content_r, HEADER_TOP + HEADER_H),
            menu: rect(menu_l, CONTENT_TOP, menu_r, content_b),
            entries: [entry(0), entry(1), entry(2), entry(3), entry(4)],
            exit: rect(menu_l, content_b - EXIT_H, menu_r, content_b),
            content: rect(content_l, CONTENT_TOP, content_r, content_b),
            hero: rect(content_l, CONTENT_TOP, content_r, hero_b),
            tiles: [tile(0), tile(1), tile(2)],
            footer: rect(MARGIN, footer_t, content_r, footer_t + FOOTER_H),
            body: rect(content_l + PAD, body_t, details_l - PAD, content_b - PAD),
            body_wide: rect(content_l + PAD, body_t, content_r - PAD, content_b - PAD),
            details: rect(details_l, body_t, content_r - PAD, content_b - PAD),
        }
    }

    pub fn snap(&self, v: f32) -> f32 {
        (v * self.ppp).round() / self.ppp
    }

    pub fn px(&self, units: f32) -> f32 {
        (units * self.ppp).round().max(1.0) / self.ppp
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WINDOWS: [(f32, f32); 11] = [
        (1280.0, 720.0),
        (1920.0, 1080.0),
        (960.0, 540.0),
        (2560.0, 1440.0),
        (3840.0, 2160.0),
        (1366.0, 768.0),
        (1280.0, 800.0),
        (1024.0, 768.0),
        (1720.0, 720.0),
        (3440.0, 1440.0),
        (1601.0, 937.0),
    ];

    fn layout(w: f32, h: f32) -> Layout {
        let ppp = pixels_per_point(vec2(w, h));
        Layout::new(Rect::from_min_size(Pos2::ZERO, vec2(w, h) / ppp), ppp)
    }

    fn parts(l: &Layout) -> Vec<(&'static str, Rect)> {
        let mut parts = vec![("header", l.header), ("menu", l.menu), ("hero", l.hero), ("footer", l.footer)];
        parts.extend(l.tiles.iter().map(|r| ("tile", *r)));
        parts
    }

    fn panel_parts(l: &Layout) -> [(&'static str, Rect); 3] {
        [("body", l.body), ("wide body", l.body_wide), ("details", l.details)]
    }

    #[test]
    fn the_panel_body_and_the_details_panel() {
        for (w, h) in WINDOWS {
            let l = layout(w, h);
            let pixel = 1.01 / l.ppp;
            for (name, r) in panel_parts(&l) {
                assert!(l.content.contains_rect(r), "{w} x {h}: {name} {r:?} leaves the panel");
                assert!(r.width() > 0.0 && r.height() > 0.0, "{w} x {h}: {name} is empty");
            }
            let body = l.body.intersect(l.details);
            assert!(body.width() <= 0.0 || body.height() <= 0.0, "{w} x {h}: the body overlaps the details panel");
            let gap = l.details.left() - l.body.right();
            assert!((gap - PAD).abs() <= pixel, "{w} x {h}: body to details {gap}");
            assert!((l.content.right() - l.details.right() - PAD).abs() <= pixel, "{w} x {h}");
            assert!((l.content.bottom() - l.body.bottom() - PAD).abs() <= pixel, "{w} x {h}");
            assert!((l.body.left() - l.content.left() - PAD).abs() <= pixel, "{w} x {h}");
            assert!((l.body.top() - l.content.top() - PANEL_HEAD - PANEL_GAP).abs() <= pixel, "{w} x {h}");
            assert_eq!((l.details.top(), l.details.bottom()), (l.body.top(), l.body.bottom()), "{w} x {h}");
            assert_eq!((l.body_wide.left(), l.body_wide.right()), (l.body.left(), l.details.right()), "{w} x {h}");
            assert_eq!((l.body_wide.top(), l.body_wide.bottom()), (l.body.top(), l.body.bottom()), "{w} x {h}");
            assert!(l.body.width() >= 500.0 && (l.details.width() - DETAILS_W).abs() <= pixel, "{w} x {h}");
            for (_, r) in panel_parts(&l) {
                for edge in [r.left(), r.right(), r.top(), r.bottom()] {
                    let px = edge * l.ppp;
                    assert!((px - px.round()).abs() < 0.01, "{w} x {h}: an edge at {px} px");
                }
            }
        }
    }

    #[test]
    fn the_canvas_is_centred_and_fills_one_axis() {
        for (w, h) in WINDOWS {
            let l = layout(w, h);
            let pixel = 1.0 / l.ppp;
            assert!(l.screen.expand(pixel).contains_rect(l.canvas), "{w} x {h}: {:?} in {:?}", l.canvas, l.screen);
            assert!((l.canvas.center() - l.screen.center()).length() <= pixel, "{w} x {h}");
            let fills = (l.screen.width() - CANVAS.x).abs() < 0.01 || (l.screen.height() - CANVAS.y).abs() < 0.01;
            assert!(fills, "{w} x {h}: screen {:?}", l.screen.size());
        }
    }

    #[test]
    fn every_part_is_inside_the_canvas_and_none_overlap() {
        for (w, h) in WINDOWS {
            let l = layout(w, h);
            let parts = parts(&l);
            for (name, r) in &parts {
                assert!(r.width() > 0.0 && r.height() > 0.0, "{w} x {h}: {name} is empty");
                assert!(l.canvas.expand(1.0 / l.ppp).contains_rect(*r), "{w} x {h}: {name} {r:?} leaves the canvas");
            }
            for (i, (a_name, a)) in parts.iter().enumerate() {
                for (b_name, b) in &parts[i + 1..] {
                    let both = a.intersect(*b);
                    assert!(both.width() <= 0.0 || both.height() <= 0.0, "{w} x {h}: {a_name} overlaps {b_name}");
                }
            }
            for pair in l.entries.windows(2) {
                assert_eq!(pair[0].bottom(), pair[1].top(), "{w} x {h}");
            }
            assert!(l.entries[4].bottom() < l.exit.top(), "{w} x {h}");
            assert!(l.entries.iter().chain([&l.exit]).all(|r| l.menu.contains_rect(*r)), "{w} x {h}");
            assert!(
                l.content.contains_rect(l.hero) && l.tiles.iter().all(|t| l.content.contains_rect(*t)),
                "{w} x {h}"
            );
        }
    }

    #[test]
    fn edges_are_shared() {
        for (w, h) in WINDOWS {
            let l = layout(w, h);
            let pixel = 1.01 / l.ppp;
            assert_eq!((l.hero.left(), l.hero.right()), (l.content.left(), l.content.right()), "{w} x {h}");
            assert_eq!((l.tiles[0].left(), l.tiles[2].right()), (l.hero.left(), l.hero.right()), "{w} x {h}");
            assert_eq!((l.menu.top(), l.menu.bottom()), (l.hero.top(), l.tiles[0].bottom()), "{w} x {h}");
            assert!(l.tiles.iter().all(|t| t.top() == l.tiles[0].top() && t.bottom() == l.content.bottom()));
            assert_eq!((l.entries[0].top(), l.entries[3].bottom()), (l.menu.top(), l.hero.bottom()), "{w} x {h}");
            assert_eq!(l.exit.bottom(), l.menu.bottom(), "{w} x {h}");
            let gutter = l.tiles[0].top() - l.hero.bottom();
            for pair in l.tiles.windows(2) {
                assert!((pair[1].left() - pair[0].right() - gutter).abs() <= pixel, "{w} x {h}: gutters differ");
                assert!((pair[1].width() - pair[0].width()).abs() <= pixel, "{w} x {h}: tile widths differ");
            }
            let (left, right) = (l.menu.left() - l.canvas.left(), l.canvas.right() - l.content.right());
            assert!((left - right).abs() <= pixel, "{w} x {h}: margins {left} and {right}");
        }
    }

    #[test]
    fn edges_are_on_pixels() {
        for (w, h) in WINDOWS {
            let l = layout(w, h);
            let all = parts(&l).into_iter().map(|(_, r)| r).chain(l.entries).chain([l.exit, l.content]);
            for r in all {
                for edge in [r.left(), r.right(), r.top(), r.bottom()] {
                    let px = edge * l.ppp;
                    assert!((px - px.round()).abs() < 0.01, "{w} x {h}: an edge at {px} px");
                }
            }
            assert_eq!(l.px(1.0) * l.ppp, 1.0_f32.max(l.ppp.round()), "{w} x {h}: a line is whole pixels");
        }
    }

    #[test]
    fn the_grid_at_720p() {
        let l = layout(1280.0, 720.0);
        let rect = |x0: f32, y0: f32, x1: f32, y1: f32| Rect::from_min_max(pos2(x0, y0), pos2(x1, y1));
        assert_eq!(l.ppp, 1.0);
        assert_eq!(l.menu, rect(48.0, 168.0, 304.0, 656.0));
        assert_eq!(l.hero, rect(336.0, 168.0, 1232.0, 456.0));
        assert_eq!(l.tiles[0], rect(336.0, 472.0, 624.0, 656.0));
        assert_eq!(l.tiles[1], rect(640.0, 472.0, 928.0, 656.0));
        assert_eq!(l.tiles[2], rect(944.0, 472.0, 1232.0, 656.0));
        assert_eq!(l.entries[1], rect(48.0, 240.0, 304.0, 312.0));
        assert_eq!(l.exit, rect(48.0, 608.0, 304.0, 656.0));
        assert_eq!(l.footer, rect(48.0, 672.0, 1232.0, 704.0));
        assert_eq!(l.body, rect(360.0, 248.0, 896.0, 632.0));
        assert_eq!(l.body_wide, rect(360.0, 248.0, 1208.0, 632.0));
        assert_eq!(l.details, rect(920.0, 248.0, 1208.0, 632.0));
        for r in parts(&l).into_iter().map(|(_, r)| r).chain(l.entries) {
            assert!([r.left(), r.right(), r.top(), r.bottom()].iter().all(|v| v % 8.0 == 0.0), "{r:?}");
        }
    }
}
