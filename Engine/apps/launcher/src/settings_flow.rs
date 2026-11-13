use std::path::PathBuf;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use eframe::egui;
use ssx_assets::settings_file::UpdateError;

use crate::app::Launcher;
use crate::command::Invocation;
use crate::settings::Settings;
use crate::ui::{theme, widgets};

// PORT(choice): 0.5 s (a name typed or a value stepped with the keys is saved once, not per key; a launcher closed
// sooner saves at its exit); the original has no launcher.
pub const SETTLE: Duration = Duration::from_millis(500);

#[derive(Clone, Copy, Debug, Default)]
pub struct Hands {
    pub pointer_down: bool,
    pub released: bool,
    pub focus_moved: bool,
}

enum Written {
    Saved(Settings),
    Replaced(PathBuf, Settings),
}

struct Saving {
    settings: Settings,
    thread: JoinHandle<Result<Written, UpdateError>>,
}

#[derive(Default)]
pub struct SettingsSave {
    pub refused: Option<Settings>,
    pub unreadable: Option<String>,
    pub note: Option<(bool, String)>,
    pub urgent: bool,
    pub replace: bool,
    path: Option<PathBuf>,
    saving: Option<Saving>,
    changed: Option<(Settings, Instant)>,
    focus: Option<egui::Id>,
}

impl SettingsSave {
    #[cfg(test)]
    fn at(path: PathBuf) -> Self {
        SettingsSave { path: Some(path), ..Default::default() }
    }

    fn path(&self) -> PathBuf {
        self.path.clone().unwrap_or_else(Settings::path)
    }

    pub fn unsaved(&self, settings: &Settings, saved: &Settings) -> bool {
        self.saving.is_some() || (settings != saved && self.refused.as_ref() != Some(settings))
    }

    pub fn hands(&mut self, ctx: &egui::Context) -> Hands {
        let (pointer_down, released) = ctx.input(|i| (i.pointer.any_down(), i.pointer.any_released()));
        let focus = ctx.memory(|m| m.focused());
        let focus_moved = std::mem::replace(&mut self.focus, focus) != focus;
        Hands { pointer_down, released, focus_moved }
    }

    pub fn step(
        &mut self,
        settings: &mut Settings,
        saved: &mut Settings,
        hands: Hands,
        now: Instant,
        wake: impl FnOnce() + Send + 'static,
    ) -> Option<Duration> {
        if self.saving.as_ref().is_some_and(|s| s.thread.is_finished())
            && let Some(done) = self.saving.take()
        {
            self.ended(done, settings, saved);
        }
        if self.saving.is_some() {
            return None;
        }
        let replace = std::mem::take(&mut self.replace);
        if !replace && !self.unsaved(settings, saved) {
            (self.changed, self.urgent) = (None, false);
            return None;
        }
        if self.changed.as_ref().is_none_or(|(s, _)| s != settings) {
            self.changed = Some((settings.clone(), now));
        }
        let since = self.changed.as_ref().map_or(Duration::ZERO, |(_, at)| now.saturating_duration_since(*at));
        let settled = !hands.pointer_down && (hands.released || hands.focus_moved || since >= SETTLE);
        if !(replace || self.urgent || settled) {
            return (!hands.pointer_down).then(|| SETTLE.saturating_sub(since));
        }
        let (writing, base, path) = (settings.clone(), saved.clone(), self.path());
        let spawned = std::thread::Builder::new().name("settings-save".into()).spawn(move || {
            let written = match replace {
                false => writing.save_to(&path, &base).map(Written::Saved),
                true => writing
                    .replace_unreadable_at(&path)
                    .map(|(aside, written)| Written::Replaced(aside, written))
                    .map_err(UpdateError::Failed),
            };
            wake();
            written
        });
        match spawned {
            Ok(thread) => self.saving = Some(Saving { settings: settings.clone(), thread }),
            Err(e) => {
                self.failed(settings.clone(), UpdateError::Failed(anyhow::anyhow!("no thread for the save: {e}")))
            }
        }
        (self.changed, self.urgent) = (None, false);
        None
    }

    pub fn flush(&mut self, settings: &mut Settings, saved: &mut Settings) {
        if let Some(done) = self.saving.take() {
            self.ended(done, settings, saved);
        }
        if !self.unsaved(settings, saved) {
            return;
        }
        match settings.save_to(&self.path(), saved) {
            Ok(written) => (*settings, *saved) = (written.clone(), written),
            Err(e) => ssx_core::eprintln!("settings: not saved at the exit: {e}"),
        }
        self.urgent = false;
    }

