use eframe::egui::{self, Color32, CursorIcon, Id, InnerResponse, Rect, Response, RichText, Sense, Ui, pos2, vec2};

use super::layout::CUT_BUTTON;
use super::panel;
use super::shapes::{self, Cut};
use super::text::{self, Fit, Style};
use super::theme::{self, Face, size};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Primary,
    Plain,
}

const BUTTON_H: f32 = 32.0;
const BUTTON_PAD: f32 = 18.0;

pub fn button(ui: &mut Ui, label: &str, kind: Kind, enabled: bool) -> Response {
    let ppp = ui.pixels_per_point();
    let style = Style::new(Face::Title, size::SMALL, theme::TEXT).spaced(1.0);
    let galley = text::layout(ui.painter(), &label.to_uppercase(), &style, Fit::Free);
    let width = text::width(&galley, &style) + 2.0 * BUTTON_PAD;
    let sense = if enabled { Sense::click() } else { Sense::hover() };
    let (rect, mut response) = ui.allocate_exact_size(vec2(width, BUTTON_H), sense);
    if enabled {
        response = response.on_hover_cursor(CursorIcon::PointingHand);
    }
    if !ui.is_rect_visible(rect) {
        return response;
    }
    let snap = |v: f32| (v * ppp).round() / ppp;
    let rect =
        Rect::from_min_max(pos2(snap(rect.left()), snap(rect.top())), pos2(snap(rect.right()), snap(rect.bottom())));
    let points = shapes::chamfered(rect, Cut::diagonal(CUT_BUTTON));
    let painter = ui.painter();
    let hot = enabled && (response.hovered() || response.has_focus());
    let down = enabled && response.is_pointer_button_down_on();
    if hot {
        shapes::glow(painter, &points, 8.0, theme::CYAN.gamma_multiply(0.30));
    }
    let fill = match (kind, hot, down) {
        (_, _, true) => Color32::from_rgb(0x0E, 0x76, 0x8A),
        (Kind::Primary, true, _) => Color32::from_rgb(0x0B, 0x66, 0x78),
        (Kind::Primary, false, _) => theme::TAB[1],
        (Kind::Plain, true, _) => Color32::from_rgb(0x0A, 0x40, 0x50),
        (Kind::Plain, false, _) => Color32::from_rgb(0x06, 0x26, 0x33),
    };
    let dim = if enabled { 1.0 } else { 0.40 };
    shapes::fill(painter, &points, fill.gamma_multiply(dim));
    let line = if hot || kind == Kind::Primary { theme::CYAN } else { theme::LINE };
    shapes::outline(painter, &points, (1.0 * ppp).round().max(1.0) / ppp, line.gamma_multiply(dim));
    let base = rect.center().y + size::SMALL * theme::CAP_HEIGHT / 2.0;
    let colour = if hot || kind == Kind::Primary { theme::TEXT } else { theme::TEXT_SOFT };
    let galley = text::layout(painter, &label.to_uppercase(), &style.color(colour.gamma_multiply(dim)), Fit::Free);
    text::paint(painter, pos2(rect.left() + BUTTON_PAD, base), galley);
    response
}

pub fn combo<R>(ui: &mut Ui, combo: egui::ComboBox, add: impl FnOnce(&mut Ui) -> R) -> InnerResponse<Option<R>> {
    combo.show_ui(ui, |ui| {
        let inner = add(ui);
        panel::focus_in_view(ui);
        inner
    })
}

pub struct Question<'a> {
    pub title: &'a str,
    pub text: &'a str,
    pub yes: &'a str,
    pub yes_enabled: bool,
}

#[derive(Clone, Copy, Default)]
struct Asked {
    opener: Option<Id>,
    cancelled: bool,
}

const QUESTION_WIDTH: f32 = 520.0;

