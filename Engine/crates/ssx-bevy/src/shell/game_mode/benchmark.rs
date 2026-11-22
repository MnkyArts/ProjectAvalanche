use bevy::diagnostic::FrameCount;
use bevy::prelude::*;

use super::{GameArgs, GameUiRes, Shell, Stage};
use crate::camera::FlyCam;
use crate::ride::Ride;
use crate::shell::benchmark::record::{BenchmarkScene, SceneWindow, frame_tag};
use crate::shell::benchmark::report::Outcome;
use crate::shell::benchmark::{Camera, FLIGHT_FRAMES_PER_SECOND, Kind, SceneRun};
use crate::shell::game_level::LevelGame;

const LAUNCH_AT: f64 = 5.0;
const MOVIE_CAP: f64 = 1.0;
const START_TIMEOUT: f64 = 400.0;
const STEPS_PER_SECOND: f32 = ssx_physics::SIM_HZ;
const FLIGHT_FROM: f32 = 5.0;
const FLIGHT_BEHIND: f32 = 25.0;
const FLIGHT_ABOVE: f32 = 20.0;
const FLIGHT_LOOK_AHEAD: f32 = 2.0;

pub(super) fn prepare(a: &mut GameArgs, run: &SceneRun) {
    a.movie_cap = Some(MOVIE_CAP);
    match run.scene.kind {
        Kind::FrontEnd { script, .. } => a.script = script.to_string(),
        Kind::Ride { drop, .. } => {
            a.launch = Some((drop.to_string(), LAUNCH_AT));
            a.autoplay = true;
            a.rides = Some(1);
        }
    }
}

#[derive(Default)]
pub(super) struct Flight {
    dir: Option<Vec3>,
}

pub(super) fn scene_frame(
    (shell, ui): (Res<Shell>, Res<GameUiRes>),
    scene: Res<BenchmarkScene>,
    mut window: ResMut<SceneWindow>,
    (count, real): (Res<FrameCount>, Res<Time<Real>>),
    (ride, level): (Option<Res<Ride>>, Option<Res<LevelGame>>),
    mut drop_checked: Local<bool>,
) {
    if window.last.is_some() || window.abort.is_some() {
        return;
    }
    let tag = frame_tag(&count);
    let elapsed = real.elapsed_secs_f64();
    match scene.0.kind {
        Kind::FrontEnd { from, seconds, .. } => {
            let t = shell.host.time;
            if window.first.is_none() && t >= from {
                if shell.stage != Stage::Menus {
                    window.fail(format!("the front end left its menus before the window ({:?})", shell.stage));
                    return;
                }
                window.open(tag, scene.0.id);
            } else if window.first.is_some() && t >= from + seconds {
                let screen = ui.fe.as_ref().map(|fe| fe.screen);
                window.close(tag, scene.0.id, format!("front end at {t:.1} s, screen {screen:?}"));
            } else if window.first.is_none() && elapsed > START_TIMEOUT {
                window.fail(format!("the front end's clock did not reach {from} s within {START_TIMEOUT} s"));
            }
        }
        Kind::Ride { drop, after_go, seconds, camera } => {
            if !*drop_checked && let Some(fe) = ui.fe.as_ref() {
                *drop_checked = true;
                if fe.career.event_named(drop).is_none() {
                    window.abort = Some(if drop.starts_with("exp_dlc_") {
                        Outcome::Skipped {
                            reason: format!(
                                "{drop} is downloadable content and not installed (the launcher's Downloadable \
                                 Content section installs it)"
                            ),
                        }
                    } else {
                        Outcome::Failed { reason: format!("no drop {drop} in the game's data") }
                    });
                    return;
                }
            }
            let Some(ride) = ride.filter(|_| shell.stage == Stage::Level) else {
                if window.first.is_some() {
                    window.fail("the level ended inside the window");
                } else if elapsed > START_TIMEOUT {
                    window.fail(format!("{drop} was not riding within {START_TIMEOUT} s"));
                }
                return;
            };
            let Some(go) = ride.go_step else {
                if elapsed > START_TIMEOUT {
                    window.fail(format!("no GO within {START_TIMEOUT} s"));
                }
                return;
            };
            let (start, end) = window_steps(go, after_go, seconds);
            let ended = level.as_ref().is_some_and(|l| l.results_shown());
            if window.first.is_none() {
                if ride.steps >= start {
                    window.open(tag, scene.0.id);
                } else if ended {
                    let after = ride.steps.saturating_sub(go) as f32 / STEPS_PER_SECOND;
                    window.fail(format!("the ride ended {after:.1} s after the GO, before the window"));
                }
                return;
            }
            let over = match camera {
                Camera::Chase => ride.steps >= end,
                Camera::Flight => window.flight_frames as f32 >= seconds * FLIGHT_FRAMES_PER_SECOND,
            };
            if over || ended {
                let (step, p) = match (camera, window.end_step) {
                    (Camera::Chase, Some(at)) if over => at,
                    _ => (ride.steps, ride.state.traj),
                };
                let check = format!(
                    "GO at ride step {go}; at the window's end ride step {}, the rider at ({:.2}, {:.2}, {:.2}){}",
                    step,
                    p.x,
                    p.y,
                    p.z,
                    if over { "" } else { "; the ride ended inside the window (cut short)" }
                );
                window.close(tag, scene.0.id, check);
            }
        }
    }
}

