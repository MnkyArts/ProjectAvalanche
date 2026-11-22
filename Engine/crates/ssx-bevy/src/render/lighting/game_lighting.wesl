import bevy_pbr::render::{
    mesh_view_bindings as view_bindings,
    mesh_view_types,
    pbr_bindings,
    pbr_types,
    shadows,
};
import package::render::lighting::local::local_lights;

struct GameLight {
    ramp_lit: vec4<f32>,
    ramp_shade: vec4<f32>,
    ramp_bounce: vec4<f32>,
    light_dir: vec4<f32>,
    bounce_dir: vec4<f32>,
    rim_shadow: vec4<f32>,
    specular: vec4<f32>,
    fog_color: vec4<f32>,
    fog_data: vec4<f32>,
    sparkle: vec4<f32>,
    misc: vec4<f32>,
}

struct RimFill {
    rim_color: vec4<f32>,
    rim: vec4<f32>,
    rim_dir: vec4<f32>,
    fill_color: vec4<f32>,
    fill_dir: vec4<f32>,
}

struct LevelLight {
    light: GameLight,
    rim_fill: RimFill,
}

@if(SSX_LOCAL_LIGHTS)
fn frame_light() -> GameLight {
    let at = local_lights::words(3u).w;
    return GameLight(
        local_lights::word(at),
        local_lights::word(at + 1u),
        local_lights::word(at + 2u),
        local_lights::word(at + 3u),
        local_lights::word(at + 4u),
        local_lights::word(at + 5u),
        local_lights::word(at + 6u),
        local_lights::word(at + 7u),
        local_lights::word(at + 8u),
        local_lights::word(at + 9u),
        local_lights::word(at + 10u),
    );
}

const LUMA: vec3<f32> = vec3(0.299, 0.587, 0.114);

fn view_z(world_position: vec4<f32>) -> f32 {
    let m = view_bindings::view.view_from_world;
    return dot(vec4(m[0].z, m[1].z, m[2].z, m[3].z), world_position);
}

fn to_eye(world_position: vec3<f32>) -> vec3<f32> {
    return normalize(view_bindings::view.world_position - world_position);
}

fn shadow_size() -> f32 {
    return f32(textureDimensions(view_bindings::directional_shadow_textures).x);
}
const PCF_HALF_WIDTH: f32 = 2.0;

fn shadow_bias(i: u32) -> f32 {
    switch i {
        case 0u: { return bitcast<f32>(0x39A4A05Eu); }
        case 1u: { return bitcast<f32>(0x3A334E77u); }
        default: { return bitcast<f32>(0x3B4CAB3Fu); }
    }
}

const NO_LIGHT: u32 = 0xffffffffu;
const CONTACT_MARK: f32 = 1.0;

fn shadow_light(cascades: u32, mark: f32) -> u32 {
    for (var i = 0u; i < view_bindings::lights.n_directional_lights; i++) {
        let light = &view_bindings::lights.directional_lights[i];
        let mapped = ((*light).flags & mesh_view_types::DIRECTIONAL_LIGHT_FLAGS_SHADOWS_ENABLED_BIT) != 0u;
        if mapped && (*light).num_cascades == cascades && (*light).shadow_depth_bias == mark {
            return i;
        }
    }
    return NO_LIGHT;
}

fn shadow_texel(layer: i32, t: vec2<i32>, thr: f32, size: i32) -> f32 {
@if(NO_ARRAY_TEXTURES_SUPPORT) {
    return 1.0;
} @else {
    if any(t < vec2(0)) || any(t >= vec2(size)) {
        return 1.0;
    }
    let d = textureLoad(view_bindings::directional_shadow_textures, t, layer, 0);
    return select(0.0, 1.0, thr > d);
}
}

fn shadow_quad(layer: i32, t: vec2<i32>, thr: f32, size: i32) -> vec4<f32> {
@if(NO_ARRAY_TEXTURES_SUPPORT) {
    return vec4(1.0);
} @else {
    // PORT(choice): one gather-compare of the four texels instead of four point loads (the shadow PCF's 20 loads become
    // 5 fetches). The same texels and the same results: the gather reads the 2×2 whose corner is at (t + 1) / map
    // size (a power of two: exact; the sampler's footprint floor(uv · size − 0.5) = t), Bevy 0.20's directional
    // comparison sampler compares `reference >= texel` (light.rs `init_shadow_samplers`: GreaterEqual, clamp to edge),
    // so the reference is the float below thr (thr > texel exactly when that float >= texel; thr is positive and
    // finite: 1 − saturate(·) + a bias > 0), and the texels outside [0, size)² are set to 1 as the point taps' border.
    // The console fetched the 20 taps of ss_shadow (shadowbuffer.fxo PS1) with point sampling.
    let uv = vec2<f32>(t + vec2(1)) / shadow_size();
    let below = bitcast<f32>(bitcast<u32>(thr) - 1u);
    let lit = textureGatherCompare(
        view_bindings::directional_shadow_textures,
        view_bindings::directional_shadow_textures_comparison_sampler,
        uv,
        layer,
        below,
    );
    let in_t = (t >= vec2(0)) & (t < vec2(size));
    let in_t1 = (t >= vec2(-1)) & (t < vec2(size - 1));
    return select(vec4(1.0), lit, vec4(in_t.x && in_t1.y, in_t1.x && in_t1.y, in_t1.x && in_t.y, in_t.x && in_t.y));
}
}

