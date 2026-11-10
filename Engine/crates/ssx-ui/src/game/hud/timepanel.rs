use super::text::HudText;
use super::{Component, HudEnv};
use crate::avm1::Ctx;
use crate::avm1::value::Value;
use crate::game::clip::Clip;
use crate::game::values::id;

#[derive(Default)]
pub struct TimePanel {
    panel: Option<Clip>,
    container: Option<Clip>,
    backer: Option<Clip>,
    stroke: Option<Clip>,
    speed: Option<HudText>,
    speed_label: Option<HudText>,
    time: Option<HudText>,
    descended: Option<HudText>,
    descended_label: Option<HudText>,
    secondary: Option<HudText>,
    secondary_label: Option<HudText>,
    speed_active: bool,
    descent_active: bool,
    time_active: bool,
    labelled: bool,
    last_int: i32,
    last_speed: f32,
    countdown: i32,
    last_time: u32,
    bound: bool,
    revealed: bool,
}

const BINDINGS: &[&str] = &[
    "db_hud_time_panel",
    "db_hud_speed_blurred",
    "db_hud_speed_label_blurred",
    "db_hud_time_container",
    "db_hud_time_blurred",
    "db_hud_descended_blurred",
    "db_hud_descended_label_blurred",
    "db_hud_secondary_descended_blurred",
    "db_hud_secondary_descended_label_blurred",
    "db_hud_backer",
    "db_hud_backer_stroke",
];

impl Component for TimePanel {
    fn bindings(&self) -> &'static [&'static str] {
        BINDINGS
    }

    fn bind(&mut self, cx: &mut Ctx, env: &mut HudEnv, name: &str, clip: Clip) {
        match name {
            "db_hud_time_panel" => self.panel = Some(clip),
            "db_hud_speed_blurred" => self.speed = Some(HudText::new(cx, clip)),
            "db_hud_speed_label_blurred" => {
                let mut t = HudText::new(cx, clip);
                let unit = env.loc.speed_unit();
                t.set_text(cx, &unit, false);
                self.speed_label = Some(t);
            }
            "db_hud_time_container" => self.container = Some(clip),
            "db_hud_time_blurred" => self.time = Some(HudText::new(cx, clip)),
            "db_hud_descended_blurred" => self.descended = Some(HudText::new(cx, clip)),
            "db_hud_descended_label_blurred" => self.descended_label = Some(HudText::new(cx, clip)),
            "db_hud_secondary_descended_blurred" => self.secondary = Some(HudText::new(cx, clip)),
            "db_hud_secondary_descended_label_blurred" => self.secondary_label = Some(HudText::new(cx, clip)),
            "db_hud_backer" => self.backer = Some(clip),
            "db_hud_backer_stroke" => self.stroke = Some(clip),
            _ => return,
        }
        self.bound = true;
    }

    fn unbind(&mut self, name: &str) {
        self.bound = false;
        match name {
            "db_hud_time_panel" => self.panel = None,
            "db_hud_speed_blurred" => self.speed = None,
            "db_hud_speed_label_blurred" => self.speed_label = None,
            "db_hud_time_container" => self.container = None,
            "db_hud_time_blurred" => self.time = None,
            "db_hud_descended_blurred" => self.descended = None,
            "db_hud_descended_label_blurred" => self.descended_label = None,
            "db_hud_secondary_descended_blurred" => self.secondary = None,
            "db_hud_secondary_descended_label_blurred" => self.secondary_label = None,
            "db_hud_backer" => self.backer = None,
            "db_hud_backer_stroke" => self.stroke = None,
            _ => {}
        }
    }

    fn update(&mut self, cx: &mut Ctx, env: &mut HudEnv, dt: f64) {
        self.speed_active = env.values.bool(id::SPEED_ACTIVE);
        self.descent_active = env.values.bool(id::DESCENT_ACTIVE);
        self.time_active = env.values.bool(id::TIME_ACTIVE);
        if self.bound {
            self.update_values(cx, env, dt);
            self.show_or_hide(cx, env);
        }
    }
}

