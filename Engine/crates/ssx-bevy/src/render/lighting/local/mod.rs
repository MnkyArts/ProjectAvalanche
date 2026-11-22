use bevy::asset::{RenderAssetUsages, uuid_handle};
use bevy::ecs::system::SystemParam;
use bevy::math::Affine3A;
use bevy::prelude::*;
use bevy::render::render_asset::RenderAssets;
use bevy::render::renderer::RenderQueue;
use bevy::render::storage::{GpuShaderBuffer, ShaderBuffer};
use bevy::render::{ExtractSchedule, MainWorld, Render, RenderApp, RenderSystems};
use bevy::shader::load_shader_library;
use ssx_assets::light_script::LightBehaviour;
use ssx_assets::track_lights::SceneLight;
use ssx_physics::controllers::{Controllers, EntityController, LightRun, LightUpdate, OfflineHost};

use crate::render::lighting::game::GameLightUniform;
use crate::render::lighting::{BigRoot, GameLighting};
use encode::{Grid, Words, assign_layers, bin_of, bounds, collect, encode, record, union};
use textures::{blank_array, texture_array};

mod encode;
mod textures;

pub const LOCAL_LIGHT_BUFFER: Handle<ShaderBuffer> = uuid_handle!("7c1d6a3e-2f4b-4e8a-9b51-0c3d2e1f4a77");
pub const LOCAL_LIGHT_TEXTURES: Handle<Image> = uuid_handle!("3e9b1c55-7a2d-4f0e-8c64-91d2a5b7e013");
pub const LOCAL_LIGHTS_BINDING: u32 = 140;

fn sphere_cos() -> f32 {
    (0.314_159_274_101_257_3_f64).cos() as f32
}

const CELL: f32 = 32.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Kind {
    Point = 0,
    Spot = 1,
    Box = 2,
    Textured = 3,
}

