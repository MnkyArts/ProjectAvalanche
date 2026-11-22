use eframe::egui::{self, Color32, CursorIcon, Id, Key, PointerButton, Rect, Response, Sense, Ui, pos2, vec2};

use super::details::{self, Detail};
use super::shapes::{self, Cut};
use super::text::{self, Fit, Style};
use super::theme::{self, Face, size};
use super::{nav, widgets};

pub const ROW_H: f32 = 38.0;
pub const CONTROL_W: f32 = 250.0;
const CONTROL_H: f32 = 30.0;
const ARROW_W: f32 = 26.0;
const LABEL_X: f32 = 12.0;
const RIGHT_PAD: f32 = 10.0;
const NUMBER_W: f32 = 62.0;
const ITEM_H: f32 = 44.0;
const SWITCH: egui::Vec2 = vec2(30.0, 16.0);

#[derive(Clone, Copy, Debug)]
pub struct Row<'a> {
    pub label: &'a str,
    pub enabled: bool,
    pub original: Option<bool>,
}

impl<'a> Row<'a> {
    pub fn new(label: &'a str) -> Self {
        Row { label, enabled: true, original: None }
    }

    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    pub fn original(mut self, is: bool) -> Self {
        self.original = Some(is);
        self
    }
}

pub fn group(ui: &mut Ui, title: &str, note: Option<&str>) {
    if ui.cursor().top() > ui.min_rect().top() + 1.0 {
        ui.add_space(10.0);
    }
    let style = Style::new(Face::Title, size::CAPS, theme::CYAN_SOFT).spaced(1.6);
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 22.0), Sense::hover());
    let base = rect.top() + 6.0 + size::CAPS * theme::CAP_HEIGHT;
    let ink = text::draw(ui.painter(), pos2(rect.left(), base), &title.to_uppercase(), &style);
    let mut right = rect.right();
    if let Some(note) = note {
        let quiet = Style::new(Face::Text, size::CAPS, theme::TEXT_DIM);
        let room = (rect.right() - ink.right() - 40.0).max(0.0);
        let galley = text::layout(ui.painter(), note, &quiet, Fit::Cut(room));
        let w = text::width(&galley, &quiet);
        text::paint(ui.painter(), pos2(rect.right() - w, base), galley);
        right = rect.right() - w - 12.0;
    }
    let ppp = ui.pixels_per_point();
    let y = ((base - size::CAPS * theme::CAP_HEIGHT / 2.0) * ppp).round() / ppp;
    if right > ink.right() + 18.0 {
        let rule = Rect::from_min_max(pos2(ink.right() + 12.0, y), pos2(right, y + 1.0 / ppp));
        ui.painter().rect_filled(rule, 0.0, theme::LINE_FAINT);
    }
    ui.add_space(6.0);
}

fn frame(ui: &mut Ui, id: Id, enabled: bool, height: f32) -> (Rect, Response) {
    let (outer, _) = ui.allocate_exact_size(vec2(ui.available_width(), height + 2.0), Sense::hover());
    let rect = outer.shrink2(vec2(0.0, 1.0));
    let sense = if enabled { Sense::click() } else { Sense::hover() };
    let response = ui.interact(rect, id, sense);
    if enabled && response.clicked_by(PointerButton::Primary) && !response.has_focus() {
        response.request_focus();
    }
    (rect, response)
}

fn by_keys(response: &Response) -> bool {
    response.clicked() && !response.clicked_by(PointerButton::Primary)
}

fn snap(ui: &Ui, r: Rect) -> Rect {
    let ppp = ui.pixels_per_point();
    let s = |v: f32| (v * ppp).round() / ppp;
    Rect::from_min_max(pos2(s(r.left()), s(r.top())), pos2(s(r.right()), s(r.bottom())))
}

fn ground(ui: &Ui, rect: Rect, response: &Response, enabled: bool) {
    let rect = snap(ui, rect);
    let points = shapes::chamfered(rect, Cut::default());
    let painter = ui.painter();
    if response.has_focus() {
        shapes::glow(painter, &points, 10.0, theme::CYAN.gamma_multiply(0.35));
        shapes::fill(painter, &points, theme::W_HOVER);
        shapes::outline(painter, &points, 1.0 / ui.pixels_per_point(), theme::CYAN);
    } else if enabled && response.hovered() {
        shapes::fill(painter, &points, theme::W_HOVER.gamma_multiply(0.45));
    }
}

