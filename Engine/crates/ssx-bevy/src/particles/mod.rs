pub mod geotag;
pub mod inputs;
pub mod light;
mod live_spray;
mod load;
pub(crate) use load::reload_textures;
pub mod render;
pub mod rider;
pub mod shadow_query;
pub mod sim;
mod snapshot;
pub mod spray;
pub mod tombstone;
pub mod track;
mod update;
pub mod weather;

use std::sync::Arc;

use bevy::prelude::*;
use bevy::render::RenderApp;
use bevy::render::extract_resource::{ExtractResource, ExtractResourcePlugin};
use ssx_assets::particles::ParticleNode;

use crate::particles::sim::{EffectInstance, Registry, Rng, SetArena};
use crate::particles::spray::SnowSprayDriver;
use crate::particles::track::{DefCache, TrackEffects};
use crate::particles::weather::WeatherManager;
use crate::render::post::GamePostCamera;
use crate::ride::Ride;
use load::build;
use update::update;

pub struct ParticlesPlugin;

impl Plugin for ParticlesPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(ExtractResourcePlugin::<ParticleFrame>::default()).init_resource::<ParticleFrame>();
        crate::shell::level::scoped::<ParticleWorld>(app);
        app.add_systems(Update, sample_depth)
            .add_systems(PostUpdate, update.after(bevy::transform::TransformSystems::Propagate))
            .add_systems(Last, update::finish);
        crate::particles::render::build(app);
        shadow_query::plugin(app);
    }
}

pub fn reset_level(world: &mut World) {
    world.insert_resource(ParticleFrame::default());
}

pub struct ParticleTexture {
    pub name: String,
    pub size: UVec2,
    pub levels: Vec<Vec<u8>>,
}

#[derive(Resource, Clone, ExtractResource)]
#[extract_app(RenderApp)]
pub struct ParticleFrame {
    pub draws: Arc<render::FrameDraws>,
    pub textures: Arc<Vec<ParticleTexture>>,
    pub light: crate::render::lighting::game::GameLightUniform,
    pub serial: u64,
}

#[derive(Clone, Copy, Debug)]
pub struct ParticleLight {
    pub position: Vec3,
    pub colour: Vec4,
    pub radius: f32,
    pub falloff: f32,
}

impl Default for ParticleLight {
    fn default() -> Self {
        ParticleLight { position: Vec3::ZERO, colour: Vec4::ZERO, radius: 1.0, falloff: 1.0 }
    }
}

impl Default for ParticleFrame {
    fn default() -> Self {
        ParticleFrame {
            draws: Default::default(),
            textures: Default::default(),
            light: crate::render::lighting::game::GameLightUniform::viewer_default(),
            serial: 0,
        }
    }
}

struct SlotEffect {
    node: ParticleNode,
    instance: EffectInstance,
    behaviour: Option<Arc<inputs::Script>>,
    start: f32,
    stop: f32,
    instant: bool,
}

struct Spray {
    effects: Vec<SlotEffect>,
    table: [[Option<usize>; 6]; 3],
    driver: SnowSprayDriver,
    inputs: crate::particles::inputs::Trackers,
}

struct TrailSlot {
    set: usize,
    chunk: usize,
    node: ParticleNode,
    instance: EffectInstance,
    start: f32,
    stop: f32,
    blend: Vec<(u8, f32)>,
}

struct SunSlot {
    node: ParticleNode,
    instance: EffectInstance,
    dir: Vec3,
}

const SUN_DISTANCE: f32 = 500.0;

#[derive(Resource)]
pub(crate) struct ParticleWorld(crate::frame_task::FrameTask<ParticleState, update::Done>);

impl ParticleWorld {
    fn new(state: ParticleState) -> Self {
        ParticleWorld(crate::frame_task::FrameTask::new(state))
    }
}

#[derive(Clone, Debug, Default)]
struct Switches {
    tricky: Option<bool>,
    log: Option<String>,
    hide: Vec<String>,
    precipitation: Option<f32>,
}

impl Switches {
    fn from_env() -> Self {
        let var = |k: &str| std::env::var(k).ok();
        Switches {
            tricky: var("SSX_RIDER_TRICKY").and_then(|f| match f.as_str() {
                "awesome" => Some(false),
                "super" => Some(true),
                _ => None,
            }),
            log: var("SSX_PARTICLES_LOG").map(|v| v.to_ascii_lowercase()),
            hide: var("SSX_FX_HIDE")
                .filter(|v| !v.is_empty())
                .map(|v| v.split(',').map(|n| n.trim().to_owned()).collect())
                .unwrap_or_default(),
            precipitation: var("SSX_PRECIPITATION").as_deref().and_then(inputs::parse_precipitation),
        }
    }

    fn log_is(&self, value: &str) -> bool {
        self.log.as_deref() == Some(value)
    }
}

