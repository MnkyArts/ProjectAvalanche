use super::*;

#[derive(Clone, Copy, Debug, Default)]
pub struct SnapshotInputs {
    pub mode: u8,
    pub flags: u32,
    pub time_dilation: f32,
    pub rewinding: bool,
    pub big_air_times: Option<(i32, i32)>,
    pub avalanche_mass: f32,
    pub avalanche_distance: f32,
    pub reverb: bool,
    pub nis: u8,
    pub nis_audio_type: i32,
    pub inside_heli: bool,
    pub eog: bool,
    pub deadly_descent: bool,
    pub kind: i32,
    pub front_end: bool,
    pub wingsuit_audio: bool,
    pub script_song: bool,
    pub tricky_clip: bool,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct SnapshotDirector {
    landing: i32,
    big_air: bool,
}

impl SnapshotDirector {
    pub fn frame(&mut self, g: &mut Graph, s: &SnapshotInputs) {
        let bit = |b: u32| s.flags >> b & 1 != 0;
        let on = |c: bool| u8::from(c);
        let [_, layer1, layer2, reverb, _] = &mut g.selectors;
        let room = if !s.reverb {
            0
        } else if bit(17) {
            3
        } else if bit(16) {
            2
        } else {
            1
        };
        reverb.set(room, 2);
        let nis = s.nis != 0;
        if !s.front_end {
            for (k, m) in [(1, 1u8), (2, 2), (3, 3)] {
                layer1.set(k, on(!nis && s.mode == m));
                layer2.set(k, on(!nis && s.mode == m));
            }
        }
        layer1.set(4, on(s.deadly_descent));
        if nis {
            let (plain, dd, eog) = match (s.eog, s.deadly_descent) {
                (true, _) => (0, 0, 1),
                (false, true) => (0, 1, 0),
                (false, false) => (1, 0, 0),
            };
            layer1.set(8, plain);
            layer1.set(9, dd);
            layer1.set(10, eog);
            layer1.set(11, on(s.inside_heli));
        } else {
            layer1.set(8, 0);
            layer1.set(9, 0);
            layer1.set(11, 0);
        }
        layer2.set(23, on(nis && s.nis_audio_type != 0));
        layer2.set(24, on(nis && s.nis_audio_type != 0 && s.kind == 4));
        if !nis {
            layer2.set(25, 0);
        } else if s.nis == 2 {
            layer2.set(25, 1);
        }
        let td = if layer2.is_set(15) { crate::sfx_game::tuning::TD_STOP } else { crate::sfx_game::tuning::TD_START };
        layer2.set(15, on(bit(7) && s.time_dilation < td));
        layer2.set(4, on((bit(0) && bit(14)) || bit(21)));
        layer2.set(5, on(bit(5)));
        layer2.set(6, on(bit(4)));
        layer2.set(7, on(bit(5) && bit(4)));
        // PORT(choice): for music mods — with a client script's song the Super Tricky mixes (8 / 9, the music pulled
        // under "It's Tricky") hold only while the clip plays, so the mod's song comes back for the rest of Super
        // Tricky (PR #53); the game's 0x830AA320 sets them from bit 18 for its whole length and ships no script songs
        // (the mod music contract).
        let tricky = bit(18) && (!s.script_song || s.tricky_clip);
        layer2.set(8, on(tricky));
        layer2.set(9, on(tricky && bit(5)));
        layer2.set(11, on(bit(3) && s.wingsuit_audio));
        use crate::sfx_game::tuning;
        if bit(28) {
            self.landing = (tuning::BIG_LANDING_HOLD * 30.0) as i32;
        } else if layer2.is_set(21) && self.landing > 0 {
            self.landing -= 1;
        } else {
            self.landing = 0;
        }
        layer2.set(21, on(self.landing > 0));
        let mut big_air = false;
        let (ms_in_air, ms_to_land) = s.big_air_times.unwrap_or((-1, -1));
        if !bit(19) && !s.rewinding {
            if ms_in_air > tuning::BIG_AIR_SNAP_MIN_AIR && ms_to_land > tuning::BIG_AIR_SNAP_START {
                (big_air, self.big_air) = (true, true);
            }
            if self.big_air {
                big_air = !(ms_to_land < tuning::BIG_AIR_SNAP_STOP || bit(0));
                self.big_air = big_air;
            }
        }
        layer2.set(10, on(big_air));
        for (k, m) in [16, 17, 18].into_iter().zip(tuning::AVALANCHE_SNAP_MASS) {
            layer2.set(k, on(s.avalanche_distance < tuning::AVALANCHE_SNAP_DISTANCE && s.avalanche_mass > m));
        }
        layer2.set(22, on(s.rewinding));
    }
}
