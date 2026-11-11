use super::*;

pub const STAGE: (f32, f32) = (1280.0, 720.0);

pub fn scene_screen(fe: &mut FrontEnd, env: &mut FeEnv) {
    let Some(level) = super::scene::level_of_screen(fe.screen as u16) else { return };
    let from_globe = fe.globe_scene.as_ref().and_then(|s| s.level).is_some_and(|l| l >= 3);
    if level == 3 && !from_globe {
        default_focus(fe);
    }
    if level == 4 && from_globe {
        let peak = fe.globe.peak;
        focus_peak(fe, peak);
    }
    if level == 5 && fe.globe.drop != 0 {
        let drop = fe.globe.drop;
        drop_chosen(fe, env, drop);
    }
    let range = scene_range(fe);
    let node = match level {
        3 => range,
        4 => fe.globe.peak,
        5 => fe.globe.drop,
        _ => 0,
    };
    let Some(scene) = fe.globe_scene.as_mut() else { return };
    scene.set_focus(if level == 2 { super::scene::MENU_FOCUS } else { range }, false);
    scene.request(level, node);
}

pub(super) fn scene_range(fe: &FrontEnd) -> u32 {
    if fe.globe.range != 0 { fe.globe.range } else { fnv1(ssx_assets::career::world_tour::START_RANGE.as_bytes()) }
}

pub(super) fn range_valid(fe: &FrontEnd, key: u32) -> bool {
    fe.game_mode != 1 || fe.profile.range_modes.get(&key).is_some_and(|w| w[1] != 0)
}

pub(super) struct Flow<'a>(&'a FrontEnd);

pub(super) const FINAL_PEAK: u32 = 0xCA56_8EDE;
pub(super) const FINAL_PEAK_GATE: u32 = 0x65E4_C947;
pub(super) const FIRST_DROP: u32 = 0xE389_832F;

impl super::scene::GlobeFlow for Flow<'_> {
    fn shown(&self, key: u32) -> bool {
        let fe = self.0;
        if fe.game_mode != 1 {
            return true;
        }
        if key == FINAL_PEAK {
            return fe.profile.drop_state(FINAL_PEAK_GATE) == DropState::Completed;
        }
        if fe.career.event(key).is_some() {
            return key != FIRST_DROP;
        }
        if fe.career.peak(key).is_some() {
            return fe.profile.campaign_globe.has_peak(key);
        }
        true
    }

    fn focusable(&self, key: u32) -> bool {
        self.shown(key)
    }

    fn range_valid(&self, key: u32) -> bool {
        range_valid(self.0, key)
    }

    fn mode(&self) -> i64 {
        self.0.game_mode
    }

    fn recommended(&self) -> u32 {
        0
    }

    fn is_new(&self, key: u32) -> bool {
        let fe = self.0;
        if fe.game_mode != 1 {
            return false;
        }
        let open = |e: &&ssx_assets::career::Event| {
            e.drop_type == DropType::Campaign && fe.profile.drop_state(e.key) != DropState::Locked
        };
        let drops: Vec<u32> = if fe.career.range(key).is_some() {
            fe.career
                .events
                .iter()
                .filter(open)
                .filter(|e| fe.career.peak_and_range(e).1 == key)
                .map(|e| e.key)
                .collect()
        } else if fe.career.peak(key).is_some() {
            fe.career
                .events
                .iter()
                .filter(open)
                .filter(|e| fe.career.peak_and_range(e).0 == key)
                .map(|e| e.key)
                .collect()
        } else if fe.career.event(key).is_some() {
            vec![key]
        } else {
            Vec::new()
        };
        drops.iter().any(|&d| fe.profile.drop_state(d) != DropState::Completed)
    }

    fn drop_point(&self, drop: u32) -> (f32, f32) {
        drop_point(self.0, drop)
    }
}

