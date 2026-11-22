use super::{Component, HudEnv};
use crate::avm1::Ctx;
use crate::avm1::value::Value;
use crate::game::clip::{Clip, DI_VISIBLE, DI_X, DI_Y, DisplayInfo};
use crate::game::values::id;

#[derive(Default)]
struct CachedText {
    clip: Option<Clip>,
    last: Option<String>,
}

impl CachedText {
    fn set(&mut self, cx: &mut Ctx, text: &str, force: bool) {
        let Some(c) = self.clip.as_mut() else { return };
        if force || self.last.as_deref() != Some(text) {
            c.set_text(cx, text);
            self.last = Some(text.to_string());
        }
    }
}

#[derive(Default)]
pub struct Scoring {
    component: Option<Clip>,
    holder: Option<Clip>,
    flow: Option<Clip>,
    flow_fill: Option<Clip>,
    splits: Option<Clip>,
    gamerpic: Option<Clip>,
    gamerpic_loader: Option<Clip>,
    body_container: Option<Clip>,
    body: Option<Clip>,
    time_container: Option<Clip>,
    time: Option<Clip>,
    medal_container: Option<Clip>,
    medal: Option<Clip>,
    tricky: Option<Clip>,
    tricky_warning: Option<Clip>,
    rewinds: Option<Clip>,
    rewinds_notice: Option<Clip>,
    burn: Option<Clip>,
    chevrons: [Option<Clip>; 6],
    score_base: CachedText,
    score_glow: CachedText,
    combo: CachedText,
    combo_title: CachedText,
    body_txt: CachedText,
    body_glow: CachedText,
    time_txt: CachedText,
    time_glow: CachedText,
    rewinds_txt: CachedText,
    notice_txt: CachedText,
    deduction_txt: CachedText,
    holder_watch: bool,
    meter_ready: bool,
    flow_intro: bool,
    score_active: bool,
    flare: bool,
    last_score: Option<i32>,
    last_rewind_score: Option<i32>,
    flow_level: i32,
    warn_tier: i32,
    last_warn_tier: i32,
    letters: u32,
    boost: f32,
    awesome: bool,
    super_awesome: bool,
    activation: f32,
    holder_frame: i32,
    notice_frame: i32,
    deposit: bool,
    refresh: bool,
    rewind_count: i32,
    last_rewind_count: Option<i32>,
    rewind_infinite: bool,
    rewind_enabled: bool,
    boosting: bool,
    burning: bool,
    last_width: f32,
    bound: bool,
    shown: Option<bool>,
    vis: [Option<bool>; 4],
    rewinding: bool,
    was_rewinding: bool,
    notice_outro: bool,
}

const BINDINGS: &[&str] = &[
    "db_hud_scoring_component",
    "db_hud_scoring_score_holder",
    "db_hud_scoring_score_base_txt",
    "db_hud_scoring_score_glow_txt",
    "db_hud_scoring_combo_txt",
    "db_hud_scoring_combo_title_txt",
    "db_hud_scoring_combo_chevron_left_outer",
    "db_hud_scoring_combo_chevron_left_middle",
    "db_hud_scoring_combo_chevron_left_inner",
    "db_hud_scoring_combo_chevron_right_outer",
    "db_hud_scoring_combo_chevron_right_middle",
    "db_hud_scoring_combo_chevron_right_inner",
    "db_hud_scoring_splits_container",
    "db_hud_scoring_splits_gamerpic",
    "db_hud_scoring_splits_gamerpic_imageloader",
    "db_hud_scoring_splits_body_container",
    "db_hud_scoring_splits_body",
    "db_hud_scoring_splits_body_txt",
    "db_hud_scoring_splits_body_glow_txt",
    "db_hud_scoring_splits_time_container",
    "db_hud_scoring_splits_time",
    "db_hud_scoring_splits_time_txt",
    "db_hud_scoring_splits_time_glow_txt",
    "db_hud_scoring_splits_medal_container",
    "db_hud_scoring_splits_medal",
    "db_hud_scoring_tricky",
    "db_hud_scoring_tricky_warning",
    "db_hud_scoring_flow",
    "db_hud_scoring_flow_fill_bar",
    "db_hud_scoring_rewinds",
    "db_hud_scoring_rewinds_txt",
    "db_hud_scoring_rewinds_deduction_txt",
    "db_hud_scoring_rewinds_notice",
    "db_hud_scoring_rewinds_notice_txt",
    "db_hud_scoring_rewinds_notice_icons",
    "db_hud_scoring_rewinds_notice_icon_1",
    "db_hud_scoring_rewinds_notice_icon_2",
    "db_hud_scoring_rewinds_notice_icon_3",
    "db_hud_scoring_rewinds_notice_icon_4",
    "db_hud_scoring_rewinds_notice_icon_5",
    "db_hud_scoring_burn",
];

