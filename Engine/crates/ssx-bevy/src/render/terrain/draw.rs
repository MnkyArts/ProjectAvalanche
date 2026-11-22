use std::ops::Range;
use std::sync::Arc;

use bevy::asset::UntypedAssetId;
use bevy::core_pipeline::core_3d::{AlphaMask3d, Opaque3d};
use bevy::core_pipeline::deferred::{AlphaMask3dDeferred, Opaque3dDeferred};
use bevy::core_pipeline::prepass::{AlphaMask3dPrepass, Opaque3dPrepass};
use bevy::ecs::query::ROQueryItem;
use bevy::ecs::system::SystemParamItem;
use bevy::ecs::system::lifetimeless::{Read, SRes};
use bevy::material::MaterialProperties;
use bevy::material::labels::{DrawFunctionLabel, InternedDrawFunctionLabel};
use bevy::pbr::{
    DeferredAlphaMaskDrawFunction, DeferredOpaqueDrawFunction, DrawMesh, MainPassAlphaMaskDrawFunction,
    MainPassOpaqueDrawFunction, PreparedMaterial, PrepassAlphaMaskDrawFunction, PrepassOpaqueDepthOnlyDrawFunction,
    PrepassOpaqueDrawFunction, PreprocessPipelines, SetMaterialBindGroup, SetMeshBindGroup, SetMeshViewBindGroup,
    SetMeshViewBindingArrayBindGroup, SetPrepassEmptyMaterialBindGroup, SetPrepassViewBindGroup,
    SetPrepassViewEmptyBindGroup, Shadow, ShadowsDepthOnlyDrawFunction, ShadowsDrawFunction,
};
use bevy::platform::collections::HashMap;
use bevy::prelude::*;
use bevy::render::batching::gpu_preprocessing::{GpuPreprocessingMode, GpuPreprocessingSupport};
use bevy::render::erased_render_asset::ErasedRenderAssets;
use bevy::render::mesh::RenderMesh;
use bevy::render::mesh::RenderMeshBufferInfo;
use bevy::render::mesh::allocator::{MeshAllocator, MeshSlabs};
use bevy::render::render_asset::RenderAssets;
use bevy::render::render_phase::{
    AddRenderCommand, DrawFunctionId, DrawFunctions, PhaseItem, PhaseItemExtraIndex, RenderCommand,
    RenderCommandResult, SetItemPipeline, TrackedRenderPass,
};
use bevy::render::render_resource::{Buffer, IndexFormat, PipelineCache};
use bevy::render::sync_world::MainEntity;
use bevy::render::view::{ExtractedView, RetainedViewEntity};
use bevy::render::{Extract, ExtractSchedule, Render, RenderApp, RenderSystems};

use super::path::TerrainPath;

pub type DrawTerrain = (
    SetItemPipeline,
    SetMeshViewBindGroup<0>,
    SetMeshViewBindingArrayBindGroup<1>,
    SetMeshBindGroup<2>,
    SetMaterialBindGroup<3>,
    DrawTerrainParts,
);

pub type DrawTerrainPrepass = (
    SetItemPipeline,
    SetPrepassViewBindGroup<0>,
    SetPrepassViewEmptyBindGroup<1>,
    SetMeshBindGroup<2>,
    SetMaterialBindGroup<3>,
    DrawTerrainParts,
);

pub type DrawTerrainDepthOnly = (
    SetItemPipeline,
    SetPrepassViewBindGroup<0>,
    SetPrepassViewEmptyBindGroup<1>,
    SetMeshBindGroup<2>,
    SetPrepassEmptyMaterialBindGroup<3>,
    DrawTerrainParts,
);

pub struct TerrainDrawPlugin;

impl Plugin for TerrainDrawPlugin {
    fn build(&self, app: &mut App) {
        let Some(render) = app.get_sub_app_mut(RenderApp) else { return };
        render.init_resource::<RenderTerrainPath>().add_systems(ExtractSchedule, extract_terrain_path).add_systems(
            Render,
            (
                terrain_draw_functions.in_set(RenderSystems::PrepareMeshes),
                prepare_terrain_draws.in_set(RenderSystems::PrepareResources),
            ),
        );
    }

