use eframe::egui::{self, Context, Id, LayerId, Rect};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Device {
    #[default]
    Mouse,
    Keys,
    Pad(PadStyle),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PadStyle {
    Xbox,
    PlayStation,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dir {
    Up,
    Down,
    Left,
    Right,
}

impl Dir {
    pub fn of(key: egui::Key) -> Option<Dir> {
        match key {
            egui::Key::ArrowUp => Some(Dir::Up),
            egui::Key::ArrowDown => Some(Dir::Down),
            egui::Key::ArrowLeft => Some(Dir::Left),
            egui::Key::ArrowRight => Some(Dir::Right),
            _ => None,
        }
    }
}

// PORT(choice): the offset is the leading edges' (left for Up / Down, top for Left / Right), not the centres' as in the
// prototype: down from a full-width row the focus lands on the first of the buttons under it, the reading order
// (Xbox Accessibility Guideline 112: the focus order follows the visual flow); the console's menus are the game's.
const ACROSS: f32 = 2.2;
const SLACK: f32 = 2.0;

pub fn next(from: Rect, dir: Dir, candidates: impl IntoIterator<Item = (Id, Rect)>) -> Option<Id> {
    let mut best = None::<(f32, Id)>;
    for (id, r) in candidates {
        let past = match dir {
            Dir::Down => r.top() >= from.bottom() - SLACK,
            Dir::Up => r.bottom() <= from.top() + SLACK,
            Dir::Right => r.left() >= from.right() - SLACK,
            Dir::Left => r.right() <= from.left() + SLACK,
        };
        if !past {
            continue;
        }
        let (gap, across) = match dir {
            Dir::Down => (r.top() - from.bottom(), (r.left() - from.left()).abs()),
            Dir::Up => (from.top() - r.bottom(), (r.left() - from.left()).abs()),
            Dir::Right => (r.left() - from.right(), (r.top() - from.top()).abs()),
            Dir::Left => (from.left() - r.right(), (r.top() - from.top()).abs()),
        };
        let score = gap + ACROSS * across;
        if best.is_none_or(|(s, _)| score < s) {
            best = Some((score, id));
        }
    }
    best.map(|(_, id)| id)
}

pub fn first(candidates: impl IntoIterator<Item = (Id, Rect)>, area: Rect) -> Option<Id> {
    candidates
        .into_iter()
        .filter(|(_, r)| area.intersects(*r))
        .min_by(|(_, a), (_, b)| a.top().total_cmp(&b.top()).then(a.left().total_cmp(&b.left())))
        .map(|(id, _)| id)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pass {
    Last,
    This,
}

pub fn controls(ctx: &Context, pass: Pass, layer: LayerId, area: Rect) -> Vec<(Id, Rect)> {
    let inner = ctx.data(|d| d.get_temp::<Registry>(registry_key()).map(|r| r.inner).unwrap_or_default());
    ctx.viewport(|v| {
        let widgets = match pass {
            Pass::Last => &v.prev_pass.widgets,
            Pass::This => &v.this_pass.widgets,
        };
        widgets
            .get_layer(layer)
            .filter(|w| w.enabled && w.sense.is_focusable() && area.intersects(w.rect) && !inner.contains(&w.id))
            .map(|w| (w.id, w.rect))
            .collect()
    })
}

pub fn placed(ctx: &Context, id: Id) -> Option<(LayerId, Rect)> {
    ctx.viewport(|v| v.prev_pass.widgets.get(id).map(|w| (w.layer_id, w.rect)))
}

#[derive(Clone, Default)]
struct Registry {
    sideways: Vec<Id>,
    inner: Vec<Id>,
}

fn registry_key() -> Id {
    Id::new("nav-registry")
}

pub fn begin_frame(ctx: &Context) {
    ctx.data_mut(|d| d.insert_temp(registry_key(), Registry::default()));
}

pub fn takes_sideways(ctx: &Context, id: Id) {
    ctx.data_mut(|d| d.get_temp_mut_or_default::<Registry>(registry_key()).sideways.push(id));
}

pub fn took_sideways(ctx: &Context, id: Id) -> bool {
    ctx.data(|d| d.get_temp::<Registry>(registry_key()).is_some_and(|r| r.sideways.contains(&id)))
}

pub fn inner(ctx: &Context, id: Id) {
    ctx.data_mut(|d| d.get_temp_mut_or_default::<Registry>(registry_key()).inner.push(id));
}

pub fn is_inner(ctx: &Context, id: Id) -> bool {
    ctx.data(|d| d.get_temp::<Registry>(registry_key()).is_some_and(|r| r.inner.contains(&id)))
}

fn steps_key() -> Id {
    Id::new("nav-steps")
}

pub fn put_steps(ctx: &Context, steps: i32) {
    ctx.data_mut(|d| *d.get_temp_mut_or_default::<i32>(steps_key()) += steps);
}

pub fn take_steps(ctx: &Context) -> i32 {
    ctx.data_mut(|d| d.remove_temp::<i32>(steps_key())).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use eframe::egui::{Id, Rect, pos2};

    use super::*;

    fn rect(x0: f32, y0: f32, x1: f32, y1: f32) -> Rect {
        Rect::from_min_max(pos2(x0, y0), pos2(x1, y1))
    }

    fn section() -> Vec<(Id, Rect)> {
        let mut all: Vec<(Id, Rect)> = (0..4)
            .map(|i| (Id::new(("row", i)), rect(360.0, 248.0 + 40.0 * i as f32, 896.0, 286.0 + 40.0 * i as f32)))
            .collect();
        for (i, x) in [360.0, 480.0, 620.0].into_iter().enumerate() {
            all.push((Id::new(("button", i)), rect(x, 420.0, x + 110.0, 452.0)));
        }
        all
    }

    fn from(all: &[(Id, Rect)], id: Id, dir: Dir) -> Option<Id> {
        let at = all.iter().find(|(i, _)| *i == id).unwrap().1;
        next(at, dir, all.iter().copied().filter(|(i, _)| *i != id))
    }

    #[test]
    fn rows_move_one_by_one_and_buttons_side_by_side() {
        let all = section();
        let (row, button) = (|i: usize| Id::new(("row", i)), |i: usize| Id::new(("button", i)));
        assert_eq!(from(&all, row(0), Dir::Down), Some(row(1)));
        assert_eq!(from(&all, row(2), Dir::Up), Some(row(1)));
        assert_eq!(from(&all, row(0), Dir::Up), None, "the top row: nothing above");
        assert_eq!(from(&all, row(3), Dir::Down), Some(button(0)));
        assert_eq!(from(&all, button(0), Dir::Right), Some(button(1)));
        assert_eq!(from(&all, button(2), Dir::Left), Some(button(1)));
        assert_eq!(from(&all, button(1), Dir::Up), Some(row(3)));
        assert_eq!(from(&all, row(1), Dir::Left), None);
        assert_eq!(from(&all, button(0), Dir::Left), None);
    }

    #[test]
    fn the_play_page_moves_between_the_hero_and_the_tiles() {
        let hero = (Id::new("hero"), rect(336.0, 168.0, 1232.0, 456.0));
        let tiles: Vec<_> = (0..3)
            .map(|i| (Id::new(("tile", i)), rect(336.0 + 304.0 * i as f32, 472.0, 624.0 + 304.0 * i as f32, 656.0)))
            .collect();
        let all: Vec<_> = std::iter::once(hero).chain(tiles.iter().copied()).collect();
        assert_eq!(from(&all, hero.0, Dir::Down), Some(tiles[0].0), "under the hero's left edge");
        assert_eq!(from(&all, tiles[0].0, Dir::Right), Some(tiles[1].0));
        assert_eq!(from(&all, tiles[2].0, Dir::Up), Some(hero.0));
        assert_eq!(from(&all, tiles[0].0, Dir::Left), None);
        let area = rect(336.0, 168.0, 1232.0, 656.0);
        assert_eq!(first(all.iter().copied(), area), Some(hero.0));
        assert_eq!(first(tiles.iter().copied(), area), Some(tiles[0].0));
        assert_eq!(first(tiles.iter().copied(), rect(0.0, 0.0, 10.0, 10.0)), None);
    }

    #[test]
    fn egui_moves_a_focus_asked_for_in_the_pass_by_its_arrow_unless_called_off() {
        use eframe::egui::{Event, FocusDirection, Key, Modifiers, RawInput, vec2};
        for (called_off, landed) in [(false, 1), (true, 0)] {
            let ctx = eframe::egui::Context::default();
            let right = Event::Key {
                key: Key::ArrowRight,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Modifiers::NONE,
            };
            let screen = rect(0.0, 0.0, 400.0, 200.0);
            let mut ids = Vec::new();
            let input = RawInput { events: vec![right], screen_rect: Some(screen), ..Default::default() };
            let output = ctx.run_ui(input, |ui| {
                ui.horizontal(|ui| {
                    ids = (0..2)
                        .map(|i| ui.add_sized(vec2(80.0, 30.0), eframe::egui::Button::new(format!("{i}"))).id)
                        .collect()
                });
                ui.memory_mut(|m| {
                    m.request_focus(ids[0]);
                    if called_off {
                        m.move_focus(FocusDirection::None);
                    }
                });
            });
            output.drop_without_applying_deltas();
            assert_eq!(ctx.memory(|m| m.focused()), Some(ids[landed]), "called off: {called_off}");
        }
    }
}
