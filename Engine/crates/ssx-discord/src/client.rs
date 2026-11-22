use std::io;
use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, mpsc};
use std::time::{Duration, Instant};

use tracing::{debug, info, warn};

use crate::activity::Activity;
use crate::frame::{self, Decoder, Frame, Opcode};
use crate::limits::{RateWindow, Retry};
use crate::payload::{self, Inbound};
use crate::transport::{self, Transport};

#[derive(Clone, Debug)]
pub struct Config {
    pub client_id: String,
    pub endpoints: Vec<PathBuf>,
    pub timing: Timing,
}

#[derive(Clone, Copy, Debug)]
pub struct Timing {
    pub retry_first: Duration,
    pub retry_max: Duration,
    pub poll: Duration,
    pub settle: Duration,
    pub handshake: Duration,
    pub write: Duration,
    pub updates: usize,
    pub window: Duration,
}

impl Default for Timing {
    // PORT(choice): Discord's rate limit of 5 updates per 20 s (the Game SDK's documentation) held on this side;
    // a new activity waits 1 s unchanged; the connect is tried again after 2 s, doubling to 60 s (a Discord started
    // after the game is found within a minute; when none runs, a try a minute is a few failed `connect`s); a reply is
    // looked for every 250 ms; READY within 10 s; a write ends after 5 s. The console had no Discord.
    fn default() -> Self {
        Timing {
            retry_first: Duration::from_secs(2),
            retry_max: Duration::from_secs(60),
            poll: Duration::from_millis(250),
            settle: Duration::from_secs(1),
            handshake: Duration::from_secs(10),
            write: Duration::from_secs(5),
            updates: 5,
            window: Duration::from_secs(20),
        }
    }
}

struct Wanted {
    activity: Option<Activity>,
    serial: u64,
    changed: Instant,
    stop: bool,
}

struct Shared {
    wanted: Mutex<Wanted>,
    wake: Condvar,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, Wanted> {
        self.wanted.lock().unwrap_or_else(|p| p.into_inner())
    }
}

pub struct Client {
    shared: Arc<Shared>,
    stopped: Mutex<mpsc::Receiver<()>>,
}

const _: () = {
    const fn shareable<T: Send + Sync>() {}
    shareable::<Client>()
};

impl Client {
    pub fn start(config: Config) -> io::Result<Client> {
        if !payload::valid_client_id(&config.client_id) {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "the application id is not a snowflake"));
        }
        let shared = Arc::new(Shared {
            wanted: Mutex::new(Wanted { activity: None, serial: 0, changed: Instant::now(), stop: false }),
            wake: Condvar::new(),
        });
        let (done, stopped) = mpsc::channel();
        let thread_shared = Arc::clone(&shared);
        std::thread::Builder::new().name("discord-presence".into()).spawn(move || {
            Worker::new(config).run(&thread_shared);
            let _ = done.send(());
        })?;
        Ok(Client { shared, stopped: Mutex::new(stopped) })
    }

    pub fn set(&self, activity: Activity) {
        let mut w = self.shared.lock();
        if w.activity.as_ref() == Some(&activity) {
            return;
        }
        w.activity = Some(activity);
        w.serial += 1;
        w.changed = Instant::now();
        drop(w);
        self.shared.wake.notify_one();
    }

    pub fn stop(self, wait: Duration) -> bool {
        self.signal_stop();
        let stopped = self.stopped.lock().unwrap_or_else(|p| p.into_inner());
        stopped.recv_timeout(wait).is_ok()
    }

    fn signal_stop(&self) {
        self.shared.lock().stop = true;
        self.shared.wake.notify_one();
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        self.signal_stop();
    }
}

struct Connection {
    transport: Box<dyn Transport>,
    decoder: Decoder,
    endpoint: PathBuf,
    opened: Instant,
    ready: bool,
    shown: Option<Activity>,
    nonce: u64,
}

const READS_PER_WAKE: usize = 8;
const READ_CHUNK: usize = 16 * 1024;

struct Worker {
    config: Config,
    retry: Retry,
    rate: RateWindow,
    conn: Option<Connection>,
    wanted: Option<(u64, Activity, Instant)>,
    absent_said: bool,
    stepped: Instant,
    buf: Vec<u8>,
}

impl Worker {
    fn new(config: Config) -> Self {
        let now = Instant::now();
        Worker {
            retry: Retry::new(config.timing.retry_first, config.timing.retry_max, now),
            rate: RateWindow::new(config.timing.updates, config.timing.window),
            config,
            conn: None,
            wanted: None,
            absent_said: false,
            stepped: now,
            buf: vec![0; READ_CHUNK],
        }
    }

    fn run(&mut self, shared: &Shared) {
        loop {
            let mut w = shared.lock();
            loop {
                if w.stop {
                    if let Some(c) = self.conn.take() {
                        debug!("Discord Rich Presence: the connection to {} closed", c.endpoint.display());
                    }
                    return;
                }
                if self.wanted.as_ref().map(|(s, ..)| *s) != Some(w.serial)
                    && let Some(a) = &w.activity
                {
                    self.wanted = Some((w.serial, a.clone(), w.changed));
                }
                let now = Instant::now();
                let due = self.next_wake(now);
                if due <= now {
                    break;
                }
                w = shared.wake.wait_timeout(w, due - now).map_or_else(|p| p.into_inner().0, |r| r.0);
            }
            drop(w);
            self.step(Instant::now());
        }
    }

    fn next_wake(&mut self, now: Instant) -> Instant {
        let t = self.config.timing;
        let Some(c) = &self.conn else { return self.retry.at() };
        let mut at = self.stepped + t.poll;
        if !c.ready {
            return at.min(c.opened + t.handshake);
        }
        if let Some((_, a, changed)) = &self.wanted
            && c.shown.as_ref() != Some(a)
        {
            at = at.min((*changed + t.settle).max(self.rate.next_allowed(now)));
        }
        at
    }

