import package::render::post::post_common::LUMA;
import package::particles::render::particle_globals::Globals;

struct Draw {
    world: mat4x4<f32>,
    time: vec4<f32>,
    stretch: vec4<f32>,
    color_mult: vec4<f32>,
    color_spread: vec4<f32>,
    ramp: array<vec4<f32>, 4>,
    ramp_times: vec4<f32>,
    size_ramp: vec4<f32>,
    size_ramp_times: vec4<f32>,
    flags: vec4<f32>,
    size_mult: vec4<f32>,
    pivot: vec4<f32>,
    fade: vec4<f32>,
    feather: vec4<f32>,
    light_pos: vec4<f32>,
    light_col: vec4<f32>,
    light_misc: vec4<f32>,
    anim: vec4<f32>,
    anim2: vec4<f32>,
    modulo: vec4<f32>,
    modulo2: vec4<f32>,
    glare: array<vec4<f32>, 4>,
}

@group(0) @binding(0) var<uniform> g: Globals;
@group(0) @binding(1) var<uniform> d: Draw;
@group(0) @binding(2) var<storage, read> records: array<vec4<f32>>;
@group(0) @binding(3) var tex: texture_2d<f32>;
@group(0) @binding(4) var tex_sampler: sampler;
@group(0) @binding(5) var particle_depth: texture_2d<f32>;

const TAU: f32 = 6.2831855;
const PI: f32 = 3.1415927;

fn ramp4(t: f32, k0: vec4<f32>, k1: vec4<f32>, k2: vec4<f32>, k3: vec4<f32>, times: vec4<f32>) -> vec4<f32> {
    let a = saturate((vec3(t) - times.xyz) / (times.yzw - times.xyz));
    let c01 = mix(k0, k1, a.x);
    let c12 = mix(c01, k2, a.y);
    return mix(c12, k3, a.z);
}

fn spread_hash(w: f32) -> vec4<f32> {
    let f = fract(w * vec4(738.5924, 65.45774, 7.80416, 0.73196197));
    let f2 = fract(w * vec2(5.907823, 0.104400896));
    let rx = fract(f.x + (f.y + (((f.z + f.w) + f2.x) + f2.y)));
    let ya = fract(w * vec3(3.0987134, 0.054759335, 0.3839215));
    let yb = fract(w * vec3(4.0933614, 34.333252, 387.39926));
    let ry = fract(((((ya.x + ya.y) + ya.z) + yb.x) + yb.y) + yb.z);
    let za = fract(w * vec3(2.1902876, 0.038705964, 0.2713702));
    let zb = fract(w * vec3(2.8933425, 24.268038, 273.8284));
    let rz = fract(((((za.x + za.y) + za.z) + zb.x) + zb.y) + zb.z);
    let wa = fract(w * vec3(1.0, 0.017671635, 0.12389707));
    let wb = fract(w * vec3(1.3209875, 11.079841, 125.01939));
    let rw = fract(((((wa.x + wa.y) + wa.z) + wb.x) + wb.y) + wb.z);
    return vec4(rw, rz, ry, rx);
}

fn particle_colour(t: f32, birth: f32) -> vec4<f32> {
    var col = vec4(1.0);
    if d.flags.x > 0.5 {
        col = ramp4(t, d.ramp[0], d.ramp[1], d.ramp[2], d.ramp[3], d.ramp_times);
    }
    if d.flags.y > 0.5 {
        col = col * d.color_mult + (spread_hash(birth) - 0.5) * d.color_spread;
    }
    return col;
}

fn anim_uv(uv: vec2<f32>, birth: f32, t: f32) -> vec2<f32> {
    if d.anim.x < 0.5 {
        return uv;
    }
    let mode = d.anim.y;
    let count = d.anim.z;
    let rows = d.anim.w;
    let k = fract(birth * vec4(11.079841, 1.3209875, 0.12389707, 0.017671635));
    let rnd = fract(((k.x + k.y) + (k.z + k.w)) + fract(birth) + fract(125.01939 * birth));
    let f = max(mode * (rnd * d.anim2.y + t * count * d.anim2.x) - 1.0
        + (1.0 - mode) * (0.06 * d.anim2.x * d.time.x + rnd * d.anim2.y), 0.0);
    let fm = f - floor(f / count) * count;
    let cols = ceil(count / rows);
    let row = floor(fm / cols);
    let col = floor(fm - cols * row);
    return vec2((uv.x + col) / cols, (uv.y + row) / rows);
}