struct Placed {
    world: Affine3A,
    light: SceneLight,
    factor: Option<f32>,
    ctrl: Option<(u32, [u32; 3])>,
    node: Affine3A,
    scale: Vec3,
    order: (u32, u32, usize, usize, usize),
    behaviour: Option<LightBehaviour>,
    entity: u32,
    texture: Option<Projected>,
    texture_layer: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Projected {
    File(String),
    Text { string: String, colour: [u32; 3] },
}

const TEXT_LAYER: u32 = 8;

pub struct LocalLightsPlugin;

impl Plugin for LocalLightsPlugin {
    fn build(&self, app: &mut App) {
        load_shader_library!(app, "local_lights.wesl");
        app.init_resource::<DynamicLocalLights>()
            .init_resource::<DynamicLightReserve>()
            .init_resource::<LiveLights>()
            .add_systems(crate::shell::level::LevelPreStartup, setup_local_lights)
            .add_systems(
                PostUpdate,
                (follow_controllers, upload_dynamic_lights.in_set(DynamicLightsUpload))
                    .chain()
                    .before(bevy::asset::AssetEventSystems),
            );
        crate::shell::level::scoped::<LocalLightBytes>(app);
        crate::shell::level::scoped::<PendingLocalLights>(app);
        use crate::render::lighting::fade::{LightingFadeSet, faded};
        app.add_systems(Update, follow_lighting.after(LightingFadeSet).run_if(faded));
        app.init_resource::<LocalLightPatches>();
        if let Some(render_app) = app.get_sub_app_mut(RenderApp) {
            render_app
                .init_resource::<LocalLightPatches>()
                .add_systems(ExtractSchedule, take_patches)
                .add_systems(Render, write_patches.in_set(RenderSystems::PrepareResources));
        }
    }
}

// PORT(choice): the per-frame changes of the light buffer (the light-track clock, the dynamic records and their bounds,
// a cross-fade's light constants) are written into the GPU buffer in place (`RenderQueue::write_buffer` of those
// vec4s); the whole buffer (490 KB on rockies0706, uploaded every frame before: 0.19 ms of the main thread, 0.10 of the
// extract, 0.24 of the render thread's asset preparation) is uploaded only when the placed lights are encoded again.
// The console wrote each light's constants into the command buffer as it drew its volume
// (LightsRenderHelper_DrawVolumes 0x829C6470).
#[derive(Resource, Default)]
struct LocalLightPatches(Vec<(usize, Vec<[u32; 4]>)>);

fn upload_all(words: &Words, buffers: &mut Assets<ShaderBuffer>, patches: &mut LocalLightPatches) {
    if let Some(mut buffer) = buffers.get_mut(&LOCAL_LIGHT_BUFFER) {
        buffer.clear();
        buffer.extend_from_slice(&words.bytes());
    }
    patches.0.clear();
}

fn upload_range(
    words: &Words,
    range: std::ops::Range<usize>,
    buffers: &mut Assets<ShaderBuffer>,
    patches: &mut LocalLightPatches,
) {
    let Some(vec4s) = words.0.get(range.clone()) else { return };
    if vec4s.is_empty() {
        return;
    }
    let Some(data) = buffers.get_mut_untracked(&LOCAL_LIGHT_BUFFER).and_then(|b| b.cast_slice_mut::<u8>()) else {
        return;
    };
    let Some(dst) = data.get_mut(range.start * 16..range.end * 16) else { return };
    for (d, w) in dst.as_chunks_mut::<4>().0.iter_mut().zip(vec4s.iter().flatten()) {
        *d = w.to_le_bytes();
    }
    patches.0.push((range.start, vec4s.to_vec()));
}

fn take_patches(mut main: ResMut<MainWorld>, mut render: ResMut<LocalLightPatches>) {
    if let Some(mut queued) = main.get_resource_mut::<LocalLightPatches>() {
        render.0.append(&mut queued.0);
    }
}

fn write_patches(
    mut patches: ResMut<LocalLightPatches>,
    buffers: Res<RenderAssets<GpuShaderBuffer>>,
    queue: Res<RenderQueue>,
) {
    let Some(gpu) = buffers.get(&LOCAL_LIGHT_BUFFER) else {
        patches.0.clear();
        return;
    };
    for (at, vec4s) in patches.0.drain(..) {
        let bytes: Vec<u8> = vec4s.iter().flatten().flat_map(|w| w.to_le_bytes()).collect();
        let offset = (at * 16) as u64;
        if offset + bytes.len() as u64 <= gpu.buffer.size() {
            queue.write_buffer(&gpu.buffer, offset, &bytes);
        }
    }
}

pub fn reset_level(world: &mut World) {
    world.insert_resource(DynamicLocalLights::default());
    world.insert_resource(DynamicLightReserve::default());
    world.insert_resource(LiveLights::default());
}

#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DynamicLightsUpload;

#[derive(Resource, Default)]
pub struct DynamicLightReserve(pub u32);

#[derive(Resource, Default)]
pub struct DynamicLocalLights {
    pub lights: Vec<DynamicLight>,
}

#[derive(Clone, Debug)]
pub struct DynamicLight {
    pub world: Affine3A,
    pub light: SceneLight,
    pub factor: f32,
}

#[derive(Resource)]
struct LocalLightBytes {
    words: Words,
    dynamic_at: usize,
    slots: u32,
    grid: Grid,
    last_count: usize,
    reported_count: usize,
    last_clock: (u32, u32),
    animated: bool,
    placed: Vec<Placed>,
    light: GameLightUniform,
    entities: ControlledEntities,
}

const LIGHT_WORDS: usize = 11;

fn light_words(l: &GameLightUniform) -> [[u32; 4]; LIGHT_WORDS] {
    let v = [
        l.ramp_lit,
        l.ramp_shade,
        l.ramp_bounce,
        l.light_dir,
        l.bounce_dir,
        l.rim_shadow,
        l.specular,
        l.fog_color,
        l.fog_data,
        l.sparkle,
        l.misc,
    ];
    v.map(|v| v.to_array().map(f32::to_bits))
}

fn follow_lighting(
    lighting: Res<GameLighting>,
    bytes: Option<ResMut<LocalLightBytes>>,
    mut buffers: ResMut<Assets<ShaderBuffer>>,
    mut patches: ResMut<LocalLightPatches>,
) {
    let Some(mut bytes) = bytes else { return };
    bytes.light = GameLightUniform::from_preset(&lighting);
    let at = bytes.words.0[3][3] as usize;
    let block = light_words(&bytes.light);
    bytes.words.0[at..at + LIGHT_WORDS].copy_from_slice(&block);
    upload_range(&bytes.words, at..at + LIGHT_WORDS, &mut buffers, &mut patches);
}

#[derive(Resource, Default)]
pub struct LiveLights {
    offline: Option<(Box<Controllers>, OfflineHost)>,
    pending: f32,
    clock: (f32, u32),
    warned: bool,
}

pub const DYNAMIC_SLOTS: u32 = 4;

pub struct LoadedLocalLights {
    placed: Vec<Placed>,
    summary: String,
    image: Option<Image>,
    recipe: Option<crate::render::reload::Recipe>,
    offline: Option<Controllers>,
    secs: f32,
}

impl LoadedLocalLights {
    pub fn queue_image(&mut self, uploads: &mut crate::render::uploads::LevelUploads, images: &mut Assets<Image>) {
        if let Some(image) = self.image.take() {
            uploads.add_image_at(images, LOCAL_LIGHT_TEXTURES.id(), image);
        }
    }
}

#[derive(Resource)]
struct PendingLocalLights(Option<LoadedLocalLights>);

pub fn insert(world: &mut World, loaded: LoadedLocalLights) {
    world.insert_resource(PendingLocalLights(Some(loaded)));
}

// PORT(choice): the local lights are read, placed into their texture layers and their projected textures decoded on
// the level's loading thread, beside its other parts after the ride, not in the level's startup on the game thread,
// where they took 7-47 ms of the spawn frame (alps0101, the heli drop, Mount Eddie, the online round's level;
// development laptop, `trace_chrome`); the startup encodes them as before with the dynamic records the level reserves.
// They read the same files after the session's content policy is set, and draw from no generator. The console's
// loading is not decoded.
pub fn load_level(
    root: Option<&std::path::Path>,
    lighting: Option<&GameLighting>,
    ride: Option<&crate::ride::Ride>,
) -> LoadedLocalLights {
    let t0 = std::time::Instant::now();
    // PORT(choice): the Local Lights switch (`settings.json`'s `local_lights`, the launcher's `SSX_LOCAL_LIGHTS=off`) is
    // the port's, for slower machines; off, it keeps the placed lights that show the player something — the textured box
    // lights, which project the course markings (the 27 `markings_*` files: jumps, banks, ends, heights, skulls) and the
    // warning texts ("USE CAUTION", "UNSTABLE", "CREVASSE AHEAD", "AVALANCHE") onto the snow — drawn as the game draws
    // them, and drops the lights that only light the scene (the point, spot and plain box lights, the markers' and
    // bounces' flicker lights); the gear's lights (the head lamp, Tricky's flares, the helicopter's) are dynamic slots
    // and stay. The console has no such switch.
    let markings_only = std::env::var("SSX_LOCAL_LIGHTS").is_ok_and(|v| v == "off");
    let map: Vec<(u32, ssx_assets::track_objects::Placement)> = ride
        .and_then(|r| r.map.as_ref())
        .map_or_else(Vec::new, |m| m.placed.iter().map(|&(_, key, p)| (key, p)).collect());
    let lights = match (lighting, root) {
        (Some(l), Some(r)) => match l.disc_track() {
            Some(track) => collect(r, Some(track), &map),
            None if !map.is_empty() => collect(r, None, &map),
            None => Ok((Vec::new(), "local lights: a map's new track places none".to_string())),
        },
        _ => Ok((Vec::new(), "local lights: no track lighting".to_string())),
    };
    let (mut placed, mut summary) = lights.unwrap_or_else(|e| {
        warn!("local lights: {e:#}");
        (Vec::new(), format!("local lights unavailable: {e:#}"))
    });
    if markings_only {
        placed.retain(|p| p.texture.is_some());
        summary.push_str(&format!(" | Local Lights off: the {} projected markings and texts only", placed.len()));
    }
    let paths = assign_layers(&mut placed);
    let textures = match (root, paths.is_empty()) {
        (Some(r), false) => texture_array(r, &paths).map(Some),
        _ => Ok(None),
    };
    let mut recipe = None;
    let image = match textures {
        Ok(Some(image)) => {
            if let Some(r) = root {
                let ids = paths
                    .iter()
                    .filter_map(|p| match p {
                        Projected::File(rel) => ssx_assets::content::textures::texture_id(rel),
                        Projected::Text { .. } => None,
                    })
                    .collect();
                let (big, all) = (r.to_path_buf(), paths.clone());
                recipe = Some(crate::render::reload::Recipe::new(ids, move || texture_array(&big, &all)));
            }
            image
        }
        Ok(None) => blank_array(),
        Err(e) => {
            warn!("local light textures: {e:#}");
            summary.push_str(" | projected textures undecodable");
            blank_array()
        }
    };
    let mut offline = None;
    if let (Some(l), Some(r)) = (lighting, root)
        && let Some(track) = l.disc_track()
    {
        let drop = (l.drop_count > 0).then_some(l.drop_key);
        match ssx_physics::spawn::load_trigger_volumes(r, track, drop) {
            Ok(t) => offline = Some(Controllers::offline(&t)),
            Err(e) => warn!("local lights: the track's entities: {e:#}"),
        }
    }
    LoadedLocalLights { placed, summary, image: Some(image), recipe, offline, secs: t0.elapsed().as_secs_f32() }
}

#[derive(SystemParam)]
struct LevelLights<'w> {
    pending: Option<ResMut<'w, PendingLocalLights>>,
    lighting: Option<Res<'w, GameLighting>>,
    root: Option<Res<'w, BigRoot>>,
    ride: Option<Res<'w, crate::ride::Ride>>,
}

