use bevy::asset::embedded_asset;
use bevy::prelude::*;
use bevy::render::render_resource::*;
use bevy::render::renderer::{RenderDevice, RenderQueue};
use bevy::render::view::ExtractedView;
use bevy::render::{Render, RenderApp, RenderStartup, RenderSystems};
use bevy::shader::load_shader_library;

use crate::particles::ParticleFrame;
use crate::render::buffers::QueuedUniform;
use crate::render::post::GamePostCamera;

mod passes;
mod pipelines;

pub use passes::draw;
use pipelines::{Tech, init_pipelines};

const COLOUR_FORMAT: TextureFormat = TextureFormat::Rgba8Unorm;
const DEPTH_FORMAT: TextureFormat = TextureFormat::Depth32Float;
const DEPTH_COPY_FORMAT: TextureFormat = TextureFormat::R32Float;

pub fn build(app: &mut App) {
    load_shader_library!(app, "particle_globals.wesl");
    embedded_asset!(app, "particles.wesl");
    embedded_asset!(app, "particles_pass.wesl");
    let Some(render_app) = app.get_sub_app_mut(RenderApp) else { return };
    render_app
        .init_resource::<ParticleGpu>()
        .add_systems(RenderStartup, init_pipelines)
        .add_systems(Render, prepare.in_set(RenderSystems::PrepareResources));
}

#[derive(Clone, Copy, Debug, Default, ShaderType)]
struct Globals {
    view_from_world: Mat4,
    clip_from_view: Mat4,
    view_from_clip: Mat4,
    world_from_view: Mat4,
    view_prev: Mat4,
    camera_pos: Vec4,
    ramp0: Vec4,
    ramp1: Vec4,
    atten: Vec4,
    fog_data: Vec4,
    fog_color: Vec4,
    sizes: Vec4,
    game_proj: Vec4,
}

#[derive(Clone, Copy, Debug, Default, ShaderType)]
struct DrawUniform {
    world: Mat4,
    time: Vec4,
    stretch: Vec4,
    color_mult: Vec4,
    color_spread: Vec4,
    ramp: [Vec4; 4],
    ramp_times: Vec4,
    size_ramp: Vec4,
    size_ramp_times: Vec4,
    flags: Vec4,
    size_mult: Vec4,
    pivot: Vec4,
    fade: Vec4,
    feather: Vec4,
    light_pos: Vec4,
    light_col: Vec4,
    light_misc: Vec4,
    anim: Vec4,
    anim2: Vec4,
    modulo: Vec4,
    modulo2: Vec4,
    glare: [Vec4; 4],
}