fn shadow_group(layer: i32, i0: vec2<i32>, o: vec2<i32>, f: vec2<f32>, thr: f32, size: i32) -> f32 {
    let q = shadow_quad(layer, i0 + o, thr, size);
    let a = mix(q.w, q.z, f.x);
    let b = mix(q.x, q.y, f.x);
    return mix(a, b, f.y);
}

fn shadow_pcf(layer: i32, pt: vec2<f32>, thr: f32, size: i32) -> f32 {
    let i0 = vec2<i32>(floor(pt));
    let f = pt - floor(pt);
    return 0.2 * (shadow_group(layer, i0, vec2(0, 0), f, thr, size) + shadow_group(layer, i0, vec2(1, 1), f, thr, size)
        + shadow_group(layer, i0, vec2(-1, 1), f, thr, size) + shadow_group(layer, i0, vec2(-1, -1), f, thr, size)
        + shadow_group(layer, i0, vec2(1, -1), f, thr, size));
}

fn sun_shadow(world_position: vec4<f32>, surface_normal: vec3<f32>, z: f32, frag_coord: vec2<f32>, receiver: bool) -> f32 {
    local_lights::record_fragment(world_position.xyz, surface_normal, frag_coord);
    if !receiver {
        return 1.0;
    }
    let sun = shadow_light(3u, 0.0);
    if sun == NO_LIGHT {
        return 1.0;
    }
    let light = &view_bindings::lights.directional_lights[sun];
    let p = vec4(world_position.xyz, 1.0);
    var cascade = 3u;
    var c = vec3(0.0);
    var c2 = vec3(0.0);
    let edge = 0.5 - PCF_HALF_WIDTH / shadow_size();
    for (var i = 0u; i < 3u; i++) {
        let clip = (*light).cascades[i].clip_from_world * p;
        let ci = vec3(0.5 * clip.x, -0.5 * clip.y, 0.5 - clip.z);
        if cascade == 3u && all(abs(ci.xy) < vec2(edge)) && abs(ci.z) < 0.5 {
            cascade = i;
            c = ci;
        }
        c2 = ci;
    }
    var bias = 100.0;
    var layer_cascade = 2u;
    if cascade < 3u {
        bias = shadow_bias(cascade);
        layer_cascade = cascade;
    } else {
        c = c2;
    }
    let thr = 1.0 - (saturate(c.z + 0.5) - bias);
    let layer = i32((*light).depth_texture_base_index + layer_cascade);
    let pcf = shadow_pcf(layer, (c.xy + 0.5) * shadow_size() - 0.5, thr, i32(shadow_size()));
    let g = saturate(20.0 * abs(c2) - 9.0);
    let outer = select(1.0, (1.0 - g.y) * (1.0 - g.z), cascade >= 2u);
    let s = saturate(pcf + 1.0 - (1.0 - g.x) * outer);
    return min(s, character_shadow(world_position.xyz));
}

const CHARACTER_SIZE: f32 = 128.0;
const CHARACTER_BIAS: f32 = 0.0001;

fn map_box_stencil(m: mat4x4<f32>, p: vec3<f32>) -> bool {
    let b = (m * vec4(p, 1.0)).xyz;
    let e = (m * vec4(view_bindings::view.world_position, 1.0)).xyz;
    let depth = max(-view_z(vec4(p, 1.0)), 1e-6);
    let a = view_bindings::view.clip_from_view[2][2];
    let bz = view_bindings::view.clip_from_view[3][2];
    var t0 = bz / (1.0 + a) / depth;
    var t1 = select(3.0e38, bz / a / depth, a > 1e-9);
    let scale = CHARACTER_SIZE / shadow_size();
    let lo = vec3(-1.0, 1.0 - 2.0 * scale, 0.0);
    let hi = vec3(-1.0 + 2.0 * scale, 1.0, 1.0);
    let d = b - e;
    for (var k = 0; k < 3; k++) {
        if abs(d[k]) < 1e-12 {
            if e[k] < lo[k] || e[k] > hi[k] {
                return false;
            }
        } else {
            let ta = (lo[k] - e[k]) / d[k];
            let tb = (hi[k] - e[k]) / d[k];
            t0 = max(t0, min(ta, tb));
            t1 = min(t1, max(ta, tb));
        }
    }
    return t0 <= t1;
}

