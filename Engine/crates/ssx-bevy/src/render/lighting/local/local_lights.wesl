import bevy_pbr::render::mesh_view_bindings as view_bindings;

@if(SSX_LOCAL_LIGHTS)
@group(constants::MATERIAL_BIND_GROUP) @binding(140) var<storage, read> local_light_data: array<vec4<f32>>;
@if(SSX_LOCAL_LIGHTS)
@group(constants::MATERIAL_BIND_GROUP) @binding(141) var local_light_textures: texture_2d_array<f32>;
@if(SSX_LOCAL_LIGHTS)
@group(constants::MATERIAL_BIND_GROUP) @binding(142) var local_light_sampler: sampler;

var<private> ll_position: vec3<f32>;
var<private> ll_dpx: vec3<f32>;
var<private> ll_dpy: vec3<f32>;

fn record_fragment(p: vec3<f32>, normal: vec3<f32>, frag: vec2<f32>) {
    ll_position = p;
    ll_dpx = dpdxCoarse(p);
    ll_dpy = dpdyCoarse(p);
}

const RECORD: u32 = 10u;
const PI: f32 = 3.14159265;

fn game_pow(x: f32, y: f32) -> f32 {
    return select(exp2(y * log2(x)), 0.0, x <= 0.0);
}

// TODO(guess): Xenia's model stands in for the Xenos EDRAM blender's 8:8:8:8 operand precision; settle: a hardware capture of a local light's L-buffer product.
fn blend8(dst: vec4<f32>, src: vec4<f32>) -> vec4<f32> {
    return floor(dst * saturate(src) * 255.0 + 0.5) / 255.0;
}

@if(SSX_LOCAL_LIGHTS)
fn word(i: u32) -> vec4<f32> {
    return local_light_data[i];
}

@if(SSX_LOCAL_LIGHTS)
fn words(i: u32) -> vec4<u32> {
    return bitcast<vec4<u32>>(local_light_data[i]);
}

@if(SSX_LOCAL_LIGHTS)
fn to_light(base: u32, p: vec3<f32>) -> vec3<f32> {
    let r0 = word(base);
    let r1 = word(base + 1u);
    let r2 = word(base + 2u);
    return vec3(dot(r0.xyz, p) + r0.w, dot(r1.xyz, p) + r1.w, dot(r2.xyz, p) + r2.w);
}

@if(SSX_LOCAL_LIGHTS)
fn culled(centre: vec3<f32>, radius: f32) -> bool {
    let p = view_bindings::view.clip_from_view;
    let tan_half = 1.0 / max(p[0][0], p[1][1]);
    let d = length(centre - view_bindings::view.world_position);
    return radius <= 0.002 * d * tan_half;
}

@if(SSX_LOCAL_LIGHTS)
fn flicker_random(seed: u32, tick: u32) -> f32 {
    var h = seed * 747796405u + tick * 2891336453u + 0x9E3779B9u;
    h = ((h >> ((h >> 28u) + 4u)) ^ h) * 277803737u;
    h = (h >> 22u) ^ h;
    return f32(h >> 8u) * (1.0 / 16777216.0);
}

@if(SSX_LOCAL_LIGHTS)
fn intensity_factor(mode: u32, seed: u32, strength: f32, frequency: f32, gap: f32) -> f32 {
    if mode == 3u {
        return strength;
    }
    let t = bitcast<f32>(words(2u).w);
    if mode == 1u {
        return 1.0 - strength * flicker_random(seed, words(3u).z);
    }
    if mode == 2u {
        let on = 1.0 / frequency;
        let period = on + gap;
        let phase = t - trunc(t / period) * period;
        return select(1.0 - strength, cos((phase / on - 0.5) * 2.0 * PI) * strength + (1.0 - strength), phase <= on);
    }
    return 1.0;
}

@if(SSX_LOCAL_LIGHTS)
fn clip_plane(n: vec3<f32>, d: f32, e: vec3<f32>, dir: vec3<f32>, range: vec2<f32>) -> vec2<f32> {
    let num = d - dot(n, e);
    let den = dot(n, dir);
    if abs(den) < 1e-12 {
        return select(range, vec2(1.0, 0.0), num < 0.0);
    }
    let t = num / den;
    return select(vec2(range.x, min(range.y, t)), vec2(max(range.x, t), range.y), den < 0.0);
}

@if(SSX_LOCAL_LIGHTS)
fn sphere_face(planes: u32, i: u32, j: u32) -> vec4<f32> {
    return word(planes + i * 10u + j);
}

