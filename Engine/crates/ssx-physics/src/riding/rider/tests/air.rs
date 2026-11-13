use super::*;

#[test]
fn janked_detector_counts_and_distance_test() {
    let t = crate::testutil::tuning().janked;
    let mut j = JankedState::new(Vec3::ZERO);
    for k in 1..=23 {
        j.update(&t, true, true, Vec3::ZERO, 2.0);
        assert_eq!(j.janked, k == 23, "step {k}");
    }
    let mut j = JankedState::new(Vec3::ZERO);
    (0..100).for_each(|_| j.update(&t, false, true, Vec3::ZERO, 2.0));
    assert!(!j.janked);
    for (v, janked) in [(0.05f32, true), (0.15, true), (0.6, false)] {
        let mut j = JankedState::new(Vec3::ZERO);
        (0..60).for_each(|k| j.update(&t, true, true, Vec3::X * (v * 2.0 * k as f32), 2.0));
        assert_eq!(j.janked, janked, "{v} m/tick");
    }
    for (gap, janked) in [(2usize, true), (3, false)] {
        let mut j = JankedState::new(Vec3::ZERO);
        for k in 0..40 {
            let on = k % (10 + gap) < 10;
            j.update(&t, on, on, Vec3::ZERO, 2.0);
        }
        assert_eq!(j.janked, janked, "gaps of {gap} steps");
    }
}

fn rock_pocket() -> World {
    let mut c = CollisionWorld::default();
    let (t, l, h) = (60f32.to_radians().tan(), 3.0f32, 8.0f32);
    let p = |x: f32, z: f32| Vec3::new(x, x.abs() * t, z);
    add_quad(&mut c, [p(-l, -l), p(-l, l), p(0.0, l), p(0.0, -l)], 1);
    add_quad(&mut c, [p(0.0, -l), p(0.0, l), p(l, l), p(l, -l)], 1);
    let v = |x: f32, y: f32, z: f32| Vec3::new(x, y, z);
    add_quad(&mut c, [v(-l, 0.0, -l), v(l, 0.0, -l), v(l, h, -l), v(-l, h, -l)], 1);
    add_quad(&mut c, [v(-l, 0.0, l), v(-l, h, l), v(l, h, l), v(l, 0.0, l)], 1);
    c.finish();
    world(c)
}

#[test]
fn janked_in_a_rock_pocket_freezes_until_a_rewind() {
    use crate::rewind::RewindBuffer;
    let w = rock_pocket();
    let mut s = RiderState::spawn(&w, Vec3::new(0.8, 3.0, 0.5), Vec3::Z).expect("pocket");
    let mut buffer = RewindBuffer::new();
    let (mut janked_at, mut frozen) = (None, None::<(Vec3, usize)>);
    for i in 0..300 {
        step(&mut s, &w, &Input::default(), DT_TICKS);
        buffer.record(&s, DT_TICKS);
        assert!(s.state != RideState::Riding, "no ridable ground in the pocket");
        if s.janked.janked && janked_at.is_none() {
            janked_at = Some(i);
            frozen = Some((s.traj, i));
        }
        if let Some((at, since)) = frozen.filter(|f| i > f.1) {
            assert!(s.traj == at && s.velocity == Vec3::ZERO);
            assert_eq!(s.janked.anim_complete, i - since >= 30, "step {i}");
            if s.janked.anim_complete {
                break;
            }
        }
    }
    let at = janked_at.expect("janked in the pocket");
    assert!((22..120).contains(&at), "janked at step {at}");
    assert!(s.janked.anim_complete && s.janked.janked);
    let n = buffer.len();
    buffer.record(&s, DT_TICKS);
    assert_eq!(buffer.len(), n, "recording stopped by BLN_JankedAnimComplete");
    let back = buffer.rewind(&mut s, 80.0).expect("snapshots");
    assert_eq!(back, 80.0);
    assert!(!s.janked.janked && !s.janked.anim_complete);
    assert_eq!(buffer.len(), n - 40);
    buffer.record(&s, DT_TICKS);
    assert_eq!(buffer.len(), n - 40 + 1, "recording again after the rewind");
}

