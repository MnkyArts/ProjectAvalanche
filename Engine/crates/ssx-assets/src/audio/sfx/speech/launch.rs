use std::collections::HashMap;
use std::path::Path;

use anyhow::Result;

use super::vars::Vars;
use crate::career::Profile;
use crate::career::profile::DropState;

pub const NO_GEAR: u32 = 0x0E38_17CA;
pub const GEAR_TYPES: usize = 15;
const DD_GEAR: [(u32, usize); 7] = [
    (0xC1CA_0EE8, 1),
    (0x1714_DF79, 6),
    (0x2778_5BD2, 3),
    (0x03A4_C210, 2),
    (0xDA2E_FD95, 5),
    (0x7A6D_221D, 7),
    (0x28BE_9486, 4),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CampaignWords {
    pub chapter: i32,
    pub chapters: i32,
    pub drops: i32,
    pub drop: i32,
    pub dd: bool,
    pub before_dd: bool,
    pub completed: bool,
    pub region_intro: bool,
    pub completions: i32,
    pub failures: i32,
}

impl Default for CampaignWords {
    fn default() -> Self {
        CampaignWords {
            chapter: 0,
            chapters: 11,
            drops: -1,
            drop: 0,
            dd: false,
            before_dd: false,
            completed: false,
            region_intro: false,
            completions: 0,
            failures: 0,
        }
    }
}

pub fn campaign_words(profile: &Profile, drop: u32, record_dd: bool) -> CampaignWords {
    use crate::career::world_tour::{CHAPTERS, chapter_drops};
    use crate::vault_db::fnv1;
    let lists: Vec<Vec<u32>> = (1..=CHAPTERS).map(|c| chapter_drops(c).iter().map(|n| fnv1(n)).collect()).collect();
    let mut found = None;
    for (c, list) in lists.iter().enumerate() {
        for (i, &key) in list.iter().enumerate() {
            if key == drop {
                found = Some((c, i));
            }
        }
    }
    let mut w = CampaignWords::default();
    let Some((c, i)) = found else { return w };
    let list = &lists[c];
    let count = list.len();
    let state = |k: usize| profile.drop_state(list[k]);
    w.dd = record_dd || (c == 0 && i == 0);
    if !w.dd {
        let last = state(count - 1);
        if last != DropState::Unlocked && last != DropState::Completed {
            w.before_dd = (0..count - 1).all(|k| k == i || state(k) == DropState::Completed);
        }
    }
    w.completed = state(i) == DropState::Completed;
    w.region_intro = state(0) == DropState::Completed && (1..count).all(|k| state(k) != DropState::Completed);
    let counts = profile.drop_counts.get(&drop).copied().unwrap_or_default();
    (w.completions, w.failures) = (counts.completions as i32, counts.failures as i32);
    (w.chapter, w.drops, w.drop) = (c as i32 + 1, count as i32, i as i32 + 1);
    w
}

pub fn required_gear(required: &[i64], forced_len: usize) -> [bool; 7] {
    let mut r = [false; 7];
    for &v in required.iter().take(forced_len) {
        if let Some(b) = usize::try_from(v).ok().and_then(|v| r.get_mut(v)) {
            *b = true;
        }
    }
    r
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LaunchBlock {
    pub campaign: CampaignWords,
    pub required: [bool; 7],
    pub gear: [u32; GEAR_TYPES],
    pub dd_hold: bool,
}

impl LaunchBlock {
    pub fn wingsuit_audio(&self) -> bool {
        self.gear[1] != 0
    }

    pub fn new(
        campaign: CampaignWords,
        required: [bool; 7],
        gear: [u32; GEAR_TYPES],
        rider: u32,
        peak_dd: Option<(u32, u32)>,
    ) -> Self {
        let mut hold = campaign.chapter >= 1 && campaign.dd;
        if hold && let Some((dd_rider, dd_gear)) = peak_dd {
            if dd_rider != rider {
                hold = false;
            }
            if dd_gear != NO_GEAR
                && let Some(&(_, t)) = DD_GEAR.iter().find(|(k, _)| *k == dd_gear)
                && gear[t] == 0
            {
                hold = false;
            }
        }
        LaunchBlock { campaign, required, gear, dd_hold: hold }
    }

    pub fn apply(&self, v: &mut Vars) {
        let w = &self.campaign;
        v.set(super::vars::id::IS_DEADLY_DESCENT, i32::from(w.dd));
        for (var, value) in [
            (300, w.chapter),
            (301, w.chapters),
            (302, w.drop),
            (303, w.drops),
            (304, i32::from(w.before_dd)),
            (305, i32::from(w.completed)),
            (309, i32::from(w.drop == 1)),
            (306, i32::from(w.region_intro)),
            (307, w.completions),
            (308, w.failures),
        ] {
            v.set(var, value);
        }
        let r = &self.required;
        let g = |t: usize| i32::from(self.gear[t] != 0);
        for (var, value) in [
            (0x52, g(1)),
            (0x51, i32::from(r[0])),
            (0x54, g(2)),
            (0x53, i32::from(r[1])),
            (0x56, g(6)),
            (0x55, i32::from(r[5])),
            (0x3A, g(7)),
            (0x39, i32::from(r[6])),
            (0x38, g(5)),
            (0x37, i32::from(r[2])),
            (0x35, g(4)),
            (0x36, i32::from(r[3])),
        ] {
            v.set(var, value);
        }
    }
}

pub fn gear_audio_infos(big_root: &Path) -> Result<HashMap<u32, u32>> {
    use crate::vault_db::{fnv1, single};
    use ssx_formats::attrib::Value;
    let class = fnv1("equipment_description");
    crate::vault_db::with_all_db(big_root, |db| {
        let mut out = HashMap::new();
        for (key, _) in db.collections_of_all(class) {
            let info = db.fields(class, key)?.into_iter().find_map(|f| match single(&f.data) {
                Some(Value::Ref(r)) if f.key == 0xB97D_9FD5 => Some(r.collection),
                _ => None,
            });
            out.entry(key).or_insert(info.unwrap_or(0));
        }
        Ok(out)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vault_db::fnv1;

    fn profile(states: &[(&str, DropState)]) -> Profile {
        let mut p = Profile::default();
        for &(n, s) in states {
            p.drop_states.insert(fnv1(n), s);
        }
        p
    }

    #[test]
    fn the_words_place_the_drop_in_its_chapter() {
        let p = profile(&[]);
        let w = campaign_words(&p, fnv1("chapter_1_drop_h2h"), false);
        assert_eq!((w.chapter, w.chapters, w.drops, w.drop, w.dd), (1, 11, 1, 1, true));
        let w = campaign_words(&p, fnv1("chapter_2_drop_gen_2"), false);
        assert_eq!((w.chapter, w.drops, w.drop, w.dd), (2, 6, 3, false));
        assert_eq!(campaign_words(&p, fnv1("exp_alps_vorassay_drop_3"), true), CampaignWords::default());
    }

    #[test]
    fn before_the_deadly_descent_and_the_region_intro() {
        use DropState::Completed;
        let done = [
            ("chapter_2_drop_h2h", Completed),
            ("chapter_2_drop_gen_1", Completed),
            ("chapter_2_drop_gen_2", Completed),
            ("chapter_2_drop_gen_3", Completed),
        ];
        let p = profile(&done);
        let w = campaign_words(&p, fnv1("chapter_2_drop_gen_4"), false);
        assert!(w.before_dd && !w.completed && !w.region_intro);
        let w = campaign_words(&p, fnv1("chapter_2_drop_gen_3"), false);
        assert!(!w.before_dd && w.completed, "gen_4 is still open");
        let p = profile(&done[..1]);
        let w = campaign_words(&p, fnv1("chapter_2_drop_gen_1"), false);
        assert!(w.region_intro && !w.before_dd);
        let mut p = profile(&done);
        p.drop_states.insert(fnv1("chapter_2_drop_dd"), DropState::Unlocked);
        assert!(!campaign_words(&p, fnv1("chapter_2_drop_gen_4"), false).before_dd, "the Deadly Descent is open");
    }

    #[test]
    fn the_hold_needs_the_regions_rider_and_gear() {
        let dd = CampaignWords { chapter: 6, dd: true, ..CampaignWords::default() };
        let (elise, zoe) = (fnv1("elise"), fnv1("zoe"));
        let mut gear = [0u32; GEAR_TYPES];
        let peak = Some((elise, 0xC1CA_0EE8));
        assert!(!LaunchBlock::new(dd, [false; 7], gear, elise, peak).dd_hold, "no wingsuit");
        gear[1] = 0xC1CA_0EE8;
        assert!(LaunchBlock::new(dd, [false; 7], gear, elise, peak).dd_hold);
        assert!(!LaunchBlock::new(dd, [false; 7], gear, zoe, peak).dd_hold);
        assert!(LaunchBlock::new(dd, [false; 7], [0; GEAR_TYPES], zoe, Some((zoe, NO_GEAR))).dd_hold);
        assert!(LaunchBlock::new(dd, [false; 7], gear, zoe, None).dd_hold, "no peak info: nothing clears it");
        let plain = CampaignWords { dd: false, ..dd };
        assert!(!LaunchBlock::new(plain, [false; 7], gear, elise, peak).dd_hold);
        assert_eq!(required_gear(&[0, 5, 9], 2), [true, false, false, false, false, true, false]);
    }
}