fn label(ui: &Ui, rect: Rect, row: &Row, focused: bool, room: f32) {
    let dim = if row.enabled { 1.0 } else { 0.42 };
    let colour = if focused { theme::TEXT } else { theme::TEXT_SOFT };
    let style = Style::new(Face::Text, size::TEXT, colour.gamma_multiply(dim));
    let base = rect.center().y + size::TEXT * theme::CAP_HEIGHT / 2.0;
    let mark = if row.original == Some(false) { 16.0 } else { 0.0 };
    let galley = text::layout(ui.painter(), row.label, &style, Fit::Cut((room - mark).max(8.0)));
    let ink = text::paint(ui.painter(), pos2(rect.left() + LABEL_X, base), galley);
    if mark > 0.0 {
        changed_mark(ui, pos2(ink.right() + 9.0, rect.center().y), dim);
    }
}

fn changed_mark(ui: &Ui, c: egui::Pos2, dim: f32) {
    let r = 4.95;
    let painter = ui.painter();
    painter.circle_filled(c, 6.0, theme::WARN.gamma_multiply(0.22 * dim));
    let points = vec![pos2(c.x, c.y - r), pos2(c.x + r, c.y), pos2(c.x, c.y + r), pos2(c.x - r, c.y)];
    painter.add(egui::Shape::convex_polygon(points, theme::WARN.gamma_multiply(dim), egui::Stroke::NONE));
}

fn control(rect: Rect, width: f32) -> Rect {
    let right = rect.right() - RIGHT_PAD;
    Rect::from_min_max(
        pos2(right - width, rect.center().y - CONTROL_H / 2.0),
        pos2(right, rect.center().y + CONTROL_H / 2.0),
    )
}

fn key_steps(ui: &Ui, id: Id, response: &Response) -> i32 {
    nav::takes_sideways(ui.ctx(), id);
    if response.has_focus() { nav::take_steps(ui.ctx()) } else { 0 }
}

pub fn choice(ui: &mut Ui, row: Row, value: &str, detail: impl FnOnce() -> Detail) -> i32 {
    stepper(ui, row, value, detail).0
}

fn stepper(ui: &mut Ui, row: Row, value: &str, detail: impl FnOnce() -> Detail) -> (i32, bool) {
    let id = ui.id().with(("row", row.label));
    let (rect, response) = frame(ui, id, row.enabled, ROW_H);
    let ctl = control(rect, CONTROL_W);
    let mut step = 0;
    let mut arrows = [false; 2];
    if row.enabled {
        step += key_steps(ui, id, &response);
        for (i, (part, dir)) in [
            (Rect::from_min_size(ctl.min, vec2(ARROW_W, CONTROL_H)), -1),
            (Rect::from_min_max(pos2(ctl.right() - ARROW_W, ctl.top()), ctl.max), 1),
        ]
        .into_iter()
        .enumerate()
        {
            let arrow = ui.interact(part, id.with(i), Sense::CLICK).on_hover_cursor(CursorIcon::PointingHand);
            arrows[i] = arrow.hovered();
            if arrow.clicked() {
                step += dir;
                response.request_focus();
            }
        }
        if by_keys(&response) {
            step += 1;
        }
    }
    ground(ui, rect, &response, row.enabled);
    let focused = response.has_focus();
    label(ui, rect, &row, focused, ctl.left() - rect.left() - LABEL_X - 12.0);
    let dim = if row.enabled { 1.0 } else { 0.42 };
    let arrow_style = Style::new(Face::Bold, 20.0, theme::CYAN);
    let base = ctl.center().y + 20.0 * theme::CAP_HEIGHT / 2.0;
    for (i, glyph) in ["‹", "›"].into_iter().enumerate() {
        let lit = if focused || arrows[i] { 1.0 } else { 0.7 };
        let style = arrow_style.color(theme::CYAN.gamma_multiply(lit * dim));
        let galley = text::layout(ui.painter(), glyph, &style, Fit::Free);
        let x = if i == 0 { ctl.left() } else { ctl.right() - ARROW_W };
        let w = text::width(&galley, &style);
        text::paint(ui.painter(), pos2(x + (ARROW_W - w) / 2.0, base), galley);
    }
    let value_style = Style::new(Face::Title, size::WIDGET, theme::TEXT.gamma_multiply(dim));
    let room = ctl.width() - 2.0 * ARROW_W - 4.0;
    let galley = text::layout(ui.painter(), value, &value_style, Fit::Cut(room));
    let w = text::width(&galley, &value_style);
    let vbase = ctl.center().y + size::WIDGET * theme::CAP_HEIGHT / 2.0;
    text::paint(ui.painter(), pos2(ctl.center().x - w / 2.0, vbase), galley);
    details::offer(ui, &response, detail);
    (step, row.enabled && response.clicked_by(PointerButton::Primary) && !arrows.iter().any(|a| *a))
}

