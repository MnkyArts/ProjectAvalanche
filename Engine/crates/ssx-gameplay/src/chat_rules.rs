use core::time::Duration;

// PORT(choice): one line a second with a burst of 5 (a person types a line in seconds; quick chat on a pad takes about
// a second a line); leads: FiveM's event buckets (rate / burst, then a flood bucket that drops the client: netEvent
// 50/s burst 200, flood 75/300) and MTA:SA's join limit (4 per 30 s); tuning: what players send in phase 4's public
// servers; the original has no chat.
pub const LINES_PER_SECOND: f64 = 1.0;
pub const LINE_BURST: f64 = 5.0;

// PORT(choice): 10 dropped lines, one more every 3 s (a correct client holds its lines after a flood notice until the
// wait it names, so only a modified client gets here); tuning: the dropped lines of correct clients in phase 4; the
// original has no chat.
pub const FLOOD_ALLOWANCE: f64 = 10.0;
pub const FLOOD_REFILL_PER_SECOND: f64 = 1.0 / 3.0;

pub const NOTICE_INTERVAL: Duration = Duration::from_secs(1);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    Pass,
    Drop { notice: bool, wait: Duration },
    Flood,
}

#[derive(Clone, Debug, Default)]
pub struct ChatBudget {
    lines: f64,
    flood: f64,
    refilled: Option<Duration>,
    noticed: Option<Duration>,
}

impl ChatBudget {
    pub fn new() -> Self {
        Self { lines: LINE_BURST, flood: FLOOD_ALLOWANCE, refilled: None, noticed: None }
    }

    fn refill(&mut self, now: Duration) {
        if let Some(then) = self.refilled {
            let s = now.saturating_sub(then).as_secs_f64();
            self.lines = (self.lines + s * LINES_PER_SECOND).min(LINE_BURST);
            self.flood = (self.flood + s * FLOOD_REFILL_PER_SECOND).min(FLOOD_ALLOWANCE);
        }
        self.refilled = Some(now);
    }

    pub fn take(&mut self, now: Duration, open: bool) -> Verdict {
        self.refill(now);
        if open && self.lines >= 1.0 {
            self.lines -= 1.0;
            return Verdict::Pass;
        }
        if self.flood < 1.0 {
            return Verdict::Flood;
        }
        self.flood -= 1.0;
        let notice = self.noticed.is_none_or(|t| now.saturating_sub(t) >= NOTICE_INTERVAL);
        if notice {
            self.noticed = Some(now);
        }
        let wait =
            if open { Duration::from_secs_f64((1.0 - self.lines).max(0.0) / LINES_PER_SECOND) } else { Duration::ZERO };
        Verdict::Drop { notice, wait }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(ms: u64) -> Duration {
        Duration::from_millis(ms)
    }

    #[test]
    fn a_burst_then_the_rate_then_a_flood() {
        let mut b = ChatBudget::new();
        for i in 0..5 {
            assert_eq!(b.take(at(i), true), Verdict::Pass, "line {i} of the burst");
        }
        let Verdict::Drop { notice: true, wait } = b.take(at(5), true) else { panic!("the sixth line") };
        assert!((990..=1000).contains(&wait.as_millis()), "{wait:?}");
        assert!(matches!(b.take(at(500), true), Verdict::Drop { notice: false, .. }));
        assert_eq!(b.take(at(1600), true), Verdict::Pass);
        let mut verdicts = Vec::new();
        for i in 0..20 {
            verdicts.push(b.take(at(1601 + i), true));
        }
        let dropped = verdicts.iter().take_while(|v| matches!(v, Verdict::Drop { .. })).count();
        assert_eq!(dropped, 8, "two of the ten were taken before: {verdicts:?}");
        assert_eq!(verdicts[dropped], Verdict::Flood);
    }

    #[test]
    fn a_steady_rate_never_floods_and_the_allowance_comes_back() {
        let mut b = ChatBudget::new();
        for s in 0..600 {
            assert_eq!(b.take(at(s * 1000), true), Verdict::Pass, "second {s}");
        }
        let mut b = ChatBudget::new();
        for i in 0..5 {
            b.take(at(i), true);
        }
        for i in 0..9 {
            assert!(matches!(b.take(at(10 + i), true), Verdict::Drop { .. }));
        }
        assert_eq!(b.take(at(30_100), true), Verdict::Pass);
        for i in 0..4 {
            b.take(at(30_101 + i), true);
        }
        for i in 0..10 {
            assert!(matches!(b.take(at(30_200 + i), true), Verdict::Drop { .. }), "drop {i}");
        }
        assert_eq!(b.take(at(30_300), true), Verdict::Flood);
    }

    #[test]
    fn a_closed_chat_drops_every_line_and_a_flood_of_them_is_sent_away() {
        let mut b = ChatBudget::new();
        assert_eq!(b.take(at(0), false), Verdict::Drop { notice: true, wait: Duration::ZERO });
        for i in 1..10 {
            assert!(matches!(b.take(at(i), false), Verdict::Drop { notice: false, .. }));
        }
        assert_eq!(b.take(at(10), false), Verdict::Flood);
    }
}