fn character_shadow(p: vec3<f32>) -> f32 {
    let index = shadow_light(1u, 0.0);
    if index == NO_LIGHT {
        return 1.0;
    }
    let light = &view_bindings::lights.directional_lights[index];
    let m = (*light).cascades[0].clip_from_world;
    if !map_box_stencil(m, p) {
        return 1.0;
    }
    let b = (m * vec4(p, 1.0)).xyz;
    let thr = 1.0 - (saturate(1.0 - b.z) - CHARACTER_BIAS);
    let pt = vec2(b.x + 1.0, 1.0 - b.y) * 0.5 * shadow_size() - 0.5;
    return shadow_pcf(i32((*light).depth_texture_base_index), pt, thr, i32(CHARACTER_SIZE));
}

const SHADOW_QUERY_FADE_START: f32 = 400.0 - 0.5 - 50.0;

fn shadow_query(p: vec3<f32>) -> f32 {
    let sun = shadow_light(3u, 0.0);
    if sun == NO_LIGHT {
        return 1.0;
    }
    let light = &view_bindings::lights.directional_lights[sun];
    let wp = vec4(p, 1.0);
    var cascade = 3u;
    var c = vec3(0.0);
    let edge = 0.5 - PCF_HALF_WIDTH / shadow_size();
    for (var i = 0u; i < 3u; i++) {
        let clip = (*light).cascades[i].clip_from_world * wp;
        let ci = vec3(0.5 * clip.x, -0.5 * clip.y, 0.5 - clip.z);
        if cascade == 3u && all(abs(ci.xy) < vec2(edge)) && abs(ci.z) < 0.5 {
            cascade = i;
            c = ci;
        }
        if i == 2u && cascade == 3u {
            c = ci;
        }
    }
    var bias = 100.0;
    var layer_cascade = 2u;
    if cascade < 3u {
        bias = shadow_bias(cascade);
        layer_cascade = cascade;
    }
    let thr = 1.0 - (saturate(c.z + 0.5) - bias);
    let layer = i32((*light).depth_texture_base_index + layer_cascade);
    let t = vec2<i32>(floor((c.xy + 0.5) * shadow_size() + 1.0));
    let lit = shadow_texel(layer, t, thr, i32(shadow_size()));
    let fade = saturate(0.02 * (-view_z(wp) - SHADOW_QUERY_FADE_START));
    return saturate(lit + fade);
}

fn contact_depth_texel(layer: i32, t: vec2<i32>) -> f32 {
@if(NO_ARRAY_TEXTURES_SUPPORT) {
    return 0.0;
} @else {
    if any(t < vec2(0)) || any(t >= vec2(i32(CHARACTER_SIZE))) {
        return 0.0;
    }
    return textureLoad(view_bindings::directional_shadow_textures, t, layer, 0);
}
}

fn contact_shadow(p: vec3<f32>) -> f32 {
    let index = shadow_light(1u, CONTACT_MARK);
    if index == NO_LIGHT {
        return 1.0;
    }
    let light = &view_bindings::lights.directional_lights[index];
    let m = (*light).cascades[0].clip_from_world;
    if !map_box_stencil(m, p) {
        return 1.0;
    }
    let b = (m * vec4(p, 1.0)).xyz;
    let z = 1.0 - b.z;
    let pt = vec2(b.x + 1.0, 1.0 - b.y) * 0.5 * shadow_size() - 0.5;
    let i0 = vec2<i32>(floor(pt));
    let f = pt - floor(pt);
    let layer = i32((*light).depth_texture_base_index);
    let top = mix(contact_depth_texel(layer, i0), contact_depth_texel(layer, i0 + vec2(1, 0)), f.x);
    let bottom = mix(contact_depth_texel(layer, i0 + vec2(0, 1)), contact_depth_texel(layer, i0 + vec2(1, 1)), f.x);
    let d = mix(top, bottom, f.y);
    return saturate(4.0 * (z - CHARACTER_BIAS)) * d + (1.0 - d);
}

fn rim_term(l: GameLight, n: vec3<f32>, v: vec3<f32>) -> f32 {
    return saturate(pow(max(1.0 - abs(dot(n, v)), 1e-7), l.ramp_shade.w));
}

fn ramp_diffuse(l: GameLight, n: vec3<f32>, v: vec3<f32>, shadow_att: f32) -> vec3<f32> {
    var k = saturate(dot(n, l.light_dir.xyz)) * shadow_att;
    k = mix(k, rim_term(l, n, v), mix(l.rim_shadow.y, l.rim_shadow.x, k));
    let spread = l.bounce_dir.w;
    let t = saturate((dot(n, l.bounce_dir.xyz) - spread) / max(1.0 - spread, 1e-4));
    let bounce = l.ramp_bounce.w * t * t * (3.0 - 2.0 * t);
    return l.ramp_lit.w * mix(l.ramp_shade.rgb, l.ramp_lit.rgb, k) + bounce * l.ramp_bounce.rgb;
}