impl LevelLights<'_> {
    fn take(&mut self) -> LoadedLocalLights {
        self.pending.as_mut().and_then(|p| p.0.take()).unwrap_or_else(|| {
            load_level(self.root.as_deref().map(|r| r.0.as_path()), self.lighting.as_deref(), self.ride.as_deref())
        })
    }
}

fn setup_local_lights(
    mut commands: Commands,
    reserve: Res<DynamicLightReserve>,
    mut level: LevelLights,
    mut buffers: ResMut<Assets<ShaderBuffer>>,
    mut images: ResMut<Assets<Image>>,
) {
    let t0 = std::time::Instant::now();
    let LoadedLocalLights { placed, summary, image, recipe, offline, secs } = level.take();
    let lighting = level.lighting.as_deref();
    if let Some(recipe) = recipe {
        crate::render::reload::track(LOCAL_LIGHT_TEXTURES.id(), recipe);
    }
    let slots = DYNAMIC_SLOTS + reserve.0;
    let light = lighting.map_or_else(GameLightUniform::viewer_default, GameLightUniform::from_preset);
    let grid = Grid::around(&placed);
    let (words, dynamic_at) = encode(&placed, &grid, slots, &light);
    if let Err(e) =
        buffers.insert(LOCAL_LIGHT_BUFFER.id(), ShaderBuffer::new(words.bytes(), RenderAssetUsages::default()))
    {
        warn!("local light buffer: {e:?}");
    }
    if let Some(c) = offline {
        commands.insert_resource(LiveLights {
            offline: Some((Box::new(c), OfflineHost::default())),
            ..LiveLights::default()
        });
    }
    commands.insert_resource(LocalLightBytes {
        words,
        dynamic_at,
        slots,
        grid,
        last_count: 0,
        reported_count: 0,
        last_clock: (0, 0),
        animated: false,
        placed,
        light,
        entities: ControlledEntities::default(),
    });
    if let Some(image) = image
        && let Err(e) = images.insert(LOCAL_LIGHT_TEXTURES.id(), image)
    {
        warn!("local light texture array: {e:?}");
    }
    info!("{summary} (read in {secs:.3} s, encoded in {:.3} s)", t0.elapsed().as_secs_f32());
}

