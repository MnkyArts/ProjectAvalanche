use super::super::clip::Clip;
use crate::avm1::Ctx;

#[derive(Clone, Debug)]
pub struct HudText {
    pub clip: Clip,
    text: String,
    timer: f32,
    count: usize,
    per_char: f32,
    active: bool,
    from_left: bool,
    visible: bool,
}

impl HudText {
    pub fn new(cx: &mut Ctx, mut clip: Clip) -> Self {
        let visible = clip.visible(cx);
        HudText {
            clip,
            text: String::new(),
            timer: -1.0,
            count: 0,
            per_char: 80.0,
            active: false,
            from_left: true,
            visible,
        }
    }

    pub fn set_rate(&mut self, ms: f32) {
        self.per_char = ms;
    }

    fn prefix(&self, n: usize) -> String {
        // PORT(choice): the port cuts characters, the game UTF-8 bytes (an EASTL substring); identical on the HUD's
        // texts, which are ASCII.
        self.text.chars().take(n).collect()
    }

    pub fn set_text(&mut self, cx: &mut Ctx, text: &str, cancel_reveal: bool) {
        self.text = text.to_string();
        if self.active && !cancel_reveal {
            let shown = self.prefix(self.count);
            self.clip.set_text(cx, &shown);
        } else {
            if self.active {
                self.active = false;
                self.timer = -1.0;
            }
            self.clip.set_text(cx, text);
        }
        if !self.visible {
            self.set_visible(cx, true);
        }
    }

    pub fn set_visible(&mut self, cx: &mut Ctx, visible: bool) {
        if self.visible != visible {
            self.clip.set_visible(cx, visible);
            self.visible = visible;
        }
    }

    pub fn text_width(&mut self, cx: &mut Ctx) -> i32 {
        self.clip.get_number(cx, "textWidth") as i32
    }

    pub fn position(&mut self, cx: &mut Ctx) -> (i32, i32) {
        let i = self.clip.info(cx);
        (i.x as i32, i.y as i32)
    }

    pub fn set_position(&mut self, cx: &mut Ctx, x: i32, y: i32) {
        self.clip.set_position(cx, f64::from(x), f64::from(y));
    }

    pub fn reveal(&mut self, cx: &mut Ctx, delay_ms: f32, from_left: bool) {
        let text = self.clip.text(cx);
        self.start(cx, delay_ms, &text, from_left);
    }

    fn start(&mut self, cx: &mut Ctx, delay_ms: f32, text: &str, from_left: bool) {
        self.active = true;
        self.text = text.to_string();
        self.from_left = from_left;
        self.timer = -1.0;
        let shown = if from_left { String::new() } else { self.text.clone() };
        self.clip.set_text(cx, &shown);
        self.count = 0;
        if delay_ms <= 0.0 {
            self.step(cx);
            self.timer = self.per_char;
        } else {
            self.timer = delay_ms;
        }
    }

    pub fn tick(&mut self, cx: &mut Ctx, dt: f64) {
        if !self.active {
            return;
        }
        if self.timer <= 0.0 {
            self.step(cx);
            self.timer = self.per_char;
        } else {
            self.timer -= (dt * 1000.0) as f32;
        }
    }

    fn step(&mut self, cx: &mut Ctx) {
        self.count += 1;
        let len = self.text.chars().count();
        if self.count < len {
            let shown = if self.from_left { self.prefix(self.count) } else { self.prefix(len - self.count) };
            self.clip.set_text(cx, &shown);
        } else {
            let shown = if self.from_left { self.text.clone() } else { String::new() };
            self.clip.set_text(cx, &shown);
            self.active = false;
            self.timer = -1.0;
        }
    }
}