@if(SSX_LOCAL_LIGHTS)
fn inside_sphere(planes: u32, q: vec3<f32>) -> bool {
    let r = length(q);
    if r < 0.9404 {
        return true;
    }
    let theta = acos(clamp(q.z / r, -1.0, 1.0));
    var phi = atan2(q.y, q.x);
    phi = select(phi, phi + 2.0 * PI, phi < 0.0);
    let i0 = i32(clamp(floor(theta / (PI / 10.0)), 0.0, 9.0));
    let j0 = i32(floor(phi / (PI / 5.0))) % 10;
    for (var di = -1; di <= 1; di++) {
        let i = i0 + di;
        if i < 0 || i > 9 {
            continue;
        }
        for (var dj = -1; dj <= 1; dj++) {
            let f = sphere_face(planes, u32(i), u32((j0 + dj + 10) % 10));
            if dot(f.xyz, q) > f.w {
                return false;
            }
        }
    }
    return true;
}

@if(SSX_LOCAL_LIGHTS)
fn sphere_behind(planes: u32, e: vec3<f32>, q: vec3<f32>) -> bool {
    var range = vec2(-3.0e38, 3.0e38);
    for (var k = 0u; k < 100u; k++) {
        let f = word(planes + k);
        range = clip_plane(f.xyz, f.w, e, q - e, range);
    }
    return range.x <= range.y && range.y > 1.0;
}

@if(SSX_LOCAL_LIGHTS)
fn cone_side(k: u32) -> vec3<f32> {
    let a = f32(k) * (PI / 5.0);
    let b = f32(k + 1u) * (PI / 5.0);
    return cross(vec3(cos(a), sin(a), -1.0), vec3(cos(b), sin(b), -1.0));
}

@if(SSX_LOCAL_LIGHTS)
fn inside_cone(q: vec3<f32>) -> bool {
    if q.z < -1.0 {
        return false;
    }
    for (var k = 0u; k < 10u; k++) {
        if dot(cone_side(k), q) > 0.0 {
            return false;
        }
    }
    return true;
}

@if(SSX_LOCAL_LIGHTS)
fn cone_behind(e: vec3<f32>, q: vec3<f32>) -> bool {
    var range = clip_plane(vec3(0.0, 0.0, -1.0), 1.0, e, q - e, vec2(-3.0e38, 3.0e38));
    for (var k = 0u; k < 10u; k++) {
        range = clip_plane(cone_side(k), 0.0, e, q - e, range);
    }
    return range.x <= range.y && range.y > 1.0;
}

@if(SSX_LOCAL_LIGHTS)
fn projection_uv(pl: vec3<f32>, a2: vec3<f32>) -> vec2<f32> {
    return vec2(saturate((pl.x + a2.x) / (2.0 * a2.x)), 1.0 - saturate((pl.y + a2.y) / (2.0 * a2.y)));
}

@if(SSX_LOCAL_LIGHTS)
fn outside(bound: u32) -> bool {
    let b = word(bound);
    return distance(ll_position, b.xyz) > b.w;
}

