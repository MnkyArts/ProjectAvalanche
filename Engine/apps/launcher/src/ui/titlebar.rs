use eframe::egui::{
    self, CursorIcon, Painter, Pos2, Rect, ResizeDirection, Sense, Shape, Stroke, Ui, ViewportCommand, pos2, vec2,
};

use super::layout::{CUT_TILE, Layout};
use super::shapes::{self, Cut};
use super::theme;

pub const BUTTON_W: f32 = 46.0;
pub const BUTTON_H: f32 = 32.0;
pub const EDGE_PX: f32 = 6.0;
pub const CORNER_PX: f32 = 16.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Button {
    Minimize,
    Maximize,
    Close,
}

impl Button {
    const ALL: [Button; 3] = [Button::Minimize, Button::Maximize, Button::Close];

    pub fn name(self) -> &'static str {
        match self {
            Button::Minimize => "window_minimize",
            Button::Maximize => "window_maximize",
            Button::Close => "window_close",
        }
    }
}

pub fn buttons(l: &Layout) -> [(Button, Rect); 3] {
    let right = l.snap(l.screen.right());
    let top = l.snap(l.screen.top());
    let edge = |n: f32| if n == 0.0 { right } else { l.snap(right - BUTTON_W * n) };
    Button::ALL.map(|b| {
        let i = Button::ALL.iter().position(|x| *x == b).unwrap_or(0) as f32;
        (b, Rect::from_min_max(pos2(edge(3.0 - i), top), pos2(edge(2.0 - i), l.snap(top + BUTTON_H))))
    })
}

pub fn drag_area(l: &Layout) -> Rect {
    Rect::from_min_max(l.screen.min, pos2(l.screen.right(), l.header.bottom()))
}

pub fn zones(screen: Rect, ppp: f32) -> Vec<(ResizeDirection, u8, Rect)> {
    let (edge, corner) = (EDGE_PX / ppp, CORNER_PX / ppp);
    let (x0, x1, y0, y1) = (screen.left(), screen.right(), screen.top(), screen.bottom());
    let r = |a: Pos2, b: Pos2| Rect::from_two_pos(a, b);
    let mut out = vec![
        (ResizeDirection::North, 0, r(pos2(x0 + corner, y0), pos2(x1 - corner, y0 + edge))),
        (ResizeDirection::South, 0, r(pos2(x0 + corner, y1 - edge), pos2(x1 - corner, y1))),
        (ResizeDirection::West, 0, r(pos2(x0, y0 + corner), pos2(x0 + edge, y1 - corner))),
        (ResizeDirection::East, 0, r(pos2(x1 - edge, y0 + corner), pos2(x1, y1 - corner))),
    ];
    for (direction, at, inward) in [
        (ResizeDirection::NorthWest, screen.left_top(), vec2(1.0, 1.0)),
        (ResizeDirection::NorthEast, screen.right_top(), vec2(-1.0, 1.0)),
        (ResizeDirection::SouthWest, screen.left_bottom(), vec2(1.0, -1.0)),
        (ResizeDirection::SouthEast, screen.right_bottom(), vec2(-1.0, -1.0)),
    ] {
        out.push((direction, 0, r(at, at + vec2(corner * inward.x, edge * inward.y))));
        out.push((direction, 1, r(at, at + vec2(edge * inward.x, corner * inward.y))));
    }
    out
}

fn cursor(direction: ResizeDirection) -> CursorIcon {
    match direction {
        ResizeDirection::North => CursorIcon::ResizeNorth,
        ResizeDirection::South => CursorIcon::ResizeSouth,
        ResizeDirection::East => CursorIcon::ResizeEast,
        ResizeDirection::West => CursorIcon::ResizeWest,
        ResizeDirection::NorthEast => CursorIcon::ResizeNorthEast,
        ResizeDirection::SouthEast => CursorIcon::ResizeSouthEast,
        ResizeDirection::NorthWest => CursorIcon::ResizeNorthWest,
        ResizeDirection::SouthWest => CursorIcon::ResizeSouthWest,
    }
}