fn follow_controllers(
    ride: Option<Res<crate::ride::Ride>>,
    time: Res<Time>,
    mut live: ResMut<LiveLights>,
    bytes: Option<ResMut<LocalLightBytes>>,
    mut buffers: ResMut<Assets<ShaderBuffer>>,
    mut patches: ResMut<LocalLightPatches>,
) {
    let Some(mut bytes) = bytes else { return };
    let live = &mut *live;
    let controllers: Option<&Controllers> = match ride.as_deref() {
        Some(r) => r.session.controllers.as_ref(),
        None => match live.offline.as_mut() {
            Some((c, host)) => {
                live.pending += time.delta_secs();
                let n = (live.pending * 30.0).floor();
                live.pending -= n / 30.0;
                for _ in 0..(n as u32).min(64) {
                    c.update_offline(1.0 / 30.0, host);
                }
                Some(&**c)
            }
            None => None,
        },
    };
    let entities = controllers.map_or(&[][..], |c| &c.entities);
    let (mut lo, mut hi) = ((f32::MAX, u32::MAX), (f32::MIN, 0u32));
    for e in entities.iter().filter(|e| e.owner.is_none()) {
        let Some(run) = running_light(e) else { continue };
        if matches!(run.update, LightUpdate::Blink { .. } | LightUpdate::Flicker { .. }) {
            (lo, hi) = ((lo.0.min(run.time), lo.1.min(run.updates)), (hi.0.max(run.time), hi.1.max(run.updates)));
        }
    }
    if lo.0 <= hi.0 {
        if (hi.0 - lo.0 > 1e-6 || hi.1 != lo.1) && !live.warned {
            warn!(
                "local lights: the running light tracks' clocks differ ({lo:?} … {hi:?}); the shader takes the latest"
            );
            live.warned = true;
        }
        live.clock = hi;
    }
    let bytes = &mut *bytes;
    bytes.entities.follow(entities.iter().map(ControlledEntities::key), &bytes.placed);
    let mut changed = false;
    for (i, of) in &bytes.entities.of_light {
        let p = &mut bytes.placed[*i];
        let run = of.iter().rev().find_map(|&e| running_light(&entities[e]));
        let b = match run.map(|r| &r.update) {
            None => LightBehaviour::Off,
            Some(&LightUpdate::Blink { strength, frequency, gap }) => {
                LightBehaviour::Blink { strength, frequency, gap }
            }
            Some(&LightUpdate::Flicker { strength }) => LightBehaviour::Flicker { strength },
            Some(_) => LightBehaviour::On,
        };
        changed |= p.behaviour != Some(b);
        p.behaviour = Some(b);
        if let Some(e) = of.iter().rev().map(|&e| &entities[e]).find(|e| e.body.is_some()) {
            let world = Affine3A::from_scale_rotation_translation(p.scale, e.rotation, e.position) * p.node;
            if world != p.world {
                p.world = world;
                changed = true;
            }
        }
    }
    if changed {
        bytes.reencode((live.clock.0.to_bits(), live.clock.1));
        upload_all(&bytes.words, &mut buffers, &mut patches);
    }
}

