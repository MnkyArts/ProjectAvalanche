import bevy_core_pipeline::fullscreen_vertex_shader::FullscreenVertexOutput;

struct FsrUniform {
    con0: vec4<f32>,
    con1: vec4<f32>,
    con2: vec4<f32>,
    con3: vec4<f32>,
    rcas: vec4<f32>,
}

@group(0) @binding(0) var<uniform> fsr: FsrUniform;
@group(0) @binding(1) var source: texture_2d<f32>;
@group(0) @binding(2) var clamp_sampler: sampler;

fn prx_lo_rcp(a: f32) -> f32 {
    return bitcast<f32>(0x7ef07ebbu - bitcast<u32>(a));
}
fn prx_med_rcp(a: f32) -> f32 {
    let b = bitcast<f32>(0x7ef19fffu - bitcast<u32>(a));
    return b * (-b * a + 2.0);
}
fn prx_lo_rsq(a: f32) -> f32 {
    return bitcast<f32>(0x5f347d74u - (bitcast<u32>(a) >> 1u));
}

struct DirLen {
    dir: vec2<f32>,
    len: f32,
}

fn easu_set(acc: DirLen, w: f32, la: f32, lb: f32, lc: f32, ld: f32, le: f32) -> DirLen {
    var o = acc;
    let dc = ld - lc;
    let cb = lc - lb;
    var len_x = max(abs(dc), abs(cb));
    len_x = prx_lo_rcp(len_x);
    let dir_x = ld - lb;
    o.dir.x += dir_x * w;
    len_x = saturate(abs(dir_x) * len_x);
    len_x *= len_x;
    o.len += len_x * w;
    let ec = le - lc;
    let ca = lc - la;
    var len_y = max(abs(ec), abs(ca));
    len_y = prx_lo_rcp(len_y);
    let dir_y = le - la;
    o.dir.y += dir_y * w;
    len_y = saturate(abs(dir_y) * len_y);
    len_y *= len_y;
    o.len += len_y * w;
    return o;
}

fn easu_tap(acc: vec4<f32>, off: vec2<f32>, dir: vec2<f32>, len: vec2<f32>, lob: f32, clp: f32, c: vec3<f32>)
    -> vec4<f32> {
    var v = vec2<f32>(off.x * dir.x + off.y * dir.y, off.x * (-dir.y) + off.y * dir.x);
    v *= len;
    let d2 = min(v.x * v.x + v.y * v.y, clp);
    var wb = (2.0 / 5.0) * d2 - 1.0;
    var wa = lob * d2 - 1.0;
    wb *= wb;
    wa *= wa;
    wb = (25.0 / 16.0) * wb - (25.0 / 16.0 - 1.0);
    let w = wb * wa;
    return acc + vec4<f32>(c * w, w);
}

