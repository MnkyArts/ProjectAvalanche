use eframe::egui::{self, Event, Key, Rangef, Rect};

use crate::app::Launcher;
use crate::ui::layout::Layout;
use crate::ui::nav::{self, Device, Dir, Pass};
use crate::ui::prompts::{Glyph, Prompt};
use crate::ui::{MenuItem, Page};

#[derive(Clone, Copy, Debug, Default)]
pub struct NavState {
    pub area: Option<(egui::LayerId, Rect)>,
    pub focus_end: Option<egui::Id>,
    pub in_menu: bool,
}

impl Launcher {
    pub fn take_input(&mut self, ctx: &egui::Context, raw: &mut egui::RawInput) {
        for event in &raw.events {
            match event {
                Event::Key { pressed: true, .. } => self.device = Device::Keys,
                Event::PointerMoved(_) | Event::PointerButton { .. } | Event::MouseWheel { .. } => {
                    self.device = Device::Mouse
                }
                _ => {}
            }
        }
        let (keys, style) = self.pads.take(ctx, raw.focused, std::time::Instant::now());
        raw.events.extend(keys);
        if let Some(style) = style {
            self.device = Device::Pad(style);
        }
        let mut at = ctx.memory(|m| m.focused());
        if at.is_none() && self.nav.focus_end.is_some() && self.device != Device::Mouse {
            self.enter_page = true;
        }
        self.nav.in_menu = at.is_none() && !self.enter_page;
        let mut events = std::mem::take(&mut raw.events);
        events.retain(|event| !self.page_key(ctx, event, &mut at));
        raw.events = events;
    }

    fn page_key(&mut self, ctx: &egui::Context, event: &Event, at: &mut Option<egui::Id>) -> bool {
        let Event::Key { key, pressed: true, modifiers, .. } = event else { return false };
        if modifiers.any() {
            return false;
        }
        let typing = ctx.text_edit_focused();
        let modal = ctx.memory(|m| m.top_modal_layer());
        if matches!(key, Key::Q | Key::E) && !typing && modal.is_none() {
            if !self.turn_section(if *key == Key::Q { -1 } else { 1 }) {
                return false;
            }
            if let Some(id) = at.take() {
                ctx.memory_mut(|m| m.surrender_focus(id));
                self.enter_page = true;
            }
            return true;
        }
        let (Some(dir), Some(id)) = (Dir::of(*key), *at) else { return false };
        if typing && (matches!(dir, Dir::Left | Dir::Right) || !nav::is_inner(ctx, id)) {
            return false;
        }
        if matches!(dir, Dir::Left | Dir::Right) && nav::took_sideways(ctx, id) {
            nav::put_steps(ctx, if dir == Dir::Left { -1 } else { 1 });
            return true;
        }
        let (Some((page, area)), Some((layer, from))) = (self.nav.area, nav::placed(ctx, id)) else { return true };
        let column = match layer == page {
            true => Rect::from_x_y_ranges(area.x_range(), Rangef::EVERYTHING),
            false => Rect::EVERYTHING,
        };
        let candidates = nav::controls(ctx, Pass::Last, layer, column).into_iter().filter(|(c, _)| *c != id);
        match nav::next(from, dir, candidates) {
            Some(to) => {
                ctx.memory_mut(|m| m.request_focus(to));
                *at = Some(to);
            }
            None if dir == Dir::Left && layer == page && modal.is_none() => {
                ctx.memory_mut(|m| m.surrender_focus(id));
                *at = None;
            }
            None => {}
        }
        true
    }

    pub fn after_page(&mut self, ctx: &egui::Context, layer: egui::LayerId, l: &Layout, drawn: Page) {
        let area = if drawn == Page::Play { l.content } else { l.body_wide.union(l.details) };
        self.nav.area = Some((layer, area));
        self.menu_keys(ctx);
        if self.enter_page {
            if self.page == drawn {
                self.enter_page = false;
                let column = Rect::from_x_y_ranges(area.x_range(), Rangef::EVERYTHING);
                if let Some(id) = nav::first(nav::controls(ctx, Pass::This, layer, column), area) {
                    ctx.memory_mut(|m| {
                        m.request_focus(id);
                        m.move_focus(egui::FocusDirection::None);
                    });
                }
            } else {
                ctx.request_repaint();
            }
        }
        self.nav.focus_end = ctx.memory(|m| m.focused());
    }

