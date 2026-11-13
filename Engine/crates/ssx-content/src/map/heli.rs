use super::MapDefinition;

pub const HELI_CELL: f32 = 10.0;
pub const HELI_FILL_BELOW: f32 = 20.0;
pub const MAX_HELI_GRID_CELLS: usize = 1 << 18;
// PORT(choice): a port cap of 2²⁴ visits (64 per cell of a full 2¹⁸-cell grid; the template's two-triangle slope visits
// 4 160, a disc track's cage terrain a few per cell); tuning: a measurement of the largest honest map's visits; nothing
// on the disc (the grids were made offline).
pub const MAX_HELI_GRID_VISITS: u64 = 1 << 24;

#[derive(Clone, Debug, PartialEq)]
pub struct HeliGrid {
    pub min: [f32; 3],
    pub max: [f32; 3],
    pub width: usize,
    pub height: usize,
    pub cells: Vec<f32>,
}

struct Extent {
    min: [f32; 3],
    max: [f32; 3],
    width: usize,
    height: usize,
}

fn extent(def: &MapDefinition) -> Result<Extent, String> {
    let mut lo = [f32::INFINITY; 3];
    let mut hi = [f32::NEG_INFINITY; 3];
    for p in def.terrain.iter().flat_map(|m| &m.positions) {
        for c in 0..3 {
            lo[c] = lo[c].min(p[c]);
            hi[c] = hi[c].max(p[c]);
        }
    }
    if !lo[0].is_finite() {
        return Err("the helicopter's height grid is built from the terrain, and the map has none".into());
    }
    let snap = |l: f32, h: f32| {
        let a = (l / HELI_CELL).floor();
        let b = (h / HELI_CELL).ceil().max(a + 1.0);
        (a * HELI_CELL, b * HELI_CELL, (b - a) as u64)
    };
    let (x0, x1, w) = snap(lo[0], hi[0]);
    let (z0, z1, h) = snap(lo[2], hi[2]);
    let cells = w.saturating_mul(h);
    if cells > MAX_HELI_GRID_CELLS as u64 {
        return Err(format!(
            "the terrain spans {w} × {h} cells of {HELI_CELL} m for the helicopter's height grid; at most \
             {MAX_HELI_GRID_CELLS} cells"
        ));
    }
    let (min, max) = ([x0, lo[1] - HELI_FILL_BELOW, z0], [x1, hi[1], z1]);
    let mut visits = 0u64;
    for m in &def.terrain {
        for t in &m.triangles {
            let (a, b) = cell_span(min, w as usize, h as usize, t.map(|v| m.positions[v as usize]));
            visits += ((a.1 - a.0 + 1) * (b.1 - b.0 + 1)) as u64;
        }
    }
    if visits > MAX_HELI_GRID_VISITS {
        return Err(format!(
            "the terrain's triangles cover {visits} cells of the helicopter's height grid between them (stacked large \
             triangles); at most {MAX_HELI_GRID_VISITS}"
        ));
    }
    Ok(Extent { min, max, width: w as usize, height: h as usize })
}

fn cell_span(min: [f32; 3], w: usize, h: usize, t: [[f32; 3]; 3]) -> ((usize, usize), (usize, usize)) {
    let range = |c: usize, origin: f32, n: usize| {
        let lo = t.iter().map(|p| p[c]).fold(f32::INFINITY, f32::min);
        let hi = t.iter().map(|p| p[c]).fold(f32::NEG_INFINITY, f32::max);
        let i = |x: f32| (((x - origin) / HELI_CELL).floor().max(0.0) as usize).min(n - 1);
        (i(lo), i(hi))
    };
    (range(0, min[0], w), range(2, min[2], h))
}

pub fn check(def: &MapDefinition) -> Result<(), String> {
    extent(def).map(|_| ())
}

pub fn heli_grid(def: &MapDefinition) -> Result<HeliGrid, String> {
    // TODO(guess): a new track's grid by the rule read off the disc's HEIG files — each 10 m cell the highest terrain point over its closed square, cells without terrain the lowest vertex − 20 m — which test_plane's grid holds (9 170 of 9 216 cells within 0.25 m) but alps0101's does not (3 726 of 43 379; stored − surface p10 −2.28 / median −0.72 / p90 +33.5 m; neighbours, objects, refinement levels 1 to 3, coarser sampling and the CPU snow displacement tried, none explains the scatter); the generator ran offline and is not on the disc; settle: a rule that reproduces alps0101's cells.
    let Extent { min, max, width, height } = extent(def)?;
    let mut cells = vec![min[1]; width * height];
    for m in &def.terrain {
        for t in &m.triangles {
            let tri = t.map(|v| m.positions[v as usize]);
            let ((x0, x1), (z0, z1)) = cell_span(min, width, height, tri);
            for iz in z0..=z1 {
                for ix in x0..=x1 {
                    let sx = min[0] + ix as f32 * HELI_CELL;
                    let sz = min[2] + iz as f32 * HELI_CELL;
                    if let Some(top) = highest_in_square(tri, [sx, sz], [sx + HELI_CELL, sz + HELI_CELL]) {
                        let c = &mut cells[iz * width + ix];
                        *c = c.max(top);
                    }
                }
            }
        }
    }
    Ok(HeliGrid { min, max, width, height, cells })
}

fn highest_in_square(t: [[f32; 3]; 3], lo: [f32; 2], hi: [f32; 2]) -> Option<f32> {
    let mut poly: Vec<[f64; 3]> = t.iter().map(|p| p.map(f64::from)).collect();
    let planes = [(0, lo[0], true), (0, hi[0], false), (2, lo[1], true), (2, hi[1], false)];
    for (c, bound, above) in planes {
        let bound = f64::from(bound);
        let inside = |p: &[f64; 3]| if above { p[c] >= bound } else { p[c] <= bound };
        let mut out = Vec::with_capacity(poly.len() + 1);
        for k in 0..poly.len() {
            let (a, b) = (poly[k], poly[(k + 1) % poly.len()]);
            if inside(&a) {
                out.push(a);
            }
            if inside(&a) != inside(&b) {
                let s = (bound - a[c]) / (b[c] - a[c]);
                out.push([a[0] + (b[0] - a[0]) * s, a[1] + (b[1] - a[1]) * s, a[2] + (b[2] - a[2]) * s]);
            }
        }
        poly = out;
        if poly.is_empty() {
            return None;
        }
    }
    poly.iter().map(|p| p[1] as f32).reduce(f32::max)
}
