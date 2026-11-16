pub mod audio;
pub mod clouds;
pub mod levels;
pub mod look;
pub mod nodes;
pub mod panel;
pub mod scene;

mod data;
mod drop_panels;
mod scene_link;
mod select;

pub use data::*;
pub use drop_panels::*;
pub use scene_link::*;
pub use select::*;

use ssx_assets::career::DropType;
use ssx_assets::career::profile::DropState;
use ssx_formats::attrib::fnv1;

use super::{FeEnv, FrontEnd, Launch, MovieRequest, screens};
use crate::avm1::Ctx;
use crate::avm1::value::Value;
use crate::game::loc::Localization;
use crate::game::values::UiValues;

pub const TUTORIAL_DROPS: [u32; 2] = [0xC145_699A, 0xB0F3_51A2];
pub const FINAL_DROP: u32 = 0x28F6_34F8;

const DEADLY_DESCENTS: [(u32, &str, &str); 10] = [
    (0x12A3_5AB4, "assets/campaign/image_dd_trees.dds", "TXT_TREES"),
    (0xA0B1_D1B1, "assets/campaign/image_dd_gravity.dds", "TXT_GRAVITY"),
    (0xE0B6_D39E, "assets/campaign/image_dd_ice.dds", "TXT_ICE"),
    (0x8FD3_21EB, "assets/campaign/image_dd_snow.dds", "TXT_AVALANCHE"),
    (0x45D2_75B0, "assets/campaign/image_dd_cold.dds", "TXT_COLD"),
    (0x954D_536D, "assets/campaign/image_dd_thinair.dds", "TXT_THIN_AIR"),
    (0x4A72_8F3A, "assets/campaign/image_dd_rock.dds", "TXT_ROCK"),
    (0x615D_9397, "assets/campaign/image_dd_darkness.dds", "TXT_DARKNESS"),
    (0x65E4_C947, "assets/campaign/image_dd_whiteout.dds", "TXT_WHITEOUT"),
    (0x28F6_34F8, "assets/campaign/image_dd_final.dds", "TXT_SHOWDOWN"),
];

#[derive(Clone, Debug, Default)]
pub struct Globe {
    pub range: u32,
    pub peak: u32,
    pub drop: u32,
    pub input: bool,
    pub provider_node: u32,
    pub provider_is_drop: bool,
    pub provider_selectable: bool,
    pub advance_free: bool,
}

fn world_tour(fe: &FrontEnd) -> bool {
    matches!(fe.screen, 0x42..=0x44)
}

fn mode_events(fe: &FrontEnd) -> Vec<u32> {
    if world_tour(fe) {
        (1..=ssx_assets::career::world_tour::CHAPTERS)
            .flat_map(ssx_assets::career::world_tour::chapter_drops)
            .map(|n| fnv1(n.as_bytes()))
            .collect()
    } else {
        fe.career.events.iter().filter(|e| e.drop_type == DropType::Explore).map(|e| e.key).collect()
    }
}

pub fn default_focus(fe: &mut FrontEnd) {
    let range = child_focus(fe, 0, 0);
    focus_range(fe, range);
}

fn open_drops(fe: &FrontEnd, level: u32, node: u32) -> Vec<u32> {
    let mode = fe.game_mode;
    let mut drops: Vec<(u32, u32, u32, i64)> = fe
        .career
        .events
        .iter()
        .filter(|e| e.drop_type != DropType::None)
        .map(|e| {
            let (peak, range) = fe.career.peak_and_range(e);
            (e.key, peak, range, drop_type_value(e.drop_type))
        })
        .collect();
    drops.sort_unstable();
    drops
        .into_iter()
        .filter(|&(_, peak, range, _)| match level {
            0 => true,
            1 => range == node,
            _ => peak == node,
        })
        .filter(|&(d, _, _, t)| (mode == 4 || t == mode) && (fe.profile.drop_state(d) != DropState::Locked || t == 3))
        .map(|(d, ..)| d)
        .collect()
}

fn drop_type_value(t: DropType) -> i64 {
    match t {
        DropType::Explore => 0,
        DropType::Campaign => 1,
        DropType::Type2 => 2,
        DropType::Custom => 3,
        DropType::None => -1,
    }
}

pub fn child_focus(fe: &mut FrontEnd, level: u32, node: u32) -> u32 {
    if level > 0 && node == 0 {
        return 0;
    }
    if let Some(last) = remembered(fe, level, node) {
        return last;
    }
    let mode = fe.game_mode;
    if mode == 1 {
        let (is_peak, is_range) = (fe.career.peak(node).is_some(), fe.career.range(node).is_some());
        let c = fe.profile.campaign_globe.child(node, is_peak, is_range);
        if c != 0 {
            return c;
        }
    }
    let open = open_drops(fe, level, node);
    let state = |d: u32| fe.profile.drop_state(d);
    let peak_of = |d: u32| fe.career.event(d).map_or(0, |e| fe.career.peak_and_range(e).0);
    let scene = fe.globe_scene.as_ref();
    match level {
        0 => {
            if open.is_empty() {
                return scene.and_then(|s| s.nodes.ranges.keys().next().copied()).unwrap_or(0);
            }
            let mut range = 0;
            for &d in &open {
                range = fe.career.peak(peak_of(d)).map_or(0, |p| p.range);
                if mode != 1 || state(d) != DropState::Completed {
                    break;
                }
            }
            range
        }
        1 => {
            if open.is_empty() {
                return scene.and_then(|s| s.nodes.peaks_of(node).map(|p| p.key).min()).unwrap_or(0);
            }
            let current = scene.map_or(0, |s| s.scene_peak);
            let mut peak = 0;
            for &d in &open {
                let p = peak_of(d);
                if (mode != 1 || state(d) != DropState::Completed) && (peak == 0 || p == current) {
                    peak = p;
                }
            }
            peak
        }
        _ => {
            let current = scene.map_or(0, |s| s.scene_drop);
            let mut drop = open.first().copied().unwrap_or(0);
            for &d in &open {
                let campaign = mode == 1 && fe.career.event(d).is_some_and(|e| e.drop_type == DropType::Campaign);
                let ok = if campaign { state(d) != DropState::Completed } else { state(d) != DropState::Locked };
                if ok && (drop == 0 || current == d) {
                    drop = d;
                }
            }
            drop
        }
    }
}

