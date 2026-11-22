use super::*;

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 32) as u32
    }
    fn f(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * (self.next() as f32 / u32::MAX as f32)
    }
}

fn ridgiprocity_cells(heights: &[u8], to_world: &Mat4) -> Vec<u8> {
    let mut out = vec![0u8; GRID * GRID];
    let cell = to_world.x_axis.truncate().length();
    let w = (0.35 / cell + 0.5).floor().max(1.0) as usize;
    let world = |x: usize, y: usize, h: u8| apply(to_world, Vec3::new(x as f32, y as f32, f32::from(h)));
    for y in 0..GRID {
        let row = &heights[y * GRID..(y + 1) * GRID];
        for x in w..GRID.saturating_sub(w) {
            let h0 = row[x];
            if h0 == 0 {
                continue;
            }
            let p = world(x, y, h0);
            let (pl, pr) = (world(x - w, y, row[x - w]), world(x + w, y, row[x + w]));
            let c = (pl - p).normalize().dot((pr - p).normalize()).clamp(-1.0, 1.0);
            let k = (1.0 + c).sqrt() * std::f32::consts::FRAC_1_SQRT_2;
            let mut r = ((k - 0.25) / (0.5 - 0.25)).clamp(0.0, 1.0);
            if 0.5 * (pl.y + pr.y) >= p.y {
                r = 0.0;
            }
            out[y * GRID + x] = (255.0 * r + 0.5).floor() as u8;
        }
        for x in 0..GRID {
            if row[x] >= 240 {
                out[y * GRID + x.saturating_sub(w)..y * GRID + (x + w).min(GRID)].fill(0);
            }
        }
    }
    out
}

fn random_region(rng: &mut Rng) -> Region {
    let i = DispatchInputs {
        traj: Vec3::new(rng.f(-500.0, 500.0), rng.f(-100.0, 900.0), rng.f(-500.0, 500.0)),
        velocity: Vec3::new(rng.f(-1.0, 1.0), rng.f(-1.0, 0.2), rng.f(-1.0, 1.0)),
        orientation: Quat::IDENTITY,
        in_air: rng.next().is_multiple_of(3),
        grind_steering: None,
        ballistic_valid: false,
        ballistic_end: (Vec3::ZERO, Vec3::ZERO),
        ticks_to_land: 0.0,
        l2: false,
        super_grind: false,
        wind: Vec3::ZERO,
        k_h_air: 0.0,
        k_v_air: 0.0,
        g_air: 0.0,
        dt: 2.0,
    };
    region(&i, &LookAhead::default())
}

#[test]
fn the_tabled_ridgiprocity_is_the_cell_form() {
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
    let mut ridges = 0;
    for case in 0..24 {
        let r = random_region(&mut rng);
        let to_world = r.to_grid.inverse();
        let mut heights = vec![0u8; GRID * GRID];
        for y in 0..GRID {
            let (base, slope) = (rng.f(20.0, 200.0), rng.f(-0.6, 0.6));
            let kind = rng.next() % 5;
            for x in 0..GRID {
                let v = match kind {
                    0 => 0.0,
                    1 => base + slope * x as f32,
                    2 => base - (x as f32 - 128.0).abs() * slope.abs(),
                    3 => (base / 16.0).floor() * 16.0,
                    _ => rng.f(0.0, 255.0),
                };
                heights[y * GRID + x] = if rng.next().is_multiple_of(17) {
                    if case % 2 == 0 { 0 } else { 250 }
                } else {
                    v.clamp(0.0, 255.0) as u8
                };
            }
        }
        let cells = ridgiprocity_cells(&heights, &to_world);
        ridges += cells.iter().filter(|&&r| r != 0).count();
        assert_eq!(ridgiprocity(&heights, &to_world), cells, "case {case}");
        let t = RidgeTables::new(&to_world);
        let plain: Vec<u8> = (0..GRID)
            .flat_map(|y| {
                let mut out = [0u8; GRID];
                ridge_row_with(&t, heights[y * GRID..(y + 1) * GRID].try_into().unwrap(), y, &mut out);
                out
            })
            .collect();
        assert_eq!(plain, cells, "case {case}");
        let sa = SurfaceAnalysis { to_world, heights: heights.clone(), ..SurfaceAnalysis::default() };
        assert_eq!(sa.ridge(100, 7), cells[7 * GRID + 100]);
        assert_eq!(sa.ridges(), cells, "case {case}");
    }
    assert!(ridges > 1000, "{ridges} ridge cells");
}