pub fn toggle(ui: &mut Ui, row: Row, on: &mut bool, detail: impl FnOnce() -> Detail) -> bool {
    let (step, clicked) = stepper(ui, row, if *on { "On" } else { "Off" }, detail);
    if step % 2 != 0 || (clicked && step == 0) {
        *on = !*on;
        return true;
    }
    false
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Slide {
    Still,
    Step(i32),
    To(f32),
}

pub fn slider(ui: &mut Ui, row: Row, fraction: f32, shown: &str, detail: impl FnOnce() -> Detail) -> Slide {
    let id = ui.id().with(("row", row.label));
    let (rect, response) = frame(ui, id, row.enabled, ROW_H);
    let ctl = control(rect, CONTROL_W);
    let track = Rect::from_min_max(
        pos2(ctl.left(), ctl.center().y - 3.0),
        pos2(ctl.right() - NUMBER_W - 10.0, ctl.center().y + 3.0),
    );
    let mut slide = Slide::Still;
    if row.enabled {
        let steps = key_steps(ui, id, &response);
        if steps != 0 {
            slide = Slide::Step(steps);
        }
        let hit = Rect::from_min_max(pos2(track.left(), ctl.top()), pos2(track.right(), ctl.bottom()));
        let drag =
            ui.interact(hit, id.with("track"), Sense::CLICK | Sense::DRAG).on_hover_cursor(CursorIcon::PointingHand);
        if (drag.clicked() || drag.dragged())
            && let Some(p) = drag.interact_pointer_pos()
        {
            slide = Slide::To(((p.x - track.left()) / track.width()).clamp(0.0, 1.0));
            response.request_focus();
        }
    }
    ground(ui, rect, &response, row.enabled);
    label(ui, rect, &row, response.has_focus(), ctl.left() - rect.left() - LABEL_X - 12.0);
    let dim = if row.enabled { 1.0 } else { 0.42 };
    let track = snap(ui, track);
    let ppp = ui.pixels_per_point();
    ui.painter().rect_filled(track, 0.0, theme::W_REST.gamma_multiply(dim));
    let done =
        Rect::from_min_max(track.min, pos2(track.left() + track.width() * fraction.clamp(0.0, 1.0), track.bottom()));
    ui.painter().rect_filled(snap(ui, done), 0.0, theme::CYAN.gamma_multiply(dim));
    let border = shapes::chamfered(track, Cut::default());
    shapes::outline(ui.painter(), &border, 1.0 / ppp, theme::LINE_FAINT);
    let style = Style::new(Face::Title, size::WIDGET, theme::TEXT.gamma_multiply(dim));
    let base = ctl.center().y + size::WIDGET * theme::CAP_HEIGHT / 2.0;
    text::draw_right(ui.painter(), pos2(ctl.right(), base), shown, &style);
    details::offer(ui, &response, detail);
    slide
}

#[derive(Clone, Copy, Debug)]
pub struct Field<'a> {
    pub chars: usize,
    pub password: bool,
    pub hint: &'a str,
}

