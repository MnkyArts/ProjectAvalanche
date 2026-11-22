use glam::Vec3;

use super::*;
use crate::rewind::RewindState;
use crate::testutil::{plane, world};
use crate::triggers::{IG_DEATH, TriggerScope, TriggerVolume, VolumeShape};

#[test]
fn a_death_volume_kills_and_the_timer_ends_the_event() {
    let mut w = world(plane(400.0, 0.3));
    w.triggers.volumes.push(TriggerVolume {
        name: "death".into(),
        type_text: "death".into(),
        groups: IG_DEATH,
        interacts: crate::triggers::interaction_mask(IG_DEATH),
        shape: VolumeShape::Box { half_extents: Vec3::new(50.0, 40.0, 5.0) },
        centre: Vec3::new(0.0, 0.0, 30.0),
        rotation: glam::Quat::IDENTITY,
        object: 0,
        instance: 0,
        placement: 0,
        scope: TriggerScope::Track,
    });
    let src = br#"BeginObjective("PlayerDeathVolumeTest")
            WAIT_TriggerVolumeByGroup(IG_DEATH)
            DO_SetGameStateValue("Player1", BLN_TriggerDeathByDeathPlane, true)
            DO_RestartObjective()
        EndObjective()"#;
    let mut d = crate::objectives::DropScripts { list: vec!["t.lua".into()], ..Default::default() };
    d.sources.insert("t.lua".into(), std::sync::Arc::from(&src[..]));
    w.survival.objectives = Some(std::sync::Arc::new(d));
    let mut s = RiderState::spawn(&w, Vec3::new(0.0, 50.0, 0.0), Vec3::Z).expect("ground");
    s.velocity = Vec3::Z * 0.4;
    let mut x = Session::default();
    x.event.on_trigger_race_start(&mut x.timer);
    let input = Input { tuck: 1.0, ..Input::default() };
    let (mut died, mut ended, mut latched) = (None, None, None);
    for i in 0..400 {
        let e = x.execute(&mut s, &w, &input, &TrickInput::default(), 0.0, &RewindPad::default(), None);
        if e.rider_dead.is_some() && died.is_none() {
            assert!(e.was_alive && !s.death.is_alive() && x.timer.hint && x.timer.remaining <= 4.0);
            died = Some(i);
        }
        if latched.is_none() && s.carving.is_dead {
            latched = Some(i);
        }
        if e.timer == DeathTimerEvent::EventEnds {
            ended = Some(i);
            break;
        }
    }
    let (died, latched, ended) = (died.expect("died"), latched.expect("IsDead"), ended.expect("event end"));
    assert!(latched == died, "the carving latches IsDead in the update that read ¬IsAlive");
    assert!((119..=121).contains(&(ended - died)), "4 s of 30 Hz executions: {died} → {ended}");
    assert!(x.event_ended && s.death.dead_by_death_plane);
    let events = &x.objectives.as_ref().expect("objectives").runner.events;
    assert!(!events.global(ssx_lua::game::events::RACE_ENDED), "no RaceEnded on the death timer's end");
    assert!(x.pads_released);
    let (mut a, mut b) = ((x.clone(), s.clone()), (x.clone(), s.clone()));
    let pushed = Input { steer: 1.0, tuck: 1.0, jump: true, ..Input::default() };
    let tricks = TrickInput { y: true, tweak: true, ..TrickInput::default() };
    let rewind = RewindPad { rewind_held: true, takeover: false, flip_accel: 1.0 };
    for _ in 0..60 {
        a.0.execute(&mut a.1, &w, &pushed, &tricks, 1.0, &rewind, None);
        b.0.execute(&mut b.1, &w, &Input::default(), &TrickInput::default(), 0.0, &RewindPad::default(), None);
    }
    assert_eq!((a.1.position, a.1.traj, a.1.velocity), (b.1.position, b.1.traj, b.1.velocity));
    assert!(!a.0.rewinding() && a.0.buffer.playback().is_none(), "no rewind after the end");
    x.reset();
    assert!(!x.pads_released, "a restart releases the latch");
}

