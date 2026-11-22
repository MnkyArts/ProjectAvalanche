import bevy_pbr::render::{
    mesh_bindings::mesh,
    mesh_functions,
    skinning,
    morph::{morph_position, morph_normal, morph_tangent},
    forward_io::{Vertex, UncompressedVertex, VertexOutput, decompress_vertex},
};
import package::render::lighting::game_lighting as gl;
import package::render::lighting::game_position::{MainOutput, place};
@if(SSX_TERRAIN_RIPPLE)
import package::render::terrain::terrain_ripple;

@if(SSX_HOOK_VERTEX)
import super::hook;
@if(SSX_HOOK_VERTEX)
import package::render::objects::hook_types::VertexIn;
@if(SSX_HOOK_VERTEX)
import bevy_pbr::render::mesh_view_bindings::globals;

@if(SSX_INVARIANT_POSITION)
alias GameVertexOutput = MainOutput;
@if(!SSX_INVARIANT_POSITION)
alias GameVertexOutput = VertexOutput;

@if(SSX_LIGHT_BUFFER)
@group(constants::MATERIAL_BIND_GROUP) @binding(140) var<storage, read> light_buffer: array<vec4<f32>>;
@else
@group(constants::MATERIAL_BIND_GROUP) @binding(constants::SSX_LIGHT_BINDING)
var<storage, read> fog_light: gl::GameLight;

@if(MORPH_TARGETS)
fn morph_vertex(vertex_in: UncompressedVertex, instance_index: u32) -> UncompressedVertex {
    var vertex = vertex_in;
    let first_vertex = mesh[instance_index].first_vertex_index;
    let vertex_index = vertex.index - first_vertex;
    let morph_descriptor_index = mesh[instance_index].morph_descriptor_index;

    let weight_count = bevy_pbr::render::morph::layer_count(morph_descriptor_index);
    for (var i: u32 = 0u; i < weight_count; i ++) {
        let weight = bevy_pbr::render::morph::weight_at(i, morph_descriptor_index);
        if weight == 0.0 {
            continue;
        }
        vertex.position += weight * morph_position(vertex_index, i, morph_descriptor_index);
@if(VERTEX_NORMALS)
        vertex.normal += weight * morph_normal(vertex_index, i, morph_descriptor_index);
@if(VERTEX_TANGENTS)
        vertex.tangent += vec4(weight * morph_tangent(vertex_index, i, morph_descriptor_index), 0.0);
    }
    return vertex;
}

@vertex
fn vertex(vertex_no_morph: Vertex) -> GameVertexOutput {
    var out: GameVertexOutput;
    let uncompressed_vertex_no_morph = decompress_vertex(vertex_no_morph, vertex_no_morph.instance_index);
@if(MORPH_TARGETS)
    var vertex = morph_vertex(uncompressed_vertex_no_morph, vertex_no_morph.instance_index);
@else
    var vertex = uncompressed_vertex_no_morph;

    let mesh_world_from_local = mesh_functions::get_world_from_local(vertex_no_morph.instance_index);

@if(SKINNED)
    var world_from_local = skinning::skin_model(
        vertex.joint_indices,
        vertex.joint_weights,
        vertex_no_morph.instance_index
    );
@else
    var world_from_local = mesh_world_from_local;

@if(VERTEX_NORMALS && SKINNED)
    out.world_normal = skinning::skin_normals(world_from_local, vertex.normal);
@elif(VERTEX_NORMALS && SSX_RAW_NORMAL)
    out.world_normal = (world_from_local * vec4<f32>(vertex.normal, 0.0)).xyz;
@elif(VERTEX_NORMALS)
    out.world_normal = mesh_functions::mesh_normal_local_to_world(vertex.normal, vertex_no_morph.instance_index);

@if(VERTEX_POSITIONS) {
    out.world_position = mesh_functions::mesh_position_local_to_world(world_from_local, vec4<f32>(vertex.position, 1.0));
    var normal = vec3(0.0, 1.0, 0.0);
@if(VERTEX_NORMALS)
    normal = out.world_normal;
@if(SSX_HOOK_VERTEX) {
    var hook_uv = vec2(0.0);
@if(VERTEX_UVS_A)
    hook_uv = vertex.uv;
    let offset = hook::vertex_offset(VertexIn(out.world_position.xyz, normal, hook_uv, globals.time));
    out.world_position = vec4(out.world_position.xyz + offset, out.world_position.w);
}
@if(SSX_TERRAIN_RIPPLE)
    let placed = terrain_ripple::place(out.world_position.xyz, normal);
@else
    let placed = place(out.world_position.xyz, normal);
    out.world_position = vec4(placed.world_position, out.world_position.w);
@if(VERTEX_NORMALS)
    out.world_normal = placed.world_normal;
    out.position = placed.clip_position;
@if(SSX_LIGHT_BUFFER)
    let at = bitcast<u32>(light_buffer[3].w);
@if(SSX_LIGHT_BUFFER)
    let fog_light = gl::GameLight(
        light_buffer[at],
        light_buffer[at + 1u],
        light_buffer[at + 2u],
        light_buffer[at + 3u],
        light_buffer[at + 4u],
        light_buffer[at + 5u],
        light_buffer[at + 6u],
        light_buffer[at + 7u],
        light_buffer[at + 8u],
        light_buffer[at + 9u],
        light_buffer[at + 10u],
    );
    out.world_position.w = gl::fog_amount(fog_light, out.world_position.xyz);
}

@if(VERTEX_UVS_A)
    out.uv = vertex.uv;
@if(VERTEX_UVS_B)
    out.uv_b = vertex.uv_b;

@if(VERTEX_TANGENTS)
    out.world_tangent = mesh_functions::mesh_tangent_local_to_world(
        world_from_local,
        vertex.tangent,
        vertex_no_morph.instance_index
    );

@if(VERTEX_COLORS)
    out.color = vertex.color;

@if(VERTEX_OUTPUT_INSTANCE_INDEX)
    out.instance_index = vertex_no_morph.instance_index;

@if(VISIBILITY_RANGE_DITHER)
    out.visibility_range_dither = mesh_functions::get_visibility_range_dither_level(
        vertex_no_morph.instance_index, mesh_world_from_local[3]);

    return out;
}