fn window_steps(go: u64, after_go: f32, seconds: f32) -> (u64, u64) {
    let start = go + (after_go * STEPS_PER_SECOND) as u64;
    (start, start + (seconds * STEPS_PER_SECOND) as u64)
}

pub(super) fn end_step(scene: Res<BenchmarkScene>, ride: Option<Res<Ride>>, mut window: ResMut<SceneWindow>) {
    let Kind::Ride { after_go, seconds, camera: Camera::Chase, .. } = scene.0.kind else { return };
    if window.end_step.is_some() || window.first.is_none() {
        return;
    }
    let Some(ride) = ride else { return };
    let Some(go) = ride.go_step else { return };
    let (_, end) = window_steps(go, after_go, seconds);
    if ride.steps >= end {
        window.end_step = Some((ride.steps, ride.state.traj));
    }
}

pub(super) fn flight_camera(
    scene: Res<BenchmarkScene>,
    mut window: ResMut<SceneWindow>,
    ride: Option<Res<Ride>>,
    mut cameras: Query<&mut Transform, With<FlyCam>>,
    mut flight: Local<Flight>,
) {
    let Kind::Ride { camera: Camera::Flight, .. } = scene.0.kind else { return };
    if window.first.is_none() || window.last.is_some() || window.abort.is_some() {
        return;
    }
    let Some(replay) =
        ride.as_ref().and_then(|r| r.session.bots.as_ref()).and_then(|b| b.bots.first()).map(|b| &b.replay)
    else {
        window.fail("the flight follows the event's first racer: the event has none");
        return;
    };
    // PORT(choice): the flight advances 1/60 s of the recorded ride a frame, so the camera takes the same path in the
    // same frames in every run whatever the frame rate (the race under it keeps the ride's own clock: its riders are
    // where the frame rate put them); the console has no such camera (a test camera of the port's benchmark).
    let t = FLIGHT_FROM + window.flight_frames as f32 / FLIGHT_FRAMES_PER_SECOND;
    let at = |t: f32| Vec3::from_array(replay.position(t.min(replay.duration())));
    let (here, ahead) = (at(t), at(t + 1.0));
    let along = Vec3::new(ahead.x - here.x, 0.0, ahead.z - here.z);
    let dir = along.try_normalize().or(flight.dir).unwrap_or(Vec3::Z);
    flight.dir = Some(dir);
    let eye = here - dir * FLIGHT_BEHIND + Vec3::Y * FLIGHT_ABOVE;
    for mut transform in &mut cameras {
        *transform = Transform::from_translation(eye).looking_at(at(t + FLIGHT_LOOK_AHEAD), Vec3::Y);
    }
    window.flight_frames += 1;
}
