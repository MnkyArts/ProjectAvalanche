import package::render::lighting::game_position;

struct TerrainRipple {
    a: vec4<f32>,
    b: vec4<f32>,
}

@group(constants::MATERIAL_BIND_GROUP) @binding(117) var<uniform> ripple: TerrainRipple;

fn apply(position: vec3<f32>, normal: vec3<f32>) -> array<vec3<f32>, 2> {
    var p = position;
    var n = normal;
    if ripple.a.x > 0.0 {
        let dxz = p.xz - ripple.b.xz;
        let d = max(length(dxz), 1e-6);
        let t = d - (ripple.a.w + ripple.b.w);
        let h = saturate(t) * ripple.a.x * (1.0 - saturate((pow(abs(t + 1.0), ripple.a.y) - 1.0) / ripple.a.x));
        p.y += h * (1.0 - cos(ripple.a.z * t));
        n = n - vec3(dxz.x, 0.0, dxz.y) / d * h * ripple.a.z * sin(ripple.a.z * t);
    }
    return array<vec3<f32>, 2>(p, n);
}

fn place(world_position: vec3<f32>, world_normal: vec3<f32>) -> game_position::Placed {
    let rippled = apply(world_position, world_normal);
    return game_position::place(rippled[0], rippled[1]);
}
