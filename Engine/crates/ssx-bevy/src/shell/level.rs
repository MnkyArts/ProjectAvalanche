use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{Context, Result};
use bevy::ecs::component::ComponentId;
use bevy::ecs::schedule::ScheduleLabel;
use bevy::prelude::{
    Add, App, Asset, Assets, ChildOf, Commands, Component, Entity, Fixed, Has, Image, Last, Mesh, Mut, Name, Node, On,
    Res, Resource, StandardMaterial, Time, Transform, Vec2, Vec3, With, Without, World, default, info, warn,
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
    pub steer_from: Option<f32>,
    pub grab: Option<Vec2>,
    pub tweak: bool,
    pub shoulders: Option<[bool; 4]>,
    pub boost: bool,
    pub boost_meter: f32,
    pub fly_camera: bool,
    pub screenshot_steps: Option<u32>,
    pub event_key: Option<u32>,
    pub free_ride: bool,
    pub collected_geotags: Vec<u64>,
    pub game: bool,
    pub classic: bool,
    pub autoplay: bool,
    pub setup: Option<ssx_physics::gear::GameSetup>,
    pub setup_presentation: Option<ssx_physics::gear::GameSetup>,
    pub intro_visits: Option<u32>,
    pub own_tombstone: Option<ssx_physics::session::tombstones::OwnRecord>,
    pub own_ghost: Option<(usize, ssx_assets::ghosts::GhostReplay)>,
    pub live: Option<ride::live::LiveRules>,
    pub game_mode: Option<i64>,
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
    local_lights: lighting::local::LoadedLocalLights,
    reflection: Option<lighting::game::LoadedReflection>,
    pub load_secs: f32,
    pub map_music: Option<ssx_content::ContentId>,
}

impl LevelData {
    pub fn load(spec: &LevelSpec) -> Result<LevelData> {
        if let Some(id) = &spec.map {
            return map::load(spec, id);
        }
        let start = Instant::now();
        let name = terrain::track_name(&spec.track_dir);
        let big_root = spec.big_root.clone();
        if let Some(root) = &big_root {
            crate::render::fx_states::init(root);
        }
        // PORT(choice): the level's parts load side by side on the cores (`load_pool`): the terrain's materials beside
        // the lighting; then the objects, the terrain, the sky and post beside the ride and the parts that follow it.
        // One after another they took 5.7 s of alps0101's load on this thread (development laptop, `trace_chrome`;
        // the objects 2.7 s, the ride 1.6 s, its bots 0.8 s), side by side the ride's path alone is left. Each part
        // reads what it read one after another and builds the same data; the parts that draw from the game's global
        // generator (the ride's bots, geotags and avalanche) stay on this thread in their order. The console's loader
        // threads are not decoded.
        let (lighting, shading) = crate::load_pool::join(
            "level terrain materials",
            || load_lighting(spec, &name, big_root.as_deref()),
            || load_shading(spec, &name, big_root.as_deref()),
        );
        let shading = shading?;
        let lighting = lighting?;
        let drop_key = lighting.as_ref().map(|l| l.drop_key);
        let world = || load_world(spec, &name, big_root.as_deref(), drop_key, lighting.as_ref(), &shading);
        let rides = || load_rides(spec, &name, big_root.as_deref(), drop_key, lighting.as_ref());
        let (rides, world) = if ride_keeps_policy(spec, big_root.as_deref()) {
            crate::load_pool::join("level world", rides, world)
        } else {
            let world = world();
            (rides(), world)
        };
        let WorldParts { track, camera, objects, sky, post, status } = world?;
        let RideParts { ride, heli, ghosts, geotags, bots, particles, local_lights, reflection } = rides?;
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
            local_lights,
            reflection,
            load_secs: start.elapsed().as_secs_f32(),
            map_music: None,
        })
    }

    pub fn max_boost(&self) -> f32 {
        self.ride.as_ref().map_or(0.0, |r| r.ride().world().tuning.scoring.max_boost)
    }

    pub fn queue_textures(&mut self, world: &mut World) {
        world.try_resource_scope(|world, mut uploads: Mut<crate::render::uploads::LevelUploads>| {
            let mut images = world.resource_mut::<Assets<Image>>();
            let (uploads, images) = (&mut *uploads, &mut *images);
            self.shading.queue_textures(uploads, images);
            if let Some(o) = self.objects.as_mut() {
                o.queue_textures(uploads, images);
            }
            // PORT(choice): the textures the level's other parts build on its loading thread go through the queue
            // before its spawn as the terrain's and the objects' do: the rider's, the board's, the avalanche's, the
            // helicopter's, the racers', the sky's (with its mid-ground and a package's background), the local lights'
            // projections and the reflection cube. Added in the spawn they went up at once in the frame after it,
            // 33-58 ms of its `prepare_assets::<GpuImage>` (alps0101, the heli drop, Mount Eddie; development laptop,
            // `trace_chrome`). Their materials are still made in the spawn, in their order, with every texture on the
            // GPU, as before; the console streams its levels into memory it owns.
            if let Some(r) = self.ride.as_mut() {
                r.queue_images(uploads, images);
            }
            if let Some(h) = self.heli.as_mut() {
                h.queue_images(uploads, images);
            }
            if let Some(b) = self.bots.as_mut() {
                b.queue_images(uploads, images);
            }
            if let Some(s) = self.sky.as_mut() {
                s.queue_images(uploads, images);
            }
            self.local_lights.queue_image(uploads, images);
            if let Some(r) = self.reflection.as_mut() {
                r.queue_image(uploads, images);
            }
            // PORT(choice): the snow tracks' two textures (`LookupTableMap`, `DispTilesMap`, made on the loading thread
            // with the terrain's draws), the survival overlay's two maps and the TOPOGRID's grid go through the queue
            // last, after the textures above, which keep their places in it. Added in the spawn they went up in the
            // frame after it with the parts' meshes (`prepare_assets::<GpuImage>` 14.7 ms of a 72 ms render frame at
            // alps0101's spawn, development laptop, `trace_chrome`). The same textures, made from the same data, with
            // the same recipes for the hot reload; the console streams its levels into memory it owns.
            self.track.queue_textures(uploads, images);
            if let Some(r) = self.ride.as_mut() {
                r.queue_post_images(uploads, images);
            }
        });
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
        lighting::local::insert(world, self.local_lights);
        if let Some(r) = self.reflection {
            lighting::game::insert_reflection(world, r);
        }
        Ok(())
    }
}

