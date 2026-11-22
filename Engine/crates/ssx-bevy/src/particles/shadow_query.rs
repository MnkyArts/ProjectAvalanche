use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use bevy::asset::embedded_asset;
use bevy::light::{NotShadowCaster, NotShadowReceiver};
use bevy::mesh::MeshVertexBufferLayoutRef;
use bevy::pbr::{Material, MaterialPipeline, MaterialPipelineKey, MaterialPlugin};
use bevy::prelude::*;
use bevy::render::render_asset::RenderAssets;
use bevy::render::render_resource::{
    AsBindGroup, Buffer, BufferDescriptor, BufferUsages, CommandEncoderDescriptor, CompareFunction, MapMode,
    RenderPipelineDescriptor, SpecializedMeshPipelineError,
};
use bevy::render::renderer::{PendingCommandBuffers, RenderDevice, RenderGraph, RenderGraphSystems};
use bevy::render::storage::{GpuShaderBuffer, ShaderBuffer, ShaderBufferData};
use bevy::render::{Extract, ExtractSchedule, GpuResourceAppExt, MainWorld, RenderApp};
use bevy::shader::ShaderRef;

use crate::render::post::GamePostCamera;
use crate::ride::Ride;

const SHADER_PATH: &str = "embedded://ssx_bevy/particles/shadow_query.wesl";
const QUAD_DISTANCE: f32 = 0.6;
const QUAD_HALF: f32 = 0.01;
const RESULT_BYTES: u64 = 16;

#[derive(Resource, Clone, Copy, Debug)]
pub struct PlayerShadowQuery {
    pub value: f32,
    pub readbacks: u64,
    pub cascade: f32,
    pub margin: f32,
}

impl Default for PlayerShadowQuery {
    fn default() -> Self {
        Self { value: 1.0, readbacks: 0, cascade: -1.0, margin: 0.0 }
    }
}

#[derive(Asset, AsBindGroup, TypePath, Debug, Clone)]
pub struct ShadowQueryMaterial {
    #[storage(0, read_only, visibility(fragment))]
    pub point: Handle<ShaderBuffer>,
    #[storage(1, visibility(fragment))]
    pub result: Handle<ShaderBuffer>,
}

impl Material for ShadowQueryMaterial {
    fn fragment_shader() -> ShaderRef {
        SHADER_PATH.into()
    }

    fn alpha_mode(&self) -> AlphaMode {
        AlphaMode::Blend
    }

    fn enable_prepass() -> bool {
        false
    }

    fn enable_shadows() -> bool {
        false
    }

    fn specialize(
        _pipeline: &MaterialPipeline,
        descriptor: &mut RenderPipelineDescriptor,
        _layout: &MeshVertexBufferLayoutRef,
        _key: MaterialPipelineKey<Self>,
    ) -> Result<(), SpecializedMeshPipelineError> {
        if let Some(depth) = descriptor.depth_stencil.as_mut() {
            depth.depth_write_enabled = Some(false);
            depth.depth_compare = Some(CompareFunction::Always);
        }
        descriptor.primitive.cull_mode = None;
        Ok(())
    }
}

pub fn plugin(app: &mut App) {
    embedded_asset!(app, "shadow_query.wesl");
    app.add_plugins(MaterialPlugin::<ShadowQueryMaterial>::default())
        .init_resource::<PlayerShadowQuery>()
        .add_systems(Update, (spawn_query, set_point));
    crate::shell::level::scoped::<QueryMaterial>(app);
    let Some(render) = app.get_sub_app_mut(RenderApp) else { return };
    render
        .init_resource::<Delivery>()
        .insert_resource(QueryLog(crate::render::post::env_is("SSX_SHADOW_QUERY_LOG", "on")))
        .init_resource::<QueryTarget>()
        .init_gpu_resource::<Slots>()
        .add_systems(ExtractSchedule, (take_reading.ambiguous_with_all(), extract_target.after(take_reading)))
        .add_systems(
            RenderGraph,
            (
                copy_result.after(RenderGraphSystems::Render).before(RenderGraphSystems::Submit),
                map_result.in_set(RenderGraphSystems::Finish).after(RenderGraphSystems::Submit),
            ),
        );
}

#[derive(Resource)]
struct QueryMaterial {
    point: Handle<ShaderBuffer>,
    result: Handle<ShaderBuffer>,
}

#[derive(Resource)]
struct QueryLog(bool);