pub fn scene_update(fe: &mut FrontEnd, env: &mut FeEnv, dt: f64) {
    let range = scene_range(fe);
    let Some(mut scene) = fe.globe_scene.take() else { return };
    match scene.level {
        Some(2) if fe.screen == crate::game::fe::screens::id::MAIN_MENU => {
            scene.set_focus(super::scene::MENU_FOCUS, false)
        }
        Some(3) => scene.set_focus(range, false),
        _ => {}
    }
    let dispatches = scene.dispatches;
    let ev = scene.update(dt as f32, fe.screen, fe.globe.input, &Flow(fe));
    for (kind, seconds, delay) in std::mem::take(&mut scene.fades) {
        fe.fade_message(kind, seconds, delay);
    }
    for id in std::mem::take(&mut scene.sounds) {
        fe.play_sound(id);
    }
    let dispatched = scene.dispatches != dispatches;
    let visible = scene.level.is_some() && (scene.visible() || scene.level.is_some_and(|l| l >= 4));
    fe.globe_scene = Some(scene);
    if dispatched {
        crate::game::fe::progression::globe_dispatched(fe);
    }
    env.values.set_bool(0xBB, ev.transitioning);
    if let Some(t) = ev.list_type {
        env.values.set_int(0xB3, t);
    }
    if let Some(r) = ev.recommendation {
        env.values.set_bool(0xC6, r);
    }
    if let Some(id) = ev.audio.and_then(super::levels::highlight_sound) {
        fe.play_sound(id);
    }
    if let Some(r) = ev.range {
        fe.note(format!("globe: the stick selects range {r:#x}"));
        focus_range(fe, r);
    }
    if let Some(p) = ev.peak {
        fe.note(format!("globe: the stick selects peak {p:#x}"));
        focus_peak(fe, p);
    }
    if let Some(d) = ev.drop {
        fe.note(format!("globe: the stick selects drop {d:#x}"));
        fe.globe.drop = d;
        drop_chosen(fe, env, d);
    }
    if visible {
        arrived(fe, env);
        fe.publish.extend(["dp_globe_data".into(), "dp_drop_overlay_data".into()]);
    }
}

pub const MANDATORY_CHARACTER: u16 = 0x3F;

pub(super) fn drop_chosen(fe: &mut FrontEnd, env: &mut FeEnv, drop: u32) {
    fe.current_drop = drop;
    env.values.set_int(MANDATORY_CHARACTER, 0);
    let campaign = fe.career.event(drop).filter(|e| e.drop_type == ssx_assets::career::data::DropType::Campaign);
    let forced = campaign.and_then(|e| e.forced.get(4).copied());
    if let Some(rider) = forced.filter(|&k| k != 0 && fe.career.riders.iter().any(|r| r.key == k)) {
        env.values.set_int(MANDATORY_CHARACTER, rider as i32);
    }
}

pub(super) fn focus_peak(fe: &mut FrontEnd, peak: u32) {
    fe.globe.peak = peak;
    fe.globe.drop = child_focus(fe, 2, peak);
}

pub(super) fn focus_range(fe: &mut FrontEnd, range: u32) {
    fe.globe.range = range;
    let peak = child_focus(fe, 1, range);
    focus_peak(fe, peak);
}

pub fn stick(fe: &mut FrontEnd, x: f32, y: f32) {
    if let Some(s) = fe.globe_scene.as_mut() {
        s.stick = (x.clamp(-1.0, 1.0), y.clamp(-1.0, 1.0));
    }
}

impl FrontEnd {
    pub fn set_widescreen(&mut self, widescreen: bool) {
        if let Some(s) = self.globe_scene.as_mut() {
            s.set_widescreen(widescreen);
        }
    }

    pub fn globe_frame(&self, aspect: f32) -> Option<super::scene::GlobeFrame> {
        self.globe_scene.as_ref().map(|s| super::scene::GlobeFrame { fade: self.screen_fade(), ..s.frame(aspect) })
    }
}
