use std::f32::consts::TAU;

use eframe::egui::epaint::PathStroke;
use eframe::egui::{Color32, Painter, Pos2, Rect, Shape, Stroke, Vec2, pos2, vec2};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Icon {
    Globe,
    Layers,
    Gear,
}

pub fn draw(painter: &Painter, icon: Icon, rect: Rect, color: Color32) {
    match icon {
        Icon::Globe => globe(painter, rect, color),
        Icon::Layers => layers(painter, rect, color),
        Icon::Gear => gear(painter, rect, color),
    }
}

fn polygon(painter: &Painter, points: &[Pos2], color: Color32) {
    painter.add(Shape::convex_polygon(points.to_vec(), color, PathStroke::NONE));
}

pub const MOUNTAIN: [&[(f32, f32)]; 3] = [
    &[(0.00, 1.00), (0.47, 0.00), (0.53, 0.44), (0.33, 1.00)],
    &[(0.54, 0.12), (1.00, 1.00), (0.72, 1.00), (0.585, 0.52)],
    &[(0.42, 1.00), (0.535, 0.64), (0.62, 1.00)],
];

pub fn mountain(painter: &Painter, rect: Rect, color: Color32) {
    for facet in MOUNTAIN {
        let points: Vec<Pos2> = facet.iter().map(|&(x, y)| rect.min + vec2(x, y) * rect.size()).collect();
        polygon(painter, &points, color);
    }
}

fn gear(painter: &Painter, rect: Rect, color: Color32) {
    let (c, r) = (rect.center(), rect.width() / 2.0);
    painter.add(Shape::circle_stroke(c, r * 0.50, Stroke::new(r * 0.36, color)));
    for i in 0..8 {
        let a = TAU * i as f32 / 8.0;
        let (out, side) = (Vec2::angled(a), Vec2::angled(a).rot90());
        let (inner, outer, wide, narrow) = (r * 0.62, r * 0.98, r * 0.21, r * 0.15);
        polygon(
            painter,
            &[
                c + out * inner - side * wide,
                c + out * outer - side * narrow,
                c + out * outer + side * narrow,
                c + out * inner + side * wide,
            ],
            color,
        );
    }
}

fn unit(rect: Rect) -> (impl Fn(f32, f32) -> Pos2, f32) {
    let s = rect.width() / 74.0;
    (move |x: f32, y: f32| rect.min + vec2(x, y) * s, 5.0 * s)
}

fn globe(painter: &Painter, rect: Rect, color: Color32) {
    let (at, weight) = unit(rect);
    let stroke = Stroke::new(weight, color);
    let c = at(37.0, 37.0);
    let r = at(67.0, 37.0).x - c.x;
    painter.add(Shape::circle_stroke(c, r, stroke));
    let (rx, ry) = (at(50.0, 0.0).x - c.x, r);
    let meridian: Vec<Pos2> = (0..48).map(|i| c + Vec2::angled(TAU * i as f32 / 48.0) * vec2(rx, ry)).collect();
    painter.add(Shape::closed_line(meridian, stroke));
    for y in [28.0, 46.0] {
        painter.add(Shape::line_segment([at(8.0, y), at(66.0, y)], stroke));
    }
}

fn layers(painter: &Painter, rect: Rect, color: Color32) {
    let (at, weight) = unit(rect);
    let stroke = Stroke::new(weight, color);
    painter.add(Shape::closed_line(vec![at(37.0, 8.0), at(66.0, 22.0), at(37.0, 36.0), at(8.0, 22.0)], stroke));
    for y in [37.0, 52.0] {
        painter.add(Shape::line(vec![at(8.0, y), at(37.0, y + 14.0), at(66.0, y)], stroke));
    }
}

pub fn chevrons(painter: &Painter, right: Pos2, height: f32, weight: f32, color: Color32) {
    let (half, depth) = (height / 2.0, height * 0.42);
    for i in 0..2 {
        let tip = pos2(right.x - i as f32 * (depth + weight * 1.1), right.y);
        let points = vec![pos2(tip.x - depth, tip.y - half), tip, pos2(tip.x - depth, tip.y + half)];
        painter.add(Shape::line(points, Stroke::new(weight, color)));
    }
}

pub fn dot(painter: &Painter, center: Pos2, radius: f32, color: Color32) {
    painter.add(Shape::circle_filled(center, radius * 2.1, color.gamma_multiply(0.18)));
    painter.add(Shape::circle_filled(center, radius, color));
}
