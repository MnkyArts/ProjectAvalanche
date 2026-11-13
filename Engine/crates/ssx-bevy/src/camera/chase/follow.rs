use std::f32::consts::FRAC_PI_2;

use bevy::prelude::*;
use ssx_physics::SIM_HZ;

use super::{CameraNoise, Chase, ChaseCamera, NisRig, Snapshot, inputs, render_blend};
use crate::camera::{CameraMode, FlyCam, GAME_FAR, GAME_NEAR, GAME_REST_HFOV_DEG, GameProjection};
use crate::ride::Ride;
use crate::rider::skin::RiderRig;

pub fn toggle(
    keys: Res<ButtonInput<KeyCode>>,
    mut mode: ResMut<CameraMode>,
    mut cameras: Query<(&Transform, &mut FlyCam, &mut Projection)>,
) {
    if !keys.just_pressed(KeyCode::Tab) {
        return;
    }
    *mode = match *mode {
        CameraMode::Fly => CameraMode::Chase,
        CameraMode::Chase => {
            for (transform, mut cam, mut projection) in &mut cameras {
                cam.sync_to(transform.rotation);
                set_projection(&mut projection, GAME_REST_HFOV_DEG, GAME_NEAR);
            }
            CameraMode::Fly
        }
    };
}

fn set_projection(projection: &mut Projection, hfov_deg: f32, near: f32) {
    if let Projection::Custom(custom) = projection
        && let Some(p) = custom.get_mut::<GameProjection>()
    {
        p.hfov_deg = hfov_deg;
        p.near = near;
        p.far = GAME_FAR;
    }
}

fn current_view<'a>(mut cameras: impl Iterator<Item = (&'a Transform, &'a Projection)>) -> Snapshot {
    let Some((transform, projection)) = cameras.next() else {
        return Snapshot {
            position: Vec3::ZERO,
            rotation: Quat::IDENTITY,
            fov_deg: GAME_REST_HFOV_DEG,
            near: GAME_NEAR,
        };
    };
    let (fov_deg, near) = match projection {
        Projection::Custom(custom) => {
            custom.get::<GameProjection>().map_or((GAME_REST_HFOV_DEG, GAME_NEAR), |p| (p.hfov_deg, p.near))
        }
        _ => (GAME_REST_HFOV_DEG, GAME_NEAR),
    };
    Snapshot {
        position: transform.translation,
        rotation: transform.rotation * Quat::from_rotation_y(FRAC_PI_2),
        fov_deg,
        near,
    }
}

fn catch_up(
    camera: &mut ChaseCamera,
    ride: &Ride,
    rig: Option<&mut RiderRig>,
    clip: Option<&CameraNoise>,
    view: impl FnOnce() -> Snapshot,
    (restore, spectate): (Option<i32>, Option<&super::SpectateTarget>),
) {
    if let (Some(saved), Some(c)) = (restore, camera.0.as_mut()) {
        c.restore_fallback(saved);
    }
    let fresh = camera.0.as_ref().and_then(|c| ride.steps.checked_sub(c.steps));
    if fresh == Some(0) {
        return;
    }
    let target = spectate.and_then(|t| t.rider.zip(t.inputs));
    if let Some((rider, i)) = target {
        match camera.0.as_mut() {
            Some(c) if c.spectating == Some(rider) && fresh == Some(1) => {
                let s = c.update(ride, &i, ride.step().1, clip);
                (c.prev, c.curr, c.steps) = (c.curr, s, ride.steps);
            }
            _ => {
                info!("chase camera: the spectator's camera on rider {rider} at step {}", ride.steps);
                let mut c = Chase::new(ride, &i, view(), clip);
                c.spectating = Some(rider);
                camera.0 = Some(c);
            }
        }
        return;
    }
    if camera.0.as_ref().is_some_and(|c| c.spectating.is_some()) {
        info!("chase camera: the spectator's camera ends at step {}: the player's own", ride.steps);
        camera.0 = None;
        return catch_up(camera, ride, rig, clip, view, (None, None));
    }
    let i = inputs(ride, rig);
    if let Some((v, nis_rig)) = nis_view(ride) {
        match camera.0.as_mut() {
            Some(c) if c.nis && fresh == Some(1) => {
                (c.prev, c.curr, c.steps, c.nis_rig) = (c.curr, v, ride.steps, nis_rig);
            }
            _ => {
                let mut c = Chase::new(ride, &i, v, clip);
                (c.prev, c.curr, c.nis, c.nis_rig) = (v, v, true, nis_rig);
                camera.0 = Some(c);
            }
        }
        return;
    }
    // TODO(guess): without a camera-end object the outro's `SF_Cam_Vista` takes `CameraController3PFar_Ground`; the normal 3P camera stands in (no disc drop lacks one); settle: porting the far camera graph.
    if let Some((v, nis_rig)) = camera.0.as_ref().filter(|c| c.nis).map(|c| (c.curr, c.nis_rig)) {
        camera.0 = Some(match nis_rig {
            Some(r) => Chase::after_nis(ride, &i, r, v, clip),
            None => Chase::new(ride, &i, v, clip),
        });
        return;
    }
    match camera.0.as_mut() {
        Some(c) if fresh == Some(1) => {
            let s = c.update(ride, &i, ride.step().1, clip);
            (c.prev, c.curr, c.steps) = (c.curr, s, ride.steps);
        }
        _ => camera.0 = Some(Chase::new(ride, &i, view(), clip)),
    }
}