fn modulo_position(p: vec3<f32>) -> vec3<f32> {
    let dims = d.modulo.xyz;
    let wv = g.view_from_world * d.world;
    let back = vec3(wv[0].z, wv[1].z, wv[2].z);
    let c = (-d.modulo2.x - 1.1) * dims * back;
    let rel = p - g.camera_pos.xyz - c;
    var m = rel - floor(rel / dims) * dims;
    m = m - dims * step(dims * 0.5, m);
    return m + c;
}

fn particle_age(birth: f32) -> f32 {
    var t = fract(d.time.z * d.time.x);
    if d.time.y > 1e-5 {
        t = (d.time.x - birth) / d.time.y;
    }
    return t;
}

fn zbias(game_clip_z: f32) -> f32 {
    return d.fade.z / game_clip_z;
}

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uvt: vec3<f32>,
    @location(1) color: vec4<f32>,
    @location(2) screen: vec4<f32>,
    @location(3) light: vec4<f32>,
    @location(4) ao_shadow: vec4<f32>,
    @location(5) fog: vec4<f32>,
    @location(6) clip_pos: vec4<f32>,
}

const CORNERS: array<vec2<f32>, 6> = array<vec2<f32>, 6>(
    vec2(0.0, 1.0), vec2(0.0, 0.0), vec2(1.0, 0.0), vec2(0.0, 1.0), vec2(1.0, 0.0), vec2(1.0, 1.0),
);

fn bin_size() -> vec2<f32> {
    if d.modulo2.w > 0.5 {
        return g.sizes.zw;
    }
    return g.sizes.xy;
}

fn vertex_fog(dist: f32) -> vec4<f32> {
    let f = pow(saturate((dist - g.fog_data.x) / (g.fog_data.y - g.fog_data.x)), g.fog_data.z);
    return vec4(g.fog_color.rgb * g.fog_data.w * f, saturate(g.fog_color.a * g.fog_data.w * f));
}

fn ao_tint(stamp: f32) -> vec3<f32> {
    let r1 = g.ramp1.rgb;
    return r1 + (1.0 - r1) * (1.0 + (fract(stamp) - 1.0) * g.atten.y);
}

