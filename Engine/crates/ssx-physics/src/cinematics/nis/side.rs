use ssx_assets::anim_graph::Blend;
use ssx_assets::nis::{ElementClock, NisActor, NisClip, NisPiece, NisScene, ROLE_CAMERA1, ROLE_CHARACTER1};

use super::{IntroState, Scene};

#[derive(Clone, Copy, Debug)]
pub struct Element<'a> {
    pub clip: &'a NisClip,
    pub offset: Option<&'a (String, String)>,
    pub clock: ElementClock,
    pub piece: Option<(usize, &'a NisPiece)>,
}

impl Element<'_> {
    pub fn clip_time(&self, t: f32) -> f32 {
        self.clock.clip_time(t)
    }

    pub fn next_begin(&self, a: &NisActor) -> Option<f32> {
        a.pieces.get(self.piece.map_or(0, |(i, _)| i + 1)).map(|p| p.clock.begin)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SceneRef {
    Cabin(usize),
    Ready,
    Jump,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Side {
    pub scene: SceneRef,
    pub ticks: f32,
    pub rider_pick: usize,
    pub camera_pick: usize,
    pub base: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SceneFade {
    pub from: Side,
    pub blend: Blend,
    pub elapsed: f32,
    pub frozen: bool,
}

const MAX_ACTIVE_NODES: usize = 5;

impl SceneFade {
    pub fn weight(&self) -> f32 {
        ssx_assets::anim_eval::blend_new_weight(&self.blend, self.elapsed)
    }

    fn done(&self, dt: f32) -> bool {
        self.blend.blend_time - self.elapsed <= dt || self.weight() >= 0.99
    }
}

pub fn random_pick(state: &mut u32, weights: &[f32]) -> usize {
    *state = state.wrapping_mul(1_103_515_245).wrapping_add(12_345);
    let total: f32 = weights.iter().sum();
    let mut r = total * f32::from((*state >> 16) as u16) * 1.525_902_2e-5;
    weights
        .iter()
        .position(|w| {
            r -= w;
            r <= 0.0
        })
        .unwrap_or(weights.len().saturating_sub(1))
}

impl IntroState {
    pub fn scene_of(&self, r: SceneRef) -> &NisScene {
        match r {
            SceneRef::Cabin(i) => &self.assets.nis.cabin[i].scene,
            SceneRef::Ready => &self.assets.nis.ready,
            SceneRef::Jump => &self.assets.nis.jump,
        }
    }

    pub fn side(&self) -> Side {
        let scene = match (self.cabin, self.scene) {
            (Some(c), _) => SceneRef::Cabin(c.index),
            (None, Scene::Ready { .. }) => SceneRef::Ready,
            (None, Scene::Jump { .. }) => SceneRef::Jump,
        };
        Side {
            scene,
            ticks: self.ticks(),
            rider_pick: self.rider_pick,
            camera_pick: self.camera_pick,
            base: self.node_base,
        }
    }

    pub fn ready_chooser(&self) -> Option<&NisActor> {
        self.assets.nis.ready.actor(ROLE_CHARACTER1).filter(|a| {
            a.choices.len() > 1
                && a.sequences.len() == a.choices.len()
                && a.chooser.as_ref().is_some_and(|c| c.looping && c.choose_on_loop)
        })
    }

    pub fn node_time(&self, s: &Side) -> f32 {
        s.base + s.ticks
    }

    pub fn side_time(&self, s: &Side) -> f32 {
        match s.scene {
            SceneRef::Cabin(i) => self.assets.nis.cabin[i].start + s.ticks,
            SceneRef::Ready if self.ready_chooser().is_some() => s.ticks,
            SceneRef::Ready => self.assets.sequence_time(Scene::Ready { ticks: s.ticks }),
            SceneRef::Jump => self.assets.sequence_time(Scene::Jump { ticks: s.ticks }),
        }
    }

    pub fn side_clip<'a>(&self, s: &Side, a: &'a NisActor) -> &'a NisClip {
        self.side_element(s, a).clip
    }

    pub fn side_element<'a>(&self, s: &Side, a: &'a NisActor) -> Element<'a> {
        self.element_at(s, a, self.side_time(s))
    }

    pub fn element_at<'a>(&self, s: &Side, a: &'a NisActor, t: f32) -> Element<'a> {
        match a.piece_at(t) {
            Some((i, p)) => Element { clip: &p.clip, offset: p.offset.as_ref(), clock: p.clock, piece: Some((i, p)) },
            None => self.first_element(s, a),
        }
    }

    pub fn element_before<'a>(&self, s: &Side, a: &'a NisActor, i: usize) -> Element<'a> {
        match i.checked_sub(1).and_then(|j| Some((j, a.pieces.get(j)?))) {
            Some((j, p)) => Element { clip: &p.clip, offset: p.offset.as_ref(), clock: p.clock, piece: Some((j, p)) },
            None => self.first_element(s, a),
        }
    }

    fn first_element<'a>(&self, s: &Side, a: &'a NisActor) -> Element<'a> {
        let pick = match a.role {
            ROLE_CHARACTER1 if s.scene != SceneRef::Ready || self.ready_chooser().is_some() => Some(s.rider_pick),
            ROLE_CAMERA1 => Some(s.camera_pick),
            _ => None,
        };
        let (clip, clock) = match pick.and_then(|k| a.choices.get(k)) {
            Some((c, _, clock)) if a.choices.len() > 1 => (c, *clock),
            _ => (&a.clip, a.clock),
        };
        Element { clip, offset: a.offset.as_ref(), clock, piece: None }
    }

    pub fn chain(&self) -> Vec<(SceneRef, f32, f32, bool)> {
        let mut sides = vec![self.side()];
        sides.extend(self.fades.iter().map(|f| f.from));
        sides
            .iter()
            .enumerate()
            .map(|(i, s)| {
                let f = self.fades.get(i);
                let w = f.map_or(1.0, SceneFade::weight);
                (s.scene, self.side_time(s), w, f.is_some_and(|f| f.blend.class == "AccelBlendAsset"))
            })
            .collect()
    }

    pub fn fades(&self) -> &[SceneFade] {
        &self.fades
    }

    pub(super) fn update_fades(&mut self, dt: f32) {
        for i in 0..self.fades.len() {
            if self.fades[i].done(dt) {
                self.fades.truncate(i);
                return;
            }
            let f = &mut self.fades[i];
            f.elapsed = (f.elapsed + dt).min(f.blend.blend_time.max(0.0));
            if !f.frozen {
                f.from.ticks += dt;
            }
        }
    }

    pub(super) fn push_fade(&mut self, blend: &Blend) {
        if blend.blend_time <= 0.0 {
            self.fades.clear();
            return;
        }
        let from = self.side();
        self.fades.insert(0, SceneFade { from, blend: blend.clone(), elapsed: 0.0, frozen: false });
        if let Some(f) = self.fades.get_mut(MAX_ACTIVE_NODES - 1) {
            f.frozen = true;
        }
        self.fades.truncate(MAX_ACTIVE_NODES - 1);
    }

    pub(super) fn ready_rechoose(&mut self, rest: f32) {
        let Some(a) = self.ready_chooser() else { return };
        let weights: Vec<f32> = a.choices.iter().map(|c| c.1).collect();
        let blend = a.chooser.as_ref().and_then(|c| c.blend.clone());
        let k = random_pick(&mut self.rider_random, &weights);
        if k != self.rider_pick
            && let Some(b) = &blend
        {
            self.push_fade(b);
        }
        self.rider_pick = k;
        self.scene = Scene::Ready { ticks: rest };
    }

    pub(super) fn ready_pick_length(&self) -> Option<f32> {
        self.ready_chooser().and_then(|a| a.sequences.get(self.rider_pick)).map(|s| s.1)
    }

    pub(super) fn go_pick(&mut self) {
        let Some(a) = self.assets.nis.jump.actor(ROLE_CHARACTER1).filter(|a| a.choices.len() > 1) else { return };
        let weights: Vec<f32> = a.choices.iter().map(|c| c.1).collect();
        self.rider_pick = random_pick(&mut self.rider_random, &weights);
    }

    pub fn go_span(&self, dt: f32) -> (f32, f32) {
        let side = self.side();
        let t = self.side_time(&side);
        match self.assets.nis.jump.actor(ROLE_CHARACTER1) {
            Some(a) if side.scene == SceneRef::Jump => {
                let e = self.element_at(&side, a, t);
                (e.clip_time((t - dt).max(e.clock.begin)), e.clip_time(t))
            }
            _ => ((t - dt).max(0.0), t),
        }
    }

    pub fn jump_clip(&self) -> usize {
        let rider = self.assets.nis.jump.actor(ROLE_CHARACTER1);
        let side = Some(self.side()).filter(|s| s.scene == SceneRef::Jump);
        if let Some((i, _)) = rider.zip(side).and_then(|(a, s)| self.side_element(&s, a).piece) {
            return self.assets.piece_clips.get(i).copied().unwrap_or(usize::MAX);
        }
        let pick = if rider.is_some_and(|a| a.choices.len() > 1) { self.rider_pick } else { 0 };
        self.assets.jump_clips.get(pick).copied().unwrap_or(self.assets.jump_clip)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_snow_choosers_draw_in_turn_from_the_seed() {
        let mut s = 1_876_900_708u32;
        let ready = [0.15, 0.15, 0.15, 1.0];
        let picks: Vec<usize> = (0..4).map(|_| random_pick(&mut s, &ready)).collect();
        assert_eq!(picks, [3, 3, 3, 3]);
        let go: Vec<usize> = (1..=4)
            .map(|k| {
                let mut s = 1_876_900_708u32;
                (0..k).for_each(|_| _ = random_pick(&mut s, &ready));
                random_pick(&mut s, &[1.0, 1.0, 1.0])
            })
            .collect();
        assert_eq!(go, [1, 2, 1, 1]);
    }

    #[test]
    fn a_fade_ends_when_its_rest_fits_the_step() {
        let f = |elapsed| SceneFade {
            from: Side { scene: SceneRef::Ready, ticks: 0.0, rider_pick: 0, camera_pick: 0, base: 0.0 },
            blend: Blend::default_transition(),
            elapsed,
            frozen: false,
        };
        assert!(!f(0.0).done(2.0) && !f(4.0).done(2.0));
        assert!(f(6.0).done(2.0));
        assert!(f(0.0).weight() < 1e-6 && (f(8.0).weight() - 1.0).abs() < 1e-6);
    }
}