#[test]
fn a_delayed_crossing_refuses_the_rewind_until_a_restart() {
    let mut w = world(plane(400.0, 0.3));
    w.triggers.volumes.push(TriggerVolume {
        name: "end".into(),
        type_text: "gameflow".into(),
        groups: crate::triggers::IG_GAMEFLOW,
        interacts: crate::triggers::interaction_mask(crate::triggers::IG_GAMEFLOW),
        shape: VolumeShape::Box { half_extents: Vec3::new(50.0, 40.0, 5.0) },
        centre: Vec3::new(0.0, 0.0, 120.0),
        rotation: glam::Quat::IDENTITY,
        object: 0,
        instance: 0,
        placement: 0,
        scope: TriggerScope::Track,
    });
    let start = || {
        let mut s = RiderState::spawn(&w, Vec3::new(0.0, 50.0, 0.0), Vec3::Z).expect("ground");
        s.velocity = Vec3::Z * 0.4;
        let mut x = Session::default();
        x.event.setup.delayed_finish = true;
        x.manager.count.infinite = false;
        x.event.on_trigger_race_start(&mut x.timer);
        (x, s)
    };
    let input = Input { tuck: 1.0, ..Input::default() };
    let rewind = RewindPad { rewind_held: true, takeover: false, flip_accel: 1.0 };
    let none = RewindPad::default();
    let ride = |x: &mut Session, s: &mut RiderState, from: usize, steps: usize| {
        let mut seen = Vec::new();
        for i in 0..steps {
            let pad = if i >= from { &rewind } else { &none };
            x.execute(s, &w, &input, &TrickInput::default(), 0.0, pad, None);
            seen.push(x.manager.state);
        }
        seen
    };
    let (mut x, mut s) = start();
    let mut crossed = None;
    for i in 0..600 {
        x.execute(&mut s, &w, &input, &TrickInput::default(), 0.0, &none, None);
        if x.event.state == crate::event::EventState::DelayedFinish {
            crossed = Some(i);
            break;
        }
    }
    let crossed = crossed.expect("the rider crosses the line");
    assert!(crossed > 60, "the line is far enough for a rewind before it: {crossed}");
    assert!(!x.manager.count.enabled && x.event.saved_rewind_enabled == Some(true));
    let left = x.manager.count.rewinds_left;
    let seen = ride(&mut x, &mut s, 0, 40);
    assert!(seen.contains(&RewindState::Abort) && !seen.contains(&RewindState::Enter), "refused: {seen:?}");
    assert!(!x.rewinding() && x.manager.count.rewinds_left == left);
    let (mut y, mut t) = start();
    let seen = ride(&mut y, &mut t, crossed - 30, crossed - 30 + 40);
    assert!(seen.contains(&RewindState::Enter), "granted before the line: {seen:?}");
    x.reset();
    assert!(x.manager.count.enabled && x.event.saved_rewind_enabled.is_none());
}