@vertex
fn vs_particle(@builtin(vertex_index) vi: u32) -> VsOut {
    var out: VsOut;
    let base = u32(d.fade.w) + (vi / 6u) * 3u;
    let pos = records[base];
    let tc0 = records[base + 1u];
    let tc1 = records[base + 2u];
    let corner = CORNERS[vi % 6u];
    var t = particle_age(tc0.w);
@if(LYNX)
    t = pow(abs(t), d.time.w);
    var col = particle_colour(t, tc0.w);
    var uv = corner;
@if(SNOWFALL)
    uv = corner.yx;
    var p_local = pos.xyz;
@if(LYNX) {
    if d.modulo.w > 0.5 {
        p_local = modulo_position(p_local);
    }
}
@if(SNOWFALL)
    p_local = modulo_position(p_local);
    let wp = (d.world * vec4(p_local, 1.0)).xyz;
    let vw = g.camera_pos.xyz - wp;
    let dist = length(vw);
    var s = d.stretch.xyz;
    if d.stretch.w > 0.5 {
        s = tc0.xyz;
    }
@if(SNOWFALL)
    let cam_d = (g.world_from_view * vec4(((g.view_from_world - g.view_prev) * vec4(wp, 1.0)).xyz, 0.0)).xyz;
@if(SNOWFALL) {
    if d.stretch.w > 0.5 {
        s = tc0.xyz + 18.0 * d.modulo2.z * cam_d;
    }
}
    var sr = 1.0;
@if(!INSTANCE) {
    if d.flags.z > 0.5 {
        sr = ramp4(t, vec4(d.size_ramp.x), vec4(d.size_ramp.y), vec4(d.size_ramp.z), vec4(d.size_ramp.w), d.size_ramp_times).x;
    }
}
    let pu = uv.x + d.pivot.x - 0.5;
    let pv = uv.y + d.pivot.y - 0.5;
@if(SPRAY)
    let align = d.size_mult.z;
@elif(LYNX)
    let align = d.size_mult.z;
@else
    let align = 0.0;
    var view = vec4(0.0, 0.0, 1.0, 1.0);
    var size = pos.w;
@if(INSTANCE)
    let use_stretch = false;
@else
    let use_stretch = length(s) > 0.0;
    if use_stretch {
        var k = 1.0;
        if align <= 1.5 {
            k = 1.0 + length(s);
        }
        col.a = col.a * pow(k, -0.25);
        let sh = normalize(s);
        var a = cross(sh, normalize(vw));
        var b = sh;
        if align > 2.5 {
            b = normalize(cross(sh, a));
        }
        if d.flags.w > 0.5 {
            a = a * d.size_mult.x;
            b = b * d.size_mult.y;
        }
        a = a * sr;
        b = b * sr;
        let p = p_local - (a * pu - b * (pv * k)) * pos.w;
        view = g.view_from_world * (d.world * vec4(p, 1.0));
    } else {
        var sx = sr;
        var sy = sr;
@if(!INSTANCE) {
        if d.flags.w > 0.5 {
            sx = d.size_mult.x * sr;
            sy = d.size_mult.y * sr;
        }
}
        let ang = fract(tc0.x / TAU + 0.5) * TAU - PI;
        let dx = pu * sx;
        let dy = pv * sy;
        let rx = dx * cos(ang) - dy * sin(ang);
        let ry = -(dx * sin(ang) + dy * cos(ang));
        size = length(d.world[0].xyz) * pos.w;
@if(INSTANCE) {
        if d.size_mult.z > 0.5 {
            view = g.view_from_world * (d.world * vec4(p_local + vec3(rx, 0.0, ry), 1.0));
        } else {
            view = g.view_from_world * vec4(wp, 1.0) + vec4(size * rx, size * ry, 0.0, 0.0);
        }
} @else {
        if align < 0.5 {
            view = g.view_from_world * vec4(wp, 1.0) + vec4(size * rx, size * ry, 0.0, 0.0);
        } else if align < 1.5 {
            view = g.view_from_world * vec4(wp + vec3(size * rx, 0.0, size * ry), 1.0);
        } else {
@if(LYNX)
            view = vec4(wp, 1.0);
@else
            col.a = 0.0;
        }
}
    }
@if(!INSTANCE) {
    var fade = saturate((-d.fade.y - view.z) / (d.fade.x - d.fade.y));
    var modulo_on = false;
@if(SNOWFALL)
    modulo_on = true;
@if(LYNX)
    modulo_on = d.modulo.w > 0.5;
    if modulo_on && d.modulo2.y < 1.0 {
        fade = fade * (1.0 - saturate((-view.z - d.modulo.x) / d.modulo.x));
    }
    col.a = col.a * fade;
}
    var clip = g.clip_from_view * view;
    let gz0 = g.game_proj.x * view.z + g.game_proj.y;
@if(SNOWFALL) {
    let gz = gz0 - d.fade.z / gz0;
    let along = abs(dot(normalize(vw), cam_d));
    let smear = 1.0 + (length(cam_d) - along) / (gz * size);
    col.a = col.a / (smear * smear);
}
    clip.z = clip.z + zbias(gz0);
@if(!SNOWFALL && !INSTANCE && !ZFEATHER) {
    if col.a <= 1.0 / 256.0 {
        clip = vec4(0.0, 0.0, 0.0, -1.0);
    }
}
    out.clip = clip;
    out.clip_pos = clip;
    out.uvt = vec3(anim_uv(uv, tc0.w, t), t);
    out.color = col;
    let ndc = clip.xyz / clip.w;
    out.screen = vec4(ndc.x * 0.5 + 0.5, -ndc.y * 0.5 + 0.5, ndc.z, size);
    let shadow = floor(tc1.x) * 0.001;
    out.ao_shadow = vec4(ao_tint(tc1.x), 1.0 + (shadow - 1.0) * g.atten.x);
@if(INSTANCE)
    out.fog = vertex_fog(length(g.camera_pos.xyz - (d.world * vec4(pos.xyz, 1.0)).xyz));
@else
    out.fog = vertex_fog(dist);
    let ld = length(wp - d.light_pos.xyz) / d.light_pos.w;
    out.light = d.light_col * g.atten.z * max(1.0 - pow(ld, d.light_misc.x), 0.0);
    return out;
}

