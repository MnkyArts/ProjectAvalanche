import bevy_pbr::render::{
    pbr_fragment::pbr_input_from_standard_material,
    forward_io::{VertexOutput, FragmentOutput},
    pbr_functions::{alpha_discard, main_pass_post_lighting_processing},
    mesh_types::MESH_FLAGS_SHADOW_RECEIVER_BIT,
    mesh_view_bindings::view,
};
import package::render::lighting::game_lighting as gl;

struct CharacterParams {
    shading: vec4<f32>,
    glow: vec4<f32>,
    lod_bias: vec4<f32>,
    board: vec4<f32>,
    holo: vec4<f32>,
    holo_bloom: vec4<f32>,
}

struct CharacterLight {
    rim_color: vec4<f32>,
    rim: vec4<f32>,
    rim_dir: vec4<f32>,
    fill_color: vec4<f32>,
    fill_dir: vec4<f32>,
    dynamic: vec4<f32>,
    shadow_point: vec4<f32>,
    frame: vec4<f32>,
}

struct RiderFrame {
    dynamic: vec4<f32>,
    shadow_point: vec4<f32>,
}

@if(SSX_LOCAL_LIGHTS)
var<private> light: gl::GameLight;
@else
@group(constants::MATERIAL_BIND_GROUP) @binding(100) var<uniform> light: gl::GameLight;
@group(constants::MATERIAL_BIND_GROUP) @binding(101) var<uniform> rider: CharacterLight;
@group(constants::MATERIAL_BIND_GROUP) @binding(102) var<uniform> params: CharacterParams;
@group(constants::MATERIAL_BIND_GROUP) @binding(103) var normal_map: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(104) var character_sampler: sampler;
@group(constants::MATERIAL_BIND_GROUP) @binding(105) var specular_map: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(106) var ambient_map: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(121) var<storage, read> rider_frame: RiderFrame;

fn rider_dynamic() -> vec4<f32> {
    return select(rider.dynamic, rider_frame.dynamic, rider.frame.x > 0.5);
}

fn rider_shadow_point() -> vec4<f32> {
    return select(rider.shadow_point, rider_frame.shadow_point, rider.frame.x > 0.5);
}

const HAIR: u32 = 1u;
const EYE: u32 = 2u;
const APPAREL: u32 = 3u;
const BOARD: u32 = 4u;
const WINGSUIT: u32 = 5u;
const SECONDARY: u32 = 6u;

fn pow_safe(x: f32, y: f32) -> f32 {
    return exp2(y * log2(max(x, 1e-7)));
}

fn player_shadow_test(frag_coord: vec2<f32>) -> f32 {
    let point = rider_shadow_point();
    if point.w <= 0.0 {
        return 1.0;
    }
    return round(saturate(gl::shadow_query(point.xyz)) * 255.0) / 255.0;
}

