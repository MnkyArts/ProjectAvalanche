import bevy_pbr::render::{
    pbr_fragment::pbr_input_from_standard_material,
    forward_io::{VertexOutput, FragmentOutput},
    pbr_functions::main_pass_post_lighting_processing,
    mesh_view_bindings as view_bindings,
    mesh_types::MESH_FLAGS_SHADOW_RECEIVER_BIT,
};
import package::render::lighting::game_lighting as gl;
import package::render::terrain::terrain_gbuffer::{
    terrain, POWDER, Surface, snow_specular, planar_weights, base_diffuse, inputs, gbuffer,
};

@if(SSX_LOCAL_LIGHTS)
var<private> light: gl::GameLight;
@else
@group(constants::MATERIAL_BIND_GROUP) @binding(106) var<uniform> light: gl::GameLight;
@group(constants::MATERIAL_BIND_GROUP) @binding(107) var reflect_cube: texture_cube<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(108) var cube_sampler: sampler;
@group(constants::MATERIAL_BIND_GROUP) @binding(109) var sparkle_normal: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(110) var sparkle_sampler: sampler;
const SNOW_ALBEDO: f32 = 0.5;

@fragment
fn fragment(in: VertexOutput, @builtin(front_facing) is_front: bool) -> FragmentOutput {
@if(SSX_LOCAL_LIGHTS)
    light = gl::frame_light();
    var pbr_input = pbr_input_from_standard_material(in, is_front);

    let ti = inputs(in.world_position.xyz, in.color, in.uv, in.uv_b);
    let s: Surface = ti.s;
    let cov = ti.cov;
    let b = ti.b;
    let base_bits = ti.base_bits;
    let group = ti.group;
    let view_z = ti.view_z;
    let fade_snow = ti.fade_snow;
    let fade_base = ti.fade_base;
    let wp = vec4(in.world_position.xyz, 1.0);
    let n = normalize(in.world_normal);
    let g = gbuffer(ti, in.world_normal);
    let normal = g.normal;
    let ecc = g.ecc;
    let snow_spec_types = g.snow_spec_types;

    let sb = terrain.snow_blend;
    let fall = dot(normal, -normalize(terrain.snow_fall.xyz));
    let x = saturate(cov + sb.x + fall);
    let t = saturate((x - (sb.y - sb.z)) / max(2.0 * sb.z, 1e-6));
    var cvg = mix(1.0, t * t * (3.0 - 2.0 * t), fade_base);
    if base_bits == 0u {
        cvg = 1.0;
    }
    let snow_spec = mix(snow_specular(POWDER), snow_spec_types, fade_snow);
    var rock = vec3(0.0);
    var rock_spec = 0.0;
    if cvg < 1.0 {
        let w = planar_weights(in.world_normal);
        var d1 = vec4(0.0);
        var d2 = vec4(0.0);
        if b != 1.0 { d1 = base_diffuse(0u, s, w); }
        if b != 0.0 { d2 = base_diffuse(1u, s, w); }
        rock = mix(d1.rgb * d1.rgb, d2.rgb * d2.rgb, b);
        rock_spec = mix(terrain.base[0].w * d1.a * d1.a, terrain.base[1].w * d2.a * d2.a, b);
    }
    let albedo = mix(rock, vec3(SNOW_ALBEDO), cvg);
    let specular = mix(rock_spec, snow_spec, cvg);

    let v = gl::to_eye(in.world_position.xyz);
    let receiver = (pbr_input.flags & MESH_FLAGS_SHADOW_RECEIVER_BIT) != 0u;
    let shadow = gl::sun_shadow(wp, n, view_z, in.position.xy, receiver);
    let lit = gl::key_light(light, normal, v, ecc, shadow, gl::contact_shadow(in.world_position.xyz), 1.0);
    let diffuse_light = lit.rgb;
    let specular_light = lit.a;

    let ao = 1.0 + light.specular.w * (in.uv_b.x - 1.0);
    let ao_tint = mix(light.ramp_shade.rgb, vec3(1.0), ao);
    let r_view = (view_bindings::view.view_from_world * vec4(reflect(-v, normal), 0.0)).xyz;
    let cube = textureSampleGrad(reflect_cube, cube_sampler, r_view, dpdx(r_view), dpdy(r_view)).rgb;
    let f0 = terrain.reflect.y;
    let fresnel = f0 + (1.0 - f0) * (1.0 - abs(dot(normal, v)));
@if(SSX_TERRAIN_SIMPLE)
    let env = vec3(0.0);
@else
    let env = pow(max(fresnel * cube, vec3(1e-6)), vec3(terrain.reflect.x)) * (1.0 - cvg) * fade_snow;
    var glint = 0.0;
    if group != 2u && group != 3u {
        let tile = light.sparkle.x;
        let sp = textureSampleGrad(sparkle_normal, sparkle_sampler, s.p.xz * tile, s.dx.xz * tile, s.dy.xz * tile);
        glint = cvg * gl::snow_glint(light, sp.xyz * 2.0 - 1.0, normal, v, diffuse_light);
    }
    let col = diffuse_light * ao_tint * albedo
        + specular * (light.specular.rgb * specular_light * ao + env)
        + glint;

    let fogged = gl::apply_fog(light, col, in.world_position.w);
    var out: FragmentOutput;
    out.color = main_pass_post_lighting_processing(pbr_input, vec4(fogged, 1.0));
    return out;
}