pub fn fill_width(boost: f32) -> f32 {
    boost * 278.0 + 26.0
}

pub fn letters(activation: f32, super_awesome: bool) -> u32 {
    if super_awesome {
        return 6;
    }
    let n = (activation * 6.0).floor() as i64 as u32;
    if n <= 6 { n } else { 6 }
}

impl Component for Scoring {
    fn bindings(&self) -> &'static [&'static str] {
        BINDINGS
    }

    fn bind(&mut self, cx: &mut Ctx, _env: &mut HudEnv, name: &str, clip: Clip) {
        let text = |t: &mut CachedText, c: Clip| {
            t.clip = Some(c);
            t.last = None;
        };
        match name {
            "db_hud_scoring_component" => self.component = Some(clip),
            "db_hud_scoring_score_holder" => self.holder = Some(clip),
            "db_hud_scoring_score_base_txt" => text(&mut self.score_base, clip),
            "db_hud_scoring_score_glow_txt" => text(&mut self.score_glow, clip),
            "db_hud_scoring_combo_txt" => text(&mut self.combo, clip),
            "db_hud_scoring_combo_title_txt" => text(&mut self.combo_title, clip),
            "db_hud_scoring_combo_chevron_left_outer" => self.chevrons[0] = Some(clip),
            "db_hud_scoring_combo_chevron_right_outer" => self.chevrons[1] = Some(clip),
            "db_hud_scoring_combo_chevron_left_middle" => self.chevrons[2] = Some(clip),
            "db_hud_scoring_combo_chevron_right_middle" => self.chevrons[3] = Some(clip),
            "db_hud_scoring_combo_chevron_left_inner" => self.chevrons[4] = Some(clip),
            "db_hud_scoring_combo_chevron_right_inner" => self.chevrons[5] = Some(clip),
            "db_hud_scoring_flow" => self.flow = Some(clip),
            "db_hud_scoring_flow_fill_bar" => self.flow_fill = Some(clip),
            "db_hud_scoring_splits_container" => self.splits = Some(clip),
            "db_hud_scoring_splits_gamerpic" => self.gamerpic = Some(clip),
            "db_hud_scoring_splits_gamerpic_imageloader" => self.gamerpic_loader = Some(clip),
            "db_hud_scoring_splits_body_container" => self.body_container = Some(clip),
            "db_hud_scoring_splits_body" => self.body = Some(clip),
            "db_hud_scoring_splits_body_txt" => text(&mut self.body_txt, clip),
            "db_hud_scoring_splits_body_glow_txt" => text(&mut self.body_glow, clip),
            "db_hud_scoring_splits_time_container" => self.time_container = Some(clip),
            "db_hud_scoring_splits_time" => self.time = Some(clip),
            "db_hud_scoring_splits_time_txt" => text(&mut self.time_txt, clip),
            "db_hud_scoring_splits_time_glow_txt" => text(&mut self.time_glow, clip),
            "db_hud_scoring_splits_medal_container" => self.medal_container = Some(clip),
            "db_hud_scoring_splits_medal" => self.medal = Some(clip),
            "db_hud_scoring_tricky" => self.tricky = Some(clip),
            "db_hud_scoring_tricky_warning" => self.tricky_warning = Some(clip),
            "db_hud_scoring_rewinds" => self.rewinds = Some(clip),
            "db_hud_scoring_rewinds_txt" => text(&mut self.rewinds_txt, clip),
            "db_hud_scoring_rewinds_notice" => self.rewinds_notice = Some(clip),
            "db_hud_scoring_rewinds_notice_txt" => text(&mut self.notice_txt, clip),
            "db_hud_scoring_rewinds_deduction_txt" => text(&mut self.deduction_txt, clip),
            "db_hud_scoring_burn" => self.burn = Some(clip),
            _ => {}
        }
        match name {
            "db_hud_scoring_component" => self.show(cx, false),
            "db_hud_scoring_flow_fill_bar" => set_vis(cx, &mut self.vis[0], self.flow_fill.as_mut(), false),
            "db_hud_scoring_tricky" => set_vis(cx, &mut self.vis[1], self.tricky.as_mut(), false),
            "db_hud_scoring_tricky_warning" => self.set_warning(cx, false),
            _ => {}
        }
        self.bound = true;
    }

    fn unbind(&mut self, _name: &str) {
        // TODO(guess): 0x8262A378 clears the one binding's pointer (0x82629E90 with 0); the port drops the update until the next bind; settle: track each binding's pointer.
        self.bound = false;
    }

    fn update(&mut self, cx: &mut Ctx, env: &mut HudEnv, dt: f64) {
        let v = &*env.values;
        let active = v.bool(id::SCORE_ACTIVE);
        let active_changed = v.is_dirty(id::SCORE_ACTIVE);
        self.score_active = active;
        let mut changed = false;
        if active {
            self.boost = v.float(id::BOOST_PERCENT);
            self.awesome = v.bool(id::IS_AWESOME);
            self.super_awesome = v.bool(id::IS_SUPER_AWESOME);
            self.activation = v.float(id::SUPER_AWESOME_ACTIVATION_PERCENTAGE);
            self.was_rewinding = self.rewinding;
            self.rewinding = v.bool(id::IS_REWINDING);
            self.boosting = v.bool(id::IS_BOOSTING);
            changed = [
                id::BOOST_PERCENT,
                id::IS_AWESOME,
                id::IS_SUPER_AWESOME,
                id::SUPER_AWESOME_ACTIVATION_PERCENTAGE,
                id::IS_REWINDING,
                id::IS_BOOSTING,
            ]
            .iter()
            .any(|&i| v.is_dirty(i));
            self.warn_tier = v.int(id::AWESOME_WARN_TIER);
            self.rewind_count = v.int(id::REWIND_COUNT);
            self.rewind_infinite = v.bool(id::REWIND_COUNT_INFINITE);
            self.rewind_enabled = v.bool(id::REWIND_COUNT_ENABLED);
            if !self.holder_watch
                && let Some(c) = self.component.as_mut()
            {
                let info = DisplayInfo { x: 640.0, y: 610.0, visible: true, ..Default::default() };
                c.set_info(cx, &info, DI_X | DI_Y | DI_VISIBLE);
                // TODO(guess): a port choice — the disc's placement (SetDisplayInfo flags 0x43: x 640, y 610, visible)
                // leaves the shown byte +0x1A9 as it is; the port records the show there. They differ only when UI
                // 0x131 rose before the bind (0x8262A9A0 → 0x8262A5B0(0): hidden, +0x1A9 = 0): the placement shows the
                // clip again and the HUD's hide (0x8238C100 → 0x82385110 → 0x8262A5B0(0)) is skipped, the meter stays
                // over the pause menu. The HUD view binds `mcScoring` at its frame 30 (`OnScoreHudLoaded`), before a
                // drop's intro reaches its GO; a GO in the view's first second (fe_run's 16 ride steps per UI frame)
                // raises 0x131 first; settle: the original with 0x131 raised before frame 30.
                self.shown = Some(true);
            }
            if self.deposit {
                if self.component.is_none() {
                    self.deposit = false;
                } else if let Some(h) = self.holder.as_mut() {
                    let f = h.current_frame(cx);
                    if !self.holder_watch {
                        self.holder_frame = f;
                        self.holder_watch = true;
                    } else {
                        let prev = std::mem::replace(&mut self.holder_frame, f);
                        if prev == f {
                            self.deposit = false;
                            env.sounds.push(0x53);
                            h.goto_label(cx, "Intro", false);
                            self.refresh = true;
                        }
                    }
                }
                if !self.deposit {
                    env.values.set_bool(id::IS_PLAYING_SCORE_ANIM, false);
                }
            }
        }
        if self.bound {
            self.meter(cx, env, changed);
            self.score(cx, env);
            self.flow_multiplier(cx, env);
            self.splits_callout(cx, env);
            let rewind_changed = env.values.is_dirty(id::REWIND_COUNT);
            self.rewind_count_text(cx, env, rewind_changed);
            let _ = dt;
        }
        if active_changed && self.bound && self.component.is_some() {
            self.show(cx, active);
        }
    }
}

