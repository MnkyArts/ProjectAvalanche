mod fe_music;
mod post;

use std::path::Path;

use anyhow::Result;

use super::SfxEngine;
pub use super::frontend_tables::{CSIS_EVENTS, UI_SOUNDS};
use super::settings::{AudioSettings, FE_MUSIC_PICKS};
use crate::career::random::{GameRandom, Source};
use crate::music::PathfinderMusic;
pub use fe_music::*;
pub use post::*;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StateMsg {
    pub screen: u32,
    pub state: i32,
    pub wind: i32,
    pub result: u8,
    pub loading: bool,
    pub quit: bool,
    pub restart: bool,
}

pub fn screen_result(screen: u32, ready: bool, goal_complete: bool) -> u8 {
    match screen {
        0x27 => 3,
        0x20..=0x29 if ready => {
            if goal_complete {
                1
            } else {
                2
            }
        }
        _ => 0,
    }
}

pub mod state {
    pub const BOOT: i32 = 1;
    pub const WELCOME_HUB: i32 = 3;
    pub const MAIN_MENU: i32 = 4;
    pub const EQUIPMENT: i32 = 9;
    pub const PAUSE: i32 = 0xA;
    pub const RACE: i32 = 0xB;
    pub const TRICK: i32 = 0xC;
    pub const SURVIVE: i32 = 0xD;
    pub const TRANSITION: i32 = 0x10;
    pub const STAGING: i32 = 0x11;
}

pub fn screen_state(screen: u32, kind: i32, boot_done: bool) -> i32 {
    match screen {
        0 | 1 => 1 + i32::from(boot_done),
        5 => 3,
        2 => 4,
        3 | 4 => 5,
        0xC | 0xF | 0x42 => 6,
        0xD | 0x10 | 0x43 => 7,
        0xE | 0x11 | 0x44 => 8,
        0x2F..=0x33 => 9,
        0x1E | 0x25 => 0xA,
        0x3F | 0x40 => 0x11,
        _ => match kind {
            2 => 0xC,
            3 | 4 => 0xD,
            _ => 0xB,
        },
    }
}

#[derive(Clone, Debug, Default)]
pub struct FlowState {
    pub last_state: i32,
    transition: u32,
    pub boot_hold: u32,
    pub equipment_wait: i32,
    pub audio_mode: u8,
    watched: [bool; 4],
    last_screen: u32,
}

pub const WATCHERS: [(u16, u32, u32, bool); 4] =
    [(0x21D, 2, 0, false), (0x222, 3, 0, false), (0x226, 4, 5, true), (0x227, 6, 7, true)];

pub const TUTORIAL_SOUNDS: [(u32, usize); 7] = [(1, 114), (2, 185), (3, 186), (4, 188), (5, 187), (6, 190), (7, 189)];

#[derive(Clone, Copy, Debug, Default)]
pub struct FlowInputs {
    pub screen: u32,
    pub kind: i32,
    pub boot_done: bool,
    pub nis: u8,
    pub results_ready: bool,
    pub goal_complete: bool,
    pub setting_67: bool,
    pub wind: i32,
}

impl FlowState {
    pub fn movie_started(&mut self) {
        self.transition = 10;
    }

    pub fn boot_flow_loaded(&mut self) {
        self.boot_hold = 10;
    }

    pub fn launch_requested(&mut self) {
        self.equipment_wait = 1;
    }

    pub fn hint_shown(&mut self, kind: i32) {
        if kind == 0 || kind == 2 {
            self.audio_mode = 1;
        }
    }

    pub fn hint_closed(&mut self) {
        self.audio_mode = 0;
    }

    pub fn popup_state(&mut self, v: i32) -> bool {
        if v == 2 && self.audio_mode == 1 {
            self.audio_mode = 2;
            return true;
        }
        false
    }