#[test]
fn a_trigger_entity_drives_weather_flags_and_the_boundary_force() {
    use crate::triggers::{IG_TRIGGER, TriggerEntity};
    const SCRIPT: &str = r#"
local s = CreateStateSet( "T" )
local st = s:AddState( "Wait" )
local tr = st:AddTransition( "In" )
tr:AddCondition( TriggerVolumeCondition( "", "Player1", "Touch" ) )
st = s:AddState( "In" )
st:AddUpdateTrack( PrecipitationTrack( 0.5 ) )
st:AddUpdateTrack( OutOfBoundsTrack( "Player1", 0.2, 2.0 ) )
st:AddEnterTrack( SetNamedFlagTrack( "inside", 1, TEMPORARY ) )
tr = st:AddTransition( "Wait" )
tr:AddCondition( NOT( TriggerVolumeCondition( "", "Player1", "Touch" ) ) )
"#;
    let mut w = world(plane(400.0, 0.3));
    let (object, instance) = (7, 9);
    w.triggers.volumes.push(TriggerVolume {
        name: "box".into(),
        type_text: "trigger".into(),
        groups: IG_TRIGGER,
        interacts: crate::triggers::interaction_mask(IG_TRIGGER),
        shape: VolumeShape::Box { half_extents: Vec3::new(50.0, 200.0, 4.0) },
        centre: Vec3::new(0.0, 0.0, 12.0),
        rotation: glam::Quat::IDENTITY,
        object,
        instance,
        placement: 0,
        scope: TriggerScope::Track,
    });
    let rotation = glam::Quat::from_rotation_y(std::f32::consts::FRAC_PI_2);
    w.triggers.entities.push(TriggerEntity {
        object,
        instance,
        placement: 0,
        scope: TriggerScope::Track,
        position: Vec3::new(0.0, 0.0, 12.0),
        rotation,
    });
    w.triggers.scripts.insert(object, ("scripts\\t.lua".into(), std::sync::Arc::from(SCRIPT)));
    let mut d = crate::objectives::DropScripts { list: vec!["o.lua".into()], ..Default::default() };
    d.sources.insert("o.lua".into(), std::sync::Arc::from(&b"BeginObjective(\"X\") EndObjective()"[..]));
    w.survival.objectives = Some(std::sync::Arc::new(d));
    let mut s = RiderState::spawn(&w, Vec3::new(0.0, 50.0, 0.0), Vec3::Z).expect("ground");
    s.velocity = Vec3::Z * 0.4;
    let mut x = Session::default();
    let input = Input { tuck: 1.0, ..Input::default() };
    let (mut inside, mut forced, mut left) = (false, false, false);
    for _ in 0..200 {
        x.execute(&mut s, &w, &input, &TrickInput::default(), 0.0, &RewindPad::default(), None);
        let c = x.controllers.as_ref().expect("controllers");
        let flag = x.objectives.as_ref().is_some_and(|o| o.runner.flags.check(&crate::objectives::Name::new("inside")));
        if c.weather.targets().0 == 0.5 {
            inside = true;
            assert!(flag, "the enter track set the flag");
            forced |=
                s.boundary.force_strength == 2.0 && (s.boundary.force_direction - rotation * Vec3::Z).length() < 1e-6;
        } else if inside {
            left = true;
            break;
        }
    }
    assert!(inside && forced && left, "inside {inside}, forced {forced}, left {left}");
    assert_eq!(x.controllers.as_ref().unwrap().weather.targets().0, 0.0, "popped back to the default");
}