#[test]
fn compression_from_the_a_hold_filter() {
    use crate::jump::calculated_compression;
    assert_eq!(calculated_compression(0.0, 0.0, 0.0), 0.0);
    assert!((calculated_compression(0.4, 0.0, 0.0) - 0.4).abs() < 1e-6);
    assert!((calculated_compression(0.2, -0.6, 0.0) - 0.6).abs() < 1e-6);
    assert!((calculated_compression(1.7, 0.0, 0.0) - 1.0).abs() < 1e-6);
    assert!((calculated_compression(0.5, 0.0, 1.0) - 0.85).abs() < 1e-6);
    let p = crate::testutil::tuning().player;
    let mut j = JumpState::default();
    (0..9).for_each(|_| j.update_compression(true, 0.0, DT_TICKS, &p));
    assert!((j.compression - (1.0 - (-1.98f32).exp())).abs() < 1e-5, "{}", j.compression);
    let held = j.compression;
    j.update_compression(false, 0.0, DT_TICKS, &p);
    assert!((j.compression - held * (-0.22f32).exp()).abs() < 1e-6);
    let w = world(plane(10.0, 0.0));
    let v = Vec3::Z * (100.0 / KMH_PER_M_PER_TICK);
    let d0 = jump_launch(&w, &launch_at(v, true, 0.0)).expect("moving").length();
    let d1 = jump_launch(&w, &launch_at(v, true, held)).expect("moving").length();
    assert!((d1 / d0 - (1.0 + 0.25 * held)).abs() < 1e-4, "{d0} {d1}");
}

fn jump_with(
    air_input: Input,
    tricks: TrickInput,
    tweak: impl FnMut(&mut RiderState, usize),
) -> (RiderState, f32, Vec<RideState>) {
    jump_with_fn(air_input, |_| tricks, tweak)
}

fn jump_with_fn(
    air_input: Input,
    tricks: impl Fn(usize) -> TrickInput,
    mut tweak: impl FnMut(&mut RiderState, usize),
) -> (RiderState, f32, Vec<RideState>) {
    let w = world(plane(3000.0, 0.0));
    let mut s = moving(&w, Vec3::new(0.0, 1.0, -2000.0), Vec3::Z, 60.0 / KMH_PER_M_PER_TICK);
    for i in 0..10 {
        let hold = (5..8).contains(&i);
        let steer = if i >= 5 { air_input.steer } else { 0.0 };
        step(&mut s, &w, &Input { jump: hold, steer, ..Input::default() }, DT_TICKS);
    }
    let (mut turned, mut states) = (0.0f32, Vec::new());
    let mut prev = s.heading;
    for i in 0..120 {
        let air = s.state != RideState::Riding;
        if air {
            tweak(&mut s, i);
        }
        let (inp, tr) = if air { (air_input, tricks(i)) } else { (Input::default(), TrickInput::default()) };
        step_with(&mut s, &w, &inp, &tr, DT_TICKS);
        states.push(s.state);
        let h = s.heading;
        if s.state != RideState::Riding {
            turned += (prev.x * h.z - prev.z * h.x).atan2(prev.x * h.x + prev.z * h.z);
        }
        prev = h;
        if states.len() > 5 && s.state == RideState::Riding && s.landing.ticks > 30.0 {
            break;
        }
    }
    (s, turned, states)
}

#[test]
fn jump_with_spin_input_lands_aligned() {
    let mut last = (0.0, 0.0);
    let (s, turned, states) = jump_with(Input { steer: 1.0, ..Input::default() }, TrickInput::default(), |s, _| {
        last = (s.air.spin.vel, s.air.ticks_to_land);
    });
    assert!(states.contains(&RideState::Jump));
    let deg = turned.to_degrees().abs();
    assert!(deg > 150.0, "spun {deg}°");
    let td = s.landing.touchdown_spin.to_degrees();
    assert!(td.abs() < 90.0 && s.landing.quality != crate::landing::LandingQuality::Bail, "{td}° ({last:?})");
    let vg = s.velocity.reject_from(s.ground_normal).normalize();
    assert!(vg.dot(s.heading).abs() > 0.99, "board along the travel after the snap: {}", vg.dot(s.heading));
    assert!((s.landing.spin_target.abs() - PI).abs() < 1e-4 || s.landing.spin_target == 0.0);
    let sc = &s.score;
    let (fs, bs) = (sc.spin_fs.to_degrees(), sc.spin_bs.to_degrees());
    assert!((fs + bs - deg).abs() < 40.0, "accumulated {fs}° + {bs}° vs {deg}° turned");
    let expect = crate::scoring::spin_points(fs) + crate::scoring::spin_points(bs);
    assert!(expect > 0.0 && (sc.last_capture - expect).abs() < 0.5, "capture {} (expected {expect})", sc.last_capture);
    assert!(sc.boost > 0.0 && !sc.last_lost);
    let (mut s, turned, _) = jump_with(Input::default(), TrickInput::default(), |_, _| {});
    assert!(turned.abs() < 0.05 && s.landing.quality == crate::landing::LandingQuality::Sloppy);
    let w = world(plane(3000.0, 0.0));
    ride(&w, &mut s, &Input::default(), 90, |_| {});
    assert_eq!(s.landing.quality, crate::landing::LandingQuality::Clean);
}

