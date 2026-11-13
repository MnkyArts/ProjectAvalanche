use bevy::diagnostic::{DiagnosticsStore, FrameTimeDiagnosticsPlugin};
use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use ssx_physics::rider::RiderState;
use ssx_physics::{KMH_PER_M_PER_TICK, SIM_HZ};

use super::{Clock, Pose, Ride};
use crate::camera::chase::ChaseCamera;
use crate::camera::{CameraMode, FlyCam};
use crate::rider::skin::RiderRig;
use crate::rider::visual::{self as rider_visual, RiderBody, RiderRoot};
use crate::shell::screenshot::{FIXED_FRAME, FixedShotClock, PipelinesReady};

pub(super) fn pose_of(s: &RiderState) -> Pose {
    Pose {
        traj: s.traj,
        traj_rot: (s.traj_orientation * Quat::from_rotation_y(-std::f32::consts::FRAC_PI_2)).normalize(),
        board: s.position,
        board_rot: s.alignment,
    }
}

pub(super) fn lerp_pose(a: &Pose, b: &Pose, t: f32) -> Pose {
    Pose {
        traj: a.traj.lerp(b.traj, t),
        traj_rot: a.traj_rot.slerp(b.traj_rot, t),
        board: a.board.lerp(b.board, t),
        board_rot: a.board_rot.slerp(b.board_rot, t),
    }
}

fn render_lag(lag: f32, fresh: u64, frame_dt: f32) -> f32 {
    let step = 1.0 / SIM_HZ;
    (lag + fresh as f32 * step - frame_dt).clamp(0.0, step)
}

fn render_blend(a: &Pose, b: &Pose, t: f32) -> Pose {
    let nlerp = |p: Quat, q: Quat| {
        let q = if p.dot(q) < 0.0 { -q } else { q };
        Quat::from_vec4(Vec4::from(p).lerp(Vec4::from(q), t)).normalize()
    };
    Pose {
        traj: a.traj.lerp(b.traj, t),
        traj_rot: nlerp(a.traj_rot, b.traj_rot),
        board: a.board.lerp(b.board, t),
        board_rot: nlerp(a.board_rot, b.board_rot),
    }
}

#[derive(Resource, Default)]
pub struct PausedFrames(pub u64);

pub(super) fn fixed_step(
    mut ride: ResMut<Ride>,
    mut rig: Option<ResMut<RiderRig>>,
    shadow: Option<Res<crate::particles::shadow_query::PlayerShadowQuery>>,
    mut paused_frames: ResMut<PausedFrames>,
    chase: Option<Res<ChaseCamera>>,
    (pads, mode): (Query<&Gamepad>, Option<Res<CameraMode>>),
) {
    if matches!(ride.clock, Clock::Fixed) && ride.hold && ride.paused {
        paused_frames.0 += 1;
        let r = &mut *ride;
        let ev = r.session.paused_frame(&mut r.state, &r.world);
        if ev.results || ev.finished || ev.looped {
            info!("Ride step {}: a paused frame's score manager: {ev:?}", r.steps);
        }
    }
    if matches!(ride.clock, Clock::Fixed) && !ride.hold {
        if ride.scripted() {
            ride.scripted_controls();
            ride.through_keys(&pads, mode.as_deref().is_none_or(|m| *m == CameraMode::Chase));
        }
        ride.state.survival.shade_query = ride.shade_query(shadow.as_deref());
        let before = shade_trace_before(&ride, shadow.as_deref());
        ride.advance(rig.as_deref_mut().map(|r| &mut r.anim), chase.as_ref().and_then(|c| c.reverse_camera()));
        shade_trace_after(&ride, before);
    }
}

fn shade_trace() -> Option<&'static std::sync::Mutex<std::io::BufWriter<std::fs::File>>> {
    static TRACE: std::sync::OnceLock<Option<std::sync::Mutex<std::io::BufWriter<std::fs::File>>>> =
        std::sync::OnceLock::new();
    TRACE
        .get_or_init(|| {
            let path = std::env::var("SSX_SHADE_TRACE").ok()?;
            let mut f = std::io::BufWriter::new(std::fs::File::create(&path).ok()?);
            use std::io::Write;
            let _ = writeln!(
                f,
                "step\tride_s\treadbacks\tgpu\tray\tshade\ttemperature\tmax\tquery_x\tquery_y\tquery_z\ttraj_x\ttraj_y\ttraj_z\tcascade\tmargin_m"
            );
            Some(std::sync::Mutex::new(f))
        })
        .as_ref()
}

