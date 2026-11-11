use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{Context, Result};
use bevy::ecs::component::ComponentId;
use bevy::ecs::schedule::ScheduleLabel;
use bevy::prelude::{
    Add, App, Asset, Assets, ChildOf, Commands, Component, Entity, Fixed, Has, Image, Last, Mesh, Name, Node, On, Res,
    Resource, StandardMaterial, Time, Transform, Vec2, Vec3, With, Without, World, default, info, warn,
};
use ssx_physics::character::DEFAULT_GEAR_MIX;
use ssx_physics::rider::Input;

use crate::camera;
use crate::render::{geotags, lighting, objects, post, sky, terrain};
use crate::ride::{self, bots, helicopter};
use crate::rider::{ghosts, skin as rider_skin, visual as rider_visual};

mod map;

#[derive(ScheduleLabel, Clone, Debug, PartialEq, Eq, Hash)]
pub struct LevelPreStartup;

#[derive(ScheduleLabel, Clone, Debug, PartialEq, Eq, Hash)]
pub struct LevelStartup;

#[derive(ScheduleLabel, Clone, Debug, PartialEq, Eq, Hash)]
pub struct LevelPostStartup;

#[derive(Component)]
pub struct LevelEntity;

#[derive(Resource)]
pub struct LevelScope {
    before: HashSet<ComponentId>,
    entities: HashSet<Entity>,
}

/// Surface shading of the terrain (`--shading`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum ShadingArg {
    /// The game's terrain material: tri-planar base surfaces and six snow
    /// layers from the vault, blended by the `.subd` attributes (the game's
    /// terrain_default shader, ported).
    Textured,
    /// Height tint (bluish grey low, white high).
    Height,
    /// Debug palette for the `.subd` material attributes (not the game's shading).
    Material,
}

#[derive(Clone, Debug, Default)]
pub struct RideOptions {
    pub spawn: Option<Vec3>,
    pub heading: Option<Vec3>,
    pub input: Option<Vec3>,
    pub jump_release: Vec<u32>,
    pub air_input: Option<Vec2>,
    pub grab: Option<Vec2>,
    pub tweak: bool,
    pub boost: bool,
    pub boost_meter: f32,
    pub fly_camera: bool,
    pub screenshot_steps: Option<u32>,
    pub event_key: Option<u32>,
    pub collected_geotags: Vec<u64>,
    pub game: bool,
    pub classic: bool,
    pub autoplay: bool,
    pub setup: Option<ssx_physics::gear::GameSetup>,
    pub setup_presentation: Option<ssx_physics::gear::GameSetup>,
    pub intro_visits: Option<u32>,
    pub own_tombstone: Option<ssx_physics::session::tombstones::OwnRecord>,
    pub live: Option<ride::live::LiveRules>,
    pub event_name: Option<String>,
    pub map_mode: Option<String>,
}

#[derive(Clone, Debug)]
pub struct LevelSpec {
    pub track_dir: PathBuf,
    pub big_root: Option<PathBuf>,
    pub subdivide: u32,
    pub shading: Option<ShadingArg>,
    pub objects: bool,
    pub drop: lighting::DropSelect,
    pub drop_explicit: bool,
    pub camera: Option<Vec3>,
    pub look_at: Option<Vec3>,
    pub aspect: f32,
    pub ride: Option<RideOptions>,
    pub character: String,
    pub screenshot: bool,
    pub map: Option<ssx_content::ContentId>,
}

pub struct LevelData {
    pub name: String,
    pub big_root: Option<PathBuf>,
    track: terrain::LoadedTrack,
    shading: terrain::Shading,
    camera: camera::CameraStart,
    objects: Option<objects::LoadedObjects>,
    pub lighting: Option<lighting::GameLighting>,
    sky: Option<sky::SkyLoad>,
    post: Option<post::PostLoad>,
    status: Vec<String>,
    pub ride: Option<ride::RideLoad>,
    heli: Option<helicopter::Loaded>,
    ghosts: Option<ghosts::Loaded>,
    geotags: Option<geotags::Loaded>,
    bots: Option<bots::Loaded>,
    particles: Option<crate::particles::LoadedParticles>,
    pub load_secs: f32,
    pub map_music: Option<ssx_content::ContentId>,
}

