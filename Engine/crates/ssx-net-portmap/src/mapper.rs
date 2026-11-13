use std::net::Ipv4Addr;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::session::{self, Made, Session};
use crate::{Config, Status, Targets, gateway};

// PORT(choice): 5 minutes (a gateway that comes up after the server, an operator who turns UPnP on); tuning: operators'
// reports; the original has no dedicated server.
pub const RETRY: Duration = Duration::from_secs(300);
// PORT(choice): 3 s for every removal (one UPnP request takes milliseconds on a LAN; NAT-PMP and PCP get two quick
// tries each); tuning: real gateways' times; the original has no dedicated server.
pub const STOP_WAIT: Duration = Duration::from_secs(3);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    Info,
    Warn,
}

pub type Log = Arc<dyn Fn(Level, &str) + Send + Sync>;

pub struct Mapper {
    stop: Option<mpsc::Sender<()>>,
    done: mpsc::Receiver<()>,
    thread: Option<JoinHandle<()>>,
    status: Arc<Mutex<Status>>,
    first: FirstAttempt,
}

#[derive(Clone, Default)]
pub struct FirstAttempt(Arc<(Mutex<bool>, Condvar)>);

impl FirstAttempt {
    fn done(&self) {
        let (m, c) = &*self.0;
        if let Ok(mut d) = m.lock() {
            *d = true;
        }
        c.notify_all();
    }

    pub fn wait(&self, max: Duration) -> bool {
        let (m, c) = &*self.0;
        let Ok(guard) = m.lock() else { return true };
        match c.wait_timeout_while(guard, max, |d| !*d) {
            Ok((d, _)) => *d,
            Err(_) => true,
        }
    }
}

impl Mapper {
    pub fn start(cfg: Config, targets: Targets, log: Log) -> std::io::Result<Self> {
        let status = Arc::new(Mutex::new(Status { summary: "starting".into(), ..Status::default() }));
        let (stop, stopped) = mpsc::channel();
        let (done_tx, done) = mpsc::channel();
        let shared = Arc::clone(&status);
        let first = FirstAttempt::default();
        let first_done = first.clone();
        let thread = std::thread::Builder::new().name("portmap".into()).spawn(move || {
            run(&cfg, &targets, &log, &shared, &stopped, &first_done);
            first_done.done();
            let _ = done_tx.send(());
        })?;
        Ok(Mapper { stop: Some(stop), done, thread: Some(thread), status, first })
    }

    pub fn status(&self) -> Arc<Mutex<Status>> {
        Arc::clone(&self.status)
    }

    pub fn first_attempt(&self) -> FirstAttempt {
        self.first.clone()
    }

    pub fn stop(mut self) {
        self.stop.take();
        if self.done.recv_timeout(STOP_WAIT + Duration::from_secs(1)).is_ok()
            && let Some(t) = self.thread.take()
        {
            let _ = t.join();
        }
    }
}

impl Drop for Mapper {
    fn drop(&mut self) {
        self.stop.take();
    }
}

fn set(status: &Mutex<Status>, f: impl FnOnce(&mut Status)) {
    if let Ok(mut s) = status.lock() {
        f(&mut s);
    }
}

fn wait(stopped: &mpsc::Receiver<()>, d: Duration) -> bool {
    !matches!(stopped.recv_timeout(d), Err(RecvTimeoutError::Timeout))
}

fn ports_text(cfg: &Config) -> String {
    cfg.ports.iter().map(ToString::to_string).collect::<Vec<_>>().join(", ")
}