#[test]
fn the_scripts_built_ahead_are_the_ones_the_first_execution_builds() {
    use crate::triggers::{IG_TRIGGER, TriggerEntity};
    use ssx_content::policy::ClientMods;
    const SCRIPT: &str = r#"
local s = CreateStateSet( "T" )
local st = s:AddState( "Wait" )
local tr = st:AddTransition( "In" )
tr:AddCondition( TriggerVolumeCondition( "", "Player1", "Touch" ) )
st = s:AddState( "In" )
st:AddUpdateTrack( PrecipitationTrack( 0.5 ) )
st:AddEnterTrack( SetNamedFlagTrack( "inside", 1, TEMPORARY ) )
"#;
    const OBJECTIVES: &[u8] = br#"
BeginObjective("A")
	WAIT_CheckNamedFlag("HintSeen")
	DO_SetPermanentNamedFlag("HintFromScript", true)
EndObjective()
"#;
    let root = std::env::temp_dir().join(format!("ssx-scripts-ahead-{}", std::process::id()));
    let make = || {
        let mut w = world(plane(400.0, 0.3));
        let (object, instance) = (7, 9);
        w.triggers.volumes.push(TriggerVolume {
            name: "box".into(),
            type_text: "trigger".into(),
            groups: IG_TRIGGER,
            interacts: crate::triggers::interaction_mask(IG_TRIGGER),
            shape: VolumeShape::Box { half_extents: Vec3::new(50.0, 200.0, 4.0) },
            centre: Vec3::new(0.0, 0.0, 12.0),
            rotation: glam::Quat::IDENTITY,
            object,
            instance,
            placement: 0,
            scope: TriggerScope::Track,
        });
        let position = Vec3::new(0.0, 0.0, 12.0);
        let (placement, scope, rotation) = (0, TriggerScope::Track, glam::Quat::IDENTITY);
        w.triggers.entities.push(TriggerEntity { object, instance, placement, scope, position, rotation });
        w.triggers.scripts.insert(object, ("scripts\\t.lua".into(), std::sync::Arc::from(SCRIPT)));
        let list = vec!["o.lua".into()];
        let mut d = crate::objectives::DropScripts { list, root: Some(root.clone()), ..Default::default() };
        d.sources.insert("o.lua".into(), std::sync::Arc::from(OBJECTIVES));
        w.survival.objectives = Some(std::sync::Arc::new(d));
        std::sync::Arc::new(w)
    };
    let seen = crate::objectives::Name::new("HintSeen").hash;
    let w = make();
    let seeded = |flag: u32| {
        let mut x = Session::default();
        x.seed_named_flags([flag]);
        x
    };
    let content = ssx_assets::content::GameContent::of(&root);
    let ride = |ahead: Option<(u32, std::sync::Arc<crate::rider::World>, ClientMods)>| {
        let mut s = RiderState::spawn(&w, Vec3::new(0.0, 50.0, 0.0), Vec3::Z).expect("ground");
        s.velocity = Vec3::Z * 0.4;
        let mut x = seeded(seen);
        if let Some((flag, from, policy)) = ahead {
            let inputs = ScriptsInputs::of(&seeded(flag));
            let build = std::thread::scope(|t| t.spawn(|| ScriptsBuild::ahead(inputs, from)).join().expect("built"));
            x.scripts_ahead = Some(build);
            content.set_sim_policy(policy);
        }
        let input = Input { tuck: 1.0, ..Input::default() };
        let (mut states, mut inside) = (Vec::new(), false);
        for _ in 0..60 {
            x.execute(&mut s, &w, &input, &TrickInput::default(), 0.0, &RewindPad::default(), None);
            let weather = x.controllers.as_ref().expect("controllers").weather.targets();
            inside |= weather.0 == 0.5;
            states.push(format!("{s:?} {:?} {weather:?} {:?}", x.objectives, x.permanent_named_flags()));
        }
        assert!(x.scripts_ahead.is_none(), "the first execution took the build or dropped it");
        assert!(inside, "the entity's controller ran its track");
        let hint = crate::objectives::Name::new("HintFromScript").hash;
        assert!(x.permanent_named_flags().contains(&hint), "the seed let the objective pass");
        (x.scripts_built_ahead, states)
    };
    let (taken, in_line) = ride(None);
    assert!(!taken);
    let any = ClientMods::Any;
    let (taken, ahead) = ride(Some((seen, w.clone(), any)));
    assert!(taken, "the same inputs: taken");
    assert!(ahead == in_line, "the build made ahead rides as the one built in the first execution");
    let (taken, other) = ride(Some((seen ^ 1, w.clone(), any)));
    assert!(!taken, "another seed: not taken");
    assert!(other == in_line, "built in the first execution instead");
    let (taken, other) = ride(Some((seen, make(), any)));
    assert!(!taken, "another world (equal, not the same): not taken");
    assert!(other == in_line, "built in the first execution instead");
    let (taken, other) = ride(Some((seen, w.clone(), ClientMods::None)));
    assert!(!taken, "another simulation policy after the build: not taken");
    assert!(other == in_line, "built in the first execution instead");
}

#[test]
fn the_profiles_named_flags_seed_the_store_and_come_back() {
    use crate::objectives::{LevelObjectives, Name};
    let (seen, hint, temp) = (Name::new("HintSeen"), Name::new("HintFromScript"), Name::new("Temp"));
    let mut s = Session::default();
    s.seed_named_flags([seen.hash, 7]);
    s.set_named_flag(7, false);
    assert_eq!(s.permanent_named_flags(), [seen.hash]);
    let script = br#"
BeginObjective("A")
	WAIT_CheckNamedFlag("HintSeen")
	DO_SetPermanentNamedFlag("HintFromScript", true)
	DO_SetTemporaryNamedFlag("Temp", true)
EndObjective()
"#;
    let batch = [("t.lua".to_string(), script.to_vec())];
    let seed: Vec<u32> = s.named_flag_seed.iter().copied().collect();
    let mut o = LevelObjectives::load(&[], &batch, crate::objectives::LoadWorld::default(), false, None, &seed);
    assert!(o.report.problems.is_empty(), "{:?}", o.report.problems);
    let mut load = crate::objectives::LoadWorld::default();
    o.runner.update(0.1, &mut load);
    s.objectives = Some(o);
    let mut want = vec![seen.hash, hint.hash];
    want.sort_unstable();
    assert_eq!(s.permanent_named_flags(), want, "the seeded flag let the objective pass");
    let flags = &s.objectives.as_ref().expect("objectives").runner.flags;
    assert!(flags.check(&temp) && !want.contains(&temp.hash));
    s.set_named_flag(seen.hash, false);
    s.reset();
    assert_eq!(s.permanent_named_flags(), [hint.hash], "a restart keeps the store");
}

