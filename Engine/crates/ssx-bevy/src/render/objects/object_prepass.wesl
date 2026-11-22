import bevy_pbr::render::{mesh_functions, skinning};
import bevy_pbr::prepass::io::{Vertex, decompress_vertex};
import package::render::lighting::game_position::place;
import package::render::lighting::prepass_output::PrepassOutput;
@if(SSX_HOOK_VERTEX)
import super::hook;
@if(SSX_HOOK_VERTEX)
import package::render::objects::hook_types::VertexIn;
@if(SSX_HOOK_VERTEX)
import bevy_render::globals::Globals;

@if(SSX_HOOK_VERTEX)
@group(0) @binding(1) var<uniform> prepass_globals: Globals;

@vertex
fn vertex(vertex_in: Vertex) -> PrepassOutput {
    var out: PrepassOutput;
    let vertex = decompress_vertex(vertex_in, vertex_in.instance_index);
    let mesh_world_from_local = mesh_functions::get_world_from_local(vertex_in.instance_index);
@if(SKINNED)
    let world_from_local = skinning::skin_model(vertex.joint_indices, vertex.joint_weights, vertex_in.instance_index);
@else
    let world_from_local = mesh_world_from_local;
    out.world_position = mesh_functions::mesh_position_local_to_world(world_from_local, vec4<f32>(vertex.position, 1.0));
@if(NORMAL_PREPASS_OR_DEFERRED_PREPASS && VERTEX_NORMALS && SKINNED)
    let normal = skinning::skin_normals(world_from_local, vertex.normal);
@elif(NORMAL_PREPASS_OR_DEFERRED_PREPASS && VERTEX_NORMALS)
    let normal = mesh_functions::mesh_normal_local_to_world(vertex.normal, vertex_in.instance_index);
@else
    let normal = vec3(0.0, 1.0, 0.0);
@if(SSX_HOOK_VERTEX) {
    var hook_uv = vec2(0.0);
@if(VERTEX_UVS_A)
    hook_uv = vertex.uv;
    let offset = hook::vertex_offset(VertexIn(out.world_position.xyz, normal, hook_uv, prepass_globals.time));
    out.world_position = vec4(out.world_position.xyz + offset, out.world_position.w);
}
    let placed = place(out.world_position.xyz, normal);
    out.world_position = vec4(placed.world_position, out.world_position.w);
    out.position = placed.clip_position;
@if(UNCLIPPED_DEPTH_ORTHO_EMULATION) {
    out.unclipped_depth = out.position.z;
    out.position.z = min(out.position.z, 1.0);
}

@if(VERTEX_UVS_A)
    out.uv = vertex.uv;
@if(VERTEX_UVS_B)
    out.uv_b = vertex.uv_b;

@if(NORMAL_PREPASS_OR_DEFERRED_PREPASS && VERTEX_NORMALS)
    out.world_normal = placed.world_normal;
@if(NORMAL_PREPASS_OR_DEFERRED_PREPASS && VERTEX_TANGENTS)
    out.world_tangent = mesh_functions::mesh_tangent_local_to_world(world_from_local, vertex.tangent, vertex_in.instance_index);

@if(VERTEX_COLORS)
    out.color = vertex.color;

@if(MOTION_VECTOR_PREPASS) {
@if(SKINNED && HAS_PREVIOUS_SKIN)
    let prev_model = skinning::skin_prev_model(vertex.joint_indices, vertex.joint_weights, vertex_in.instance_index);
@else
    let prev_model = mesh_functions::get_previous_world_from_local(vertex_in.instance_index);
    out.previous_world_position = mesh_functions::mesh_position_local_to_world(prev_model, vec4<f32>(vertex.position, 1.0));
}

@if(VERTEX_OUTPUT_INSTANCE_INDEX)
    out.instance_index = vertex_in.instance_index;

@if(VISIBILITY_RANGE_DITHER)
    out.visibility_range_dither = mesh_functions::get_visibility_range_dither_level(vertex_in.instance_index, mesh_world_from_local[3]);

    return out;
}
