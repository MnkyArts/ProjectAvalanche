use super::{BOUNDARY_CELL, BoundaryArea, MAX_BOUNDARY_CELLS};

pub const MAX_BOUNDARY_SPANS: u64 = 1 << 22;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct BoundaryRaster {
    pub min: [f32; 2],
    pub width: usize,
    pub height: usize,
    pub cells: Vec<u16>,
}

pub const OUT_OF_BOUNDS: u16 = 0x8000;

pub fn layout(b: &BoundaryArea) -> Option<([f64; 2], usize, usize)> {
    let (mut lo, mut hi) = ([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]);
    for p in b.triangles.iter().flatten() {
        for k in 0..2 {
            lo[k] = lo[k].min(f64::from(p[k]));
            hi[k] = hi[k].max(f64::from(p[k]));
        }
    }
    if !(lo[0] <= hi[0] && lo[1] <= hi[1]) {
        return None;
    }
    let cell = f64::from(BOUNDARY_CELL);
    let first = lo.map(|v| (v / cell).floor() - 1.0);
    let size = |k: usize| ((hi[k] / cell).floor() - first[k] + 2.0) as usize;
    Some((first.map(|f| f * cell), size(0), size(1)))
}

fn rows(t: &[[f32; 2]; 3], oz: f64, height: usize) -> (usize, usize) {
    let cell = f64::from(BOUNDARY_CELL);
    let (zmin, zmax) =
        t.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), p| (a.min(f64::from(p[1])), b.max(f64::from(p[1]))));
    let first = ((zmin - oz) / cell - 0.5).ceil().max(0.0) as usize;
    let last = ((zmax - oz) / cell - 0.5).floor();
    if last < 0.0 {
        return (1, 0);
    }
    (first, (last as usize).min(height.saturating_sub(1)))
}

fn span(t: &[[f32; 2]; 3], z: f64) -> Option<(f64, f64)> {
    let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
    for k in 0..3 {
        let (a, b) = (t[k].map(f64::from), t[(k + 1) % 3].map(f64::from));
        if (a[1] < z && b[1] < z) || (a[1] > z && b[1] > z) {
            continue;
        }
        if a[1] == b[1] {
            lo = lo.min(a[0].min(b[0]));
            hi = hi.max(a[0].max(b[0]));
        } else {
            let x = a[0] + (z - a[1]) / (b[1] - a[1]) * (b[0] - a[0]);
            lo = lo.min(x);
            hi = hi.max(x);
        }
    }
    (lo <= hi).then_some((lo, hi))
}

pub fn raster(b: &BoundaryArea) -> Result<BoundaryRaster, String> {
    let Some((origin, width, height)) = layout(b) else { return Err("the boundary has no triangle".into()) };
    let cells = (width as u64).saturating_mul(height as u64);
    if cells > MAX_BOUNDARY_CELLS as u64 {
        return Err(format!("the boundary spans {cells} cells of {BOUNDARY_CELL} m; at most {MAX_BOUNDARY_CELLS}"));
    }
    let spans: u64 = b
        .triangles
        .iter()
        .map(|t| {
            let (first, last) = rows(t, origin[1], height);
            (last + 1).saturating_sub(first) as u64
        })
        .sum();
    if spans > MAX_BOUNDARY_SPANS {
        return Err(format!(
            "the boundary's triangles cross {spans} grid rows in all (cells of {BOUNDARY_CELL} m); at most \
             {MAX_BOUNDARY_SPANS}: fewer, larger triangles"
        ));
    }
    let cell = f64::from(BOUNDARY_CELL);
    let mut diff = vec![0i32; height * (width + 1)];
    for t in &b.triangles {
        let (first, last) = rows(t, origin[1], height);
        for j in first..=last {
            let z = origin[1] + (j as f64 + 0.5) * cell;
            let Some((x0, x1)) = span(t, z) else { continue };
            let i0 = ((x0 - origin[0]) / cell - 0.5).ceil().max(0.0) as usize;
            let i1 = ((x1 - origin[0]) / cell - 0.5).floor();
            if i1 < 0.0 || i0 >= width {
                continue;
            }
            let i1 = (i1 as usize).min(width - 1);
            if i0 <= i1 {
                diff[j * (width + 1) + i0] += 1;
                diff[j * (width + 1) + i1 + 1] -= 1;
            }
        }
    }
    let mut inside = vec![false; width * height];
    for j in 0..height {
        let mut run = 0i32;
        for i in 0..width {
            run += diff[j * (width + 1) + i];
            inside[j * width + i] = run > 0;
        }
    }
    drop(diff);
    let out = directions(&inside, width, height).ok_or_else(|| {
        format!("the boundary covers no cell's centre (cells of {BOUNDARY_CELL} m on a {BOUNDARY_CELL} m grid)")
    })?;
    Ok(BoundaryRaster { min: origin.map(|v| v as f32), width, height, cells: out })
}

