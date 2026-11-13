use super::*;

pub fn advance(fe: &mut FrontEnd, env: &mut FeEnv) {
    if !fe.globe.advance_free {
        return;
    }
    if !fe.globe.provider_selectable {
        fe.note("A_GameAdvanceScreen: the selected entry is not selectable (FE manager +0x88(3))");
        fe.play_sound(3);
        return;
    }
    select_drop_screen(fe, env);
    fe.globe.advance_free = false;
}

pub fn freeride(fe: &mut FrontEnd, env: &mut FeEnv) {
    if !fe.globe.advance_free || !fe.globe.provider_is_drop {
        return;
    }
    let drop = fe.globe.drop;
    let Some(e) = fe.career.event(drop) else { return };
    if e.cost != 0
        || e.event_type == 4
        || matches!(e.drop_type, ssx_assets::career::DropType::Custom | ssx_assets::career::DropType::Type2)
    {
        fe.note(format!("A_GameFreeride: drop {drop:#010x} is not a free drop; refused"));
        return;
    }
    fe.free_ride_drop = Some(drop);
    fe.note(format!("A_GameFreeride: drop {drop:#010x} as a free ride"));
    select_drop_screen(fe, env);
    fe.globe.advance_free = false;
}

pub(super) fn select_drop_screen(fe: &mut FrontEnd, env: &mut FeEnv) {
    let screen = fe.screen;
    fe.note(format!("Globe_SelectDrop on screen {screen:#x}"));
    let node = fe.globe.provider_node;
    match screen {
        0x42 | 0xC => {
            let range = if fe.career.range(node).is_some() { node } else { fe.globe.range };
            if screen == 0x42 {
                fe.profile.campaign_globe.select_range(range);
            }
            let wt = fe.game_mode == 1;
            let queue_playing = fe.progression.running() && !fe.movie_queued();
            if screen == 0x42 && wt && !fe.profile.new_game_active() && !queue_playing {
                let movie = fe.career.range(range).map(|r| r.movie.clone()).unwrap_or_default();
                if !movie.is_empty() && range_movie_plays(fe, &movie) {
                    fe.note(format!("FLOW_PlayMovie {movie} (range)"));
                    fe.queue_movie(MovieRequest::new(movie, "range"));
                }
            }
            focus_range(fe, range);
            fe.go_to_screen(env, if screen == 0x42 { 0x43 } else { 0xD });
        }
        0x43 | 0xD => {
            if fe.career.peak(node).is_some() {
                focus_peak(fe, node);
            }
            fe.go_to_screen(env, if screen == 0x43 { 0x44 } else { 0xE });
        }
        0x44 | 0xE | 0x11 => select_drop(fe, env),
        _ => {}
    }
    fe.note("FEGlobeNodeSelection (0x32B67D88)");
}

fn range_movie_plays(fe: &mut FrontEnd, movie: &str) -> bool {
    use ssx_assets::career::profile::range_movies::RangeMovieOutcome;
    let outcome = fe.profile.range_movie(fe.range_movies, movie);
    match &outcome {
        RangeMovieOutcome::EveryTime => {}
        RangeMovieOutcome::FirstTime => {
            fe.note(format!("range movie {movie}: first selection in this profile, recorded (first time only)"))
        }
        RangeMovieOutcome::PlayedBefore => {
            fe.note(format!("range movie {movie}: played before in this profile, not posted (first time only)"))
        }
        RangeMovieOutcome::NotRecorded(why) => {
            fe.note(format!("range movie {movie}: WARNING the record: {why}; posted (first time only)"))
        }
    }
    outcome.plays()
}

