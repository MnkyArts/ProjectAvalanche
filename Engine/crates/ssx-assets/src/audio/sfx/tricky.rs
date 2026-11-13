use crate::sfx_aems::{Aems, CsisHandle};

pub const TRICKY_OFF: i32 = 0;
pub const TRICKY_STANDARD: i32 = 1;
pub const TRICKY_ALWAYS: i32 = 2;

const SURVIVE_IT: i32 = 2;
const DEADLY_DESCENT: i32 = 3;

pub const STATE_PAUSE: i32 = 0xA;

pub const TRICKY_FN: (u32, &str) = (0x004D_5326, "Tricky");
pub const TRICKY_SAMPLE_END: (u32, &str) = (0x004D_52FC, "TrickySampleEnd");

#[derive(Clone, Debug)]
pub struct TrickySong {
    pub race_event_type: i32,
    handle: Option<CsisHandle>,
    pub enabled: bool,
    paused: bool,
    countdown: u32,
    ducking: bool,
    pub playing: i32,
    super_tricky: bool,
    last_setting: i32,
    rewinding: bool,
    sample_end: i32,
    pub log: Vec<String>,
}

impl Default for TrickySong {
    fn default() -> Self {
        Self::new(TRICKY_STANDARD)
    }
}

impl TrickySong {
    pub fn clip_playing(&self) -> bool {
        matches!(self.playing, 1 | 3 | 5)
    }

    pub fn new(setting: i32) -> Self {
        Self {
            race_event_type: -1,
            handle: None,
            enabled: false,
            paused: false,
            countdown: 0,
            ducking: false,
            playing: -1,
            super_tricky: false,
            last_setting: setting,
            rewinding: false,
            sample_end: 0,
            log: Vec::new(),
        }
    }

    fn stop(&mut self, aems: &mut Aems) {
        if let Some(h) = self.handle.take() {
            aems.stop(h);
            self.log.push(format!("stop (was {})", self.playing));
        }
        self.playing = -1;
    }

    fn start(&mut self, aems: &mut Aems, param: i32) {
        self.stop(aems);
        if self.race_event_type == DEADLY_DESCENT || self.rewinding {
            return;
        }
        let Some(key) = aems.resolve(TRICKY_FN.0, TRICKY_FN.1) else {
            self.log.push("no CSIS function Tricky (hudsfx.csi not loaded)".into());
            return;
        };
        self.handle = Some(aems.call(key, &[param.clamp(0, 10)]));
        self.playing = param;
        self.log.push(format!("Tricky [{param}]"));
    }

    pub fn post(&mut self, aems: &mut Aems, name: &str, value: f32, setting: i32) {
        if name == "RaceEventType" {
            self.race_event_type = value as i32;
            return;
        }
        if !self.enabled {
            return;
        }
        let survive = self.race_event_type == SURVIVE_IT;
        match name {
            "AwesomeModeStart" => self.start(aems, if survive { 2 } else { 0 }),
            "AwesomeModeStop" => self.stop(aems),
            "SuperAwesomeModeStart" => {
                let param = match (survive, setting) {
                    (true, _) => 3,
                    (false, TRICKY_OFF) => 0,
                    (false, TRICKY_ALWAYS) => 5,
                    (false, _) => 1,
                };
                self.start(aems, param);
                self.super_tricky = true;
            }
            "SuperAwesomeModeStop" => {
                self.stop(aems);
                self.super_tricky = false;
            }
            _ => {}
        }
    }

    pub fn rewind(&mut self, aems: &mut Aems, rewinding: bool) {
        if !self.enabled {
            return;
        }
        if rewinding && !self.rewinding {
            self.stop(aems);
            self.rewinding = true;
        } else if !rewinding && self.rewinding {
            self.rewinding = false;
        }
    }

    pub fn game_state(&mut self, state: i32) {
        if state == STATE_PAUSE {
            self.paused = true;
        } else if self.paused {
            self.paused = false;
            self.countdown = 5;
        }
    }

    pub fn results(&mut self, aems: &mut Aems) {
        self.super_tricky = false;
        self.stop(aems);
    }

    pub fn reset(&mut self, aems: &mut Aems) {
        self.stop(aems);
    }

    pub fn sample_end(&mut self, value: i32) {
        self.sample_end = value;
    }