fn set_vis(cx: &mut Ctx, state: &mut Option<bool>, clip: Option<&mut Clip>, v: bool) {
    let Some(c) = clip else { return };
    if *state != Some(v) {
        c.set_visible(cx, v);
        *state = Some(v);
    }
}

impl Scoring {
    fn show(&mut self, cx: &mut Ctx, v: bool) {
        set_vis(cx, &mut self.shown, self.component.as_mut(), v);
    }

    fn set_warning(&mut self, cx: &mut Ctx, v: bool) {
        let Some(w) = self.tricky_warning.as_mut() else { return };
        if self.vis[2] != Some(v) {
            if !v {
                w.goto_frame(cx, 1, true);
            }
            w.set_visible(cx, v);
            self.vis[2] = Some(v);
        }
    }

    fn reset_meter(&mut self, cx: &mut Ctx) {
        self.flare = false;
        self.warn_tier = 0;
        self.letters = 0;
        if let Some(c) = self.component.as_mut() {
            c.goto_label(cx, "Normal", true);
        }
        if let Some(f) = self.flow_fill.as_mut() {
            f.set_visible(cx, true);
            f.set(cx, "_width", Value::Number(26.0));
        }
        self.vis[0] = Some(true);
        self.last_width = 26.0;
        for (k, c) in [(1, self.tricky.as_mut()), (2, self.tricky_warning.as_mut()), (3, self.rewinds.as_mut())] {
            if let Some(c) = c {
                c.set_visible(cx, false);
            }
            self.vis[k] = Some(false);
        }
    }

