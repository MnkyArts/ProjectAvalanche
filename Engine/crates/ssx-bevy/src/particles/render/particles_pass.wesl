import bevy_core_pipeline::fullscreen_vertex_shader::FullscreenVertexOutput;
import package::render::post::post_common::{PostUniform, Exposure, hable};
import package::particles::render::particle_globals::Globals;

@group(0) @binding(0) var<uniform> post: PostUniform;
@group(0) @binding(1) var<uniform> pg: Globals;
@group(0) @binding(2) var src: texture_2d<f32>;
@group(0) @binding(3) var src_sampler: sampler;
@group(0) @binding(4) var<storage, read> exposure: Exposure;
@group(0) @binding(5) var scene_depth: texture_depth_2d;
@group(0) @binding(6) var half_depth: texture_2d<f32>;

struct DepthOut {
    @location(0) depth_copy: vec4<f32>,
    @builtin(frag_depth) depth: f32,
}

@fragment
fn depth_filter(in: FullscreenVertexOutput) -> DepthOut {
    let full = vec2<i32>(pg.sizes.zw);
    let p = vec2<i32>(in.position.xy) * 2;
    let a = textureLoad(scene_depth, min(p, full - 1), 0);
    let b = textureLoad(scene_depth, min(p + vec2(1, 0), full - 1), 0);
    let c = textureLoad(scene_depth, min(p + vec2(0, 1), full - 1), 0);
    let e = textureLoad(scene_depth, min(p + vec2(1, 1), full - 1), 0);
    let m = min(min(a, b), min(c, e));
    var out: DepthOut;
    out.depth_copy = vec4(m, 0.0, 0.0, 0.0);
    out.depth = m;
    return out;
}

fn particle_tonemap(s: vec4<f32>, e: f32) -> vec4<f32> {
    let t = s.a;
    let x = s.rgb * s.rgb * (1.0 / (1.0 - t)) * post.exposure.x * e;
    let h = hable(x, post.hable_a, post.hable_b) * post.hable_b.z;
    return vec4(sqrt(abs(h * (1.0 - t))), t);
}

@fragment
fn tonemap(in: FullscreenVertexOutput) -> @location(0) vec4<f32> {
    return particle_tonemap(textureSample(src, src_sampler, in.uv), exposure.exposure);
}

@fragment
fn bloom(in: FullscreenVertexOutput) -> @location(0) vec4<f32> {
    return particle_tonemap(textureSample(src, src_sampler, in.uv), exposure.bloom_exposure);
}

fn view_dist(depth: f32) -> f32 {
    let p = pg.view_from_clip * vec4(0.0, 0.0, depth, 1.0);
    return -p.z / p.w;
}

@fragment
fn composite(in: FullscreenVertexOutput) -> @location(0) vec4<f32> {
    let full = vec2<i32>(pg.sizes.zw);
    let half = vec2<i32>(pg.sizes.xy);
    let off = 1.0 / pg.sizes.zw;
    let uv_p = in.uv + off;
    let uv_m = in.uv - off;
    let max_d = 500.0;
    let d0 = min(view_dist(textureLoad(scene_depth, clamp(vec2<i32>(in.position.xy), vec2(0), full - 1), 0)), max_d);
    let dp = min(view_dist(textureLoad(half_depth, clamp(vec2<i32>(uv_p * pg.sizes.xy), vec2(0), half - 1), 0).x), max_d);
    let dm = min(view_dist(textureLoad(half_depth, clamp(vec2<i32>(uv_m * pg.sizes.xy), vec2(0), half - 1), 0).x), max_d);
    let wp = 1.0 / (abs(d0 - dp) + 1.0);
    let wm = 1.0 / (abs(d0 - dm) + 1.0);
    let sp = textureSample(src, src_sampler, uv_p);
    let sm = textureSample(src, src_sampler, uv_m);
    return (wp * sp + wm * sm) / (wp + wm);
}

@fragment
fn depth_copy(in: FullscreenVertexOutput) -> @location(0) vec4<f32> {
    let full = vec2<i32>(pg.sizes.zw);
    return vec4(textureLoad(scene_depth, clamp(vec2<i32>(in.position.xy), vec2(0), full - 1), 0), 0.0, 0.0, 0.0);
}