fn scan_divisions(raster: &mut [u8], v0: Vec3, v1: Vec3, v2: Vec3) {
    let round = |v: Vec3| Vec3::new((v.x + 0.5).floor(), (v.y + 0.5).floor(), (v.z + 0.5).floor());
    let mut p = [round(v0), round(v1), round(v2)];
    p.sort_by(|a, b| a.y.total_cmp(&b.y));
    let [t, m, b] = p;
    if t.y == b.y {
        return;
    }
    let (yt, ym, yb) = (t.y as i64, m.y as i64, b.y as i64);
    let (xt, xm, xb) = (t.x as i64, m.x as i64, b.x as i64);
    let s_l = slope(b.z - t.z, b.y - t.y);
    let s_tm = slope(t.z - m.z, t.y - m.y);
    let s_mb = slope(b.z - m.z, b.y - m.y);
    let y0 = t.y.max(0.0);
    let mut zl = s_l.mul_add(y0, t.z - s_l * t.y);
    let mut ztm = s_tm.mul_add(y0, m.z - s_tm * m.y);
    let mut zmb = s_mb.mul_add(y0, m.z - s_mb * m.y);
    let neg = (xm - xt) * (yb - yt) - (ym - yt) * (xb - xt) < 0;
    let edge = |a: Vec3, b: Vec3, y: i64, sign: i64| {
        let dy = (b.y - a.y) as i64;
        if dy == 0 {
            return if sign >= 0 { 1 << 20 } else { -(1 << 20) };
        }
        a.x as i64 + ((y - a.y as i64) * (b.x - a.x) as i64).div_euclid(dy)
    };
    for y in yt.max(0)..=yb.min(255) {
        let xl = edge(t, b, y, 0);
        let (x1, x2) = (edge(t, m, y, xm - xt), edge(m, b, y, xm - xb));
        let pick_tm = if neg { x1 > x2 } else { x1 < x2 };
        let (xs, zs) = if pick_tm { (x1, ztm) } else { (x2, zmb) };
        let dzdx = slope(zs - zl, xs as f32 - xl as f32);
        let (lo, hi) = (xl.min(xs).max(0), xl.max(xs).min(255));
        if lo <= hi {
            let xa = lo & !15;
            let base = xa as f32 - xl as f32;
            let mut vals = [0.0f32; 4];
            for (lane, v) in vals.iter_mut().enumerate() {
                *v = (255.0 - dzdx.mul_add(base + lane as f32, zl)).floor();
            }
            let step = dzdx * -4.0;
            let mut x = xa;
            while x <= hi {
                for (lane, v) in vals.iter_mut().enumerate() {
                    let xx = x + lane as i64;
                    if (lo..=hi).contains(&xx) {
                        let val = trunc_sat(*v);
                        if (0..=255).contains(&val) {
                            let cell = &mut raster[y as usize * GRID + xx as usize];
                            *cell = (*cell).max(val as u8);
                        }
                    }
                    *v += step;
                }
                x += 4;
            }
        }
        zl += s_l;
        ztm += s_tm;
        zmb += s_mb;
    }
}