    fn meter(&mut self, cx: &mut Ctx, env: &mut HudEnv, changed: bool) {
        let all = self.component.is_some() && self.flow_fill.is_some() && self.tricky.is_some();
        if !all || self.tricky_warning.is_none() || self.rewinds.is_none() || !(changed || !self.meter_ready) {
            return;
        }
        if !self.meter_ready {
            self.reset_meter(cx);
        }
        if self.rewinding {
            if !self.was_rewinding {
                self.start_rewinding(cx, env);
                self.notice_outro = false;
            }
            self.rewind_score(cx, env);
        } else if self.was_rewinding {
            if let Some(n) = self.rewinds_notice.as_mut() {
                n.goto_label(cx, "Outro", false);
                env.sounds.push(0x76);
                self.notice_outro = true;
                self.notice_frame = n.current_frame(cx);
            }
        } else if self.notice_outro {
            if let Some(n) = self.rewinds_notice.as_mut() {
                let f = n.current_frame(cx);
                let prev = std::mem::replace(&mut self.notice_frame, f);
                if prev == f {
                    self.reset_meter(cx);
                    self.notice_outro = false;
                }
            }
        } else if self.boost == 1.0 && (self.awesome || self.super_awesome) {
            if !self.flare {
                set_vis(cx, &mut self.vis[1], self.tricky.as_mut(), true);
                self.set_warning(cx, false);
                if let Some(c) = self.component.as_mut() {
                    c.goto_label(cx, "Flare", false);
                }
                if let Some(t) = self.tricky.as_mut() {
                    t.goto_label(cx, "Intro", false);
                }
                self.letters = 0;
                self.warn_tier = 0;
                self.flare = true;
                env.sounds.push(0x51);
            }
            let mut force = false;
            if self.warn_tier != self.last_warn_tier {
                if self.warn_tier == 0 {
                    self.set_warning(cx, false);
                    set_vis(cx, &mut self.vis[1], self.tricky.as_mut(), true);
                    force = true;
                } else if self.warn_tier == 1 {
                    self.set_warning(cx, true);
                    set_vis(cx, &mut self.vis[1], self.tricky.as_mut(), false);
                    force = true;
                    env.sounds.push(0x6E);
                }
                self.last_warn_tier = self.warn_tier;
            }
            let old = self.letters;
            self.letters = letters(self.activation, self.super_awesome);
            if force || self.letters != old {
                let target = if self.warn_tier == 1 { self.tricky_warning.as_mut() } else { self.tricky.as_mut() };
                if let Some(t) = target {
                    t.goto_label(cx, &format!("letter{}", self.letters), false);
                }
                env.sounds.push(0x67 + self.letters);
            }
            set_vis(cx, &mut self.vis[0], self.flow_fill.as_mut(), false);
        } else {
            set_vis(cx, &mut self.vis[1], self.tricky.as_mut(), false);
            self.set_warning(cx, false);
            self.letters = 0;
            if self.boost == 0.0 {
                set_vis(cx, &mut self.vis[0], self.flow_fill.as_mut(), false);
            } else {
                let w = fill_width(self.boost);
                if (w - self.last_width).abs() >= 0.25 {
                    self.last_width = w;
                    if let Some(f) = self.flow_fill.as_mut() {
                        f.set(cx, "_width", Value::Number(f64::from(w as i32)));
                    }
                }
                set_vis(cx, &mut self.vis[0], self.flow_fill.as_mut(), true);
                if !self.boosting {
                    if self.burning {
                        if let Some(b) = self.burn.as_mut() {
                            b.goto_label(cx, "off", true);
                        }
                        self.burning = false;
                    }
                } else {
                    if !self.burning {
                        if let Some(b) = self.burn.as_mut() {
                            b.goto_label(cx, "on", false);
                        }
                        self.burning = true;
                    }
                    if let Some(b) = self.burn.as_mut() {
                        let info = DisplayInfo { x: f64::from(w) - 163.0, visible: true, ..Default::default() };
                        b.set_info(cx, &info, DI_X | DI_VISIBLE);
                    }
                }
            }
            if self.flare
                && let Some(c) = self.component.as_mut()
            {
                c.goto_label(cx, "Normal", true);
            }
            self.flare = false;
            env.sounds.push(0x50);
        }
        if self.burning && !self.boosting {
            if let Some(b) = self.burn.as_mut() {
                b.goto_label(cx, "off", true);
            }
            self.burning = false;
        }
        self.meter_ready = true;
    }

