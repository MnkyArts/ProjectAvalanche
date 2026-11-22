import bevy_pbr::render::{
    forward_io::{VertexOutput, FragmentOutput},
    pbr_fragment::pbr_input_from_standard_material,
    pbr_functions::main_pass_post_lighting_processing,
};
import package::render::lighting::game_lighting as gl;

struct HeliTranslucentParams {
    shading: vec4<f32>,
    tiles: vec4<f32>,
}

struct LdrTonemap {
    hable_a: vec4<f32>,
    hable_b: vec4<f32>,
    exposure: vec4<f32>,
}

@group(constants::MATERIAL_BIND_GROUP) @binding(100) var<storage, read> light: gl::GameLight;
@group(constants::MATERIAL_BIND_GROUP) @binding(101) var<uniform> params: HeliTranslucentParams;
@group(constants::MATERIAL_BIND_GROUP) @binding(102) var normal_map: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(103) var object_sampler: sampler;
@group(constants::MATERIAL_BIND_GROUP) @binding(104) var specular_map: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(122) var<storage, read> ldr_tonemap: LdrTonemap;

@fragment
fn fragment(in: VertexOutput, @builtin(front_facing) is_front: bool) -> FragmentOutput {
    var pbr_input = pbr_input_from_standard_material(in, is_front);
    var d = vec4(1.0);
    var tex_n = vec2(0.5);
    var spec_map = 1.0;
@if(VERTEX_UVS_A) {
    d = gl::diffuse_texel(in.uv, 0.0);
    tex_n = textureSample(normal_map, object_sampler, in.uv * params.tiles.xy).xy;
    spec_map = textureSample(specular_map, object_sampler, in.uv * params.tiles.zw).y;
}
    var ao = 1.0;
@if(VERTEX_UVS_B)
    ao = in.uv_b.x;
    let p = in.world_position.xyz;
    var n = normalize(in.world_normal);
@if(VERTEX_TANGENTS)
    n = gl::game_tbn(in.world_normal, in.world_tangent, normalize(vec3(tex_n * 2.0 - 1.0, 1.0)));
    let v = gl::to_eye(p);
    let albedo = d.rgb * d.rgb;
    let dif = gl::forward_ramp(light, n, v) * mix(light.ramp_shade.rgb, vec3(1.0), ao);
    let phong = pow(saturate(dot(v, reflect(-light.light_dir.xyz, n))), max(params.shading.y, 1e-4));
    let spec = params.shading.x * spec_map;
    var col = dif * albedo + phong * light.specular.rgb * ao * spec * spec;
    let glow = params.shading.z * params.shading.w * d.a * albedo;
    let gl_mult = max(light.misc.y, 1e-6);
    col = gl::apply_fog(light, col + glow / gl_mult, in.world_position.w);
    let alpha = max(d.a, params.shading.z);
    var out: FragmentOutput;
@if(SSX_LDR_TRANSLUCENT) {
    let t = ldr_tonemap;
    let x = col * t.exposure.x * t.exposure.y;
    let h = (x * (t.hable_a.x * x + t.hable_a.z) + t.hable_a.w) / (x * (t.hable_a.x * x + t.hable_a.y) + t.hable_b.x)
        - t.hable_b.y;
    out.color = vec4(sqrt(abs(h * t.hable_b.z)), alpha);
} @else {
    out.color = main_pass_post_lighting_processing(pbr_input, vec4(col, alpha));
}
    return out;
}
