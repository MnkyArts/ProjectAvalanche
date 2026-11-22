use std::time::Duration;

use bevy::prelude::*;
use ssx_net::server::LONG_FRAME;

// PORT(choice): a minute: the official servers' stalls came about 300 an hour each (v0.4.0, v0.4.1), one line a stall
// flooded their logs; one line a minute at most (60 an hour) keeps each stall's count, the longest and the ticks caught
// up, and the first stall after a quiet minute is told at once; the original has no dedicated server.
pub const REPORT_EVERY: Duration = Duration::from_secs(60);

#[derive(Default)]
pub struct Stalls {
    count: u64,
    longest: Duration,
    caught_up: f64,
    first: Option<Duration>,
    said: Option<Duration>,
}

impl Stalls {
    pub fn frame(&mut self, now: Duration, real: Duration, steps: Duration) -> Option<String> {
        if real > LONG_FRAME {
            self.count += 1;
            self.longest = self.longest.max(real);
            self.caught_up += steps.as_secs_f64() / ssx_net::tick_duration().as_secs_f64();
            self.first.get_or_insert(now);
        }
        if self.count == 0 || self.said.is_some_and(|at| now.saturating_sub(at) < REPORT_EVERY) {
            return None;
        }
        let over = LONG_FRAME.as_millis();
        let line = if self.count == 1 {
            format!(
                "a stall: a frame of {} ms (over {over} ms); the fixed steps caught up {:.1} ticks of it",
                self.longest.as_millis(),
                self.caught_up
            )
        } else {
            let span = now.saturating_sub(self.first.unwrap_or(now));
            format!(
                "{} stalls in {} s: frames over {over} ms, the longest {} ms; the fixed steps caught up {:.1} ticks of \
                 them (at most one such line a minute)",
                self.count,
                span.as_secs(),
                self.longest.as_millis(),
                self.caught_up
            )
        };
        *self = Stalls { said: Some(now), ..Stalls::default() };
        Some(line)
    }
}

pub fn report(real: Res<Time<Real>>, virt: Res<Time<Virtual>>, mut stalls: Local<Stalls>) {
    if let Some(line) = stalls.frame(real.elapsed(), real.delta(), virt.delta()) {
        warn!("the server: {line}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FRAME: Duration = Duration::from_millis(33);

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn quiet_frames_say_nothing() {
        let mut s = Stalls::default();
        for i in 0..100_000u64 {
            assert_eq!(s.frame(ms(i * 33), FRAME, FRAME), None);
        }
        assert_eq!(s.frame(ms(3_300_250), LONG_FRAME, LONG_FRAME), None);
    }

    #[test]
    fn the_first_stall_is_told_at_once() {
        let mut s = Stalls::default();
        let line = s.frame(ms(5_000), ms(289), ms(289)).expect("a line");
        assert_eq!(line, "a stall: a frame of 289 ms (over 250 ms); the fixed steps caught up 8.7 ticks of it");
    }

    #[test]
    fn stalls_every_twelve_seconds_make_a_line_a_minute_at_most() {
        let mut s = Stalls::default();
        let (mut lines, mut counted) = (Vec::new(), 0u64);
        let mut now = Duration::ZERO;
        for frame in 0..(3_600 * 30u64) {
            let stall = frame % 360 == 7;
            let real = if stall { ms(250 + frame % 279) } else { FRAME };
            now += real;
            if let Some(line) = s.frame(now, real, real) {
                counted += if line.starts_with("a stall") {
                    1
                } else {
                    line.split(' ').next().and_then(|n| n.parse().ok()).expect("a count")
                };
                lines.push((now, line));
            }
        }
        let stalls = (0..(3_600 * 30u64)).filter(|f| f % 360 == 7).count() as u64;
        assert_eq!(counted, stalls, "every stall counted once");
        assert!(lines.len() <= 62, "{} lines in about an hour", lines.len());
        for w in lines.windows(2) {
            assert!(w[1].0 - w[0].0 >= REPORT_EVERY, "two lines {:?} apart", w[1].0 - w[0].0);
        }
        assert!(lines[1].1.contains("stalls in") && lines[1].1.contains("the longest"), "{}", lines[1].1);
    }

    #[test]
    fn the_stalls_of_a_minute_come_in_one_line_with_the_longest() {
        let mut s = Stalls::default();
        assert!(s.frame(ms(1_000), ms(300), ms(300)).is_some());
        assert_eq!(s.frame(ms(2_000), ms(400), ms(400)), None);
        assert_eq!(s.frame(ms(30_000), ms(528), ms(528)), None);
        assert_eq!(s.frame(ms(60_999), FRAME, FRAME), None);
        let line = s.frame(ms(61_000), FRAME, FRAME).expect("the minute's line");
        assert_eq!(
            line,
            "2 stalls in 59 s: frames over 250 ms, the longest 528 ms; the fixed steps caught up 27.8 ticks of them (at \
             most one such line a minute)"
        );
        assert_eq!(s.frame(ms(200_000), FRAME, FRAME), None, "nothing since");
    }
}
