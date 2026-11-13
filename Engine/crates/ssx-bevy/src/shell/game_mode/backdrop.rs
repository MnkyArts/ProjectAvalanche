use std::sync::mpsc::{Receiver, TryRecvError};
use std::sync::{Arc, Mutex};

use bevy::prelude::{Assets, Handle, Image, error, info};
use ssx_assets::content::GameContent;
use ssx_assets::content::fmv::port::fe_background_id;
use ssx_content::ContentId;

use crate::ui::movie::{LoopingMovie, LoopingStart};

enum Decided {
    Empty,
    Started(Box<LoopingStart>),
    Failed(String),
}

enum State {
    Idle(Option<u64>),
    Deciding { generation: u64, rx: Mutex<Receiver<Decided>> },
    Playing { generation: u64, movie: LoopingMovie },
}

pub(super) struct BackdropMovie {
    content: Arc<GameContent>,
    slot: ContentId,
    state: State,
}

impl BackdropMovie {
    pub(super) fn new(content: Arc<GameContent>) -> Self {
        Self { content, slot: fe_background_id(), state: State::Idle(None) }
    }

    // PORT(choice): a hidden slot movie (the sky drawn at a range, a level, a full-screen movie) stops and starts from
    // its beginning when the globe shows again, rather than pausing with its decoder's process held (the slot is the
    // port's own; the disc's backdrop is a still picture).
    pub(super) fn frame(&mut self, shown: bool, dt: f64, images: &mut Assets<Image>) -> Option<Handle<Image>> {
        if !shown {
            if let State::Playing { movie, .. } = &self.state {
                info!("shell: the globe's backdrop stopped (the globe hidden): {}", movie.picture_counts());
            }
            if !matches!(self.state, State::Idle(_)) {
                self.state = State::Idle(None);
            }
            return None;
        }
        let now = self.content.generation();
        self.follow_changes(now);
        match &mut self.state {
            State::Idle(Some(_)) => None,
            State::Idle(None) => {
                self.decide(now);
                None
            }
            State::Deciding { generation, rx } => {
                let got = rx.get_mut().map_or(Err(TryRecvError::Disconnected), |rx| rx.try_recv());
                let generation = *generation;
                match got {
                    Err(TryRecvError::Empty) => None,
                    Ok(Decided::Empty) => {
                        self.state = State::Idle(Some(generation));
                        None
                    }
                    Ok(Decided::Started(start)) => {
                        let movie = LoopingMovie::show(*start, images);
                        info!("shell: the globe's backdrop plays {} (looping, without its sound)", movie.describe());
                        self.state = State::Playing { generation, movie };
                        None
                    }
                    Ok(Decided::Failed(why)) => {
                        error!("shell: the globe's backdrop {}: {why}; the disc's picture instead", self.slot);
                        self.state = State::Idle(Some(generation));
                        None
                    }
                    Err(TryRecvError::Disconnected) => {
                        error!("shell: the globe's backdrop {}: its worker ended without an answer", self.slot);
                        self.state = State::Idle(Some(generation));
                        None
                    }
                }
            }
            State::Playing { generation, movie } => {
                if !movie.advance(dt, images) {
                    return movie.picture().cloned();
                }
                match movie.failure() {
                    Some((id, source, why)) => {
                        error!(
                            "shell: the globe's backdrop {id} of {source} stopped: {why}; the disc's picture instead"
                        );
                        info!("shell: the globe's backdrop: {}", movie.picture_counts());
                        self.content.refuse_fmv(id, source, why);
                        self.state = State::Idle(None);
                    }
                    None => {
                        info!("shell: the globe's backdrop ended: {}", movie.picture_counts());
                        self.state = State::Idle(Some(*generation));
                    }
                }
                None
            }
        }
    }

    fn follow_changes(&mut self, now: u64) {
        let at = match &self.state {
            State::Idle(Some(g)) | State::Deciding { generation: g, .. } | State::Playing { generation: g, .. } => *g,
            State::Idle(None) => return,
        };
        if at == now {
            return;
        }
        if self.content.changes_since(at).is_none_or(|ids| ids.contains(&self.slot)) {
            self.state = State::Idle(None);
        } else if let State::Idle(Some(g))
        | State::Deciding { generation: g, .. }
        | State::Playing { generation: g, .. } = &mut self.state
        {
            *g = now;
        }
    }

