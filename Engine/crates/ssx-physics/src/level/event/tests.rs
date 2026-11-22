use super::*;
use crate::death::{DeathTimer, DeathTimerEvent, RiderDead};

#[test]
fn splits_progress_and_clock() {
    let c = Course { start_y: 100.0, end_y: -805.0 };
    let s = c.splits();
    assert_eq!(s[0], 95.0);
    assert!((s[8] - (95.0 - 8.0 * 100.0)).abs() < 1e-2, "{s:?}");
    assert_eq!(c.split_index(96.0), -1);
    assert_eq!(c.split_index(94.0), 0);
    assert_eq!(c.split_index(-800.0), 8);
    assert_eq!(c.progress(120.0), 0.0);
    assert!((c.progress(-352.5) - 0.5).abs() < 1e-6);
    assert_eq!(c.progress(-900.0), 1.0);
    let mut k = EventClock::default();
    k.advance(1.0);
    assert_eq!(k.ms(), 0, "a cleared clock does not run");
    k.restart();
    k.advance(1.234_5);
    assert_eq!(k.ms(), 1234);
    k.stop();
    k.advance(5.0);
    assert_eq!(k.ms(), 1234);
    k.restart();
    k.advance(4000.0);
    assert_eq!(k.seconds, EVENT_CLOCK_MAX_S);
}

#[test]
fn ghost_db_rows_and_tuned_scores() {
    let csv = "ui_drop_id,startingidx,ghostid,score,tuned_score,characterid,catchup,split0\n\
               465827782,0,1727147421,89398,-1,3660148671,748895195,0\n\
               26804557,0,338713384,3257527,957527,1079886293,748895195,0\n";
    let rows = parse_ghost_db(csv);
    assert_eq!(rows.len(), 2);
    assert_eq!((rows[0].0, rows[0].1.value(), rows[0].1.tuned_score), (465_827_782, 89_398, u32::MAX));
    assert_eq!(rows[1].1.value(), 957_527, "the tuned score replaces the score");
    assert_eq!((rows[0].1.lap, rows[0].1.end_position()), (0, [0.0; 3]), "no position columns");
}

#[test]
fn ghost_db_lap_and_end_position_columns() {
    let (x, y, z) = (-235.25f32, 429.875f32, -798.5f32);
    let csv = format!(
        "ui_drop_id,startingidx,ghostid,score,tuned_score,characterid,catchup,split0,split1,split2,split3,split4\n\
         2317586716,0,1,4365093,-1,3660148671,748895195,1,{},{},{},0,0\n",
        x.to_bits(),
        y.to_bits(),
        z.to_bits()
    );
    let rows = parse_ghost_db(&csv);
    assert_eq!(rows[0].1.lap, 1);
    assert_eq!(rows[0].1.end_position(), [x, y, z]);
}

fn race(rivals: Vec<u32>) -> EventManager {
    let setup = EventSetup {
        event_type: Some(EventType::RaceGhosts),
        delayed_finish: false,
        rivals,
        results: Some(ResultsFlow::Explore),
        places: None,
        goal: None,
        course: Some(Course { start_y: 0.0, end_y: -900.0 }),
        force_descent_in_air: false,
        objectives: None,
        message: Default::default(),
        time_up_result: false,
        free_ride: false,
    };
    EventManager::new(setup)
}

fn inputs(y: f32, end: bool, allow: bool) -> EventInputs {
    EventInputs {
        dt_s: 1.0 / 30.0,
        player_y: y,
        touching_end: end,
        health: 1.0,
        allow_race_end: allow,
        rewind_enabled: true,
        ..Default::default()
    }
}