fn nis_view(ride: &Ride) -> Option<(Snapshot, Option<NisRig>)> {
    let intro = ride.state.start.as_ref()?.intro.as_ref()?;
    let (pose, _) = ride.step();
    if let Some(v) = intro.camera((pose.traj_rot, pose.traj)) {
        let rig = NisRig {
            trajectory: v.trajectory,
            eye: v.eye,
            fov_deg: v.hfov_deg,
            nis_orientation: intro.orientation,
            start_position: ride.state.level_start,
        };
        let (traj_rot, traj_pos) = v.trajectory;
        let near = super::collision::post_update_near(
            &ride.world().collision,
            traj_pos,
            traj_rot,
            v.hfov_deg,
            v.near.unwrap_or(super::collision::DEFAULT_NEAR_CLIPPING_PLANE),
        );
        return Some((Snapshot { position: v.position, rotation: v.rotation, fov_deg: v.hfov_deg, near }, Some(rig)));
    }
    let cut = intro.assets.choice.variant == ssx_physics::nis::IntroVariant::Outro
        && matches!(intro.scene, ssx_physics::nis::Scene::Jump { .. });
    let v = ride.vista.filter(|_| cut)?;
    let near =
        super::collision::post_update_near(&ride.world().collision, v.position, v.rotation, v.fov_deg, GAME_NEAR);
    Some((Snapshot { position: v.position, rotation: v.rotation, fov_deg: v.fov_deg, near }, None))
}

pub(super) fn step(
    mode: Res<CameraMode>,
    ride: Res<Ride>,
    mut rig: Option<ResMut<RiderRig>>,
    mut camera: ResMut<ChaseCamera>,
    cameras: Query<(&Transform, &Projection), With<FlyCam>>,
    (clip, spectate): (Option<Res<CameraNoise>>, Option<Res<super::SpectateTarget>>),
    mut link: Option<ResMut<crate::shell::game_level::CameraTypeLink>>,
) {
    if *mode == CameraMode::Chase {
        let restore = link.as_mut().and_then(|l| l.restore.take());
        let view = || current_view(cameras.iter());
        catch_up(&mut camera, &ride, rig.as_deref_mut(), clip.as_deref(), view, (restore, spectate.as_deref()));
    }
    report_type(&camera, link);
}

fn report_type(camera: &ChaseCamera, link: Option<ResMut<crate::shell::game_level::CameraTypeLink>>) {
    if let (Some(mut l), Some(c)) = (link, camera.0.as_ref())
        && l.current != Some(c.meta.camera_type)
    {
        l.current = Some(c.meta.camera_type);
    }
}

pub fn follow(
    mode: Res<CameraMode>,
    ride: Res<Ride>,
    mut rig: Option<ResMut<RiderRig>>,
    mut camera: ResMut<ChaseCamera>,
    mut cameras: Query<(&mut Transform, &mut Projection), With<FlyCam>>,
    (clip, spectate): (Option<Res<CameraNoise>>, Option<Res<super::SpectateTarget>>),
    mut link: Option<ResMut<crate::shell::game_level::CameraTypeLink>>,
) {
    if *mode != CameraMode::Chase {
        return;
    }
    let restore = link.as_mut().and_then(|l| l.restore.take());
    let view = || current_view(cameras.iter());
    catch_up(&mut camera, &ride, rig.as_deref_mut(), clip.as_deref(), view, (restore, spectate.as_deref()));
    report_type(&camera, link);
    let Some(c) = camera.0.as_ref() else { return };
    let t = (ride.render_time * SIM_HZ - (c.steps as f32 - 1.0)).clamp(0.0, 1.0);
    let view = render_blend(&c.prev, &c.curr, t);
    let rotation = view.rotation * Quat::from_rotation_y(-FRAC_PI_2);
    let placed = Transform::from_translation(view.position).with_rotation(rotation);
    let placed = debug_view(&ride).unwrap_or(placed);
    for (mut transform, mut projection) in &mut cameras {
        *transform = placed;
        set_projection(&mut projection, view.fov_deg, view.near);
    }
}

fn debug_view(ride: &Ride) -> Option<Transform> {
    static VIEW: std::sync::OnceLock<Option<(String, f32)>> = std::sync::OnceLock::new();
    let (kind, distance) = VIEW
        .get_or_init(|| {
            let v = std::env::var("SSX_CHASE_VIEW").ok()?;
            let (kind, d) = v.split_once(',').unwrap_or((v.as_str(), "3.5"));
            Some((kind.trim().to_owned(), d.trim().parse().ok()?))
        })
        .as_ref()?;
    let v = ride.state.velocity;
    let forward = Vec3::new(v.x, 0.0, v.z).try_normalize().unwrap_or(Vec3::X);
    let target = ride.render.traj - Vec3::Y * 0.35;
    let (eye, up) = match kind.as_str() {
        "side" => (target + forward.cross(Vec3::Y) * *distance + Vec3::Y * 0.3, Vec3::Y),
        "front" => (target + forward * *distance + Vec3::Y * 0.5, Vec3::Y),
        "top" => (target + Vec3::Y * *distance, forward),
        _ => return None,
    };
    Some(Transform::from_translation(eye).looking_at(target, up))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn current_view_inverts_the_drawn_rotation() {
        let game = Quat::from_rotation_y(0.7) * Quat::from_rotation_z(-0.3);
        let transform = Transform::from_xyz(1.0, 2.0, 3.0).with_rotation(game * Quat::from_rotation_y(-FRAC_PI_2));
        let projection = Projection::default();
        let view = current_view([(&transform, &projection)].into_iter());
        assert!(view.rotation.angle_between(game) < 1e-5 && view.position == Vec3::new(1.0, 2.0, 3.0));
        assert_eq!(view.fov_deg, GAME_REST_HFOV_DEG);
    }
}
