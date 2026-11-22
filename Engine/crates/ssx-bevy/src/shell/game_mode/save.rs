use std::path::{Path, PathBuf};
use std::sync::{Arc, PoisonError};
use std::time::Instant;

use anyhow::{Context, Result};
use bevy::prelude::{error, info, warn};
use ssx_assets::career::Profile;
use ssx_assets::career::save::{SaveExtras, SaveSchema};
use ssx_ui::game::GameUi;

use super::summary;

mod writer;

const MAX_REASONS_SHOWN: usize = 2048;

pub struct SaveSlot {
    path: PathBuf,
    pub(super) schema: SaveSchema,
    pub(super) extras: SaveExtras,
    header: Option<ssx_formats::save::Header>,
    pub(super) damaged: bool,
    pub(super) no_saving: bool,
    asides: Arc<writer::Asides>,
    // PORT(choice): the profile is written on a thread of its own, the save writer (the newest save asked for, each
    // after the one before: the file whole with `ssx_files::replace`, synced, renamed, the folder synced), so that the
    // frames go on while the disk takes its time — on the frame's thread a boot's save stopped them for up to 1.3 s
    // by itself (the syncs of the new folder and the file), 16–48 s with another program writing 3 GB, and 17.5 s
    // once ended a network session (btrfs, 2026-11-16); the reads of the file and the game's exit wait for it. The
    // console's save is a job of the save-load manager (0x82457FE8 → 0x8250B278 → 0x8244E038 → `FlowProgressSave`
    // 0x8316C588 → `FlowAutoSave` 0x83173E60) that the frame loop runs over frames through the save library, the
    // dock's "Saving" icon up meanwhile.
    writer: writer::Writer,
}

struct SaveRead {
    bytes: usize,
    header: Option<ssx_formats::save::Header>,
    profile: Result<(Profile, SaveExtras)>,
}

impl SaveSlot {
    fn read(path: &Path, schema: &SaveSchema) -> Result<Option<SaveRead>> {
        if !path.is_file() {
            return Ok(None);
        }
        let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
        let (header, profile) = match ssx_formats::save::split(&bytes) {
            Ok((header, buffer)) => {
                let profile = ssx_formats::save::ProfileFile::parse(buffer)
                    .map_err(anyhow::Error::from)
                    .and_then(|file| Profile::from_save(&file, schema));
                (header, profile)
            }
            Err(e) => (None, Err(anyhow::Error::from(e))),
        };
        Ok(Some(SaveRead { bytes: bytes.len(), header, profile }))
    }

    pub fn load(path: PathBuf, schema: SaveSchema) -> Result<(Self, Profile, String)> {
        let asides = Arc::new(writer::Asides::default());
        let writer =
            writer::start(Arc::clone(&asides)).context("the profile's save writer (a thread) could not start")?;
        let slot = |path, schema, extras, header, damaged| SaveSlot {
            path,
            schema,
            extras,
            header,
            damaged,
            no_saving: false,
            asides,
            writer,
        };
        match Self::read(&path, &schema)? {
            None => {
                let note = format!("save: {} does not exist: a new profile", path.display());
                Ok((slot(path, schema, SaveExtras::fresh(), None, false), Profile::new(), note))
            }
            Some(SaveRead { bytes: n, header, profile: Ok((mut profile, extras)) }) => {
                let mut note = format!("save: loaded {} ({n} bytes): {}", path.display(), summary(&profile));
                let s = slot(path, schema, extras, header, false);
                note.push_str(&s.read_mod_state(&mut profile));
                Ok((s, profile, note))
            }
            Some(SaveRead { bytes: n, header: _, profile: Err(e) }) => {
                // TODO(guess): a file with a right CRC whose chunks the port cannot read counts as corrupt too; settle: what the chunk loader 0x825580D0 does with another chunk count (0x8315F1C0 → 0x82506BD0).
                let note = format!(
                    "save: ERROR: {} ({n} bytes) is not a profile the game can read ({e:#}); a new profile starts \
                     and the sign-in asks what to do with the file",
                    path.display(),
                );
                Ok((slot(path, schema, SaveExtras::fresh(), None, true), Profile::new(), note))
            }
        }
    }

    pub(super) fn describe(&self) -> String {
        self.writer.settle();
        let state = if self.no_saving {
            "not written in this session (continue without saving)".to_string()
        } else if self.damaged {
            "damaged and not written (the dialog was not answered)".to_string()
        } else {
            match Self::read(&self.path, &self.schema) {
                Ok(Some(SaveRead { bytes: n, profile: Ok((p, _)), .. })) => {
                    format!("{n} bytes, reads back: {}", summary(&p))
                }
                Ok(Some(SaveRead { bytes: n, profile: Err(e), .. })) => {
                    format!("ERROR {n} bytes, not a profile: {e:#}")
                }
                Ok(None) => "not written yet".to_string(),
                Err(e) => format!("ERROR {e:#}"),
            }
        };
        format!("{}: {state}", self.path.display())
    }

    fn read_mod_state(&self, profile: &mut Profile) -> String {
        let state = ssx_content::profile_state::state_path(&self.path);
        let mut unread = self.asides.state_unread.lock().unwrap_or_else(PoisonError::into_inner);
        match ssx_content::profile_state::ModState::read_file(&state) {
            Ok(Some(st)) => {
                *unread = None;
                let note = format!("; mod state: {} entries", st.entries.len());
                profile.mod_state = st;
                note
            }
            Ok(None) => {
                *unread = None;
                String::new()
            }
            Err(e) => {
                let shown = ssx_core::text::shown_max(&e, MAX_REASONS_SHOWN).to_string();
                let note = format!("; ERROR: the mod state is not read ({shown}); it is kept aside at the next save");
                *unread = Some(shown);
                note
            }
        }
    }

