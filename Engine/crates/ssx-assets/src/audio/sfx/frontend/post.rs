use super::{CSIS_EVENTS, SfxEngine, UI_SOUNDS};
use crate::sfx_aems::CsisHandle;

pub const PROMPT_MS: f32 = 30.0;
pub const BEAT_SNAP_MS: f32 = 50.0;
const SNAP_WINDOW: u32 = 50;
const MODE_DROPS: i32 = 4;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BeatStatus {
    pub beat: u32,
    pub to_beat: u32,
    pub bar: u32,
    pub to_bar: u32,
}

pub const GLOBE_WIND_FN: (u32, &str) = (0x3360_4130, "Globe");
pub const GLOBE_WIND_MAX: i32 = 3000;

pub fn post_index(id: i32) -> Option<usize> {
    let name = *UI_SOUNDS.get(usize::try_from(id).ok().filter(|&k| k != 0)?)?;
    Some(CSIS_EVENTS.iter().rposition(|&(_, n)| n == name).unwrap_or(0))
}

#[derive(Clone, Debug, Default)]
pub struct PostSlot {
    pending: Option<Pending>,
    prev_to_beat: u32,
}

#[derive(Clone, Copy, Debug)]
struct Pending {
    index: usize,
    time_ms: f64,
    delay_ms: f32,
}

impl PostSlot {
    pub fn receive(&mut self, index: usize, delay_ms: f32, time_ms: f64, now_ms: f64) -> Option<usize> {
        if delay_ms <= PROMPT_MS {
            return Some(index);
        }
        let elapsed = (now_ms - time_ms) as f32;
        if elapsed <= delay_ms {
            self.pending = Some(Pending { index, time_ms: now_ms, delay_ms: delay_ms - elapsed });
            None
        } else {
            self.pending = None;
            Some(index)
        }
    }

    pub fn update(&mut self, now_ms: f64, to_beat: Option<u32>, state: i32) -> Option<usize> {
        let mut fired = None;
        if let Some(tb) = to_beat {
            let passed = self.prev_to_beat >= SNAP_WINDOW && (tb < SNAP_WINDOW || tb > self.prev_to_beat);
            if passed && let Some(p) = self.pending.filter(|p| p.index > 0 && p.delay_ms <= BEAT_SNAP_MS) {
                self.pending = None;
                fired = (state != MODE_DROPS).then_some(p.index);
            }
            self.prev_to_beat = tb;
        }
        if let Some(p) = self.pending.as_mut().filter(|p| p.index > 0) {
            let elapsed = (now_ms - p.time_ms) as f32;
            if elapsed > p.delay_ms {
                let index = p.index;
                self.pending = None;
                fired = (state != MODE_DROPS).then_some(index);
            } else {
                p.time_ms = now_ms;
                p.delay_ms -= elapsed;
            }
        }
        fired
    }

    pub fn waiting(&self) -> bool {
        self.pending.is_some()
    }
}

#[derive(Clone, Debug, Default)]
pub struct GlobeWind {
    call: Option<CsisHandle>,
}

impl GlobeWind {
    pub fn state_message(&mut self, sfx: &mut SfxEngine, wind: i32) -> Option<String> {
        let param = wind.clamp(0, GLOBE_WIND_MAX);
        let mut line = None;
        if self.call.is_none() && wind != 0 {
            let handle =
                sfx.aems.resolve(GLOBE_WIND_FN.0, GLOBE_WIND_FN.1).map_or(0, |key| sfx.call_key(key, &[param]));
            self.call = Some(handle);
            line = Some(format!("globe wind: Globe [{param}] → handle {handle}"));
        }
        if let Some(h) = self.call {
            if h != 0 {
                sfx.update(h, &[param]);
            }
            if wind == 0 {
                if h != 0 {
                    sfx.stop(h);
                }
                self.call = None;
                line = Some(format!("globe wind: stop (handle {h})"));
            }
        }
        line
    }

    pub fn blowing(&self) -> bool {
        self.call.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_last_csis_entry_of_the_name() {
        let id = UI_SOUNDS.iter().position(|&n| n == "globe_transition_end").expect("0x3D") as i32;
        assert_eq!(id, 0x3D);
        let index = post_index(id).expect("posted");
        assert_eq!(CSIS_EVENTS[index].1, "globe_transition_end");
        assert!(CSIS_EVENTS[index + 1..].iter().all(|&(_, n)| n != "globe_transition_end"));
        assert_eq!(post_index(0), None, "0x82510F20 posts nothing for id 0");
        assert_eq!(post_index(120), None);
    }

    #[test]
    fn a_long_delay_waits_and_snaps_to_the_beat() {
        let mut s = PostSlot::default();
        assert_eq!(s.receive(7, 0.0, 0.0, 0.0), Some(7), "30 ms or less fires now");
        assert_eq!(s.receive(7, 500.0, 1000.0, 1000.0), None);
        assert!(s.waiting());
        assert_eq!(s.update(1400.0, Some(300), 0), None);
        assert_eq!(s.update(1440.0, Some(60), 0), None);
        assert_eq!(s.update(1450.0, Some(45), 0), None);
        assert_eq!(s.update(1460.0, Some(500), 0), None);
        assert_eq!(s.update(1470.0, Some(20), 0), Some(7), "the beat passed with 40 ms left");
        assert!(!s.waiting());
        assert_eq!(s.receive(9, 100.0, 3000.0, 3000.0), None);
        assert_eq!(s.update(3150.0, None, 0), Some(9));
        assert_eq!(s.receive(9, 100.0, 4000.0, 4000.0), None);
        assert_eq!(s.update(4150.0, None, 4), None);
        assert!(!s.waiting());
        assert_eq!(s.receive(9, 100.0, 5000.0, 5200.0), Some(9));
    }
}
