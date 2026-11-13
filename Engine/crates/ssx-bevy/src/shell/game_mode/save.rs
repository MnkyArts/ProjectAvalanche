use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use bevy::prelude::{error, info, warn};
use ssx_assets::career::Profile;
use ssx_assets::career::save::{SaveExtras, SaveSchema};
use ssx_ui::game::GameUi;

use super::summary;

const MAX_REASONS_SHOWN: usize = 2048;

pub struct SaveSlot {
    path: PathBuf,
    pub(super) schema: SaveSchema,
    pub(super) extras: SaveExtras,
    header: Option<ssx_formats::save::Header>,
    pub(super) damaged: bool,
    pub(super) no_saving: bool,
    state_unread: std::sync::Mutex<Option<String>>,
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
        let slot = |path, schema, extras, header, damaged| SaveSlot {
            path,
            schema,
            extras,
            header,
            damaged,
            no_saving: false,
            state_unread: Default::default(),
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
        let mut unread = self.state_unread.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
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
        let names = (1..100).map(|n| self.path.with_extension(format!("damaged{n}")));
        match ssx_files::move_aside(&self.path, names) {
            Ok(Some(a)) => {
                info!("save: the damaged file is kept as {}", a.display());
                self.damaged = false;
                self.state_aside_with(&a);
            }
            Err(e) => error!("save: the damaged {} could not be moved aside: {}", self.path.display(), e.io()),
            Ok(None) => error!("save: a hundred damaged saves are kept already; {} stays", self.path.display()),
        }
    }

    fn state_aside_with(&self, aside: &Path) {
        let state = ssx_content::profile_state::state_path(&self.path);
        if !state.exists() {
            return;
        }
        let to = ssx_content::profile_state::state_path(aside);
        let moved = ssx_files::rename_new(&state, &to).map_err(|e| match e.kind() {
            std::io::ErrorKind::AlreadyExists => format!("{} exists", to.display()),
            _ => e.io().to_string(),
        });
        let mut unread = self.state_unread.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        match moved {
            Ok(()) => {
                info!("save: the damaged profile's mod state is kept as {}", to.display());
                *unread = None;
            }
            Err(e) => {
                error!("save: the damaged profile's mod state {} could not be moved aside: {e}", state.display());
                *unread = Some("the damaged profile's".to_string());
            }
        }
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

    fn write_mod_state(&self, profile: &Profile) -> Result<()> {
        let path = ssx_content::profile_state::state_path(&self.path);
        let mut unread = self.state_unread.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(why) = unread.as_ref()
            && path.exists()
        {
            let names = (1..100).map(|n| path.with_extension(format!("toml.unread{n}")));
            let aside = ssx_files::move_aside(&path, names)?
                .with_context(|| format!("{}: a hundred unread states are kept already", path.display()))?;
            warn!("save: the mod state {} ({why}) is kept as {}", path.display(), aside.display());
        }
        *unread = None;
        drop(unread);
        let state = profile.state_to_write();
        if state.entries.is_empty() && !path.exists() {
            return Ok(());
        }
        state.write_file(&path).map_err(anyhow::Error::msg)
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
        let r = (|| -> Result<usize> {
            let file = profile.to_save(&self.schema, &self.extras)?;
            let bytes = match &self.header {
                Some(header) => file.write_with_header(header),
                None => file.write(),
            };
            if let Some(dir) = self.path.parent().filter(|d| !d.as_os_str().is_empty()) {
                ssx_files::create_dir_all(dir)?;
            }
            ssx_files::replace(&self.path, &bytes, &ssx_files::Options::new())?;
            self.write_mod_state(profile)?;
            Ok(bytes.len())
        })();
        match r {
            Ok(n) => info!("save ({why}): {} ({n} bytes): {}", self.path.display(), summary(profile)),
            Err(e) => error!("save ({why}) failed: {} — {e:#}", self.path.display()),
        }
    }
}
