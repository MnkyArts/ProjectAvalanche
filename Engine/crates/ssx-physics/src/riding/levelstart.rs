use glam::{Quat, Vec2, Vec3};

use crate::rider::World;
use crate::rootmotion::RootMotion;
use crate::surface::ChatterInputs;
use crate::tags::{BoolTag, TagTracker, TagWindow};

pub const READY_LOOP_CLIP: &str = "snow_start_ready_loop_male_breath_character";
pub const GO_CHOOSER: &str = "chooser_snow_start_go_character";
pub const GO_CLIPS: [&str; 3] =
    ["snow_start_go_male_character", "snow_start_go_character_01", "snow_start_go_character_03"];
pub const GO_STOP_TICK: f32 = 40.0;
const GO_STOP_WINDOW: TagWindow = TagWindow::new(GO_STOP_TICK, 0.0, false);
const GO_STOP_TAG: BoolTag = BoolTag { value: true, mode: 1, reset: None };
pub const OFFSET_RAY_UP: f32 = 1.0;
pub const OFFSET_RAY_DOWN: f32 = 5.0;
pub const RSTICK_UP_MAGNITUDE: f32 = 0.54;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Phase {
    Idle { ticks: f32 },
    Go { ticks: f32, clip: usize },
}

#[derive(Clone, Debug, PartialEq)]
pub struct LevelStart {
    pub phase: Phase,
    pub position: Vec3,
    pub orientation: Quat,
    pub offset: Vec3,
    pub request_transition: bool,
    pub request_stop: bool,
    pub request_skip: bool,
    pub trigger_transition: bool,
    pub trigger_stop: bool,
    pub trigger_skip: bool,
    stop_tag: TagTracker,
    pub intro: Option<crate::nis::IntroState>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct GoInputs {
    pub a_press: bool,
    pub right_stick: Vec2,
    pub get_going: bool,
}

impl GoInputs {
    pub fn go(&self) -> bool {
        let s = self.right_stick;
        let a = s.y.atan2(s.x).rem_euclid(std::f32::consts::TAU);
        let up = (std::f32::consts::FRAC_PI_4..=3.0 * std::f32::consts::FRAC_PI_4).contains(&a)
            && s.length().min(1.0) >= RSTICK_UP_MAGNITUDE;
        self.a_press || up || self.get_going
    }
}

impl LevelStart {
    pub fn new(world: &World, position: Vec3, orientation: Quat) -> Option<Self> {
        Some(LevelStart {
            phase: Phase::Idle { ticks: 0.0 },
            position,
            orientation: orientation.normalize(),
            offset: offset_level_start(world, position).unwrap_or(position),
            request_transition: false,
            request_stop: false,
            request_skip: false,
            trigger_transition: false,
            trigger_stop: false,
            trigger_skip: false,
            stop_tag: TagTracker::new(&[GO_STOP_WINDOW]),
            intro: None,
        })
    }

    pub fn with_intro(mut self, world: &World, assets: std::sync::Arc<crate::nis::IntroAssets>) -> Option<Self> {
        let intro = crate::nis::IntroState::new(assets, world, self.position, self.orientation)?;
        self.offset = floor_joint(&intro).unwrap_or(self.offset);
        self.intro = Some(intro);
        Some(self)
    }

    pub fn nis_scene_op(&mut self) {
        self.trigger_stop = self.request_stop;
        self.trigger_skip = self.request_skip && !self.request_stop;
        self.trigger_transition = self.request_transition && !self.trigger_skip && !self.request_stop;
        (self.request_transition, self.request_stop, self.request_skip) = (false, false, false);
    }

    pub fn controller_update(&mut self, dt: f32, pick: impl FnOnce() -> usize) {
        if let Some(intro) = self.intro.as_mut() {
            if self.trigger_skip {
                intro.skip();
            }
            if self.trigger_transition || intro.branch_due() {
                intro.transition();
            }
            intro.advance(dt);
            let ticks = intro.ticks();
            self.phase = match intro.root_motion() {
                true => Phase::Go { ticks: intro.sequence_time(ticks), clip: intro.jump_clip() },
                false => Phase::Idle { ticks },
            };
            if intro.attached
                && let Some(f) = floor_joint(intro)
            {
                self.offset = f;
            }
            return;
        }
        if let Phase::Idle { .. } = self.phase
            && self.trigger_transition
        {
            self.phase = Phase::Go { ticks: 0.0, clip: pick() };
            self.stop_tag = TagTracker::new(&[GO_STOP_WINDOW]);
        }
        match &mut self.phase {
            Phase::Idle { ticks } | Phase::Go { ticks, .. } => *ticks += dt,
        }
    }

    pub fn tags(&mut self, inputs: &GoInputs) {
        if let Some(intro) = self.intro.as_mut() {
            self.request_skip = intro.takes_skip() && inputs.a_press;
            match intro.scene {
                crate::nis::Scene::Ready { .. } => self.request_transition = intro.takes_go() && inputs.go(),
                crate::nis::Scene::Jump { .. } => self.request_stop |= intro.stop_request(),
            }
            return;
        }
        match self.phase {
            Phase::Idle { .. } => self.request_transition = inputs.go(),
            Phase::Go { ticks, .. } => {
                if self.stop_tag.update(ticks) {
                    let mut saved = false;
                    for (_, e) in self.stop_tag.process() {
                        GO_STOP_TAG.on(e, &mut self.request_stop, &mut saved);
                    }
                }
            }
        }
    }