fn running_light(e: &EntityController) -> Option<&LightRun> {
    e.fx.lights.get("").filter(|r| !r.group && r.on)
}

// PORT(choice): the entities' keys are compared with the ones indexed each frame and the index made again only when
// they differ; before, two hash maps of every entity were built each frame (0.18 ms of the main thread on
// rockies0706). The console's light component holds its lights (`DynamicLightsEntity`, 0x8297EAA8) and needs no
// lookup.
type EntityKey = (u32, [u32; 3]);

#[derive(Default)]
struct ControlledEntities {
    keys: Option<Vec<Option<EntityKey>>>,
    of_light: Vec<(usize, Vec<usize>)>,
}

impl ControlledEntities {
    fn key(e: &EntityController) -> Option<EntityKey> {
        e.owner.is_none().then(|| (e.object, e.placement_position.to_array().map(f32::to_bits)))
    }

    fn follow<K>(&mut self, keys: K, placed: &[Placed])
    where
        K: ExactSizeIterator<Item = Option<EntityKey>> + Clone,
    {
        if let Some(indexed) = &self.keys
            && indexed.len() == keys.len()
            && indexed.iter().zip(keys.clone()).all(|(k, e)| *k == e)
        {
            return;
        }
        let keys: Vec<_> = keys.collect();
        let mut by_key: std::collections::HashMap<EntityKey, Vec<usize>> = Default::default();
        for (i, k) in keys.iter().enumerate() {
            if let Some(k) = k {
                by_key.entry(*k).or_default().push(i);
            }
        }
        self.of_light = placed
            .iter()
            .enumerate()
            .filter_map(|(i, p)| p.ctrl.map(|k| (i, by_key.get(&k).cloned().unwrap_or_default())))
            .collect();
        self.keys = Some(keys);
    }
}

