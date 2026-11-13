use super::*;

#[derive(Clone, Copy, Debug, Default)]
pub struct StepStart {
    pub velocity: Vec3,
    pub wipeout: bool,
    pub raw_stick: Vec2,
}

pub struct LevelStartGraph {
    pub(super) animator: Animator,
    pub(super) blend: ssx_assets::anim_graph::Blend,
    pub(super) running: bool,
    pub(super) elapsed: Option<f32>,
    pub(super) intro: Option<IntroGraph>,
}

pub struct IntroGraph {
    pub(super) cabin: Vec<Animator>,
    pub(super) ready: Vec<Animator>,
    pub(super) jump: Vec<Animator>,
    shown: Vec<(SideKey, f32)>,
    blends: Vec<(ssx_assets::anim_graph::Blend, f32)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SideKey {
    Cabin(usize),
    Ready(usize),
    Jump(usize),
}

impl IntroGraph {
    pub fn load(
        big_root: &Path,
        nis: &ssx_assets::nis::IntroNis,
        rig: &SceneGraph,
        bind: Vec<Srt>,
        floats: &[String],
    ) -> Result<Self> {
        let dir = big_root.join(ssx_assets::rig_anim::ANIM_DIR);
        let db = ssx_assets::anim_db::AssetDb::load(&dir, &nis.bundles.iter().map(String::as_str).collect::<Vec<_>>())?;
        let new = |root: &str| Animator::new(&db, root, rig, bind.clone(), floats);
        let cabin = nis.cabin.iter().map(|c| new(&c.scene.sequence)).collect::<Result<Vec<_>>>()?;
        let rider = |sc: &ssx_assets::nis::NisScene| sc.actor(ssx_assets::nis::ROLE_CHARACTER1).cloned();
        let ready = match rider(&nis.ready).filter(|a| a.choices.len() > 1 && a.sequences.len() == a.choices.len()) {
            Some(a) => a.sequences.iter().map(|(seq, _)| new(seq)).collect::<Result<Vec<_>>>()?,
            None => vec![new(&nis.ready.sequence)?],
        };
        let jump = match rider(&nis.jump).filter(|a| a.choices.len() > 1) {
            Some(a) => a.choices.iter().map(|(clip, _)| new(&clip.name)).collect::<Result<Vec<_>>>()?,
            None => vec![new(&nis.jump.sequence)?],
        };
        Ok(Self { cabin, ready, jump, shown: Vec::new(), blends: Vec::new() })
    }

    fn key(&self, s: &crate::nis::Side) -> SideKey {
        match s.scene {
            crate::nis::SceneRef::Cabin(i) => SideKey::Cabin(i),
            crate::nis::SceneRef::Ready => SideKey::Ready(if self.ready.len() > 1 { s.rider_pick } else { 0 }),
            crate::nis::SceneRef::Jump => SideKey::Jump(if self.jump.len() > 1 { s.rider_pick } else { 0 }),
        }
    }

    fn get(&self, k: SideKey) -> Option<&Animator> {
        match k {
            SideKey::Cabin(i) => self.cabin.get(i),
            SideKey::Ready(i) => self.ready.get(i),
            SideKey::Jump(i) => self.jump.get(i),
        }
    }

    fn get_mut(&mut self, k: SideKey) -> Option<&mut Animator> {
        match k {
            SideKey::Cabin(i) => self.cabin.get_mut(i),
            SideKey::Ready(i) => self.ready.get_mut(i),
            SideKey::Jump(i) => self.jump.get_mut(i),
        }
    }

    pub(super) fn animators_mut(&mut self) -> impl Iterator<Item = &mut Animator> {
        self.cabin.iter_mut().chain(self.ready.iter_mut()).chain(self.jump.iter_mut())
    }

    pub(super) fn current(&self) -> &Animator {
        self.shown.first().and_then(|&(k, _)| self.get(k)).unwrap_or(&self.ready[0])
    }

    pub(super) fn reset(&mut self, random: &ssx_assets::anim_eval::RandomState) {
        for a in self.animators_mut() {
            a.reset_with_random(random.clone());
        }
        (self.shown, self.blends) = (Vec::new(), Vec::new());
    }