    fn finish(&self, app: &mut App) {
        let Some(render) = app.get_sub_app_mut(RenderApp) else { return };
        render
            .add_render_command::<Opaque3d, DrawTerrain>()
            .add_render_command::<AlphaMask3d, DrawTerrain>()
            .add_render_command::<Opaque3dPrepass, DrawTerrainPrepass>()
            .add_render_command::<Opaque3dPrepass, DrawTerrainDepthOnly>()
            .add_render_command::<AlphaMask3dPrepass, DrawTerrainPrepass>()
            .add_render_command::<AlphaMask3dPrepass, DrawTerrainDepthOnly>()
            .add_render_command::<Opaque3dDeferred, DrawTerrainPrepass>()
            .add_render_command::<AlphaMask3dDeferred, DrawTerrainPrepass>()
            .add_render_command::<Shadow, DrawTerrainPrepass>()
            .add_render_command::<Shadow, DrawTerrainDepthOnly>();
        let w = render.world();
        let functions = TerrainDrawFunctions([
            (MainPassOpaqueDrawFunction.intern(), w.resource::<DrawFunctions<Opaque3d>>().read().id::<DrawTerrain>()),
            (
                MainPassAlphaMaskDrawFunction.intern(),
                w.resource::<DrawFunctions<AlphaMask3d>>().read().id::<DrawTerrain>(),
            ),
            (
                PrepassOpaqueDrawFunction.intern(),
                w.resource::<DrawFunctions<Opaque3dPrepass>>().read().id::<DrawTerrainPrepass>(),
            ),
            (
                PrepassAlphaMaskDrawFunction.intern(),
                w.resource::<DrawFunctions<AlphaMask3dPrepass>>().read().id::<DrawTerrainPrepass>(),
            ),
            (
                PrepassOpaqueDepthOnlyDrawFunction.intern(),
                w.resource::<DrawFunctions<Opaque3dPrepass>>().read().id::<DrawTerrainDepthOnly>(),
            ),
            (
                DeferredOpaqueDrawFunction.intern(),
                w.resource::<DrawFunctions<Opaque3dDeferred>>().read().id::<DrawTerrainPrepass>(),
            ),
            (
                DeferredAlphaMaskDrawFunction.intern(),
                w.resource::<DrawFunctions<AlphaMask3dDeferred>>().read().id::<DrawTerrainPrepass>(),
            ),
            (ShadowsDrawFunction.intern(), w.resource::<DrawFunctions<Shadow>>().read().id::<DrawTerrainPrepass>()),
            (
                ShadowsDepthOnlyDrawFunction.intern(),
                w.resource::<DrawFunctions<Shadow>>().read().id::<DrawTerrainDepthOnly>(),
            ),
        ]);
        render.insert_resource(functions);
    }
}

#[derive(Resource)]
struct TerrainDrawFunctions([(InternedDrawFunctionLabel, DrawFunctionId); 9]);

struct PartDraw {
    vertex: Buffer,
    index: Option<(Buffer, IndexFormat)>,
    range: Range<u32>,
    base_vertex: i32,
}

#[derive(Resource, Default)]
pub struct RenderTerrainPath {
    meshes: Arc<[AssetId<Mesh>]>,
    materials: Arc<[UntypedAssetId]>,
    carriers: Vec<MainEntity>,
    views: Vec<(RetainedViewEntity, Vec<(u32, u32)>)>,
    resolved: Vec<Option<(MeshSlabs, PartDraw)>>,
    lists: HashMap<(RetainedViewEntity, MainEntity), Range<usize>>,
    order: Vec<u32>,
}

fn extract_terrain_path(main: Extract<Option<Res<TerrainPath>>>, mut path: ResMut<RenderTerrainPath>) {
    let path = &mut *path;
    let Some(main) = main.as_deref() else {
        if !path.meshes.is_empty() || !path.views.is_empty() {
            *path = RenderTerrainPath::default();
        }
        return;
    };
    if !Arc::ptr_eq(&path.meshes, &main.meshes) {
        path.meshes = main.meshes.clone();
        path.materials = main.materials.clone();
        path.carriers = main.carrier_entities().map(MainEntity::from).collect();
        path.resolved.clear();
    }
    path.views.truncate(main.views.len());
    for (i, from) in main.views.iter().enumerate() {
        match path.views.get_mut(i) {
            Some((view, draws)) => {
                *view = from.view;
                draws.clear();
                draws.extend_from_slice(&from.draws);
            }
            None => path.views.push((from.view, from.draws.clone())),
        }
    }
}