struct Targets {
    pub(super) size: UVec2,
    half: UVec2,
    colour_a: TextureView,
    colour_b: TextureView,
    pub(super) depth: TextureView,
    depth_copy: TextureView,
    depth_full: TextureView,
    dummy: TextureView,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Bin {
    Tonemapped,
    HalfRes,
    FullRes,
}

struct DrawCmd {
    tech: Tech,
    bin: Bin,
    texture: usize,
    offset: u32,
    vertices: u32,
}

struct FrameDraw {
    tech: Tech,
    bin: Bin,
    texture: usize,
    vertices: u32,
    uniform: DrawUniform,
}

#[derive(Default)]
pub struct FrameDraws {
    draws: Vec<FrameDraw>,
    records: Vec<u8>,
    pub drawn: usize,
    pub vertices: usize,
}

// PORT(choice): the draws' uniforms and their records' bytes are made in the particles' update on Bevy's compute pool
// (`update::step`), beside the frame's other systems, and the render world only copies them to the GPU. Made in the
// render world's preparation they took 0.76 ms of the render thread an update on Mount Eddie (a traced build with
// timers, round perf-x1), and the step spent 0.43 ms on each draw's list of named parameters (strings) and 0.29 ms on
// the registry's snapshots. The registry keeps the 42 names the draw uniform reads (`REGISTRY_NAMES`); each draw
// passes its own before its uniform reads them, a draw left out too, as before, so every uniform holds the same
// values. wgpu's buffers are the port's renderer; the console gathered a draw's constants in
// `LynxRender_GatherRenderState` 0x829B4140.
pub fn frame_draws(
    bufs: &crate::particles::sim::DrawBuffers,
    registry: &mut crate::particles::sim::Registry,
    light: &crate::particles::ParticleLight,
    textures: &[crate::particles::ParticleTexture],
    unported: &mut std::collections::HashSet<(String, String)>,
) -> FrameDraws {
    let mut texture_of: std::collections::HashMap<&str, usize> = Default::default();
    for (i, t) in textures.iter().enumerate() {
        texture_of.entry(t.name.as_str()).or_insert(i);
    }
    let mut out = FrameDraws {
        draws: Vec::with_capacity(bufs.draws.len()),
        records: Vec::new(),
        drawn: bufs.draws.len(),
        vertices: bufs.draws.iter().map(|d| d.records.len()).sum(),
    };
    for d in &bufs.draws {
        for &(slot, v) in &bufs.extras[d.extras.clone()] {
            registry[slot] = v;
        }
        let (shader, technique) = (&*d.render.shader, &*d.render.technique);
        let Some(&texture) = texture_of.get(&*d.render.texture) else { continue };
        let Some(tech) = Tech::of(shader, technique) else {
            if unported.insert((shader.to_owned(), technique.to_owned())) {
                warn!("particles: no port of {shader} / {technique}");
            }
            continue;
        };
        let bin = if d.tonemap {
            Bin::Tonemapped
        } else if d.half {
            Bin::HalfRes
        } else {
            Bin::FullRes
        };
        if tech.is_ribbon() != (d.mesh_type == 2) {
            if unported.insert((shader.to_owned(), format!("iType {}", d.mesh_type))) {
                warn!("particles: {shader} with mesh builder {} not ported", d.mesh_type);
            }
            continue;
        }
        let n = d.records.len() as u32;
        let vertices = match tech {
            t if t.is_ribbon() && n < 2 => continue,
            t if t.is_ribbon() => n * 2,
            _ => n * 6,
        };
        let first = (d.records.start * 3) as u32;
        let uniform = draw_uniform(d, registry, first, light, bin == Bin::FullRes);
        out.draws.push(FrameDraw { tech, bin, texture, vertices, uniform });
    }
    out.records = record_bytes(&bufs.records);
    out
}

fn record_bytes(records: &[crate::particles::sim::VertexRecord]) -> Vec<u8> {
    const RECORD: usize = std::mem::size_of::<f32>() * 12;
    let mut bytes = vec![0u8; records.len() * RECORD];
    for (out, record) in bytes.as_chunks_mut::<RECORD>().0.iter_mut().zip(records) {
        for (out, x) in out.as_chunks_mut::<4>().0.iter_mut().zip(record.iter().flat_map(|row| row.to_array())) {
            *out = x.to_le_bytes();
        }
    }
    bytes
}

#[derive(Resource, Default)]
struct ParticleGpu {
    textures: Option<(usize, Vec<TextureView>)>,
    targets: Option<Targets>,
    globals: QueuedUniform<Globals>,
    draws: DynamicUniformBuffer<DrawUniform>,
    records: Option<Buffer>,
    cmds: Vec<DrawCmd>,
    serial: u64,
    view_prev: Option<Mat4>,
}

fn texture(device: &RenderDevice, label: &'static str, size: UVec2, format: TextureFormat, mips: u32) -> Texture {
    device.create_texture(&TextureDescriptor {
        label: Some(label),
        size: Extent3d { width: size.x.max(1), height: size.y.max(1), depth_or_array_layers: 1 },
        mip_level_count: mips,
        sample_count: 1,
        dimension: TextureDimension::D2,
        format,
        usage: TextureUsages::TEXTURE_BINDING | TextureUsages::RENDER_ATTACHMENT | TextureUsages::COPY_DST,
        view_formats: &[],
    })
}

fn view(t: &Texture) -> TextureView {
    t.create_view(&TextureViewDescriptor::default())
}

macro_rules! slot {
    ($name:literal) => {
        const {
            match crate::particles::sim::registry_slot($name) {
                Some(i) => i,
                None => panic!(concat!("not a registry name: ", $name)),
            }
        }
    };
}

fn draw_uniform(
    d: &crate::particles::sim::Draw,
    registry: &crate::particles::sim::Registry,
    first: u32,
    light: &crate::particles::ParticleLight,
    full_res: bool,
) -> DrawUniform {
    let e = |slot: usize| registry[slot];
    let (light_pos, light_col, radius, falloff) = if d.use_global_lighting {
        (light.position, light.colour, light.radius, light.falloff)
    } else {
        (d.light.0, d.light.1, d.light.2, d.light.3)
    };
    DrawUniform {
        world: d.world,
        time: Vec4::new(d.time_now as f32, d.age_max as f32, e(slot!("fTimeSpeed")).x, d.age_warp_inv),
        stretch: d.stretch.extend(d.stretch_per_particle as u8 as f32),
        color_mult: d.color_mult,
        color_spread: e(slot!("vColorMultSpread")),
        ramp: d.color_ramp,
        ramp_times: e(slot!("vColorRampTimes")),
        size_ramp: e(slot!("vSizeRamp")),
        size_ramp_times: e(slot!("vSizeRampTimes")),
        flags: Vec4::new(
            e(slot!("bUseColorRamp")).x,
            e(slot!("bUseColorMult")).x,
            e(slot!("bUseSizeRamp")).x,
            e(slot!("bUseSizeMult")).x,
        ),
        size_mult: Vec4::new(
            e(slot!("fSizeMultX")).x,
            e(slot!("fSizeMultY")).x,
            e(slot!("iAlign")).x,
            e(slot!("iScreenSpace")).x,
        ),
        pivot: e(slot!("vPivotShift")),
        fade: Vec4::new(e(slot!("fNearFadeStart")).x, e(slot!("fNearFadeEnd")).x, e(slot!("fZBias")).x, first as f32),
        feather: Vec4::new(
            d.z_feather[2],
            d.z_feather[3],
            e(slot!("fContactShadowFalloff")).x,
            e(slot!("fContactShadowOffset")).x,
        ),
        light_pos: light_pos.extend(radius),
        light_col,
        light_misc: Vec4::new(falloff, 0.0, 0.0, 0.0),
        anim: Vec4::new(
            e(slot!("bUseAnimTexture")).x,
            e(slot!("iAnimTextureMode")).x,
            e(slot!("iAnimTextureCount")).x,
            e(slot!("iAnimTextureRows")).x,
        ),
        anim2: Vec4::new(
            e(slot!("fAnimTextureSpeed")).x,
            e(slot!("iAnimTextureRandomStart")).x,
            e(slot!("iTile")).x,
            d.records.len() as f32,
        ),
        modulo: d.dimensions.extend(d.modulo_position as u8 as f32),
        modulo2: Vec4::new(
            e(slot!("fCameraPush")).x,
            e(slot!("iNoFadeFar")).x,
            d.stretch_per_camera as u8 as f32,
            full_res as u8 as f32,
        ),
        glare: [
            Vec4::new(
                e(slot!("fGlareBloomScale")).x,
                e(slot!("fGlareBloomSpread")).x,
                e(slot!("fGlareSensitivityCenter")).x,
                e(slot!("fGlareSensitivityEdge")).x,
            ),
            Vec4::new(
                e(slot!("fGlareSensitivityPower")).x,
                e(slot!("fGlareRotationRate")).x,
                e(slot!("fGlareDirCenterPower")).x,
                e(slot!("fGlareDirEdgePower")).x,
            ),
            Vec4::new(
                e(slot!("fGlareDirOrientation")).x,
                e(slot!("fGlareSizeMultSpread")).x,
                e(slot!("fFlareOffsetScale")).x,
                e(slot!("fFlareEndScale")).x,
            ),
            Vec4::new(e(slot!("bUseGlareSeed")).x, e(slot!("fFlareMovementRate")).x, 0.0, 0.0),
        ],
    }
}

#[allow(clippy::too_many_arguments)]
fn prepare(
    mut gpu: ResMut<ParticleGpu>,
    frame: Res<ParticleFrame>,
    views: Query<&ExtractedView, With<GamePostCamera>>,
    device: Res<RenderDevice>,
    queue: Res<RenderQueue>,
) {
    let Ok(v) = views.single() else { return };
    let gpu = &mut *gpu;
    let size = v.viewport.zw();
    if gpu.targets.as_ref().is_none_or(|t| t.size != size) {
        let half = (size + UVec2::ONE) / 2;
        gpu.targets = Some(Targets {
            size,
            half,
            colour_a: view(&texture(&device, "ssx_particles_colour_a", half, COLOUR_FORMAT, 1)),
            colour_b: view(&texture(&device, "ssx_particles_colour_b", half, COLOUR_FORMAT, 1)),
            depth: view(&texture(&device, "ssx_particles_depth", half, DEPTH_FORMAT, 1)),
            depth_copy: view(&texture(&device, "ssx_particles_depth_copy", half, DEPTH_COPY_FORMAT, 1)),
            depth_full: view(&texture(&device, "ssx_particles_depth_full", size, DEPTH_COPY_FORMAT, 1)),
            dummy: view(&texture(&device, "ssx_particles_dummy", UVec2::ONE, DEPTH_COPY_FORMAT, 1)),
        });
    }
    let id = std::sync::Arc::as_ptr(&frame.textures) as usize;
    if gpu.textures.as_ref().is_none_or(|(k, _)| *k != id) {
        let views = frame
            .textures
            .iter()
            .map(|t| {
                let tex = texture(&device, "ssx_particle_texture", t.size, COLOUR_FORMAT, t.levels.len() as u32);
                for (level, data) in t.levels.iter().enumerate() {
                    let (w, h) = ((t.size.x >> level).max(1), (t.size.y >> level).max(1));
                    queue.write_texture(
                        TexelCopyTextureInfo {
                            texture: &tex,
                            mip_level: level as u32,
                            origin: Origin3d::ZERO,
                            aspect: TextureAspect::All,
                        },
                        data,
                        TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(4 * w), rows_per_image: Some(h) },
                        Extent3d { width: w, height: h, depth_or_array_layers: 1 },
                    );
                }
                view(&tex)
            })
            .collect();
        gpu.textures = Some((id, views));
    }
    let t = gpu.targets.as_ref().unwrap();
    let sizes = Vec4::new(t.half.x as f32, t.half.y as f32, size.x as f32, size.y as f32);
    let world_from_view = v.world_from_view.to_matrix();
    let view_from_world = world_from_view.inverse();
    let view_prev = gpu.view_prev.replace(view_from_world).unwrap_or(view_from_world);
    let (a, b) = (v.clip_from_view.z_axis.z, v.clip_from_view.w_axis.z);
    let far = if a != 0.0 { b / a } else { 8000.0 };
    let near = a * far / (1.0 + a);
    let game_proj = Vec4::new(far / (near - far), near * far / (near - far), 0.0, 0.0);
    let l = &frame.light;
    // PORT(choice): one buffer of the view's constants for the draws and the half-resolution passes
    // (`particle_globals.wesl`; the passes read its `view_from_clip` and `sizes`, the values their own buffer held), so
    // one queue write a frame instead of two: wgpu 30 makes a staging buffer for every write. Traced on Mount Eddie
    // (`--benchmark`) this system took 0.18 instead of 0.26 ms a frame, measured together with the camera schedules'
    // single-threaded executor and the level's light buffer, which do not touch it and moved it 0.50 -> 0.45 ms on
    // their own (an earlier build); this write's own share was not timed apart. Uniform buffers are the port's
    // renderer; the console's GPU set shader constants.
    gpu.globals.write(
        &device,
        &queue,
        "ssx_particle_globals",
        &Globals {
            view_from_world,
            clip_from_view: v.clip_from_view,
            view_from_clip: v.clip_from_view.inverse(),
            world_from_view,
            view_prev,
            camera_pos: v.world_from_view.translation().extend(1.0),
            ramp0: l.ramp_lit,
            ramp1: l.ramp_shade.truncate().extend(0.0),
            atten: Vec4::new(l.rim_shadow.z, l.specular.w, l.misc.z, l.misc.y),
            fog_data: l.fog_data,
            fog_color: l.fog_color,
            sizes,
            game_proj,
        },
    );
    if gpu.serial == frame.serial {
        return;
    }
    gpu.serial = frame.serial;
    let gpu = &mut *gpu;
    let f = &*frame.draws;
    gpu.cmds.clear();
    gpu.cmds.extend(f.draws.iter().map(|d| DrawCmd {
        tech: d.tech,
        bin: d.bin,
        texture: d.texture,
        offset: 0,
        vertices: d.vertices,
    }));
    if !f.draws.is_empty() {
        match gpu.draws.get_writer(f.draws.len(), &device, &queue) {
            Some(mut writer) => {
                for (cmd, d) in gpu.cmds.iter_mut().zip(&f.draws) {
                    cmd.offset = writer.write(&d.uniform);
                }
            }
            None => gpu.cmds.clear(),
        }
    }
    let bytes = if f.records.is_empty() { &[0u8; 16][..] } else { &f.records[..] };
    // PORT(choice): the records' buffer lives across frames and is written through the queue (16 bytes a record, so
    // whole words); it grows (a new buffer, a power of two of at least 64 KiB) only when a frame's records do not fit,
    // so the draws' bind groups (`passes::ParticleBinds`) stay valid. Every draw reads only its own records (the
    // shaders' `first` and its particles), never the tail past the frame's; the copies of a `bFollowCamera` draw read
    // their original's (one copy of the records each before: 8 for `fx_falling_snow`, `iPeriodicBoxNum` 2), and a draw
    // left out keeps its records unread. A new buffer each update made the draws' bind groups (two a texture) anew
    // each time. wgpu's buffers and bind groups are the port's renderer; the console's GPU had no bind groups.
    let size = bytes.len() as u64;
    if gpu.records.as_ref().is_none_or(|b| b.size() < size) {
        gpu.records = Some(device.create_buffer(&BufferDescriptor {
            label: Some("ssx_particle_records"),
            size: size.next_power_of_two().max(1 << 16),
            usage: BufferUsages::STORAGE | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        }));
    }
    if let Some(buffer) = &gpu.records {
        queue.write_buffer(buffer, 0, bytes);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use bevy::prelude::*;
    use ssx_formats::lynx::ParticleSystem;

    use super::{Bin, frame_draws, record_bytes};
    use crate::particles::sim::{Draw, DrawBuffers, EffectDef, REGISTRY_NAMES, RenderConsts};
    use crate::particles::{ParticleLight, ParticleTexture};

    fn consts(texture: &str, extra: &str) -> Arc<RenderConsts> {
        let xml = format!(
            r#"<particleSystem><particleEffect name="e"><particleGroup name="g">
               <particleAction className="ParticleActionRender"><parameter name="sShader" value="lynx.fx" />
               <parameter name="sTechnique" value="AdditiveBlend" /><parameter name="sTexture" value="{texture}" />
               {extra}</particleAction></particleGroup></particleEffect></particleSystem>"#
        );
        let sys = ParticleSystem::parse(xml.as_bytes()).unwrap();
        let def = EffectDef::compile(&sys.effects[0]);
        def.groups[0].actions[0].render.clone().unwrap()
    }

    fn draw(render: Arc<RenderConsts>, bufs: &mut DrawBuffers, particles: usize, extras: &[(usize, Vec4)]) {
        let first = bufs.records.len();
        bufs.records.extend((first..first + particles).map(|i| [Vec4::splat(i as f32), Vec4::ONE, Vec4::ZERO]));
        let e = bufs.extras.len();
        bufs.extras.extend_from_slice(extras);
        bufs.draws.push(Draw {
            effect: 0,
            world: Mat4::IDENTITY,
            layer: 0,
            half: true,
            tonemap: false,
            render,
            time_now: 0,
            age_max: 0,
            age_warp_inv: 1.0,
            stretch: Vec3::ZERO,
            stretch_per_particle: false,
            color_mult: Vec4::ONE,
            color_ramp: [Vec4::ZERO; 4],
            z_feather: [0.0; 4],
            use_global_lighting: false,
            light: (Vec3::ZERO, Vec4::ZERO, 1.0, 1.0),
            modulo_position: false,
            dimensions: Vec3::ZERO,
            stretch_per_camera: false,
            mesh_type: 0,
            extras: e..bufs.extras.len(),
            records: first..bufs.records.len(),
            aabb: (Vec3::ZERO, Vec3::ZERO),
        });
    }

    #[test]
    fn draws_pass_and_read_the_registry_in_order() {
        let speed = REGISTRY_NAMES.iter().position(|n| *n == "fTimeSpeed").unwrap();
        let passing =
            consts("t.dds", r#"<parameter name="fTimeSpeed" value="2.5" /><parameter name="fOwn" value="7" />"#);
        assert_eq!(passing.extras_slots().collect::<Vec<_>>(), vec![speed]);
        let missing = consts("none.dds", r#"<parameter name="fTimeSpeed" value="4.0" />"#);
        let plain = consts("t.dds", "");
        let mut bufs = DrawBuffers::default();
        draw(plain.clone(), &mut bufs, 2, &[]);
        draw(missing, &mut bufs, 3, &[(speed, Vec4::splat(4.0))]);
        draw(plain.clone(), &mut bufs, 1, &[]);
        draw(passing, &mut bufs, 2, &[(speed, Vec4::splat(2.5))]);
        let textures = vec![ParticleTexture { name: "t.dds".into(), size: UVec2::ONE, levels: vec![vec![0; 4]] }];
        let mut registry = [Vec4::ZERO; REGISTRY_NAMES.len()];
        let mut unported = Default::default();
        let f = frame_draws(&bufs, &mut registry, &ParticleLight::default(), &textures, &mut unported);
        assert_eq!((f.drawn, f.vertices, f.draws.len()), (4, 8, 3));
        let speeds: Vec<f32> = f.draws.iter().map(|d| d.uniform.time.z).collect();
        assert_eq!(speeds, vec![0.0, 4.0, 2.5]);
        let firsts: Vec<f32> = f.draws.iter().map(|d| d.uniform.fade.w).collect();
        assert_eq!(firsts, vec![0.0, 15.0, 18.0]);
        assert!(f.draws.iter().all(|d| d.bin == Bin::HalfRes && d.texture == 0));
        assert_eq!(f.records.len(), 8 * 48);
        assert_eq!(f.records[15 * 16..15 * 16 + 4], 5.0f32.to_le_bytes());
        let f = frame_draws(&bufs, &mut registry, &ParticleLight::default(), &textures, &mut unported);
        assert_eq!(f.draws[0].uniform.time.z, 2.5);
    }

    #[test]
    fn record_bytes_are_the_floats_in_order() {
        let specials =
            [0.0, -0.0, f32::MIN_POSITIVE / 2.0, f32::INFINITY, f32::NEG_INFINITY, f32::from_bits(0x7FC0_1234)];
        let mut state = 0x1234_5678u32;
        let mut next = || {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            f32::from_bits(state)
        };
        let mut records: Vec<crate::particles::sim::VertexRecord> =
            (0..97).map(|_| [0; 3].map(|_| Vec4::new(next(), next(), next(), next()))).collect();
        records[3][1] = Vec4::from_array([specials[0], specials[1], specials[2], specials[3]]);
        records[50][2] = Vec4::new(specials[4], specials[5], 1.5, -2.25);
        let mut expected = Vec::new();
        for v in records.iter().flatten() {
            for x in v.to_array() {
                expected.extend_from_slice(&x.to_le_bytes());
            }
        }
        assert_eq!(record_bytes(&records), expected);
        assert!(record_bytes(&[]).is_empty());
    }
}