#[test]
fn the_stepped_edges_are_the_divisions() {
    let mut rng = Rng(0x1234_5678_9abc_def1);
    for case in 0..20_000 {
        let span = [8.0, 40.0, 300.0, 3000.0][case % 4];
        let mut v =
            [0; 3].map(|_| Vec3::new(rng.f(-span, 255.0 + span), rng.f(-span, 255.0 + span), rng.f(-60.0, 300.0)));
        if case % 7 == 0 {
            v[1].y = v[0].y;
        }
        let (mut a, mut b) = (vec![0u8; GRID * GRID], vec![0u8; GRID * GRID]);
        scan_triangle(&mut a, v[0], v[1], v[2]);
        scan_divisions(&mut b, v[0], v[1], v[2]);
        assert!(a == b, "case {case}: {v:?}");
    }
}

#[cfg(target_arch = "x86_64")]
#[test]
fn the_sse2_span_is_the_scalar_one() {
    let mut rng = Rng(0x0f0f_1234_5555_aaaa);
    for case in 0..20_000 {
        let lo = (rng.next() % 300) as i64 - 20;
        let hi = lo + (rng.next() % 280) as i64 - 10;
        let (lo, hi) = (lo.max(0), hi.min(255));
        if lo > hi {
            continue;
        }
        let xa = lo & !15;
        let mut vals = [0.0f32; 4];
        for v in &mut vals {
            *v = match rng.next() % 9 {
                0 => f32::NAN,
                1 => rng.f(-1e10, 1e10),
                2 => rng.f(-2.0, 1.0),
                _ => rng.f(-40.0, 300.0).floor(),
            };
        }
        let step = if case % 5 == 0 { f32::NAN } else { rng.f(-3.0, 3.0) };
        let mut base = [0u8; GRID];
        for b in &mut base {
            *b = (rng.next() % 256) as u8;
        }
        let (mut a, mut b) = (base, base);
        span(&mut a, lo, hi, xa, vals, step);
        unsafe { span_sse2(&mut b, lo, hi, xa, vals, step) };
        assert_eq!(a, b, "case {case}: {lo}..={hi} {vals:?} {step}");
    }
}

#[test]
fn the_fused_rasteriser_is_the_plain_one() {
    let mut rng = Rng(0x2545_f491_4f6c_dd1d);
    let n = 60;
    let mut c = CollisionWorld::default();
    for _ in 0..n {
        c.vertices.push(Vec3::new(rng.f(-12.0, 12.0), rng.f(-9.0, 4.0), rng.f(-12.0, 12.0)));
    }
    for _ in 0..200 {
        c.triangles.push([0, 1, 2].map(|_| (rng.next() % n) as u8));
    }
    let cl = crate::collision::Cluster {
        min: Vec3::splat(-12.0),
        max: Vec3::splat(12.0),
        entry_min: Vec3::splat(-12.0),
        entry_max: Vec3::splat(12.0),
        vert_start: 0,
        vert_count: n as u16,
        tri_start: 0,
        tri_count: 200,
        surface_id: 0,
        terrain: true,
        class: 0,
    };
    for _ in 0..8 {
        let r = random_region(&mut rng);
        let m = Mat4::from_translation(r.anchor);
        let to_raster = r.to_raster * m;
        let mut plain = vec![0u8; GRID * GRID];
        let mut scratch = [Vec3::ZERO; CLUSTER_VERTICES];
        rasterize_cluster(&mut plain, &to_raster, &c, &cl, &mut scratch, false);
        let mut corners = vec![0u8; GRID * GRID];
        for t in &c.triangles {
            let v = t.map(|k| raster_vertex(&to_raster, c.vertices[k as usize]));
            rasterize_triangle(&mut corners, v[0], v[1], v[2]);
        }
        assert_eq!(plain, corners);
        assert!(plain.iter().any(|&h| h != 0), "the triangles cover the box");
        if fused_ops() {
            let mut fused = vec![0u8; GRID * GRID];
            rasterize_cluster(&mut fused, &to_raster, &c, &cl, &mut scratch, true);
            assert_eq!(fused, plain);
        }
    }
}