    fn start_rewinding(&mut self, cx: &mut Ctx, env: &mut HudEnv) {
        if let Some(c) = self.component.as_mut() {
            c.goto_label(cx, "Rewinding", false);
        }
        if let Some(n) = self.rewinds_notice.as_mut() {
            n.goto_label(cx, "Intro", false);
        }
        set_vis(cx, &mut self.vis[1], self.tricky.as_mut(), false);
        self.set_warning(cx, false);
        set_vis(cx, &mut self.vis[0], self.flow_fill.as_mut(), false);
        env.sounds.push(0x75);
        let text = if self.rewind_infinite {
            env.loc.text("TXT_HUD_REWINDING_MESSAGE_UNLIMITED")
        } else {
            let n = env.loc.int(i64::from(self.rewind_count));
            env.loc.format("TXT_HUD_REWINDING_MESSAGE_LIMITED", &[&n])
        };
        self.notice_txt.set(cx, &text, false);
        self.deduction_txt.set(cx, " ", false);
        self.last_rewind_score = None;
    }

    fn rewind_score(&mut self, cx: &mut Ctx, env: &mut HudEnv) {
        let v = env.values.float(id::REWIND_SCORE) as i32;
        if self.last_rewind_score != Some(v) {
            let text = env.loc.int(i64::from(v));
            self.deduction_txt.set(cx, &text, false);
            self.last_rewind_score = Some(v);
        }
    }