    pub fn idle(&self) -> bool {
        matches!(self.phase, Phase::Idle { .. })
    }

    pub fn idle_pose(&self, rm: &RootMotion) -> Option<(Vec3, Quat)> {
        if let Some(intro) = &self.intro {
            // TODO(guess): `VEC_Velocity` of these updates is 0 (the game's while the driver 0x82D02BA8 holds the root is not traced; 0x82DA8590 writes it from the clip's step; the hand-over is unaffected); settle: trace the two against each other.
            crate::guess_hit!();
            let (q, p) = intro.placed_root()?;
            return Some((p, (q * Quat::from_rotation_y(std::f32::consts::FRAC_PI_2)).normalize()));
        }
        let clip = rm.ready_loop.as_ref()?;
        let Phase::Idle { ticks } = self.phase else { return None };
        let (q, t) = clip.sample(ticks.rem_euclid(clip.num_ticks().max(1.0))).ok()?;
        let q_ai = (self.orientation * q).normalize();
        Some((
            self.offset + self.orientation * t,
            (q_ai * Quat::from_rotation_y(std::f32::consts::FRAC_PI_2)).normalize(),
        ))
    }
}

fn floor_joint(intro: &crate::nis::IntroState) -> Option<Vec3> {
    let d = intro.current().actor(ssx_assets::nis::ROLE_CHARACTER1)?.tags.drivers.first()?;
    intro.heli_joint(intro.ticks(), &d.target_joint).map(|f| f.1)
}

pub fn offset_level_start(world: &World, p: Vec3) -> Option<Vec3> {
    let hit = world.collision.ray_cast(p + Vec3::Y * OFFSET_RAY_UP, -Vec3::Y, OFFSET_RAY_UP + OFFSET_RAY_DOWN)?;
    let weights = world.collision.surface_weights(hit.cluster, hit.tri, hit.bary, hit.normal.y);
    let snow = world.tuning.surfaces.blend(&weights, &ChatterInputs::default()).snow_depth;
    Some(p + hit.normal * snow)
}

pub fn go_pick(seed: u32) -> usize {
    let s = seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
    let mut r = 3.0 * f32::from((s >> 16) as u16) * 1.525_902_2e-5;
    (0..3)
        .position(|_| {
            r -= 1.0;
            r <= 0.0
        })
        .unwrap_or(2)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_become_triggers_on_the_next_execution() {
        let mut ls = LevelStart {
            phase: Phase::Idle { ticks: 0.0 },
            position: Vec3::ZERO,
            orientation: Quat::IDENTITY,
            offset: Vec3::ZERO,
            request_transition: false,
            request_stop: false,
            request_skip: false,
            trigger_transition: false,
            trigger_stop: false,
            trigger_skip: false,
            stop_tag: TagTracker::new(&[GO_STOP_WINDOW]),
            intro: None,
        };
        assert!(!GoInputs { right_stick: Vec2::new(0.0, 0.5), ..GoInputs::default() }.go());
        assert!(GoInputs { right_stick: Vec2::new(0.0, 0.6), ..GoInputs::default() }.go());
        ls.nis_scene_op();
        ls.controller_update(2.0, || 1);
        ls.tags(&GoInputs { get_going: true, ..GoInputs::default() });
        assert!(ls.idle() && ls.request_transition && !ls.trigger_transition);
        ls.nis_scene_op();
        ls.controller_update(2.0, || 1);
        assert_eq!(ls.phase, Phase::Go { ticks: 2.0, clip: 1 });
        let mut stop_at = None;
        for i in 0..30 {
            ls.tags(&GoInputs::default());
            if ls.request_stop && stop_at.is_none() {
                stop_at = Some(i);
            }
            ls.nis_scene_op();
            if ls.trigger_stop {
                break;
            }
            ls.controller_update(2.0, || 0);
        }
        assert_eq!(stop_at, Some(19), "the go reaches tick 40 on its 20th update");
        assert!(ls.trigger_stop);
        assert_eq!(go_pick(1_876_900_708), 2, "Feature:RandomValue's first draw: _03");
    }

    #[test]
    fn skip_and_stop_take_precedence_over_the_transition() {
        let mut ls = LevelStart {
            phase: Phase::Idle { ticks: 0.0 },
            position: Vec3::ZERO,
            orientation: Quat::IDENTITY,
            offset: Vec3::ZERO,
            request_transition: true,
            request_stop: false,
            request_skip: true,
            trigger_transition: false,
            trigger_stop: false,
            trigger_skip: false,
            stop_tag: TagTracker::new(&[GO_STOP_WINDOW]),
            intro: None,
        };
        ls.nis_scene_op();
        assert!(ls.trigger_skip && !ls.trigger_transition && !ls.trigger_stop);
        assert!(!ls.request_skip && !ls.request_transition, "the requests are cleared");
        (ls.request_transition, ls.request_skip, ls.request_stop) = (true, true, true);
        ls.nis_scene_op();
        assert!(ls.trigger_stop && !ls.trigger_skip && !ls.trigger_transition);
        ls.request_transition = true;
        ls.nis_scene_op();
        assert!(ls.trigger_transition && !ls.trigger_skip && !ls.trigger_stop);
    }
}
