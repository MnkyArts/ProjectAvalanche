use std::collections::{BTreeMap, VecDeque};
use std::time::Duration;

use bevy::prelude::*;
use ssx_net::server::TicketRequest;
use ssx_net_proto::join::{MAX_REFUSAL_TEXT, RefusalCode, hold_text};
use ssx_script::host::server::joins::MAX_HELD;
use tokio::sync::{oneshot, watch};

use crate::bridge::{HeldJoin, JoinQueue, JoinTask, JoinVerdict};

pub const DEFER_BOUND: Duration = ssx_net_proto::join::MAX_HOLD;

// PORT(choice): 3 s; the scripts' thread answers each tick's job, `onPlayerConnecting` runs in the job after the join
// is put to them (one tick, 33 ms at 30 Hz) unless a script's budget holds the thread; tuning: the thread's answer
// times under phase 4's load with a gamemode's handlers; the original runs no mod scripts.
pub const FIRST_WORD: Duration = Duration::from_secs(3);

enum Answer {
    First(oneshot::Sender<JoinVerdict>),
    Held { text: watch::Sender<String>, verdict: oneshot::Sender<JoinVerdict> },
}

impl Answer {
    fn gone(&self) -> bool {
        match self {
            Self::First(tx) => tx.is_closed(),
            Self::Held { verdict, .. } => verdict.is_closed(),
        }
    }

    fn send(self, verdict: JoinVerdict) {
        let _ = match self {
            Self::First(tx) => tx.send(verdict),
            Self::Held { verdict: tx, .. } => tx.send(verdict),
        };
    }
}

struct Waiting {
    ticket: TicketRequest,
    answer: Answer,
    asked: Duration,
    held: Option<Duration>,
}

#[derive(Resource, Default)]
pub struct HeldJoins {
    next: u64,
    pub(super) asking: bool,
    waiting: BTreeMap<u64, Waiting>,
    pub(super) to_ask: Vec<(u64, String, String)>,
    pub(super) forgotten: Vec<u64>,
    ready: VecDeque<JoinTask>,
    pub refused: u64,
    pub let_through: u64,
    pub held_answered: u64,
    pub given_up: u64,
}

fn refusal_text(s: &str) -> String {
    let mut t: String = s.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    while t.len() > MAX_REFUSAL_TEXT {
        t.pop();
    }
    t
}

impl HeldJoins {
    pub(super) fn decide(&mut self, id: u64, accept: Result<(), String>) {
        let Some(w) = self.waiting.remove(&id) else { return };
        match accept {
            Ok(()) => {
                self.let_through += 1;
                let reply = match w.answer {
                    Answer::First(tx) => tx,
                    Answer::Held { verdict, .. } => verdict,
                };
                self.ready.push_back(JoinTask { ticket: w.ticket, reply });
            }
            Err(why) => {
                self.refused += 1;
                w.answer.send(JoinVerdict::Refused(RefusalCode::Scripts, refusal_text(&why)));
            }
        }
    }

    pub(super) fn hold(&mut self, id: u64, text: &str, now: Duration) {
        let Some(w) = self.waiting.get_mut(&id) else { return };
        let text = hold_text(text);
        if let Answer::Held { text: tx, .. } = &w.answer {
            tx.send_if_modified(|t| {
                let changed = *t != text;
                if changed {
                    t.clone_from(&text);
                }
                changed
            });
            return;
        }
        let Some(mut w) = self.waiting.remove(&id) else { return };
        let Answer::First(first) = w.answer else { return };
        let (text_tx, text_rx) = watch::channel(text);
        let (verdict_tx, verdict_rx) = oneshot::channel();
        if first.send(JoinVerdict::Held(HeldJoin { text: text_rx, verdict: verdict_rx })).is_err() {
            self.given_up += 1;
            self.forgotten.push(id);
            return;
        }
        self.held_answered += 1;
        w.answer = Answer::Held { text: text_tx, verdict: verdict_tx };
        w.held = Some(now);
        self.waiting.insert(id, w);
    }

    pub(super) fn expire(&mut self, now: Duration) {
        let mut gone = Vec::new();
        let mut late = Vec::new();
        for (id, w) in &self.waiting {
            if w.answer.gone() {
                gone.push(*id);
            } else if match w.held {
                None => now.saturating_sub(w.asked) > FIRST_WORD,
                Some(at) => now.saturating_sub(at) > DEFER_BOUND + Duration::from_secs(2),
            } {
                late.push(*id);
            }
        }
        for id in gone {
            if self.waiting.remove(&id).is_some() {
                self.given_up += 1;
                self.forgotten.push(id);
            }
        }
        for id in late {
            if let Some(w) = self.waiting.remove(&id) {
                self.refused += 1;
                self.forgotten.push(id);
                let text = "the server's scripts did not answer in time".to_string();
                w.answer.send(JoinVerdict::Refused(RefusalCode::Busy, text));
            }
        }
    }

    pub fn held(&self) -> usize {
        self.waiting.len()
    }
}

pub fn next_join(queue: &mut JoinQueue, held: Option<&mut HeldJoins>, now: Duration) -> Option<JoinTask> {
    let Some(h) = held else { return queue.0.try_recv().ok() };
    if let Some(t) = h.ready.pop_front() {
        return Some(t);
    }
    loop {
        let task = queue.0.try_recv().ok()?;
        if !h.asking {
            return Some(task);
        }
        if h.waiting.len() >= MAX_HELD {
            h.refused += 1;
            let text = format!("{MAX_HELD} joins wait for the server's scripts");
            let _ = task.reply.send(JoinVerdict::Refused(RefusalCode::Busy, text));
            continue;
        }
        h.next += 1;
        let id = h.next;
        h.to_ask.push((id, task.ticket.name.clone(), task.ticket.player.id().to_hex()));
        h.waiting
            .insert(id, Waiting { ticket: task.ticket, answer: Answer::First(task.reply), asked: now, held: None });
    }
}

#[cfg(test)]
mod tests;