    fn score(&mut self, cx: &mut Ctx, env: &mut HudEnv) {
        if self.component.is_none() || self.deposit {
            return;
        }
        let s = env.values.float(id::SCORE) as i32;
        if self.refresh || self.last_score != Some(s) {
            let text = env.loc.int(i64::from(s));
            self.score_base.set(cx, &text, false);
            self.score_glow.set(cx, &text, false);
            self.last_score = Some(s);
            self.refresh = false;
        }
        let pct = env.values.float(id::COMBO_CURRENT_TIMER_PERCENTAGE);
        let lit = if pct > 0.75 {
            [false, false, false, false, false]
        } else if pct > 0.5 {
            [true, true, false, false, false]
        } else if pct > 0.25 {
            [true, true, true, true, false]
        } else {
            [true, true, true, true, true]
        };
        for (k, on) in lit.iter().enumerate() {
            if let Some(c) = self.chevrons[k].as_mut() {
                c.goto_label(cx, if *on { "lit" } else { "normal" }, true);
            }
        }
        if let Some(c) = self.chevrons[5].as_mut() {
            c.goto_label(cx, if lit[4] { "lit" } else { "normal" }, true);
        }
    }

    fn flow_multiplier(&mut self, cx: &mut Ctx, env: &mut HudEnv) {
        let level = env.values.int(id::FLOW_LEVEL);
        let old = std::mem::replace(&mut self.flow_level, level);
        if self.component.is_none() {
            return;
        }
        if !self.flow_intro {
            if let Some(h) = self.holder.as_mut() {
                h.goto_label(cx, "Intro", false);
            }
            self.flow_intro = true;
        } else if old == level {
            return;
        }
        let label = match level {
            -1 => "mult0".to_string(),
            0..=19 => format!("mult{}", level + 1),
            _ => "mult1".to_string(),
        };
        if let Some(f) = self.flow.as_mut() {
            f.goto_label(cx, &label, false);
        }
    }

    fn rewind_count_text(&mut self, cx: &mut Ctx, env: &mut HudEnv, changed: bool) {
        if self.rewinds_txt.clip.is_none() || self.rewinding || !changed {
            return;
        }
        let show = if self.rewind_infinite {
            false
        } else {
            if !self.rewind_enabled {
                self.rewinds_txt.set(cx, "0", false);
            } else if self.last_rewind_count != Some(self.rewind_count) {
                let t = env.loc.int(i64::from(self.rewind_count));
                self.rewinds_txt.set(cx, &t, false);
                self.last_rewind_count = Some(self.rewind_count);
            }
            true
        };
        set_vis(cx, &mut self.vis[3], self.rewinds.as_mut(), show);
    }

    pub fn deposit(&mut self, cx: &mut Ctx, env: &mut HudEnv, points: f32, bonus: f32, combo: u32) {
        let Some(h) = self.holder.as_mut() else { return };
        h.goto_label(cx, "Deposit", false);
        env.sounds.push(0x52);
        let n = env.loc.number(f64::from(points), 0);
        let text = if points <= 0.0 { n } else { env.loc.format("TXT_PLUS_VARIABLE", &[&n]) };
        self.score_base.set(cx, &text, false);
        self.score_glow.set(cx, &text, false);
        if combo < 2 {
            self.combo_title.set(cx, " ", false);
            self.combo.set(cx, " ", false);
        } else {
            let c = env.loc.int(i64::from(combo));
            let title = env.loc.format("TXT_HUD_TRICK_COMBO_DEPOSIT", &[&c]);
            self.combo_title.set(cx, &title, false);
            let b = env.loc.int(i64::from(bonus as i32));
            let plus = env.loc.format("TXT_PLUS_VARIABLE", &[&b]);
            self.combo.set(cx, &plus, false);
        }
        self.deposit = true;
        self.holder_watch = false;
        env.values.set_bool(id::IS_PLAYING_SCORE_ANIM, true);
    }

