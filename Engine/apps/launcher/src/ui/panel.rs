use eframe::egui::{self, Align, CursorIcon, Id, Painter, Rect, Sense, Ui, UiBuilder, pos2};

use super::Section;
use super::layout::{CUT_HERO, Layout, PAD, PANEL_HEAD};
use super::nav::Device;
use super::prompts::{self, Glyph};
use super::shapes::{self, Cut};
use super::text::{self, Fit, Style};
use super::theme::{self, Face, size};

const TAB_PAD: f32 = 14.0;
const SHOULDER_PAD: f32 = 4.0;

pub fn frame(painter: &Painter, l: &Layout, title: &str) {
    let rect = l.content;
    let points = shapes::chamfered(rect, Cut::diagonal(CUT_HERO));
    shapes::glow(painter, &points, 14.0, theme::CYAN.gamma_multiply(0.16));
    shapes::fill(painter, &points, theme::PANEL_DEEP);
    shapes::outline(painter, &points, l.px(1.0), theme::CYAN.gamma_multiply(0.70));
    let base = rect.top() + PANEL_HEAD / 2.0 + size::TITLE * theme::CAP_HEIGHT / 2.0;
    text::draw(painter, pos2(rect.left() + PAD, base), title, &Style::new(Face::Bold, size::TITLE, theme::TEXT));
    let (rule, line) = (l.snap(rect.top() + PANEL_HEAD), l.px(1.0));
    let across = Rect::from_min_max(pos2(rect.left() + PAD, rule - line), pos2(rect.right() - PAD, rule));
    painter.rect_filled(across, 0.0, theme::LINE);
}

pub fn tabs<T: Section>(ui: &mut Ui, l: &Layout, current: &mut T, device: Device) -> bool {
    let painter = ui.painter().clone();
    let rect = l.content;
    let rule = l.snap(rect.top() + PANEL_HEAD);
    let base = rect.top() + PANEL_HEAD / 2.0 + size::CAPS * theme::CAP_HEIGHT / 2.0 + 2.0;
    let style = Style::new(Face::Title, size::CAPS, theme::TEXT_DIM).spaced(1.4);
    let shoulders = prompts::shown(device);
    let centre = rect.top() + PANEL_HEAD / 2.0 + 4.0;
    let mut right = rect.right() - PAD;
    if let Some(pad) = shoulders {
        let w = prompts::width(&painter, pad, Glyph::NextSection);
        prompts::draw(&painter, pad, Glyph::NextSection, right - SHOULDER_PAD - w, centre);
        right -= w + 2.0 * SHOULDER_PAD;
    }
    let mut changed = false;
    for (i, (tab, label)) in T::ALL.iter().enumerate().rev() {
        let galley = text::layout(&painter, label, &style, Fit::Free);
        let width = text::width(&galley, &style) + 2.0 * TAB_PAD;
        let cell = Rect::from_min_max(pos2(l.snap(right - width), rect.top() + 12.0), pos2(l.snap(right), rule));
        let response =
            ui.interact(cell, ui.id().with(("tab", i)), Sense::CLICK).on_hover_cursor(CursorIcon::PointingHand);
        if response.clicked() && *current != *tab {
            *current = *tab;
            changed = true;
        }
        let colour = match (*current == *tab, response.hovered()) {
            (true, _) => theme::CYAN,
            (false, true) => theme::TEXT,
            (false, false) => theme::TEXT_DIM,
        };
        text::draw(&painter, pos2(cell.left() + TAB_PAD, base), label, &style.color(colour));
        if *current == *tab {
            let under = Rect::from_min_max(pos2(cell.left(), rule - l.px(2.0)), pos2(cell.right(), rule));
            painter.rect_filled(under, 0.0, theme::CYAN);
        }
        right -= width;
    }
    if let Some(pad) = shoulders {
        let w = prompts::width(&painter, pad, Glyph::PrevSection);
        prompts::draw(&painter, pad, Glyph::PrevSection, right - SHOULDER_PAD - w, centre);
    }
    changed
}