#[test]
fn a_race_runs_its_clock_through_the_splits_to_the_finish_and_the_medal() {
    let mut m = race(vec![59_000, 61_000, 70_000]);
    let mut timer = DeathTimer::default();
    let idle = EventUpdate { ghost_clock_ms: Some(0), ..EventUpdate::default() };
    assert_eq!(m.update(&mut timer, &inputs(0.0, true, false)), idle, "idle before the start");
    m.on_trigger_race_start(&mut timer);
    let mut splits = Vec::new();
    for i in 0..1800 {
        let y = -(i as f32) * 0.5;
        let u = m.update(&mut timer, &inputs(y, false, false));
        splits.extend(u.split.map(|(k, _)| k));
    }
    assert_eq!(splits, (0..9).collect::<Vec<_>>());
    assert!(m.progress > 0.99 && m.state == EventState::Running);
    let u = m.update(&mut timer, &inputs(-900.0, true, false));
    assert!(u.crossed && u.finished && m.track_finished && m.state == EventState::Finished);
    let t = m.finish_ms.expect("finish time");
    assert!((60_000..=60_034).contains(&t), "{t}");
    for _ in 0..70 {
        let u = m.update(&mut timer, &inputs(-900.0, false, true));
        if u.results {
            break;
        }
    }
    let r = m.result.as_ref().expect("results");
    assert_eq!(m.state, EventState::Results);
    assert!(m.allow_outro && !r.failed && r.time_ms == t);
    assert_eq!((r.rank, r.medal), (Some(1), Some(Medal::Silver)), "behind 59 s, ahead of 61 s and 70 s");
}

#[test]
fn the_results_keep_the_last_safe_ground_for_the_explore_scores_splits() {
    let mut m = race(Vec::new());
    let mut timer = DeathTimer::default();
    assert_eq!(m.explore_splits(), None, "no result yet");
    m.on_trigger_race_start(&mut timer);
    let at = [12.5f32, -900.25, 3.0];
    let end = EventInputs { last_safe_ground: at, ..inputs(-900.0, true, false) };
    assert!(m.update(&mut timer, &end).finished);
    let posted = (0..70)
        .any(|_| m.update(&mut timer, &EventInputs { last_safe_ground: at, ..inputs(-900.0, false, true) }).results);
    assert!(posted);
    assert_eq!(m.explore_splits(), Some([0, at[0].to_bits(), at[1].to_bits(), at[2].to_bits()]));
}

#[test]
fn a_pause_holds_the_clock_but_not_the_results_after_the_finish() {
    let mut m = race(vec![61_000]);
    let mut timer = DeathTimer::default();
    m.on_trigger_race_start(&mut timer);
    m.update(&mut timer, &inputs(-10.0, false, false));
    let ms = m.clock.ms();
    m.waiting = true;
    for _ in 0..30 {
        assert_eq!(m.update_paused(&mut timer, &inputs(-10.0, true, true)), EventUpdate::default());
    }
    assert_eq!((m.clock.ms(), m.state), (ms, EventState::Running));
    m.waiting = false;
    let u = m.update(&mut timer, &inputs(-900.0, true, false));
    assert!(u.finished);
    m.waiting = true;
    let posted = (0..70).position(|_| m.update_paused(&mut timer, &inputs(-900.0, false, true)).results);
    assert_eq!(posted, Some(62), "the finish message's intro, second and outro, then the results");
    assert_eq!(m.state, EventState::Results);
}

