mod blend_array;
mod chooser;
mod layers;
mod mirror;
mod seq;
mod timeline;

use crate::anim_eval::{Controller, Env, GameState, PathNode, Window, blend_new_weight, instantiate};
use crate::anim_graph::{Blend, BlendId, ChooserEntry, Ctrl, CtrlId, Selector, SigId};
use crate::anim_pose::{AnimPose, Delta};

pub use blend_array::*;
pub(crate) use blend_array::{damp as blend_array_damp, select as blend_array_select};
pub use chooser::*;
pub use layers::*;
pub use mirror::*;
pub use seq::*;
pub use timeline::*;

pub struct SwitchBlend {
    pub blend: Blend,
    pub elapsed: f32,
    pub old: Box<dyn Controller>,
}

impl SwitchBlend {
    pub fn old_weight(&self) -> f32 {
        1.0 - crate::anim_eval::blend_new_weight(&self.blend, self.elapsed)
    }

    pub fn update(&mut self, env: &Env, gs: &mut GameState, dt: f32) {
        let step = dt.clamp(0.0, (self.blend.blend_time - self.elapsed).max(0.0));
        self.elapsed += step;
        self.old.update(env, gs, dt);
    }

    pub fn done(&self) -> bool {
        self.old_weight() <= 0.01 || self.elapsed >= self.blend.blend_time
    }
}

pub fn phase(c: &dyn Controller) -> f32 {
    let l = c.length();
    if l > 0.0 { c.time() / l } else { 0.0 }
}

pub fn set_phase(env: &Env, gs: &GameState, c: &mut dyn Controller, p: f32) {
    let l = c.length();
    if l > 0.0 {
        c.set_time(env, gs, p * l);
    }
}
