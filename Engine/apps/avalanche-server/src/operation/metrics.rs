use std::fmt::Write as _;
use std::sync::PoisonError;
use std::sync::atomic::Ordering;

use super::http::OperationHttp;
use crate::http::HttpState;

// PORT(choice): the buckets around phase 2's measured 4–12 kB/s per rider and loopback-to-intercontinental round trips;
// tuning: the distributions of public servers; the original has no dedicated server.
pub const RATE_BOUNDS: [f64; 9] = [1e3, 2e3, 4e3, 8e3, 16e3, 32e3, 64e3, 128e3, 256e3];
pub const RTT_BOUNDS: [f64; 10] = [0.01, 0.025, 0.05, 0.075, 0.1, 0.15, 0.2, 0.3, 0.5, 1.0];
pub const JITTER_BOUNDS: [f64; 8] = [0.001, 0.0025, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25];

#[derive(Clone, Debug, Default)]
pub struct Histogram {
    pub buckets: Vec<u64>,
    pub sum: f64,
    pub count: u64,
}

impl Histogram {
    pub fn observe(&mut self, bounds: &[f64], value: f64) {
        if self.buckets.len() != bounds.len() {
            self.buckets = vec![0; bounds.len()];
        }
        for (b, bound) in self.buckets.iter_mut().zip(bounds) {
            if value <= *bound {
                *b += 1;
            }
        }
        self.sum += value;
        self.count += 1;
    }
}

#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub instances: Vec<(u16, &'static str, u32, u16)>,
    pub sent_rate: Histogram,
    pub received_rate: Histogram,
    pub rtt: Histogram,
    pub jitter: Histogram,
    pub validation: Vec<(&'static str, [u64; 5])>,
    pub chat_received: u64,
    pub chat_sent: u64,
    pub chat_dropped: u64,
    pub chat_floods: u64,
    pub chat_muted: u64,
    pub sessions: u64,
    pub kicked: u64,
    pub sent_bytes: u64,
    pub received_bytes: u64,
}

struct Page(String);

impl Page {
    fn head(&mut self, name: &str, kind: &str, help: &str) {
        let _ = writeln!(self.0, "# HELP {name} {help}\n# TYPE {name} {kind}");
    }

    fn sample(&mut self, name: &str, labels: &[(&str, &str)], value: impl std::fmt::Display) {
        self.0.push_str(name);
        if !labels.is_empty() {
            let inner: Vec<String> = labels.iter().map(|(k, v)| format!("{k}=\"{v}\"")).collect();
            let _ = write!(self.0, "{{{}}}", inner.join(","));
        }
        let _ = writeln!(self.0, " {value}");
    }

    fn histogram(&mut self, name: &str, help: &str, bounds: &[f64], h: &Histogram) {
        self.head(name, "histogram", help);
        for (k, bound) in bounds.iter().enumerate() {
            let le = format!("{bound}");
            self.sample(&format!("{name}_bucket"), &[("le", &le)], h.buckets.get(k).copied().unwrap_or(0));
        }
        self.sample(&format!("{name}_bucket"), &[("le", "+Inf")], h.count);
        self.sample(&format!("{name}_sum"), &[], h.sum);
        self.sample(&format!("{name}_count"), &[], h.count);
    }
}