#[test]
fn a_free_ride_keeps_its_events_finish_and_loops_after_it() {
    let mut m = race(vec![1]);
    m.setup.free_ride = true;
    assert!(m.loops_after_finish() && m.setup.event_type != Some(EventType::SurviveToDeath));
    let mut timer = DeathTimer::default();
    m.on_trigger_race_start(&mut timer);
    for _ in 0..30 {
        m.update(&mut timer, &inputs(-10.0, false, false));
    }
    assert!(m.clock.ms() > 0, "the event's clock runs");
    assert!(m.update(&mut timer, &inputs(-900.0, true, false)).crossed && m.state == EventState::Finished);
    let mut i = inputs(-900.0, false, true);
    for _ in 0..70 {
        assert!(!m.update(&mut timer, &i).results, "no results at a free ride's finish");
    }
    assert!(m.allow_outro && m.result.is_none() && m.state == EventState::Finished);
    i.nis_outro_active = true;
    m.update(&mut timer, &i);
    assert_eq!(m.state, EventState::LoopOutro);
    i.nis_outro_active = false;
    let u = m.update(&mut timer, &i);
    assert!(u.looped && m.request_loop_to_top && m.state == EventState::LoopRestart);
    m.on_trigger_race_start(&mut timer);
    m.on_entered_gameplay();
    timer.on_rider_dead(&RiderDead { by_death_volume: true, ..RiderDead::default() }, false, 0, false);
    let ends =
        (0..200).find(|_| m.update(&mut timer, &inputs(-10.0, false, false)).timer == Some(DeathTimerEvent::EventEnds));
    assert!(matches!(ends, Some(119..=120)), "{ends:?}");
    assert!(m.result.as_ref().is_some_and(|r| r.failed) && m.state == EventState::Idle);
}

#[test]
fn the_death_message_counts_only_while_the_event_runs() {
    let msg = RiderDead { by_death_volume: true, ..RiderDead::default() };
    for state in [
        EventState::Idle,
        EventState::Running,
        EventState::DelayedFinish,
        EventState::Finished,
        EventState::LoopOutro,
        EventState::LoopRestart,
        EventState::Results,
    ] {
        let m = EventManager { state, ..race(vec![]) };
        let mut timer = DeathTimer::default();
        m.on_rider_dead(&mut timer, &msg, true, 3, true);
        let started = matches!(state, EventState::Running | EventState::DelayedFinish);
        assert_eq!((timer.remaining >= 0.0, timer.hint), (started, started), "{state:?}");
    }
}

#[test]
fn a_death_fails_a_race_and_scores_a_survive_to_death() {
    let msg = RiderDead { by_death_volume: true, ..RiderDead::default() };
    let mut m = race(vec![1]);
    let mut timer = DeathTimer::default();
    m.on_trigger_race_start(&mut timer);
    timer.on_rider_dead(&msg, false, 0, false);
    let ends =
        (0..200).find(|_| m.update(&mut timer, &inputs(-10.0, false, false)).timer == Some(DeathTimerEvent::EventEnds));
    assert!(matches!(ends, Some(119..=120)), "{ends:?}");
    let r = m.result.as_ref().expect("a failed result");
    assert!(r.failed && r.medal.is_none() && m.state == EventState::Idle);
    let mut s = EventManager::new(EventSetup { event_type: Some(EventType::SurviveToDeath), ..Default::default() });
    let mut timer = DeathTimer::default();
    s.on_trigger_race_start(&mut timer);
    timer.on_rider_dead(&RiderDead { by_health: true, ..RiderDead::default() }, false, 0, false);
    let u = s.update(&mut timer, &EventInputs { dt_s: 0.033, descent: 1234.5, ..Default::default() });
    assert!(u.results && s.state == EventState::Results);
    let r = s.result.as_ref().expect("results");
    assert!(!r.failed && r.descent_mm == 1_234_500);
}

