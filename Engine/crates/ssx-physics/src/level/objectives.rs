use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

pub use ssx_lua::game::objectives::binder::{BatchReport, load_batch};
pub use ssx_lua::game::objectives::{
    DropMessage, Effect, GsValue, LogEntry, Logged, Name, NisRequest, Runner, SetId, World as ObjectiveWorld,
    game_state_name,
};

pub const LEVEL_SCRIPTS: [&str; 4] = [
    "default_track_objectives.lua",
    "default_nis_objectives.lua",
    "stats\\trackbonuses.lua",
    "global_tutorial_objectives.lua",
];

pub const GAME_SCRIPTS: [&str; 2] = ["default_global_objectives.lua", "stats\\awards.lua"];

pub const STATE_SETS_SCRIPT: &str = "world_entity_controllers_default.lua";
pub const SCHEMAS_SCRIPT: &str = "world_entity_schemas_default.lua";
pub const BADGES_SCRIPT: &str = "init_badges.lua";
pub const SCENE_SCRIPT: &str = "default_scene.lua";

#[derive(Clone, Default)]
pub struct DropScripts {
    pub list: Vec<String>,
    pub scene: Vec<String>,
    pub sources: BTreeMap<String, Arc<[u8]>>,
    pub root: Option<std::path::PathBuf>,
}

impl PartialEq for DropScripts {
    fn eq(&self, o: &Self) -> bool {
        self.list == o.list && self.scene == o.scene && self.sources.keys().eq(o.sources.keys())
    }
}

impl std::fmt::Debug for DropScripts {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let keys = self.sources.keys();
        f.debug_struct("DropScripts")
            .field("list", &self.list)
            .field("scene", &self.scene)
            .field("sources", &keys)
            .finish()
    }
}

impl DropScripts {
    pub fn load(big_root: &Path, drop: Vec<String>) -> Self {
        let mut d =
            DropScripts { list: drop, scene: Vec::new(), sources: BTreeMap::new(), root: Some(big_root.to_path_buf()) };
        let game = [STATE_SETS_SCRIPT, SCHEMAS_SCRIPT, BADGES_SCRIPT, SCENE_SCRIPT].into_iter().chain(GAME_SCRIPTS);
        for n in LEVEL_SCRIPTS.into_iter().chain(game).map(str::to_string).chain(d.list.clone()) {
            d.add(big_root, &n);
        }
        d
    }

    pub fn add_scene(&mut self, big_root: &Path, scene: Vec<String>) {
        for n in &scene {
            self.add(big_root, n);
        }
        self.scene = scene;
    }

    pub fn named(&self, names: &[&str]) -> Vec<(String, Vec<u8>)> {
        names.iter().filter_map(|n| Some((n.to_string(), self.sources.get(*n)?.to_vec()))).collect()
    }

    pub fn scene_batch(&self, event: Option<&DropScripts>) -> Vec<(String, Vec<u8>)> {
        let names = std::iter::once(SCENE_SCRIPT.to_string())
            .chain(self.scene.iter().cloned())
            .chain(event.map_or_else(Vec::new, |e| e.scene.clone()));
        names
            .filter_map(|n| {
                let src = self.sources.get(&n).or_else(|| event.and_then(|e| e.sources.get(&n)))?;
                Some((n, src.to_vec()))
            })
            .collect()
    }

    pub fn of(big_root: &Path, scripts: &[String]) -> Self {
        let mut d = DropScripts {
            list: scripts.to_vec(),
            scene: Vec::new(),
            sources: BTreeMap::new(),
            root: Some(big_root.to_path_buf()),
        };
        for s in scripts {
            d.add(big_root, s);
        }
        d
    }

    fn add(&mut self, big_root: &Path, name: &str) {
        if self.sources.contains_key(name) {
            return;
        }
        let rel = name.replace('\\', "/");
        let path = ssx_assets::data_roots::find(big_root, &format!("data/common/scripts/{rel}"))
            .unwrap_or_else(|| big_root.join(crate::spawn::SCRIPTS_DIR).join(&rel));
        if let Ok(src) = std::fs::read(path) {
            self.sources.insert(name.to_string(), Arc::from(src));
        }
    }

