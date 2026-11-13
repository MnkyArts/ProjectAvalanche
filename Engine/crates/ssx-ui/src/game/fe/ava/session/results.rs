use ssx_net_proto::StandingState;

use crate::game::GameUi;
use crate::game::hud::results::{CareerResults, ResultsInput, post_results};
use crate::game::values::{LeaderboardRow, UiValue, id};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResultsExit {
    Restart,
    Quit,
}

// PORT(choice): 10 s, well above the menu's activation (a few UI frames; the restart stage's own loading screen comes
// after it); tuning: the activation's frames measured in a window's rounds; the original has no real-time multiplayer.
pub const RESULTS_EXIT_WAIT_S: f64 = 10.0;

#[derive(Clone, Debug, PartialEq)]
pub struct ResultRow {
    pub name: String,
    pub icon: String,
    pub state: StandingState,
    pub invalid: Option<ssx_net_proto::Check>,
    pub clock_ms: u32,
    pub points: u32,
    pub descent_mm: u32,
    pub medal: Option<u8>,
    pub own: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SessionResults {
    pub event_type: i32,
    pub rows: Vec<ResultRow>,
    pub rewinds: u32,
    pub character_pic: String,
    pub character: String,
}

fn value_text(ui: &mut GameUi, event_type: i32, row: &ResultRow) -> String {
    let key = match row.state {
        StandingState::Finished => {
            let v = match event_type {
                5..=7 => row.clock_ms,
                3 | 4 => row.descent_mm,
                _ => row.points,
            };
            return crate::game::hud::results::value_text(&mut ui.loc, event_type, v);
        }
        StandingState::Racing => "TXT_AVA_RESULTS_NO_FINISH",
        StandingState::Failed => "TXT_AVA_RESULTS_FAILED",
        StandingState::Left => "TXT_AVA_RESULTS_LEFT",
        StandingState::Invalidated => {
            let why = row.invalid.map_or_else(String::new, |c| ui.loc.text(check_key(c)));
            return ui.loc.format("TXT_AVA_RESULTS_INVALID", &[&why]);
        }
    };
    ui.loc.text(key)
}

pub fn check_key(check: ssx_net_proto::Check) -> &'static str {
    use ssx_net_proto::Check;
    match check {
        Check::Speed => "TXT_AVA_CHECK_SPEED",
        Check::Step => "TXT_AVA_CHECK_STEP",
        Check::Splits => "TXT_AVA_CHECK_SPLITS",
        Check::Finish => "TXT_AVA_CHECK_FINISH",
        Check::Clock => "TXT_AVA_CHECK_CLOCK",
        Check::Tricks => "TXT_AVA_CHECK_TRICKS",
    }
}

impl GameUi {
    pub fn session_leave_results(&mut self, exit: ResultsExit) {
        self.fe_call(|fe, env| {
            fe.note(format!("results: left for the next round ({exit:?}): to the post-race menu"));
            crate::game::fe::ava::session::players::close_over_results(fe, env);
            fe.ava.session.results_exit = Some((exit, fe.time));
            crate::game::fe::flow::results_advance(fe, env, false);
        });
    }

    pub fn session_quit_results(&mut self) {
        let Some(fe) = self.fe.as_ref() else { return };
        match (fe.ava.session.results_exit, fe.ava.session.results_quit) {
            (_, Some(_)) => {}
            (Some((_, since)), None) => self.fe_call(|fe, _| {
                fe.note("results: the session is over: the post-race menu's quit");
                fe.ava.session.results_exit = Some((ResultsExit::Quit, since));
            }),
            (None, None) => self.session_leave_results(ResultsExit::Quit),
        }
    }

    pub fn session_level_left(&mut self) {
        if let Some(fe) = self.fe.as_mut()
            && fe.ava.session.results_quit.take().is_some()
        {
            fe.note("results: the level is left for the next round's drop");
        }
    }

    pub fn session_results_exit_pending(&self) -> bool {
        self.fe.as_ref().is_some_and(|f| f.ava.session.results_exit.is_some() || f.ava.session.results_quit.is_some())
    }