#[test]
fn a_survive_to_death_event_loops_until_the_death() {
    let course = Course { start_y: 0.0, end_y: -900.0 };
    let setup = EventSetup { event_type: Some(EventType::SurviveToDeath), course: Some(course), ..Default::default() };
    let mut m = EventManager::new(setup);
    let mut timer = DeathTimer::default();
    m.on_trigger_race_start(&mut timer);
    assert!(m.update(&mut timer, &inputs(-900.0, true, false)).crossed && m.state == EventState::Finished);
    let mut i = inputs(-900.0, false, true);
    for _ in 0..70 {
        assert!(!m.update(&mut timer, &i).results, "no results at the finish");
    }
    assert!(m.allow_outro && m.state == EventState::Finished && m.loops == 0);
    i.nis_outro_active = true;
    m.update(&mut timer, &i);
    assert!(!m.allow_outro && m.state == EventState::LoopOutro);
    i.nis_outro_active = false;
    let u = m.update(&mut timer, &i);
    assert!(u.looped && m.loops == 1 && m.state == EventState::LoopRestart);
    assert!(m.request_loop_to_top && m.nis_short && m.waiting && m.track_finished);
    m.on_trigger_race_start(&mut timer);
    assert!(m.state == EventState::Running && !m.track_finished && m.split == -1 && m.clock.seconds == 0.0);
    m.update(&mut timer, &inputs(-10.0, false, false));
    assert_eq!(m.clock.seconds, 0.0, "the clock waits for MsgEnteredGameplay");
    m.on_entered_gameplay();
    m.update(&mut timer, &inputs(-10.0, false, false));
    assert!(m.clock.seconds > 0.0 && m.split == 0);
    timer.on_rider_dead(&RiderDead { by_health: true, ..RiderDead::default() }, false, 0, false);
    let u = m.update(&mut timer, &EventInputs { dt_s: 0.033, descent: 123.456, ..inputs(-10.0, false, false) });
    let r = m.result.as_ref().expect("results");
    assert!(u.results && !r.failed && r.loops == 1);
    assert_eq!(r.descent_mm, 900_000 + 123_456, "a loop adds the whole course");
}

#[test]
fn a_delayed_finish_builds_at_once_and_posts_after_the_race_end() {
    let mut m = race(vec![]);
    m.setup.delayed_finish = true;
    let mut timer = DeathTimer::default();
    m.on_trigger_race_start(&mut timer);
    let u = m.update(&mut timer, &inputs(-900.0, true, false));
    assert!(u.crossed && !u.finished && !u.results && m.built.is_some());
    assert!(m.result.is_none(), "the record is built, not posted");
    assert!(m.state == EventState::DelayedFinish);
    assert_eq!(
        (u.rewind_enabled, m.saved_rewind_enabled),
        (Some(false), Some(true)),
        "BLN_RewindEnabled saved, cleared"
    );
    assert!(!m.track_finished, "IsTrackFinished waits for BLN_AllowRaceEnd (the heli outro's)");
    let u = m.update(&mut timer, &EventInputs { rewind_enabled: false, ..inputs(-900.0, false, false) });
    assert!(!u.results && u.rewind_enabled.is_none(), "written once, at the crossing");
    let u = m.update(&mut timer, &EventInputs { rewind_enabled: false, ..inputs(-900.0, false, true) });
    assert!(u.finished, "FinishTrack (0x1CC8ED4D) runs at the race end");
    assert!(m.track_finished && m.state == EventState::Finished);
    assert_eq!((u.rewind_enabled, m.saved_rewind_enabled), (Some(true), None), "+0x1DF given back at the race end");
    let mut posted = false;
    for _ in 0..70 {
        posted |= m.update(&mut timer, &inputs(-900.0, false, true)).results;
    }
    assert!(posted && m.state == EventState::Results && m.result.is_some());
}

#[test]
fn a_delayed_survive_finish_loops_without_results_until_the_death() {
    let course = Course { start_y: 0.0, end_y: -900.0 };
    let setup = EventSetup {
        event_type: Some(EventType::SurviveToDeath),
        delayed_finish: true,
        course: Some(course),
        ..Default::default()
    };
    let mut m = EventManager::new(setup);
    let mut timer = DeathTimer::default();
    m.on_trigger_race_start(&mut timer);
    for lap in 1..=2 {
        let u = m.update(&mut timer, &inputs(-900.0, true, false));
        assert!(u.crossed && !u.results && m.state == EventState::DelayedFinish);
        assert!(m.built.is_some() && m.result.is_none(), "loop {lap}: built at the crossing, not posted");
        let mut i = inputs(-900.0, false, true);
        assert!(m.update(&mut timer, &i).finished && m.state == EventState::Finished);
        for _ in 0..70 {
            assert!(!m.update(&mut timer, &i).results, "loop {lap}: no results after the grab");
        }
        i.nis_outro_active = true;
        m.update(&mut timer, &i);
        i.nis_outro_active = false;
        assert!(m.update(&mut timer, &i).looped && m.loops == lap && m.result.is_none());
        m.on_trigger_race_start(&mut timer);
        m.on_entered_gameplay();
        assert!(m.state == EventState::Running && !m.track_finished);
    }
    timer.on_rider_dead(&RiderDead { by_health: true, ..RiderDead::default() }, false, 0, false);
    let u = m.update(&mut timer, &EventInputs { dt_s: 0.033, descent: 100.0, ..inputs(-10.0, false, false) });
    let r = m.result.as_ref().expect("results at the death");
    assert!(u.results && !r.failed && r.loops == 2);
    assert_eq!(r.descent_mm, 2 * 900_000 + 100_000);
}