fn directions(inside: &[bool], width: usize, height: usize) -> Option<Vec<u16>> {
    if !inside.iter().any(|&b| b) {
        return None;
    }
    // PORT(choice): a map's out cell pushes towards its nearest in cell (Euclidean, cell centres). The game only reads
    // the sector (Boundary_CellCorrectionDirection 0x82CF9D58); the disc's grids are authored: large regions of one
    // direction, matched exactly by this rule in 15.2 % of 221 676 sampled out cells of all 61 `_track_boundary.spdt`
    // (39.2 % within one sector, 11.3 % next to the area); the distance field's gradient and a 5 × 5 average do no
    // better; EA's boundary tool is not on the disc; directions authored per boundary triangle by the map would replace
    // the rule.
    // per column: the nearest inside row of each cell
    let mut col_near = vec![None; width * height];
    let mut f = vec![None; height.max(width)];
    for i in 0..width {
        for j in 0..height {
            f[j] = inside[j * width + i].then_some(0i64);
        }
        for (j, q) in nearest(&f[..height]).into_iter().enumerate() {
            col_near[j * width + i] = q;
        }
    }
    let mut cells = vec![0u16; width * height];
    for j in 0..height {
        for i in 0..width {
            f[i] = col_near[j * width + i].map(|q| (j as i64 - q as i64).pow(2));
        }
        for (i, src) in nearest(&f[..width]).into_iter().enumerate() {
            let k = j * width + i;
            if inside[k] {
                continue;
            }
            let (si, sj) = match src.and_then(|si| Some((si, col_near[j * width + si]?))) {
                Some(s) => s,
                None => continue,
            };
            let (dx, dz) = (si as f64 - i as f64, sj as f64 - j as f64);
            cells[k] = OUT_OF_BOUNDS | (sector(dx, dz) << 8);
        }
    }
    Some(cells)
}

fn nearest(f: &[Option<i64>]) -> Vec<Option<usize>> {
    let mut sites: Vec<(usize, i64)> = Vec::new();
    let mut starts: Vec<Option<(i128, i128)>> = Vec::new();
    let le = |(a, b): (i128, i128), (c, d): (i128, i128)| a * d <= c * b;
    for (q, fq) in f.iter().enumerate() {
        let Some(fq) = *fq else { continue };
        loop {
            let Some(&(p, fp)) = sites.last() else {
                sites.push((q, fq));
                starts.push(None);
                break;
            };
            let (qi, pi) = (q as i128, p as i128);
            let s = ((i128::from(fq) + qi * qi) - (i128::from(fp) + pi * pi), 2 * (qi - pi));
            match starts.last().copied().flatten() {
                Some(z) if le(s, z) => {
                    sites.pop();
                    starts.pop();
                }
                _ => {
                    sites.push((q, fq));
                    starts.push(Some(s));
                    break;
                }
            }
        }
    }
    let mut out = vec![None; f.len()];
    if sites.is_empty() {
        return out;
    }
    let mut k = 0;
    for (x, o) in out.iter_mut().enumerate() {
        while let Some(Some((num, den))) = starts.get(k + 1).copied()
            && num < x as i128 * den
        {
            k += 1;
        }
        *o = Some(sites[k].0);
    }
    out
}

pub fn sector(dx: f64, dz: f64) -> u16 {
    let a = dx.atan2(dz);
    let a = if a < 0.0 { a + std::f64::consts::TAU } else { a };
    ((a * 63.0 / std::f64::consts::TAU).round() as u16) % 63
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square(lo: f32, hi: f32) -> BoundaryArea {
        BoundaryArea {
            triangles: vec![[[lo, lo], [hi, lo], [hi, hi]], [[lo, lo], [hi, hi], [lo, hi]]],
            ..BoundaryArea::default()
        }
    }

    #[test]
    fn a_square_is_inside_and_the_wind_points_back() {
        let g = raster(&square(0.0, 40.0)).expect("a grid");
        assert_eq!((g.min, g.width, g.height), ([-10.0, -10.0], 7, 7));
        let at = |i: usize, j: usize| g.cells[j * g.width + i];
        assert!((1..=4).all(|i| (1..=4).all(|j| at(i, j) == 0)));
        assert_eq!(at(0, 2), OUT_OF_BOUNDS | (16 << 8));
        assert_eq!(at(2, 5), OUT_OF_BOUNDS | (32 << 8));
        assert_eq!(at(2, 6), OUT_OF_BOUNDS | (32 << 8));
        assert_eq!(at(6, 6), OUT_OF_BOUNDS | (39 << 8));
        assert_eq!(at(2, 0), OUT_OF_BOUNDS);
        assert_eq!(g.cells.len(), 49);
    }

    #[test]
    fn hostile_areas_are_refused_before_the_work() {
        let wide = square(-100_000.0, 100_000.0);
        assert!(raster(&wide).unwrap_err().contains("cells of 10 m"));
        let tiny = square(1.0, 2.0);
        assert!(raster(&tiny).unwrap_err().contains("covers no cell's centre"));
        let tall: Vec<[[f32; 2]; 3]> =
            (0..40_000).map(|k| [[k as f32 * 0.05, 0.0], [k as f32 * 0.05 + 0.04, 0.0], [0.0, 5000.0]]).collect();
        let area = BoundaryArea { triangles: tall, ..BoundaryArea::default() };
        assert!(raster(&area).unwrap_err().contains("grid rows in all"));
        assert!(raster(&BoundaryArea::default()).is_err());
    }

    #[test]
    fn the_nearest_inside_cell_is_the_euclidean_one() {
        let f = [Some(9), None, None, None, None, None, None, None, None, None, Some(0)];
        let n = nearest(&f);
        assert_eq!((n[4], n[5]), (Some(0), Some(10)));
        let g = [Some(0), None, Some(0)];
        assert_eq!(nearest(&g), vec![Some(0), Some(0), Some(2)]);
        assert_eq!(nearest(&[None, None]), vec![None, None]);
    }

    #[test]
    fn sectors() {
        assert_eq!(sector(0.0, 1.0), 0);
        assert_eq!(sector(1.0, 0.0), 16);
        assert_eq!(sector(0.0, -1.0), 32);
        assert_eq!(sector(-1.0, 0.0), 47);
        assert_eq!(sector(-1e-9, 1.0), 0);
    }
}
