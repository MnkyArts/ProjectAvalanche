use std::path::Path;

use glam::Vec3;
use ssx_formats::spdt::{SpdtCells, SpdtFile};

#[derive(Clone, Debug)]
pub struct BoundaryGrid {
    pub min: Vec3,
    pub max: Vec3,
    pub cells_per_metre: Vec3,
    pub width: usize,
    pub height: usize,
    pub cells: Vec<u16>,
}

impl BoundaryGrid {
    pub fn from_spdt(f: &SpdtFile) -> Option<Self> {
        let SpdtCells::Boundary(cells) = &f.cells else { return None };
        let (min, max) = (Vec3::from(f.bbox_min), Vec3::from(f.bbox_max));
        let (width, height) = (usize::from(f.width), usize::from(f.height));
        if width == 0 || height == 0 || cells.len() < width * height {
            return None;
        }
        let span = max - min;
        let cells_per_metre = Vec3::new(width as f32 / span.x, 0.0, height as f32 / span.z);
        Some(Self { min, max: max - Vec3::splat(0.01), cells_per_metre, width, height, cells: cells.clone() })
    }

    pub fn load(track_dir: &Path) -> Option<Self> {
        let track = track_dir.file_name()?.to_str()?;
        let name = format!("{track}_track_boundary.spdt");
        let data = match ssx_assets::data_roots::overlay_track_file(track, &name) {
            Some(path) => std::fs::read(path).ok()?,
            None => std::fs::read(track_dir.join(&name)).ok()?,
        };
        Self::from_spdt(&SpdtFile::parse(&data).ok()?)
    }

    pub fn cell(&self, p: Vec3) -> u16 {
        let c = (p.max(self.min).min(self.max) - self.min) * self.cells_per_metre;
        let (x, z) = ((c.x.floor() as usize).min(self.width - 1), (c.z.floor() as usize).min(self.height - 1));
        self.cells[z * self.width + x]
    }
}

pub fn cell_direction(cell: u16) -> Vec3 {
    let a = f32::from((cell >> 8) & 0x3F) * std::f32::consts::TAU / 63.0;
    Vec3::new(a.sin(), 0.0, a.cos())
}

#[derive(Clone, Copy, Debug, Default)]
pub struct BoundaryTuning {
    pub time_pow: f32,
    pub direction_pow: f32,
    pub strength_max: f32,
    pub timer_max: f32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct BoundaryWind {
    pub timer: f32,
    pub wind: Vec3,
    pub out_of_bounds: bool,
    pub force_timer: f32,
    pub force_strength: f32,
    pub force_direction: Vec3,
}

impl BoundaryWind {
    pub fn update(&mut self, t: &BoundaryTuning, grid: Option<&BoundaryGrid>, position: Vec3, velocity: Vec3, dt: f32) {
        let (timer_max, strength_max, d) = if self.force_strength > 0.0 && self.force_timer > 0.0 {
            let f = (self.force_timer, self.force_strength, self.force_direction);
            (self.force_timer, self.force_strength) = (0.0, 0.0);
            self.out_of_bounds = true;
            f
        } else {
            let cell = grid.map(|g| g.cell(position)).filter(|c| c & 0xC000 != 0);
            self.out_of_bounds = cell.is_some();
            let Some(cell) = cell else {
                (self.timer, self.wind) = (0.0, Vec3::ZERO);
                return;
            };
            (t.timer_max, t.strength_max, cell_direction(cell))
        };
        self.timer += dt;
        let x = (self.timer * (1.0 / 60.0) / timer_max).min(1.0);
        let strength = strength_max * x.powf(t.time_pow);
        let v = velocity.normalize_or_zero();
        let along = ((1.0 - v.dot(d)) * 0.5).abs().powf(t.direction_pow);
        self.wind = d * strength * along;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grid() -> BoundaryGrid {
        let k: u16 = 47;
        let f = SpdtFile {
            kind: ssx_formats::spdt::SpdtKind::Boundary,
            type_name: "snowboun".into(),
            version: 1,
            bbox_min: [0.0, 0.0, 0.0],
            bbox_max: [20.0, 100.0, 10.0],
            width: 2,
            height: 1,
            cells: SpdtCells::Boundary(vec![0x0000, 0x4000 | (k << 8)]),
        };
        BoundaryGrid::from_spdt(&f).expect("grid")
    }

    const T: BoundaryTuning = BoundaryTuning { time_pow: 2.0, direction_pow: 2.0, strength_max: 1.0, timer_max: 0.5 };

    #[test]
    fn the_wind_grows_over_half_a_second_against_the_travel() {
        let g = grid();
        let d = cell_direction(g.cell(Vec3::new(15.0, 0.0, 5.0)));
        assert!(d.x < -0.99, "{d:?}");
        let mut b = BoundaryWind::default();
        b.update(&T, Some(&g), Vec3::new(15.0, 0.0, 5.0), Vec3::X * 0.3, 2.0);
        let s1 = (2.0f32 / 30.0).powi(2);
        assert!(b.out_of_bounds && (b.wind.length() - s1 * ((1.0 - Vec3::X.dot(d)) * 0.5).powi(2)).abs() < 1e-6);
        for _ in 0..20 {
            b.update(&T, Some(&g), Vec3::new(15.0, 0.0, 5.0), Vec3::X * 0.3, 2.0);
        }
        assert!(b.timer == 42.0 && (b.wind - d * ((1.0 - Vec3::X.dot(d)) * 0.5).powi(2)).length() < 1e-5);
        b.update(&T, Some(&g), Vec3::new(15.0, 0.0, 5.0), d * 0.3, 2.0);
        assert!(b.wind.length() < 1e-6);
        b.update(&T, Some(&g), Vec3::new(15.0, 0.0, 5.0), Vec3::ZERO, 2.0);
        assert!((b.wind.length() - 0.25).abs() < 1e-6);
        b.update(&T, Some(&g), Vec3::new(-50.0, 0.0, 5.0), Vec3::X, 2.0);
        assert_eq!(b, BoundaryWind::default());
        b.update(&T, None, Vec3::new(15.0, 0.0, 5.0), Vec3::X, 2.0);
        assert_eq!(b, BoundaryWind::default());
    }
}
