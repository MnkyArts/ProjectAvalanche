use super::*;

pub mod fe_event {
    pub const BOOT: u32 = 0x01FA_81B6;
    pub const WELCOME_HUB: u32 = 0x01D1_6D4B;
    pub const MENU: u32 = 0x0180_28C5;
    pub const EQUIPMENT: u32 = 0x014B_FF95;
    pub const LOADING: u32 = 0x0185_CD6C;
    pub const STOP: u32 = 0x0141_3AD3;
}

const STREAM1: u32 = 0x1100_0001;
const STREAM2: u32 = 0x1100_0002;

pub const PICKS: [usize; 3] = [12, 12, 11];

#[derive(Clone, Debug)]
pub struct FeMusicModule {
    loaded: bool,
    load_requested: bool,
    play: u8,
    muted: bool,
    pub mode: u32,
    last_state: i32,
    restart: bool,
    quit: bool,
    pending: [u32; 2],
    pub(super) picks: [Vec<u32>; 3],
    pub rng: Source,
    pub log: Vec<String>,
}

impl FeMusicModule {
    pub fn new(rng: Source) -> Self {
        Self {
            loaded: false,
            load_requested: false,
            play: 0,
            muted: false,
            mode: 0,
            last_state: 0,
            restart: false,
            quit: false,
            pending: [0; 2],
            picks: Default::default(),
            rng,
            log: Vec::new(),
        }
    }

    fn wanted(&self) -> bool {
        (1..=9).contains(&self.last_state) || self.restart || self.quit
    }

    fn silent(rt: &crate::pathfinder::Pathfinder) -> bool {
        (0..2).all(|i| rt.stream_state(i) < 2)
    }

    fn busy(rt: &crate::pathfinder::Pathfinder) -> bool {
        (0..2).all(|i| matches!(rt.stream_state(i), 2 | 6))
    }

    fn post_on(&mut self, rt: &mut crate::pathfinder::Pathfinder, k: usize, event: u32) {
        if Self::busy(rt) {
            self.pending[k] = event;
            self.log.push(format!("defer {event:#010x} (stream {})", k + 1));
            return;
        }
        if !self.wanted() || !self.loaded {
            return;
        }
        rt.cancel_events();
        rt.post(event);
        self.play = 2;
        self.log.push(format!("post {event:#010x}"));
    }

    pub fn update(&mut self, rt: &mut crate::pathfinder::Pathfinder) {
        if self.load_requested && !self.loaded {
            self.loaded = true;
        }
        for k in 0..2 {
            if self.pending[k] != 0 && !Self::busy(rt) {
                let ev = std::mem::take(&mut self.pending[k]);
                self.post_on(rt, k, ev);
            }
        }
    }

    fn mute(&mut self, rt: &mut crate::pathfinder::Pathfinder, on: bool) {
        if on == self.muted || !self.loaded {
            return;
        }
        if !on {
            if !self.wanted() {
                return;
            }
            if self.play == 3 {
                rt.set_paused(false);
                self.play = 2;
            }
        } else if !Self::silent(rt) && self.play != 4 {
            if self.play == 2 {
                rt.set_paused(true);
                self.play = 3;
            }
        }
        self.muted = on;
        self.log.push((if on { "muted" } else { "unmuted" }).to_string());
    }

    pub(super) fn refill(&mut self, fe7: u32) {
        if self.picks.iter().all(|l| l.len() >= 4) {
            return;
        }
        let excl = [(fe7 & 0xFFFF).min(0x78), (fe7 >> 16).min(0x78), 0];
        for (k, list) in self.picks.iter_mut().enumerate() {
            let n = PICKS[k];
            list.clear();
            list.extend(0..n as u32);
            for _ in 0..list.len() {
                let i = self.rng.below(list.len() as u32 - 1) as usize;
                let j = self.rng.below(list.len() as u32 - 1) as usize;
                list.swap(i, j);
            }
            for i in 0..n.saturating_sub(1) {
                if list[i] == excl[k] / 10 {
                    list.swap(i, n - 1);
                }
            }
        }
    }

    pub(super) fn pick(&mut self, k: usize) -> u32 {
        let list = &mut self.picks[k];
        let i = self.rng.below((list.len() / 2) as u32) as usize;
        let v = list.remove(i);
        list.push(v);
        v * 10
    }

    fn picks_word(&self) -> u32 {
        let last = |k: usize| self.picks[k].last().copied().unwrap_or(0);
        (last(1) * 0xA_0000) | (last(0) * 10)
    }

    pub fn mutes(m: &StateMsg, controller_playing: bool, menu_playlist: bool) -> bool {
        let mut mute = controller_playing || (menu_playlist && m.state > 2);
        if m.state == state::TRANSITION {
            mute = true;
        }
        mute && !(m.restart || m.quit)
    }