#[test]
fn a_maps_time_limit_ends_by_the_rules() {
    for (result, t) in
        [(false, EventType::TrickScore), (true, EventType::TrickScore), (false, EventType::SurviveToDeath)]
    {
        let mut m = race(Vec::new());
        m.setup.event_type = Some(t);
        m.setup.time_up_result = result;
        m.state = EventState::Running;
        let mut timer = crate::death::DeathTimer::default();
        m.time_up = true;
        let u = m.update(&mut timer, &inputs(-10.0, false, false));
        let r = m.result.as_ref().expect("a result");
        assert!(u.results);
        assert_eq!(r.failed, !result, "{t:?}");
        assert_eq!(m.state, if result { EventState::Results } else { EventState::Idle });
    }
}

#[test]
fn a_survive_to_death_run_loops_only_through_its_outro() {
    let mut timer = crate::death::DeathTimer::default();
    let mut m = race(Vec::new());
    m.setup.event_type = Some(EventType::SurviveToDeath);
    m.state = EventState::Finished;
    let u = m.update(&mut timer, &inputs(-900.0, true, true));
    assert!(!u.looped && m.state == EventState::Finished && m.allow_outro, "it waits for its outro");
    let mut on = inputs(-900.0, true, true);
    on.nis_outro_active = true;
    let u = m.update(&mut timer, &on);
    assert!(!u.looped && m.state == EventState::LoopOutro && !m.allow_outro);
    let u = m.update(&mut timer, &inputs(-900.0, true, true));
    assert!(u.looped && m.request_loop_to_top && m.waiting);
    assert_eq!((m.state, m.loops), (EventState::LoopRestart, 1));
}