fn land_pitched(pitch: f32) -> RiderState {
    jump_with(Input::default(), TrickInput::default(), |s, _| {
        if s.air.prediction.is_some_and(|p| p.ticks < 9.0) {
            let h = Vec3::new(s.velocity.x, 0.0, s.velocity.z).normalize();
            let level = Quat::from_mat3(&glam::Mat3::from_cols(h, Vec3::Y, h.cross(Vec3::Y)));
            s.alignment = level * Quat::from_rotation_z(pitch);
            s.traj_orientation = s.alignment;
            (s.air.flip.vel, s.air.spin.vel) = (0.0, 0.0);
            (s.angular_velocity_q, s.angular_velocity) = (Quat::IDENTITY, Vec3::ZERO);
            s.score.air_bank = 500.0;
            s.score.boost = 40.0;
        }
    })
    .0
}

#[test]
fn flipped_late_bails_and_loses_the_bank() {
    let s = land_pitched(160f32.to_radians());
    assert!(s.landing.touchdown_pitch > 135f32.to_radians(), "{}°", s.landing.touchdown_pitch.to_degrees());
    assert_eq!(s.landing.quality, crate::landing::LandingQuality::Bail);
    let sc = &s.score;
    assert!(sc.last_lost && sc.last_capture >= 500.0 && sc.score == 0.0, "{sc:?}");
    assert!((s.score.boost - 20.0).abs() < 1e-3, "boost {}", s.score.boost);
    let s = land_pitched(-160f32.to_radians());
    assert!(
        s.landing.touchdown_pitch < -135f32.to_radians() && s.landing.quality == crate::landing::LandingQuality::Bail
    );
    assert!(!s.score.last_lost && s.score.last_capture >= 500.0, "{:?}", s.score);
}

#[test]
fn landing_mid_grab_is_an_uber_crash() {
    let super_tricky = |s: &mut RiderState| {
        (s.score.awesome_timer, s.score.super_timer, s.score.super_activation) = (5000.0, 5000.0, 1.0);
        (s.score.is_awesome, s.score.is_super_awesome) = (true, true);
    };
    let sig = TrickInput { y: true, signature: true, ..TrickInput::default() };
    let (s, _, _) = jump_with(Input::default(), sig, |s, _| super_tricky(s));
    assert_ne!(s.landing.quality, crate::landing::LandingQuality::UberCrash, "gate still closed");
    let (s, _, _) = jump_with(Input::default(), sig, |s, i| {
        super_tricky(s);
        if i == 0 {
            s.traj_velocity.y += 0.05;
        }
    });
    assert_eq!(s.landing.quality, crate::landing::LandingQuality::UberCrash);
    let rocket = TrickInput { y: true, ..TrickInput::default() };
    let (s, _, _) = jump_with(Input::default(), rocket, |_, _| {});
    assert_ne!(s.landing.quality, crate::landing::LandingQuality::UberCrash, "gate still closed");
    let lift = |s: &mut RiderState, i: usize| {
        if i == 0 {
            s.traj_velocity.y += 0.05;
        }
    };
    let (s, _, _) = jump_with(Input::default(), rocket, lift);
    assert_eq!(s.landing.quality, crate::landing::LandingQuality::UberCrash);
    assert!(s.score.last_lost || s.score.last_capture == 0.0, "the crash is a scoring wipeout");
    let (s, _, _) = jump_with_fn(Input::default(), |i| TrickInput { y: i < 30, ..TrickInput::default() }, lift);
    assert_ne!(s.landing.quality, crate::landing::LandingQuality::UberCrash);
    assert!(s.score.last_capture > 0.0 && !s.score.last_lost, "grab points paid: {}", s.score.last_capture);
}