pub fn body(ui: &mut Ui, rect: Rect, id: &str, add: impl FnOnce(&mut Ui)) {
    let builder = UiBuilder::new().max_rect(rect).layout(egui::Layout::top_down(Align::Min)).id_salt(id);
    ui.scope_builder(builder, |ui| {
        ui.set_clip_rect(rect.expand2(egui::vec2(4.0, 2.0)));
        egui::ScrollArea::vertical().auto_shrink([false, false]).animated(false).show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui);
            if focus_in_view(ui) {
                ui.ctx().request_discard("the keyboard's focus moved out of view");
            }
        });
    });
}

pub fn focus_in_view(ui: &Ui) -> bool {
    let ctx = ui.ctx();
    let focused = ctx.memory(|m| m.focused());
    let seen = ui.id().with("focus in view");
    if ctx.data_mut(|d| d.get_temp::<Option<Id>>(seen).flatten()) == focused {
        return false;
    }
    ctx.data_mut(|d| d.insert_temp(seen, focused));
    let widget = focused.and_then(|id| ctx.viewport(|v| v.this_pass.widgets.get(id).copied()));
    let Some(widget) = widget.filter(|w| w.layer_id == ui.layer_id() && ui.min_rect().intersects(w.rect)) else {
        return false;
    };
    let view = ui.clip_rect();
    let out = widget.rect.top() < view.top() || widget.rect.bottom() > view.bottom();
    if out {
        ui.scroll_to_rect_animation(widget.rect, None, egui::style::ScrollAnimation::none());
    }
    out
}

#[cfg(test)]
mod tests {
    use eframe::egui::{self, Context, Event, Id, Key, Modifiers, RawInput, Rect, Response, Ui, pos2, vec2};

    use super::body;

    const ROWS: usize = 40;
    const BODY: Rect = Rect { min: pos2(100.0, 100.0), max: pos2(500.0, 400.0) };
    const NONE: Modifiers = Modifiers::NONE;

    struct Pass {
        controls: Vec<Response>,
        view: Rect,
        gained: Vec<bool>,
        elsewhere: Option<Response>,
        focused: Option<Id>,
    }

    impl Pass {
        fn new() -> Self {
            Self { controls: Vec::new(), view: Rect::NOTHING, gained: Vec::new(), elsewhere: None, focused: None }
        }

        fn focused_control(&self) -> Option<usize> {
            self.controls.iter().position(|r| Some(r.id) == self.focused)
        }

        fn in_view(&self, i: usize) -> bool {
            let r = self.controls[i].rect;
            self.view.top() <= r.top() && r.bottom() <= self.view.bottom()
        }

        fn offset(&self) -> f32 {
            self.view.top() - self.controls[0].rect.top()
        }
    }

    fn capture(ui: &Ui, controls: Vec<Response>, pass: &mut Pass) {
        pass.view = ui.clip_rect();
        pass.gained = controls.iter().map(Response::gained_focus).collect();
        pass.controls = controls;
    }

    struct Window {
        ctx: Context,
        time: f64,
    }

    impl Window {
        fn new() -> Self {
            Self { ctx: Context::default(), time: 0.0 }
        }

