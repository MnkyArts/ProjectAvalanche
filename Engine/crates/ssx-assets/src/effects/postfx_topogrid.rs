use ssx_formats::attrib::fnv1;
use ssx_formats::vmx;

use crate::postfx_state::{PostFxScene, PostFxState, PostFxValues};

pub const FIELD_ON: u32 = 0x5320_D81A;
pub const FIELD_UNK_DE26D7F4: u32 = 0xDE26_D7F4;
pub const FIELD_SCALE: &str = "topogrid_scale";
pub const FIELD_COLOUR: &str = "topogrid_colour";
pub const FIELD_COLOUR_GRID: &str = "topogrid_colour_grid";
pub const FIELD_DIST: [&str; 4] =
    ["topogrid_dist_start", "topogrid_dist_end", "topogrid_dist_fadein", "topogrid_dist_fadeout"];
pub const FIELD_PULSE_RATE: &str = "topogrid_pulse_rate";
pub const FIELD_PULSE_FALLOFF: &str = "topogrid_pulse_falloff";
pub const FIELD_MIN: &str = "topogrid_min_intensities";
pub const FIELD_MAX: &str = "topogrid_max_intensities";

fn field_key(name: &str) -> u32 {
    fnv1(name.as_bytes())
}

pub fn runs(state: &PostFxState) -> bool {
    state.float(FIELD_ON).is_some_and(|v| v > 0.001)
}

