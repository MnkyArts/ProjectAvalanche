use eframe::egui::{self, Color32, Context, Id, Rect, Response, Sense, Ui, UiBuilder, pos2, vec2};

use super::layout::CUT_TILE;
use super::shapes::{self, Cut};
use super::text::{self, Fit, Style};
use super::theme::{self, Face, size};

#[derive(Clone, Debug, PartialEq)]
pub struct Fact {
    pub name: &'static str,
    pub value: String,
    pub colour: Color32,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Detail {
    pub title: String,
    pub text: Vec<String>,
    pub warn: Option<String>,
    pub facts: Vec<Fact>,
    pub applies: Option<String>,
}

impl Detail {
    pub fn new(title: impl Into<String>) -> Self {
        Detail { title: title.into(), ..Default::default() }
    }

    pub fn text(mut self, paragraph: impl Into<String>) -> Self {
        self.text.push(paragraph.into());
        self
    }

    pub fn warn(mut self, warning: Option<String>) -> Self {
        self.warn = warning;
        self
    }

    pub fn fact(mut self, name: &'static str, value: impl Into<String>) -> Self {
        self.facts.push(Fact { name, value: value.into(), colour: theme::TEXT });
        self
    }

    pub fn now(mut self, now: impl Into<String>, original: Option<(String, bool)>) -> Self {
        self = self.fact("NOW", now);
        if let Some((value, in_use)) = original {
            let value = if in_use { format!("{value} (in use)") } else { value };
            self.facts.push(Fact { name: "ORIGINAL", value, colour: theme::GOOD });
        }
        self
    }

    pub fn applies(mut self, when: impl Into<String>) -> Self {
        self.applies = Some(when.into());
        self
    }
}

pub const APPLIES_GAME: &str = "Saved at once. Applies the next time the game starts; the game's own Options change \
     it too.";
pub const APPLIES_NEXT_START: &str = "Saved at once. Applies the next time the game starts.";
pub const APPLIES_TOOLS: &str = "Saved at once. Applies the next time a tool starts.";

#[derive(Clone, Default)]
struct Offered {
    focused: Option<Detail>,
    hovered: Option<Detail>,
}

fn key() -> Id {
    Id::new("details-offered")
}

pub fn begin(ctx: &Context) {
    ctx.data_mut(|d| d.insert_temp(key(), Offered::default()));
}

pub fn offer(ui: &Ui, response: &Response, detail: impl FnOnce() -> Detail) {
    let (focused, hovered) = (response.has_focus(), response.hovered());
    if !focused && !hovered {
        return;
    }
    let taken = ui.ctx().data(|d| d.get_temp::<Offered>(key()).unwrap_or_default());
    if focused {
        let made = detail();
        ui.ctx().data_mut(|d| d.get_temp_mut_or_default::<Offered>(key()).focused = Some(made));
    } else if taken.hovered.is_none() {
        let made = detail();
        ui.ctx().data_mut(|d| d.get_temp_mut_or_default::<Offered>(key()).hovered = Some(made));
    }
}

pub fn take(ctx: &Context) -> Option<Detail> {
    let offered = ctx.data_mut(|d| d.remove_temp::<Offered>(key())).unwrap_or_default();
    offered.focused.or(offered.hovered)
}

const INSET: f32 = 20.0;
const PARAGRAPH: Style = Style::new(Face::Text, size::WIDGET, theme::TEXT_SOFT).leading(19.0);

pub fn idle(section: &str, text: &str) -> Detail {
    let mut name = section.to_lowercase();
    if let Some(first) = name.get(..1) {
        name = first.to_uppercase() + &name[1..];
    }
    Detail::new(name).text(text)
}

pub const PICK: &str = "Pick an option to see what it does.";

pub fn draw(ui: &mut Ui, rect: Rect, ppp: f32, detail: &Detail) {
    let points = shapes::chamfered(rect, Cut::diagonal(CUT_TILE));
    shapes::fill(ui.painter(), &points, theme::NIGHT.gamma_multiply(0.55));
    shapes::outline(ui.painter(), &points, (ppp.round().max(1.0)) / ppp, theme::LINE_FAINT);
    let inner = rect.shrink2(vec2(INSET - 2.0, INSET));
    let builder =
        UiBuilder::new().max_rect(inner).layout(egui::Layout::top_down(egui::Align::Min)).sense(Sense::hover());
    ui.scope_builder(builder, |ui| {
        ui.set_clip_rect(rect.shrink(2.0));
        let id = Id::new("details").with(&detail.title);
        egui::ScrollArea::vertical().id_salt(id).auto_shrink([false, false]).animated(false).show(ui, |ui| {
            let width = ui.available_width() - 6.0;
            ui.spacing_mut().item_spacing.y = 0.0;
            let title = Style::new(Face::Bold, size::TILE_TITLE, theme::TEXT);
            label(ui, &detail.title, title, width);
            ui.add_space(10.0);
            for paragraph in &detail.text {
                label(ui, paragraph, PARAGRAPH, width);
                ui.add_space(10.0);
            }
            if let Some(warning) = &detail.warn {
                label(ui, warning, PARAGRAPH.color(theme::WARN), width);
                ui.add_space(10.0);
            }
            if !detail.facts.is_empty() {
                ui.add_space(4.0);
                facts(ui, &detail.facts, width);
            }
            if let Some(when) = &detail.applies {
                ui.add_space(14.0);
                let (rule, _) = ui.allocate_exact_size(vec2(width, 1.0), Sense::hover());
                ui.painter().rect_filled(rule, 0.0, theme::LINE_FAINT);
                ui.add_space(10.0);
                label(ui, when, Style::new(Face::Text, size::SMALL, theme::TEXT_DIM).leading(18.0), width);
            }
        });
    });
}

fn label(ui: &mut Ui, words: &str, style: Style, width: f32) {
    let galley = text::layout(ui.painter(), &super::widgets::shown(words), &style, Fit::Wrap(width));
    ui.add(egui::Label::new(galley).selectable(false));
}

fn facts(ui: &mut Ui, facts: &[Fact], width: f32) {
    const NAMES: f32 = 72.0;
    let name_style = Style::new(Face::Title, 12.0, theme::TEXT_DIM).spaced(0.8);
    for fact in facts {
        let value = Style::new(Face::Text, size::SMALL, fact.colour).leading(18.0);
        let galley = text::layout(ui.painter(), &super::widgets::shown(&fact.value), &value, Fit::Wrap(width - NAMES));
        let (rect, _) = ui.allocate_exact_size(vec2(width, galley.rect.height().max(18.0) + 4.0), Sense::hover());
        let base = rect.top() + 14.0;
        text::draw(ui.painter(), pos2(rect.left(), base), fact.name, &name_style);
        text::paint(ui.painter(), pos2(rect.left() + NAMES, base), galley);
    }
}
