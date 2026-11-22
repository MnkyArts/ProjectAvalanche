use eframe::egui::{Color32, Painter, pos2};

use super::layout::Layout;
use super::nav::Device;
use super::text::{self, Style};
use super::theme::{self, Face, size};
use super::{icons, prompts};

pub fn version() -> String {
    let version = env!("CARGO_PKG_VERSION");
    match option_env!("GITHUB_SHA").filter(|s| s.len() >= 7 && s.is_ascii()) {
        Some(sha) => format!("v{version} · {}", &sha[..7]),
        None => format!("v{version}"),
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Status {
    pub colour: Color32,
    pub label: String,
}

pub fn draw(painter: &Painter, l: &Layout, status: &Status, device: Device, shown: &[prompts::Prompt]) {
    let base = l.footer.center().y + size::SMALL * theme::CAP_HEIGHT / 2.0;
    let left = l.footer.left();
    let name = Style::new(Face::Title, size::SMALL, theme::TEXT_SOFT).spaced(0.6);
    let rest = Style::new(Face::Text, size::SMALL, theme::TEXT_DIM);
    let name_rect = text::draw(painter, pos2(left, base), "PROJECT AVALANCHE", &name);
    text::draw(painter, pos2(name_rect.right() + 10.0, base), "Native PC reimplementation of SSX (2012)", &rest);

    let label = Style::new(Face::Text, size::SMALL, theme::TEXT_SOFT);
    let label_rect = text::draw_right(painter, pos2(l.footer.right(), base), &status.label, &label);
    let dot = pos2(label_rect.left() - 12.0, base - size::SMALL * theme::CAP_HEIGHT / 2.0);
    icons::dot(painter, dot, 4.0, status.colour);
    let version = text::draw_right(painter, pos2(dot.x - 24.0, base), &version(), &rest);
    if let Some(pad) = prompts::shown(device) {
        let width = prompts::row_width(painter, pad, shown);
        prompts::row(painter, pad, shown, version.left() - 28.0 - width, l.footer.center().y);
    }
}
