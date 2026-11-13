use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use ssx_net::address::AddressKey;
use ssx_net_proto::join::{HOLD_TICKET_SIZE, HeldAsk, Hold, hold_text};
use ssx_net_proto::keys::{ReplyKey, random_bytes};
use subtle::ConstantTimeEq;

use crate::bridge::HeldJoin;

// PORT(choice): 1 s (the protocol allows 0.5 to 5 s): a decision reaches the player a second after it is made at most,
// and 64 held joins cost the server 64 sealed asks a second; tuning: the connecting screen with a held join over real
// links (phase F2's popup) and a full server's held joins measured; the original has no dedicated server.
pub const ASK_AFTER_MS: u16 = 1_000;

// PORT(choice): the pause and 14 s; tuning: asks over real links (phase 5's servers), their slowest measured; the
// original has no dedicated server.
pub const TICKET_WINDOW: Duration = Duration::from_millis(ASK_AFTER_MS as u64 + 14_000);

// PORT(choice): 4; tuning: phase 4's public servers' joins per address; the original has no dedicated server.
pub const MAX_HELD_PER_ADDRESS: usize = 4;

pub const MAX_TICKETS: usize = ssx_script::host::server::joins::MAX_HELD;

pub struct Waiting {
    pub hold: HeldJoin,
    pub since: Instant,
    pub who: String,
}

struct Entry {
    ticket: [u8; HOLD_TICKET_SIZE],
    address: AddressKey,
    reply: ReplyKey,
    deadline: Instant,
    waiting: Waiting,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refused {
    Full,
    Address,
}

#[derive(Default)]
pub struct HeldTickets {
    table: Mutex<BTreeMap<[u8; 8], Entry>>,
}

fn id_of(ticket: &[u8; HOLD_TICKET_SIZE]) -> [u8; 8] {
    let mut id = [0u8; 8];
    id.copy_from_slice(&ticket[..8]);
    id
}

impl HeldTickets {
    pub fn issue(
        &self,
        address: AddressKey,
        reply: ReplyKey,
        waiting: Waiting,
        now: Instant,
    ) -> Result<Hold, (Refused, Waiting)> {
        let Ok(mut table) = self.table.lock() else { return Err((Refused::Full, waiting)) };
        let expired = sweep_table(&mut table, now);
        if table.len() >= MAX_TICKETS {
            return Err((Refused::Full, waiting));
        }
        if table.values().filter(|e| e.address == address).count() >= MAX_HELD_PER_ADDRESS {
            return Err((Refused::Address, waiting));
        }
        let ticket = loop {
            let t: [u8; HOLD_TICKET_SIZE] = random_bytes();
            if !table.contains_key(&id_of(&t)) {
                break t;
            }
        };
        let text = hold_text(&waiting.hold.text.borrow());
        let deadline = now + TICKET_WINDOW;
        table.insert(id_of(&ticket), Entry { ticket, address, reply, deadline, waiting });
        drop(table);
        log_expired(expired);
        Ok(Hold { ticket, ask_after_ms: ASK_AFTER_MS, text })
    }

    pub fn take(&self, ask: &HeldAsk, address: AddressKey, now: Instant) -> Option<Waiting> {
        let mut table = self.table.lock().ok()?;
        let id = id_of(&ask.ticket);
        let e = table.get(&id)?;
        let same = e.ticket.ct_eq(&ask.ticket) & e.reply.as_bytes().ct_eq(ask.reply.as_bytes());
        if !bool::from(same) || e.address != address || now > e.deadline {
            return None;
        }
        table.remove(&id).map(|e| e.waiting)
    }

    pub fn sweep(&self, now: Instant) {
        let expired = match self.table.lock() {
            Ok(mut table) => sweep_table(&mut table, now),
            Err(_) => return,
        };
        log_expired(expired);
    }

    pub fn out_now(&self) -> usize {
        self.table.lock().map_or(0, |t| t.len())
    }
}

fn sweep_table(table: &mut BTreeMap<[u8; 8], Entry>, now: Instant) -> Vec<Waiting> {
    let late: Vec<[u8; 8]> = table.iter().filter(|(_, e)| now > e.deadline).map(|(id, _)| *id).collect();
    late.into_iter().filter_map(|id| table.remove(&id)).map(|e| e.waiting).collect()
}

fn log_expired(expired: Vec<Waiting>) {
    for w in expired {
        bevy::log::info!(
            "join of {}: held {} s, its client did not ask again within {} s: given up",
            w.who,
            w.since.elapsed().as_secs(),
            TICKET_WINDOW.as_secs()
        );
    }
}

#[cfg(test)]
mod tests;