pub fn text_field(
    ui: &mut Ui,
    row: Row,
    value: &mut String,
    field: Field,
    detail: impl FnOnce() -> Detail,
) -> Response {
    let id = ui.id().with(("row", row.label));
    let edit_id = id.with("field");
    let (rect, response) = frame(ui, id, row.enabled, ROW_H);
    let ctl = control(rect, CONTROL_W);
    if row.enabled && response.clicked() {
        ui.memory_mut(|m| m.request_focus(edit_id));
    }
    ground(ui, rect, &response, row.enabled);
    let editing = ui.memory(|m| m.has_focus(edit_id));
    label(ui, rect, &row, response.has_focus() || editing, ctl.left() - rect.left() - LABEL_X - 12.0);
    let boxed = snap(ui, ctl);
    let points = shapes::chamfered(boxed, Cut::default());
    shapes::fill(ui.painter(), &points, theme::W_REST);
    let line = if editing { theme::CYAN } else { theme::LINE };
    shapes::outline(ui.painter(), &points, 1.0 / ui.pixels_per_point(), line);
    let edit = egui::TextEdit::singleline(value)
        .id(edit_id)
        .frame(egui::Frame::NONE)
        .margin(vec2(8.0, 6.0))
        .font(Face::Text.id(size::WIDGET))
        .text_color(theme::TEXT)
        .char_limit(field.chars)
        .password(field.password)
        .hint_text(egui::RichText::new(field.hint).color(theme::TEXT_DIM));
    let mut field_ui = ui.new_child(egui::UiBuilder::new().max_rect(ctl));
    if !row.enabled {
        field_ui.disable();
    }
    let edited = field_ui.put(ctl, edit);
    nav::inner(ui.ctx(), edit_id);
    if edited.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter) || i.key_pressed(Key::Escape)) {
        response.request_focus();
    }
    let shown = if edited.has_focus() || edited.hovered() { &edited } else { &response };
    details::offer(ui, shown, detail);
    edited
}

pub fn number(
    ui: &mut Ui,
    row: Row,
    value: &mut u16,
    range: std::ops::RangeInclusive<u16>,
    detail: impl FnOnce() -> Detail,
) -> bool {
    let id = ui.id().with(("row", row.label));
    let buffer_id = id.with("digits");
    let editing = ui.memory(|m| m.has_focus(id.with("field")));
    let mut digits = match editing {
        true => ui.data(|d| d.get_temp::<String>(buffer_id)).unwrap_or_else(|| value.to_string()),
        false => value.to_string(),
    };
    let row_id = id;
    let steps = if row.enabled {
        nav::takes_sideways(ui.ctx(), row_id);
        let focused = ui.memory(|m| m.has_focus(row_id));
        if focused { nav::take_steps(ui.ctx()) } else { 0 }
    } else {
        0
    };
    let field = Field { chars: 5, password: false, hint: "" };
    let edited = text_field(ui, row, &mut digits, field, detail);
    let mut next = *value;
    if edited.changed() {
        digits.retain(|c| c.is_ascii_digit());
        ui.data_mut(|d| d.insert_temp(buffer_id, digits.clone()));
        if let Ok(n) = digits.parse::<u32>() {
            next = n.clamp(u32::from(*range.start()), u32::from(*range.end())) as u16;
        }
    }
    if edited.lost_focus() {
        ui.data_mut(|d| d.remove::<String>(buffer_id));
    }
    if steps != 0 {
        let stepped = i64::from(next) + i64::from(steps);
        next = stepped.clamp(i64::from(*range.start()), i64::from(*range.end())) as u16;
    }
    let changed = next != *value;
    *value = next;
    changed
}

pub fn pick<T: PartialEq + Clone>(
    ui: &mut Ui,
    row: Row,
    value: &mut T,
    all: &[T],
    name: impl Fn(&T) -> String,
    detail: impl FnOnce() -> Detail,
) -> bool {
    let step = choice(ui, row, &name(value), detail);
    if step == 0 || all.is_empty() {
        return false;
    }
    let n = all.len() as i32;
    let next = match all.iter().position(|v| v == value) {
        Some(i) => (i as i32 + step).rem_euclid(n),
        None if step > 0 => (step - 1).rem_euclid(n),
        None => (n + step).rem_euclid(n),
    };
    let new = all[next as usize].clone();
    let changed = new != *value;
    *value = new;
    changed
}

