import bevy_pbr::render::{
    forward_io::{VertexOutput, FragmentOutput},
    pbr_fragment::pbr_input_from_standard_material,
    pbr_functions::main_pass_post_lighting_processing,
};
import bevy_pbr::render::mesh_view_bindings as view_bindings;
import package::render::lighting::game_lighting as gl;

@group(constants::MATERIAL_BIND_GROUP) @binding(100) var<storage, read> light: gl::GameLight;
@group(constants::MATERIAL_BIND_GROUP) @binding(101) var<uniform> reach: vec4<f32>;

@fragment
fn fragment(in: VertexOutput, @builtin(front_facing) is_front: bool) -> FragmentOutput {
    let pbr_input = pbr_input_from_standard_material(in, is_front);
    let n = normalize(in.world_normal);
    var ao = 1.0;
@if(VERTEX_COLORS)
    ao = 1.0 + light.specular.w * (in.color.z - 1.0);
    let diffuse = gl::forward_ramp(light, n, gl::to_eye(in.world_position.xyz));
    let c = 0.5 * diffuse * mix(light.ramp_shade.rgb, vec3(1.0), ao);
    let fogged = gl::apply_fog(light, c, in.world_position.w);
    // PORT(choice): a discard at the interpolated view depth, the same pixels as the plane (both linear in the world
    // position); the console clipped with D3D user clip plane 0 = (0, 0, 1, -z/w of 480 m), which wgpu offers only as
    // the optional CLIP_DISTANCES feature.
    let view_depth = -dot(vec4(
        view_bindings::view.view_from_world[0].z,
        view_bindings::view.view_from_world[1].z,
        view_bindings::view.view_from_world[2].z,
        view_bindings::view.view_from_world[3].z,
    ), vec4(in.world_position.xyz, 1.0));
    if view_depth < reach.y {
        discard;
    }
    var out: FragmentOutput;
    out.color = main_pass_post_lighting_processing(pbr_input, vec4(fogged, 1.0));
    return out;
}