fn load_shading(spec: &LevelSpec, name: &str, big_root: Option<&Path>) -> Result<terrain::Shading> {
    Ok(match spec.shading {
        Some(ShadingArg::Height) => terrain::Shading::Vertex(terrain::ColorMode::Height),
        Some(ShadingArg::Material) => terrain::Shading::Vertex(terrain::ColorMode::Material),
        Some(ShadingArg::Textured) => terrain::Shading::Textured(terrain::load_textured(big_root, name)?),
        None => match terrain::load_textured(big_root, name) {
            Ok(textured) => terrain::Shading::Textured(textured),
            Err(e) => {
                warn!("textured shading unavailable, using height: {e:#}");
                terrain::Shading::Vertex(terrain::ColorMode::Height)
            }
        },
    })
}

fn load_lighting(spec: &LevelSpec, name: &str, big_root: Option<&Path>) -> Result<Option<lighting::GameLighting>> {
    Ok(match big_root.map(|root| lighting::load(root, name, &spec.drop)) {
        Some(Ok(l)) => Some(l),
        Some(Err(e)) if spec.drop_explicit => return Err(e),
        Some(Err(e)) => {
            warn!("track lighting unavailable: {e:#}");
            None
        }
        None => None,
    })
}

struct WorldParts {
    track: terrain::LoadedTrack,
    camera: camera::CameraStart,
    objects: Option<objects::LoadedObjects>,
    sky: Option<sky::SkyLoad>,
    post: Option<post::PostLoad>,
    status: Vec<String>,
}