pub fn render(state: &HttpState, ops: &OperationHttp) -> String {
    let shared = &state.shared;
    let snap = ops.snapshot.lock().unwrap_or_else(PoisonError::into_inner).clone();
    let mut p = Page(String::new());
    let micros = |v: u32| f64::from(v) / 1e6;
    p.head("ssxa_tick_p50_seconds", "gauge", "The median tick time over the last 30 s.");
    p.sample("ssxa_tick_p50_seconds", &[], micros(shared.tick_p50_us.load(Ordering::Relaxed)));
    p.head("ssxa_tick_p99_seconds", "gauge", "The 99th percentile tick time over the last 30 s.");
    p.sample("ssxa_tick_p99_seconds", &[], micros(shared.tick_p99_us.load(Ordering::Relaxed)));
    p.head("ssxa_players", "gauge", "Connected players.");
    p.sample("ssxa_players", &[], shared.players.load(Ordering::Relaxed));
    p.head("ssxa_max_players", "gauge", "The server's player slots.");
    p.sample("ssxa_max_players", &[], state.max_players);
    p.head("ssxa_instance_players", "gauge", "Players per instance.");
    for (id, mode, players, _) in &snap.instances {
        p.sample("ssxa_instance_players", &[("instance", &id.to_string()), ("mode", mode)], players);
    }
    p.head("ssxa_instance_round", "gauge", "Each instance's round number.");
    for (id, _, _, round) in &snap.instances {
        p.sample("ssxa_instance_round", &[("instance", &id.to_string())], round);
    }
    p.histogram(
        "ssxa_client_sent_bytes_per_second",
        "Each client's bytes a second sent to it (UDP payload), observed every second.",
        &RATE_BOUNDS,
        &snap.sent_rate,
    );
    p.histogram(
        "ssxa_client_received_bytes_per_second",
        "Each client's bytes a second received from it, observed every second.",
        &RATE_BOUNDS,
        &snap.received_rate,
    );
    p.histogram(
        "ssxa_client_rtt_seconds",
        "Each client's round trip (lightyear's link statistics), observed every second.",
        &RTT_BOUNDS,
        &snap.rtt,
    );
    p.histogram(
        "ssxa_client_jitter_seconds",
        "Each client's jitter (lightyear's link statistics), observed every second.",
        &JITTER_BOUNDS,
        &snap.jitter,
    );
    p.head("ssxa_joins_accepted_total", "counter", "Joins accepted (tickets issued).");
    p.sample("ssxa_joins_accepted_total", &[], shared.joins_accepted.load(Ordering::Relaxed));
    p.head("ssxa_join_refusals_total", "counter", "Joins refused, by the refusal's code.");
    for (code, n) in ops.refusals.lock().unwrap_or_else(PoisonError::into_inner).iter() {
        p.sample("ssxa_join_refusals_total", &[("code", code)], n);
    }
    p.head("ssxa_sessions_total", "counter", "Sessions begun (connections with a claimed ticket).");
    p.sample("ssxa_sessions_total", &[], snap.sessions);
    p.head("ssxa_kicks_total", "counter", "Connections the server ended.");
    p.sample("ssxa_kicks_total", &[], snap.kicked);
    p.head(
        "ssxa_validation_total",
        "counter",
        "The validation's outcomes per check (measures made, found beyond their limit; riders warned, invalidated, kicked).",
    );
    for (check, counts) in &snap.validation {
        for (outcome, n) in ["made", "found", "warned", "invalidated", "kicked"].iter().zip(counts) {
            p.sample("ssxa_validation_total", &[("check", check), ("outcome", outcome)], n);
        }
    }
    p.head(
        "ssxa_chat_lines_total",
        "counter",
        "Chat lines received from players and sent to them (once per receiver).",
    );
    p.sample("ssxa_chat_lines_total", &[("direction", "received")], snap.chat_received);
    p.sample("ssxa_chat_lines_total", &[("direction", "sent")], snap.chat_sent);
    p.head(
        "ssxa_chat_refusals_total",
        "counter",
        "Chat lines refused: over the sender's rate or the chat off; muted by the operator.",
    );
    p.sample("ssxa_chat_refusals_total", &[("reason", "rate_or_off")], snap.chat_dropped);
    p.sample("ssxa_chat_refusals_total", &[("reason", "muted")], snap.chat_muted);
    p.head("ssxa_chat_floods_total", "counter", "Players sent away for a chat flood.");
    p.sample("ssxa_chat_floods_total", &[], snap.chat_floods);
    p.head("ssxa_traffic_bytes_total", "counter", "UDP payload bytes over the clients' links.");
    p.sample("ssxa_traffic_bytes_total", &[("direction", "sent")], snap.sent_bytes);
    p.sample("ssxa_traffic_bytes_total", &[("direction", "received")], snap.received_bytes);
    p.head("ssxa_http_requests_total", "counter", "HTTP requests answered, and those a limit refused.");
    p.sample("ssxa_http_requests_total", &[("outcome", "answered")], shared.http_requests.load(Ordering::Relaxed));
    p.sample("ssxa_http_requests_total", &[("outcome", "limited")], shared.http_limited.load(Ordering::Relaxed));
    p.head("ssxa_rcon_commands_total", "counter", "Remote console requests: commands run, requests refused.");
    p.sample("ssxa_rcon_commands_total", &[("outcome", "ran")], ops.rcon_ran.load(Ordering::Relaxed));
    p.sample("ssxa_rcon_commands_total", &[("outcome", "refused")], ops.rcon_refused.load(Ordering::Relaxed));
    let (bans, mutes) = {
        let store = ops.operation.bans();
        (store.bans().len(), store.mutes().len())
    };
    p.head("ssxa_bans", "gauge", "Bans in the ban store (passed ones until the next sweep).");
    p.sample("ssxa_bans", &[], bans);
    p.head("ssxa_mutes", "gauge", "Mutes in the ban store.");
    p.sample("ssxa_mutes", &[], mutes);
    let lists = state.shared.blocklists.lock().unwrap_or_else(PoisonError::into_inner).clone();
    p.head("ssxa_blocklist_entries", "gauge", "Blocked resource files per followed list (applied or not).");
    for g in &lists {
        p.sample("ssxa_blocklist_entries", &[("source", &g.source)], g.entries);
    }
    p.head(
        "ssxa_blocklist_applied",
        "gauge",
        "Whether a followed list applies (1) or not (none kept, too old, unreadable).",
    );
    for g in &lists {
        p.sample("ssxa_blocklist_applied", &[("source", &g.source)], u8::from(g.applied));
    }
    p.head("ssxa_blocklist_age_seconds", "gauge", "Seconds since a master last answered for its list.");
    for g in lists.iter().filter(|g| g.age_secs.is_some()) {
        p.sample("ssxa_blocklist_age_seconds", &[("source", &g.source)], g.age_secs.unwrap_or_default());
    }
    p.head("ssxa_script_seconds_total", "counter", "Script time per resource (its server scripts).");
    for (resource, us) in ops.script_us.lock().unwrap_or_else(PoisonError::into_inner).iter() {
        p.sample("ssxa_script_seconds_total", &[("resource", resource)], *us as f64 / 1e6);
    }
    p.0
}