fn run(
    cfg: &Config,
    targets: &Targets,
    log: &Log,
    status: &Mutex<Status>,
    stopped: &mpsc::Receiver<()>,
    first: &FirstAttempt,
) {
    let gateway: Ipv4Addr = match cfg.gateway.map(Ok).unwrap_or_else(gateway::default_gateway) {
        Ok(g) => g,
        Err(why) => {
            log(Level::Warn, &format!("port mapping: off: {why}"));
            set(status, |s| {
                s.failures += 1;
                s.last_error = Some(why.clone());
                s.summary = format!("not mapped: {why}");
            });
            first.done();
            let _ = stopped.recv();
            return;
        }
    };
    set(status, |s| s.gateway = Some(gateway));
    let mut active: Option<Session> = None;
    loop {
        let made = match active.take() {
            Some(session) => {
                let method = session.method();
                match session::renew(session, gateway, cfg, targets) {
                    Ok(made) => {
                        set(status, |s| s.renewals += 1);
                        Some(Ok(made))
                    }
                    Err(e) => {
                        log(
                            Level::Warn,
                            &format!(
                                "port mapping: renewing by {} at {gateway} failed: {e}; trying every method again",
                                method.title()
                            ),
                        );
                        attempt(gateway, cfg, targets, log, status, stopped)
                    }
                }
            }
            None => attempt(gateway, cfg, targets, log, status, stopped),
        };
        first.done();
        match made {
            Some(Ok(made)) => {
                let Made { session, external, lifetime } = made;
                let renew = session::renew_after(lifetime);
                set(status, |s| {
                    s.mapped = true;
                    s.method = Some(session.method());
                    s.external = external;
                    s.lifetime = lifetime;
                    s.last_error = None;
                    s.summary = format!(
                        "{} at {gateway}{}: {} mapped{}, {}",
                        session.method().title(),
                        session.detail(),
                        ports_text(cfg),
                        external.map_or(String::new(), |e| format!(" on {e}")),
                        if lifetime == 0 {
                            "permanent (added again every 30 minutes)".to_string()
                        } else {
                            format!("for {lifetime} s, renewed every {} s", renew.as_secs())
                        }
                    );
                });
                if wait(stopped, renew) {
                    let failed =
                        session::remove(&session, gateway, cfg, targets, &cfg.ports, Instant::now() + STOP_WAIT);
                    set(status, |s| {
                        s.mapped = false;
                        s.removals += (cfg.ports.len() - failed.len()) as u64;
                        s.summary = "removed at the stop".into();
                    });
                    if failed.is_empty() {
                        log(Level::Info, &format!("port mapping: removed {} at {gateway}", ports_text(cfg)));
                    } else {
                        log(
                            Level::Warn,
                            &format!(
                                "port mapping: not removed at {gateway}: {} (they lapse by themselves)",
                                failed.join("; ")
                            ),
                        );
                    }
                    return;
                }
                active = Some(session);
            }
            Some(Err(())) => {
                set(status, |s| s.mapped = false);
                if wait(stopped, RETRY) {
                    return;
                }
            }
            None => return,
        }
    }
}

fn attempt(
    gateway: Ipv4Addr,
    cfg: &Config,
    targets: &Targets,
    log: &Log,
    status: &Mutex<Status>,
    stopped: &mpsc::Receiver<()>,
) -> Option<Result<Made, ()>> {
    let mut reasons = Vec::new();
    for m in &cfg.methods {
        match session::start(*m, gateway, cfg, targets) {
            Ok(made) => {
                if matches!(stopped.try_recv(), Err(mpsc::TryRecvError::Disconnected) | Ok(())) {
                    let _ =
                        session::remove(&made.session, gateway, cfg, targets, &cfg.ports, Instant::now() + STOP_WAIT);
                    return None;
                }
                set(status, |s| s.maps += 1);
                log(
                    Level::Info,
                    &format!(
                        "port mapping: {} at {gateway}{} mapped {}{} ({})",
                        m.title(),
                        made.session.detail(),
                        ports_text(cfg),
                        made.external.map_or(String::new(), |e| format!(" on {e}")),
                        if made.lifetime == 0 { "permanent".to_string() } else { format!("{} s", made.lifetime) }
                    ),
                );
                return Some(Ok(made));
            }
            Err(e) => reasons.push(format!("{}: {e}", m.title())),
        }
        if matches!(stopped.try_recv(), Err(mpsc::TryRecvError::Disconnected) | Ok(())) {
            return None;
        }
    }
    let why = reasons.join("; ");
    log(Level::Warn, &format!("port mapping at {gateway} failed ({why}); tried again in {} s", RETRY.as_secs()));
    set(status, |s| {
        s.failures += 1;
        s.last_error = Some(why.clone());
        s.summary = format!("not mapped at {gateway}: {why}");
    });
    Some(Err(()))
}