    pub fn watch(&mut self, value: &dyn Fn(u16) -> bool) -> Vec<u32> {
        let mut out = Vec::new();
        for (k, &(id, on, off, needs_no_success)) in WATCHERS.iter().enumerate() {
            let v = value(id);
            let last = self.watched[k];
            let n = match (v, last) {
                (true, false) => on,
                (false, true) if !needs_no_success || !self.watched[1] => off,
                _ => 0,
            };
            if n != 0 {
                out.push(n);
            }
            self.watched[k] = v;
        }
        out
    }

    pub fn frame(&mut self, i: &FlowInputs) -> StateMsg {
        let screen = if i.screen == 0x34 || self.boot_hold != 0 { self.last_screen } else { i.screen };
        self.last_screen = screen;
        let mut st = screen_state(i.screen, i.kind, i.boot_done);
        if self.boot_hold != 0 && st == state::EQUIPMENT {
            st = state::BOOT;
            self.equipment_wait = 0;
        } else {
            if self.boot_hold != 0 {
                self.boot_hold -= 1;
            }
            if st != state::EQUIPMENT {
                self.equipment_wait = 0;
            } else if self.equipment_wait != 0 && self.equipment_wait != -1 {
                self.equipment_wait += 1;
            }
        }
        if i.screen == 0x34 || (0x13..=0x1A).contains(&i.screen) {
            st = self.last_state;
        }
        if (0xB..=0xD).contains(&st) && matches!(i.nis, 1 | 2) {
            st = if i.nis == 2 { 0xF } else { 0xE };
        }
        if self.transition > 0 {
            st = state::TRANSITION;
            self.transition -= 1;
        }
        let loading = i.screen == 0x33
            || (self.last_state == state::EQUIPMENT && i.setting_67 && self.equipment_wait as u32 > 100);
        self.last_state = st;
        StateMsg {
            screen,
            state: st,
            wind: i.wind,
            result: screen_result(i.screen, i.results_ready, i.goal_complete),
            loading,
            quit: matches!(i.screen, 0x40 | 0x41),
            restart: i.screen == 0x3F,
        }
    }
}

#[derive(Clone, Debug)]
pub struct UiSounds {
    guard: [i32; 120],
    reload: [i32; 120],
    last_boost: i32,
    pub log: Vec<String>,
}

impl Default for UiSounds {
    fn default() -> Self {
        let mut reload = [1; 120];
        (reload[12], reload[18], reload[54]) = (5, 5, 30);
        Self { guard: [0; 120], reload, last_boost: 0, log: Vec::new() }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct UiContext {
    pub last_state: i32,
    pub boost: f32,
    pub audio_mode: u8,
    pub screen_state: i32,
    pub popup_menu: bool,
}

impl UiSounds {
    pub fn frame(&mut self) {
        for g in &mut self.guard {
            if *g > 0 {
                *g -= 1;
            }
        }
    }

    pub fn play(&mut self, sfx: &mut SfxEngine, id: i32, ctx: &UiContext) -> Vec<&'static str> {
        let Some(k) = usize::try_from(id).ok().filter(|&k| k < UI_SOUNDS.len()) else { return Vec::new() };
        let mut name = UI_SOUNDS[k];
        let mut value = 0;
        let mut ok = true;
        if ctx.last_state == state::MAIN_MENU && k == 4 {
            ok = false;
        } else if k == 80 {
            let pct = (f64::from(ctx.boost) * 100.0) as i32;
            if self.last_boost < pct {
                name = "boost_bar_increase";
            } else if pct < self.last_boost {
                name = "boost_bar_decrease";
            } else {
                ok = false;
            }
            value = pct - self.last_boost;
            self.last_boost = pct;
        }
        if ctx.audio_mode == 2 {
            if k == 4 && ctx.screen_state != state::PAUSE {
                ok = false;
            }
            if k == 0 && ctx.screen_state != state::PAUSE && !ctx.popup_menu {
                ok = false;
            }
        }
        if k == 46 && ctx.last_state == state::MAIN_MENU {
            name = "MM_show";
        } else if k == 40 && ctx.last_state <= 9 {
            name = "globe_tutorial_item";
        }
        if !ok || self.guard[k] != 0 {
            return Vec::new();
        }
        self.guard[k] = self.reload[k];
        let mut fired = Vec::new();
        for (index, &(csis, n)) in CSIS_EVENTS.iter().enumerate() {
            if n != name {
                continue;
            }
            if index == 73 {
                sfx.set_global("BoostBar_Delta", value.clamp(0, 100));
                continue;
            }
            match sfx.aems.resolve(csis, n) {
                Some(key) => {
                    sfx.fire(key, &[value]);
                    fired.push(n);
                }
                None => self.log.push(format!("ui sound {id}: {n} ({csis:#010x}) not loaded")),
            }
        }
        self.log.push(format!("ui sound {id} ({name}) → {fired:?}"));
        fired
    }
}

impl UiSounds {
    pub fn fire_posted(&mut self, sfx: &mut SfxEngine, index: usize, received: bool) -> Option<&'static str> {
        let &(csis, name) = CSIS_EVENTS.get(index)?;
        if received && index == 73 {
            sfx.set_global("BoostBar_Delta", 0);
            return None;
        }
        let fired = sfx.aems.resolve(csis, name).map(|key| {
            sfx.fire(key, &[0]);
            name
        });
        self.log.push(format!("posted sound {index} ({name}) → {fired:?}"));
        fired
    }

