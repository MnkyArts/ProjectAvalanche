use bevy::prelude::*;

use super::pause::pause_now;
use super::{CameraTypeLink, LevelGame};
use crate::audio::music::frontend::FrontEndAudio;
use crate::camera::FlyCam;
use crate::ride::Ride;
use crate::shell::game_mode::GameUiRes;
use ssx_ui::game::GameUi;
use ssx_ui::game::sim_hold::SimHold;

fn hud_camera(camera: &Camera, transform: &GlobalTransform) -> ssx_ui::game::hud::billboards::Camera {
    let viewport = camera.logical_viewport_size().filter(|v| v.x > 0.0 && v.y > 0.0);
    hud_projection(camera.clip_from_view(), transform.to_matrix(), viewport)
}

fn hud_projection(
    clip_from_view: Mat4,
    world_from_view: Mat4,
    viewport: Option<Vec2>,
) -> ssx_ui::game::hud::billboards::Camera {
    const NEAR: f32 = 0.5;
    const FAR: f32 = 8000.0;
    let vp = clip_from_view * world_from_view.inverse();
    let s = FAR / (FAR - NEAR);
    let mut rows = [[0.0; 4]; 4];
    for (i, row) in rows.iter_mut().enumerate() {
        let c = vp.col(i);
        *row = [c.x, c.y, c.w * s, c.w];
    }
    rows[3][2] = (rows[3][3] - NEAR) * s;
    if let Some(v) = viewport {
        let (k, _) = ssx_ui::render::stage_scale((1280.0, 720.0), (v.x, v.y), ssx_ui::render::ScaleMode::NoBorder);
        let (kx, ky) = (v.x / k / 1280.0, v.y / k / 720.0);
        for row in &mut rows {
            row[0] *= kx;
            row[1] *= ky;
        }
    }
    ssx_ui::game::hud::billboards::Camera { view_proj: rows, screen: [1280.0, 720.0] }
}

pub(super) fn prompt_step(
    mut keys: ResMut<ButtonInput<KeyCode>>,
    pads: Query<&Gamepad>,
    mode: Option<Res<crate::camera::CameraMode>>,
    mut level: ResMut<LevelGame>,
    ride: Option<ResMut<Ride>>,
    ui: Res<GameUiRes>,
) {
    let (Some(mut ride), l) = (ride, &mut *level) else { return };
    let Some(p) = l.prompts.as_mut().filter(|_| l.up && !l.results_posted && !ride.hold) else { return };
    p.step(f64::from(ride.seconds()), l.clock, &ui.0, &ride, keys.bypass_change_detection());
    let chase = mode.is_none_or(|m| *m == crate::camera::CameraMode::Chase);
    crate::ride::read_live_controls(&mut ride, &keys, &pads, chase);
}

