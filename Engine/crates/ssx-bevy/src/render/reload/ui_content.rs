use bevy::prelude::*;
use ssx_content::Kind;

use super::ContentReload;
use crate::shell::game_mode::GameUiRes;
use crate::ui::{UiScene, UiSceneState};

pub(crate) fn apply_fonts(scene: &mut UiSceneState, game: Option<&mut ssx_ui::game::GameUi>) -> usize {
    let region = game.as_ref().map_or("na", |g| g.loc.language.font_region());
    scene.lib.apply_package_fonts(region);
    let n = scene.player.relayout_texts(&scene.lib);
    if let Some(fe) = game.and_then(|g| g.fe.as_mut()) {
        let all = scene
            .lib
            .font_library()
            .values()
            .chain(scene.lib.disc_fallback())
            .flat_map(|f| f.codes.iter().filter_map(|&c| char::from_u32(u32::from(c))));
        fe.ava.glyphs = Some(all.collect());
    }
    n
}

pub(super) fn ui_content(
    reload: Res<ContentReload>,
    mut seen: Local<u64>,
    mut scene: Option<ResMut<UiScene>>,
    game: Option<ResMut<GameUiRes>>,
) {
    if std::mem::replace(&mut *seen, reload.generation) == reload.generation {
        return;
    }
    let Some(scene) = scene.as_deref_mut().and_then(UiScene::state) else { return };
    let changed = |k: Kind| reload.ids.as_ref().is_none_or(|ids| ids.iter().any(|id| id.kind() == k));
    if changed(Kind::Ui) {
        let n = scene.lib.forget_movies(|path| {
            ssx_content::ui::movie_id(path, false).is_some_and(|id| reload.touches(&id))
                || ssx_content::ui::movie_id(path, true).is_some_and(|id| reload.touches(&id))
        });
        info!("mod-dev: {n} UI movies to load again at their screens' next load");
    }
    let mut game = game;
    if changed(Kind::Font) {
        let n = apply_fonts(scene, game.as_deref_mut().map(|g| &mut g.0));
        info!("mod-dev: the packages' fonts converted again, {n} text fields laid out anew");
    }
    if changed(Kind::String)
        && let Some(mut game) = game
    {
        match game.reload_strings(scene.lib.files()) {
            Ok(n) => info!("mod-dev: the string table read again ({n} package strings)"),
            Err(e) => warn!("mod-dev: the string table not read again: {e:#}"),
        }
    }
}