@fragment
fn fragment(in: VertexOutput, @builtin(front_facing) is_front: bool) -> FragmentOutput {
@if(SSX_LOCAL_LIGHTS)
    light = gl::frame_light();
    var pbr_input = pbr_input_from_standard_material(in, is_front);
    let effect = u32(round(params.shading.x));
    let p = in.world_position.xyz;

    var tex_n = vec2(0.5);
    var s = vec3(1.0);
    var ao_map = 1.0;
    var glow_mask = pbr_input.material.base_color.a;
@if(VERTEX_UVS_A) {
    tex_n = textureSampleBias(normal_map, character_sampler, in.uv, params.lod_bias.y).xy;
    s = textureSampleBias(specular_map, character_sampler, in.uv, params.lod_bias.z).rgb;
    let ao_uv0 = textureSampleBias(ambient_map, character_sampler, in.uv, params.lod_bias.w).y;
    if effect == EYE {
        ao_map = ao_uv0;
    }
    pbr_input.material.base_color = gl::diffuse_texel(in.uv, params.lod_bias.x);
    glow_mask = pbr_input.material.base_color.a;
}
@if(VERTEX_UVS_B) {
    let ao_uv1 = textureSampleBias(ambient_map, character_sampler, in.uv_b, params.lod_bias.w).y;
    if effect == HAIR {
        ao_map = ao_uv1;
    }
}

    if effect == WINGSUIT {
        if 0.0 > max(pbr_input.material.base_color.a, rider_dynamic().z) - 0.25 {
            discard;
        }
    } else {
        pbr_input.material.base_color =
            alpha_discard(pbr_input.material.flags, pbr_input.material.alpha_cutoff, pbr_input.material.base_color);
    }
    let d = pbr_input.material.base_color;

    var n = normalize(in.world_normal);
@if(VERTEX_TANGENTS && VERTEX_UVS_A) {
    if params.shading.w > 0.0 {
        let nt = normalize(vec3(tex_n * 2.0 - 1.0, 1.0));
        n = gl::game_tbn(in.world_normal, in.world_tangent, nt);
    }
}

    var holo_add = vec3(0.0);
    var holo_glow = 0.0;
    if (effect == SECONDARY || effect == BOARD) && params.holo.x + params.holo.y > 0.0 {
        let nz = (view.view_from_world * vec4(n, 0.0)).z;
        if params.holo.y > 0.0 && 0.4 + nz - 1.4 * params.holo.y < 0.0 {
            discard;
        }
        let ndc_x = (in.position.x - view.viewport.x) / view.viewport.z * 2.0 - 1.0;
        let scan = 2.0 * abs(fract(60.0 * ndc_x) - 0.5);
        if params.holo.y > 0.0 && 1.4 * params.holo.y > 0.3 + nz {
            holo_add = params.holo_bloom.rgb * scan;
        }
        holo_glow = (params.holo.x + params.holo.y) * scan * scan * scan;
    }

    let v = gl::to_eye(p);
    let receiver = (pbr_input.flags & MESH_FLAGS_SHADOW_RECEIVER_BIT) != 0u;
    let wp = vec4(p, 1.0);
    let shadow = gl::sun_shadow(wp, normalize(in.world_normal), gl::view_z(wp), in.position.xy, receiver);
    let lit = gl::key_light(light, n, v, params.shading.z, shadow, gl::contact_shadow(p), 1.0);

    var pst = 1.0;
    if effect != SECONDARY {
        pst = player_shadow_test(in.position.xy);
    }
    let rim_dir = normalize((view.world_from_view * vec4(rider.rim_dir.xyz, 0.0)).xyz);
    let rim_base = rider.rim_color.w * pow_safe(saturate((1.0 - abs(dot(n, v))) * (1.0 + rider.rim.y)), rider.rim.x);
    let rim_offset = 1.0 + rider.rim.w * (pow_safe(dot(n, rim_dir) * 0.5 + 0.5, rider.rim.z) - 1.0);
    let rim = rim_base * rim_offset * (1.0 + rider.rim_dir.w * (pst - 1.0));
    let fill = rider.fill_color.w * saturate(dot(n, rider.fill_dir.xyz)) * (1.0 + rider.fill_dir.w * (pst - 1.0));

    let dyn_ao = rider_dynamic().x;
    let albedo = d.rgb * d.rgb + holo_add;
    let spec_light = light.specular.rgb * lit.a;
    let si = params.shading.y;
    var col: vec3<f32>;
    if effect == HAIR {
        let tint = mix(light.ramp_shade.rgb, vec3(1.0), ao_map * ao_map);
        let spec = (si * s) * (si * s);
        col = albedo * tint * (lit.rgb + rim * rider.rim_color.rgb + fill * rider.fill_color.rgb) + spec * spec_light;
    } else if effect == EYE {
        let tint = mix(light.ramp_shade.rgb, vec3(1.0), ao_map * ao_map * dyn_ao);
        let spec = (si * s.g) * (si * s.g);
        col = albedo * tint * (lit.rgb + fill * rider.fill_color.rgb) + spec * spec_light * dyn_ao;
    } else {
        let tint = mix(light.ramp_shade.rgb, vec3(1.0), dyn_ao);
        let spec = (si * s.g) * (si * s.g);
        var rim_light = rim;
        if effect == BOARD {
            rim_light = rim * params.board.x;
        }
        col = albedo * tint * (lit.rgb + rim_light * rider.rim_color.rgb + fill * rider.fill_color.rgb)
            + spec * spec_light * dyn_ao;
        if effect == APPAREL {
            col += params.glow.rgb * glow_mask * rider_dynamic().y / max(light.misc.y, 1e-6);
        } else if effect == BOARD {
            col += params.board.y * glow_mask * rider_dynamic().y * albedo / max(light.misc.y, 1e-6);
        }
        col += holo_glow * albedo;
    }

    let fogged = gl::apply_fog(light, col, in.world_position.w);
    var out: FragmentOutput;
    out.color = main_pass_post_lighting_processing(pbr_input, vec4(fogged, 1.0));
    return out;
}