#[test]
fn a_rounds_end_ends_a_run_still_on_as_its_time_limit_would() {
    let mut timer = crate::death::DeathTimer::default();
    let mut m = race(Vec::new());
    (m.setup.event_type, m.setup.time_up_result) = (Some(EventType::SurviveToDeath), true);
    m.on_trigger_race_start(&mut timer);
    for i in 0..300 {
        m.update(&mut timer, &inputs(-(i as f32), false, false));
    }
    m.end_round(true);
    let u = m.update(&mut timer, &inputs(-300.0, false, false));
    let r = m.result.clone().expect("the round's end is the run's");
    assert!(u.results && !r.failed && m.state == EventState::Results);
    let u = m.update(&mut timer, &inputs(-900.0, true, true));
    assert!(!u.looped && !u.crossed && !m.request_loop_to_top && m.loops == 0, "no finish, no loop after the round");
    m.on_trigger_race_start(&mut timer);
    assert_eq!((m.state, m.result.as_ref()), (EventState::Results, Some(&r)), "a race start starts nothing");
    let mut m = race(Vec::new());
    (m.setup.event_type, m.setup.time_up_result) = (Some(EventType::SurviveToDeath), true);
    m.state = EventState::Finished;
    let mut on = inputs(-900.0, true, true);
    on.nis_outro_active = true;
    m.update(&mut timer, &on);
    assert!(m.update(&mut timer, &inputs(-900.0, true, true)).looped && m.request_loop_to_top);
    m.end_round(true);
    let u = m.update(&mut timer, &inputs(-900.0, false, true));
    assert!(u.results && !m.request_loop_to_top && !m.waiting && m.state == EventState::Results);
    assert_eq!(m.loops, 1, "the loop counted before the round's end stays counted");
    for (t, failed) in [(EventType::SurviveToDeath, false), (EventType::RaceClock, true)] {
        let mut m = race(Vec::new());
        m.setup.event_type = Some(t);
        m.on_trigger_race_start(&mut timer);
        m.update(&mut timer, &inputs(-10.0, false, false));
        m.end_round(false);
        m.update(&mut timer, &inputs(-11.0, false, false));
        assert_eq!(m.result.as_ref().map(|r| r.failed), Some(failed), "{t:?}");
        assert_eq!(m.state, if failed { EventState::Idle } else { EventState::Results });
    }
    let mut m = race(Vec::new());
    m.on_trigger_race_start(&mut timer);
    assert!(m.update(&mut timer, &inputs(-900.0, true, false)).finished && m.state == EventState::Finished);
    m.end_round(true);
    let posted = (0..70).any(|_| m.update(&mut timer, &inputs(-900.0, false, true)).results);
    assert!(posted && m.result.as_ref().is_some_and(|r| !r.failed), "the finish crossed before the round's end");
    m.reset();
    assert_eq!(m.round_over, None);
}

#[test]
fn the_launched_event_is_the_level_event() {
    let row = |ghost_id| Rival {
        starting_idx: 0,
        ghost_id,
        character: 0,
        catchup: 0,
        score: 1000,
        tuned_score: u32::MAX,
        lap: 0,
        end_bits: [0; 3],
    };
    let event = |name: &str, key, t, ghosts: [i32; 3]| EventDef {
        name: name.into(),
        key,
        drop_key: 7,
        event_type: Some(t),
        scripts: Vec::new(),
        rivals: ghosts.into_iter().map(row).collect(),
        required_equipment: Vec::new(),
        avalanche_scripts: Vec::new(),
        forced_rider: 0,
        forced_items: [0; 5],
        loadout: None,
        objectives: None,
        message: Default::default(),
        places: None,
        goal: None,
    };
    let events = [
        event("exp_x_drop_7_race", 1, EventType::RaceGhosts, [11, 12, 13]),
        event("exp_x_drop_7_trick", 2, EventType::TrickGhosts, [21, 22, 23]),
    ];
    let asked = || -> String { panic!("the viewer's choice asked for a launched event") };
    let trick = level_event(&events, Some(2), asked).expect("the launched event");
    assert_eq!(trick.name, "exp_x_drop_7_trick");
    let mut ghosts: Vec<i32> = trick.rivals.iter().map(|r| r.ghost_id).collect();
    ghosts.sort_unstable();
    assert_eq!(ghosts, [21, 22, 23], "the trick's own rows");
    assert!(level_event(&events, Some(3), asked).is_none(), "a launched key not on the drop: no event");
    assert_eq!(level_event(&events, None, || "race".into()).map(|e| e.key), Some(1));
    assert_eq!(level_event(&events, None, || "trick".into()).map(|e| e.key), Some(2));
    assert!(level_event(&events, None, || "none".into()).is_none());
}

