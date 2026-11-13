use super::*;

#[test]
fn the_reverse_camera_turns_the_stick_half_around() {
    let up = Vec2::Y;
    let behind = crate::rider::reverse_camera_stick(up, Quat::IDENTITY, Quat::IDENTITY, 1.0, Vec3::X);
    assert!((behind - up).length() < 1e-6, "the normal camera leaves the stick alone: {behind}");

    let ahead = Quat::from_rotation_y(std::f32::consts::PI);
    let flipped = crate::rider::reverse_camera_stick(up, ahead, Quat::IDENTITY, 1.0, Vec3::X);
    assert!((flipped - Vec2::NEG_Y).length() < 1e-5, "up (tuck) becomes down (brake): {flipped}");
    let right = crate::rider::reverse_camera_stick(Vec2::X, ahead, Quat::IDENTITY, 1.0, Vec3::X);
    assert!((right - Vec2::NEG_X).length() < 1e-5, "right becomes left: {right}");
}

#[test]
fn the_reverse_camera_uses_the_boards_forward_when_slow() {
    let ahead = Quat::from_rotation_y(std::f32::consts::PI);
    let slow = crate::rider::reverse_camera_stick(Vec2::Y, ahead, Quat::IDENTITY, -1.0, Vec3::X * 0.01);
    assert!((slow - Vec2::Y).length() < 1e-5, "slow: the backward board's axis: {slow}");
    let fast = crate::rider::reverse_camera_stick(Vec2::Y, ahead, Quat::IDENTITY, -1.0, Vec3::X);
    assert!((fast - Vec2::NEG_Y).length() < 1e-5, "fast: travel toward the reverse camera: {fast}");
    let edge = crate::rider::reverse_camera_stick(Vec2::Y, ahead, Quat::IDENTITY, -1.0, Vec3::X * 0.049);
    assert!((edge - Vec2::Y).length() < 1e-5, "under the threshold: the board's axis: {edge}");
}

#[test]
fn the_step_rewrites_the_game_state_stick_only() {
    let w = world(plane(3000.0, 0.0));
    let mut s = moving(&w, Vec3::new(0.0, 1.0, 0.0), Vec3::Z, 0.25);
    s.reverse_camera = Some(Quat::from_rotation_y(std::f32::consts::FRAC_PI_2));
    let raw = Input { tuck: 1.0, ..Input::default() };
    step(&mut s, &w, &raw, DT_TICKS);
    assert!(s.pad_stick.y < -0.9, "up is rewritten to down: {}", s.pad_stick);
    assert!(s.stick_prev.y > 0.9, "the get-going edge keeps the raw pad: {}", s.stick_prev);
}

#[test]
fn omni_get_going_accepts_every_direction() {
    assert!(crate::rider::omni_get_going_gesture(Vec2::NEG_Y, Vec2::ZERO), "straight down enters");
    assert!(crate::rider::omni_get_going_gesture(Vec2::Y, Vec2::ZERO), "straight up enters");
    assert!(!crate::rider::omni_get_going_gesture(Vec2::NEG_Y * 0.2, Vec2::ZERO), "below 0.25 does not");
    assert!(!crate::rider::omni_get_going_gesture(Vec2::NEG_Y, Vec2::NEG_Y * 0.5), "a held stick is no edge");
    assert!(!crate::rider::get_going_gesture(Vec2::NEG_Y, Vec2::ZERO), "LeftStick@158 excludes straight back");
}
