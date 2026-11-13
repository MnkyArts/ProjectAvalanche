#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Beat {
    pub beat: u32,
    pub to_beat: u32,
    pub bar: u32,
    pub to_bar: u32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Post {
    pub id: i32,
    pub delay_ms: f32,
}

const WELCOME_HUB: u32 = crate::game::fe::screens::id::WELCOME_HUB;
const GLOBE_INTRO: i32 = 0x3E;
const RANGE_TRANSIT: i32 = 0x3A;
const TRANSITION_END: i32 = 0x3D;
const TRANSIT_LEAD: f32 = 0.6;
const END_CORRECTION: f32 = 0.033_333_34;
const BEAT_STALE_MS: f32 = 1000.0;
const WIND_STATES: [i32; 5] = [9, 0xB, 0xC, 0xE, 0x12];
const WIND_CUT: f32 = 0.8;
const WIND_ARMS: f32 = 0.1;
const WIND_HOLD: f32 = 0.2;

#[derive(Clone, Debug)]
pub struct FeAudio {
    pub screen: u32,
    pub state: i32,
    beat: Beat,
    stamp_ms: f64,
    clock: bool,
    pub globe_state: i32,
    pub synced: f32,
    last_progress: f32,
    wind: f32,
    wind_hold: f32,
    pub wind_param: i32,
    now_ms: f64,
    pub posts: Vec<Post>,
}

impl Default for FeAudio {
    fn default() -> Self {
        FeAudio {
            screen: 0,
            state: 0,
            beat: Beat::default(),
            stamp_ms: 0.0,
            clock: false,
            globe_state: 0,
            synced: 0.0,
            last_progress: 0.0,
            wind: 0.0,
            wind_hold: WIND_HOLD,
            wind_param: 0,
            now_ms: 0.0,
            posts: Vec::new(),
        }
    }
}

impl FeAudio {
    pub fn beat_message(&mut self, beat: Beat) {
        self.beat = beat;
        self.stamp_ms = self.now_ms;
        self.clock = true;
    }

    pub fn adopt(&mut self, screen: u32, state: i32) {
        self.screen = screen;
        self.state = state;
    }

    fn post(&mut self, id: i32, delay_ms: f32) {
        if (self.screen == WELCOME_HUB && id != GLOBE_INTRO) || id == 0 || !self.clock {
            return;
        }
        self.posts.push(Post { id, delay_ms });
    }

    pub fn globe_state(&mut self, state: u32, plays: &mut Vec<i32>) {
        let (play, post): (Option<i32>, Option<i32>) = match state {
            5 => (None, Some(GLOBE_INTRO)),
            7 => (None, Some(0x3F)),
            0xC => (Some(0x45), Some(0x39)),
            0xD => (None, Some(RANGE_TRANSIT)),
            0xE => (Some(4), Some(0x3B)),
            0x10 => (Some(0x47), Some(0x3C)),
            0x12 => (Some(4), Some(0x38)),
            0x14 => (Some(0x49), None),
            _ => (None, None),
        };
        plays.extend(play);
        if let Some(id) = post {
            self.post(id, 0.0);
        }
        let state = i32::try_from(state).unwrap_or(i32::MAX);
        self.globe_state = if self.globe_state == 0xB && state == 0xB { 0 } else { state };
        self.last_progress = 0.0;
    }

    pub fn highlight(&mut self, n: i32, plays: &mut Vec<i32>) {
        match n {
            1 => plays.push(0x44),
            2 => {
                plays.push(0x46);
                self.post(0x37, 0.0);
                self.globe_state = 0x16;
            }
            3 => plays.push(0x48),
            _ => {}
        }
    }