#[test]
fn uber_crash_recovers_after_a_stop_and_a_hop() {
    let w = world(plane(3000.0, 0.0));
    let mut s = moving(&w, Vec3::new(0.0, 1.0, -2000.0), Vec3::Z, 30.0 / KMH_PER_M_PER_TICK);
    s.landing = crate::landing::LandingState { uber_crash: true, ..Default::default() };
    s.uber_trigger = true;
    s.tricks.ticks_since_sig_held = 100.0;
    let (mut getting_up, mut hopped, mut back) = (None, false, None);
    for i in 0..400 {
        step(&mut s, &w, &Input { steer: 1.0, ..Input::default() }, DT_TICKS);
        match s.landing.uber_recovery {
            UberRecovery::GettingUp(_) if getting_up.is_none() => getting_up = Some(i),
            UberRecovery::Hop(_) => hopped = true,
            _ => {}
        }
        if hopped && s.state == RideState::Riding && !s.landing.uber_crash {
            back = Some(i);
            break;
        }
    }
    let up = getting_up.expect("the slide ends with a stop request");
    let back = back.expect("recovered");
    assert!(hopped && (back - up) as f32 * DT_TICKS >= 110.0, "get-up {up} → back {back}");
    assert!(!s.landing.is_bailing());
}

#[test]
fn a_held_signature_crashes_twice_more_on_the_recovery_hops() {
    let w = world(plane(3000.0, 0.0));
    let mut s = moving(&w, Vec3::new(0.0, 1.0, -2000.0), Vec3::Z, 30.0 / KMH_PER_M_PER_TICK);
    s.landing = crate::landing::LandingState { uber_crash: true, ..Default::default() };
    s.uber_trigger = true;
    s.tricks.ticks_since_sig_held = 0.0;
    let mut landings = Vec::new();
    for _ in 0..1500 {
        let was_air = s.state != RideState::Riding;
        step(&mut s, &w, &Input::default(), DT_TICKS);
        if was_air && s.state == RideState::Riding {
            landings.push(s.landing.uber_crash);
            if !s.landing.uber_crash {
                break;
            }
        }
    }
    assert_eq!(landings, [true, true, false], "hop landings: crash, crash, ride");
}

#[test]
fn prewind_builds_on_the_ground_and_decays_without_the_stick() {
    let w = world(plane(2000.0, 0.0));
    let mut s = moving(&w, Vec3::new(0.0, 1.0, -1500.0), Vec3::Z, 0.0);
    let held = Input { jump: true, ..Input::default() };
    for _ in 0..5 {
        step(&mut s, &w, &held, DT_TICKS);
    }
    assert!((s.prewind - (1.0 - (-0.1f32 * 10.0).exp())).abs() < 1e-5, "accum 0.1/tick: {}", s.prewind);
    let p0 = s.prewind;
    step(&mut s, &w, &Input { tuck: 0.8, ..Input::default() }, DT_TICKS);
    assert!((s.prewind - p0).abs() < 1e-6);
    step(&mut s, &w, &Input::default(), DT_TICKS);
    assert!((s.prewind - p0 * (-0.4f32).exp()).abs() < 1e-5, "{}", s.prewind);
    s.state = RideState::Airborne;
    let before = s.prewind;
    s.prewind_reset = false;
    update_prewind(&mut s, &w, &held, false, DT_TICKS);
    assert!(s.prewind < before);
    s.state = RideState::Riding;
    s.landing.tumbling = true;
    update_prewind(&mut s, &w, &held, false, DT_TICKS);
    assert_eq!(s.prewind, 0.0);
}

#[test]
fn no_jump_while_wiping_out_and_an_air_phase_ends_the_bail() {
    let w = world(plane(2000.0, 0.0));
    let mut s = moving(&w, Vec3::new(0.0, 1.0, -1500.0), Vec3::Z, 60.0 / KMH_PER_M_PER_TICK);
    (s.landing.bail_on_landing, s.landing.tumbling, s.landing.stable) = (true, true, false);
    s.landing.pitch = 150f32.to_radians();
    s.nested_stable = false;
    for i in 0..12 {
        step(&mut s, &w, &Input { jump: i < 8, ..Input::default() }, DT_TICKS);
        assert_eq!(s.state, RideState::Riding, "step {i}");
    }
    assert!(s.landing.bail_on_landing && s.landing.landing_bail_ticks < crate::landing::BAIL_ON_LANDING_MAX_TICKS);
    s.tricks.ticks_since_sig_held = f32::MAX;
    s.traj.y += 5.0;
    let (mut was_air, mut landed) = (false, false);
    for _ in 0..60 {
        step(&mut s, &w, &Input::default(), DT_TICKS);
        was_air |= s.state == RideState::Airborne;
        landed |= was_air && s.state == RideState::Riding;
        if landed {
            break;
        }
    }
    assert!(landed && !s.landing.bail_on_landing && !s.landing.is_bailing(), "{:?}", s.landing.quality);
}