pub fn info(ui: &mut Ui, row: Row, value: &str, colour: Color32, detail: impl FnOnce() -> Detail) -> Response {
    let id = ui.id().with(("row", row.label));
    let (rect, response) = frame(ui, id, row.enabled, ROW_H);
    let ctl = control(rect, CONTROL_W);
    ground(ui, rect, &response, row.enabled);
    label(ui, rect, &row, response.has_focus(), ctl.left() - rect.left() - LABEL_X - 12.0);
    let style = Style::new(Face::Title, size::WIDGET, colour);
    let galley = text::layout(ui.painter(), &widgets::shown(value), &style, Fit::Cut(ctl.width()));
    let w = text::width(&galley, &style);
    text::paint(ui.painter(), pos2(ctl.right() - w, ctl.center().y + size::WIDGET * theme::CAP_HEIGHT / 2.0), galley);
    details::offer(ui, &response, detail);
    response
}

pub fn state(ui: &mut Ui, row: Row, word: &str, colour: Color32, detail: impl FnOnce() -> Detail) -> Response {
    let id = ui.id().with(("row", row.label));
    let (rect, response) = frame(ui, id, row.enabled, ROW_H);
    ground(ui, rect, &response, row.enabled);
    let ctl = control(rect, CONTROL_W);
    let shown = chip(ui, ctl.right(), rect.center().y, &word.to_uppercase(), colour);
    label(ui, rect, &row, response.has_focus(), shown.left() - rect.left() - LABEL_X - 12.0);
    details::offer(ui, &response, detail);
    response
}

pub fn chip(ui: &Ui, right: f32, centre_y: f32, word: &str, colour: Color32) -> Rect {
    let style = Style::new(Face::Bold, size::CHIP, colour).spaced(1.3);
    let galley = text::layout(ui.painter(), word, &style, Fit::Free);
    let w = text::width(&galley, &style) + 14.0;
    let rect = snap(ui, Rect::from_min_max(pos2(right - w, centre_y - 9.0), pos2(right, centre_y + 9.0)));
    let points = shapes::chamfered(rect, Cut::default());
    shapes::outline(ui.painter(), &points, 1.0 / ui.pixels_per_point(), colour.gamma_multiply(0.5));
    text::paint(ui.painter(), pos2(rect.left() + 7.0, centre_y + size::CHIP * theme::CAP_HEIGHT / 2.0), galley);
    rect
}

pub fn switch(ui: &Ui, rect: Rect, on: bool, enabled: bool) {
    let rect = snap(ui, rect);
    let dim = if enabled { 1.0 } else { 0.42 };
    let points = shapes::chamfered(rect, Cut::default());
    let (fill, line, knob) = match on {
        true => (theme::GOOD.gamma_multiply(0.15), theme::GOOD, theme::GOOD),
        false => (theme::W_REST, theme::LINE, theme::TEXT_DIM),
    };
    shapes::fill(ui.painter(), &points, fill.gamma_multiply(dim));
    shapes::outline(ui.painter(), &points, 1.0 / ui.pixels_per_point(), line.gamma_multiply(dim));
    let x = if on { rect.right() - 12.0 } else { rect.left() + 2.0 };
    let knob_rect = snap(ui, Rect::from_min_size(pos2(x, rect.top() + 3.0), vec2(10.0, 10.0)));
    ui.painter().rect_filled(knob_rect, 0.0, knob.gamma_multiply(dim));
}

#[derive(Clone, Copy, Debug)]
pub struct Item<'a> {
    pub switch: Option<bool>,
    pub name: &'a str,
    pub meta: &'a str,
    pub chip: Option<(&'a str, Color32)>,
    pub enabled: bool,
    pub selected: bool,
}

pub struct ItemResponse {
    pub response: Response,
    pub toggle: bool,
}