    fn step(&mut self, now: Instant) {
        self.stepped = now;
        if self.conn.is_none() {
            if now >= self.retry.at() {
                self.connect(now);
            }
            return;
        }
        let result = self.pump().and_then(|()| self.handshake_in_time(now)).and_then(|()| self.send_wanted(now));
        if let Err(why) = result {
            self.lost(now, &why);
        }
    }

    fn connect(&mut self, now: Instant) {
        let hello = match frame::encode(Opcode::Handshake, &payload::handshake(&self.config.client_id)) {
            Ok(b) => b,
            Err(e) => {
                warn!("Discord Rich Presence: the handshake is not a frame ({e})");
                self.retry.failed(now);
                return;
            }
        };
        for endpoint in &self.config.endpoints {
            let Ok(mut transport) = transport::connect(endpoint, self.config.timing.write) else { continue };
            match transport.send(&hello) {
                Ok(()) => {
                    debug!("Discord Rich Presence: connected to {}, handshake sent", endpoint.display());
                    self.conn = Some(Connection {
                        transport,
                        decoder: Decoder::new(),
                        endpoint: endpoint.clone(),
                        opened: now,
                        ready: false,
                        shown: None,
                        nonce: 0,
                    });
                    return;
                }
                Err(e) => debug!("Discord Rich Presence: {} took no handshake ({e})", endpoint.display()),
            }
        }
        let delay = self.retry.failed(now);
        if !self.absent_said {
            self.absent_said = true;
            info!(
                "Discord Rich Presence: no Discord client answered ({} places looked at); looking again in the \
                 background (next in {} s)",
                self.config.endpoints.len(),
                delay.as_secs()
            );
        }
    }

    fn lost(&mut self, now: Instant, why: &str) {
        let endpoint = self.conn.take().map(|c| c.endpoint.display().to_string()).unwrap_or_default();
        let delay = self.retry.failed(now);
        self.absent_said = false;
        info!("Discord Rich Presence: {why} ({endpoint}); looking again in {} s", delay.as_secs());
    }

    fn pump(&mut self) -> Result<(), String> {
        let Some(c) = self.conn.as_mut() else { return Ok(()) };
        for _ in 0..READS_PER_WAKE {
            let n = match c.transport.receive(&mut self.buf) {
                Ok(Some(n)) => n,
                Ok(None) => break,
                Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => {
                    return Err("Discord closed the connection".into());
                }
                Err(e) => return Err(format!("the connection failed ({e})")),
            };
            let frames = c.decoder.feed(&self.buf[..n]).map_err(|e| format!("Discord sent {e}"))?;
            for f in frames {
                if handle(c, f, &mut self.retry)? {
                    self.absent_said = false;
                }
            }
        }
        Ok(())
    }

    fn handshake_in_time(&self, now: Instant) -> Result<(), String> {
        match &self.conn {
            Some(c) if !c.ready && now.saturating_duration_since(c.opened) >= self.config.timing.handshake => {
                Err(format!("no READY within {} s of the handshake", self.config.timing.handshake.as_secs()))
            }
            _ => Ok(()),
        }
    }

    fn send_wanted(&mut self, now: Instant) -> Result<(), String> {
        let t = self.config.timing;
        let (Some(c), Some((_, activity, changed))) = (self.conn.as_mut(), self.wanted.as_ref()) else {
            return Ok(());
        };
        if !c.ready || c.shown.as_ref() == Some(activity) || now < *changed + t.settle {
            return Ok(());
        }
        if self.rate.next_allowed(now) > now {
            return Ok(());
        }
        c.nonce += 1;
        let body = payload::set_activity(std::process::id(), activity, c.nonce);
        let bytes = frame::encode(Opcode::Frame, &body).map_err(|e| format!("the activity is not a frame ({e})"))?;
        c.transport.send(&bytes).map_err(|e| format!("the activity could not be sent ({e})"))?;
        self.rate.record(Instant::now());
        c.shown = Some(activity.clone());
        debug!("Discord Rich Presence: activity sent (nonce {})", c.nonce);
        Ok(())
    }
}

fn handle(c: &mut Connection, f: Frame, retry: &mut Retry) -> Result<bool, String> {
    match f.opcode {
        Opcode::Frame => match payload::inbound(&f.payload).map_err(|e| format!("Discord sent a payload {e}"))? {
            Inbound::Ready if !c.ready => {
                c.ready = true;
                retry.reset();
                info!("Discord Rich Presence: connected ({})", c.endpoint.display());
                return Ok(true);
            }
            Inbound::Reply { nonce, error: Some(e) } => warn!(
                "Discord Rich Presence: Discord refused the activity (nonce {nonce}, code {}): {}",
                e.code.map_or_else(|| "none".to_string(), |c| c.to_string()),
                e.message
            ),
            Inbound::Ready | Inbound::Reply { .. } | Inbound::Other => {}
        },
        Opcode::Ping => {
            let pong = frame::encode(Opcode::Pong, &f.payload).map_err(|e| format!("a PONG is not a frame ({e})"))?;
            c.transport.send(&pong).map_err(|e| format!("the PONG could not be sent ({e})"))?;
        }
        Opcode::Pong => {}
        Opcode::Close => {
            let r = payload::close_reason(&f.payload);
            let code = r.code.map_or_else(|| "no code".to_string(), |c| format!("code {c}"));
            return Err(format!("Discord closed the connection ({code}: {})", r.message));
        }
        Opcode::Handshake => return Err("Discord sent a handshake".into()),
    }
    Ok(false)
}
