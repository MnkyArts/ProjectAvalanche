import bevy_pbr::render::forward_io::VertexOutput;
import bevy_pbr::render::mesh_view_bindings as view_bindings;
import package::render::lighting::game_lighting as gl;

@group(constants::MATERIAL_BIND_GROUP) @binding(0) var<storage, read> point: vec4<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(1) var<storage, read_write> result: array<vec4<f32>>;

fn query_debug(p: vec3<f32>) -> vec2<f32> {
    let sun = gl::shadow_light(3u, 0.0);
    if sun == gl::NO_LIGHT {
        return vec2(-1.0, 0.0);
    }
    let light = &view_bindings::lights.directional_lights[sun];
    let wp = vec4(p, 1.0);
    var cascade = 3u;
    var c = vec3(0.0);
    let edge = 0.5 - gl::PCF_HALF_WIDTH / gl::shadow_size();
    for (var i = 0u; i < 3u; i++) {
        let clip = (*light).cascades[i].clip_from_world * wp;
        let ci = vec3(0.5 * clip.x, -0.5 * clip.y, 0.5 - clip.z);
        if cascade == 3u && all(abs(ci.xy) < vec2(edge)) && abs(ci.z) < 0.5 {
            cascade = i;
            c = ci;
        }
    }
    if cascade == 3u {
        return vec2(3.0, 0.0);
    }
    let thr = 1.0 - (saturate(c.z + 0.5) - gl::shadow_bias(cascade));
    let layer = i32((*light).depth_texture_base_index + cascade);
    let t = vec2<i32>(floor((c.xy + 0.5) * gl::shadow_size() + 1.0));
@if(NO_ARRAY_TEXTURES_SUPPORT) {
    return vec2(f32(cascade), 0.0);
} @else {
    let d = textureLoad(view_bindings::directional_shadow_textures, t, layer, 0);
    return vec2(f32(cascade), thr - d);
}
}

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    var lit = 1.0;
    var debug = vec2(-1.0, 0.0);
    if point.w > 0.0 {
        lit = round(saturate(gl::shadow_query(point.xyz)) * 255.0) / 255.0;
        debug = query_debug(point.xyz);
    }
    result[0] = vec4(lit, point.w, debug.x, debug.y);
    return vec4(0.0);
}
