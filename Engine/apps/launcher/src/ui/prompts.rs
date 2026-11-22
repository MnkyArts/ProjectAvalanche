use eframe::egui::{Color32, CornerRadius, Painter, Pos2, Rect, Shape, Stroke, StrokeKind, pos2, vec2};

use super::nav::{Device, PadStyle};
use super::text::{self, Fit, Style};
use super::theme::{self, Face, size};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Glyph {
    Confirm,
    Back,
    PrevSection,
    NextSection,
    Move,
    Change,
}

const XBOX_A: Color32 = Color32::from_rgb(0x5E, 0xD3, 0x6A);
const XBOX_B: Color32 = Color32::from_rgb(0xFF, 0x6B, 0x5E);
const PS_CROSS: Color32 = Color32::from_rgb(0x8C, 0xB4, 0xFF);
const PS_CIRCLE: Color32 = Color32::from_rgb(0xFF, 0x7A, 0x8A);

const FACE: f32 = 22.0;
const SHOULDER: (f32, f32) = (30.0, 20.0);
const KEY_H: f32 = 22.0;

fn letters(painter: &Painter, word: &str, colour: Color32) -> (std::sync::Arc<eframe::egui::Galley>, f32) {
    let style = Style::new(Face::Bold, size::CAPS, colour);
    let galley = text::layout(painter, word, &style, Fit::Free);
    let w = text::width(&galley, &style);
    (galley, w)
}

fn centred(painter: &Painter, c: Pos2, word: &str, colour: Color32) {
    let (galley, w) = letters(painter, word, colour);
    text::paint(painter, pos2(c.x - w / 2.0, c.y + size::CAPS * theme::CAP_HEIGHT / 2.0), galley);
}

fn key_word(glyph: Glyph) -> Option<&'static str> {
    match glyph {
        Glyph::Confirm => Some("Enter"),
        Glyph::Back => Some("Esc"),
        Glyph::PrevSection => Some("Q"),
        Glyph::NextSection => Some("E"),
        Glyph::Move | Glyph::Change => None,
    }
}

pub fn width(painter: &Painter, pad: Option<PadStyle>, glyph: Glyph) -> f32 {
    match (pad, glyph) {
        (Some(_), Glyph::Confirm | Glyph::Back) => FACE,
        (Some(_), Glyph::PrevSection | Glyph::NextSection) => SHOULDER.0,
        (Some(_), Glyph::Move) => 18.0,
        (Some(_), Glyph::Change) => 22.0,
        (None, g) => match key_word(g) {
            Some(word) => (letters(painter, word, theme::TEXT).1 + 12.0).max(24.0),
            None => 30.0,
        },
    }
}

fn arrow(painter: &Painter, c: Pos2, dir: (f32, f32), r: f32, colour: Color32) {
    let (dx, dy) = dir;
    let (nx, ny) = (-dy, dx);
    let points = vec![
        pos2(c.x + dx * r, c.y + dy * r),
        pos2(c.x - dx * r * 0.6 + nx * r * 0.85, c.y - dy * r * 0.6 + ny * r * 0.85),
        pos2(c.x - dx * r * 0.6 - nx * r * 0.85, c.y - dy * r * 0.6 - ny * r * 0.85),
    ];
    painter.add(Shape::convex_polygon(points, colour, Stroke::NONE));
}

