use std::collections::VecDeque;

use ssx_net_proto::{ClientControl, ProgressReport};

use super::time::Received;

// PORT(choice): 64 messages, a port bound (an honest hold measured at most a handful; the round's reports are a few a
// second at the most); tuning: the largest hold of the network runs; the original has no real-time multiplayer.
pub const MAX_HELD: usize = 64;

pub const HELD_ENTRY_BYTES: usize = size_of::<Held>() + size_of::<ProgressReport>();

pub const MAX_HELD_BYTES: usize = MAX_HELD * HELD_ENTRY_BYTES;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Due {
    pub instance: u16,
    pub round: u16,
    pub execution: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Held {
    pub message: ClientControl,
    pub received: Received,
    pub due: Option<Due>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ahead {
    pub elapsed: f64,
    pub horizon: f64,
    pub due: Due,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Offer {
    Now(Box<ClientControl>, Received),
    Held,
    Refused(String),
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Hold {
    held: VecDeque<Held>,
}

impl Hold {
    pub fn is_empty(&self) -> bool {
        self.held.is_empty()
    }

    pub fn len(&self) -> usize {
        self.held.len()
    }

    pub fn holds_ready(&self, instance: u16, round: u16) -> bool {
        self.held.iter().any(|h| {
            matches!(h.message, ClientControl::Ready { round: r, .. } if r == round)
                && h.due.is_some_and(|d| d.instance == instance && d.round == round)
        })
    }

    pub fn offer(&mut self, message: ClientControl, received: Received, ahead: Option<Ahead>) -> Offer {
        let due = match ahead {
            Some(a) if a.due.execution > a.elapsed + a.horizon => {
                return Offer::Refused(format!(
                    "a report of execution {:.0} came {:.2} ticks after the round's start: {:.2} ahead of the server's \
                     time, beyond the run's horizon of {:.2}",
                    a.due.execution,
                    a.elapsed,
                    a.due.execution - a.elapsed,
                    a.horizon
                ));
            }
            Some(a) if a.due.execution > a.elapsed => Some(a.due),
            _ => None,
        };
        if due.is_none() && self.held.is_empty() {
            return Offer::Now(Box::new(message), received);
        }
        if self.held.len() >= MAX_HELD {
            return Offer::Refused(format!("the hold is full ({MAX_HELD} messages behind a report ahead of its time)"));
        }
        self.held.push_back(Held { message, received, due });
        Offer::Held
    }

    pub fn release(&mut self, elapsed: impl Fn(&Due) -> Option<f64>) -> Vec<Held> {
        let mut out = Vec::new();
        while let Some(front) = self.held.front() {
            let ready = front.due.is_none_or(|d| elapsed(&d).is_none_or(|e| d.execution <= e));
            if !ready {
                break;
            }
            if let Some(h) = self.held.pop_front() {
                out.push(h);
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use super::*;

    fn received() -> Received {
        let at = Instant::now();
        Received { at, after: at }
    }

    fn ahead(execution: f64, elapsed: f64) -> Option<Ahead> {
        Some(Ahead { elapsed, horizon: 4.0, due: Due { instance: 1, round: 2, execution } })
    }

    fn ready(round: u16) -> ClientControl {
        crate::harness::ready(round)
    }

    #[test]
    fn a_report_within_the_server_time_goes_now_one_ahead_waits_one_beyond_the_horizon_is_refused() {
        let mut h = Hold::default();
        assert!(matches!(h.offer(ready(1), received(), ahead(30.0, 30.0)), Offer::Now(..)));
        assert!(matches!(h.offer(ready(2), received(), ahead(29.0, 30.0)), Offer::Now(..)));
        assert_eq!(h.offer(ready(3), received(), ahead(34.0, 30.0)), Offer::Held);
        assert!(matches!(h.offer(ready(4), received(), ahead(34.01, 30.0)), Offer::Refused(_)));
        assert_eq!(h.len(), 1);
    }

    #[test]
    fn messages_behind_a_held_one_wait_and_go_in_their_order() {
        let mut h = Hold::default();
        assert_eq!(h.offer(ready(1), received(), ahead(33.0, 30.0)), Offer::Held);
        assert_eq!(h.offer(ready(2), received(), None), Offer::Held);
        assert_eq!(h.offer(ready(3), received(), ahead(31.0, 30.0)), Offer::Held);
        assert_eq!(h.offer(ready(4), received(), ahead(34.0, 30.5)), Offer::Held);
        assert!(h.release(|_| Some(32.0)).is_empty());
        let out: Vec<_> = h.release(|_| Some(33.0)).into_iter().map(|m| m.message).collect();
        assert_eq!(out, vec![ready(1), ready(2), ready(3)]);
        assert_eq!(h.len(), 1);
        let out: Vec<_> = h.release(|_| None).into_iter().map(|m| m.message).collect();
        assert_eq!(out, vec![ready(4)]);
        assert!(h.is_empty());
    }

    #[test]
    fn an_early_ready_waits_for_the_start_and_the_reports_behind_it_too() {
        let mut h = Hold::default();
        let at = |execution: f64, elapsed: f64| {
            Some(Ahead { elapsed, horizon: 150.0, due: Due { instance: 1, round: 2, execution } })
        };
        assert_eq!(h.offer(ready(2), received(), at(0.0, -40.0)), Offer::Held);
        assert!(h.holds_ready(1, 2) && !h.holds_ready(1, 3) && !h.holds_ready(2, 2));
        assert_eq!(h.offer(ready(9), received(), at(30.0, -10.0)), Offer::Held);
        assert!(h.release(|_| Some(-1.0)).is_empty());
        let out: Vec<_> = h.release(|_| Some(0.0)).into_iter().map(|m| m.message).collect();
        assert_eq!(out, vec![ready(2)]);
        assert!(!h.holds_ready(1, 2));
        assert_eq!(h.release(|_| Some(30.0)).len(), 1);
    }

    #[test]
    fn the_hold_is_bounded_by_its_count_and_memory() {
        let mut h = Hold::default();
        assert_eq!(h.offer(ready(0), received(), ahead(34.0, 30.0)), Offer::Held);
        for k in 1..MAX_HELD {
            assert_eq!(h.offer(ready(k as u16), received(), None), Offer::Held, "{k}");
        }
        assert_eq!(h.len(), MAX_HELD);
        assert!(matches!(h.offer(ready(999), received(), None), Offer::Refused(_)));
        assert!(matches!(h.offer(ready(999), received(), ahead(31.0, 30.0)), Offer::Refused(_)));
        assert_eq!(h.len(), MAX_HELD);
        const { assert!(MAX_HELD_BYTES <= 64 * 1024) };
    }
}
