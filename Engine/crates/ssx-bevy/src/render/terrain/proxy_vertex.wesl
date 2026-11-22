import bevy_pbr::render::{
    mesh_functions,
    forward_io::{Vertex, VertexOutput, decompress_vertex},
    view_transformations::position_world_to_clip,
};
import bevy_pbr::render::mesh_view_bindings as view_bindings;
import package::render::lighting::game_lighting as gl;

@group(constants::MATERIAL_BIND_GROUP) @binding(100) var<storage, read> light: gl::GameLight;
@group(constants::MATERIAL_BIND_GROUP) @binding(101) var<uniform> reach: vec4<f32>;

const OVERLAP = vec3(-1.0 / 120.0, 5.0, 120.0);
const NORMAL_PUSH: f32 = -6.0;

@vertex
fn vertex(v: Vertex) -> VertexOutput {
    var out: VertexOutput;
    let vertex = decompress_vertex(v, v.instance_index);
    let world_from_local = mesh_functions::get_world_from_local(v.instance_index);
    var n = vec3(0.0);
@if(VERTEX_NORMALS) {
    n = (world_from_local * vec4(vertex.normal, 0.0)).xyz;
    out.world_normal = n;
}
@if(VERTEX_POSITIONS) {
    let p = mesh_functions::mesh_position_local_to_world(world_from_local, vec4(vertex.position, 1.0)).xyz;
    let to_eye = view_bindings::view.world_position - p;
    let d2 = dot(to_eye, to_eye);
    let k = saturate(reach.x + sqrt(d2) * OVERLAP.x);
    let moved = p + NORMAL_PUSH * n * k - (0.5 * OVERLAP.z) * k * to_eye * inverseSqrt(d2);
    out.position = position_world_to_clip(moved);
    out.world_position = vec4(moved, gl::fog_amount(light, moved));
}
@if(VERTEX_COLORS)
    out.color = vertex.color;
@if(VERTEX_OUTPUT_INSTANCE_INDEX)
    out.instance_index = v.instance_index;
    return out;
}