    pub fn tutorial_sound(&mut self, sfx: &mut SfxEngine, n: u32) -> Option<&'static str> {
        let &(_, index) = TUTORIAL_SOUNDS.iter().find(|t| t.0 == n)?;
        let (csis, name) = CSIS_EVENTS[index];
        let fired = match sfx.aems.resolve(csis, name) {
            Some(key) => {
                sfx.fire(key, &[0]);
                Some(name)
            }
            None => None,
        };
        self.log.push(format!("tutorial sound {n} ({name}) → {fired:?}"));
        fired
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn screens_map_to_states() {
        assert_eq!(screen_state(1, 1, false), 1);
        assert_eq!(screen_state(2, 1, true), 4);
        assert_eq!(screen_state(0x44, 1, true), 8);
        assert_eq!(screen_state(0x33, 1, true), 9);
        assert_eq!(screen_state(0x1E, 2, true), 0xA);
        assert_eq!(screen_state(0x37, 2, true), 0xC);
        assert_eq!(screen_state(0x37, 4, true), 0xD);
        assert_eq!(screen_state(0x3F, 1, true), 0x11);
        assert_eq!(screen_result(0x27, false, false), 3);
        assert_eq!(screen_result(0x23, false, true), 0);
        assert_eq!(screen_result(0x23, true, true), 1);
        assert_eq!(screen_result(0x26, true, false), 2);
        assert_eq!(screen_result(0x37, true, true), 0);
    }

    #[test]
    fn a_movie_start_reports_ten_transition_frames() {
        let mut f = FlowState::default();
        let i = FlowInputs { screen: 2, kind: 1, boot_done: true, ..Default::default() };
        assert_eq!(f.frame(&i).state, 4);
        f.movie_started();
        for _ in 0..10 {
            assert_eq!(f.frame(&i).state, state::TRANSITION);
        }
        assert_eq!(f.frame(&i).state, 4);
        let login = FlowInputs { screen: 0x15, ..i };
        assert_eq!(f.frame(&login).state, 4);
    }

    #[test]
    fn the_controller_mutes_the_menu_music_in_any_state() {
        let msg = |state: i32| StateMsg { state, ..Default::default() };
        for st in [1, 2, 3, 9] {
            assert!(FeMusicModule::mutes(&msg(st), true, false), "state {st}");
            assert!(!FeMusicModule::mutes(&msg(st), false, false), "state {st}");
        }
        assert!(!FeMusicModule::mutes(&msg(2), false, true) && FeMusicModule::mutes(&msg(3), false, true));
        assert!(FeMusicModule::mutes(&msg(state::TRANSITION), false, false));
        let restart = StateMsg { state: 3, restart: true, ..Default::default() };
        assert!(!FeMusicModule::mutes(&restart, true, true));
    }

