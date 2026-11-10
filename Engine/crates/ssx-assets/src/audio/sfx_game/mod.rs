pub mod audio_objects;
mod rider;
pub mod tuning;

use crate::sfx::SfxEngine;
use crate::sfx_aems::CsisHandle;

pub use rider::*;

#[derive(Clone, Copy, Debug, Default)]
pub struct SurfaceWeights {
    pub coverage: f32,
    pub windswept: f32,
    pub wet: f32,
    pub crusty: f32,
    pub groomed: f32,
    pub crud: f32,
    pub powder: f32,
    pub rock: f32,
    pub ice: f32,
    pub wood: f32,
    pub concrete: f32,
    pub metal: f32,
    pub metal_hollow: f32,
    pub heli: f32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SurfaceSummary {
    pub coverage: f32,
    pub hard: u16,
    pub soft: u16,
}

impl SurfaceSummary {
    pub fn new(w: &SurfaceWeights) -> Self {
        let soft = if w.powder + w.groomed + w.crud + w.crusty + w.wet > w.windswept { 1 } else { 2 };
        let (mut hard, mut best) = (1u16, w.ice);
        for (bit, v) in
            [(2, w.rock), (4, w.wood), (8, w.concrete), (0x10, w.metal), (0x20, w.metal_hollow), (0x40, w.heli)]
        {
            if v > best {
                hard = bit;
                best = v;
            }
        }
        Self { coverage: w.coverage, hard, soft }
    }

    pub fn class(&self, m: i32) -> i32 {
        if m != 0 {
            return m;
        }
        if self.coverage >= tuning::SNOW_COVERAGE {
            return if self.soft & 2 != 0 { 1 } else { 2 };
        }
        let h = self.hard;
        if h & 2 != 0 {
            4
        } else if h & 8 != 0 {
            5
        } else if h & 0x70 != 0 {
            6
        } else if h & 4 != 0 {
            7
        } else {
            3
        }
    }

