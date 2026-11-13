use std::sync::Arc;
use std::sync::Mutex;
use std::sync::mpsc::{Receiver, channel};

use bevy::ecs::system::SystemParam;
use bevy::log::info;
use bevy::prelude::{Local, Res, ResMut};
use ssx_assets::content::audio::SoundDecision;
use ssx_content::Kind;

use super::{ContentReload, ReloadState};
use crate::audio::music::frontend::{FrontEndAudio, FrontEndReload};
use crate::audio::music::{GameAudio, Music, MusicReload};
use crate::audio::sfx_input::SpeechLanguage;

pub(super) struct AudioLoaded {
    level: Arc<SoundDecision>,
    front_end: Arc<SoundDecision>,
    music: Option<MusicReload>,
    front_end_music: Option<FrontEndReload>,
    seconds: f64,
}

fn touches(reload: &ContentReload) -> bool {
    reload
        .ids
        .as_ref()
        .is_none_or(|ids| ids.iter().any(|id| matches!(id.kind(), Kind::Sound | Kind::Music | Kind::Character)))
}

#[derive(SystemParam)]
pub(super) struct Engines<'w> {
    music: Option<Res<'w, Music>>,
    fe: Option<ResMut<'w, FrontEndAudio>>,
    level: Option<Res<'w, crate::audio::sfx::Sfx>>,
    game: Option<Res<'w, GameAudio>>,
    ui: Option<ResMut<'w, crate::shell::game_mode::GameUiRes>>,
    lang: Option<Res<'w, SpeechLanguage>>,
}

pub(super) fn follow(
    reload: Res<ContentReload>,
    state: Res<ReloadState>,
    mut seen: Local<Option<u64>>,
    mut pending: Local<Option<Mutex<Receiver<AudioLoaded>>>>,
    engines: Engines,
) {
    let Engines { music, mut fe, level, game, mut ui, lang } = engines;
    if let Some(rx) = pending.as_ref() {
        let Some(got) = rx.lock().ok().and_then(|r| r.try_recv().ok()) else { return };
        *pending = None;
        let started = std::time::Instant::now();
        if let Some(g) = &game {
            let _turn = crate::audio::music::GameTurn::take();
            if let Ok(mut m) = g.out.lock()
                && let Some(e) = m.sfx.as_mut()
            {
                let d = if level.is_some() { &got.level } else { &got.front_end };
                for id in e.set_overrides(&d.sounds) {
                    info!("mod-dev: {id}: no interface of the engine defines it");
                }
            }
        }
        if let Some(fe) = &fe {
            fe.set_unplugged_sounds(&got.front_end.sounds);
        }
        if let (Some(m), Some(r)) = (&music, got.music) {
            m.apply_reload(r);
        }
        if let (Some(fe), Some(mut r)) = (fe.as_mut(), got.front_end_music) {
            fe.apply_reload(&mut r);
            if let (Some(songs), Some(menu)) = (r.menu_songs.take(), ui.as_mut().and_then(|u| u.fe.as_mut())) {
                menu.music.replace_songs(songs);
            }
        }
        info!(
            "mod-dev: sounds and music re-applied ({} effects, {} speech events; decided in {:.2} s, put in {:.3} s)",
            got.level.sounds.effects.len(),
            got.level.sounds.speech.len(),
            got.seconds,
            started.elapsed().as_secs_f64()
        );
        return;
    }
    let Some(prev) = *seen else {
        *seen = Some(reload.generation);
        return;
    };
    if reload.generation == prev {
        return;
    }
    *seen = Some(reload.generation);
    if !touches(&reload) {
        return;
    }
    let root = state.big_root.clone();
    let inputs = music.as_ref().map(|m| m.reload_inputs());
    let rider = level.as_ref().map(|l| l.character().to_string());
    let front_end = fe.is_some() && reload.ids.as_ref().is_none_or(|ids| ids.iter().any(|id| id.kind() == Kind::Music));
    let lang = lang.map_or_else(|| SpeechLanguage::of(None), |l| *l);
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        let start = std::time::Instant::now();
        let content = ssx_assets::content::GameContent::of(&root);
        let level = content.sound_overrides_for(Some(lang.code()), rider.as_deref());
        let front_end_sounds = content.sound_overrides(None);
        let music = inputs.map(|(scope, dd)| Music::prepare_reload(&root, &scope, dd));
        let front_end_music = front_end.then(|| FrontEndAudio::prepare_reload(&root));
        let seconds = start.elapsed().as_secs_f64();
        let _ = tx.send(AudioLoaded { level, front_end: front_end_sounds, music, front_end_music, seconds });
    });
    *pending = Some(Mutex::new(rx));
}
