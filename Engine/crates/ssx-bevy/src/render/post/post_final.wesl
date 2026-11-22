import bevy_core_pipeline::fullscreen_vertex_shader::FullscreenVertexOutput;
import package::render::post::post_common::{PostUniform, Exposure, hable};

@group(0) @binding(0) var<uniform> u: PostUniform;
@group(0) @binding(1) var scene: texture_2d<f32>;
@group(0) @binding(2) var bloom: texture_2d<f32>;
@group(0) @binding(3) var samp: sampler;
@group(0) @binding(4) var lut: texture_3d<f32>;
@group(0) @binding(5) var<storage, read> exposure: Exposure;

struct OverlayUniform {
    tint0: vec4<f32>,
    tint1: vec4<f32>,
    fadeoffbounds: vec4<f32>,
    uvscale: vec4<f32>,
}
@group(0) @binding(6) var<uniform> overlay: OverlayUniform;
@group(0) @binding(7) var overlay_map0: texture_2d<f32>;
@group(0) @binding(8) var overlay_map1: texture_2d<f32>;
@group(0) @binding(9) var overlay_sampler: sampler;

struct TopogridUniform {
    world_from_clip: mat4x4<f32>,
    back: vec4<f32>,
    colour: vec4<f32>,
    gridcolour: vec4<f32>,
    invrange: vec4<f32>,
    origin: vec4<f32>,
    scale: vec4<f32>,
}
@group(0) @binding(10) var<uniform> topogrid: TopogridUniform;
@group(0) @binding(11) var grid_texture: texture_2d<f32>;
@group(0) @binding(12) var grid_sampler: sampler;
@group(0) @binding(13) var gbuffer_depth: texture_depth_2d;
@group(0) @binding(14) var gbuffer_normal: texture_2d<f32>;

fn apply_overlay(c: vec3<f32>, uv: vec2<f32>) -> vec3<f32> {
    // TODO(guess): beyond 16:9 the circle's 16:9 size across (k = 0.9 · 16:9 / a): at k = 0.9 a 21:9 frame's sides
    // reach |q| = 1.06, outside what any 16:9 frame (corners 0.918) or 4:3 frame (0.833) showed, and the survival and
    // pause fades go black there; below 16:9, k = 0.9 (the 4:3 branch has k = 1 but drew the game's 16:9 view; the
    // port's narrower window shows more above and below instead); settle: a 4:3 console capture of a survival overlay
    // next to the port's at 4:3 (no console renders wider than 16:9).
    let q0 = uv * overlay.uvscale.xy - overlay.uvscale.zw;
    let d = textureDimensions(scene);
    let r = f32(d.x) / f32(d.y) / (16.0 / 9.0);
    let s = select(vec2<f32>(r, 1.0), vec2<f32>(1.0, 1.0 / r), r > 1.0);
    let q = select(q0 * s, q0, d.x * 9u == d.y * 16u);
    let fob = overlay.fadeoffbounds;
    let span = fob.y - fob.x;
    var t = select(0.0, 1.0, length(q) > fob.x);
    if span != 0.0 {
        t = saturate((length(q) - fob.x) / span);
    }
    let m0 = textureSampleLevel(overlay_map0, overlay_sampler, uv, 0.0);
    let m1 = textureSampleLevel(overlay_map1, overlay_sampler, uv, 0.0);
    let a0 = saturate(m0.a * overlay.tint0.w - 1.0);
    let a1 = saturate(m1.a * overlay.tint1.w - 1.0);
    let x = mix(c, m1.rgb * overlay.tint1.rgb, a1);
    let k = max(t * t * (3.0 - 2.0 * t), fob.z);
    return mix(x * k, m0.rgb * overlay.tint0.rgb, a0);
}

fn apply_topogrid(c: vec3<f32>, p: vec2<i32>, uv: vec2<f32>) -> vec3<f32> {
    let depth = textureLoad(gbuffer_depth, p, 0);
    let clip = vec4<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, depth, 1.0);
    let h = topogrid.world_from_clip * clip;
    let w = h.xyz / h.w;
    let g = textureSample(grid_texture, grid_sampler, w.xy * topogrid.scale.x).x;
    let to = w - topogrid.origin.xyz;
    let d2 = dot(to, to);
    let ab = saturate(d2 * topogrid.invrange.yx + topogrid.invrange.wz);
    let n = normalize(textureLoad(gbuffer_normal, p, 0).xyz * 2.0 - 1.0);
    let up = saturate(dot(n, topogrid.back.xyz));
    let fade = ab.x * topogrid.origin.w;
    let kill = fade * ab.y;
    let ring = ab.y * ab.x;
    let tint = topogrid.colour.rgb * up * (1.0 - g) + g * topogrid.gridcolour.rgb;
    return select(c * (1.0 - kill) + ring * tint, c, h.w == 0.0);
}

fn srgb_to_linear(c: vec3<f32>) -> vec3<f32> {
    return select(pow((c + 0.055) / 1.055, vec3<f32>(2.4)), c / 12.92, c <= vec3<f32>(0.04045));
}

@fragment
fn tonemap_pass(in: FullscreenVertexOutput) -> @location(0) vec4<f32> {
    let p = vec2<i32>(floor(in.position.xy));
    let c = max(textureLoad(scene, p, 0).rgb, vec3<f32>(0.0)) * u.exposure.x;
    let t = hable(c * exposure.exposure, u.hable_a, u.hable_b) * u.hable_b.z;
    return vec4<f32>(sqrt(clamp(t, vec3<f32>(0.0), vec3<f32>(1.0))), 1.0);
}

fn composite(in: FullscreenVertexOutput) -> vec3<f32> {
    let base = textureLoad(scene, vec2<i32>(floor(in.position.xy)), 0).rgb;
    var lin = base * base;
    if (u.bloom.z != 0.0) {
        let b = textureSampleLevel(bloom, samp, in.uv, 0.0).rgb;
        lin += b * b * u.bloom.y;
    }
    let g = sqrt(lin);
    var l = min(g, vec3<f32>(1.0));
    if (u.grade.x != 0.0) {
        l = textureSampleLevel(lut, samp, g * (31.0 / 32.0) + 1.0 / 64.0, 0.0).rgb;
    }
    var v = clamp(l, vec3<f32>(0.0), vec3<f32>(1.0));
    let topo = topogrid.colour.w > 0.0;
    if overlay.fadeoffbounds.w > 0.0 || topo {
        var sq = l * l;
        if overlay.fadeoffbounds.w > 0.0 {
            sq = apply_overlay(sq, in.uv);
        }
        if topo {
            sq = apply_topogrid(sq, vec2<i32>(floor(in.position.xy)), in.uv);
        }
        v = clamp(sqrt(max(sq, vec3<f32>(0.0))), vec3<f32>(0.0), vec3<f32>(1.0));
    }
    return v;
}

@fragment
fn composite_pass(in: FullscreenVertexOutput) -> @location(0) vec4<f32> {
    return vec4<f32>(srgb_to_linear(composite(in)), 1.0);
}

@fragment
fn composite_codes_pass(in: FullscreenVertexOutput) -> @location(0) vec4<f32> {
    return vec4<f32>(composite(in), 1.0);
}