impl LevelData {
    pub fn load(spec: &LevelSpec) -> Result<LevelData> {
        if let Some(id) = &spec.map {
            return map::load(spec, id);
        }
        let start = Instant::now();
        let track = terrain::load(&spec.track_dir, spec.subdivide)?;
        let camera = camera::CameraStart {
            pose: camera::initial_pose(&track, spec.camera, spec.look_at, spec.aspect),
            bounds: track.bounds,
        };
        let name = track.stats.name.clone();
        let big_root = spec.big_root.clone().or_else(|| crate::render::terrain::look::find_big_root(&spec.track_dir));
        if let Some(root) = &big_root {
            crate::render::fx_states::init(root);
        }
        let shading = match spec.shading {
            Some(ShadingArg::Height) => terrain::Shading::Vertex(terrain::ColorMode::Height),
            Some(ShadingArg::Material) => terrain::Shading::Vertex(terrain::ColorMode::Material),
            Some(ShadingArg::Textured) => {
                terrain::Shading::Textured(terrain::load_textured(&spec.track_dir, &name, big_root.clone())?)
            }
            None => match terrain::load_textured(&spec.track_dir, &name, big_root.clone()) {
                Ok(textured) => terrain::Shading::Textured(textured),
                Err(e) => {
                    warn!("textured shading unavailable, using height: {e:#}");
                    terrain::Shading::Vertex(terrain::ColorMode::Height)
                }
            },
        };
        let objects = match (&big_root, spec.objects) {
            (_, false) => None,
            (Some(root), true) => {
                objects::load(root, &name).inspect_err(|e| warn!("track objects unavailable: {e:#}")).ok()
            }
            (None, true) => {
                warn!("track objects unavailable: no extracted archive root (Game-Data/Extracted/big) found");
                None
            }
        };
        let lighting = match big_root.as_ref().map(|root| lighting::load(root, &name, &spec.drop)) {
            Some(Ok(l)) => Some(l),
            Some(Err(e)) if spec.drop_explicit => return Err(e),
            Some(Err(e)) => {
                warn!("track lighting unavailable: {e:#}");
                None
            }
            None => None,
        };
        let drop_key = lighting.as_ref().map(|l| l.drop_key);
        let mut status = Vec::new();
        let sky = match (&lighting, &big_root) {
            (Some(l), Some(root)) => sky::load(l, root),
            _ => Err(anyhow::anyhow!("no track lighting preset")),
        };
        let sky = sky
            .inspect(|s| status.push(s.status.clone()))
            .inspect_err(|e| {
                warn!("game sky off: {e:#}");
                status.push(format!("sky: off ({e:#}); flat clear colour"));
            })
            .ok();
        let post = match (&lighting, &big_root) {
            (Some(l), Some(root)) => post::load(l, root, spec.screenshot),
            _ => Err(anyhow::anyhow!("no track lighting preset")),
        };
        let post = post
            .inspect(|p| status.push(p.status.clone()))
            .inspect_err(|e| {
                warn!("game post chain off: {e:#}");
                status.push(format!("post: off ({e:#}); Bevy tonemapping"));
            })
            .ok();
        let mut riders = None;
        let ride = match &spec.ride {
            Some(options) => {
                let root = big_root.as_deref().context("--ride needs the extracted archive root (--big-root)")?;
                let level = lighting.as_ref().map(|l| l.level.clone());
                let (setup, decision) =
                    load_ride(options, &spec.character, root, &spec.track_dir, &name, drop_key, level.as_deref())?;
                riders = Some(decision);
                Some(ride::prepare(setup, root, lighting.as_ref())?)
            }
            None => None,
        };
        let riding = ride.is_some();
        let root = big_root.as_deref();
        let heli = helicopter::load_level(root, Some(&name), ride.as_ref().and_then(ride::RideLoad::heli_intro));
        let ghosts = if ride.as_ref().is_some_and(|r| {
            let session = &r.ride().session;
            !ssx_physics::event::shows_explore_ghosts(session.event.setup.event_type, session.bots.is_some())
        }) {
            None
        } else {
            ghosts::load_level(root, &name, drop_key, riding)
        };
        let geotags = geotags::load_level(root, &name, riding);
        let bots = ride.as_ref().and_then(|r| bots::load_level(root, &name, drop_key, r.ride(), riders.as_ref()));
        let particles = root.and_then(|root| {
            crate::particles::load_level(
                root,
                ride.as_ref().map(ride::RideLoad::ride),
                lighting.as_ref(),
                ride.as_ref().and_then(ride::RideLoad::avalanche),
            )
        });
        Ok(LevelData {
            name,
            big_root,
            track,
            shading,
            camera,
            objects,
            lighting,
            sky,
            post,
            status,
            ride,
            heli,
            ghosts,
            geotags,
            bots,
            particles,
            load_secs: start.elapsed().as_secs_f32(),
            map_music: None,
        })
    }