fn spawn_query(
    mut commands: Commands,
    existing: Option<Res<QueryMaterial>>,
    cameras: Query<Entity, With<GamePostCamera>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<ShadowQueryMaterial>>,
    mut buffers: ResMut<Assets<ShaderBuffer>>,
) {
    if existing.is_some() {
        return;
    }
    let Some(camera) = cameras.iter().next() else { return };
    let mut buffer = ShaderBuffer::from(vec![Vec4::new(1.0, 0.0, 0.0, 0.0)]);
    buffer.buffer_usage |= BufferUsages::COPY_SRC;
    let result = buffers.add(buffer);
    let point = buffers.add(ShaderBuffer::from(vec![Vec4::ZERO]));
    let material = materials.add(ShadowQueryMaterial { point: point.clone(), result: result.clone() });
    let quad = meshes.add(Rectangle::new(2.0 * QUAD_HALF, 2.0 * QUAD_HALF));
    commands.spawn((
        Name::new("player shadow query"),
        Mesh3d(quad),
        MeshMaterial3d(material),
        Transform::from_xyz(0.0, 0.0, -QUAD_DISTANCE),
        NotShadowCaster,
        NotShadowReceiver,
        ChildOf(camera),
    ));
    commands.insert_resource(QueryMaterial { point, result });
}

fn set_point(ride: Option<Res<Ride>>, handle: Option<Res<QueryMaterial>>, mut buffers: ResMut<Assets<ShaderBuffer>>) {
    let Some(handle) = handle else { return };
    let point = ride.map_or(Vec4::ZERO, |r| r.render.traj.extend(1.0));
    // PORT(choice): the point is a buffer of its own that the material binds, written in place when it moves (the same
    // 16 bytes, so Bevy's `GpuShaderBuffer` writes the GPU buffer it has and the material stays as it is). As a uniform
    // of the material, every frame of a moving rider changed the material: Bevy made its bind group again (0.08 ms a
    // frame in the render world; Mount Eddie, `--benchmark`, traced) and flagged its entity for specialising again
    // (`AssetChanged` in `check_entities_needing_specialization`, which runs every frame either way). Now that
    // preparation is 0.01 ms a frame and the buffers' 0.01 more. The shader reads the point in the same frame either
    // way: the buffer's write is queued in the frame's `PrepareAssets`, before the frame's passes are submitted. Bind
    // groups and wgpu's buffers are the port's renderer; the console's query (0x82FBB620) has neither.
    let Some(buffer) = buffers.get(&handle.point) else { return };
    if let ShaderBufferData::Initialized { data, .. } = &buffer.data
        && read_vec4(data.as_slice()) == Some(point)
    {
        return;
    }
    if let Some(mut buffer) = buffers.get_mut(&handle.point) {
        buffer.clear();
        buffer.extend_from_slice(&[point]);
    }
}

#[derive(Default)]
struct Delivered {
    newest: Option<(AssetId<ShaderBuffer>, Vec4)>,
    count: u64,
    failed: Option<String>,
}

#[derive(Resource, Clone, Default)]
struct Delivery(Arc<Mutex<Delivered>>);

impl Delivery {
    fn deliver(&self, id: AssetId<ShaderBuffer>, got: Result<Vec4, String>) {
        let mut d = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        match got {
            Ok(v) => {
                d.newest = Some((id, v));
                d.count += 1;
            }
            Err(e) => {
                d.failed.get_or_insert(e);
            }
        }
    }
}

fn read_vec4(bytes: &[u8]) -> Option<Vec4> {
    let words: &[u8; 16] = bytes.try_into().ok()?;
    let (words, _) = words.as_chunks::<4>();
    Some(Vec4::from_array(std::array::from_fn(|i| f32::from_le_bytes(words[i]))))
}

fn take_reading(
    mut main: ResMut<MainWorld>,
    delivery: Res<Delivery>,
    log: Res<QueryLog>,
    mut seen: Local<u64>,
    mut said: Local<bool>,
) {
    let d = delivery.0.lock().unwrap_or_else(PoisonError::into_inner);
    if !*said && let Some(e) = &d.failed {
        *said = true;
        warn!("player shadow query: a read-back failed ({e}); the value read before stays");
    }
    let arrived = d.count - *seen;
    if arrived == 0 {
        return;
    }
    *seen = d.count;
    let Some((id, v)) = d.newest else { return };
    if main.get_resource::<QueryMaterial>().is_none_or(|m| m.result.id() != id) {
        return;
    }
    let Some(mut q) = main.get_resource_mut::<PlayerShadowQuery>() else { return };
    let before = q.readbacks;
    (q.value, q.readbacks, q.cascade, q.margin) = (v.x, before + arrived, v.z, v.w);
    if log.0 && q.readbacks.div_ceil(30) > before.div_ceil(30) {
        info!("player shadow query: {:.3} (point valid {}, read-back {})", v.x, v.y, q.readbacks);
    }
}