    pub(super) fn update(&mut self, state: &crate::nis::IntroState, dt: f32) {
        let mut sides = vec![state.side()];
        sides.extend(state.fades().iter().map(|f| f.from));
        let keys: Vec<(SideKey, f32)> = sides.iter().map(|s| (self.key(s), s.ticks)).collect();
        for (&(k, ticks), s) in keys.iter().zip(&sides) {
            let entered = !self.shown.iter().any(|&(o, t)| o == k && t <= ticks);
            let random = self.current().gs.random.clone();
            let start = state.side_time(&crate::nis::Side { ticks: 0.0, ..*s });
            let Some(a) = self.get_mut(k) else { continue };
            if entered {
                a.reset_with_random(random);
                a.set_time(start);
                a.update(s.ticks);
            } else {
                a.update(dt);
            }
        }
        self.shown = keys;
        self.blends = state.fades().iter().map(|f| (f.blend.clone(), f.weight())).collect();
    }

    pub(super) fn advance(&mut self, dt: f32) {
        for (k, _) in self.shown.clone() {
            if let Some(a) = self.get_mut(k) {
                a.update(dt);
            }
        }
    }

    pub(super) fn anim_pose(&self) -> ssx_assets::anim_pose::AnimPose {
        let mut acc: Option<ssx_assets::anim_pose::AnimPose> = None;
        for i in (0..self.shown.len()).rev() {
            let Some(a) = self.get(self.shown[i].0) else { continue };
            let new = a.anim_pose();
            acc = Some(match (acc, self.blends.get(i)) {
                (Some(mut old), Some((b, w))) => {
                    old.blend(&new, *w);
                    if b.class == "AccelBlendAsset"
                        && let Some(t) = a.rig.trajectory
                    {
                        old.pose.nodes[t] = new.pose.nodes[t];
                        old.valid_nodes[t] = new.valid_nodes[t];
                    }
                    old
                }
                _ => new,
            });
        }
        acc.unwrap_or_else(|| self.ready[0].anim_pose())
    }

    pub(super) fn delta(&self) -> Delta {
        let mut acc: Option<Delta> = None;
        for i in (0..self.shown.len()).rev() {
            let Some(a) = self.get(self.shown[i].0) else { continue };
            let new = a.delta();
            acc = Some(match (acc, self.blends.get(i)) {
                (Some(_), Some((b, _))) if b.class == "AccelBlendAsset" => new,
                (Some(old), Some((_, w))) => Delta::blend(old, new, *w, None),
                _ => new,
            });
        }
        acc.unwrap_or(Delta::None)
    }
}

impl LevelStartGraph {
    pub(super) fn current(&self) -> &Animator {
        match &self.intro {
            Some(i) => i.current(),
            None => &self.animator,
        }
    }

    pub(super) fn anim_pose(&self) -> ssx_assets::anim_pose::AnimPose {
        match &self.intro {
            Some(i) => i.anim_pose(),
            None => self.animator.anim_pose(),
        }
    }
}

impl LevelStartGraph {
    pub fn load(
        db: &ssx_assets::anim_db::AssetDb,
        rig: &SceneGraph,
        bind: Vec<Srt>,
        floats: &[String],
    ) -> Result<Self> {
        let animator = Animator::new(db, "SF_default_level_start", rig, bind, floats)?;
        let a = db.by_name("accel_medium").context("no accel_medium blend")?;
        anyhow::ensure!(a.class() == "AccelBlendAsset", "accel_medium is a {}", a.class());
        let blend = ssx_assets::anim_graph::Blend {
            class: a.class().to_string(),
            blend_time: a.f32("BlendTime").context("accel_medium has no BlendTime")?,
            launch_angle: 0.0,
            curve: -1,
            sync_phase: false,
            weight: None,
            samples: Vec::new(),
            dof_curves: None,
        };
        Ok(Self { animator, blend, running: false, elapsed: None, intro: None })
    }
}
