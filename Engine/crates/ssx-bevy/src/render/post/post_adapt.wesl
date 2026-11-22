import package::render::post::post_common::{PostUniform, Exposure, LUMA};

@group(0) @binding(0) var<uniform> u: PostUniform;
@group(0) @binding(1) var quarter: texture_2d<f32>;
@group(0) @binding(2) var<storage, read_write> adapted: array<f32>;
@group(0) @binding(3) var<storage, read_write> exposure: Exposure;

const ENC: f32 = 0.0433217;
const DEC: f32 = 23.08312;

fn store16(x: f32) -> f32 {
    let h = unpack2x16float(pack2x16float(vec2<f32>(x, 0.0))).x;
    return round(clamp(h, 0.0, 1.0) * 65535.0) / 65535.0;
}

@compute @workgroup_size(8, 8, 1)
fn adapt(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = u.sizes.zw;
    if (id.x >= size.x || id.y >= size.y) {
        return;
    }
    let q = textureLoad(quarter, vec2<i32>(id.xy), 0).rgb;
    let c = q * q;
    let l_enc = store16(log2(max(dot(c, LUMA), 1.0)) * ENC);
    let i = id.y * size.x + id.x;
    var a_enc = 0.0;
    if (u.bloom.w == 1.0) {
        a_enc = l_enc;
    } else if (u.bloom.w == 0.0 || u.bloom.w == 3.0) {
        let a = exp2(DEC * adapted[i]);
        let lc = select(exp2(DEC * l_enc), 1.0, u.bloom.w == 3.0);
        let k = max(1.0 - exp2(-u.exposure.w * 1.442695 * u.exposure.z), 0.0);
        a_enc = store16(log2(max(a + (lc - a) * k, 1.0)) * ENC);
    }
    adapted[i] = a_enc;
}

fn fetch(base: u32, w: u32, h: u32, uv: vec2<f32>) -> f32 {
    let p = uv * vec2<f32>(f32(w), f32(h)) - 0.5;
    let p0 = floor(p);
    let f = p - p0;
    let x0 = u32(clamp(p0.x, 0.0, f32(w - 1u)));
    let y0 = u32(clamp(p0.y, 0.0, f32(h - 1u)));
    let x1 = u32(clamp(p0.x + 1.0, 0.0, f32(w - 1u)));
    let y1 = u32(clamp(p0.y + 1.0, 0.0, f32(h - 1u)));
    let a = mix(adapted[base + y0 * w + x0], adapted[base + y0 * w + x1], f.x);
    let b = mix(adapted[base + y1 * w + x0], adapted[base + y1 * w + x1], f.x);
    return mix(a, b, f.y);
}

struct Level {
    base: u32,
    w: u32,
    h: u32,
}

fn chain_level_next(l: Level) -> Level {
    return Level(l.base + l.w * l.h, max(l.w / 2u, 1u), max(l.h / 2u, 1u));
}

fn chain_level(k: u32) -> Level {
    var l = Level(0u, u.sizes.z, u.sizes.w);
    for (var j = 0u; j < k; j++) {
        if (l.w == 1u && l.h == 1u) {
            break;
        }
        l = chain_level_next(l);
    }
    return l;
}

fn chain_texel(l: Level, i: u32) {
    let nw = max(l.w / 2u, 1u);
    let nh = max(l.h / 2u, 1u);
    let uv = (vec2<f32>(f32(i % nw), f32(i / nw)) + 0.5) / vec2<f32>(f32(nw), f32(nh));
    adapted[l.base + l.w * l.h + i] = store16(fetch(l.base, l.w, l.h, uv));
}

// PORT(choice): the chain's first PARALLEL_LEVELS levels (94 % of its texels: 240 × 135 and 120 × 67 of 43 055 at
// 1920 × 1080) are made by dispatches of many workgroups, one texel an invocation (`reduce_level`), and only the rest
// by the single workgroup of `reduce`, which made every level before (127 rounds of its 256 invocations for level 1
// at 1080p). Every texel is the same function of the level before it, so the same values in any order. The console
// made each level with a BlitL16 draw.
const PARALLEL_LEVELS: u32 = 2u;

fn reduce_level(k: u32, i: u32) {
    let l = chain_level(k);
    if ((l.w == 1u && l.h == 1u) || i >= max(l.w / 2u, 1u) * max(l.h / 2u, 1u)) {
        return;
    }
    chain_texel(l, i);
}

@compute @workgroup_size(256, 1, 1)
fn reduce_level1(@builtin(global_invocation_id) id: vec3<u32>) {
    reduce_level(0u, id.x);
}

@compute @workgroup_size(256, 1, 1)
fn reduce_level2(@builtin(global_invocation_id) id: vec3<u32>) {
    reduce_level(1u, id.x);
}

@compute @workgroup_size(256, 1, 1)
fn reduce(@builtin(local_invocation_index) li: u32) {
    var l = chain_level(PARALLEL_LEVELS);
    loop {
        if (l.w == 1u && l.h == 1u) {
            break;
        }
        for (var i = li; i < max(l.w / 2u, 1u) * max(l.h / 2u, 1u); i += 256u) {
            chain_texel(l, i);
        }
        storageBarrier();
        workgroupBarrier();
        l = chain_level_next(l);
    }
    let base = l.base;
    if (li == 0u) {
        let m = DEC * adapted[base];
        let lavg = exp2(m);
        let e = u.exposure.y / lavg;
        exposure.log2_mean = m;
        exposure.lavg = lavg;
        exposure.exposure = e;
        exposure.bloom_exposure = exp2(log2(max(e, 0.0001)) - u.bloom.x);
    }
}
