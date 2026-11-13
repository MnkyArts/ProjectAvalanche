#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Tier {
    Near,
    Middle,
    Far,
}

// PORT(choice): 150 m and 400 m, chosen by S0.2 from the error per rate and the pixels per metre at the game's field of
// view (57…90°); tuning: the tier distances seen in real sessions over the internet; the original has no real-time
// multiplayer.
pub const NEAR_M: f32 = 150.0;
pub const MIDDLE_M: f32 = 400.0;

pub const SEND_HZ: f32 = 15.0;

impl Tier {
    pub fn at(distance: Option<f32>) -> Self {
        match distance {
            Some(d) if d > MIDDLE_M => Self::Far,
            Some(d) if d > NEAR_M => Self::Middle,
            _ => Self::Near,
        }
    }

    pub fn interval_ticks(self) -> f64 {
        match self {
            Self::Near => 2.0,
            Self::Middle => 6.0,
            Self::Far => 10.5,
        }
    }

    // PORT(choice): 4 / 2 / 1, ordering the tiers only (lightyear sends by priority when a bandwidth quota is set; the
    // server sets none until phase 4's limits); tuning: phase 4's per-client bandwidth limit measured with 32 riders;
    // the original has no real-time multiplayer.
    pub fn priority(self) -> f32 {
        match self {
            Self::Near => 4.0,
            Self::Middle => 2.0,
            Self::Far => 1.0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TierRates {
    pub near: f64,
    pub middle: f64,
    pub far: f64,
}

impl Default for TierRates {
    fn default() -> Self {
        Self {
            near: Tier::Near.interval_ticks(),
            middle: Tier::Middle.interval_ticks(),
            far: Tier::Far.interval_ticks(),
        }
    }
}

impl TierRates {
    pub fn with_near(ticks: f64) -> Self {
        Self { near: ticks.max(1.0), ..Self::default() }
    }

    pub fn interval(&self, tier: Tier) -> f64 {
        match tier {
            Tier::Near => self.near,
            Tier::Middle => self.middle,
            Tier::Far => self.far,
        }
    }
}

pub fn distance(a: [f32; 3], b: [f32; 3]) -> f32 {
    (0..3).map(|i| (a[i] - b[i]).powi(2)).sum::<f32>().sqrt()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SampleOrder {
    pub epoch: u8,
    pub execution: u32,
    pub tick: u32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Decimator {
    due: Option<f64>,
    at: f64,
    last: Option<SampleOrder>,
}

impl Decimator {
    pub fn admit(&mut self, s: SampleOrder, tier: Tier) -> bool {
        self.admit_every(s, tier.interval_ticks())
    }

    pub fn admit_every(&mut self, s: SampleOrder, interval: f64) -> bool {
        let step = self.last.filter(|l| l.epoch == s.epoch).map(|l| match s.execution.cmp(&l.execution) {
            core::cmp::Ordering::Equal => f64::from(s.tick.saturating_sub(l.tick)),
            _ => f64::from(s.execution.saturating_sub(l.execution)),
        });
        self.last = Some(s);
        let Some(step) = step else {
            self.at = 0.0;
            self.due = Some(interval);
            return true;
        };
        self.at += step;
        let t = self.at;
        match self.due {
            Some(due) if t < due => false,
            Some(due) if t < due + interval => {
                self.due = Some(due + interval);
                true
            }
            _ => {
                self.due = Some(t + interval);
                true
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(execution: u32, tick: u32) -> SampleOrder {
        SampleOrder { epoch: 0, execution, tick }
    }

    #[test]
    fn the_tiers_relay_their_rates() {
        for (tier, hz) in [(Tier::Near, 15.0), (Tier::Middle, 5.0), (Tier::Far, 30.0 / 10.5)] {
            let mut d = Decimator::default();
            let relayed = (0..1800u32).step_by(2).filter(|&t| d.admit(at(t, 1000 + t), tier)).count();
            let rate = relayed as f64 / 60.0;
            assert!((rate - hz).abs() < 0.05, "{tier:?}: {rate} Hz");
        }
        let mut d = Decimator::default();
        let ticks: Vec<u32> = (0..60u32).step_by(2).filter(|&t| d.admit(at(t, t), Tier::Far)).collect();
        assert_eq!(ticks, [0, 12, 22, 32, 42, 54], "the disc's 11 / 10 alternation on even ticks");
    }

    #[test]
    fn a_lost_sample_costs_no_extra_delay_and_a_gap_starts_over() {
        let mut d = Decimator::default();
        assert!(d.admit(at(0, 0), Tier::Middle));
        assert!(!d.admit(at(4, 4), Tier::Middle));
        assert!(d.admit(at(8, 8), Tier::Middle));
        assert!(!d.admit(at(10, 10), Tier::Middle));
        assert!(d.admit(at(12, 12), Tier::Middle), "due at 12, not 14");
        assert!(d.admit(at(312, 312), Tier::Middle));
        assert!(!d.admit(at(316, 316), Tier::Middle));
        assert!(d.admit(at(318, 318), Tier::Middle));
    }

    #[test]
    fn the_schedule_follows_the_counts_not_the_stamps() {
        for (tier, every) in [(Tier::Near, 1usize), (Tier::Middle, 3)] {
            let mut d = Decimator::default();
            let mut admitted = Vec::new();
            for (k, x) in (0..80u32).step_by(2).enumerate() {
                let stamp = if x < 40 { 1000 + x } else { 1000 + x - 52 };
                if d.admit(at(x, stamp), tier) {
                    admitted.push(k);
                }
            }
            let expected: Vec<usize> = (0..40).step_by(every).collect();
            assert_eq!(admitted, expected, "{tier:?}");
        }
        let mut d = Decimator::default();
        let before_go: Vec<u32> = (0..12u32).step_by(2).filter(|&t| d.admit(at(0, 500 + t), Tier::Middle)).collect();
        assert_eq!(before_go, [0, 6], "one count: the stamps order");
        assert!(!d.admit(at(0, 503), Tier::Middle));
        assert!(d.admit(SampleOrder { epoch: 1, execution: 0, tick: 400 }, Tier::Middle), "a new epoch starts over");
        assert!(!d.admit(SampleOrder { epoch: 1, execution: 2, tick: 402 }, Tier::Middle));
    }

    #[test]
    fn distances_choose_the_tier() {
        assert_eq!(Tier::at(None), Tier::Near);
        assert_eq!(Tier::at(Some(150.0)), Tier::Near);
        assert_eq!(Tier::at(Some(150.1)), Tier::Middle);
        assert_eq!(Tier::at(Some(400.0)), Tier::Middle);
        assert_eq!(Tier::at(Some(400.1)), Tier::Far);
        assert_eq!(distance([0.0, 3.0, 0.0], [4.0, 0.0, 0.0]), 5.0);
    }
}
