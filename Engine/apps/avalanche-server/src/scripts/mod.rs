mod catalog;
mod frame;
mod joins;
mod lifecycle;
mod rights;
mod thread;
mod world;

#[cfg(test)]
mod tests;

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender, TrySendError, sync_channel};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use bevy::prelude::*;
use ssx_script::host::game::Catalog;
use ssx_script::host::monitor::Monitor;

pub use catalog::catalog_of;
pub use joins::{DEFER_BOUND, FIRST_WORD, HeldJoins, next_join};
pub use lifecycle::transact;
pub use rights::{
    Identities, OperationOrders, OperationRights, OrderStats, apply_orders, record_requests, requests_at_start,
};
pub use world::{Pending, ScriptStats, ShutdownHeard, collect, shutdown_begun, step};

pub(crate) use thread::CommandReply;
use thread::{Done, Job};

pub const STORAGE_FOLDER: &str = "script-data";

const JOBS: usize = 64;

// PORT(choice): 30 s; each call of a change is cut at the server's call backstop (400 ms) and each chain at 800 ms, so
// a change of up to about 25 resources whose scripts each spend a whole call and a whole chain starting still answers,
// a larger change of such scripts is refused (and undone by the thread); tuning: the start times of real servers' sets;
// the original runs no mod scripts.
pub(crate) const ANSWER_BOUND: Duration = Duration::from_secs(30);

#[derive(Clone)]
pub struct ScriptsHandle {
    jobs: SyncSender<Job>,
}

impl ScriptsHandle {
    fn send(&self, job: Job) -> Result<(), String> {
        self.jobs.try_send(job).map_err(|e| match e {
            TrySendError::Full(_) => format!("{JOBS} jobs wait for the scripts' thread"),
            TrySendError::Disconnected(_) => "the scripts' thread has ended".to_string(),
        })
    }
}

#[derive(Resource)]
pub struct ServerScripts {
    pub handle: ScriptsHandle,
    done: Mutex<Receiver<Done>>,
    thread: Option<JoinHandle<()>>,
    in_flight: bool,
    pub monitor: Option<(Monitor, usize)>,
}

impl ServerScripts {
    pub fn spawn(data_dir: &Path) -> Result<Self, String> {
        let storage: PathBuf = data_dir.join(STORAGE_FOLDER);
        ssx_files::create_dir_all(&storage).map_err(|e| format!("{}: {e}", storage.display()))?;
        let (jobs, rx) = sync_channel(JOBS);
        let (tx, done) = sync_channel(JOBS);
        let (ready_tx, ready) = sync_channel(1);
        let thread = std::thread::Builder::new()
            .name("scripts".into())
            .spawn(move || thread::run(storage, rx, tx, ready_tx))
            .map_err(|e| format!("the scripts' thread does not start: {e}"))?;
        match ready.recv_timeout(ANSWER_BOUND) {
            Ok(Ok(())) => Ok(Self {
                handle: ScriptsHandle { jobs },
                done: Mutex::new(done),
                thread: Some(thread),
                in_flight: false,
                monitor: None,
            }),
            Ok(Err(e)) => Err(format!("the server's script host: {e}")),
            Err(RecvTimeoutError::Timeout) => {
                Err(format!("the scripts' thread did not start its host within {} s", ANSWER_BOUND.as_secs()))
            }
            Err(RecvTimeoutError::Disconnected) => Err("the scripts' thread ended before its host was up".into()),
        }
    }

    pub fn set_catalog(&self, catalog: Arc<Catalog>) -> Result<(), String> {
        self.handle.send(Job::Catalog(catalog))
    }

    pub fn status(&self, out: &mut Vec<String>) {
        match &self.monitor {
            Some((m, elements)) => {
                out.extend(m.render().lines().map(str::to_string));
                out.push(format!(
                    "scripts: {elements} elements in the tree (the players', riders', instances', scripts')"
                ));
            }
            None => out.push("scripts: no monitor yet".into()),
        }
    }
}

pub fn register(app: &mut App, scripts: ServerScripts) {
    let identities = Identities::default();
    if let Some(op) = app.world().get_resource::<crate::operation::commands::OperationRes>() {
        let rights = OperationRights { operation: Arc::clone(&op.0), players: identities.clone() };
        if let Err(e) = scripts.handle.send(Job::Rights(Arc::new(rights))) {
            error!("scripts: the operator's rights not set (every privileged call is refused): {e}");
        }
    }
    app.insert_resource(scripts).init_resource::<HeldJoins>().init_resource::<Pending>().init_resource::<ScriptStats>();
    app.insert_resource(identities).init_resource::<OperationOrders>().init_resource::<OrderStats>();
    app.init_resource::<ShutdownHeard>().add_observer(shutdown_begun);
    app.add_systems(
        Update,
        (collect, apply_orders, step).chain().before(crate::rounds::log_rounds).before(crate::systems::decide_joins),
    );
    app.add_systems(Update, record_requests.after(crate::resources::run_queue));
}

pub fn console(scripts: &ServerScripts, line: &str, reply: Option<CommandReply>) -> Result<(), String> {
    scripts.handle.send(Job::Command(line.to_string(), reply))
}

impl Drop for ServerScripts {
    fn drop(&mut self) {
        let deadline = Instant::now() + ANSWER_BOUND;
        let mut stop = Job::Stop;
        loop {
            match self.handle.jobs.try_send(stop) {
                Err(TrySendError::Full(job)) if Instant::now() < deadline => {
                    stop = job;
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(TrySendError::Full(_)) => {
                    error!(
                        "scripts: {JOBS} jobs still wait after {} s: the thread is not stopped",
                        ANSWER_BOUND.as_secs()
                    );
                    return;
                }
                Ok(()) | Err(TrySendError::Disconnected(_)) => break,
            }
        }
        let Some(t) = self.thread.take() else { return };
        while !t.is_finished() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        if !t.is_finished() {
            error!(
                "scripts: the thread did not end within {} s; its scripts' storage may be unwritten",
                ANSWER_BOUND.as_secs()
            );
        } else if t.join().is_err() {
            error!("scripts: the thread panicked");
        }
    }
}
