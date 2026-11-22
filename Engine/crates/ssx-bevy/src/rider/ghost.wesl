import bevy_pbr::render::{
    forward_io::VertexOutput,
    mesh_view_bindings::view,
};

struct GhostParams {
    ghost_color: vec4<f32>,
    ghost_base_color: vec4<f32>,
    shading: vec4<f32>,
}

struct GhostFrame {
    tint: vec4<f32>,
    hologlitch: vec4<f32>,
}

@group(constants::MATERIAL_BIND_GROUP) @binding(0) var<uniform> params: GhostParams;
@group(constants::MATERIAL_BIND_GROUP) @binding(1) var normal_texture: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(2) var normal_sampler: sampler;
@group(constants::MATERIAL_BIND_GROUP) @binding(3) var<storage, read> frame: GhostFrame;

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
@if(VERTEX_UVS_A)
    let uv = in.uv;
@else
    let uv = vec2(0.0);
    let t = textureSample(normal_texture, normal_sampler, uv).xy;
    let ts = normalize(vec3(1.0, 2.0 * t.y - 1.0, 2.0 * t.x - 1.0));
    let n_geo = normalize(in.world_normal);
@if(VERTEX_TANGENTS)
    let tangent = normalize(in.world_tangent.xyz);
@else
    let tangent = vec3(0.0);
    let b = cross(n_geo, tangent);
    let n = normalize(ts.x * n_geo + ts.y * b + ts.z * tangent);
    let v = normalize(view.world_position - in.world_position.xyz);
    let fres = pow(saturate((1.0 - abs(dot(n, v))) * (1.0 + params.shading.z)), params.shading.y);
    let c = mix(params.ghost_base_color.rgb, params.ghost_color.rgb, fres);
    let clip_x = (view.clip_from_world * vec4(in.world_position.xyz, 1.0)).x;
    let stripe = 0.5 * (1.0 + 2.0 * abs(fract(clip_x * 60.0) - 0.5));
    let col = c * stripe;
    let luma = dot(col, vec3(0.299, 0.587, 0.114));
    let nz = (view.view_from_world * vec4(n, 0.0)).z;
    let hb = frame.hologlitch.y;
    var keep = 1.0;
    var glitch = 0.0;
    if hb > 0.0 {
        keep = select(0.0, 1.0, 0.4 + nz - 1.4 * hb > 0.0);
        glitch = select(0.0, 1.0, 1.4 * hb > 0.3 + nz);
    }
    let rgb = col * (1.0 - glitch) * frame.tint.rgb * params.shading.x;
    let a = frame.tint.a * luma * keep;
    if params.shading.w > 0.5 {
        return vec4(sqrt(abs(rgb)), a);
    }
    return vec4(min(rgb, vec3(1.0)), a);
}
