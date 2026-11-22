#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ScriptCalls {
    pub air_trick_enter: bool,
    pub grab_exit: bool,
    pub grab_reset: bool,
    pub ground_trick_reset: bool,
    pub bail_exit: bool,
}

impl ScriptCalls {
    pub fn any(&self) -> bool {
        self.air_trick_enter || self.grab_exit || self.grab_reset || self.ground_trick_reset || self.bail_exit
    }

    fn add(&mut self, f: Script) {
        match f {
            Script::AirTrickEnter => self.air_trick_enter = true,
            Script::GrabExit => self.grab_exit = true,
            Script::GrabReset => self.grab_reset = true,
            Script::GroundTrickReset => self.ground_trick_reset = true,
            Script::BailExit => self.bail_exit = true,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Script {
    AirTrickEnter,
    GrabExit,
    GrabReset,
    GroundTrickReset,
    BailExit,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ScriptQueue {
    pub now: ScriptCalls,
    pub next: ScriptCalls,
}

impl ScriptQueue {
    pub fn take(&mut self) -> ScriptCalls {
        let now = self.now;
        (self.now, self.next) = (self.next, ScriptCalls::default());
        now
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum NodeScript {
    #[default]
    None,
    OnEnter(Script),
    OnExit(Script),
    OnExitAfterTick1(Script),
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct NodeTags {
    pub script: NodeScript,
    pub updates: u32,
    pub clock: f32,
    pub updated: bool,
    pub window_left: bool,
}

impl NodeTags {
    pub fn new(script: NodeScript) -> Self {
        Self { script, ..Self::default() }
    }

    pub fn update(&mut self, dt: f32, now: &mut ScriptCalls) {
        match self.script {
            NodeScript::OnEnter(f) if self.updates == 0 => now.add(f),
            NodeScript::OnExitAfterTick1(f) if self.updates > 0 && !self.window_left && self.clock > 1.0 => {
                self.window_left = true;
                now.add(f);
            }
            _ => {}
        }
        self.updates += 1;
        self.clock += dt;
        self.updated = true;
    }

    pub fn exit(&mut self, q: &mut ScriptQueue) {
        let calls = if self.updated { &mut q.next } else { &mut q.now };
        match self.script {
            NodeScript::OnExit(f) if self.updates > 0 => calls.add(f),
            NodeScript::OnExitAfterTick1(f) if self.updates > 0 && !self.window_left => {
                self.window_left = true;
                calls.add(f);
            }
            _ => {}
        }
        self.updates = 0;
    }

    pub fn begin_step(&mut self) {
        self.updated = false;
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LeftFlow {
    AirTricks,
    Jump,
    RideNested,
}

impl LeftFlow {
    pub fn name(self) -> &'static str {
        match self {
            Self::AirTricks => "SF_AirTricks",
            Self::Jump => "SF_Jump",
            Self::RideNested => "SF_Ride_Nested",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LeftSet {
    pub tags: NodeTags,
    pub flow: LeftFlow,
    pub serial: Option<u64>,
    pub captured: bool,
    pub first_update_trigger: Option<bool>,
}

impl LeftSet {
    pub fn new(tags: NodeTags, flow: LeftFlow) -> Self {
        Self { tags, flow, serial: None, captured: false, first_update_trigger: None }
    }
}
