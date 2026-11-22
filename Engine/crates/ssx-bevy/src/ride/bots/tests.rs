use super::*;

#[test]
fn the_own_ghost_fades_out_on_the_player() {
    let mut f = HoloFade::default();
    for _ in 0..6 {
        f.update(HOLOGRAM_RACER_TYPE, 1.0, 100.0, 4, |_| 0.0);
    }
    assert_eq!(f.fade, 1.0);
    let far = f.update(HOLOGRAM_RACER_TYPE, 0.0, 100.0, 4, |max| max);
    assert_eq!((f.alpha, far.x, far.y), (1.0, 0.0, 0.0));
    let on = f.update(HOLOGRAM_RACER_TYPE, 0.0, 0.5, 4, |max| max);
    assert_eq!((f.alpha, on.x), (0.0, 1.0));
    assert!((on.y - 0.7).abs() < 1e-6);
    f.update(HOLOGRAM_RACER_TYPE, 0.0, 1.05, 4, |_| 0.0);
    assert!((f.alpha - 0.5).abs() < 1e-6, "{}", f.alpha);
    let mid = f.update(HOLOGRAM_RACER_TYPE, 0.0, 10.0, 4, |_| 0.0);
    assert!((mid.x - 0.5).abs() < 1e-6, "{}", mid.x);
    f.update(2, 0.0, 0.5, 4, |_| 0.0);
    assert_eq!(f.alpha, 1.0, "an explore ghost is not faded on the player");
}

#[test]
fn the_fades_follow_the_simulation_not_the_frames() {
    let mut c = FadeClock { steps: 0, paused: 7 };
    assert_eq!(c.advance(0, 7), 0, "a held frame before the first step");
    assert_eq!(c.advance(1, 7), 1);
    assert_eq!(c.advance(1, 7), 0, "a second frame on the same step");
    assert_eq!(c.advance(1, 9), 2, "two paused frames");
    assert_eq!(c.advance(3, 9), 2, "a catch-up batch's two steps");
    assert_eq!(c.advance(1, 9), 1, "restarted: the new ride's step 1");
    assert_eq!(c, FadeClock { steps: 1, paused: 9 });
}