pub fn draw(painter: &Painter, pad: Option<PadStyle>, glyph: Glyph, left: f32, centre_y: f32) -> f32 {
    let w = width(painter, pad, glyph);
    let c = pos2(left + w / 2.0, centre_y);
    match (pad, glyph) {
        (Some(style), Glyph::Confirm | Glyph::Back) => {
            let colour = match (style, glyph) {
                (PadStyle::Xbox, Glyph::Confirm) => XBOX_A,
                (PadStyle::Xbox, _) => XBOX_B,
                (PadStyle::PlayStation, Glyph::Confirm) => PS_CROSS,
                (PadStyle::PlayStation, _) => PS_CIRCLE,
            };
            painter.circle(c, FACE / 2.0 - 0.75, Color32::from_black_alpha(90), Stroke::new(1.5, colour));
            match (style, glyph) {
                (PadStyle::Xbox, Glyph::Confirm) => centred(painter, c, "A", colour),
                (PadStyle::Xbox, _) => centred(painter, c, "B", colour),
                (PadStyle::PlayStation, Glyph::Confirm) => {
                    let r = 4.5;
                    let stroke = Stroke::new(1.8, colour);
                    painter.line_segment([pos2(c.x - r, c.y - r), pos2(c.x + r, c.y + r)], stroke);
                    painter.line_segment([pos2(c.x - r, c.y + r), pos2(c.x + r, c.y - r)], stroke);
                }
                (PadStyle::PlayStation, _) => {
                    painter.circle_stroke(c, 5.0, Stroke::new(1.8, colour));
                }
            }
        }
        (Some(style), Glyph::PrevSection | Glyph::NextSection) => {
            let rect = Rect::from_center_size(c, vec2(SHOULDER.0, SHOULDER.1));
            let round = CornerRadius { nw: 9, ne: 9, sw: 4, se: 4 };
            painter.rect_stroke(rect, round, Stroke::new(1.5, theme::TEXT_SOFT), StrokeKind::Inside);
            let word = match (style, glyph) {
                (PadStyle::Xbox, Glyph::PrevSection) => "LB",
                (PadStyle::Xbox, _) => "RB",
                (PadStyle::PlayStation, Glyph::PrevSection) => "L1",
                (PadStyle::PlayStation, _) => "R1",
            };
            centred(painter, c, word, theme::TEXT);
        }
        (Some(_), Glyph::Move) => {
            let (long, thick) = (16.0, 5.5);
            painter.rect_filled(Rect::from_center_size(c, vec2(long, thick)), 1.0, theme::TEXT);
            painter.rect_filled(Rect::from_center_size(c, vec2(thick, long)), 1.0, theme::TEXT);
        }
        (Some(_), Glyph::Change) => {
            arrow(painter, pos2(c.x - 5.5, c.y), (-1.0, 0.0), 5.0, theme::TEXT);
            arrow(painter, pos2(c.x + 5.5, c.y), (1.0, 0.0), 5.0, theme::TEXT);
        }
        (None, _) => {
            let rect = Rect::from_center_size(c, vec2(w, KEY_H));
            painter.rect_filled(rect, 0.0, theme::W_REST);
            painter.rect_stroke(rect, 0.0, Stroke::new(1.0, theme::LINE), StrokeKind::Inside);
            let floor = Rect::from_min_max(pos2(rect.left(), rect.bottom() - 2.0), rect.max);
            painter.rect_filled(floor, 0.0, theme::LINE);
            let c = pos2(c.x, c.y - 0.5);
            match key_word(glyph) {
                Some(word) => {
                    let style = Style::new(Face::Title, size::CAPS, theme::TEXT).spaced(0.5);
                    let galley = text::layout(painter, word, &style, Fit::Free);
                    let tw = text::width(&galley, &style);
                    text::paint(painter, pos2(c.x - tw / 2.0, c.y + size::CAPS * theme::CAP_HEIGHT / 2.0), galley);
                }
                None if glyph == Glyph::Move => {
                    arrow(painter, pos2(c.x - 5.0, c.y), (0.0, -1.0), 4.5, theme::TEXT);
                    arrow(painter, pos2(c.x + 5.0, c.y), (0.0, 1.0), 4.5, theme::TEXT);
                }
                None => {
                    arrow(painter, pos2(c.x - 5.0, c.y), (-1.0, 0.0), 4.5, theme::TEXT);
                    arrow(painter, pos2(c.x + 5.0, c.y), (1.0, 0.0), 4.5, theme::TEXT);
                }
            }
        }
    }
    left + w
}

pub type Prompt = (&'static [Glyph], &'static str);

pub fn shown(device: Device) -> Option<Option<PadStyle>> {
    match device {
        Device::Mouse => None,
        Device::Keys => Some(None),
        Device::Pad(style) => Some(Some(style)),
    }
}

const GAP_GLYPHS: f32 = 3.0;
const GAP_WORD: f32 = 6.0;
const GAP_PROMPTS: f32 = 16.0;

fn word_style() -> Style {
    Style::new(Face::Title, size::CAPS, theme::TEXT_SOFT).spaced(1.3)
}

pub fn row_width(painter: &Painter, pad: Option<PadStyle>, prompts: &[Prompt]) -> f32 {
    let mut w = 0.0;
    for (i, (glyphs, word)) in prompts.iter().enumerate() {
        if i > 0 {
            w += GAP_PROMPTS;
        }
        for (j, g) in glyphs.iter().enumerate() {
            w += width(painter, pad, *g) + if j > 0 { GAP_GLYPHS } else { 0.0 };
        }
        let galley = text::layout(painter, word, &word_style(), Fit::Free);
        w += GAP_WORD + text::width(&galley, &word_style());
    }
    w
}

pub fn row(painter: &Painter, pad: Option<PadStyle>, prompts: &[Prompt], left: f32, centre_y: f32) {
    let mut x = left;
    for (i, (glyphs, word)) in prompts.iter().enumerate() {
        if i > 0 {
            x += GAP_PROMPTS;
        }
        for (j, g) in glyphs.iter().enumerate() {
            if j > 0 {
                x += GAP_GLYPHS;
            }
            x = draw(painter, pad, *g, x, centre_y);
        }
        x += GAP_WORD;
        let galley = text::layout(painter, word, &word_style(), Fit::Free);
        let w = text::width(&galley, &word_style());
        text::paint(painter, pos2(x, centre_y + size::CAPS * theme::CAP_HEIGHT / 2.0), galley);
        x += w;
    }
}