    pub(super) fn retry(&mut self) -> Option<Profile> {
        self.writer.settle();
        match Self::read(&self.path, &self.schema) {
            Ok(Some(SaveRead { bytes: n, header, profile: Ok((mut profile, extras)) })) => {
                let state = self.read_mod_state(&mut profile);
                info!("save: retry: loaded {} ({n} bytes): {}{state}", self.path.display(), summary(&profile));
                self.extras = extras;
                self.header = header;
                self.damaged = false;
                Some(profile)
            }
            Ok(None) => {
                info!("save: retry: {} is gone: the new profile stays", self.path.display());
                self.damaged = false;
                None
            }
            Ok(Some(SaveRead { bytes: n, profile: Err(e), .. })) => {
                error!("save: retry: {} ({n} bytes) still is not a profile ({e:#})", self.path.display());
                None
            }
            Err(e) => {
                error!("save: retry: {e:#}");
                None
            }
        }
    }

    // TODO(guess): the game deletes the corrupt file; the port keeps it aside under another name (its reader is unchecked against a console save); settle: a console save read by `save_verify`.
    pub(super) fn replace_damaged(&mut self) {
        self.damaged = false;
        self.asides.damaged.store(true, std::sync::atomic::Ordering::Release);
        info!("save: {} is kept aside by the save writer before the new profile is written", self.path.display());
    }

    pub(super) fn profile_into(
        &self,
        game: &mut GameUi,
        profile: Profile,
    ) -> Option<ssx_assets::sfx::settings::AudioSettings> {
        let stored_audio = self.extras.audio_settings();
        if let Some(fe) = game.fe.as_mut() {
            fe.profile = profile;
            fe.profile.adopt_mod_state(&fe.career);
            fe.music.set_flags(&self.extras.song_flags());
            Self::music_switches_from(fe);
            fe.profile.campaign_globe.merge_start(&fe.campaign_start);
            fe.profile.own_campaign_riders(&fe.rules);
            ssx_ui::game::stats::register(&mut fe.profile);
            if let Some(a) = &stored_audio {
                fe.audio_store = ssx_ui::game::fe::settings::audio_values(a);
            }
        }
        self.front_end_into(&mut game.values);
        if let Some(a) = &stored_audio {
            for (id, v) in a.ui_values() {
                game.values.set_int(id as u16, v);
            }
        }
        stored_audio
    }

    fn music_switches_from(fe: &mut ssx_ui::game::fe::FrontEnd) {
        let keys = fe.music.package_keys();
        let listed: std::collections::BTreeSet<&ssx_content::ContentId> = keys.iter().collect();
        let (switches, reasons) = fe.profile.music_switches(&keys);
        fe.music.set_package_switches(&switches);
        for r in reasons {
            fe.note(format!("save: WARNING: a music switch of the mod state is not taken: {r}"));
        }
        let kept = fe
            .profile
            .mod_state
            .entries
            .iter()
            .filter(|(id, f)| id.kind() == ssx_content::Kind::Music && f.contains_key("on") && !listed.contains(id))
            .count();
        let off = switches.iter().filter(|(_, on)| !on).count();
        if !keys.is_empty() || kept > 0 {
            fe.note(format!(
                "save: music switches: {} package songs listed, {off} off; {kept} switches of songs not mounted kept",
                keys.len()
            ));
        }
    }

    pub(super) fn music_switches_into(&self, fe: &mut ssx_ui::game::fe::FrontEnd) {
        for key in fe.profile.set_music_switches(&fe.music.package_switches()) {
            warn!(
                "save ({}): the switch of {key} is not kept: the mod state holds {} entries already",
                self.path.display(),
                ssx_content::profile_state::MAX_ENTRIES
            );
        }
    }

    pub(super) fn front_end_from(&mut self, values: &ssx_ui::game::values::UiValues) {
        let c = &mut self.extras.front_end;
        c.controller_scheme = values.int(0x1F0) as u32;
        c.low_survive_never_show = u8::from(values.bool(0x9E));
        c.restart_never_show = u8::from(values.bool(0x9D));
    }

    fn front_end_into(&self, values: &mut ssx_ui::game::values::UiValues) {
        let c = &self.extras.front_end;
        values.set_int(0x1F0, c.controller_scheme as i32);
        values.set_bool(0x9E, c.low_survive_never_show != 0);
        values.set_bool(0x9D, c.restart_never_show != 0);
    }

    pub(super) fn write(&self, profile: &Profile, why: &str) {
        if self.no_saving {
            // TODO(guess): "Continue without saving" skips every save of the session; settle: the flag's readers in the save flows (0x83160D00's callers) and what clears it (0x83160D38).
            info!("save ({why}) skipped: the player continues without saving");
            return;
        }
        if self.damaged {
            warn!("save ({why}) skipped: {} is damaged and the dialog is not answered", self.path.display());
            return;
        }
        let asked = Instant::now();
        let file = match profile.to_save(&self.schema, &self.extras) {
            Ok(file) => file,
            Err(e) => {
                error!("save ({why}) failed: {} — {e:#}", self.path.display());
                return;
            }
        };
        let bytes = match &self.header {
            Some(header) => file.write_with_header(header),
            None => file.write(),
        };
        writer::ask(
            &self.writer,
            writer::Job {
                why: why.to_string(),
                path: self.path.clone(),
                bytes,
                state: profile.state_to_write(),
                summary: summary(profile),
                asked,
                made: asked.elapsed(),
            },
        );
    }

    pub(super) fn writing(&self) -> bool {
        self.writer.writing()
    }
}

impl Drop for SaveSlot {
    fn drop(&mut self) {
        if !self.writer.finish() {
            error!("save: the save writer stopped with a panic");
        }
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests;