fn bailing_on(slope: f32) -> (World, RiderState) {
    let w = world(plane(20000.0, slope));
    let down = Vec3::new(0.0, -slope.sin(), slope.cos());
    let mut s = moving(&w, down * -15000.0 + Vec3::Y, down, 60.0 / KMH_PER_M_PER_TICK);
    (s.landing.bail_on_landing, s.landing.tumbling, s.landing.stable) = (true, true, false);
    s.landing.pitch = 150f32.to_radians();
    s.nested_stable = false;
    (w, s)
}

#[test]
fn a_landing_bail_on_smooth_ground_ends_at_the_bail_limit() {
    for degrees in [0.0f32, 35.0] {
        let (w, mut s) = bailing_on(degrees.to_radians());
        let (mut bail_steps, mut airborne) = (0, false);
        for _ in 0..900 {
            step(&mut s, &w, &Input::default(), DT_TICKS);
            airborne |= s.state != RideState::Riding;
            bail_steps += usize::from(s.landing.bail_on_landing);
        }
        assert!(!airborne, "{degrees}°: the slide stays on the ground");
        assert_eq!((bail_steps + 1) as f32 * DT_TICKS, crate::landing::BAIL_ON_LANDING_MAX_TICKS, "{degrees}°");
        let l = &s.landing;
        assert!(!l.is_bailing() && !l.tumbling && l.pitch.abs() < 25f32.to_radians(), "{degrees}°: {l:?}");
    }
}

#[test]
fn a_dead_riders_landing_bail_is_not_lifted() {
    let (w, mut s) = bailing_on(0.0);
    s.carving.is_dead = true;
    ride(&w, &mut s, &Input::default(), 200, |_| {});
    assert!(s.landing.bail_on_landing && s.landing.landing_bail_ticks == 0.0);
}

#[test]
fn the_nested_flow_chains_offbalance_stable_stop_in_one_update() {
    let w = world(plane(500.0, 0.0));
    let mut s = moving(&w, Vec3::new(0.0, 1.0, 0.0), Vec3::Z, 0.0);
    (s.nested_stable, s.request_stop, s.stopped) = (false, true, false);
    step(&mut s, &w, &Input::default(), DT_TICKS);
    assert!(s.stopped && s.nested_stable);
    let mut s = moving(&w, Vec3::new(0.0, 1.0, 0.0), Vec3::Z, 0.0);
    (s.nested_stable, s.request_stop, s.landing.bail_on_landing) = (false, true, true);
    step(&mut s, &w, &Input::default(), DT_TICKS);
    assert!(!s.stopped && !s.nested_stable);
}

#[test]
fn player_speed_and_the_tuck_blend() {
    let w = world(plane(500.0, 0.0));
    let mut s = moving(&w, Vec3::new(0.0, 1.0, 0.0), Vec3::Z, 0.5);
    s.traj_velocity = Vec3::ZERO;
    update_player_speed(&mut s, false);
    assert!((s.player_speed - 30.0).abs() < 1e-4 && (s.player_speed_ratio - 30.0 / 45.0).abs() < 1e-6);
    update_player_speed(&mut s, true);
    assert_eq!((s.player_speed, s.player_speed_ratio), (0.0, 0.0));
    s.velocity = Vec3::Z;
    update_player_speed(&mut s, false);
    assert_eq!(s.player_speed_ratio, 1.0);
    s.stick_y_filtered = 0.25;
    assert!((tuck_blend(&s) - (0.3 + 0.4)).abs() < 1e-6);
    s.stick_y_filtered = 1.0;
    assert_eq!(tuck_blend(&s), 1.0);
}