static DYNAMIC_OFF: std::sync::OnceLock<bool> = std::sync::OnceLock::new();

fn upload_dynamic_lights(
    dynamic: Res<DynamicLocalLights>,
    clock: Res<LiveLights>,
    bytes: Option<ResMut<LocalLightBytes>>,
    mut buffers: ResMut<Assets<ShaderBuffer>>,
    mut patches: ResMut<LocalLightPatches>,
) {
    let Some(mut bytes) = bytes else { return };
    let tracks = (clock.clock.0.to_bits(), clock.clock.1);
    let off = *DYNAMIC_OFF.get_or_init(|| crate::render::post::env_is("SSX_DYNAMIC_LIGHTS", "off"));
    let none: &[DynamicLight] = &[];
    let requested = if off { none } else { &dynamic.lights[..] };
    let Some(written) = bytes.write_dynamic(requested, tracks) else { return };
    if let Some(count) = bytes.count_to_report() {
        info!(
            "local lights: {count} dynamic record(s) of {} reserved ({} requested)",
            bytes.slots,
            dynamic.lights.len()
        );
    }
    for range in written {
        upload_range(&bytes.words, range, &mut buffers, &mut patches);
    }
}

impl LocalLightBytes {
    fn reencode(&mut self, tracks: (u32, u32)) {
        let (mut words, dynamic_at) = encode(&self.placed, &self.grid, self.slots, &self.light);
        (words.0[2][3], words.0[3][2]) = tracks;
        self.animated = self
            .placed
            .iter()
            .any(|p| matches!(p.behaviour, Some(LightBehaviour::Flicker { .. } | LightBehaviour::Blink { .. })));
        (self.words, self.dynamic_at, self.last_clock, self.last_count) = (words, dynamic_at, tracks, 0);
    }

