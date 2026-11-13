mod entity;
pub mod physics;
mod player;
pub mod ripple;

use std::collections::BTreeMap;
use std::sync::Arc;

use glam::{Quat, Vec3};
pub use ssx_lua::game::controllers::tracks::{LightUpdate, Switch, Target};
use ssx_lua::game::controllers::{Controller, Phase, StateSets, Track, TrackCall};
use ssx_lua::game::events::PostedEvents;
use ssx_lua::game::objectives::{GsValue, Name, NamedFlags};

pub use entity::{EntityHost, OfflineHost};

use crate::objectives::SessionWorld;
use crate::weather::{Channel, WeatherStacks};

#[derive(Clone, Debug, PartialEq)]
pub enum ParticleTarget {
    Track(Target),
    GearItem(i32),
}

#[derive(Clone, Debug, PartialEq)]
pub struct ParticleOp {
    pub seq: u64,
    pub target: ParticleTarget,
    pub switch: Switch,
}

#[derive(Clone, Debug, PartialEq)]
pub struct LightRun {
    pub name: String,
    pub group: bool,
    pub head_lamp: bool,
    pub on: bool,
    pub factor: f32,
    pub time: f32,
    pub updates: u32,
    pub update: LightUpdate,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PostFxRun {
    pub preset: Name,
    pub weight: f32,
    pub blend_in: f32,
    pub modulated: bool,
    pub modulation: Option<(u16, bool)>,
    pub started: bool,
}

pub const MAX_OPS: usize = 256;

fn push_op(ops: &mut Vec<ParticleOp>, next_seq: &mut u64, target: ParticleTarget, switch: Switch) {
    *next_seq += 1;
    ops.push(ParticleOp { seq: *next_seq, target, switch });
    if ops.len() > MAX_OPS {
        ops.remove(0);
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct PlayerFx {
    pub particles: Vec<ParticleOp>,
    pub next_seq: u64,
    pub lights: BTreeMap<String, LightRun>,
    pub postfx: BTreeMap<u64, PostFxRun>,
    pub render_attributes: BTreeMap<String, [f32; 4]>,
    pub head_lamp_scale: f32,
    pub oxygen: bool,
    pub temperature: bool,
    pub world_ao: f32,
    pub ripples: u32,
    pub ripple: Option<Vec3>,
    pub pulse_goggles: bool,
}

impl PlayerFx {
    pub fn head_lamp(&self) -> Option<f32> {
        self.lights.values().find(|l| l.head_lamp && l.on).map(|l| l.factor)
    }

    pub fn modulated_weight(&self, preset: &str) -> Option<f32> {
        self.postfx.values().find(|p| p.modulated && p.preset.is(preset)).map(|p| p.weight)
    }

    pub fn render_attribute(&self, name: &str) -> Option<[f32; 4]> {
        self.render_attributes.get(name).copied()
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct EntityFx {
    pub particles: Vec<ParticleOp>,
    pub next_seq: u64,
    pub lights: BTreeMap<String, LightRun>,
    pub camera: Option<Vec3>,
    pub lighting: Option<(Name, f32)>,
    pub world_ao: f32,
    pub heli_ao: BTreeMap<u64, (crate::helicopter::downwash::AoWindow, f32, f32)>,
    pub physics: physics::PhysicsComponent,
}

#[derive(Clone, Debug)]
pub struct EntityController {
    pub object: u32,
    pub instance: u32,
    pub placement: usize,
    pub script: String,
    pub owner: Option<Name>,
    pub position: Vec3,
    pub rotation: Quat,
    pub placement_position: Vec3,
    pub placement_rotation: Quat,
    pub body: Option<usize>,
    pub volumes: Vec<usize>,
    pub controller: Controller,
    pub fx: EntityFx,
}

#[derive(Clone, Debug)]
pub struct Controllers {
    pub sets: Arc<StateSets>,
    pub player: Option<Controller>,
    player_default: Option<Name>,
    activated: bool,
    pub time: f32,
    pub fx: PlayerFx,
    pub entities: Vec<EntityController>,
    pub rigid: crate::rigid::RigidWorld,
    pub weather: WeatherStacks,
    pushed: BTreeMap<u64, (Channel, u32)>,
    inst: BTreeMap<u64, InstState>,
    pub log: Option<Vec<String>>,
}

pub const PLAYER: &str = "Player1";

#[derive(Clone, Debug, PartialEq)]
enum InstState {
    RenderAttribute { kept: Option<[f32; 4]>, elapsed: f32 },
    Ao { elapsed: f32, start: f32 },
    AoGroundContact { samples: std::collections::VecDeque<(f32, f32)>, elapsed: f32, start: f32 },
}

impl EntityController {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        sets: Arc<StateSets>,
        object: u32,
        instance: u32,
        placement: usize,
        script: String,
        owner: Option<Name>,
        (position, rotation): (Vec3, Quat),
        volumes: Vec<usize>,
    ) -> Option<Self> {
        if sets.sets.is_empty() {
            return None;
        }
        let controller = Controller::new(sets, 0);
        Some(EntityController {
            object,
            instance,
            placement,
            script,
            owner,
            position,
            rotation,
            placement_position: position,
            placement_rotation: rotation,
            body: None,
            volumes,
            controller,
            // TODO(guess): `DropHeli`'s `WorldAo` starts at 1.0 like a track object's without the property; settle: the `DropHelicopter` schema's property set (`default_scene.lua`, property 0x32ABFFEF).
            fx: {
                crate::guess_hit!();
                EntityFx { world_ao: 1.0, ..EntityFx::default() }
            },
        })
    }
}

pub fn script_sets(name: &str, src: &str) -> Arc<StateSets> {
    let batch = [(name.to_string(), src.as_bytes().to_vec())];
    let g = ssx_lua::game::graph::run_batch(ssx_lua::game::Binder::StateSets, &batch, None);
    Arc::new(StateSets::from_graph(&g))
}

// PORT(choice): a port cap of 2²⁰ instructions (1 024 × the disc's largest controller, 1 024); the scripting round's
// budget for a package's controller; nothing on the disc (the game runs only its own scripts).
pub const MAP_CONTROLLER_BUDGET: u64 = 1 << 20;

pub fn script_sets_within(name: &str, src: &str, budget: u64) -> Result<Arc<StateSets>, String> {
    let batch = [(name.to_string(), src.as_bytes().to_vec())];
    let g = ssx_lua::game::graph::run_batch_with(ssx_lua::game::Binder::StateSets, &batch, None, &move |l| {
        l.budget = budget;
    });
    match &g.scripts[0].1 {
        Some(e) => Err(e.clone()),
        None => Ok(Arc::new(StateSets::from_graph(&g))),
    }
}

impl Controllers {
    pub fn load(
        state_sets: &ssx_lua::game::graph::Graph,
        scene: &ssx_lua::game::scene::Scene,
        schemas: &ssx_lua::game::scene::Schemas,
        triggers: &crate::triggers::TriggerSet,
        head_lamp_scale: f32,
    ) -> Self {
        let sets = Arc::new(StateSets::from_graph(state_sets));
        let schema = scene.schema_of(PLAYER).and_then(|s| schemas.schemas.get(s));
        let player = schema.and_then(|s| Some(Controller::new(sets.clone(), sets.set(s.controller.as_deref()?)?)));
        let player_default = schema.and_then(|s| s.default_state.as_deref()).map(Name::new);
        let mut c = Self::offline(triggers);
        (c.sets, c.player, c.player_default) = (sets, player, player_default);
        c.fx.head_lamp_scale = head_lamp_scale;
        c
    }

    pub fn offline(triggers: &crate::triggers::TriggerSet) -> Self {
        let mut by_script: BTreeMap<&str, Arc<StateSets>> = BTreeMap::new();
        let mut entities = Vec::new();
        let mut rigid = crate::rigid::RigidWorld::default();
        for e in &triggers.entities {
            let Some((script, src)) = triggers.scripts.get(&e.object) else { continue };
            let sets = by_script.entry(script.as_str()).or_insert_with(|| script_sets(script, src)).clone();
            let volumes = (0..triggers.volumes.len())
                .filter(|&i| {
                    let v = &triggers.volumes[i];
                    (v.object, v.instance, v.placement, v.scope) == (e.object, e.instance, e.placement, e.scope)
                })
                .collect();
            let frame = (e.position, e.rotation);
            let Some(mut x) =
                EntityController::new(sets, e.object, e.instance, e.placement, script.clone(), None, frame, volumes)
            else {
                continue;
            };
            if let Some(def) = triggers.bodies.get(&e.object) {
                x.body = Some(rigid.bodies.len());
                rigid.bodies.push(crate::rigid::Body::new(*def, e.position, e.rotation));
            }
            entities.push(x);
        }
        Controllers {
            sets: Arc::default(),
            player: None,
            player_default: None,
            activated: false,
            time: 0.0,
            fx: PlayerFx { world_ao: 1.0, ..PlayerFx::default() },
            entities,
            rigid,
            weather: WeatherStacks::default(),
            pushed: BTreeMap::new(),
            inst: BTreeMap::new(),
            log: None,
        }
    }

    pub fn add_entity(&mut self, e: EntityController) {
        self.entities.push(e);
    }

    pub fn entity(&self, instance: u32, placement: usize) -> Option<&EntityController> {
        self.entities.iter().find(|e| (e.instance, e.placement) == (instance, placement) && e.owner.is_none())
    }
}

pub struct Ctx<'a, 'b> {
    pub sw: &'a mut SessionWorld<'b>,
    pub flags: &'a mut NamedFlags,
    pub events: &'a PostedEvents,
}

impl Controllers {
    // TODO(guess): the player first and the entities in the trigger set's order (no disc script pair depends on it); settle: the entity list behind the manager's +0x9C (0x82D3DB98).
    pub fn update(&mut self, dt: f32, ctx: &mut Ctx<'_, '_>) {
        crate::guess_hit!();
        let first = !self.activated;
        self.activated = true;
        self.time += dt;
        if let Some(c) = self.player.as_mut() {
            let mut w = player::PlayerWorld { ctx, fx: &mut self.fx, inst: &mut self.inst, log: self.log.as_mut() };
            if first {
                c.set_state(self.player_default.as_ref(), &mut w);
            }
            c.update(dt, &mut w);
        }
        let mut host = entity::SessionHost { ctx };
        self.update_entities(dt, &mut host, first);
    }

    pub fn update_offline(&mut self, dt: f32, host: &mut dyn EntityHost) {
        let first = !self.activated;
        self.activated = true;
        self.time += dt;
        self.update_entities(dt, host, first);
    }

    fn update_entities(&mut self, dt: f32, host: &mut dyn EntityHost, first: bool) {
        for e in &mut self.entities {
            let mut w = entity::EntityWorld {
                host: &mut *host,
                body: e.body.and_then(|i| self.rigid.bodies.get_mut(i)),
                fx: &mut e.fx,
                owner: e.owner.as_ref(),
                position: e.position,
                rotation: e.rotation,
                volumes: &e.volumes,
                weather: &mut self.weather,
                pushed: &mut self.pushed,
                log: self.log.as_mut(),
                script: &e.script,
                time: self.time,
            };
            if first {
                e.controller.set_state(None, &mut w);
            }
            e.controller.update(dt, &mut w);
        }
    }

    pub fn step_bodies(&mut self, dt: f32, geo: &crate::collision::CollisionWorld) {
        self.rigid.step(dt, geo);
        for e in &mut self.entities {
            if let Some(b) = e.body.and_then(|i| self.rigid.bodies.get(i)) {
                (e.position, e.rotation) = (b.motion.position, b.motion.orientation);
                e.fx.physics.frozen = b.frozen;
            }
        }
    }

    pub fn reset_bodies(&mut self) {
        for e in &mut self.entities {
            if let Some(b) = e.body.and_then(|i| self.rigid.bodies.get_mut(i)) {
                *b = crate::rigid::Body::new(b.def, e.placement_position, e.placement_rotation);
                (e.position, e.rotation) = (e.placement_position, e.placement_rotation);
                e.fx.physics.frozen = true;
            }
        }
    }

    pub fn player_states(&self) -> Vec<String> {
        self.player.as_ref().map_or_else(Vec::new, Controller::states)
    }
}

fn player_name() -> Name {
    Name::new(PLAYER)
}

fn as_float(v: Option<GsValue>) -> f32 {
    match v {
        Some(GsValue::Float(x)) => x,
        Some(GsValue::Int(i)) => i as f32,
        Some(GsValue::Bool(b)) => f32::from(u8::from(b)),
        _ => 0.0,
    }
}

fn apply_light(lights: &mut BTreeMap<String, LightRun>, c: TrackCall<'_>, phase: Phase, battery: f32, scale: f32) {
    let Track::Light { name, group, on_start, update, on_end, head_lamp } = c.kind else { return };
    let key = if *head_lamp { "#headlamp".to_string() } else { name.clone() };
    let run = lights.entry(key).or_insert_with(|| LightRun {
        name: name.clone(),
        group: *group,
        head_lamp: *head_lamp,
        on: false,
        factor: 1.0,
        time: 0.0,
        updates: 0,
        update: *update,
    });
    let apply = |run: &mut LightRun, mode: Switch| match mode {
        Switch::Activate => run.on = true,
        Switch::Deactivate | Switch::DeactivateImmediately => run.on = false,
        Switch::None => {}
    };
    match phase {
        Phase::Begin => {
            (run.time, run.updates, run.update) = (0.0, 0, *update);
            apply(run, *on_start);
        }
        Phase::Update(dt) => {
            run.time += dt;
            run.updates = run.updates.wrapping_add(1);
            if *update == LightUpdate::HeadLamp {
                run.factor = battery.clamp(0.0, 1.0) * scale;
            }
        }
        Phase::End => apply(run, *on_end),
    }
}

pub fn postfx_presets(
    drop: &crate::objectives::DropScripts,
    event: Option<&crate::objectives::DropScripts>,
) -> Vec<String> {
    use ssx_lua::game::Binder;
    use ssx_lua::game::graph::run_batch;
    let sets = StateSets::from_graph(&run_batch(
        Binder::StateSets,
        &drop.named(&[crate::objectives::STATE_SETS_SCRIPT]),
        None,
    ));
    let mut out: Vec<String> = sets
        .kinds
        .iter()
        .filter_map(|k| match k {
            Track::PostFx { preset, .. } | Track::PostFxModulated { preset, .. } => Some(preset.text.to_string()),
            _ => None,
        })
        .collect();
    let level = run_batch(Binder::Objectives, &drop.level_batch(event), None);
    out.extend(
        level.calls.iter().filter(|c| c.name() == "DO_PostFXStart").filter_map(|c| c.arg(1).str().map(str::to_string)),
    );
    out.sort();
    out.dedup();
    out
}

#[derive(Clone, Debug)]
pub struct NamedEntity {
    pub name: String,
    pub object: u32,
    pub script: String,
    pub source: Arc<str>,
}

impl NamedEntity {
    pub fn load(big_root: &std::path::Path, track: &str, name: &str, object: u32) -> Option<Self> {
        Self::load_in(big_root, Some(track), name, object)
    }

    pub fn load_in(big_root: &std::path::Path, track: Option<&str>, name: &str, object: u32) -> Option<Self> {
        let scripts = ssx_assets::vault_db::with_level_db(big_root, track, |db| {
            ssx_assets::track_particles::object_controllers(db, &[object])
        })
        .ok()?;
        let script = scripts.get(&object)?.clone();
        let path = ssx_assets::track_objects::asset_file(big_root, &script)?;
        let source = Arc::from(std::fs::read_to_string(path).ok()?);
        Some(NamedEntity { name: name.to_string(), object, script, source })
    }

    pub fn controller(&self) -> Option<EntityController> {
        let sets = script_sets(&self.script, &self.source);
        let frame = (Vec3::ZERO, Quat::IDENTITY);
        EntityController::new(sets, self.object, 0, 0, self.script.clone(), Some(Name::new(&self.name)), frame, vec![])
    }
}