#[derive(Resource, Default)]
struct QueryTarget(Option<AssetId<ShaderBuffer>>);

fn extract_target(mut target: ResMut<QueryTarget>, material: Extract<Option<Res<QueryMaterial>>>) {
    target.0 = material.as_ref().map(|m| m.result.id());
}

struct Slot {
    buffer: Buffer,
    busy: Arc<AtomicBool>,
}

impl Slot {
    fn new(device: &RenderDevice) -> Self {
        let buffer = device.create_buffer(&BufferDescriptor {
            label: Some("ssx_shadow_query_read"),
            size: RESULT_BYTES,
            usage: BufferUsages::COPY_DST | BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        Slot { buffer, busy: Arc::default() }
    }
}

#[derive(Resource, Default)]
struct Slots {
    slots: Vec<Slot>,
    copied: Option<(usize, AssetId<ShaderBuffer>)>,
}

fn copy_result(
    mut slots: ResMut<Slots>,
    target: Res<QueryTarget>,
    buffers: Res<RenderAssets<GpuShaderBuffer>>,
    device: Res<RenderDevice>,
    mut pending: ResMut<PendingCommandBuffers>,
) {
    let Some(id) = target.0 else { return };
    let Some(src) = buffers.get(id) else { return };
    let index = match slots.slots.iter().position(|s| !s.busy.load(Ordering::Acquire)) {
        Some(index) => index,
        None => {
            slots.slots.push(Slot::new(&device));
            slots.slots.len() - 1
        }
    };
    let slot = &slots.slots[index];
    let mut encoder = device.create_command_encoder(&CommandEncoderDescriptor { label: Some("ssx_shadow_query_copy") });
    encoder.copy_buffer_to_buffer(&src.buffer, 0, &slot.buffer, 0, RESULT_BYTES);
    pending.push([encoder.finish()]);
    slot.busy.store(true, Ordering::Release);
    slots.copied = Some((index, id));
}

fn map_result(mut slots: ResMut<Slots>, delivery: Res<Delivery>) {
    let Some((index, id)) = slots.copied.take() else { return };
    let Some(slot) = slots.slots.get(index) else { return };
    let (buffer, busy, delivery) = (slot.buffer.clone(), slot.busy.clone(), delivery.clone());
    slot.buffer.slice(..).map_async(MapMode::Read, move |mapped| {
        let got = match mapped {
            Ok(()) => {
                let value = buffer.slice(..).get_mapped_range().map(|view| read_vec4(&view));
                buffer.unmap();
                match value {
                    Ok(Some(v)) => Ok(v),
                    Ok(None) => Err(format!("the mapped range is not {RESULT_BYTES} bytes")),
                    Err(e) => Err(e.to_string()),
                }
            }
            Err(e) => Err(e.to_string()),
        };
        delivery.deliver(id, got);
        busy.store(false, Ordering::Release);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_failed_map_is_kept_once_and_the_value_before_it_stays() {
        let d = Delivery::default();
        let id = AssetId::<ShaderBuffer>::default();
        d.deliver(id, Ok(Vec4::new(0.5, 1.0, 2.0, -0.25)));
        d.deliver(id, Err("Device is lost".into()));
        d.deliver(id, Err("a later failure".into()));
        let got = d.0.lock().unwrap_or_else(PoisonError::into_inner);
        assert_eq!(got.newest, Some((id, Vec4::new(0.5, 1.0, 2.0, -0.25))));
        assert_eq!(got.count, 1);
        assert_eq!(got.failed.as_deref(), Some("Device is lost"));
    }

    #[test]
    fn the_result_reads_as_the_gpu_wrote_it() {
        let bytes: Vec<u8> = [1.0f32, 0.0, 3.0, -0.002].iter().flat_map(|f| f.to_le_bytes()).collect();
        assert_eq!(read_vec4(&bytes), Some(Vec4::new(1.0, 0.0, 3.0, -0.002)));
        assert_eq!(read_vec4(&bytes[..12]), None);
        assert_eq!(read_vec4(&[0; 20]), None);
    }
}