#[test]
fn getting_going_stands_in_the_jump_start_for_36_ticks() {
    let theta = 25f32.to_radians();
    let w = world(plane(500.0, theta));
    let mut s = RiderState::spawn(&w, Vec3::new(0.0, 1.0, 0.0), Vec3::Z).expect("ground");
    ride(&w, &mut s, &Input::default(), 60, |_| {});
    assert!(s.stopped);
    let at = s.traj;
    step(&mut s, &w, &Input { tuck: 1.0, ..Input::default() }, DT_TICKS);
    assert_eq!(s.get_going.as_ref().map(|g| g.ticks), Some(2.0));
    for i in 1..18 {
        step(&mut s, &w, &Input::default(), DT_TICKS);
        assert_eq!(s.get_going.as_ref().map(|g| g.ticks), Some(2.0 * (i + 1) as f32), "step {i}");
    }
    assert!((s.traj - at).reject_from(s.ground_normal).length() < 0.05, "{:?}", s.traj - at);
    s.prewind_reset = false;
    step(&mut s, &w, &Input::default(), DT_TICKS);
    assert!(s.get_going.is_none() && !s.stopped && s.state == RideState::Riding);
    assert!(s.prewind_reset && s.landing.landing_score == 1.0);
}

#[test]
fn entering_gameplay_starts_the_track_one_step_later() {
    let w = world(plane(500.0, 0.1));
    let mut s = RiderState::spawn(&w, Vec3::new(0.0, 1.0, 0.0), Vec3::Z).expect("ground");
    (s.track_started, s.trigger_go, s.gameplay) = (false, false, Some(GameplayNode::new()));
    step(&mut s, &w, &Input::default(), DT_TICKS);
    assert!(s.trigger_go && !s.track_started);
    step(&mut s, &w, &Input::default(), DT_TICKS);
    assert!(s.trigger_go && s.track_started);
    assert_eq!(s.gameplay.as_ref().map(|g| g.ticks), Some(2.0 * DT_TICKS));
}

#[test]
fn a_janked_flag_waits_for_the_get_going_to_end() {
    let w = world(plane(500.0, 25f32.to_radians()));
    let mut s = RiderState::spawn(&w, Vec3::new(0.0, 1.0, 0.0), Vec3::Z).expect("ground");
    ride(&w, &mut s, &Input::default(), 60, |_| {});
    assert!(s.stopped);
    step(&mut s, &w, &Input { tuck: 1.0, ..Input::default() }, DT_TICKS);
    for _ in 1..6 {
        step(&mut s, &w, &Input::default(), DT_TICKS);
    }
    s.janked.janked = true;
    for i in 6..18 {
        step(&mut s, &w, &Input::default(), DT_TICKS);
        assert_eq!(s.get_going.as_ref().map(|g| g.ticks), Some(2.0 * (i + 1) as f32), "step {i}");
        assert_eq!(s.janked.anim_ticks, 0.0, "step {i}");
    }
    step(&mut s, &w, &Input::default(), DT_TICKS);
    assert!(s.get_going.is_none() && s.state == RideState::Riding);
    assert_eq!((s.janked.anim_ticks, s.velocity), (DT_TICKS, Vec3::ZERO));
    step(&mut s, &w, &Input::default(), DT_TICKS);
    assert_eq!(s.janked.anim_ticks, 2.0 * DT_TICKS);
}

#[test]
fn the_janked_pin_holds_the_trajectory_from_tick_58() {
    let w = world(plane(500.0, 0.0));
    let mut s = moving(&w, Vec3::new(0.0, 1.0, 0.0), Vec3::Z, 0.0);
    s.janked.janked = true;
    for _ in 0..28 {
        step(&mut s, &w, &Input::default(), DT_TICKS);
    }
    assert!(s.janked_pin.is_none() && s.janked.anim_ticks == 56.0);
    let cached = s.traj_cached;
    step(&mut s, &w, &Input::default(), DT_TICKS);
    assert_eq!(s.janked_pin.map(|p| p.0), Some(cached));
    s.traj += Vec3::X;
    s.traj_cached = s.traj;
    step(&mut s, &w, &Input::default(), DT_TICKS);
    assert!((s.traj - cached).length() < 1e-3, "{:?}", s.traj - cached);
}

#[test]
fn air_tricks_start_after_the_jump_sequences_end_window() {
    let w = world(plane(500.0, 0.0));
    let mut s = moving(&w, Vec3::new(0.0, 1.0, 0.0), Vec3::Z, 0.5);
    s.state = RideState::Jump;
    let runs: Vec<Option<f32>> = (1..=9)
        .map(|k| {
            s.jump_ticks = 2.0 * k as f32;
            air_tricks_ticks(&s, DT_TICKS)
        })
        .collect();
    assert_eq!(runs[..7], [None; 7]);
    assert_eq!((runs[7], runs[8]), (Some(1.0), Some(2.0)), "{runs:?}");
    s.state = RideState::Airborne;
    assert_eq!(air_tricks_ticks(&s, DT_TICKS), Some(2.0));
}