    fn ended(&mut self, done: Saving, settings: &mut Settings, saved: &mut Settings) {
        let outcome =
            done.thread.join().unwrap_or_else(|_| Err(UpdateError::Failed(anyhow::anyhow!("the save stopped"))));
        let written = match outcome {
            Ok(Written::Saved(written)) => written,
            Ok(Written::Replaced(aside, written)) => {
                let aside = aside.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                self.note = Some((true, format!("the unreadable file was kept as {aside}; saved")));
                written
            }
            Err(e) => return self.failed(done.settings, e),
        };
        *settings = settings.rebased(&written, &done.settings);
        *saved = written;
        self.refused = None;
    }

    fn failed(&mut self, settings: Settings, e: UpdateError) {
        let text = format!("{e}");
        ssx_core::eprintln!("settings: not saved: {text}");
        self.refused = Some(settings);
        match e {
            UpdateError::Unreadable(why) => self.unreadable = Some(why),
            UpdateError::Failed(_) => self.note = Some((false, format!("not saved: {text}"))),
        }
    }
}

impl Launcher {
    pub fn settings_frame(&mut self, ctx: &egui::Context) {
        let game_closed = self.started_game && self.viewer.as_ref().is_some_and(|v| v.status().is_some());
        if !self.displays.synced && game_closed && !self.settings_save.unsaved(&self.settings, &self.saved) {
            self.displays.synced = true;
            if let Err(e) = self.settings.reload(&mut self.saved) {
                ssx_core::eprintln!("settings: not read again after the game: {e}");
                self.settings_save.note = Some((false, format!("not read again after the game: {e}")));
            }
        }
        let hands = self.settings_save.hands(ctx);
        let wake = ctx.clone();
        let due = self
            .settings_save
            .step(&mut self.settings, &mut self.saved, hands, Instant::now(), move || wake.request_repaint());
        if let Some(after) = due {
            ctx.request_repaint_after(after);
        }
        if !self.settings_save.unsaved(&self.settings, &self.saved)
            && let Some((inv, game)) = self.start_after_save.take()
        {
            self.start(ctx, &inv, game);
        }
    }

    pub fn save_settings_now(&mut self) {
        self.settings_save.urgent = true;
    }

    pub fn start_after_saving(&mut self, ctx: &egui::Context, inv: &Invocation, game: bool) -> bool {
        if !self.settings_save.unsaved(&self.settings, &self.saved) {
            return false;
        }
        self.save_settings_now();
        self.start_after_save = Some((inv.clone(), game));
        ctx.request_repaint();
        true
    }

    pub fn settings_question(&mut self, ctx: &egui::Context) {
        let path = Settings::path();
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let text = self.settings_save.unreadable.as_ref().map(|why| {
            format!(
                "{}\n\nThe launcher's changes are not saved while it cannot be read. Keep it beside it as \
                 {name}.broken-<n> and write a new one with the launcher's settings? What the game kept in it (its \
                 Options, your name on servers, the window) starts over.",
                widgets::shown(why)
            )
        });
        let asked = text.as_deref().map(|text| widgets::Question {
            title: "The settings file cannot be read",
            text,
            yes: "Keep it aside and save",
            yes_enabled: true,
        });
        let Some(yes) = widgets::question(ctx, egui::Id::new("settings-unreadable"), asked) else { return };
        self.settings_save.unreadable = None;
        if !yes {
            self.settings_save.note = Some((false, format!("not saved: {name} cannot be read")));
            return;
        }
        self.settings_save.replace = true;
    }

