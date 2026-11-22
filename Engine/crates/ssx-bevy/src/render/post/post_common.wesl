struct PostUniform {
    hable_a: vec4<f32>,
    hable_b: vec4<f32>,
    exposure: vec4<f32>,
    bloom: vec4<f32>,
    blur_h_samples: vec4<f32>,
    blur_h_offsets: vec4<f32>,
    blur_v_samples: vec4<f32>,
    blur_v_offsets: vec4<f32>,
    sizes: vec4<u32>,
    grade: vec4<f32>,
}

struct Exposure {
    log2_mean: f32,
    lavg: f32,
    exposure: f32,
    bloom_exposure: f32,
}

const LUMA: vec3<f32> = vec3<f32>(0.299, 0.587, 0.114);

fn hable(x: vec3<f32>, a: vec4<f32>, b: vec4<f32>) -> vec3<f32> {
    return (x * (a.x * x + a.z) + a.w) / (x * (a.x * x + a.y) + b.x) - b.y;
}