fn load_world(
    spec: &LevelSpec,
    name: &str,
    big_root: Option<&Path>,
    drop_key: Option<u32>,
    lighting: Option<&lighting::GameLighting>,
    shading: &terrain::Shading,
) -> Result<WorldParts> {
    let objects = || match (big_root, spec.objects) {
        (_, false) => None,
        (Some(root), true) => {
            objects::load(root, name, drop_key).inspect_err(|e| warn!("track objects unavailable: {e:#}")).ok()
        }
        (None, true) => {
            warn!("track objects unavailable: no extracted archive root (Game-Data/Extracted/big) found");
            None
        }
    };
    let terrain = || -> Result<(terrain::LoadedTrack, camera::CameraStart)> {
        let drop_files = drop_terrain_files(big_root, &spec.track_dir, name, drop_key)?;
        let mut track = terrain::load(&spec.track_dir, big_root, spec.subdivide, drop_files.as_deref())?;
        let camera = camera::CameraStart {
            pose: camera::initial_pose(&track, spec.camera, spec.look_at, spec.aspect),
            bounds: track.bounds,
        };
        track.cut_draws(shading);
        Ok((track, camera))
    };
    let sky_post = || {
        let sky = match (lighting, big_root) {
            (Some(l), Some(root)) => sky::load(l, root),
            _ => Err(anyhow::anyhow!("no track lighting preset")),
        };
        let post = match (lighting, big_root) {
            (Some(l), Some(root)) => post::load(l, root, spec.screenshot),
            _ => Err(anyhow::anyhow!("no track lighting preset")),
        };
        (sky, post)
    };
    let (terrain, (objects, (sky, post))) =
        crate::load_pool::join("level objects", terrain, || crate::load_pool::join("level sky", objects, sky_post));
    let (track, camera) = terrain?;
    let mut status = Vec::new();
    let sky = sky
        .inspect(|s| status.push(s.status.clone()))
        .inspect_err(|e| {
            warn!("game sky off: {e:#}");
            status.push(format!("sky: off ({e:#}); flat clear colour"));
        })
        .ok();
    let post = post
        .inspect(|p| status.push(p.status.clone()))
        .inspect_err(|e| {
            warn!("game post chain off: {e:#}");
            status.push(format!("post: off ({e:#}); Bevy tonemapping"));
        })
        .ok();
    Ok(WorldParts { track, camera, objects, sky, post, status })
}

struct RideParts {
    ride: Option<ride::RideLoad>,
    heli: Option<helicopter::Loaded>,
    ghosts: Option<ghosts::Loaded>,
    geotags: Option<geotags::Loaded>,
    bots: Option<bots::Loaded>,
    particles: Option<crate::particles::LoadedParticles>,
    local_lights: lighting::local::LoadedLocalLights,
    reflection: Option<lighting::game::LoadedReflection>,
}

fn load_rides(
    spec: &LevelSpec,
    name: &str,
    big_root: Option<&Path>,
    drop_key: Option<u32>,
    lighting: Option<&lighting::GameLighting>,
) -> Result<RideParts> {
    let mut riders = None;
    let ride = match &spec.ride {
        Some(options) => {
            let root = big_root.context("--ride needs the extracted archive root (--big-root)")?;
            let level = lighting.map(|l| l.level.clone());
            let (setup, decision) =
                load_ride(options, &spec.character, root, &spec.track_dir, name, drop_key, level.as_deref())?;
            riders = Some(decision);
            Some(ride::prepare(setup, root, lighting)?)
        }
        None => None,
    };
    let riding = ride.is_some();
    let root = big_root;
    let lights = || {
        let local = lighting::local::load_level(root, lighting, ride.as_ref().map(ride::RideLoad::ride));
        (local, lighting.map(lighting::game::load_reflection))
    };
    let follow = || ride_parts(name, root, drop_key, lighting, ride.as_ref(), riders.as_ref(), riding);
    let (parts, (local_lights, reflection)) = crate::load_pool::join("level lights", follow, lights);
    let FollowParts { heli, ghosts, geotags, bots, particles } = parts;
    Ok(RideParts { ride, heli, ghosts, geotags, bots, particles, local_lights, reflection })
}

struct FollowParts {
    heli: Option<helicopter::Loaded>,
    ghosts: Option<ghosts::Loaded>,
    geotags: Option<geotags::Loaded>,
    bots: Option<bots::Loaded>,
    particles: Option<crate::particles::LoadedParticles>,
}

fn ride_parts(
    name: &str,
    root: Option<&Path>,
    drop_key: Option<u32>,
    lighting: Option<&lighting::GameLighting>,
    ride: Option<&ride::RideLoad>,
    riders: Option<&ssx_assets::content::characters::budget::RiderLooks>,
    riding: bool,
) -> FollowParts {
    let heli = helicopter::load_level(root, Some(name), ride.and_then(ride::RideLoad::heli_intro));
    let ghosts = if loads_ghost_holograms(ride.map(|r| &r.ride().session)) {
        ghosts::load_level(root, name, drop_key, ride.map(ride::RideLoad::ride))
    } else {
        None
    };
    let geotags = geotags::load_level(root, name, riding);
    let bots = ride.and_then(|r| bots::load_level(root, name, drop_key, r.ride(), riders));
    let particles = root.map(|root| {
        let mut p = crate::particles::load_level(
            root,
            ride.map(ride::RideLoad::ride),
            lighting,
            ride.and_then(ride::RideLoad::avalanche),
        );
        load_particle_effects(&mut p, root, lighting, ride, heli.as_ref());
        p
    });
    FollowParts { heli, ghosts, geotags, bots, particles }
}