#[test]
fn badge_progress_is_kept() {
    let mut stats = crate::stats::StatsStore::default();
    stats.badge_progress.insert(42, 0.25);
    for scope in [crate::stats::TRACK, crate::stats::TRICK, crate::stats::PERSISTENT] {
        stats.reset(scope);
    }
    assert_eq!(stats.badge_progress.get(&42), Some(&0.25));
}

#[test]
fn the_actors_run_after_the_step_once_per_execution() {
    let w = world(plane(400.0, 0.3));
    let mut s = RiderState::spawn(&w, Vec3::new(0.0, 50.0, 0.0), Vec3::Z).expect("ground");
    s.velocity = Vec3::Z * 0.4;
    let mut x = Session::default();
    let input = Input { tuck: 1.0, ..Input::default() };
    for _ in 0..3 {
        let before = s.traj;
        let mut calls = Vec::new();
        let e = x.execute_with(
            &mut s,
            &w,
            &input,
            &TrickInput::default(),
            0.0,
            &RewindPad::default(),
            None,
            &mut |_, st, dt| calls.push((st.traj, dt)),
        );
        assert_eq!(calls.len(), 1);
        let (traj, dt) = calls[0];
        assert!(traj != before && traj == s.traj && dt == e.dt, "{before} → {traj} (now {}), dt {dt}", s.traj);
    }
}

#[test]
fn the_snapshot_holds_the_actors_writes() {
    let w = world(plane(400.0, 0.3));
    let mut s = RiderState::spawn(&w, Vec3::new(0.0, 50.0, 0.0), Vec3::Z).expect("ground");
    s.velocity = Vec3::Z * 0.4;
    s.track_started = true;
    let mut x = Session::default();
    let input = Input { tuck: 1.0, ..Input::default() };
    for _ in 0..3 {
        let mut written = Vec3::ZERO;
        x.execute_with(
            &mut s,
            &w,
            &input,
            &TrickInput::default(),
            0.0,
            &RewindPad::default(),
            None,
            &mut |_, st, _| {
                st.traj += Vec3::X;
                written = st.traj;
            },
        );
        let mut restored = s.clone();
        restored.traj = Vec3::ZERO;
        assert!(x.buffer.rewind(&mut restored, 0.0).is_some(), "no snapshot recorded");
        assert_eq!(restored.traj, written);
    }
}

#[test]
fn a_restart_clears_the_heli_jump_of_the_run_before() {
    use glam::Quat;
    let data = heli_jump::HeliJumpData {
        end_location: (Vec3::ZERO, Quat::IDENTITY),
        heli_start: (Vec3::new(0.0, 10.0, 0.0), Quat::IDENTITY),
        rail: (Quat::IDENTITY, Vec3::ZERO),
    };
    let mut x = Session::default();
    let mut h =
        heli_jump::HeliJump::new(std::sync::Arc::new(data), true, std::path::Path::new("root"), "event", "elise");
    h.contact = true;
    h.intercept = heli_jump::Intercept { valid: true, position: Vec3::ONE, angle: 1.0, length_scale: 8.0 };
    x.heli_jump = Some(h);
    x.reset();
    let h = x.heli_jump.as_ref().expect("kept");
    assert!(!h.contact && h.outro.is_none());
    assert_eq!(h.intercept, heli_jump::Intercept::default());
}