        fn frame(&mut self, events: Vec<Event>, mut content: impl FnMut(&mut Ui, &mut Pass)) -> Vec<Pass> {
            self.time += 1.0 / 60.0;
            let input = RawInput {
                screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(800.0, 600.0))),
                time: Some(self.time),
                events,
                ..RawInput::default()
            };
            let mut passes = Vec::new();
            let output = self.ctx.run_ui(input, |ui| {
                let mut pass = Pass::new();
                body(ui, BODY, "test", |ui| content(ui, &mut pass));
                pass.focused = ui.ctx().memory(|m| m.focused());
                passes.push(pass);
            });
            output.drop_without_applying_deltas();
            passes
        }

        fn key(&mut self, key: Key, modifiers: Modifiers, content: impl FnMut(&mut Ui, &mut Pass)) -> Vec<Pass> {
            let event = |pressed| Event::Key { key, physical_key: None, pressed, repeat: false, modifiers };
            self.frame(vec![event(true), event(false)], content)
        }
    }

    fn rows(ui: &mut Ui, pass: &mut Pass) {
        let controls = (0..ROWS).map(|i| ui.button(format!("row {i}"))).collect();
        capture(ui, controls, pass);
    }

    fn shown(passes: &[Pass]) -> &Pass {
        passes.last().expect("a pass ran")
    }

    fn assert_focus_shown(passes: &[Pass]) {
        let pass = shown(passes);
        if let Some(i) = pass.focused_control() {
            assert!(pass.in_view(i), "control {i} focused out of view: {:?} in {:?}", pass.controls[i].rect, pass.view);
        }
    }

    #[test]
    fn tab_shows_each_control_of_a_long_body_in_the_frame_it_moves() {
        let mut w = Window::new();
        let first = w.frame(Vec::new(), rows);
        assert!(!shown(&first).in_view(ROWS - 1), "the body is longer than its view");
        for row in 0..ROWS {
            let passes = w.key(Key::Tab, NONE, rows);
            assert_eq!(shown(&passes).focused_control(), Some(row));
            assert_focus_shown(&passes);
            if shown(&first).in_view(row) {
                assert_eq!(shown(&passes).offset(), 0.0, "row {row}");
            }
        }
        let wrapped = w.key(Key::Tab, NONE, rows);
        assert_eq!(shown(&wrapped).focused, None);
        let passes = w.frame(Vec::new(), rows);
        assert_eq!(shown(&passes).focused_control(), Some(0));
        assert_focus_shown(&passes);
    }

    #[test]
    fn shift_tab_reaches_the_last_control_and_goes_back_in_view() {
        let mut w = Window::new();
        w.frame(Vec::new(), rows);
        let passes = w.key(Key::Tab, Modifiers::SHIFT, rows);
        assert_eq!(shown(&passes).focused_control(), Some(ROWS - 1));
        assert_focus_shown(&passes);
        for row in (0..ROWS - 1).rev() {
            assert_focus_shown(&w.key(Key::Tab, Modifiers::SHIFT, rows));
            let passes = w.frame(Vec::new(), rows);
            assert_eq!(shown(&passes).focused_control(), Some(row));
            assert_focus_shown(&passes);
        }
    }

    #[test]
    fn arrow_moves_report_no_gained_focus_yet_are_shown() {
        let mut w = Window::new();
        w.frame(Vec::new(), rows);
        let tab = w.key(Key::Tab, NONE, rows);
        assert!(tab.iter().any(|p| p.gained[0]));
        for row in 1..ROWS {
            let pressed = w.key(Key::ArrowDown, NONE, rows);
            let next = w.frame(Vec::new(), rows);
            assert!(pressed.iter().chain(&next).all(|p| !p.gained[row]), "row {row}");
            assert_eq!(shown(&next).focused_control(), Some(row));
            assert_focus_shown(&pressed);
            assert_focus_shown(&next);
        }
    }

    #[test]
    fn a_focus_that_stays_does_not_pull_the_view_back() {
        let mut w = Window::new();
        w.frame(Vec::new(), rows);
        w.key(Key::Tab, NONE, rows);
        w.frame(Vec::new(), |ui, pass| {
            rows(ui, pass);
            ui.scroll_with_delta_animation(vec2(0.0, -600.0), egui::style::ScrollAnimation::none());
        });
        let passes = (0..3).map(|_| w.frame(Vec::new(), rows)).last().expect("three frames");
        assert_eq!(shown(&passes).focused_control(), Some(0));
        assert!(!shown(&passes).in_view(0));
        assert!(shown(&passes).offset() > 500.0, "offset {}", shown(&passes).offset());
    }

    #[test]
    fn a_focus_in_another_layer_leaves_the_body_alone() {
        fn content(ui: &mut Ui, pass: &mut Pass) {
            rows(ui, pass);
            let area = egui::Area::new(Id::new("list")).order(egui::Order::Foreground).fixed_pos(pos2(120.0, 520.0));
            pass.elsewhere = Some(area.show(ui.ctx(), |ui| ui.button("in the list")).inner);
        }
        let mut w = Window::new();
        w.frame(Vec::new(), content);
        w.frame(Vec::new(), content);
        let passes = w.key(Key::Tab, Modifiers::SHIFT, content);
        let pass = shown(&passes);
        let button = pass.elsewhere.as_ref().expect("the list was shown");
        assert_eq!(pass.focused, Some(button.id));
        assert_eq!(button.layer_id.order, egui::Order::Foreground);
        assert!(button.rect.top() > BODY.bottom() && button.rect.bottom() < pass.controls[ROWS - 1].rect.top());
        for _ in 0..2 {
            assert_eq!(shown(&w.frame(Vec::new(), content)).offset(), 0.0);
        }
    }
}