    pub fn grind_class(&self) -> i32 {
        if self.coverage >= tuning::SNOW_COVERAGE {
            return if self.soft & 2 != 0 { 1 } else { 2 };
        }
        let h = self.hard;
        match () {
            _ if h & 2 != 0 => 4,
            _ if h & 0x10 != 0 => 6,
            _ if h & 0x20 != 0 => 9,
            _ if h & 0x40 != 0 => 10,
            _ if h & 8 != 0 => 5,
            _ => ((i32::from(h) >> 2) & 1) << 2 | 3,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct RideAudio {
    pub paused: bool,
    pub grounded: bool,
    pub wingsuit: bool,
    pub awesome: bool,
    pub super_awesome: bool,
    pub boosting: bool,
    pub goofy: bool,
    pub time_dilating: bool,
    pub time_dilation_state_one: bool,
    pub time_dilation: f32,
    pub crashing: bool,
    pub sliding: bool,
    pub grinding: bool,
    pub heli_grinding: bool,
    pub ice_axe: bool,
    pub tunnel_small: bool,
    pub tunnel_large: bool,
    pub dead_by_death_plane: bool,
    pub heli_target: bool,
    pub big_landing: bool,
    pub speed: f32,
    pub board_velocity_angle: f32,
    pub spray: f32,
    pub lean: f32,
    pub nose_tail: f32,
    pub weights: SurfaceWeights,
    pub ground_material: i32,
    pub collision_material: i32,
    pub boost_percent: f32,
    pub awesome_percent: f32,
    pub stick: [f32; 2],
    pub jumped: bool,
    pub tricking: bool,
    pub spin_speed: f32,
    pub flip_speed: f32,
    pub rewinding: bool,
    pub ms_in_air: i32,
    pub ms_to_land: i32,
    pub health_lost: f32,
    pub descent_percent: f32,
    pub ice_axe_blend: f32,
    pub vignettes: Vignettes,
    pub event_ended_by_death: bool,
    pub avalanche: crate::sfx_avalanche::AvalancheMsg,
    pub board_particle: bool,
    pub nis: u8,
    pub nis_audio_type: i32,
    pub inside_heli: bool,
    pub eog: bool,
    pub deadly_descent: bool,
    pub kind: i32,
    pub trick_update: Option<(i32, i32)>,
}

pub fn event_kind(ui_event_type: u32) -> i32 {
    match ui_event_type {
        5 | 6 => 1,
        7 => 5,
        0..=2 => 2,
        3 => 3,
        4 => 4,
        _ => 6,
    }
}

pub fn kind_mode(kind: i32) -> u8 {
    match kind {
        1 | 5 => 1,
        2 => 2,
        3 | 4 => 3,
        _ => 0,
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Vignettes {
    pub oxygen: f32,
    pub temperature: f32,
    pub health: f32,
}

impl Default for Vignettes {
    fn default() -> Self {
        Self { oxygen: 1.0, temperature: 1.0, health: 1.0 }
    }
}

pub fn flags(r: &RideAudio) -> u32 {
    let bits = [
        (0, r.grounded),
        (1, !r.grounded),
        (3, r.wingsuit),
        (4, r.awesome),
        (5, r.boosting),
        (6, r.goofy),
        (7, r.time_dilating),
        (9, r.time_dilation_state_one),
        (12, r.crashing || r.sliding),
        (13, r.sliding),
        (14, r.grinding),
        (15, r.ice_axe),
        (16, r.tunnel_small),
        (17, r.tunnel_large),
        (18, r.super_awesome),
        (19, r.dead_by_death_plane),
        (20, r.heli_target),
        (21, r.heli_grinding),
        (28, r.big_landing),
    ];
    bits.iter().filter(|b| b.1).fold(0, |f, b| f | 1 << b.0)
}

const fn bit(f: u32, b: u32) -> bool {
    f >> b & 1 != 0
}

fn norm(x: f32, n: f32, scale: f32) -> i32 {
    let v = x / n * scale;
    (if v - scale < 0.0 { v } else { scale }) as i32
}

pub fn material_class(material: i32) -> i32 {
    match material {
        1 => 9,
        2 | 11..=13 => 4,
        3 => 5,
        4 | 5 | 7..=10 | 14 | 15 => 6,
        6 | 16..=19 => 7,
        0x19 => 8,
        _ => 0,
    }
}

fn wingsuit_params(spd: i32, r: &RideAudio) -> [i32; 3] {
    let axis = |v: f32, lo: i32| ((v * 1000.0) as i32).clamp(lo, 1000);
    [spd.clamp(0, 1000), axis(r.stick[0], -1000), axis(r.stick[1], 0)]
}

#[derive(Default)]
pub struct EnvironmentSounds {
    wind: Option<CsisHandle>,
    ambience: Option<CsisHandle>,
    pub ambience_index: i32,
}

impl EnvironmentSounds {
    pub fn new(ambience_index: i32) -> Self {
        Self { ambience_index, ..Self::default() }
    }

    pub fn update(&mut self, sfx: &mut SfxEngine, r: &RideAudio) {
        if self.ambience.is_none() {
            self.ambience = sfx.call_live("SFX_Ambience", &[self.ambience_index]);
        }
        let zone = if r.descent_percent > 66.0 {
            3
        } else if r.descent_percent > 33.0 {
            2
        } else {
            1
        };
        let p = [((r.speed * 20.0).min(1000.0) as i32).max(0), self.ambience_index, i32::from(r.boosting), zone];
        match self.wind {
            Some(h) => sfx.update(h, &p),
            None => self.wind = sfx.call_live("SFX_Wind", &p),
        }
    }

    pub fn shutdown(&mut self, sfx: &mut SfxEngine) {
        stop(sfx, &mut self.wind);
        stop(sfx, &mut self.ambience);
    }
}
