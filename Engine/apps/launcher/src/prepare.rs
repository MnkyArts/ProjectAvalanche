use std::path::Path;
use std::sync::atomic::AtomicBool;

use ssx_assets::setup::{self, DiscCheck, Progress};

use crate::settings::Settings;

pub fn run(settings: &Settings, iso: Option<&Path>) -> i32 {
    let out = settings.game_data();
    let Some(iso) = iso else {
        if already_set_up(&out, None) {
            ssx_core::println!("progress 1 Already set up");
            return 0;
        }
        ssx_core::eprintln!("error: --prepare needs --game <your SSX (2012) Xbox 360 disc image>");
        return 2;
    };
    ssx_core::println!("progress 0 Checking your disc image");
    let check = match setup::inspect(iso) {
        Ok(check) => check,
        Err(e) => {
            ssx_core::eprintln!("error: {e:#}");
            return 1;
        }
    };
    if already_set_up(&out, Some(&check)) {
        ssx_core::println!("progress 1 Already set up");
        return 0;
    }
    let mut last = (usize::MAX, -1.0f32);
    let result = setup::run(iso, &out, &AtomicBool::new(false), &mut |p: &Progress| {
        let fraction = if p.bytes_total == 0 { 0.0 } else { p.bytes_done as f32 / p.bytes_total as f32 };
        if p.archive_index != last.0 || fraction - last.1 >= 0.005 {
            last = (p.archive_index, fraction);
            let (n, of) = ((p.archive_index + 1).min(p.archive_count.max(1)), p.archive_count);
            ssx_core::println!("progress {:.3} Extracting the game data: {} ({n}/{of})", fraction.min(1.0), p.archive);
        }
    });
    match result {
        Ok(s) => {
            ssx_core::println!(
                "progress 1 Game data set up: {} files written, {} kept, {} movies copied",
                s.members_written,
                s.members_kept + s.movies_kept,
                s.movies_written
            );
            0
        }
        Err(e) => {
            ssx_core::eprintln!("error: setup failed: {e:#}");
            1
        }
    }
}

fn already_set_up(out: &Path, check: Option<&DiscCheck>) -> bool {
    let Some(marker) = setup::read_marker(out) else { return false };
    let same_disc = check.is_none_or(|check| {
        marker["fingerprint"].as_str() == Some(check.fingerprint.as_str())
            && marker["iso_size"].as_u64() == Some(check.image_size)
    });
    same_disc && ssx_assets::folder::state(out).is_complete()
}
