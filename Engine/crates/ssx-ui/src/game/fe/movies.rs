use super::*;

impl FrontEnd {
    pub fn queue_movie(&mut self, m: MovieRequest) {
        if self.movie.is_none() {
            self.movie = Some(m);
            (self.movie_seen, self.movie_end) = (false, false);
            self.movie_serial += 1;
        } else {
            self.movie_queue.push_back(m);
        }
        self.movie_skip = -1;
    }

    pub fn movie_queued(&self) -> bool {
        self.movie.is_some() || !self.movie_queue.is_empty()
    }

    pub fn skip_movie(&mut self, frame: i32) {
        if !self.movie_queued() {
            return;
        }
        if 1 + self.movie_queue.len() != 1 {
            self.stop_movie();
        } else if frame >= MOVIE_SKIP_FROM_FRAME && self.movie_skip < 0 {
            self.note(format!("movie skip at frame {frame}: the stop in 3 updates (0x823F94F0)"));
            self.movie_skip = 3;
        } else if frame < MOVIE_SKIP_FROM_FRAME {
            self.note(format!("movie skip at frame {frame}: not before frame {MOVIE_SKIP_FROM_FRAME}"));
        }
    }

    pub(super) fn stop_movie(&mut self) {
        match self.movie.as_ref().map(|m| (m.can_stop, m.name.clone())) {
            Some((true, name)) => {
                self.note(format!("movie {name}: stopped"));
                self.movie_stop = true;
            }
            Some((false, name)) => self.note(format!("movie {name}: a stop is asked, it cannot stop")),
            None => {}
        }
    }

    pub(super) fn movie_update(&mut self) {
        if self.movie.is_some() && self.movie_skip >= 0 {
            if self.movie_skip == 0 {
                self.stop_movie();
            }
            self.movie_skip -= 1;
        }
    }

    pub fn movie_finished(&mut self, env: &mut FeEnv) {
        if self.movie.is_some() && !self.movie_seen {
            self.movie_end = true;
            return;
        }
        let Some(m) = self.movie.take() else { return };
        self.note(format!("movie {} over", m.name));
        self.movie = self.movie_queue.pop_front();
        (self.movie_skip, self.movie_stop) = (-1, false);
        (self.movie_seen, self.movie_end) = (false, false);
        self.movie_serial += 1;
        if m.by == "prelaunch" && m.name == progression::FINAL_TRACK_MOVIE {
            progression::load_to_final_track(self, env);
        }
        self.movie_finished_latch = true;
    }
}