#[test]
fn the_tutorials_presentation_type_reaches_the_session_and_survives_a_restart() {
    for (constant, want) in [("TutorialSkydrop", 1), ("TutorialGround", 2)] {
        let mut w = world(plane(400.0, 0.3));
        let src = format!("BeginObjective(\"Tutorial\")\n\tDO_AudioSetPresentationType({constant})\nEndObjective()");
        let mut d = crate::objectives::DropScripts { list: vec!["t.lua".into()], ..Default::default() };
        d.sources.insert("t.lua".into(), std::sync::Arc::from(src.as_bytes()));
        w.survival.objectives = Some(std::sync::Arc::new(d));
        let mut s = RiderState::spawn(&w, Vec3::new(0.0, 50.0, 0.0), Vec3::Z).expect("ground");
        let mut x = Session::default();
        assert_eq!(x.presentation, 0, "the loading screen's 0 before the scripts");
        x.execute(&mut s, &w, &Input::default(), &TrickInput::default(), 0.0, &RewindPad::default(), None);
        let o = x.objectives.as_ref().expect("objectives");
        assert!(o.report.problems.is_empty(), "{:?}", o.report.problems);
        assert_eq!(x.presentation, want, "{constant}");
        x.restart_objectives(&mut s, &w, false);
        assert_eq!(x.presentation, want, "{constant}: a restart keeps it");
    }
}

fn ride_with_objectives(
    mut w: crate::rider::World,
    script: &str,
    n: usize,
) -> (crate::rider::World, Session, RiderState) {
    let mut d = crate::objectives::DropScripts { list: vec!["t.lua".into()], ..Default::default() };
    d.sources.insert("t.lua".into(), std::sync::Arc::from(script.as_bytes()));
    w.survival.objectives = Some(std::sync::Arc::new(d));
    let mut s = RiderState::spawn(&w, Vec3::new(0.0, 50.0, 0.0), Vec3::Z).expect("ground");
    let mut x = Session::default();
    for _ in 0..n {
        x.execute(&mut s, &w, &Input::default(), &TrickInput::default(), 0.0, &RewindPad::default(), None);
    }
    let o = x.objectives.as_ref().expect("objectives");
    assert!(o.report.problems.is_empty(), "{:?}", o.report.problems);
    (w, x, s)
}

#[test]
fn without_the_ground_ray_query_the_spq_publishes_no_ground() {
    let script = |on: bool| {
        format!(
            "BeginObjective(\"Render NIS\")\n\tDO_SetGameStateValue(\"Player1\", BLN_GroundRayQueryEnabled, {on})\nEndObjective()"
        )
    };
    let rock = || {
        let (sn, c) = 0.3f32.sin_cos();
        let p = |x: f32, z: f32| Vec3::new(x, -z * sn, z * c);
        let mut w = crate::collision::CollisionWorld::default();
        w.push_cluster(
            &[p(-400.0, -400.0), p(-400.0, 400.0), p(400.0, 400.0), p(400.0, -400.0)],
            None,
            None,
            &[[0, 1, 2], [0, 2, 3]],
            10,
            0,
        );
        w.finish();
        world(w)
    };
    let (_, _, on) = ride_with_objectives(rock(), &script(true), 3);
    let slope = on.contact.expect("the ground with the query").normal;
    assert!(slope.y < 0.99 && on.close.is_close && on.ground_normal == slope, "{on:?}");
    let (w, mut x, off) = ride_with_objectives(rock(), &script(false), 3);
    assert!(off.contact.is_none(), "no published ray: {:?}", off.contact);
    assert_eq!(off.ground_normal, Vec3::Y);
    assert!(!off.close.is_close && !off.close.is_very_close);
    assert_eq!(off.weights, on.weights, "the weights follow the raw hit");
    assert_ne!(off.weights, crate::surface::no_contact_weights());
    let mut n = RiderState::spawn(&w, Vec3::new(0.0, 50.0, 0.0), Vec3::Z).expect("ground");
    n.carry_game_state(&off);
    x.execute(&mut n, &w, &Input::default(), &TrickInput::default(), 0.0, &RewindPad::default(), None);
    assert!(n.contact.is_none(), "carried into the fresh level start");
}