    fn menu_keys(&mut self, ctx: &egui::Context) {
        let modal = ctx.memory(|m| m.top_modal_layer().is_some());
        if !self.nav.in_menu || modal || ctx.memory(|m| m.focused().is_some()) || ctx.any_popup_open() {
            return;
        }
        let pressed = |key| ctx.input(|i| i.key_pressed(key));
        let step = pressed(Key::ArrowDown) as isize - pressed(Key::ArrowUp) as isize;
        if step != 0 {
            self.cursor = self.cursor.moved(step);
            if let MenuItem::Page(page) = self.cursor {
                self.page = page;
            }
        }
        if pressed(Key::Enter) {
            match self.cursor {
                MenuItem::Exit => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
                MenuItem::Page(Page::Play) => self.hero_clicked(ctx),
                MenuItem::Page(page) => {
                    self.go(page);
                    self.enter_page = true;
                }
            }
        } else if pressed(Key::ArrowRight)
            && let MenuItem::Page(page) = self.cursor
        {
            self.go(page);
            self.enter_page = true;
        }
        if pressed(Key::Escape) {
            self.go(Page::Play);
        }
    }

    pub fn nav_report(&self, ctx: &egui::Context) -> String {
        let Some((layer, area)) = self.nav.area else { return "nav: no page drawn\n".to_owned() };
        let column = Rect::from_x_y_ranges(area.x_range(), Rangef::EVERYTHING);
        let all = nav::controls(ctx, Pass::This, layer, column);
        let mut reached: Vec<egui::Id> = nav::first(all.iter().copied(), area).into_iter().collect();
        let mut at = 0;
        while let Some(&id) = reached.get(at) {
            if let Some(&(_, from)) = all.iter().find(|(c, _)| *c == id) {
                for dir in [Dir::Up, Dir::Down, Dir::Left, Dir::Right] {
                    let others = all.iter().copied().filter(|(c, _)| *c != id);
                    if let Some(to) = nav::next(from, dir, others)
                        && !reached.contains(&to)
                    {
                        reached.push(to);
                    }
                }
            }
            at += 1;
        }
        let section = match self.page {
            Page::Play => String::new(),
            Page::Online => format!(" {:?}", self.online_tab),
            Page::Content => format!(" {:?} {:?}", self.content_tab, self.mods_view),
            Page::Settings => format!(" {:?}", self.settings_tab),
            Page::Extras => format!(" {:?}", self.extras_tab),
        };
        let focused = ctx.memory(|m| m.focused());
        let shown = |r: Rect| format!("{:.0},{:.0}..{:.0},{:.0}", r.left(), r.top(), r.right(), r.bottom());
        let focus = match focused.and_then(|id| all.iter().find(|(c, _)| *c == id)) {
            Some(&(_, r)) => shown(r),
            None if focused.is_some() => "a control outside the page".to_owned(),
            None => "none (the menu)".to_owned(),
        };
        let mut out = format!(
            "nav: page {:?}{section} controls {} reached {} focused {focus}\n",
            self.page,
            all.len(),
            reached.len()
        );
        for (id, r) in &all {
            if !reached.contains(id) {
                out += &format!("nav: unreached {}\n", shown(*r));
            }
        }
        out
    }

    pub fn prompts(&self, ctx: &egui::Context) -> Vec<Prompt> {
        const SECTIONS: &[Glyph] = &[Glyph::PrevSection, Glyph::NextSection];
        let focused = ctx.memory(|m| m.focused());
        let mut out: Vec<Prompt> = Vec::new();
        match focused {
            None => {
                let open = match self.cursor {
                    MenuItem::Exit => "EXIT",
                    MenuItem::Page(Page::Play) if self.data_ready => "PLAY",
                    MenuItem::Page(Page::Play) => "SET UP",
                    MenuItem::Page(_) => "OPEN",
                };
                out.push((&[Glyph::Confirm], open));
                out.push((&[Glyph::Move], "MOVE"));
                if self.page != Page::Play {
                    out.push((&[Glyph::Back], "PLAY PAGE"));
                }
            }
            Some(id) => {
                let sideways = nav::took_sideways(ctx, id);
                out.push((&[Glyph::Confirm], if sideways { "NEXT" } else { "SELECT" }));
                if sideways {
                    out.push((&[Glyph::Change], "CHANGE"));
                }
                out.push((&[Glyph::Back], "BACK"));
            }
        }
        if self.page != Page::Play {
            out.push((SECTIONS, "SECTION"));
        }
        out
    }
}
