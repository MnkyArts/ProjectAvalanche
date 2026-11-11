use crate::sfx::SfxEngine;
use crate::sfx_aems::CsisHandle;
use crate::sfx_game::tuning;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct AvalancheMsg {
    pub position: [f32; 3],
    pub orientation: [f32; 4],
    pub span: [f32; 2],
    pub speed: f32,
    pub mass: f32,
    pub probe_mass: f32,
    pub distance: f32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Emitters {
    pub center: [f32; 3],
    pub corners: [[f32; 3]; 4],
    pub velocity: [f32; 3],
}

fn rotate(q: [f32; 4], v: [f32; 3]) -> [f32; 3] {
    let u = [q[0], q[1], q[2]];
    let cross =
        |a: [f32; 3], b: [f32; 3]| [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]];
    let t = cross(u, v).map(|x| 2.0 * x);
    let c = cross(u, t);
    [v[0] + q[3] * t[0] + c[0], v[1] + q[3] * t[1] + c[1], v[2] + q[3] * t[2] + c[2]]
}

#[derive(Default)]
pub struct AvalancheSounds {
    started: bool,
    instance: Option<CsisHandle>,
    running: bool,
    position: [f32; 3],
    pub emitters: Emitters,
    pub orientation: [f32; 4],
    pub size: i32,
    pub speed: i32,
    pub drop_warning: i32,
}

pub fn ui_drop_warning(big_root: &std::path::Path, ui_drop: u32) -> anyhow::Result<i32> {
    use anyhow::Context as _;
    use ssx_formats::attrib::{Schema, Value, Vault, collection};
    let dir = big_root.join("bootcache/data/xenon/attribdb");
    let read = |stem: &str| -> anyhow::Result<(Vec<u8>, Vec<u8>)> {
        Ok((std::fs::read(dir.join(format!("{stem}.vlt")))?, std::fs::read(dir.join(format!("{stem}.bin")))?))
    };
    let (sv, sb) = read("attribdb_schema")?;
    let schema = Schema::from_vault(&Vault::parse(&sv, &sb)?).context("reading the schema")?;
    let (v, b) = read("attribdb")?;
    let vault = Vault::parse(&v, &b)?;
    let cols = collection::collections(&vault)?;
    let mut decoded = cols.iter().map(|c| c.decode(&vault, &schema)).collect::<ssx_formats::Result<Vec<_>>>()?;
    crate::content::data::events::route_ui_drops(big_root, &cols, &mut decoded);
    let index = collection::index(&cols);
    let class = crate::vault_db::fnv1("ui_drop");
    let i = *index.get(&(class, ui_drop)).with_context(|| format!("no ui_drop {ui_drop:#010x}"))?;
    let w = collection::with_inherited(&cols, &decoded, &index, i)?
        .into_iter()
        .find_map(|(_, f)| match crate::vault_db::single(&f.data) {
            Some(Value::Raw { bytes, .. }) if f.key == 0x9A4C_11BE && bytes.len() == 4 => {
                Some(i32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
            }
            Some(Value::Int32(v)) if f.key == 0x9A4C_11BE => Some(*v),
            _ => None,
        })
        .unwrap_or(0);
    Ok(if (1..=3).contains(&w) { w } else { 0 })
}

fn percent(x: f32, norm: f32) -> i32 {
    let v = x / norm * 100.0;
    (if v - 100.0 < 0.0 { v } else { 100.0 }) as i32
}

impl AvalancheSounds {
    fn start(&mut self, sfx: &mut SfxEngine) {
        sfx.set_global("Avalanche_Size", 0);
        sfx.set_global("Avalanche_Speed", 0);
        (self.size, self.speed, self.started) = (0, 0, true);
    }

    pub fn update(&mut self, sfx: &mut SfxEngine, m: &AvalancheMsg) {
        if !self.started {
            self.start(sfx);
        }
        if self.position == [0.0; 3] || m.position != [0.0; 3] {
            self.position = m.position;
        }
        self.size = percent(m.mass, tuning::AVALANCHE_MASS);
        sfx.set_global("Avalanche_Size", self.size);
        self.speed = percent(m.speed, tuning::AVALANCHE_SPEED);
        sfx.set_global("Avalanche_Speed", self.speed);
        let q = m.orientation;
        self.orientation = q;
        let (x, z) = (rotate(q, [m.span[0], 0.0, 0.0]), rotate(q, [0.0, 0.0, m.span[1]]));
        let c = self.position;
        let at = |sx: f32, sz: f32| [0, 1, 2].map(|i| c[i] + sx * x[i] + sz * z[i]);
        self.emitters = Emitters {
            center: c,
            corners: [at(1.0, 1.0), at(-1.0, 1.0), at(1.0, -1.0), at(-1.0, -1.0)],
            velocity: rotate(q, [m.speed, 0.0, 0.0]),
        };
        if !self.running {
            if self.instance.is_none() {
                self.instance = sfx.call("SFX_Avalanche", &[self.drop_warning.clamp(1, 3)]);
            }
            self.running = true;
        }
    }

    pub fn emitters3d(&self) -> [crate::sfx_dsp::sound3d::Emitter; 5] {
        let e = &self.emitters;
        let buses = [
            "Avalanche_Center_BUS",
            "Avalanche_Front_Left_BUS",
            "Avalanche_Front_Right_BUS",
            "Avalanche_Rear_Left_BUS",
            "Avalanche_Rear_Right_BUS",
        ];
        let pos = [e.center, e.corners[0], e.corners[1], e.corners[2], e.corners[3]];
        let q = self.orientation;
        std::array::from_fn(|i| crate::sfx_dsp::sound3d::Emitter {
            bus: buses[i],
            settings: crate::sfx_dsp::sound3d::AVALANCHE,
            base: pos[i],
            rotation: q,
            velocity: e.velocity,
        })
    }

    pub fn shutdown(&mut self, sfx: &mut SfxEngine) {
        self.running = false;
        sfx.fire_event("Stop_SFX_Avalanche", &[]);
        if let Some(h) = self.instance.take() {
            sfx.stop(h);
        }
        self.started = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn corners_follow_the_orientation_and_the_globals_saturate() {
        let h = std::f32::consts::FRAC_1_SQRT_2;
        let q = [0.0, h, 0.0, h];
        let x = rotate(q, [2.0, 0.0, 0.0]);
        assert!((x[0]).abs() < 1e-6 && (x[2] + 2.0).abs() < 1e-6);
        let z = rotate(q, [0.0, 0.0, 3.0]);
        assert!((z[0] - 3.0).abs() < 1e-6 && z[2].abs() < 1e-6);
        assert_eq!(percent(80.0, tuning::AVALANCHE_MASS), 50);
        assert_eq!(percent(500.0, tuning::AVALANCHE_MASS), 100);
        assert_eq!(percent(0.5, tuning::AVALANCHE_SPEED), 25);
    }
}
