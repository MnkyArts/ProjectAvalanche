use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::JoinHandle;

use bevy::log::{info, warn};
use ssx_assets::display::DisplaySettings;

pub(super) struct Keeper {
    to: Option<Sender<DisplaySettings>>,
    worker: Option<JoinHandle<()>>,
}

impl Keeper {
    pub(super) fn start(file: PathBuf, kept: DisplaySettings) -> std::io::Result<Self> {
        let (to, from) = mpsc::channel();
        let worker = std::thread::Builder::new().name("display-keep".into()).spawn(move || work(&file, kept, &from))?;
        Ok(Keeper { to: Some(to), worker: Some(worker) })
    }

    pub(super) fn keep(&self, settings: DisplaySettings) {
        if let Some(to) = &self.to
            && to.send(settings).is_err()
        {
            warn!("display: the settings' writer has ended: the window's state is not kept");
        }
    }

    pub(super) fn finish(&mut self) {
        self.to.take();
        if let Some(worker) = self.worker.take()
            && worker.join().is_err()
        {
            warn!("display: the settings' writer stopped with a panic");
        }
    }
}

impl Drop for Keeper {
    fn drop(&mut self) {
        self.finish();
    }
}

fn work(file: &Path, mut kept: DisplaySettings, from: &Receiver<DisplaySettings>) {
    let mut failed = None;
    while let Ok(mut next) = from.recv() {
        while let Ok(newer) = from.try_recv() {
            next = newer;
        }
        failed = write(file, &mut kept, next);
    }
    if let Some(last) = failed
        && write(file, &mut kept, last).is_some()
    {
        warn!("display: the window's state was not kept at the exit");
    }
}

fn write(file: &Path, kept: &mut DisplaySettings, next: DisplaySettings) -> Option<DisplaySettings> {
    if next == *kept {
        return None;
    }
    match ssx_assets::display::write_member(file, kept, &next) {
        Ok(()) => {
            info!("display: kept in {}", file.display());
            *kept = next;
            None
        }
        Err(e) => {
            warn!("display: {e:#}");
            Some(next)
        }
    }
}