    fn splits_callout(&mut self, cx: &mut Ctx, env: &mut HudEnv) {
        if !env.values.bool(id::SPLITS_CALLOUT_ENABLED) || env.values.bool(id::SPLITS_CALLOUT_FORCE_OFF) {
            return;
        }
        env.values.set_bool(id::SPLITS_CALLOUT_ENABLED, false);
        let Some(s) = env.values.splits(id::SPLITS_CALLOUT).cloned() else { return };
        if let Some(c) = self.splits.as_mut() {
            c.goto_label(cx, "intro", false);
        }
        let (mut lead, mut width) = (0i32, 40i32);
        match s.kind {
            0 => {
                vis(cx, self.gamerpic.as_mut(), false);
                vis(cx, self.medal.as_mut(), true);
                let (label, sound) = match s.medal {
                    0 => ("gold", 0x71),
                    1 => ("silver", 0x70),
                    _ => ("bronze", 0x6F),
                };
                if let Some(m) = self.medal.as_mut() {
                    m.goto_label(cx, label, false);
                }
                if let Some(b) = self.body.as_mut() {
                    b.goto_label(cx, label, false);
                }
                env.sounds.push(sound);
                lead = self.medal.as_mut().map_or(0, |m| m.get_number(cx, "_width") as i32);
                width = lead + 40;
            }
            2 => {
                vis(cx, self.gamerpic.as_mut(), false);
                vis(cx, self.medal.as_mut(), false);
                if let Some(b) = self.body.as_mut() {
                    b.goto_label(cx, "normal", false);
                }
            }
            _ => {
                vis(cx, self.gamerpic.as_mut(), true);
                vis(cx, self.medal.as_mut(), false);
                if let Some(b) = self.body.as_mut() {
                    b.goto_label(cx, "normal", false);
                }
                if let Some(l) = self.gamerpic_loader.as_mut() {
                    l.invoke(cx, "LoadImage", &[Value::str(&s.image)]);
                }
                lead = 0x27;
                width = 0x4F;
            }
        }
        let mut time_scale = 0;
        if !s.show_time {
            vis(cx, self.time.as_mut(), false);
            self.time_txt.set(cx, "", false);
            self.time_glow.set(cx, "", false);
        } else {
            vis(cx, self.time.as_mut(), true);
            if let Some(t) = self.time.as_mut() {
                t.goto_label(cx, if s.ahead { "green" } else { "red" }, false);
            }
            self.time_txt.set(cx, &s.time, false);
            self.time_glow.set(cx, &s.time, false);
            time_scale = self.time_container.as_mut().map_or(0, |c| c.get_number(cx, "_xscale") as i32);
            let tw = self.time_txt.clip.as_mut().map_or(0, |c| c.get_number(cx, "textWidth") as i32);
            width += tw * time_scale / 100;
        }
        self.body_txt.set(cx, &s.body, false);
        self.body_glow.set(cx, &s.body, false);
        let body_scale = self.body_container.as_mut().map_or(0, |c| c.get_number(cx, "_xscale") as i32);
        let bw = self.body_txt.clip.as_mut().map_or(0, |c| c.get_number(cx, "textWidth") as i32);
        let body_w = bw * body_scale / 100;
        let left = -((body_w + width) / 2);
        let (anchor, target) = if s.kind == 0 {
            (self.medal_container.as_mut(), self.medal.as_mut())
        } else {
            (self.gamerpic.as_mut(), self.gamerpic_loader.as_mut())
        };
        let x0 = anchor.map_or(0, |a| a.get_number(cx, "_x") as i32);
        if let Some(t) = target {
            set_x(cx, t, left - x0);
        }
        let start = left + lead + 30;
        let bx = self.body_container.as_mut().map_or(0, |c| c.get_number(cx, "_x") as i32);
        let off = start - bx;
        if let Some(b) = self.body.as_mut() {
            set_x(cx, b, (100 - body_scale) * off / 100 + off);
        }
        if s.show_time {
            let tx = self.time_container.as_mut().map_or(0, |c| c.get_number(cx, "_x") as i32);
            let off = (start + body_w + 10) - tx;
            if let Some(t) = self.time.as_mut() {
                set_x(cx, t, (100 - time_scale) * off / 100 + off);
            }
        }
    }
}

