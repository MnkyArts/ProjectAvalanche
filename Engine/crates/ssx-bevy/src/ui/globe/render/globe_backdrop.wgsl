struct FullOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) ndc: vec2<f32>,
};

@vertex
fn vs_full(@builtin(vertex_index) i: u32) -> FullOut {
    let uv = vec2(f32(i >> 1u), f32(i & 1u)) * 2.0;
    var out: FullOut;
    let ndc = uv * vec2(2.0, -2.0) + vec2(-1.0, 1.0);
    out.position = vec4(ndc, 0.0, 1.0);
    out.uv = uv;
    out.ndc = ndc;
    return out;
}

struct Backdrop {
    proj_inv: array<vec4<f32>, 4>,
    view_inv: array<vec4<f32>, 4>,
    view: array<vec4<f32>, 4>,
    camera: vec4<f32>,
    beta_r: vec4<f32>,
    beta_dash_r: vec4<f32>,
    beta_m: vec4<f32>,
    beta_dash_m: vec4<f32>,
    heights: vec4<f32>,
    multipliers: vec4<f32>,
    light_dir: vec4<f32>,
    light_colour: vec4<f32>,
    cloud_ramp0: vec4<f32>,
    cloud_ramp1: vec4<f32>,
    cloud: vec4<f32>,
    sun: vec4<f32>,
}

@group(0) @binding(0) var<uniform> b: Backdrop;
@group(0) @binding(1) var background: texture_2d<f32>;
@group(0) @binding(2) var cloud_density: texture_2d<f32>;
@group(0) @binding(3) var cloud_normal: texture_2d<f32>;
@group(0) @binding(4) var light_texture: texture_2d<f32>;
@group(0) @binding(5) var clamp_linear: sampler;
@group(0) @binding(6) var cloud_sampler: sampler;

fn row_mul(p: vec4<f32>, m: array<vec4<f32>, 4>) -> vec4<f32> {
    return p.x * m[0] + p.y * m[1] + p.z * m[2] + p.w * m[3];
}

fn d3d9_uv(position: vec4<f32>) -> vec2<f32> {
    return (position.xy - 0.5) * b.sun.zw;
}

@fragment
fn fs_background(in: FullOut) -> @location(0) vec4<f32> {
    let t = textureSampleLevel(background, clamp_linear, d3d9_uv(in.position), 0.0).rgb;
    return vec4<f32>(sqrt(abs(b.camera.w * t * t)), 1.0);
}

// PORT(choice): a slot movie keeps its aspect and covers the frame, cropped at the frame's edges (the slot is the
// port's own; the disc's fe_background is stretched over any frame, its full-screen movies fitted whole with black
// bars).
@fragment
fn fs_background_movie(in: FullOut) -> @location(0) vec4<f32> {
    let frame = 1.0 / b.sun.zw;
    let movie = vec2<f32>(textureDimensions(background));
    let covered = movie * max(frame.x / movie.x, frame.y / movie.y);
    let uv = (d3d9_uv(in.position) - 0.5) * frame / covered + 0.5;
    let t = textureSampleLevel(background, clamp_linear, uv, 0.0).rgb;
    return vec4<f32>(sqrt(abs(b.camera.w * t * t)), 1.0);
}

const PI: f32 = 3.14159265;

fn sky_atan2(y: f32, x: f32) -> f32 {
    let t = min(abs(y), abs(x)) / max(abs(y), abs(x));
    let t2 = t * t;
    let a = t * (0.999866 + t2 * (-0.3302995 + t2 * (0.180141 + t2 * (-0.085133 + 0.0208351 * t2))));
    var th = select(a, 1.5707964 - a, abs(y) > abs(x));
    th = select(th, th - 3.1415927, x < 0.0);
    return select(th, -th, (y < 0.0) != (x < 0.0));
}

fn sky_acos(c: f32) -> f32 {
    let a = abs(c);
    let r = sqrt(1.0 - a) * (1.5707288 + a * (-0.2121144 + a * (0.074261 - 0.0187293 * a)));
    return select(r, 3.1415927 - r, c < 0.0);
}