@if(SSX_LOCAL_LIGHTS)
fn light_source(base: u32, planes: u32, n: vec3<f32>, v: vec3<f32>, ecc: f32, lli: f32) -> vec4<f32> {
    let r3 = word(base + 3u);
    let flags = bitcast<u32>(r3.w);
    let r6 = word(base + 6u);
    let r8 = word(base + 8u);
    if culled(r8.xyz, r6.w) {
        return vec4(1.0);
    }
    let p = ll_position;
    let pl = to_light(base, p);
    let r4 = word(base + 4u);
    let r5 = word(base + 5u);
    let r9 = word(base + 9u);
    let kind = flags & 3u;
    let e_dir = -v;
    var att = 0.0;
    var l_dir = vec3(0.0);
    var tex = vec4(1.0);
    if kind <= 1u {
        let d = length(pl);
        att = 1.0 - game_pow(saturate((d - r5.x) / (r5.y - r5.x)), r5.z);
        if kind == 1u {
            let c = pl.z / d;
            let u = saturate((-c - r6.y) / (r6.x - r6.y));
            att *= u * u * (3.0 - 2.0 * u) * game_pow(abs(c), r6.z);
        }
        if att <= 0.0 {
            return vec4(1.0);
        }
        let eye = to_light(base, view_bindings::view.world_position);
        if kind == 0u {
            let q = pl / r9.z;
            if !inside_sphere(planes, q) && !sphere_behind(planes, eye / r9.z, q) {
                return vec4(1.0);
            }
        } else {
            let s = vec3(r9.z, r9.z, r9.w);
            let q = pl / s;
            if !inside_cone(q) && !cone_behind(eye / s, q) {
                return vec4(1.0);
            }
        }
        l_dir = normalize(r3.xyz - p);
    } else {
        if pl.z >= 0.0 {
            return vec4(1.0);
        }
        let sx = saturate((abs(pl.x) - (r6.x - r6.z)) / r6.z);
        let sy = saturate((abs(pl.y) - (r6.y - r6.z)) / r6.z);
        let tz = saturate((-pl.z - r5.x) / (r5.y - r5.x));
        att = (1.0 - sx * sx * (3.0 - 2.0 * sx)) * (1.0 - sy * sy * (3.0 - 2.0 * sy)) * (1.0 - game_pow(tz, r5.z));
        if att <= 0.0 {
            return vec4(1.0);
        }
        l_dir = word(base + 7u).xyz;
        if kind == 3u {
            // TODO(guess): the neighbours' positions come from the fragment's own triangle, the game's from the depth buffer; settle: the Xenos fetch's gradient rule and a capture of a marking across a terrain edge.
            let uv = projection_uv(pl, r6.xyz);
            let ux = projection_uv(to_light(base, p + ll_dpx), r6.xyz);
            let uy = projection_uv(to_light(base, p + ll_dpy), r6.xyz);
            let layer = (flags >> 8u) & 255u;
            if (flags & 8u) != 0u {
                let half = vec2(1.0, 0.5);
                let dx = (ux - uv) * half * 512.0;
                let dy = (uy - uv) * half * 512.0;
                let lod = clamp(0.5 * log2(max(max(dot(dx, dx), dot(dy, dy)), 1e-20)), 0.0, 2.0);
                tex = textureSampleLevel(local_light_textures, local_light_sampler, uv * half, layer, lod);
            } else {
                tex = textureSampleGrad(local_light_textures, local_light_sampler, uv, layer, ux - uv, uy - uv);
            }
        }
    }
    let f = intensity_factor((flags >> 4u) & 3u, bitcast<u32>(word(base + 7u).w), r8.w, r9.x, r9.y);
    let diffuse = lli * r4.w * f * r4.rgb * saturate(dot(n, l_dir)) * att * tex.rgb;
    var spec = 0.0;
    if (flags & 4u) != 0u {
        spec = lli * r5.w * f * pow(saturate(dot(e_dir, reflect(l_dir, n))), max(ecc, 1e-4)) * att * tex.r;
    }
    return exp2(-vec4(diffuse, spec));
}

@if(SSX_LOCAL_LIGHTS)
fn bin_at(base: u32) -> u32 {
    let flags = bitcast<u32>(word(base + 3u).w);
    return (flags & 3u) * 2u + ((flags >> 2u) & 1u);
}

fn apply(l8_in: vec4<f32>, n: vec3<f32>, v: vec3<f32>, ecc: f32, lli: f32) -> vec4<f32> {
    var l8 = l8_in;
@if(SSX_LOCAL_LIGHTS) {
    let h0 = words(0u);
    let h1 = words(1u);
    let h2 = words(2u);
    let h3 = words(3u);
    let dynamic_bounds = h3.w + 11u;
    let dynamic_count = select(h3.y, 0u, outside(dynamic_bounds));
    var list = vec2(0u);
    if h0.w != 0u {
        let cell = vec2<i32>(floor((ll_position.xz - vec2(bitcast<f32>(h0.x), bitcast<f32>(h0.y))) / bitcast<f32>(h0.z)));
        let inside = all(cell >= vec2(0)) && all(cell < vec2<i32>(h1.xy));
        list = words(h1.z + select(h1.x * h1.y, u32(cell.y) * h1.x + u32(cell.x), inside)).xy;
    }
    var k = 0u;
    var next_dynamic = 0u;
    var listed = false;
    var listed_base = 0u;
    var listed_bin = 0u;
    // PORT(choice): each list entry's bounds are tested before its light's index, record and bin are read; the dynamic
    // records of the lower bins are multiplied in before the next listed light inside its bounds, all of them in one
    // loop with one evaluation of a light (`light_source`) in it; the 8-bit products and their order are the same. The
    // console drew each visible light's volume in its bin's order (DL_LightBufferPass, StaticLightsRenderObj before
    // DynamicLightsRenderObj per bin).
    loop {
        if !listed {
            while k < list.y && outside(h2.z + list.x + k) {
                k++;
            }
            if k < list.y {
                let at = list.x + k;
                listed_base = h2.x + words(h1.w + at / 4u)[at % 4u] * RECORD;
                listed_bin = bin_at(listed_base);
                listed = true;
                k++;
            }
        }
        var base = listed_base;
        if next_dynamic < dynamic_count && (!listed || bin_at(h3.x + next_dynamic * RECORD) < listed_bin) {
            base = h3.x + next_dynamic * RECORD;
            let bound = dynamic_bounds + 1u + next_dynamic;
            next_dynamic++;
            if outside(bound) {
                continue;
            }
        } else if listed {
            listed = false;
        } else {
            break;
        }
        let src = light_source(base, h2.y, n, v, ecc, lli);
        if any(src < vec4(1.0)) {
            l8 = blend8(l8, src);
        }
    }
}
    return l8;
}