pub fn question(ctx: &egui::Context, id: Id, asked: Option<Question<'_>>) -> Option<bool> {
    let state = id.with("asked");
    let Some(asked) = asked else {
        if ctx.memory(|m| m.top_modal_layer().is_none())
            && let Some(Asked { opener: Some(opener), cancelled: true }) = ctx.data_mut(|d| d.remove_temp(state))
        {
            ctx.memory_mut(|m| m.request_focus(opener));
        }
        return None;
    };
    let frame = egui::Frame::new().fill(theme::PANEL).stroke(egui::Stroke::new(1.0, theme::WARN)).inner_margin(10.0);
    let modal = egui::Modal::new(id).frame(frame).show(ctx, |ui| {
        ui.set_width(QUESTION_WIDTH);
        ui.label(RichText::new(shown(asked.title)).font(Face::Title.id(size::TEXT)).color(theme::WARN));
        paragraph(ui, &shown(asked.text));
        ui.push_id(asked.yes, |ui| {
            ui.horizontal(|ui| {
                let yes = button(ui, asked.yes, Kind::Primary, asked.yes_enabled);
                let cancel = button(ui, "Cancel", Kind::Plain, true);
                let answer = if yes.clicked() { Some(true) } else { cancel.clicked().then_some(false) };
                (answer, [yes.id, cancel.id])
            })
            .inner
        })
        .inner
    });
    let (mut answer, [yes, cancel]) = modal.inner;
    if answer.is_none() && modal.should_close() {
        answer = Some(false);
    }
    let focused = ctx.memory(|m| m.focused());
    match answer {
        None if focused != Some(yes) && focused != Some(cancel) => {
            ctx.data_mut(|d| _ = d.get_temp_mut_or_insert_with(state, || Asked { opener: focused, cancelled: false }));
            ctx.memory_mut(|m| m.request_focus(cancel));
            ctx.request_discard("the keyboard's focus left the question");
        }
        None => {}
        Some(go) => {
            ctx.data_mut(|d| d.get_temp_mut_or_insert_with(state, Asked::default).cancelled = !go);
            if !go {
                ctx.memory_mut(|m| m.request_focus(cancel));
            }
        }
    }
    answer
}

pub fn heading(ui: &mut Ui, label: &str) {
    let style = Style::new(Face::Title, size::CAPS, theme::CYAN_SOFT).spaced(1.6);
    let galley = text::layout(ui.painter(), &label.to_uppercase(), &style, Fit::Free);
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 22.0), Sense::hover());
    let base = rect.top() + 6.0 + size::CAPS * theme::CAP_HEIGHT;
    let ink = text::paint(ui.painter(), pos2(rect.left(), base), galley);
    let ppp = ui.pixels_per_point();
    let y = ((base - size::CAPS * theme::CAP_HEIGHT / 2.0) * ppp).round() / ppp;
    let rule = Rect::from_min_max(pos2(ink.right() + 6.0, y), pos2(rect.right(), y + 1.0 / ppp));
    ui.painter().rect_filled(rule, 0.0, theme::LINE_FAINT);
}

pub fn field(ui: &mut Ui, label: &str) {
    ui.label(RichText::new(label).color(theme::TEXT_DIM));
}

pub fn paragraph(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text).color(theme::TEXT_SOFT));
}

pub fn note(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(shown(text)).color(theme::TEXT_DIM));
}

pub fn shown(text: &str) -> String {
    ssx_core::text::shown_lines(text, ssx_core::text::LINE_CHARS).to_string()
}

pub fn path(ui: &mut Ui, path: &std::path::Path) {
    let text = ssx_core::text::shown_max(&path.to_string_lossy(), ssx_core::text::PATH_CHARS).to_string();
    let label = egui::Label::new(RichText::new(&text).monospace().color(theme::TEXT)).truncate();
    ui.add(label).on_hover_text(text);
}

pub fn status(ui: &mut Ui, colour: Color32, text: impl Into<String>) {
    ui.label(RichText::new(shown(&text.into())).color(colour));
}

pub fn status_cut(ui: &mut Ui, colour: Color32, text: impl Into<String>) {
    let text = shown(&text.into());
    ui.add(egui::Label::new(RichText::new(&text).color(colour)).truncate()).on_hover_text(text);
}

