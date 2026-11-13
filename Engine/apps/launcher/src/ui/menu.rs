use eframe::egui::{Color32, CursorIcon, Painter, Rect, Sense, Ui, pos2};

use super::layout::{CUT_TILE, Layout, PAD, TAB_TIP};
use super::shapes::{self, Cut};
use super::text::{self, Style};
use super::theme::{self, Face, size};
use super::{MenuItem, Page, icons};

const TITLE_REST: Color32 = Color32::from_rgb(0xA6, 0xE9, 0xF6);
const TITLE_BASE: f32 = 35.0;
const SUB_BASE: f32 = 55.0;

fn labels(page: Page, play: &'static str) -> (&'static str, &'static str) {
    match page {
        Page::Play => ("PLAY", play),
        Page::Host => ("HOST", "A SERVER FOR YOUR FRIENDS"),
        Page::Mods => ("MODS", "INSTALL, ORDER, SWITCH"),
        Page::Settings => ("SETTINGS", "GRAPHICS, CONTROLS, AUDIO"),
        Page::Extras => ("EXTRAS", "TOOLS, CREDITS, UPDATES"),
    }
}

#[derive(Clone, Copy)]
struct Look {
    selected: bool,
    hovered: bool,
}

fn entry(painter: &Painter, l: &Layout, rect: Rect, (title, sub): (&str, &str), look: Look) {
    let x = rect.left() + PAD;
    if look.selected {
        let points = shapes::tab(rect, TAB_TIP);
        shapes::glow(painter, &points, 12.0, theme::CYAN.gamma_multiply(if look.hovered { 0.42 } else { 0.30 }));
        let (x0, w) = (rect.left(), rect.width() + TAB_TIP);
        shapes::fill_with(painter, &points, 1.0 / l.ppp, |p| {
            theme::TAB[0].lerp_to_gamma(theme::TAB[1], ((p.x - x0) / w).clamp(0.0, 1.0))
        });
        shapes::outline(painter, &points, l.px(2.0), theme::CYAN);
        icons::chevrons(painter, pos2(rect.right() - 12.0, rect.center().y), 18.0, 3.0, theme::CYAN_SOFT);
    } else if look.hovered {
        painter.rect_filled(rect, 0.0, theme::TAB[0].gamma_multiply(0.55));
        let bar = Rect::from_min_max(rect.min, pos2(rect.left() + l.px(3.0), rect.bottom()));
        painter.rect_filled(bar, 0.0, theme::CYAN);
    }
    let bright = look.selected || look.hovered;
    let title_style = Style::new(Face::Bold, size::TITLE, if bright { theme::TEXT } else { TITLE_REST });
    let sub_colour = if look.selected { theme::CYAN_SOFT } else { theme::TEXT_DIM };
    let sub_style = Style::new(Face::Text, size::MENU_TEXT, sub_colour).spaced(0.6);
    text::draw(painter, pos2(x, rect.top() + TITLE_BASE), title, &title_style);
    text::draw(painter, pos2(x, rect.top() + SUB_BASE), sub, &sub_style);
}

pub fn show(ui: &mut Ui, l: &Layout, cursor: MenuItem, play: &'static str, forced: Option<&str>) -> Option<MenuItem> {
    let painter = ui.painter().clone();
    let column = shapes::chamfered(l.menu, Cut { bl: CUT_TILE, ..Default::default() });
    shapes::fill(&painter, &column, theme::PANEL);
    shapes::outline(&painter, &column, l.px(1.0), theme::LINE);
    let line = l.px(1.0);
    let rule = |y: f32| {
        let rect = Rect::from_min_max(pos2(l.menu.left() + line, y - line), pos2(l.menu.right() - line, y));
        painter.rect_filled(rect, 0.0, theme::LINE_FAINT);
    };
    let mut clicked = None;
    let mut sense = |ui: &mut Ui, item: MenuItem, rect: Rect| -> bool {
        let response =
            ui.interact(rect, ui.id().with(item.name()), Sense::CLICK).on_hover_cursor(CursorIcon::PointingHand);
        if response.clicked() {
            clicked = Some(item);
        }
        response.hovered() || forced == Some(item.name())
    };
    for (i, page) in Page::ALL.into_iter().enumerate() {
        let (item, rect) = (MenuItem::Page(page), l.entries[i]);
        let hovered = sense(ui, item, rect);
        if i > 0 {
            rule(rect.top());
        }
        entry(&painter, l, rect, labels(page, play), Look { selected: cursor == item, hovered });
    }
    rule(l.entries[l.entries.len() - 1].bottom());
    let (hovered, selected) = (sense(ui, MenuItem::Exit, l.exit), cursor == MenuItem::Exit);
    rule(l.exit.top());
    if hovered || selected {
        let points = shapes::chamfered(l.exit, Cut { bl: CUT_TILE, ..Default::default() });
        shapes::fill(&painter, &points, theme::TAB[0].gamma_multiply(0.55));
        if selected {
            shapes::outline(&painter, &points, l.px(2.0), theme::CYAN);
        }
    }
    let colour = if hovered || selected { theme::TEXT } else { theme::TEXT_DIM };
    let base = l.exit.center().y + size::TILE_TITLE * theme::CAP_HEIGHT / 2.0;
    text::draw(&painter, pos2(l.exit.left() + PAD, base), "EXIT", &Style::new(Face::Title, size::TILE_TITLE, colour));
    clicked
}