    pub fn state_msg(
        &mut self,
        rt: &mut crate::pathfinder::Pathfinder,
        m: &StateMsg,
        settings: &mut AudioSettings,
        controller_playing: bool,
    ) {
        let st = m.state;
        let mute = Self::mutes(m, controller_playing, false);
        if mute {
            self.mute(rt, true);
        }
        self.last_state = st;
        if !mute {
            self.mute(rt, false);
        }
        let fe = (1..=9).contains(&st);
        let mut want = self.mode;
        if fe && !self.loaded && !m.loading {
            self.load_requested = true;
            want = 0;
        } else if fe {
            if self.loaded {
                let mut next = want;
                if st < 2 {
                    next = 1;
                }
                if st == 3 {
                    want = 2;
                } else {
                    if matches!(st, 4..=6) {
                        next = if want == 4 && st != 4 { 4 } else { 3 };
                    }
                    if matches!(st, 7 | 8) {
                        next = 3 + u32::from(want == 4);
                    }
                    want = next;
                    if st == 9 {
                        want = 4 + u32::from(m.loading);
                    }
                }
            }
        } else if st != state::TRANSITION {
            want = if m.loading { 5 } else { 0 };
        }
        let restart_edge = !self.restart && m.restart;
        let quit_edge = !self.quit && m.quit;
        (self.restart, self.quit) = (m.restart, m.quit);
        if !self.wanted() && st != state::TRANSITION && !self.restart && !self.quit {
            want = 0;
        }
        if restart_edge {
            (want, self.mode) = (5, 0);
        }
        if self.restart {
            want = 5;
        }
        if quit_edge {
            (want, self.mode) = (4, 0);
        }
        if self.quit {
            want = 4;
        }
        if !self.loaded || !self.wanted() || self.muted || want == self.mode {
            return;
        }
        match want {
            1 => self.post_on(rt, 0, fe_event::BOOT),
            2 => self.post_on(rt, 0, fe_event::WELCOME_HUB),
            3 => {
                self.refill(settings.0[FE_MUSIC_PICKS] as u32);
                let first = self.mode < 3;
                let mut r = self.pick(0);
                while (first && r < 80) || (!first && r > 79) {
                    r = self.pick(0);
                }
                rt.set_player_level(STREAM2, r + 10);
                self.post_on(rt, 1, fe_event::MENU);
                settings.0[FE_MUSIC_PICKS] = self.picks_word() as i32;
            }
            4 => {
                self.refill(settings.0[FE_MUSIC_PICKS] as u32);
                let r = self.pick(1);
                rt.set_player_level(STREAM1, r + 10);
                self.post_on(rt, 0, fe_event::EQUIPMENT);
                settings.0[FE_MUSIC_PICKS] = self.picks_word() as i32;
            }
            5 => {
                self.refill(settings.0[FE_MUSIC_PICKS] as u32);
                let r = self.pick(2);
                rt.set_player_level(STREAM2, r + 10);
                self.post_on(rt, 1, fe_event::LOADING);
            }
            _ => {}
        }
        self.log.push(format!("mode {} → {want} (state {st:#x})", self.mode));
        self.mode = want;
    }

    pub fn gameplay_start(&mut self, rt: &mut crate::pathfinder::Pathfinder) {
        if !Self::silent(rt) {
            if self.play == 2 {
                if self.loaded && self.wanted() {
                    rt.post(fe_event::STOP);
                    self.log.push("stop".into());
                }
                self.play = 4;
                return;
            }
            if (0..2).all(|i| rt.stream_state(i) == 2) {
                rt.stop_all();
                self.log.push("stopped at once (buffering)".into());
                self.play = 4;
                return;
            }
            if self.play != 1 {
                return;
            }
        }
        self.play = 0;
    }

    pub fn awake(&self, rt: &crate::pathfinder::Pathfinder) -> bool {
        !Self::silent(rt) && !self.muted
    }

    pub fn finished(&self, rt: &crate::pathfinder::Pathfinder) -> bool {
        Self::silent(rt) && self.pending == [0; 2]
    }
}

pub struct FrontEndMusic {
    pub music: PathfinderMusic,
    pub module: FeMusicModule,
}

impl FrontEndMusic {
    pub fn load(big_root: &Path, rng: Source) -> Result<Self> {
        Ok(Self { music: PathfinderMusic::load(big_root, "ssx_fe", &[])?, module: FeMusicModule::new(rng) })
    }

    pub fn awake(&self) -> bool {
        self.module.awake(&self.music.rt)
    }

    pub fn beat_status(&self) -> Option<super::BeatStatus> {
        let s = self.music.rt.first_status()?;
        Some(super::BeatStatus { beat: s.beat, to_beat: s.to_beat, bar: s.bar, to_bar: s.to_bar })
    }

    pub fn state_msg(&mut self, m: &StateMsg, settings: &mut AudioSettings) {
        self.module.state_msg(&mut self.music.rt, m, settings, false);
        self.module.update(&mut self.music.rt);
    }

    pub fn gameplay_start(&mut self) {
        self.module.gameplay_start(&mut self.music.rt);
    }

    pub fn finished(&self) -> bool {
        self.module.finished(&self.music.rt)
    }
}
