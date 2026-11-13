use super::*;

fn e54aeb82() -> CatchUpCurve {
    let xs = [-50.0, -33.333_336, -16.666_668, 0.0, 16.666_656, 33.333_33, 50.0];
    let ys = [2.0, 2.0, 1.5, 1.2, 1.2, 0.8, 0.5];
    CatchUpCurve { points: xs.into_iter().zip(ys).collect() }
}

#[test]
fn the_curve_passes_its_points_and_holds_its_ends() {
    let c = e54aeb82();
    for &(x, y) in &c.points {
        assert!((c.eval(x) - y).abs() < 1e-6, "{x}: {} vs {y}", c.eval(x));
    }
    assert_eq!(c.eval(-80.0), 2.0);
    assert_eq!(c.eval(120.0), 0.5);
    assert!((c.eval(8.333_328) - 1.206_25).abs() < 1e-5, "{}", c.eval(8.333_328));
}

#[test]
fn the_raw_value_parses() {
    let words: [u32; 21] = [
        0xC248_0000,
        0,
        0x4248_0000,
        0x4000_0000,
        7,
        0xC248_0000,
        0xC205_5557,
        0xC185_5557,
        0,
        0x4185_5552,
        0x4205_5554,
        0x4248_0000,
        0x4248_0000,
        0x4000_0000,
        0x4000_0000,
        0x3FC0_0000,
        0x3F99_999A,
        0x3F99_999A,
        0x3F4C_CCCD,
        0x3F00_0000,
        0x3F00_0000,
    ];
    let bytes: Vec<u8> = words.iter().flat_map(|w| w.to_be_bytes()).collect();
    let c = CatchUpCurve::from_raw(&bytes).expect("curve");
    assert_eq!(c.points.len(), 7);
    assert_eq!(c.points[0], (-50.0, 2.0));
    assert!((c.points[3].1 - 1.2).abs() < 1e-6 && c.points[6] == (50.0, 0.5));
}

#[test]
fn the_factor_is_clamped_near_the_end_and_one_after_the_finish() {
    let c = e54aeb82();
    assert_eq!(catch_up_factor(Some(&c), -50.0, 900.0, false), 2.0);
    assert_eq!(catch_up_factor(Some(&c), -50.0, 400.0, false), 1.0);
    assert_eq!(catch_up_factor(Some(&c), 50.0, 400.0, false), 0.5);
    assert_eq!(catch_up_factor(Some(&c), 50.0, 900.0, true), 1.0);
    assert_eq!(catch_up_factor(None, 50.0, 900.0, false), 1.0);
    let (d, h) = descent_and_height_left(300.0, -1000.0, 350.0);
    assert_eq!((d, h), (0.0, 1300.0));
    let (d, h) = descent_and_height_left(300.0, -1000.0, -1200.0);
    assert_eq!((d, h), (1300.0, 0.0));
}

#[test]
fn one_random_row_per_rival_character() {
    let row = |ghost_id, character, start| Rival {
        starting_idx: start,
        ghost_id,
        character,
        catchup: CATCHUP_NONE,
        score: 1000 * ghost_id as u32,
        tuned_score: 7,
        lap: 0,
        end_bits: [0; 3],
    };
    let mac = fnv1(b"mac");
    let griff = fnv1(b"griff");
    let rows = [row(1, mac, 1), row(2, mac, 1), row(3, mac, 1), row(4, griff, 2)];
    let mut picks = Vec::new();
    let reqs = bot_requests(
        &[mac, griff],
        &rows,
        |_| true,
        |n| {
            picks.push(n);
            n - 1
        },
    );
    assert_eq!(picks, [3]);
    assert_eq!(reqs.iter().map(|r| (r.ghost_id, r.start_slot)).collect::<Vec<_>>(), [(3, 1), (4, 2)]);
    assert_eq!(reqs.iter().map(|r| r.score).collect::<Vec<_>>(), [3000, 4000]);
    assert!(reqs.iter().all(|r| r.racer_type == 1 && r.collision && r.tint == BOT_TINT));
    let reqs = bot_requests(&[fnv1(b"zoe")], &rows, |c| c != griff, |_| 0);
    assert_eq!(reqs.iter().map(|r| r.ghost_id).collect::<Vec<_>>(), [4]);
    assert_eq!((reqs[0].character, reqs[0].tint), (GENERIC_CHARACTER, GENERIC_TINT));
}