    pub fn max_boost(&self) -> f32 {
        self.ride.as_ref().map_or(0.0, |r| r.ride().world().tuning.scoring.max_boost)
    }

    pub fn insert(self, world: &mut World) -> Result<()> {
        crate::render::lighting::local::reset_level(world);
        crate::render::shadows::reset_level(world);
        crate::particles::reset_level(world);
        crate::camera::chase::reset_level(world);
        ghosts::reset_level(world);
        geotags::reset_level(world);
        post::reset_level(world);
        if let Some(root) = &self.big_root {
            world.insert_resource(lighting::BigRoot(root.clone()));
        }
        if let Some(l) = self.lighting {
            world.insert_resource(l);
        }
        world.insert_resource(self.camera);
        if let Some(p) = self.post {
            post::insert(world, p)?;
        }
        if let Some(s) = self.sky {
            sky::insert(world, s)?;
        }
        world.insert_resource(post::PostStatus(self.status.join("\n")));
        terrain::insert(world, self.track, self.shading);
        if let Some(o) = self.objects {
            objects::insert(world, o);
        }
        if let Some(r) = self.ride {
            ride::insert(world, r)?;
        }
        if let Some(h) = self.heli {
            helicopter::insert(world, h);
        }
        if let Some(g) = self.ghosts {
            ghosts::insert(world, g);
        }
        if let Some(g) = self.geotags {
            geotags::insert(world, g);
        }
        if let Some(b) = self.bots {
            bots::insert(world, b);
        }
        if let Some(p) = self.particles {
            p.insert(world);
        }
        Ok(())
    }
}

fn load_ride(
    cli: &RideOptions,
    character: &str,
    big_root: &Path,
    track_dir: &Path,
    track: &str,
    drop_key: Option<u32>,
    level: Option<&ssx_assets::content::level::LevelPackages>,
) -> Result<(ride::RideSetup, ssx_assets::content::characters::budget::RiderLooks)> {
    use ssx_physics::spawn;
    let start = Instant::now();
    let session = cli.live.is_some();
    let content = ssx_assets::content::GameContent::of(big_root);
    let policy = content.session_policy().unwrap_or(ssx_content::policy::ClientMods::SESSION_DEFAULT);
    content.set_sim_policy(if session { policy } else { ssx_content::policy::ClientMods::Any });
    let sim_content_id = cli
        .event_name
        .as_deref()
        .filter(|_| !session)
        .and_then(|event| ssx_assets::content::data::simid::result_sim_id(big_root, track, event));
    let drop =
        drop_key.and_then(|key| spawn::load_drop_starts(big_root, track).ok()?.iter().position(|s| s.drop_key == key));
    let world = match drop {
        Some(d) => spawn::track_world_for_drop(big_root, track_dir, track, DEFAULT_GEAR_MIX, true, d),
        None => spawn::track_world(big_root, track_dir, track, DEFAULT_GEAR_MIX, true),
    }
    .context("building the ride's physics world")?;
    if let Some(d) = &world.collision.stats.prop_decision {
        info!("ride: {d}");
    }
    for p in &world.collision.stats.prop_packages {
        info!("ride: collision from a package: {p}");
    }
    let spawn = match drop {
        Some(d) => spawn::spawn_pose_for_drop(&world, d, cli.spawn, cli.heading)?,
        None => {
            let bounds = spawn::Boundary::load(track_dir, track);
            spawn::spawn_pose(&world, bounds.as_ref(), cli.spawn, cli.heading)?
        }
    };
    let load_secs = start.elapsed().as_secs_f32();
    let w = RideWorld {
        world,
        spawn,
        drop_start: drop.is_some() && cli.spawn.is_none(),
        drop_key,
        load_secs,
        map: None,
        vault_track: Some(track),
        track_name: track,
        sim_content_id,
    };
    ride_setup(cli, character, big_root, level, w)
}