    pub fn level_batch(&self, event: Option<&DropScripts>) -> Vec<(String, Vec<u8>)> {
        let names = LEVEL_SCRIPTS.iter().map(|s| s.to_string()).chain(self.list.iter().cloned());
        let event_names = event.map_or_else(Vec::new, |e| e.list.clone());
        names
            .chain(event_names)
            .filter_map(|n| {
                let src = self.sources.get(&n).or_else(|| event.and_then(|e| e.sources.get(&n)))?;
                Some((n, src.to_vec()))
            })
            .collect()
    }

    pub fn batch(&self) -> Vec<(String, Vec<u8>)> {
        self.list.iter().filter_map(|n| Some((n.clone(), self.sources.get(n)?.to_vec()))).collect()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct EventObjectives {
    pub scripts: DropScripts,
    pub probe: Probe,
}

impl EventObjectives {
    pub fn new(big_root: &Path, drop: &DropScripts, def: &crate::event::EventDef) -> Self {
        let mut scripts = DropScripts::of(big_root, &def.scripts);
        scripts.add_scene(big_root, def.avalanche_scripts.clone());
        let slots = crate::character::GearStats::default_gear().slots;
        let probe = Probe::run(&drop.level_batch(Some(&scripts)), def.event_type.map(|t| t as i32), slots);
        EventObjectives { scripts, probe }
    }
}

pub const EVENT_TYPE_SCRIPT: &str = "common/track_environment/drop_event_type.lua";

impl EventObjectives {
    pub fn for_map(
        big_root: &Path,
        drop: Option<&DropScripts>,
        event_type: crate::event::EventType,
        list: &[String],
    ) -> Self {
        let scripts = DropScripts::of(big_root, list);
        let slots = crate::character::GearStats::default_gear().slots;
        let batch = drop.map_or_else(|| scripts.batch(), |d| d.level_batch(Some(&scripts)));
        let probe = Probe::run(&batch, Some(event_type as i32), slots);
        EventObjectives { scripts, probe }
    }
}

#[derive(Clone, Debug, Default)]
pub struct LoadWorld {
    pub slots: [bool; 15],
    pub event_type: Option<i32>,
    pub message: DropMessage,
    pub effects: Vec<(String, Effect)>,
}

impl ObjectiveWorld for LoadWorld {
    fn equipped(&mut self, item: i32, _equipment_type: i32) -> Option<bool> {
        usize::try_from(item).ok().and_then(|i| self.slots.get(i)).copied()
    }
    fn event_type(&mut self) -> Option<i32> {
        self.event_type
    }
    fn drop_message(&mut self) -> Option<DropMessage> {
        Some(self.message.clone())
    }
    fn effect(&mut self, objective: &Name, e: &Effect) {
        self.effects.push((objective.text.to_string(), e.clone()));
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Probe {
    pub writes: Vec<(String, u16, GsValue)>,
    pub effects: Vec<(String, Effect)>,
    pub flags: Vec<String>,
    pub scripts: BTreeMap<String, String>,
    pub problems: Vec<String>,
}

impl Probe {
    pub fn run(batch: &[(String, Vec<u8>)], event_type: Option<i32>, slots: [bool; 15]) -> Self {
        let runner = Runner { level_active: true, ..Runner::default() };
        let load = LoadWorld { slots, event_type, ..LoadWorld::default() };
        let (mut runner, mut lw, report) = load_batch(runner, SetId::Level, batch, load, None);
        // TODO(guess): the first update's dt is one simulation step (no load-time consumer depends on it); settle: the frame time of the level's first module update.
        crate::guess_hit!();
        runner.update(1.0 / 30.0, &mut lw);
        let writes = lw
            .effects
            .iter()
            .filter_map(|(_, e)| match e {
                Effect::SetGameState { entity, id, value } => Some((entity.text.to_string(), *id, *value)),
                _ => None,
            })
            .collect();
        let mut problems: Vec<String> =
            report.scripts.iter().filter_map(|(n, e)| e.as_ref().map(|e| format!("{n}: {e}"))).collect();
        problems.extend(report.problems.iter().cloned());
        let scripts =
            runner.sets[1].iter().map(|o| (o.name.text.to_string(), o.script.clone())).collect::<BTreeMap<_, _>>();
        Probe { writes, effects: lw.effects, flags: runner.flags.names(), scripts, problems }
    }

    pub fn value(&self, entity: &str, name: &str) -> Option<GsValue> {
        self.writes.iter().rev().find(|(e, id, _)| e == entity && game_state_name(*id) == name).map(|w| w.2)
    }

    pub fn set_true(&self, entity: &str, name: &str) -> bool {
        self.value(entity, name) == Some(GsValue::Bool(true))
    }

    pub fn flag(&self, name: &str) -> bool {
        self.flags.iter().any(|f| f == name)
    }

    pub fn first_nis(&self) -> Option<(&str, &NisRequest)> {
        self.effects.iter().find_map(|(o, e)| match e {
            Effect::PlayNis(r) => Some((o.as_str(), r)),
            _ => None,
        })
    }

    pub fn before_first_nis(&self) -> &[(String, Effect)] {
        let end = self.effects.iter().position(|(_, e)| matches!(e, Effect::PlayNis(_))).unwrap_or(self.effects.len());
        &self.effects[..end]
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct HeliOrders {
    pub move_to: Option<(glam::Vec3, glam::Quat)>,
    pub warp: Option<(glam::Vec3, glam::Quat)>,
    pub ends_with_heli_jump: bool,
    pub reverse_cam: bool,
    pub generation: u32,
}

#[derive(Clone, Debug, Default)]
pub struct LevelScripts {
    pub state_sets: ssx_lua::game::graph::Graph,
    pub schemas: ssx_lua::game::scene::Schemas,
    pub badges: ssx_lua::game::graph::Graph,
    pub scene: ssx_lua::game::scene::Scene,
}

impl LevelScripts {
    // TODO(guess): the game's start runs once per session (once per boot in the game; the state sets and schemas are read only); settle: a game-wide owner of the batches across levels (the game shell's).
    pub fn load(drop: &DropScripts, event: Option<&DropScripts>) -> Self {
        crate::guess_hit!();
        use ssx_lua::game::Binder;
        use ssx_lua::game::graph::run_batch;
        use ssx_lua::game::scene::{Scene, Schemas};
        LevelScripts {
            state_sets: run_batch(Binder::StateSets, &drop.named(&[STATE_SETS_SCRIPT]), None),
            schemas: Schemas::from_graph(&run_batch(Binder::Schemas, &drop.named(&[SCHEMAS_SCRIPT]), None)),
            badges: run_batch(Binder::Badges, &drop.named(&[BADGES_SCRIPT]), None),
            scene: Scene::from_graph(&run_batch(Binder::Scene, &drop.scene_batch(event), None)),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct LevelObjectives {
    pub runner: Runner,
    pub report: BatchReport,
    pub overlay: BTreeMap<(u32, u16), GsValue>,
    pub effects: Vec<(String, Effect)>,
    pub heli: HeliOrders,
    pub nis_starts: BTreeMap<(String, String), Option<(glam::Vec3, glam::Quat)>>,
    pub unapplied: std::collections::BTreeSet<String>,
    pub postfx: BTreeMap<String, (f32, f32)>,
}

impl LevelObjectives {
    pub fn load(
        game: &[(String, Vec<u8>)],
        batch: &[(String, Vec<u8>)],
        load: LoadWorld,
        keep_log: bool,
        root: Option<&Path>,
        permanent_flags: &[u32],
    ) -> Self {
        let mut runner = Runner { level_active: true, log: keep_log.then(Vec::new), ..Runner::default() };
        for &hash in permanent_flags {
            runner.flags.set_permanent_hash(hash, true);
        }
        let (runner, load, mut game_report) = load_batch(runner, SetId::Game, game, load, None);
        let (runner, lw, mut report) = load_batch(runner, SetId::Level, batch, load, None);
        game_report.scripts.append(&mut report.scripts);
        report.scripts = game_report.scripts;
        report.problems.append(&mut game_report.problems);
        let mut nis_starts = BTreeMap::new();
        if let Some(root) = root {
            let mut wanted = Vec::new();
            for o in &runner.sets[1] {
                o.nodes.iter().for_each(|n| nis_moves(n, &mut wanted));
            }
            for (nis, role) in wanted {
                let start = nis_role_id(&role)
                    .and_then(|r| ssx_assets::nis::binding_item_start(root, &nis, r).ok().flatten())
                    .map(|(p, q)| (glam::Vec3::from_array(p), glam::Quat::from_array(q)));
                nis_starts.insert((nis, role), start);
            }
        }
        LevelObjectives {
            runner,
            report,
            overlay: BTreeMap::new(),
            effects: lw.effects,
            heli: HeliOrders::default(),
            nis_starts,
            unapplied: Default::default(),
            postfx: BTreeMap::new(),
        }
    }
}

fn nis_moves(n: &ssx_lua::game::objectives::Node, out: &mut Vec<(String, String)>) {
    use ssx_lua::game::objectives::{Do, Node};
    match n {
        Node::Do(Do::Effect(Effect::MoveToNisStartLocation { nis, role, .. })) => {
            out.push((nis.clone(), role.text.to_string()));
        }
        Node::And { a, b, .. } | Node::Or { a, b, .. } => {
            nis_moves(a, out);
            b.iter().for_each(|b| nis_moves(b, out));
        }
        Node::Not(Some(c)) => nis_moves(c, out),
        _ => {}
    }
}

// TODO(guess): the role enumeration's names are taken as the roles' without the digit (`Character` → `NIS_Role_Character1` 0, `Camera1` 10, `Helicopter1` 20; `FUN_82C745A0` compares exactly via 0x82FF0138, loader 0x82D5F5E0, 0x831AE2F0; the image holds no `NIS_Role` string); settle: the name the ANT loader hands 0x82D5F5E0.
fn nis_role_id(role: &str) -> Option<i64> {
    use ssx_assets::nis::{ROLE_CAMERA1, ROLE_CHARACTER1, ROLE_HELICOPTER1};
    match role {
        "Character" => Some(ROLE_CHARACTER1),
        "Camera" => Some(ROLE_CAMERA1),
        "Helicopter" => Some(ROLE_HELICOPTER1),
        _ => None,
    }
}

pub struct SessionWorld<'a> {
    pub s: &'a mut crate::rider::RiderState,
    pub w: &'a crate::rider::World,
    pub tracker: &'a crate::triggers::TriggerTracker,
    pub event: &'a mut crate::event::EventManager,
    pub rewind: &'a mut crate::rewind::RewindCount,
    pub nis_active: bool,
    pub heli_contact: Option<bool>,
    pub overlay: &'a mut BTreeMap<(u32, u16), GsValue>,
    pub effects: &'a mut Vec<(String, Effect)>,
    pub heli: &'a mut HeliOrders,
    pub nis_starts: &'a BTreeMap<(String, String), Option<(glam::Vec3, glam::Quat)>>,
    pub intercept: Option<&'a mut crate::session::heli_jump::Intercept>,
    pub tutorial: Option<&'a mut crate::session::tutorial::Tutorial>,
    pub unapplied: &'a mut std::collections::BTreeSet<String>,
    pub nis: &'a crate::nis_events::NisEvents,
    pub postfx: &'a mut BTreeMap<String, (f32, f32)>,
    pub backwards: bool,
    pub stats: &'a mut crate::stats::StatsStore,
    pub ui_flags: &'a BTreeMap<u16, bool>,
}

pub const UI_HUD_READY: u16 = 0x124;
pub const UI_HUD_RACE_STARTED: u16 = 0x121;

fn player_value(x: &SessionWorld, name: &str) -> Option<GsValue> {
    use GsValue::{Bool, Float, Int};
    let s = &*x.s;
    let intro = s.start.as_ref().and_then(|l| l.intro.as_ref());
    Some(match name {
        "FLT_Descent" => Float(s.score.descent),
        "BLN_IsAwesome" => Bool(s.score.is_awesome),
        "BLN_IsSuperAwesome" => Bool(s.score.is_super_awesome),
        "BLN_IsTrackFinished" => Bool(x.event.track_finished),
        "INT_RewindCountCurrent" => Int(x.rewind.rewinds_left),
        "BLN_NisActive" => Bool(x.nis_active),
        "BLN_InAir" => Bool(!s.on_ground),
        "BLN_IsNisReadyLoop" => Bool(intro.is_some_and(|i| i.in_ready_loop())),
        "BLN_PlayerHeliContact" => Bool(x.heli_contact?),
        "FLT_OxygenTankLevel" => Float(s.survival.oxygen_tank),
        "FLT_NumTicksToLanding" => Float(s.ballistic.ticks),
        "BLN_NisOutroActive" => Bool(intro.is_some_and(|i| i.outro_active())),
        "BLN_NisAllowOutro" => Bool(x.event.allow_outro),
        "INT_ScoringTrickId" => Int(i32::from(s.score.trick_id.value())),
        "FLT_VignetteOxygen" => Float(s.survival.vignette_oxygen),
        "FLT_SurfaceWeightRock" => Float(s.weights[crate::surface::ROCK]),
        "FLT_SurfaceWeightIce" => Float(s.weights[crate::surface::ICE]),
        "FLT_Speed" => Float(s.player_speed),
        "FLT_InternalTemperature" => Float(s.survival.temperature),
        "FLT_InternalTemperatureMax" => Float(s.survival.temperature_max),
        "BLN_State_Wingsuit" => Bool(s.wingsuit.signal),
        "BLN_IsGrinding" => Bool(s.grind.is_grinding()),
        "BLN_DisableWorldRender" => Bool(intro.is_some_and(|i| i.signals().iter().any(|n| n == name))),
        "FLT_ScoringFlightDistance" => Float(s.score.flight_distance),
        "FLT_Armour" => Float(s.survival.armour),
        "FLT_ScoringAccumSpinTotal" => Float(s.score.spin_fs.abs() + s.score.spin_bs.abs()),
        "FLT_ScoringAccumFlipTotal" => Float(s.score.flip_fwd.abs() + s.score.flip_bwd.abs()),
        "BLN_IsTweaking" => Bool(s.tricks.is_tweaking),
        "BLN_Goofy" => Bool(s.goofy),
        "BLN_RewindEnabled" => Bool(x.rewind.enabled),
        "BLN_RewindCountInfinite" => Bool(x.rewind.infinite),
        "BLN_WingsuitEnabled" => Bool(s.wingsuit.enabled),
        "BLN_EnableAirborneAutoUprighting" => Bool(s.airborne_auto_upright),
        "FLT_HeadLampBatteryLevel" => Float(s.survival.head_lamp_battery),
        "FLT_VignetteHealth" => Float(s.survival.vignette_health),
        "FLT_VignetteTemperature" => Float(s.survival.vignette_temperature),
        "BLN_IsBoosting" => Bool(s.boost.is_boosting),
        "BLN_IsBigLanding" => Bool(s.score.is_big_landing),
        "BLN_ImpactThisFrame" => Bool(s.impact.impact_this_frame),
        "FLT_SurfaceWeightMetal" => Float(s.weights[9]),
        "FLT_SurfaceWeightSnowCoverage" => Float(s.weights[crate::surface::SNOW_COVERAGE]),
        "BLN_HideGoggles" | "BLN_InsideHeli" => Bool(
            intro.is_some_and(|i| i.signals().iter().any(|n| n == name)) || x.nis.value("Player1", name) == Some(true),
        ),
        "BLN_ForceDescentInAir" => Bool(x.event.setup.force_descent_in_air),
        "BLN_ForceCaptureScoring" => Bool(s.score.force_capture),
        "BLN_ForceResetScoring" => Bool(s.score.overrides.force_reset),
        "BLN_ForceAwesomeOn" => Bool(s.score.overrides.force_awesome_on),
        "BLN_ForceAwesomeOff" => Bool(s.score.overrides.force_awesome_off),
        "BLN_ForceSuperAwesomeOn" => Bool(s.score.overrides.force_super_awesome_on),
        "BLN_ForceSuperAwesomeOff" => Bool(s.score.overrides.force_super_awesome_off),
        // TODO(guess): no writer of id 0x6F (`BLN_IsBailedDueToAvalanche`) was found, so it keeps its default (the avalanche's tumble is the carving's ordinary `BailOnLanding` bail); settle: a writer (a scene op's slot bound to it).
        "BLN_IsBailedDueToAvalanche" => {
            crate::guess_hit!();
            Bool(false)
        }
        _ => return None,
    })
}

fn camera_value(x: &SessionWorld, name: &str) -> Option<GsValue> {
    let intro = x.s.start.as_ref().and_then(|l| l.intro.as_ref());
    match name {
        // TODO(guess): the camera's flow is the rider's node's (one `IntroState` per binding, 0x82CDE0E0); settle: the camera actor's own transitions (its windows on its own sequence time).
        // The node's active signal or the camera actor's `PostEvent_InsideHeli` / `_OutsideHeli` tags.
        "BLN_InsideHeli" => {
            crate::guess_hit!();
            Some(GsValue::Bool(
                intro.is_some_and(|i| i.signals().iter().any(|n| n == name))
                    || x.nis.value("TheCamera", name) == Some(true),
            ))
        }
        _ => None,
    }
}

impl ObjectiveWorld for SessionWorld<'_> {
    fn game_state(&mut self, entity: &Name, id: u16) -> Option<GsValue> {
        let modelled = if entity.is("Player1") {
            player_value(self, game_state_name(id))
        } else if entity.is("TheCamera") {
            camera_value(self, game_state_name(id))
        } else {
            None
        };
        let tagged = || self.nis.value(&entity.text, game_state_name(id)).map(GsValue::Bool);
        modelled.or_else(tagged).or_else(|| self.overlay.get(&(entity.hash, id)).copied())
    }
    fn night(&mut self) -> Option<bool> {
        Some(self.w.survival.night)
    }
    fn tutorial_message_ready(&mut self) -> Option<bool> {
        self.tutorial.as_deref().map(crate::session::tutorial::Tutorial::message_ready)
    }
    fn ui_flag(&mut self, id: u16) -> Option<bool> {
        self.ui_flags.get(&id).copied().or((id == UI_HUD_READY).then_some(true))
    }
    fn free_ride(&mut self) -> Option<bool> {
        Some(self.event.setup.free_ride)
    }
    fn drop_message(&mut self) -> Option<DropMessage> {
        Some(self.event.setup.message.clone())
    }
    fn tutorial_tricks(&mut self, entity: &Name) -> Option<bool> {
        let t = self.tutorial.take()?;
        let done = t.test_tricks(&mut |id| self.game_state(entity, id));
        if done {
            self.write(entity, crate::session::tutorial::ID_FORCE_CAPTURE_SCORING, GsValue::Bool(true));
        }
        self.tutorial = Some(t);
        Some(done)
    }
    fn event_type(&mut self) -> Option<i32> {
        self.event.setup.event_type.map(|t| t as i32)
    }
    fn touching_name(&mut self, volume: &Name) -> Option<bool> {
        Some(self.tracker.touching_name(&self.w.triggers, &volume.text))
    }
    fn touching_group(&mut self, mask: u16) -> Option<bool> {
        Some(self.tracker.touching_group(&self.w.triggers, mask))
    }
    fn racer(&mut self, entity: &Name, q: ssx_lua::game::objectives::RacerQuery) -> Option<f32> {
        use ssx_lua::game::objectives::RacerQuery;
        if !entity.is("Player1") {
            return None;
        }
        let c = self.event.setup.course?;
        match q {
            RacerQuery::RemainingDistance => Some((1.0 - self.event.progress) * (c.start_y - c.end_y)),
            RacerQuery::Progress => Some(self.event.progress),
            RacerQuery::Loops => Some(self.event.loops as f32),
        }
    }
    fn equipped(&mut self, item: i32, _equipment_type: i32) -> Option<bool> {
        usize::try_from(item).ok().and_then(|i| self.w.survival.slots.get(i)).copied()
    }
    fn stat(&mut self, key: &ssx_lua::game::objectives::StatKey) -> Option<ssx_lua::game::objectives::StatValue> {
        self.stats.get(key)
    }
    fn badge_set(&mut self, badge: &Name, category: i32) -> Option<bool> {
        Some(self.stats.awarded(badge.hash, usize::try_from(category).unwrap_or(2)))
    }
    fn track_bonus_set(&mut self, bonus: &Name) -> Option<bool> {
        Some(self.stats.awarded(bonus.hash, 0))
    }
    fn effect(&mut self, objective: &Name, e: &Effect) {
        let locator = |n: &Name| crate::outro::locator(&self.w.triggers, &n.text);
        match e {
            Effect::SetGameState { entity, id, value } => self.write(entity, *id, *value),
            Effect::TutorialTrickReset { entity, .. } => {
                self.write(entity, crate::session::tutorial::ID_FORCE_RESET_SCORING, GsValue::Bool(true))
            }
            Effect::AiChaseTarget { entity, .. } if entity.is("DropHeli") => {
                self.heli.move_to = None;
                self.heli.generation += 1;
            }
            Effect::AiMoveToLocator { entity, locator: l, offset, yaw_deg } if entity.is("DropHeli") => {
                if let Some((p, q)) = locator(l) {
                    let o = glam::Vec3::from_array(*offset);
                    self.heli.move_to = Some(crate::helicopter::move_to_locator(p, q, o, *yaw_deg));
                    self.heli.generation += 1;
                }
            }
            Effect::MoveToNisStartLocation { nis, role, entity, locator: l } if entity.is("DropHeli") => {
                if let Some((p, q)) = locator(l) {
                    let start = self.nis_starts.get(&(nis.clone(), role.text.to_string())).copied().flatten();
                    let (sp, sq) = start.unwrap_or((glam::Vec3::ZERO, glam::Quat::IDENTITY));
                    self.heli.move_to = Some((p + q * sp, (q * sq).normalize()));
                    self.heli.generation += 1;
                }
            }
            Effect::PostFxStart { name, start, end } => {
                self.postfx.insert(name.text.to_string(), (*start, *end));
            }
            Effect::PostFxStop { name, .. } => {
                self.postfx.remove(&*name.text);
            }
            Effect::SetStatsWriteable(b) => self.stats.writeable = *b,
            Effect::AwardTrackBonus { bonus, .. } => self.stats.award(&bonus.text, 0),
            Effect::AwardBadge { badge, .. } => self.stats.award(&badge.text, 2),
            Effect::BadgeProgress { badge, progress } => {
                self.stats.badge_progress.insert(badge.hash, *progress);
            }
            Effect::WarpEntity { entity, locator: l } if entity.is("DropHeli") => {
                if let Some(f) = locator(l) {
                    self.heli.warp = Some(f);
                }
            }
            Effect::WriteLocatorToGameState { entity, locator: l, position_id, offset, .. }
                if entity.is("Player1") && game_state_name(*position_id as u16) == "VEC_InterceptPosition" =>
            {
                if let (Some((p, q)), Some(i)) = (locator(l), self.intercept.as_deref_mut()) {
                    i.position = p + q * glam::Vec3::from_array(*offset);
                }
            }
            _ => {}
        }
        if let Some(t) = self.tutorial.as_deref_mut() {
            t.effect(e);
        }
        self.effects.push((objective.text.to_string(), e.clone()));
    }
}

impl SessionWorld<'_> {
    pub(crate) fn write(&mut self, entity: &Name, id: u16, value: GsValue) {
        match (entity.is("Player1"), game_state_name(id), value) {
            (true, "BLN_RewindCountInfinite", GsValue::Bool(b)) => self.rewind.infinite = b,
            (true, "BLN_RewindEnabled", GsValue::Bool(b)) => self.rewind.enabled = b,
            (true, "BLN_WingsuitEnabled", GsValue::Bool(b)) => self.s.wingsuit.enabled = b,
            (true, "BLN_EnableAirborneAutoUprighting", GsValue::Bool(b)) => self.s.airborne_auto_upright = b,
            (true, "BLN_ForceDescentInAir", GsValue::Bool(b)) => self.event.setup.force_descent_in_air = b,
            (true, "BLN_ForceCaptureScoring", GsValue::Bool(b)) => self.s.score.force_capture = b,
            (true, "BLN_ForceResetScoring", GsValue::Bool(b)) => self.s.score.overrides.force_reset = b,
            (true, "BLN_ForceAwesomeOn", GsValue::Bool(b)) => self.s.score.overrides.force_awesome_on = b,
            (true, "BLN_ForceAwesomeOff", GsValue::Bool(b)) => self.s.score.overrides.force_awesome_off = b,
            (true, "BLN_ForceSuperAwesomeOn", GsValue::Bool(b)) => self.s.score.overrides.force_super_awesome_on = b,
            (true, "BLN_ForceSuperAwesomeOff", GsValue::Bool(b)) => self.s.score.overrides.force_super_awesome_off = b,
            (true, "BLN_EnableAirTricks", GsValue::Bool(_)) => {}
            (true, "BLN_TriggerDeathByDeathPlane", GsValue::Bool(b)) => self.s.death.trigger = b,
            (true, "INT_AvalancheTuningSet", GsValue::Int(v)) => self.s.avalanche.switches.tuning_set = v,
            (true, "FLT_AvalancheCarvingSensitivity", GsValue::Float(v)) => {
                self.s.avalanche.switches.carving_sensitivity = v
            }
            (true, "FLT_AvalancheImpactSensitivity", GsValue::Float(v)) => {
                self.s.avalanche.switches.impact_sensitivity = v
            }
            (false, "BLN_TrackEndsWithHeliJump", GsValue::Bool(b)) if entity.is("DropHeli") => {
                self.heli.ends_with_heli_jump = b
            }
            (false, "BLN_HeliReverseCamLevel", GsValue::Bool(b)) if entity.is("DropHeli") => self.heli.reverse_cam = b,
            (true, "BLN_HeliInteractionAllowed", GsValue::Bool(b)) => self.s.heli_interaction_allowed = b,
            (true, "BLN_AllowRaceEnd", GsValue::Bool(b)) => self.s.allow_race_end = b,
            (true, "BLN_InterceptPositionValid", GsValue::Bool(b)) if self.intercept.is_some() => {
                self.intercept.as_deref_mut().into_iter().for_each(|i| i.valid = b)
            }
            (true, "FLT_InterceptAssistanceAngle", GsValue::Float(v)) if self.intercept.is_some() => {
                self.intercept.as_deref_mut().into_iter().for_each(|i| i.angle = v)
            }
            (true, "FLT_InterceptAssistanceLengthScale", GsValue::Float(v)) if self.intercept.is_some() => {
                self.intercept.as_deref_mut().into_iter().for_each(|i| i.length_scale = v)
            }
            _ => {
                self.unapplied.insert(format!("{entity}.{}", game_state_name(id)));
                self.overlay.insert((entity.hash, id), value);
            }
        }
    }
}
