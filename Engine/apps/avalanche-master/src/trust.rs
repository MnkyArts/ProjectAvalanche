use std::net::IpAddr;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use tokio::sync::oneshot;
use tracing::{info, warn};

use crate::address::canonical;
use crate::forwarded::{Network, TrustsPeer};
use crate::names::{Answer, HostName, Resolve, addresses, judge};

// PORT(choice): every 2 s (one query to Docker's DNS, 127.0.0.11 inside the container, per name and family; a Traefik
// stand-in recreated at a new address was trusted 0.40 s after its start, round net4d); tuning: a proxy restart on
// Coolify measured (how long its requests reach the master untrusted); the original has no master server.
pub const REFRESH: Duration = Duration::from_secs(2);
// PORT(choice): 2 s (Docker's DNS answers from the container's own loopback; glibc's default is 5 s per try and server;
// a resolver that does not answer within it is treated as failed); the original has no master server.
pub const LOOKUP_TIMEOUT: Duration = Duration::from_secs(2);
// PORT(choice): 10 s (five refreshes: a resolver's hiccup costs no trust, a proxy gone and its address given to another
// container is trusted at most this long); tuning: Docker's DNS failures measured on a busy Coolify host; the original
// has no master server.
pub const STALE_AFTER: Duration = Duration::from_secs(10);
const ERROR_CHARS: usize = 160;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NameState {
    Pending,
    Trusted { addrs: Vec<IpAddr>, until: Instant, failing: Option<String> },
    Refused(String),
    Unresolved(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Holder<'a> {
    Network(&'a Network),
    Name(&'a HostName),
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TrustSet {
    networks: Vec<Network>,
    names: Vec<(HostName, NameState)>,
}

impl TrustSet {
    pub fn new(networks: Vec<Network>, names: Vec<HostName>) -> Self {
        Self { networks, names: names.into_iter().map(|n| (n, NameState::Pending)).collect() }
    }

    pub fn networks(&self) -> &[Network] {
        &self.networks
    }

    pub fn names(&self) -> &[(HostName, NameState)] {
        &self.names
    }

    pub fn is_empty(&self) -> bool {
        self.networks.is_empty() && self.names.is_empty()
    }

    pub fn holder_at(&self, ip: IpAddr, now: Instant) -> Option<Holder<'_>> {
        let ip = canonical(ip);
        if let Some(n) = self.networks.iter().find(|n| n.contains(ip)) {
            return Some(Holder::Network(n));
        }
        self.names.iter().find_map(|(name, state)| match state {
            NameState::Trusted { addrs, until, .. } if now < *until && addrs.contains(&ip) => Some(Holder::Name(name)),
            _ => None,
        })
    }

    pub fn describe(&self, now: Instant) -> String {
        let mut parts: Vec<String> = self.networks.iter().map(ToString::to_string).collect();
        parts.extend(self.names.iter().map(|(name, state)| match state {
            NameState::Trusted { addrs, until, .. } if now < *until => format!("{name} ({})", addresses(addrs)),
            NameState::Trusted { .. } => format!("{name} (its addresses expired)"),
            NameState::Pending => format!("{name} (not looked up yet)"),
            NameState::Refused(_) => format!("{name} (refused)"),
            NameState::Unresolved(_) => format!("{name} (not resolved)"),
        }));
        parts.join(", ")
    }
}

#[derive(Clone, Debug)]
pub struct Moment {
    pub set: Arc<TrustSet>,
    pub at: Instant,
}

impl TrustsPeer for Moment {
    fn holder(&self, peer: IpAddr) -> Option<Holder<'_>> {
        self.set.holder_at(peer, self.at)
    }
}

#[derive(Debug)]
pub struct Trust {
    current: Mutex<Arc<TrustSet>>,
}

impl Trust {
    pub fn new(set: TrustSet) -> Self {
        Self { current: Mutex::new(Arc::new(set)) }
    }

    pub fn set(&self) -> Arc<TrustSet> {
        Arc::clone(&self.current.lock().unwrap_or_else(PoisonError::into_inner))
    }

    pub fn now(&self) -> Moment {
        Moment { set: self.set(), at: Instant::now() }
    }

    fn replace(&self, set: TrustSet) {
        *self.current.lock().unwrap_or_else(PoisonError::into_inner) = Arc::new(set);
    }
}

pub fn next_state(old: &NameState, answer: Result<Vec<IpAddr>, String>, now: Instant) -> NameState {
    let why = match answer {
        Ok(found) => {
            return match judge(&found) {
                Answer::Trusted(addrs) => NameState::Trusted { addrs, until: now + STALE_AFTER, failing: None },
                Answer::Refused(why) => NameState::Refused(why),
            };
        }
        Err(e) => ssx_core::text::shown_max(&e, ERROR_CHARS).to_string(),
    };
    match old {
        NameState::Trusted { addrs, until, .. } if now < *until => {
            NameState::Trusted { addrs: addrs.clone(), until: *until, failing: Some(why) }
        }
        _ => NameState::Unresolved(why),
    }
}

pub fn transition(name: &HostName, old: &NameState, new: &NameState) -> Option<(bool, String)> {
    use NameState::*;
    let stay = STALE_AFTER.as_secs();
    match (old, new) {
        (Trusted { addrs: a, failing: fa, .. }, Trusted { addrs: b, failing: fb, .. }) if a == b => match (fa, fb) {
            (None, Some(why)) => Some((
                true,
                format!(
                    "the trusted name {name} did not resolve ({why}): its addresses {} stay trusted at most {stay} s \
                     after their last answer",
                    addresses(a)
                ),
            )),
            (Some(_), None) => Some((false, format!("the trusted name {name} resolves again: {}", addresses(b)))),
            _ => None,
        },
        (Trusted { addrs: a, .. }, Trusted { addrs: b, .. }) => {
            Some((false, format!("the trusted name {name} now resolves to {} (was {})", addresses(b), addresses(a))))
        }
        (_, Trusted { addrs, .. }) => {
            Some((false, format!("the trusted name {name} resolves to {}", addresses(addrs))))
        }
        (Refused(a), Refused(b)) if a == b => None,
        (_, Refused(why)) => {
            Some((true, format!("the trusted name {name} is refused ({why}): none of its addresses is trusted")))
        }
        (Unresolved(a), Unresolved(b)) if a == b => None,
        (Trusted { addrs, .. }, Unresolved(why)) => Some((
            true,
            format!(
                "the trusted name {name} gave no answer for {stay} s ({why}): its addresses {} are no longer trusted",
                addresses(addrs)
            ),
        )),
        (_, Unresolved(why)) => Some((
            true,
            format!("the trusted name {name} does not resolve ({why}): no address of it is trusted until it does"),
        )),
        (_, Pending) => None,
    }
}

type Reply = oneshot::Sender<Result<Vec<IpAddr>, String>>;

pub struct Lookups {
    resolver: Arc<dyn Resolve>,
    timeout: Duration,
    requests: Option<std::sync::mpsc::Sender<(HostName, Reply)>>,
    late: Option<oneshot::Receiver<Result<Vec<IpAddr>, String>>>,
}

impl Lookups {
    pub fn new(resolver: Arc<dyn Resolve>) -> Self {
        Self::with_timeout(resolver, LOOKUP_TIMEOUT)
    }

    pub fn with_timeout(resolver: Arc<dyn Resolve>, timeout: Duration) -> Self {
        Self { resolver, timeout, requests: None, late: None }
    }

    fn thread(&mut self) -> Option<&std::sync::mpsc::Sender<(HostName, Reply)>> {
        if self.requests.is_none() {
            let (tx, rx) = std::sync::mpsc::channel::<(HostName, Reply)>();
            let resolver = Arc::clone(&self.resolver);
            let started = std::thread::Builder::new().name("trusted-names".into()).spawn(move || {
                while let Ok((name, reply)) = rx.recv() {
                    let _ = reply.send(resolver.resolve(&name));
                }
            });
            if let Err(e) = started {
                warn!("the trusted names' lookup thread did not start: {e}");
                return None;
            }
            self.requests = Some(tx);
        }
        self.requests.as_ref()
    }

    pub async fn lookup(&mut self, name: &HostName) -> Result<Vec<IpAddr>, String> {
        let waited = waited(self.timeout);
        if let Some(late) = &mut self.late {
            match late.try_recv() {
                Err(oneshot::error::TryRecvError::Empty) => {
                    return Err(format!("an earlier lookup has not ended (no answer within {waited})"));
                }
                Ok(_) | Err(oneshot::error::TryRecvError::Closed) => self.late = None,
            }
        }
        let (reply, mut answer) = oneshot::channel();
        let sent = match self.thread() {
            Some(requests) => requests.send((name.clone(), reply)).is_ok(),
            None => return Err("the lookup thread did not start".into()),
        };
        if !sent {
            self.requests = None;
            return Err("the lookup thread ended".into());
        }
        match tokio::time::timeout(self.timeout, &mut answer).await {
            Ok(Ok(found)) => found,
            Ok(Err(_)) => {
                self.requests = None;
                Err("the lookup thread ended".into())
            }
            Err(_) => {
                self.late = Some(answer);
                Err(format!("no answer within {waited}"))
            }
        }
    }
}

fn waited(d: Duration) -> String {
    if d.subsec_millis() == 0 { format!("{} s", d.as_secs()) } else { format!("{} ms", d.as_millis()) }
}

pub async fn refresh(trust: &Trust, lookups: &mut Lookups) {
    let old = trust.set();
    let mut names = Vec::with_capacity(old.names.len());
    for (name, state) in &old.names {
        let answer = lookups.lookup(name).await;
        let new = next_state(state, answer, Instant::now());
        match transition(name, state, &new) {
            Some((true, line)) => warn!("{line}"),
            Some((false, line)) => info!("{line}"),
            None => {}
        }
        names.push((name.clone(), new));
    }
    trust.replace(TrustSet { networks: old.networks.clone(), names });
}

pub fn looked_up(config: &crate::config::Config, resolver: Arc<dyn Resolve>) -> Result<Moment, String> {
    let trust = Trust::new(TrustSet::new(config.trusted.networks().to_vec(), config.trusted_hosts.clone()));
    if !config.trusted_hosts.is_empty() {
        let runtime = tokio::runtime::Builder::new_current_thread().enable_time().build().map_err(|e| e.to_string())?;
        runtime.block_on(refresh(&trust, &mut Lookups::new(resolver)));
    }
    Ok(trust.now())
}

pub fn name_line(name: &HostName, state: &NameState) -> String {
    match state {
        NameState::Trusted { addrs, .. } => format!(
            "trusted name {name}: {} (looked up again every {} s while the master runs)",
            addresses(addrs),
            REFRESH.as_secs()
        ),
        NameState::Refused(why) => {
            format!("warning: the trusted name {name} is refused ({why}): none of its addresses is trusted")
        }
        NameState::Unresolved(why) => format!(
            "warning: the trusted name {name} does not resolve here ({why}): check sees the network it runs in; run \
             it inside the master's container (docker exec <container> avalanche-master check --config …), where \
             Coolify's proxy, the container coolify-proxy, shares the master's network"
        ),
        NameState::Pending => format!("note: the trusted name {name} was not looked up"),
    }
}

pub async fn keep_fresh(master: Arc<crate::state::Master>, mut lookups: Lookups) {
    let mut tick = tokio::time::interval(REFRESH);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    tick.tick().await;
    loop {
        tick.tick().await;
        refresh(&master.trust, &mut lookups).await;
    }
}

#[cfg(test)]
mod tests;