fn shade_trace_before(
    ride: &Ride,
    q: Option<&crate::particles::shadow_query::PlayerShadowQuery>,
) -> Option<(u64, f32, f32, Vec3, Vec3, f32, f32)> {
    shade_trace()?;
    let ray = ride
        .world
        .survival
        .sun
        .map_or(1.0, |sun| ssx_physics::survival::shade_value(&ride.world.collision, ride.state.traj, sun));
    let (readbacks, gpu, cascade, margin) =
        q.map_or((0, f32::NAN, -1.0, 0.0), |q| (q.readbacks, q.value, q.cascade, q.margin));
    Some((readbacks, gpu, ray, ride.render.traj, ride.state.traj, cascade, margin * 1000.0))
}

fn shade_trace_after(ride: &Ride, before: Option<(u64, f32, f32, Vec3, Vec3, f32, f32)>) {
    let (Some(t), Some((readbacks, gpu, ray, q, p, cascade, margin))) = (shade_trace(), before) else { return };
    let v = &ride.state.survival;
    let Ok(mut f) = t.lock() else { return };
    use std::io::Write;
    let _ = writeln!(
        f,
        "{}\t{:.3}\t{readbacks}\t{gpu:.3}\t{ray:.0}\t{:.3}\t{:.4}\t{:.4}\t{:.2}\t{:.2}\t{:.2}\t{:.2}\t{:.2}\t{:.2}\t{cascade:.0}\t{margin:.3}",
        ride.steps,
        ride.steps as f32 / SIM_HZ,
        v.shade,
        v.temperature,
        v.temperature_max,
        q.x,
        q.y,
        q.z,
        p.x,
        p.y,
        p.z
    );
    if ride.steps.is_multiple_of(30) {
        let _ = f.flush();
    }
}

pub(super) fn frame_step(
    ready: Option<Res<PipelinesReady>>,
    mut ride: ResMut<Ride>,
    mut rig: Option<ResMut<RiderRig>>,
    shadow: Option<Res<crate::particles::shadow_query::PlayerShadowQuery>>,
    chase: Option<Res<ChaseCamera>>,
    (mut clock, mut time, mut strategy): (
        Option<ResMut<FixedShotClock>>,
        ResMut<Time<Virtual>>,
        ResMut<TimeUpdateStrategy>,
    ),
    (pads, mode): (Query<&Gamepad>, Option<Res<CameraMode>>),
) {
    let ready = ready.is_some_and(|r| r.0) && !ride.hold;
    let (step_now, last, frame) = match &mut ride.clock {
        Clock::PerFrame { started, remaining, sub, drawn, phase } => {
            *started |= ready;
            let go = *started && *remaining > 0;
            if go {
                *remaining -= 1;
                *phase = *drawn % *sub;
                *drawn += 1;
            }
            (go && *phase == 0, go && *remaining == 0, (go && *remaining > 0).then(|| FIXED_FRAME / *sub))
        }
        Clock::Fixed => (false, false, None),
    };
    if let Some(clock) = clock.as_deref_mut() {
        clock.ride = true;
        match frame {
            Some(d) => {
                *strategy = TimeUpdateStrategy::ManualDuration(d);
                time.unpause();
            }
            None => time.pause(),
        }
    }
    if step_now {
        if ride.scripted() {
            ride.scripted_controls();
            ride.through_keys(&pads, mode.as_deref().is_none_or(|m| *m == CameraMode::Chase));
        }
        ride.state.survival.shade_query = ride.shade_query(shadow.as_deref());
        let before = shade_trace_before(&ride, shadow.as_deref());
        ride.advance(rig.as_deref_mut().map(|r| &mut r.anim), chase.as_ref().and_then(|c| c.reverse_camera()));
        shade_trace_after(&ride, before);
    }
    ride.report |= last;
}