// PORT(choice): the terrain's materials keep Bevy's `MaterialPlugin` (their bind groups, pipelines and preparation) and
// only their draw functions are the path's, set on Bevy's prepared material: bevy_pbr 0.20 picks a mesh's draw function
// from its material's `MaterialProperties::draw_functions`, which the material trait gives no hook for. The console had
// no such engine: its renderer bound a technique and drew a slot's index ranges with it (`render_shadowbuffer` over the
// face groups' ranges +0xF0..+0xFC, 0x829BE7CC; the simple techniques over the whole list, 0x829BE6FC / 0x829BE788).
fn terrain_draw_functions(
    path: Res<RenderTerrainPath>,
    functions: Res<TerrainDrawFunctions>,
    mut materials: ResMut<ErasedRenderAssets<PreparedMaterial>>,
) {
    for &id in path.materials.iter() {
        let Some(material) = materials.get_mut(id) else { continue };
        let ours = |label: &InternedDrawFunctionLabel| functions.0.iter().find(|(l, _)| l == label).map(|&(_, f)| f);
        if material.properties.draw_functions.iter().all(|(label, f)| ours(label).is_none_or(|o| o == *f)) {
            continue;
        }
        let p = &*material.properties;
        let mut draw_functions = p.draw_functions.clone();
        for (label, f) in draw_functions.iter_mut() {
            if let Some(o) = ours(label) {
                *f = o;
            }
        }
        material.properties = Arc::new(MaterialProperties {
            render_method: p.render_method,
            alpha_mode: p.alpha_mode,
            mesh_pipeline_key_bits: p.mesh_pipeline_key_bits,
            depth_bias: p.depth_bias,
            reads_view_transmission_texture: p.reads_view_transmission_texture,
            render_phase_type: p.render_phase_type,
            material_layout: p.material_layout.clone(),
            draw_functions,
            shaders: p.shaders.clone(),
            bindless: p.bindless,
            base_specialize: p.base_specialize,
            prepass_specialize: p.prepass_specialize,
            user_specialize: p.user_specialize,
            material_key: p.material_key.clone(),
            shadows_enabled: p.shadows_enabled,
            prepass_enabled: p.prepass_enabled,
            oit_enabled: p.oit_enabled,
        });
    }
}

type DrawKey = (u32, MeshSlabs, AssetId<Mesh>, u32);

fn prepare_terrain_draws(
    mut path: ResMut<RenderTerrainPath>,
    allocator: Res<MeshAllocator>,
    meshes: Res<RenderAssets<RenderMesh>>,
    mut order: Local<Vec<DrawKey>>,
) {
    let path = &mut *path;
    if meshes.is_changed() || allocator.meshes_displaced_by_slab_growth().next().is_some() {
        path.resolved.clear();
    }
    let parts = path.meshes.len();
    path.resolved.resize_with(parts, || None);
    path.lists.clear();
    path.order.clear();
    for (view, draws) in &path.views {
        order.clear();
        for &(c, p) in draws {
            let (Some(slot), Some(&id)) = (path.resolved.get_mut(p as usize), path.meshes.get(p as usize)) else {
                continue;
            };
            if slot.is_none() {
                *slot = allocator.mesh_slabs(&id).zip(part_draw(&allocator, &meshes, id));
            }
            if let Some((slabs, _)) = slot {
                order.push((c, *slabs, id, p));
            }
        }
        order.sort_unstable();
        let mut i = 0;
        while i < order.len() {
            let carrier = order[i].0;
            let start = path.order.len();
            while i < order.len() && order[i].0 == carrier {
                path.order.push(order[i].3);
                i += 1;
            }
            if let Some(&c) = path.carriers.get(carrier as usize) {
                path.lists.insert((*view, c), start..path.order.len());
            }
        }
    }
}

fn part_draw(allocator: &MeshAllocator, meshes: &RenderAssets<RenderMesh>, id: AssetId<Mesh>) -> Option<PartDraw> {
    let mesh = meshes.get(id)?;
    let vertex = allocator.mesh_vertex_slice(&id)?;
    let base_vertex = vertex.range.start as i32;
    Some(match &mesh.buffer_info {
        RenderMeshBufferInfo::Indexed { index_format, count } => {
            let index = allocator.mesh_index_slice(&id)?;
            PartDraw {
                vertex: vertex.buffer.clone(),
                index: Some((index.buffer.clone(), *index_format)),
                range: index.range.start..index.range.start + count,
                base_vertex,
            }
        }
        RenderMeshBufferInfo::NonIndexed => {
            PartDraw { vertex: vertex.buffer.clone(), index: None, range: vertex.range.clone(), base_vertex }
        }
    })
}