fn safe_normalize(v: vec3<f32>) -> vec3<f32> {
    let l = length(v);
    if l > 0.0 {
        return v / l;
    }
    return vec3(0.0);
}

@vertex
fn vs_ribbon(@builtin(vertex_index) vi: u32) -> VsOut {
    var out: VsOut;
    let n = u32(d.anim2.w);
    let i = vi / 2u;
    let side = f32(vi % 2u);
    let first = u32(d.fade.w);
    let rec = records[first + i * 3u];
    let birth = records[first + i * 3u + 1u].w;
    let p = (d.world * vec4(rec.xyz, 1.0)).xyz;
    let prev = (d.world * vec4(records[first + max(i, 1u) * 3u - 3u].xyz, 1.0)).xyz;
    let next = (d.world * vec4(records[first + min(i + 1u, n - 1u) * 3u].xyz, 1.0)).xyz;
    var t = 0.0;
    if d.time.y > 1e-5 {
        t = pow(abs((d.time.x - birth) / d.time.y), d.time.w);
    }
    var col = vec4(1.0);
    if d.flags.y > 0.5 {
        col = d.color_mult;
    }
    if d.flags.x > 0.5 {
        col = col * ramp4(t, d.ramp[0], d.ramp[1], d.ramp[2], d.ramp[3], d.ramp_times);
    }
    col = col + (spread_hash(birth) - 0.5) * d.color_spread;
    var size = rec.w;
    if d.flags.z > 0.5 {
        size = size * ramp4(t, vec4(d.size_ramp.x), vec4(d.size_ramp.y), vec4(d.size_ramp.z), vec4(d.size_ramp.w), d.size_ramp_times).x;
    }
    let vh = normalize(g.camera_pos.xyz - p);
    var nside: vec3<f32>;
    if i == 0u {
        nside = safe_normalize(cross(vh, safe_normalize(next - p)));
    } else if i + 1u >= n {
        nside = safe_normalize(cross(vh, safe_normalize(p - prev)));
    } else {
        nside = 0.5 * (safe_normalize(cross(vh, safe_normalize(p - prev))) + safe_normalize(cross(vh, safe_normalize(next - p))));
    }
    let wp = p + (2.0 * side - 1.0) * size * nside;
    let view = g.view_from_world * vec4(wp, 1.0);
    col.a = col.a * saturate((-d.fade.y - view.z) / (d.fade.x - d.fade.y));
    var clip = g.clip_from_view * view;
    clip.z = clip.z + zbias(g.game_proj.x * view.z + g.game_proj.y);
    out.clip = clip;
    out.clip_pos = clip;
    var v = f32(i) / max(f32(n) - 1.0, 1.0);
    if d.anim2.z > 1e-5 {
        v = (f32(n) - 1.0) * fract(v);
    }
    out.uvt = vec3(side, v, t);
    out.color = col;
    let ndc = clip.xyz / clip.w;
    out.screen = vec4(ndc.x * 0.5 + 0.5, -ndc.y * 0.5 + 0.5, ndc.z, size);
    out.ao_shadow = vec4(1.0);
    out.fog = vec4(0.0);
    out.light = vec4(0.0);
    return out;
}

fn view_z(depth: f32) -> f32 {
    let p = g.view_from_clip * vec4(0.0, 0.0, depth, 1.0);
    return p.z / p.w;
}

fn scene_dz(s: vec2<f32>, ndc_z: f32) -> f32 {
    let size = bin_size();
    let px = vec2<i32>(s * size);
    let scene = textureLoad(particle_depth, clamp(px, vec2(0), vec2<i32>(size) - 1), 0).x;
    return view_z(ndc_z) - view_z(scene);
}

fn apply_fog(c: vec3<f32>, fog: vec4<f32>) -> vec3<f32> {
    let grey = mix(c, vec3(dot(c, LUMA)), fog.a);
    return mix(grey, fog.rgb, fog.a);
}

