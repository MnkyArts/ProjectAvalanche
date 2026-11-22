use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Mutex};

use bevy::log::{info, warn};
use bevy::prelude::{Commands, Local, Res, ResMut};

use super::{ContentReload, ReloadState};
use crate::particles::ParticleWorld;
use crate::render::objects::{LoadedObjects, ObjectLods, ReloadedObjects};
use crate::render::snow_tracks::SnowTracks;
use crate::ui::UiScene;
use crate::ui::globe::{GlobeAssets, GlobeView};

pub(super) fn particles(
    reload: Res<ContentReload>,
    state: Res<ReloadState>,
    mut seen: Local<u64>,
    world: Option<ResMut<ParticleWorld>>,
) {
    if std::mem::replace(&mut *seen, reload.generation) == reload.generation {
        return;
    }
    let Some(mut w) = world else { return };
    let n = crate::particles::reload_textures(&mut w, &state.big_root, |id| reload.touches(id));
    if n > 0 {
        info!("mod-dev: {n} particle textures reloaded");
    }
}

pub(super) fn globe(
    reload: Res<ContentReload>,
    state: Res<ReloadState>,
    mut seen: Local<u64>,
    mut pending: Local<Option<Mutex<Receiver<anyhow::Result<GlobeAssets>>>>>,
    view: Option<ResMut<GlobeView>>,
) {
    let Some(mut view) = view else { return };
    if let Some(rx) = pending.as_ref() {
        let got = rx.lock().ok().and_then(|r| r.try_recv().ok());
        match got {
            Some(Ok(assets)) => {
                view.assets = Some(Arc::new(assets));
                *pending = None;
                info!("mod-dev: the globe's assets reloaded");
            }
            Some(Err(e)) => {
                warn!("mod-dev: the globe's assets not reloaded: {e:#}");
                *pending = None;
            }
            None => return,
        }
    }
    if std::mem::replace(&mut *seen, reload.generation) == reload.generation {
        return;
    }
    let mid = view.midground.as_ref().is_some_and(|m| {
        let materials = m.parts.iter().map(|p| &p.material).chain(m.waves.iter().map(|w| &w.material));
        materials
            .flat_map(|mat| mat.textures.values())
            .filter_map(|rel| ssx_assets::content::textures::texture_id(&rel.replace('\\', "/")))
            .any(|id| reload.touches(&id))
    });
    if mid {
        view.midground = None;
    }
    let Some(assets) = view.assets.clone() else { return };
    if !assets.texture_ids().iter().any(|id| reload.touches(id)) {
        return;
    }
    let (tx, rx) = channel();
    let root = state.big_root.clone();
    std::thread::spawn(move || {
        let _ = tx.send(GlobeAssets::load(&root, &assets.data));
    });
    *pending = Some(Mutex::new(rx));
}

pub(super) fn ui_images(reload: Res<ContentReload>, mut seen: Local<u64>, mut scene: Option<ResMut<UiScene>>) {
    if std::mem::replace(&mut *seen, reload.generation) == reload.generation {
        return;
    }
    let Some(scene) = scene.as_deref_mut().and_then(UiScene::state) else { return };
    let n = scene.lib.reload_images(|source| {
        let path = ssx_ui::fs::normalise(source);
        ssx_assets::content::textures::common_texture_id(&path).is_some_and(|id| reload.touches(&id))
    });
    if n > 0 {
        info!("mod-dev: {n} UI images reloaded");
    }
}

pub(super) fn snow_noise(
    reload: Res<ContentReload>,
    state: Res<ReloadState>,
    mut seen: Local<u64>,
    tracks: Option<ResMut<SnowTracks>>,
) {
    if std::mem::replace(&mut *seen, reload.generation) == reload.generation {
        return;
    }
    let Some(mut tracks) = tracks else { return };
    let Some(id) = ssx_assets::content::textures::texture_id("worlds/textures/snow_noise_displace") else { return };
    if !reload.touches(&id) {
        return;
    }
    match crate::render::terrain::load_snow_noise(&state.big_root) {
        Ok(noise) => {
            tracks.set_noise(noise);
            info!("mod-dev: the snow tracks' noise reloaded");
        }
        Err(e) => warn!("mod-dev: the snow tracks' noise not reloaded: {e:#}"),
    }
}

pub(super) fn objects(
    reload: Res<ContentReload>,
    mut seen: Local<u64>,
    mut pending: Local<Option<Mutex<Receiver<anyhow::Result<LoadedObjects>>>>>,
    lods: Option<Res<ObjectLods>>,
    mut commands: Commands,
) {
    if let Some(rx) = pending.as_ref() {
        let got = rx.lock().ok().and_then(|r| r.try_recv().ok());
        match got {
            Some(Ok(objects)) => {
                info!("mod-dev: the track's objects loaded again in {:.2} s", objects.stats.load_secs);
                commands.insert_resource(ReloadedObjects::new(objects));
                *pending = None;
            }
            Some(Err(e)) => {
                warn!("mod-dev: the track's objects not loaded again: {e:#}");
                *pending = None;
            }
            None => return,
        }
    }
    if std::mem::replace(&mut *seen, reload.generation) == reload.generation {
        return;
    }
    let Some(lods) = lods else { return };
    use ssx_content::Kind;
    let kinds = [Kind::Model, Kind::Material, Kind::Object];
    if !reload.ids.as_ref().is_none_or(|ids| ids.iter().any(|id| kinds.contains(&id.kind()))) {
        return;
    }
    let (root, source) = lods.source().clone();
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        let _ = tx.send(crate::render::objects::load_from(&root, &source));
    });
    *pending = Some(Mutex::new(rx));
}