    fn write_dynamic(&mut self, requested: &[DynamicLight], tracks: (u32, u32)) -> Option<[std::ops::Range<usize>; 3]> {
        if requested.is_empty() && self.last_count == 0 && (!self.animated || tracks == self.last_clock) {
            return None;
        }
        (self.words.0[2][3], self.words.0[3][2]) = tracks;
        self.last_clock = tracks;
        let mut lights: Vec<Placed> = requested
            .iter()
            .take(self.slots as usize)
            .map(|d| Placed {
                world: d.world,
                light: d.light.clone(),
                factor: Some(d.factor),
                ctrl: None,
                node: Affine3A::IDENTITY,
                scale: Vec3::ONE,
                order: (0, 1, 0, 0, 0),
                behaviour: None,
                entity: 0,
                texture: None,
                texture_layer: 0,
            })
            .collect();
        lights.sort_by_key(bin_of);
        let mut w = Words::default();
        lights.iter().for_each(|p| record(&mut w, p));
        let union_at = self.words.0[3][3] as usize + LIGHT_WORDS;
        let reach: Vec<[f32; 4]> = lights.iter().map(bounds).collect();
        self.words.0[union_at] = union(&reach).map(f32::to_bits);
        for (k, b) in reach.iter().enumerate() {
            self.words.0[union_at + 1 + k] = b.map(f32::to_bits);
        }
        let at = self.dynamic_at;
        self.words.0[3][1] = lights.len() as u32;
        self.words.0[at..at + w.0.len()].copy_from_slice(&w.0);
        self.last_count = lights.len();
        Some([2..4, at..at + w.0.len(), union_at..union_at + 1 + lights.len()])
    }

    fn count_to_report(&mut self) -> Option<usize> {
        (self.last_count != self.reported_count).then(|| {
            self.reported_count = self.last_count;
            self.last_count
        })
    }
}

#[cfg(test)]
mod tests {
    use ssx_assets::track_lights::LightShape;

    use super::*;

    fn bytes() -> LocalLightBytes {
        let light = GameLightUniform::default();
        let grid = Grid::around(&[]);
        let (words, dynamic_at) = encode(&[], &grid, DYNAMIC_SLOTS, &light);
        LocalLightBytes {
            words,
            dynamic_at,
            slots: DYNAMIC_SLOTS,
            grid,
            last_count: 0,
            reported_count: 0,
            last_clock: (0, 0),
            animated: false,
            placed: Vec::new(),
            light,
            entities: ControlledEntities::default(),
        }
    }

    fn dynamic(x: f32) -> DynamicLight {
        DynamicLight {
            world: Affine3A::from_translation(Vec3::new(x, 0.0, 0.0)),
            light: SceneLight {
                name: String::new(),
                debug_name: String::new(),
                color: [1.0; 4],
                diffuse_intensity: 1.0,
                specular_intensity: 0.0,
                fall_off: 1.0,
                enable_specular: false,
                enable: true,
                shape: LightShape::Point { outer_radius: 5.0, inner_radius: 1.0 },
            },
            factor: 1.0,
        }
    }

    #[test]
    fn reencoding_rewrites_the_dynamic_records_and_reports_count_changes_only() {
        let mut b = bytes();
        let two = [dynamic(1.0), dynamic(2.0)];
        assert!(b.write_dynamic(&two, (0, 0)).is_some());
        assert_eq!(b.words.0[3][1], 2);
        assert_eq!(b.count_to_report(), Some(2));
        let at = b.words.0[3][3] as usize + LIGHT_WORDS;
        let f = |v: [u32; 4]| v.map(f32::from_bits);
        let each = [f(b.words.0[at + 1]), f(b.words.0[at + 2])];
        assert_eq!((each[0][0], each[1][0]), (1.0, 2.0));
        assert!(each.iter().all(|e| e[3] > 5.0 && e[3] < 5.01), "{each:?}");
        assert_eq!(f(b.words.0[at]), union(&each));
        let written = b.words.0.clone();
        for frame in 1..5 {
            b.reencode((0, 0));
            assert_eq!(b.words.0[3][1], 0, "the encoded buffer holds no dynamic record");
            assert!(b.write_dynamic(&two, (0, 0)).is_some());
            assert_eq!(b.words.0, written, "frame {frame}: the same buffer as before the encode");
            assert_eq!(b.count_to_report(), None, "frame {frame}: the count did not change");
        }
        assert!(b.write_dynamic(&two[..1], (4, 4)).is_some());
        assert_eq!(b.count_to_report(), Some(1));
        assert!(b.write_dynamic(&[], (4, 4)).is_some());
        assert_eq!((b.words.0[3][1], b.count_to_report()), (0, Some(0)));
        assert!(b.write_dynamic(&[], (5, 5)).is_none());
        b.reencode((6, 6));
        assert!(b.write_dynamic(&[], (6, 6)).is_none());
        assert_eq!(b.count_to_report(), None);
    }

