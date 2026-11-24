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
    fe.game_mode == 2 || fe.profile.range_word(fe.game_mode, key) != 0
}

pub fn entries(fe: &FrontEnd) -> Vec<super::scene::GlobeEntry> {
    fe.globe_scene.as_ref().map(|s| s.entries(STAGE, STAGE.0 / STAGE.1, &Flow(fe))).unwrap_or_default()
}

pub(super) struct Flow<'a>(pub(super) &'a FrontEnd);

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
        self.0.recommend.flow_drop
    }

    fn is_new(&self, key: u32) -> bool {
        node_is_new(&self.0.career, &self.0.profile, self.0.game_mode, key)
    }

    fn drop_point(&self, drop: u32) -> (f32, f32) {
        drop_point(self.0, drop)
    }
}

fn node_is_new(
    career: &ssx_assets::career::CareerDb,
    profile: &ssx_assets::career::Profile,
    mode: i64,
    key: u32,
) -> bool {
    if mode != 1 {
        return false;
    }
    let incomplete = |drop: u32| profile.drop_state(drop) != DropState::Completed;
    let under = |side: fn((u32, u32)) -> u32| {
        career.events.iter().any(|e| {
            e.drop_type == DropType::Campaign
                && profile.drop_state(e.key) != DropState::Locked
                && side(career.peak_and_range(e)) == key
                && incomplete(e.key)
        })
    };
    if career.range(key).is_some() {
        under(|(_, range)| range)
    } else if career.peak(key).is_some() {
        under(|(peak, _)| peak)
    } else {
        career.event(key).is_some() && incomplete(key)
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
        remember(fe, 2, d);
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
    remember(fe, 1, peak);
    fe.globe.drop = child_focus(fe, 2, peak);
}

pub(super) fn focus_range(fe: &mut FrontEnd, range: u32) {
    fe.globe.range = range;
    remember(fe, 0, range);
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

#[cfg(test)]
mod tests {
    use super::node_is_new;
    use ssx_assets::career::profile::DropState;
    use ssx_assets::career::{CareerDb, DropType, Event, Peak, Profile, Range};

    const RANGE: u32 = 0x10;
    const OTHER_RANGE: u32 = 0x11;
    const PEAK: u32 = 0x20;
    const PEAK_2: u32 = 0x21;
    const OTHER_PEAK: u32 = 0x22;
    const WON: u32 = 0x100;
    const LOCKED: u32 = 0x101;
    const OPEN: u32 = 0x102;
    const ELSEWHERE: u32 = 0x103;
    const EXPLORE: u32 = 0x104;

    fn range(key: u32) -> Range {
        Range { key, name: String::new(), title: String::new(), movie: String::new(), unk_index: 0, selectable: true }
    }

    fn peak(key: u32, range: u32) -> Peak {
        Peak {
            key,
            name: String::new(),
            range,
            title: String::new(),
            info: String::new(),
            image: String::new(),
            selectable: true,
            campaign_type: 1,
            unk_flag: false,
            unk_floats: [0.0; 3],
        }
    }

    fn event(key: u32, drop_type: DropType, peak_ref: u32) -> Event {
        Event {
            key,
            name: String::new(),
            drop: 0,
            event_type: 0,
            drop_type,
            mode: String::new(),
            slot: String::new(),
            peak_short: String::new(),
            hud_title: String::new(),
            hud_body: String::new(),
            goal_body: String::new(),
            results_title: String::new(),
            results_body: String::new(),
            unlock_text: String::new(),
            deadly_descent: false,
            warning: 0,
            time_of_day: 0,
            cost: 0,
            difficulty: 0,
            reward: 0,
            places: None,
            targets: Vec::new(),
            required: Vec::new(),
            unk_b683b553: Vec::new(),
            rivals: Vec::new(),
            forced: Vec::new(),
            ghost_drop: false,
            unk_89d02fa4: false,
            unk_12bccfb5: 0,
            unk_0b03fc19: None,
            unk_98e9ebcc: Vec::new(),
            custom_status: 0,
            unk_links: [0; 3],
            peak_ref,
            helicopter: 0,
            map_point: [0.0; 2],
        }
    }

    fn career() -> CareerDb {
        CareerDb {
            ranges: vec![range(RANGE), range(OTHER_RANGE)],
            peaks: vec![peak(PEAK, RANGE), peak(PEAK_2, RANGE), peak(OTHER_PEAK, OTHER_RANGE)],
            events: vec![
                event(WON, DropType::Campaign, PEAK),
                event(LOCKED, DropType::Campaign, PEAK),
                event(OPEN, DropType::Campaign, PEAK_2),
                event(ELSEWHERE, DropType::Campaign, OTHER_PEAK),
                event(EXPLORE, DropType::Explore, PEAK),
            ],
            ..Default::default()
        }
    }

    #[test]
    fn a_node_is_new_while_an_open_world_tour_drop_under_it_is_not_complete() {
        let db = career();
        let mut profile = Profile::new();
        profile.drop_states.insert(WON, DropState::Completed);
        profile.drop_states.insert(OPEN, DropState::Unlocked);
        profile.drop_states.insert(ELSEWHERE, DropState::Completed);
        profile.drop_states.insert(EXPLORE, DropState::Unlocked);
        let new = |p: &Profile, key| node_is_new(&db, p, 1, key);
        assert!(new(&profile, RANGE), "the open drop on its second peak");
        assert!(new(&profile, PEAK_2));
        assert!(!new(&profile, PEAK));
        assert!(!new(&profile, OTHER_RANGE));
        assert!(!new(&profile, OTHER_PEAK));
        assert!(new(&profile, OPEN) && new(&profile, LOCKED) && !new(&profile, WON));
        assert!(!new(&profile, 0x999));
        profile.drop_states.insert(OPEN, DropState::Completed);
        assert!(!new(&profile, RANGE) && !new(&profile, PEAK_2));
        profile.drop_states.insert(LOCKED, DropState::Unlocked);
        assert!(new(&profile, RANGE) && new(&profile, PEAK));
        for mode in [0, 2, 3] {
            assert!(!node_is_new(&db, &profile, mode, RANGE) && !node_is_new(&db, &profile, mode, LOCKED));
        }
    }
}