    pub fn frame(&mut self, aems: &mut Aems, setting: i32, menu_sfx: i32) -> Option<bool> {
        if self.sample_end == 1 {
            self.playing = -1;
            self.sample_end = -1;
            self.log.push("TrickySampleEnd".into());
        }
        self.countdown = self.countdown.saturating_sub(1);
        if self.super_tricky && self.race_event_type != SURVIVE_IT && !self.paused && self.countdown == 0 {
            if setting == self.last_setting {
                if setting == TRICKY_ALWAYS && self.playing == -1 && !self.rewinding {
                    self.start(aems, 5);
                }
            } else {
                match setting {
                    TRICKY_OFF if matches!(self.playing, 1 | 5) => self.stop(aems),
                    TRICKY_STANDARD if matches!(self.playing, 5 | -1) => self.start(aems, 1),
                    TRICKY_ALWAYS if matches!(self.playing, 1 | -1) => self.start(aems, 5),
                    _ => {}
                }
            }
            self.last_setting = setting;
        }
        if matches!(self.playing, 1 | 5) {
            if !self.ducking && menu_sfx > 0 {
                self.ducking = true;
                return Some(true);
            }
            if self.ducking && menu_sfx == 0 {
                self.ducking = false;
                return Some(false);
            }
            None
        } else if self.ducking {
            self.ducking = false;
            Some(false)
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ssx_formats::csi::{CsiFile, Entry, Kind};

    fn aems() -> Aems {
        let mut a = Aems::new();
        let entry = |kind, name: &str, id| Entry { kind, name: name.into(), id, default: 0 };
        a.add_interface(&CsiFile {
            unk_04: [0; 6],
            interface: 0x4D,
            entries: vec![
                entry(Kind::Function, TRICKY_FN.1, TRICKY_FN.0),
                entry(Kind::Global, TRICKY_SAMPLE_END.1, TRICKY_SAMPLE_END.0),
            ],
        });
        a
    }

    fn race(setting: i32) -> (Aems, TrickySong) {
        let mut t = TrickySong::new(setting);
        let mut a = aems();
        t.post(&mut a, "RaceEventType", 1.0, setting);
        t.enabled = true;
        (a, t)
    }

    #[test]
    fn tricky_plays_the_stinger_super_tricky_the_song_and_ducks() {
        let (mut a, mut t) = race(TRICKY_STANDARD);
        t.post(&mut a, "AwesomeModeStart", 0.0, TRICKY_STANDARD);
        assert_eq!(t.playing, 0);
        assert_eq!(t.frame(&mut a, TRICKY_STANDARD, 10), None);
        t.post(&mut a, "SuperAwesomeModeStart", 0.0, TRICKY_STANDARD);
        assert_eq!(t.playing, 1);
        assert_eq!(t.frame(&mut a, TRICKY_STANDARD, 10), Some(true));
        t.sample_end(1);
        assert_eq!(t.frame(&mut a, TRICKY_STANDARD, 10), Some(false));
        assert_eq!(t.playing, -1);
        assert_eq!(t.frame(&mut a, TRICKY_STANDARD, 10), None);
        assert_eq!(t.playing, -1);
    }

    #[test]
    fn always_restarts_the_loop_and_settings_switch_the_song() {
        let (mut a, mut t) = race(TRICKY_ALWAYS);
        t.post(&mut a, "SuperAwesomeModeStart", 0.0, TRICKY_ALWAYS);
        assert_eq!(t.playing, 5);
        t.sample_end(1);
        t.frame(&mut a, TRICKY_ALWAYS, 10);
        assert_eq!(t.playing, 5);
        t.frame(&mut a, TRICKY_STANDARD, 10);
        assert_eq!(t.playing, 1);
        t.frame(&mut a, TRICKY_OFF, 10);
        assert_eq!(t.playing, -1);
        t.post(&mut a, "SuperAwesomeModeStop", 0.0, TRICKY_OFF);
        t.post(&mut a, "SuperAwesomeModeStart", 0.0, TRICKY_OFF);
        assert_eq!(t.playing, 0);
    }

    #[test]
    fn survive_deadly_descent_pause_and_rewind() {
        let (mut a, mut t) = race(TRICKY_STANDARD);
        t.post(&mut a, "RaceEventType", 2.0, TRICKY_STANDARD);
        t.post(&mut a, "AwesomeModeStart", 0.0, TRICKY_STANDARD);
        assert_eq!(t.playing, 2);
        t.post(&mut a, "SuperAwesomeModeStart", 0.0, TRICKY_STANDARD);
        assert_eq!(t.playing, 3);
        t.post(&mut a, "RaceEventType", 3.0, TRICKY_STANDARD);
        t.post(&mut a, "SuperAwesomeModeStart", 0.0, TRICKY_STANDARD);
        assert_eq!(t.playing, -1);
        let (mut a, mut t) = race(TRICKY_STANDARD);
        t.post(&mut a, "SuperAwesomeModeStart", 0.0, TRICKY_STANDARD);
        t.game_state(STATE_PAUSE);
        t.frame(&mut a, TRICKY_OFF, 10);
        assert_eq!(t.playing, 1);
        t.game_state(0xB);
        for _ in 0..4 {
            t.frame(&mut a, TRICKY_OFF, 10);
            assert_eq!(t.playing, 1);
        }
        t.frame(&mut a, TRICKY_OFF, 10);
        assert_eq!(t.playing, -1);
        let (mut a, mut t) = race(TRICKY_ALWAYS);
        t.post(&mut a, "SuperAwesomeModeStart", 0.0, TRICKY_ALWAYS);
        t.rewind(&mut a, true);
        assert_eq!(t.playing, -1);
        t.frame(&mut a, TRICKY_ALWAYS, 10);
        assert_eq!(t.playing, -1);
        t.rewind(&mut a, false);
        t.frame(&mut a, TRICKY_ALWAYS, 10);
        assert_eq!(t.playing, 5);
    }

    #[test]
    fn the_ducker_follows_the_menu_sfx_volume() {
        let (mut a, mut t) = race(TRICKY_STANDARD);
        t.post(&mut a, "SuperAwesomeModeStart", 0.0, TRICKY_STANDARD);
        assert_eq!(t.frame(&mut a, TRICKY_STANDARD, 0), None);
        assert_eq!(t.frame(&mut a, TRICKY_STANDARD, 3), Some(true));
        assert_eq!(t.frame(&mut a, TRICKY_STANDARD, 0), Some(false));
        t.results(&mut a);
        assert_eq!(t.playing, -1);
        assert_eq!(t.frame(&mut a, TRICKY_STANDARD, 10), None);
    }
}
