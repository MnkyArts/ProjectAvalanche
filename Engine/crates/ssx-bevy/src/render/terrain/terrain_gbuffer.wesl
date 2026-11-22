import bevy_pbr::render::mesh_view_bindings as view_bindings;

struct TerrainParams {
    base: array<vec4<f32>, 2>,
    snow_tile: array<vec4<f32>, 2>,
    snow_eccentricity: array<vec4<f32>, 2>,
    snow_specular: array<vec4<f32>, 2>,
    planar_scatter: vec4<f32>,
    snow_blend: vec4<f32>,
    snow_fall: vec4<f32>,
    reflect: vec4<f32>,
}

@group(constants::MATERIAL_BIND_GROUP) @binding(100) var<uniform> terrain: TerrainParams;
@group(constants::MATERIAL_BIND_GROUP) @binding(101) var rock_diffuse_1: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(102) var rock_normal_1: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(111) var rock_diffuse_2: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(112) var rock_normal_2: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(103) var snow_normal: texture_2d_array<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(104) var terrain_sampler: sampler;
@group(constants::MATERIAL_BIND_GROUP) @binding(105) var scatter_normal: texture_2d<f32>;
struct SnowTracks {
    tile: vec4<f32>,
    size: vec4<f32>,
}
@group(constants::MATERIAL_BIND_GROUP) @binding(113) var<uniform> tracks: SnowTracks;
@group(constants::MATERIAL_BIND_GROUP) @binding(114) var track_lookup: texture_2d<u32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(115) var track_tiles: texture_2d_array<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(116) var track_sampler: sampler;

const NO_LAYER: u32 = 7u;
const POWDER: u32 = 5u;
const FADE: vec4<f32> = vec4(0.02, 0.025, 2.5, 12.0);
fn snow_tile(i: u32) -> f32 { return terrain.snow_tile[i / 4u][i % 4u]; }
fn snow_eccentricity(i: u32) -> f32 { return terrain.snow_eccentricity[i / 4u][i % 4u]; }
fn snow_specular(i: u32) -> f32 { return terrain.snow_specular[i / 4u][i % 4u]; }

struct Surface {
    p: vec3<f32>,
    dx: vec3<f32>,
    dy: vec3<f32>,
}

fn plane_uv(v: vec3<f32>, axis: u32) -> vec2<f32> {
    switch axis {
        case 0u: { return vec2(v.z, -v.y); }
        case 1u: { return vec2(v.x, v.z); }
        default: { return vec2(v.x, -v.y); }
    }
}

fn sample_plane(tex: texture_2d<f32>, s: Surface, axis: u32, tile: f32) -> vec4<f32> {
    return textureSampleGrad(
        tex, terrain_sampler, plane_uv(s.p, axis) * tile,
        plane_uv(s.dx, axis) * tile, plane_uv(s.dy, axis) * tile,
    );
}

fn sample_base_normal(k: u32, s: Surface, axis: u32, tile: f32) -> vec4<f32> {
    if k == 0u {
        return sample_plane(rock_normal_1, s, axis, tile);
    }
    return sample_plane(rock_normal_2, s, axis, tile);
}

fn sample_base_diffuse(k: u32, s: Surface, axis: u32, tile: f32) -> vec4<f32> {
    if k == 0u {
        return sample_plane(rock_diffuse_1, s, axis, tile);
    }
    return sample_plane(rock_diffuse_2, s, axis, tile);
}

fn planar_weights(n: vec3<f32>) -> vec3<f32> {
    let w = max(abs(n) * terrain.planar_scatter.x + terrain.planar_scatter.y, vec3(0.0));
    return w / max(w.x + w.y + w.z, 1e-6);
}

fn base_perturbation(k: u32, s: Surface, w: vec3<f32>) -> vec3<f32> {
    let tile = terrain.base[k].x;
    var d = vec3(0.0);
    if w.x > 0.0 {
        let t = sample_base_normal(k, s, 0u, tile).xy * 2.0 - 1.0;
        d += w.x * vec3(0.0, t.y, t.x);
    }
    if w.y > 0.0 {
        let t = sample_base_normal(k, s, 1u, tile).xy * 2.0 - 1.0;
        d += w.y * vec3(t.y, 0.0, t.x);
    }
    if w.z > 0.0 {
        let t = sample_base_normal(k, s, 2u, tile).xy * 2.0 - 1.0;
        d += w.z * vec3(t.x, t.y, 0.0);
    }
    return d;
}

fn base_diffuse(k: u32, s: Surface, w: vec3<f32>) -> vec4<f32> {
    let tile = terrain.base[k].x;
    var sum = vec4(0.0);
    for (var axis = 0u; axis < 3u; axis++) {
        if w[axis] > 0.0 {
            sum += w[axis] * sample_base_diffuse(k, s, axis, tile);
        }
    }
    return sum;
}

fn deformation(n: vec3<f32>, p: vec3<f32>) -> vec3<f32> {
    let tile_uv = p.xz * tracks.tile.x + tracks.tile.yz;
    let size = vec2<i32>(tracks.size.xy);
    let idx = textureLoad(track_lookup, clamp(vec2<i32>(floor(tile_uv)), vec2(0), size - 1), 0).x;
    if idx >= 65535u {
        return vec3(0.0, 0.0, 1.0);
    }
    let d = textureSampleLevel(track_tiles, track_sampler, fract(tile_uv), i32(idx), 0.0);
    let f = saturate(4.0 * n.y - 0.6);
    return vec3(2.0 * (d.xy - 0.5) * f, saturate(1.0 - 100.0 * abs((d.w - 0.75) * f)));
}