@fragment
fn fs_spray(in: VsOut) -> @location(0) vec4<f32> {
    let texel = textureSampleBias(tex, tex_sampler, in.uvt.xy, -1.0);
    let dz = scene_dz(in.screen.xy, in.screen.z);
    let size = in.screen.w;
    let soft = saturate(d.feather.x * dz / size - d.feather.y);
    let contact = saturate(d.feather.z * dz / size - d.feather.w + 1.0 / size);
    let k = texel.y * in.ao_shadow.w * contact;
    var c = (mix(g.ramp1.rgb, g.ramp0.rgb, k) * g.ramp0.w + in.light.rgb) * in.color.rgb * in.ao_shadow.rgb;
    c = apply_fog(c, in.fog);
    let a = in.color.a * texel.w * soft;
    if a <= 1.0 / 255.0 {
        discard;
    }
    return vec4(sqrt(max(c, vec3(0.0))), a);
}

@fragment
fn fs_avalanche(in: VsOut) -> @location(0) vec4<f32> {
    let texel = textureSampleBias(tex, tex_sampler, in.uvt.xy, -1.0);
    let dz = scene_dz(in.screen.xy, in.screen.z);
    let size = in.screen.w;
    let soft = saturate(d.feather.x * dz / size - d.feather.y);
    let contact = saturate(d.feather.z * dz / size - d.feather.w + 1.0 / size);
    let k = texel.y * in.ao_shadow.w * contact;
    let c = mix(g.ramp1.rgb, g.ramp0.rgb, k) * g.ramp0.w * in.color.rgb * in.ao_shadow.rgb;
    let a = in.color.a * texel.w * soft;
    if a <= 1.0 / 255.0 {
        discard;
    }
    return vec4(sqrt(max(c, vec3(0.0))), a);
}

@fragment
fn fs_additive(in: VsOut) -> @location(0) vec4<f32> {
    let c = textureSampleBias(tex, tex_sampler, in.uvt.xy, -1.0) * in.color;
    let a = max(c.r, max(c.g, c.b));
    if a <= 1.0 / 255.0 {
        discard;
    }
    return vec4(c.rgb * c.a, a);
}

@fragment
fn fs_zfeather_additive(in: VsOut) -> @location(0) vec4<f32> {
    let c = textureSampleBias(tex, tex_sampler, in.uvt.xy, -1.0) * in.color;
    let soft = saturate(scene_dz(in.screen.xy, in.screen.z) * d.feather.x - d.feather.y);
    let a = max(c.r, max(c.g, c.b));
    if a <= 1.0 / 255.0 {
        discard;
    }
    return vec4(c.rgb * c.a * soft, a);
}

@fragment
fn fs_zfeather_alpha(in: VsOut) -> @location(0) vec4<f32> {
    let c = textureSampleBias(tex, tex_sampler, in.uvt.xy, -1.0) * in.color;
    let soft = saturate(scene_dz(in.screen.xy, in.screen.z) * d.feather.x - d.feather.y);
    let a = c.a * soft;
    if a <= 1.0 / 255.0 {
        discard;
    }
    return vec4(sqrt(max(c.rgb, vec3(0.0))), a);
}

@fragment
fn fs_lynx_alpha(in: VsOut) -> @location(0) vec4<f32> {
    let texel = textureSampleBias(tex, tex_sampler, in.uvt.xy, -1.0);
    let a = texel.a * in.color.a;
    if a <= 1.0 / 255.0 {
        discard;
    }
    return vec4(sqrt(max(texel.rgb * in.color.rgb, vec3(0.0))), a);
}

@fragment
fn fs_snow_ramp(in: VsOut) -> @location(0) vec4<f32> {
    let texel = textureSampleBias(tex, tex_sampler, in.uvt.xy, -1.0);
    let soft = saturate(d.feather.x * scene_dz(in.screen.xy, in.screen.z) - d.feather.y);
    var c = mix(g.ramp1.rgb, g.ramp0.rgb, texel.y) * g.ramp0.w * in.color.rgb * in.ao_shadow.rgb;
    c = apply_fog(c, in.fog);
    let a = texel.a * in.color.a * soft;
    if a <= 1.0 / 255.0 {
        discard;
    }
    return vec4(sqrt(max(c, vec3(0.0))), a);
}

