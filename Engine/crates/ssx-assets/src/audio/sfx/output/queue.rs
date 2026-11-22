use std::sync::OnceLock;
use std::sync::atomic::Ordering::Relaxed;
use std::time::{Duration, Instant};

use super::stats;
use crate::sfx::OutputMix;
use crate::sfx_queue::{Receiver, Sender, queue};

pub type Command = Box<dyn FnOnce(&mut OutputMix) + Send>;

struct Queued {
    kind: &'static str,
    apply: Command,
}

#[derive(Clone)]
pub struct MixQueue {
    tx: Sender<Queued>,
}

pub struct MixInbox {
    rx: Receiver<Queued>,
}

pub fn mix_queue() -> (MixQueue, MixInbox) {
    let (tx, rx) = queue();
    (MixQueue { tx }, MixInbox { rx })
}

impl MixQueue {
    pub fn post<F: FnOnce(&mut OutputMix) + Send + 'static>(&self, f: F) -> bool {
        self.post_as(std::any::type_name::<F>(), f)
    }

    pub fn post_as(&self, kind: &'static str, f: impl FnOnce(&mut OutputMix) + Send + 'static) -> bool {
        let sent = self.tx.send(Queued { kind, apply: Box::new(f) }).is_ok();
        if sent {
            stats::POSTED.fetch_add(1, Relaxed);
        }
        sent
    }
}

impl MixInbox {
    pub fn drain(&mut self, mix: &mut OutputMix) -> usize {
        self.drain_each(mix, |_, _| {})
    }

    pub fn drain_each(&mut self, mix: &mut OutputMix, mut each: impl FnMut(&'static str, Duration)) -> usize {
        let mut n = 0;
        while let Ok(command) = self.rx.try_recv() {
            let start = Instant::now();
            (command.apply)(mix);
            each(command.kind, start.elapsed());
            n += 1;
        }
        n
    }
}

// PORT(choice): the port's mixer thread hands what a command took out of the mix (a level's or the front end's engine,
// a music source) to a thread that frees it (`ssx_audio_reaper`); the console has one engine for the whole game and
// frees nothing on its mixer thread.
pub fn discard(x: impl Send + 'static) {
    static REAPER: OnceLock<Option<Sender<Box<dyn Send>>>> = OnceLock::new();
    let reaper = REAPER.get_or_init(|| {
        let (tx, mut rx) = queue::<Box<dyn Send>>();
        let spawned = std::thread::Builder::new().name("ssx_audio_reaper".into()).spawn(move || {
            while let Ok(x) = rx.recv() {
                drop(x);
            }
        });
        spawned.ok().map(|_| tx)
    });
    match reaper {
        Some(tx) => drop(tx.send(Box::new(x))),
        None => drop(x),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_apply_in_order_and_fail_without_a_mixer() {
        let (q, mut inbox) = mix_queue();
        let mut mix = OutputMix::new(None, 44_100, None);
        for k in 0..5 {
            assert!(q.post(move |m| m.music_volume = m.music_volume * 10.0 + k as f32));
        }
        assert_eq!(inbox.drain(&mut mix), 5);
        assert_eq!(mix.music_volume, 101_234.0);
        assert_eq!(inbox.drain(&mut mix), 0);
        assert!(q.post(|m| m.music_volume = 0.0));
        assert!(q.post_as("the caller's", |m| m.music_volume = 1.0));
        let mut kinds = Vec::new();
        assert_eq!(inbox.drain_each(&mut mix, |kind, _| kinds.push(kind)), 2);
        assert!(kinds[0].contains("commands_apply_in_order_and_fail_without_a_mixer"), "{}", kinds[0]);
        assert_eq!(kinds[1], "the caller's");
        drop(inbox);
        assert!(!q.post(|m| m.music_volume = 0.0));
    }

    #[test]
    fn a_discarded_value_is_dropped() {
        struct Flag(std::sync::mpsc::Sender<()>);
        impl Drop for Flag {
            fn drop(&mut self) {
                let _ = self.0.send(());
            }
        }
        let (tx, rx) = std::sync::mpsc::channel();
        discard(Flag(tx));
        assert!(rx.recv_timeout(std::time::Duration::from_secs(10)).is_ok());
    }
}