pub(super) fn select_drop(fe: &mut FrontEnd, env: &mut FeEnv) {
    let drop = fe.globe.drop;
    if fe.free_ride_drop == Some(drop) {
        fe.note(format!("free ride: drop {drop:#010x} chosen; the event's setup is skipped"));
        fe.equipment.drop = drop;
        fe.go_to_screen(env, screens::id::EQUIPMENT);
        return;
    }
    fe.free_ride_drop = None;
    let state = fe.profile.drop_state(drop);
    if state == DropState::Locked && fe.screen == 0x44 {
        fe.note("drop locked");
        return;
    }
    if tutorial_launch(fe, env, drop) {
        return;
    }
    if drop == FINAL_DROP && state == DropState::Unlocked {
        crate::game::fe::progression::final_drop_start(fe, env);
        fe.play_sound(2);
        return;
    }
    if fe.screen == 0xE {
        match fe.profile.buy_drop(&fe.rules, drop) {
            Ok(cost) => {
                env.values.set_int(0xB9, drop as i32);
                let adj = env.values.int(0x93);
                env.values.set_int(0x93, adj - cost as i32);
                if cost > 0 {
                    fe.note(format!("Explore drop bought for {cost}"));
                }
                fe.note(format!(
                    "message 0x2D28E498; message 0x03C0FC45 → SelectedGlobeNodeMessage {{3, {drop:#010x}}}"
                ));
                write_panels(fe, env);
                fe.play_sound(2);
                if cost > 0 {
                    return;
                }
            }
            Err(e) => {
                fe.note(format!("cannot buy: {e:?}"));
                env.nav_events.push("go_to_drop_cost_not_enough_money_popup".into());
                fe.play_sound(3);
                return;
            }
        }
    }
    fe.equipment.drop = drop;
    fe.note("message 0x759464C1; current drop set");
    fe.go_to_screen(env, screens::id::EQUIPMENT);
}

pub fn tutorial_launch(fe: &mut FrontEnd, env: &mut FeEnv, drop: u32) -> bool {
    if !TUTORIAL_DROPS.contains(&drop) || chapter_done(fe, drop) {
        return false;
    }
    let mut items = vec![0x405D_C1D5, 0x55BB_536F];
    if drop == 0xC145_699A {
        items.extend([0x8650_5C81, 0xE318_0443]);
    } else {
        items.push(0x00D8_B4DE);
    }
    fe.messages.push(crate::game::fe::FeMessage::LoadoutRefresh);
    fe.note("tutorial drop: 0x17D251BE, Zoe and her items equipped, ForceStartTrackMessage");
    for key in items {
        crate::game::fe::equipment::flow_equip(fe, key, 0);
    }
    crate::game::fe::flow::launch(fe, env, drop, true);
    true
}

pub(super) fn chapter_done(fe: &FrontEnd, drop: u32) -> bool {
    use ssx_assets::career::world_tour::{CHAPTERS, chapter_drops};
    (1..=CHAPTERS)
        .map(chapter_drops)
        .find(|l| l.iter().any(|n| fnv1(n.as_bytes()) == drop))
        .is_some_and(|l| l.last().is_some_and(|n| fe.profile.drop_state(fnv1(n.as_bytes())) == DropState::Completed))
}

pub fn arrived(fe: &FrontEnd, env: &mut FeEnv) -> bool {
    let list = env.values.int(0xB3);
    let node = fe.globe.provider_node;
    let title = match fe.screen {
        0xD | 0x10 | 0x43 => fe.career.peak(node).and_then(|p| fe.career.range(p.range)).map(|r| r.title.clone()),
        0xE | 0x11 | 0x44 => {
            let peak = fe.career.event(node).map_or(node, |e| fe.career.peak_and_range(e).0);
            fe.career.peak(peak).map(|p| p.title.clone())
        }
        _ => None,
    };
    if let Some(key) = title {
        let t = crate::game::loc::breadcrumb_capitals(&env.loc.text(&key));
        env.values.set_str(0x33, t);
    }
    match fe.screen {
        2 => list == 0,
        0xC | 0xF | 0x42 => list == 1,
        0xD | 0x10 | 0x43 => list == 2,
        0xE | 0x11 | 0x44 => list == 3,
        _ => false,
    }
}

pub fn previous(fe: &mut FrontEnd, env: &mut FeEnv) {
    if !arrived(fe, env) {
        return;
    }
    env.values.set_int(0x232, 2);
    env.nav_events.push("previous".into());
    fe.note("previous; FEGlobeNodeBackout (0xA63FDCA3)");
}

pub fn back_from_equipment(fe: &mut FrontEnd, env: &mut FeEnv) {
    fe.free_ride_drop = None;
    fe.note("equipment: save end (0x825FAFC8) → previous");
    env.nav_events.push("previous".into());
}