pub(super) fn feed_step(
    mut ride: ResMut<Ride>,
    mut level: ResMut<LevelGame>,
    mut ui: ResMut<GameUiRes>,
    cams: Query<(&Camera, &GlobalTransform), With<FlyCam>>,
    mut audio: Option<ResMut<FrontEndAudio>>,
    (link, bots, paused_frames): (
        Option<ResMut<CameraTypeLink>>,
        Option<Res<crate::ride::bots::Bots>>,
        Res<crate::ride::PausedFrames>,
    ),
) {
    let l = &mut *level;
    let paused_frame = paused_frames.0 > l.fed_paused;
    l.fed_paused = paused_frames.0;
    if ride.steps <= l.fed {
        l.fed = ride.steps;
        if paused_frame {
            l.feed.camera = cams.iter().next().map(|(c, t)| hud_camera(c, t));
            l.feed.paused_frame(&mut ui.0, &ride.state, &ride.session);
        }
        return;
    }
    l.fed = ride.steps;
    let Some(e) = ride.last_exec else { return };
    let ui = &mut ui.0;
    // TODO(guess): a port choice — the game's pause holds the simulation and with it every write the simulation makes
    // into the UI (0x823D2590: the step +0x22, the scripts +0x1BC, the score manager +0x1DC; in game mode 3 also the
    // live-event manager, 0x823833E8 → 0x82422640), so a ride never steps under it; a session's ride does, and its HUD
    // feed and tutorial step write into held copies of the value table, the HUD's messages, the gear slots and the
    // screen fader that the pause's end hands over (each value's last, the messages in order, the held fader when a
    // fade message reached it); settle: none (the original has no ride stepping under a pause).
    let held = l.live && l.paused;
    if held && l.sim_hold.is_none() {
        l.sim_hold = Some(SimHold::new(ui));
    }
    if let Some(fe) = ui.fe.as_mut().filter(|f| f.fader_by_ride) {
        fe.fader.step(1.0 / ssx_physics::SIM_HZ);
        if let Some(hold) = l.sim_hold.as_mut() {
            hold.step_fader(1.0 / ssx_physics::SIM_HZ);
        }
    }
    let clock = &ride.session.event.clock;
    let (ghosts, racers) = l.ghosts.step(clock.seconds, clock.ms(), &ride.world().triggers, &mut ui.loc);
    l.feed.ghosts =
        if ssx_physics::event::shows_explore_ghosts(ride.session.event.setup.event_type, ride.session.bots.is_some()) {
            ghosts
        } else {
            Vec::new()
        };
    if let Some(b) = bots.as_deref() {
        l.feed.ghosts.extend(b.live.billboards());
    }
    l.feed.ghost_racers = racers;
    l.feed.camera = cams.iter().next().map(|(c, t)| hud_camera(c, t));
    match l.sim_hold.as_mut() {
        Some(hold) => {
            l.feed.paused_frame(ui, &ride.state, &ride.session);
            let feed = &mut l.feed;
            hold.run(ui, |ui| feed.feed(ui, &ride.state, &ride.session, ride.world(), Some(&e)));
        }
        None => l.feed.feed(ui, &ride.state, &ride.session, ride.world(), Some(&e)),
    }
    if let (Some(mut link), Some(v)) = (link, ui.fe.as_mut().and_then(|f| f.camera_restore.take())) {
        if let Some(old) = link.restore {
            warn!("level: the camera did not take the last race start's camera type {old}");
        }
        link.restore = Some(v);
    }
    if let Some(g) = &ride.session.geotags {
        for ev in &g.events {
            if let ssx_physics::session::geotags::GeotagEvent::Collected { id, value, .. } = ev {
                let row = l.geotag_rows.iter().find(|r| r.id == *id);
                let paid = match (row, ui.fe.as_mut()) {
                    (Some(row), Some(fe)) => fe.profile.collect_geotag(g.data.track_key, row),
                    _ => None,
                };
                info!("level: geotag {id} picked up ({value} credits): paid {paid:?}");
            }
        }
    }
    if l.result.is_none()
        && let Some(r) = &ride.session.event.result
    {
        info!(
            "level: results at {:.2} s: time {} ms, points {}, place {:?}, medal {:?}, failed {}",
            ride.seconds(),
            r.time_ms,
            r.points,
            r.rank.map(|k| k + 1),
            r.medal,
            r.failed
        );
        l.result = Some(r.clone());
    }
    if l.result.is_none() && l.win_after.is_some_and(|s| ride.seconds() >= s) {
        let r = ssx_ui::game::fe::host::forced_win(&ride.session.event, &l.rival_values);
        info!(
            "level: --ride-win at {:.2} s: a won result (value {}, time {} ms, points {}, place 1 of {} rival rows) — \
             a test option, not the game's",
            ride.seconds(),
            r.value,
            r.time_ms,
            r.points,
            l.rival_values.len()
        );
        l.result = Some(r);
    }
    let events = ride.session.tutorial.as_mut().map_or_else(Vec::new, |t| t.take_events());
    let rider = l.feed.rider.clone();
    let tutorial = |ui: &mut GameUi| {
        let popups = ui.tutorial_step(&ride.state, &ride.session, &events, &rider);
        if events.iter().any(|e| matches!(e, ssx_physics::session::tutorial::TutorialEvent::Message { .. })) {
            use ssx_ui::game::values::id;
            let v = &ui.values;
            info!(
                "level: the HUD message at ride time {:.2} s{}: enabled {}, {:?} / {:?}; the tutorial HUD {}",
                ride.seconds(),
                if held { " (held by the pause)" } else { "" },
                v.bool(id::HUDMESSAGE_IS_ENABLED),
                v.str(id::HUDMESSAGE_TITLE),
                v.str(id::HUDMESSAGE_MESSAGE),
                v.bool(ssx_ui::game::tutorial::TRICK_IS_ENABLED)
            );
        }
        popups
    };
    let popups = match l.sim_hold.as_mut() {
        Some(hold) => hold.run(ui, tutorial),
        None => tutorial(ui),
    };
    for key in ui.fe.as_mut().map(|fe| std::mem::take(&mut fe.withheld_popups)).unwrap_or_default() {
        info!("level: a network session: the tutorial popup {key:?} is not shown");
    }
    if popups > 0 {
        info!("level: {popups} tutorial popup(s) at ride time {:.2} s", ride.seconds());
        pause_now(l, &mut ride, ui, audio.as_deref_mut());
    }
}

pub fn nis_state(ride: &Ride) -> u8 {
    match ride.state.start.as_ref() {
        None => 0,
        Some(s) if s.intro.as_ref().is_some_and(|i| i.in_ready_loop()) => 2,
        Some(_) => 1,
    }
}

#[cfg(test)]
mod tests;