impl TimePanel {
    fn update_values(&mut self, cx: &mut Ctx, env: &mut HudEnv, dt: f64) {
        let event_type = env.event_type;
        if self.speed_active
            && let (Some(speed), Some(label)) = (self.speed.as_mut(), self.speed_label.as_mut())
        {
            let v = env.values.float(id::PLAYER_SPEED);
            if (v - self.last_speed).abs() >= 0.25 {
                let text = env.loc.speed(v);
                speed.set_text(cx, &text, false);
                let (x, _) = speed.position(cx);
                let nx = speed.text_width(cx) + x + 6;
                let (lx, ly) = label.position(cx);
                if lx != nx {
                    label.set_position(cx, nx, ly);
                }
                self.last_speed = v;
            }
            speed.tick(cx, dt);
            label.tick(cx, dt);
        }
        if self.time_active && self.time.is_some() {
            if !self.labelled {
                let label = if (4..=7).contains(&event_type) { "race" } else { "normal" };
                if let Some(c) = self.container.as_mut() {
                    c.goto_label(cx, label, false);
                }
                self.set_labels(cx, env, event_type == 4);
                self.labelled = true;
            }
            let time = self.time.as_mut().expect("checked");
            if event_type == 4 {
                let v = env.values.float(id::PLAYER_DESCENT_WITH_LAPS) as i32;
                if v != self.last_int {
                    let text = env.loc.distance_with_unit(f64::from(v));
                    time.set_text(cx, &text, false);
                    self.last_int = v;
                    if let (Some(b), Some(s)) = (self.backer.as_mut(), self.stroke.as_mut()) {
                        let w = Value::Number(f64::from(time.text_width(cx) + 0x14));
                        b.set(cx, "_width", w.clone());
                        s.set(cx, "_width", w);
                    }
                }
            } else {
                self.redraw_clock(cx, env, ClockField::Time);
            }
            if let Some(t) = self.time.as_mut() {
                t.tick(cx, dt);
            }
        }
        let all = self.descended.is_some()
            && self.descended_label.is_some()
            && self.secondary.is_some()
            && self.secondary_label.is_some();
        if self.descent_active && all {
            let (start, end) = (env.values.float(id::START_ELEVATION), env.values.float(id::END_ELEVATION));
            let left = (start - end) - env.values.float(id::PLAYER_DESCENT);
            let left = if left > 0.0 { left } else { 0.0 } as i32;
            if left != self.last_int {
                let text = env.loc.distance(f64::from(left));
                if event_type == 4 {
                    place_after(cx, self.secondary.as_mut(), self.secondary_label.as_mut(), &text);
                } else {
                    place_after(cx, self.descended.as_mut(), self.descended_label.as_mut(), &text);
                    if let Some(s) = self.secondary.as_mut() {
                        s.set_visible(cx, false);
                        s.set_text(cx, " ", false);
                    }
                }
                self.last_int = left;
            }
            if event_type == 4 {
                self.redraw_clock(cx, env, ClockField::Descended);
            }
            for t in [&mut self.descended, &mut self.descended_label, &mut self.secondary, &mut self.secondary_label]
                .into_iter()
                .flatten()
            {
                t.tick(cx, dt);
            }
        }
    }

    fn redraw_clock(&mut self, cx: &mut Ctx, env: &mut HudEnv, field: ClockField) {
        let t = env.values.int(id::RACE_TIME) as u32;
        self.countdown -= 1;
        if self.countdown < 1 && t != self.last_time {
            let text = env.loc.race_time(t);
            match field {
                ClockField::Time => {
                    if let Some(f) = self.time.as_mut() {
                        f.set_text(cx, &text, false);
                    }
                }
                ClockField::Descended => {
                    if let Some(f) = self.descended.as_mut() {
                        f.set_text(cx, &text, false);
                    }
                    if let Some(l) = self.descended_label.as_mut() {
                        l.set_visible(cx, false);
                    }
                }
            }
            self.last_time = t;
            self.countdown = 2;
        }
    }

    fn set_labels(&mut self, cx: &mut Ctx, env: &mut HudEnv, survive: bool) {
        let unit = env.loc.distance_unit();
        let remaining = env.loc.text("TXT_REMAINING");
        let label = format!("{unit} {remaining}");
        if let Some(l) = self.descended_label.as_mut() {
            l.set_text(cx, &label, false);
        }
        if let Some(l) = self.secondary_label.as_mut() {
            if survive {
                l.set_text(cx, &label, false);
            } else {
                l.set_text(cx, " ", false);
                l.set_visible(cx, false);
            }
        }
    }

    fn show_or_hide(&mut self, cx: &mut Ctx, env: &mut HudEnv) {
        let Some(panel) = self.panel.as_mut() else { return };
        let visible = panel.visible(cx);
        let all = self.speed_active && self.time_active && self.descent_active;
        if all && !visible {
            self.show(cx, env);
        } else if !all && visible {
            panel.set_visible(cx, false);
        }
    }

    fn show(&mut self, cx: &mut Ctx, env: &mut HudEnv) {
        if !self.revealed {
            if env.event_type == 4
                && let Some(time) = self.time.as_mut()
            {
                let v = env.values.float(id::PLAYER_DESCENT_WITH_LAPS) as i32;
                let text = env.loc.distance_with_unit(f64::from(v));
                time.set_text(cx, &text, false);
                if let (Some(b), Some(s)) = (self.backer.as_mut(), self.stroke.as_mut()) {
                    let w = Value::Number(f64::from(time.text_width(cx) + 0x14));
                    b.set(cx, "_width", w.clone());
                    s.set(cx, "_width", w);
                }
            }
            let reveals: [(&mut Option<HudText>, f32); 7] = [
                (&mut self.time, 0.0),
                (&mut self.speed_label, 250.0),
                (&mut self.speed, 250.0),
                (&mut self.descended_label, 500.0),
                (&mut self.descended, 500.0),
                (&mut self.secondary_label, 750.0),
                (&mut self.secondary, 750.0),
            ];
            for (t, delay) in reveals {
                if let Some(t) = t.as_mut() {
                    t.reveal(cx, delay, true);
                }
            }
            self.revealed = true;
        }
        if let Some(p) = self.panel.as_mut() {
            p.set_visible(cx, true);
        }
    }
}

#[derive(Clone, Copy)]
enum ClockField {
    Time,
    Descended,
}

fn place_after(cx: &mut Ctx, value: Option<&mut HudText>, label: Option<&mut HudText>, text: &str) {
    let (Some(value), Some(label)) = (value, label) else { return };
    value.set_text(cx, text, false);
    let (x, _) = value.position(cx);
    let nx = value.text_width(cx) + x + 6;
    let (lx, ly) = label.position(cx);
    if lx != nx {
        label.set_position(cx, nx, ly);
    }
}
