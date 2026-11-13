use std::collections::VecDeque;
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use super::{Read, ResourceCommand, Started};

pub(super) trait Reading: Send + Sync + 'static {
    fn read(&self, command: &ResourceCommand, running: Vec<Started>, had_ids: bool) -> Result<Read, String>;
    fn remount(&self, running: &[Started]) -> Result<(), String>;
}

// PORT(choice): 4 replacements in 10 minutes (a read panics only on a defect: a command an operator repeats would
// otherwise start a thread per try; a later defect, minutes on, is still read); tuning: a defect seen in the field; the
// original has no dedicated server.
pub(super) const MAX_RESTARTS: usize = 4;
pub(super) const RESTART_WINDOW: Duration = Duration::from_secs(600);

enum Job {
    Read { command: ResourceCommand, running: Vec<Started>, had_ids: bool },
    Remount { running: Vec<Started> },
}

pub(super) enum Answer {
    Read(ResourceCommand, Result<Box<Read>, String>),
    Remounted(Result<(), String>),
    Ended(Option<ResourceCommand>, String),
}

struct Thread {
    jobs: Sender<Job>,
    answers: Mutex<Receiver<Answer>>,
    handle: JoinHandle<()>,
}

enum Busy {
    Read(ResourceCommand),
    Remount,
}

pub(super) struct Reader {
    reading: Arc<dyn Reading>,
    thread: Option<Thread>,
    busy: Option<Busy>,
    remount: bool,
    restarts: VecDeque<Instant>,
    started: u64,
    refusing: bool,
}

impl Reader {
    pub(super) fn start(reading: Arc<dyn Reading>) -> Result<Self, String> {
        let mut r = Self {
            reading,
            thread: None,
            busy: None,
            remount: false,
            restarts: VecDeque::new(),
            started: 0,
            refusing: false,
        };
        r.thread = Some(r.spawn()?);
        Ok(r)
    }

    pub(super) fn busy(&self) -> bool {
        self.busy.is_some()
    }

    pub(super) fn needs_remount(&self) -> bool {
        self.remount
    }

    pub(super) fn started(&self) -> u64 {
        self.started
    }

    pub(super) fn read(
        &mut self,
        command: ResourceCommand,
        running: Vec<Started>,
        had_ids: bool,
        now: Instant,
    ) -> Result<(), (ResourceCommand, String)> {
        if let Err(why) = self.ensure_thread(now) {
            return Err((command, why));
        }
        self.busy = Some(Busy::Read(command.clone()));
        let kept = command.clone();
        self.send(Job::Read { command, running, had_ids }).map_err(|why| (kept, why))
    }

    pub(super) fn remount(&mut self, running: Vec<Started>, now: Instant) -> Result<(), String> {
        self.ensure_thread(now)?;
        self.busy = Some(Busy::Remount);
        self.send(Job::Remount { running })
    }

    pub(super) fn poll(&mut self) -> Option<Answer> {
        self.busy.as_ref()?;
        let Some(t) = self.thread.as_ref() else {
            let (command, why) = self.ended();
            return Some(Answer::Ended(command, why));
        };
        let answers = t.answers.lock().unwrap_or_else(PoisonError::into_inner);
        let got = match answers.try_recv() {
            Ok(a) => Some(a),
            Err(TryRecvError::Empty) if !t.handle.is_finished() => return None,
            Err(TryRecvError::Empty | TryRecvError::Disconnected) => None,
        };
        drop(answers);
        match got {
            Some(a) => {
                if matches!(a, Answer::Remounted(_)) {
                    self.remount = false;
                }
                self.busy = None;
                Some(a)
            }
            None => {
                let (command, why) = self.ended();
                Some(Answer::Ended(command, why))
            }
        }
    }

    fn ended(&mut self) -> (Option<ResourceCommand>, String) {
        let why = match self.thread.take().map(|t| t.handle.join()) {
            Some(Err(payload)) => {
                let text = payload
                    .downcast_ref::<&str>()
                    .map(|s| (*s).to_string())
                    .or_else(|| payload.downcast_ref::<String>().cloned())
                    .unwrap_or_else(|| "a panic without a text".into());
                format!("it panicked: {}", ssx_core::text::quoted_max(&text, 300))
            }
            _ => "its channel closed".into(),
        };
        self.remount = true;
        let command = match self.busy.take() {
            Some(Busy::Read(c)) => Some(c),
            _ => None,
        };
        (command, format!("the reading thread ended without its result ({why})"))
    }

    fn ensure_thread(&mut self, now: Instant) -> Result<(), String> {
        if self.thread.is_some() {
            return Ok(());
        }
        while self.restarts.front().is_some_and(|&t| now.saturating_duration_since(t) >= RESTART_WINDOW) {
            self.restarts.pop_front();
        }
        if self.restarts.len() >= MAX_RESTARTS {
            let left =
                self.restarts.front().map_or(RESTART_WINDOW, |&t| RESTART_WINDOW - now.saturating_duration_since(t));
            let why = format!(
                "the reading thread was replaced {MAX_RESTARTS} times within {} s and ended again; commands are \
                 refused for {} s more",
                RESTART_WINDOW.as_secs(),
                left.as_secs().max(1)
            );
            if !self.refusing {
                self.refusing = true;
                bevy::log::error!("resources: {why}");
            }
            return Err(why);
        }
        self.thread = Some(self.spawn()?);
        self.refusing = false;
        self.restarts.push_back(now);
        bevy::log::warn!(
            "resources: the reading thread started again ({} of {MAX_RESTARTS} within {} s)",
            self.restarts.len(),
            RESTART_WINDOW.as_secs()
        );
        Ok(())
    }

    fn send(&mut self, job: Job) -> Result<(), String> {
        let sent = match &self.thread {
            Some(t) => t.jobs.send(job).is_ok(),
            None => false,
        };
        if sent {
            return Ok(());
        }
        self.busy = None;
        Err(self.ended().1)
    }

    fn spawn(&mut self) -> Result<Thread, String> {
        let (jobs, inbox) = mpsc::channel::<Job>();
        let (outbox, answers) = mpsc::channel::<Answer>();
        let reading = Arc::clone(&self.reading);
        let handle = std::thread::Builder::new()
            .name("resources".into())
            .spawn(move || run(&*reading, &inbox, &outbox))
            .map_err(|e| format!("the reading thread does not start: {e}"))?;
        self.started += 1;
        Ok(Thread { jobs, answers: Mutex::new(answers), handle })
    }
}

fn run(reading: &dyn Reading, inbox: &Receiver<Job>, outbox: &Sender<Answer>) {
    while let Ok(job) = inbox.recv() {
        let answer = match job {
            Job::Read { command, running, had_ids } => {
                let result = reading.read(&command, running, had_ids).map(Box::new);
                Answer::Read(command, result)
            }
            Job::Remount { running } => Answer::Remounted(reading.remount(&running)),
        };
        if outbox.send(answer).is_err() {
            return;
        }
    }
}

#[cfg(test)]
mod tests;
