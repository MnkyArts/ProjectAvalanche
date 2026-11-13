use std::collections::BTreeMap;
use std::time::Duration;

use crate::host::ScriptHost;
use crate::host::call::{Dispatch, dispatch};
use crate::host::elements::ROOT;
use crate::value::NetValue;

// PORT(choice): 64 held joins (the server's join queue's 64, which a held join is not part of); tuning: a measured join
// burst of phase 4's 32-player servers with a deferring gamemode; the original runs no mod scripts.
pub const MAX_HELD: usize = 64;

pub const MAX_TEXT: usize = 256;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JoinVerdict {
    Accept,
    Refuse(String),
    Held,
}

pub(crate) struct Held {
    pub name: String,
    pub holds: BTreeMap<String, Hold>,
    pub refused: Option<String>,
    pub since: Duration,
}

#[derive(Default)]
pub(crate) struct Hold {
    pub accepted: bool,
    pub text: String,
}

impl Held {
    fn verdict(&self) -> JoinVerdict {
        match &self.refused {
            Some(why) => JoinVerdict::Refuse(why.clone()),
            None if self.holds.values().all(|h| h.accepted) => JoinVerdict::Accept,
            None => JoinVerdict::Held,
        }
    }

    fn waiting(&self) -> String {
        let open = self.holds.iter().filter(|(_, h)| !h.accepted);
        open.map(|(p, h)| if h.text.is_empty() { p.clone() } else { format!("{p}: {}", h.text) })
            .collect::<Vec<_>>()
            .join("; ")
    }
}

fn bounded(text: &str, max: usize) -> String {
    let mut out = String::new();
    for c in text.chars().filter(|c| !ssx_core::text::is_hidden(*c)) {
        if out.len() + c.len_utf8() > max {
            break;
        }
        out.push(c);
    }
    out
}

#[derive(Default)]
pub struct Joins {
    next: u64,
    pub(crate) held: BTreeMap<u64, Held>,
}

impl ScriptHost {
    pub fn player_connecting(&self, name: &str, key_id: &str, now: Duration) -> (u64, JoinVerdict) {
        let id = {
            let mut w = self.shared.lock();
            if w.server.joins.held.len() >= MAX_HELD {
                return (0, JoinVerdict::Refuse("too many joins are held by the server's scripts".into()));
            }
            w.server.joins.next += 1;
            let id = w.server.joins.next;
            let held = Held { name: name.to_string(), holds: BTreeMap::new(), refused: None, since: now };
            w.server.joins.held.insert(id, held);
            id
        };
        let s = |v: &str| NetValue::String(v.as_bytes().to_vec());
        let args = [s(name), s(key_id), NetValue::Number(id as f64)];
        let d = Dispatch { name: "onPlayerConnecting", source: ROOT, args: &args, only: None, client: None };
        let cancelled = dispatch(&self.shared, &d).unwrap_or(false);
        self.settle();
        let mut w = self.shared.lock();
        let Some(h) = w.server.joins.held.get(&id) else {
            return (id, JoinVerdict::Refuse("the join was dropped".into()));
        };
        let verdict = match h.verdict() {
            JoinVerdict::Refuse(why) => JoinVerdict::Refuse(why),
            _ if cancelled => JoinVerdict::Refuse("the server's scripts refused the join".into()),
            v => v,
        };
        if verdict != JoinVerdict::Held {
            w.server.joins.held.remove(&id);
        }
        (id, verdict)
    }

    pub fn join_verdict(&self, id: u64) -> Option<JoinVerdict> {
        let mut w = self.shared.lock();
        let v = w.server.joins.held.get(&id)?.verdict();
        if v == JoinVerdict::Held {
            return None;
        }
        w.server.joins.held.remove(&id);
        Some(v)
    }

    pub fn join_text(&self, id: u64) -> Option<String> {
        let w = self.shared.lock();
        let h = w.server.joins.held.get(&id)?;
        let open = h.holds.values().filter(|h| !h.accepted && !h.text.is_empty());
        Some(open.map(|h| h.text.as_str()).collect::<Vec<_>>().join("; "))
    }

    pub fn join_expired(&self, id: u64) -> Option<String> {
        self.shared.lock().server.joins.held.remove(&id).map(|h| h.waiting())
    }

    pub fn held_joins(&self) -> Vec<(u64, String, String, Duration)> {
        let w = self.shared.lock();
        w.server.joins.held.iter().map(|(id, h)| (*id, h.name.clone(), h.waiting(), h.since)).collect()
    }
}

pub(crate) fn decide(j: &mut Joins, package: &str, id: u64, what: Decision) -> Result<bool, String> {
    let h = j.held.get_mut(&id).ok_or_else(|| format!("no join {id} is held"))?;
    if h.refused.is_some() {
        return Ok(false);
    }
    Ok(match what {
        Decision::Defer if h.holds.contains_key(package) => false,
        Decision::Defer => {
            h.holds.insert(package.to_string(), Hold::default());
            true
        }
        Decision::Accept => match h.holds.get_mut(package) {
            Some(hold) if !hold.accepted => {
                hold.accepted = true;
                true
            }
            _ => false,
        },
        Decision::Refuse(reason) => {
            h.refused = Some(bounded(&reason, MAX_TEXT));
            true
        }
        Decision::Update(text) => match h.holds.get_mut(package) {
            Some(hold) => {
                hold.text = bounded(&text, MAX_TEXT);
                true
            }
            None => false,
        },
    })
}

pub(crate) fn package_stopped(j: &mut Joins, package: &str) {
    for h in j.held.values_mut() {
        if h.refused.is_none() && h.holds.get(package).is_some_and(|hold| !hold.accepted) {
            h.refused = Some(format!("{package} stopped while it held the join"));
        }
    }
}

pub(crate) enum Decision {
    Defer,
    Accept,
    Refuse(String),
    Update(String),
}