// TODO(guess): a port choice — each globe mode (Explore, World Tour, Freeride, Global Events) opens on the range, peak
// and drop last looked at in it, kept in the profile's attributes (`PGR0`..`PGD3`: port, globe, range / peak / drop,
// mode) so they last across restarts; not while the World Tour leads the way (its progression or the new game's
// guide); settle: none (the disc opens on its own pick each time).
fn memory_key(level: u32, mode: i64) -> Option<u32> {
    let mode = u8::try_from(mode).ok().filter(|m| *m <= 3)?;
    let what = *b"RPD".get(level as usize)?;
    Some(u32::from_be_bytes([b'P', b'G', what, b'0' + mode]))
}

fn guided(fe: &FrontEnd) -> bool {
    fe.progression.running() || fe.profile.new_game_active()
}

pub(super) fn remember(fe: &mut FrontEnd, level: u32, node: u32) {
    if node == 0 || guided(fe) {
        return;
    }
    if let Some(key) = memory_key(level, fe.game_mode) {
        fe.profile.attribs.insert(key, i64::from(node as i32));
    }
}

fn remembered(fe: &FrontEnd, level: u32, parent: u32) -> Option<u32> {
    use scene::GlobeFlow;
    if guided(fe) {
        return None;
    }
    let last = *fe.profile.attribs.get(&memory_key(level, fe.game_mode)?)? as u32;
    let flow = scene_link::Flow(fe);
    let ok = flow.shown(last)
        && match level {
            0 => fe.career.range(last).is_some() && scene_link::range_valid(fe, last),
            1 => fe.career.peak(last).is_some_and(|p| p.range == parent),
            _ => open_drops(fe, 2, parent).contains(&last),
        };
    ok.then_some(last)
}

pub fn peak_drops(fe: &FrontEnd) -> Vec<u32> {
    mode_events(fe)
        .into_iter()
        .filter(|k| fe.career.event(*k).is_some_and(|e| fe.career.peak_and_range(e).0 == fe.globe.peak))
        .collect()
}

pub fn dpad(fe: &mut FrontEnd, x: f32) {
    if let Some(s) = fe.globe_scene.as_mut() {
        s.dpad = x.clamp(-1.0, 1.0);
    }
}

pub fn construct(fe: &mut FrontEnd) {
    fe.globe.provider_node = 0;
    fe.globe.provider_is_drop = false;
    fe.globe.provider_selectable = false;
}

pub fn provider_update(fe: &mut FrontEnd, env: &mut FeEnv) {
    if fe.game_mode == 2 || !arrived(fe, env) {
        return;
    }
    let entries = fe.globe_scene.as_ref().map(|s| s.entries(STAGE, STAGE.0 / STAGE.1)).unwrap_or_default();
    fe.globe.provider_selectable = false;
    let Some(e) = entries.iter().find(|e| e.selected) else { return };
    let selectable = e.selectable
        && match e.kind {
            1 => range_valid(fe, e.key),
            3 => fe.profile.drop_state(e.key) != DropState::Locked,
            _ => true,
        };
    let changed = e.key != fe.globe.provider_node;
    fe.globe.provider_selectable = selectable;
    fe.globe.provider_node = e.key;
    if !changed {
        return;
    }
    if let Some(level) = match e.kind {
        1 => Some(0),
        2 => Some(1),
        3 => Some(2),
        _ => None,
    } {
        remember(fe, level, e.key);
    }
    fe.globe.provider_is_drop = e.kind == 3;
    fe.note(format!("SelectedGlobeNodeMessage {{{}, {:#010x}}} (the globe's builder)", e.kind, e.key));
    if e.kind == 3 {
        fe.globe.drop = e.key;
    }
    write_panels(fe, env);
}

pub fn activate(fe: &mut FrontEnd, env: &mut FeEnv, _view_model: &str) {
    if fe.screen != screens::id::MAIN_MENU {
        let key = match fe.game_mode {
            1 => Some("TXT_M_GLOBE_CAMPAIGN"),
            2 => Some("TXT_M_GLOBE_FREERIDE"),
            3 => Some("TXT_M_GLOBAL_EVENTS"),
            0 => Some("TXT_M_GLOBE_EXPLORE"),
            _ => None,
        };
        if let Some(key) = key {
            let t = env.loc.text(key);
            env.values.set_str(0x32, t);
        }
    }
    fe.globe.input = true;
    fe.publish.extend(["dp_globe_data".into(), "dp_drop_overlay_data".into(), "dp_drop_metadata".into()]);
}

pub fn activate_modes(fe: &mut FrontEnd, env: &mut FeEnv, _view_model: &str) {
    env.values.set_bool(0xBB, false);
    let _ = fe;
}

pub const DROP_INDICATOR_X: u16 = 0xB5;
pub const DROP_INDICATOR_Y: u16 = 0xB6;