#[test]
fn a_racers_race_clock_is_written_while_it_runs_unfinished() {
    let mut m = race(vec![59_000]);
    let mut timer = DeathTimer::default();
    let racer = |y: f32, end: bool| {
        let mut i = inputs(-10.0, false, false);
        i.racers[0] = Some(RacerInput { y, touching_end: end, points: 0.0, trigger_wipeout: false, in_air: false });
        i
    };
    m.update(&mut timer, &racer(0.0, false));
    assert!(m.racers.iter().all(|r| r.clock_ms == 0), "idle before the start: no score update of the racer");
    m.on_trigger_race_start(&mut timer);
    m.update(&mut timer, &racer(-1.0, false));
    let first = m.racers[0].clock_ms;
    assert!(first > 0 && first == m.clock.ms(), "the first running update writes the advanced clock ({first} ms)");
    for _ in 0..10 {
        m.update(&mut timer, &racer(-2.0, false));
    }
    assert_eq!(m.racers[0].clock_ms, m.clock.ms());
    m.update(&mut timer, &racer(-900.0, true));
    let at_finish = m.racers[0].clock_ms;
    assert_eq!(m.racers[0].finish_ms, Some(at_finish));
    for _ in 0..10 {
        m.update(&mut timer, &racer(-900.0, true));
    }
    assert_eq!(m.racers[0].clock_ms, at_finish, "a finished racer keeps its last");
    m.reset();
    assert!(m.racers.iter().all(|r| r.clock_ms == 0));
}

#[test]
fn an_unfinished_trick_bot_is_projected_from_its_points_at_the_results() {
    let setup = EventSetup {
        event_type: Some(EventType::TrickBots),
        results: Some(ResultsFlow::WorldTour),
        course: Some(Course { start_y: 0.0, end_y: -1000.0 }),
        ..EventSetup::default()
    };
    let mut m = EventManager::new(setup);
    let mut timer = DeathTimer::default();
    let frame = |player_y: f32, end: bool, allow: bool, bot_y: f32, bot_points: f32| {
        let mut i = EventInputs { points: 450_000.0, ..inputs(player_y, end, allow) };
        let bot =
            RacerInput { y: bot_y, touching_end: false, points: bot_points, trigger_wipeout: false, in_air: false };
        i.racers[0] = Some(bot);
        i
    };
    m.on_trigger_race_start(&mut timer);
    m.update(&mut timer, &frame(-500.0, false, false, -700.0, 250_000.0));
    let crossing = m.update(&mut timer, &frame(-1000.0, true, false, -750.0, 300_000.0));
    assert!(crossing.finished && m.state == EventState::Finished);
    assert_eq!((m.racers[0].progress, m.racers[0].finish_ms, m.racers[0].points), (0.75, None, 300_000.0));
    let result = (0..70).find_map(|k| {
        let u = m.update(&mut timer, &frame(-1000.0, false, true, -800.0 - k as f32, 400_000.0));
        u.results.then(|| m.result.clone()).flatten()
    });
    let r = result.expect("results");
    assert_eq!(m.racers[0].progress, 0.75, "the progress stands as at the crossing");
    assert_eq!(m.racers[0].points, 400_000.0, "the points are the feed's");
    assert_eq!(r.rivals, [500_000]);
    assert_eq!((r.value, r.place, r.rank), (450_000, u32::MAX, Some(1)));
}

#[test]
fn the_ghost_clock_runs_on_past_the_finish_and_holds_while_waiting() {
    let mut m = race(vec![59_000]);
    let mut timer = DeathTimer::default();
    m.on_trigger_race_start(&mut timer);
    let mut last = 0;
    for i in 0..3000 {
        let u = m.update(&mut timer, &inputs(-(i as f32).min(900.0), i > 900, true));
        last = u.ghost_clock_ms.expect("written while +0x1DC is clear");
    }
    assert!(m.finish_ms.is_some());
    assert!(m.clock.ms() < last, "{} < {last}", m.clock.ms());
    assert_eq!(last, m.ghost_clock.ms());
    m.waiting = true;
    assert_eq!(m.update(&mut timer, &inputs(-3000.0, false, false)).ghost_clock_ms, None);
    assert_eq!(m.ghost_clock.ms(), last);
    m.waiting = false;
    m.reset();
    assert_eq!((m.ghost_clock.seconds, m.ghost_clock.running), (0.0, false));
}