#[test]
fn the_out_of_bounds_correction_is_off_while_the_scripts_say_so() {
    let run = |off: bool| {
        let mut w = world(plane(400.0, 0.3));
        w.collision.boundary = Some(crate::rider::boundary::BoundaryGrid {
            min: Vec3::splat(-500.0),
            max: Vec3::splat(499.99),
            cells_per_metre: Vec3::new(1.0 / 1000.0, 0.0, 1.0 / 1000.0),
            width: 1,
            height: 1,
            cells: vec![0x4000 | (31 << 8)],
        });
        let script = format!(
            "BeginObjective(\"ForceSpatialDataOutOfBoundsCorrectionOff\")\n\tDO_SetGameStateValue(\"Player1\", \
             BLN_ForceSpatialDataOutOfBoundsCorrectionOff, {off})\nEndObjective()"
        );
        ride_with_objectives(w, &script, 10).2
    };
    let on = run(false);
    assert!(
        on.boundary.out_of_bounds && on.boundary.timer > 0.0 && on.boundary.wind.length() > 0.0,
        "{:?}",
        on.boundary
    );
    let off = run(true);
    assert!(
        !off.boundary.out_of_bounds && off.boundary.timer == 0.0 && off.boundary.wind == Vec3::ZERO,
        "{:?}",
        off.boundary
    );
}

#[test]
fn a_fresh_level_start_keeps_the_heli_interaction_switch() {
    let w = world(plane(400.0, 0.3));
    let mut s = RiderState::spawn(&w, Vec3::new(0.0, 50.0, 0.0), Vec3::Z).expect("ground");
    s.heli_interaction_allowed = false;
    let mut n = RiderState::spawn(&w, Vec3::new(0.0, 50.0, 0.0), Vec3::Z).expect("ground");
    n.carry_game_state(&s);
    assert!(!n.heli_interaction_allowed);
}

#[test]
fn the_bodies_step_by_the_rider_s_restored_ticks() {
    let w = world(plane(400.0, 0.3));
    let mut s = RiderState::spawn(&w, Vec3::new(0.0, 50.0, 0.0), Vec3::Z).expect("ground");
    s.velocity = Vec3::Z * 0.4;
    let mut x = Session::default();
    x.event.on_trigger_race_start(&mut x.timer);
    let input = Input { tuck: 1.0, ..Input::default() };
    for _ in 0..200 {
        x.execute(&mut s, &w, &input, &TrickInput::default(), 0.0, &RewindPad::default(), None);
    }
    let flare = crate::rigid::RigidBodyDef {
        offset: Vec3::new(-0.19, 0.0373, 0.0),
        axis: Vec3::X,
        half_height: 0.18,
        radius: 0.04,
        fat: 0.0,
        inv_mass: 1.0,
        inv_inertia: Vec3::new(1250.0, 89.2857, 89.2857),
        max_speed: 1e32,
        max_angular_speed: 100.0,
        linear_damping: 0.0,
        angular_damping: 0.0,
        material: [0.2, 0.2, 0.1],
    };
    let mut c = crate::controllers::Controllers::offline(&w.triggers);
    let mut body = crate::rigid::Body::new(flare, Vec3::new(0.0, 2000.0, 0.0), glam::Quat::IDENTITY);
    body.push(Vec3::X * 5.0, Vec3::ZERO);
    c.rigid.bodies.push(body);
    x.controllers = Some(c);
    let rewind = RewindPad { rewind_held: true, takeover: false, flip_accel: 1.0 };
    let (mut restored, mut entered) = (0, false);
    for _ in 0..40 {
        let v = x.controllers.as_ref().unwrap().rigid.bodies[0].motion.velocity.y;
        let e = x.execute(&mut s, &w, &input, &TrickInput::default(), 0.0, &rewind, None);
        let stepped = (x.controllers.as_ref().unwrap().rigid.bodies[0].motion.velocity.y - v) / crate::rigid::GRAVITY.y;
        let ticks = stepped * crate::TICKS_PER_SECOND;
        assert!(
            (ticks - e.rewind.delta_ticks).abs() < 2e-3,
            "stepped {ticks} ticks, the rider's {}",
            e.rewind.delta_ticks
        );
        entered |= x.manager.state == RewindState::Enter;
        let live = crate::DT_TICKS * x.executed_dilation.1;
        if (e.rewind.delta_ticks - live).abs() > 0.1 {
            restored += 1;
        }
    }
    assert!(entered, "the rewind entered");
    assert!(restored >= 8, "the enter phase restores the pre-rewind's ticks: {restored} executions differ");
}

