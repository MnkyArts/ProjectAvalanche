use bevy::camera::visibility::NoFrustumCulling;
use bevy::prelude::*;

#[derive(Component)]
pub struct WarmUp(u32);

// PORT(choice): a warm-up draw stays 5 frames (a material's pipeline is queued at the first frame the render world sees
// the entity in a view, the compile then runs on Bevy's background tasks) and sits beyond every far plane; the game
// compiles its effects offline (the `.fxo` files).
const FRAMES: u32 = 5;

const NOWHERE: Vec3 = Vec3::splat(-1.0e6);

pub fn draw() -> (Transform, NoFrustumCulling, WarmUp) {
    (Transform::from_translation(NOWHERE).with_scale(Vec3::splat(1.0e-3)), NoFrustumCulling, WarmUp(FRAMES))
}

pub fn end(mut commands: Commands, mut draws: Query<(Entity, &mut WarmUp)>) {
    for (e, mut w) in &mut draws {
        if w.0 == 0 {
            commands.entity(e).despawn();
        } else {
            w.0 -= 1;
        }
    }
}