    pub fn settings_notes(&self, ui: &mut egui::Ui) {
        if let Some((good, text)) = &self.settings_save.note {
            let colour = if *good { theme::GOOD } else { theme::ERROR };
            widgets::status(ui, colour, format!("Settings {}", widgets::shown(text)));
        }
        for note in &self.settings.notes {
            widgets::note(ui, &widgets::shown(note));
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use serde_json::Value as Json;
    use ssx_assets::settings_file;

    use super::*;

    fn temp(name: &str, text: &[u8]) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("launcher-flow-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");
        std::fs::write(&path, text).unwrap();
        path
    }

    fn file(path: &Path) -> Json {
        serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
    }

    fn until_saved(save: &mut SettingsSave, settings: &mut Settings, saved: &mut Settings, now: Instant) {
        let deadline = Instant::now() + Duration::from_secs(20);
        while save.saving.is_some() {
            assert!(Instant::now() < deadline, "the save never ended");
            std::thread::sleep(Duration::from_millis(5));
            save.step(settings, saved, Hands::default(), now, || {});
        }
    }

    #[test]
    fn a_change_is_saved_once_it_settles() {
        let path = temp("settle", br#"{"vsync": true, "sfx_volume": 1.0}"#);
        let mut saved = Settings::load_from(&path);
        let (mut settings, mut save) = (saved.clone(), SettingsSave::at(path.clone()));
        let t0 = Instant::now();
        let held = Hands { pointer_down: true, ..Hands::default() };
        for step in 1..=20 {
            settings.sfx_volume = 1.0 - step as f32 * 0.01;
            assert_eq!(save.step(&mut settings, &mut saved, held, t0, || {}), None, "a drag under way");
        }
        assert!(save.saving.is_none() && file(&path)["sfx_volume"] == 1.0, "nothing written while dragging");
        let ended = Hands { released: true, ..Hands::default() };
        save.step(&mut settings, &mut saved, ended, t0, || {});
        assert!(save.saving.is_some(), "the drag's end saves");
        until_saved(&mut save, &mut settings, &mut saved, t0);
        assert_eq!(file(&path)["sfx_volume"], Json::from(0.8f32));
        assert_eq!(saved, settings);
        settings.vsync = false;
        let wait = save.step(&mut settings, &mut saved, Hands::default(), t0, || {});
        assert_eq!(wait, Some(SETTLE), "the frame runs again when the change is due");
        let early = t0 + SETTLE / 2;
        assert_eq!(save.step(&mut settings, &mut saved, Hands::default(), early, || {}), Some(SETTLE / 2));
        assert!(save.saving.is_none());
        save.step(&mut settings, &mut saved, Hands::default(), t0 + SETTLE, || {});
        until_saved(&mut save, &mut settings, &mut saved, t0 + SETTLE);
        assert_eq!(file(&path)["vsync"], Json::Bool(false));
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn a_save_never_waits_on_the_ui_and_nothing_is_lost_at_the_exit() {
        let path = temp("busy", br#"{"vsync": true, "sfx_volume": 1.0}"#);
        let mut saved = Settings::load_from(&path);
        let (mut settings, mut save) = (saved.clone(), SettingsSave::at(path.clone()));
        let (held, game_path) = (std::sync::Arc::new(std::sync::Barrier::new(2)), path.clone());
        let started = held.clone();
        let game = std::thread::spawn(move || {
            settings_file::update_object(&game_path, |m| {
                started.wait();
                std::thread::sleep(Duration::from_millis(400));
                m.insert("player_name".into(), Json::from("Elise"));
                Ok(())
            })
            .unwrap();
        });
        held.wait();
        settings.vsync = false;
        save.urgent = true;
        let t = Instant::now();
        save.step(&mut settings, &mut saved, Hands::default(), t, || {});
        assert!(t.elapsed() < Duration::from_millis(200), "the step waited {:?}", t.elapsed());
        assert!(save.saving.is_some());
        settings.sfx_volume = 0.5;
        until_saved(&mut save, &mut settings, &mut saved, t);
        game.join().unwrap();
        assert_eq!((settings.vsync, settings.sfx_volume, settings.player_name.as_deref()), (false, 0.5, Some("Elise")));
        assert_eq!((saved.vsync, saved.sfx_volume), (false, 1.0), "the file as written: the later change not yet");
        let v = file(&path);
        assert_eq!((v["vsync"].clone(), v["player_name"].clone()), (Json::Bool(false), Json::from("Elise")));
        save.flush(&mut settings, &mut saved);
        assert_eq!(file(&path)["sfx_volume"], Json::from(0.5f32));
        assert!(!save.unsaved(&settings, &saved));
        settings.vsync = true;
        save.urgent = true;
        save.step(&mut settings, &mut saved, Hands::default(), Instant::now(), || {});
        save.flush(&mut settings, &mut saved);
        assert_eq!(file(&path)["vsync"], Json::Bool(true));
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn an_unreadable_file_is_replaced_only_after_the_yes() {
        let path = temp("unreadable", b"{\"vsync\": tru");
        let mut saved = Settings::load_from(&path);
        let (mut settings, mut save) = (saved.clone(), SettingsSave::at(path.clone()));
        settings.vsync = false;
        save.urgent = true;
        save.step(&mut settings, &mut saved, Hands::default(), Instant::now(), || {});
        until_saved(&mut save, &mut settings, &mut saved, Instant::now());
        assert!(save.unreadable.is_some() && save.refused.as_ref() == Some(&settings));
        assert_eq!(std::fs::read(&path).unwrap(), b"{\"vsync\": tru", "left as it was");
        assert!(!save.unsaved(&settings, &saved), "not tried again until the settings change");
        save.replace = true;
        save.step(&mut settings, &mut saved, Hands::default(), Instant::now(), || {});
        until_saved(&mut save, &mut settings, &mut saved, Instant::now());
        assert_eq!(file(&path)["vsync"], Json::Bool(false));
        assert!(save.note.as_ref().is_some_and(|(good, t)| *good && t.contains("settings.json.broken-1")));
        assert_eq!(std::fs::read(path.with_file_name("settings.json.broken-1")).unwrap(), b"{\"vsync\": tru");
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }
}