    #[test]
    fn the_controlled_lights_find_their_entities() {
        let light = |ctrl: Option<(u32, [u32; 3])>| Placed {
            world: Affine3A::IDENTITY,
            light: dynamic(0.0).light,
            factor: None,
            ctrl,
            node: Affine3A::IDENTITY,
            scale: Vec3::ONE,
            order: (0, 0, 0, 0, 0),
            behaviour: None,
            entity: 0,
            texture: None,
            texture_layer: 0,
        };
        let (a, b) = ((7, [1, 2, 3]), (9, [4, 5, 6]));
        let placed = [light(None), light(Some(a)), light(Some(b)), light(Some((11, [0; 3])))];
        let mut index = ControlledEntities::default();
        let keys = [Some(b), None, Some(a), Some(b)];
        index.follow(keys.iter().copied(), &placed);
        assert_eq!(index.of_light, vec![(1, vec![2]), (2, vec![0, 3]), (3, vec![])]);
        index.of_light[2].1.push(99);
        index.follow(keys.iter().copied(), &placed);
        assert_eq!(index.of_light[2].1, vec![99]);
        index.follow([Some(a)].into_iter(), &placed);
        assert_eq!(index.of_light, vec![(1, vec![0]), (2, vec![]), (3, vec![])]);
        index.follow([].into_iter(), &placed);
        assert_eq!(index.of_light, vec![(1, vec![]), (2, vec![]), (3, vec![])]);
    }

    #[test]
    fn patches_keep_the_asset_and_the_gpu_copy_equal_to_the_words() {
        let mut b = bytes();
        let mut assets = Assets::<ShaderBuffer>::default();
        let mut patches = LocalLightPatches::default();
        assets
            .insert(LOCAL_LIGHT_BUFFER.id(), ShaderBuffer::new(b.words.bytes(), RenderAssetUsages::default()))
            .expect("the buffer's asset");
        let mut gpu = b.words.bytes();
        let asset_bytes = |assets: &mut Assets<ShaderBuffer>| {
            assets.get_mut_untracked(&LOCAL_LIGHT_BUFFER).and_then(|a| a.cast_slice_mut::<u8>().map(|s| s.to_vec()))
        };
        for (frame, lights) in [vec![dynamic(1.0), dynamic(2.0)], vec![dynamic(3.0)], vec![]].into_iter().enumerate() {
            let written = b.write_dynamic(&lights, (frame as u32 + 1, frame as u32 + 1)).expect("written");
            for range in written {
                upload_range(&b.words, range, &mut assets, &mut patches);
            }
            assert_eq!(asset_bytes(&mut assets), Some(b.words.bytes()), "frame {frame}: the asset's data");
            for (at, vec4s) in patches.0.drain(..) {
                let bytes: Vec<u8> = vec4s.iter().flatten().flat_map(|w| w.to_le_bytes()).collect();
                gpu[at * 16..at * 16 + bytes.len()].copy_from_slice(&bytes);
            }
            assert_eq!(gpu, b.words.bytes(), "frame {frame}: the GPU's copy");
        }
        assert!(b.write_dynamic(&[dynamic(4.0)], (9, 9)).is_some());
        upload_range(&b.words, 2..4, &mut assets, &mut patches);
        upload_all(&b.words, &mut assets, &mut patches);
        assert!(patches.0.is_empty());
        assert_eq!(asset_bytes(&mut assets), Some(b.words.bytes()));
    }
}