pub(crate) struct ParticleState {
    spray: Option<Spray>,
    bot_sprays: Vec<Spray>,
    live_sprays: live_spray::LiveSprays,
    sun: Option<SunSlot>,
    rider: Option<rider::RiderEffects>,
    rider_tried: bool,
    heli: Option<rider::RiderEffects>,
    heli_tried: bool,
    defs: DefCache,
    texture_names: Vec<String>,
    trails: Vec<TrailSlot>,
    geotags: geotag::GeotagSlots,
    tombstones: tombstone::TombstoneSlots,
    track: TrackEffects,
    weather: WeatherManager,
    arena: SetArena,
    rng: Rng,
    snapshots: snapshot::Snapshots,
    clock: f32,
    last_render_time: f32,
    frame: u32,
    dt_smoothed: f32,
    last_step: u64,
    pending: f32,
    textures: Arc<Vec<ParticleTexture>>,
    registry: Registry,
    unported: std::collections::HashSet<(String, String)>,
    light: Option<crate::particles::light::NearestLight>,
    level: Option<Arc<ssx_assets::content::level::LevelPackages>>,
    switches: Switches,
}

pub struct LoadedParticles(ParticleState);

impl LoadedParticles {
    pub fn insert(self, world: &mut World) {
        world.insert_resource(ParticleWorld::new(self.0));
    }

    // PORT(choice): the rider's and the helicopter's effects (their particle nodes, definitions and textures) load on
    // the level's loading thread, not in the particles' first update of the level's spawn frame on the game thread,
    // where they took 32-51 ms (alps0101, the heli drop, Mount Eddie; development laptop, `trace_chrome`). They read
    // the rig's graph, the character's name, the ride's loadout and the helicopter's nodes alone, which do not change
    // from the load to the spawn, and enter the effects' arena first in the same order; the console's loading is not
    // decoded.
    pub fn load_effects(
        &mut self,
        root: &std::path::Path,
        track: Option<&str>,
        rider: Option<(&str, &ssx_assets::character::Loadout, &ssx_formats::crsf::SceneGraph)>,
        heli: Option<(&[ssx_assets::particles::ParticleNode], &ssx_formats::crsf::SceneGraph)>,
    ) {
        let w = &mut self.0;
        if let Some((summary, loadout, graph)) = rider.filter(|_| !w.rider_tried) {
            load::load_rider_effects_of(w, root, track, summary, loadout, graph);
        }
        if let Some((nodes, rig)) = heli.filter(|_| !w.heli_tried) {
            load::load_heli_effects_of(w, root, nodes, rig);
        }
    }
}

// PORT(choice): the Particles switch is the port's, for slower machines; off, it keeps every effect that shows the
// player something, drawn as the game draws it — a survive event's tombstones (`tomb_stone_gameobject` has no mesh: its
// `fx_tombstone_light` is the tombstone, a rival's run end), the geotags' pickup bursts, the avalanche's trails, and the
// track's course markers and death-pit clouds ([`track::GAMEPLAY_EFFECTS`]) — and drops the look alone: the snow
// sprays (the rider's, the bots', the live racers'), the rider's and the helicopter's other effects, the sun's glare
// and the track's ambience (mist, wispy and blowing snow, steam, god rays, the falling snow, …). The console has no
// such switch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    All,
    Gameplay,
}

impl Scope {
    pub fn of_settings() -> Scope {
        if crate::render::post::env_is("SSX_PARTICLES", "off") { Scope::Gameplay } else { Scope::All }
    }
}

pub fn load_level(
    root: &std::path::Path,
    ride: Option<&Ride>,
    lighting: Option<&crate::render::lighting::GameLighting>,
    avalanche: Option<&crate::render::avalanche::AvalancheScene>,
) -> LoadedParticles {
    LoadedParticles(build(root, ride, lighting, avalanche, Scope::of_settings()))
}

pub fn reload_world(world: &mut World) {
    if !world.contains_resource::<ParticleWorld>() {
        return;
    }
    let Some(root) = world.get_resource::<crate::render::lighting::BigRoot>().map(|r| r.0.clone()) else { return };
    let built = build(
        &root,
        world.get_resource::<Ride>(),
        world.get_resource::<crate::render::lighting::GameLighting>(),
        world.get_resource::<crate::render::avalanche::AvalancheScene>(),
        Scope::of_settings(),
    );
    world.insert_resource(ParticleWorld::new(built));
    info!("mod-dev: the particle world built again");
}

fn sample_depth(mut cameras: Query<&mut Camera3d, With<GamePostCamera>>) {
    for mut cam in &mut cameras {
        let usage = bevy::render::render_resource::TextureUsages::from(cam.depth_texture_usages);
        if !usage.contains(bevy::render::render_resource::TextureUsages::TEXTURE_BINDING) {
            cam.depth_texture_usages = (usage | bevy::render::render_resource::TextureUsages::TEXTURE_BINDING).into();
        }
    }
}
