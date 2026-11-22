import bevy_pbr::render::{
    pbr_functions::alpha_discard,
    mesh_view_bindings::view,
};
import package::render::lighting::game_lighting as gl;

struct CharacterParams {
    shading: vec4<f32>,
    glow: vec4<f32>,
    lod_bias: vec4<f32>,
    board: vec4<f32>,
    holo_bloom: vec4<f32>,
}

struct CharacterLight {
    dynamic: vec4<f32>,
    shadow_point: vec4<f32>,
    frame: vec4<f32>,
}

struct CharacterFrame {
    dynamic: vec4<f32>,
    shadow_point: vec4<f32>,
    holo: vec4<f32>,
}

@group(constants::MATERIAL_BIND_GROUP) @binding(101) var<uniform> rider: CharacterLight;
@group(constants::MATERIAL_BIND_GROUP) @binding(102) var<uniform> params: CharacterParams;
@group(constants::MATERIAL_BIND_GROUP) @binding(103) var normal_map: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(104) var character_sampler: sampler;
@group(constants::MATERIAL_BIND_GROUP) @binding(121) var<storage, read> character_frame: CharacterFrame;

fn rider_dynamic() -> vec4<f32> {
    return select(rider.dynamic, character_frame.dynamic, rider.frame.x > 0.5);
}

fn rider_shadow_point() -> vec4<f32> {
    return select(rider.shadow_point, character_frame.shadow_point, rider.frame.x > 0.5);
}

fn holo() -> vec4<f32> {
    return character_frame.holo;
}

const HAIR: u32 = 1u;
const EYE: u32 = 2u;
const APPAREL: u32 = 3u;
const BOARD: u32 = 4u;
const WINGSUIT: u32 = 5u;
const SECONDARY: u32 = 6u;

fn ps5_alpha(effect: u32, d: vec4<f32>, flags: u32, cutoff: f32) -> vec4<f32> {
    if effect == WINGSUIT {
        if 0.0 > max(d.a, rider_dynamic().z) - 0.25 {
            discard;
        }
        return d;
    }
    return alpha_discard(flags, cutoff, d);
}

fn ps5_normal(world_normal: vec3<f32>, world_tangent: vec4<f32>, mapped: bool, tex_n: vec2<f32>) -> vec3<f32> {
    var n = normalize(world_normal);
    if mapped {
        let nt = normalize(vec3(tex_n * 2.0 - 1.0, 1.0));
        n = gl::game_tbn(world_normal, world_tangent, nt);
    }
    return n;
}

fn holo_on(effect: u32) -> bool {
    let h = holo();
    return (effect == SECONDARY || effect == BOARD) && h.x + h.y > 0.0;
}

fn holo_nz(n: vec3<f32>) -> f32 {
    return (view.view_from_world * vec4(n, 0.0)).z;
}

fn holo_kill(nz: f32) {
    let y = holo().y;
    if y > 0.0 && 0.4 + nz - 1.4 * y < 0.0 {
        discard;
    }
}