    #[test]
    fn picks_avoid_the_last_piece_and_rotate() {
        let mut m = FeMusicModule::new(Source::Own(crate::career::random::Mt19937::seeded(1)));
        m.refill(70);
        assert_eq!(m.picks[0].last(), Some(&7));
        assert_eq!(m.picks[2].len(), 11);
        let mut seen = std::collections::BTreeSet::new();
        for _ in 0..12 {
            let v = m.pick(0) / 10;
            assert!(!m.picks[0][..m.picks[0].len() / 2].contains(&v));
            seen.insert(v);
        }
        assert!(seen.len() > 3);
    }

    #[test]
    fn the_message_keeps_its_screen_on_0x34_and_in_the_boot_hold() {
        let mut f = FlowState::default();
        let at = |screen| FlowInputs { screen, kind: 1, boot_done: true, ..Default::default() };
        assert_eq!(f.frame(&at(2)).screen, 2);
        assert_eq!(f.frame(&at(0x34)).screen, 2);
        assert_eq!(f.frame(&at(5)).screen, 5);
        f.boot_flow_loaded();
        for _ in 0..10 {
            assert_eq!(f.frame(&at(2)).screen, 5, "the hold");
        }
        assert_eq!(f.frame(&at(2)).screen, 2);
    }

    #[test]
    fn the_boot_hold_and_the_equipment_wait() {
        let mut f = FlowState::default();
        let eq = FlowInputs { screen: 0x30, kind: 1, boot_done: true, setting_67: true, ..Default::default() };
        let menu = FlowInputs { screen: 2, ..eq };
        f.boot_flow_loaded();
        assert_eq!(f.frame(&eq).state, state::BOOT);
        assert_eq!(f.boot_hold, 10);
        for _ in 0..10 {
            assert_eq!(f.frame(&menu).state, state::MAIN_MENU);
        }
        assert_eq!(f.frame(&eq).state, state::EQUIPMENT);
        f.launch_requested();
        let mut first_loading = None;
        for n in 0..110 {
            if f.frame(&eq).loading && first_loading.is_none() {
                first_loading = Some(n);
            }
        }
        assert_eq!(first_loading, Some(99));
        assert!(!f.frame(&FlowInputs { setting_67: false, ..eq }).loading);
        f.frame(&menu);
        assert_eq!(f.equipment_wait, 0);
    }

    #[test]
    fn the_audio_mode_and_the_watchers() {
        let mut f = FlowState::default();
        f.hint_shown(1);
        assert_eq!(f.audio_mode, 0);
        f.hint_shown(2);
        assert!(!f.popup_state(1));
        assert!(f.popup_state(2));
        assert_eq!(f.audio_mode, 2);
        f.hint_closed();
        assert_eq!(f.audio_mode, 0);
        let vals = std::cell::RefCell::new(std::collections::BTreeMap::<u16, bool>::new());
        let get = |id: u16| vals.borrow().get(&id).copied().unwrap_or(false);
        vals.borrow_mut().insert(0x226, true);
        assert_eq!(f.watch(&get), vec![4]);
        vals.borrow_mut().insert(0x222, true);
        assert_eq!(f.watch(&get), vec![3]);
        vals.borrow_mut().insert(0x226, false);
        assert_eq!(f.watch(&get), Vec::<u32>::new());
        vals.borrow_mut().insert(0x222, false);
        vals.borrow_mut().insert(0x227, true);
        assert_eq!(f.watch(&get), vec![6]);
        vals.borrow_mut().insert(0x227, false);
        assert_eq!(f.watch(&get), vec![7]);
    }
}
