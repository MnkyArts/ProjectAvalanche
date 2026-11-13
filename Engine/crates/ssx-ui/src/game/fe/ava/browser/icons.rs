use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use ssx_net_join::browse::refresh::{Ask, Refresher};
use ssx_net_join::browse::{Entry, State};
use ssx_net_proto::icon::{ICON_SIDE, Icon};

use crate::assets::image::ImageData;
use crate::avm1::Ctx;

// PORT(choice): 64 icons (a session browses tens of servers; 64 × 36 KiB of pixels); tuning: the browser over real
// lists (how many servers a player selects in a run); the original has no server browser.
pub const MAX_ICONS: usize = 64;
const ICON_DIR: &str = "ava_server_icons/";

#[derive(Clone, Debug)]
enum Slot {
    Asked,
    Ready(Arc<Icon>),
    Failed,
}

#[derive(Default)]
pub struct Icons {
    slots: HashMap<[u8; 32], Slot>,
}

impl Icons {
    pub fn ask(&mut self, refresher: &mut Refresher, entry: &Entry, now: Instant) -> bool {
        let Some(hash) = hash_of(entry) else { return false };
        if self.slots.contains_key(&hash) || self.slots.len() >= MAX_ICONS {
            return false;
        }
        let asked = refresher.ask(&entry.address, Ask::Icon(hash), Duration::ZERO, now);
        if asked {
            self.slots.insert(hash, Slot::Asked);
        }
        asked
    }

    pub fn answered(&mut self, hash: [u8; 32], result: Result<Icon, String>) -> String {
        let hex = ssx_net_proto::keys::to_hex(&hash);
        match result {
            Ok(icon) => {
                self.slots.insert(hash, Slot::Ready(Arc::new(icon)));
                format!("browser: the icon {} read", &hex[..12])
            }
            Err(why) => {
                self.slots.insert(hash, Slot::Failed);
                format!("browser: the icon {} did not read: {}", &hex[..12], ssx_core::text::shown_max(&why, 200))
            }
        }
    }

    pub fn forget_asked(&mut self) {
        self.slots.retain(|_, s| !matches!(s, Slot::Asked));
    }

    pub fn path(&self, entry: &Entry, cx: &mut Ctx) -> String {
        let Some(hash) = hash_of(entry) else { return String::new() };
        let Some(Slot::Ready(icon)) = self.slots.get(&hash) else { return String::new() };
        let path = format!("{ICON_DIR}{}.png", ssx_net_proto::keys::to_hex(&hash));
        let image = ImageData { width: ICON_SIDE, height: ICON_SIDE, rgba: icon.rgba.clone(), source: path.clone() };
        match cx.lib.add_image_movie(&path, image) {
            Ok(_) => path,
            Err(_) => String::new(),
        }
    }
}

fn hash_of(entry: &Entry) -> Option<[u8; 32]> {
    match &entry.state {
        State::Info(info) => ssx_net_proto::info::icon_hash(&info.icon).ok().flatten(),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_answer_is_kept_by_its_hash_and_a_failure_is_not_asked_again() {
        let mut icons = Icons::default();
        let note = icons.answered([7; 32], Err("not a PNG\u{1b}[31m".into()));
        assert!(note.contains("did not read") && !note.contains('\u{1b}'), "{note}");
        assert!(matches!(icons.slots.get(&[7; 32]), Some(Slot::Failed)));
        icons.slots.insert([8; 32], Slot::Asked);
        icons.forget_asked();
        assert!(!icons.slots.contains_key(&[8; 32]) && icons.slots.contains_key(&[7; 32]));
    }
}