    pub fn session_results_exit_frame(&mut self) -> Result<(), String> {
        let Some(fe) = self.fe.as_ref() else { return Ok(()) };
        if let Some(chosen) = fe.ava.session.results_quit
            && fe.time - chosen > RESULTS_EXIT_WAIT_S
        {
            self.fe_call(|fe, _| fe.ava.session.results_quit = None);
            return Err(format!(
                "the level was not left within {RESULTS_EXIT_WAIT_S:.0} s of the post-race menu's quit"
            ));
        }
        let Some((exit, since)) = fe.ava.session.results_exit else { return Ok(()) };
        if !fe.post_race.active {
            if fe.time - since > RESULTS_EXIT_WAIT_S {
                self.fe_call(|fe, _| fe.ava.session.results_exit = None);
                return Err(format!("the post-race menu did not come within {RESULTS_EXIT_WAIT_S:.0} s"));
            }
            return Ok(());
        }
        self.fe_call(|fe, env| {
            fe.ava.session.results_exit = None;
            if exit == ResultsExit::Quit {
                fe.ava.session.results_quit = Some(fe.time);
            }
            let action = match exit {
                ResultsExit::Restart => "A_GameRestartStage",
                ResultsExit::Quit => "A_GameQuitStage",
            };
            fe.note(format!("results: the post-race menu's {action} for the next round"));
            crate::game::fe::menus::postrace::action(fe, env, action);
        });
        Ok(())
    }

    pub fn session_results(&mut self, r: &SessionResults) -> Option<usize> {
        let own = r.rows.iter().position(|row| row.own);
        let mine = own.map(|k| r.rows[k].clone());
        let (money, level) = match self.fe.as_ref() {
            Some(fe) => {
                let rider = fe.rules.riders.get(&r.character).copied().unwrap_or(0);
                let mut stats = fe.profile.riders.get(&rider).cloned().unwrap_or_default();
                let rules = &fe.rules;
                let level = ssx_assets::career::profile::update_rider_level(
                    &mut stats,
                    &rules.level_thresholds,
                    &rules.level_prices,
                );
                (fe.profile.credits(), level)
            }
            None => (0, (0, 0)),
        };
        let finished = mine.as_ref().is_some_and(|m| m.state == StandingState::Finished);
        let input = ResultsInput {
            event_type: r.event_type,
            game_mode: 0,
            points: mine.as_ref().map_or(0, |m| m.points),
            time_ms: mine.as_ref().map_or(0, |m| m.clock_ms),
            descent_mm: mine.as_ref().map_or(0, |m| m.descent_mm),
            rewinds: r.rewinds,
            failed: !finished,
            rank: own,
            medal: mine.as_ref().and_then(|m| m.medal).map_or(-1, i32::from),
            career: CareerResults {
                money_previous: money,
                level_before: level,
                level_after: level,
                success: mine.as_ref().is_some_and(|m| m.medal.is_some()),
                ..CareerResults::default()
            },
            tutorial: false,
            character_pic: r.character_pic.clone(),
        };
        post_results(&mut self.values, &mut self.loc, &input);
        let mut rows = Vec::with_capacity(r.rows.len());
        for (k, row) in r.rows.iter().enumerate() {
            let score = value_text(self, r.event_type, row);
            rows.push(LeaderboardRow {
                value: k as i32,
                name: crate::game::fe::ava::bounded(
                    &row.name,
                    ssx_net_proto::name::MAX_PLAYER_NAME_CHARS,
                    self.fe.as_ref().and_then(|f| f.ava.glyphs.as_ref()),
                ),
                icon: row.icon.clone(),
                score,
                rank_text: (k + 1).to_string(),
                reward: String::new(),
                medal: row.medal.map_or(-1, i32::from),
                rank: k as i32 + 1,
                is_player: row.own,
                score_is_icon: false,
                player_passed: own.is_some_and(|o| k > o),
            });
        }
        let passed = own.map_or(0, |o| r.rows.len() - o - 1);
        self.values.set(id::RESULT_LEADERBOARD_LEADERBOARD, UiValue::Rows(rows));
        self.values.set_int(id::RESULT_LEADERBOARD_TOTAL_PASSED_COUNT, passed as i32);
        self.values.set_int(id::RESULT_LEADERBOARD_TOTAL_REWARD, 0);
        self.values.set_bool(id::RESULT_LEADERBOARD_IS_ENABLED, !r.rows.is_empty());
        own.map(|k| k + 1)
    }
}
