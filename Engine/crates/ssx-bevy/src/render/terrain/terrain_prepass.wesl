import bevy_pbr::render::mesh_functions;
import bevy_pbr::prepass::io::{Vertex, decompress_vertex};
import package::render::lighting::game_position::place;
import package::render::lighting::prepass_output::PrepassOutput;
@if(SSX_TERRAIN_RIPPLE)
import package::render::terrain::terrain_ripple;

@vertex
fn vertex(vertex_in: Vertex) -> PrepassOutput {
    var out: PrepassOutput;
    let vertex = decompress_vertex(vertex_in, vertex_in.instance_index);
    let world_from_local = mesh_functions::get_world_from_local(vertex_in.instance_index);
    out.world_position = mesh_functions::mesh_position_local_to_world(world_from_local, vec4<f32>(vertex.position, 1.0));
@if(NORMAL_PREPASS_OR_DEFERRED_PREPASS && VERTEX_NORMALS)
    let normal = (world_from_local * vec4<f32>(vertex.normal, 0.0)).xyz;
@else
    let normal = vec3(0.0, 1.0, 0.0);
@if(SSX_TERRAIN_RIPPLE)
    let placed = terrain_ripple::place(out.world_position.xyz, normal);
@else
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
    let prev_model = mesh_functions::get_previous_world_from_local(vertex_in.instance_index);
    out.previous_world_position = mesh_functions::mesh_position_local_to_world(prev_model, vec4<f32>(vertex.position, 1.0));
}

@if(VERTEX_OUTPUT_INSTANCE_INDEX)
    out.instance_index = vertex_in.instance_index;

@if(VISIBILITY_RANGE_DITHER)
    out.visibility_range_dither = mesh_functions::get_visibility_range_dither_level(vertex_in.instance_index, world_from_local[3]);

    return out;
}