@fragment
fn fs_snow_alpha(in: VsOut) -> @location(0) vec4<f32> {
    let texel = textureSampleBias(tex, tex_sampler, in.uvt.xy, -1.0);
    let soft = saturate(d.feather.x * scene_dz(in.screen.xy, in.screen.z) - d.feather.y);
    let c = apply_fog(texel.rgb * in.color.rgb, in.fog);
    let a = texel.a * in.color.a * soft;
    if a <= 1.0 / 255.0 {
        discard;
    }
    return vec4(sqrt(max(c, vec3(0.0))), a);
}

@fragment
fn fs_instance_ramp(in: VsOut) -> @location(0) vec4<f32> {
    let texel = textureSampleBias(tex, tex_sampler, in.uvt.xy, -1.0);
    let ndc = in.clip_pos.xyz / in.clip_pos.w;
    let s = vec2(ndc.x * 0.5 + 0.5, -ndc.y * 0.5 + 0.5);
    let soft = saturate(d.feather.x * scene_dz(s, ndc.z) - d.feather.y);
    let zp = view_z(ndc.z);
    let near = saturate((-d.fade.y - zp) / (d.fade.x - d.fade.y));
    var c = mix(g.ramp1.rgb, g.ramp0.rgb, texel.y) * g.ramp0.w * in.color.rgb * in.ao_shadow.rgb;
    c = apply_fog(c, in.fog);
    let a = in.color.a * texel.a * soft * near;
    if a <= 1.0 / 255.0 {
        discard;
    }
    return vec4(sqrt(max(c, vec3(0.0))), a);
}

@fragment
fn fs_snowfall(in: VsOut) -> @location(0) vec4<f32> {
    let texel = textureSampleBias(tex, tex_sampler, in.uvt.xy, -1.0);
    let c = texel.rgb * g.ramp0.rgb * g.ramp0.w * in.color.rgb;
    let a = texel.a * in.color.a;
    if a <= 1.0 / 255.0 {
        discard;
    }
    return vec4(sqrt(max(c, vec3(0.0))), a);
}

@fragment
fn fs_ribbon(in: VsOut) -> @location(0) vec4<f32> {
    let c = textureSampleBias(tex, tex_sampler, in.uvt.xy, -1.0) * in.color;
    let soft = saturate(scene_dz(in.screen.xy, in.screen.z) * d.feather.x - d.feather.y);
    let a = max(c.r, max(c.g, c.b));
    if a <= 1.0 / 255.0 {
        discard;
    }
    return vec4(c.rgb * c.a * soft, a);
}

@fragment
fn fs_ribbon_alpha(in: VsOut) -> @location(0) vec4<f32> {
    let c = textureSampleBias(tex, tex_sampler, in.uvt.xy, -1.0) * in.color;
    let soft = saturate(scene_dz(in.screen.xy, in.screen.z) * d.feather.x - d.feather.y);
    let a = c.a * soft;
    if a <= 1.0 / 255.0 {
        discard;
    }
    return vec4(sqrt(max(c.rgb, vec3(0.0))), a);
}

@fragment
fn fs_spray_ramp(in: VsOut) -> @location(0) vec4<f32> {
    let texel = textureSampleBias(tex, tex_sampler, in.uvt.xy, -1.0);
    let soft = saturate(d.feather.x * scene_dz(in.screen.xy, in.screen.z) - d.feather.y);
    let k = texel.y * in.ao_shadow.w;
    var c = (mix(g.ramp1.rgb, g.ramp0.rgb, k) * g.ramp0.w + in.light.rgb) * in.color.rgb * in.ao_shadow.rgb;
    c = apply_fog(c, in.fog);
    let a = in.color.a * texel.w * soft;
    if a <= 1.0 / 255.0 {
        discard;
    }
    return vec4(sqrt(max(c, vec3(0.0))), a);
}

fn glare_acos(a: f32) -> f32 {
    let x = abs(a);
    let r = sqrt(max(1.0 - x, 0.0)) * (1.5707288 + x * (-0.2121144 + x * (0.074261 + x * -0.0187293)));
    return select(r, PI - r, a < 0.0);
}