@fragment
fn fs_sky(in: FullOut) -> @location(0) vec4<f32> {
    let uv = d3d9_uv(in.position);
    let vp = row_mul(vec4<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, 1.0, 1.0), b.proj_inv);
    let v = normalize(vp.xyz / vp.w);
    let w = normalize(row_mul(vec4<f32>(v, 0.0), b.view_inv).xyz);
    let lw = b.light_dir.xyz;
    let lv = normalize(row_mul(vec4<f32>(lw, 0.0), b.view).xyz);
    let lc = b.light_colour.rgb;

    let radius = b.beta_dash_r.w;
    let s = b.beta_m.w;
    let p = (b.camera.xyz + s * 1000.0 * w) * 0.001;
    let h = length(vec3<f32>(p.x, p.y + radius, p.z)) - radius;
    let rho_r = exp((b.heights.y - h) * b.heights.x);
    let rho_m = exp((b.heights.w - h) * b.heights.z);
    let sigma = rho_r * b.beta_r.xyz + rho_m * b.beta_m.xyz;
    let t = exp(-sigma * s);
    let tc = exp(-sigma * s * b.beta_dash_m.w);

    let cw = clamp(dot(w, lw), 0.0, 1.0);
    let g = b.beta_r.w;
    let num = 1.0 - g * g;
    let den = pow(max(1.0 + g * g - 2.0 * g * cw, 0.0), 1.5);
    let mie_phase = select(num / den, 0.0, num == 0.0);
    let bd = b.beta_dash_r.xyz * (1.0 + cw * cw) + b.beta_dash_m.xyz * mie_phase;
    let fr = bd / (b.beta_r.xyz + b.beta_m.xyz);
    let lin = fr * (1.0 - t) * b.multipliers.y;
    let lin_c = fr * (1.0 - tc) * b.multipliers.y;

    let z = h / length(vec3<f32>(p.x, h, p.z));
    let cuv = vec2<f32>(sky_atan2(p.z, p.x) / (2.0 * PI) + 0.5 + b.light_colour.w, sky_acos(z) * 0.63661975);
    let d = textureSample(cloud_density, cloud_sampler, cuv).y;
    let c = clamp(d * b.cloud_ramp1.w, 0.0, 1.0);
    let tn = textureSample(cloud_normal, cloud_sampler, cuv).xy;
    let nt = normalize(vec3<f32>(1.0, 2.0 * tn.x - 1.0, 2.0 * tn.y - 1.0));
    let tg = cross(v, vec3<f32>(0.0, 1.0, 0.0));
    let bt = cross(tg, v);
    let nc = nt.x * (-v) + nt.y * tg + nt.z * bt;
    let wrap = 0.5 * dot(nc, lv) + 0.5;
    let gl_mult = b.multipliers.z;
    let cloud_col = gl_mult * b.cloud_ramp0.w * mix(b.cloud_ramp1.rgb, b.cloud_ramp0.rgb, mix(1.0 - d, d, wrap));

    let st = textureSampleLevel(light_texture, clamp_linear, b.light_dir.w * (v.xy - lv.xy) + 0.5, 0.0);
    let sa = b.multipliers.w * select(st.a, 0.0, lv.z >= 0.0);
    let sky_col = lin * lc + st.rgb * gl_mult * sa;
    var col = mix(sky_col, b.multipliers.x * tc * cloud_col + lin_c * lc, c);

    let cv = clamp(dot(lv, v), 0.0, 1.0) + 1e-7;
    let rim_d = abs(c - b.cloud.x);
    let rim_q = select(rim_d * b.cloud.y, 0.0, rim_d == 0.0);
    let rim = c * b.cloud.z * pow(cv, b.cloud.w) * clamp(1.0 - rim_q, 0.0, 1.0);
    let k = clamp((1.0 - sa) * pow(cv, b.sun.y) + rim, 0.0, 1.0) * (1.0 - c);
    col = mix(col, lc * (b.sun.x + lin), k);
    return vec4<f32>(sqrt(max(col, vec3<f32>(0.0))), 1.0);
}