#[test]
fn the_query_hands_over_the_bots_velocity_within_reach() {
    let v = Vec3::new(0.3, -0.1, 0.0);
    assert_eq!(ghost_query(true, Vec3::ZERO, v, Vec3::new(0.5, 0.0, 0.5)), Some(v));
    assert_eq!(ghost_query(true, Vec3::ZERO, v, Vec3::new(0.6, 0.0, 0.6)), None);
    assert_eq!(ghost_query(false, Vec3::ZERO, v, Vec3::ZERO), None);
    let mut clock = BotClock::default();
    clock.advance(1.5, None, 1.0 / 30.0);
    clock.advance(1.5, Some(0.5), 1.0 / 30.0);
    assert!((clock.seconds - 0.075).abs() < 1e-6);
}

#[test]
fn character_keys_name_the_riders() {
    assert_eq!(character_name(0xAFD9_E65D), "alex");
    assert_eq!(character_name(0x3E7E_1256), "mac");
    assert_eq!(character_name(0x405D_C1D5), "zoe");
    assert_eq!(character_name(1), "griff");
}

#[test]
fn playback_time_scale_pulls_to_one_and_avoids() {
    let (s, o) = playback_time_scale(0.5, None);
    assert!((s - 0.6).abs() < 1e-6 && !o);
    assert_eq!(playback_time_scale(0.95, None), (1.0, true));
    assert_eq!(playback_time_scale(1.0, None), (1.0, true));
    let a = AvoidInputs {
        position: Vec3::new(4.0, 0.0, 0.0),
        velocity: Vec3::new(-1.0, 0.0, 0.0),
        player_location: Vec3::ZERO,
        player_velocity: Vec3::new(1.0, 0.0, 0.0),
    };
    let (s, o) = playback_time_scale(1.0, Some(a));
    assert!((s - (1.0 + 1.0 / 3.0)).abs() < 1e-6 && !o, "towards 3 at 1/(1 + 2): {s}");
    let b = AvoidInputs { player_velocity: Vec3::new(-0.2, 0.0, 0.0), velocity: Vec3::new(-1.0, 0.0, 0.0), ..a };
    let (s, _) = playback_time_scale(1.0, Some(b));
    assert!((s - (1.0 - 1.0 / 3.0)).abs() < 1e-6, "towards 0: {s}");
}

#[test]
fn explore_rows_become_racers_of_type_two_in_leaderboard_order() {
    let row = |ghost_id, score| Rival {
        starting_idx: 0,
        ghost_id,
        character: fnv1(b"mac"),
        catchup: CATCHUP_NONE,
        score,
        tuned_score: u32::MAX,
        lap: 0,
        end_bits: [0; 3],
    };
    let mut def = crate::event::EventDef {
        name: "exp_test".into(),
        key: 0,
        drop_key: 0,
        event_type: Some(crate::event::EventType::RaceGhosts),
        scripts: Vec::new(),
        rivals: vec![row(1, 90_000), row(2, 70_000), row(3, 80_000)],
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
    let reqs = explore_requests(&def);
    assert_eq!(reqs.iter().map(|r| r.ghost_id).collect::<Vec<_>>(), [2, 3, 1]);
    assert!(reqs.iter().all(|r| r.racer_type == EXPLORE_RACER_TYPE && !r.collision && r.start_slot == 0));
    def.event_type = Some(crate::event::EventType::TrickGhosts);
    let reqs = explore_requests(&def);
    assert_eq!(reqs.iter().map(|r| r.ghost_id).collect::<Vec<_>>(), [1, 3, 2]);
}
