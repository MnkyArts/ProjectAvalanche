use super::*;

pub struct Animator {
    pub graph: Graph,
    clips: Vec<RigClip>,
    clip_index: HashMap<ssx_formats::ant::Guid, usize>,
    pub bind: RigPose,
    pub rig: crate::anim_pose::PoseRig,
    root: Box<dyn Controller>,
    pub root_id: CtrlId,
    pub gs: GameState,
    pub problems: Vec<String>,
    physics: Option<StepPhysics>,
}

impl Animator {
    pub fn new(
        db: &crate::anim_db::AssetDb,
        root: &str,
        rig: &ssx_formats::crsf::SceneGraph,
        bind: Vec<crate::character::Srt>,
        floats: &[String],
    ) -> anyhow::Result<Self> {
        let (graph, root_id) = Graph::load(db, root)?;
        let (clips, clip_index, problems) = crate::rig_anim::load_clips_db(db, rig, &graph.clip_guids())?;
        let (_, all) = db
            .streams
            .iter()
            .find(|(n, _)| n == crate::rig_anim::RIG_BUNDLE)
            .ok_or_else(|| anyhow::anyhow!("anim_all.cba not loaded"))?;
        let dofs = ssx_formats::ant_anim::dof_list(all, "genchar_rig", "AnimRig")?;
        let pose_rig = crate::anim_pose::PoseRig::new(rig, &dofs, floats);
        let bind = RigPose { nodes: bind, floats: vec![0.0; floats.len()] };
        let gs = GameState::new(&graph);
        let root = {
            let env =
                Env { g: &graph, clips: &clips, clip_index: &clip_index, bind: &bind, rig: &pose_rig, physics: None };
            instantiate(&env, &gs, root_id)
        };
        Ok(Self { graph, clips, clip_index, bind, rig: pose_rig, root, root_id, gs, problems, physics: None })
    }

    pub(super) fn env(&self) -> Env<'_> {
        Env {
            g: &self.graph,
            clips: &self.clips,
            clip_index: &self.clip_index,
            bind: &self.bind,
            rig: &self.rig,
            physics: self.physics.as_ref(),
        }
    }

    pub fn set_physics(&mut self, physics: Option<StepPhysics>) {
        self.physics = physics;
    }

    pub fn delta(&self) -> Delta {
        let env = self.env();
        self.root.delta(&env, &self.gs)
    }

    pub fn clip_count(&self) -> usize {
        self.clips.len()
    }

    pub fn package_clips(&self) -> Vec<(&str, &str)> {
        self.clips.iter().filter_map(|c| c.package().map(|m| (c.name.as_str(), m.source.as_str()))).collect()
    }

    pub fn reset(&mut self) {
        self.reset_with_random(RandomState(std::sync::atomic::AtomicU32::new(self.graph.random_seed.unwrap_or(0))));
    }

    pub fn reset_with_random(&mut self, random: RandomState) {
        self.gs = GameState::new(&self.graph);
        self.gs.random = random;
        let env = self.env();
        let root = instantiate(&env, &self.gs, self.root_id);
        self.root = root;
    }

    pub fn set_time(&mut self, t: f32) {
        let env = Env {
            g: &self.graph,
            clips: &self.clips,
            clip_index: &self.clip_index,
            bind: &self.bind,
            rig: &self.rig,
            physics: self.physics.as_ref(),
        };
        self.root.set_time(&env, &self.gs, t);
    }

    pub fn take_audio_tags(&mut self) -> Vec<AudioTagEvent> {
        std::mem::take(&mut self.gs.audio_tags)
    }

    pub fn clear_audio_tags(&mut self) {
        self.gs.audio_tags.clear();
    }

    pub fn update(&mut self, dt: f32) {
        let env = Env {
            g: &self.graph,
            clips: &self.clips,
            clip_index: &self.clip_index,
            bind: &self.bind,
            rig: &self.rig,
            physics: self.physics.as_ref(),
        };
        self.root.update(&env, &mut self.gs, dt);
    }

    pub fn anim_pose(&self) -> AnimPose {
        let env = self.env();
        let mut out = AnimPose::empty(&self.bind);
        self.root.pose(&env, &self.gs, &mut out);
        out.fill_invalid(&self.bind);
        out
    }

    pub fn pose(&self) -> RigPose {
        self.anim_pose().pose
    }

    pub fn playing(&self) -> Vec<(String, f32)> {
        let env = self.env();
        let mut out = Vec::new();
        self.root.label(&env, 1.0, &mut out);
        out.retain(|x| x.1 > 0.01);
        out.sort_by(|a, b| b.1.total_cmp(&a.1));
        out
    }

    pub fn path(&self) -> Vec<String> {
        self.path_times().into_iter().map(|n| n.0).collect()
    }

    pub fn path_times(&self) -> Vec<(String, f32)> {
        let env = self.env();
        let mut out = Vec::new();
        self.root.path(&env, &mut out);
        out
    }

    pub fn running_nodes(&self, dt: f32) -> Vec<String> {
        let env = self.env();
        self.root.as_flow().map_or_else(Vec::new, |f| f.running_nodes(&env, dt))
    }

    pub fn set(&mut self, name: &str, v: f32) {
        if let Some(i) = self.graph.signal_id(name) {
            self.gs.set(i, v);
        }
    }
}