pub fn progress(ui: &mut Ui, fraction: f32) {
    let ppp = ui.pixels_per_point();
    let snap = |v: f32| (v * ppp).round() / ppp;
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 12.0), Sense::hover());
    let rect =
        Rect::from_min_max(pos2(snap(rect.left()), snap(rect.top())), pos2(snap(rect.right()), snap(rect.bottom())));
    let track = shapes::chamfered(rect, Cut::diagonal(4.0));
    shapes::fill(ui.painter(), &track, Color32::from_rgb(0x02, 0x10, 0x18));
    let done =
        Rect::from_min_max(rect.min, pos2(snap(rect.left() + rect.width() * fraction.clamp(0.0, 1.0)), rect.bottom()));
    if done.width() > 8.0 {
        let fill = shapes::chamfered(done, Cut { bl: 4.0, ..Default::default() });
        let (x0, w) = (rect.left(), rect.width());
        shapes::fill_with(ui.painter(), &fill, 1.0 / ppp, |p| {
            theme::TAB[1].lerp_to_gamma(theme::CYAN, ((p.x - x0) / w).clamp(0.0, 1.0))
        });
    }
    shapes::outline(ui.painter(), &track, 1.0 / ppp, theme::LINE);
}

#[cfg(test)]
mod tests {
    use eframe::egui::{Context, Event, Id, Key, Modifiers, Order, RawInput, Rect, pos2, vec2};

    use super::{Question, question};
    use crate::ui::{panel, theme};

    const ROWS: usize = 40;
    const BODY: Rect = Rect { min: pos2(100.0, 100.0), max: pos2(700.0, 500.0) };
    const OPENER: usize = 30;

    #[derive(Default)]
    struct Page {
        asked: Option<&'static str>,
        then: Option<&'static str>,
        answers: Vec<bool>,
        opener: Option<usize>,
        rows: Vec<Id>,
        at_keys: Vec<Option<Id>>,
    }

    struct Window {
        ctx: Context,
        time: f64,
        page: Page,
    }

    impl Window {
        fn new() -> Self {
            let ctx = Context::default();
            theme::install(&ctx);
            let mut window = Self { ctx, time: 0.0, page: Page::default() };
            window.frame(Vec::new());
            window.frame(Vec::new());
            window
        }

