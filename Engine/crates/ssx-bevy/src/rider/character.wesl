import bevy_pbr::render::{
    pbr_fragment::pbr_input_from_standard_material,
    forward_io::{VertexOutput, FragmentOutput},
    pbr_functions::main_pass_post_lighting_processing,
    mesh_types::MESH_FLAGS_SHADOW_RECEIVER_BIT,
    mesh_view_bindings::view,
};
import package::render::lighting::game_lighting as gl;
import package::rider::character_normal::{
    params, rider, normal_map, character_sampler, rider_dynamic, rider_shadow_point,
    HAIR, EYE, APPAREL, BOARD, SECONDARY, ps5_alpha, ps5_normal, holo, holo_on, holo_nz, holo_kill,
};

@if(SSX_LOCAL_LIGHTS)
var<private> light: gl::GameLight;
@else
@group(constants::MATERIAL_BIND_GROUP) @binding(100) var<uniform> light: gl::GameLight;
@group(constants::MATERIAL_BIND_GROUP) @binding(105) var specular_map: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(106) var ambient_map: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(107) var<storage, read> level: gl::LevelLight;

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

    pbr_input.material.base_color =
        ps5_alpha(effect, pbr_input.material.base_color, pbr_input.material.flags, pbr_input.material.alpha_cutoff);
    let d = pbr_input.material.base_color;

    var tangent = vec4(0.0);
    var mapped = false;
@if(VERTEX_TANGENTS && VERTEX_UVS_A) {
    tangent = in.world_tangent;
    mapped = params.shading.w > 0.0;
}
    let n = ps5_normal(in.world_normal, tangent, mapped, tex_n);

    var holo_add = vec3(0.0);
    var holo_glow = 0.0;
    if holo_on(effect) {
        let nz = holo_nz(n);
        holo_kill(nz);
        let ndc_x = (in.position.x - view.viewport.x) / view.viewport.z * 2.0 - 1.0;
        let scan = 2.0 * abs(fract(60.0 * ndc_x) - 0.5);
        let h = holo();
        if h.y > 0.0 && 1.4 * h.y > 0.3 + nz {
            holo_add = params.holo_bloom.rgb * scan;
        }
        holo_glow = (h.x + h.y) * scan * scan * scan;
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
    let preset = level.rim_fill;
    let rim_dir = normalize((view.world_from_view * vec4(preset.rim_dir.xyz, 0.0)).xyz);
    let rim_base = preset.rim_color.w * pow_safe(saturate((1.0 - abs(dot(n, v))) * (1.0 + preset.rim.y)), preset.rim.x);
    let rim_offset = 1.0 + preset.rim.w * (pow_safe(dot(n, rim_dir) * 0.5 + 0.5, preset.rim.z) - 1.0);
    let rim = rim_base * rim_offset * (1.0 + preset.rim_dir.w * (pst - 1.0));
    let fill = preset.fill_color.w * saturate(dot(n, preset.fill_dir.xyz)) * (1.0 + preset.fill_dir.w * (pst - 1.0));

    let dyn_ao = rider_dynamic().x;
    let albedo = d.rgb * d.rgb + holo_add;
    let spec_light = light.specular.rgb * lit.a;
    let si = params.shading.y;
    var col: vec3<f32>;
    if effect == HAIR {
        let tint = mix(light.ramp_shade.rgb, vec3(1.0), ao_map * ao_map);
        let spec = (si * s) * (si * s);
        col = albedo * tint * (lit.rgb + rim * preset.rim_color.rgb + fill * preset.fill_color.rgb) + spec * spec_light;
    } else if effect == EYE {
        let tint = mix(light.ramp_shade.rgb, vec3(1.0), ao_map * ao_map * dyn_ao);
        let spec = (si * s.g) * (si * s.g);
        col = albedo * tint * (lit.rgb + fill * preset.fill_color.rgb) + spec * spec_light * dyn_ao;
    } else {
        let tint = mix(light.ramp_shade.rgb, vec3(1.0), dyn_ao);
        let spec = (si * s.g) * (si * s.g);
        var rim_light = rim;
        if effect == BOARD {
            rim_light = rim * params.board.x;
        }
        col = albedo * tint * (lit.rgb + rim_light * preset.rim_color.rgb + fill * preset.fill_color.rgb)
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