pub fn show(ui: &mut Ui, l: &Layout, forced: Option<&str>) {
    let ctx = ui.ctx().clone();
    let (maximized, fullscreen) =
        ctx.input(|i| (i.viewport().maximized.unwrap_or(false), i.viewport().fullscreen.unwrap_or(false)));
    let drag = ui.interact(drag_area(l), ui.id().with("window drag"), Sense::CLICK | Sense::DRAG);
    if drag.double_clicked() && !fullscreen {
        ctx.send_viewport_cmd(ViewportCommand::Maximized(!maximized));
    } else if drag.drag_started_by(egui::PointerButton::Primary) && !fullscreen {
        ctx.send_viewport_cmd(ViewportCommand::StartDrag);
    }
    let painter = ui.painter().clone();
    let all = buttons(l);
    let strip = Rect::from_min_max(all[0].1.min, all[2].1.max);
    let points = shapes::chamfered(strip, Cut { bl: CUT_TILE, ..Default::default() });
    shapes::fill(&painter, &points, theme::PANEL);
    for (button, rect) in all {
        let response =
            ui.interact(rect, ui.id().with(button.name()), Sense::CLICK).on_hover_cursor(CursorIcon::PointingHand);
        let hovered = response.hovered() || forced == Some(button.name());
        if hovered {
            let cut = if button == Button::Minimize { CUT_TILE } else { 0.0 };
            let part = shapes::chamfered(rect, Cut { bl: cut, ..Default::default() });
            let (fill, glow) = match button {
                Button::Close => (theme::ERROR.gamma_multiply(0.85), theme::ERROR.gamma_multiply(0.35)),
                _ => (theme::TAB[1], theme::CYAN.gamma_multiply(0.30)),
            };
            shapes::glow(&painter, &part, 8.0, glow);
            shapes::fill(&painter, &part, fill);
        }
        let colour = if hovered { theme::TEXT } else { theme::CYAN_SOFT };
        glyph(&painter, l, rect.center(), button, maximized, colour);
        if response.clicked() {
            ctx.send_viewport_cmd(match button {
                Button::Minimize => ViewportCommand::Minimized(true),
                Button::Maximize => ViewportCommand::Maximized(!maximized),
                Button::Close => ViewportCommand::Close,
            });
        }
    }
    for (_, rect) in &all[1..] {
        let x = rect.left();
        let rule = Rect::from_min_max(pos2(x, strip.top() + 8.0), pos2(x + l.px(1.0), strip.bottom() - 8.0));
        painter.rect_filled(rule, 0.0, theme::LINE_FAINT);
    }
    shapes::outline(&painter, &points, l.px(1.0), theme::LINE);
    if !maximized && !fullscreen {
        edges(ui, l);
    }
}

fn edges(ui: &mut Ui, l: &Layout) {
    for (direction, arm, rect) in zones(l.screen, l.ppp) {
        let id = ui.id().with(("window resize", direction as u8, arm));
        let response = ui.interact(rect, id, Sense::DRAG).on_hover_cursor(cursor(direction));
        if response.drag_started_by(egui::PointerButton::Primary) {
            ui.ctx().send_viewport_cmd(ViewportCommand::BeginResize(direction));
        }
    }
}

