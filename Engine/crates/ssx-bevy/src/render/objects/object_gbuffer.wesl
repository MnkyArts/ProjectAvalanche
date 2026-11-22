import bevy_pbr::prepass::io as prepass_io;
import bevy_pbr::render::{
    pbr_bindings,
    pbr_functions::alpha_discard,
};
import package::render::lighting::game_lighting as gl;
import package::render::objects::object_normal::{ObjectParams, SHADOW_ONLY, NEUTRAL_COV, gbuffer_normal};
import package::render::objects::shadow_alpha::shadow_alpha_test;
@if(SSX_HOOK_SURFACE)
import super::hook;
@if(SSX_HOOK_SURFACE)
import package::render::objects::hook_types::{SurfaceIn, Surface};
@if(SSX_HOOK_SURFACE)
import bevy_render::globals::Globals;

@group(constants::MATERIAL_BIND_GROUP) @binding(101) var<uniform> params: ObjectParams;
@group(constants::MATERIAL_BIND_GROUP) @binding(102) var normal_map: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(103) var object_sampler: sampler;
@group(constants::MATERIAL_BIND_GROUP) @binding(104) var specular_map: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(105) var scatter_map: texture_2d<f32>;
@if(SSX_HOOK_SURFACE)
@group(0) @binding(1) var<uniform> prepass_globals: Globals;

struct Fetched {
    tex_n: vec2<f32>,
    scatter: vec3<f32>,
    cov: f32,
}

fn main_pass_kills(in: prepass_io::VertexOutput) -> Fetched {
    let family = u32(round(params.shading.x));
    if family == SHADOW_ONLY {
        discard;
    }
    let p = in.world_position.xyz;
    var cov = NEUTRAL_COV;
@if(VERTEX_UVS_B)
    cov = 2.0 * in.uv_b.y - 1.0;
    var tex_n = vec2(0.5);
    var base = pbr_bindings::material.base_color;
@if(VERTEX_COLORS)
    base = in.color * base;
@if(VERTEX_UVS_A) {
    tex_n = textureSampleBias(normal_map, object_sampler, in.uv * params.tiles.xy, params.lod_bias.y).xy;
    base = gl::diffuse_texel(in.uv, params.lod_bias.x);
}
    let scatter = textureSampleBias(scatter_map, object_sampler, p.xz * params.scatter.x, params.lod_bias.w).xyz;
@if(SSX_HOOK_SURFACE && NORMAL_PREPASS) {
    var spec_map = 1.0;
    var hook_uv = vec2(0.0);
    var hook_uv_b = vec2(0.0);
@if(VERTEX_UVS_A) {
    spec_map = textureSampleBias(specular_map, object_sampler, in.uv * params.tiles.zw, params.lod_bias.z).y;
    hook_uv = in.uv;
}
@if(VERTEX_UVS_B)
    hook_uv_b = in.uv_b;
    let hooked = hook::surface(
        SurfaceIn(hook_uv, hook_uv_b, p, normalize(in.world_normal), gl::to_eye(p), prepass_globals.time),
        Surface(base, tex_n, spec_map, vec3(0.0)),
        pbr_bindings::base_color_texture,
        pbr_bindings::base_color_sampler,
        normal_map,
        specular_map,
        object_sampler,
    );
    base = hooked.base_color;
    tex_n = hooked.normal_xy;
}
    let m = pbr_bindings::material;
    if alpha_discard(m.flags, m.alpha_cutoff, base).a < params.alpha_test.x {
        discard;
    }
    return Fetched(tex_n, scatter, cov);
}

@if(NORMAL_PREPASS)
fn gbuffer(in: prepass_io::VertexOutput) -> vec3<f32> {
    if params.alpha_test.y > 0.0 {
        discard;
    }
    let f = main_pass_kills(in);
    let family = u32(round(params.shading.x));
    var tangent = vec4(0.0);
    var mapped = false;
@if(VERTEX_TANGENTS && VERTEX_UVS_A) {
    tangent = in.world_tangent;
    mapped = params.scatter.w > 0.0;
}
    return gbuffer_normal(in.world_normal, tangent, mapped, f.tex_n, family, params, f.cov, f.scatter).n;
}

@if(PREPASS_FRAGMENT)
@fragment
fn fragment(in: prepass_io::VertexOutput) -> prepass_io::FragmentOutput {
    var out: prepass_io::FragmentOutput;
@if(NORMAL_PREPASS)
    out.normal = vec4(gbuffer(in) * 0.5 + vec3(0.5), 1.0);
@elif(SSX_DEPTH_PREPASS)
    _ = main_pass_kills(in);
@else
    shadow_alpha_test(in);
    return out;
}
@else
@fragment
fn fragment(in: prepass_io::VertexOutput) {
@if(SSX_DEPTH_PREPASS)
    _ = main_pass_kills(in);
@else
    shadow_alpha_test(in);
}