fn key_light(l: GameLight, n: vec3<f32>, v: vec3<f32>, e: f32, s: f32, c: f32, ao: f32) -> vec4<f32> {
    let shadow_att = mix(1.0, s, l.rim_shadow.z);
    let contact_att = mix(1.0, c, l.rim_shadow.w);
    let diffuse = ramp_diffuse(l, n, v, shadow_att)
        * mix(l.ramp_shade.rgb, vec3(1.0), contact_att * ao);
    let phong = pow(saturate(dot(v, reflect(-l.light_dir.xyz, n))), max(e, 1e-4));
    let spec = phong * l.light_dir.w * contact_att * ao * s;
    let lbuffer = local_lights::apply(lbuffer_encode(vec4(diffuse, spec)), n, v, e, l.misc.z);
    return lbuffer_decode(lbuffer);
}

fn lbuffer_encode(i: vec4<f32>) -> vec4<f32> {
    return round(saturate(exp2(-max(i, vec4(0.0)))) * 255.0) / 255.0;
}

fn lbuffer_decode(l: vec4<f32>) -> vec4<f32> {
    return -log2(max(l, vec4(1e-4)));
}

fn lbuffer_roundtrip(i: vec4<f32>) -> vec4<f32> {
    return lbuffer_decode(lbuffer_encode(i));
}

fn forward_ramp(l: GameLight, n: vec3<f32>, v: vec3<f32>) -> vec3<f32> {
    return ramp_diffuse(l, n, v, 1.0);
}

fn snow_glint(l: GameLight, sp_view: vec3<f32>, n: vec3<f32>, v: vec3<f32>, diffuse_light: vec3<f32>) -> f32 {
    let sp = (view_bindings::view.world_from_view * vec4(sp_view, 0.0)).xyz;
    let ns = normalize(n + sp);
    let lr = reflect(l.light_dir.xyz, ns);
    let gate = select(0.0, 1.0, dot(sp_view, sp_view) > 0.01);
    let glint = saturate(gate * l.sparkle.z * pow(saturate(dot(-v, lr)), l.sparkle.y));
    let range = l.misc.x - l.sparkle.w;
    let ramp = saturate((dot(diffuse_light, LUMA) - l.sparkle.w) / select(range, 1e-6, abs(range) < 1e-6));
    return glint * ramp;
}

fn fog_amount(l: GameLight, world_position: vec3<f32>) -> f32 {
    let d = length(view_bindings::view.world_position - world_position);
    let span = l.fog_data.y - l.fog_data.x;
    let x = saturate((d - l.fog_data.x) / select(span, 1e-6, abs(span) < 1e-6));
    return l.fog_data.w * select(pow(x, l.fog_data.z), 0.0, x <= 0.0);
}

fn apply_fog(l: GameLight, c: vec3<f32>, fog: f32) -> vec3<f32> {
    let a = saturate(l.fog_color.a * fog);
    let grey = mix(c, vec3(dot(c, LUMA)), a);
    return mix(grey, l.fog_color.rgb * fog, a);
}

fn diffuse_texel(uv: vec2<f32>, bias: f32) -> vec4<f32> {
    let m = pbr_bindings::material;
    if (m.flags & pbr_types::STANDARD_MATERIAL_FLAGS_BASE_COLOR_TEXTURE_BIT) == 0u {
        return m.base_color;
    }
    let t = (m.uv_transform * vec3(uv, 1.0)).xy;
    return m.base_color * textureSampleBias(pbr_bindings::base_color_texture, pbr_bindings::base_color_sampler, t, bias);
}

fn game_tbn(world_normal: vec3<f32>, world_tangent: vec4<f32>, nt: vec3<f32>) -> vec3<f32> {
    let n = safe_normalize(world_normal);
    let t = safe_normalize(world_tangent.xyz);
    let b = cross(n, t);
    return safe_normalize(nt.x * t + nt.y * b + nt.z * n);
}

fn signed_tbn(world_normal: vec3<f32>, world_tangent: vec4<f32>, nt: vec3<f32>) -> vec3<f32> {
    let n = safe_normalize(world_normal);
    let t = safe_normalize(world_tangent.xyz);
    let b = cross(n, t) * select(1.0, -1.0, world_tangent.w < 0.0);
    return safe_normalize(nt.x * t + nt.y * b + nt.z * n);
}

fn safe_normalize(v: vec3<f32>) -> vec3<f32> {
    let l = dot(v, v);
    return select(v * inverseSqrt(l), vec3(0.0), l == 0.0);
}