#[test]
fn a_restart_builds_the_levels_objectives_anew() {
    use crate::objectives::{LevelObjectives, LoadWorld, Name};
    use ssx_lua::game::objectives::{Node, Wait};
    let script = br#"
BeginObjective("Banner")
	WAIT_ObjectiveMessage("Player1")
EndObjective()
"#;
    let batch = [("t.lua".to_string(), script.to_vec())];
    let mut o = LevelObjectives::load(&[], &batch, LoadWorld::default(), false, None, &[]);
    assert!(o.report.problems.is_empty(), "{:?}", o.report.problems);
    o.runner.flags.set(&Name::new("Kept"), true, false);
    o.runner.flags.set(&Name::new("Dropped"), true, true);
    let banner = |o: &LevelObjectives| match o.runner.waiting_at("Banner") {
        Some(Node::Wait(Wait::ObjectiveMessage(m))) => (m.shown, m.time),
        other => panic!("the banner's wait: {other:?}"),
    };
    if let Some(Node::Wait(Wait::ObjectiveMessage(m))) = o.runner.sets[1][0].nodes.get_mut(0) {
        (m.shown, m.time) = (true, 3.0);
    }
    assert_eq!(banner(&o), (true, 3.0));
    let mut s = Session { objectives: Some(o), ..Session::default() };
    s.reset();
    let o = s.objectives.as_ref().expect("objectives");
    assert_eq!(banner(o), (false, 0.0), "a new node");
    assert_eq!(o.runner.sets[1].len(), 1, "the batch's objective once");
    let flags = &o.runner.flags;
    assert!(flags.check(&Name::new("Kept")) && !flags.check(&Name::new("Dropped")));
}

#[test]
fn a_restart_readies_the_hud_message() {
    use tutorial::{HudMessage, Tutorial};
    let dt = 1.0 / 30.0;
    let mut s = Session { tutorial: Some(Tutorial::default()), ..Session::default() };
    let t = s.tutorial.as_mut().expect("tutorial");
    t.message.request(HudMessage::SHOWING, 5.0);
    t.update(dt);
    assert_eq!(t.message.current, HudMessage::SHOWING, "the tip shows");
    s.reset();
    let t = s.tutorial.as_mut().expect("tutorial");
    t.update(dt);
    assert_eq!(t.message.current, HudMessage::READY, "the restart's request taken");
    t.message.request(HudMessage::SHOWING, 0.0);
    for _ in 0..600 {
        t.update(dt);
    }
    assert_eq!(t.message.current, HudMessage::SHOWING, "held after 20 s");
}

#[test]
fn a_paused_frame_runs_the_hud_messages_seconds() {
    use tutorial::{HudMessage, Tutorial};
    let w = world(plane(400.0, 0.3));
    let mut s = RiderState::spawn(&w, Vec3::new(0.0, 50.0, 0.0), Vec3::Z).expect("ground");
    let mut x = Session { tutorial: Some(Tutorial::default()), ..Session::default() };
    x.tutorial.as_mut().expect("tutorial").message.request(HudMessage::SHOWING, 4.0);
    x.pause_begin();
    x.paused_frame(&mut s, &w);
    assert_eq!(x.tutorial.as_ref().expect("tutorial").message.current, HudMessage::SHOWING, "taken in the pause");
    let current = |x: &Session| x.tutorial.as_ref().expect("tutorial").message.current;
    for _ in 0..117 {
        x.paused_frame(&mut s, &w);
    }
    assert_eq!(current(&x), HudMessage::SHOWING, "up for its seconds");
    for _ in 0..6 {
        x.paused_frame(&mut s, &w);
    }
    assert_eq!(current(&x), HudMessage::READY, "run out under the pause");
}