fn snow_detail(n: vec3<f32>, s: Surface, w: vec4<f32>, lanes_in: array<u32, 4>) -> vec3<f32> {
    var lanes = lanes_in;
    var t = vec2(0.0);
    for (var i = 0u; i < 4u; i++) {
        let layer = lanes[i];
        if layer != NO_LAYER && w[i] != 0.0 {
            let tile = snow_tile(layer);
            let m = textureSampleGrad(snow_normal, terrain_sampler, s.p.xz * tile, layer, s.dx.xz * tile, s.dy.xz * tile);
            t += w[i] * (m.xy * 2.0 - 1.0);
        }
    }
    let st = terrain.planar_scatter.z;
    let gb = st * exp2(terrain.reflect.z);
    let scatter = textureSampleGrad(scatter_normal, terrain_sampler, s.p.xz * st, s.dx.xz * gb, s.dy.xz * gb);
    let sc = terrain.planar_scatter.w * (scatter.xyz * 2.0 - 1.0);
    let dm = deformation(n, s.p);
    return normalize(vec3(n.x + dm.x + dm.z * t.y + sc.x, n.y + sc.y, n.z + dm.y + dm.z * t.x + sc.z));
}

struct Inputs {
    s: Surface,
    w_snow: vec4<f32>,
    cov: f32,
    b: f32,
    base_bits: u32,
    lanes: array<u32, 4>,
    has_snow: bool,
    group: u32,
    view_z: f32,
    fade_snow: f32,
    fade_base: f32,
}

fn inputs(p: vec3<f32>, color: vec4<f32>, uv: vec2<f32>, uv_b: vec2<f32>) -> Inputs {
    var t: Inputs;
    t.s.p = p;
    t.s.dx = dpdx(p);
    t.s.dy = dpdy(p);

    t.w_snow = color;
    t.cov = 2.0 * uv.x - 1.0;
    let code = u32(round(uv_b.y));
@if(SSX_TERRAIN_SIMPLE)
    t.group = 3u;
@else
    t.group = u32(round(terrain.snow_fall.w));
    t.base_bits = (code >> 12u) & 3u;
    if t.group == 1u {
        t.base_bits = 0u;
    }
    t.has_snow = false;
    for (var i = 0u; i < 4u; i++) {
        t.lanes[i] = (code >> (3u * i)) & 7u;
        t.has_snow = t.has_snow || t.lanes[i] != NO_LAYER;
    }
    if t.group == 2u {
        t.has_snow = false;
    }
    if t.group == 3u {
        t.has_snow = true;
    }
    t.b = uv.y;
    if t.base_bits == 1u {
        t.b = 0.0;
    } else if t.base_bits == 2u {
        t.b = 1.0;
    }

    let wp = vec4(p, 1.0);
    t.view_z = dot(vec4(
        view_bindings::view.view_from_world[0].z,
        view_bindings::view.view_from_world[1].z,
        view_bindings::view.view_from_world[2].z,
        view_bindings::view.view_from_world[3].z,
    ), wp);
    t.fade_snow = saturate(t.view_z * FADE.x + FADE.z);
    t.fade_base = saturate(t.view_z * FADE.y + FADE.w);
    if terrain.snow_blend.w == 0.0 {
        t.fade_snow = 1.0;
        t.fade_base = 1.0;
    }
    if t.group == 3u {
        t.fade_snow = 0.0;
    }
    return t;
}

struct Gbuffer {
    normal: vec3<f32>,
    ecc: f32,
    snow_spec_types: f32,
}

fn gbuffer(t: Inputs, world_normal: vec3<f32>) -> Gbuffer {
    let s = t.s;
    let b = t.b;
    let n = normalize(world_normal);
    var n_base = n;
    var ecc_base = 1.0;
    if t.base_bits != 0u {
        ecc_base = mix(terrain.base[0].z, terrain.base[1].z, b);
        if t.fade_base > 0.0 {
            let w = planar_weights(n);
            var d = vec3(0.0);
            if b != 1.0 { d += (1.0 - b) * terrain.base[0].y * base_perturbation(0u, s, w); }
            if b != 0.0 { d += b * terrain.base[1].y * base_perturbation(1u, s, w); }
            n_base = mix(n, normalize(n + d), t.fade_base);
        }
    }
    var normal = n_base;
    var ecc = ecc_base;
    var snow_spec_types = 0.0;
    if t.has_snow {
        var ecc_types = 0.0;
        for (var i = 0u; i < 4u; i++) {
            var ecc_i = 1.0;
            var spec_i = 1.0;
            if t.lanes[i] != NO_LAYER {
                ecc_i = snow_eccentricity(t.lanes[i]);
                spec_i = snow_specular(t.lanes[i]);
            }
            ecc_types += t.w_snow[i] * ecc_i;
            snow_spec_types += t.w_snow[i] * spec_i;
        }
        let ecc_snow = mix(snow_eccentricity(POWDER), ecc_types, t.fade_snow);
        var n_snow = n;
        if t.fade_snow > 0.0 {
            n_snow = mix(n, snow_detail(n, s, t.w_snow, t.lanes), t.fade_snow);
        }
        let c = saturate(t.cov);
        normal = mix(n_base, normalize(n_snow), c);
        ecc = mix(ecc_base, ecc_snow, c);
    }
    normal = normalize(normal);
    return Gbuffer(normal, ecc, snow_spec_types);
}