pub struct DrawTerrainParts;

impl<P: PhaseItem> RenderCommand<P> for DrawTerrainParts {
    type Param = (
        SRes<RenderTerrainPath>,
        SRes<PipelineCache>,
        Option<SRes<PreprocessPipelines>>,
        SRes<GpuPreprocessingSupport>,
        <DrawMesh as RenderCommand<P>>::Param,
    );
    type ViewQuery = (Read<ExtractedView>, <DrawMesh as RenderCommand<P>>::ViewQuery);
    type ItemQuery = <DrawMesh as RenderCommand<P>>::ItemQuery;

    fn render<'w>(
        item: &P,
        (view, has_preprocess_bind_groups): ROQueryItem<'w, '_, Self::ViewQuery>,
        entity: Option<ROQueryItem<'w, '_, Self::ItemQuery>>,
        (path, cache, preprocess, support, mesh): SystemParamItem<'w, '_, Self::Param>,
        pass: &mut TrackedRenderPass<'w>,
    ) -> RenderCommandResult {
        let path = path.into_inner();
        let Some(range) = path.lists.get(&(view.retained_view_entity, item.main_entity())) else {
            return DrawMesh::render(item, has_preprocess_bind_groups, entity, mesh, pass);
        };
        if let Some(pipelines) = preprocess.map(|p| p.into_inner())
            && (!has_preprocess_bind_groups || !preprocessing_loaded(pipelines, &cache, &support))
        {
            return RenderCommandResult::Skip;
        }
        if let PhaseItemExtraIndex::IndirectParametersIndex { .. } = item.extra_index() {
            return RenderCommandResult::Failure("the terrain path draws in direct views only (NoIndirectDrawing)");
        }
        let instances = item.batch_range().clone();
        if instances.len() != 1 {
            return RenderCommandResult::Failure("a terrain carrier's item holds more than its own instance");
        }
        let parts = path.order.get(range.clone()).unwrap_or_default();
        for d in parts.iter().filter_map(|&p| path.resolved.get(p as usize)?.as_ref().map(|(_, d)| d)) {
            pass.set_vertex_buffer(0, d.vertex.slice(..));
            match &d.index {
                Some((buffer, format)) => {
                    pass.set_index_buffer(buffer.slice(..), *format);
                    pass.draw_indexed(d.range.clone(), d.base_vertex, instances.clone());
                }
                None => pass.draw(d.range.clone(), instances.clone()),
            }
        }
        RenderCommandResult::Success
    }
}

fn preprocessing_loaded(p: &PreprocessPipelines, cache: &PipelineCache, support: &GpuPreprocessingSupport) -> bool {
    let loaded = |id: Option<bevy::render::render_resource::CachedComputePipelineId>| {
        id.is_some_and(|id| cache.get_compute_pipeline(id).is_some())
    };
    let phase = |p: &bevy::pbr::PreprocessPhasePipelines| {
        loaded(p.reset_indirect_batch_sets.pipeline_id)
            && loaded(p.gpu_occlusion_culling_build_indexed_indirect_params.pipeline_id)
            && loaded(p.gpu_occlusion_culling_build_non_indexed_indirect_params.pipeline_id)
    };
    match support.max_supported_mode {
        GpuPreprocessingMode::None => false,
        GpuPreprocessingMode::PreprocessingOnly => {
            loaded(p.direct_preprocess.pipeline_id) && loaded(p.gpu_frustum_culling_preprocess.pipeline_id)
        }
        GpuPreprocessingMode::Culling => {
            loaded(p.direct_preprocess.pipeline_id)
                && loaded(p.gpu_frustum_culling_preprocess.pipeline_id)
                && loaded(p.early_gpu_occlusion_culling_preprocess.pipeline_id)
                && loaded(p.late_gpu_occlusion_culling_preprocess.pipeline_id)
                && loaded(p.gpu_frustum_culling_build_indexed_indirect_params.pipeline_id)
                && loaded(p.gpu_frustum_culling_build_non_indexed_indirect_params.pipeline_id)
                && phase(&p.early_phase)
                && phase(&p.late_phase)
                && phase(&p.main_phase)
        }
    }
}