fn load_particle_effects(
    particles: &mut crate::particles::LoadedParticles,
    root: &Path,
    lighting: Option<&lighting::GameLighting>,
    ride: Option<&ride::RideLoad>,
    heli: Option<&helicopter::Loaded>,
) {
    let rider = lighting.and(ride).and_then(ride::RideLoad::rider_effect_inputs);
    let track = lighting.and_then(lighting::GameLighting::disc_track);
    particles.load_effects(root, track, rider, heli.map(helicopter::Loaded::effect_inputs));
}

fn ride_policy(options: &RideOptions, content: &ssx_assets::content::GameContent) -> ssx_content::policy::ClientMods {
    let policy = content.session_policy().unwrap_or(ssx_content::policy::ClientMods::SESSION_DEFAULT);
    if options.live.is_some() { policy } else { ssx_content::policy::ClientMods::Any }
}

fn ride_keeps_policy(spec: &LevelSpec, big_root: Option<&Path>) -> bool {
    match (&spec.ride, big_root) {
        (Some(options), Some(root)) => {
            let content = ssx_assets::content::GameContent::of(root);
            ride_policy(options, &content) == content.sim_policy()
        }
        _ => true,
    }
}

fn drop_terrain_files(
    big_root: Option<&Path>,
    track_dir: &Path,
    track: &str,
    drop_key: Option<u32>,
) -> Result<Option<Vec<String>>> {
    let (Some(root), Some(key)) = (big_root, drop_key) else { return Ok(None) };
    let starts = ssx_physics::spawn::load_drop_starts(root, track)
        .with_context(|| format!("{track}: the drops' start locators"))?;
    let Some(start) = starts.into_iter().find(|s| s.drop_key == key) else {
        anyhow::bail!("{track}: drop {key:#010x} has no start locator");
    };
    let all = ssx_assets::terrain::track_file_names(track_dir)?;
    let chosen = ssx_assets::terrain::select_drop_files(&all, Some(&start.terrain_files));
    let mut drawn: Vec<String> = Vec::with_capacity(chosen.len());
    for name in &start.terrain_files {
        if let Some(file) = chosen.iter().find(|f| f.eq_ignore_ascii_case(name))
            && !drawn.iter().any(|d| d == file)
        {
            drawn.push((*file).to_owned());
        }
    }
    info!("terrain: drop {key:#010x} draws {} of {} files {drawn:?}", drawn.len(), all.len());
    Ok(Some(drawn))
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
    content.set_sim_policy(ride_policy(cli, &content));
    let sim_content_id = cli
        .event_name
        .as_deref()
        .filter(|_| !session)
        .and_then(|event| ssx_assets::content::data::simid::result_sim_id(big_root, track, event));
    let drop =
        drop_key.and_then(|key| spawn::load_drop_starts(big_root, track).ok()?.iter().position(|s| s.drop_key == key));
    let world_and_spawn = || -> Result<_> {
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
        Ok((world, spawn))
    };
    let (world, looks) = crate::load_pool::join("ride looks", world_and_spawn, || {
        ride_looks(cli, character, big_root, level, Some(track))
    });
    let (world, spawn) = world?;
    let looks = looks?;
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
    Ok(setup_with_looks(cli, big_root, w, looks))
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
    let looks = ride_looks(cli, character, big_root, level, w.vault_track)?;
    Ok(setup_with_looks(cli, big_root, w, looks))
}

pub(super) struct RideLooks {
    board: Option<rider_visual::LoadedBoard>,
    game_setup: Option<ssx_physics::gear::GameSetup>,
    stand_in: String,
    wire_key: u32,
    rider: Option<rider_skin::LoadedRider>,
    decision: ssx_assets::content::characters::budget::RiderLooks,
}