#[test]
fn the_hud_feed_runs_where_the_score_manager_calls_it() {
    let course = Course { start_y: 0.0, end_y: -900.0 };
    let setup = EventSetup { event_type: Some(EventType::SurviveToDeath), course: Some(course), ..Default::default() };
    let mut m = EventManager::new(setup);
    let mut timer = DeathTimer::default();
    let calls = |m: &EventManager| (m.hud_feed.feeds, m.hud_feed.resets, m.hud_feed.loop_restarts, m.hud_feed.state);
    assert_eq!(calls(&m), (1, 1, 0, EventState::Idle), "the Init's reset");
    m.update(&mut timer, &inputs(0.0, false, false));
    assert_eq!(calls(&m), (1, 1, 0, EventState::Idle), "state 0 feeds nothing");
    m.on_trigger_race_start(&mut timer);
    assert_eq!(calls(&m), (2, 2, 0, EventState::Idle), "the race start's reset");
    m.update(&mut timer, &inputs(-10.0, false, false));
    assert_eq!(calls(&m), (3, 2, 0, EventState::Running));
    m.waiting = true;
    m.update_paused(&mut timer, &inputs(-10.0, false, false));
    assert_eq!(m.hud_feed.feeds, 3, "+0x1DC holds state 1's feed");
    m.waiting = false;
    m.update(&mut timer, &inputs(-900.0, true, false));
    assert_eq!(calls(&m), (4, 2, 0, EventState::Finished), "the finish's update reads state 3");
    m.waiting = true;
    m.update_paused(&mut timer, &inputs(-900.0, false, true));
    assert_eq!(calls(&m), (5, 2, 0, EventState::Finished), "state 3 feeds in a pause");
    m.waiting = false;
    let mut i = inputs(-900.0, false, true);
    i.nis_outro_active = true;
    while m.state != EventState::LoopOutro {
        m.update(&mut timer, &i);
    }
    i.nis_outro_active = false;
    let fed = m.hud_feed.feeds;
    assert!(m.update(&mut timer, &i).looped && m.state == EventState::LoopRestart);
    assert_eq!(calls(&m), (fed + 1, 2, 0, EventState::LoopOutro), "fed in state 4, then the loop");
    m.update(&mut timer, &i);
    assert_eq!(m.hud_feed.feeds, fed + 1, "state 5 feeds nothing");
    m.on_trigger_race_start(&mut timer);
    assert_eq!(calls(&m), (fed + 2, 2, 1, EventState::LoopRestart), "the loop restart reads state 5");
    m.reset();
    assert_eq!(calls(&m), (fed + 3, 3, 1, EventState::Idle), "a restart's reset");
}

#[test]
fn the_hud_is_fed_the_finish_time_the_results_post() {
    let mut m = race(vec![61_000]);
    let mut timer = DeathTimer::default();
    m.on_trigger_race_start(&mut timer);
    for i in 0..900 {
        m.update(&mut timer, &inputs(-(i as f32), false, false));
    }
    let (feeds, before) = (m.hud_feed.feeds, m.clock.ms());
    assert!(m.update(&mut timer, &inputs(-900.0, true, false)).crossed);
    let t = m.clock.ms();
    assert!(t > before && !m.clock.running, "the crossing's step counts, then the clock stops: {before} → {t}");
    assert_eq!((m.hud_feed.feeds, m.hud_feed.state), (feeds + 1, EventState::Finished), "the crossing's update feeds");
    assert_eq!(m.finish_ms, Some(t));
    let mut fed = Vec::new();
    loop {
        let f = m.hud_feed.feeds;
        let u = m.update(&mut timer, &inputs(-905.0, false, true));
        if m.hud_feed.feeds != f {
            fed.push(m.clock.ms());
        }
        if u.results {
            break;
        }
    }
    assert!(fed.len() > 60 && fed.iter().all(|&ms| ms == t), "{} feeds after the line", fed.len());
    assert_eq!(m.result.as_ref().map(|r| (r.time_ms, r.value)), Some((t, t)));
}
