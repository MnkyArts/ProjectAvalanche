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
    if !offers_free_ride(e, fe.profile.purchased.contains(&drop)) {
        fe.note(format!("A_GameFreeride: drop {drop:#010x} is not a free drop; refused"));
        return;
    }
    fe.free_ride_drop = Some(drop);
    fe.note(format!("A_GameFreeride: drop {drop:#010x} as a free ride"));
    select_drop_screen(fe, env);
    fe.globe.advance_free = false;
}

fn offers_free_ride(e: &ssx_assets::career::Event, purchased: bool) -> bool {
    use ssx_assets::career::DropType;
    let drop_cost = if purchased { 0 } else { e.cost };
    // PORT(choice): `A_GameFreeride` re-checks the button's condition (the view's X handler 0x2690 dispatches
    // `freeride` only under `mAllowFreeride`) and refuses the online-only custom / type-2 records, so a mod's view
    // cannot start a free ride the disc's view never offers; the console's handler (TU2 0x825BA214…0x825BA26C)
    // checks only the provider's byte +0xC, the flow's +0x10738 and +0x14 <= 0 and trusts the view.
    drop_cost <= 0 && e.event_type != 4 && !matches!(e.drop_type, DropType::Custom | DropType::Type2)
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

#[cfg(test)]
mod tests {
    use ssx_assets::career::{DropType, Event};

    use super::offers_free_ride;

    fn event(event_type: i64, drop_type: DropType, cost: i64) -> Event {
        Event {
            key: 0,
            name: String::new(),
            drop: 0,
            event_type,
            drop_type,
            mode: String::new(),
            slot: String::new(),
            peak_short: String::new(),
            hud_title: String::new(),
            hud_body: String::new(),
            goal_body: String::new(),
            results_title: String::new(),
            results_body: String::new(),
            unlock_text: String::new(),
            deadly_descent: false,
            warning: 0,
            time_of_day: 0,
            cost,
            difficulty: 0,
            reward: 0,
            places: None,
            targets: Vec::new(),
            required: Vec::new(),
            unk_b683b553: Vec::new(),
            rivals: Vec::new(),
            forced: Vec::new(),
            ghost_drop: false,
            unk_89d02fa4: false,
            unk_12bccfb5: 0,
            unk_0b03fc19: None,
            unk_98e9ebcc: Vec::new(),
            custom_status: 0,
            unk_links: [0; 3],
            peak_ref: 0,
            helicopter: 0,
            map_point: [0.0; 2],
        }
    }

    #[test]
    fn a_bought_drop_offers_its_free_ride() {
        let race = event(6, DropType::Explore, 15_000);
        assert!(!offers_free_ride(&race, false), "priced and not bought");
        assert!(offers_free_ride(&race, true), "bought");
        assert!(offers_free_ride(&event(2, DropType::Explore, 0), false), "free");
    }

    #[test]
    fn no_free_ride_for_survive_to_death_or_the_online_records() {
        for purchased in [false, true] {
            assert!(!offers_free_ride(&event(4, DropType::Explore, 0), purchased));
            assert!(!offers_free_ride(&event(6, DropType::Custom, 0), purchased));
            assert!(!offers_free_ride(&event(6, DropType::Type2, 0), purchased));
        }
    }
}