fn easu(ip: vec2<f32>) -> vec3<f32> {
    var pp = ip * fsr.con0.xy + fsr.con0.zw;
    let fp = floor(pp);
    pp -= fp;
    let p0 = fp * fsr.con1.xy + fsr.con1.zw;
    let p1 = p0 + fsr.con2.xy;
    let p2 = p0 + fsr.con2.zw;
    let p3 = p0 + fsr.con3.xy;
    let bczz_r = textureGather(0, source, clamp_sampler, p0);
    let bczz_g = textureGather(1, source, clamp_sampler, p0);
    let bczz_b = textureGather(2, source, clamp_sampler, p0);
    let ijfe_r = textureGather(0, source, clamp_sampler, p1);
    let ijfe_g = textureGather(1, source, clamp_sampler, p1);
    let ijfe_b = textureGather(2, source, clamp_sampler, p1);
    let klhg_r = textureGather(0, source, clamp_sampler, p2);
    let klhg_g = textureGather(1, source, clamp_sampler, p2);
    let klhg_b = textureGather(2, source, clamp_sampler, p2);
    let zzon_r = textureGather(0, source, clamp_sampler, p3);
    let zzon_g = textureGather(1, source, clamp_sampler, p3);
    let zzon_b = textureGather(2, source, clamp_sampler, p3);
    let bczz_l = bczz_b * 0.5 + (bczz_r * 0.5 + bczz_g);
    let ijfe_l = ijfe_b * 0.5 + (ijfe_r * 0.5 + ijfe_g);
    let klhg_l = klhg_b * 0.5 + (klhg_r * 0.5 + klhg_g);
    let zzon_l = zzon_b * 0.5 + (zzon_r * 0.5 + zzon_g);
    let bl = bczz_l.x;
    let cl = bczz_l.y;
    let il = ijfe_l.x;
    let jl = ijfe_l.y;
    let fl = ijfe_l.z;
    let el = ijfe_l.w;
    let kl = klhg_l.x;
    let ll = klhg_l.y;
    let hl = klhg_l.z;
    let gl = klhg_l.w;
    let ol = zzon_l.z;
    let nl = zzon_l.w;
    var dl = DirLen(vec2<f32>(0.0), 0.0);
    dl = easu_set(dl, (1.0 - pp.x) * (1.0 - pp.y), bl, el, fl, gl, jl);
    dl = easu_set(dl, pp.x * (1.0 - pp.y), cl, fl, gl, hl, kl);
    dl = easu_set(dl, (1.0 - pp.x) * pp.y, fl, il, jl, kl, nl);
    dl = easu_set(dl, pp.x * pp.y, gl, jl, kl, ll, ol);
    var dir = dl.dir;
    let dir2 = dir * dir;
    var dir_r = dir2.x + dir2.y;
    let zro = dir_r < 1.0 / 32768.0;
    dir_r = select(prx_lo_rsq(dir_r), 1.0, zro);
    dir.x = select(dir.x, 1.0, zro);
    dir *= dir_r;
    var len = dl.len * 0.5;
    len *= len;
    let stretch = (dir.x * dir.x + dir.y * dir.y) * prx_lo_rcp(max(abs(dir.x), abs(dir.y)));
    let len2 = vec2<f32>(1.0 + (stretch - 1.0) * len, 1.0 - 0.5 * len);
    let lob = 0.5 + ((1.0 / 4.0 - 0.04) - 0.5) * len;
    let clp = prx_lo_rcp(lob);
    let f = vec3<f32>(ijfe_r.z, ijfe_g.z, ijfe_b.z);
    let g = vec3<f32>(klhg_r.w, klhg_g.w, klhg_b.w);
    let j = vec3<f32>(ijfe_r.y, ijfe_g.y, ijfe_b.y);
    let k = vec3<f32>(klhg_r.x, klhg_g.x, klhg_b.x);
    let min4 = min(min(min(f, g), j), k);
    let max4 = max(max(max(f, g), j), k);
    var a = vec4<f32>(0.0);
    a = easu_tap(a, vec2<f32>(0.0, -1.0) - pp, dir, len2, lob, clp, vec3<f32>(bczz_r.x, bczz_g.x, bczz_b.x));
    a = easu_tap(a, vec2<f32>(1.0, -1.0) - pp, dir, len2, lob, clp, vec3<f32>(bczz_r.y, bczz_g.y, bczz_b.y));
    a = easu_tap(a, vec2<f32>(-1.0, 1.0) - pp, dir, len2, lob, clp, vec3<f32>(ijfe_r.x, ijfe_g.x, ijfe_b.x));
    a = easu_tap(a, vec2<f32>(0.0, 1.0) - pp, dir, len2, lob, clp, j);
    a = easu_tap(a, vec2<f32>(0.0, 0.0) - pp, dir, len2, lob, clp, f);
    a = easu_tap(a, vec2<f32>(-1.0, 0.0) - pp, dir, len2, lob, clp, vec3<f32>(ijfe_r.w, ijfe_g.w, ijfe_b.w));
    a = easu_tap(a, vec2<f32>(1.0, 1.0) - pp, dir, len2, lob, clp, k);
    a = easu_tap(a, vec2<f32>(2.0, 1.0) - pp, dir, len2, lob, clp, vec3<f32>(klhg_r.y, klhg_g.y, klhg_b.y));
    a = easu_tap(a, vec2<f32>(2.0, 0.0) - pp, dir, len2, lob, clp, vec3<f32>(klhg_r.z, klhg_g.z, klhg_b.z));
    a = easu_tap(a, vec2<f32>(1.0, 0.0) - pp, dir, len2, lob, clp, g);
    a = easu_tap(a, vec2<f32>(1.0, 2.0) - pp, dir, len2, lob, clp, vec3<f32>(zzon_r.z, zzon_g.z, zzon_b.z));
    a = easu_tap(a, vec2<f32>(0.0, 2.0) - pp, dir, len2, lob, clp, vec3<f32>(zzon_r.w, zzon_g.w, zzon_b.w));
    return min(max4, max(min4, a.rgb * (1.0 / a.w)));
}

@fragment
fn easu_pass(in: FullscreenVertexOutput) -> @location(0) vec4<f32> {
    return vec4<f32>(easu(floor(in.position.xy)), 1.0);
}

const RCAS_LIMIT: f32 = 0.25 - 1.0 / 16.0;

fn rcas_load(p: vec2<i32>) -> vec3<f32> {
    let last = vec2<i32>(textureDimensions(source)) - 1;
    return textureLoad(source, clamp(p, vec2<i32>(0), last), 0).rgb;
}

fn max3(a: f32, b: f32, c: f32) -> f32 {
    return max(a, max(b, c));
}

fn rcas(sp: vec2<i32>) -> vec3<f32> {
    let b = rcas_load(sp + vec2<i32>(0, -1));
    let d = rcas_load(sp + vec2<i32>(-1, 0));
    let e = rcas_load(sp);
    let f = rcas_load(sp + vec2<i32>(1, 0));
    let h = rcas_load(sp + vec2<i32>(0, 1));
    let mn4 = min(min(min(b, d), f), h);
    let mx4 = max(max(max(b, d), f), h);
    let hit_min = min(mn4, e) * (1.0 / (4.0 * mx4));
    let hit_max = (1.0 - max(mx4, e)) * (1.0 / (4.0 * mn4 - 4.0));
    let lobe_rgb = max(-hit_min, hit_max);
    let lobe = max(-RCAS_LIMIT, min(max3(lobe_rgb.r, lobe_rgb.g, lobe_rgb.b), 0.0)) * fsr.rcas.x;
    let rcp_l = prx_med_rcp(4.0 * lobe + 1.0);
    return (lobe * b + lobe * d + lobe * h + lobe * f + e) * rcp_l;
}

fn srgb_to_linear(c: vec3<f32>) -> vec3<f32> {
    return select(pow((c + 0.055) / 1.055, vec3<f32>(2.4)), c / 12.92, c <= vec3<f32>(0.04045));
}

@fragment
fn rcas_pass(in: FullscreenVertexOutput) -> @location(0) vec4<f32> {
    let pix = rcas(vec2<i32>(floor(in.position.xy)));
    return vec4<f32>(srgb_to_linear(clamp(pix, vec3<f32>(0.0), vec3<f32>(1.0))), 1.0);
}