    pub fn beat_sync(&mut self, total: f32, window: f32) -> f32 {
        let beat = self.beat.beat as f32;
        let total_ms = total * 1000.0;
        if beat < 1.0 {
            return total;
        }
        let mut at = total_ms;
        if self.clock {
            let age = (self.now_ms - self.stamp_ms) as f32;
            if age > BEAT_STALE_MS {
                return total;
            }
            let to_beat = self.beat.to_beat as f32;
            at = if age <= to_beat { to_beat - age } else { beat };
            if at < total_ms {
                let mut next = at + beat;
                while next < total_ms {
                    at += beat;
                    next += beat;
                }
                if next - total_ms <= total_ms - at {
                    at = next;
                }
            }
        }
        let mut r = at * 0.001;
        if r < total - window {
            r = total - window;
        }
        if total + window < r {
            r = total + window;
        }
        self.synced = r;
        r
    }

    pub fn transition_end(&mut self, total: f32) {
        let now = (self.state == 6 && self.globe_state == 0xC)
            || (self.state == 7 && self.globe_state == 0xE)
            || self.globe_state == 9;
        if now {
            self.post(TRANSITION_END, total * 1000.0 - END_CORRECTION);
        }
    }

    pub fn shot_step(&mut self, progress: f32, duration: f32, dt: f32) {
        if self.globe_state == 9 && self.last_progress <= 0.001 {
            self.transition_end(duration - progress * duration);
        }
        let mut wind = 0.0;
        if WIND_STATES.contains(&self.globe_state) && progress > self.last_progress {
            let rate = (progress - self.last_progress) / (dt / duration);
            let rate = if -rate >= 0.0 { 0.0 } else { rate };
            wind = if 1.0 - rate >= 0.0 { rate } else { 1.0 };
            if self.globe_state == 0xB && progress > WIND_CUT {
                wind = 0.0;
            }
        }
        self.wind = wind;
        self.last_progress = progress;
        if wind > WIND_ARMS {
            self.wind_hold = WIND_HOLD;
        }
    }