pub(super) fn report(
    mut ride: ResMut<Ride>,
    cameras: Query<&Transform, With<FlyCam>>,
    rig: Option<Res<RiderRig>>,
    diagnostics: Res<DiagnosticsStore>,
) {
    if !ride.report {
        return;
    }
    ride.report = false;
    let s = &ride.state;
    let frame_ms = diagnostics.get(&FrameTimeDiagnosticsPlugin::FRAME_TIME).and_then(|d| d.smoothed()).unwrap_or(0.0);
    let drawn = rig.as_ref().map_or(ride.render.traj, |r| r.drawn_trajectory.0);
    let board = rig.as_ref().map_or(ride.render.board, |r| r.drawn_board);
    info!(
        "Ride after {} steps ({:.1} s): {:.1} km/h, {:?}{}, board point {:.2}, trajectory point {:.2}, drawn \
         AITrajectory {:.2}, drawn Board joint {:.2} ({:+.3} m along the normal from the board point), lean {:.1} deg, distance {:.0} m, physics step {:.3} ms, frame {frame_ms:.1} ms \
         (smoothed){}",
        ride.steps,
        ride.seconds(),
        s.speed() * KMH_PER_M_PER_TICK,
        s.state,
        if s.stopped { " (stopped)" } else { "" },
        s.position,
        s.traj,
        drawn,
        board,
        (board - s.position).dot(s.ground_normal),
        s.lean.to_degrees(),
        ride.distance,
        ride.step_ms,
        rig.map_or(String::new(), |r| {
            let (seconds, _, updates) = r.anim.graph().script_cost();
            format!(
                "; rider animation {:.3} ms/frame, its scripts {:.1} µs/step; {}",
                r.anim_ms,
                seconds * 1e6 / updates.max(1) as f64,
                r.anim.label()
            )
        }),
    );
    if s.contact.is_some() {
        let c =
            ssx_physics::ground::snow_compression(&ride.world.tuning.carving, &s.surface, s.carving.distance_carved);
        let drawn = s.weights[ssx_physics::surface::SNOW_COVERAGE] * s.surface.snow_depth;
        info!(
            "Ride snow: drawn surface {drawn:.3} m above the collision, ridden floor {c:.3} m (SnowDepth {:.2}, \
             RidingDepth {:.2}, carved {:.0} m)",
            s.surface.snow_depth, s.surface.riding_depth, s.carving.distance_carved
        );
    }
    let (l, sc) = (&s.landing, &s.score);
    info!(
        "Ride airborne play: landing {:?} (touchdown pitch {:+.0}, yaw {:+.0} deg; balance pitch {:+.0}), bailing {}, \
         trick {:?} {:?} {:?}, spin {:.0} / flip {:.0} deg, score {:.0}, bank {:.0}, last capture {:.0}{}, boost {:.1}",
        l.quality,
        l.touchdown_pitch.to_degrees(),
        l.touchdown_spin.to_degrees(),
        l.pitch.to_degrees(),
        l.is_bailing(),
        s.tricks.trick_type,
        s.tricks.phase,
        s.tricks.level,
        (sc.spin_fs + sc.spin_bs).to_degrees(),
        (sc.flip_fwd + sc.flip_bwd).to_degrees(),
        sc.score,
        sc.air_bank,
        sc.last_capture,
        if sc.last_lost { " (lost)" } else { "" },
        sc.boost,
    );
    for camera in &cameras {
        let look = camera.translation + camera.forward() * 10.0;
        info!(
            "Ride camera: --camera {:.2},{:.2},{:.2} --look-at {:.2},{:.2},{:.2}",
            camera.translation.x, camera.translation.y, camera.translation.z, look.x, look.y, look.z
        );
    }
}

pub(super) fn place_rider(
    time: Res<Time>,
    mut ride: ResMut<Ride>,
    rig: Option<Res<RiderRig>>,
    mut roots: Query<&mut Transform, (With<RiderRoot>, Without<RiderBody>)>,
    mut bodies: Query<&mut Transform, (With<RiderBody>, Without<RiderRoot>)>,
) {
    let step = 1.0 / SIM_HZ;
    let lag = match ride.clock {
        Clock::Fixed => render_lag(ride.render_lag, ride.steps.saturating_sub(ride.render_steps), time.delta_secs()),
        Clock::PerFrame { started: true, sub, phase, .. } => step * (sub - 1 - phase) as f32 / sub as f32,
        Clock::PerFrame { .. } => 0.0,
    };
    let frac = if ride.steps == 0 { 1.0 } else { 1.0 - lag / step };
    (ride.render_lag, ride.render_steps) = (lag, ride.steps);
    ride.render = render_blend(&ride.prev, &ride.curr, frac);
    ride.render_time = (ride.steps as f32 / SIM_HZ - lag).max(0.0);
    let root = match rig {
        Some(_) => Transform::IDENTITY,
        None => Transform::from_translation(ride.render.board).with_rotation(ride.render.board_rot),
    };
    for mut transform in &mut roots {
        *transform = root;
    }
    for mut transform in &mut bodies {
        *transform = rider_visual::body_transform(ride.render.board_rot);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_clock_alternates_newest_and_half_step() {
        let (frame, step) = (1.0 / 60.0, 1.0 / SIM_HZ);
        let (mut lag, mut overstep) = (0.0_f32, 0.0_f32);
        let mut lags = Vec::new();
        for _ in 0..8 {
            overstep += frame;
            let fresh = (overstep / step + 1e-4).floor() as u64;
            overstep -= fresh as f32 * step;
            lag = render_lag(lag, fresh, frame);
            lags.push((lag / step * 2.0).round() as u32);
        }
        assert_eq!(lags, [0, 1, 0, 1, 0, 1, 0, 1]);
        assert_eq!(render_lag(0.5 * step, 0, 0.1), 0.0);
    }
}