pub fn can_run(scene: &PostFxScene) -> bool {
    std::iter::once(&scene.base).chain(scene.presets.values().map(|(_, s)| s)).any(runs)
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TopogridValues {
    pub on: f32,
    pub scale: f32,
    pub colour: [f32; 4],
    pub colour_grid: [f32; 4],
    pub dist: [f32; 4],
    pub pulse_rate: f32,
    pub pulse_falloff: f32,
    pub unk_de26d7f4: f32,
    pub min: [f32; 2],
    pub max: [f32; 2],
}

impl TopogridValues {
    pub fn of(v: &PostFxValues) -> TopogridValues {
        let float = |n: &str| v.float(field_key(n));
        let v2 = |n: &str| {
            let x = v.get(field_key(n));
            [x[0], x[1]]
        };
        TopogridValues {
            on: v.float(FIELD_ON),
            scale: float(FIELD_SCALE),
            colour: v.get(field_key(FIELD_COLOUR)),
            colour_grid: v.get(field_key(FIELD_COLOUR_GRID)),
            dist: FIELD_DIST.map(float),
            pulse_rate: float(FIELD_PULSE_RATE),
            pulse_falloff: float(FIELD_PULSE_FALLOFF),
            unk_de26d7f4: v.float(FIELD_UNK_DE26D7F4),
            min: v2(FIELD_MIN),
            max: v2(FIELD_MAX),
        }
    }

    pub fn shader_params(&self, pulse: f32, origin: [f32; 3]) -> Option<TopogridParams> {
        if self.on <= 0.001 || self.on.is_nan() {
            return None;
        }
        let one_minus = 1.0 - pulse;
        let pow = |e: f32| f64::from(one_minus).powf(f64::from(e)) as f32;
        let (pf, pg) = (pow(self.pulse_falloff), pow(self.unk_de26d7f4));
        let fade = self.min[0] + (self.max[0] - self.min[0]) * pf;
        let grid = self.min[1] + (self.max[1] - self.min[1]) * pg;
        let [a, b, c, d] = self.dist;
        let (a2, b2, c2, d2) = (vmx::mul(a, a), vmx::mul(b, b), vmx::mul(c, c), -vmx::mul(d, d));
        let (rc, rd) = (vmx::recip(c2), vmx::recip(d2));
        Some(TopogridParams {
            pulsefade: fade,
            colour: [self.colour[0] * fade, self.colour[1] * fade, self.colour[2] * fade, 0.0],
            gridcolour: [self.colour_grid[0] * grid, self.colour_grid[1] * grid, self.colour_grid[2] * grid, 0.0],
            origin,
            scale: self.scale,
            invrange: [vmx::mul(rc, 1.0), vmx::mul(rd, 1.0), vmx::mul(rc, -a2), vmx::mul(rd, -b2)],
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TopogridParams {
    pub pulsefade: f32,
    pub colour: [f32; 4],
    pub gridcolour: [f32; 4],
    pub origin: [f32; 3],
    pub scale: f32,
    pub invrange: [f32; 4],
}

pub const WHITEOUT_CONDITIONS: [&str; 2] = ["whiteout_pre_lighting", "whiteout_lighting"];

#[derive(Clone, Debug, PartialEq)]
pub struct Modification {
    pub field: String,
    pub stat: i32,
    pub x: f32,
    pub y: Option<f32>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GogglesDef {
    pub outer: f32,
    pub threshold: f32,
}

fn stat_fraction(stat: i32) -> f32 {
    let t = (stat & 0xff) as f32 * 0.01;
    let t = if -t < 0.0 { t } else { 0.0 };
    if 1.0 - t < 0.0 { 1.0 } else { t }
}

pub fn apply_modification(holder: &mut PostFxState, m: &Modification) {
    let t = stat_fraction(m.stat);
    // PORT(choice): a modification whose mode does not match the field's type (mode 1 on a vector, mode 2 on a float,
    // a 3- or a 4-vector) changes nothing; the game copies the field's size through an 8-byte stack slot
    // (0x8313D0B8 from 0x82933B90 / 0x82933CE0), reading and writing past it; no disc script does this, a package's
    // script is input.
    match (holder.field_mut(field_key(&m.field)), m.y) {
        (Some((v, 1)), None) => v[0] = t.mul_add(v[0] - m.x, m.x),
        (Some((v, 2)), Some(y)) => (v[0], v[1]) = (t.mul_add(v[0] - m.x, m.x), t.mul_add(v[1] - y, y)),
        _ => {}
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct GogglesTrack {
    pub pulse_rate: f32,
    pub time: f32,
    pub dist_end: f32,
    pub min: [f32; 2],
    pub max: [f32; 2],
    pub holder: PostFxState,
    pub key: u32,
    pub writable: bool,
}

impl GogglesTrack {
    pub fn begin(preset: (u32, PostFxState), mods: &[Modification], copy_key: u32, whiteout: bool) -> GogglesTrack {
        let (key, mut holder) = preset;
        let writable = !mods.is_empty();
        if writable {
            for m in mods {
                apply_modification(&mut holder, m);
            }
        }
        let mut g = GogglesTrack::default();
        let v2 = |n: &str| holder.get(field_key(n)).map(|v| [v[0], v[1]]);
        g.pulse_rate = holder.float(field_key(FIELD_PULSE_RATE)).unwrap_or(g.pulse_rate);
        g.dist_end = holder.float(field_key(FIELD_DIST[1])).unwrap_or(g.dist_end);
        g.min = v2(FIELD_MIN).unwrap_or(g.min);
        g.max = v2(FIELD_MAX).unwrap_or(g.max);
        if !whiteout {
            g.min[0] *= 0.0;
            g.max[0] *= f32::from_bits(0x3E4C_CCCD);
        }
        GogglesTrack { holder, key: if writable { copy_key } else { key }, writable, ..g }
    }

    pub fn update(&mut self, dt: f32, battery: f32, def: GogglesDef) -> f32 {
        self.time += dt;
        let f = self.time * self.pulse_rate;
        let pulse = f - (f as i32) as f32;
        let inner = def.outer * self.dist_end;
        let dist = (self.dist_end - inner).mul_add(battery, inner);
        let s = def.threshold;
        let (lo, hi) = if battery <= s {
            (0.0, if s > 0.0 { battery / s } else { 1.0 })
        } else {
            ((battery - s) / (1.0 - s), 1.0)
        };
        if self.writable {
            if let Some((d, _)) = self.holder.field_mut(field_key(FIELD_DIST[1])) {
                d[0] = dist;
            }
            if let Some((m, _)) = self.holder.field_mut(field_key(FIELD_MIN)) {
                (m[0], m[1]) = (self.min[0] * lo, self.min[1] * lo);
            }
            if let Some((m, _)) = self.holder.field_mut(field_key(FIELD_MAX)) {
                (m[0], m[1]) = (self.max[0] * hi, self.max[1] * hi);
            }
        }
        pulse
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::postfx::{ColourCubeBlend, PostFxPreset};
    use crate::postfx_overlay::OverlayState;
    use crate::postfx_state::{GOGGLES_PRIORITY, PostFxService};

    fn whiteout_1() -> PostFxState {
        let mut p = PostFxPreset::default();
        p.floats.insert(format!("{FIELD_ON:#010x}"), 1.0);
        p.floats.insert(format!("{FIELD_UNK_DE26D7F4:#010x}"), 1.0);
        for (n, v) in [(FIELD_SCALE, 1.0), (FIELD_PULSE_RATE, 0.6), (FIELD_PULSE_FALLOFF, 3.0)] {
            p.floats.insert(n.into(), v);
        }
        for (n, v) in FIELD_DIST.into_iter().zip([0.0, 300.0, 20.0, 80.0]) {
            p.floats.insert(n.into(), v);
        }
        p.vectors.insert(FIELD_COLOUR.into(), vec![0.375, 0.47, 0.5, 0.0]);
        p.vectors.insert(FIELD_COLOUR_GRID.into(), vec![0.05, 0.392, 0.5, 0.0]);
        p.vectors.insert(FIELD_MIN.into(), vec![0.1, 0.5]);
        p.vectors.insert(FIELD_MAX.into(), vec![1.0, 1.0]);
        PostFxState::of(&p)
    }

    fn values(s: &PostFxState) -> TopogridValues {
        let mut v = PostFxValues::default();
        v.hold(s);
        TopogridValues::of(&v)
    }

    #[test]
    fn modifications_lerp_from_the_script_value_by_the_stat() {
        assert_eq!(stat_fraction(50), 0.5);
        assert_eq!(stat_fraction(0x1_00 + 20), stat_fraction(20), "the stat's low byte");
        assert_eq!(stat_fraction(200), 1.0);
        let mut h = whiteout_1();
        let m = |field: &str, x: f32, y: Option<f32>| Modification { field: field.into(), stat: 50, x, y };
        apply_modification(&mut h, &m("topogrid_dist_end", 80.0, None));
        assert_eq!(h.float(field_key(FIELD_DIST[1])), Some(190.0));
        apply_modification(&mut h, &m("topogrid_max_intensity", 0.0, Some(0.0)));
        apply_modification(&mut h, &m(FIELD_MAX, 0.0, None));
        apply_modification(&mut h, &m(FIELD_COLOUR, 0.0, Some(0.0)));
        assert_eq!(h.get(field_key(FIELD_MAX)), Some([1.0, 1.0, 0.0, 0.0]));
        assert_eq!(h.get(field_key(FIELD_COLOUR)), whiteout_1().get(field_key(FIELD_COLOUR)));
        apply_modification(&mut h, &m(FIELD_MIN, 0.0, Some(1.0)));
        assert_eq!(h.get(field_key(FIELD_MIN)), Some([0.05, 0.75, 0.0, 0.0]));
    }

    #[test]
    fn the_goggles_dim_outside_a_whiteout_and_follow_the_battery() {
        let mods = [Modification { field: "topogrid_dist_end".into(), stat: 100, x: 80.0, y: None }];
        let g = GogglesTrack::begin((1, whiteout_1()), &mods, 2, true);
        assert_eq!((g.key, g.dist_end, g.min, g.max), (2, 300.0, [0.1, 0.5], [1.0, 1.0]));
        let mut g = GogglesTrack::begin((1, whiteout_1()), &mods, 2, false);
        assert_eq!((g.min, g.max), ([0.0, 0.5], [0.2, 1.0]));
        let def = GogglesDef { outer: 0.2, threshold: 0.35 };
        let p = g.update(0.5, 1.0, def);
        assert_eq!(p, 0.3);
        let held = |g: &GogglesTrack| {
            let v = values(&g.holder);
            (v.dist[1], v.min, v.max)
        };
        assert_eq!(held(&g), (300.0, [0.0, 0.5], [0.2, 1.0]));
        g.update(0.0, 0.175, def);
        assert_eq!(held(&g), (102.0, [0.0, 0.0], [0.1, 0.5]));
        let mut plain = GogglesTrack::begin((1, whiteout_1()), &[], 2, true);
        plain.update(0.1, 0.0, def);
        assert_eq!((plain.key, plain.holder), (1, whiteout_1()), "the vault preset is not written");
    }

    #[test]
    fn the_constants_of_whiteout_1() {
        let mut v = values(&whiteout_1());
        let p = v.shader_params(0.0, [1.0, 2.0, 3.0]).unwrap();
        assert_eq!(p.pulsefade, 1.0);
        assert_eq!(p.colour, [0.375, 0.47, 0.5, 0.0]);
        assert_eq!(p.invrange[..3], [1.0 / 400.0, -1.0 / 6400.0, -0.0]);
        assert!((p.invrange[3] - 14.0625).abs() < 1e-5);
        v.on = 0.001;
        assert_eq!(v.shader_params(0.0, [0.0; 3]), None);
    }

    #[test]
    fn the_frame_blends_default_then_the_goggles_and_holds_what_no_layer_blends() {
        let mut base = PostFxState::default();
        base.fields[crate::postfx_state::field_index(FIELD_ON).unwrap()] = Some([0.0; 4]);
        let mut scene = PostFxScene { base, ..PostFxScene::default() };
        assert!(!can_run(&scene));
        let g = whiteout_1();
        let mut fx = PostFxService::default();
        let handle = fx.request_state(7, g.clone(), GOGGLES_PRIORITY, 0.0, 0.5);
        let v = TopogridValues::of(scene.frame(&fx));
        assert_eq!(v.on, (1.0f32 - 0.0).mul_add(0.5, 0.0));
        assert_eq!((v.scale, v.dist), (1.0, [0.0, 300.0, 20.0, 80.0]), "absent in default: copied at any weight");
        fx.release(handle, 0.0);
        let v = TopogridValues::of(scene.frame(&fx));
        assert_eq!((v.on, v.scale, v.dist[1]), (0.0, 1.0, 300.0));
        assert_eq!(v.shader_params(0.0, [0.0; 3]), None);
        scene.presets.insert("night".into(), (Some(GOGGLES_PRIORITY), g));
        assert!(can_run(&scene));
    }

    #[test]
    fn a_goggles_preset_blends_every_field_through_the_layer() {
        let mut p = PostFxPreset::default();
        p.floats.insert(format!("{FIELD_ON:#010x}"), 1.0);
        p.floats.insert(FIELD_DIST[1].into(), 250.0);
        p.floats.insert("overlay_1_intensity".into(), 0.5);
        p.colour_cubes.push(ColourCubeBlend { stack: 1, mode: 0, group: "night_vision".into(), weight: 1.0 });
        let mut d = PostFxPreset::default();
        d.floats.insert(format!("{FIELD_ON:#010x}"), 0.0);
        d.floats.insert("overlay_1_intensity".into(), 0.0);
        d.vectors.insert("overlay_1_tintcolour".into(), vec![0.25, 0.5, 0.75, 0.0]);
        let mut scene = PostFxScene { base: PostFxState::of(&d), ..PostFxScene::default() };
        let g = GogglesTrack::begin((7, PostFxState::of(&p)), &[], 1, true);
        let mut fx = PostFxService::default();
        let handle = fx.request_state(g.key, g.holder.clone(), GOGGLES_PRIORITY, 1.0, 1.0);
        fx.update(0.5);
        let v = scene.frame(&fx).clone();
        let overlay = OverlayState::of(&v).shader_params().expect("the goggles' overlay field is drawn");
        assert_eq!(overlay.tint_intensity[0], [0.25, 0.5, 0.75, 1.0 / (1.0 - 0.25)]);
        let topogrid = TopogridValues::of(&v);
        assert_eq!((topogrid.on, topogrid.dist[1]), (0.5, 250.0));
        assert!(topogrid.shader_params(0.0, [0.0; 3]).is_some());
        assert_eq!(v.colour_cubes.len(), 1);
        assert_eq!((v.colour_cubes[0].group.as_str(), v.colour_cubes[0].weight), ("night_vision", 0.5));
        fx.release(handle, 0.0);
        let v = scene.frame(&fx).clone();
        assert_eq!(OverlayState::of(&v).shader_params(), None);
        assert_eq!(TopogridValues::of(&v).shader_params(0.0, [0.0; 3]), None);
        assert!(v.colour_cubes.is_empty());
    }
}