    pub fn update(&mut self, dt: f32) {
        self.now_ms += f64::from(dt) * 1000.0;
        let held = self.wind_hold > 0.0;
        let left = self.wind_hold - dt;
        self.wind_hold = if left > 0.0 { left.min(WIND_HOLD) } else { 0.0 };
        self.wind_param = if held { (self.wind * 1000.0) as i32 } else { 0 };
        if self.synced > 0.0 {
            if self.synced >= TRANSIT_LEAD && self.synced - dt < TRANSIT_LEAD && self.globe_state == 0xC {
                self.post(RANGE_TRANSIT, 0.0);
                self.globe_state = 0;
            }
            self.synced -= dt;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MAIN_MENU: u32 = crate::game::fe::screens::id::MAIN_MENU;

    fn ready(screen: u32, state: i32) -> FeAudio {
        let mut a = FeAudio::default();
        a.adopt(screen, state);
        a.beat_message(Beat { beat: 500, to_beat: 300, bar: 2000, to_bar: 300 });
        a
    }

    #[test]
    fn nothing_is_posted_before_the_first_beat_message() {
        let mut a = FeAudio::default();
        let mut plays = Vec::new();
        a.globe_state(0xC, &mut plays);
        assert_eq!(plays, [0x45], "the play goes on");
        assert!(a.posts.is_empty());
        assert_eq!(a.beat_sync(1.0, 1.0), 1.0, "no beat: the length as it is");
    }

    #[test]
    fn the_welcome_hub_takes_only_the_globe_intro() {
        let mut a = ready(WELCOME_HUB, 3);
        let mut plays = Vec::new();
        for state in [5, 7, 0xC, 0xD, 0x12] {
            a.globe_state(state, &mut plays);
        }
        a.highlight(2, &mut plays);
        assert_eq!(plays, [0x45, 4, 0x46]);
        assert_eq!(a.posts, [Post { id: GLOBE_INTRO, delay_ms: 0.0 }]);
        assert_eq!(a.globe_state, 0x16, "the peak's highlight");
    }

    #[test]
    fn the_same_wait_state_twice_clears_it() {
        let mut a = ready(MAIN_MENU, 4);
        a.globe_state(0xB, &mut Vec::new());
        assert_eq!(a.globe_state, 0xB);
        a.globe_state(0xB, &mut Vec::new());
        assert_eq!(a.globe_state, 0);
    }

    #[test]
    fn the_queue_snaps_to_the_nearest_beat() {
        let mut a = ready(MAIN_MENU, 4);
        assert!((a.beat_sync(1.0, 1.0) - 0.8).abs() < 1e-6);
        assert!((a.synced - 0.8).abs() < 1e-6);
        assert!((a.beat_sync(1.1, 1.0) - 1.3).abs() < 1e-6);
        assert!((a.beat_sync(0.55, 1.0) - 0.8).abs() < 1e-6);
        let mut b = FeAudio::default();
        b.beat_message(Beat { beat: 2000, to_beat: 1900, bar: 8000, to_bar: 1900 });
        assert!((b.beat_sync(0.25, 0.1) - 0.35).abs() < 1e-6);
        a.update(0.1);
        assert!((a.beat_sync(0.25, 1.0) - 0.2).abs() < 1e-6);
        a.update(1.0);
        assert_eq!(a.beat_sync(0.9, 1.0), 0.9);
    }

    #[test]
    fn the_range_transition_posts_its_end_and_the_transit() {
        let mut a = ready(MAIN_MENU, 6);
        a.globe_state(0xC, &mut Vec::new());
        a.posts.clear();
        let total = a.beat_sync(1.0, 1.0);
        a.transition_end(total);
        assert_eq!(a.posts, [Post { id: TRANSITION_END, delay_ms: total * 1000.0 - END_CORRECTION }]);
        a.posts.clear();
        let mut t = 0.0;
        while a.synced > 0.0 {
            a.update(1.0 / 30.0);
            t += 1.0 / 30.0;
            if !a.posts.is_empty() {
                assert_eq!(a.posts, [Post { id: RANGE_TRANSIT, delay_ms: 0.0 }]);
                assert!((total - t - TRANSIT_LEAD).abs() <= 1.0 / 30.0 + 1e-4, "at {t}");
                assert_eq!(a.globe_state, 0);
                a.posts.clear();
            }
        }
        assert_eq!(a.globe_state, 0, "posted");
        let mut b = ready(MAIN_MENU, 7);
        b.globe_state(0xC, &mut Vec::new());
        b.posts.clear();
        b.transition_end(1.0);
        assert!(b.posts.is_empty());
    }

    #[test]
    fn state_9_posts_the_end_at_its_first_step() {
        let mut a = ready(MAIN_MENU, 4);
        a.globe_state(9, &mut Vec::new());
        a.posts.clear();
        a.shot_step(0.25, 2.0, 1.0 / 30.0);
        assert_eq!(a.posts, [Post { id: TRANSITION_END, delay_ms: 1500.0 - END_CORRECTION }]);
        a.shot_step(0.5, 2.0, 1.0 / 30.0);
        assert_eq!(a.posts.len(), 1, "only while the last progress is at most 0.001");
    }

    #[test]
    fn the_wind_follows_the_shots_rate_and_holds() {
        let mut a = ready(MAIN_MENU, 6);
        a.globe_state(0xC, &mut Vec::new());
        a.shot_step(0.5, 1.0, 0.25);
        a.update(1.0 / 30.0);
        assert_eq!(a.wind_param, 1000);
        a.shot_step(0.625, 1.0, 0.25);
        a.update(1.0 / 30.0);
        assert_eq!(a.wind_param, 500);
        a.shot_step(0.625, 1.0, 0.25);
        a.update(1.0 / 30.0);
        assert_eq!(a.wind_param, 0);
        for _ in 0..7 {
            a.update(1.0 / 30.0);
        }
        a.wind = 0.5;
        a.update(1.0 / 30.0);
        assert_eq!(a.wind_param, 0, "the hold ran out");
        let mut b = ready(MAIN_MENU, 4);
        b.globe_state(0xB, &mut Vec::new());
        b.shot_step(0.85, 1.0, 1.0 / 30.0);
        assert_eq!(b.wind, 0.0);
        b.globe_state(0x10, &mut Vec::new());
        b.shot_step(0.2, 1.0, 1.0 / 30.0);
        assert_eq!(b.wind, 0.0);
    }
}