    fn decide(&mut self, generation: u64) {
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        let (content, slot) = (self.content.clone(), self.slot.clone());
        let spawned = std::thread::Builder::new().name("ssx_backdrop_movie".into()).spawn(move || {
            let decided = match content.fmv(&slot) {
                None => Decided::Empty,
                Some(fmv) => match LoopingMovie::start(&fmv, &content) {
                    Ok(start) => Decided::Started(Box::new(start)),
                    Err(e) => Decided::Failed(format!("{e:#}")),
                },
            };
            let _ = tx.send(decided);
        });
        self.state = match spawned {
            Ok(_) => State::Deciding { generation, rx: Mutex::new(rx) },
            Err(e) => {
                error!("shell: the globe's backdrop {}: no worker thread ({e}); the disc's picture instead", self.slot);
                State::Idle(Some(generation))
            }
        };
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::time::{Duration, Instant};

    use ssx_content::PackageSource;
    use ssx_content::movie::write::*;

    use super::*;

    fn package(dir: &std::path::Path) -> PackageSource {
        std::fs::create_dir_all(dir.join("movies")).unwrap();
        let tracks = element(0x1654_AE6B, &video_track(1, 320, 180, &av1_private(BASE_SEQ), Some(33_333_333)));
        let blocks = [simple_block(1, 0, 0x80, &unit()), simple_block(1, 34, 0, &unit())];
        std::fs::write(dir.join("movies/loop.webm"), file(&[info(1000.0), tracks, cluster(0, &blocks)])).unwrap();
        let manifest = "[resource]\nname = \"broken-backdrop\"\nversion = \"1.0.0\"\napi = 1\n[override]\n\
                        \"ava:movie/fe_background\" = \"movies/loop.webm\"\n";
        std::fs::write(dir.join("resource.toml"), manifest).unwrap();
        PackageSource::open(dir).unwrap()
    }

    fn settle(b: &mut BackdropMovie, images: &mut Assets<Image>) -> usize {
        let deadline = Instant::now() + Duration::from_secs(30);
        for n in 1.. {
            assert_eq!(b.frame(true, 0.005, images), None, "the disc's picture");
            if matches!(b.state, State::Idle(Some(_))) || Instant::now() > deadline {
                return n;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        unreachable!()
    }

    #[test]
    fn an_empty_slot_and_a_failing_movie_leave_the_discs_picture() {
        let root = std::env::temp_dir().join(format!("ssx-backdrop-{}-absent", std::process::id()));
        let dir = std::env::temp_dir().join(format!("ssx-backdrop-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let content = GameContent::of(&root);
        let mut images = Assets::<Image>::default();
        let mut b = BackdropMovie::new(content.clone());
        settle(&mut b, &mut images);
        assert!(matches!(b.state, State::Idle(Some(0))), "decided once: empty");
        assert_eq!(b.frame(true, 0.005, &mut images), None);
        assert!(matches!(b.state, State::Idle(Some(0))), "not decided again at the same generation");

        let slot = fe_background_id();
        content.swap_dev_package(None, package(&dir), BTreeSet::from([slot.clone()]));
        settle(&mut b, &mut images);
        assert!(matches!(b.state, State::Idle(Some(1))), "decided again at the change: empty after the failure");
        assert_eq!(content.fmv(&slot), None, "the failed package passed over for the session");

        content.swap_dev_package(
            Some("broken-backdrop"),
            package(&dir),
            BTreeSet::from(["x:movie/y".parse().unwrap()]),
        );
        assert_eq!(b.frame(true, 0.005, &mut images), None);
        assert!(matches!(b.state, State::Idle(Some(2))), "a change of another id: followed, not decided again");
        assert_eq!(b.frame(false, 0.005, &mut images), None);
        assert!(matches!(b.state, State::Idle(Some(2))));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
