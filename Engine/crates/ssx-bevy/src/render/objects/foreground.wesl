import bevy_pbr::render::{
    pbr_fragment::pbr_input_from_standard_material,
    forward_io::{VertexOutput, FragmentOutput},
    pbr_functions::main_pass_post_lighting_processing,
};
import package::render::lighting::game_lighting as gl;

struct ForegroundParams {
    tiles: vec4<f32>,
    ao_tile: vec4<f32>,
}

@group(constants::MATERIAL_BIND_GROUP) @binding(100) var<uniform> fg: ForegroundParams;
@group(constants::MATERIAL_BIND_GROUP) @binding(101) var mask_texture: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(102) var base_texture: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(103) var red_texture: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(104) var green_texture: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(105) var fg_sampler: sampler;
@group(constants::MATERIAL_BIND_GROUP) @binding(106) var<storage, read> light: gl::GameLight;
@group(constants::MATERIAL_BIND_GROUP) @binding(107) var occlusion_texture: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(108) var normal_texture: texture_2d<f32>;

@fragment
fn fragment(in: VertexOutput, @builtin(front_facing) is_front: bool) -> FragmentOutput {
    let pbr_input = pbr_input_from_standard_material(in, is_front);
    var n = normalize(in.world_normal);
    var alb = vec3(0.5);
    var ao = 1.0;
@if(VERTEX_UVS_A) {
    let uv = in.uv;
    let base = textureSample(base_texture, fg_sampler, uv * fg.tiles.x).rgb;
    let red = textureSample(red_texture, fg_sampler, uv * fg.tiles.y).rgb;
    let green = textureSample(green_texture, fg_sampler, uv * fg.tiles.z).rgb;
    let occ = textureSample(occlusion_texture, fg_sampler, uv * fg.ao_tile.xy).y;
    let m = textureSample(mask_texture, fg_sampler, uv).rgb;
    let tn = textureSample(normal_texture, fg_sampler, uv).xy;
    ao = occ * occ;
    alb = mix(base * base, green * green, m.g);
    alb = mix(alb, vec3(0.5), saturate(m.b + fg.tiles.w));
    alb = mix(alb, red * red, m.r);
@if(VERTEX_TANGENTS) {
    if fg.ao_tile.z > 0.0 {
        let nt = normalize(vec3(tn * 2.0 - 1.0, 1.0));
        n = gl::game_tbn(in.world_normal, in.world_tangent, nt);
    }
}
}
    let diffuse = gl::forward_ramp(light, n, gl::to_eye(in.world_position.xyz));
    let c = diffuse * mix(light.ramp_shade.rgb, vec3(1.0), ao) * alb;
    let fogged = gl::apply_fog(light, c, in.world_position.w);
    var out: FragmentOutput;
    out.color = main_pass_post_lighting_processing(pbr_input, vec4(fogged, 1.0));
    return out;
}