fn glyph(painter: &Painter, l: &Layout, c: Pos2, button: Button, maximized: bool, colour: egui::Color32) {
    let stroke = Stroke::new(l.px(1.5).max(1.2), colour);
    let h = 5.0;
    match button {
        Button::Minimize => {
            painter.add(Shape::line_segment([pos2(c.x - h, c.y), pos2(c.x + h, c.y)], stroke));
        }
        Button::Maximize if maximized => {
            let back = Rect::from_center_size(c + vec2(1.5, -1.5), vec2(8.0, 8.0));
            let front = Rect::from_center_size(c + vec2(-1.5, 1.5), vec2(8.0, 8.0));
            let behind = vec![
                pos2(back.left(), front.top()),
                back.left_top(),
                back.right_top(),
                back.right_bottom(),
                pos2(front.right(), back.bottom()),
            ];
            painter.add(Shape::line(behind, stroke));
            painter.rect_stroke(front, 0.0, stroke, egui::StrokeKind::Middle);
        }
        Button::Maximize => {
            let square = Rect::from_center_size(c, vec2(2.0 * h, 2.0 * h));
            painter.rect_stroke(square, 0.0, stroke, egui::StrokeKind::Middle);
        }
        Button::Close => {
            painter.add(Shape::line_segment([pos2(c.x - h, c.y - h), pos2(c.x + h, c.y + h)], stroke));
            painter.add(Shape::line_segment([pos2(c.x - h, c.y + h), pos2(c.x + h, c.y - h)], stroke));
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Click {
    Minimize,
    Maximize,
    Close,
    Move,
    Resize,
}

impl Click {
    pub const NAMES: [(&'static str, Click); 5] = [
        ("minimize", Click::Minimize),
        ("maximize", Click::Maximize),
        ("close", Click::Close),
        ("move", Click::Move),
        ("resize", Click::Resize),
    ];

    pub fn parse(name: &str) -> Option<Click> {
        Self::NAMES.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)).map(|(_, c)| *c)
    }

    pub fn frames(self, l: &Layout) -> Vec<Vec<egui::Event>> {
        let all = buttons(l);
        let press = |p: Pos2, pressed: bool| egui::Event::PointerButton {
            pos: p,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        let click = |p: Pos2| vec![vec![egui::Event::PointerMoved(p), press(p, true)], vec![press(p, false)]];
        let drag = |p: Pos2, by: egui::Vec2| {
            let to = p + by;
            vec![
                vec![egui::Event::PointerMoved(p), press(p, true)],
                vec![egui::Event::PointerMoved(to)],
                vec![press(to, false)],
            ]
        };
        let edge = EDGE_PX / l.ppp / 2.0;
        match self {
            Click::Minimize => click(all[0].1.center()),
            Click::Maximize => click(all[1].1.center()),
            Click::Close => click(all[2].1.center()),
            Click::Move => drag(pos2(l.screen.center().x, l.screen.top() + BUTTON_H / 2.0), vec2(40.0, 30.0)),
            Click::Resize => drag(l.screen.right_bottom() - vec2(edge, edge), vec2(-40.0, -30.0)),
        }
    }
}

#[cfg(test)]
mod tests {
    use eframe::egui::{Rect, ResizeDirection, pos2, vec2};

    use super::*;

    #[test]
    fn the_edges_and_corners_resize_and_the_inside_does_not() {
        let screen = Rect::from_min_size(pos2(0.0, 0.0), vec2(1280.0, 720.0));
        let at_ppp = |ppp: f32, x: f32, y: f32| {
            zones(screen, ppp).into_iter().find(|(_, _, r)| r.contains(pos2(x, y))).map(|(d, _, _)| d)
        };
        let at = |x: f32, y: f32| at_ppp(1.0, x, y);
        assert_eq!(at(640.0, 360.0), None);
        assert_eq!(at(640.0, 2.0), Some(ResizeDirection::North));
        assert_eq!(at(640.0, 718.0), Some(ResizeDirection::South));
        assert_eq!(at(1.0, 360.0), Some(ResizeDirection::West));
        assert_eq!(at(1279.0, 360.0), Some(ResizeDirection::East));
        assert_eq!(at(2.0, 2.0), Some(ResizeDirection::NorthWest));
        assert_eq!(at(12.0, 2.0), Some(ResizeDirection::NorthWest));
        assert_eq!(at(1279.0, 12.0), Some(ResizeDirection::NorthEast));
        assert_eq!(at(2.0, 712.0), Some(ResizeDirection::SouthWest));
        assert_eq!(at(1272.0, 719.0), Some(ResizeDirection::SouthEast));
        assert_eq!(at(640.0, 8.0), None, "past the band");
        assert_eq!(at_ppp(2.0, 640.0, 2.0), Some(ResizeDirection::North));
        assert_eq!(at_ppp(2.0, 640.0, 4.0), None);
    }

    #[test]
    fn the_buttons_sit_in_the_window_corner_at_every_size() {
        for (w, h) in [(960.0, 540.0), (1280.0, 720.0), (1920.0, 1080.0), (1280.0, 1024.0), (2560.0, 1080.0)] {
            let ppp = crate::ui::layout::pixels_per_point(vec2(w, h));
            let screen = Rect::from_min_size(pos2(0.0, 0.0), vec2(w, h) / ppp);
            let l = Layout::new(screen, ppp);
            let all = buttons(&l);
            assert!((all[2].1.right() - screen.right()).abs() <= 0.5 / ppp, "{w} x {h}");
            assert!(all[0].1.right() == all[1].1.left() && all[1].1.right() == all[2].1.left(), "{w} x {h}: shared");
            assert!((all[0].1.top() - screen.top()).abs() <= 1.0 / ppp, "{w} x {h}");
            for (i, (_, rect)) in all.iter().enumerate() {
                assert!(!rect.intersects(l.menu) && !rect.intersects(l.content), "{w} x {h}: button {i}");
                assert!(!rect.intersects(l.header), "{w} x {h}: button {i} over the header");
            }
            let drag = drag_area(&l);
            assert!(drag.bottom() <= l.content.top() && drag.bottom() <= l.menu.top(), "{w} x {h}");
        }
    }

    fn run(frames: Vec<Vec<egui::Event>>) -> Vec<Vec<ViewportCommand>> {
        let ctx = egui::Context::default();
        let screen = Rect::from_min_size(pos2(0.0, 0.0), vec2(1280.0, 720.0));
        let mut out = Vec::new();
        for (i, events) in std::iter::once(Vec::new()).chain(frames).enumerate() {
            let input = egui::RawInput {
                screen_rect: Some(screen),
                time: Some(i as f64 / 10.0),
                events,
                ..egui::RawInput::default()
            };
            let output = ctx.run_ui(input, |ui| show(ui, &Layout::new(screen, 1.0), None));
            let commands = output.viewport_output.get(&egui::ViewportId::ROOT).map(|v| v.commands.clone());
            output.drop_without_applying_deltas();
            out.push(commands.unwrap_or_default());
        }
        out.remove(0);
        out
    }

    fn at(x: f32, y: f32) -> egui::Event {
        egui::Event::PointerMoved(pos2(x, y))
    }

    fn button(x: f32, y: f32, pressed: bool) -> egui::Event {
        egui::Event::PointerButton {
            pos: pos2(x, y),
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        }
    }

    fn click(x: f32, y: f32) -> Vec<Vec<egui::Event>> {
        vec![vec![at(x, y), button(x, y, true)], vec![button(x, y, false)]]
    }

    #[test]
    fn the_buttons_minimize_maximize_and_close() {
        let screen = Rect::from_min_size(pos2(0.0, 0.0), vec2(1280.0, 720.0));
        let all = buttons(&Layout::new(screen, 1.0));
        for (button, rect, wanted) in [
            (Button::Minimize, all[0].1, ViewportCommand::Minimized(true)),
            (Button::Maximize, all[1].1, ViewportCommand::Maximized(true)),
            (Button::Close, all[2].1, ViewportCommand::Close),
        ] {
            let c = rect.center();
            let sent = run(click(c.x, c.y)).concat();
            assert_eq!(sent, [wanted], "{button:?}");
        }
    }

    #[test]
    fn the_top_band_moves_the_window_and_a_double_click_maximizes_it() {
        let sent = run(vec![vec![at(700.0, 20.0), button(700.0, 20.0, true)], vec![at(740.0, 30.0)]]).concat();
        assert_eq!(sent, [ViewportCommand::StartDrag]);
        let sent = run(vec![vec![at(900.0, 100.0), button(900.0, 100.0, true)], vec![at(940.0, 110.0)]]).concat();
        assert_eq!(sent, [ViewportCommand::StartDrag]);
        let mut twice = click(700.0, 20.0);
        twice.extend(click(700.0, 20.0));
        assert_eq!(run(twice).concat(), [ViewportCommand::Maximized(true)]);
        assert!(
            run(vec![vec![at(700.0, 300.0), button(700.0, 300.0, true)], vec![at(740.0, 330.0)]]).concat().is_empty()
        );
    }

    #[test]
    fn the_edges_and_corners_start_the_systems_resize() {
        for (x, y, direction) in [
            (640.0, 1.0, ResizeDirection::North),
            (1279.0, 400.0, ResizeDirection::East),
            (1.0, 400.0, ResizeDirection::West),
            (640.0, 719.0, ResizeDirection::South),
            (1279.0, 719.0, ResizeDirection::SouthEast),
            (1.0, 1.0, ResizeDirection::NorthWest),
        ] {
            let sent = run(vec![vec![at(x, y), button(x, y, true)], vec![at(x - 10.0, y - 10.0)]]).concat();
            assert_eq!(sent, [ViewportCommand::BeginResize(direction)], "{x}, {y}");
        }
    }

    #[test]
    fn the_click_names() {
        for (name, click) in Click::NAMES {
            assert_eq!(Click::parse(name), Some(click));
        }
        assert_eq!(Click::parse("drag"), None);
    }
}