pub(super) struct RideWorld<'a> {
    pub(super) world: ssx_physics::rider::World,
    pub(super) spawn: (Vec3, Vec3),
    pub(super) drop_start: bool,
    pub(super) drop_key: Option<u32>,
    pub(super) load_secs: f32,
    pub(super) map: Option<ride::MapRide>,
    pub(super) vault_track: Option<&'a str>,
    pub(super) track_name: &'a str,
    pub(super) sim_content_id: Option<String>,
}

pub(super) fn ride_setup(
    cli: &RideOptions,
    character: &str,
    big_root: &Path,
    level: Option<&ssx_assets::content::level::LevelPackages>,
    w: RideWorld<'_>,
) -> Result<(ride::RideSetup, ssx_assets::content::characters::budget::RiderLooks)> {
    let session = cli.live.is_some();
    let policy = ssx_assets::content::GameContent::of(big_root)
        .session_policy()
        .unwrap_or(ssx_content::policy::ClientMods::SESSION_DEFAULT);
    let board = rider_visual::load_board(big_root, w.vault_track)
        .inspect_err(|e| warn!("snowboard mesh unavailable, drawing a placeholder: {e:#}"))
        .ok();
    let mut game_setup =
        match (session && policy == ssx_content::policy::ClientMods::Presentation, &cli.setup_presentation) {
            (true, Some(p)) => {
                info!("ride: the session rides the disc's simulation fields (client mods: {policy:?})");
                Some(p.clone())
            }
            _ => cli.setup.clone(),
        };
    let item_keys: Vec<u32> = game_setup
        .as_ref()
        .map(|s| s.items.iter().filter(|g| !g.is_cleared()).map(|g| g.description).collect())
        .unwrap_or_default();
    let viewer_board = cli.setup.is_none();
    let rr = ssx_assets::content::characters::ride::ride_rider(big_root, character, level, &item_keys, viewer_board)
        .map_err(anyhow::Error::msg)?;
    for n in &rr.notes {
        warn!("ride: {n}");
    }
    let stand_in = rr.stand_in().to_string();
    let wire_key = ssx_assets::content::GameContent::of(big_root).appearance_key(&rr.request.id, &stand_in);
    if stand_in != character {
        info!("ride: {character} rides as {stand_in} (its stand-in: the simulation, the scenes, the wire)");
        if let Some(s) = game_setup.as_mut().filter(|s| s.character != 0) {
            s.character = ssx_formats::attrib::fnv1(stand_in.as_bytes());
        }
    }
    let rider = rider_skin::load_rider(big_root, w.vault_track, &rr.request)
        .inspect_err(|e| warn!("character {character} unavailable, drawing a placeholder capsule: {e:#}"))
        .ok()
        .map(|mut r| {
            r.riders = Some(rr.decision.clone());
            r
        });
    let fixed_input = cli.input.map(|v| Input { steer: v.x, brake: v.y, tuck: v.z, jump: false, ..Input::default() });
    let setup = ride::RideSetup {
        world: w.world,
        spawn: w.spawn,
        drop_start: w.drop_start,
        big_root: big_root.to_path_buf(),
        character: stand_in,
        wire_key,
        track: w.track_name.to_string(),
        drop_key: w.drop_key,
        fixed_input,
        jump_release: cli.jump_release.clone(),
        air_input: cli.air_input,
        grab: cli.grab,
        tweak: cli.tweak,
        boost: cli.boost,
        boost_meter: cli.boost_meter,
        screenshot_steps: cli.screenshot_steps,
        board,
        rider,
        fly_camera: cli.fly_camera,
        load_secs: w.load_secs,
        event_key: cli.event_key,
        collected_geotags: cli.collected_geotags.clone(),
        game: cli.game,
        classic: cli.classic,
        autoplay: cli.autoplay,
        setup: game_setup,
        intro_visits: cli.intro_visits,
        own_tombstone: cli.own_tombstone.clone(),
        live: cli.live,
        sim_content_id: w.sim_content_id,
        map: w.map,
    };
    Ok((setup, rr.decision))
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Registration {
    pub wireframe: bool,
    pub viewer_hud: bool,
}

pub fn render_plugin() -> bevy::render::RenderPlugin {
    use bevy::render::{render_resource::WgpuFeatures, settings::WgpuSettings};
    bevy::render::RenderPlugin {
        render_creation: WgpuSettings {
            features: WgpuFeatures::POLYGON_MODE_LINE | WgpuFeatures::ADDRESS_MODE_CLAMP_TO_BORDER,
            ..default()
        }
        .into(),
        ..default()
    }
}

pub fn log_plugin() -> bevy::log::LogPlugin {
    bevy::log::LogPlugin {
        fmt_layer: |_| {
            Some(Box::new(
                bevy::log::tracing_subscriber::fmt::Layer::default()
                    .with_writer(std::io::stderr)
                    .fmt_fields(ssx_core::text::log::EscapedFields),
            ))
        },
        ..default()
    }
}

fn log_content_notices() {
    for n in ssx_assets::content::store::drain_notices() {
        if n.problem {
            warn!("{}", n.text);
        } else {
            info!("{}", n.text);
        }
    }
}

pub fn register(app: &mut App, r: Registration) {
    for label in [LevelPreStartup.intern(), LevelStartup.intern(), LevelPostStartup.intern()] {
        app.init_schedule(label);
    }
    app.insert_resource(Time::<Fixed>::from_hz(f64::from(ssx_physics::SIM_HZ)));
    app.add_plugins((
        bevy::pbr::wireframe::WireframePlugin::default(),
        crate::render::scene::ScenePlugin { wireframe: r.wireframe },
        camera::FlyCameraPlugin,
        crate::render::hooks::HooksPlugin,
        crate::render::terrain::material::TerrainMaterialPlugin,
        crate::render::objects::foreground::ForegroundMaterialPlugin,
        sky::SkyPlugin,
        post::PostPlugin,
        crate::render::snow_tracks::SnowTracksPlugin,
        crate::render::lighting::local::LocalLightsPlugin,
        crate::particles::ParticlesPlugin,
    ));
    if r.viewer_hud {
        app.add_plugins(crate::shell::viewer_hud::HudPlugin);
    }
    terrain::register(app);
    objects::register(app);
    app.add_systems(Last, log_content_notices);
    ride::register(app, r.viewer_hud);
    helicopter::register(app);
    ghosts::register(app);
    geotags::register(app);
    bots::register(app);
    app.add_observer(tag_transform).add_observer(tag_node);
    if let Some(render) = app.get_sub_app_mut(bevy::render::RenderApp) {
        render.add_systems(
            bevy::render::ExtractSchedule,
            (
                drop_stale::<sky::SkySettings>,
                drop_stale::<post::PostSettings>,
                drop_stale::<crate::render::snow_tracks::SnowTrackGpu>,
            ),
        );
    }
    scoped::<lighting::BigRoot>(app);
    scoped::<lighting::GameLighting>(app);
    scoped::<camera::CameraStart>(app);
    scoped::<post::PostStatus>(app);
}

fn drop_stale<R: Resource>(mut commands: Commands, main: bevy::render::Extract<Option<Res<R>>>, local: Option<Res<R>>) {
    if main.is_none() && local.is_some() {
        commands.remove_resource::<R>();
    }
}

fn tag_transform(add: On<Add<Transform>>, scope: Option<Res<LevelScope>>, mut commands: Commands) {
    if scope.is_some() {
        commands.entity(add.entity).insert(LevelEntity);
    }
}

fn tag_node(add: On<Add<Node>>, scope: Option<Res<LevelScope>>, mut commands: Commands) {
    if scope.is_some() {
        commands.entity(add.entity).insert(LevelEntity);
    }
}

#[derive(Resource, Default)]
struct LevelResources(Vec<fn(&mut World)>);

pub fn scoped<R: Resource>(app: &mut App) {
    app.world_mut().get_resource_or_init::<LevelResources>().0.push(|w: &mut World| {
        w.remove_resource::<R>();
    });
}

fn resources(world: &World) -> HashSet<ComponentId> {
    world.iter_resources().map(|(id, _, _)| id).collect()
}

pub fn begin(world: &mut World) {
    let before = resources(world);
    let entities = world.query::<Entity>().iter(world).collect();
    world.insert_resource(LevelScope { before, entities });
}

pub fn run_startup(world: &mut World) -> f32 {
    let t0 = Instant::now();
    let mut times = Vec::new();
    for label in [LevelPreStartup.intern(), LevelStartup.intern(), LevelPostStartup.intern()] {
        let t = Instant::now();
        world.run_schedule(label);
        times.push(format!("{:.3}", t.elapsed().as_secs_f32()));
    }
    info!("level: startup schedules (pre / main / post) {} s", times.join(" / "));
    t0.elapsed().as_secs_f32()
}

pub fn startup_system(world: &mut World) {
    let secs = run_startup(world);
    info!("level: startup schedules {:.3} s", secs);
}

pub fn teardown(world: &mut World) -> (usize, usize) {
    let Some(scope) = world.remove_resource::<LevelScope>() else { return (0, 0) };
    let mut q = world.query_filtered::<Entity, With<LevelEntity>>();
    let entities: Vec<Entity> = q.iter(world).collect();
    let mut despawned = 0;
    for e in entities {
        if world.get_entity(e).is_ok() && world.despawn(e) {
            despawned += 1;
        }
    }
    let removers = world.get_resource::<LevelResources>().map(|r| r.0.clone()).unwrap_or_default();
    for remove in removers {
        remove(world);
    }
    crate::particles::reset_level(world);
    post::reset_level(world);
    crate::render::shadows::reset_level(world);
    crate::render::lighting::local::reset_level(world);
    crate::camera::chase::reset_level(world);
    ghosts::reset_level(world);
    geotags::reset_level(world);
    world.flush();
    let left = resources(world).difference(&scope.before).count();
    let mut q = world.query_filtered::<(Entity, Option<&Name>, Has<bevy::ecs::observer::Observer>, Has<ChildOf>), Without<bevy::ecs::resource::IsResource>>();
    let stray: Vec<String> = q
        .iter(world)
        .filter(|(e, ..)| !scope.entities.contains(e))
        .map(|(e, name, observer, child)| {
            format!(
                "{e}{}{}{}",
                name.map_or(String::new(), |n| format!(" {n}")),
                if observer { " observer" } else { "" },
                if child { " child" } else { "" }
            )
        })
        .collect();
    if !stray.is_empty() {
        info!("level: {} entities new since the level began remain: {}", stray.len(), stray.join(", "));
    }
    (despawned, left)
}

pub fn stats(world: &mut World) -> String {
    let entities = world.query_filtered::<Entity, Without<bevy::ecs::resource::IsResource>>().iter(world).count();
    fn len<A: Asset>(w: &World) -> String {
        w.get_resource::<Assets<A>>().map_or_else(|| "-".into(), |a| a.len().to_string())
    }
    let w = &*world;
    format!(
        "entities {entities}, meshes {}, images {}, materials: std {} terrain {} object {} foreground {} character {}, \
         shader buffers {}, resident {}",
        len::<Mesh>(w),
        len::<Image>(w),
        len::<StandardMaterial>(w),
        len::<crate::render::terrain::material::TerrainMaterial>(w),
        len::<crate::render::objects::material::ObjectMaterial>(w),
        len::<crate::render::objects::foreground::ForegroundMaterial>(w),
        len::<crate::rider::material::CharacterMaterial>(w),
        len::<bevy::render::storage::ShaderBuffer>(w),
        resident()
    )
}

fn resident() -> String {
    let pages = std::fs::read_to_string("/proc/self/statm")
        .ok()
        .and_then(|s| s.split_whitespace().nth(1).and_then(|p| p.parse::<u64>().ok()));
    pages.map_or_else(|| "-".into(), |p| format!("{:.0} MiB", (p * 4096) as f64 / (1024.0 * 1024.0)))
}