@vertex
fn vs_glare(@builtin(vertex_index) vi: u32) -> VsOut {
    var out: VsOut;
    let base = u32(d.fade.w) + (vi / 6u) * 3u;
    let pos = records[base];
    let tc0 = records[base + 1u];
    let corner = CORNERS[vi % 6u];
    let spp = d.stretch.w;
    let nsp = 1.0 - spp;
    let wp = (d.world * vec4(pos.xyz, 1.0)).xyz;
    let sh = normalize((d.world * vec4(tc0.x + 1e-5, tc0.y, tc0.z, 0.0)).xyz);
    var seed = tc0.w;
    if d.glare[3].x > 0.5 {
        seed = nsp * tc0.z;
    }
    let h = spread_hash(seed) - 0.5;
    let t = fract(d.time.x * d.time.z);
    var col = vec4(1.0);
    if d.flags.x > 0.5 {
        col = ramp4(t, d.ramp[0], d.ramp[1], d.ramp[2], d.ramp[3], d.ramp_times);
    }
    if d.flags.y > 0.5 {
        col = col * d.color_mult + h * d.color_spread;
    }
    let angle0 = nsp * tc0.x;
    let flare = nsp * tc0.y;
    let view = g.view_from_world * vec4(wp, 1.0);
    let to_cam = g.camera_pos.xyz - wp;
    let dist = length(to_cam);
    let tc = to_cam / dist;
    let tcv = (g.view_from_world * vec4(tc, 0.0)).x;
    let r = glare_acos(tcv) * d.glare[1].y + d.glare[2].x + angle0;
    let rot = TAU * fract(r / TAU + 0.5) - PI;
    let back = vec3(g.view_from_world[0].z, g.view_from_world[1].z, g.view_from_world[2].z);
    let dir = normalize(back + d.glare[0].y * h.xyz);
    let sp1 = d.glare[1].x + 1.0;
    let cos_h = dot(normalize(dir.xz), normalize(tc.xz));
    let m = max(pow(abs(dot(dir, tc)), sp1), pow(abs(cos_h), sp1));
    let sc = d.glare[0].z;
    let se = d.glare[0].w;
    let sens = saturate((sc - (1.0 - m)) / max(sc, 1e-5)) + saturate((se - m) / max(se, 1e-5));
    let sd = max(-spp * dot(sh, tc), 0.0);
    let dirf = saturate(1.0 - pow(1.0 - pow(sd, d.glare[1].w + 1.0), d.glare[1].z + 1.0)) * spp + nsp;
    let scale = (sens * d.glare[0].x + 1.0 + d.glare[2].y * h.y + flare * d.glare[2].w) * dirf;
    let k = 0.01 * flare * d.glare[2].z * dist * (1.0 + d.glare[3].y * abs(view.x) * 0.01);
    var px = d.pivot.x + corner.x - 0.5;
    var py = d.pivot.y + 0.5 - corner.y;
    if d.flags.w > 0.5 {
        px = px * d.size_mult.x;
        py = py * d.size_mult.y;
    }
    let size = pos.w * scale;
    let vx = size * (cos(rot) * px - sin(rot) * py) + view.x * (1.0 - k);
    let vy = size * (-sin(rot) * px - cos(rot) * py) + view.y * (1.0 - k);
    var clip = g.clip_from_view * vec4(vx, vy, view.z, 1.0);
    let gz = g.game_proj.x * view.z + g.game_proj.y;
    clip.z = clip.z + zbias(gz);
    out.clip = clip;
    out.clip_pos = clip;
    out.uvt = vec3(corner, t);
    out.color = col;
    let centre = g.clip_from_view * view;
    out.screen = vec4(centre.x / centre.w * 0.5 + 0.5, -centre.y / centre.w * 0.5 + 0.5, gz, size);
    out.ao_shadow = vec4(1.0);
    out.fog = vec4(0.0);
    out.light = vec4(0.0);
    return out;
}

@fragment
fn fs_glare(in: VsOut) -> @location(0) vec4<f32> {
    let c = textureSampleBias(tex, tex_sampler, in.uvt.xy, -1.0) * in.color;
    let size = bin_size();
    let px = clamp(vec2<i32>(in.screen.xy * size), vec2(0), vec2<i32>(size) - 1);
    let scene = textureLoad(particle_depth, px, 0).x;
    let vis = select(0.0, 1.0, -view_z(scene) >= in.screen.z);
    let a = max(c.r, max(c.g, c.b));
    if a <= 1.0 / 255.0 {
        discard;
    }
    return vec4(c.rgb * c.a * vis, a);
}