pub(super) fn ride_looks(
    cli: &RideOptions,
    character: &str,
    big_root: &Path,
    level: Option<&ssx_assets::content::level::LevelPackages>,
    vault_track: Option<&str>,
) -> Result<RideLooks> {
    let session = cli.live.is_some();
    let policy = ssx_assets::content::GameContent::of(big_root)
        .session_policy()
        .unwrap_or(ssx_content::policy::ClientMods::SESSION_DEFAULT);
    let board = rider_visual::load_board(big_root, vault_track)
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
    let rider = rider_skin::load_rider(big_root, vault_track, &rr.request)
        .inspect_err(|e| warn!("character {character} unavailable, drawing a placeholder capsule: {e:#}"))
        .ok()
        .map(|mut r| {
            r.riders = Some(rr.decision.clone());
            r
        });
    Ok(RideLooks { board, game_setup, stand_in, wire_key, rider, decision: rr.decision })
}

fn setup_with_looks(
    cli: &RideOptions,
    big_root: &Path,
    w: RideWorld<'_>,
    looks: RideLooks,
) -> (ride::RideSetup, ssx_assets::content::characters::budget::RiderLooks) {
    let RideLooks { board, game_setup, stand_in, wire_key, rider, decision } = looks;
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
        steer_from: cli.steer_from,
        grab: cli.grab,
        tweak: cli.tweak,
        shoulders: cli.shoulders,
        boost: cli.boost,
        boost_meter: cli.boost_meter,
        screenshot_steps: cli.screenshot_steps,
        board,
        rider,
        fly_camera: cli.fly_camera,
        load_secs: w.load_secs,
        event_key: cli.event_key,
        free_ride: cli.free_ride,
        collected_geotags: cli.collected_geotags.clone(),
        game: cli.game,
        classic: cli.classic,
        autoplay: cli.autoplay,
        setup: game_setup,
        intro_visits: cli.intro_visits,
        own_tombstone: cli.own_tombstone.clone(),
        own_ghost: cli.own_ghost.clone(),
        live: cli.live,
        game_mode: cli.game_mode,
        sim_content_id: w.sim_content_id,
        map: w.map,
    };
    (setup, decision)
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Registration {
    pub wireframe: bool,
    pub view_distance: ssx_assets::port_options::ViewDistance,
    pub viewer_hud: bool,
    pub shadow_quality: ssx_assets::port_options::ShadowQuality,
    pub post_levels: crate::render::post::PostLevels,
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

pub fn task_pool_plugin() -> bevy::app::TaskPoolPlugin {
    let mut task_pool_options = bevy::app::TaskPoolOptions::default();
    let threads = compute_threads(bevy::tasks::available_parallelism());
    task_pool_options.compute.min_threads = threads;
    task_pool_options.compute.max_threads = threads;
    bevy::app::TaskPoolPlugin { task_pool_options }
}

fn compute_threads(cpus: usize) -> usize {
    // PORT(choice): the compute pool gets every CPU but one (the main thread's), at most four, at least one. Bevy
    // 0.20's default gives the I/O and the async-compute pools 25 % of the CPUs each (rounded, 1–4 threads) and the
    // compute pool the rest: 2 threads with 4 or 6 CPUs, 8 with 16 (here 3, 4 and 4). During a ride the main world's
    // systems, the render world's, the render graph's, the command encoders' finish and the ride's workers share the
    // compute pool; the async pool's threads run 0.00 ms of CPU a frame. Measured on the dev laptop (Core Ultra 7 255H:
    // 6 P-, 8 E- and 2 LP-E-cores) with the swap chain's image taken after the present (`render::swap_chain`). With
    // two threads the render graph meets the main update's systems on both: 4 P-cores, alps0101, `render_system` 2.32
    // ms with Bevy's own acquire, 2.83 with 2 threads, 2.46–2.47 with 3; 6 P-cores, 4 threads against 2, Mount Eddie's
    // p99 11.94–11.99 -> 10.46–10.54 ms. More threads look for work, take the E-cores and lower the clocks: 6 P-cores,
    // 5 threads against 4, the main thread 6.37–6.43 -> 6.04–6.19 ms; all 16 CPUs, Bevy's 8 against 4, alps0101's
    // frame 7.94–8.04 -> 7.29–7.35 ms, and without the acquire after the present the pool's CPU 21.1–21.3 -> 14.4–14.7
    // ms a frame, the main thread on a P-core in 62–65 -> 78–79 % of its samples. Engine mechanics: the console's
    // threads were the game's own.
    cpus.saturating_sub(1).clamp(1, 4)
}

pub fn log_plugin() -> bevy::log::LogPlugin {
    bevy::log::LogPlugin {
        fmt_layer: |app| {
            match ssx_core::output::start() {
                Ok(running) => {
                    app.insert_resource(LogWriter { _running: running });
                }
                Err(e) => {
                    ssx_core::eprintln!("log: the output's writer thread did not start ({e}): written where said")
                }
            }
            Some(Box::new(
                bevy::log::tracing_subscriber::fmt::Layer::default()
                    .with_writer(ssx_core::text::log::QueuedStderr)
                    .fmt_fields(ssx_core::text::log::EscapedFields),
            ))
        },
        ..default()
    }
}

#[derive(Resource)]
struct LogWriter {
    _running: ssx_core::output::Running,
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
    app.insert_resource(crate::render::terrain::ViewDistance(r.view_distance))
        .insert_resource(crate::render::shadows::ShadowSettings(r.shadow_quality))
        .insert_resource(r.post_levels);
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
        crate::render::device_poll::DevicePollPlugin,
        crate::render::swap_chain::SwapChainPlugin,
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
    crate::shell::device_loss::plugin(app);
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

// PORT(choice): a ride on an `exp_` ghost event whose racers did not load (`BotData::load` failed: the rig bundle or
// the vaults missing) draws its rivals as the viewer's holograms, with their medal labels; the console's disc always
// holds the racers' data, so its explore rivals are always racers.
fn loads_ghost_holograms(ride: Option<&ssx_physics::session::Session>) -> bool {
    ride.is_none_or(|s| {
        s.bots.is_none()
            && !s.event.setup.free_ride
            && ssx_physics::event::shows_explore_ghosts(s.event.setup.event_type)
    })
}

#[cfg(test)]
mod tests {
    use super::loads_ghost_holograms;

    #[cfg(target_os = "linux")]
    fn written_by_this_thread() -> u64 {
        let io = std::fs::read_to_string("/proc/thread-self/io").expect("/proc/thread-self/io (the task's I/O counts)");
        io.lines()
            .find_map(|l| l.strip_prefix("wchar:"))
            .and_then(|v| v.trim().parse().ok())
            .expect("a wchar line in /proc/thread-self/io")
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn the_thread_that_logs_writes_none_of_the_log() {
        let mut app = bevy::prelude::App::new();
        app.add_plugins(super::log_plugin());
        let before = written_by_this_thread();
        for i in 0..200 {
            bevy::log::info!("io1: a log line {i}");
            bevy::log::warn!("io1: a warning {i}");
            ssx_core::eprintln!("io1: a printed line {i}");
        }
        let on_this_thread = written_by_this_thread() - before;
        drop(app);
        assert_eq!(on_this_thread, 0, "the logging thread wrote {on_this_thread} bytes of its lines itself");
    }

    use ssx_physics::event::EventType;
    use ssx_physics::session::Session;
    use ssx_physics::session::bots::BotRace;

    #[test]
    fn a_ride_beside_racers_loads_no_holograms() {
        assert!(loads_ghost_holograms(None));
        let ride = |t: Option<EventType>, racers: Option<BotRace>, free_ride: bool| {
            let mut s = Session::default();
            s.event.setup.event_type = t;
            s.event.setup.free_ride = free_ride;
            s.bots = racers;
            loads_ghost_holograms(Some(&s))
        };
        for t in [EventType::RaceGhosts, EventType::TrickGhosts] {
            assert!(!ride(Some(t), Some(BotRace::default()), false), "{t:?} with racers");
            assert!(!ride(Some(t), Some(BotRace { running: true, ..BotRace::default() }), false), "{t:?} live");
            assert!(!ride(Some(t), None, true), "{t:?} free ride");
            assert!(ride(Some(t), None, false), "{t:?} without racers");
        }
        for t in [EventType::RaceBots, EventType::TrickBots, EventType::SurviveToDeath, EventType::SurviveToBottom] {
            assert!(!ride(Some(t), None, false), "{t:?}");
        }
        assert!(!ride(None, None, false));
    }

    #[test]
    fn the_compute_pool_takes_every_cpu_but_one_at_most_four() {
        let threads = |cpus| super::compute_threads(cpus);
        assert_eq!([1, 2, 3, 4, 5, 6, 16, 64].map(threads), [1, 1, 2, 3, 4, 4, 4, 4]);
    }
}