pub fn item(ui: &mut Ui, id: Id, item: Item, detail: impl FnOnce() -> Detail) -> ItemResponse {
    let (rect, response) = frame(ui, id, true, ITEM_H);
    let switch_rect = Rect::from_min_size(pos2(rect.left() + 10.0, rect.center().y - SWITCH.y / 2.0), SWITCH);
    let mut toggle = false;
    if let Some(on) = item.switch {
        let hit = ui.interact(
            switch_rect.expand(4.0),
            id.with("switch"),
            if item.enabled { Sense::CLICK } else { Sense::hover() },
        );
        if item.enabled {
            if hit.clicked() {
                toggle = true;
                response.request_focus();
            }
            if by_keys(&response) {
                toggle = true;
            }
        }
        ground(ui, rect, &response, true);
        switch(ui, switch_rect, on, item.enabled);
    } else {
        ground(ui, rect, &response, true);
    }
    if item.selected && !response.has_focus() {
        let bar = snap(ui, Rect::from_min_max(rect.min, pos2(rect.left() + 3.0, rect.bottom())));
        ui.painter().rect_filled(bar, 0.0, theme::CYAN);
    }
    let left = rect.left() + 10.0 + 34.0 + 10.0;
    let mut right = rect.right() - 10.0;
    if let Some((word, colour)) = item.chip {
        right = chip(ui, right, rect.center().y, word, colour).left() - 10.0;
    }
    let room = (right - left).max(8.0);
    let name = Style::new(Face::Title, size::TEXT, theme::TEXT);
    let meta = Style::new(Face::Text, size::CAPS, theme::TEXT_DIM).spaced(0.2);
    let has_meta = !item.meta.is_empty();
    let name_base = if has_meta { rect.top() + 19.0 } else { rect.center().y + size::TEXT * theme::CAP_HEIGHT / 2.0 };
    let galley = text::layout(ui.painter(), &widgets::shown(item.name), &name, Fit::Cut(room));
    text::paint(ui.painter(), pos2(left, name_base), galley);
    if has_meta {
        let galley = text::layout(ui.painter(), &widgets::shown(item.meta), &meta, Fit::Cut(room));
        text::paint(ui.painter(), pos2(left, rect.top() + 36.0), galley);
    }
    details::offer(ui, &response, detail);
    ItemResponse { response, toggle }
}

pub fn buttons<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    ui.add_space(8.0);
    let inner = ui
        .horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(10.0, 10.0);
            add(ui)
        })
        .inner;
    ui.add_space(6.0);
    inner
}

pub fn step_card<R>(ui: &mut Ui, n: usize, done: bool, title: &str, add: impl FnOnce(&mut Ui) -> R) -> R {
    let frame = egui::Frame::new()
        .fill(theme::PANEL_DEEP.gamma_multiply(0.6))
        .stroke(egui::Stroke::new(1.0, theme::LINE_FAINT))
        .inner_margin(egui::Margin { left: 62, right: 14, top: 12, bottom: 12 });
    let shown = frame.show(ui, |ui| {
        ui.set_width(ui.available_width());
        let style = Style::new(Face::Bold, 17.0, theme::TEXT);
        let (head, _) = ui.allocate_exact_size(vec2(ui.available_width(), 24.0), Sense::hover());
        text::draw(ui.painter(), pos2(head.left(), head.top() + 17.0), title, &style);
        add(ui)
    });
    let r = shown.response.rect;
    let square = snap(ui, Rect::from_min_size(pos2(r.left() + 14.0, r.top() + 12.0), vec2(30.0, 30.0)));
    let colour = if done { theme::GOOD } else { theme::CYAN };
    let points = shapes::chamfered(square, Cut::default());
    shapes::outline(ui.painter(), &points, 1.0 / ui.pixels_per_point(), colour);
    let c = square.center();
    if done {
        let tick = vec![pos2(c.x - 7.0, c.y), pos2(c.x - 2.0, c.y + 5.0), pos2(c.x + 7.0, c.y - 5.0)];
        ui.painter().add(egui::Shape::line(tick, egui::Stroke::new(2.4, colour)));
    } else {
        let style = Style::new(Face::Bold, size::TEXT, colour);
        let galley = text::layout(ui.painter(), &n.to_string(), &style, Fit::Free);
        let w = text::width(&galley, &style);
        text::paint(ui.painter(), pos2(c.x - w / 2.0, c.y + size::TEXT * theme::CAP_HEIGHT / 2.0), galley);
    }
    ui.add_space(10.0);
    shown.inner
}
