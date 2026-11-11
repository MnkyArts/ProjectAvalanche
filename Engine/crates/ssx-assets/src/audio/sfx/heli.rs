use super::SfxEngine;
use crate::sfx_aems::CsisHandle;
use crate::sfx_dsp::sound3d::{Emitter, HELICOPTER, HELICOPTER_EMITTERS};

pub mod tuning {
    pub const ROTOR_BASE: i32 = 450;
    pub const TD_START: f32 = 0.95;
    pub const TD_STOP: f32 = 0.85;
    pub const NIS_HOLD: f32 = 1.2;
    pub const AHEAD: f32 = 300.0;
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct HeliTransform {
    pub position: [f32; 3],
    pub rotation: [f32; 4],
    pub velocity: [f32; 3],
    pub speed_kmh: f32,
}

impl HeliTransform {
    pub fn new(position: [f32; 3], rotation: [f32; 4], velocity: [f32; 3]) -> Self {
        let v2 = velocity.iter().map(|x| x * x).sum::<f32>();
        Self { position, rotation, velocity, speed_kmh: v2.sqrt() * 216.0 }
    }
}

#[derive(Default)]
pub struct HeliSounds {
    started: bool,
    instance: Option<CsisHandle>,
    nis_instance: Option<CsisHandle>,
    nis: bool,
    nis_release: Option<f32>,
    dilation: bool,
    pub transform: Option<HeliTransform>,
    pub params: [i32; 8],
    pub speech_on: bool,
}

fn clamp_params(p: [i32; 8]) -> [i32; 8] {
    const RANGES: [(i32, i32); 8] = [(0, 1000), (-1, 1), (0, 1), (0, 2), (0, 1000), (0, 1000), (-100, 100), (0, 1)];
    std::array::from_fn(|i| p[i].clamp(RANGES[i].0, RANGES[i].1))
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn length(a: [f32; 3]) -> f32 {
    a.iter().map(|x| x * x).sum::<f32>().sqrt()
}

pub fn kind_param(kind: i32) -> i32 {
    match kind {
        1 | 5 => 2,
        3 | 4 => 3,
        _ => 0,
    }
}

impl HeliSounds {
    fn start(&mut self, sfx: &mut SfxEngine) {
        sfx.set_global("Chopper_Speech_On", 0);
        (self.dilation, self.speech_on, self.started) = (false, false, true);
    }

    pub fn nis_state(&mut self, sfx: &mut SfxEngine, state: u8, now_ms: f32) {
        if !self.started {
            self.start(sfx);
        }
        match state {
            1 | 2 if !self.nis => {
                self.nis = true;
                self.nis_instance = sfx.call("Chopper_Int_NIS", &[]);
            }
            3 if self.nis => {
                if let Some(h) = self.nis_instance.take() {
                    sfx.stop(h);
                }
                self.nis_release = Some(tuning::NIS_HOLD * 1000.0 + now_ms);
            }
            _ => {}
        }
    }

    pub fn frame_flags(&mut self, time_dilating: bool, time_dilation: f32) {
        if !self.dilation {
            if time_dilating && time_dilation < tuning::TD_START {
                self.dilation = true;
            }
        } else if time_dilation > tuning::TD_STOP || !time_dilating {
            self.dilation = false;
        }
    }

    pub fn transform(
        &mut self,
        sfx: &mut SfxEngine,
        m: &HeliTransform,
        listener: ([f32; 3], [f32; 3]),
        kind: i32,
        now_ms: f32,
    ) {
        if !self.started {
            self.start(sfx);
        }
        self.transform = Some(*m);
        let base = tuning::ROTOR_BASE;
        let rotor = ((1000 - base) as f32 * m.speed_kmh).mul_add(0.002_857_142_8, base as f32);
        let rotor = (if rotor - 1000.0 < 0.0 { rotor } else { 1000.0 }) as i32;
        let moving = i32::from(m.speed_kmh >= 1.0);
        let (eye, look) = listener;
        let feet = (length(sub(m.position, eye)) * 3.280_84) as i32;
        let ahead = [0, 1, 2].map(|i| tuning::AHEAD.mul_add(look[i], eye[i]));
        let far = length(sub(ahead, m.position)).min(1000.0) as i32;
        let d = sub(m.position, eye);
        let n = length(d);
        let cos = if n > 0.0 { (d[0] * look[0] + d[1] * look[1] + d[2] * look[2]) / n } else { 0.0 };
        let facing = (cos * 100.0) as i32;
        if self.nis_release.is_some_and(|t| t <= now_ms) {
            (self.nis_release, self.nis) = (None, false);
        }
        let p = clamp_params([
            rotor,
            moving,
            i32::from(self.nis),
            kind_param(kind),
            feet,
            far,
            facing,
            i32::from(self.dilation),
        ]);
        match self.instance {
            Some(h) => sfx.update(h, &p),
            None => self.instance = sfx.call("SFX_Helicopter", &p),
        }
        self.params = p;
    }

    pub fn update(&mut self, sfx: &mut SfxEngine, pilot_speaking: bool) {
        if !self.started {
            self.start(sfx);
        }
        if pilot_speaking != self.speech_on {
            self.speech_on = pilot_speaking;
            sfx.set_global("Chopper_Speech_On", i32::from(pilot_speaking));
        }
    }

    pub fn emitters(&self) -> Vec<Emitter> {
        let Some(t) = self.transform else { return Vec::new() };
        HELICOPTER_EMITTERS
            .iter()
            .map(|&(bus, offset)| Emitter {
                bus,
                settings: crate::sfx_dsp::sound3d::EmitterSettings { offset, ..HELICOPTER },
                base: t.position,
                rotation: t.rotation,
                velocity: t.velocity,
            })
            .collect()
    }

    pub fn shutdown(&mut self, sfx: &mut SfxEngine) {
        sfx.set_global("Chopper_Speech_On", 0);
        for h in [self.instance.take(), self.nis_instance.take()].into_iter().flatten() {
            sfx.stop(h);
        }
        *self = Self::default();
    }
}

pub fn helicopter_bank(big_root: &std::path::Path, helicopter: u32) -> anyhow::Result<String> {
    use anyhow::Context as _;
    let descr = super::speech::helicopter_text(big_root, helicopter, 0x8C6B_24ED)?;
    let all = super::audio_assets(big_root, "SfxHelicopterBank")?;
    all.into_iter()
        .find(|(d, _)| *d == descr)
        .map(|(_, n)| n)
        .with_context(|| format!("no SfxHelicopterBank \"{descr}\""))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parameters_clamp_and_kinds_map() {
        assert_eq!(clamp_params([2000, 5, -1, 3, -4, 1001, -300, 7]), [1000, 1, 0, 2, 0, 1000, -100, 1]);
        assert_eq!([1, 5, 2, 3, 4, 6].map(kind_param), [2, 2, 0, 3, 3, 0]);
        assert!((HeliTransform::new([0.0; 3], [0.0, 0.0, 0.0, 1.0], [0.3, 0.0, 0.4]).speed_kmh - 108.0).abs() < 1e-3);
    }

    #[test]
    fn slow_motion_latch_has_hysteresis() {
        let mut h = HeliSounds::default();
        h.frame_flags(true, 0.96);
        assert!(!h.dilation);
        h.frame_flags(true, 0.9);
        assert!(h.dilation);
        h.frame_flags(true, 0.86);
        assert!(!h.dilation, "above 0.85 the latch clears");
        h.frame_flags(true, 0.5);
        h.frame_flags(false, 0.5);
        assert!(!h.dilation);
    }
}