fn vis(cx: &mut Ctx, c: Option<&mut Clip>, v: bool) {
    if let Some(c) = c {
        c.set_visible(cx, v);
    }
}

fn set_x(cx: &mut Ctx, c: &mut Clip, x: i32) {
    let mut info = c.info(cx);
    info.x = f64::from(x);
    c.set_info(cx, &info, DI_X);
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::avm1::Vm;
    use crate::display::Player;
    use crate::display::tests::{empty_lib, movie, po};
    use crate::game::loc::{Language, Localization};
    use crate::game::values::UiValues;
    use crate::library::FrameTag;

    #[test]
    fn a_component_bound_after_the_score_came_on_hides_with_the_hud() {
        let mut lib = empty_lib();
        let mut p = po(1, Some(10), false, 0);
        p.name = Some("mcScoring".into());
        let m = movie(&mut lib, vec![vec![FrameTag::Place(Arc::new(p))]]);
        let mut player = Player::new();
        let mut vm = Vm::new();
        vm.load_level(&mut lib, &mut player, m, 0);
        let mut cx = vm.ctx(&mut lib, &mut player);
        let mut values = UiValues::new();
        let mut loc = Localization::empty(Language::English);
        let (mut sounds, mut nav_events) = (Vec::new(), Vec::new());
        let mut env = HudEnv {
            values: &mut values,
            loc: &mut loc,
            event_type: 0,
            sounds: &mut sounds,
            nav_events: &mut nav_events,
            gear: [false; 15],
        };
        let dt = 1.0 / 30.0;
        let mut s = Scoring::default();
        env.values.set_bool(id::SCORE_ACTIVE, true);
        s.update(&mut cx, &mut env, dt);
        env.values.clear_dirty();
        let mut clip = Clip::resolve(&mut cx, "_level0.mcScoring").expect("the placed instance");
        s.bind(&mut cx, &mut env, "db_hud_scoring_component", clip.clone());
        assert!(!clip.visible(&mut cx), "the bind hides the component");
        s.update(&mut cx, &mut env, dt);
        assert!(clip.visible(&mut cx), "the placement shows it while 0x131 holds");
        env.values.clear_dirty();
        env.values.set_bool(id::SCORE_ACTIVE, false);
        s.update(&mut cx, &mut env, dt);
        assert!(!clip.visible(&mut cx), "the HUD's hide reaches it");
        env.values.clear_dirty();
        env.values.set_bool(id::SCORE_ACTIVE, true);
        s.update(&mut cx, &mut env, dt);
        assert!(clip.visible(&mut cx));
    }

    #[test]
    fn meter_formulas() {
        assert_eq!(fill_width(0.0), 26.0);
        assert_eq!(fill_width(1.0), 304.0);
        assert_eq!(letters(0.5, false), 3);
        assert_eq!(letters(0.2, true), 6);
        assert_eq!(letters(1.0, false), 6);
        assert_eq!(letters(11.0 / 12.0, false), 5);
        assert_eq!(letters(0.99, false), 5);
        assert_eq!(letters(0.16, false), 0);
        assert_eq!(letters(-0.1, false), 6);
        assert_eq!(letters(f32::NAN, false), 0);
    }
}