        fn frame(&mut self, events: Vec<Event>) {
            self.time += 1.0 / 60.0;
            let input = RawInput {
                screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(800.0, 600.0))),
                time: Some(self.time),
                events,
                ..RawInput::default()
            };
            let page = &mut self.page;
            page.at_keys.clear();
            let output = self.ctx.run_ui(input, |ui| {
                panel::body(ui, BODY, "page", |ui| {
                    let asked = page.asked.map(|yes| Question {
                        title: "Remove it?",
                        text: "It goes.",
                        yes,
                        yes_enabled: true,
                    });
                    if let Some(go) = question(ui.ctx(), Id::new("question"), asked) {
                        page.answers.push(go);
                        page.asked = if go { page.then.take() } else { None };
                    }
                    page.rows.clear();
                    ui.vertical_centered(|ui| {
                        for i in 0..ROWS {
                            let row = ui.button(format!("row {i}"));
                            page.rows.push(row.id);
                            if row.clicked() {
                                page.asked = Some("Move to the trash");
                                page.opener = Some(i);
                            }
                        }
                    });
                });
                page.at_keys.push(ui.ctx().memory(|m| m.focused()));
            });
            output.drop_without_applying_deltas();
        }

        fn key(&mut self, key: Key) {
            let event =
                |pressed| Event::Key { key, physical_key: None, pressed, repeat: false, modifiers: Modifiers::NONE };
            self.frame(vec![event(true), event(false)]);
        }

        fn focused(&self) -> Option<Id> {
            self.ctx.memory(|m| m.focused())
        }

        fn focus_in_question(&self) -> bool {
            let widget = self.focused().and_then(|id| self.ctx.read_response(id));
            widget.is_some_and(|w| w.layer_id.order == Order::Foreground)
        }

        fn ask(&mut self) {
            for _ in 0..=OPENER {
                self.key(Key::Tab);
            }
            assert_eq!(self.focused(), Some(self.page.rows[OPENER]), "Tab reached the opener");
            self.key(Key::Space);
            assert_eq!((self.page.asked, self.page.opener), (Some("Move to the trash"), Some(OPENER)));
            self.frame(Vec::new());
            assert!(self.focus_in_question(), "the focus moved into the question as it opened");
        }

        fn keys_quiet(&self) -> bool {
            self.page.at_keys.iter().all(Option::is_some)
        }

        fn opener_gets_the_focus_back(&mut self) {
            for frame in 0..3 {
                assert!(self.keys_quiet(), "frame {frame} after the answer: the focus was on nothing");
                if self.focused() == Some(self.page.rows[OPENER]) {
                    let opener = self.ctx.read_response(self.page.rows[OPENER]).map(|r| r.rect);
                    assert!(opener.is_some_and(|r| BODY.contains_rect(r)), "the opener is in view: {opener:?}");
                    return;
                }
                self.frame(Vec::new());
            }
            panic!("the opener did not get the focus back: {:?}", self.focused());
        }
    }

    #[test]
    fn the_focus_stays_in_a_question_and_goes_back_to_its_opener() {
        let mut w = Window::new();
        w.ask();
        w.key(Key::ArrowUp);
        assert!(w.keys_quiet() && !w.focus_in_question(), "premise: ArrowUp left the question for a row beneath it");
        w.frame(Vec::new());
        assert!(w.focus_in_question(), "the question took the focus back");
        for key in [Key::ArrowDown, Key::ArrowLeft, Key::ArrowRight, Key::ArrowUp, Key::Tab, Key::Tab] {
            w.key(key);
            assert!(w.keys_quiet(), "{key:?}: the focus was on nothing");
            w.frame(Vec::new());
            assert!(w.focus_in_question(), "{key:?}: the focus is not in the question");
        }
        assert!(w.page.answers.is_empty() && w.page.opener == Some(OPENER), "nothing was answered, no row pressed");
        w.key(Key::Escape);
        assert_eq!(w.page.answers, [false]);
        w.opener_gets_the_focus_back();
    }

    #[test]
    fn a_stray_key_or_click_keeps_what_was_asked_about() {
        for key in [Key::Enter, Key::Space] {
            let mut w = Window::new();
            w.ask();
            w.key(key);
            assert_eq!(w.page.answers, [false], "{key:?} pressed Cancel");
            w.opener_gets_the_focus_back();
        }
        let mut w = Window::new();
        w.ask();
        w.key(Key::ArrowUp);
        w.key(Key::Enter);
        assert!(w.page.answers.is_empty() && w.page.opener == Some(OPENER), "an Enter pressed nothing");
        w.frame(Vec::new());
        assert!(w.focus_in_question());
        let mut w = Window::new();
        w.ask();
        let (pos, button) = (pos2(20.0, 20.0), eframe::egui::PointerButton::Primary);
        let click = |pressed| Event::PointerButton { pos, button, pressed, modifiers: Modifiers::NONE };
        w.frame(vec![Event::PointerMoved(pos), click(true)]);
        w.frame(vec![click(false)]);
        assert_eq!(w.page.answers, [false], "a click beside the question cancelled it");
        w.opener_gets_the_focus_back();
    }

    #[test]
    fn yes_goes_ahead_and_the_question_it_asks_next_is_a_new_one() {
        let mut w = Window::new();
        w.page.then = Some("Delete permanently");
        w.ask();
        w.key(Key::Tab);
        w.key(Key::Space);
        assert_eq!((w.page.answers.as_slice(), w.page.asked), (&[true][..], Some("Delete permanently")));
        w.frame(Vec::new());
        assert!(w.focus_in_question());
        w.key(Key::Space);
        assert_eq!(w.page.answers, [true, false]);
        w.opener_gets_the_focus_back();
        let mut w = Window::new();
        w.ask();
        w.key(Key::Tab);
        w.key(Key::Space);
        assert_eq!(w.page.answers, [true]);
        for _ in 0..3 {
            w.frame(Vec::new());
        }
        assert_eq!(w.focused(), None);
    }
}
